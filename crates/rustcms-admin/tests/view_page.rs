//! The default view page (plan branch 4): a resource without `infolist()` is
//! shown by entries made from its form, each drawing its value as the form's
//! read-only view did. Writes `fixtures/contract/view-page.*.json`, which the
//! SPA's parity test draws both ways.

mod support;

use std::sync::Arc;

use rustcms_admin::schema::{ComponentSchema, ResourceSchema};
use rustcms_admin::*;
use rustcms_testing::{TestDb, db_test};
use sea_orm::{ConnectionTrait, Schema};
use serde_json::{Value, json};
use support::{CategoryResource, admin, category};

pub mod lesson {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "lessons")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub title: String,
        pub summary: Option<String>,
        pub pin: Option<String>,
        pub level: String,
        pub category_id: Option<i32>,
        pub draft: bool,
        /// A switch on a number column: not a boolean entry.
        pub stars: i32,
        pub body: Option<Json>,
        pub faq: Option<Json>,
        pub serial: i64,
        pub notes: Option<String>,
        pub cover: Option<String>,
        pub photos: Option<Json>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
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
        vec![
            Field::column(C::Title).required(),
            Field::column(C::Summary),
            Field::column(C::Pin),
            Field::column(C::Level),
            Field::column(C::CategoryId).belongs_to("categories"),
            Field::column(C::Draft),
            Field::column(C::Stars),
            Field::column(C::Body).rich_text(),
            Field::column(C::Faq),
            Field::column(C::Serial),
            Field::column(C::Notes).readable(|_| false),
            Field::column(C::Cover),
            Field::column(C::Photos).media_list(3),
        ]
    }

    fn form() -> Form {
        Form::new().columns(2).schema(components![
            Section::make("Basics").columns(2).schema(components![
                TextInput::make("title").help("Shown in lists").prefix("#"),
                Textarea::make("summary").column_span(2),
                TextInput::make("pin").password(),
                SelectInput::make("level").options([("easy", "Easy"), ("hard", "Hard")]),
                SelectInput::make("category_id"),
            ]),
            Tabs::make().schema(components![
                Tab::make("Content").schema(components![
                    RichEditor::make("body"),
                    Repeater::make("faq").schema(components![
                        TextInput::make("question"),
                        SelectInput::make("kind").options([("text", "Text"), ("link", "Link")]),
                        Hidden::make("ref"),
                        TextInput::make("url").shown_when(when("kind").eq("link")),
                        DatePicker::make("asked_on"),
                        Toggle::make("pinned"),
                        // Three deep, as forms allow.
                        Repeater::make("links").schema(components![
                            TextInput::make("href"),
                            Repeater::make("checks").schema(components![DateTimePicker::make("at")]),
                        ]),
                    ]),
                ]),
                // Holds only inputs shown nowhere on the view page: left out.
                Tab::make("Internal").schema(components![Hidden::make("serial")]),
            ]),
            Toggle::make("draft"),
            Toggle::make("stars"),
            // Shown under a condition naming a hidden input, of a big integer.
            TextInput::make("cover").shown_when(when("serial").eq("9007199254740993")),
            Textarea::make("notes"),
            FileUpload::make("photos"),
            // The same field twice: both shown.
            TextInput::make("title").label("Title again"),
        ])
    }
}

async fn registry(db: &TestDb) -> Arc<Registry> {
    let schema = Schema::new(db.conn().get_database_backend());
    for statement in [
        schema.create_table_from_entity(category::Entity),
        schema.create_table_from_entity(lesson::Entity),
        schema.create_table_from_entity(rustcms_admin::media::entity::Entity),
        schema.create_table_from_entity(rustcms_core::models::audit_log::Entity).if_not_exists().to_owned(),
        schema.create_table_from_entity(support::note::Entity),
    ] {
        db.conn().execute(&statement).await.unwrap();
    }
    Registry::new()
        .register::<CategoryResource>()
        .register::<LessonResource>()
        .register::<rustcms_admin::media::MediaResource>()
        .register::<support::NoteResource>()
        .finish()
}

