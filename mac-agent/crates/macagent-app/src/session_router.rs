//! ctrl DataChannel ↔ Unix socket bidirectional bridge.
//!
//! - ctrl: LaunchSession → launcher.launch_in_terminal; pending until producer connects
//! - ctrl: AttachSession/DetachSession/KillSession/Input/Resize → forwarded to producer via registry
//! - socket: P2A::TermSnapshot/Delta/History → forwarded to ctrl as CtrlPayload
//! - socket: P2A::ProducerExit → SessionExited + SessionRemoved + unregister
//! - socket disconnect → SessionRemoved { reason: "window_closed" }

use crate::agent_socket::ProducerEvent;
use crate::launcher::{launch_in_terminal, LauncherConfig};
use crate::producer_registry::ProducerRegistry;
use anyhow::Result;
use macagent_core::ctrl_msg::{CtrlPayload, SessionInfo, SessionSource};
use macagent_core::socket_proto::{A2P, P2A};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant};

// ── Public event type ────────────────────────────────────────────────────────

/// Events from the socket layer forwarded to SessionRouter.
#[allow(dead_code, clippy::enum_variant_names)]
pub enum SocketEvent {
    /// A producer connected and was registered; comes with its frame channel.
    ProducerConnected {
        sid: String,
        info: SessionInfo,
        frames_rx: mpsc::UnboundedReceiver<P2A>,
    },
    /// A frame arrived from a producer.
    ProducerFrame { sid: String, frame: P2A },
    /// Producer disconnected (normal or error).
    ProducerDisconnected { sid: String },
}

// ── Internal pending launch ──────────────────────────────────────────────────

struct PendingLaunch {
    req_id: String,
    deadline: Instant,
}

// ── SessionRouter ────────────────────────────────────────────────────────────

pub struct SessionRouter {
    registry: Arc<ProducerRegistry>,
    /// launcher_id → list of pending launches waiting for a matching producer
    pending: Arc<Mutex<HashMap<String, Vec<PendingLaunch>>>>,
    /// channel to push CtrlPayload out to iOS via rtc_glue
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
    launcher_config: Arc<LauncherConfig>,
}

impl SessionRouter {
    pub fn new(
        registry: Arc<ProducerRegistry>,
        ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
        launcher_config: Arc<LauncherConfig>,
    ) -> Self {
        Self {
            registry,
            pending: Arc::new(Mutex::new(HashMap::new())),
            ctrl_tx,
            launcher_config,
        }
    }

    // ── ctrl (iOS → Mac) ─────────────────────────────────────────────────────

    /// Called by rtc_glue when a verified CtrlPayload arrives from iOS.
    pub async fn handle_ctrl_from_ios(&self, payload: CtrlPayload) -> Result<()> {
        match payload {
            CtrlPayload::LaunchSession {
                req_id,
                launcher_id,
                cwd_override,
            } => {
                self.handle_launch_session(req_id, launcher_id, cwd_override)
                    .await?;
            }
            CtrlPayload::AttachSession { sid } => {
                let _ = self.registry.send_to(&sid, A2P::AttachStart).await;
                self.registry.set_streaming(&sid, true).await;
            }
            CtrlPayload::DetachSession { sid } => {
                let _ = self.registry.send_to(&sid, A2P::AttachStop).await;
                self.registry.set_streaming(&sid, false).await;
            }
            CtrlPayload::KillSession { sid } => {
                let _ = self
                    .registry
                    .send_to(
                        &sid,
                        A2P::KillRequest {
                            reason: "ios_kill".into(),
                        },
                    )
                    .await;
            }
            CtrlPayload::Input { sid, payload } => {
                let _ = self.registry.send_to(&sid, A2P::Input { payload }).await;
            }
            CtrlPayload::Resize { sid, cols, rows } => {
                let _ = self
                    .registry
                    .send_to(&sid, A2P::Resize { cols, rows })
                    .await;
            }
            _ => {
                // Other variants (Heartbeat, Ping, etc.) are handled by rtc_glue.
            }
        }
        Ok(())
    }

