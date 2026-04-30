//! Unix socket server: accept producer connections + frame codec.

use crate::producer_registry::ProducerRegistry;
use anyhow::Result;
use bytes::BytesMut;
use macagent_core::ctrl_msg::SessionSource;
use macagent_core::socket_proto::{codec, A2P, P2A};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

pub fn socket_path() -> PathBuf {
    let mut p = dirs::home_dir().expect("home_dir");
    p.push("Library/Application Support/macagent/agent.sock");
    p
}

/// Events emitted by AgentSocket to the upper layer (session_router / M3.5).
pub enum ProducerEvent {
    /// A producer sent ProducerHello and was registered. Upper layer receives
    /// the sid plus channels to read P2A frames and write A2P frames.
    Connected {
        sid: String,
        argv: Vec<String>,
        pid: u32,
        cols: u16,
        rows: u16,
        source: SessionSource,
        frames_rx: mpsc::UnboundedReceiver<P2A>,
        // FIXME(M3.8): consumed by run_socket_event_loop when session_router uses direct send_tx
        #[allow(dead_code)]
        send_tx: mpsc::UnboundedSender<A2P>,
    },
    /// Producer connection closed (normal or error); registry already unregistered.
    Disconnected { sid: String },
}

pub struct AgentSocket {
    pub events_rx: mpsc::UnboundedReceiver<ProducerEvent>,
    _accept_handle: tokio::task::JoinHandle<()>,
}

impl AgentSocket {
    pub async fn start(registry: Arc<ProducerRegistry>) -> Result<Self> {
        let path = socket_path();
        // Remove stale socket file if present
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(&path)?;
        let (events_tx, events_rx) = mpsc::unbounded_channel::<ProducerEvent>();

        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let reg = Arc::clone(&registry);
                        let ev_tx = events_tx.clone();
                        tokio::spawn(handle_connection(stream, reg, ev_tx));
                    }
                    Err(e) => {
                        eprintln!("[agent_socket] accept error: {e}");
                    }
                }
            }
        });

        Ok(Self {
            events_rx,
            _accept_handle: handle,
        })
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    registry: Arc<ProducerRegistry>,
    events_tx: mpsc::UnboundedSender<ProducerEvent>,
) {
    let mut buf = BytesMut::with_capacity(4096);

    // ---- 1. Read ProducerHello ----
    let hello = loop {
        match stream.read_buf(&mut buf).await {
            Ok(0) => {
                eprintln!("[agent_socket] connection closed before hello");
                return;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[agent_socket] read error before hello: {e}");
                return;
            }
        }
        match codec::try_decode::<P2A>(&mut buf) {
            Ok(Some(P2A::ProducerHello {
                argv,
                pid,
                cwd,
                cols,
                rows,
                source,
            })) => {
                break (argv, pid, cwd, cols, rows, source);
            }
            Ok(Some(other)) => {
                eprintln!("[agent_socket] expected ProducerHello, got {other:?}");
                return;
            }
            Ok(None) => continue,
            Err(e) => {
                eprintln!("[agent_socket] frame decode error: {e}");
                return;
            }
        }
    };
    let (argv, pid, _cwd, cols, rows, source) = hello;

    // ---- 2. Register in ProducerRegistry ----
    // The A2P sender that the registry (and upper layer) use to write to this producer.
    let (a2p_tx, mut a2p_rx) = mpsc::unbounded_channel::<A2P>();
    // The P2A sender that the per-connection reader task uses to forward frames up.
    let (p2a_tx, p2a_rx) = mpsc::unbounded_channel::<P2A>();

    let sid = match registry
        .register(
            argv.clone(),
            pid,
            cols,
            rows,
            source.clone(),
            a2p_tx.clone(),
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[agent_socket] registry error: {e}");
            return;
        }
    };

    // ---- 3. Send ProducerWelcome ----
    let welcome = A2P::ProducerWelcome { sid: sid.clone() };
    match codec::encode(&welcome) {
        Ok(frame) => {
            if let Err(e) = stream.write_all(&frame).await {
                eprintln!("[agent_socket] write welcome error: {e}");
                registry.unregister(&sid).await;
                return;
            }
        }
        Err(e) => {
            eprintln!("[agent_socket] encode welcome error: {e}");
            registry.unregister(&sid).await;
            return;
        }
    }

    // ---- 4. Notify upper layer ----
    let _ = events_tx.send(ProducerEvent::Connected {
        sid: sid.clone(),
        argv,
        pid,
        cols,
        rows,
        source,
        frames_rx: p2a_rx,
        send_tx: a2p_tx,
    });

    // ---- 5. Split stream and drive read/write tasks ----
    let (mut reader, mut writer) = stream.into_split();

    // Write task: forward A2P from channel → socket
    let write_sid = sid.clone();
    let write_task = tokio::spawn(async move {
        while let Some(msg) = a2p_rx.recv().await {
            match codec::encode(&msg) {
                Ok(frame) => {
                    if let Err(e) = writer.write_all(&frame).await {
                        eprintln!("[agent_socket] write error (sid={write_sid}): {e}");
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("[agent_socket] encode A2P error (sid={write_sid}): {e}");
                }
            }
        }
    });

    // Read task: forward P2A from socket → channel
    let read_sid = sid.clone();
    let mut read_buf = BytesMut::with_capacity(4096);
    loop {
        match reader.read_buf(&mut read_buf).await {
            Ok(0) => break, // connection closed
            Ok(_) => {}
            Err(e) => {
                eprintln!("[agent_socket] read error (sid={read_sid}): {e}");
                break;
            }
        }
        loop {
            match codec::try_decode::<P2A>(&mut read_buf) {
                Ok(Some(frame)) => {
                    if p2a_tx.send(frame).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("[agent_socket] frame decode error (sid={read_sid}): {e}");
                    break;
                }
            }
        }
    }

    // ---- 6. Cleanup ----
    write_task.abort();
    registry.unregister(&sid).await;
    let _ = events_tx.send(ProducerEvent::Disconnected { sid });
}

