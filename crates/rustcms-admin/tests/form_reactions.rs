//! Form reactions (ADR 0043): who may ask, what the reaction sees, which
//! patches are kept, that saves never run them, the route's limit, and
//! declarations that cannot hold refusing to boot. SQLite, PostgreSQL, MySQL.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use rustcms_admin::schema::{FormReactRequest, FormReactResponse};
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, AppError, Config, Principal};
use rustcms_testing::{Factory, TestApp, TestDb, UserFactory, db_test};
use sea_orm::{QueryFilter, Schema};
use serde_json::{Value, json};
use support::PrincipalExt;

mod support;

pub mod article {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "articles")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub title: String,
        pub slug: Option<String>,
        pub status: Option<String>,
        pub summary: Option<String>,
        pub lines: Option<Json>,
        pub total: Option<i32>,
        pub note: Option<String>,
        pub locked: Option<String>,
        pub tier: Option<String>,
        pub discount: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// `Hello World` → `hello-world`.
fn slugify(text: &str) -> String {
    text.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join("-")
}

/// The lines' total, and a warning under lines without a quantity.
fn totals(state: &FormState) -> Vec<FormPatch> {
    let rows = state.rows("lines");
    let total: i64 = rows.iter().map(|r| r["qty"].as_i64().unwrap_or(0)).sum();
    let mut patches = vec![FormPatch::set("total", total)];
    for (index, row) in rows.iter().enumerate() {
        if row["qty"].as_i64().unwrap_or(0) == 0 {
            patches.push(FormPatch::flag_row("lines", index as u32, "No quantity"));
        }
    }
    patches
}

/// Every kind of patch, most of which the server must drop.
fn probe(state: &FormState) -> Vec<FormPatch> {
    vec![
        FormPatch::set("nope", 1),                                                 // not an input
        FormPatch::set("locked", "x"),                                             // not writable
        FormPatch::set("note", "x"),       // writable, not readable: never sent to the user
        FormPatch::set("summary", "s"),    // hidden unless live
        FormPatch::set("status", "other"), // not an option
        FormPatch::set("total", "abc"),    // not a number
        FormPatch::set("slug", state.get("slug").cloned().unwrap_or(Value::Null)), // no change
        FormPatch::set_if_empty("title", "T"), // filled
        FormPatch::add_row("total", json!({})), // not a repeater
        FormPatch::remove_row("lines", 9), // out of range
        FormPatch::add_row("lines", json!({"qty": 1})), // a row without its name
        FormPatch::set("total", 5),
        FormPatch::add_row_at("lines", 0, json!({"name": "a", "qty": 1})),
        FormPatch::move_row("lines", 0, 1),
        FormPatch::move_row("lines", 1, 1), // no change
        FormPatch::flag_row("lines", 0, "Check"),
        FormPatch::notice(format!("note seen: {}", state.get("note").is_some())),
    ]
}

pub struct ArticleResource;

