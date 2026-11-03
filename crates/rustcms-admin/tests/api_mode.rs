//! API mode (ADR 0036) with the admin panel registered: nothing of the panel
//! answers, while everything other instances rely on (permissions, job kinds,
//! schedules, commands) is declared as in `full`. All databases.

mod support;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use rustcms_core::models::schedule_state;
use rustcms_core::queue::Jobs;
use rustcms_core::schedule::Schedules;
use rustcms_core::{AppBuilder, AppMode, Config};
use rustcms_testing::{TestApp, TestDb, db_test};
use sea_orm::{EntityTrait, PaginatorTrait};

fn api(config: Config) -> Config {
    config.with("app.mode", AppMode::Api).unwrap()
}

fn build_api(config: Config) -> AppBuilder {
    support::app::build(api(config))
}

async fn api_app(db: TestDb) -> (TestApp, support::app::Users) {
    let config = rustcms_testing::test_config(db.url());
    support::app::setup_built(db, config, build_api).await
}

db_test!(nothing_of_the_panel_answers, |db: TestDb| async move {
    let (app, users) = api_app(db).await;
    let admin = app.acting_as(&users.admin).await;
    let paths = [
        "/admin".to_owned(),
        "/admin/".to_owned(),
        "/admin/posts".to_owned(),
        "/admin/assets/brand/theme.css".to_owned(),
        "/admin/assets/ext/test-widgets.js".to_owned(),
        app.admin_api("schema"),
        app.admin_api("resources/posts/records"),
        app.admin_api("resources/posts/export"),
        app.admin_api("media"),
        app.admin_api("dashboard"),
        app.admin_api("settings/general"),
        app.admin_api("queue/jobs"),
        app.admin_api("schedules"),
        app.admin_api("audit"),
        app.admin_api("auth/me"),
    ];
    for path in &paths {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            let response = admin.send(method.clone(), path, Some(&serde_json::json!({}))).await;
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{method} {path}");
            assert!(response.header("set-cookie").is_none(), "{method} {path}");
        }
    }
    let routes = build_api(app.state().config().clone()).described_routes();
    assert!(routes.iter().all(|r| !r.path.starts_with("/admin")), "{routes:?}");
});

db_test!(declarations_are_the_same_in_both_modes, |db: TestDb| async move {
    let (app, _) = api_app(db).await;
    let full = app.state().config().clone().with("app.mode", AppMode::Full).unwrap();
    let declared = |builder: AppBuilder| {
        let auth = builder.service::<rustcms_auth::AuthState>().unwrap();
        let schedules: Vec<String> = builder
            .service::<Schedules>()
            .map(|s| s.all().iter().map(|s| s.name().to_owned()).collect())
            .unwrap_or_default();
        let commands: Vec<String> = builder.commands().iter().map(|c| c.definition().get_name().to_owned()).collect();
        (auth.permissions.all(), schedules, commands)
    };
    // Built (services bound): the job kinds and schedules of the running apps.
    let (full_app, api_app) = (
        support::app::build(full.clone()).build_with_db(app.state().db().clone()).await.unwrap(),
        build_api(full.clone()).build_with_db(app.state().db().clone()).await.unwrap(),
    );
    let kinds = |state: &rustcms_core::AppState| state.service::<Jobs>().unwrap().kinds();
    assert_eq!(kinds(api_app.state()), kinds(full_app.state()));
    assert!(kinds(api_app.state()).iter().any(|k| k.contains("export")), "{:?}", kinds(api_app.state()));
    let names = |state: &rustcms_core::AppState| -> Vec<String> {
        state.service::<Schedules>().unwrap().all().iter().map(|s| s.name().to_owned()).collect()
    };
    assert_eq!(names(api_app.state()), names(full_app.state()));
    let (api_declared, full_declared) = (declared(build_api(full.clone())), declared(support::app::build(full)));
    assert_eq!(api_declared.0, full_declared.0, "permissions");
    assert_eq!(api_declared.2, full_declared.2, "commands");
});

/// Serves `config` on the test database for a moment; returns the schedule
/// state rows the scheduler created (one per schedule when it runs).
async fn scheduled_by_serving(app: &TestApp, config: Config) -> u64 {
    let served = support::app::build(config).build_with_db(app.state().db().clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(served.serve_on(listener, async move {
        let _ = stopped.await;
    }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();
    schedule_state::Entity::find().count(app.state().db()).await.unwrap()
}

db_test!(api_mode_serves_without_the_scheduler_unless_enabled, |db: TestDb| async move {
    let (app, _) = api_app(db).await;
    let config = app.state().config().clone().with("queue.enabled", false).unwrap();
    assert_eq!(scheduled_by_serving(&app, config.clone()).await, 0, "off by default in API mode");
    let enabled = config.with("schedule.enabled", true).unwrap();
    assert!(scheduled_by_serving(&app, enabled).await > 0, "on when set");
});

db_test!(serve_warns_about_what_several_instances_need, |db: TestDb| async move {
    let (app, _) = api_app(db).await;
    let warnings = rustcms_core::app::api_mode_warnings(app.state());
    let has = |text: &str| warnings.iter().any(|w| w.contains(text));
    assert!(has("Redis") && has("`local`"), "{warnings:?}");
    let sqlite = app.state().db().get_database_backend() == sea_orm::DbBackend::Sqlite;
    assert_eq!(has("SQLite"), sqlite, "{warnings:?}");
});
