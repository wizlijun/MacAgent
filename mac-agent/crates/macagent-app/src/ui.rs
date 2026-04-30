//! egui UI state machine for macagent.
//!
//! States: NotPaired → Pairing (QR shown) → Paired
//! Transitions are driven by results arriving from the reqwest task via mpsc.

use std::sync::mpsc;

use anyhow::Result;
use egui::ColorImage;
use macagent_core::pair_auth::{PairAuth, PairRecord, PairToken};

use crate::{keychain, pair_qr};

// ── Keychain keys ───────────────────────────────────────────────────────────

const KC_LOCAL_SECRET: &str = "local_secret_key";
const KC_PAIR_ID: &str = "pair_id";
const KC_PEER_PUBKEY: &str = "peer_pubkey_b64";
const KC_DEVICE_SECRET: &str = "device_secret_b64";
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

pub enum PairResult {
    Created { _token: PairToken, png: Vec<u8> },
    Error(String),
}

// ── App ─────────────────────────────────────────────────────────────────────

pub struct MacAgentApp {
    pub worker_url: String,
    pub local_keys: PairAuth,
    pub state: PairState,
    pub last_error: Option<String>,
    /// PNG bytes from a just-completed /pair/create, pending texture upload.
    pub pending_png: Option<Vec<u8>>,
    pub runtime: tokio::runtime::Handle,
    pub rx: mpsc::Receiver<PairResult>,
    pub tx: mpsc::SyncSender<PairResult>,
}

impl MacAgentApp {
    /// Build from environment + Keychain.
    pub fn new(runtime: tokio::runtime::Handle) -> Result<Self> {
        let worker_url = std::env::var("MACAGENT_WORKER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());

        // Load or generate local X25519 keypair.
        let local_keys = match keychain::load(KC_LOCAL_SECRET)? {
            Some(bytes) if bytes.len() == 32 => {
                let arr: [u8; 32] = bytes.try_into().unwrap();
                PairAuth::from_secret_bytes(arr)
            }
            _ => {
                let keys = PairAuth::generate();
                keychain::save(KC_LOCAL_SECRET, &keys.secret_bytes())?;
                keys
            }
        };

        // Check for existing pair record in Keychain.
        let state = match Self::load_pair_record()? {
            Some(record) => PairState::Paired { record },
            None => PairState::NotPaired,
        };

        let (tx, rx) = mpsc::sync_channel(4);

        Ok(Self {
            worker_url,
            local_keys,
            state,
            last_error: None,
            pending_png: None,
            runtime,
            rx,
            tx,
        })
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
        let device_secret_b64 = match keychain::load(KC_DEVICE_SECRET)? {
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
            device_secret_b64,
            worker_url,
        }))
    }

    #[allow(dead_code)] // called when pair/claim completes (future milestone)
    pub fn save_pair_record(record: &PairRecord) -> Result<()> {
        keychain::save(KC_PAIR_ID, record.pair_id.as_bytes())?;
        keychain::save(KC_PEER_PUBKEY, record.peer_pubkey_b64.as_bytes())?;
        keychain::save(KC_DEVICE_SECRET, record.device_secret_b64.as_bytes())?;
        keychain::save(KC_WORKER_URL, record.worker_url.as_bytes())?;
        Ok(())
    }

    fn revoke_pair_record() -> Result<()> {
        keychain::delete(KC_PAIR_ID)?;
        keychain::delete(KC_PEER_PUBKEY)?;
        keychain::delete(KC_DEVICE_SECRET)?;
        keychain::delete(KC_WORKER_URL)?;
        Ok(())
    }

    /// Spawn a reqwest task to call POST /pair/create.
    fn start_pairing(&self) {
        let worker_url = self.worker_url.clone();
        let pubkey_b64 = self.local_keys.public_key_b64();
        let tx = self.tx.clone();

        self.runtime.spawn(async move {
            match pair_create_request(&worker_url, &pubkey_b64).await {
                Ok((_token, png)) => {
                    let _ = tx.send(PairResult::Created { _token, png });
                }
                Err(e) => {
                    let _ = tx.send(PairResult::Error(e.to_string()));
                }
            }
        });
    }

    /// Poll the channel and apply any incoming result.
    fn poll_rx(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                PairResult::Created { _token: _, png } => {
                    self.last_error = None;
                    // Store PNG; texture is uploaded on the next frame when ctx is available.
                    self.pending_png = Some(png);
                }
                PairResult::Error(e) => {
                    self.last_error = Some(e);
                    self.state = PairState::NotPaired;
                }
            }
        }
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
    Revoke,
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
    };
    Ok((token, png))
}

// ── eframe App impl ─────────────────────────────────────────────────────────

impl eframe::App for MacAgentApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // If a PNG arrived from the background task, upload it as a texture now that ctx is live.
        if let Some(png) = self.pending_png.take() {
            let texture = Self::load_texture(ctx, &png);
            self.state = PairState::Pairing {
                qr_texture: texture,
            };
        }

        self.poll_rx();

        let transition = egui::CentralPanel::default()
            .show(ctx, |ui| {
                ui.heading(format!("macagent v{}", macagent_core::version()));
                ui.separator();
                Self::render_state(&self.state, &self.last_error, ui)
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
                StateTransition::Revoke => {
                    if let Err(e) = Self::revoke_pair_record() {
                        self.last_error = Some(e.to_string());
                    } else {
                        self.state = PairState::NotPaired;
                    }
                }
            }
        }
    }
}
