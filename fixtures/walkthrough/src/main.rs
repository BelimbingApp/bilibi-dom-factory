//! Factory walkthrough on a disposable, freshly migrated database:
//! `BILIBI_FACTORY_FIXTURE=disposable DATABASE_URL=… cargo run` from this
//! folder (the container README has the whole sequence). It installs the
//! Platform walkthrough fixture, gives the fixture roles Inventory's
//! capabilities, then drives the Inventory facade as real signed-in scopes and
//! prints each outcome and refusal. It asserts nothing; read the output.
//! Re-running starts from the seeded state.
use bilibi_authz::{
    grants::Grants, CapabilityDefinition, RoleDefinition, VocabularyContribution, VOCABULARY,
};
use bilibi_database::pool;
use bilibi_factory_inventory::{Inventory, ItemSettings, NewLocation, NewUnit, SERVICE};
use bilibi_tenancy::Scope;
use std::fmt::Debug;

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();

// The fixture's roles hold Inventory's capabilities: the manager configures,
// the viewer reads, staff holds none. These keys are Inventory's own (declared
// by the mounted crate); the roles are the Platform fixture's.
bilibi_walkthrough_fixture::inventory::submit! { bilibi_walkthrough_fixture::registry::Contribution::new(VOCABULARY, "factory.walkthrough.roles", bilibi_walkthrough_fixture::registry::Provenance { module: "base/authz", source: file!(), line: line!() }, &VocabularyContribution {
    domains: &[],
    verbs: &[],
    capabilities: &[] as &[CapabilityDefinition],
    roles: &[
        RoleDefinition { code: "manager", name: "manager", description: "Walkthrough fixture role", grant_all: false, capabilities: &["factory.inventory.configuration.view", "factory.inventory.configuration.manage"] },
        RoleDefinition { code: "viewer", name: "viewer", description: "Walkthrough fixture role", grant_all: false, capabilities: &["factory.inventory.configuration.view"] },
    ],
    system_principals: &[],
}) }