    async fn handle_launch_session(
        &self,
        req_id: String,
        launcher_id: String,
        cwd_override: Option<String>,
    ) -> Result<()> {
        // Find launcher config
        let launcher = self
            .launcher_config
            .launchers
            .iter()
            .find(|l| l.id == launcher_id)
            .cloned();

        let launcher = match launcher {
            Some(l) => l,
            None => {
                self.send_ctrl(CtrlPayload::LaunchReject {
                    req_id,
                    code: "unknown_launcher".into(),
                    reason: format!("launcher '{}' not found", launcher_id),
                });
                return Ok(());
            }
        };

        // Record pending launch
        {
            let mut pending = self.pending.lock().await;
            pending
                .entry(launcher_id.clone())
                .or_default()
                .push(PendingLaunch {
                    req_id: req_id.clone(),
                    deadline: Instant::now() + Duration::from_secs(30),
                });
        }

        // Start timeout task
        {
            let pending = Arc::clone(&self.pending);
            let ctrl_tx = self.ctrl_tx.clone();
            let lid = launcher_id.clone();
            let rid = req_id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let mut guard = pending.lock().await;
                if let Some(vec) = guard.get_mut(&lid) {
                    if let Some(pos) = vec.iter().position(|p| p.req_id == rid) {
                        vec.remove(pos);
                        // Notify iOS of timeout
                        let _ = ctrl_tx.send(CtrlPayload::LaunchReject {
                            req_id: rid,
                            code: "timeout".into(),
                            reason: "producer did not connect within 30s".into(),
                        });
                    }
                }
            });
        }

        // Launch Terminal.app
        if let Err(e) = launch_in_terminal(&launcher, cwd_override.as_deref()).await {
            eprintln!("[router] launch_in_terminal error: {e}");
            // Remove pending entry
            let mut pending = self.pending.lock().await;
            if let Some(vec) = pending.get_mut(&launcher_id) {
                vec.retain(|p| p.req_id != req_id);
            }
            self.send_ctrl(CtrlPayload::LaunchReject {
                req_id,
                code: "applescript_error".into(),
                reason: e.to_string(),
            });
        }

        Ok(())
    }

    // ── socket (producer → Mac) ───────────────────────────────────────────────

    /// Called by the socket event loop for each producer event.
    pub async fn handle_socket_event(&self, event: SocketEvent) -> Result<()> {
        match event {
            SocketEvent::ProducerConnected {
                sid,
                info,
                frames_rx,
            } => {
                self.on_producer_connected(sid, info, frames_rx).await;
            }
            SocketEvent::ProducerFrame { sid, frame } => {
                self.on_producer_frame(sid, frame).await;
            }
            SocketEvent::ProducerDisconnected { sid } => {
                self.on_producer_disconnected(sid, None).await;
            }
        }
        Ok(())
    }

    async fn on_producer_connected(
        &self,
        sid: String,
        info: SessionInfo,
        mut frames_rx: mpsc::UnboundedReceiver<P2A>,
    ) {
        // Match pending launch by launcher_id
        if let SessionSource::IosLaunched { launcher_id } = &info.source {
            let mut pending = self.pending.lock().await;
            if let Some(vec) = pending.get_mut(launcher_id) {
                // Take the oldest non-expired pending launch
                if let Some(pos) = vec.iter().position(|p| p.deadline > Instant::now()) {
                    let pl = vec.remove(pos);
                    self.send_ctrl(CtrlPayload::LaunchAck {
                        req_id: pl.req_id,
                        sid: sid.clone(),
                    });
                }
            }
        }

        // Notify iOS of new session
        self.send_ctrl(CtrlPayload::SessionAdded { session: info });

        // Spawn frame forwarding task
        let sid_clone = sid.clone();
        let ctrl_tx = self.ctrl_tx.clone();
        let registry = Arc::clone(&self.registry);
        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            while let Some(frame) = frames_rx.recv().await {
                match frame {
                    P2A::ProducerExit {
                        exit_status,
                        reason,
                    } => {
                        registry.unregister(&sid_clone).await;
                        let _ = ctrl_tx.send(CtrlPayload::SessionExited {
                            sid: sid_clone.clone(),
                            exit_status,
                            reason,
                        });
                        let _ = ctrl_tx.send(CtrlPayload::SessionRemoved {
                            sid: sid_clone.clone(),
                            reason: "exited".into(),
                        });
                        return;
                    }
                    P2A::TermSnapshot {
                        revision,
                        cols,
                        rows,
                        cursor_row,
                        cursor_col,
                        cursor_visible,
                        title,
                        lines,
                    } => {
                        let _ = ctrl_tx.send(CtrlPayload::TermSnapshot {
                            sid: sid_clone.clone(),
                            revision,
                            cols,
                            rows,
                            cursor_row,
                            cursor_col,
                            cursor_visible,
                            title,
                            lines,
                        });
                    }
                    P2A::TermDelta {
                        revision,
                        cols,
                        rows,
                        cursor_row,
                        cursor_col,
                        cursor_visible,
                        title,
                        lines,
                    } => {
                        let _ = ctrl_tx.send(CtrlPayload::TermDelta {
                            sid: sid_clone.clone(),
                            revision,
                            cols,
                            rows,
                            cursor_row,
                            cursor_col,
                            cursor_visible,
                            title,
                            lines,
                        });
                    }
                    P2A::TermHistorySnapshot { revision, lines } => {
                        let _ = ctrl_tx.send(CtrlPayload::TermHistorySnapshot {
                            sid: sid_clone.clone(),
                            revision,
                            lines,
                        });
                    }
                    P2A::TermHistoryAppend { revision, lines } => {
                        let _ = ctrl_tx.send(CtrlPayload::TermHistoryAppend {
                            sid: sid_clone.clone(),
                            revision,
                            lines,
                        });
                    }
                    P2A::ProducerHello { .. } => {
                        // Should not arrive after initial handshake; ignore.
                    }
                }
            }
            // Channel closed without ProducerExit → treat as disconnect
            registry.unregister(&sid_clone).await;
            // Drop pending reference to suppress unused variable warning
            drop(pending);
            let _ = ctrl_tx.send(CtrlPayload::SessionRemoved {
                sid: sid_clone,
                reason: "window_closed".into(),
            });
        });
    }

    async fn on_producer_frame(&self, sid: String, frame: P2A) {
        // Frames are forwarded via the per-producer task spawned in on_producer_connected.
        // This method exists to satisfy SocketEvent::ProducerFrame if used directly;
        // in practice agent_socket sends frames through ProducerEvent::Connected's frames_rx.
        eprintln!(
            "[router] unexpected direct frame for sid={}: {:?}",
            sid, frame
        );
    }

    async fn on_producer_disconnected(&self, sid: String, exit_status: Option<i32>) {
        self.registry.unregister(&sid).await;
        let _ = exit_status; // not used in disconnect path
        self.send_ctrl(CtrlPayload::SessionRemoved {
            sid,
            reason: "window_closed".into(),
        });
    }

    fn send_ctrl(&self, payload: CtrlPayload) {
        let _ = self.ctrl_tx.send(payload);
    }
}

