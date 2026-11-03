//! Personal access tokens: one statement reads the token and its owner;
//! `last_used_at` is written at most once a minute, and no statement is sent
//! when it was just written. Every refusal holds whatever `last_used_at` is.
//! All databases.

mod support;

use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use rustcms_auth::entities::personal_access_token as token;
use rustcms_auth::tokens;
use rustcms_core::models::user;
use rustcms_migration::Migrator;
use rustcms_testing::{TestDb, db_test};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

/// A connection to the same pool that records every statement sent.
fn recording(db: &DatabaseConnection) -> (DatabaseConnection, Arc<Mutex<Vec<String>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut conn = db.clone();
    let log = sent.clone();
    conn.set_metric_callback(move |info| log.lock().unwrap().push(info.statement.sql.clone()));
    (conn, sent)
}

async fn set_last_used(db: &DatabaseConnection, id: i64, at: Option<chrono::DateTime<Utc>>) {
    token::Entity::update_many()
        .col_expr(token::Column::LastUsedAt, Expr::value(at))
        .filter(token::Column::Id.eq(id))
        .exec(db)
        .await
        .unwrap();
}

async fn last_used(db: &DatabaseConnection, id: i64) -> Option<chrono::DateTime<Utc>> {
    token::Entity::find_by_id(id).one(db).await.unwrap().unwrap().last_used_at
}

db_test!(a_recent_use_sends_one_statement, migrate = Migrator, |db: TestDb| async move {
    let (app, seed) = support::setup(db).await;
    let db = app.state().db();
    let issued = tokens::issue(db, seed.reader.id, "t", &[], None).await.unwrap();
    let (conn, sent) = recording(db);

    // Never used: read, then recorded.
    let (found, owner) = tokens::authenticate(&conn, &issued.plain_text).await.unwrap();
    assert_eq!((found.id, owner.id), (issued.token.id, seed.reader.id));
    assert_eq!(sent.lock().unwrap().len(), 2, "{:?}", sent.lock().unwrap());
    assert!(sent.lock().unwrap()[1].to_uppercase().starts_with("UPDATE"));
    let recorded = last_used(db, issued.token.id).await.expect("recorded");

    // Used just now: the read alone.
    sent.lock().unwrap().clear();
    tokens::authenticate(&conn, &issued.plain_text).await.unwrap();
    let statements = sent.lock().unwrap().clone();
    assert_eq!(statements.len(), 1, "{statements:?}");
    assert!(statements[0].to_uppercase().starts_with("SELECT"), "{statements:?}");
    assert_eq!(last_used(db, issued.token.id).await, Some(recorded));

    // Used more than a minute ago: recorded again.
    set_last_used(db, issued.token.id, Some(Utc::now() - Duration::minutes(5))).await;
    sent.lock().unwrap().clear();
    tokens::authenticate(&conn, &issued.plain_text).await.unwrap();
    assert_eq!(sent.lock().unwrap().len(), 2);
    assert!(last_used(db, issued.token.id).await.unwrap() > Utc::now() - Duration::minutes(1));
});

db_test!(refusals_hold_whatever_the_last_use, migrate = Migrator, |db: TestDb| async move {
    let (app, seed) = support::setup(db).await;
    let db = app.state().db();
    let fresh = Some(Utc::now());
    let stale = Some(Utc::now() - Duration::minutes(5));
    for last in [None, fresh, stale] {
        // A wrong secret.
        let issued = tokens::issue(db, seed.reader.id, "t", &[], None).await.unwrap();
        set_last_used(db, issued.token.id, last).await;
        let (prefix, _) = issued.plain_text.rsplit_once('_').unwrap();
        let wrong = format!("{prefix}_{}", "a".repeat(40));
        assert!(tokens::authenticate(db, &wrong).await.is_err(), "wrong secret, {last:?}");
        assert!(tokens::authenticate(db, &issued.plain_text).await.is_ok(), "the right one, {last:?}");

        // Revoked.
        tokens::revoke(db, issued.token.id).await.unwrap();
        set_last_used(db, issued.token.id, last).await;
        assert!(tokens::authenticate(db, &issued.plain_text).await.is_err(), "revoked, {last:?}");

        // Expired.
        let expired =
            tokens::issue(db, seed.reader.id, "t", &[], Some(Utc::now() - Duration::seconds(1))).await.unwrap();
        set_last_used(db, expired.token.id, last).await;
        assert!(tokens::authenticate(db, &expired.plain_text).await.is_err(), "expired, {last:?}");

        // An inactive owner.
        let owner = rustcms_testing::Factory::create(&rustcms_testing::UserFactory::new(), db).await;
        let theirs = tokens::issue(db, owner.id, "t", &[], None).await.unwrap();
        set_last_used(db, theirs.token.id, last).await;
        user::Entity::update_many()
            .col_expr(user::Column::IsActive, Expr::value(false))
            .filter(user::Column::Id.eq(owner.id))
            .exec(db)
            .await
            .unwrap();
        assert!(tokens::authenticate(db, &theirs.plain_text).await.is_err(), "inactive owner, {last:?}");

        // A deleted owner (its tokens may outlive it where no foreign key cascades).
        let gone = rustcms_testing::Factory::create(&rustcms_testing::UserFactory::new(), db).await;
        let orphan = tokens::issue(db, gone.id, "t", &[], None).await.unwrap();
        set_last_used(db, orphan.token.id, last).await;
        let _ = user::Entity::delete_by_id(gone.id).exec(db).await;
        assert!(tokens::authenticate(db, &orphan.plain_text).await.is_err(), "deleted owner, {last:?}");
    }
});
