use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Deserializer, Serialize};

/// Newtype identifiers. These stop silent argument swaps between the three
/// integer IDs the pipeline juggles.
macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<i64> for $name {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }

        impl From<$name> for i64 {
            fn from(v: $name) -> Self {
                v.0
            }
        }

        impl FromStr for $name {
            type Err = std::num::ParseIntError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.parse()?))
            }
        }
    };
}

id_type!(CampaignId);
id_type!(ListId);
id_type!(SubscriberId);
id_type!(CohortId);

#[derive(Debug, Clone, Deserialize)]
pub struct Envelope<T> {
    pub data: T,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Page<T> {
    pub results: Vec<T>,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub per_page: i64,
}

/// listmonk's campaign stats serialize `campaign_lists.list_id` and
/// `campaign_media.media_id` raw, and both columns are nullable with
/// `ON DELETE SET NULL`. A campaign that references a since-deleted list or
/// attachment therefore contains `"id": null`. Treat that like listmonk's own
/// campaign query, which coalesces deleted references to `0`.
fn null_id_as_zero<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<i64>::deserialize(deserializer)?.unwrap_or(0))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListRef {
    #[serde(default, deserialize_with = "null_id_as_zero")]
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaRef {
    #[serde(default, deserialize_with = "null_id_as_zero")]
    pub id: i64,
    #[serde(default)]
    pub filename: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Campaign {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub name: String,
    pub subject: String,
    #[serde(default)]
    pub from_email: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub body_source: Option<String>,
    #[serde(default)]
    pub altbody: Option<String>,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub template_id: Option<i64>,
    #[serde(default)]
    pub messenger: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub headers: Vec<serde_json::Value>,
    #[serde(default)]
    pub attribs: serde_json::Value,
    #[serde(default)]
    pub lists: Vec<ListRef>,
    #[serde(default)]
    pub media: Vec<MediaRef>,
    #[serde(default)]
    pub to_send: i64,
    #[serde(default)]
    pub sent: i64,
    #[serde(default)]
    pub created_at: Option<DateTime<FixedOffset>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<FixedOffset>>,
    #[serde(default)]
    pub started_at: Option<DateTime<FixedOffset>>,
    #[serde(default)]
    pub send_at: Option<DateTime<FixedOffset>>,
}

impl Campaign {
    pub fn source_list_ids(&self) -> Vec<i64> {
        self.lists
            .iter()
            .map(|l| l.id)
            .filter(|id| *id > 0)
            .collect()
    }

    pub fn is_finished(&self) -> bool {
        self.status == "finished"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NewCampaign {
    pub name: String,
    pub subject: String,
    pub lists: Vec<i64>,
    #[serde(rename = "type")]
    pub kind: String,
    pub content_type: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altbody: Option<String>,
    pub from_email: String,
    pub template_id: i64,
    pub messenger: String,
    pub tags: Vec<String>,
    pub headers: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub attribs: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<i64>,
}

impl NewCampaign {
    /// Build a resend draft from the source campaign.
    ///
    /// Deliberately does not copy `archive*`, `send_at`, `uuid`, or stats:
    /// the resend must never be published to the public archive and must not
    /// be scheduled.
    pub fn resend(source: &Campaign, list: ListId, cohort: CohortId) -> Self {
        let mut tags = source.tags.clone();
        for tag in ["encore:resend", &format!("encore-cohort-{cohort}")] {
            if !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_string());
            }
        }

        Self {
            name: format!("{} (resend #{})", source.name, cohort),
            subject: source.subject.clone(),
            lists: vec![list.0],
            kind: if source.kind.is_empty() {
                "regular".to_string()
            } else {
                source.kind.clone()
            },
            content_type: if source.content_type.is_empty() {
                "richtext".to_string()
            } else {
                source.content_type.clone()
            },
            body: source.body.clone(),
            body_source: source.body_source.clone(),
            altbody: source.altbody.clone(),
            from_email: source.from_email.clone(),
            template_id: source.template_id.unwrap_or(0),
            messenger: if source.messenger.is_empty() {
                "email".to_string()
            } else {
                source.messenger.clone()
            },
            tags,
            headers: source.headers.clone(),
            attribs: source.attribs.clone(),
            media: source
                .media
                .iter()
                .map(|m| m.id)
                .filter(|id| *id > 0)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MailingList {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub optin: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub subscriber_count: i64,
}

impl MailingList {
    pub fn is_archived(&self) -> bool {
        self.status == "archived"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NewList {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub optin: String,
    pub tags: Vec<String>,
}

impl NewList {
    pub fn pending(cohort: CohortId, campaign: CampaignId) -> Self {
        Self {
            name: format!("encore/campaign-{campaign}/cohort-{cohort}"),
            kind: "private".to_string(),
            optin: "single".to_string(),
            tags: vec!["encore".to_string(), format!("encore-cohort-{cohort}")],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateList {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub optin: String,
    pub status: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
}

impl UpdateList {
    /// listmonk's update endpoint replaces the full list, so archiving has to
    /// round-trip the existing name/type/optin/tags.
    pub fn archived(list: &MailingList) -> Self {
        Self {
            name: list.name.clone(),
            kind: list.kind.clone(),
            optin: list.optin.clone(),
            status: "archived".to_string(),
            tags: list.tags.clone(),
            description: list.description.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Subscriber {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubscriberIds {
    pub ids: Vec<i64>,
    pub action: String,
    pub target_list_ids: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Campaign {
        Campaign {
            id: 42,
            uuid: "u".into(),
            name: "October Dispatch".into(),
            subject: "Hello".into(),
            from_email: "noreply@example.com".into(),
            body: "<h1>Hi</h1>".into(),
            body_source: None,
            altbody: Some("Hi".into()),
            content_type: "richtext".into(),
            template_id: Some(1),
            messenger: "email".into(),
            kind: "regular".into(),
            status: "finished".into(),
            tags: vec!["dispatch".into()],
            headers: vec![],
            attribs: serde_json::json!({"edition": 10}),
            lists: vec![ListRef {
                id: 5,
                name: "Newsletter".into(),
            }],
            media: vec![MediaRef {
                id: 9,
                filename: "hero.png".into(),
            }],
            to_send: 5000,
            sent: 5000,
            created_at: None,
            updated_at: None,
            started_at: None,
            send_at: None,
        }
    }

    #[test]
    fn resend_copies_content_and_media() {
        let c = NewCampaign::resend(&source(), ListId(12), CohortId(7));
        let v = serde_json::to_value(&c).unwrap();

        assert_eq!(v["lists"], serde_json::json!([12]));
        assert_eq!(v["body"], "<h1>Hi</h1>");
        assert_eq!(v["media"], serde_json::json!([9]));
        assert!(v.get("archive").is_none());
        assert!(v.get("send_at").is_none());
        assert!(v.get("uuid").is_none());
        assert!(
            v["tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t == "encore-cohort-7")
        );
    }

    #[test]
    fn resend_never_duplicates_tags() {
        let mut s = source();
        s.tags.push("encore:resend".into());
        let c = NewCampaign::resend(&s, ListId(12), CohortId(7));
        assert_eq!(c.tags.iter().filter(|t| *t == "encore:resend").count(), 1);
    }

    #[test]
    fn campaign_accepts_null_template_id() {
        let raw = serde_json::json!({
            "id": 420,
            "name": "October Dispatch",
            "subject": "Hello",
            "template_id": null
        });
        let c: Campaign = serde_json::from_value(raw).unwrap();
        assert_eq!(c.template_id, None);
    }

    #[test]
    fn campaign_accepts_deleted_list_and_media_refs() {
        let raw = serde_json::json!({
            "id": 420,
            "name": "October Dispatch",
            "subject": "Hello",
            "lists": [
                {"id": null, "name": "gone"},
                {"id": 5, "name": "Newsletter"}
            ],
            "media": [
                {"id": null, "filename": "gone.png"},
                {"id": 9, "filename": "hero.png"}
            ]
        });
        let c: Campaign = serde_json::from_value(raw).unwrap();

        assert_eq!(c.source_list_ids(), vec![5]);
        let resend = NewCampaign::resend(&c, ListId(12), CohortId(7));
        assert_eq!(resend.media, vec![9]);
    }

    #[test]
    fn archived_list_payload_keeps_required_fields() {
        let list = MailingList {
            id: 12,
            uuid: "u".into(),
            name: "encore/campaign-420/cohort-1".into(),
            kind: "private".into(),
            optin: "single".into(),
            status: "active".into(),
            tags: vec!["encore".into()],
            description: "resend cohort".into(),
            subscriber_count: 3,
        };
        let v = serde_json::to_value(UpdateList::archived(&list)).unwrap();
        assert_eq!(v["name"], "encore/campaign-420/cohort-1");
        assert_eq!(v["type"], "private");
        assert_eq!(v["optin"], "single");
        assert_eq!(v["status"], "archived");
        assert_eq!(v["tags"], serde_json::json!(["encore"]));
    }
}
