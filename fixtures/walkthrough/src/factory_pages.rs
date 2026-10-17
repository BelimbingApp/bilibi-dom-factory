//! V4 browser host for all twelve Factory routes. It creates no domain data;
//! use a freshly migrated disposable database for the R1 empty-state sweep,
//! or the XP1/XP2/XP3/XP4 walkthrough database for populated floor flows.
use bilibi_authz::{grants::Grants, RoleDefinition, VocabularyContribution, VOCABULARY};

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
bilibi_walkthrough_fixture::inventory::submit! {
    bilibi_walkthrough_fixture::registry::Contribution::new(
        VOCABULARY,
        "factory.v4.roles",
        bilibi_walkthrough_fixture::registry::Provenance {
            module: "base/authz", source: file!(), line: line!(),
        },
        &VocabularyContribution {
            domains: &[], verbs: &[], capabilities: &[], system_principals: &[],
            roles: &[
                RoleDefinition {
                    code: "manager", name: "manager", description: "Walkthrough fixture role",
                    grant_all: false,
                    capabilities: &[
                        "factory.inventory.configuration.view",
                        "factory.inventory.configuration.manage",
                        "commerce.inventory.item.create", "commerce.inventory.item.list",
                        "commerce.inventory.item.view", "commerce.inventory.item.update",
                        "factory.product-definition.configuration.view",
                        "factory.product-definition.configuration.manage",
                        "factory.production-execution.configuration.view",
                        "factory.production-execution.configuration.manage",
                        "factory.production-execution.floor.view",
                        "factory.production-execution.wastage.record",
                        "factory.production-execution.wastage.correct",
                        "factory.production-execution.measurement.record",
                        "factory.production-execution.measurement.correct",
                        "factory.production-execution.labour.record",
                        "factory.production-execution.labour.manage",
                    ],
                },
                RoleDefinition {
                    code: "viewer", name: "viewer", description: "Walkthrough fixture role",
                    grant_all: false,
                    capabilities: &[
                        "factory.inventory.configuration.view",
                        "commerce.inventory.item.list", "commerce.inventory.item.view",
                        "factory.product-definition.configuration.view",
                        "factory.production-execution.configuration.view",
                        "factory.production-execution.floor.view",
                    ],
                },
            ],
        },
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let origin = std::env::var("APP_ORIGIN")?;
    let bind = std::env::var("BIND_ADDR")?;
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    bilibi_walkthrough_fixture::install(&db).await?;
    // The Platform fixture leaves the viewer unlinked. Affiliation is proof
    // data only, matching the manager's company and the XD2 page host.
    db.get().await?.batch_execute(
        "INSERT INTO persons(id,tenant_id,name,kind,email) VALUES(4,1,'Lee Viewer','human','lee@example.test') ON CONFLICT DO NOTHING;
         INSERT INTO person_affiliations(id,tenant_id,person_id,organisation_id,title,work_email) VALUES(3,1,4,1,'Viewer','viewer@example.test') ON CONFLICT DO NOTHING;
         UPDATE users SET person_id=4,affiliation_id=3,organisation_id=1 WHERE tenant_id=1 AND id=2 AND person_id IS NULL AND affiliation_id IS NULL;",
    ).await?;
    let app = bilibi_web::App::new(db, &origin)?;
    app.services
        .get::<Grants>("base/authz.grants")?
        .reconcile()
        .await?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("Factory browser sweep listening at {origin}/factory/materials");
    axum::serve(listener, bilibi_web::router(app)).await?;
    Ok(())
}