fn entries(components: &Value, out: &mut Vec<Value>) {
    for component in components.as_array().unwrap() {
        match component.get("components") {
            Some(children) if !component["type"].as_str().unwrap().ends_with("_entry") => entries(children, out),
            _ => out.push(component.clone()),
        }
    }
}

db_test!(the_default_view_page_shows_the_form_read_only, |db: TestDb| async move {
    let registry = registry(&db).await;
    let ctx = Ctx::new(db.conn().clone(), admin(), registry.clone());
    let category = registry
        .get("categories")
        .unwrap()
        .create(&ctx, json!({"name": "Tools", "slug": "tools"}).as_object().unwrap().clone())
        .await
        .unwrap();
    let lessons = registry.get("lessons").unwrap();
    let input = json!({
        "title": "Fractions",
        "summary": "Line one\nLine two",
        "pin": "1234",
        "level": "hard",
        "category_id": category.key.parse::<i32>().unwrap(),
        "draft": false,
        "stars": 4,
        "body": {"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Hello"}]}]},
        "faq": [{
            "question": "Why?", "kind": "link", "ref": "r1", "url": "https://example.com", "asked_on": "2026-11-16", "pinned": true,
            "links": [{"href": "/a", "checks": [{"at": "2026-11-16T08:00:00Z"}]}],
        }],
        "serial": "9007199254740993",
        "notes": "private",
        "cover": "cover.png",
    });
    let created = lessons.create(&ctx, input.as_object().unwrap().clone()).await.unwrap();
    let view: ResourceSchema = lessons.schema(&ctx, SchemaOperation::View, Some(&created.key)).await.unwrap();
    let schema = serde_json::to_value(&view).unwrap();
    let infolist = &schema["infolist"];
    let mut all = Vec::new();
    entries(&infolist["components"], &mut all);
    let entry = |name: &str| all.iter().find(|e| e["name"] == name).unwrap_or_else(|| panic!("{name}")).clone();
    let kinds: Vec<(&str, &str)> =
        all.iter().map(|e| (e["name"].as_str().unwrap(), e["type"].as_str().unwrap())).collect();
    assert_eq!(
        kinds,
        [
            ("title", "text_entry"),
            ("summary", "text_entry"),
            ("pin", "text_entry"),
            ("level", "text_entry"),
            ("category_id", "relation_entry"),
            ("body", "rich_text_entry"),
            ("faq", "repeatable_entry"),
            ("draft", "boolean_entry"),
            // A switch on a number column: shown as text, never a boot failure.
            ("stars", "text_entry"),
            ("cover", "text_entry"),
            // A media list: thumbnails.
            ("photos", "media_entry"),
            // `notes` may not be read; `serial` and the internal tab show nothing.
            ("title", "text_entry"),
        ]
    );
    // Layouts and columns as the form's; a tab holding nothing shown is left out.
    assert_eq!(infolist["columns"], json!(2));
    assert_eq!(infolist["components"][0]["type"], json!("section"));
    assert_eq!(infolist["components"][1]["components"].as_array().unwrap().len(), 1);
    // Options as the form shows them: labels, prefixes, masks, line breaks; no help.
    let title = entry("title");
    assert_eq!((title["label"].clone(), title["prefix"].clone()), (json!("Title"), json!("#")));
    assert!(title.get("help").is_none());
    assert_eq!(entry("summary")["multiline"], json!(true));
    assert_eq!(entry("summary")["columnSpan"], json!(2));
    assert_eq!(entry("pin")["password"], json!(true));
    assert_eq!(entry("level")["options"][1], json!({"value": "hard", "label": "Hard"}));
    assert_eq!(entry("category_id")["relationship"], json!({"resource": "categories"}));
    assert_eq!(entry("cover")["shownWhen"], json!({"op": "eq", "name": "serial", "value": "9007199254740993"}));
    assert_eq!(all.last().unwrap()["label"], json!("Title again"));
    // Rows: entries of the row's inputs, of the row value's kind; the hidden one left out.
    let faq = entry("faq");
    let mut rows = Vec::new();
    entries(&faq["components"], &mut rows);
    let row: Vec<(&str, &str, &str)> = rows
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["type"].as_str().unwrap(), e["valueType"].as_str().unwrap()))
        .collect();
    assert_eq!(
        row,
        [
            ("question", "text_entry", "string"),
            ("kind", "text_entry", "string"),
            ("url", "text_entry", "string"),
            ("asked_on", "text_entry", "date"),
            ("pinned", "boolean_entry", "bool"),
            ("links", "repeatable_entry", "json"),
        ]
    );
    assert_eq!(rows[1]["options"][1], json!({"value": "link", "label": "Link"}));
    // The third level: rows of rows of rows, with their values' kinds.
    let checks = &rows[5]["components"][1];
    assert_eq!((checks["name"].clone(), checks["type"].clone()), (json!("checks"), json!("repeatable_entry")));
    assert_eq!(checks["components"][0]["valueType"], json!("instant"));
    assert_eq!(rows[2]["shownWhen"], json!({"op": "eq", "name": "kind", "value": "link"}));

    // The form is still sent; lists ask for `view` without a key and get no infolist.
    assert!(!view.form.components.is_empty());
    let keyless = lessons.schema(&ctx, SchemaOperation::View, None).await.unwrap();
    assert!(keyless.infolist.is_none());
    assert!(lessons.schema(&ctx, SchemaOperation::Edit, Some(&created.key)).await.unwrap().infolist.is_none());
    assert!(view.form.components.iter().all(|c| !matches!(c, ComponentSchema::Entry(_))));

    if db.backend() == rustcms_testing::Backend::Sqlite {
        let record = lessons.find(&ctx, &created.key).await.unwrap();
        support::contract("view-page.schema.view.json", &schema);
        support::contract("view-page.record.json", &serde_json::to_value(&record).unwrap());
    }
});

