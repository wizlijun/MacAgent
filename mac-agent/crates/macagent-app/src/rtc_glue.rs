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
//! 9. ctrl_channel.on_message → 转发给 ctrl_msg_tx（M2.5 心跳处理）
//!
//! SignalingClient 并发模式：单 Mutex 持有，send 走 mpsc outbox → send loop；
//! recv loop 在同一 task 内串行处理（Mutex lock 仅用于 send）。

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use macagent_core::pair_auth::{hmac_sign, PairAuth};
use macagent_core::rtc_peer::{IceServer, PeerState, RtcPeer};
use macagent_core::signaling::{SignalingClient, WsAuthQuery};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
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
    /// Used for ECDH in M2.5+ ctrl-channel auth; carried here for forward compatibility.
    #[allow(dead_code)]
    pub local_keys: Arc<PairAuth>,
    /// Used for ECDH in M2.5+ ctrl-channel auth; carried here for forward compatibility.
    #[allow(dead_code)]
    pub peer_pubkey_b64: String,
}

// ── Wire frame ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum SignalFrame {
    #[serde(rename = "sdp")]
    Sdp { side: String, sdp: String },
    #[serde(rename = "ice")]
    Ice {
        candidate: String,
        #[serde(rename = "sdpMid")]
        sdp_mid: Option<String>,
        #[serde(rename = "sdpMLineIndex")]
        sdp_mline_index: Option<u16>,
    },
    #[serde(rename = "restart")]
    Restart { reason: String },
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

    let peer = Arc::new(RtcPeer::new(ice).await?);
    let ctrl = peer.open_ctrl_channel().await?;

    // ctrl channel → ctrl_msg_tx
    let cmsg_tx = ctrl_msg_tx.clone();
    ctrl.on_message(move |m| {
        let _ = cmsg_tx.send(m);
    })
    .await;

    // Peer state → state_tx
    let st_tx = state_tx.clone();
    peer.on_state_change(move |s| {
        let gs = match s {
            PeerState::Connected => GlueState::PeerConnected,
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
