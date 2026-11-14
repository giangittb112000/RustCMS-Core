//! Record titles from a template (ADR 0038): in envelopes, relationship
//! options (searched over every field of the template) and labels; no title
//! for a user who may not read one of its fields; templates that cannot hold
//! refuse to boot. SQLite, PostgreSQL, MySQL.

use axum::http::StatusCode;
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, Config};
use rustcms_testing::{Factory, TestApp, TestClient, TestDb, UserFactory, db_test};
use sea_orm::Schema;
use serde_json::{Value, json};

pub mod subject {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "subjects")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub grade: Option<i32>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod lesson {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "lessons")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub title: String,
        pub subject_id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Who may read a subject's grade.
const GRADES: &str = "subjects.grades";

fn subject_fields() -> Vec<Field> {
    use subject::Column as C;
    vec![Field::column(C::Name).required().searchable(), Field::column(C::Grade).readable(|ctx| ctx.can(GRADES))]
}

pub struct SubjectResource;

impl Resource for SubjectResource {
    type Entity = subject::Entity;
    const SLUG: &'static str = "subjects";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Subject").title_template("{name} (grade {grade})")
    }

    fn fields() -> Vec<Field> {
        subject_fields()
    }
}

pub struct LessonResource;

impl Resource for LessonResource {
    type Entity = lesson::Entity;
    const SLUG: &'static str = "lessons";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Lesson").title("title")
    }

    fn fields() -> Vec<Field> {
        use lesson::Column as C;
        vec![Field::column(C::Title).required(), Field::column(C::SubjectId).required().belongs_to("subjects")]
    }
}

fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    app.provider(AuthProvider::new().permission(ADMIN_GUARD, GRADES, "subjects"));
    app.provider(AdminProvider::new().resource::<SubjectResource>().resource::<LessonResource>());
    app
}

struct Users {
    admin: rustcms_core::models::user::Model,
    teacher: rustcms_core::models::user::Model,
}

async fn setup(db: TestDb) -> (TestApp, Users) {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(subject::Entity)).await;
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(lesson::Entity)).await;
    let app = TestApp::new(build, db).await;
    let auth = app.state().service::<AuthState>().unwrap();
    let db = app.state().db();
    let rbac = RbacService::new(db, &auth.resolver);
    rbac.sync_permissions(&auth.permissions.all()).await.unwrap();
    let admin = UserFactory::new().name("Admin").create(db).await;
    let super_admin = rustcms_auth::entities::role::super_admin_id(db, ADMIN_GUARD).await.unwrap().unwrap();
    rbac.assign_role(Actor::System, admin.id, super_admin).await.unwrap();
    // Everything on both resources, but not the subjects' grades.
    let teacher = UserFactory::new().name("Teacher").create(db).await;
    let permissions: Vec<String> = ["subjects", "lessons"]
        .iter()
        .flat_map(|r| ["view_any", "view", "create", "update"].map(|a| format!("{r}.{a}")))
        .collect();
    let role = rbac.create_role(Actor::System, ADMIN_GUARD, "teacher", &permissions).await.unwrap();
    rbac.assign_role(Actor::System, teacher.id, role.id).await.unwrap();
    (app, Users { admin, teacher })
}

async fn create(client: &TestClient<'_>, app: &TestApp, slug: &str, body: Value) -> String {
    let response = client.post_json(&app.admin_api(&format!("resources/{slug}/records")), &body).await;
    response.assert_status(StatusCode::CREATED);
    response.json()["data"]["key"].as_str().unwrap().to_owned()
}

/// `(key, label)` of the options of `lessons.subject_id`.
async fn options(client: &TestClient<'_>, app: &TestApp, search: &str) -> Vec<(String, String)> {
    let url = app.admin_api(&format!("resources/lessons/fields/subject_id/options?search={search}"));
    let found = client.get(&url).await.json();
    found["data"]
        .as_array()
        .unwrap()
        .iter()
        // A numeric key comes as a number.
        .map(|o| (o["value"].to_string().trim_matches('"').to_owned(), o["label"].as_str().unwrap().to_owned()))
        .collect()
}

