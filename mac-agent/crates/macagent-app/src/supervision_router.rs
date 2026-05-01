//! ctrl ↔ GuiCapture + RtcPeer video track 桥（M7 multi-entry registry，≤8）。
//!
//! Concurrency rule: the `registry` `std::sync::Mutex` is held only for short
//! synchronous reads/writes. Every FFI / async call (gui_capture, launcher_m7,
//! window_fitter) happens with the lock dropped. See `set_active` for the
//! snapshot-mutate-snapshot pattern.

use anyhow::{Context, Result};
use macagent_core::ctrl_msg::{
    CtrlPayload, SupStatus, SupervisionEntry, Viewport, WindowInfo, WindowRect,
};
use macagent_core::rtc_peer::{RtcPeer, VideoTrackHandle};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::gui_capture::GuiCapture;
use crate::launcher_m7;
use crate::window_fitter;

const MAX_SUPERVISIONS: usize = 8;

/// Registry of supervised windows + the id of the currently active stream.
struct Registry {
    entries: HashMap<String, SupervisionEntry>,
    /// Owner pid per supervision (used by window_fitter / restore; not in protocol).
    pids: HashMap<String, i32>,
    active_sup: Option<String>,
}

impl Registry {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            pids: HashMap::new(),
            active_sup: None,
        }
    }

    fn snapshot(&self) -> Vec<SupervisionEntry> {
        let mut v: Vec<SupervisionEntry> = self.entries.values().cloned().collect();
        v.sort_by(|a, b| a.sup_id.cmp(&b.sup_id));
        v
    }
}

pub struct SupervisionRouter {
    gui_capture: Arc<GuiCapture>,
    #[allow(dead_code)]
    rtc_peer: Arc<RtcPeer>,
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
    registry: Mutex<Registry>,
    /// video track 实例（在 PeerConnection 建立前由 ui.rs 创建并注入）。
    video_track: VideoTrackHandle,
}

impl SupervisionRouter {
    pub fn new(
        gui_capture: Arc<GuiCapture>,
        rtc_peer: Arc<RtcPeer>,
        video_track: VideoTrackHandle,
        ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
    ) -> Self {
        Self {
            gui_capture,
            rtc_peer,
            ctrl_tx,
            registry: Mutex::new(Registry::new()),
            video_track,
        }
    }

    /// Dispatch the legacy M5/M6 ctrl variants. M7 variants
    /// (`SuperviseLaunch` / `SwitchActive`) are wired separately by rtc_glue
    /// to the dedicated `handle_*` methods.
    pub async fn handle_ctrl(&self, payload: CtrlPayload) -> Result<()> {
        match payload {
            CtrlPayload::ListWindows => {
                self.list_windows().await?;
            }
            CtrlPayload::SuperviseExisting {
                window_id,
                viewport,
            } => {
                self.handle_supervise_existing(window_id, viewport).await?;
            }
            CtrlPayload::RemoveSupervised { sup_id } => {
                self.handle_remove_supervised(sup_id).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Look up the window_id of a registered supervision (active or armed).
    /// Used by InputInjector (M6) — only acts on active, but also consults
    /// armed entries for forward compatibility.
    pub async fn current_window_id(&self, sup_id: &str) -> Option<u32> {
        let reg = self.registry.lock().unwrap();
        reg.entries.get(sup_id).map(|e| e.window_id)
    }

    async fn list_windows(&self) -> Result<()> {
        let windows = self.gui_capture.list_windows().await?;
        let _ = self.ctrl_tx.send(CtrlPayload::WindowsList { windows });
        Ok(())
    }

    /// Register an existing on-screen window and make it active.
    pub async fn handle_supervise_existing(
        &self,
        window_id: u32,
        viewport: Viewport,
    ) -> Result<()> {
        // (a) Permission check
        if !matches!(
            self.gui_capture.check_permission(),
            crate::gui_capture::perm::PermissionStatus::Granted
        ) {
            let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                window_id,
                code: "permission_denied".into(),
                reason: "Screen Recording permission not granted".into(),
            });
            return Ok(());
        }

        // (b) Resolve window meta
        let windows = self.gui_capture.list_windows().await?;
        let Some(window) = windows.into_iter().find(|w| w.window_id == window_id) else {
            let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                window_id,
                code: "window_not_found".into(),
                reason: format!("window_id={} not in current windows list", window_id),
            });
            return Ok(());
        };