impl Resource for ArticleResource {
    type Entity = article::Entity;
    const SLUG: &'static str = "articles";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Article").title("title").audit(false).importable()
    }

    fn fields() -> Vec<Field> {
        use article::Column as C;
        vec![
            Field::column(C::Title).required().max_length(60),
            Field::column(C::Slug),
            Field::column(C::Status),
            Field::column(C::Summary),
            Field::column(C::Lines),
            Field::column(C::Total),
            // Writable, never read: reactions do not see it.
            Field::column(C::Note).readable(|_| false),
            Field::column(C::Locked).creatable(|_| false).updatable(|_| false),
            // Internal: nobody reads or writes it from the panel; it shows the discount.
            Field::column(C::Tier).readable(|_| false).creatable(|_| false).updatable(|_| false),
            Field::column(C::Discount),
        ]
    }

    fn form() -> Form {
        Form::new().schema(components![
            TextInput::make("title")
                .reacts(|_, state| {
                    let title = state.text("title").to_owned();
                    Box::pin(async move {
                        match title.as_str() {
                            "boom" => Err(AppError::internal("the reaction broke")),
                            "slow" => {
                                tokio::time::sleep(Duration::from_millis(300)).await;
                                Ok(Vec::new())
                            }
                            // Conditions follow the patches: live first shows the summary.
                            "go live" => {
                                Ok(vec![FormPatch::set("status", "live"), FormPatch::set("summary", "Now live")])
                            }
                            "many" => Ok((0..300).map(|i| FormPatch::notice(format!("n{i}"))).collect()),
                            "long" => Ok(vec![FormPatch::notice("x".repeat(501)), FormPatch::notice("short")]),
                            _ => Ok(vec![
                                FormPatch::set_if_empty("slug", slugify(&title)),
                                // Shown only for gold tiers: never for a user who cannot read the tier.
                                FormPatch::set("discount", "10%"),
                            ]),
                        }
                    })
                })
                .reaction_timeout(Duration::from_millis(100)),
            TextInput::make("slug"),
            SelectInput::make("status").options([("draft", "Draft"), ("live", "Live")]).reacts(|_, state| {
                let patches = probe(state);
                Box::pin(async move { Ok(patches) })
            }),
            // Hidden unless live.
            TextInput::make("summary").shown_when(when("status").eq("live")).reacts(|_, state| {
                let previous = state.previous().cloned();
                Box::pin(async move { Ok(vec![FormPatch::notice(format!("previous: {previous:?}"))]) })
            }),
            Repeater::make("lines")
                .schema(components![TextInput::make("name").required(), NumberInput::make("qty").integer()])
                .max_items(3)
                .reacts(|_, state| {
                    let fail = state.text("title") == "fail";
                    let patches = totals(state);
                    Box::pin(async move { if fail { Err(AppError::internal("no totals today")) } else { Ok(patches) } })
                }),
            NumberInput::make("total"),
            TextInput::make("note"),
            TextInput::make("locked"),
            Hidden::make("tier"),
            TextInput::make("discount").shown_when(when("tier").eq("gold")),
        ])
    }

    fn scope(query: sea_orm::Select<article::Entity>, _ctx: &Ctx) -> sea_orm::Select<article::Entity> {
        use sea_orm::ColumnTrait;
        query.filter(article::Column::Title.not_like("Hidden%"))
    }
}

async fn registry(db: &TestDb) -> Arc<Registry> {
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(article::Entity)).await;
    Registry::new().register::<ArticleResource>().finish()
}

fn data(value: Value) -> Data {
    value.as_object().unwrap().clone()
}

fn request(value: Value) -> FormReactRequest {
    serde_json::from_value(value).unwrap()
}

async fn react(ctx: &Ctx, value: Value) -> Result<FormReactResponse, AppError> {
    ctx.registry.get("articles").unwrap().react(ctx, request(value)).await
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
        "articles.view_any",
        "articles.view",
        "articles.create",
        "articles.update",
    ])
}

db_test!(a_reaction_answers_with_patches_the_user_could_make, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry);
    let answer =
        react(&ctx, json!({"operation": "create", "changed": "title", "state": {"title": "Hello World"}, "seq": 3}))
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({"seq": 3, "patches": [
            {"op": "set_if_empty", "field": "slug", "value": "hello-world"},
        ]})
    );
    // A slug typed by the user is kept: nothing to change.
    let answer = react(
        &ctx,
        json!({"operation": "create", "changed": "title", "state": {"title": "Hello", "slug": "mine"}, "seq": 4}),
    )
    .await
    .unwrap();
    assert!(answer.patches.is_empty() && !answer.failed);

    // The probe: each patch the user could not make is dropped, the others
    // kept in order, each applied for the next (indexes, no-ops).
    let answer = react(
        &ctx,
        json!({"operation": "create", "changed": "status", "seq": 5, "state": {
            "title": "Probe", "slug": "probe", "status": "draft", "note": "secret",
            "lines": [{"name": "b", "qty": 2}], "total": "not a number yet",
        }}),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&answer.patches).unwrap(),
        json!([
            {"op": "set", "field": "total", "value": 5},
            {"op": "add_row", "field": "lines", "index": 0, "row": {"name": "a", "qty": 1}},
            {"op": "move_row", "field": "lines", "from": 0, "to": 1},
            {"op": "flag_row", "field": "lines", "index": 0, "message": "Check"},
            // The reaction never sees a field the user may not read.
            {"op": "notice", "message": "note seen: false"},
        ])
    );
});

