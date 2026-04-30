//! `macagent run` producer subcommand.
//!
//! Forks a PTY, tees output to the local terminal (user-visible), feeds bytes
//! into an alacritty `Term`, and pushes snapshots/deltas to the menu bar Agent
//! via Unix socket.

mod parser;
mod pty;
mod socket_client;

use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args;
use macagent_core::{
    ctrl_msg::SessionSource,
    socket_proto::{A2P, P2A},
    terminal::TerminalSnapshot,
};

use parser::ParserState;
use pty::{resize_pty, spawn_pty, PtyHandles};
use socket_client::SocketClient;

/// CLI arguments for the `run` subcommand.
#[derive(Args, Debug)]
pub struct RunArgs {
    /// Optional launcher_id (set by Agent when launching via AppleScript)
    #[arg(long)]
    pub launcher_id: Option<String>,

    /// Initial cols (default: detected from tty or 80)
    #[arg(long)]
    pub cols: Option<u16>,

    /// Initial rows (default: detected from tty or 24)
    #[arg(long)]
    pub rows: Option<u16>,

    /// Command and arguments (must follow `--`)
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

/// Entry point for `macagent run`.
pub fn run_main(args: RunArgs) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let mut producer = Producer::new(args).await?;
        producer.run().await
    })
}

// ─── Producer ────────────────────────────────────────────────────────────────

struct Producer {
    socket: SocketClient,
    pty: PtyHandles,
    parser: ParserState,
    #[allow(dead_code)]
    sid: String,
    streaming: bool,
}

impl Producer {
    async fn new(args: RunArgs) -> Result<Self> {
        let mut socket = SocketClient::connect().await?;

        let (cols, rows) = detect_tty_size(args.cols, args.rows);

        // Send hello.
        let source = match &args.launcher_id {
            Some(id) => SessionSource::IosLaunched {
                launcher_id: id.clone(),
            },
            None => SessionSource::UserManual,
        };
        let pid = std::process::id();
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string());

        socket
            .send(&P2A::ProducerHello {
                argv: args.command.clone(),
                pid,
                cwd,
                cols,
                rows,
                source,
            })
            .await?;

        // Wait for welcome.
        let sid = match socket.recv().await? {
            A2P::ProducerWelcome { sid } => sid,
            other => bail!("expected ProducerWelcome, got {:?}", other),
        };

        eprintln!("[macagent run] session={sid} cmd={:?}", args.command);

        // Fork PTY — inject session id so child processes can use it.
        let extra_env = vec![("MACAGENT_SESSION_ID".to_string(), sid.clone())];
        let pty = spawn_pty(&args.command, cols, rows, &extra_env)?;
        let parser = ParserState::new(cols, rows);

