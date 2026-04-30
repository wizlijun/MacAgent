//! `macagent notify -- <cmd>` subcommand.
//!
//! 跑 <cmd>（继承父进程的 stdin/stdout/stderr）；同时连 agent.sock 注册命令；
//! 命令退出后向 agent 上报 exit code 触发 APNs 推送。
//!
//! 失败模式：
//! - agent socket 不可达：仍然 exec <cmd>，stderr 一行警告，按命令真实 exit code 退出。
//! - agent ack 超时：同上，warning，命令照常跑、不发推送。

use anyhow::{Context, Result};
use bytes::BytesMut;
use clap::Args;
use macagent_core::socket_proto::{codec, A2P, P2A};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Args, Debug)]
pub struct NotifyArgs {
    /// 推送通知的 title；默认用 argv[0]。
    #[arg(long)]
    pub title: Option<String>,

    /// 实际要跑的命令；用 `--` 分隔。
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

pub fn run_main(args: NotifyArgs) -> Result<i32> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(notify_run(args))
}

async fn notify_run(args: NotifyArgs) -> Result<i32> {
    let register_id = uuid::Uuid::new_v4().to_string();
    let started_at_ms = current_ms();
    let session_hint = std::env::var("MACAGENT_SESSION_ID").ok();

    // 1. 尝试连 socket（best-effort）
    let mut socket = match try_register(&register_id, &args, started_at_ms, session_hint).await {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!(
                "warning: macagent notify could not register with agent: {e}; running without push"
            );
            None
        }
    };

    // 2. fork+exec+wait（继承父进程 stdio）
    let mut child = Command::new(&args.command[0])
        .args(&args.command[1..])
        .spawn()
        .context("failed to spawn command")?;
    let status = child.wait().context("failed to wait for child")?;
    let exit_code = status.code().unwrap_or(-1);
    let ended_at_ms = current_ms();

    // 3. 上报完成（best-effort）
    if let Some(s) = socket.as_mut() {
        let frame = P2A::NotifyComplete {
            register_id: register_id.clone(),
            exit_code,
            ended_at_ms,
        };
        if let Err(e) = send_frame(s, &frame).await {
            eprintln!("warning: failed to send NotifyComplete: {e}");
        }
    }

    Ok(exit_code)
}

async fn try_register(
    register_id: &str,
    args: &NotifyArgs,
    started_at_ms: u64,
    session_hint: Option<String>,
) -> Result<UnixStream> {
    let path = socket_path()?;
    let mut stream = tokio::time::timeout(Duration::from_secs(2), UnixStream::connect(&path))
        .await
        .context("agent socket connect timed out")??;

    let hello = P2A::NotifyRegister {
        register_id: register_id.to_string(),
        argv: args.command.clone(),
        started_at_ms,
        session_hint,
        title: args.title.clone(),
    };
    send_frame(&mut stream, &hello).await?;

    // 等 NotifyAck (3s timeout)
    let ack: A2P = tokio::time::timeout(Duration::from_secs(3), recv_frame(&mut stream))
        .await
        .context("agent did not ack within 3s")??;

    match ack {
        A2P::NotifyAck { register_id: id } if id == register_id => Ok(stream),
        other => anyhow::bail!("unexpected ack: {:?}", other),
    }
}

fn socket_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("home_dir not found")?;
    Ok(home.join("Library/Application Support/macagent/agent.sock"))
}

async fn send_frame<T: serde::Serialize>(stream: &mut UnixStream, msg: &T) -> Result<()> {
    let buf = codec::encode(msg)?;
    stream.write_all(&buf).await?;
    Ok(())
}

async fn recv_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut buf = BytesMut::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            anyhow::bail!("socket closed before receiving frame");
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(value) = codec::try_decode::<T>(&mut buf)? {
            return Ok(value);
        }
    }
}

fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
