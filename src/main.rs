use clap::Parser;
use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

use encore_listmonk::cli::{Cli, run};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("encore: {e}");
            std::process::exit(e.exit_code());
        }
    }
}
