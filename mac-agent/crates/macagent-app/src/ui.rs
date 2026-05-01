//! egui UI state machine for macagent.
//!
//! States: NotPaired → Pairing (QR shown) → Paired
//! Transitions are driven by results arriving from the reqwest task via mpsc.

use std::sync::{mpsc, Arc};
use std::time::Duration;

use anyhow::Result;
use egui::ColorImage;
use macagent_core::pair_auth::{PairAuth, PairRecord, PairToken, X25519Pub};
use tokio::sync::mpsc as async_mpsc;

use crate::agent_socket::AgentSocket;
use crate::clipboard_bridge::ClipboardBridge;
use crate::gui_capture::{GuiCapture, VideoConfig};
use crate::launcher::LauncherConfig;
use crate::notify_engine::NotifyEngine;
use crate::producer_registry::ProducerRegistry;
use crate::push_client::PushClient;
use crate::rtc_glue::{run_glue, GlueConfig, GlueState};
use crate::session_router::{run_socket_event_loop, SessionRouter};
use crate::supervision_router::SupervisionRouter;
use crate::{keychain, pair_qr};

// ── Keychain keys ───────────────────────────────────────────────────────────

const KC_LOCAL_SECRET: &str = "local_secret_key";
const KC_PAIR_ID: &str = "pair_id";
const KC_PEER_PUBKEY: &str = "peer_pubkey_b64";
const KC_MAC_DEVICE_SECRET: &str = "mac_device_secret_b64";
const KC_WORKER_URL: &str = "worker_url";

// ── Pair state ──────────────────────────────────────────────────────────────

pub enum PairState {
    NotPaired,
    /// Waiting for QR texture to be loaded, or showing it once loaded.
    Pairing {
        qr_texture: Option<egui::TextureHandle>,
    },
    Paired {
        record: PairRecord,
    },
}

// ── Background task result ──────────────────────────────────────────────────

pub enum UiEvent {
    Created {
        _token: PairToken,
        png: Vec<u8>,
    },
    Paired {
        pair_id: String,
        peer_pubkey_b64: String,
    },
    Error(String),
}

// ── App ─────────────────────────────────────────────────────────────────────

pub struct MacAgentApp {
    pub worker_url: String,
    pub local_keys: Arc<PairAuth>,
    pub state: PairState,
    pub last_error: Option<String>,
    /// PNG bytes from a just-completed /pair/create, pending texture upload.
    pub pending_png: Option<Vec<u8>>,
    pub runtime: tokio::runtime::Handle,
    pub rx: mpsc::Receiver<UiEvent>,
    pub tx: mpsc::SyncSender<UiEvent>,
    /// room_id pending texture upload (carried alongside pending_png).
    pub pending_room_id: Option<String>,
    /// mac_device_secret from /pair/create, held until pairing completes.
    pub pending_mac_device_secret: Option<String>,
    // ── RTC glue ──────────────────────────────────────────────────────────
    pub glue_state: Option<GlueState>,
    pub glue_state_tx: async_mpsc::UnboundedSender<GlueState>,
    glue_state_rx: async_mpsc::UnboundedReceiver<GlueState>,
    pub glue_msg_tx: async_mpsc::UnboundedSender<String>,
    glue_msg_rx: async_mpsc::UnboundedReceiver<String>,
    // ── Session router channels ────────────────────────────────────────────
    /// Outbound CtrlPayload from SessionRouter → rtc_glue → iOS.
    /// Held to keep the sender alive so the Arc<Mutex<Receiver>> stays open.
    #[allow(dead_code)]
    ctrl_send_tx: async_mpsc::UnboundedSender<macagent_core::ctrl_msg::CtrlPayload>,
    ctrl_send_rx: std::sync::Arc<
        tokio::sync::Mutex<async_mpsc::UnboundedReceiver<macagent_core::ctrl_msg::CtrlPayload>>,
    >,
    /// Inbound CtrlPayload from iOS → SessionRouter.
    ctrl_recv_tx: async_mpsc::UnboundedSender<macagent_core::ctrl_msg::CtrlPayload>,
    /// Held to keep the receiver alive (task drains it).
    #[allow(dead_code)]
    ctrl_recv_rx: std::sync::Arc<
        tokio::sync::Mutex<async_mpsc::UnboundedReceiver<macagent_core::ctrl_msg::CtrlPayload>>,
    >,
    /// Shared router (kept alive as long as the app runs).
    _router: std::sync::Arc<SessionRouter>,
    /// Swappable notify engine wrapper; shared with SessionRouter and AgentSocket.
    notify_engine_cell: std::sync::Arc<std::sync::RwLock<std::sync::Arc<NotifyEngine>>>,
}

