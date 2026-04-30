//! macagent entry point — clap subcommand dispatch.

mod keychain;
mod pair_qr;
mod rtc_glue;
mod run;
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Ui) {
        Command::Ui => ui::run_main(),
        Command::Run(args) => run::run_main(args),
    }
}