type Error = Box<dyn std::error::Error>;
fn show(label: &str, value: impl Debug) {
    println!("{label}: {value:?}");
}
fn refused<T: Debug, E: std::error::Error + 'static>(label: &str, result: Result<T, E>) {
    match result {
        Ok(value) => println!("{label}: ACCEPTED {value:?}"),
        Err(error) => {
            // PostgreSQL's own message, when the refusal came from the database.
            let reason = (&error as &dyn std::error::Error)
                .downcast_ref::<tokio_postgres::Error>()
                .and_then(|error| error.as_db_error())
                .map_or_else(|| error.to_string(), |db| db.message().to_owned());
            println!("{label}: refused ({reason})");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable for this walkthrough".into());
    }
    let db = pool(&std::env::var("DATABASE_URL")?)?;
    bilibi_walkthrough_fixture::install(&db).await?;
    let app = bilibi_web::App::new(db.clone(), "http://localhost:35591")?;
    app.services
        .get::<Grants>("base/authz.grants")?
        .reconcile()
        .await?;
    let inventory = app.services.get::<Inventory>(SERVICE)?;
    async fn login(app: &bilibi_web::App, email: &str) -> Result<Scope, Error> {
        let login = app
            .user
            .login(email, bilibi_walkthrough_fixture::PASSWORD)
            .await?;
        Ok(app.security.guard(&login.token).await?)
    }
    let manager = login(&app, "manager@example.test").await?;
    let viewer = login(&app, "viewer@example.test").await?;
    let staff = login(&app, "staff@example.test").await?;
    let other = login(&app, "manager@other.example.test").await?;

    let client = db.get().await?;
    let audited_before: i64 = client
        .query_one("SELECT COALESCE(max(id),0) FROM base_audit_mutations", &[])
        .await?
        .get(0);
    client
        .batch_execute(
            "DELETE FROM factory_inventory_units; DELETE FROM factory_inventory_locations;
             DELETE FROM base_settings WHERE key LIKE 'factory.inventory.%';
             UPDATE companies SET status='active' WHERE tenant_id=1 AND id=3",
        )
        .await?;

    println!("== item settings (company 1)");
    show(
        "effective at first",
        inventory.item_settings(&manager, 1).await,
    );
    let statuses = vec!["draft".to_owned(), "active".into(), "retired".into()];
    show(
        "configure",
        inventory
            .configure_item_settings(
                &manager,
                1,
                &ItemSettings {
                    statuses: Some(statuses.clone()),
                    default_currency_code: Some("MYR".into()),
                },
            )
            .await,
    );
    show(
        "overrides",
        inventory.item_setting_overrides(&manager, 1).await,
    );
    refused(
        "duplicate statuses",
        inventory
            .configure_item_settings(
                &manager,
                1,
                &ItemSettings {
                    statuses: Some(vec!["a".into(), "a".into()]),
                    default_currency_code: None,
                },
            )
            .await,
    );
    refused(
        "padded status",
        inventory
            .configure_item_settings(
                &manager,
                1,
                &ItemSettings {
                    statuses: Some(vec![" a".into()]),
                    default_currency_code: None,
                },
            )
            .await,
    );
    refused(
        "lower-case currency",
        inventory
            .configure_item_settings(
                &manager,
                1,
                &ItemSettings {
                    statuses: None,
                    default_currency_code: Some("myr".into()),
                },
            )
            .await,
    );
    show(
        "company 2 inherits nothing",
        inventory.item_settings(&manager, 2).await,
    );
    show(
        "clear the currency override",
        inventory
            .configure_item_settings(
                &manager,
                1,
                &ItemSettings {
                    statuses: Some(statuses),
                    default_currency_code: None,
                },
            )
            .await,
    );

    println!("== units");
    for (code, name) in [("kg", "Kilogram"), ("g", "Gram"), ("pcs", "Pieces")] {
        show(
            &format!("create {code}"),
            inventory
                .create_unit(
                    &manager,
                    1,
                    &NewUnit {
                        code: format!(" {code} "),
                        name: name.into(),
                    },
                )
                .await,
        );
    }
    refused(
        "duplicate code",
        inventory
            .create_unit(
                &manager,
                1,
                &NewUnit {
                    code: "kg".into(),
                    name: "Again".into(),
                },
            )
            .await,
    );
    refused(
        "blank code and name",
        inventory
            .create_unit(&manager, 1, &NewUnit::default())
            .await,
    );
    refused(
        "code over 32",
        inventory
            .create_unit(
                &manager,
                1,
                &NewUnit {
                    code: "x".repeat(33),
                    name: "Long".into(),
                },
            )
            .await,
    );
    let units = inventory.list_units(&manager, 1, None).await?;
    show("list", &units);
    let pcs = units
        .iter()
        .find(|u| u.code == "pcs")
        .map(|u| u.id)
        .unwrap_or_default();
    show(
        "rename pcs",
        inventory.rename_unit(&manager, 1, pcs, "Each").await,
    );
    show("retire pcs", inventory.retire_unit(&manager, 1, pcs).await);
    refused(
        "rename a retired unit",
        inventory.rename_unit(&manager, 1, pcs, "Again").await,
    );
    refused(
        "retire it twice",
        inventory.retire_unit(&manager, 1, pcs).await,
    );
    refused(
        "limit 501",
        inventory.list_units(&manager, 1, Some(501)).await,
    );
    show(
        "company 2 unit",
        inventory
            .create_unit(
                &manager,
                2,
                &NewUnit {
                    code: "kg".into(),
                    name: "Company 2 kilogram".into(),
                },
            )
            .await,
    );
    refused(
        "a company 2 unit read through company 1",
        inventory.get_unit(&manager, 1, pcs + 1000).await,
    );

    println!("== locations");
    show(
        "create",
        inventory
            .create_location(
                &manager,
                1,
                &NewLocation {
                    code: " wh-a ".into(),
                    name: "Warehouse A".into(),
                },
            )
            .await,
    );
    refused(
        "same code, other case",
        inventory
            .create_location(
                &manager,
                1,
                &NewLocation {
                    code: "WH-a".into(),
                    name: "Again".into(),
                },
            )
            .await,
    );
    show("list", inventory.list_locations(&manager, 1, None).await);

    println!("== who may do what");
    show(
        "viewer lists units",
        inventory
            .list_units(&viewer, 1, None)
            .await
            .map(|u| u.len()),
    );
    refused(
        "viewer creates a unit",
        inventory
            .create_unit(
                &viewer,
                1,
                &NewUnit {
                    code: "l".into(),
                    name: "Litre".into(),
                },
            )
            .await,
    );
    refused(
        "viewer configures",
        inventory
            .configure_item_settings(&viewer, 1, &ItemSettings::default())
            .await,
    );
    refused(
        "staff (no capability) lists",
        inventory.list_units(&staff, 1, None).await,
    );
    refused(
        "other tenant reads company 1",
        inventory.list_units(&other, 1, None).await,
    );
    refused(
        "missing company",
        inventory.list_units(&manager, 9999, None).await,
    );

    println!("== an archived company is read-only");
    client
        .execute(
            "UPDATE companies SET status='archived' WHERE tenant_id=1 AND id=3",
            &[],
        )
        .await?;
    show(
        "read",
        inventory
            .list_units(&manager, 3, None)
            .await
            .map(|u| u.len()),
    );
    refused(
        "write",
        inventory
            .create_unit(
                &manager,
                3,
                &NewUnit {
                    code: "kg".into(),
                    name: "Kilogram".into(),
                },
            )
            .await,
    );
    client
        .execute(
            "UPDATE companies SET status='active' WHERE tenant_id=1 AND id=3",
            &[],
        )
        .await?;

    println!("== PostgreSQL's own refusals");
    for sql in [
        "UPDATE factory_inventory_units SET code='x' WHERE code='kg' AND company_id=1",
        "UPDATE factory_inventory_units SET retired_at=NULL WHERE code='pcs' AND company_id=1",
        "INSERT INTO factory_inventory_units(tenant_id,company_id,code,name) VALUES(1,1,' padded','x')",
        "INSERT INTO factory_inventory_units(tenant_id,company_id,code,name) VALUES(2,1,'t','x')",
        "INSERT INTO factory_inventory_locations(tenant_id,company_id,code,name) VALUES(1,1,'lower','x')",
    ] {
        refused(sql, client.execute(sql, &[]).await);
    }
    println!("== the audited changes");
    for row in client
        .query(
            "SELECT auditable_type,auditable_id,event,actor_id,new_values::text FROM base_audit_mutations WHERE id > $1 AND (auditable_type LIKE 'factory_inventory_%' OR auditable_type = 'base/settings') ORDER BY id",
            &[&audited_before],
        )
        .await?
    {
        let (kind, id, event, actor, new): (String, String, String, i64, Option<String>) =
            (row.get(0), row.get(1), row.get(2), row.get(3), row.get(4));
        println!("{kind}#{id} {event} by user {actor}: {}", new.unwrap_or_default());
    }
    Ok(())
}
