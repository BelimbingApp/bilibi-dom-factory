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
use bilibi_factory_inventory::{
    Decimal, Inventory, ItemSettings, ItemUpdate, Map, MaterialTypeUpdate, MaterialTyping, NewItem,
    NewLocation, NewMaterialType, NewUnit, PropertyDefinition, Value, SERVICE,
};
use bilibi_tenancy::Scope;
use std::fmt::Debug;

mod ledger;

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();

// The fixture's roles hold Inventory's capabilities: the manager configures
// and keeps the item master, the viewer reads, staff holds none. These keys are Inventory's own (declared
// by the mounted crate); the roles are the Platform fixture's.
bilibi_walkthrough_fixture::inventory::submit! { bilibi_walkthrough_fixture::registry::Contribution::new(VOCABULARY, "factory.walkthrough.roles", bilibi_walkthrough_fixture::registry::Provenance { module: "base/authz", source: file!(), line: line!() }, &VocabularyContribution {
    domains: &[],
    verbs: &[],
    capabilities: &[] as &[CapabilityDefinition],
    roles: &[
        RoleDefinition { code: "manager", name: "manager", description: "Walkthrough fixture role", grant_all: false, capabilities: &["factory.inventory.configuration.view", "factory.inventory.configuration.manage", "commerce.inventory.item.create", "commerce.inventory.item.list", "commerce.inventory.item.update", "commerce.inventory.item.view"] },
        RoleDefinition { code: "viewer", name: "viewer", description: "Walkthrough fixture role", grant_all: false, capabilities: &["factory.inventory.configuration.view", "commerce.inventory.item.list", "commerce.inventory.item.view"] },
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
            // Conversion versions are append-only; only this disposable reset
            // lifts the trigger to start again.
            "ALTER TABLE factory_inventory_transactions DISABLE TRIGGER factory_inventory_transactions_no_truncate;
             ALTER TABLE factory_inventory_transaction_entries DISABLE TRIGGER factory_inventory_entries_no_truncate;
             ALTER TABLE factory_inventory_identities DISABLE TRIGGER factory_inventory_identities_no_truncate;
             ALTER TABLE factory_inventory_genealogy_links DISABLE TRIGGER factory_inventory_genealogy_links_no_truncate;
             TRUNCATE factory_inventory_transactions,factory_inventory_transaction_entries,factory_inventory_identities,factory_inventory_genealogy_links;
             ALTER TABLE factory_inventory_transactions ENABLE TRIGGER factory_inventory_transactions_no_truncate;
             ALTER TABLE factory_inventory_transaction_entries ENABLE TRIGGER factory_inventory_entries_no_truncate;
             ALTER TABLE factory_inventory_identities ENABLE TRIGGER factory_inventory_identities_no_truncate;
             ALTER TABLE factory_inventory_genealogy_links ENABLE TRIGGER factory_inventory_genealogy_links_no_truncate;
             ALTER TABLE factory_inventory_unit_conversions DISABLE TRIGGER factory_inventory_unit_conversions_append_only;
             DELETE FROM factory_inventory_unit_conversions;
             ALTER TABLE factory_inventory_unit_conversions ENABLE TRIGGER factory_inventory_unit_conversions_append_only;
             DELETE FROM factory_inventory_materials; DELETE FROM commerce_inventory_items;
             DELETE FROM factory_inventory_material_types;
             DELETE FROM factory_inventory_units; DELETE FROM factory_inventory_locations;
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

    println!("== items (company 1 statuses: draft, active, retired; no default currency)");
    show(
        "create with the default status",
        inventory
            .create_item(
                &manager,
                1,
                &NewItem {
                    sku: " sheet-001 ".into(),
                    title: "Sheet stock".into(),
                    currency_code: Some("myr".into()),
                    storage_location: Some("  ".into()),
                    ..NewItem::default()
                },
            )
            .await,
    );
    refused(
        "duplicate SKU, other case",
        inventory
            .create_item(
                &manager,
                1,
                &NewItem {
                    sku: "Sheet-001".into(),
                    title: "Again".into(),
                    currency_code: Some("MYR".into()),
                    ..NewItem::default()
                },
            )
            .await,
    );
    refused(
        "status outside the vocabulary, no currency, negative quantity",
        inventory
            .create_item(
                &manager,
                1,
                &NewItem {
                    sku: "x-1".into(),
                    title: "X".into(),
                    status: Some("archived".into()),
                    quantity_on_hand: Some(-1),
                    ..NewItem::default()
                },
            )
            .await,
    );
    refused(
        "company 2 has no vocabulary, so a status is required",
        inventory
            .create_item(
                &manager,
                2,
                &NewItem {
                    sku: "x-1".into(),
                    title: "X".into(),
                    currency_code: Some("MYR".into()),
                    ..NewItem::default()
                },
            )
            .await,
    );
    show(
        "by SKU, lower case",
        inventory.get_item_by_sku(&manager, 1, "sheet-001").await,
    );
    show(
        "list status active",
        inventory
            .list_items(&manager, 1, Some("active"), None)
            .await
            .map(|items| items.len()),
    );

    println!("== material types");
    let definitions = vec![
        PropertyDefinition {
            key: "width".into(),
            label: " Width ".into(),
            value_type: "decimal".into(),
            unit: Some("mm".into()),
            required: true,
        },
        PropertyDefinition {
            key: "grade".into(),
            label: "Grade".into(),
            value_type: "string".into(),
            unit: None,
            required: false,
        },
    ];
    let sheet_type = inventory
        .create_material_type(
            &manager,
            1,
            &NewMaterialType {
                code: " type-a ".into(),
                name: "Type A".into(),
                property_definitions: definitions.clone(),
            },
        )
        .await;
    show("create", &sheet_type);
    let sheet_type = sheet_type?.id;
    refused(
        "duplicate key",
        inventory
            .create_material_type(
                &manager,
                1,
                &NewMaterialType {
                    code: "type-b".into(),
                    name: "Type B".into(),
                    property_definitions: vec![definitions[1].clone(), definitions[1].clone()],
                },
            )
            .await,
    );
    refused(
        "unit on a string property",
        inventory
            .create_material_type(
                &manager,
                1,
                &NewMaterialType {
                    code: "type-b".into(),
                    name: "Type B".into(),
                    property_definitions: vec![PropertyDefinition {
                        unit: Some("mm".into()),
                        ..definitions[1].clone()
                    }],
                },
            )
            .await,
    );
    refused(
        "taken code",
        inventory
            .create_material_type(
                &manager,
                1,
                &NewMaterialType {
                    code: "TYPE-A".into(),
                    name: "Again".into(),
                    property_definitions: Vec::new(),
                },
            )
            .await,
    );
    let spare_type = inventory
        .create_material_type(
            &manager,
            1,
            &NewMaterialType {
                code: "type-c".into(),
                name: "Type C".into(),
                property_definitions: Vec::new(),
            },
        )
        .await?
        .id;
    show(
        "retire type C",
        inventory
            .retire_material_type(&manager, 1, spare_type)
            .await,
    );
    refused(
        "edit a retired type",
        inventory
            .update_material_type(
                &manager,
                1,
                spare_type,
                &MaterialTypeUpdate {
                    name: Some("Again".into()),
                    ..MaterialTypeUpdate::default()
                },
            )
            .await,
    );

    println!("== materials");
    let kg = units
        .iter()
        .find(|u| u.code == "kg")
        .map(|u| u.id)
        .unwrap_or_default();
    let g = units
        .iter()
        .find(|u| u.code == "g")
        .map(|u| u.id)
        .unwrap_or_default();
    let values = |pairs: &[(&str, Value)]| -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    };
    let typed = |properties| MaterialTyping {
        material_type_id: Some(sheet_type),
        properties,
    };
    let new_item = |sku: &str| NewItem {
        sku: sku.into(),
        title: format!("Material {sku}"),
        currency_code: Some("MYR".into()),
        ..NewItem::default()
    };
    show(
        "create with a type and values",
        inventory
            .create_material(
                &manager,
                1,
                &new_item("mat-1"),
                kg,
                &typed(values(&[
                    ("width", Value::from("1200.50")),
                    ("grade", Value::from(" B ")),
                ])),
            )
            .await,
    );
    refused(
        "missing required value (and the item is not created)",
        inventory
            .create_material(&manager, 1, &new_item("mat-2"), kg, &typed(Map::new()))
            .await,
    );
    show(
        "item mat-2 after the refusal",
        inventory.get_item_by_sku(&manager, 1, "mat-2").await,
    );
    refused(
        "undefined key",
        inventory
            .create_material(
                &manager,
                1,
                &new_item("mat-2"),
                kg,
                &typed(values(&[
                    ("width", Value::from(5)),
                    ("colour", Value::from("x")),
                ])),
            )
            .await,
    );
    refused(
        "wrong value type",
        inventory
            .create_material(
                &manager,
                1,
                &new_item("mat-2"),
                kg,
                &typed(values(&[("width", Value::from(true))])),
            )
            .await,
    );
    refused(
        "values without a type",
        inventory
            .create_material(
                &manager,
                1,
                &new_item("mat-2"),
                kg,
                &MaterialTyping {
                    material_type_id: None,
                    properties: values(&[("width", Value::from(5))]),
                },
            )
            .await,
    );
    refused(
        "a retired type",
        inventory
            .create_material(
                &manager,
                1,
                &new_item("mat-2"),
                kg,
                &MaterialTyping {
                    material_type_id: Some(spare_type),
                    properties: Map::new(),
                },
            )
            .await,
    );
    refused(
        "a retired native unit",
        inventory
            .create_material(
                &manager,
                1,
                &new_item("mat-2"),
                pcs,
                &MaterialTyping::default(),
            )
            .await,
    );
    let sheet = inventory
        .get_item_by_sku(&manager, 1, "SHEET-001")
        .await?
        .id;
    show(
        "register an existing item without a type",
        inventory
            .register_material(&manager, 1, sheet, g, &MaterialTyping::default())
            .await,
    );
    refused(
        "register it twice",
        inventory
            .register_material(&manager, 1, sheet, kg, &MaterialTyping::default())
            .await,
    );
    let mat = inventory.get_item_by_sku(&manager, 1, "MAT-1").await?.id;
    show(
        "update details and values",
        inventory
            .update_material(
                &manager,
                1,
                mat,
                &ItemUpdate {
                    title: Some("Material one".into()),
                    status: Some("active".into()),
                    notes: Some("Checked".into()),
                    ..ItemUpdate::default()
                },
                &values(&[("width", Value::from(1250))]),
            )
            .await,
    );
    refused(
        "update with a status outside the vocabulary",
        inventory
            .update_material(
                &manager,
                1,
                mat,
                &ItemUpdate {
                    status: Some("gone".into()),
                    ..ItemUpdate::default()
                },
                &values(&[("width", Value::from(1250))]),
            )
            .await,
    );
    refused(
        "change definitions of a type in use",
        inventory
            .update_material_type(
                &manager,
                1,
                sheet_type,
                &MaterialTypeUpdate {
                    property_definitions: Some(definitions[..1].to_vec()),
                    ..MaterialTypeUpdate::default()
                },
            )
            .await,
    );
    show(
        "rename a type in use",
        inventory
            .update_material_type(
                &manager,
                1,
                sheet_type,
                &MaterialTypeUpdate {
                    code: Some("type-a2".into()),
                    name: Some("Type A, renamed".into()),
                    ..MaterialTypeUpdate::default()
                },
            )
            .await,
    );
    refused(
        "retire kg, an active material's native unit",
        inventory.retire_unit(&manager, 1, kg).await,
    );
    show("list", inventory.list_materials(&manager, 1, None).await);

    println!("== conversions (mat-1, native kg)");
    for factor in ["0.001", "0.0010"] {
        show(
            &format!("g at {factor}"),
            inventory
                .define_conversion(&manager, 1, mat, g, factor.parse::<Decimal>()?)
                .await,
        );
    }
    refused(
        "the native unit",
        inventory
            .define_conversion(&manager, 1, mat, kg, Decimal::ONE)
            .await,
    );
    refused(
        "a retired unit",
        inventory
            .define_conversion(&manager, 1, mat, pcs, Decimal::ONE)
            .await,
    );
    for factor in ["0", "0.0000000000001", "1000000000000"] {
        refused(
            &format!("factor {factor}"),
            inventory
                .define_conversion(&manager, 1, mat, g, factor.parse::<Decimal>()?)
                .await,
        );
    }
    show(
        "every version",
        inventory.list_conversions(&manager, 1, mat).await,
    );
    show(
        "current",
        inventory.get_conversion(&manager, 1, mat, g, None).await,
    );
    show(
        "version 1",
        inventory.get_conversion(&manager, 1, mat, g, Some(1)).await,
    );
    refused(
        "version 9",
        inventory.get_conversion(&manager, 1, mat, g, Some(9)).await,
    );
    refused(
        "conversions of an item that is not a material",
        inventory
            .list_conversions(
                &manager,
                1,
                inventory
                    .create_item(&manager, 1, &new_item("plain"))
                    .await?
                    .id,
            )
            .await,
    );

    println!("== retirement");
    show(
        "retire mat-1",
        inventory.retire_material(&manager, 1, mat).await,
    );
    refused(
        "retire it twice",
        inventory.retire_material(&manager, 1, mat).await,
    );
    refused(
        "update a retired material",
        inventory
            .update_material(&manager, 1, mat, &ItemUpdate::default(), &Map::new())
            .await,
    );
    show(
        "retire kg once its only material is retired",
        inventory.retire_unit(&manager, 1, kg).await,
    );
    refused(
        "a company 1 material read through company 2",
        inventory.get_material(&manager, 2, mat).await,
    );

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
    show(
        "viewer lists items",
        inventory
            .list_items(&viewer, 1, None, None)
            .await
            .map(|items| items.len()),
    );
    refused(
        "viewer creates an item",
        inventory
            .create_item(&viewer, 1, &new_item("viewer-1"))
            .await,
    );
    refused(
        "viewer creates a material type",
        inventory
            .create_material_type(&viewer, 1, &NewMaterialType::default())
            .await,
    );
    refused(
        "staff (no capability) lists materials",
        inventory.list_materials(&staff, 1, None).await,
    );
    refused(
        "other tenant reads company 1 items",
        inventory.list_items(&other, 1, None, None).await,
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
        "UPDATE commerce_inventory_items SET sku='OTHER' WHERE sku='MAT-1' AND company_id=1",
        "INSERT INTO commerce_inventory_items(tenant_id,company_id,sku,status,title,quantity_on_hand,currency_code) VALUES(1,1,'lower','draft','x',1,'myr')",
        "UPDATE factory_inventory_materials SET native_unit_id=native_unit_id+1 WHERE company_id=1",
        "UPDATE factory_inventory_materials SET retired_at=NULL WHERE company_id=1 AND retired_at IS NOT NULL",
        "UPDATE factory_inventory_material_types SET retired_at=NULL WHERE company_id=1 AND retired_at IS NOT NULL",
        "UPDATE factory_inventory_materials SET properties='{\"width\":1}' WHERE company_id=1 AND material_type_id IS NULL",
        "INSERT INTO factory_inventory_materials(tenant_id,company_id,item_id,native_unit_id) SELECT 1,2,i.id,u.id FROM commerce_inventory_items i, factory_inventory_units u WHERE i.company_id=1 AND i.sku='PLAIN' AND u.company_id=2 LIMIT 1",
        "UPDATE factory_inventory_unit_conversions SET factor=2",
        "DELETE FROM factory_inventory_unit_conversions",
        "TRUNCATE factory_inventory_unit_conversions",
        "INSERT INTO factory_inventory_unit_conversions(tenant_id,company_id,material_id,unit_id,version,factor) SELECT 1,1,m.id,m.native_unit_id,99,1 FROM factory_inventory_materials m WHERE m.company_id=1 LIMIT 1",
    ] {
        refused(sql, client.execute(sql, &[]).await);
    }
    ledger::run(&inventory, &db, &manager, &viewer, &staff, &other).await?;

    println!("== the audited changes");
    for row in client
        .query(
            "SELECT auditable_type,auditable_id,event,actor_id,new_values::text FROM base_audit_mutations WHERE id > $1 AND (auditable_type LIKE 'factory_inventory_%' OR auditable_type IN ('base/settings','commerce_inventory_items')) ORDER BY id",
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
