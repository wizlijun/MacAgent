//! ctrl ↔ GuiCapture + RtcPeer video track 桥。

use anyhow::Result;
use macagent_core::ctrl_msg::{CtrlPayload, SupStatus, SupervisionEntry};
use macagent_core::rtc_peer::{RtcPeer, VideoTrackHandle};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::gui_capture::GuiCapture;

pub struct SupervisionRouter {
    gui_capture: Arc<GuiCapture>,
    #[allow(dead_code)]
    rtc_peer: Arc<RtcPeer>,
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
    /// 当前活跃的 supervision (M5 同时只有一个)
    active: Mutex<Option<ActiveSupervision>>,
    /// video track 实例（在 PeerConnection 建立前由 ui.rs 创建并注入）
    video_track: VideoTrackHandle,
}

#[allow(dead_code)]
struct ActiveSupervision {
    sup_id: String,
    /// Used by InputInjector (M6) to re-resolve live frame each event.
    window_id: u32,
    app_name: String,
    title: String,
    started_ts: u64,
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
            active: Mutex::new(None),
            video_track,
        }
    }

    pub async fn handle_ctrl(&self, payload: CtrlPayload) -> Result<()> {
        match payload {
            CtrlPayload::ListWindows => {
                self.list_windows().await?;
            }
            CtrlPayload::SuperviseExisting {
                window_id,
                viewport: _,
            } => {
                self.supervise_existing(window_id).await?;
            }
            CtrlPayload::RemoveSupervised { sup_id } => {
                self.remove_supervised(sup_id).await?;
            }
            CtrlPayload::ViewportChanged { .. } => {
                // M5 不响应（M7 加 fit_window）；仅 acknowledge by ignoring
            }
            _ => {}
        }
        Ok(())
    }

    async fn list_windows(&self) -> Result<()> {
        let windows = self.gui_capture.list_windows().await?;
        let _ = self.ctrl_tx.send(CtrlPayload::WindowsList { windows });
        Ok(())
    }

    async fn supervise_existing(&self, window_id: u32) -> Result<()> {
        // 0. 检查权限
        match self.gui_capture.check_permission() {
            crate::gui_capture::perm::PermissionStatus::Granted => {}
            _ => {
                let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                    window_id,
                    code: "permission_denied".into(),
                    reason: "Screen Recording permission not granted".into(),
                });
                return Ok(());
            }
        }

        // 1. 找窗口元信息（用于 SupervisionEntry）
        let windows = self.gui_capture.list_windows().await?;
        let window = match windows.iter().find(|w| w.window_id == window_id) {
            Some(w) => w.clone(),
            None => {
                let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                    window_id,
                    code: "window_not_found".into(),
                    reason: format!("window_id={} not in current windows list", window_id),
                });
                return Ok(());
            }
        };

        // 2. 如有先 stop 旧的（M5 同时 1 个）
        let mut active = self.active.lock().await;
        if let Some(prev) = active.take() {
            let _ = self.gui_capture.remove_supervised(&prev.sup_id).await;
            let _ = self.ctrl_tx.send(CtrlPayload::StreamEnded {
                sup_id: prev.sup_id,
                reason: "replaced".into(),
            });
        }

        // 3. 复用启动时创建的 video track
        let track = self.video_track.clone();

        // 4. 启动 GuiCapture
        let sup_id = Uuid::new_v4().to_string()[..8].to_string();
        let started_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        match self
            .gui_capture
            .supervise_existing(sup_id.clone(), window_id, track)
            .await
        {
            Ok(()) => {
                let entry = SupervisionEntry {
                    sup_id: sup_id.clone(),
                    window_id,
                    app_name: window.app_name.clone(),
                    title: window.title.clone(),
                    width: window.width,
                    height: window.height,
                    status: SupStatus::Active,
                    original_frame: None,
                    thumb_jpeg_b64: None,
                };
                *active = Some(ActiveSupervision {
                    sup_id: sup_id.clone(),
                    window_id,
                    app_name: window.app_name,
                    title: window.title,
                    started_ts,
                });
                let _ = self
                    .ctrl_tx
                    .send(CtrlPayload::SupervisedAck { sup_id, entry });
            }
            Err(e) => {
                let _ = self.ctrl_tx.send(CtrlPayload::SuperviseReject {
                    window_id,
                    code: "supervise_failed".into(),
                    reason: e.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Look up the window_id of the active supervision matching `sup_id`.
    /// Returns None if no supervision is active or the id doesn't match.
    /// Used by InputInjector (M6.4) — wired up in M6.5.
    #[allow(dead_code)]
    pub async fn current_window_id(&self, sup_id: &str) -> Option<u32> {
        let active = self.active.lock().await;
        active
            .as_ref()
            .and_then(|a| (a.sup_id == sup_id).then_some(a.window_id))
    }

    async fn remove_supervised(&self, sup_id: String) -> Result<()> {
        let mut active = self.active.lock().await;
        if let Some(a) = active.as_ref() {
            if a.sup_id == sup_id {
                let _ = self.gui_capture.remove_supervised(&sup_id).await;
                *active = None;
                let _ = self.ctrl_tx.send(CtrlPayload::StreamEnded {
                    sup_id,
                    reason: "removed".into(),
                });
            }
        }
        Ok(())
    }
}
