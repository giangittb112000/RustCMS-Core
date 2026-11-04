//! Several application instances on one database: what one commits, the
//! others see on their next request. All databases.

mod support;

use rustcms_auth::{API_GUARD, Actor, RbacService};
use rustcms_migration::Migrator;
use rustcms_testing::{TestApp, TestDb, db_test};
use support::{auth, build, principal};

/// Without a generation cache (`permission_generation_cache_ms = 0`).
fn uncached(config: rustcms_core::Config) -> rustcms_core::AppBuilder {
    build(config.with("auth.permission_generation_cache_ms", 0).unwrap())
}

db_test!(rbac_changes_reach_every_instance_at_once, migrate = Migrator, |db: TestDb| async move {
    let pool = db.reopen().await;
    let a = TestApp::new(uncached, db).await;
    let seed = support::seed_app(&a).await;
    // A second instance: same configuration (same APP_KEY), its own pool and caches.
    let b = TestApp::with_config(build, a.state().config().clone(), pool).await;
    assert!(principal(&b, API_GUARD, &seed.reader).await.permissions.contains("posts.view"), "cached on B");

    // Instance A takes the permission away.
    let (state_a, state_b) = (auth(&a), auth(&b));
    let rbac = RbacService::new(a.state().db(), &state_a.resolver);
    rbac.set_role_permissions(Actor::System, seed.reader_role.id, &[]).await.unwrap();

    // B's cache entry is from an older generation: never used again.
    assert!(!principal(&b, API_GUARD, &seed.reader).await.permissions.contains("posts.view"));
    // And B's own changes reach A the same way.
    let warm = principal(&a, API_GUARD, &seed.reader).await;
    assert!(warm.permissions.is_empty());
    RbacService::new(b.state().db(), &state_b.resolver)
        .set_role_permissions(Actor::System, seed.reader_role.id, &["posts.view".to_owned()])
        .await
        .unwrap();
    assert!(principal(&a, API_GUARD, &seed.reader).await.permissions.contains("posts.view"));
});

db_test!(
    rbac_changes_reach_other_instances_within_the_generation_period,
    migrate = Migrator,
    |db: TestDb| async move {
        let pool = db.reopen().await;
        let cached =
            |config: rustcms_core::Config| build(config.with("auth.permission_generation_cache_ms", 300).unwrap());
        let a = TestApp::new(cached, db).await;
        let seed = support::seed_app(&a).await;
        let b = TestApp::with_config(build, a.state().config().clone(), pool).await;
        assert!(principal(&b, API_GUARD, &seed.reader).await.permissions.contains("posts.view"));

        // A takes the permission away: A sees it on its next request (its own
        // generation moved), B once its period has passed.
        let state_a = auth(&a);
        RbacService::new(a.state().db(), &state_a.resolver)
            .set_role_permissions(Actor::System, seed.reader_role.id, &[])
            .await
            .unwrap();
        assert!(principal(&a, API_GUARD, &seed.reader).await.permissions.is_empty(), "at once where it was made");
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(!principal(&b, API_GUARD, &seed.reader).await.permissions.contains("posts.view"), "after the period");
    }
);
