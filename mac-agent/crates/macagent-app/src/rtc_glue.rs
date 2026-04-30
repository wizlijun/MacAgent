//! WS 信令 ↔ RtcPeer 桥接。
//!
//! 启动一个 async task：
//! 1. POST /turn/cred 拿 ICE servers
//! 2. 连接 SignalingClient WS 到 /signal/<pair_id>
//! 3. 创建 RtcPeer + open ctrl_channel
//! 4. RtcPeer.on_local_candidate → 经内部 mpsc 推到 send loop 发 {kind:"ice"}
//! 5. WS 收到 {kind:"sdp", side:"answer"} → peer.apply_remote_answer
//! 6. WS 收到 {kind:"ice"} → peer.apply_remote_candidate
//! 7. WS 收到 {kind:"restart"} → peer.restart_ice + 把新 offer 经 WS 发回
//! 8. peer.create_offer → 经 WS 发 {kind:"sdp", side:"offer"}
//! 9. ctrl_channel.on_message → 处理 Heartbeat / HeartbeatAck（M2.5）
//!
//! SignalingClient 并发模式：单 Mutex 持有，send 走 mpsc outbox → send loop；
//! recv loop 在同一 task 内串行处理（Mutex lock 仅用于 send）。

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use macagent_core::ctrl_msg::{self, CtrlPayload};
use macagent_core::pair_auth::{derive_shared_secret, hmac_sign, PairAuth, X25519Pub};
use macagent_core::rtc_peer::{CtrlChannel, IceServer, PeerState, RtcPeer};
use macagent_core::signaling::{SignalingClient, WsAuthQuery};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Mutex};

// ── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum GlueState {
    FetchingTurn,
    SignalingConnected,
    NegotiatingSdp,
    PeerConnected,
    Failed,
}

pub struct GlueConfig {
    pub worker_url: String,
    pub pair_id: String,
    pub mac_device_secret_b64: String,
    pub local_keys: Arc<PairAuth>,
    pub peer_pubkey_b64: String,
}

// ── Wire frame ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum SignalFrame {
    #[serde(rename = "sdp")]
    Sdp { side: String, sdp: String },
    #[serde(rename = "ice")]
    Ice { candidate: String },
    #[serde(rename = "restart")]
    Restart { reason: String },
}

// ── Heartbeat ────────────────────────────────────────────────────────────────

/// Spawns a task that sends a Heartbeat every 10 s over the ctrl channel.
/// `ack_rx` receives a () each time a HeartbeatAck arrives.
/// If 30 s elapse without an ack the miss_tx channel is signalled.
fn start_heartbeat(
    ctrl: Arc<CtrlChannel>,
    shared_secret: [u8; 32],
    cancel: Arc<AtomicBool>,
    miss_tx: mpsc::UnboundedSender<()>,
    ack_rx: mpsc::UnboundedReceiver<()>,
) {
    tokio::spawn(async move {
        let mut ack_rx = ack_rx;
        // Track when we last received an ack (start = now so we don't immediately miss)
        let mut last_ack = tokio::time::Instant::now();
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            interval.tick().await;
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            // Drain any pending acks before checking
            while ack_rx.try_recv().is_ok() {
                last_ack = tokio::time::Instant::now();
            }

            // Check for miss (> 30 s since last ack, but only after first hb sent)
            if last_ack.elapsed() > Duration::from_secs(30) {
                eprintln!("[hb] 30s miss — signalling ICE restart");
                let _ = miss_tx.send(());
                last_ack = tokio::time::Instant::now(); // reset so we don't spam
            }

            // Send heartbeat
            let nonce_bytes: [u8; 16] = rand::random();
            let nonce = B64.encode(nonce_bytes);
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let payload = CtrlPayload::Heartbeat { ts, nonce };
            let signed = ctrl_msg::sign(payload, &shared_secret);
            match serde_json::to_string(&signed) {
                Ok(json) => {
                    let _ = ctrl.send_text(&json).await;
                }
                Err(e) => eprintln!("[hb] serialize error: {e}"),
            }
            // Reset last_ack timer baseline after first send
            // (so miss window is measured from first hb, not startup)
        }
    });
}

// ── Entry point ──────────────────────────────────────────────────────────────

