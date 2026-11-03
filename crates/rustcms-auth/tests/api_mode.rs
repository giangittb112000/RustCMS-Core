//! API mode (`app.mode = "api"`, ADR 0036): the API guard's routes work as in
//! `full`; nothing under the admin path answers but a 404, and no session is
//! loaded for it. All databases.

mod support;

use axum::http::{Method, StatusCode};
use rustcms_auth::{API_GUARD, Access, AdminAccess, AdminRoutes, ApiRoutes};
use rustcms_core::{AppBuilder, AppMode, Config};
use rustcms_migration::Migrator;
use rustcms_testing::{TestApp, TestDb, db_test};
use serde_json::json;
use support::{PASSWORD, Seed};

fn api_routes() -> ApiRoutes {
    ApiRoutes::new("/api/v2")
        .get("/open", Access::public(), || async { "open" })
        .get("/posts", Access::can("posts.view"), || async { "posts" })
        .post("/posts", Access::can("posts.create"), || async { "created" })
}

fn admin_routes() -> AdminRoutes {
    AdminRoutes::new("/tools").get("/me", AdminAccess::authenticated(), || async { "me" })
}

fn build(config: Config) -> AppBuilder {
    let mut app = support::build(config);
    api_routes().register(&mut app);
    admin_routes().register(&mut app);
    app
}

fn build_api(config: Config) -> AppBuilder {
    build(config.with("app.mode", AppMode::Api).unwrap())
}

async fn api_app(db: TestDb) -> (TestApp, Seed) {
    support::setup_with(build_api, db, &[]).await
}

/// Every path under the admin path, whatever the method and credentials: a
/// JSON 404 without a cookie.
const ADMIN_PATHS: &[&str] = &[
    "/admin",
    "/admin/",
    "/admin/login",
    "/admin/assets/index.js",
    "/admin/assets/brand/theme.css",
    "/admin/api/auth/csrf",
    "/admin/api/auth/login",
    "/admin/api/auth/me",
    "/admin/api/resources/posts/records",
    "/admin/api/ping",
    "/admin/api/x/tools/me",
    "/admin/api/x/tools/events",
    "/admin/api/nothing/here",
];

fn assert_absent(response: &rustcms_testing::TestResponse, what: &str) {
    assert_eq!(response.status, StatusCode::NOT_FOUND, "{what}");
    assert!(response.header("set-cookie").is_none(), "{what}: no cookie");
    assert_eq!(response.json()["code"], "not_found", "{what}");
}

db_test!(the_admin_path_is_absent, migrate = Migrator, |db: TestDb| async move {
    let (app, seed) = api_app(db).await;
    assert_eq!(app.state().mode(), AppMode::Api);
    let anonymous = app.client();
    let signed_in = app.acting_as(&seed.admin).await;
    let malformed = app.client().bearer("rcms_not-a-token");
    for path in ADMIN_PATHS {
        for method in [Method::GET, Method::POST, Method::DELETE] {
            for (who, client) in [("anonymous", &anonymous), ("session", &signed_in), ("bearer", &malformed)] {
                let response = client.send(method.clone(), path, Some(&json!({}))).await;
                assert_absent(&response, &format!("{method} {path} as {who}"));
            }
        }
    }
});

db_test!(no_session_is_loaded_under_the_admin_path, migrate = Migrator, |db: TestDb| async move {
    use sea_orm::ConnectionTrait;
    let (app, seed) = api_app(db).await;
    let signed_in = app.acting_as(&seed.admin).await;
    // Without its table, any session read would fail with a 500.
    app.state().db().execute_unprepared("ALTER TABLE sessions RENAME TO sessions_offline").await.unwrap();
    for path in ["/admin/api/auth/me", "/admin/api/x/tools/me", "/admin/"] {
        assert_absent(&signed_in.send(Method::GET, path, None).await, path);
    }
    // The API guard never needed it.
    let reader = app.with_token(&seed.reader, &["*"]).await;
    reader.send(Method::GET, "/api/v2/posts", None).await.assert_status(StatusCode::OK);
});

