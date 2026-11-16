//! Infolists (ADR 0039): a declared infolist replaces the form on the view
//! page; entries of fields the user may not read are left out; computed
//! values run only for users who may view the record, alone (a failure stays
//! in its entry), never for lists or reads; panel pages show their data with
//! entries; infolists that cannot hold refuse to boot. SQLite, PostgreSQL,
//! MySQL.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, AppError, AppResult, Config};
use rustcms_testing::{Factory, TestApp, TestClient, TestDb, UserFactory, db_test};
use sea_orm::Schema;
use serde_json::{Value, json};

mod support;

pub mod course {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "courses")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub title: String,
        pub code: String,
        pub status: String,
        pub published: bool,
        pub meta: Option<Json>,
        pub teacher_id: i32,
        pub fee: Option<i32>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod teacher {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "teachers")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Who may read fees (the `fee` field and the `revenue` computed value).
const FEES: &str = "courses.fees";

/// Runs of the `learners` computed value.
static LEARNERS_RUNS: AtomicUsize = AtomicUsize::new(0);

fn course_fields() -> Vec<Field> {
    use course::Column as C;
    vec![
        Field::column(C::Title).required(),
        Field::column(C::Code).required(),
        Field::column(C::Status).required(),
        Field::column(C::Published),
        Field::column(C::Meta).key_value(),
        Field::column(C::TeacherId).required().belongs_to("teachers"),
        Field::column(C::Fee).readable(|ctx| ctx.can(FEES)),
    ]
}

fn course_infolist() -> Infolist {
    Infolist::new().columns(2).schema(components![
        Section::make("Course").schema(components![
            TextEntry::make("title").copyable(),
            TextEntry::make("status").badge().color("open", "success").options([("open", "Open")]),
            BooleanEntry::make("published").labels("Live", "Hidden"),
            RelationEntry::make("teacher_id"),
        ]),
        Section::make("Money").schema(components![TextEntry::make("fee").number(0).suffix(" VND")]),
        KeyValueEntry::make("meta").shown_when(when("published").is_true()),
        CustomEntry::make("code", "acme/code").props(json!({"size": "large"})),
        ComputedEntry::make("learners").label("Learners"),
        ComputedEntry::make("revenue").number(2),
        ComputedEntry::make("broken"),
        ComputedEntry::make("slow"),
        ComputedEntry::make("huge"),
    ])
}

fn course_computed() -> Vec<Computed<course::Entity>> {
    vec![
        Computed::make("learners", |_, _| {
            Box::pin(async move {
                LEARNERS_RUNS.fetch_add(1, Ordering::SeqCst);
                Ok(json!(42))
            })
        }),
        Computed::make("revenue", |_, course: &course::Model| {
            Box::pin(async move { Ok(json!(format!("{}.00", course.fee.unwrap_or(0) * 10))) })
        })
        .readable(|ctx| ctx.can(FEES)),
        Computed::make("broken", |_, _| Box::pin(async move { Err(AppError::internal("boom")) })),
        Computed::make("slow", |_, _| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(json!(1))
            })
        })
        .timeout(Duration::from_millis(200)),
        Computed::make("huge", |_, _| Box::pin(async move { Ok(json!("x".repeat(70_000))) })),
    ]
}

pub struct CourseResource;

impl Resource for CourseResource {
    type Entity = course::Entity;
    const SLUG: &'static str = "courses";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Course").title("title")
    }

    fn fields() -> Vec<Field> {
        course_fields()
    }

    fn infolist() -> Infolist {
        course_infolist()
    }

    fn computed() -> Vec<Computed<course::Entity>> {
        course_computed()
    }
}

pub struct TeacherResource;

impl Resource for TeacherResource {
    type Entity = teacher::Entity;
    const SLUG: &'static str = "teachers";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Teacher").title("name")
    }

    fn fields() -> Vec<Field> {
        vec![Field::column(teacher::Column::Name).required()]
    }
}

/// A panel page showing its data with entries.
pub struct StatsPage;

#[async_trait::async_trait]
impl PanelPage for StatsPage {
    const SLUG: &'static str = "stats";

    fn meta() -> PanelPageMeta {
        PanelPageMeta::new("Stats").permission("courses.view_any")
    }

    fn view() -> PageView {
        PageView::Infolist(|| {
            Infolist::new().schema(components![Section::make("Totals").schema(components![
                TextEntry::make("courses").number(0),
                BooleanEntry::make("healthy"),
                TextEntry::make("note").shown_when(when("courses").filled()),
            ])])
        })
    }

    async fn data(_ctx: &Ctx) -> AppResult<Value> {
        Ok(json!({"courses": 3, "healthy": true, "note": ""}))
    }
}

fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    app.provider(AuthProvider::new().permission(ADMIN_GUARD, FEES, "courses"));
    app.provider(AdminProvider::new().resource::<CourseResource>().resource::<TeacherResource>().page::<StatsPage>());
    app
}

struct Users {
    admin: rustcms_core::models::user::Model,
    /// Views courses, but neither fees nor teachers.
    reader: rustcms_core::models::user::Model,
    /// No permission.
    outsider: rustcms_core::models::user::Model,
}

async fn setup(db: TestDb) -> (TestApp, Users, String) {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(course::Entity)).await;
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(teacher::Entity)).await;
    let app = TestApp::new(build, db).await;
    let auth = app.state().service::<AuthState>().unwrap();
    let db = app.state().db();
    let rbac = RbacService::new(db, &auth.resolver);
    rbac.sync_permissions(&auth.permissions.all()).await.unwrap();
    let admin = UserFactory::new().name("Admin").create(db).await;
    let super_admin = rustcms_auth::entities::role::super_admin_id(db, ADMIN_GUARD).await.unwrap().unwrap();
    rbac.assign_role(Actor::System, admin.id, super_admin).await.unwrap();
    let reader = UserFactory::new().name("Reader").create(db).await;
    let permissions = ["courses.view_any".to_owned(), "courses.view".to_owned()];
    let role = rbac.create_role(Actor::System, ADMIN_GUARD, "reader", &permissions).await.unwrap();
    rbac.assign_role(Actor::System, reader.id, role.id).await.unwrap();
    let outsider = UserFactory::new().name("Outsider").create(db).await;

    let client = app.acting_as(&admin).await;
    let teacher = create(&client, &app, "teachers", json!({"name": "Ada"})).await;
    let course = create(
        &client,
        &app,
        "courses",
        json!({
            "title": "Algebra", "code": "ALG-1", "status": "open", "published": true,
            "meta": {"room": "B2"}, "teacher_id": teacher.parse::<i32>().unwrap(), "fee": 50,
        }),
    )
    .await;
    (app, Users { admin, reader, outsider }, course)
}

async fn create(client: &TestClient<'_>, app: &TestApp, slug: &str, body: Value) -> String {
    let response = client.post_json(&app.admin_api(&format!("resources/{slug}/records")), &body).await;
    response.assert_status(StatusCode::CREATED);
    response.json()["data"]["key"].as_str().unwrap().to_owned()
}

async fn view_schema(client: &TestClient<'_>, app: &TestApp, slug: &str, key: &str) -> Value {
    let url = app.admin_api(&format!("resources/{slug}/schema?operation=view&key={key}"));
    let response = client.get(&url).await;
    response.assert_status(StatusCode::OK);
    response.json()
}

/// Every entry of `components`, depth first.
fn entries(components: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for component in components.as_array().unwrap() {
        if let Some(children) = component.get("components") {
            out.extend(entries(children));
        } else {
            out.push(component.clone());
        }
    }
    out
}

fn names(components: &Value) -> Vec<String> {
    entries(components).iter().map(|e| e["name"].as_str().unwrap().to_owned()).collect()
}