impl MacAgentApp {
    /// Build from environment + Keychain.
    pub fn new(runtime: tokio::runtime::Handle) -> Result<Self> {
        let worker_url = std::env::var("MACAGENT_WORKER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());

        // Load or generate local X25519 keypair.
        let local_keys = Arc::new(match keychain::load(KC_LOCAL_SECRET)? {
            Some(bytes) if bytes.len() == 32 => {
                let arr: [u8; 32] = bytes.try_into().unwrap();
                PairAuth::from_secret_bytes(arr)
            }
            _ => {
                let keys = PairAuth::generate();
                keychain::save(KC_LOCAL_SECRET, &keys.secret_bytes())?;
                keys
            }
        });

        // Check for existing pair record in Keychain.
        let state = match Self::load_pair_record()? {
            Some(record) => PairState::Paired { record },
            None => PairState::NotPaired,
        };

        let (tx, rx) = mpsc::sync_channel(4);
        let (glue_state_tx, glue_state_rx) = async_mpsc::unbounded_channel();
        let (glue_msg_tx, glue_msg_rx) = async_mpsc::unbounded_channel();

        // ── Session router setup ──────────────────────────────────────────────
        let (ctrl_send_tx, ctrl_send_rx) = async_mpsc::unbounded_channel();
        let (ctrl_recv_tx, ctrl_recv_rx) = async_mpsc::unbounded_channel();

        let registry = std::sync::Arc::new(ProducerRegistry::new());
        let launcher_config = std::sync::Arc::new(
            runtime
                .block_on(crate::launcher::load_or_init())
                .unwrap_or_else(|_| LauncherConfig::default_config()),
        );

        // ClipboardBridge: polls NSPasteboard every 500ms and forwards changes to iOS.
        let clipboard_bridge = std::sync::Arc::new(ClipboardBridge::new(ctrl_send_tx.clone()));
        {
            let bridge = std::sync::Arc::clone(&clipboard_bridge);
            runtime.spawn(async move {
                bridge.run_polling().await;
            });
        }

        // Build initial NotifyEngine. If already paired, wire up PushClient now.
        // The cell allows rebuild_notify_engine() to swap in a new engine after
        // first-time pairing without restarting the menu bar.
        let initial_engine = Self::build_notify_engine(
            if let PairState::Paired { ref record } = state {
                Some(record)
            } else {
                None
            },
            ctrl_send_tx.clone(),
        );
        let notify_engine_cell = std::sync::Arc::new(std::sync::RwLock::new(initial_engine));

        let router = std::sync::Arc::new(SessionRouter::new(
            std::sync::Arc::clone(&registry),
            ctrl_send_tx.clone(),
            std::sync::Arc::clone(&launcher_config),
            clipboard_bridge,
            std::sync::Arc::clone(&notify_engine_cell),
        ));

        // Start AgentSocket and wire socket events → router
        let router_for_socket = std::sync::Arc::clone(&router);
        let ne_cell_for_socket = std::sync::Arc::clone(&notify_engine_cell);
        runtime.spawn(async move {
            match AgentSocket::start(registry, ne_cell_for_socket).await {
                Ok(socket) => {
                    run_socket_event_loop(socket.events_rx, router_for_socket).await;
                }
                Err(e) => eprintln!("[ui] AgentSocket start error: {e}"),
            }
        });

        // Start ctrl_recv dispatch: ctrl_recv_rx → router.handle_ctrl_from_ios
        let router_for_ctrl = std::sync::Arc::clone(&router);
        let ctrl_send_rx_arc = std::sync::Arc::new(tokio::sync::Mutex::new(ctrl_send_rx));
        let ctrl_recv_rx_arc = std::sync::Arc::new(tokio::sync::Mutex::new(ctrl_recv_rx));
        let ctrl_recv_rx_for_task = std::sync::Arc::clone(&ctrl_recv_rx_arc);
        runtime.spawn(async move {
            loop {
                let payload = {
                    let mut guard = ctrl_recv_rx_for_task.lock().await;
                    guard.recv().await
                };
                match payload {
                    Some(p) => {
                        if let Err(e) = router_for_ctrl.handle_ctrl_from_ios(p).await {
                            eprintln!("[ui] ctrl_from_ios error: {e}");
                        }
                    }
                    None => break,
                }
            }
        });

        Ok(Self {
            worker_url,
            local_keys,
            state,
            last_error: None,
            pending_png: None,
            pending_room_id: None,
            pending_mac_device_secret: None,
            runtime,
            rx,
            tx,
            glue_state: None,
            glue_state_tx,
            glue_state_rx,
            glue_msg_tx,
            glue_msg_rx,
            ctrl_send_tx,
            ctrl_send_rx: ctrl_send_rx_arc,
            ctrl_recv_tx,
            ctrl_recv_rx: ctrl_recv_rx_arc,
            _router: router,
            notify_engine_cell,
        })
    }

    /// Construct a NotifyEngine, optionally wiring in a PushClient from a pair record.
    fn build_notify_engine(
        record: Option<&PairRecord>,
        ctrl_tx: async_mpsc::UnboundedSender<macagent_core::ctrl_msg::CtrlPayload>,
    ) -> std::sync::Arc<NotifyEngine> {
        let push_client = record.and_then(|r| {
            match PushClient::new(
                r.worker_url.clone(),
                r.pair_id.clone(),
                &r.mac_device_secret_b64,
            ) {
                Ok(pc) => Some(std::sync::Arc::new(pc)),
                Err(e) => {
                    eprintln!("[ui] PushClient init failed (push disabled): {e}");
                    None
                }
            }
        });
        NotifyEngine::new(push_client, ctrl_tx)
    }

    /// Rebuild NotifyEngine + PushClient for a paired record and swap it into the cell.
    /// Called both at startup (when Keychain already has a record) and after first-time pairing.
    fn rebuild_notify_engine(&self, record: &PairRecord) {
        let engine = Self::build_notify_engine(Some(record), self.ctrl_send_tx.clone());
        *self.notify_engine_cell.write().unwrap() = engine;
        eprintln!("[ui] rebuilt NotifyEngine for pair_id={}", record.pair_id);
    }

    fn load_pair_record() -> Result<Option<PairRecord>> {
        let pair_id = match keychain::load(KC_PAIR_ID)? {
            Some(v) => String::from_utf8(v)?,
            None => return Ok(None),
        };
        let peer_pubkey_b64 = match keychain::load(KC_PEER_PUBKEY)? {
            Some(v) => String::from_utf8(v)?,
            None => return Ok(None),
        };
        let mac_device_secret_b64 = match keychain::load(KC_MAC_DEVICE_SECRET)? {
            Some(v) => String::from_utf8(v)?,
            None => return Ok(None),
        };
        let worker_url = match keychain::load(KC_WORKER_URL)? {
            Some(v) => String::from_utf8(v)?,
            None => return Ok(None),
        };
        Ok(Some(PairRecord {
            pair_id,
            peer_pubkey_b64,
            mac_device_secret_b64,
            worker_url,
        }))
    }

    pub fn save_pair_record(record: &PairRecord) -> Result<()> {
        keychain::save(KC_PAIR_ID, record.pair_id.as_bytes())?;
        keychain::save(KC_PEER_PUBKEY, record.peer_pubkey_b64.as_bytes())?;
        keychain::save(
            KC_MAC_DEVICE_SECRET,
            record.mac_device_secret_b64.as_bytes(),
        )?;
        keychain::save(KC_WORKER_URL, record.worker_url.as_bytes())?;
        Ok(())
    }

    fn revoke_pair_record() -> Result<()> {
        keychain::delete(KC_PAIR_ID)?;
        keychain::delete(KC_PEER_PUBKEY)?;
        keychain::delete(KC_MAC_DEVICE_SECRET)?;
        keychain::delete(KC_WORKER_URL)?;
        Ok(())
    }

    fn spawn_worker_revoke(&self, record: &PairRecord) {
        let worker_url = record.worker_url.clone();
        let pair_id = record.pair_id.clone();
        let mac_device_secret_b64 = record.mac_device_secret_b64.clone();
        self.runtime.spawn(async move {
            if let Err(e) = worker_revoke(&worker_url, &pair_id, &mac_device_secret_b64).await {
                eprintln!("worker_revoke failed (best-effort): {e}");
            }
        });
    }

    /// Spawn a reqwest task to call POST /pair/create.
    fn start_pairing(&self) {
        let worker_url = self.worker_url.clone();
        let pubkey_b64 = self.local_keys.public_key_b64();
        let tx = self.tx.clone();

        self.runtime.spawn(async move {
            match pair_create_request(&worker_url, &pubkey_b64).await {
                Ok((token, png)) => {
                    let _ = tx.send(UiEvent::Created { _token: token, png });
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::Error(e.to_string()));
                }
            }
        });
    }