        Ok(Self {
            socket,
            pty,
            parser,
            sid,
            streaming: false,
        })
    }

    async fn run(&mut self) -> Result<()> {
        let mut delta_tick = tokio::time::interval(Duration::from_millis(50));
        let mut keyframe_tick = tokio::time::interval(Duration::from_secs(5));
        delta_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keyframe_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                // PTY output → parser → maybe push events.
                Some(chunk) = self.pty.chunk_rx.recv() => {
                    let history_lines = self.parser.feed(&chunk);
                    if self.streaming && !history_lines.is_empty() {
                        let rev = self.parser.history_revision();
                        self.socket.send(&P2A::TermHistoryAppend {
                            revision: rev,
                            lines: history_lines,
                        }).await?;
                    }
                }

                // 50ms delta tick.
                _ = delta_tick.tick() => {
                    if self.streaming {
                        self.push_delta().await?;
                    }
                }

                // 5s keyframe tick.
                _ = keyframe_tick.tick() => {
                    if self.streaming {
                        self.push_snapshot().await?;
                    }
                }

                // Agent → producer messages.
                msg = self.socket.recv() => {
                    match msg? {
                        A2P::Input { payload } => {
                            use macagent_core::ctrl_msg::TerminalInput;
                            let bytes: Vec<u8> = match payload {
                                TerminalInput::Text { data } => data.into_bytes(),
                                TerminalInput::Key { key } => input_key_bytes(key),
                            };
                            let _ = self.pty.input_tx.send(bytes);
                        }
                        A2P::Resize { cols, rows } => {
                            resize_pty(&self.pty.master, cols, rows);
                            self.parser.resize(cols, rows);
                        }
                        A2P::KillRequest { reason } => {
                            eprintln!("[macagent run] kill requested: {reason}");
                            // Send SIGTERM to child process group via kill.
                            let pid = self.pty.pid;
                            unsafe { libc::kill(pid as i32, libc::SIGTERM); }
                            break;
                        }
                        A2P::AttachStart => {
                            self.streaming = true;
                            // Push immediate history + snapshot.
                            let lines = self.parser.history_lines();
                            if !lines.is_empty() {
                                self.socket.send(&P2A::TermHistorySnapshot {
                                    revision: self.parser.history_revision(),
                                    lines,
                                }).await?;
                            }
                            self.push_snapshot().await?;
                        }
                        A2P::AttachStop => {
                            self.streaming = false;
                        }
                        A2P::ProducerWelcome { .. } => {
                            // Ignore duplicate welcome.
                        }
                        A2P::NotifyAck { .. } => {
                            // Not expected on a run session; ignore.
                        }
                    }
                }

                // Child exit.
                Some(code) = self.pty.exit_rx.recv() => {
                    eprintln!("[macagent run] process exited with code {code}");
                    let _ = self.socket.send(&P2A::ProducerExit {
                        exit_status: Some(code),
                        reason: "process exited".to_string(),
                    }).await;
                    break;
                }

                else => break,
            }
        }
        Ok(())
    }

    async fn push_delta(&mut self) -> Result<()> {
        if let Some(delta) = self.parser.diff() {
            self.socket
                .send(&P2A::TermDelta {
                    revision: delta.revision,
                    cols: delta.cols,
                    rows: delta.rows,
                    cursor_row: delta.cursor_row,
                    cursor_col: delta.cursor_col,
                    cursor_visible: delta.cursor_visible,
                    title: delta.title,
                    lines: delta.lines,
                })
                .await?;
        }
        Ok(())
    }

    async fn push_snapshot(&mut self) -> Result<()> {
        let snap: TerminalSnapshot = self.parser.snapshot();
        self.parser.last_snapshot = Some(snap.clone());
        self.socket
            .send(&P2A::TermSnapshot {
                revision: snap.revision,
                cols: snap.cols,
                rows: snap.rows,
                cursor_row: snap.cursor_row,
                cursor_col: snap.cursor_col,
                cursor_visible: snap.cursor_visible,
                title: snap.title,
                lines: snap.lines,
            })
            .await?;
        Ok(())
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn detect_tty_size(cols_override: Option<u16>, rows_override: Option<u16>) -> (u16, u16) {
    let (default_cols, default_rows) = terminal_size_from_tty().unwrap_or((80, 24));
    (
        cols_override.unwrap_or(default_cols),
        rows_override.unwrap_or(default_rows),
    )
}

fn terminal_size_from_tty() -> Option<(u16, u16)> {
    // TIOCGWINSZ via libc.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            Some((ws.ws_col, ws.ws_row))
        } else {
            None
        }
    }
}

fn input_key_bytes(key: macagent_core::ctrl_msg::InputKey) -> Vec<u8> {
    use macagent_core::ctrl_msg::InputKey::*;
    match key {
        Enter => b"\r".to_vec(),
        Tab => b"\t".to_vec(),
        ShiftTab => b"\x1b[Z".to_vec(),
        Backspace => b"\x7f".to_vec(),
        Escape => b"\x1b".to_vec(),
        ArrowUp => b"\x1b[A".to_vec(),
        ArrowDown => b"\x1b[B".to_vec(),
        ArrowRight => b"\x1b[C".to_vec(),
        ArrowLeft => b"\x1b[D".to_vec(),
        Home => b"\x1b[H".to_vec(),
        End => b"\x1b[F".to_vec(),
        PageUp => b"\x1b[5~".to_vec(),
        PageDown => b"\x1b[6~".to_vec(),
        Delete => b"\x1b[3~".to_vec(),
        CtrlA => b"\x01".to_vec(),
        CtrlC => b"\x03".to_vec(),
        CtrlD => b"\x04".to_vec(),
        CtrlE => b"\x05".to_vec(),
        CtrlK => b"\x0b".to_vec(),
        CtrlL => b"\x0c".to_vec(),
        CtrlR => b"\x12".to_vec(),
        CtrlU => b"\x15".to_vec(),
        CtrlW => b"\x17".to_vec(),
        CtrlZ => b"\x1a".to_vec(),
        F1 => b"\x1bOP".to_vec(),
        F2 => b"\x1bOQ".to_vec(),
        F3 => b"\x1bOR".to_vec(),
        F4 => b"\x1bOS".to_vec(),
        F5 => b"\x1b[15~".to_vec(),
        F6 => b"\x1b[17~".to_vec(),
        F7 => b"\x1b[18~".to_vec(),
        F8 => b"\x1b[19~".to_vec(),
        F9 => b"\x1b[20~".to_vec(),
        F10 => b"\x1b[21~".to_vec(),
        F11 => b"\x1b[23~".to_vec(),
        F12 => b"\x1b[24~".to_vec(),
    }
}