db_test!(a_reaction_runs_for_whoever_may_write_the_form, |db: TestDb| async move {
    let registry = registry(&db).await;
    let admin = Ctx::new(db.conn().clone(), support::admin(), registry.clone());
    let articles = registry.get("articles").unwrap();
    let own = articles.create(&admin, data(json!({"title": "Mine", "slug": "mine", "total": 0}))).await.unwrap().key;
    let hidden =
        articles.create(&admin, data(json!({"title": "Hidden one", "slug": "h", "total": 0}))).await.unwrap().key;
    let create = json!({"operation": "create", "changed": "title", "state": {"title": "X"}, "seq": 1});
    let edit =
        |key: &str| json!({"operation": "edit", "key": key, "changed": "title", "state": {"title": "X"}, "seq": 1});

    let reader = Ctx::new(db.conn().clone(), editor().without_all().with(&["articles.view"]), registry.clone());
    assert!(matches!(react(&reader, create.clone()).await, Err(AppError::Forbidden)));
    assert!(matches!(react(&reader, edit(&own)).await, Err(AppError::Forbidden)));

    let editor = Ctx::new(db.conn().clone(), editor(), registry.clone());
    react(&editor, create).await.unwrap();
    // On edit, the fields not sent are the stored ones.
    let answer = react(
        &editor,
        json!({"operation": "edit", "key": own, "changed": "title", "state": {"title": "Renamed"}, "seq": 1}),
    )
    .await
    .unwrap();
    assert!(answer.patches.is_empty(), "the stored slug is filled: {answer:?}");
    // Outside the scope: not found.
    assert!(matches!(react(&editor, edit(&hidden)).await, Err(AppError::NotFound)));
    // Never for a view page; a key on edit only; an input that reacts.
    for (body, field) in [
        (json!({"operation": "view", "key": own, "changed": "title", "state": {}, "seq": 1}), "operation"),
        (json!({"operation": "edit", "changed": "title", "state": {}, "seq": 1}), "operation"),
        (json!({"operation": "create", "key": own, "changed": "title", "state": {}, "seq": 1}), "operation"),
        (json!({"operation": "create", "changed": "slug", "state": {}, "seq": 1}), "changed"),
        (json!({"operation": "create", "changed": "locked", "state": {}, "seq": 1}), "changed"),
    ] {
        let issues = issues(react(&editor, body).await.unwrap_err());
        assert!(issues.get(field).is_some(), "{issues}");
    }
});

db_test!(a_failing_reaction_answers_failed, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry);
    for title in ["boom", "slow"] {
        let answer =
            react(&ctx, json!({"operation": "create", "changed": "title", "state": {"title": title}, "seq": 9}))
                .await
                .unwrap();
        assert_eq!(serde_json::to_value(&answer).unwrap(), json!({"seq": 9, "patches": [], "failed": true}));
    }
    // A value that does not decode is left out (the form is half typed).
    let answer = react(
        &ctx,
        json!({"operation": "create", "changed": "lines", "state": {"lines": [{"name": "a", "qty": 2}], "total": "2x"}, "seq": 1}),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(&answer.patches).unwrap(), json!([{"op": "set", "field": "total", "value": 2}]));
});