db_test!(links_and_media_lists_on_the_default_view_page, |db: TestDb| async move {
    let fixture = support::Fixture::new(db).await;
    let ctx = fixture.ctx(admin());
    let entries_of = |schema: &ResourceSchema| {
        let mut all = Vec::new();
        entries(&serde_json::to_value(schema).unwrap()["infolist"]["components"], &mut all);
        all.iter()
            .map(|e| (e["name"].as_str().unwrap().to_owned(), e["type"].as_str().unwrap().to_owned()))
            .collect::<Vec<_>>()
    };
    // A belongs-to-many select shows nothing: the links are in the relation section.
    let posts = fixture.resource("posts");
    let post = posts.create(&ctx, json!({"title": "Hello"}).as_object().unwrap().clone()).await.unwrap();
    let view = posts.schema(&ctx, SchemaOperation::View, Some(&post.key)).await.unwrap();
    assert!(!entries_of(&view).iter().any(|(name, _)| name == "tags"), "{:?}", entries_of(&view));
    assert!(view.relations.iter().any(|r| r.name == "tags"));
    // A resource without `form()`: its generated form has text inputs for media and rich
    // text fields (`auto_form`), so its view page shows them as text, as the read-only form did.
    let notes = fixture.resource("notes");
    let note = notes.create(&ctx, json!({"title": "Note"}).as_object().unwrap().clone()).await.unwrap();
    let view = notes.schema(&ctx, SchemaOperation::View, Some(&note.key)).await.unwrap();
    let made = entries_of(&view);
    assert_eq!(
        made.iter().map(|(n, k)| (n.as_str(), k.as_str())).collect::<Vec<_>>(),
        [
            ("title", "text_entry"),
            ("category_id", "relation_entry"),
            ("cover", "text_entry"),
            ("attachments", "text_entry"),
            ("body", "text_entry"),
        ]
    );
});
