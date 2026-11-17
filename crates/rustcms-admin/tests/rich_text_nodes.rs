//! Rich text version 3 (ADR 0042) in resources: fields accept the
//! application nodes they name, saves fill in defaults and refuse the rest
//! on every path, schemas announce the nodes, and declarations that cannot
//! hold refuse to boot. SQLite, PostgreSQL, MySQL.

use std::sync::Arc;

use axum::http::StatusCode;
use rustcms_admin::rich_text::{Attr, RichTextNode};
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, AppError, Config, Principal};
use rustcms_testing::{Factory, TestApp, TestDb, UserFactory, db_test};
use sea_orm::Schema;
use serde_json::{Value, json};
use support::PrincipalExt;

mod support;

pub mod question {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "questions")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub title: String,
        pub body: Option<Json>,
        pub intro: Option<Json>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn blank() -> RichTextNode {
    RichTextNode::inline_atom("app.blank")
        .label("Blank")
        .attr("n", Attr::integer().range(1..=99))
        .attr("size", Attr::one_of(["short", "long"]).default("short").label("Size"))
        .text(|attrs| format!("({})", attrs.integer("n").unwrap_or_default()))
}

fn card() -> RichTextNode {
    RichTextNode::block_atom("app.card").label("Card").attr("id", Attr::uuid())
}

struct QuestionResource;

#[async_trait::async_trait]
impl Resource for QuestionResource {
    type Entity = question::Entity;
    const SLUG: &'static str = "questions";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Question").title("title").audit(false)
    }

    fn fields() -> Vec<Field> {
        use question::Column as C;
        vec![
            Field::column(C::Title).required(),
            Field::column(C::Body).rich_text().nodes(["app.blank"]),
            // Rich text without application nodes.
            Field::column(C::Intro).rich_text(),
        ]
    }

    fn form() -> Form {
        Form::new().schema(components![TextInput::make("title"), RichEditor::make("body"), RichEditor::make("intro"),])
    }

    /// A hook writing a document without the defaults.
    async fn mutate_before_create(
        _ctx: &Ctx,
        _txn: &sea_orm::DatabaseTransaction,
        mut data: Data,
    ) -> rustcms_core::AppResult<Data> {
        if data.get("title") == Some(&json!("hook")) {
            data.insert("body".into(), doc(3, json!({"type": "app.blank", "attrs": {"n": 5}})));
        }
        Ok(data)
    }
}

async fn registry(db: &TestDb) -> Arc<Registry> {
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(question::Entity)).await;
    Registry::new().rich_text_node(blank()).register::<QuestionResource>().rich_text_node(card()).finish()
}

fn data(value: Value) -> Data {
    value.as_object().unwrap().clone()
}

fn doc(version: u64, inline: Value) -> Value {
    json!({"type": "doc", "version": version, "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "The capital is "}, inline
    ]}]})
}

/// The field issues of a validation error, as JSON.
fn issues(err: AppError) -> Value {
    match err {
        AppError::Validation(errors) => serde_json::to_value(errors.issues()).unwrap(),
        other => panic!("not a validation error: {other:?}"),
    }
}

fn editor() -> Principal {
    Principal { id: "7".into(), guard: "admin".into(), ..Default::default() }.with(&[
        "questions.view_any",
        "questions.view",
        "questions.create",
        "questions.update",
    ])
}

