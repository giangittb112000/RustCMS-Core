//! Staged files and storage cleanups (ADR 0031, amended for API mode): each
//! instance sweeps its own staging directory (`serve` in any mode,
//! `queue:work`, `media:prune`); queued media deletions and expired exports
//! are framework schedules, claimed once across instances. All databases.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use rustcms_core::schedule::Schedules;
use rustcms_core::{AppCommand, AppState};
use rustcms_testing::{TestApp, TestDb, db_test};

/// A file staged two hours ago (abandoned).
fn stale(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(7_200)).unwrap();
    path
}

/// An abandoned upload, export and import, and an upload in progress.
fn staged(state: &AppState) -> (Vec<PathBuf>, PathBuf) {
    let dir = rustcms_core::storage::of(state).unwrap().staging_dir();
    let abandoned = vec![
        stale(&dir, &format!("{}.upload", uuid::Uuid::now_v7())),
        stale(&dir, &format!("export-1-{}.csv", uuid::Uuid::now_v7())),
        stale(&dir, &format!("import-{}.csv", uuid::Uuid::now_v7())),
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
    for name in ["rustcms.media-prune", "rustcms.exports-prune"] {
        assert!(schedules.get(name).is_some(), "{name} is scheduled");
    }
    // Building the app (a command, a test) sweeps nothing: no boot loop.
    let files = staged(app.state());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(files.0.iter().all(|p| p.exists()));
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
