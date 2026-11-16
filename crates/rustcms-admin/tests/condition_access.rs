//! Conditions (ADR 0037) are evaluated on what the user's form holds, never
//! on values the user may not see: a field the user may not read, a record
//! the user may not view (blind writes). The outcome of a write (stored,
//! dropped, required, cleared) therefore tells nothing about them. The key a
//! relation binds is never dropped by a condition. SQLite, PostgreSQL, MySQL.

use axum::http::{Method, StatusCode};
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, Config};
use rustcms_testing::{Factory, TestApp, TestClient, TestDb, TestResponse, UserFactory, db_test};
use sea_orm::{EntityTrait, Schema};
use serde_json::{Value, json};

pub mod team {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "teams")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod staff {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "staff")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub team_id: Option<i32>,
        /// Readable by super admins only.
        pub band: Option<String>,
        pub level: Option<String>,
        /// Shown when `band` is "C", cleared when hidden.
        pub bonus: Option<String>,
        /// Shown when `band` is "C", required when shown.
        pub perk: Option<String>,
        /// Shown when `level` is "high", cleared when hidden.
        pub tip: Option<String>,
        /// Shown when `team_id` is filled.
        pub seat: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub struct TeamResource;

impl Resource for TeamResource {
    type Entity = team::Entity;
    const SLUG: &'static str = "teams";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Team").title("name")
    }

    fn fields() -> Vec<Field> {
        vec![Field::column(team::Column::Name).required()]
    }

    fn relations() -> Vec<Relation> {
        vec![Relation::has_many("staff", "staff", "team_id")]
    }
}

pub struct StaffResource;

impl Resource for StaffResource {
    type Entity = staff::Entity;
    const SLUG: &'static str = "staff";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Staff").title("name")
    }

    fn fields() -> Vec<Field> {
        use staff::Column as C;
        vec![
            Field::column(C::Name).required(),
            Field::column(C::TeamId).belongs_to("teams"),
            Field::column(C::Band).readable(|ctx| ctx.principal.super_admin),
            Field::column(C::Level),
            Field::column(C::Bonus),
            Field::column(C::Perk).required(),
            Field::column(C::Tip),
            Field::column(C::Seat),
        ]
    }

    fn form() -> Form {
        Form::new().schema(components![
            TextInput::make("name"),
            // The team: hidden unless a level is set (a relation still binds it).
            SelectInput::make("team_id").shown_when(when("level").filled()),
            TextInput::make("band"),
            TextInput::make("level"),
            TextInput::make("bonus").shown_when(when("band").eq("C")).clear_when_hidden(),
            TextInput::make("perk").shown_when(when("band").eq("C")),
            TextInput::make("tip").shown_when(when("level").eq("high")).clear_when_hidden(),
            TextInput::make("seat").shown_when(when("team_id").filled()),
        ])
    }
}

fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    app.provider(AuthProvider::new());
    app.provider(AdminProvider::new().resource::<TeamResource>().resource::<StaffResource>());
    app
}

struct Users {
    admin: rustcms_core::models::user::Model,
    /// Views and edits staff, may not read `band`.
    editor: rustcms_core::models::user::Model,
    /// Edits staff without viewing them (blind writes).
    blind: rustcms_core::models::user::Model,
}

