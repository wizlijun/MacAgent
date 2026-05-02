//! PTY fork helper: spawn a child process under a pseudo-terminal.
//!
//! Provides sync reader/writer threads that communicate via tokio mpsc channels,
//! following the spawn_blocking pattern used in hurryvc/src/producer.rs.

use std::{
    io::{ErrorKind, Read, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

/// Sender for bytes destined for the PTY child process.
pub type PtyInputTx = mpsc::UnboundedSender<Vec<u8>>;

pub struct PtyHandles {
    /// Child process id.
    pub pid: u32,
    /// Send bytes to PTY stdin.
    pub input_tx: PtyInputTx,
    /// Receive chunks from PTY stdout/stderr.
    pub chunk_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    /// Receives exit code when child terminates.
    pub exit_rx: mpsc::UnboundedReceiver<i32>,
    /// Shared master pty for resize.
    pub master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

/// Fork a PTY running `command[0]` with `command[1..]` as arguments.
///
/// `extra_env` is a list of `(key, value)` pairs injected into the child's
/// environment at fork time (e.g. `MACAGENT_SESSION_ID`).
pub fn spawn_pty(
    command: &[String],
    cols: u16,
    rows: u16,
    extra_env: &[(String, String)],
) -> Result<PtyHandles> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty failed")?;

    let mut cmd = CommandBuilder::new(&command[0]);
    for arg in command.iter().skip(1) {
        cmd.arg(arg);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .context("failed to spawn command in pty")?;
    let pid = child.process_id().unwrap_or_default();
    drop(pair.slave);

    let master = Arc::new(Mutex::new(pair.master));

    let mut raw_writer = {
        let guard = master.lock().expect("pty master poisoned");
        guard.take_writer().context("failed to take pty writer")?
    };
    let mut reader = {
        let guard = master.lock().expect("pty master poisoned");
        guard
            .try_clone_reader()
            .context("failed to clone pty reader")?
    };

    let (chunk_tx, chunk_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (exit_tx, exit_rx) = mpsc::unbounded_channel::<i32>();

    // Writer thread: drain input_rx → PTY stdin.
    std::thread::spawn(move || {
        while let Some(bytes) = input_rx.blocking_recv() {
            if raw_writer.write_all(&bytes).is_err() || raw_writer.flush().is_err() {
                break;
            }
        }
    });

    // Local-stdin forwarder thread: parent terminal keystrokes → PTY stdin.
    // Without this the Mac terminal where `macagent run` was launched is
    // read-only — iOS can input but the host can't. Best-effort: if stdin
    // isn't a TTY (piped/redirected), this thread just blocks harmlessly.
    let stdin_input_tx = input_tx.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        let mut buf = [0u8; 1024];
        loop {
            match handle.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin_input_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Reader thread: PTY stdout → chunk_tx + local stdout tee.
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buffer[..n].to_vec();
                    // Tee to local stdout so the user sees output in their terminal.
                    let _ = std::io::stdout().write_all(&chunk);
                    let _ = std::io::stdout().flush();
                    if chunk_tx.send(chunk).is_err() {
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::WouldBlock | ErrorKind::Interrupted | ErrorKind::TimedOut
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    // Wait thread: reap child → exit_tx.
    std::thread::spawn(move || {
        if let Ok(status) = child.wait() {
            let _ = exit_tx.send(status.exit_code() as i32);
        }
    });

    Ok(PtyHandles {
        pid,
        input_tx,
        chunk_rx,
        exit_rx,
        master,
    })
}

/// Resize the PTY master.
pub fn resize_pty(master: &Arc<Mutex<Box<dyn MasterPty + Send>>>, cols: u16, rows: u16) {
    if let Ok(guard) = master.lock() {
        let _ = guard.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_echo_and_read() {
        let mut handles = spawn_pty(&["echo".to_string(), "hello".to_string()], 80, 24, &[])
            .expect("spawn_pty failed");

        // Collect chunks until child exits or we see "hello".
        let mut collected = Vec::new();
        loop {
            tokio::select! {
                Some(chunk) = handles.chunk_rx.recv() => {
                    collected.extend_from_slice(&chunk);
                    let text = String::from_utf8_lossy(&collected);
                    if text.contains("hello") {
                        break;
                    }
                }
                Some(code) = handles.exit_rx.recv() => {
                    assert_eq!(code, 0, "echo should exit 0");
                    break;
                }
            }
        }

        let text = String::from_utf8_lossy(&collected);
        assert!(
            text.contains("hello"),
            "expected 'hello' in pty output, got: {text:?}"
        );
    }
}
