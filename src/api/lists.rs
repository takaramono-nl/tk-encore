use reqwest::Method;

use crate::error::Result;
use crate::model::{ListId, MailingList, NewList, Page, UpdateList};

use super::Client;

impl Client {
    pub async fn list(&self, id: ListId) -> Result<MailingList> {
        self.get_json("get list", &format!("/api/lists/{}", id), &[])
            .await
    }

    pub async fn lists_matching(&self, name: &str) -> Result<Vec<MailingList>> {
        let page: Page<MailingList> = self
            .get_json(
                "find list",
                "/api/lists",
                &[("query", name.to_string()), ("per_page", "all".to_string())],
            )
            .await?;
        Ok(page.results)
    }

    pub async fn create_list(&self, body: &NewList) -> Result<MailingList> {
        self.send_json("create list", Method::POST, "/api/lists", &[], body)
            .await
    }

    pub async fn update_list(&self, id: ListId, body: &UpdateList) -> Result<MailingList> {
        self.send_json(
            "update list",
            Method::PUT,
            &format!("/api/lists/{}", id),
            &[],
            body,
        )
        .await
    }

    pub async fn archive_list(&self, list: &MailingList) -> Result<MailingList> {
        self.update_list(ListId(list.id), &UpdateList::archived(list))
            .await
    }
}