db_test!(a_declared_infolist_replaces_the_view_page, |db: TestDb| async move {
    let sqlite = db.backend() == rustcms_testing::Backend::Sqlite;
    let (app, users, course) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;

    let schema = view_schema(&admin, &app, "courses", &course).await;
    if sqlite {
        // Shared with the SPA tests: the schema, the record and its computed values.
        support::contract("infolist.schema.view.json", &schema);
        let record = admin.get(&app.admin_api(&format!("resources/courses/records/{course}"))).await.json();
        support::contract("infolist.record.json", &record);
        let computed = admin.get(&app.admin_api(&format!("resources/courses/records/{course}/computed"))).await.json();
        support::contract("infolist.computed.json", &computed);
    }
    let infolist = &schema["infolist"];
    assert_eq!(infolist["columns"], json!(2));
    assert_eq!(
        names(&infolist["components"]),
        [
            "title",
            "status",
            "published",
            "teacher_id",
            "fee",
            "meta",
            "code",
            "learners",
            "revenue",
            "broken",
            "slow",
            "huge"
        ]
    );
    let all = entries(&infolist["components"]);
    let entry = |name: &str| all.iter().find(|e| e["name"] == name).unwrap().clone();
    assert_eq!(
        entry("title"),
        json!({"type": "text_entry", "name": "title", "label": "Title", "valueType": "string", "columnSpan": 1, "copyable": true})
    );
    assert_eq!(entry("status")["badge"], json!(true));
    assert_eq!(entry("status")["colors"], json!([{"value": "open", "color": "success"}]));
    assert_eq!(entry("status")["options"], json!([{"value": "open", "label": "Open"}]));
    assert_eq!(entry("published")["labels"], json!(["Live", "Hidden"]));
    assert_eq!(entry("teacher_id")["relationship"], json!({"resource": "teachers"}));
    assert_eq!(entry("teacher_id")["link"], json!(true));
    assert_eq!(entry("fee")["decimals"], json!(0));
    assert_eq!(entry("fee")["suffix"], json!(" VND"));
    assert_eq!(entry("meta")["shownWhen"], json!({"op": "is_true", "name": "published"}));
    assert_eq!(entry("code")["widget"], json!("acme/code"));
    assert_eq!(entry("code")["props"], json!({"size": "large"}));
    assert_eq!(entry("learners")["valueType"], json!("json"));
    // The server says which values come from `records/{key}/computed`.
    assert_eq!(entry("learners")["computed"], json!(true));
    assert!(entry("code").get("computed").is_none());
    assert_eq!(entry("learners")["label"], json!("Learners"));
    assert_eq!(entry("revenue")["label"], json!("Revenue"));

    // The form is still there (the dashboard reads it), and the other operations have no infolist.
    assert!(!schema["form"]["components"].as_array().unwrap().is_empty());
    let edit = admin.get(&app.admin_api(&format!("resources/courses/schema?operation=edit&key={course}"))).await.json();
    assert!(edit.get("infolist").is_none());
    let create = admin.get(&app.admin_api("resources/courses/schema?operation=create")).await.json();
    assert!(create.get("infolist").is_none());

    // Lists ask for `view` without a key: no infolist (only a record's view page draws it).
    let keyless = admin.get(&app.admin_api("resources/courses/schema?operation=view")).await.json();
    assert!(keyless.get("infolist").is_none());

    // A resource without an infolist: entries made from its form (plan branch 4).
    let teachers = admin.get(&app.admin_api("resources/teachers/records")).await.json();
    let teacher = teachers["data"][0]["key"].as_str().unwrap().to_owned();
    let made = view_schema(&admin, &app, "teachers", &teacher).await;
    assert_eq!(entries(&made["infolist"]["components"])[0]["type"], json!("text_entry"));
    assert_eq!(names(&made["infolist"]["components"]), ["name"]);
});

db_test!(entries_the_user_may_not_read_are_left_out, |db: TestDb| async move {
    let (app, users, course) = setup(db).await;
    let reader = app.acting_as(&users.reader).await;

    let schema = view_schema(&reader, &app, "courses", &course).await;
    let components = &schema["infolist"]["components"];
    // The fee, and the section holding only it, are gone; so is the revenue.
    assert_eq!(
        names(components),
        ["title", "status", "published", "teacher_id", "meta", "code", "learners", "broken", "slow", "huge"]
    );
    let labels: Vec<&Value> = components.as_array().unwrap().iter().filter_map(|c| c.get("label")).collect();
    assert!(!labels.contains(&&json!("Money")));
    // Teachers cannot be listed: the title is shown without a link.
    let teacher = entries(components).into_iter().find(|e| e["name"] == "teacher_id").unwrap();
    assert!(teacher.get("link").is_none());
});

db_test!(computed_values_fail_alone, |db: TestDb| async move {
    let (app, users, course) = setup(db).await;
    let url = app.admin_api(&format!("resources/courses/records/{course}/computed"));

    let admin = app.acting_as(&users.admin).await;
    let response = admin.get(&url).await;
    response.assert_status(StatusCode::OK);
    // An error, a timeout and a value over 64 KiB fail alone, in declaration order.
    assert_eq!(
        response.json(),
        json!({"data": {"learners": 42, "revenue": "500.00"}, "failed": ["broken", "slow", "huge"]})
    );

    // The revenue is not computed for a user who may not see it.
    let reader = app.acting_as(&users.reader).await;
    assert_eq!(reader.get(&url).await.json(), json!({"data": {"learners": 42}, "failed": ["broken", "slow", "huge"]}));
});

