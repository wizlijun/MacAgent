//! Thread-safe sid → producer mapping.
// Public API is consumed by agent_socket + session_router (M3.5); allow dead_code until then.
#![allow(dead_code)]

use macagent_core::ctrl_msg::{SessionInfo, SessionSource};
use macagent_core::socket_proto::A2P;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_SESSIONS: usize = 8;

#[derive(Debug)]
pub enum RegistryError {
    SessionLimit,
    UnknownSession,
    SendFailed,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::SessionLimit => write!(f, "session limit reached ({MAX_SESSIONS})"),
            RegistryError::UnknownSession => write!(f, "unknown session"),
            RegistryError::SendFailed => write!(f, "send failed (producer disconnected)"),
        }
    }
}

impl std::error::Error for RegistryError {}

struct Inner {
    sessions: HashMap<String, SessionInfo>,
    senders: HashMap<String, mpsc::UnboundedSender<A2P>>,
}

pub struct ProducerRegistry {
    inner: Mutex<Inner>,
}

impl ProducerRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                sessions: HashMap::new(),
                senders: HashMap::new(),
            }),
        }
    }

    /// Register a new producer. Returns assigned sid.
    pub async fn register(
        &self,
        argv: Vec<String>,
        pid: u32,
        cols: u16,
        rows: u16,
        source: SessionSource,
        send_tx: mpsc::UnboundedSender<A2P>,
    ) -> Result<String, RegistryError> {
        let mut inner = self.inner.lock().await;
        if inner.sessions.len() >= MAX_SESSIONS {
            return Err(RegistryError::SessionLimit);
        }
        let sid = Uuid::new_v4().to_string();
        let label = argv.first().cloned().unwrap_or_else(|| "unknown".into());
        let started_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let info = SessionInfo {
            sid: sid.clone(),
            label,
            argv,
            pid,
            cols,
            rows,
            started_ts,
            streaming: false,
            source,
        };
        inner.sessions.insert(sid.clone(), info);
        inner.senders.insert(sid.clone(), send_tx);
        Ok(sid)
    }

    pub async fn unregister(&self, sid: &str) -> Option<SessionInfo> {
        let mut inner = self.inner.lock().await;
        inner.senders.remove(sid);
        inner.sessions.remove(sid)
    }

    pub async fn get(&self, sid: &str) -> Option<SessionInfo> {
        let inner = self.inner.lock().await;
        inner.sessions.get(sid).cloned()
    }

    pub async fn list(&self) -> Vec<SessionInfo> {
        let inner = self.inner.lock().await;
        inner.sessions.values().cloned().collect()
    }

    pub async fn send_to(&self, sid: &str, msg: A2P) -> Result<(), RegistryError> {
        let inner = self.inner.lock().await;
        let tx = inner
            .senders
            .get(sid)
            .ok_or(RegistryError::UnknownSession)?;
        tx.send(msg).map_err(|_| RegistryError::SendFailed)
    }

    pub async fn set_streaming(&self, sid: &str, streaming: bool) {
        let mut inner = self.inner.lock().await;
        if let Some(info) = inner.sessions.get_mut(sid) {
            info.streaming = streaming;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macagent_core::ctrl_msg::SessionSource;

    fn make_sender() -> (mpsc::UnboundedSender<A2P>, mpsc::UnboundedReceiver<A2P>) {
        mpsc::unbounded_channel()
    }

    #[tokio::test]
    async fn register_and_list() {
        let reg = ProducerRegistry::new();

        let (tx1, _rx1) = make_sender();
        let sid1 = reg
            .register(
                vec!["zsh".into(), "-l".into()],
                1001,
                80,
                24,
                SessionSource::UserManual,
                tx1,
            )
            .await
            .unwrap();

        let (tx2, _rx2) = make_sender();
        let sid2 = reg
            .register(
                vec!["claude".into()],
                1002,
                120,
                40,
                SessionSource::IosLaunched {
                    launcher_id: "claude-code".into(),
                },
                tx2,
            )
            .await
            .unwrap();

        assert_ne!(sid1, sid2);

        let list = reg.list().await;
        assert_eq!(list.len(), 2);

        let info = reg.get(&sid1).await.unwrap();
        assert_eq!(info.argv, vec!["zsh", "-l"]);
        assert_eq!(info.pid, 1001);
        assert!(!info.streaming);

        // set_streaming
        reg.set_streaming(&sid1, true).await;
        let info = reg.get(&sid1).await.unwrap();
        assert!(info.streaming);

        // unregister
        let removed = reg.unregister(&sid1).await;
        assert!(removed.is_some());
        assert_eq!(reg.list().await.len(), 1);
        assert!(reg.get(&sid1).await.is_none());
    }

    #[tokio::test]
    async fn session_limit_enforced() {
        let reg = ProducerRegistry::new();
        for i in 0..MAX_SESSIONS {
            let (tx, _rx) = make_sender();
            reg.register(
                vec![format!("cmd{i}")],
                i as u32,
                80,
                24,
                SessionSource::UserManual,
                tx,
            )
            .await
            .unwrap();
        }
        // 9th registration should fail
        let (tx, _rx) = make_sender();
        let err = reg
            .register(
                vec!["overflow".into()],
                999,
                80,
                24,
                SessionSource::UserManual,
                tx,
            )
            .await;
        assert!(matches!(err, Err(RegistryError::SessionLimit)));
    }

    #[tokio::test]
    async fn send_to_delivers_message() {
        let reg = ProducerRegistry::new();
        let (tx, mut rx) = make_sender();
        let sid = reg
            .register(
                vec!["zsh".into()],
                2000,
                80,
                24,
                SessionSource::UserManual,
                tx,
            )
            .await
            .unwrap();

        reg.send_to(&sid, A2P::AttachStart).await.unwrap();
        let msg = rx.recv().await.unwrap();
        assert!(matches!(msg, A2P::AttachStart));
    }

    #[tokio::test]
    async fn send_to_unknown_sid_returns_error() {
        let reg = ProducerRegistry::new();
        let result = reg.send_to("nonexistent", A2P::AttachStop).await;
        assert!(matches!(result, Err(RegistryError::UnknownSession)));
    }
}
