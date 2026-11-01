use reqwest::Method;

use crate::error::Result;
use crate::model::{ListId, Page, Subscriber, SubscriberIds};

use super::Client;

/// Addresses per lookup query. Keeps the SQL expression well below URL and
/// proxy limits while still resolving a whole manual-send list in one call.
pub const EMAIL_LOOKUP_CHUNK: usize = 100;

impl Client {
    /// Resolve exact, case-insensitive email addresses to subscribers.
    ///
    /// Unknown addresses are simply absent from the result. Like the cohort
    /// query, this needs the API user's `subscribers:sql_query` permission.
    pub async fn subscribers_by_emails(&self, emails: &[String]) -> Result<Vec<Subscriber>> {
        let mut out = Vec::new();

        for chunk in emails.chunks(EMAIL_LOOKUP_CHUNK) {
            let page = self
                .subscribers_page(&[], &email_query(chunk), 1, EMAIL_LOOKUP_CHUNK as u32)
                .await?;
            out.extend(page.results);
        }

        Ok(out)
    }

    pub async fn subscribers_page(
        &self,
        list_ids: &[i64],
        query: &str,
        page: u32,
        per_page: u32,
    ) -> Result<Page<Subscriber>> {
        let mut params: Vec<(&str, String)> = Vec::with_capacity(list_ids.len() + 3);
        for id in list_ids {
            params.push(("list_id", id.to_string()));
        }
        params.push(("query", query.to_string()));
        params.push(("page", page.to_string()));
        params.push(("per_page", per_page.to_string()));

        self.get_json("query subscribers", "/api/subscribers", &params)
            .await
    }

    pub async fn add_to_list(&self, ids: &[i64], list: ListId) -> Result<()> {
        self.send_ok(
            "add subscribers to list",
            Method::PUT,
            "/api/subscribers/lists",
            &SubscriberIds {
                ids: ids.to_vec(),
                action: "add".to_string(),
                target_list_ids: vec![list.0],
                status: Some("confirmed".to_string()),
            },
        )
        .await
    }

    pub async fn remove_from_list(&self, ids: &[i64], list: ListId) -> Result<()> {
        self.send_ok(
            "remove subscribers from list",
            Method::PUT,
            "/api/subscribers/lists",
            &SubscriberIds {
                ids: ids.to_vec(),
                action: "remove".to_string(),
                target_list_ids: vec![list.0],
                status: None,
            },
        )
        .await
    }
}

fn email_query(emails: &[String]) -> String {
    let list = emails
        .iter()
        .map(|e| format!("'{}'", e.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");

    format!("LOWER(subscribers.email) IN ({list})")
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use std::time::Duration;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::config::AuthMode;
    use crate::model::ListId;

    use super::{Client, email_query};

    #[tokio::test]
    async fn add_sends_confirmed_membership_payload() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/api/subscribers/lists"))
            .and(body_json(serde_json::json!({
                "ids": [1, 2],
                "action": "add",
                "target_list_ids": [12],
                "status": "confirmed"
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": true})),
            )
            .mount(&server)
            .await;

        let api = Client::build(
            &server.uri(),
            "user",
            &SecretString::from("token"),
            AuthMode::Basic,
            Duration::from_secs(5),
            false,
        )
        .unwrap();

        api.add_to_list(&[1, 2], ListId(12)).await.unwrap();
    }

    #[tokio::test]
    async fn emails_are_resolved_with_a_lowercased_expression() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/subscribers"))
            .and(query_param(
                "query",
                "LOWER(subscribers.email) IN ('a@example.com','b@example.com')",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "results": [
                        {"id": 1, "email": "a@example.com"},
                        {"id": 2, "email": "b@example.com"}
                    ],
                    "total": 2,
                    "page": 1,
                    "per_page": 100
                }
            })))
            .mount(&server)
            .await;

        let api = Client::build(
            &server.uri(),
            "user",
            &SecretString::from("token"),
            AuthMode::Basic,
            Duration::from_secs(5),
            false,
        )
        .unwrap();

        let subscribers = api
            .subscribers_by_emails(&["a@example.com".into(), "b@example.com".into()])
            .await
            .unwrap();
        assert_eq!(subscribers.len(), 2);
        assert_eq!(subscribers[0].id, 1);
    }

    #[test]
    fn email_expression_escapes_quotes() {
        assert_eq!(
            email_query(&["o'connor@example.com".into()]),
            "LOWER(subscribers.email) IN ('o''connor@example.com')"
        );
    }
}
