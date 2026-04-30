//! ctrl 通道消息类型 + 端到端签名/校验。

use crate::pair_auth::{hmac_sign, hmac_verify};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CtrlPayload {
    Ping { ts: u64, nonce: String },
    Pong { ts: u64, nonce: String },
    Error { code: String, msg: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCtrl {
    #[serde(flatten)]
    pub payload: CtrlPayload,
    pub sig: String, // base64
}

pub fn canonical_bytes(payload: &CtrlPayload) -> Vec<u8> {
    // 用 BTreeMap 排序保证 key 排序稳定
    let v = serde_json::to_value(payload).unwrap();
    let sorted: BTreeMap<String, serde_json::Value> =
        v.as_object().unwrap().clone().into_iter().collect();
    serde_json::to_vec(&sorted).unwrap()
}

pub fn sign(payload: CtrlPayload, shared_secret: &[u8]) -> SignedCtrl {
    let bytes = canonical_bytes(&payload);
    let sig = B64.encode(hmac_sign(shared_secret, &bytes));
    SignedCtrl { payload, sig }
}

pub fn verify(signed: &SignedCtrl, shared_secret: &[u8]) -> Result<()> {
    let sig = B64.decode(&signed.sig)?;
    let bytes = canonical_bytes(&signed.payload);
    if hmac_verify(shared_secret, &bytes, &sig) {
        Ok(())
    } else {
        Err(anyhow!("ctrl signature invalid"))
    }
}
