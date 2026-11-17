//! Count columns of has-many relations (ADR 0040, part 3): each record's
//! children the user may list (scope, trash, view policy), one grouped query
//! per page when the policy decides once, a bounded scan when it decides per
//! record; the column is left out for users who may not list the relation.
//! SQLite, PostgreSQL, MySQL.

use axum::http::StatusCode;
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, Config};
use rustcms_testing::{Factory, TestApp, TestDb, UserFactory, db_test};
use sea_orm::{ConnectionTrait, EntityTrait, QueryFilter, Schema};
use serde_json::{Value, json};

pub mod order {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "orders")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub number: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod line {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "order_lines")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub order_id: i32,
        pub product: String,
        pub quantity: i32,
        pub position: i32,
        pub deleted_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(has_many = "super::label::Entity")]
        Labels,
    }

    impl Related<super::label::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Labels.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

/// Labels of a line: a scope joining them yields a line once per label.
pub mod label {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "line_labels")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub line_id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(belongs_to = "super::line::Entity", from = "Column::LineId", to = "super::line::Column::Id")]
        Line,
    }

    impl Related<super::line::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Line.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub struct OrderResource;

impl Resource for OrderResource {
    type Entity = order::Entity;
    const SLUG: &'static str = "orders";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Order").title("number")
    }

    fn fields() -> Vec<Field> {
        vec![Field::column(order::Column::Number).required()]
    }

    fn form() -> Form {
        Form::new().schema(components![TextInput::make("number")])
    }

    fn table() -> Table {
        Table::new().columns(vec![
            TextColumn::make("number"),
            TextColumn::make("lines_count").counts("lines"),
            TextColumn::make("labelled_count").counts("labelled"),
        ])
    }

    fn relations() -> Vec<Relation> {
        vec![
            Relation::has_many("lines", "order_lines", "order_id"),
            Relation::has_many("labelled", "labelled_lines", "order_id"),
        ]
    }
}

/// Lines with a label: a scope joining the labels, which yields a line once
/// per label (the count must count each line once).
pub struct LabelledLineResource;

impl Resource for LabelledLineResource {
    type Entity = line::Entity;
    const SLUG: &'static str = "labelled_lines";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Labelled line").title("product")
    }

    fn fields() -> Vec<Field> {
        vec![Field::column(line::Column::OrderId).belongs_to("orders"), Field::column(line::Column::Product)]
    }

    fn scope(query: sea_orm::Select<line::Entity>, _ctx: &Ctx) -> sea_orm::Select<line::Entity> {
        use sea_orm::{JoinType, QuerySelect, RelationTrait};
        query.join(JoinType::InnerJoin, line::Relation::Labels.def())
    }
}

/// Lines: the panel's permissions, and lines named "secret" visible to no one
/// (decided per record once [`PER_RECORD`] is set; nextest runs each test in
/// its own process).
struct LinePolicy;

static PER_RECORD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[async_trait::async_trait]
impl Policy for LinePolicy {
    async fn allows(&self, ctx: &Ctx, ability: &str, record: Option<&Data>) -> bool {
        let secret = record.is_some_and(|r| r.get("product") == Some(&json!("secret")));
        ctx.can(&format!("order_lines.{ability}")) && !(secret && PER_RECORD.load(std::sync::atomic::Ordering::SeqCst))
    }

    async fn admits(&self, ctx: &Ctx, ability: &str) -> bool {
        ctx.can(&format!("order_lines.{ability}"))
    }