db_test!(computed_values_run_only_for_viewers_of_the_view_page, |db: TestDb| async move {
    let (app, users, course) = setup(db).await;
    let url = app.admin_api(&format!("resources/courses/records/{course}/computed"));
    let runs = LEARNERS_RUNS.load(Ordering::SeqCst);

    // Not for a user who may not view the record.
    let outsider = app.acting_as(&users.outsider).await;
    outsider.get(&url).await.assert_status(StatusCode::FORBIDDEN);
    // Not for an unknown record.
    let admin = app.acting_as(&users.admin).await;
    admin.get(&app.admin_api("resources/courses/records/999/computed")).await.assert_status(StatusCode::NOT_FOUND);
    // Not for lists, reads or schemas.
    admin.get(&app.admin_api("resources/courses/records")).await.assert_status(StatusCode::OK);
    admin.get(&app.admin_api(&format!("resources/courses/records/{course}"))).await.assert_status(StatusCode::OK);
    view_schema(&admin, &app, "courses", &course).await;
    assert_eq!(LEARNERS_RUNS.load(Ordering::SeqCst), runs);

    admin.get(&url).await.assert_status(StatusCode::OK);
    assert_eq!(LEARNERS_RUNS.load(Ordering::SeqCst), runs + 1);

    // A resource without computed values answers with none.
    let teachers = admin.get(&app.admin_api("resources/teachers/records")).await.json();
    let teacher = teachers["data"][0]["key"].as_str().unwrap();
    let none = admin.get(&app.admin_api(&format!("resources/teachers/records/{teacher}/computed"))).await;
    assert_eq!(none.json(), json!({"data": {}, "failed": []}));
});

db_test!(computed_values_are_limited_per_user, |db: TestDb| async move {
    let (app, users, _) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let teachers = admin.get(&app.admin_api("resources/teachers/records")).await.json();
    let teacher = teachers["data"][0]["key"].as_str().unwrap().to_owned();
    let url = app.admin_api(&format!("resources/teachers/records/{teacher}/computed"));
    for _ in 0..60 {
        admin.get(&url).await.assert_status(StatusCode::OK);
    }
    admin.get(&url).await.assert_status(StatusCode::TOO_MANY_REQUESTS);
    // Another user has their own allowance.
    let reader = app.acting_as(&users.reader).await;
    let courses = reader.get(&app.admin_api("resources/courses/records")).await.json();
    let course = courses["data"][0]["key"].as_str().unwrap();
    reader
        .get(&app.admin_api(&format!("resources/courses/records/{course}/computed")))
        .await
        .assert_status(StatusCode::OK);
});

db_test!(panel_pages_show_their_data_with_entries, |db: TestDb| async move {
    let (app, users, _) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let page = admin.get(&app.admin_api("pages/stats")).await;
    page.assert_status(StatusCode::OK);
    let content = &page.json()["content"];
    assert_eq!(content["kind"], json!("infolist"));
    assert_eq!(content["data"], json!({"courses": 3, "healthy": true, "note": ""}));
    assert_eq!(names(&content["infolist"]["components"]), ["courses", "healthy", "note"]);
    let section = &content["infolist"]["components"][0];
    assert_eq!(section["label"], json!("Totals"));
    assert_eq!(section["components"][0]["valueType"], json!("json"));
    assert_eq!(section["components"][0]["label"], json!("Courses"));

    let outsider = app.acting_as(&users.outsider).await;
    outsider.get(&app.admin_api("pages/stats")).await.assert_status(StatusCode::FORBIDDEN);
});

macro_rules! refused_at_boot {
    ($test:ident, $resource:ident, $message:literal, infolist: $infolist:expr, computed: $computed:expr) => {
        refused_at_boot!($test, $resource, $message, infolist: $infolist, computed: $computed, form: Form::default());
    };
    ($test:ident, $resource:ident, $message:literal, infolist: $infolist:expr, computed: $computed:expr, form: $form:expr) => {
        pub struct $resource;

        impl Resource for $resource {
            type Entity = course::Entity;
            const SLUG: &'static str = "courses";

            fn meta() -> ResourceMeta {
                ResourceMeta::new("Course")
            }

            fn fields() -> Vec<Field> {
                course_fields()
            }

            fn form() -> Form {
                $form
            }

            fn infolist() -> Infolist {
                $infolist
            }

            fn computed() -> Vec<Computed<course::Entity>> {
                $computed
            }
        }

        #[test]
        #[should_panic(expected = $message)]
        fn $test() {
            Registry::new().register::<$resource>();
        }
    };
}

fn list(components: Vec<rustcms_admin::ui::Component>) -> Infolist {
    Infolist::new().schema(components)
}

fn learners() -> Vec<Computed<course::Entity>> {
    vec![Computed::make("learners", |_, _| Box::pin(async move { Ok(json!(1)) }))]
}

refused_at_boot!(an_unknown_field, UnknownField, "entry `level` is not a field",
    infolist: list(components![TextEntry::make("level")]), computed: Vec::new());
refused_at_boot!(a_relation_entry_without_relationship, NotRelation,
    "entry `title`: a RelationEntry (belongs_to fields) does not fit this field",
    infolist: list(components![RelationEntry::make("title")]), computed: Vec::new());
