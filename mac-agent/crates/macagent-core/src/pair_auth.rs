//! 配对 + 端到端密钥管理。
//!
//! - X25519 keypair（私钥钥串永存 macOS Keychain，公钥 base64 上 Worker）
//! - 与对端公钥做 ECDH 派生 shared_secret（32B）
//! - HMAC-SHA256 用 shared_secret 签名 ctrl 消息
//! - device_secret 用单独的 HMAC（仅本机 + Worker 知道，用于 WS 握手）

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairToken {
    pub pair_token: String,
    pub room_id: String,
    pub worker_url: String,
    pub mac_device_secret: String, // base64-encoded 32B secret issued by Worker /pair/create
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairRecord {
    pub pair_id: String,
    pub peer_pubkey_b64: String,
    pub mac_device_secret_b64: String,
    pub worker_url: String,
}

#[derive(Debug, Clone)]
pub struct X25519Pub(PublicKey);

impl X25519Pub {
    pub fn from_b64(s: &str) -> Result<Self> {
        let bytes = B64.decode(s).context("decode base64")?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "expected 32 bytes for X25519 pubkey, got {}",
                bytes.len()
            ));
        }
        let arr: [u8; 32] = bytes.try_into().unwrap();
        Ok(X25519Pub(PublicKey::from(arr)))
    }

    pub fn bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    pub fn to_b64(&self) -> String {
        B64.encode(self.0.as_bytes())
    }
}

pub struct PairAuth {
    secret: StaticSecret,
    public: PublicKey,
}

impl PairAuth {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        PairAuth { secret, public }
    }

    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        let secret = StaticSecret::from(bytes);
        let public = PublicKey::from(&secret);
        PairAuth { secret, public }
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    pub fn public_key(&self) -> X25519Pub {
        X25519Pub(self.public)
    }

    pub fn public_key_b64(&self) -> String {
        B64.encode(self.public.as_bytes())
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        *self.public.as_bytes()
    }
}

pub fn derive_shared_secret(local: &PairAuth, peer: &X25519Pub) -> Result<[u8; 32]> {
    let s = local.secret.diffie_hellman(&peer.0);
    Ok(*s.as_bytes())
}

pub fn hmac_sign(secret: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    m.update(msg);
    m.finalize().into_bytes().to_vec()
}

pub fn hmac_verify(secret: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let mut m = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    m.update(msg);
    m.verify_slice(sig).is_ok()
}