db_test!(saves_fill_in_defaults_and_refuse_what_a_field_does_not_accept, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry.clone());
    let questions = registry.get("questions").unwrap();
    let blank = json!({"type": "app.blank", "attrs": {"n": 1}});
    let created = questions
        .create(&ctx, data(json!({"title": "Q", "body": doc(3, blank.clone()), "intro": doc(3, json!({"type": "text", "text": "x", "marks": [{"type": "underline"}]}))})))
        .await
        .unwrap();
    // Stored complete: the default filled in.
    let filled = doc(3, json!({"type": "app.blank", "attrs": {"n": 1, "size": "short"}}));
    assert_eq!(created.data["body"], filled);
    let shown = questions.find(&ctx, &created.key).await.unwrap();
    assert_eq!(shown.data["body"], filled);
    // Updates too, and an explicit value is kept.
    let long = json!({"type": "app.blank", "attrs": {"n": 2, "size": "long"}});
    let updated = questions.update(&ctx, &created.key, data(json!({"body": doc(3, long.clone())}))).await.unwrap();
    assert_eq!(updated.data["body"], doc(3, long));
    // A document written by a hook is completed the same way.
    let hooked = questions.create(&ctx, data(json!({"title": "hook"}))).await.unwrap();
    let stored = questions.find(&ctx, &hooked.key).await.unwrap();
    assert_eq!(stored.data["body"], doc(3, json!({"type": "app.blank", "attrs": {"n": 5, "size": "short"}})));

    for (input, field, message, code) in [
        // A field without nodes, and a node the field does not accept.
        (json!({"intro": doc(3, blank.clone())}), "intro", "has an unknown element `app.blank`", "document_element"),
        (
            json!({"body": doc(3, json!({"type": "app.card", "attrs": {"id": "0199d3a4-1b2c-7d3e-8f40-123456789abc"}}))}),
            "body",
            "has an unknown element `app.card`",
            "document_element",
        ),
        (
            json!({"body": doc(2, blank.clone())}),
            "body",
            "has the element `app.blank`, which needs document version 3",
            "document_needs_version",
        ),
        (
            json!({"body": doc(3, json!({"type": "app.blank", "attrs": {"n": 100}}))}),
            "body",
            "has an element `app.blank` with an invalid attribute `n`",
            "document_element",
        ),
        (
            json!({"body": doc(3, json!({"type": "app.blank", "attrs": {"size": "long"}}))}),
            "body",
            "has an element `app.blank` without its attribute `n`",
            "document_element",
        ),
        (
            json!({"intro": doc(2, json!({"type": "text", "text": "x", "marks": [{"type": "superscript"}]}))}),
            "intro",
            "has the mark `superscript`, which needs document version 3",
            "document_needs_version",
        ),
    ] {
        let err = questions.update(&ctx, &created.key, data(input.clone())).await.unwrap_err();
        let issues = issues(err);
        let issue = rustcms_core::i18n::Issue::of(message);
        assert_eq!(issue.code, code, "{message}");
        assert_eq!(issues[field], json!([issue]), "{input}");
    }
});

db_test!(documents_are_stored_in_the_lowest_version_they_need, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry.clone());
    let questions = registry.get("questions").unwrap();
    // Written as version 3 with only version 1 parts (the API, an import, a hook):
    // stored as version 1, so 0.10 to 0.13 readers keep reading it (ADR 0008).
    let plain = doc(3, json!({"type": "text", "text": "Paris"}));
    let created = questions
        .create(
            &ctx,
            data(
                json!({"title": "Q", "intro": plain, "body": doc(3, json!({"type": "app.blank", "attrs": {"n": 1}}))}),
            ),
        )
        .await
        .unwrap();
    let stored = questions.find(&ctx, &created.key).await.unwrap();
    assert_eq!(stored.data["intro"]["version"], json!(1));
    assert_eq!(stored.data["intro"]["content"], doc(1, json!({"type": "text", "text": "Paris"}))["content"]);
    // A document that needs version 3 stays version 3.
    assert_eq!(stored.data["body"]["version"], json!(3));
});

db_test!(defaults_may_not_grow_a_document_past_its_limit, |db: TestDb| async move {
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(question::Entity)).await;
    let note = RichTextNode::inline_atom("app.blank").attr("n", Attr::text().max(1000).default("x".repeat(1000)));
    let registry = Registry::new().rich_text_node(note).register::<QuestionResource>().finish();
    let ctx = Ctx::new(db.conn().clone(), editor(), registry.clone());
    let questions = registry.get("questions").unwrap();
    // Under 10 KB sent, over 256 KiB once completed: refused, nothing stored.
    let many = json!({"type": "doc", "version": 3, "content": [{"type": "paragraph", "content": vec![json!({"type": "app.blank"}); 300]}]});
    assert!(serde_json::to_vec(&many).unwrap().len() < 10_000);
    let err = questions.create(&ctx, data(json!({"title": "big", "body": many}))).await.unwrap_err();
    assert_eq!(issues(err)["body"][0]["code"], json!("max_kib"));
    assert_eq!(questions.list(&ctx, &ListQuery::default()).await.unwrap().meta.total, 0);
});