refused_at_boot!(a_boolean_entry_on_text, NotBoolean, "entry `title`: a BooleanEntry (boolean columns) does not fit",
    infolist: list(components![BooleanEntry::make("title")]), computed: Vec::new());
refused_at_boot!(a_key_value_entry_on_text, NotKeyValue, "entry `title`: a KeyValueEntry",
    infolist: list(components![KeyValueEntry::make("title")]), computed: Vec::new());
refused_at_boot!(a_badge_on_a_boolean, BadgeOnBoolean, "entry `published`: `badge` does not fit a BooleanEntry",
    infolist: list(components![BooleanEntry::make("published").badge()]), computed: Vec::new());
refused_at_boot!(a_color_without_badge, ColorWithoutBadge, "entry `title`: `color` needs `badge`",
    infolist: list(components![TextEntry::make("title").color("a", "success")]), computed: Vec::new());
refused_at_boot!(a_number_on_text, NumberOnText, "entry `title`: `number` needs a number field",
    infolist: list(components![TextEntry::make("title").number(2)]), computed: Vec::new());
refused_at_boot!(labels_on_text, LabelsOnText, "entry `title`: `labels` does not fit a TextEntry",
    infolist: list(components![TextEntry::make("title").labels("a", "b")]), computed: Vec::new());
refused_at_boot!(an_entry_twice, Twice, "entry `title` appears twice",
    infolist: list(components![TextEntry::make("title"), TextEntry::make("title")]), computed: Vec::new());
refused_at_boot!(a_computed_entry_without_value, NoValue, "computed entry `total` names no value of `computed()`",
    infolist: list(components![ComputedEntry::make("total")]), computed: Vec::new());
refused_at_boot!(a_computed_value_named_like_a_field, NamedLikeField,
    "computed value `title` has the name of a field",
    infolist: Infolist::new(),
    computed: vec![Computed::make("title", |_, _| Box::pin(async move { Ok(json!(1)) }))]);
refused_at_boot!(a_computed_value_twice, ComputedTwice, "computed value `learners` declared twice",
    infolist: Infolist::new(), computed: [learners(), learners()].concat());
refused_at_boot!(a_timeout_over_thirty_seconds, LongTimeout, "the timeout must be above 0 and at most 30 seconds",
    infolist: Infolist::new(),
    computed: vec![Computed::make("slow", |_, _| Box::pin(async move { Ok(json!(1)) })).timeout(Duration::from_secs(31))]);
refused_at_boot!(an_invalid_custom_entry, BadWidget, "invalid entry name `Acme`",
    infolist: list(components![CustomEntry::make("code", "Acme")]), computed: Vec::new());
refused_at_boot!(an_input_in_an_infolist, InputInInfolist, "an infolist holds entries, not inputs",
    infolist: list(components![TextInput::make("title")]), computed: Vec::new());
refused_at_boot!(an_entry_in_a_form, EntryInForm, "a form holds inputs, not entries",
    infolist: Infolist::new(), computed: Vec::new(),
    form: Form::new().schema(components![TextInput::make("title"), TextEntry::make("code")]));
refused_at_boot!(a_condition_on_a_computed_value, ConditionOnComputed, "names `learners`, whose value conditions cannot read",
    infolist: list(components![ComputedEntry::make("learners"), TextEntry::make("title").shown_when(when("learners").filled())]),
    computed: learners());
refused_at_boot!(a_condition_on_a_missing_entry, ConditionOnMissing, "names `status`, which is not an input (or entry)",
    infolist: list(components![TextEntry::make("title").shown_when(when("status").eq("open"))]), computed: Vec::new());

/// A panel page whose infolist shows a relation.
pub struct RelationPage;

#[async_trait::async_trait]
impl PanelPage for RelationPage {
    const SLUG: &'static str = "relation";

    fn meta() -> PanelPageMeta {
        PanelPageMeta::new("Relation").permission("courses.view_any")
    }

    fn view() -> PageView {
        PageView::Infolist(|| Infolist::new().schema(components![RelationEntry::make("teacher")]))
    }
}

#[tokio::test]
#[should_panic(expected = "a page's infolist shows its data with text, boolean, key/value and custom entries only")]
async fn a_page_infolist_with_a_relation_entry() {
    let db = TestDb::sqlite().await;
    let _ = TestApp::new(
        |config| {
            let mut app = AppBuilder::new(config);
            app.provider(AuthProvider::new());
            app.provider(AdminProvider::new().page::<RelationPage>());
            app
        },
        db,
    )
    .await;
}
