//! A column that may not be null, left out of a create (the field is not
//! declared required and has no default): the field's "is required" (422),
//! never a server error; a column outside the fields: a form error. SQLite,
//! PostgreSQL, MySQL.

use axum::http::StatusCode;
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, Config};
use rustcms_testing::{Factory, TestApp, TestDb, UserFactory, db_test};
use sea_orm::Schema;
use serde_json::json;

pub mod note {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "strict_notes")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub title: String,
        /// Not null, no default, not declared required.
        pub pinned: bool,
        /// Not null, no default, not a field of the resource.
        pub internal: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub struct NoteResource;

#[async_trait::async_trait]
impl Resource for NoteResource {
    type Entity = note::Entity;
    const SLUG: &'static str = "strict_notes";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Note").title("title")
    }

    fn fields() -> Vec<Field> {
        vec![Field::column(note::Column::Title).required(), Field::column(note::Column::Pinned)]
    }

    /// Notes titled "complete" get the internal column from the hook.
    async fn mutate_before_create(
        _ctx: &Ctx,
        _txn: &sea_orm::DatabaseTransaction,
        mut data: Data,
    ) -> rustcms_core::AppResult<Data> {
        if data.get("title") == Some(&json!("complete")) {
            data.insert("internal".into(), json!("set"));
        }
        Ok(data)
    }
}

fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    app.provider(AuthProvider::new());
    app.provider(AdminProvider::new().resource::<NoteResource>());
    app
}

db_test!(a_column_left_null_is_a_validation_error, |db: TestDb| async move {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(note::Entity)).await;
    let app = TestApp::new(build, db).await;
    let auth = app.state().service::<AuthState>().unwrap();
    let conn = app.state().db();
    let rbac = RbacService::new(conn, &auth.resolver);
    rbac.sync_permissions(&auth.permissions.all()).await.unwrap();
    let admin = UserFactory::new().name("Admin").create(conn).await;
    let super_admin = rustcms_auth::entities::role::super_admin_id(conn, ADMIN_GUARD).await.unwrap().unwrap();
    rbac.assign_role(Actor::System, admin.id, super_admin).await.unwrap();
    let client = app.acting_as(&admin).await;
    let url = app.admin_api("resources/strict_notes/records");

    // `pinned` left out: its "is required", not a 500.
    let missing = client.post_json(&url, &json!({"title": "complete"})).await;
    missing.assert_validation_error("pinned");
    // A column outside the fields left out: a form error (422).
    let incomplete = client.post_json(&url, &json!({"title": "other", "pinned": true})).await;
    incomplete.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert!(incomplete.json()["issues"].get("internal").is_none(), "nothing named after a column outside the fields");
    // Everything set: created.
    client.post_json(&url, &json!({"title": "complete", "pinned": false})).await.assert_status(StatusCode::CREATED);
});