#[cfg(test)]
mod tests {
    use super::*;
    use macagent_core::ctrl_msg::SessionSource;
    use macagent_core::socket_proto::{codec, A2P, P2A};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    /// Override socket path to a tempdir for tests.
    async fn start_test_socket(
        registry: Arc<ProducerRegistry>,
        sock_path: std::path::PathBuf,
    ) -> mpsc::UnboundedReceiver<ProducerEvent> {
        if sock_path.exists() {
            std::fs::remove_file(&sock_path).unwrap();
        }
        let listener = UnixListener::bind(&sock_path).unwrap();
        let (events_tx, events_rx) = mpsc::unbounded_channel::<ProducerEvent>();

        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let reg = Arc::clone(&registry);
                let ev = events_tx.clone();
                tokio::spawn(handle_connection(stream, reg, ev));
            }
        });

        events_rx
    }

    #[tokio::test]
    async fn accept_connection_assigns_sid() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("test.sock");

        let registry = Arc::new(ProducerRegistry::new());
        let mut events_rx = start_test_socket(Arc::clone(&registry), sock_path.clone()).await;

        // Give the listener a moment to bind
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Connect as a producer
        let mut client = UnixStream::connect(&sock_path).await.unwrap();

        // Send ProducerHello
        let hello = P2A::ProducerHello {
            argv: vec!["zsh".into(), "-l".into()],
            pid: 42,
            cwd: None,
            cols: 80,
            rows: 24,
            source: SessionSource::UserManual,
        };
        let frame = codec::encode(&hello).unwrap();
        client.write_all(&frame).await.unwrap();

        // Read ProducerWelcome
        let mut buf = BytesMut::with_capacity(256);
        loop {
            client.read_buf(&mut buf).await.unwrap();
            if let Ok(Some(msg)) = codec::try_decode::<A2P>(&mut buf) {
                match msg {
                    A2P::ProducerWelcome { sid } => {
                        assert!(!sid.is_empty());
                        // Verify registry has the session
                        let list = registry.list().await;
                        assert_eq!(list.len(), 1);
                        assert_eq!(list[0].sid, sid);
                        break;
                    }
                    _ => panic!("expected ProducerWelcome"),
                }
            }
        }

        // Verify Connected event was emitted
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, ProducerEvent::Connected { .. }));
    }
}
