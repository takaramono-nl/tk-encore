use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::cohort::{self, BATCH_SIZE};
use crate::error::{EncoreError, Result};
use crate::model::{Campaign, CampaignId, CohortId, ListId, NewCampaign, NewList};
use crate::store::{self, Cohort, Member, state};

pub struct PreflightOk;
pub struct ListCreated;
pub struct Materialized;
pub struct CampaignCreated;
pub struct Running;
pub struct Finished;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SendSummary {
    pub cohort_id: i64,
    pub campaign_id: i64,
    pub expected: i64,
    pub sent: i64,
    pub to_send: i64,
}

/// Typestate pipeline. Each transition consumes `self`, so the compiler rejects
/// sending before the list is materialized or the campaign is started.
pub struct Pipeline<S> {
    app: Arc<App>,
    cohort: Cohort,
    members: Vec<Member>,
    source: Campaign,
    _stage: PhantomData<S>,
}

impl<S> Pipeline<S> {
    pub async fn load(app: Arc<App>, id: CohortId) -> Result<Self> {
        let cohort = app
            .store
            .cohort(id)
            .await?
            .ok_or(EncoreError::CohortNotFound(id.0))?;

        if !cohort.is_open() {
            return Err(EncoreError::CohortNotResumable(id.0, cohort.state.clone()));
        }

        let members = app.store.members(id).await?;
        if members.is_empty() {
            return Err(EncoreError::Preflight(format!(
                "cohort {id} has no members"
            )));
        }

        let source = app.api.campaign(CampaignId(cohort.campaign_id)).await?;
        cohort::check_source(&source)?;

        Ok(Self {
            app,
            cohort,
            members,
            source,
            _stage: PhantomData,
        })
    }

    fn cast<T>(self) -> Pipeline<T> {
        Pipeline {
            app: self.app,
            cohort: self.cohort,
            members: self.members,
            source: self.source,
            _stage: PhantomData,
        }
    }

    async fn transition(&mut self, next: &str) -> Result<()> {
        self.app
            .store
            .set_state(CohortId(self.cohort.id), next)
            .await?;
        self.cohort.state = next.to_string();
        Ok(())
    }

    fn cohort_id(&self) -> CohortId {
        CohortId(self.cohort.id)
    }

    fn pending_list(&self) -> Result<ListId> {
        self.cohort.pending_list_id.map(ListId).ok_or_else(|| {
            EncoreError::InvalidData(format!("cohort {} has no pending list", self.cohort.id))
        })
    }

    fn resend_campaign(&self) -> Result<CampaignId> {
        self.cohort
            .resend_campaign_id
            .map(CampaignId)
            .ok_or_else(|| {
                EncoreError::InvalidData(format!(
                    "cohort {} has no resend campaign",
                    self.cohort.id
                ))
            })
    }
}

impl Pipeline<PreflightOk> {
    pub async fn create_list(mut self) -> Result<Pipeline<ListCreated>> {
        let cid = self.cohort_id();

        if self.cohort.pending_list_id.is_none() {
            let spec = NewList::pending(cid, CampaignId(self.cohort.campaign_id));

            let existing = self
                .app
                .api
                .lists_matching(&spec.name)
                .await?
                .into_iter()
                .find(|l| l.name == spec.name);

            let list = match existing {
                Some(list) => list,
                None => self.app.api.create_list(&spec).await?,
            };

            self.app.store.set_pending_list(cid, list.id).await?;
            self.cohort.pending_list_id = Some(list.id);
        }

        self.transition(state::LIST_CREATED).await?;
        Ok(self.cast())
    }
}

impl Pipeline<ListCreated> {
    pub async fn materialize(mut self) -> Result<Pipeline<Materialized>> {
        let list = self.pending_list()?;

        // Re-adding is an upsert on listmonk's side, so a resumed run can
        // safely replay every batch instead of tracking acknowledged ones.
        for chunk in self.members.chunks(BATCH_SIZE) {
            let ids: Vec<i64> = chunk.iter().map(|m| m.subscriber_id).collect();
            self.app.api.add_to_list(&ids, list).await?;
        }

        // Read the live membership count rather than the cached list column:
        // this is the guard that the pending list matches the planned cohort.
        let live = self
            .app
            .api
            .subscribers_page(&[list.0], "TRUE", 1, 1)
            .await?
            .total;

        if live != self.members.len() as i64 {
            return Err(EncoreError::SizeMismatch {
                got: live,
                expected: self.members.len() as i64,
            });
        }

        self.transition(state::MATERIALIZED).await?;
        Ok(self.cast())
    }
}

impl Pipeline<Materialized> {
    pub async fn clone_campaign(mut self) -> Result<Pipeline<CampaignCreated>> {
        let cid = self.cohort_id();
        let list = self.pending_list()?;

        let campaign = if let Some(id) = self.cohort.resend_campaign_id {
            self.app.api.campaign(CampaignId(id)).await?
        } else {
            let tag = format!("encore-cohort-{cid}");
            let existing = self.app.api.campaign_by_tag(&tag).await?;

            let campaign = match existing {
                Some(campaign) => campaign,
                None => {
                    let body = NewCampaign::resend(&self.source, list, cid);
                    self.app.api.create_campaign(&body).await?
                }
            };

            self.app.store.set_resend_campaign(cid, campaign.id).await?;
            self.cohort.resend_campaign_id = Some(campaign.id);
            campaign
        };

        if !campaign.lists.iter().any(|l| l.id == list.0) {
            return Err(EncoreError::InvalidData(format!(
                "resend campaign {} does not target pending list {}",
                campaign.id, list.0
            )));
        }

        self.transition(state::CAMPAIGN_CREATED).await?;
        Ok(self.cast())
    }
}