    /// Spawn a polling task to GET /pair/event/:room_id until iOS claims.
    fn start_polling(&self, room_id: String) {
        let worker_url = self.worker_url.clone();
        let tx = self.tx.clone();

        self.runtime.spawn(async move {
            poll_room_event(worker_url, room_id, tx).await;
        });
    }

    /// Poll the channel and apply any incoming result.
    fn poll_rx(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                UiEvent::Created { _token, png } => {
                    self.last_error = None;
                    self.pending_room_id = Some(_token.room_id.clone());
                    self.pending_mac_device_secret = Some(_token.mac_device_secret.clone());
                    // Store PNG; texture is uploaded on the next frame when ctx is available.
                    self.pending_png = Some(png);
                }
                UiEvent::Paired {
                    pair_id,
                    peer_pubkey_b64,
                } => {
                    let mac_device_secret_b64 =
                        self.pending_mac_device_secret.take().unwrap_or_default();
                    match self.complete_pairing(pair_id, peer_pubkey_b64, mac_device_secret_b64) {
                        Ok(record) => {
                            self.last_error = None;
                            // Rebuild NotifyEngine so push works immediately without restart.
                            self.rebuild_notify_engine(&record);
                            self.state = PairState::Paired { record };
                        }
                        Err(e) => {
                            self.last_error = Some(e.to_string());
                            self.state = PairState::NotPaired;
                        }
                    }
                }
                UiEvent::Error(e) => {
                    self.last_error = Some(e);
                    self.state = PairState::NotPaired;
                }
            }
        }
    }

    /// Write Keychain entries and return PairRecord.
    ///
    /// `mac_device_secret_b64` is the base64-encoded 32B secret issued by the Worker during
    /// /pair/create.  It is intentionally kept separate from the ECDH shared_secret, which is
    /// derived on-demand from local_keys + peer_pubkey and never persisted.
    fn complete_pairing(
        &self,
        pair_id: String,
        peer_pubkey_b64: String,
        mac_device_secret_b64: String,
    ) -> Result<PairRecord> {
        // Validate peer pubkey parses (we'll need it for ECDH at runtime).
        let _peer_pub = X25519Pub::from_b64(&peer_pubkey_b64)?;
        let record = PairRecord {
            pair_id,
            peer_pubkey_b64,
            mac_device_secret_b64,
            worker_url: self.worker_url.clone(),
        };
        Self::save_pair_record(&record)?;
        Ok(record)
    }

    /// Load a PNG byte buffer into an egui texture.
    fn load_texture(ctx: &egui::Context, png: &[u8]) -> Option<egui::TextureHandle> {
        let img = image::load_from_memory(png).ok()?.into_rgba8();
        let (w, h) = img.dimensions();
        let pixels: Vec<egui::Color32> = img
            .pixels()
            .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
            .collect();
        let color_image = ColorImage {
            size: [w as usize, h as usize],
            pixels,
        };
        Some(ctx.load_texture("pair_qr", color_image, egui::TextureOptions::LINEAR))
    }

    /// Render the current state. Returns Some(next_state) if a transition is needed.
    fn render_state(
        state: &PairState,
        last_error: &Option<String>,
        glue_state: Option<GlueState>,
        ui: &mut egui::Ui,
    ) -> Option<StateTransition> {
        match state {
            PairState::NotPaired => {
                ui.label("No paired device.");
                if let Some(e) = last_error {
                    ui.colored_label(egui::Color32::RED, format!("Error: {}", e));
                }
                if ui.button("Pair new device").clicked() {
                    return Some(StateTransition::StartPairing);
                }
            }
            PairState::Pairing { qr_texture } => {
                ui.label("Scan this QR code with the iPhone app:");
                if let Some(tex) = qr_texture {
                    let size = egui::vec2(256.0, 256.0);
                    ui.image((tex.id(), size));
                } else {
                    ui.spinner();
                }
                if ui.button("Cancel").clicked() {
                    return Some(StateTransition::CancelPairing);
                }
            }
            PairState::Paired { record } => {
                let id_len = record.pair_id.len().min(8);
                let short_id = &record.pair_id[..id_len];
                ui.label(format!("Paired — device id: {}…", short_id));
                ui.label(format!("Worker: {}", record.worker_url));
                if ui.button("Connect (M2)").clicked() {
                    return Some(StateTransition::Connect);
                }
                if let Some(gs) = glue_state {
                    ui.label(format!("RTC: {:?}", gs));
                }
                if ui.button("Revoke").clicked() {
                    return Some(StateTransition::Revoke);
                }
            }
        }
        None
    }
}