/// Consume raw `ProducerEvent`s from AgentSocket and dispatch to SessionRouter.
pub async fn run_socket_event_loop(
    mut events_rx: mpsc::UnboundedReceiver<ProducerEvent>,
    router: Arc<SessionRouter>,
) {
    while let Some(event) = events_rx.recv().await {
        match event {
            ProducerEvent::Connected {
                sid,
                argv,
                pid,
                cols,
                rows,
                source,
                frames_rx,
                send_tx: _,
            } => {
                let info = SessionInfo {
                    sid: sid.clone(),
                    label: argv.first().cloned().unwrap_or_else(|| "unknown".into()),
                    argv,
                    pid,
                    cols,
                    rows,
                    started_ts: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    streaming: false,
                    source,
                };
                let socket_event = SocketEvent::ProducerConnected {
                    sid,
                    info,
                    frames_rx,
                };
                if let Err(e) = router.handle_socket_event(socket_event).await {
                    eprintln!("[router] socket event error: {e}");
                }
            }
            ProducerEvent::Disconnected { sid } => {
                let socket_event = SocketEvent::ProducerDisconnected { sid };
                if let Err(e) = router.handle_socket_event(socket_event).await {
                    eprintln!("[router] socket event error: {e}");
                }
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use macagent_core::ctrl_msg::SessionSource;
    use macagent_core::socket_proto::A2P;
    use tokio::sync::mpsc;

    fn make_registry() -> Arc<ProducerRegistry> {
        Arc::new(ProducerRegistry::new())
    }

    fn make_launcher_config() -> Arc<LauncherConfig> {
        Arc::new(LauncherConfig::default_config())
    }

    #[tokio::test]
    async fn handle_attach_calls_send_to_registry() {
        let registry = make_registry();
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<CtrlPayload>();
        let router = SessionRouter::new(Arc::clone(&registry), ctrl_tx, make_launcher_config());

        // Register a fake producer
        let (a2p_tx, mut a2p_rx) = mpsc::unbounded_channel::<A2P>();
        let sid = registry
            .register(
                vec!["zsh".into()],
                100,
                80,
                24,
                SessionSource::UserManual,
                a2p_tx,
            )
            .await
            .unwrap();

        // Send AttachSession
        router
            .handle_ctrl_from_ios(CtrlPayload::AttachSession { sid: sid.clone() })
            .await
            .unwrap();

        // Producer should receive AttachStart
        let msg = a2p_rx.recv().await.unwrap();
        assert!(matches!(msg, A2P::AttachStart));

        // Registry streaming flag should be true
        let info = registry.get(&sid).await.unwrap();
        assert!(info.streaming);

        // No ctrl messages should have been emitted
        assert!(ctrl_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pending_launches_match_on_producer_connect() {
        let registry = make_registry();
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<CtrlPayload>();
        let router = Arc::new(SessionRouter::new(
            Arc::clone(&registry),
            ctrl_tx,
            make_launcher_config(),
        ));

        // Manually inject a pending launch (bypassing AppleScript)
        {
            let mut pending = router.pending.lock().await;
            pending
                .entry("zsh".to_string())
                .or_default()
                .push(PendingLaunch {
                    req_id: "req-001".to_string(),
                    deadline: Instant::now() + Duration::from_secs(30),
                });
        }

        // Register a fake producer with IosLaunched source
        let (a2p_tx, _a2p_rx) = mpsc::unbounded_channel::<A2P>();
        let sid = registry
            .register(
                vec!["zsh".into(), "-l".into()],
                200,
                80,
                24,
                SessionSource::IosLaunched {
                    launcher_id: "zsh".into(),
                },
                a2p_tx,
            )
            .await
            .unwrap();

        let info = registry.get(&sid).await.unwrap();
        let (frames_tx, frames_rx) = mpsc::unbounded_channel::<P2A>();
        // frames_tx unused in this test; drop it to close channel
        drop(frames_tx);

        let socket_event = SocketEvent::ProducerConnected {
            sid: sid.clone(),
            info,
            frames_rx,
        };
        router.handle_socket_event(socket_event).await.unwrap();

        // Expect LaunchAck
        let msg = ctrl_rx.recv().await.unwrap();
        assert!(
            matches!(msg, CtrlPayload::LaunchAck { ref req_id, .. } if req_id == "req-001"),
            "expected LaunchAck, got {msg:?}"
        );

        // Expect SessionAdded
        let msg2 = ctrl_rx.recv().await.unwrap();
        assert!(matches!(msg2, CtrlPayload::SessionAdded { .. }));
    }

    #[tokio::test]
    async fn launch_session_rejects_unknown_launcher() {
        let registry = make_registry();
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<CtrlPayload>();
        let router = SessionRouter::new(Arc::clone(&registry), ctrl_tx, make_launcher_config());

        router
            .handle_ctrl_from_ios(CtrlPayload::LaunchSession {
                req_id: "req-x".into(),
                launcher_id: "does-not-exist".into(),
                cwd_override: None,
            })
            .await
            .unwrap();

        let msg = ctrl_rx.recv().await.unwrap();
        assert!(
            matches!(msg, CtrlPayload::LaunchReject { ref code, .. } if code == "unknown_launcher"),
            "expected LaunchReject, got {msg:?}"
        );
    }
}