async fn setup(db: TestDb) -> (TestApp, Users) {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(team::Entity)).await;
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(staff::Entity)).await;
    let app = TestApp::new(build, db).await;
    let auth = app.state().service::<AuthState>().unwrap();
    let db = app.state().db();
    let rbac = RbacService::new(db, &auth.resolver);
    rbac.sync_permissions(&auth.permissions.all()).await.unwrap();
    let admin = UserFactory::new().name("Admin").create(db).await;
    let super_admin = rustcms_auth::entities::role::super_admin_id(db, ADMIN_GUARD).await.unwrap().unwrap();
    rbac.assign_role(Actor::System, admin.id, super_admin).await.unwrap();
    let role = |name: &'static str, abilities: &'static [&'static str]| {
        let rbac = &rbac;
        async move {
            let mut permissions: Vec<String> = abilities.iter().map(|a| format!("staff.{a}")).collect();
            permissions.extend(["teams.view_any", "teams.view"].map(String::from));
            let user = UserFactory::new().name(name).create(db).await;
            let role = rbac.create_role(Actor::System, ADMIN_GUARD, &name.to_lowercase(), &permissions).await.unwrap();
            rbac.assign_role(Actor::System, user.id, role.id).await.unwrap();
            user
        }
    };
    let editor = role("Editor", &["view_any", "view", "create", "update"]).await;
    let blind = role("Blind", &["view_any", "update"]).await;
    (app, Users { admin, editor, blind })
}

async fn create(client: &TestClient<'_>, app: &TestApp, body: Value) -> String {
    let response = client.post_json(&app.admin_api("resources/staff/records"), &body).await;
    response.assert_status(StatusCode::CREATED);
    response.json()["data"]["key"].as_str().unwrap().to_owned()
}

async fn update(client: &TestClient<'_>, app: &TestApp, key: &str, body: Value) -> TestResponse {
    client.send(Method::PATCH, &app.admin_api(&format!("resources/staff/records/{key}")), Some(&body)).await
}

async fn stored(app: &TestApp, key: &str) -> staff::Model {
    staff::Entity::find_by_id(key.parse::<i32>().unwrap()).one(app.state().db()).await.unwrap().unwrap()
}

db_test!(an_unreadable_field_read_by_a_condition_tells_nothing, |db: TestDb| async move {
    let (app, users) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    // Band C shows `bonus` and `perk` to whoever reads the band; band A hides them.
    let c = create(&admin, &app, json!({"name": "c", "band": "C", "bonus": "b", "perk": "p"})).await;
    let a = create(&admin, &app, json!({"name": "a", "band": "A"})).await;
    sqlx_null_perk(&app, &c).await;

    let editor = app.acting_as(&users.editor).await;
    for key in [&c, &a] {
        // A value for `bonus`: hidden for the editor (the band reads as null), dropped
        // the same way whatever the band is.
        update(&editor, &app, key, json!({"bonus": "sent"})).await.assert_status(StatusCode::OK);
        // An unrelated edit: neither "required" (perk) nor clearing (bonus) depends
        // on the band.
        update(&editor, &app, key, json!({"name": format!("{key}!")})).await.assert_status(StatusCode::OK);
    }
    assert_eq!(stored(&app, &c).await.bonus.as_deref(), Some("b"), "kept, not cleared");
    assert_eq!(stored(&app, &a).await.bonus, None);

    // The super admin sees the band: on C, `bonus` is shown and stored.
    update(&admin, &app, &c, json!({"bonus": "admin"})).await.assert_status(StatusCode::OK);
    assert_eq!(stored(&app, &c).await.bonus.as_deref(), Some("admin"));
    // On C with no perk, an unrelated edit is fine (perk was shown before);
    // showing it by this write makes it required.
    update(&admin, &app, &c, json!({"name": "c2"})).await.assert_status(StatusCode::OK);
    update(&admin, &app, &a, json!({"band": "C"})).await.assert_validation_error("perk");
    // Hiding `bonus` by this write clears it.
    update(&admin, &app, &c, json!({"band": "B"})).await.assert_status(StatusCode::OK);
    assert_eq!(stored(&app, &c).await.bonus, None);
});

/// Leaves `perk` null on record `key` (as a record created before `perk` was
/// required would be).
async fn sqlx_null_perk(app: &TestApp, key: &str) {
    use sea_orm::{ActiveModelTrait, IntoActiveModel, Set};
    let mut model = stored(app, key).await.into_active_model();
    model.perk = Set(None);
    model.update(app.state().db()).await.unwrap();
}

