use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

use crate::error::{EncoreError, Result};
use crate::model::{CampaignId, CohortId};

pub mod state {
    pub const PLANNED: &str = "planned";
    pub const LIST_CREATED: &str = "list_created";
    pub const MATERIALIZED: &str = "materialized";
    pub const CAMPAIGN_CREATED: &str = "campaign_created";
    pub const RUNNING: &str = "running";
    pub const FINISHED: &str = "finished";
    pub const CLOSED: &str = "closed";
    pub const CANCELLED: &str = "cancelled";
    pub const FAILED: &str = "failed";

    pub const OPEN: &[&str] = &[
        PLANNED,
        LIST_CREATED,
        MATERIALIZED,
        CAMPAIGN_CREATED,
        RUNNING,
        FINISHED,
        FAILED,
    ];
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct Cohort {
    pub id: i64,
    pub campaign_id: i64,
    pub campaign_uuid: String,
    pub source_list_ids: String,
    pub pending_list_id: Option<i64>,
    pub resend_campaign_id: Option<i64>,
    pub strategy: String,
    pub state: String,
    pub since_at: String,
    pub query_text: String,
    pub expected_count: i64,
    pub created_at: String,
    pub sent_at: Option<String>,
    pub closed_at: Option<String>,
    pub last_error: Option<String>,
}

impl Cohort {
    pub fn source_list_ids(&self) -> Result<Vec<i64>> {
        serde_json::from_str(&self.source_list_ids).map_err(|e| {
            EncoreError::InvalidData(format!(
                "cohort {} has invalid source_list_ids: {e}",
                self.id
            ))
        })
    }

    pub fn cutoff(&self) -> Result<DateTime<Utc>> {
        Ok(DateTime::parse_from_rfc3339(&self.since_at)
            .map_err(|e| {
                EncoreError::InvalidData(format!("cohort {} has invalid since_at: {e}", self.id))
            })?
            .with_timezone(&Utc))
    }