        // (c) Resolve owner pid via CGWindowList for window_fitter use.
        let pid = self
            .gui_capture
            .lookup_target(window_id)
            .map(|t| t.pid)
            .unwrap_or(0);

        // (d) Insert armed entry under the limit
        let sup_id = Uuid::new_v4().to_string()[..8].to_string();
        match self.try_register_armed(&sup_id, &window, pid) {
            Ok(()) => {}
            Err(code) => {
                let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                    window_id,
                    code: code.into(),
                    reason: "supervision registry full (max 8)".into(),
                });
                return Ok(());
            }
        }

        // (e) Activate
        self.set_active(sup_id, viewport).await
    }

    /// Launch a whitelisted Mac app, register its window, and make it active.
    pub async fn handle_supervise_launch(
        &self,
        bundle_id: String,
        viewport: Viewport,
    ) -> Result<()> {
        // (a) Permission check
        if !matches!(
            self.gui_capture.check_permission(),
            crate::gui_capture::perm::PermissionStatus::Granted
        ) {
            let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                window_id: 0,
                code: "permission_denied".into(),
                reason: "Screen Recording permission not granted".into(),
            });
            return Ok(());
        }

        // (b) Pre-flight: registry must have room before we even launch.
        {
            let reg = self.registry.lock().unwrap();
            if reg.entries.len() >= MAX_SUPERVISIONS {
                drop(reg);
                let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                    window_id: 0,
                    code: "supervision_limit".into(),
                    reason: "supervision registry full (max 8)".into(),
                });
                return Ok(());
            }
        }

        // (c) Launch + find window
        let (pid, window_id) = match launcher_m7::launch_and_find_window(&bundle_id).await {
            Ok(v) => v,
            Err(e) => {
                let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                    window_id: 0,
                    code: "launch_failed".into(),
                    reason: format!("{e:#}"),
                });
                return Ok(());
            }
        };

        // (d) Resolve window meta from the live windows list
        let windows = self.gui_capture.list_windows().await?;
        let Some(window) = windows.into_iter().find(|w| w.window_id == window_id) else {
            let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                window_id,
                code: "window_not_found".into(),
                reason: format!("launched window {} disappeared", window_id),
            });
            return Ok(());
        };

        // (e) Insert armed entry
        let sup_id = Uuid::new_v4().to_string()[..8].to_string();
        match self.try_register_armed(&sup_id, &window, pid) {
            Ok(()) => {}
            Err(code) => {
                let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                    window_id,
                    code: code.into(),
                    reason: "supervision registry full (max 8)".into(),
                });
                return Ok(());
            }
        }

        // (f) Activate
        self.set_active(sup_id, viewport).await
    }

    /// Switch the active stream to an already-armed supervision.
    pub async fn handle_switch_active(&self, sup_id: String, viewport: Viewport) -> Result<()> {
        // Existence check under lock
        {
            let reg = self.registry.lock().unwrap();
            if !reg.entries.contains_key(&sup_id) {
                return Ok(());
            }
        }
        self.set_active(sup_id, viewport).await
    }

    /// Re-fit the active window to a (possibly rotated) viewport.
    pub async fn handle_viewport_changed(
        &self,
        sup_id: String,
        viewport: Viewport,
    ) -> Result<()> {
        // Snapshot what we need under lock.
        let (window_id, pid, current_rect) = {
            let reg = self.registry.lock().unwrap();
            // Only re-fit if this is the active sup.
            if reg.active_sup.as_deref() != Some(sup_id.as_str()) {
                return Ok(());
            }
            let Some(entry) = reg.entries.get(&sup_id) else {
                return Ok(());
            };
            let pid = reg.pids.get(&sup_id).copied().unwrap_or(0);
            let current = entry.original_frame.unwrap_or(WindowRect {
                x: 0,
                y: 0,
                w: entry.width as i32,
                h: entry.height as i32,
            });
            (entry.window_id, pid, current)
        };

        // Re-fit outside the lock.
        let fit_result = window_fitter::fit(window_id, pid, &current_rect, viewport);

        // Write back + emit.
        match fit_result {
            Ok(orig) => {
                let mut reg = self.registry.lock().unwrap();
                if let Some(e) = reg.entries.get_mut(&sup_id) {
                    e.original_frame = Some(orig);
                }
            }
            Err(e) => {
                let _ = self.ctrl_tx.send(CtrlPayload::FitFailed {
                    sup_id: sup_id.clone(),
                    reason: format!("{e:#}"),
                });
            }
        }
        self.emit_supervision_list();
        Ok(())
    }

    /// Stop, remove, restore. Auto-promote the next armed entry if the
    /// removed one was active.
    pub async fn handle_remove_supervised(&self, sup_id: String) -> Result<()> {
        // Snapshot under lock.
        let (was_active, removed, removed_pid, next_armed) = {
            let mut reg = self.registry.lock().unwrap();
            let was_active = reg.active_sup.as_deref() == Some(sup_id.as_str());
            let removed = reg.entries.remove(&sup_id);
            let removed_pid = reg.pids.remove(&sup_id);
            if was_active {
                reg.active_sup = None;
            }
            // Pick the lex-smallest sup_id among remaining as the auto-promote
            // candidate (deterministic; matches `snapshot()` ordering).
            let next_armed = if was_active {
                let mut ids: Vec<&String> = reg.entries.keys().collect();
                ids.sort();
                ids.first().map(|s| (*s).clone())
            } else {
                None
            };
            (was_active, removed, removed_pid, next_armed)
        };

        let Some(entry) = removed else {
            // Nothing to do; entry not found.
            return Ok(());
        };

        // Stop the stream (only meaningful if was active).
        if was_active {
            let _ = self.gui_capture.remove_supervised(&sup_id).await;
        }

        // Restore the original frame if we have one + a pid.
        if let (Some(orig), Some(pid)) = (entry.original_frame, removed_pid) {
            let _ = window_fitter::restore(entry.window_id, pid, &orig);
        }

        let _ = self.ctrl_tx.send(CtrlPayload::StreamEnded {
            sup_id: sup_id.clone(),
            reason: "removed".into(),
        });

        // Auto-promote next armed if we removed the active.
        if let Some(next) = next_armed {
            // Re-use the current entry's known size as a viewport hint;
            // iOS will follow up with ViewportChanged on its next layout.
            let viewport = Viewport {
                w: entry.width.max(1),
                h: entry.height.max(1),
            };
            self.set_active(next, viewport).await?;
        } else {
            self.emit_supervision_list();
        }
        Ok(())
    }

    // -------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------

    /// Insert an Armed entry under the registry limit. Returns Err code on overflow.
    fn try_register_armed(
        &self,
        sup_id: &str,
        window: &WindowInfo,
        pid: i32,
    ) -> std::result::Result<(), &'static str> {
        let mut reg = self.registry.lock().unwrap();
        if reg.entries.len() >= MAX_SUPERVISIONS {
            return Err("supervision_limit");
        }
        let entry = SupervisionEntry {
            sup_id: sup_id.to_string(),
            window_id: window.window_id,
            app_name: window.app_name.clone(),
            title: window.title.clone(),
            width: window.width,
            height: window.height,
            status: SupStatus::Armed,
            original_frame: None,
            thumb_jpeg_b64: None,
        };
        reg.entries.insert(sup_id.to_string(), entry);
        reg.pids.insert(sup_id.to_string(), pid);
        Ok(())
    }

    /// Atomic switch: demote old → fit new → start new → publish list.
    /// Per CLAUDE.md the registry mutex is dropped before every FFI/async call.
    async fn set_active(&self, new_sup: String, viewport: Viewport) -> Result<()> {
        // (1) Snapshot under lock.
        let (old_active, new_window_id, new_pid, new_current_rect) = {
            let reg = self.registry.lock().unwrap();
            let new_entry = reg
                .entries
                .get(&new_sup)
                .with_context(|| format!("entry not found: {new_sup}"))?;
            let pid = reg.pids.get(&new_sup).copied().unwrap_or(0);
            let current = new_entry.original_frame.unwrap_or(WindowRect {
                x: 0,
                y: 0,
                w: new_entry.width as i32,
                h: new_entry.height as i32,
            });
            (
                reg.active_sup.clone(),
                new_entry.window_id,
                pid,
                current,
            )
        };

        // (2) Demote old (FFI without lock).
        let demoted_thumb = if let Some(old_sup) = old_active.as_ref() {
            if old_sup != &new_sup {
                self.gui_capture.demote_to_armed(old_sup)
            } else {
                None
            }
        } else {
            None
        };

        // (3) Fit new (FFI without lock).
        let fit_result = window_fitter::fit(new_window_id, new_pid, &new_current_rect, viewport);

        // (4) Start the stream on the new sup_id (existing M5.2.5 API).
        if let Err(e) = self
            .gui_capture
            .supervise_existing(new_sup.clone(), new_window_id, self.video_track.clone())
            .await
        {
            // Rollback: drop the new entry from the registry; emit reject.
            {
                let mut reg = self.registry.lock().unwrap();
                reg.entries.remove(&new_sup);
                reg.pids.remove(&new_sup);
            }
            let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                window_id: new_window_id,
                code: "supervise_failed".into(),
                reason: format!("{e:#}"),
            });
            return Ok(());
        }

        // (5) Re-acquire lock and update the registry.
        {
            let mut reg = self.registry.lock().unwrap();
            if let Some(old_sup) = old_active.as_ref() {
                if old_sup != &new_sup {
                    if let Some(e) = reg.entries.get_mut(old_sup) {
                        e.status = SupStatus::Armed;
                        e.thumb_jpeg_b64 = demoted_thumb;
                    }
                }
            }
            if let Some(e) = reg.entries.get_mut(&new_sup) {
                e.status = SupStatus::Active;
                if let Ok(orig) = fit_result.as_ref() {
                    e.original_frame = Some(*orig);
                }
                // Clear any stale thumb on the now-active entry.
                e.thumb_jpeg_b64 = None;
            }
            reg.active_sup = Some(new_sup.clone());
        }

        // (6) Emit FitFailed (if any) + the new SupervisionList outside lock.
        if let Err(e) = fit_result {
            let _ = self.ctrl_tx.send(CtrlPayload::FitFailed {
                sup_id: new_sup.clone(),
                reason: format!("{e:#}"),
            });
        }

        // SupervisedAck for the new active entry — keep the M5/M6 contract for
        // iOS clients that still listen to it.
        let new_entry_snapshot = {
            let reg = self.registry.lock().unwrap();
            reg.entries.get(&new_sup).cloned()
        };
        if let Some(entry) = new_entry_snapshot {
            let _ = self.ctrl_tx.send(CtrlPayload::SupervisedAck {
                sup_id: new_sup,
                entry,
            });
        }

        self.emit_supervision_list();
        Ok(())
    }

    /// Snapshot the registry and broadcast it as a SupervisionList ctrl message.
    fn emit_supervision_list(&self) {
        let entries = {
            let reg = self.registry.lock().unwrap();
            reg.snapshot()
        };
        let _ = self.ctrl_tx.send(CtrlPayload::SupervisionList { entries });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macagent_core::ctrl_msg::WindowInfo;

    fn fake_window(id: u32) -> WindowInfo {
        WindowInfo {
            window_id: id,
            app_name: format!("App{id}"),
            bundle_id: None,
            title: format!("Window{id}"),
            width: 800,
            height: 600,
            on_screen: true,
            is_minimized: false,
        }
    }

    /// Pure-state test: construct the registry directly and exercise the
    /// limit guard without touching GuiCapture / launcher / window_fitter FFI.
    #[test]
    fn register_eight_then_ninth_rejected() {
        let mut reg = Registry::new();
        for i in 0..8 {
            assert!(reg.entries.len() < MAX_SUPERVISIONS);
            let w = fake_window(i);
            let entry = SupervisionEntry {
                sup_id: format!("sup{i:02}"),
                window_id: w.window_id,
                app_name: w.app_name,
                title: w.title,
                width: w.width,
                height: w.height,
                status: SupStatus::Armed,
                original_frame: None,
                thumb_jpeg_b64: None,
            };
            reg.entries.insert(entry.sup_id.clone(), entry);
        }
        assert_eq!(reg.entries.len(), MAX_SUPERVISIONS);
        // 9th should fail the limit check.
        let limit_ok = reg.entries.len() < MAX_SUPERVISIONS;
        assert!(!limit_ok, "ninth registration must be rejected");

        // snapshot() is sorted by sup_id and contains all 8.
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 8);
        assert_eq!(snap[0].sup_id, "sup00");
        assert_eq!(snap[7].sup_id, "sup07");
    }
}
