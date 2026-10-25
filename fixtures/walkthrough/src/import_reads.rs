//! Import-only read proof, called after the production execution walkthrough.
use super::{decimal, refusal, verify, Error};
use bilibi_authz::grants::Grants;
use bilibi_factory_inventory::{Inventory, NewUnit};
use bilibi_factory_production_execution::ProductionExecution;
use bilibi_tenancy::Scope;

pub(super) async fn run(
    app: &bilibi_web::App,
    admin: &Scope,
    manager: &Scope,
    principal: &Scope,
    evidence: (i64, i64, i64),
) -> Result<(), Error> {
    println!("== import-only material, conversion, yield and backward trace reads");
    let (material, execution, identity) = evidence;
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let pe = app
        .services
        .get::<ProductionExecution>(bilibi_factory_production_execution::SERVICE)?;
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    grants
        .set_system(
            admin,
            "factory-import-fixture",
            "factory.production-execution.import",
            Some(true),
        )
        .await?;
    let importer = pe.importer(principal, 1)?;
    let unit = inventory
        .create_unit(
            manager,
            1,
            &NewUnit {
                code: "IMPORT-READ".into(),
                name: "Import read unit".into(),
            },
        )
        .await?;
    let first = inventory
        .define_conversion(manager, 1, material, unit.id, decimal("1.234567890123"))
        .await?;
    let second = inventory
        .define_conversion(manager, 1, material, unit.id, decimal("2.345678901234"))
        .await?;
    verify(
        "import material matches ordinary material",
        importer.get_material(material).await?
            == inventory.get_material(manager, 1, material).await?,
    )?;
    verify(
        "import conversion preserves exact old version after a new version",
        importer
            .get_conversion(material, unit.id, first.version)
            .await?
            == first
            && importer
                .get_conversion(material, unit.id, second.version)
                .await?
                == second,
    )?;
    refusal(
        "missing conversion version never falls back to current",
        importer
            .get_conversion(material, unit.id, second.version + 1)
            .await,
        "Conversion not found",
    )?;
    verify(
        "import yield matches ordinary native totals and conversion evidence",
        format!("{:?}", importer.get_run_yield(execution).await?)
            == format!("{:?}", pe.get_run_yield(manager, 1, execution).await?),
    )?;
    let trace = importer.trace_backward(identity).await?;
    verify(
        "import trace includes source lots and enriched production runs",
        trace.material.identities.len() > 1
            && !trace.material.links.is_empty()
            && trace.runs.iter().any(|run| run.id == execution),
    )?;
    verify(
        "import backward trace matches ordinary genealogy and resources",
        format!("{trace:?}") == format!("{:?}", pe.trace_backward(manager, 1, identity).await?),
    )?;

    refusal(
        "ordinary material read retains configuration view",
        inventory.get_material(principal, 1, material).await,
        "Access denied",
    )?;
    refusal(
        "ordinary exact conversion retains configuration view",
        inventory
            .get_conversion(principal, 1, material, unit.id, Some(first.version))
            .await,
        "Access denied",
    )?;
    refusal(
        "ordinary run yield retains floor view",
        pe.get_run_yield(principal, 1, execution).await,
        "Access denied",
    )?;
    refusal(
        "ordinary backward trace retains floor view",
        pe.trace_backward(principal, 1, identity).await,
        "Access denied",
    )?;
    refusal(
        "ordinary Inventory trace retains configuration view",
        inventory.trace_backward(principal, 1, identity).await,
        "Access denied",
    )?;

    let db = app
        .services
        .get::<bilibi_database::Db>("base/database.pool")?;
    let client = db.get().await?;
    let foreign: i64 = client
        .query_one("SELECT id FROM companies WHERE tenant_id<>1 LIMIT 1", &[])
        .await?
        .get(0);
    let foreign_importer = pe.importer(principal, foreign)?;
    refusal(
        "material import read refuses a foreign company",
        foreign_importer.get_material(material).await,
        "Not found",
    )?;
    refusal(
        "conversion import read refuses a foreign company",
        foreign_importer
            .get_conversion(material, unit.id, first.version)
            .await,
        "Not found",
    )?;
    refusal(
        "yield import read refuses a foreign company",
        foreign_importer.get_run_yield(execution).await,
        "Not found",
    )?;
    refusal(
        "trace import read refuses a foreign company",
        foreign_importer.trace_backward(identity).await,
        "Not found",
    )?;
    refusal(
        "missing material stays missing",
        importer.get_material(i64::MAX).await,
        "Material not found",
    )?;
    refusal(
        "missing run stays missing",
        importer.get_run_yield(i64::MAX).await,
        "Execution not found",
    )?;
    refusal(
        "missing lot stays missing",
        importer.trace_backward(i64::MAX).await,
        "Identity not found",
    )?;

    client
        .execute(
            "UPDATE companies SET status='archived' WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    refusal(
        "material import read rechecks live company",
        importer.get_material(material).await,
        "archived",
    )?;
    refusal(
        "conversion import read rechecks live company",
        importer
            .get_conversion(material, unit.id, first.version)
            .await,
        "archived",
    )?;
    refusal(
        "yield import read rechecks live company",
        importer.get_run_yield(execution).await,
        "archived",
    )?;
    refusal(
        "trace import read rechecks live company",
        importer.trace_backward(identity).await,
        "archived",
    )?;
    client
        .execute(
            "UPDATE companies SET status='active' WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    grants
        .set_system(
            admin,
            "factory-import-fixture",
            "factory.production-execution.import",
            Some(false),
        )
        .await?;
    refusal(
        "retained importer material read rechecks revoked grant",
        importer.get_material(material).await,
        "Access denied",
    )?;
    refusal(
        "retained importer conversion rechecks revoked grant",
        importer
            .get_conversion(material, unit.id, first.version)
            .await,
        "Access denied",
    )?;
    refusal(
        "retained importer yield rechecks revoked grant",
        importer.get_run_yield(execution).await,
        "Access denied",
    )?;
    refusal(
        "retained importer trace rechecks revoked grant",
        importer.trace_backward(identity).await,
        "Access denied",
    )?;
    Ok(())
}