impl Pipeline<CampaignCreated> {
    pub async fn start(mut self) -> Result<Pipeline<Running>> {
        let cid = self.cohort_id();
        let campaign_id = self.resend_campaign()?;

        let current = self.app.api.campaign(campaign_id).await?;
        match current.status.as_str() {
            "draft" | "paused" => {
                self.app
                    .api
                    .set_campaign_status(campaign_id, "running")
                    .await?;
            }
            "running" | "finished" => {}
            other => {
                return Err(EncoreError::PartialSend(format!(
                    "campaign {campaign_id} is {other}"
                )));
            }
        }

        if self.cohort.sent_at.is_none() {
            let now = store::now();
            self.app.store.set_sent_at(cid, &now).await?;
            self.cohort.sent_at = Some(now);
        }

        self.transition(state::RUNNING).await?;
        Ok(self.cast())
    }
}

impl Pipeline<Running> {
    pub async fn wait(mut self) -> Result<SendSummary> {
        let cid = self.cohort_id();
        let campaign_id = self.resend_campaign()?;
        let deadline = tokio::time::Instant::now() + self.app.cfg.send_timeout;

        loop {
            let campaign = self.app.api.campaign(campaign_id).await?;

            if campaign.to_send > 0 && campaign.to_send != self.members.len() as i64 {
                return Err(EncoreError::SizeMismatch {
                    got: campaign.to_send,
                    expected: self.members.len() as i64,
                });
            }

            match campaign.status.as_str() {
                "finished" => {
                    self.transition(state::FINISHED).await?;
                    let summary = summary(&campaign, cid, self.members.len() as i64);
                    if campaign.to_send > 0 && campaign.sent < campaign.to_send {
                        return Err(EncoreError::PartialSend(format!(
                            "campaign {} finished with sent={} of to_send={}; rerun `encore run --campaign {} --resume` to finish the cohort",
                            campaign.id, campaign.sent, campaign.to_send, self.cohort.campaign_id
                        )));
                    }
                    return Ok(summary);
                }
                "paused" => {
                    return Err(EncoreError::PartialSend(format!(
                        "campaign {} is paused; resume it in listmonk or rerun with --resume",
                        campaign.id
                    )));
                }
                "cancelled" => {
                    return Err(EncoreError::PartialSend(format!(
                        "campaign {} was cancelled",
                        campaign.id
                    )));
                }
                _ => {}
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(EncoreError::PartialSend(format!(
                    "campaign {} did not finish within {}s; it may still be running",
                    campaign.id,
                    self.app.cfg.send_timeout.as_secs()
                )));
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

pub async fn run(app: Arc<App>, id: CohortId, resume: bool) -> Result<SendSummary> {
    let cohort = app
        .store
        .cohort(id)
        .await?
        .ok_or(EncoreError::CohortNotFound(id.0))?;

    if !cohort.is_open() {
        return Err(EncoreError::CohortNotResumable(id.0, cohort.state.clone()));
    }

    if !resume && cohort.state != state::PLANNED {
        return Err(EncoreError::CohortNotResumable(
            id.0,
            format!("{} (pass --resume to continue)", cohort.state),
        ));
    }

    let expected = cohort.expected_count;

    match cohort.state.as_str() {
        state::PLANNED => {
            Pipeline::<PreflightOk>::load(app, id)
                .await?
                .create_list()
                .await?
                .materialize()
                .await?
                .clone_campaign()
                .await?
                .start()
                .await?
                .wait()
                .await
        }
        state::LIST_CREATED => {
            Pipeline::<ListCreated>::load(app, id)
                .await?
                .materialize()
                .await?
                .clone_campaign()
                .await?
                .start()
                .await?
                .wait()
                .await
        }
        state::MATERIALIZED => {
            Pipeline::<Materialized>::load(app, id)
                .await?
                .clone_campaign()
                .await?
                .start()
                .await?
                .wait()
                .await
        }
        state::CAMPAIGN_CREATED => {
            Pipeline::<CampaignCreated>::load(app, id)
                .await?
                .start()
                .await?
                .wait()
                .await
        }
        state::RUNNING => Pipeline::<Running>::load(app, id).await?.wait().await,
        state::FINISHED => {
            let campaign_id = cohort.resend_campaign_id.map(CampaignId).ok_or_else(|| {
                EncoreError::InvalidData(format!("cohort {id} has no resend campaign"))
            })?;
            let campaign = app.api.campaign(campaign_id).await?;
            Ok(summary(&campaign, id, expected))
        }
        other => Err(EncoreError::CohortNotResumable(id.0, other.to_string())),
    }
}

fn summary(campaign: &Campaign, cohort: CohortId, expected: i64) -> SendSummary {
    SendSummary {
        cohort_id: cohort.0,
        campaign_id: campaign.id,
        expected,
        sent: campaign.sent,
        to_send: campaign.to_send,
    }
}