    fn view_depends_on_record(&self) -> bool {
        PER_RECORD.load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub struct LineResource;

#[async_trait::async_trait]
impl Resource for LineResource {
    type Entity = line::Entity;
    const SLUG: &'static str = "order_lines";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Line").title("product").soft_deletes()
    }

    fn fields() -> Vec<Field> {
        use line::Column as C;
        vec![
            Field::column(C::OrderId).required().belongs_to("orders"),
            Field::column(C::Product).required().max_length(20),
            Field::column(C::Quantity).required(),
            Field::column(C::Position).guarded().readable(|_| true),
            Field::column(C::DeletedAt).guarded().readable(|_| true),
        ]
    }

    fn policy() -> std::sync::Arc<dyn Policy> {
        std::sync::Arc::new(LinePolicy)
    }

    // Lines named "hidden" are outside everyone's scope.
    fn scope(query: sea_orm::Select<line::Entity>, _ctx: &Ctx) -> sea_orm::Select<line::Entity> {
        query.filter(sea_orm::ColumnTrait::ne(&line::Column::Product, "hidden"))
    }
}

fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    app.provider(AuthProvider::new());
    app.provider(
        AdminProvider::new().resource::<OrderResource>().resource::<LineResource>().resource::<LabelledLineResource>(),
    );
    app
}

async fn setup(db: TestDb) -> (TestApp, rustcms_core::models::user::Model) {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(order::Entity)).await;
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(line::Entity)).await;
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(label::Entity)).await;
    let app = TestApp::new(build, db).await;
    let auth = app.state().service::<AuthState>().unwrap();
    let db = app.state().db();
    let rbac = RbacService::new(db, &auth.resolver);
    rbac.sync_permissions(&auth.permissions.all()).await.unwrap();
    let admin = UserFactory::new().name("Admin").create(db).await;
    let super_admin = rustcms_auth::entities::role::super_admin_id(db, ADMIN_GUARD).await.unwrap().unwrap();
    rbac.assign_role(Actor::System, admin.id, super_admin).await.unwrap();
    (app, admin)
}

/// An order with active lines "a", "b", a trashed line "c" and a line
/// outside the scope ("hidden").
async fn order_with_lines(app: &TestApp) -> String {
    let db = app.state().db();
    db.execute_unprepared("INSERT INTO orders (number) VALUES ('A-1')").await.unwrap();
    let order = order::Entity::find()
        .filter(sea_orm::ColumnTrait::eq(&order::Column::Number, "A-1"))
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .id;
    for (product, position, trashed) in [("a", 1, false), ("b", 2, false), ("c", 3, true), ("hidden", 4, false)] {
        let deleted = if trashed { "'2026-01-01 00:00:00'" } else { "NULL" };
        db.execute_unprepared(&format!(
            "INSERT INTO order_lines (order_id, product, quantity, position, deleted_at) \
             VALUES ({order}, '{product}', 1, {position}, {deleted})"
        ))
        .await
        .unwrap();
    }
    order.to_string()
}

