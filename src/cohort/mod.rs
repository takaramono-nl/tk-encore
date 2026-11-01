use std::collections::HashSet;

use chrono::{DateTime, SecondsFormat, Utc};

use crate::api::Client;
use crate::error::{EncoreError, Result};
use crate::model::{Campaign, CampaignId, SubscriberId};

/// Page size for reading subscribers. Kept well below listmonk's "all" mode so
/// a large newsletter never has to be materialised in one response.
pub const PAGE_SIZE: u32 = 1000;

/// listmonk's batch endpoint is happy with a few thousand IDs; 500 keeps the
/// request small and gives crash recovery a fine-grained boundary.
pub const BATCH_SIZE: usize = 500;

const MAX_PAGES: u32 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub id: SubscriberId,
    pub email: String,
}

#[derive(Debug, Clone)]
pub struct Computed {
    pub members: Vec<Member>,
    pub query: String,
    pub cutoff: DateTime<Utc>,
    pub excluded_local: usize,
}

/// The cutoff is deliberately conservative: `updated_at` moves on every send
/// batch and on the final status change, so anyone created after it cannot
/// have been in the original send.
pub fn resolve_cutoff(campaign: &Campaign, since: Option<DateTime<Utc>>) -> Result<DateTime<Utc>> {
    if let Some(since) = since {
        return Ok(since);
    }

    let started = campaign.started_at.ok_or_else(|| {
        EncoreError::InvalidData(format!(
            "campaign {} has no started_at; pass --since explicitly",
            campaign.id
        ))
    })?;

    let updated = campaign.updated_at.unwrap_or(started);
    Ok(started.max(updated).with_timezone(&Utc))
}

pub fn check_source(campaign: &Campaign) -> Result<()> {
    match campaign.status.as_str() {
        "finished" => Ok(()),
        "running" | "paused" => Err(EncoreError::CampaignInFlight(
            campaign.id,
            campaign.status.clone(),
        )),
        other => Err(EncoreError::CampaignNotFinished(
            campaign.id,
            other.to_string(),
        )),
    }
}

/// Refuse to resend one of Encore's own clones.
pub fn check_not_resend(campaign: &Campaign) -> Result<()> {
    if campaign.tags.iter().any(|t| t == "encore:resend") {
        return Err(EncoreError::Preflight(format!(
            "campaign {} is an Encore resend; refusing to resend a resend",
            campaign.id
        )));
    }
    Ok(())
}

/// Build the listmonk subscriber SQL expression.
///
/// The expression is evaluated server-side by listmonk, so the only local
/// exclusions left are the ledger and open cohorts.
pub fn expression(source_lists: &[i64], campaign: CampaignId, cutoff: DateTime<Utc>) -> String {
    let ids = source_lists
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let cutoff = cutoff.to_rfc3339_opts(SecondsFormat::Secs, true);

    format!(
        "subscribers.status = 'enabled' \
         AND EXISTS (SELECT 1 FROM subscriber_lists cur \
             WHERE cur.subscriber_id = subscribers.id \
               AND cur.list_id IN ({ids}) \
               AND cur.status = 'confirmed') \
         AND NOT EXISTS (SELECT 1 FROM subscriber_lists old \
             WHERE old.subscriber_id = subscribers.id \
               AND old.list_id IN ({ids}) \
               AND old.created_at <= '{cutoff}' \
               AND old.status <> 'unsubscribed') \
         AND NOT EXISTS (SELECT 1 FROM campaign_views cv \
             WHERE cv.subscriber_id = subscribers.id \
               AND cv.campaign_id = {}) \
         AND NOT EXISTS (SELECT 1 FROM link_clicks lc \
             WHERE lc.subscriber_id = subscribers.id \
               AND lc.campaign_id = {})",
        campaign.0, campaign.0
    )
}

pub async fn compute(
    api: &Client,
    campaign: &Campaign,
    source_lists: &[i64],
    since: Option<DateTime<Utc>>,
    local_exclusions: &HashSet<i64>,
) -> Result<Computed> {
    check_source(campaign)?;
    if source_lists.is_empty() {
        return Err(EncoreError::Preflight(format!(
            "campaign {} has no source lists",
            campaign.id
        )));
    }

    let cutoff = resolve_cutoff(campaign, since)?;
    let query = expression(source_lists, CampaignId(campaign.id), cutoff);
    let raw = fetch_all(api, source_lists, &query, PAGE_SIZE).await?;

    let before = raw.len();
    let members: Vec<Member> = raw
        .into_iter()
        .filter(|m| !local_exclusions.contains(&m.id.0))
        .collect();
    let excluded_local = before - members.len();

    Ok(Computed {
        members,
        query,
        cutoff,
        excluded_local,
    })
}