enum StateTransition {
    StartPairing,
    CancelPairing,
    Connect,
    Revoke,
}

// ── polling helper ──────────────────────────────────────────────────────────

async fn poll_room_event(worker_url: String, room_id: String, tx: mpsc::SyncSender<UiEvent>) {
    let url = format!(
        "{}/pair/event/{}",
        worker_url.trim_end_matches('/'),
        room_id
    );
    // 150 iterations × 2s = 5 minutes timeout.
    for _ in 0..150 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let (Some(pid), Some(ipub)) =
                        (json["pair_id"].as_str(), json["ios_pubkey_b64"].as_str())
                    {
                        let _ = tx.send(UiEvent::Paired {
                            pair_id: pid.into(),
                            peer_pubkey_b64: ipub.into(),
                        });
                        return;
                    }
                }
            }
            _ => {} // 404 or network error: keep polling
        }
    }
}

// ── reqwest helper ──────────────────────────────────────────────────────────

async fn pair_create_request(worker_url: &str, pubkey_b64: &str) -> Result<(PairToken, Vec<u8>)> {
    let url = format!("{}/pair/create", worker_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "mac_pubkey": pubkey_b64 }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("pair/create failed {}: {}", status, body));
    }

    let body: serde_json::Value = resp.json().await?;
    let pair_token = body
        .get("pair_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing pair_token"))?
        .to_string();
    let room_id = body
        .get("room_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing room_id"))?
        .to_string();
    let mac_device_secret = body
        .get("mac_device_secret")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing mac_device_secret"))?
        .to_string();

    let payload = serde_json::json!({
        "pair_token": pair_token,
        "room_id": room_id,
        "worker_url": worker_url,
    });
    let payload_str = serde_json::to_string(&payload)?;
    let png = pair_qr::encode_pair_qr_png(&payload_str)?;

    let token = PairToken {
        pair_token,
        room_id,
        worker_url: worker_url.to_string(),
        mac_device_secret,
    };
    Ok((token, png))
}

