use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use clap::Args;

use crate::app::App;
use crate::cohort::{self, Computed};
use crate::engine;
use crate::error::{EncoreError, Result};
use crate::finalize;
use crate::model::{Campaign, CampaignId, CohortId};
use crate::store::{self, Cohort, NewCohort};
use crate::view::{self, RunReport};

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Source campaign ID
    #[arg(long)]
    pub campaign: i64,

    /// Minimum number of new recipients required to send
    #[arg(long, default_value_t = 1)]
    pub min_new: i64,

    /// Show the cohort without creating one or sending
    #[arg(long)]
    pub dry_run: bool,

    /// Skip the confirmation prompt
    #[arg(long)]
    pub yes: bool,

    /// Override the cutoff (RFC3339 or YYYY-MM-DD)
    #[arg(long)]
    pub since: Option<String>,

    /// Limit to specific source list IDs (must belong to the campaign)
    #[arg(long = "list", value_name = "ID")]
    pub lists: Vec<i64>,

    /// Continue an unfinished cohort
    #[arg(long)]
    pub resume: bool,

    /// Discard an unfinished cohort without recording its members as served
    #[arg(long)]
    pub abandon: bool,
}

pub async fn run(cfg: crate::config::Config, json: bool, args: RunArgs) -> Result<i32> {
    let app = App::load(cfg, !args.dry_run).await?;
    execute(app, json, args).await
}

pub async fn execute(app: App, json: bool, args: RunArgs) -> Result<i32> {
    validate(&args)?;

    let app = Arc::new(app);
    let campaign_id = CampaignId(args.campaign);
    let campaign = app.api.campaign(campaign_id).await?;
    cohort::check_source(&campaign)?;
    cohort::check_not_resend(&campaign)?;

    let source_lists = resolve_lists(&campaign, &args.lists)?;
    let since = args.since.as_deref().map(parse_since).transpose()?;

    if let Some(existing) = app.store.open_cohort(campaign_id).await? {
        if args.abandon {
            let outcome = finalize::abandon(&app, &existing).await?;
            print_abandon(json, &outcome);
            return Ok(0);
        }

        if args.resume {
            return resume(&app, json, existing).await;
        }

        if existing.state == store::state::FINISHED {
            if args.dry_run {
                println!(
                    "cohort {} is finished but still open; run without --dry-run to close it",
                    existing.id
                );
                return Ok(0);
            }
            let outcome = finalize::close(&app, &existing).await?;
            if !json {
                println!(
                    "closed finished cohort {}: {} members recorded",
                    outcome.cohort_id, outcome.members
                );
            }
        } else {
            return Err(EncoreError::Preflight(format!(
                "cohort {} is {}; rerun with --resume to continue it or --abandon to discard it",
                existing.id, existing.state
            )));
        }
    } else if args.resume || args.abandon {
        return Err(EncoreError::Preflight(format!(
            "campaign {campaign_id} has no unfinished cohort"
        )));
    }

    let excluded = app
        .store
        .exclusions(&campaign.uuid, campaign_id, None)
        .await?;
    let computed = cohort::compute(&app.api, &campaign, &source_lists, since, &excluded).await?;

    let report = RunReport {
        campaign_id: campaign_id.0,
        campaign_name: campaign.name.clone(),
        campaign_status: campaign.status.clone(),
        source_lists: list_labels(&campaign, &source_lists),
        strategy: "since".to_string(),
        cutoff: timestamp(computed.cutoff),
        recipients: computed.members.len(),
        excluded_local: computed.excluded_local,
        min_new: args.min_new,
        dry_run: args.dry_run,
    };

    if args.dry_run {
        print_report(json, &report)?;
        return Ok(0);
    }

    let recipients = computed.members.len();
    if recipients == 0 || (recipients as i64) < args.min_new {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "campaign_id": campaign_id.0,
                    "recipients": recipients,
                    "min_new": args.min_new,
                    "action": "none",
                }))?
            );
        } else {
            println!(
                "campaign {}: {} new subscriber(s), need {}; nothing sent",
                campaign_id, recipients, args.min_new
            );
        }
        return Ok(0);
    }

    if !args.yes && !prompt(&campaign, &computed)? {
        println!("aborted");
        return Ok(0);
    }

    let source_json = serde_json::to_string(&source_lists)?;
    let members = to_store(&computed.members);
    let since_at = timestamp(computed.cutoff);
    let cohort_id = app
        .store
        .insert_cohort(NewCohort {
            campaign_id,
            campaign_uuid: &campaign.uuid,
            source_list_ids: &source_json,
            strategy: "since",
            since_at: &since_at,
            query_text: &computed.query,
            members: &members,
        })
        .await?;

    send_and_close(&app, json, cohort_id, false).await
}

