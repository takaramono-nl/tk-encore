use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Args;

use crate::app::App;
use crate::cohort;
use crate::config::Config;
use crate::error::{EncoreError, Result};
use crate::model::CampaignId;
use crate::store::Member;
use crate::view::{self, BackfillReport};

#[derive(Args, Debug)]
pub struct BackfillArgs {
    /// Campaign whose subscribers should be marked as already served
    #[arg(long)]
    pub campaign: i64,

    /// File with one email address per line (`-` reads stdin)
    #[arg(long, value_name = "FILE")]
    pub emails: Option<PathBuf>,

    /// Email address to mark as served (repeatable)
    #[arg(long = "email", value_name = "ADDR")]
    pub email: Vec<String>,

    /// Show what would be recorded without writing
    #[arg(long)]
    pub dry_run: bool,

    /// Skip addresses that are not existing listmonk subscribers
    #[arg(long)]
    pub skip_unknown: bool,
}

pub async fn run(cfg: Config, json: bool, args: BackfillArgs) -> Result<i32> {
    let app = App::load(cfg, !args.dry_run).await?;
    execute(app, json, args).await
}

pub async fn execute(app: App, json: bool, args: BackfillArgs) -> Result<i32> {
    let supplied = args.emails.is_some() || !args.email.is_empty();
    let emails = collect_emails(&args)?;
    if emails.is_empty() {
        if !supplied {
            return Err(EncoreError::Preflight(
                "no email addresses supplied; use --emails FILE or --email ADDR".into(),
            ));
        }

        // An empty file or stream is a successful no-op, so
        // `encore run --dry-run --print-emails | encore backfill --emails -`
        // stays quiet when the cohort is empty.
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "campaign_id": args.campaign,
                    "requested": 0,
                    "recorded": 0,
                    "action": "none",
                }))?
            );
        } else {
            println!(
                "campaign {}: no email addresses supplied; nothing to backfill",
                args.campaign
            );
        }
        return Ok(0);
    }

    let campaign_id = CampaignId(args.campaign);
    let campaign = app.api.campaign(campaign_id).await?;
    cohort::check_not_resend(&campaign)?;

    if campaign.uuid.trim().is_empty() {
        return Err(EncoreError::InvalidData(format!(
            "campaign {campaign_id} has no uuid; refusing to backfill without a ledger key"
        )));
    }

    // An open cohort's members are frozen and were computed before these ledger
    // rows existed, so backfilling now would not prevent the duplicate.
    if let Some(open) = app.store.open_cohort(campaign_id).await? {
        return Err(EncoreError::Preflight(format!(
            "campaign {campaign_id} has an open cohort {} ({}); finish or --abandon it before backfilling",
            open.id, open.state
        )));
    }

    let requested: BTreeSet<&str> = emails.iter().map(String::as_str).collect();
    let mut resolved: Vec<Member> = Vec::new();
    let mut matched: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();

    for sub in app.api.subscribers_by_emails(&emails).await? {
        let email = sub.email.trim().to_ascii_lowercase();
        if !requested.contains(email.as_str()) || !seen.insert(sub.id) {
            continue;
        }
        matched.insert(email);
        resolved.push(Member {
            subscriber_id: sub.id,
            email: sub.email,
        });
    }
    resolved.sort_by_key(|m| m.subscriber_id);

    let unknown: Vec<String> = emails
        .iter()
        .filter(|e| !matched.contains(*e))
        .cloned()
        .collect();

    if !unknown.is_empty() && !args.skip_unknown {
        return Err(EncoreError::Preflight(format!(
            "{} email(s) are not listmonk subscribers: {}{}; fix them or pass --skip-unknown",
            unknown.len(),
            unknown
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            if unknown.len() > 10 {
                format!(" and {} more", unknown.len() - 10)
            } else {
                String::new()
            }
        )));
    }

    let already = app.store.served_ids(&campaign.uuid).await?;
    let pending: Vec<Member> = resolved
        .iter()
        .filter(|m| !already.contains(&m.subscriber_id))
        .cloned()
        .collect();

    let recorded = if args.dry_run || pending.is_empty() {
        0
    } else {
        app.store
            .record_backfill(&campaign.uuid, campaign_id, &pending)
            .await?
    };

    let report = BackfillReport {
        campaign_id: campaign_id.0,
        campaign_name: campaign.name.clone(),
        campaign_uuid: campaign.uuid.clone(),
        requested: emails.len(),
        resolved: resolved.len(),
        already_served: resolved.len() - pending.len(),
        pending: pending.len(),
        recorded,
        unknown,
        dry_run: args.dry_run,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", view::render_backfill(&report));
    }

    Ok(0)
}

fn collect_emails(args: &BackfillArgs) -> Result<Vec<String>> {
    let mut raw = args.email.clone();

    if let Some(path) = &args.emails {
        let text = if path.as_path() == Path::new("-") {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text
        } else {
            std::fs::read_to_string(path).map_err(|e| {
                EncoreError::Preflight(format!("cannot read {}: {e}", path.display()))
            })?
        };

        for line in text.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                raw.push(line.to_string());
            }
        }
    }

    normalize_emails(raw)
}

fn normalize_emails(raw: Vec<String>) -> Result<Vec<String>> {
    let mut emails = BTreeSet::new();

    for email in raw {
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() {
            continue;
        }
        if !email.contains('@')
            || email
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\'')
        {
            return Err(EncoreError::Preflight(format!(
                "invalid email address: {email:?}"
            )));
        }
        emails.insert(email);
    }

    Ok(emails.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> BackfillArgs {
        BackfillArgs {
            campaign: 42,
            emails: None,
            email: vec![],
            dry_run: false,
            skip_unknown: false,
        }
    }

    #[test]
    fn email_files_ignore_blanks_comments_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("emails.txt");
        std::fs::write(
            &path,
            "# manual test sends\n  A@Example.com \n\nb@example.com\na@example.com\n",
        )
        .unwrap();

        let mut a = args();
        a.emails = Some(path);
        assert_eq!(
            collect_emails(&a).unwrap(),
            vec!["a@example.com", "b@example.com"]
        );
    }

    #[test]
    fn invalid_addresses_are_refused() {
        assert!(normalize_emails(vec!["not-an-email".into()]).is_err());
        assert!(normalize_emails(vec!["a b@example.com".into()]).is_err());
    }

    #[test]
    fn blank_input_is_allowed_and_reported_empty() {
        assert!(
            normalize_emails(vec!["".into(), "  ".into()])
                .unwrap()
                .is_empty()
        );
    }
}
