//! `min`, `max` and `step` of number inputs: shown in the schema, `min` and
//! `max` bound client writes to a resource (hooks may write any value);
//! declarations that cannot hold refuse to boot. SQLite, PostgreSQL, MySQL.

use axum::http::StatusCode;
use rustcms_admin::*;
use rustcms_auth::{ADMIN_GUARD, Actor, AuthProvider, AuthState, RbacService};
use rustcms_core::{AppBuilder, AppResult, Config};
use rustcms_testing::{Factory, TestApp, TestClient, TestDb, TestResponse, UserFactory, db_test};
use sea_orm::{EntityTrait, Schema};
use serde_json::{Value, json};

pub mod gauge {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "gauges")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub level: i32,
        pub reading: Option<i64>,
        #[sea_orm(column_type = "Decimal(Some((10, 2)))", nullable)]
        pub price: Option<Decimal>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn gauge_fields() -> Vec<Field> {
    use gauge::Column as C;
    vec![
        Field::column(C::Name).required(),
        Field::column(C::Level).required(),
        Field::column(C::Reading),
        Field::column(C::Price),
    ]
}

pub struct GaugeResource;

#[async_trait::async_trait]
impl Resource for GaugeResource {
    type Entity = gauge::Entity;
    const SLUG: &'static str = "gauges";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Gauge").title("name")
    }

    fn fields() -> Vec<Field> {
        gauge_fields()
    }

    fn form() -> Form {
        Form::new().schema(components![
            TextInput::make("name"),
            NumberInput::make("level").min(1).max(5),
            NumberInput::make("reading").max(1000).step(10),
            NumberInput::make("price").min(0.5).step(0.01),
        ])
    }

    /// A gauge named `hooked` gets a level out of the bounds from the hook.
    async fn mutate_before_create(_ctx: &Ctx, _txn: &sea_orm::DatabaseTransaction, mut data: Data) -> AppResult<Data> {
        if data.get("name") == Some(&json!("hooked")) {
            data.insert("level".into(), json!(9));
        }
        Ok(data)
    }
}

fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    app.provider(AuthProvider::new());
    app.provider(AdminProvider::new().resource::<GaugeResource>());
    app
}

async fn setup(db: TestDb) -> (TestApp, rustcms_core::models::user::Model) {
    db.migrate::<rustcms_migration::Migrator>().await;
    let schema = Schema::new(db.conn().get_database_backend());
    rustcms_testing::create_table(db.conn(), &schema.create_table_from_entity(gauge::Entity)).await;
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

async fn create(client: &TestClient<'_>, app: &TestApp, body: Value) -> TestResponse {
    client.post_json(&app.admin_api("resources/gauges/records"), &body).await
}

/// The first issue of `field` in a 422: code and parameters.
fn issue(response: &TestResponse, field: &str) -> Value {
    response.assert_validation_error(field);
    response.json()["issues"][field][0].clone()
}

db_test!(client_writes_stay_within_the_bounds, |db: TestDb| async move {
    let (app, admin) = setup(db).await;
    let client = app.acting_as(&admin).await;

    let refused = [
        (json!({"level": 0}), "level", json!({"code": "min_value", "params": {"min": "1"}})),
        (json!({"level": 6}), "level", json!({"code": "max_value", "params": {"max": "5"}})),
        (json!({"reading": "1001"}), "reading", json!({"code": "max_value", "params": {"max": "1000"}})),
        (json!({"price": "0.49"}), "price", json!({"code": "min_value", "params": {"min": "0.5"}})),
    ];
    for (patch, field, expected) in refused {
        let mut body = json!({"name": "a", "level": 3});
        body.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
        assert_eq!(issue(&create(&client, &app, body.clone()).await, field), expected, "{body}");
    }
    assert_eq!(gauge::Entity::find().all(app.state().db()).await.unwrap().len(), 0, "nothing written");

    // The bounds themselves, `null`, and a value off the step (a hint only).
    for body in [
        json!({"name": "low", "level": 1, "reading": "1000", "price": "0.50"}),
        json!({"name": "high", "level": 5, "reading": null, "price": null}),
        json!({"name": "off-step", "level": 2, "reading": "7"}),
    ] {
        create(&client, &app, body).await.assert_status(StatusCode::CREATED);
    }

    // An update is bounded too.
    let key = create(&client, &app, json!({"name": "u", "level": 2})).await.json()["data"]["key"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = app.admin_api(&format!("resources/gauges/records/{key}"));
    let update = client.send(axum::http::Method::PATCH, &url, Some(&json!({"level": 9}))).await;
    assert_eq!(issue(&update, "level")["code"], json!("max_value"));

    // Hooks write what they decide.
    create(&client, &app, json!({"name": "hooked", "level": 3})).await.assert_status(StatusCode::CREATED);
    let hooked = gauge::Entity::find().all(app.state().db()).await.unwrap().into_iter().find(|g| g.name == "hooked");
    assert_eq!(hooked.map(|g| g.level), Some(9));
});

db_test!(the_schema_shows_the_bounds, |db: TestDb| async move {
    let (app, admin) = setup(db).await;
    let client = app.acting_as(&admin).await;
    let schema = client.get(&app.admin_api("resources/gauges/schema")).await.json();
    let inputs = schema["form"]["components"].as_array().unwrap();
    let number = |name: &str| inputs.iter().find(|i| i["name"] == json!(name)).unwrap().get("number").cloned();
    assert_eq!(number("level"), Some(json!({"min": 1.0, "max": 5.0})));
    assert_eq!(number("reading"), Some(json!({"max": 1000.0, "step": 10.0})));
    assert_eq!(number("price"), Some(json!({"min": 0.5, "step": 0.01})));
    assert_eq!(number("name"), None, "no bounds: no key");
});

macro_rules! refused_at_boot {
    ($test:ident, $resource:ident, $message:literal, $form:expr) => {
        pub struct $resource;

        impl Resource for $resource {
            type Entity = gauge::Entity;
            const SLUG: &'static str = "gauges";

            fn meta() -> ResourceMeta {
                ResourceMeta::new("Gauge").title("name")
            }

            fn fields() -> Vec<Field> {
                gauge_fields()
            }

            fn form() -> Form {
                Form::new().schema($form)
            }
        }

        #[test]
        #[should_panic(expected = $message)]
        fn $test() {
            Registry::new().register::<$resource>();
        }
    };
}

refused_at_boot!(
    bounds_on_a_text_input,
    TextBounds,
    "`min`, `max` and `step` are for number inputs (`name`)",
    components![TextInput::make("name").max(3)]
);

refused_at_boot!(
    min_above_max,
    Inverted,
    "number input `level` has `min` above `max`",
    components![NumberInput::make("level").min(5).max(1)]
);

refused_at_boot!(
    a_step_that_is_not_positive,
    ZeroStep,
    "number input `level` needs a positive step",
    components![NumberInput::make("level").step(0)]
);

refused_at_boot!(
    other_bounds_for_the_same_field,
    OtherBounds,
    "field `level` appears in several inputs that do not have the same bounds",
    components![NumberInput::make("level").max(5), NumberInput::make("level").max(6)]
);