async fn resume(app: &Arc<App>, json: bool, cohort: Cohort) -> Result<i32> {
    if cohort.state == store::state::FAILED {
        return Err(EncoreError::Preflight(format!(
            "cohort {} failed; inspect it with `encore status --cohort {}` or discard it with --abandon",
            cohort.id, cohort.id
        )));
    }
    send_and_close(app, json, CohortId(cohort.id), true).await
}

async fn send_and_close(
    app: &Arc<App>,
    json: bool,
    cohort_id: CohortId,
    resume: bool,
) -> Result<i32> {
    let result = engine::run(app.clone(), cohort_id, resume).await;

    match result {
        Ok(summary) => {
            let cohort = app
                .store
                .cohort(cohort_id)
                .await?
                .ok_or(EncoreError::CohortNotFound(cohort_id.0))?;
            let outcome = finalize::close(app, &cohort).await?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "cohort_id": summary.cohort_id,
                        "campaign_id": summary.campaign_id,
                        "expected": summary.expected,
                        "sent": summary.sent,
                        "to_send": summary.to_send,
                        "state": outcome.state,
                        "members": outcome.members,
                    }))?
                );
            } else {
                println!(
                    "campaign {} finished for cohort {}: {} / {} handed off",
                    summary.campaign_id,
                    summary.cohort_id,
                    num(summary.sent),
                    num(summary.to_send)
                );
                println!(
                    "cohort {} closed: {} members recorded, pending list archived",
                    outcome.cohort_id,
                    num(outcome.members as i64)
                );
            }
            Ok(0)
        }
        Err(e) => {
            let _ = app.store.set_error(cohort_id, &e.to_string()).await;
            // A finished campaign can still be recorded even when wait()
            // reported a partial send.
            if let Ok(Some(cohort)) = app.store.cohort(cohort_id).await
                && cohort.state == store::state::FINISHED
            {
                let _ = finalize::close(app, &cohort).await;
            }
            Err(e)
        }
    }
}

fn validate(args: &RunArgs) -> Result<()> {
    if args.min_new < 0 {
        return Err(EncoreError::Preflight("--min-new must be >= 0".into()));
    }
    if args.resume && args.abandon {
        return Err(EncoreError::Preflight(
            "--resume and --abandon cannot be combined".into(),
        ));
    }
    if args.dry_run && (args.resume || args.abandon) {
        return Err(EncoreError::Preflight(
            "--dry-run cannot be combined with --resume or --abandon".into(),
        ));
    }
    Ok(())
}

fn resolve_lists(campaign: &Campaign, requested: &[i64]) -> Result<Vec<i64>> {
    let all = campaign.source_list_ids();
    if requested.is_empty() {
        if all.is_empty() {
            return Err(EncoreError::Preflight(format!(
                "campaign {} has no source lists",
                campaign.id
            )));
        }
        return Ok(all);
    }

    let allowed: HashSet<i64> = all.iter().copied().collect();
    for id in requested {
        if !allowed.contains(id) {
            return Err(EncoreError::Preflight(format!(
                "list {id} is not a source list of campaign {}",
                campaign.id
            )));
        }
    }
    Ok(requested.to_vec())
}

fn parse_since(raw: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is valid")
            .and_utc());
    }
    Err(EncoreError::Preflight(format!(
        "invalid --since '{raw}': use RFC3339 or YYYY-MM-DD"
    )))
}

fn timestamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn to_store(members: &[cohort::Member]) -> Vec<store::Member> {
    members
        .iter()
        .map(|m| store::Member {
            subscriber_id: m.id.0,
            email: m.email.clone(),
        })
        .collect()
}

fn list_labels(campaign: &Campaign, ids: &[i64]) -> Vec<String> {
    ids.iter()
        .map(|id| {
            match campaign
                .lists
                .iter()
                .find(|l| l.id == *id)
                .map(|l| l.name.trim())
            {
                Some(name) if !name.is_empty() => format!("{id} ({name})"),
                _ => id.to_string(),
            }
        })
        .collect()
}

fn prompt(campaign: &Campaign, computed: &Computed) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Err(EncoreError::Preflight(
            "refusing to send without --yes in non-interactive mode".into(),
        ));
    }

    print!(
        "Resend campaign {} \"{}\" to {} subscribers? [y/N] ",
        campaign.id,
        campaign.name,
        num(computed.members.len() as i64)
    );
    std::io::stdout().flush()?;

    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn print_report(json: bool, report: &RunReport) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print!("{}", view::render_run(report));
    }
    Ok(())
}

fn print_abandon(json: bool, outcome: &finalize::Outcome) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "cohort_id": outcome.cohort_id,
                "state": outcome.state,
            }))
            .unwrap_or_default()
        );
    } else {
        println!(
            "cohort {} abandoned: pending list archived, ledger unchanged",
            outcome.cohort_id
        );
    }
}

fn num(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    if n < 0 {
        out.push('-');
    }
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
