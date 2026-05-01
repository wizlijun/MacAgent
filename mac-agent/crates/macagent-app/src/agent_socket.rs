//! Unix socket server: accept producer connections + frame codec.

use crate::notify_engine::NotifyEngine;
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
    pub async fn start(
        registry: Arc<ProducerRegistry>,
        notify_engine: Arc<std::sync::RwLock<Arc<NotifyEngine>>>,
    ) -> Result<Self> {
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
                        // Read the current engine at connection time so a rebuilt engine
                        // is picked up for each new notify connection.
                        let ne = Arc::clone(&*notify_engine.read().unwrap());
                        tokio::spawn(handle_connection(stream, reg, ev_tx, ne));
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
    notify_engine: Arc<NotifyEngine>,
) {
    let mut buf = BytesMut::with_capacity(4096);

    // ---- 1. Read first frame and branch by type ----
    let first_frame = loop {
        match stream.read_buf(&mut buf).await {
            Ok(0) => {
                eprintln!("[agent_socket] connection closed before first frame");
                return;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[agent_socket] read error before first frame: {e}");
                return;
            }
        }
        match codec::try_decode::<P2A>(&mut buf) {
            Ok(Some(frame)) => break frame,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("[agent_socket] frame decode error: {e}");
                return;
            }
        }
    };

    match first_frame {
        P2A::NotifyRegister {
            register_id,
            argv,
            started_at_ms,
            session_hint,
            title,
        } => {
            notify_engine
                .register_notify(
                    register_id.clone(),
                    argv,
                    started_at_ms,
                    session_hint,
                    title,
                )
                .await;
            // Send NotifyAck
            if let Ok(frame) = codec::encode(&A2P::NotifyAck {
                register_id: register_id.clone(),
            }) {
                if let Err(e) = stream.write_all(&frame).await {
                    eprintln!("[agent_socket] write NotifyAck error: {e}");
                    return;
                }
            }
            // Wait for NotifyComplete
            loop {
                match stream.read_buf(&mut buf).await {
                    Ok(0) => return,
                    Ok(_) => {}
                    Err(_) => return,
                }
                loop {
                    match codec::try_decode::<P2A>(&mut buf) {
                        Ok(Some(P2A::NotifyComplete {
                            register_id,
                            exit_code,
                            ended_at_ms,
                        })) => {
                            notify_engine
                                .complete_notify(register_id, exit_code, ended_at_ms)
                                .await;
                            return;
                        }
                        Ok(Some(_)) => continue,
                        Ok(None) => break,
                        Err(_) => return,
                    }
                }
            }
        }
        P2A::ProducerHello {
            argv,
            pid,
            cwd: _cwd,
            cols,
            rows,
            source,
        } => {
            handle_producer_hello(
                stream, registry, events_tx, argv, pid, cols, rows, source, buf,
            )
            .await;
        }
        other => {
            eprintln!("[agent_socket] unexpected first frame: {other:?}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_producer_hello(
    mut stream: UnixStream,
    registry: Arc<ProducerRegistry>,
    events_tx: mpsc::UnboundedSender<ProducerEvent>,
    argv: Vec<String>,
    pid: u32,
    cols: u16,
    rows: u16,
    source: SessionSource,
    buf: BytesMut,
) {
    // (buf may contain bytes read after the hello frame)

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
    // Seed read_buf with any bytes already buffered from the hello-frame read.
    let read_sid = sid.clone();
    let mut read_buf = buf;
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
        use crate::notify_engine::NotifyEngine;
        use tokio::sync::mpsc as async_mpsc;
        if sock_path.exists() {
            std::fs::remove_file(&sock_path).unwrap();
        }
        let listener = UnixListener::bind(&sock_path).unwrap();
        let (events_tx, events_rx) = mpsc::unbounded_channel::<ProducerEvent>();
        let (ctrl_tx, _ctrl_rx) = async_mpsc::unbounded_channel();
        let ne = NotifyEngine::new(None, ctrl_tx);

        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let reg = Arc::clone(&registry);
                let ev = events_tx.clone();
                let ne_clone = Arc::clone(&ne);
                tokio::spawn(handle_connection(stream, reg, ev, ne_clone));
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