    pub fn is_open(&self) -> bool {
        !matches!(self.state.as_str(), state::CLOSED | state::CANCELLED)
    }
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct Member {
    pub subscriber_id: i64,
    pub email: String,
}

pub struct NewCohort<'a> {
    pub campaign_id: CampaignId,
    pub campaign_uuid: &'a str,
    pub source_list_ids: &'a str,
    pub strategy: &'a str,
    pub since_at: &'a str,
    pub query_text: &'a str,
    pub members: &'a [Member],
}

pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn insert_cohort(&self, new: NewCohort<'_>) -> Result<CohortId> {
        let now = now();
        let mut tx = self.pool.begin().await?;

        let res = sqlx::query(
            "INSERT INTO cohorts (
                campaign_id, campaign_uuid, source_list_ids, strategy, state, since_at,
                query_text, expected_count, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(new.campaign_id.0)
        .bind(new.campaign_uuid)
        .bind(new.source_list_ids)
        .bind(new.strategy)
        .bind(state::PLANNED)
        .bind(new.since_at)
        .bind(new.query_text)
        .bind(new.members.len() as i64)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        let cohort_id = res.last_insert_rowid();

        for m in new.members {
            sqlx::query(
                "INSERT INTO cohort_members (cohort_id, subscriber_id, email, added_at)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT DO NOTHING",
            )
            .bind(cohort_id)
            .bind(m.subscriber_id)
            .bind(&m.email)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(CohortId(cohort_id))
    }

    pub async fn cohort(&self, id: CohortId) -> Result<Option<Cohort>> {
        Ok(
            sqlx::query_as::<_, Cohort>("SELECT * FROM cohorts WHERE id = ?")
                .bind(id.0)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn open_cohort(&self, campaign: CampaignId) -> Result<Option<Cohort>> {
        Ok(sqlx::query_as::<_, Cohort>(
            "SELECT * FROM cohorts
             WHERE campaign_id = ? AND state NOT IN ('closed', 'cancelled')
             ORDER BY id DESC LIMIT 1",
        )
        .bind(campaign.0)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn cohorts(&self) -> Result<Vec<Cohort>> {
        Ok(
            sqlx::query_as::<_, Cohort>("SELECT * FROM cohorts ORDER BY id DESC")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn members(&self, cohort: CohortId) -> Result<Vec<Member>> {
        Ok(sqlx::query_as::<_, Member>(
            "SELECT subscriber_id, email FROM cohort_members
             WHERE cohort_id = ? ORDER BY subscriber_id",
        )
        .bind(cohort.0)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Subscribers already owed or already served this campaign:
    /// the ledger plus members of every *other* open cohort.
    pub async fn exclusions(
        &self,
        campaign_uuid: &str,
        campaign_id: CampaignId,
        except: Option<CohortId>,
    ) -> Result<HashSet<i64>> {
        let except = except.map(|c| c.0).unwrap_or(-1);
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT subscriber_id FROM ledger WHERE campaign_uuid = ?
             UNION
             SELECT cm.subscriber_id
             FROM cohort_members cm
             JOIN cohorts c ON c.id = cm.cohort_id
             WHERE c.campaign_id = ?
               AND c.state NOT IN ('closed', 'cancelled')
               AND c.id <> ?",
        )
        .bind(campaign_uuid)
        .bind(campaign_id.0)
        .bind(except)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    pub async fn set_state(&self, cohort: CohortId, state: &str) -> Result<()> {
        sqlx::query("UPDATE cohorts SET state = ? WHERE id = ?")
            .bind(state)
            .bind(cohort.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_pending_list(&self, cohort: CohortId, list: i64) -> Result<()> {
        sqlx::query("UPDATE cohorts SET pending_list_id = ? WHERE id = ?")
            .bind(list)
            .bind(cohort.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_resend_campaign(&self, cohort: CohortId, campaign: i64) -> Result<()> {
        sqlx::query("UPDATE cohorts SET resend_campaign_id = ? WHERE id = ?")
            .bind(campaign)
            .bind(cohort.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_sent_at(&self, cohort: CohortId, at: &str) -> Result<()> {
        sqlx::query("UPDATE cohorts SET sent_at = ? WHERE id = ?")
            .bind(at)
            .bind(cohort.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn close(&self, cohort: CohortId, state: &str) -> Result<()> {
        sqlx::query("UPDATE cohorts SET state = ?, closed_at = ? WHERE id = ?")
            .bind(state)
            .bind(now())
            .bind(cohort.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_error(&self, cohort: CohortId, err: &str) -> Result<()> {
        sqlx::query("UPDATE cohorts SET last_error = ? WHERE id = ?")
            .bind(err)
            .bind(cohort.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn record_ledger(
        &self,
        campaign_uuid: &str,
        campaign: CampaignId,
        members: &[Member],
    ) -> Result<usize> {
        self.record_served(campaign_uuid, campaign, members, "resend")
            .await
    }

    /// Record subscribers who already received the campaign outside Encore,
    /// such as recipients of listmonk's "Send test" feature.
    pub async fn record_backfill(
        &self,
        campaign_uuid: &str,
        campaign: CampaignId,
        members: &[Member],
    ) -> Result<usize> {
        self.record_served(campaign_uuid, campaign, members, "backfill")
            .await
    }

    async fn record_served(
        &self,
        campaign_uuid: &str,
        campaign: CampaignId,
        members: &[Member],
        served_via: &str,
    ) -> Result<usize> {
        let at = now();
        let mut tx = self.pool.begin().await?;
        let mut inserted = 0usize;

        for m in members {
            let res = sqlx::query(
                "INSERT INTO ledger (
                    campaign_uuid, subscriber_id, campaign_id, email, served_via, recorded_at
                 ) VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(campaign_uuid, subscriber_id) DO NOTHING",
            )
            .bind(campaign_uuid)
            .bind(m.subscriber_id)
            .bind(campaign.0)
            .bind(&m.email)
            .bind(served_via)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
            inserted += res.rows_affected() as usize;
        }

        tx.commit().await?;
        Ok(inserted)
    }

    /// Subscriber IDs already recorded as served for a campaign UUID.
    pub async fn served_ids(&self, campaign_uuid: &str) -> Result<HashSet<i64>> {
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT subscriber_id FROM ledger WHERE campaign_uuid = ?")
                .bind(campaign_uuid)
                .fetch_all(&self.pool)
                .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CampaignId;

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("encore.db")).await.unwrap();
        (dir, store)
    }

    fn member(id: i64) -> Member {
        Member {
            subscriber_id: id,
            email: format!("s{id}@example.com"),
        }
    }

    #[tokio::test]
    async fn cohort_roundtrip_and_exclusions() {
        let (_dir, store) = store().await;
        let members = vec![member(1), member(2), member(3)];
        let id = store
            .insert_cohort(NewCohort {
                campaign_id: CampaignId(42),
                campaign_uuid: "uuid-42",
                source_list_ids: "[5]",
                strategy: "since",
                since_at: "2026-10-01T00:00:00Z",
                query_text: "TRUE",
                members: &members,
            })
            .await
            .unwrap();

        let c = store.cohort(id).await.unwrap().unwrap();
        assert_eq!(c.expected_count, 3);
        assert_eq!(c.state, "planned");
        assert_eq!(store.members(id).await.unwrap(), members);

        store
            .record_ledger("uuid-42", CampaignId(42), &[member(2)])
            .await
            .unwrap();

        let excluded = store
            .exclusions("uuid-42", CampaignId(42), Some(id))
            .await
            .unwrap();
        assert!(excluded.contains(&2));
        assert!(!excluded.contains(&1));
    }

    #[tokio::test]
    async fn only_one_open_cohort_per_campaign() {
        let (_dir, store) = store().await;
        let members = vec![member(1)];
        let new = || NewCohort {
            campaign_id: CampaignId(42),
            campaign_uuid: "uuid-42",
            source_list_ids: "[5]",
            strategy: "since",
            since_at: "2026-10-01T00:00:00Z",
            query_text: "TRUE",
            members: &members,
        };

        let first = store.insert_cohort(new()).await.unwrap();
        assert!(store.insert_cohort(new()).await.is_err());

        store.close(first, state::CLOSED).await.unwrap();
        assert!(store.insert_cohort(new()).await.is_ok());
    }

    #[tokio::test]
    async fn backfill_is_idempotent_and_keeps_its_via() {
        let (_dir, store) = store().await;
        let members = vec![member(1), member(2)];

        assert_eq!(
            store
                .record_backfill("uuid-42", CampaignId(42), &members)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .record_backfill("uuid-42", CampaignId(42), &members)
                .await
                .unwrap(),
            0
        );

        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT subscriber_id, served_via FROM ledger
             WHERE campaign_uuid = 'uuid-42' ORDER BY subscriber_id",
        )
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(rows, vec![(1, "backfill".into()), (2, "backfill".into())]);

        let served = store.served_ids("uuid-42").await.unwrap();
        assert_eq!(served.len(), 2);
        assert!(served.contains(&1) && served.contains(&2));
    }
}
