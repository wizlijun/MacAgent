//! macagent entry point — clap subcommand dispatch.

mod agent_socket;
mod clipboard_bridge;
mod keychain;
mod launcher;
mod notify;
mod notify_engine;
mod pair_qr;
mod producer_registry;
mod push_client;
mod rtc_glue;
mod run;
mod session_router;
mod ui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "macagent", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run menu bar UI (default if no subcommand)
    Ui,
    /// Run as producer in current terminal
    Run(run::RunArgs),
    /// Run a command and notify on completion
    Notify(notify::NotifyArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Ui) {
        Command::Ui => ui::run_main(),
        Command::Run(args) => run::run_main(args),
        Command::Notify(args) => {
            let code = notify::run_main(args)?;
            std::process::exit(code);
        }
    }
}