db_test!(a_reaction_reads_only_what_the_users_form_holds, |db: TestDb| async move {
    let registry = registry(&db).await;
    let admin = Ctx::new(db.conn().clone(), support::admin(), registry.clone());
    let articles = registry.get("articles").unwrap();
    let key = articles.create(&admin, data(json!({"title": "Gold", "slug": "gold", "total": 0}))).await.unwrap().key;
    // The tier no panel user may read: gold, which shows the discount.
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let stored = article::Entity::find_by_id(key.parse::<i32>().unwrap()).one(db.conn()).await.unwrap().unwrap();
    let mut model: article::ActiveModel = stored.into();
    model.tier = Set(Some("gold".into()));
    model.update(db.conn()).await.unwrap();
    let edit =
        |title: &str| json!({"operation": "edit", "key": key, "changed": "title", "state": {"title": title}, "seq": 1});
    let fields = |answer: &FormReactResponse| -> Vec<String> {
        serde_json::to_value(&answer.patches)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["field"].as_str().map(str::to_owned))
            .collect()
    };
    // Conditions read the tier as the user's form holds it (empty): the discount
    // stays hidden and its patch is dropped, whatever is stored.
    let viewer = Ctx::new(db.conn().clone(), editor(), registry.clone());
    let answer = react(&viewer, edit("Gold two")).await.unwrap();
    assert!(fields(&answer).is_empty(), "{answer:?}");
    // A user who may update the record but not view it: its stored values are
    // not read (the slug is empty in their form), and nothing tells the tier.
    let blind = Ctx::new(db.conn().clone(), editor().without_all().with(&["articles.update"]), registry.clone());
    let answer = react(&blind, edit("New name")).await.unwrap();
    assert_eq!(
        serde_json::to_value(&answer.patches).unwrap(),
        json!([{"op": "set_if_empty", "field": "slug", "value": "new-name"}])
    );
    // Values sent for fields the user may not write are not the form's: the
    // tier sent as gold changes nothing.
    let mut sent = edit("Gold three");
    sent["state"]["tier"] = json!("gold");
    assert!(fields(&react(&viewer, sent).await.unwrap()).is_empty());
});

db_test!(patches_follow_the_form_and_stay_bounded, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry);
    let create = |title: &str| json!({"operation": "create", "changed": "title", "state": {"title": title}, "seq": 1});
    // Live first, then the summary it shows: both kept.
    let answer = react(&ctx, create("go live")).await.unwrap();
    assert_eq!(
        serde_json::to_value(&answer.patches).unwrap(),
        json!([
            {"op": "set", "field": "status", "value": "live"},
            {"op": "set", "field": "summary", "value": "Now live"},
        ])
    );
    // At most 200 patches; a message of more than 500 characters is dropped.
    assert_eq!(react(&ctx, create("many")).await.unwrap().patches.len(), 200);
    assert_eq!(
        serde_json::to_value(&react(&ctx, create("long")).await.unwrap().patches).unwrap(),
        json!([{"op": "notice", "message": "short"}])
    );
    // More rows than the repeater may hold: refused before the reaction runs.
    let rows = vec![json!({"name": "a", "qty": 1}); 4];
    let err = react(&ctx, json!({"operation": "create", "changed": "lines", "state": {"lines": rows}, "seq": 1}))
        .await
        .unwrap_err();
    assert_eq!(issues(err), json!({"lines": [rustcms_core::i18n::Issue::of("may not have more than 3 items")]}));
    // The changed input's previous value reaches the reaction, when it decodes.
    let answer = react(
        &ctx,
        json!({"operation": "create", "changed": "summary", "state": {"status": "live", "summary": "B"}, "previous": "A", "seq": 1}),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&answer.patches).unwrap(),
        json!([{"op": "notice", "message": "previous: Some(String(\"A\"))"}])
    );
});

db_test!(saves_never_run_reactions, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry.clone());
    let articles = registry.get("articles").unwrap();
    // A total that does not match the lines, and a title whose reaction fails:
    // reactions help the user fill the form, they do not refuse saves.
    articles
        .create(&ctx, data(json!({"title": "boom", "lines": [{"name": "a", "qty": 2}], "total": 3})))
        .await
        .unwrap();
});

db_test!(schemas_announce_reactions_on_forms_the_user_may_change, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), editor(), registry.clone());
    let articles = registry.get("articles").unwrap();
    let key = articles.create(&ctx, data(json!({"title": "A", "total": 0}))).await.unwrap().key;
    let reacting = |schema: &ResourceSchema| -> Vec<String> {
        let text = serde_json::to_value(&schema.form).unwrap();
        fn walk(value: &Value, out: &mut Vec<String>) {
            if value["reacts"] == json!(true) {
                out.push(value["name"].as_str().unwrap().to_owned());
            }
            for child in value["components"].as_array().into_iter().flatten() {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        walk(&text, &mut out);
        out
    };
    let create = articles.schema(&ctx, SchemaOperation::Create, None).await.unwrap();
    assert_eq!(reacting(&create), ["title", "status", "summary", "lines"]);
    let edit = articles.schema(&ctx, SchemaOperation::Edit, Some(&key)).await.unwrap();
    assert_eq!(reacting(&edit), ["title", "status", "summary", "lines"]);
    let view = articles.schema(&ctx, SchemaOperation::View, Some(&key)).await.unwrap();
    assert!(reacting(&view).is_empty());
});