pub async fn run_glue(
    cfg: GlueConfig,
    state_tx: mpsc::UnboundedSender<GlueState>,
    ctrl_msg_tx: mpsc::UnboundedSender<String>,
) -> Result<()> {
    let _ = state_tx.send(GlueState::FetchingTurn);
    let ice = fetch_turn_cred(&cfg).await?;

    let _ = state_tx.send(GlueState::SignalingConnected);
    let signaling = connect_signaling(&cfg).await?;

    // Wrap in Arc<Mutex> so the on_local_candidate callback can also send.
    let signaling = Arc::new(Mutex::new(signaling));

    // Derive shared_secret for ctrl-channel HMAC
    let peer_pub = X25519Pub::from_b64(&cfg.peer_pubkey_b64)?;
    let shared_secret = derive_shared_secret(&cfg.local_keys, &peer_pub)?;

    let peer = Arc::new(RtcPeer::new(ice).await?);
    let ctrl = Arc::new(peer.open_ctrl_channel().await?);

    // Channels for heartbeat ack signalling and miss notification
    let (ack_tx, ack_rx) = mpsc::unbounded_channel::<()>();
    let (miss_tx, mut miss_rx) = mpsc::unbounded_channel::<()>();

    // ctrl channel → ctrl_msg_tx + handle hb/hb_ack
    let cmsg_tx = ctrl_msg_tx.clone();
    let ctrl_for_cb = Arc::clone(&ctrl);
    let ss_for_cb = shared_secret;
    ctrl.on_message(move |m| {
        // Try to parse as SignedCtrl and handle heartbeat variants
        if let Ok(signed) = serde_json::from_str::<macagent_core::ctrl_msg::SignedCtrl>(&m) {
            if ctrl_msg::verify(&signed, &ss_for_cb).is_ok() {
                match &signed.payload {
                    CtrlPayload::Heartbeat { nonce, .. } => {
                        // Reply with HeartbeatAck
                        let ack_nonce = nonce.clone();
                        let ctrl_reply = Arc::clone(&ctrl_for_cb);
                        let ss = ss_for_cb;
                        tokio::spawn(async move {
                            let ts = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64;
                            let ack = ctrl_msg::sign(
                                CtrlPayload::HeartbeatAck {
                                    ts,
                                    nonce: ack_nonce,
                                },
                                &ss,
                            );
                            if let Ok(json) = serde_json::to_string(&ack) {
                                let _ = ctrl_reply.send_text(&json).await;
                            }
                        });
                        return; // don't forward to ctrl_msg_tx
                    }
                    CtrlPayload::HeartbeatAck { .. } => {
                        let _ = ack_tx.send(());
                        return; // don't forward to ctrl_msg_tx
                    }
                    _ => {}
                }
            }
        }
        let _ = cmsg_tx.send(m);
    })
    .await;

    // Peer state → state_tx; also start heartbeat on Connected
    let st_tx = state_tx.clone();
    let ctrl_for_hb = Arc::clone(&ctrl);
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_for_hb = Arc::clone(&cancel_flag);
    let miss_tx_for_state = miss_tx.clone();
    let hb_started = Arc::new(AtomicBool::new(false));
    let hb_started_clone = Arc::clone(&hb_started);

    // We need ack_rx in the heartbeat task but also need it captured once.
    // Wrap in Option<> to move into closure once.
    let ack_rx_cell = std::sync::Mutex::new(Some(ack_rx));

    peer.on_state_change(move |s| {
        let gs = match s {
            PeerState::Connected => {
                if !hb_started_clone.swap(true, Ordering::Relaxed) {
                    if let Some(rx) = ack_rx_cell.lock().unwrap().take() {
                        start_heartbeat(
                            Arc::clone(&ctrl_for_hb),
                            shared_secret,
                            Arc::clone(&cancel_for_hb),
                            miss_tx_for_state.clone(),
                            rx,
                        );
                    }
                }
                GlueState::PeerConnected
            }
            PeerState::Failed => GlueState::Failed,
            _ => GlueState::NegotiatingSdp,
        };
        let _ = st_tx.send(gs);
    })
    .await;

    // ICE candidate → WS (sync callback → async send via Mutex)
    let sig_for_ice = Arc::clone(&signaling);
    peer.on_local_candidate(move |candidate_json| {
        let frame = serde_json::json!({
            "kind": "ice",
            "candidate": candidate_json,
        })
        .to_string();
        let sig = Arc::clone(&sig_for_ice);
        tokio::spawn(async move {
            let mut guard = sig.lock().await;
            let _ = guard.send_text(&frame).await;
        });
    })
    .await;

    // Create offer and send it.
    let _ = state_tx.send(GlueState::NegotiatingSdp);
    let offer_sdp = peer.create_offer().await?;
    let offer_frame = serde_json::to_string(&SignalFrame::Sdp {
        side: "offer".into(),
        sdp: offer_sdp,
    })?;
    signaling.lock().await.send_text(&offer_frame).await?;

    // Miss → ICE restart task
    let sig_for_miss = Arc::clone(&signaling);
    let peer_for_miss = Arc::clone(&peer);
    tokio::spawn(async move {
        while miss_rx.recv().await.is_some() {
            eprintln!("[hb] triggering ICE restart");
            match peer_for_miss.restart_ice().await {
                Ok(new_offer) => {
                    let frame = serde_json::to_string(&SignalFrame::Sdp {
                        side: "offer".into(),
                        sdp: new_offer,
                    })
                    .unwrap_or_default();
                    let mut guard = sig_for_miss.lock().await;
                    let _ = guard.send_text(&frame).await;
                }
                Err(e) => eprintln!("[hb] restart_ice error: {e}"),
            }
        }
    });

    // Main recv loop — we hold the lock only while sending.
    loop {
        let text = {
            let mut guard = signaling.lock().await;
            guard.recv_text().await
        };
        match text {
            Err(_) => break,
            Ok(text) => {
                let frame: SignalFrame = match serde_json::from_str(&text) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("bad signal frame: {e}");
                        continue;
                    }
                };
                match frame {
                    SignalFrame::Sdp { side, sdp } if side == "answer" => {
                        peer.apply_remote_answer(&sdp).await?;
                    }
                    SignalFrame::Sdp { side, sdp } if side == "offer" => {
                        peer.apply_remote_offer(&sdp).await?;
                        let answer = peer.create_answer().await?;
                        let answer_frame = serde_json::to_string(&SignalFrame::Sdp {
                            side: "answer".into(),
                            sdp: answer,
                        })?;
                        signaling.lock().await.send_text(&answer_frame).await?;
                    }
                    SignalFrame::Ice { candidate, .. } => {
                        peer.apply_remote_candidate(&candidate).await?;
                    }
                    SignalFrame::Restart { .. } => {
                        let new_offer = peer.restart_ice().await?;
                        let restart_frame = serde_json::to_string(&SignalFrame::Sdp {
                            side: "offer".into(),
                            sdp: new_offer,
                        })?;
                        signaling.lock().await.send_text(&restart_frame).await?;
                    }
                    _ => {}
                }
            }
        }
    }

    cancel_flag.store(true, Ordering::Relaxed);
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn fetch_turn_cred(cfg: &GlueConfig) -> Result<Vec<IceServer>> {
    let ts: u64 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let secret = B64.decode(&cfg.mac_device_secret_b64)?;
    let sig = B64.encode(hmac_sign(
        &secret,
        format!("turn-cred|{}|{}", cfg.pair_id, ts).as_bytes(),
    ));
    let resp: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/turn/cred", cfg.worker_url))
        .json(&serde_json::json!({ "pair_id": &cfg.pair_id, "ts": ts, "sig": sig }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let arr = resp["ice_servers"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing ice_servers"))?;
    let mut out = Vec::new();
    for s in arr {
        let urls = s["urls"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        out.push(IceServer {
            urls,
            username: s["username"].as_str().map(String::from),
            credential: s["credential"].as_str().map(String::from),
        });
    }
    Ok(out)
}

async fn connect_signaling(cfg: &GlueConfig) -> Result<SignalingClient> {
    let ts: u64 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let nonce_bytes: [u8; 16] = rand::random();
    let nonce = B64.encode(nonce_bytes);
    let device_secret = B64.decode(&cfg.mac_device_secret_b64)?;
    let q = WsAuthQuery::build("mac", &cfg.pair_id, ts, &nonce, &device_secret);
    let url = format!(
        "{}/signal/{}?{}",
        cfg.worker_url
            .replace("http://", "ws://")
            .replace("https://", "wss://"),
        cfg.pair_id,
        q,
    );
    SignalingClient::connect(&url).await
}
