//! XD1 live-facade walkthrough. Run once on a freshly migrated disposable DB;
//! no unit-test harness, product seeds, or production entry point.
use bilibi_authz::{
    grants::Grants, CapabilityDefinition, RoleDefinition, VocabularyContribution, VOCABULARY,
};
use bilibi_factory_inventory::{Inventory, MaterialTyping, NewItem, NewUnit, PropertyDefinition};
use bilibi_factory_product_definition::{
    NewProduct, NewResource, NewResourceType, ProductDefinition, ResourceTypeUpdate, ResourceUpdate,
};
use bilibi_tenancy::Scope;
use serde_json::json;
use std::fmt::Debug;

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
bilibi_walkthrough_fixture::inventory::submit! { bilibi_walkthrough_fixture::registry::Contribution::new(VOCABULARY,"factory.xd1.roles",bilibi_walkthrough_fixture::registry::Provenance {module:"base/authz",source:file!(),line:line!()}, &VocabularyContribution {
    domains:&[],verbs:&[],capabilities:&[] as &[CapabilityDefinition],system_principals:&[],
    roles:&[
        RoleDefinition {code:"manager",name:"manager",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.product-definition.configuration.view","factory.product-definition.configuration.manage","factory.inventory.configuration.view","factory.inventory.configuration.manage","commerce.inventory.item.create","commerce.inventory.item.view"]},
        RoleDefinition {code:"viewer",name:"viewer",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.product-definition.configuration.view"]},
    ],
}) }
type Error = Box<dyn std::error::Error>;
fn observe(label: &str, value: impl Debug) {
    println!("{label}: {value:?}");
}
fn refusal<T: Debug, E: std::error::Error>(
    label: &str,
    value: Result<T, E>,
    expected: &str,
) -> Result<(), Error> {
    match value {
        Err(error) if error.to_string().contains(expected) => {
            println!("{label}: refused ({error})");
            Ok(())
        }
        other => Err(format!("{label}: expected {expected}, got {other:?}").into()),
    }
}
fn verify(label: &str, correct: bool) -> Result<(), Error> {
    if !correct {
        return Err(format!("{label}: unexpected runtime result").into());
    }
    println!("{label}: verified");
    Ok(())
}
async fn login(app: &bilibi_web::App, email: &str) -> Result<Scope, Error> {
    let login = app
        .user
        .login(email, bilibi_walkthrough_fixture::PASSWORD)
        .await?;
    Ok(app.security.guard(&login.token).await?)
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    bilibi_walkthrough_fixture::install(&db).await?;
    let app = bilibi_web::App::new(db.clone(), "http://localhost:35591")?;
    app.services
        .get::<Grants>("base/authz.grants")?
        .reconcile()
        .await?;
    let pd = app
        .services
        .get::<ProductDefinition>(bilibi_factory_product_definition::SERVICE)?;
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let manager = login(&app, "manager@example.test").await?;
    let viewer = login(&app, "viewer@example.test").await?;
    let staff = login(&app, "staff@example.test").await?;
    let other = login(&app, "manager@other.example.test").await?;
    let client = db.get().await?;
    if client
        .query_one("SELECT count(*) FROM factory_products", &[])
        .await?
        .get::<_, i64>(0)
        != 0
    {
        return Err("use a fresh disposable database".into());
    }

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
    let spare_unit = inventory
        .create_unit(
            &manager,
            1,
            &NewUnit {
                code: "alt".into(),
                name: "Alternate".into(),
            },
        )
        .await?;
    let mut item_ids = Vec::new();
    for sku in ["XD-INPUT", "XD-OUTPUT", "XD-EXTRA"] {
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
        item_ids.push(item.id);
    }
    let input = item_ids[0];
    let output = item_ids[1];
    let extra = item_ids[2];
    inventory
        .register_material(&manager, 1, input, unit.id, &MaterialTyping::default())
        .await?;
    let new_product = NewProduct {
        item_id: output,
        code: "Mixed-case".into(),
        name: "Fixture output".into(),
    };
    let product = pd.create_product(&manager, 1, &new_product).await?;
    verify("product code retains case", product.code == "Mixed-case")?;
    refusal(
        "duplicate product",
        pd.create_product(&manager, 1, &new_product).await,
        "already been taken",
    )?;
    refusal(
        "missing item",
        pd.create_product(
            &manager,
            1,
            &NewProduct {
                item_id: i64::MAX,
                code: "absent".into(),
                name: "Absent".into(),
            },
        )
        .await,
        "Item not found",
    )?;

    let definition = PropertyDefinition {
        key: "capacity".into(),
        label: " Capacity ".into(),
        value_type: "decimal".into(),
        unit: Some(" each ".into()),
        required: true,
    };
    let kind = pd
        .create_resource_type(
            &manager,
            1,
            &NewResourceType {
                code: " cell ".into(),
                name: "Cell".into(),
                property_definitions: vec![definition],
            },
        )
        .await?;
    verify(
        "type/property normalization",
        kind.code == "CELL"
            && kind.property_definitions[0].label == "Capacity"
            && kind.property_definitions[0].unit.as_deref() == Some("each"),
    )?;
    let new_resource = NewResource {
        code: "Cell-a".into(),
        name: "Cell A".into(),
        resource_type_id: kind.id,
        properties: json!({"capacity":"12.50"}).as_object().unwrap().clone(),
    };
    let resource = pd.create_resource(&manager, 1, &new_resource).await?;
    verify(
        "decimal property round trip",
        resource.properties["capacity"] == "12.50",
    )?;
    refusal(
        "duplicate resource",
        pd.create_resource(&manager, 1, &new_resource).await,
        "already been taken",
    )?;
    refusal(
        "required property",
        pd.create_resource(
            &manager,
            1,
            &NewResource {
                code: "missing".into(),
                properties: Default::default(),
                ..new_resource.clone()
            },
        )
        .await,
        "Invalid properties",
    )?;
    refusal(
        "unknown property",
        pd.update_resource(
            &manager,
            1,
            resource.id,
            &ResourceUpdate {
                properties: Some(
                    json!({"capacity":"1","unknown":1})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                ..Default::default()
            },
        )
        .await,
        "Invalid properties",
    )?;
    refusal(
        "definitions freeze in use",
        pd.update_resource_type(
            &manager,
            1,
            kind.id,
            &ResourceTypeUpdate {
                property_definitions: Some(vec![]),
                ..Default::default()
            },
        )
        .await,
        "Resource type in use",
    )?;
    refusal(
        "active resource blocks type retirement",
        pd.retire_resource_type(&manager, 1, kind.id).await,
        "Resource type in use",
    )?;
    let renamed = pd
        .update_resource(
            &manager,
            1,
            resource.id,
            &ResourceUpdate {
                name: Some("Renamed".into()),
                ..Default::default()
            },
        )
        .await?;
    verify(
        "omitted properties preserved",
        renamed.properties == resource.properties && renamed.name == "Renamed",
    )?;
    pd.update_resource_type(
        &manager,
        1,
        kind.id,
        &ResourceTypeUpdate {
            name: Some("Renamed type".into()),
            property_definitions: Some(kind.property_definitions.clone()),
            ..Default::default()
        },
    )
    .await?;

    let formula = json!({"lines":[
        {"item_id":input,"unit_id":unit.id,"role":"input","quantity":"2.75","material_hold_rule":{"policy":"company-defined"}},
        {"item_id":output,"unit_id":unit.id,"role":"output","quantity":1,"output_role":"primary"},
        {"item_id":extra,"unit_id":unit.id,"role":"output","quantity":"0.25"}
    ],"process_config":{"process_family":"fixture","tolerances":{"quantity":"0.1"}}});
    let routing = json!({"operations":[{"code":"finish","sequence":7,"inputs":[input],"outputs":[output],"allowed_resource_ids":[resource.id]}]});
    let f1 = pd
        .publish_formula(&manager, 1, product.id, &formula)
        .await?;
    let r1 = pd
        .publish_routing(&manager, 1, product.id, &routing)
        .await?;
    verify(
        "first independent versions",
        f1.version == 1 && r1.version == 1 && f1.lines[0].quantity == "2.75",
    )?;
    let selected = pd.select_revisions(&viewer, 1, product.id, 1, 1).await?;
    verify(
        "subset selection permits unused formula output",
        selected.formula == f1 && selected.routing == r1,
    )?;
    let (a, b) = tokio::join!(
        pd.publish_formula(&manager, 1, product.id, &formula),
        pd.publish_formula(&manager, 1, product.id, &formula)
    );
    let mut versions = vec![a?.version, b?.version];
    versions.sort();
    verify(
        "concurrent publication versions 2 and 3",
        versions == [2, 3],
    )?;
    verify(
        "old version unchanged",
        pd.get_formula_revision(&manager, 1, product.id, 1).await? == f1,
    )?;
    let r2 = pd
        .publish_routing(&manager, 1, product.id, &routing)
        .await?;
    verify(
        "formula 1 with routing 2",
        pd.select_revisions(&viewer, 1, product.id, 1, r2.version)
            .await?
            .routing
            .version
            == 2,
    )?;
    refusal(
        "missing exact version",
        pd.select_revisions(&manager, 1, product.id, 99, 1).await,
        "Formula revision not found",
    )?;
    refusal(
        "zero version",
        pd.get_routing_revision(&manager, 1, product.id, 0).await,
        "Routing revision not found",
    )?;
    let mut mismatch = routing.clone();
    mismatch["operations"][0]["inputs"] = json!([extra]);
    let mismatched = pd
        .publish_routing(&manager, 1, product.id, &mismatch)
        .await?;
    refusal(
        "routing/formula mismatch",
        pd.select_revisions(&manager, 1, product.id, 1, mismatched.version)
            .await,
        "Routing formula mismatch",
    )?;
    for (label, attrs, reason) in [
        ("empty lines", json!({"lines":[]}), "Invalid lines"),
        (
            "zero quantity",
            json!({"lines":[{"item_id":output,"unit_id":unit.id,"role":"output","quantity":0}]}),
            "Invalid lines",
        ),
        (
            "product output missing",
            json!({"lines":[{"item_id":extra,"unit_id":unit.id,"role":"output","quantity":1}]}),
            "Product output missing",
        ),
        (
            "input output-role",
            json!({"lines":[{"item_id":input,"unit_id":unit.id,"role":"input","quantity":1,"output_role":"bad"}]}),
            "Invalid lines",
        ),
    ] {
        refusal(
            label,
            pd.publish_formula(&manager, 1, product.id, &attrs).await,
            reason,
        )?;
    }
    let mut bad_config = formula.clone();
    bad_config["process_config"] = json!({"unknown":1});
    refusal(
        "unknown process config",
        pd.publish_formula(&manager, 1, product.id, &bad_config)
            .await,
        "Invalid process config",
    )?;
    let mut duplicate = routing.clone();
    duplicate["operations"]
        .as_array_mut()
        .unwrap()
        .push(routing["operations"][0].clone());
    refusal(
        "duplicate operation",
        pd.publish_routing(&manager, 1, product.id, &duplicate)
            .await,
        "Invalid operations",
    )?;
    let mut missing_resource = routing.clone();
    missing_resource["operations"][0]["allowed_resource_ids"] = json!([i64::MAX]);
    refusal(
        "missing resource",
        pd.publish_routing(&manager, 1, product.id, &missing_resource)
            .await,
        "Resource not found",
    )?;
    verify(
        "failed publication consumes no version",
        pd.publish_formula(&manager, 1, product.id, &formula)
            .await?
            .version
            == 4,
    )?;
    observe("products", pd.list_products(&viewer, 1, None).await?);
    observe("types", pd.list_resource_types(&viewer, 1, None).await?);
    observe("resources", pd.list_resources(&viewer, 1, None).await?);
    verify(
        "bounded formula list",
        pd.list_formula_revisions(&viewer, 1, product.id, Some(2))
            .await?
            .iter()
            .map(|f| f.version)
            .collect::<Vec<_>>()
            == [1, 2],
    )?;
    verify(
        "routing list",
        pd.list_routing_revisions(&viewer, 1, product.id, None)
            .await?
            .len()
            == 3,
    )?;
    refusal(
        "staff read",
        pd.list_products(&staff, 1, None).await,
        "Access denied",
    )?;
    refusal(
        "viewer write",
        pd.create_resource_type(&viewer, 1, &NewResourceType::default())
            .await,
        "Access denied",
    )?;
    refusal(
        "other tenant",
        pd.get_product(&other, 1, product.id).await,
        "Not found",
    )?;
    refusal(
        "other company record",
        pd.get_product(&manager, 2, product.id).await,
        "Product not found",
    )?;
    refusal(
        "resource type from other company",
        pd.create_resource(&manager, 2, &new_resource).await,
        "Resource type not found",
    )?;

    let admin = login(&app, "admin@example.test").await?;
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    let manage = "factory.product-definition.configuration.manage";
    grants
        .set_direct(&admin, manager.user(), manage, Some(false))
        .await?;
    refusal(
        "grant revoked after session creation",
        pd.update_resource(&manager, 1, resource.id, &ResourceUpdate::default())
            .await,
        "Access denied",
    )?;
    grants
        .set_direct(&admin, manager.user(), manage, None)
        .await?;

    // Fault injection only in this disposable fixture: audit rejection must
    // roll the business row back, not merely report a failed audit afterward.
    client.batch_execute("CREATE FUNCTION xd1_refuse_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.auditable_type='factory_resource_types' THEN RAISE EXCEPTION 'fixture audit refusal'; END IF; RETURN NEW; END $$; CREATE TRIGGER xd1_refuse_audit BEFORE INSERT ON base_audit_mutations FOR EACH ROW EXECUTE FUNCTION xd1_refuse_audit()").await?;
    let failed = pd
        .create_resource_type(
            &manager,
            1,
            &NewResourceType {
                code: "ROLLBACK".into(),
                name: "Rollback proof".into(),
                ..Default::default()
            },
        )
        .await;
    client.batch_execute("DROP TRIGGER xd1_refuse_audit ON base_audit_mutations; DROP FUNCTION xd1_refuse_audit()").await?;
    refusal("audit rejection", failed, "Database unavailable")?;
    verify(
        "audit failure leaves no business row",
        !pd.list_resource_types(&viewer, 1, None)
            .await?
            .iter()
            .any(|r| r.code == "ROLLBACK"),
    )?;

    inventory.retire_unit(&manager, 1, spare_unit.id).await?;
    let mut retired_unit = formula.clone();
    retired_unit["lines"][0]["unit_id"] = json!(spare_unit.id);
    refusal(
        "retired unit publication",
        pd.publish_formula(&manager, 1, product.id, &retired_unit)
            .await,
        "Unit retired",
    )?;
    inventory.retire_material(&manager, 1, input).await?;
    refusal(
        "retired material publication",
        pd.publish_formula(&manager, 1, product.id, &formula).await,
        "Material retired",
    )?;
    pd.retire_resource(&manager, 1, resource.id).await?;
    let mut retired_resource = routing.clone();
    retired_resource["operations"][0]["inputs"] = json!([]);
    refusal(
        "retired resource publication",
        pd.publish_routing(&manager, 1, product.id, &retired_resource)
            .await,
        "Resource retired",
    )?;
    refusal(
        "retired resource edit",
        pd.update_resource(&manager, 1, resource.id, &ResourceUpdate::default())
            .await,
        "Resource retired",
    )?;
    refusal(
        "retired resource still freezes definitions",
        pd.update_resource_type(
            &manager,
            1,
            kind.id,
            &ResourceTypeUpdate {
                property_definitions: Some(vec![]),
                ..Default::default()
            },
        )
        .await,
        "Resource type in use",
    )?;
    pd.retire_resource_type(&manager, 1, kind.id).await?;
    refusal(
        "retired type new resource",
        pd.create_resource(
            &manager,
            1,
            &NewResource {
                code: "new".into(),
                ..new_resource
            },
        )
        .await,
        "Resource type retired",
    )?;
    verify(
        "history remains selectable after retirement",
        pd.select_revisions(&viewer, 1, product.id, 1, 2)
            .await?
            .formula
            == f1,
    )?;

    for table in ["factory_formula_revisions", "factory_routing_revisions"] {
        for command in [
            format!("UPDATE {table} SET version=99"),
            format!("DELETE FROM {table}"),
            format!("TRUNCATE {table}"),
        ] {
            let error = client
                .batch_execute(&command)
                .await
                .expect_err("immutable SQL must refuse");
            verify(
                &command,
                error
                    .as_db_error()
                    .is_some_and(|e| e.message() == "Product definition revisions are immutable"),
            )?;
        }
    }
    client
        .execute(
            "UPDATE companies SET status='archived' WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    refusal(
        "archived company write",
        pd.create_resource_type(
            &manager,
            1,
            &NewResourceType {
                code: "ARCHIVED".into(),
                name: "Archived".into(),
                ..Default::default()
            },
        )
        .await,
        "archived",
    )?;
    client
        .execute(
            "UPDATE companies SET status='active' WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    let rows=client.query("SELECT auditable_type,event,count(*) FROM base_audit_mutations WHERE auditable_type IN ('factory_products','factory_resources','factory_resource_types','factory_formula_revisions','factory_routing_revisions') GROUP BY 1,2 ORDER BY 1,2",&[]).await?;
    for row in rows {
        println!(
            "audit: {} {} {}",
            row.get::<_, String>(0),
            row.get::<_, String>(1),
            row.get::<_, i64>(2)
        );
    }
    println!("XD1 walkthrough complete");
    Ok(())
}
