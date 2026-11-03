//! Staged files and storage cleanups (ADR 0031, amended for API mode): each
//! instance sweeps its own staging directory (`serve` in any mode,
//! `queue:work`, `media:prune`); queued media deletions and expired exports
//! are framework schedules, claimed once across instances. All databases.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use rustcms_core::models::schedule_run;
use rustcms_core::schedule::{ScheduleRun, Schedules};
use rustcms_core::storage::StorageKey;
use rustcms_core::{AppCommand, AppState};
use rustcms_testing::{TestApp, TestDb, db_test};
use sea_orm::sea_query::{Alias, Query};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder};

/// A file staged two hours ago (abandoned).
fn stale(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(7_200)).unwrap();
    path
}

/// An abandoned upload, export, import and partial file, and an upload in progress.
fn staged(state: &AppState) -> (Vec<PathBuf>, PathBuf) {
    staged_in(rustcms_core::storage::of(state).unwrap().as_ref())
}

fn staged_in(storage: &dyn rustcms_core::storage::Storage) -> (Vec<PathBuf>, PathBuf) {
    let dir = storage.staging_dir();
    // A local disk's partial file, next to its objects (`yyyy/mm/`).
    let month = dir.parent().unwrap().join("2026").join("01");
    std::fs::create_dir_all(&month).unwrap();
    let abandoned = vec![
        stale(&dir, &format!("{}.upload", uuid::Uuid::now_v7())),
        stale(&dir, &format!("export-1-{}.csv", uuid::Uuid::now_v7())),
        stale(&dir, &format!("import-{}.csv", uuid::Uuid::now_v7())),
        stale(&month, &format!("{}.png.partial", uuid::Uuid::now_v7())),
    ];
    let recent = dir.join(format!("{}.upload", uuid::Uuid::now_v7()));
    std::fs::write(&recent, b"in progress").unwrap();
    (abandoned, recent)
}

fn assert_swept((abandoned, recent): &(Vec<PathBuf>, PathBuf)) {
    for path in abandoned {
        assert!(!path.exists(), "{} is abandoned", path.display());
    }
    assert!(recent.exists(), "files in progress stay");
}

async fn app(db: TestDb) -> TestApp {
    let config = rustcms_testing::test_config(db.url()).with("queue.enabled", false).unwrap();
    support::app::setup_with(db, config).await.0
}