db_test!(schemas_announce_the_nodes_a_field_accepts, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry.clone());
    let questions = registry.get("questions").unwrap();
    let key = questions.create(&ctx, data(json!({"title": "Q"}))).await.unwrap().key;
    // `richNodes` of every component, by name.
    let nodes = |schema: &ResourceSchema| -> Vec<(String, Value)> {
        fn walk(value: &Value, out: &mut Vec<(String, Value)>) {
            if let Some(nodes) = value.get("richNodes") {
                out.push((value["name"].as_str().unwrap().to_owned(), nodes.clone()));
            }
            for (_, child) in value.as_object().into_iter().flatten() {
                for item in child.as_array().into_iter().flatten() {
                    walk(item, out);
                }
                if child.is_object() {
                    walk(child, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&serde_json::to_value(schema).unwrap(), &mut out);
        out
    };
    let expected = json!([{"name": "app.blank", "label": "Blank", "inline": true, "attrs": [
        // Without a label, its name; with one, the label.
        {"name": "n", "label": "n", "kind": "integer", "required": true, "min": 1, "max": 99},
        {"name": "size", "label": "Size", "kind": "one_of", "required": false, "default": "short", "options": ["short", "long"]},
    ]}]);
    for (operation, key) in [(SchemaOperation::Create, None), (SchemaOperation::Edit, Some(key.as_str()))] {
        let schema = questions.schema(&ctx, operation, key).await.unwrap();
        assert_eq!(nodes(&schema), [("body".to_owned(), expected.clone())], "{operation:?}");
    }
    // The view page's entry.
    let view = questions.schema(&ctx, SchemaOperation::View, Some(&key)).await.unwrap();
    assert!(nodes(&view).contains(&("body".to_owned(), expected)), "{:?}", nodes(&view));
    // The panel's table, for rendering with the same declarations.
    let names: Vec<&str> = registry.rich_nodes().iter().map(|n| n.name()).collect();
    assert_eq!(names, ["app.blank", "app.card"]);
    // Labels are listed for translation.
    assert!(questions.display_strings().contains(&(true, "Blank".to_owned())));
    assert!(!questions.display_strings().iter().any(|(_, s)| s == "Card"));
});

fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    app.provider(AuthProvider::new());
    app.provider(AdminProvider::new().resource::<QuestionResource>().rich_text_node(blank()).rich_text_node(card()));
    app
}

db_test!(the_api_checks_and_completes_documents, |db: TestDb| async move {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(question::Entity)).await;
    let app = TestApp::new(build, db).await;
    let auth = app.state().service::<AuthState>().unwrap();
    let conn = app.state().db();
    let rbac = RbacService::new(conn, &auth.resolver);
    rbac.sync_permissions(&auth.permissions.all()).await.unwrap();
    let admin = UserFactory::new().name("Admin").create(conn).await;
    let super_admin = rustcms_auth::entities::role::super_admin_id(conn, ADMIN_GUARD).await.unwrap().unwrap();
    rbac.assign_role(Actor::System, admin.id, super_admin).await.unwrap();
    let client = app.acting_as(&admin).await;
    let url = app.admin_api("resources/questions/records");
    let created = client
        .post_json(&url, &json!({"title": "Q", "body": doc(3, json!({"type": "app.blank", "attrs": {"n": 3}}))}))
        .await;
    created.assert_status(StatusCode::CREATED);
    let body = created.json();
    assert_eq!(
        body["data"]["data"]["body"]["content"][0]["content"][1]["attrs"],
        json!({"n": 3, "size": "short"}),
        "{body}"
    );
    let refused = client
        .post_json(&url, &json!({"title": "Q", "intro": doc(3, json!({"type": "app.blank", "attrs": {"n": 3}}))}))
        .await;
    refused.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.json()["errors"]["intro"][0], json!("has an unknown element `app.blank`"), "{}", refused.json());
});

/// A registry whose question resource declares `fields`.
fn boot(fields: fn() -> Vec<Field>, nodes: Vec<RichTextNode>) {
    struct Bad;
    static FIELDS: std::sync::OnceLock<fn() -> Vec<Field>> = std::sync::OnceLock::new();
    FIELDS.set(fields).ok();
    impl Resource for Bad {
        type Entity = question::Entity;
        const SLUG: &'static str = "bad";
        fn meta() -> ResourceMeta {
            ResourceMeta::new("Bad")
        }
        fn fields() -> Vec<Field> {
            FIELDS.get().unwrap()()
        }
    }
    let mut registry = Registry::new();
    for node in nodes {
        registry = registry.rich_text_node(node);
    }
    registry.register::<Bad>().finish();
}

#[test]
#[should_panic(expected = "accepts the rich text node `app.card`, which is not declared")]
fn a_field_names_declared_nodes_only() {
    boot(|| vec![Field::column(question::Column::Body).rich_text().nodes(["app.card"])], vec![blank()]);
}

#[test]
#[should_panic(expected = "accepts rich text nodes but is not `rich_text()`")]
fn nodes_need_a_rich_text_field() {
    boot(|| vec![Field::column(question::Column::Body).nodes(["app.blank"])], vec![blank()]);
}

#[test]
#[should_panic(expected = "names a rich text node twice")]
fn a_field_names_a_node_once() {
    boot(|| vec![Field::column(question::Column::Body).rich_text().nodes(["app.blank", "app.blank"])], vec![blank()]);
}

#[test]
#[should_panic(expected = "declared twice")]
fn a_node_is_declared_once() {
    boot(|| vec![Field::column(question::Column::Body).rich_text()], vec![blank(), blank()]);
}
