use clap::Args;

use crate::api::Client;
use crate::config::Config;
use crate::error::{EncoreError, Result};
use crate::store::Store;

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Overwrite an existing config file
    #[arg(long)]
    pub force: bool,

    /// Skip the credential test
    #[arg(long)]
    pub no_test: bool,
}

pub async fn run(cfg: Config, json: bool, args: InitArgs) -> Result<i32> {
    if cfg.config_path.exists() && !args.force {
        return Err(EncoreError::Config(format!(
            "{} already exists; pass --force to overwrite",
            cfg.config_path.display()
        )));
    }

    cfg.require_credentials()?;

    if !args.no_test {
        let api = Client::new(&cfg)?;
        api.probe().await?;
    }

    cfg.write()?;
    let store = Store::open(&cfg.db_path).await?;
    drop(store);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "config": cfg.config_path,
                "database": cfg.db_path,
                "url": cfg.listmonk_url,
            }))?
        );
    } else {
        println!("config    {}", cfg.config_path.display());
        println!("database  {}", cfg.db_path.display());
        println!("listmonk  {}", cfg.listmonk_url);
        println!();
        println!("Required listmonk API user permissions:");
        println!("  campaigns:get, campaigns:manage, campaigns:send,");
        println!("  lists:manage, subscribers:manage, subscribers:sql_query");
    }

    Ok(0)
}
