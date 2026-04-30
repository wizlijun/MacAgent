//! WebSocket 信令客户端。
//!
//! 仅做：建连（带签名 query）、发/收 JSON 帧。
//! 不做：消息加密验证（那是 ctrl_msg 的事）、重连（M1.6 集成时加）。

use crate::pair_auth::hmac_sign;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub struct WsAuthQuery;

impl WsAuthQuery {
    pub fn build(
        device: &str,
        pair_id: &str,
        ts: u64,
        nonce: &str,
        device_secret: &[u8],
    ) -> String {
        let msg = format!("ws-auth|{device}|{pair_id}|{ts}|{nonce}");
        let sig = B64.encode(hmac_sign(device_secret, msg.as_bytes()));
        format!(
            "device={device}&pair_id={pair_id}&ts={ts}&nonce={}&sig={}",
            urlencoding::encode(nonce),
            urlencoding::encode(&sig),
        )
    }
}

pub struct SignalingClient {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl SignalingClient {
    pub async fn connect(url: &str) -> Result<Self> {
        let (ws, _resp) = connect_async(url).await.context("ws connect")?;
        Ok(SignalingClient { ws })
    }

    pub async fn send_text(&mut self, s: &str) -> Result<()> {
        self.ws
            .send(Message::Text(s.to_owned()))
            .await
            .context("ws send")?;
        Ok(())
    }

    pub async fn recv_text(&mut self) -> Result<String> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(s))) => return Ok(s),
                Some(Ok(Message::Binary(_))) => continue,
                Some(Ok(Message::Ping(p))) => {
                    self.ws.send(Message::Pong(p)).await?;
                    continue;
                }
                Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => continue,
                Some(Ok(Message::Close(_))) | None => return Err(anyhow!("ws closed")),
                Some(Err(e)) => return Err(e.into()),
            }
        }
    }

    pub async fn close(mut self) -> Result<()> {
        self.ws.close(None).await.ok();
        Ok(())
    }
}