db_test!(a_count_column_counts_the_rows_the_user_may_list, |db: TestDb| async move {
    let (app, admin) = setup(db).await;
    let admin = app.acting_as(&admin).await;
    // A line in the trash and the line outside the scope are not counted.
    let order = order_with_lines(&app).await;
    let schema = admin.get(&app.admin_api("resources/orders/schema")).await.json();
    let column = schema["table"]["columns"].as_array().unwrap().iter().find(|c| c["name"] == "lines_count").cloned();
    let column = column.expect("the count column");
    assert_eq!((column["counts"].clone(), column["label"].clone()), (json!("lines"), json!("Lines count")));
    assert_eq!(column["sortable"], json!(false));
    let list = admin.get(&app.admin_api("resources/orders/records")).await.json();
    assert_eq!(list["data"][0]["counts"]["lines"], json!(2));
    assert!(list["data"][0]["data"].get("lines_count").is_none());
    // A record without lines counts zero; a single read carries no counts.
    app.state().db().execute_unprepared("INSERT INTO orders (number) VALUES ('A-0')").await.unwrap();
    let empty = order::Entity::find()
        .filter(sea_orm::ColumnTrait::eq(&order::Column::Number, "A-0"))
        .one(app.state().db())
        .await
        .unwrap()
        .unwrap();
    let empty = json!(empty.id.to_string());
    let list = admin.get(&app.admin_api("resources/orders/records")).await.json();
    let row = list["data"].as_array().unwrap().iter().find(|r| r["key"] == empty).unwrap().clone();
    assert_eq!(row["counts"]["lines"], json!(0));
    let one = admin.get(&app.admin_api(&format!("resources/orders/records/{order}"))).await.json();
    assert!(one["data"].get("counts").is_none());

    // A policy deciding per record: one scan for the page, each line checked.
    app.state()
        .db()
        .execute_unprepared(&format!(
            "INSERT INTO order_lines (order_id, product, quantity, position) VALUES ({order}, 'secret', 1, 9)"
        ))
        .await
        .unwrap();
    let counts = |list: Value| -> Vec<Value> {
        list["data"].as_array().unwrap().iter().map(|r| r["counts"]["lines"].clone()).collect()
    };
    let list = admin.get(&app.admin_api("resources/orders/records")).await.json();
    assert_eq!(counts(list), [json!(3), json!(0)], "decided once: the scope alone");
    PER_RECORD.store(true, std::sync::atomic::Ordering::SeqCst);
    let list = admin.get(&app.admin_api("resources/orders/records")).await.json();
    assert_eq!(counts(list), [json!(2), json!(0)], "per record: the secret line is not listed");
    PER_RECORD.store(false, std::sync::atomic::Ordering::SeqCst);

    // Someone who may not list lines gets neither the column nor the counts.
    let auth = app.state().service::<AuthState>().unwrap();
    let rbac = RbacService::new(app.state().db(), &auth.resolver);
    let viewer = UserFactory::new().name("Viewer").create(app.state().db()).await;
    let role = rbac
        .create_role(Actor::System, ADMIN_GUARD, "viewer", &["orders.view_any".to_owned(), "orders.view".to_owned()])
        .await
        .unwrap();
    rbac.assign_role(Actor::System, viewer.id, role.id).await.unwrap();
    let viewer = app.acting_as(&viewer).await;
    let schema = viewer.get(&app.admin_api(&format!("resources/orders/schema?operation=view&key={order}"))).await;
    let schema = schema.assert_status(StatusCode::OK).json();
    assert!(schema["table"]["columns"].as_array().unwrap().iter().all(|c| c["name"] != "lines_count"));
    let list = viewer.get(&app.admin_api("resources/orders/records")).await.json();
    assert!(list["data"].as_array().unwrap().iter().all(|r| r.get("counts").is_none()));
});

pub struct RepeatedLines;

impl Resource for RepeatedLines {
    type Entity = order::Entity;
    const SLUG: &'static str = "orders";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Order")
    }

    fn fields() -> Vec<Field> {
        vec![Field::column(order::Column::Number).required()]
    }

    fn form() -> Form {
        Form::new().schema(components![
            TextInput::make("number"),
            Repeater::make("lines").relationship().schema(components![TextInput::make("product")]),
        ])
    }

    fn relations() -> Vec<Relation> {
        vec![Relation::has_many("lines", "order_lines", "order_id")]
    }
}

/// A relationship repeater whose child does not lock its parent is refused
/// (ADR 0041: the family serializes on the parent row).
#[test]
#[should_panic(expected = "`order_lines` must lock its parent")]
fn a_relationship_repeater_needs_its_child_to_lock_the_parent() {
    Registry::new().register::<RepeatedLines>().register::<LineResource>().finish();
}

db_test!(a_scope_joining_rows_counts_each_child_once, |db: TestDb| async move {
    let (app, admin) = setup(db).await;
    let admin = app.acting_as(&admin).await;
    let order = order_with_lines(&app).await;
    let db = app.state().db();
    // "a" has two labels, "b" one, "hidden" none: the join yields "a" twice.
    for (line, name) in [("a", "red"), ("a", "blue"), ("b", "red")] {
        db.execute_unprepared(&format!(
            "INSERT INTO line_labels (line_id, name) SELECT id, '{name}' FROM order_lines \
             WHERE order_id = {order} AND product = '{line}'"
        ))
        .await
        .unwrap();
    }
    let list = admin.get(&app.admin_api("resources/orders/records")).await.json();
    let row = list["data"].as_array().unwrap().iter().find(|r| r["key"] == json!(order)).unwrap().clone();
    // Labelled lines: "a" and "b", each once ("c" and "hidden" have no label).
    assert_eq!(row["counts"]["labelled"], json!(2), "{row}");
});