db_test!(a_blind_write_reads_only_what_it_sends, |db: TestDb| async move {
    let (app, users) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let high = create(&admin, &app, json!({"name": "h", "level": "high", "tip": "t", "perk": "p"})).await;
    let low = create(&admin, &app, json!({"name": "l", "level": "low", "perk": "p"})).await;

    let blind = app.acting_as(&users.blind).await;
    for key in [&high, &low] {
        // The stored level is not the blind writer's: `tip` is hidden either way
        // (dropped, and not cleared since this write does not hide it).
        update(&blind, &app, key, json!({"tip": "x"})).await.assert_status(StatusCode::OK);
    }
    assert_eq!(stored(&app, &high).await.tip.as_deref(), Some("t"));
    assert_eq!(stored(&app, &low).await.tip, None);
    // What it sends counts: with the level sent, `tip` is shown and stored.
    update(&blind, &app, &low, json!({"level": "high", "tip": "y"})).await.assert_status(StatusCode::OK);
    assert_eq!(stored(&app, &low).await.tip.as_deref(), Some("y"));
});

db_test!(a_key_bound_by_a_relation_is_never_dropped, |db: TestDb| async move {
    let (app, users) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let team = admin.post_json(&app.admin_api("resources/teams/records"), &json!({"name": "T"})).await;
    team.assert_status(StatusCode::CREATED);
    let team = team.json()["data"]["key"].as_str().unwrap().to_owned();
    let editor = app.acting_as(&users.editor).await;
    let through = app.admin_api(&format!("resources/teams/records/{team}/relations/staff"));
    // `team_id` is hidden (no level), yet the relation binds it: created, in the team.
    let created = editor.post_json(&through, &json!({"name": "n", "seat": "s"})).await;
    created.assert_status(StatusCode::CREATED);
    let key = created.json()["data"]["key"].as_str().unwrap().to_owned();
    let row = stored(&app, &key).await;
    assert_eq!(row.team_id, Some(team.parse().unwrap()));
    // A condition reading the bound key: hidden by its own condition, it reads
    // as null (as everywhere), so `seat` is hidden and dropped.
    assert_eq!(row.seat, None);
    // With a level, the bound key is shown and `seat` with it.
    let shown = editor.post_json(&through, &json!({"name": "m", "level": "x", "seat": "s"})).await;
    shown.assert_status(StatusCode::CREATED);
    let key = shown.json()["data"]["key"].as_str().unwrap().to_owned();
    assert_eq!(stored(&app, &key).await.seat.as_deref(), Some("s"));
});

db_test!(a_value_sent_as_null_is_checked_when_shown_only, |db: TestDb| async move {
    let (app, users) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let c = create(&admin, &app, json!({"name": "c", "band": "C", "perk": "p", "bonus": "b"})).await;
    // `perk` shown and sent as null: required.
    update(&admin, &app, &c, json!({"perk": null})).await.assert_validation_error("perk");
    // `bonus` sent as null with the band changed: hidden, cleared (to null).
    update(&admin, &app, &c, json!({"band": "A", "bonus": null})).await.assert_status(StatusCode::OK);
    assert_eq!(stored(&app, &c).await.bonus, None);
    // Hidden `perk` sent as null: dropped, not "required".
    update(&admin, &app, &c, json!({"perk": null})).await.assert_status(StatusCode::OK);
    assert_eq!(stored(&app, &c).await.perk.as_deref(), Some("p"));
});

db_test!(schemas_send_conditions_in_canonical_form, |db: TestDb| async move {
    let (app, users) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let schema = admin.get(&app.admin_api("resources/staff/schema")).await.json();
    let components = schema["form"]["components"].as_array().unwrap();
    let bonus = components.iter().find(|c| c["name"] == json!("bonus")).unwrap();
    assert_eq!(bonus["shownWhen"], json!({"op": "eq", "name": "band", "value": "C"}));
});
