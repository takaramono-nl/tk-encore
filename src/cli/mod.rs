pub mod backfill;
pub mod init;
pub mod run;
pub mod status;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{AuthMode, Config, Overrides};
use crate::error::Result;

#[derive(Parser, Debug)]
#[command(
    name = "encore",
    version,
    about = "Resend past listmonk campaigns to subscribers who have not received them",
    propagate_version = true
)]
pub struct Cli {
    /// Config file [env: ENCORE_CONFIG]
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// listmonk base URL [env: ENCORE_LISTMONK_URL]
    #[arg(long, global = true)]
    pub url: Option<String>,

    /// listmonk API user [env: ENCORE_LISTMONK_USER]
    #[arg(long, global = true)]
    pub user: Option<String>,

    /// listmonk API token [env: ENCORE_LISTMONK_TOKEN]
    #[arg(long, global = true)]
    pub token: Option<String>,

    /// Auth style: basic (`-u user:token`) or token header
    #[arg(long, global = true, value_enum)]
    pub auth: Option<AuthMode>,

    /// Ledger database path [env: ENCORE_DB]
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Accept invalid TLS certificates (local listmonk only)
    #[arg(long, global = true)]
    pub insecure: bool,

    /// Print machine-readable JSON
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Write config, test credentials, and create the ledger
    Init(init::InitArgs),
    /// Compute the cohort, send if enough new subscribers, and close
    Run(run::RunArgs),
    /// Mark subscribers as already served (manual or listmonk test sends)
    Backfill(backfill::BackfillArgs),
    /// Show cohorts and progress
    Status(status::StatusArgs),
}

impl Cli {
    fn overrides(&self) -> Overrides {
        Overrides {
            config: self.config.clone(),
            url: self.url.clone(),
            user: self.user.clone(),
            token: self.token.clone(),
            auth: self.auth,
            db: self.db.clone(),
            insecure: self.insecure,
        }
    }
}

pub async fn run(cli: Cli) -> Result<i32> {
    let cfg = Config::resolve(&cli.overrides())?;
    let json = cli.json;

    match cli.command {
        Command::Init(args) => init::run(cfg, json, args).await,
        Command::Run(args) => run::run(cfg, json, args).await,
        Command::Backfill(args) => backfill::run(cfg, json, args).await,
        Command::Status(args) => status::run(cfg, json, args).await,
    }
}