fn build(config: Config) -> AppBuilder {
    // Uploaded CSV files (imports) go to a temporary directory of this app.
    let root = std::env::temp_dir().join(format!("rustcms-reactions-{}", uuid::Uuid::now_v7()));
    let config = config.with("storage.root", root.to_string_lossy()).expect("storage config");
    let mut app = AppBuilder::new(config);
    app.provider(rustcms_core::storage::StorageProvider);
    app.provider(AuthProvider::new());
    app.provider(AdminProvider::new().resource::<ArticleResource>());
    app
}

/// The app with the articles, and its super admin signed in.
async fn served(db: TestDb) -> (TestApp, rustcms_core::models::user::Model) {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(article::Entity)).await;
    let app = TestApp::new(build, db).await;
    let auth = app.state().service::<AuthState>().unwrap();
    let conn = app.state().db();
    let rbac = RbacService::new(conn, &auth.resolver);
    rbac.sync_permissions(&auth.permissions.all()).await.unwrap();
    let admin = UserFactory::new().name("Admin").create(conn).await;
    let super_admin = rustcms_auth::entities::role::super_admin_id(conn, ADMIN_GUARD).await.unwrap().unwrap();
    rbac.assign_role(Actor::System, admin.id, super_admin).await.unwrap();
    (app, admin)
}

db_test!(the_route_is_limited_per_user, |db: TestDb| async move {
    let (app, admin) = served(db).await;
    let client = app.acting_as(&admin).await;
    let url = app.admin_api("resources/articles/form/react");
    let body = json!({"operation": "create", "changed": "title", "state": {"title": "Hi there"}, "seq": 1});
    let first = client.post_json(&url, &body).await;
    first.assert_status(StatusCode::OK);
    assert_eq!(first.json()["patches"][0]["value"], json!("hi-there"));
    for _ in 1..120 {
        client.post_json(&url, &body).await.assert_status(StatusCode::OK);
    }
    client.post_json(&url, &body).await.assert_status(StatusCode::TOO_MANY_REQUESTS);
});

/// A resource whose form is `form`, registered to check it at boot.
fn boot(form: fn() -> Form) {
    struct Bad;
    static FORM: std::sync::OnceLock<fn() -> Form> = std::sync::OnceLock::new();
    FORM.set(form).ok();
    impl Resource for Bad {
        type Entity = article::Entity;
        const SLUG: &'static str = "bad";
        fn meta() -> ResourceMeta {
            ResourceMeta::new("Bad")
        }
        fn fields() -> Vec<Field> {
            use article::Column as C;
            vec![Field::column(C::Title), Field::column(C::Note), Field::column(C::Lines)]
        }
        fn form() -> Form {
            FORM.get().unwrap()()
        }
    }
    Registry::new().register::<Bad>().finish();
}

fn nothing<'a>(_: &'a Ctx, _: &'a FormState) -> BoxFuture<'a, rustcms_core::AppResult<Vec<FormPatch>>> {
    Box::pin(async { Ok(Vec::new()) })
}

#[test]
#[should_panic(expected = "needs `.reacts()`")]
fn a_timeout_needs_a_reaction() {
    boot(|| Form::new().schema(components![TextInput::make("title").reaction_timeout(Duration::from_secs(1))]));
}

#[test]
#[should_panic(expected = "at most")]
fn the_timeout_is_bounded() {
    boot(|| {
        Form::new()
            .schema(components![TextInput::make("title").reacts(nothing).reaction_timeout(Duration::from_secs(31))])
    });
}

#[test]
#[should_panic(expected = "only inputs of fields react")]
fn hidden_inputs_do_not_react() {
    boot(|| Form::new().schema(components![Hidden::make("note").reacts(nothing)]));
}

#[test]
#[should_panic(expected = "cannot react")]
fn row_inputs_do_not_react() {
    boot(|| {
        Form::new()
            .schema(components![Repeater::make("lines").schema(components![TextInput::make("name").reacts(nothing)])])
    });
}
