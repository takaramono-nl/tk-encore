use clap::Args;

use crate::config::Config;
use crate::error::{EncoreError, Result};
use crate::model::CohortId;
use crate::store::Store;
use crate::view;

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Show one cohort in detail
    #[arg(long)]
    pub cohort: Option<i64>,
}

/// Reads only the local ledger, so status works without listmonk credentials.
pub async fn run(cfg: Config, json: bool, args: StatusArgs) -> Result<i32> {
    let store = Store::open(&cfg.db_path).await?;

    if let Some(raw) = args.cohort {
        let id = CohortId(raw);
        let cohort = store
            .cohort(id)
            .await?
            .ok_or(EncoreError::CohortNotFound(raw))?;
        let members = store.members(id).await?.len() as i64;

        if json {
            println!("{}", serde_json::to_string_pretty(&cohort)?);
        } else {
            print!("{}", view::render_cohort_detail(&cohort, members));
        }
    } else {
        let cohorts = store.cohorts().await?;

        if json {
            println!("{}", serde_json::to_string_pretty(&cohorts)?);
        } else {
            print!("{}", view::render_cohorts(&cohorts));
        }
    }

    Ok(0)
}
