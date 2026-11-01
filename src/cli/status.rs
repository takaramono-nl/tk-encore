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

    /// Print only the cohort's email addresses to stdout, one per line
    /// (requires --cohort)
    #[arg(long)]
    pub print_emails: bool,
}

/// Reads only the local ledger, so status works without listmonk credentials.
pub async fn run(cfg: Config, json: bool, args: StatusArgs) -> Result<i32> {
    validate(&args, json)?;

    let store = Store::open(&cfg.db_path).await?;

    if let Some(raw) = args.cohort {
        let id = CohortId(raw);
        let cohort = store
            .cohort(id)
            .await?
            .ok_or(EncoreError::CohortNotFound(raw))?;
        let members = store.members(id).await?;

        if args.print_emails {
            view::write_emails(members.iter().map(|m| m.email.as_str()))?;
            eprint!(
                "{}",
                view::render_cohort_detail(&cohort, members.len() as i64)
            );
        } else if json {
            println!("{}", serde_json::to_string_pretty(&cohort)?);
        } else {
            print!(
                "{}",
                view::render_cohort_detail(&cohort, members.len() as i64)
            );
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

fn validate(args: &StatusArgs, json: bool) -> Result<()> {
    if args.print_emails && args.cohort.is_none() {
        return Err(EncoreError::Preflight(
            "--print-emails requires --cohort".into(),
        ));
    }
    if args.print_emails && json {
        return Err(EncoreError::Preflight(
            "--print-emails cannot be combined with --json".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> StatusArgs {
        StatusArgs {
            cohort: Some(7),
            print_emails: false,
        }
    }

    #[test]
    fn print_emails_requires_a_cohort() {
        let mut a = args();
        a.cohort = None;
        a.print_emails = true;
        assert!(validate(&a, false).is_err());

        let mut a = args();
        a.print_emails = true;
        assert!(validate(&a, false).is_ok());
    }

    #[test]
    fn print_emails_rejects_json() {
        let mut a = args();
        a.print_emails = true;
        assert!(validate(&a, true).is_err());
        assert!(validate(&a, false).is_ok());
    }
}