// ── revoke helper ───────────────────────────────────────────────────────────

async fn worker_revoke(
    worker_url: &str,
    pair_id: &str,
    mac_device_secret_b64: &str,
) -> anyhow::Result<()> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts: u64 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let secret = B64.decode(mac_device_secret_b64)?;
    let msg = format!("revoke|{pair_id}|{ts}");
    let sig = B64.encode(macagent_core::pair_auth::hmac_sign(&secret, msg.as_bytes()));
    let resp = reqwest::Client::new()
        .post(format!("{}/pair/revoke", worker_url.trim_end_matches('/')))
        .json(&serde_json::json!({ "pair_id": pair_id, "ts": ts, "sig": sig }))
        .send()
        .await?;
    resp.error_for_status()?;
    Ok(())
}

/// Run the menu bar UI. Blocks until the window is closed.
pub fn run_main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let handle = rt.handle().clone();
    let app = MacAgentApp::new(handle)?;
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("macagent")
            .with_inner_size([400.0, 300.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "macagent",
        native_options,
        Box::new(move |_cc| Ok(Box::new(UiHolder { rt, app }))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {}", e))
}

struct UiHolder {
    #[allow(dead_code)]
    rt: tokio::runtime::Runtime,
    app: MacAgentApp,
}

impl eframe::App for UiHolder {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.update(ctx, frame);
    }
}

// ── eframe App impl ─────────────────────────────────────────────────────────

impl eframe::App for MacAgentApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // If a PNG arrived from the background task, upload it as a texture now that ctx is live.
        if let Some(png) = self.pending_png.take() {
            let texture = Self::load_texture(ctx, &png);
            let room_id = self.pending_room_id.take().unwrap_or_default();
            // Start polling for iOS claim now that we have the room_id.
            self.start_polling(room_id);
            self.state = PairState::Pairing {
                qr_texture: texture,
            };
        }

        self.poll_rx();

        // Drain glue state updates.
        while let Ok(gs) = self.glue_state_rx.try_recv() {
            self.glue_state = Some(gs);
        }
        // Drain glue ctrl messages (M2.5 will process them; discard for now).
        while self.glue_msg_rx.try_recv().is_ok() {}

        let glue_state = self.glue_state;
        let transition = egui::CentralPanel::default()
            .show(ctx, |ui| {
                ui.heading(format!("macagent v{}", macagent_core::version()));
                ui.separator();
                Self::render_state(&self.state, &self.last_error, glue_state, ui)
            })
            .inner;

        // Apply any transition outside the immutable borrow of self.state.
        if let Some(t) = transition {
            match t {
                StateTransition::StartPairing => {
                    self.start_pairing();
                    // Show spinner immediately while waiting for the channel result.
                    self.state = PairState::Pairing { qr_texture: None };
                }
                StateTransition::CancelPairing => {
                    self.state = PairState::NotPaired;
                }
                StateTransition::Connect => {
                    if let PairState::Paired { record } = &self.state {
                        let gui_capture = Arc::new(GuiCapture::new(VideoConfig::default()));
                        let ctrl_send_tx = self.ctrl_send_tx.clone();
                        {
                            let tx = ctrl_send_tx.clone();
                            gui_capture.on_stream_ended(move |sup_id, reason| {
                                let _ = tx.send(macagent_core::ctrl_msg::CtrlPayload::StreamEnded {
                                    sup_id,
                                    reason,
                                });
                            });
                        }

                        // Build RtcPeer inside the async block; supervision_router needs it.
                        let worker_url = record.worker_url.clone();
                        let pair_id = record.pair_id.clone();
                        let mac_device_secret_b64 = record.mac_device_secret_b64.clone();
                        let local_keys = Arc::clone(&self.local_keys);
                        let peer_pubkey_b64 = record.peer_pubkey_b64.clone();
                        let ctrl_recv_tx = self.ctrl_recv_tx.clone();
                        let ctrl_send_rx = Arc::clone(&self.ctrl_send_rx);
                        let state_tx = self.glue_state_tx.clone();
                        let msg_tx = self.glue_msg_tx.clone();

                        self.runtime.spawn(async move {
                            use macagent_core::rtc_peer::RtcPeer;
                            let ice = match crate::rtc_glue::fetch_turn_cred_for(
                                &worker_url,
                                &pair_id,
                                &mac_device_secret_b64,
                            )
                            .await
                            {
                                Ok(v) => v,
                                Err(e) => {
                                    eprintln!("[ui] fetch_turn_cred error: {e}");
                                    return;
                                }
                            };
                            let rtc_peer = match RtcPeer::new(ice).await {
                                Ok(p) => Arc::new(p),
                                Err(e) => {
                                    eprintln!("[ui] RtcPeer init error: {e}");
                                    return;
                                }
                            };
                            // Add H.264 track up front so the first offer SDP
                            // includes m=video; SupervisionRouter reuses this handle.
                            let video_track = match rtc_peer.add_local_h264_video_track().await {
                                Ok(t) => t,
                                Err(e) => {
                                    eprintln!("[ui] add_local_h264_video_track error: {e}");
                                    return;
                                }
                            };
                            let supervision_router = Arc::new(SupervisionRouter::new(
                                gui_capture,
                                Arc::clone(&rtc_peer),
                                video_track,
                                ctrl_send_tx,
                            ));
                            let cfg = GlueConfig {
                                worker_url,
                                pair_id,
                                mac_device_secret_b64,
                                local_keys,
                                peer_pubkey_b64,
                                ctrl_recv_tx: Some(ctrl_recv_tx),
                                ctrl_send_rx: Some(ctrl_send_rx),
                                supervision_router: Some(supervision_router),
                                peer: Some(rtc_peer),
                            };
                            if let Err(e) = run_glue(cfg, state_tx, msg_tx).await {
                                eprintln!("glue error: {e}");
                            }
                        });
                    }
                }
                StateTransition::Revoke => {
                    // Best-effort: fire-and-forget the Worker revoke call.
                    if let PairState::Paired { record } = &self.state {
                        self.spawn_worker_revoke(record);
                    }
                    if let Err(e) = Self::revoke_pair_record() {
                        self.last_error = Some(e.to_string());
                    } else {
                        self.state = PairState::NotPaired;
                        self.glue_state = None;
                    }
                }
            }
        }
    }
}
