use std::collections::HashSet;
use std::time::Duration;

use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use encore_listmonk::app::App;
use encore_listmonk::cli::backfill::{BackfillArgs, execute};
use encore_listmonk::cohort;
use encore_listmonk::config::{AuthMode, Config};
use encore_listmonk::model::CampaignId;
use encore_listmonk::store;
use encore_listmonk::view;

fn campaign() -> serde_json::Value {
    json!({
        "data": {
            "id": 42,
            "uuid": "uuid-42",
            "name": "October Dispatch",
            "subject": "Hello",
            "from_email": "noreply@example.com",
            "body": "hello",
            "content_type": "richtext",
            "template_id": 1,
            "messenger": "email",
            "type": "regular",
            "status": "finished",
            "tags": [],
            "lists": [{"id": 5, "name": "Newsletter"}],
            "to_send": 5000,
            "sent": 5000,
            "started_at": "2026-10-01T10:00:00Z",
            "updated_at": "2026-10-02T10:00:00Z"
        }
    })
}

fn config(server: &MockServer, dir: &tempfile::TempDir) -> Config {
    Config {
        listmonk_url: server.uri(),
        user: "user".into(),
        token: SecretString::from("token"),
        auth: AuthMode::Basic,
        db_path: dir.path().join("encore.db"),
        lock_path: dir.path().join("encore.lock"),
        config_path: dir.path().join("config.toml"),
        timeout: Duration::from_secs(5),
        send_timeout: Duration::from_secs(5),
        insecure: false,
    }
}

fn args(dry_run: bool) -> BackfillArgs {
    BackfillArgs {
        campaign: 42,
        emails: None,
        email: vec!["A@Example.com".into(), "b@example.com".into()],
        dry_run,
        skip_unknown: false,
    }
}

async fn mount_campaign(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/campaigns/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(campaign()))
        .mount(server)
        .await;
}

async fn mount_lookup(server: &MockServer, results: Vec<serde_json::Value>) {
    let total = results.len();

    Mock::given(method("GET"))
        .and(path("/api/subscribers"))
        .and(query_param(
            "query",
            "LOWER(subscribers.email) IN ('a@example.com','b@example.com')",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "results": results,
                "total": total,
                "page": 1,
                "per_page": 100
            }
        })))
        .mount(server)
        .await;
}

fn both_subscribers() -> Vec<serde_json::Value> {
    vec![
        json!({"id": 1, "email": "a@example.com"}),
        json!({"id": 2, "email": "b@example.com"}),
    ]
}

#[tokio::test]
async fn backfill_records_known_subscribers() {
    let server = MockServer::start().await;
    mount_campaign(&server).await;
    mount_lookup(&server, both_subscribers()).await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    assert_eq!(execute(app, false, args(false)).await.unwrap(), 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert_eq!(
        store.served_ids("uuid-42").await.unwrap(),
        HashSet::from([1, 2])
    );
}

#[tokio::test]
async fn backfill_dry_run_writes_nothing() {
    let server = MockServer::start().await;
    mount_campaign(&server).await;
    mount_lookup(&server, both_subscribers()).await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), false).await.unwrap();

    assert_eq!(execute(app, false, args(true)).await.unwrap(), 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert!(store.served_ids("uuid-42").await.unwrap().is_empty());
}

#[tokio::test]
async fn backfill_is_idempotent() {
    let server = MockServer::start().await;
    mount_campaign(&server).await;
    mount_lookup(&server, both_subscribers()).await;

    let dir = tempfile::tempdir().unwrap();

    let app = App::load(config(&server, &dir), true).await.unwrap();
    assert_eq!(execute(app, false, args(false)).await.unwrap(), 0);

    let app = App::load(config(&server, &dir), true).await.unwrap();
    assert_eq!(execute(app, false, args(false)).await.unwrap(), 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert_eq!(
        store.served_ids("uuid-42").await.unwrap(),
        HashSet::from([1, 2])
    );
}

#[tokio::test]
async fn backfill_refuses_unknown_subscribers() {
    let server = MockServer::start().await;
    mount_campaign(&server).await;
    mount_lookup(&server, vec![json!({"id": 1, "email": "a@example.com"})]).await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    let err = execute(app, false, args(false)).await.unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("not listmonk subscribers"));
    assert!(err.to_string().contains("subscribers:get_all"));

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert!(store.served_ids("uuid-42").await.unwrap().is_empty());
}

#[tokio::test]
async fn empty_email_source_is_a_noop_success() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    let path = dir.path().join("empty.txt");
    std::fs::write(&path, "# nothing to do\n\n").unwrap();

    let mut a = args(false);
    a.email.clear();
    a.emails = Some(path);

    assert_eq!(execute(app, false, a).await.unwrap(), 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert!(store.served_ids("uuid-42").await.unwrap().is_empty());
}

#[tokio::test]
async fn dry_run_listing_round_trips_into_backfill() {
    let server = MockServer::start().await;
    mount_campaign(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/subscribers"))
        .and(query_param("list_id", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "results": both_subscribers(),
                "total": 2,
                "page": 1,
                "per_page": 1000
            }
        })))
        .mount(&server)
        .await;
    mount_lookup(&server, both_subscribers()).await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    let campaign = app.api.campaign(CampaignId(42)).await.unwrap();
    let computed = cohort::compute(&app.api, &campaign, &[5], None, &HashSet::new())
        .await
        .unwrap();
    let listing = view::render_emails(computed.members.iter().map(|m| m.email.as_str()));
    assert_eq!(listing, "a@example.com\nb@example.com\n");

    let path = dir.path().join("cohort.txt");
    std::fs::write(&path, listing).unwrap();

    let mut a = args(false);
    a.email.clear();
    a.emails = Some(path);
    assert_eq!(execute(app, false, a).await.unwrap(), 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert_eq!(
        store.served_ids("uuid-42").await.unwrap(),
        HashSet::from([1, 2])
    );
}

#[tokio::test]
async fn backfill_refuses_while_a_cohort_is_open() {
    let server = MockServer::start().await;
    mount_campaign(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    let member = store::Member {
        subscriber_id: 1,
        email: "a@example.com".into(),
    };
    app.store
        .insert_cohort(store::NewCohort {
            campaign_id: CampaignId(42),
            campaign_uuid: "uuid-42",
            source_list_ids: "[5]",
            strategy: "since",
            since_at: "2026-10-02T10:00:00Z",
            query_text: "TRUE",
            members: &[member],
        })
        .await
        .unwrap();

    let err = execute(app, false, args(false)).await.unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("open cohort"));
}
