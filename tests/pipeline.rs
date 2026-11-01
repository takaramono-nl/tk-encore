use std::time::Duration;

use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use encore_listmonk::app::App;
use encore_listmonk::cli::run::{RunArgs, execute};
use encore_listmonk::config::{AuthMode, Config};
use encore_listmonk::store;

fn source_campaign() -> serde_json::Value {
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

fn campaign(id: i64, status: &str, sent: i64, to_send: i64) -> serde_json::Value {
    json!({
        "data": {
            "id": id,
            "uuid": format!("uuid-{id}"),
            "name": "October Dispatch (resend #1)",
            "subject": "Hello",
            "from_email": "noreply@example.com",
            "body": "hello",
            "content_type": "richtext",
            "template_id": null,
            "messenger": "email",
            "type": "regular",
            "status": status,
            "tags": ["encore:resend", "encore-cohort-1"],
            "lists": [{"id": 12, "name": "encore/campaign-42/cohort-1"}],
            "to_send": to_send,
            "sent": sent
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

fn args(campaign_id: i64, min_new: i64) -> RunArgs {
    RunArgs {
        campaign: campaign_id,
        min_new,
        dry_run: false,
        print_emails: false,
        yes: true,
        since: None,
        lists: vec![],
        resume: false,
        abandon: false,
    }
}

#[tokio::test]
async fn run_sends_closes_and_records_the_ledger() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/campaigns/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(source_campaign()))
        .mount(&server)
        .await;

    // Two post-cutoff subscribers.
    Mock::given(method("GET"))
        .and(path("/api/subscribers"))
        .and(query_param("list_id", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "results": [
                    {"id": 1, "email": "a@example.com"},
                    {"id": 2, "email": "b@example.com"}
                ],
                "total": 2,
                "page": 1,
                "per_page": 1000
            }
        })))
        .mount(&server)
        .await;

    // No pending list exists yet.
    Mock::given(method("GET"))
        .and(path("/api/lists"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"results": [], "total": 0}})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 12,
                "name": "encore/campaign-42/cohort-1",
                "type": "private",
                "optin": "single",
                "status": "active"
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/api/subscribers/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": true})))
        .mount(&server)
        .await;

    // Materialize reads live membership instead of the cached list column.
    Mock::given(method("GET"))
        .and(path("/api/subscribers"))
        .and(query_param("list_id", "12"))
        .and(query_param("per_page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "results": [{"id": 1, "email": "a@example.com"}],
                "total": 2,
                "page": 1,
                "per_page": 1
            }
        })))
        .mount(&server)
        .await;

    // No campaign with this cohort tag exists yet.
    Mock::given(method("GET"))
        .and(path("/api/campaigns"))
        .and(query_param("tags", "encore-cohort-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"results": [], "total": 0}})),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/campaigns"))
        .respond_with(ResponseTemplate::new(200).set_body_json(campaign(19, "draft", 0, 0)))
        .mount(&server)
        .await;

    // start() reads draft once, then wait() and close() read finished.
    Mock::given(method("GET"))
        .and(path("/api/campaigns/19"))
        .respond_with(ResponseTemplate::new(200).set_body_json(campaign(19, "draft", 0, 0)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/campaigns/19"))
        .respond_with(ResponseTemplate::new(200).set_body_json(campaign(19, "finished", 2, 2)))
        .with_priority(2)
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/api/campaigns/19/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(campaign(19, "running", 0, 2)))
        .mount(&server)
        .await;

    // close() archives the pending list.
    Mock::given(method("GET"))
        .and(path("/api/lists/12"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 12,
                "name": "encore/campaign-42/cohort-1",
                "type": "private",
                "optin": "single",
                "status": "active"
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/api/lists/12"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 12,
                "name": "encore/campaign-42/cohort-1",
                "type": "private",
                "optin": "single",
                "status": "archived"
            }
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    let code = execute(app, false, args(42, 1)).await.unwrap();
    assert_eq!(code, 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    let cohorts = store.cohorts().await.unwrap();
    assert_eq!(cohorts.len(), 1);
    let cohort = &cohorts[0];
    assert_eq!(cohort.state, store::state::CLOSED);
    assert_eq!(cohort.pending_list_id, Some(12));
    assert_eq!(cohort.resend_campaign_id, Some(19));
    assert_eq!(cohort.expected_count, 2);

    let served = store
        .exclusions("uuid-42", encore_listmonk::model::CampaignId(42), None)
        .await
        .unwrap();
    assert_eq!(served.len(), 2);
    assert!(served.contains(&1) && served.contains(&2));

    let requests = server.received_requests().await.unwrap();
    let create = requests
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path() == "/api/campaigns")
        .expect("campaign was created");
    let body: serde_json::Value = create.body_json().unwrap();
    assert_eq!(body["lists"], json!([12]));
    assert_eq!(body["body"], "hello");
    assert!(body.get("archive").is_none());
    assert!(body.get("send_at").is_none());
    assert!(body.get("uuid").is_none());
    assert!(
        body["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "encore-cohort-1")
    );

    assert!(
        requests
            .iter()
            .any(|r| r.method.as_str() == "PUT" && r.url.path() == "/api/lists/12"),
        "pending list was archived"
    );
    let add = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "PUT"
                && r.url.path() == "/api/subscribers/lists"
                && r.body_json::<serde_json::Value>()
                    .map(|b| b["action"] == "add")
                    .unwrap_or(false)
        })
        .expect("members were added");
    let add_body: serde_json::Value = add.body_json().unwrap();
    assert_eq!(add_body["status"], "confirmed");
    assert_eq!(add_body["target_list_ids"], json!([12]));
}

