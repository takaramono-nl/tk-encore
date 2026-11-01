use reqwest::Method;

use crate::error::Result;
use crate::model::{ListId, Page, Subscriber, SubscriberIds};

use super::Client;

impl Client {
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

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use std::time::Duration;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::config::AuthMode;
    use crate::model::ListId;

    use super::Client;

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
}
