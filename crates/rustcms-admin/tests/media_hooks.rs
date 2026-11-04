//! Media hooks (`AdminProvider::media_hook`): run in the transaction of each
//! change to a media record; an error rolls the change back. All databases.

mod support;

use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use base64::Engine;
use rustcms_admin::media::{MediaChange, MediaEvent, MediaHook, entity as media};
use rustcms_admin::{AdminProvider, Ctx};
use rustcms_core::{AppBuilder, AppResult, Config, ValidationErrors};
use rustcms_testing::{TestApp, TestClient, TestDb, TestResponse, db_test};
use sea_orm::{DatabaseTransaction, EntityTrait};
use serde_json::json;

/// What the hook saw: (media id, event, name, alt, previous alt).
type Seen = (String, MediaEvent, String, Option<String>, Option<Option<String>>);

static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

/// Records every change; refuses alternative text `boom` (rolls back).
struct Recorder;

#[async_trait::async_trait]
impl MediaHook for Recorder {
    async fn changed(&self, _ctx: &Ctx, _txn: &DatabaseTransaction, change: &MediaChange<'_>) -> AppResult<()> {
        if change.media.alt.as_deref() == Some("boom") {
            return Err(ValidationErrors::field("alt", "is refused by the application").into());
        }
        SEEN.lock().unwrap().push((
            change.media.id.to_string(),
            change.event,
            change.media.name.clone(),
            change.media.alt.clone(),
            change.previous.map(|p| p.alt.clone()),
        ));
        Ok(())
    }
}

fn with_hook(admin: AdminProvider) -> AdminProvider {
    admin.media_hook(Recorder)
}

fn build(config: Config) -> AppBuilder {
    support::app::build_extended(config, with_hook)
}

/// What the hook saw for media `id`, in order.
fn seen(id: &str) -> Vec<Seen> {
    SEEN.lock().unwrap().iter().filter(|s| s.0 == id).cloned().collect()
}

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=")
        .unwrap()
}

async fn upload(app: &TestApp, client: &TestClient<'_>, name: &str) -> TestResponse {
    client.fetch_csrf().await;
    let boundary = "rustcms-test-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&png());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let request = Request::builder()
        .method(Method::POST)
        .uri(app.admin_api("media"))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .header("content-length", body.len())
        .body(Body::from(body))
        .unwrap();
    client.request(request).await
}

db_test!(hooks_see_every_change_in_its_transaction, |db: TestDb| async move {
    let config = rustcms_testing::test_config(db.url());
    let (app, users) = support::app::setup_built(db, config, build).await;
    let admin = app.acting_as(&users.admin).await;
    let created = upload(&app, &admin, "cat.png").await;
    created.assert_status(StatusCode::CREATED);
    let id = created.json()["data"]["id"].as_str().unwrap().to_owned();
    let record = app.admin_api(&format!("resources/media/records/{id}"));

    // Alternative text: the hook sees the record as it will be stored, and as it was.
    admin.send(Method::PATCH, &record, Some(&json!({"alt": "A cat"}))).await.assert_status(StatusCode::OK);
    // Refused by the hook: nothing changes.
    let refused = admin.send(Method::PATCH, &record, Some(&json!({"alt": "boom"}))).await;
    refused.assert_validation_error("alt");
    let stored = media::Entity::find_by_id(id.parse::<uuid::Uuid>().unwrap()).one(app.state().db()).await.unwrap();
    assert_eq!(stored.unwrap().alt.as_deref(), Some("A cat"), "rolled back");

    admin.send(Method::DELETE, &record, None).await.assert_status(StatusCode::NO_CONTENT);
    admin.send(Method::POST, &format!("{record}/restore"), None).await.assert_status(StatusCode::OK);
    admin.send(Method::DELETE, &record, None).await.assert_status(StatusCode::NO_CONTENT);
    admin.send(Method::DELETE, &format!("{record}/force"), None).await.assert_status(StatusCode::NO_CONTENT);

    let events: Vec<(MediaEvent, Option<String>, Option<Option<String>>)> =
        seen(&id).into_iter().map(|(_, event, _, alt, previous)| (event, alt, previous)).collect();
    assert_eq!(
        events,
        [
            (MediaEvent::Uploaded, None, None),
            (MediaEvent::Updated, Some("A cat".into()), Some(None)),
            (MediaEvent::Trashed, Some("A cat".into()), None),
            (MediaEvent::Restored, Some("A cat".into()), None),
            (MediaEvent::Trashed, Some("A cat".into()), None),
            (MediaEvent::Deleted, Some("A cat".into()), None),
        ]
    );
});

db_test!(a_failing_hook_undoes_the_upload, |db: TestDb| async move {
    /// Refuses every upload.
    struct NoUploads;
    #[async_trait::async_trait]
    impl MediaHook for NoUploads {
        async fn changed(&self, _: &Ctx, _: &DatabaseTransaction, change: &MediaChange<'_>) -> AppResult<()> {
            if change.event == MediaEvent::Uploaded {
                return Err(ValidationErrors::field("file", "uploads are closed").into());
            }
            Ok(())
        }
    }
    let refusing = |config: Config| support::app::build_extended(config, |admin| admin.media_hook(NoUploads));
    let config = rustcms_testing::test_config(db.url());
    let (app, users) = support::app::setup_built(db, config, refusing).await;
    let admin = app.acting_as(&users.admin).await;
    upload(&app, &admin, "cat.png").await.assert_validation_error("file");
    assert!(media::Entity::find().all(app.state().db()).await.unwrap().is_empty(), "no row");
});
