use crate::app::App;
use crate::cohort::BATCH_SIZE;
use crate::error::{EncoreError, Result};
use crate::model::{CampaignId, CohortId, ListId};
use crate::store::{self, Cohort, Member};

pub struct Outcome {
    pub cohort_id: i64,
    pub state: String,
    pub members: usize,
}

/// Record the served set and archive the pending list once listmonk has
/// finished the resend campaign.
pub async fn close(app: &App, cohort: &Cohort) -> Result<Outcome> {
    let id = CohortId(cohort.id);
    if !cohort.is_open() {
        return Ok(Outcome {
            cohort_id: cohort.id,
            state: cohort.state.clone(),
            members: 0,
        });
    }

    let members = app.store.members(id).await?;
    let pending_list = cohort.pending_list_id.map(ListId);

    if let Some(campaign_id) = cohort.resend_campaign_id.map(CampaignId) {
        let campaign = app.api.campaign(campaign_id).await?;

        if !campaign.is_finished() {
            return Err(EncoreError::Preflight(format!(
                "campaign {} is {}; rerun with --resume or --abandon",
                campaign.id, campaign.status
            )));
        }

        if campaign.sent > 0 {
            app.store
                .record_ledger(
                    &cohort.campaign_uuid,
                    CampaignId(cohort.campaign_id),
                    &members,
                )
                .await?;
        } else {
            tracing::warn!(
                campaign = campaign.id,
                "campaign finished with sent=0; leaving the ledger empty so the cohort can be retried"
            );
        }

        if campaign.to_send > 0 && campaign.sent < campaign.to_send {
            tracing::warn!(
                campaign = campaign.id,
                sent = campaign.sent,
                to_send = campaign.to_send,
                "closing a partially sent campaign"
            );
        }
    }

    if let Some(list) = pending_list {
        remove_members_best_effort(app, list, &members).await;
        archive_if_needed(app, list).await?;
    }

    app.store.close(id, store::state::CLOSED).await?;
    Ok(Outcome {
        cohort_id: id.0,
        state: store::state::CLOSED.to_string(),
        members: members.len(),
    })
}

/// Drop an unfinished cohort without recording its members as served.
pub async fn abandon(app: &App, cohort: &Cohort) -> Result<Outcome> {
    let id = CohortId(cohort.id);
    if !cohort.is_open() {
        return Ok(Outcome {
            cohort_id: cohort.id,
            state: cohort.state.clone(),
            members: 0,
        });
    }

    if let Some(campaign_id) = cohort.resend_campaign_id.map(CampaignId) {
        let campaign = app.api.campaign(campaign_id).await?;
        if matches!(campaign.status.as_str(), "running" | "paused") {
            app.api
                .set_campaign_status(campaign_id, "cancelled")
                .await?;
        }
    }

    if let Some(list) = cohort.pending_list_id.map(ListId) {
        archive_if_needed(app, list).await?;
    }

    app.store.close(id, store::state::CANCELLED).await?;
    Ok(Outcome {
        cohort_id: id.0,
        state: store::state::CANCELLED.to_string(),
        members: 0,
    })
}

async fn archive_if_needed(app: &App, list: ListId) -> Result<()> {
    let current = app.api.list(list).await?;
    if current.is_archived() {
        return Ok(());
    }
    app.api.archive_list(&current).await?;
    Ok(())
}

async fn remove_members_best_effort(app: &App, list: ListId, members: &[Member]) {
    for chunk in members.chunks(BATCH_SIZE) {
        let ids: Vec<i64> = chunk.iter().map(|m| m.subscriber_id).collect();
        if let Err(e) = app.api.remove_from_list(&ids, list).await {
            tracing::warn!(list = list.0, error = %e, "could not clear pending list members");
            return;
        }
    }
}