db_test!(storage_cleanups_are_framework_schedules, |db: TestDb| async move {
    let app = app(db).await;
    let schedules = app.state().service::<Schedules>().unwrap();
    for name in [MEDIA, EXPORTS] {
        assert!(schedules.get(name).is_some(), "{name} is scheduled");
    }
    // Another instance booting (a command, a worker): files staged before
    // its boot stay, as nothing runs at boot.
    let other = support::app::build(app.state().config().clone());
    let storage = other.service::<std::sync::Arc<dyn rustcms_core::storage::Storage>>().unwrap();
    let files = staged_in(storage.as_ref().as_ref());
    let other = other.build_with_db(app.state().db().clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(files.0.iter().all(|p| p.exists()), "nothing swept at boot");
    drop(other);
});

const MEDIA: &str = "rustcms.media-prune.local";
const EXPORTS: &str = "rustcms.exports-prune.local";

async fn schedule_run(app: &TestApp, name: &str) -> anyhow::Result<()> {
    let run = ScheduleRun;
    run.run(app.state(), &run.definition().get_matches_from(["schedule:run", name])).await
}

async fn last_run(app: &TestApp, name: &str) -> schedule_run::Model {
    schedule_run::Entity::find()
        .filter(schedule_run::Column::Name.eq(name))
        .order_by_desc(schedule_run::Column::StartedAt)
        .one(app.state().db())
        .await
        .unwrap()
        .unwrap()
}

/// A stored object the application no longer references (an upload whose
/// row never committed), its intent two hours old.
async fn orphan(app: &TestApp) -> StorageKey {
    let storage = rustcms_core::storage::of(app.state()).unwrap();
    let key = StorageKey::generate("png").unwrap();
    let file = std::env::temp_dir().join(format!("orphan-{}", uuid::Uuid::now_v7()));
    std::fs::write(&file, b"x").unwrap();
    storage.put_file(&key, &file, "image/png").await.unwrap();
    std::fs::remove_file(&file).unwrap();
    let insert = Query::insert()
        .into_table(Alias::new("media_uploads"))
        .columns(["id", "disk", "key", "created_at"].map(Alias::new))
        .values_panic([
            uuid::Uuid::now_v7().into(),
            "local".into(),
            key.as_str().into(),
            (chrono::Utc::now() - chrono::Duration::hours(2)).into(),
        ])
        .to_owned();
    app.state().db().execute(&insert).await.unwrap();
    key
}

db_test!(the_media_schedule_deletes_abandoned_uploads, |db: TestDb| async move {
    let app = app(db).await;
    let key = orphan(&app).await;
    schedule_run(&app, MEDIA).await.unwrap();
    assert!(!rustcms_core::storage::of(app.state()).unwrap().exists(&key).await.unwrap());
    let run = last_run(&app, MEDIA).await;
    assert_eq!((run.status.as_str(), run.summary.as_deref()), ("succeeded", Some("1 media files deleted")));
});

db_test!(a_failed_exports_pass_does_not_stop_the_imports_pass, |db: TestDb| async move {
    use rustcms_admin::imports::entity;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    let app = app(db).await;
    let users = app.state().db();
    let owner = rustcms_testing::Factory::create(&rustcms_testing::UserFactory::new(), users).await;
    // An expired import and its file.
    let storage = rustcms_core::storage::of(app.state()).unwrap();
    let key = StorageKey::generate("csv").unwrap();
    let file = std::env::temp_dir().join(format!("import-{}", uuid::Uuid::now_v7()));
    std::fs::write(&file, b"title\r\nA\r\n").unwrap();
    storage.put_file(&key, &file, "text/csv").await.unwrap();
    std::fs::remove_file(&file).unwrap();
    let now = chrono::Utc::now();
    let id = uuid::Uuid::now_v7();
    entity::ActiveModel {
        id: Set(id),
        user_id: Set(owner.id),
        auth_version: Set(owner.auth_version),
        resource: Set("notes".into()),
        disk: Set("local".into()),
        storage_key: Set(key.as_str().to_owned()),
        sha256: Set("0".repeat(64)),
        header: Set(serde_json::json!(["title"])),
        rows: Set(1),
        mapping: Set(None),
        status: Set("done".into()),
        created: Set(1),
        updated: Set(0),
        failed: Set(0),
        report: Set(None),
        error: Set(None),
        created_at: Set(now - chrono::Duration::days(2)),
        started_at: Set(Some(now - chrono::Duration::days(2))),
        expires_at: Set(now - chrono::Duration::hours(1)),
    }
    .insert(users)
    .await
    .unwrap();
    // The exports pass fails: its table is gone.
    users.execute_unprepared("ALTER TABLE exports RENAME TO exports_offline").await.unwrap();
    let error = schedule_run(&app, EXPORTS).await.unwrap_err();
    assert!(format!("{error:#}").contains("prune_failed"), "{error:#}");
    assert!(entity::Entity::find_by_id(id).one(users).await.unwrap().is_none(), "the imports pass ran");
    assert!(!storage.exists(&key).await.unwrap());
    let run = last_run(&app, EXPORTS).await;
    assert_eq!(run.status, "failed");
});

db_test!(queue_work_once_and_media_prune_sweep_this_instance, |db: TestDb| async move {
    let app = app(db).await;
    let files = staged(app.state());
    let work = rustcms_core::queue::QueueWork;
    work.run(app.state(), &work.definition().get_matches_from(["queue:work", "--once"])).await.unwrap();
    assert_swept(&files);

    let files = staged(app.state());
    let commands = support::app::build(app.state().config().clone()).commands();
    let prune = commands.iter().find(|c| c.definition().get_name() == "media:prune").expect("media:prune");
    prune.run(app.state(), &prune.definition().get_matches_from(["media:prune"])).await.unwrap();
    assert_swept(&files);
});

db_test!(serve_sweeps_without_workers_or_scheduler, |db: TestDb| async move {
    let app = app(db).await;
    let config = app
        .state()
        .config()
        .clone()
        .with("schedule.enabled", false)
        .and_then(|c| c.with("queue.enabled", false))
        .unwrap();
    // Another instance on the same database (its own storage directory).
    let served = support::app::build(config).build_with_db(app.state().db().clone()).await.unwrap();
    let files = staged(served.state());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(served.serve_on(listener, async move {
        let _ = stopped.await;
    }));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while files.0.iter().any(|p| p.exists()) {
        assert!(tokio::time::Instant::now() < deadline, "serve did not sweep");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_swept(&files);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();
});