#[tokio::test]
async fn open_cohort_requires_resume_or_abandon() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/campaigns/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(source_campaign()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    let members = vec![store::Member {
        subscriber_id: 1,
        email: "a@example.com".into(),
    }];
    let cohort_id = app
        .store
        .insert_cohort(store::NewCohort {
            campaign_id: encore_listmonk::model::CampaignId(42),
            campaign_uuid: "uuid-42",
            source_list_ids: "[5]",
            strategy: "since",
            since_at: "2026-10-02T10:00:00Z",
            query_text: "TRUE",
            members: &members,
        })
        .await
        .unwrap();

    let err = execute(app, false, args(42, 1)).await.unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("--resume"));

    let app = App::load(config(&server, &dir), true).await.unwrap();
    let mut abandon = args(42, 1);
    abandon.abandon = true;
    assert_eq!(execute(app, false, abandon).await.unwrap(), 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    let cohort = store.cohort(cohort_id).await.unwrap().unwrap();
    assert_eq!(cohort.state, store::state::CANCELLED);
}

#[tokio::test]
async fn below_threshold_creates_no_cohort() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/campaigns/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(source_campaign()))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/subscribers"))
        .and(query_param("list_id", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "results": [{"id": 1, "email": "a@example.com"}],
                "total": 1,
                "page": 1,
                "per_page": 1000
            }
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), true).await.unwrap();

    let code = execute(app, false, args(42, 5)).await.unwrap();
    assert_eq!(code, 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert!(store.cohorts().await.unwrap().is_empty());

    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.method.as_str() == "POST" && r.url.path() == "/api/campaigns"),
        "no campaign should be created below the threshold"
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.method.as_str() == "POST" && r.url.path() == "/api/lists"),
        "no pending list should be created below the threshold"
    );
}

#[tokio::test]
async fn dry_run_print_emails_list_without_writing() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/campaigns/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(source_campaign()))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/subscribers"))
        .and(query_param("list_id", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "results": [
                    {"id": 1, "email": "a@example.com"},
                    {"id": 2, "email": "b@example.com"}
                ],
                "total": 2,
                "page": 1,
                "per_page": 1000
            }
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let app = App::load(config(&server, &dir), false).await.unwrap();

    let mut listing = args(42, 1);
    listing.dry_run = true;
    listing.print_emails = true;
    assert_eq!(execute(app, false, listing).await.unwrap(), 0);

    let store = store::Store::open(&dir.path().join("encore.db"))
        .await
        .unwrap();
    assert!(store.cohorts().await.unwrap().is_empty());

    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "a listing must not create lists or campaigns"
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.method.as_str() == "PUT" && r.url.path() == "/api/subscribers/lists"),
        "a listing must not add members to a list"
    );
}
