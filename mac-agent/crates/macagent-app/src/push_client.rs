//! Sends push notifications to iOS via the Cloudflare Worker `/push` endpoint.

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct PushClient {
    worker_url: String,
    pair_id: String,
    mac_device_secret: Vec<u8>,
    http: Arc<reqwest::Client>,
}

impl PushClient {
    pub fn new(worker_url: String, pair_id: String, mac_device_secret_b64: &str) -> Result<Self> {
        let mac_device_secret = B64.decode(mac_device_secret_b64)?;
        Ok(Self {
            worker_url,
            pair_id,
            mac_device_secret,
            http: Arc::new(reqwest::Client::new()),
        })
    }

    pub async fn send(
        &self,
        title: &str,
        body: &str,
        deeplink: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<()> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let msg = format!("push|{}|{}|{}|{}", self.pair_id, ts, title, body);
        let sig = B64.encode(macagent_core::pair_auth::hmac_sign(
            &self.mac_device_secret,
            msg.as_bytes(),
        ));

        let mut payload = serde_json::json!({
            "pair_id": self.pair_id,
            "ts": ts,
            "sig": sig,
            "title": title,
            "body": body,
        });
        if let Some(dl) = deeplink {
            payload["deeplink"] = serde_json::Value::String(dl.to_string());
        }
        if let Some(tid) = thread_id {
            payload["thread_id"] = serde_json::Value::String(tid.to_string());
        }

        let resp = self
            .http
            .post(format!("{}/push", self.worker_url))
            .json(&payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("push returned {}: {}", status, txt);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_with_dummy_secret() {
        let c = PushClient::new(
            "http://x".into(),
            "p".into(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap();
        assert_eq!(c.pair_id, "p");
    }

    #[test]
    fn rejects_invalid_base64() {
        assert!(PushClient::new("http://x".into(), "p".into(), "not!base64").is_err());
    }
}
