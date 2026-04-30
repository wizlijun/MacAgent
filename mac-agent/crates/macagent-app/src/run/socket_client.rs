//! Unix socket client: connects to `~/Library/Application Support/macagent/agent.sock`,
//! exchanges length-prefixed JSON frames with the menu bar Agent.

use anyhow::{Context, Result};
use bytes::BytesMut;
use macagent_core::socket_proto::{
    codec::{encode, try_decode},
    A2P, P2A,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

pub struct SocketClient {
    stream: UnixStream,
    read_buf: BytesMut,
}

impl SocketClient {
    /// Connect to the agent socket at the well-known path.
    pub async fn connect() -> Result<Self> {
        let path = agent_socket_path();
        let stream = UnixStream::connect(&path).await.with_context(|| {
            format!("agent socket not reachable at {path:?}; is macagent UI running?")
        })?;
        Ok(Self {
            stream,
            read_buf: BytesMut::with_capacity(4096),
        })
    }

    /// Send a P2A message.
    pub async fn send(&mut self, msg: &P2A) -> Result<()> {
        let buf = encode(msg)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Receive one A2P message (blocks until a complete frame arrives).
    pub async fn recv(&mut self) -> Result<A2P> {
        loop {
            if let Some(msg) = try_decode::<A2P>(&mut self.read_buf)? {
                return Ok(msg);
            }
            let mut tmp = [0u8; 4096];
            let n = self.stream.read(&mut tmp).await?;
            if n == 0 {
                anyhow::bail!("agent socket closed unexpectedly");
            }
            self.read_buf.extend_from_slice(&tmp[..n]);
        }
    }
}

fn agent_socket_path() -> std::path::PathBuf {
    // macOS: ~/Library/Application Support/macagent/agent.sock
    if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home).join("Library/Application Support/macagent/agent.sock")
    } else {
        std::path::PathBuf::from("/tmp/macagent/agent.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macagent_core::ctrl_msg::SessionSource;
    use macagent_core::socket_proto::codec::encode;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn send_recv_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        // Spawn a minimal echo-style server: accept, read one frame, write welcome.
        let server_path = sock_path.clone();
        tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            // Read raw frame bytes.
            let mut buf = BytesMut::with_capacity(4096);
            let mut tmp = [0u8; 4096];
            loop {
                let n = conn.read(&mut tmp).await.unwrap();
                buf.extend_from_slice(&tmp[..n]);
                if let Ok(Some(_msg)) = try_decode::<P2A>(&mut buf) {
                    break;
                }
            }
            // Reply with ProducerWelcome.
            let reply = A2P::ProducerWelcome {
                sid: "test-sid".to_string(),
            };
            let frame = encode(&reply).unwrap();
            conn.write_all(&frame).await.unwrap();
            drop(server_path); // suppress unused warning
        });

        // Give server a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let mut client = SocketClient {
            stream,
            read_buf: BytesMut::with_capacity(4096),
        };

        let hello = P2A::ProducerHello {
            argv: vec!["echo".to_string()],
            pid: std::process::id(),
            cwd: None,
            cols: 80,
            rows: 24,
            source: SessionSource::UserManual,
        };
        client.send(&hello).await.unwrap();

        let resp = client.recv().await.unwrap();
        match resp {
            A2P::ProducerWelcome { sid } => assert_eq!(sid, "test-sid"),
            other => panic!("unexpected: {:?}", other),
        }
    }
}
