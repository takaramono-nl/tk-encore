use reqwest::Method;

use crate::error::Result;
use crate::model::{Campaign, CampaignId, NewCampaign, Page};

use super::Client;

impl Client {
    pub async fn campaign(&self, id: CampaignId) -> Result<Campaign> {
        self.get_json("get campaign", &format!("/api/campaigns/{}", id), &[])
            .await
    }

    /// Find the resend campaign created by a previous run, if it exists.
    /// The tag is written at create time, so a crash between the POST and the
    /// local commit is recoverable.
    pub async fn campaign_by_tag(&self, tag: &str) -> Result<Option<Campaign>> {
        let page: Page<Campaign> = self
            .get_json(
                "find campaign by tag",
                "/api/campaigns",
                &[
                    ("tag", tag.to_string()),
                    ("per_page", "all".to_string()),
                    ("no_body", "true".to_string()),
                ],
            )
            .await?;

        Ok(page
            .results
            .into_iter()
            .filter(|c| c.tags.iter().any(|t| t == tag))
            .max_by_key(|c| c.id))
    }

    pub async fn create_campaign(&self, body: &NewCampaign) -> Result<Campaign> {
        self.send_json("create campaign", Method::POST, "/api/campaigns", &[], body)
            .await
    }

    pub async fn set_campaign_status(&self, id: CampaignId, status: &str) -> Result<Campaign> {
        self.send_json(
            "set campaign status",
            Method::PUT,
            &format!("/api/campaigns/{}/status", id),
            &[],
            &serde_json::json!({ "status": status }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use std::time::Duration;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::config::AuthMode;

    use super::Client;

    fn client(server: &MockServer) -> Client {
        Client::build(
            &server.uri(),
            "user",
            &SecretString::from("token"),
            AuthMode::Basic,
            Duration::from_secs(5),
            false,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn tag_lookup_filters_exactly_and_picks_latest() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/campaigns"))
            .and(query_param("tag", "encore-cohort-7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "results": [
                        {"id": 19, "name": "resend", "subject": "s", "tags": ["encore-cohort-7"]},
                        {"id": 20, "name": "unrelated", "subject": "s", "tags": ["other"]},
                        {"id": 21, "name": "resend again", "subject": "s", "tags": ["encore-cohort-7"]}
                    ],
                    "total": 3
                }
            })))
            .mount(&server)
            .await;

        let found = client(&server)
            .campaign_by_tag("encore-cohort-7")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.id, 21);
    }

    #[tokio::test]
    async fn tag_lookup_tolerates_deleted_list_and_media_refs() {
        let server = MockServer::start().await;

        // listmonk serializes deleted list/media references from
        // campaign_lists/campaign_media as `"id": null`, including for
        // campaigns that do not match the requested tag.
        Mock::given(method("GET"))
            .and(path("/api/campaigns"))
            .and(query_param("tag", "encore-cohort-7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "results": [
                        {
                            "id": 18,
                            "name": "deleted list",
                            "subject": "s",
                            "tags": ["other"],
                            "lists": [{"id": null, "name": "gone"}]
                        },
                        {
                            "id": 21,
                            "name": "resend again",
                            "subject": "s",
                            "tags": ["encore-cohort-7"],
                            "lists": [
                                {"id": null, "name": "gone"},
                                {"id": 5, "name": "Newsletter"}
                            ],
                            "media": [{"id": null, "filename": "gone.png"}]
                        }
                    ],
                    "total": 2
                }
            })))
            .mount(&server)
            .await;

        let found = client(&server)
            .campaign_by_tag("encore-cohort-7")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.id, 21);
        assert_eq!(found.source_list_ids(), vec![5]);
        assert_eq!(
            found.media.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![0]
        );
    }
}