async fn fetch_all(
    api: &Client,
    source_lists: &[i64],
    query: &str,
    page_size: u32,
) -> Result<Vec<Member>> {
    let mut page = 1u32;
    let mut seen: HashSet<i64> = HashSet::new();
    let mut out = Vec::new();
    let mut fetched: i64 = 0;

    loop {
        let resp = api
            .subscribers_page(source_lists, query, page, page_size)
            .await?;

        let count = resp.results.len();
        fetched += count as i64;
        for s in resp.results {
            if seen.insert(s.id) {
                out.push(Member {
                    id: SubscriberId(s.id),
                    email: s.email,
                });
            }
        }

        if count < page_size as usize {
            break;
        }
        if resp.total > 0 && fetched >= resp.total {
            break;
        }
        page += 1;
        if page > MAX_PAGES {
            return Err(EncoreError::Other(format!(
                "subscriber query exceeded {MAX_PAGES} pages"
            )));
        }
    }

    out.sort_by_key(|m| m.id);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn campaign(status: &str) -> Campaign {
        Campaign {
            id: 42,
            uuid: "uuid-42".into(),
            name: "October Dispatch".into(),
            subject: "Hello".into(),
            from_email: "noreply@example.com".into(),
            body: "body".into(),
            body_source: None,
            altbody: None,
            content_type: "richtext".into(),
            template_id: Some(1),
            messenger: "email".into(),
            kind: "regular".into(),
            status: status.into(),
            tags: vec![],
            headers: vec![],
            attribs: serde_json::Value::Null,
            lists: vec![],
            media: vec![],
            to_send: 10,
            sent: 10,
            created_at: None,
            updated_at: Some("2026-10-02T10:00:00+02:00".parse().unwrap()),
            started_at: Some("2026-10-01T10:00:00+02:00".parse().unwrap()),
            send_at: None,
        }
    }

    #[test]
    fn cutoff_uses_latest_activity_in_utc() {
        let c = campaign("finished");
        let cutoff = resolve_cutoff(&c, None).unwrap();
        assert_eq!(
            cutoff.to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-10-02T08:00:00Z"
        );
    }

    #[test]
    fn expression_contains_receipt_and_membership_exclusions() {
        let cutoff = "2026-10-01T08:00:00Z".parse().unwrap();
        let q = expression(&[5, 6], CampaignId(42), cutoff);

        assert!(q.contains("cur.list_id IN (5,6)"));
        assert!(q.contains("old.created_at <= '2026-10-01T08:00:00Z'"));
        assert!(q.contains("campaign_views cv"));
        assert!(q.contains("link_clicks lc"));
        assert!(q.contains("cv.campaign_id = 42"));
        assert!(q.contains("subscribers.status = 'enabled'"));
    }

    #[test]
    fn in_flight_campaigns_are_refused() {
        assert!(matches!(
            check_source(&campaign("running")),
            Err(EncoreError::CampaignInFlight(42, _))
        ));
        assert!(matches!(
            check_source(&campaign("draft")),
            Err(EncoreError::CampaignNotFinished(42, _))
        ));
        assert!(check_source(&campaign("finished")).is_ok());
    }

    #[test]
    fn resend_campaigns_are_refused() {
        let mut c = campaign("finished");
        c.tags = vec!["encore:resend".into()];
        assert!(matches!(
            check_not_resend(&c),
            Err(EncoreError::Preflight(_))
        ));
        assert!(check_not_resend(&campaign("finished")).is_ok());
    }

    #[tokio::test]
    async fn pagination_collects_and_dedups() {
        use secrecy::SecretString;
        use std::time::Duration;
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/subscribers"))
            .and(query_param("list_id", "5"))
            .and(query_param("page", "1"))
            .and(query_param("per_page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "results": [
                        {"id": 1, "email": "a@example.com"},
                        {"id": 2, "email": "b@example.com"}
                    ],
                    "total": 4,
                    "page": 1,
                    "per_page": 2
                }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/subscribers"))
            .and(query_param("list_id", "5"))
            .and(query_param("page", "2"))
            .and(query_param("per_page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "results": [
                        {"id": 2, "email": "b@example.com"},
                        {"id": 3, "email": "c@example.com"}
                    ],
                    "total": 4,
                    "page": 2,
                    "per_page": 2
                }
            })))
            .mount(&server)
            .await;

        let api = Client::build(
            &server.uri(),
            "user",
            &SecretString::from("token"),
            crate::config::AuthMode::Basic,
            Duration::from_secs(5),
            false,
        )
        .unwrap();

        let members = fetch_all(&api, &[5], "TRUE", 2).await.unwrap();
        let ids: Vec<i64> = members.iter().map(|m| m.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(members[1].email, "b@example.com");
    }
}
