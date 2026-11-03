use rustcms::admin::{Field, Resource, ResourceMeta};
use rustcms::auth::{API_GUARD, Actor, AuthState, NewUser, RbacService};
use rustcms::clap::{ArgMatches, Command};
use rustcms::migration::*;
use rustcms::models::user;
use rustcms::prelude::*;
use rustcms::{AppCommand, async_trait};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set};

pub mod item {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "items")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub name: String,
        pub body: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub fn build(config: Config) -> AppBuilder {
    let mut app = AppBuilder::new(config);
    let api = routes();
    app.provider(AuthProvider::new().api_permissions(&api));
    app.provider(AdminProvider::new().brand("bench").resource::<ItemResource>());
    app.command(BenchSetup);
    api.register(&mut app);
    app
}

fn routes() -> ApiRoutes {
    ApiRoutes::new("/api/v1")
        .get("/ping", Access::public(), ping)
        .get("/items", Access::can("items.view"), index)
        .post("/items", Access::can("items.create"), store)
}

async fn ping() -> Json<serde_json::Value> {
    Json(json!({ "pong": true }))
}

#[derive(serde::Serialize)]
struct ItemJson {
    id: String,
    name: String,
}

/// The 20 newest items.
async fn index(State(app): State<AppState>) -> AppResult<Json<Vec<ItemJson>>> {
    let items = item::Entity::find().order_by_desc(item::Column::Id).limit(20).all(app.db()).await?;
    Ok(Json(items.into_iter().map(|i| ItemJson { id: i.id.to_string(), name: i.name }).collect()))
}

#[derive(serde::Deserialize)]
struct NewItem {
    name: String,
}

async fn store(State(app): State<AppState>, Json(input): Json<NewItem>) -> AppResult<(StatusCode, Json<ItemJson>)> {
    let item = item::ActiveModel { name: Set(input.name), body: Set(String::new()), ..Default::default() }
        .insert(app.db())
        .await?;
    Ok((StatusCode::CREATED, Json(ItemJson { id: item.id.to_string(), name: item.name })))
}

pub struct ItemResource;

impl Resource for ItemResource {
    type Entity = item::Entity;
    const SLUG: &'static str = "items";

    fn meta() -> ResourceMeta {
        ResourceMeta::new("Item").title("name")
    }

    fn fields() -> Vec<Field> {
        vec![Field::column(item::Column::Name).required().max_length(200)]
    }
}

/// `bench:setup`: the bench user (API role with the items permissions), 1,000
/// items, and a token printed on stdout.
struct BenchSetup;

const EMAIL: &str = "bench@example.test";
pub const PASSWORD: &str = "bench password 123";

#[async_trait]
impl AppCommand for BenchSetup {
    fn definition(&self) -> Command {
        Command::new("bench:setup").about("Create the bench user, items and an API token (printed)")
    }

    async fn run(&self, state: &AppState, _: &ArgMatches) -> anyhow::Result<()> {
        let auth = state.service::<AuthState>().expect("AuthProvider");
        let db = state.db();
        let rbac = RbacService::new(db, &auth.resolver);
        rbac.sync_permissions(&auth.permissions.all()).await?;
        let user = match user::Entity::find().filter(user::Column::Email.eq(EMAIL)).one(db).await? {
            Some(user) => user,
            None => {
                let password_hash = auth.passwords.hash(PASSWORD.to_owned()).await?;
                let new = NewUser { name: "Bench", email: EMAIL, password_hash, super_admin_guard: None };
                let user = rbac.create_user(Actor::System, new).await?;
                let permissions = ["items.view".to_owned(), "items.create".to_owned()];
                let role = rbac.create_role(Actor::System, API_GUARD, "bench", &permissions).await?;
                rbac.assign_role(Actor::System, user.id, role.id).await?;
                user
            }
        };
        if item::Entity::find().count(db).await? < 1_000 {
            for n in 0..1_000 {
                item::ActiveModel {
                    name: Set(format!("item {n}")),
                    body: Set("lorem ipsum ".repeat(20)),
                    ..Default::default()
                }
                .insert(db)
                .await?;
            }
        }
        let issued = rustcms::auth::tokens::issue(db, user.id, "bench", &[], None).await?;
        println!("{}", issued.plain_text);
        Ok(())
    }
}

pub struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        let mut migrations = rustcms::migration::framework();
        migrations.push(Box::new(CreateItems));
        migrations
    }
}

#[derive(DeriveMigrationName)]
struct CreateItems;

#[async_trait]
impl MigrationTrait for CreateItems {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            manager,
            Table::create()
                .table(Items::Table)
                .col(ColumnDef::new(Items::Id).big_integer().not_null().auto_increment().primary_key())
                .col(ColumnDef::new(Items::Name).string_len(200).not_null())
                .col(ColumnDef::new(Items::Body).text().not_null())
                .to_owned(),
        )
        .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        drop_table(manager, Items::Table).await
    }
}

#[derive(DeriveIden)]
enum Items {
    Table,
    Id,
    Name,
    Body,
}
