//! Unix socket protocol between `macagent run` producer and menu bar Agent.
//!
//! 4-byte BE length prefix + JSON body. No signing (local-only trust).

use crate::ctrl_msg::{SessionSource, TerminalInput, TerminalLine};
use serde::{Deserialize, Serialize};

/// Messages from producer → Agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum P2A {
    ProducerHello {
        argv: Vec<String>,
        pid: u32,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        source: SessionSource,
    },
    TermSnapshot {
        revision: u64,
        cols: u16,
        rows: u16,
        cursor_row: u16,
        cursor_col: u16,
        cursor_visible: bool,
        title: Option<String>,
        lines: Vec<TerminalLine>,
    },
    TermDelta {
        revision: u64,
        cols: u16,
        rows: u16,
        cursor_row: u16,
        cursor_col: u16,
        cursor_visible: bool,
        title: Option<String>,
        lines: Vec<TerminalLine>,
    },
    TermHistorySnapshot {
        revision: u64,
        lines: Vec<String>,
    },
    TermHistoryAppend {
        revision: u64,
        lines: Vec<String>,
    },
    ProducerExit {
        exit_status: Option<i32>,
        reason: String,
    },
    NotifyRegister {
        register_id: String,
        argv: Vec<String>,
        started_at_ms: u64,
        session_hint: Option<String>,
        title: Option<String>,
    },
    NotifyComplete {
        register_id: String,
        exit_code: i32,
        ended_at_ms: u64,
    },
}

/// Messages from Agent → producer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum A2P {
    ProducerWelcome { sid: String },
    Input { payload: TerminalInput },
    Resize { cols: u16, rows: u16 },
    KillRequest { reason: String },
    AttachStart,
    AttachStop,
    NotifyAck { register_id: String },
}

/// 4-byte BE length prefix frame codec.
pub mod codec {
    use bytes::{Buf, BufMut, BytesMut};
    use serde::{de::DeserializeOwned, Serialize};

    pub fn encode<T: Serialize>(value: &T) -> anyhow::Result<BytesMut> {
        let body = serde_json::to_vec(value)?;
        let mut buf = BytesMut::with_capacity(4 + body.len());
        buf.put_u32(body.len() as u32);
        buf.extend_from_slice(&body);
        Ok(buf)
    }

    pub fn try_decode<T: DeserializeOwned>(buf: &mut BytesMut) -> anyhow::Result<Option<T>> {
        if buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if buf.len() < 4 + len {
            return Ok(None);
        }
        buf.advance(4);
        let body = buf.split_to(len);
        let value = serde_json::from_slice(&body)?;
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctrl_msg::{InputKey, SessionSource};
    use bytes::BytesMut;

    #[test]
    fn round_trip_p2a_producer_hello() {
        let msg = P2A::ProducerHello {
            argv: vec!["zsh".to_string(), "-l".to_string()],
            pid: 1234,
            cwd: Some("/home/user".to_string()),
            cols: 80,
            rows: 24,
            source: SessionSource::UserManual,
        };
        let mut buf = codec::encode(&msg).unwrap();
        let decoded: P2A = codec::try_decode(&mut buf).unwrap().unwrap();
        assert!(buf.is_empty());
        let json_orig = serde_json::to_string(&msg).unwrap();
        let json_dec = serde_json::to_string(&decoded).unwrap();
        assert_eq!(json_orig, json_dec);
    }

    #[test]
    fn round_trip_a2p_input() {
        let msg = A2P::Input {
            payload: TerminalInput::Key {
                key: InputKey::CtrlC,
            },
        };
        let mut buf = codec::encode(&msg).unwrap();
        let decoded: A2P = codec::try_decode(&mut buf).unwrap().unwrap();
        assert!(buf.is_empty());
        let json_orig = serde_json::to_string(&msg).unwrap();
        let json_dec = serde_json::to_string(&decoded).unwrap();
        assert_eq!(json_orig, json_dec);
    }

    #[test]
    fn round_trip_p2a_notify_register() {
        let msg = P2A::NotifyRegister {
            register_id: "reg-001".to_string(),
            argv: vec!["cargo".to_string(), "test".to_string()],
            started_at_ms: 1_700_000_000_000,
            session_hint: Some("sess-abc".to_string()),
            title: Some("cargo test".to_string()),
        };
        let mut buf = codec::encode(&msg).unwrap();
        let decoded: P2A = codec::try_decode(&mut buf).unwrap().unwrap();
        assert!(buf.is_empty());
        let json_orig = serde_json::to_string(&msg).unwrap();
        let json_dec = serde_json::to_string(&decoded).unwrap();
        assert_eq!(json_orig, json_dec);
    }

    #[test]
    fn round_trip_p2a_notify_complete() {
        let msg = P2A::NotifyComplete {
            register_id: "reg-001".to_string(),
            exit_code: 0,
            ended_at_ms: 1_700_000_005_000,
        };
        let mut buf = codec::encode(&msg).unwrap();
        let decoded: P2A = codec::try_decode(&mut buf).unwrap().unwrap();
        assert!(buf.is_empty());
        let json_orig = serde_json::to_string(&msg).unwrap();
        let json_dec = serde_json::to_string(&decoded).unwrap();
        assert_eq!(json_orig, json_dec);
    }

    #[test]
    fn round_trip_a2p_notify_ack() {
        let msg = A2P::NotifyAck {
            register_id: "reg-001".to_string(),
        };
        let mut buf = codec::encode(&msg).unwrap();
        let decoded: A2P = codec::try_decode(&mut buf).unwrap().unwrap();
        assert!(buf.is_empty());
        let json_orig = serde_json::to_string(&msg).unwrap();
        let json_dec = serde_json::to_string(&decoded).unwrap();
        assert_eq!(json_orig, json_dec);
    }

    #[test]
    fn partial_frame_returns_none() {
        let msg = A2P::ProducerWelcome {
            sid: "abc-123".to_string(),
        };
        let full = codec::encode(&msg).unwrap();
        // Only first 3 bytes — no length header yet
        let mut partial = BytesMut::from(&full[..3]);
        let result: Option<A2P> = codec::try_decode(&mut partial).unwrap();
        assert!(result.is_none());
        // 4 bytes header but body incomplete
        let mut partial2 = BytesMut::from(&full[..5]);
        let result2: Option<A2P> = codec::try_decode(&mut partial2).unwrap();
        assert!(result2.is_none());
    }
}