db_test!(the_api_guard_works_as_in_full_mode, migrate = Migrator, |db: TestDb| async move {
    let (app, seed) = api_app(db).await;
    let email = seed.reader.email.clone();
    let anonymous = app.client();
    anonymous.send(Method::GET, "/api/v2/open", None).await.assert_status(StatusCode::OK);
    anonymous.send(Method::GET, "/api/v2/posts", None).await.assert_status(StatusCode::UNAUTHORIZED);

    // Tokens: issued, used, refused without the permission, revoked.
    let issued = anonymous
        .send(
            Method::POST,
            "/api/v1/auth/token",
            Some(&json!({"email": email, "password": PASSWORD, "token_name": "cli"})),
        )
        .await;
    issued.assert_status(StatusCode::CREATED);
    let token = issued.json()["token"].as_str().unwrap().to_owned();
    let reader = app.client().bearer(&token);
    reader.send(Method::GET, "/api/v1/auth/me", None).await.assert_status(StatusCode::OK);
    reader.send(Method::GET, "/api/v2/posts", None).await.assert_status(StatusCode::OK);
    reader.send(Method::POST, "/api/v2/posts", None).await.assert_status(StatusCode::FORBIDDEN);
    reader.send(Method::DELETE, "/api/v1/auth/token", None).await.assert_status(StatusCode::NO_CONTENT);
    reader.send(Method::GET, "/api/v2/posts", None).await.assert_status(StatusCode::UNAUTHORIZED);

    // JWTs: signed in, used, refreshed, signed out.
    let signed =
        anonymous.send(Method::POST, "/api/v1/auth/jwt", Some(&json!({"email": email, "password": PASSWORD}))).await;
    signed.assert_status(StatusCode::CREATED);
    let body = signed.json();
    let access = body["accessToken"].as_str().unwrap();
    let jwt = app.client().bearer(access);
    jwt.send(Method::GET, "/api/v2/posts", None).await.assert_status(StatusCode::OK);
    let refreshed = anonymous
        .send(Method::POST, "/api/v1/auth/refresh", Some(&json!({"refreshToken": body["refreshToken"]})))
        .await;
    refreshed.assert_status(StatusCode::OK);
    let refresh = refreshed.json()["refreshToken"].clone();
    anonymous
        .send(Method::POST, "/api/v1/auth/jwt/logout", Some(&json!({"refreshToken": refresh})))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    jwt.send(Method::GET, "/api/v2/posts", None).await.assert_status(StatusCode::UNAUTHORIZED);
});

db_test!(routes_listed_are_the_routes_served, migrate = Migrator, |db: TestDb| async move {
    let (app, _) = api_app(db).await;
    let builder = build_api(app.state().config().clone());
    let routes: Vec<String> = builder.described_routes().into_iter().map(|r| r.path).collect();
    assert!(routes.iter().all(|p| !p.starts_with("/admin")), "{routes:?}");
    for path in ["/api/v1/auth/token", "/api/v1/auth/jwt", "/api/v2/posts", "/health"] {
        assert!(routes.iter().any(|p| p == path), "{path} in {routes:?}");
    }
    // The same declarations in `full`: the panel's routes are listed too.
    let full = build(app.state().config().clone().with("app.mode", AppMode::Full).unwrap());
    let routes: Vec<String> = full.described_routes().into_iter().map(|r| r.path).collect();
    assert!(routes.iter().any(|p| p == "/admin/api/auth/login") && routes.iter().any(|p| p == "/admin/api/x/tools/me"));
});

db_test!(permissions_are_the_same_in_both_modes, migrate = Migrator, |db: TestDb| async move {
    let (app, _) = api_app(db).await;
    let declared = |builder: AppBuilder| {
        let auth = builder.service::<rustcms_auth::AuthState>().unwrap();
        auth.permissions.all().into_keys().collect::<Vec<_>>()
    };
    let config = app.state().config().clone();
    let api = declared(build_api(config.clone()));
    let full = declared(build(config.with("app.mode", AppMode::Full).unwrap()));
    assert_eq!(api, full);
    assert!(api.iter().any(|(guard, name)| guard == API_GUARD && name == "posts.view"));
});
