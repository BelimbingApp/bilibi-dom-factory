//! XD2 page walkthrough: installs the fixture accounts and a little Inventory
//! data into a freshly migrated disposable database, then serves the composed
//! app so the Product Definition pages can be used in a browser. No product
//! seeds, no unit-test harness. Sign in as manager@example.test (manage) or
//! viewer@example.test (view only); the password is the fixture's.
use bilibi_authz::{
    grants::Grants, CapabilityDefinition, RoleDefinition, VocabularyContribution, VOCABULARY,
};
use bilibi_factory_inventory::{Inventory, MaterialTyping, NewItem, NewUnit};

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
bilibi_walkthrough_fixture::inventory::submit! { bilibi_walkthrough_fixture::registry::Contribution::new(VOCABULARY,"factory.xd2.roles",bilibi_walkthrough_fixture::registry::Provenance {module:"base/authz",source:file!(),line:line!()}, &VocabularyContribution {
    domains:&[],verbs:&[],capabilities:&[] as &[CapabilityDefinition],system_principals:&[],
    roles:&[
        RoleDefinition {code:"manager",name:"manager",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.product-definition.configuration.view","factory.product-definition.configuration.manage","factory.inventory.configuration.view","factory.inventory.configuration.manage","commerce.inventory.item.create","commerce.inventory.item.list","commerce.inventory.item.view"]},
        RoleDefinition {code:"viewer",name:"viewer",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.product-definition.configuration.view","commerce.inventory.item.list","commerce.inventory.item.view"]},
    ],
}) }
type Error = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    bilibi_walkthrough_fixture::install(&db).await?;
    // The viewer is affiliated with Company 001 like the manager; the
    // fixture leaves every other account unlinked on purpose.
    db.get()
        .await?
        .batch_execute(
            "INSERT INTO persons(id,tenant_id,name,code,kind,email) VALUES(4,1,'Lee Viewer','factory_viewer','human','lee@example.test') ON CONFLICT DO NOTHING;
             UPDATE users SET person_id=4 WHERE tenant_id=1 AND id=2 AND person_id IS NULL;
             INSERT INTO user_desks(tenant_id,person_id,company_id) SELECT 1,person_id,1 FROM users WHERE tenant_id=1 AND id=2 ON CONFLICT DO NOTHING;",
        )
        .await?;
    let origin = std::env::var("APP_ORIGIN")?;
    let app = bilibi_web::App::new(db.clone(), &origin)?;
    app.services
        .get::<Grants>("base/authz.grants")?
        .reconcile()
        .await?;
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let login = app
        .user
        .login("manager@example.test", bilibi_walkthrough_fixture::PASSWORD)
        .await?;
    let manager = app.security.guard(&login.token).await?;
    if inventory.list_units(&manager, 1, None).await?.is_empty() {
        let unit = inventory
            .create_unit(
                &manager,
                1,
                &NewUnit {
                    code: "each".into(),
                    name: "Each".into(),
                },
            )
            .await?;
        for sku in ["XD-INPUT", "XD-OUTPUT"] {
            let item = inventory
                .create_item(
                    &manager,
                    1,
                    &NewItem {
                        sku: sku.into(),
                        title: sku.into(),
                        status: Some("active".into()),
                        currency_code: Some("MYR".into()),
                        ..Default::default()
                    },
                )
                .await?;
            inventory
                .register_material(&manager, 1, item.id, unit.id, &MaterialTyping::default())
                .await?;
            println!("item {} = {} (unit {} = each)", item.id, sku, unit.id);
        }
    }
    let bind = std::env::var("BIND_ADDR")?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("Product Definition pages at {origin}");
    axum::serve(listener, bilibi_web::router(app)).await?;
    Ok(())
}