db_test!(titles_come_from_the_template, |db: TestDb| async move {
    let (app, users) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let math10 = create(&admin, &app, "subjects", json!({"name": "Math", "grade": 10})).await;
    let math11 = create(&admin, &app, "subjects", json!({"name": "Math", "grade": 11})).await;
    let art = create(&admin, &app, "subjects", json!({"name": "Art", "grade": null})).await;

    // Envelopes: the record page heading, breadcrumbs, row menus.
    let read = admin.get(&app.admin_api(&format!("resources/subjects/records/{math10}"))).await.json();
    assert_eq!(read["data"]["title"], json!("Math (grade 10)"));
    let read = admin.get(&app.admin_api(&format!("resources/subjects/records/{art}"))).await.json();
    assert_eq!(read["data"]["title"], json!("Art (grade )"), "a null field is left out");

    // Options: ordered by the template's fields, searched over each of them.
    let all = options(&admin, &app, "").await;
    assert_eq!(
        all,
        [
            (art.clone(), "Art (grade )".to_owned()),
            (math10.clone(), "Math (grade 10)".to_owned()),
            (math11.clone(), "Math (grade 11)".to_owned()),
        ]
    );
    assert_eq!(options(&admin, &app, "11").await, [(math11.clone(), "Math (grade 11)".to_owned())]);
    assert_eq!(options(&admin, &app, "mat").await.len(), 2);
    // As displayed: each word matches a field.
    assert_eq!(options(&admin, &app, "Math%2010").await, [(math10.clone(), "Math (grade 10)".to_owned())]);

    // Labels of a relationship value.
    let lesson =
        create(&admin, &app, "lessons", json!({"title": "Fractions", "subject_id": math10.parse::<i32>().unwrap()}))
            .await;
    let read = admin.get(&app.admin_api(&format!("resources/lessons/records/{lesson}"))).await.json();
    assert_eq!(read["data"]["labels"]["subject_id"], json!("Math (grade 10)"));

    // The schema names the template's fields.
    let schema = admin.get(&app.admin_api("resources/subjects/schema")).await.json();
    assert_eq!(schema["resource"]["titleFields"], json!(["name", "grade"]));
    assert!(schema["resource"].get("titleField").is_none());
});

db_test!(no_title_without_every_field_of_it, |db: TestDb| async move {
    let (app, users) = setup(db).await;
    let admin = app.acting_as(&users.admin).await;
    let math = create(&admin, &app, "subjects", json!({"name": "Math", "grade": 10})).await;
    let teacher = app.acting_as(&users.teacher).await;

    // The grade is hidden from teachers: no title (the panel shows the key),
    // and the grade cannot be guessed by searching it.
    let read = teacher.get(&app.admin_api(&format!("resources/subjects/records/{math}"))).await.json();
    assert_eq!(read["data"]["title"], Value::Null);
    assert!(read["data"]["data"].get("grade").is_none());
    assert!(options(&teacher, &app, "10").await.is_empty());
});

macro_rules! refused_at_boot {
    ($test:ident, $resource:ident, $message:literal, $template:literal) => {
        pub struct $resource;

        impl Resource for $resource {
            type Entity = subject::Entity;
            const SLUG: &'static str = "subjects";

            fn meta() -> ResourceMeta {
                ResourceMeta::new("Subject").title_template($template)
            }

            fn fields() -> Vec<Field> {
                subject_fields()
            }
        }

        #[test]
        #[should_panic(expected = $message)]
        fn $test() {
            Registry::new().register::<$resource>();
        }
    };
}

refused_at_boot!(an_unknown_field, UnknownField, "title field `level` is not a field", "{name} {level}");
refused_at_boot!(an_unclosed_brace, Unclosed, "the title `{name` has an unclosed `{`", "{name");
refused_at_boot!(no_field_at_all, NoField, "names no field", "Subject");
