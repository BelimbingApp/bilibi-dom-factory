//! XP2 runtime proof, mirroring Bilimbi ProductionExecution capture modules.
//! Run AFTER production-execution, once, on its disposable database. SQL is
//! confined to fixture discovery, independent assertions and failure injection.
use bilibi_audit::RequestInfo;
use bilibi_authz::grants::Grants;
use bilibi_factory_inventory::{Decimal, Inventory, StockLine};
use bilibi_factory_production_execution::{Completion, ProductionExecution, Source};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use std::fmt::Debug;

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();

type Error = Box<dyn std::error::Error>;
fn verify(label: &str, ok: bool) -> Result<(), Error> {
    if !ok {
        return Err(format!("{label}: unexpected runtime result").into());
    }
    println!("{label}: verified");
    Ok(())
}
fn refusal<T: Debug, E: std::error::Error>(
    label: &str,
    r: Result<T, E>,
    expected: &str,
) -> Result<(), Error> {
    match r {
        Err(e) if e.to_string().contains(expected) => {
            println!("{label}: refused ({e})");
            Ok(())
        }
        other => Err(format!("{label}: expected {expected:?}, got {other:?}").into()),
    }
}
fn id(v: &Value) -> i64 {
    v["id"].as_i64().expect("public row id")
}
fn number(s: &str) -> Decimal {
    s.parse().expect("exact numeric text")
}
fn exact(v: &Value, key: &str, n: &str) -> bool {
    v[key].as_str().is_some_and(|s| number(s) == number(n))
}
async fn login(app: &bilibi_web::App, email: &str) -> Result<(Scope, String), Error> {
    let l = app
        .user
        .login(email, bilibi_walkthrough_fixture::PASSWORD)
        .await?;
    Ok((app.security.guard(&l.token).await?, l.token))
}
async fn counts(c: &tokio_postgres::Client) -> Result<Vec<i64>, Error> {
    let row = c.query_one("SELECT (SELECT count(*) FROM factory_wastage_records),(SELECT count(*) FROM factory_inventory_transactions),(SELECT count(*) FROM base_audit_mutations),(SELECT count(*) FROM factory_measurements),(SELECT count(*) FROM factory_labour_entries)", &[]).await?;
    Ok((0..row.len()).map(|i| row.get::<_, i64>(i)).collect())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    let app = bilibi_web::App::new(db.clone(), "http://localhost:35591")?;
    let pe = app
        .services
        .get::<ProductionExecution>(bilibi_factory_production_execution::SERVICE)?;
    let inv = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    let c = db.get().await?;
    if c.query_opt("SELECT 1 FROM factory_operation_executions WHERE tenant_id=1 AND company_id=1 AND request_id='run-1'",&[]).await?.is_none(){return Err("run production-execution first on a disposable database".into());}
    // The shared fixture deliberately leaves these accounts unlinked. XP2
    // needs actual company-affiliated workers and a supervisor, not a bypass
    // for an unlinked tenant administrator. Fixture setup only, before login.
    for user in [2_i64, 3, 4] {
        c.execute("WITH p AS (INSERT INTO persons(tenant_id,name,kind) SELECT tenant_id,name,'human' FROM users WHERE tenant_id=1 AND id=$1 RETURNING id,tenant_id), a AS (INSERT INTO person_affiliations(tenant_id,person_id,organisation_id) SELECT tenant_id,id,1 FROM p RETURNING id,person_id,tenant_id,organisation_id) UPDATE users u SET person_id=a.person_id,affiliation_id=a.id,organisation_id=a.organisation_id FROM a WHERE u.tenant_id=a.tenant_id AND u.id=$1",&[&user]).await?;
    }
    // Do not reconcile XP1's roles away. Grants are real authenticated writes.
    let (admin, token) = login(&app, "admin@example.test").await?;
    let (manager, _) = login(&app, "manager@example.test").await?;
    let (viewer, _) = login(&app, "viewer@example.test").await?;
    let (staff, _) = login(&app, "staff@example.test").await?;
    let (other, _) = login(&app, "manager@other.example.test").await?;
    for suffix in [
        "floor.view",
        "configuration.view",
        "configuration.manage",
        "wastage.record",
        "wastage.correct",
        "measurement.record",
        "measurement.correct",
        "labour.record",
    ] {
        grants
            .set_direct(
                &admin,
                manager.user(),
                &format!("factory.production-execution.{suffix}"),
                Some(true),
            )
            .await?;
    }
    // Explicit deny demonstrates own-record versus manage, regardless of XP1 grants.
    grants
        .set_direct(
            &admin,
            manager.user(),
            "factory.production-execution.labour.manage",
            Some(false),
        )
        .await?;
    let order: i64 = c.query_one("SELECT id FROM factory_production_orders WHERE tenant_id=$1 AND company_id=1 AND code='ORD-1'", &[&admin.tenant()]).await?.get(0);
    let batch: i64 = c.query_one("SELECT id FROM factory_production_orders WHERE tenant_id=$1 AND company_id=1 AND code='BATCH-1'", &[&admin.tenant()]).await?.get(0);
    let run: i64 = c.query_one("SELECT id FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=1 AND request_id='run-1'", &[&admin.tenant()]).await?.get(0);
    let execution = pe.get_execution(&admin, 1, run).await?;
    let tx = inv
        .get_transaction(&admin, 1, execution.inventory_transaction_id)
        .await?;
    let output = tx
        .entries
        .iter()
        .find(|e| e.role == "stock" && number(&e.native_quantity) == number("14"))
        .expect("XP1 output");
    let input = tx
        .entries
        .iter()
        .find(|e| e.role == "stock" && e.native_quantity.starts_with('-'))
        .expect("XP1 input");
    let item = output.item_id.expect("item");
    let location = output.location_id.expect("location");
    let identity = output.identity_id.expect("output identity");
    let input_identity = input.identity_id.expect("input identity");
    verify(
        "XP1 data and empty XP2 captures",
        execution.order_id == order
            && pe.list_wastage(&admin, 1, run).await?.is_empty()
            && pe.list_measurements(&admin, 1, run).await?.is_empty()
            && pe.list_labour(&admin, 1, order).await?.is_empty(),
    )?;

    println!("== capture configuration");
    let reason = pe
        .create_wastage_reason(&manager, 1, &json!({"code":" xp2-loss ","label":" Loss "}))
        .await?;
    let role = pe
        .create_labour_role(&manager, 1, &json!({"code":" xp2-work ","label":" Work "}))
        .await?;
    let kind = pe.create_measurement_type(&manager,1,&json!({"code":" xp2-size ","label":" Size ","value_type":"decimal","unit":"mm","minimum":"2","maximum":"4","target":"3"})).await?;
    let unbounded = pe
        .create_measurement_type(
            &manager,
            1,
            &json!({"code":"XP2-FREE","label":"Unbounded","value_type":"integer"}),
        )
        .await?;
    verify(
        "normalized configuration",
        reason["code"] == "XP2-LOSS"
            && reason["label"] == "Loss"
            && role["code"] == "XP2-WORK"
            && kind["code"] == "XP2-SIZE"
            && exact(&kind, "minimum", "2"),
    )?;
    refusal(
        "normalized duplicate",
        pe.create_wastage_reason(&manager, 1, &json!({"code":"xp2-loss","label":"duplicate"}))
            .await,
        "Code has already been taken",
    )?;
    refusal(
        "configuration read-only viewer",
        pe.create_labour_role(&viewer, 1, &json!({"code":"NO","label":"No"}))
            .await,
        "Access denied",
    )?;
    let changed = pe
        .update_measurement_type(
            &manager,
            1,
            id(&kind),
            &json!({"code":"CHANGED","value_type":"boolean","unit":"other","label":"Updated"}),
        )
        .await?;
    verify(
        "immutable type code and unit",
        changed["code"] == kind["code"]
            && changed["value_type"] == "decimal"
            && changed["unit"] == "mm",
    )?;
    let changed = pe
        .update_labour_role(
            &manager,
            1,
            id(&role),
            &json!({"code":"CHANGED","label":"Updated"}),
        )
        .await?;
    verify(
        "immutable role code",
        changed["code"] == role["code"]
            && pe.get_labour_role(&manager, 1, id(&role)).await? == changed,
    )?;
    let changed = pe
        .update_wastage_reason(
            &manager,
            1,
            id(&reason),
            &json!({"code":"CHANGED","active":false}),
        )
        .await?;
    verify(
        "immutable reason code and active filters",
        changed["code"] == reason["code"]
            && pe.get_wastage_reason(&manager, 1, id(&reason)).await? == changed
            && pe
                .list_wastage_reasons(&manager, 1, Some(false))
                .await?
                .iter()
                .any(|r| id(r) == id(&reason))
            && !pe
                .list_wastage_reasons(&manager, 1, Some(true))
                .await?
                .iter()
                .any(|r| id(r) == id(&reason)),
    )?;
    let waste = |request: &str, quantity: &str| json!({"request_id":request,"reason_id":id(&reason),"item_id":item,"location_id":location,"identity_id":identity,"quantity":quantity,"observation":"measured","note":"capture"});
    refusal(
        "inactive reason new capture",
        pe.record_wastage(&manager, 1, run, &waste("xp2-inactive", "1"))
            .await,
        "Wastage reason inactive",
    )?;
    pe.update_wastage_reason(&manager, 1, id(&reason), &json!({"active":true}))
        .await?;

    println!("== wastage ledger and corrections");
    let stock = inv.get_stock_position(&admin, 1, item, location).await?;
    let before = number(&stock.quantity);
    let root = pe
        .record_wastage(&manager, 1, run, &waste("xp2-waste", "2"))
        .await?;
    verify(
        "consumption exact quantity and recorder",
        exact(&root, "quantity", "2")
            && root["corrected_by_id"].is_null()
            && root["recorded_by_type"] == "user"
            && root["recorded_by_id"] == manager.user()
            && root["occurred_at"]
                .as_str()
                .is_some_and(|s| s.contains('T'))
            && number(
                &inv.get_stock_position(&admin, 1, item, location)
                    .await?
                    .quantity,
            ) == before - number("2"),
    )?;
    let audit_before = counts(&c).await?;
    let mut optional = waste("xp2-waste", "2.000");
    optional["note"] = json!(" capture ");
    optional["identity_id"] = json!(identity);
    verify(
        "wastage replay no new audit",
        pe.record_wastage(&manager, 1, run, &optional).await? == root
            && counts(&c).await? == audit_before,
    )?;
    refusal(
        "wastage conflicting retry",
        pe.record_wastage(&manager, 1, run, &waste("xp2-waste", "3"))
            .await,
        "Request id conflict",
    )?;
    pe.update_wastage_reason(&manager, 1, id(&reason), &json!({"active":false}))
        .await?;
    let mut previous = root.clone();
    for (request, quantity, net, posted) in [
        ("xp2-up", "3", "3", true),
        ("xp2-down", "1", "1", true),
        ("xp2-same", "1", "1", false),
        ("xp2-zero", "0", "0", true),
    ] {
        let next = pe
            .correct_wastage(
                &manager,
                1,
                id(&previous),
                &json!({"request_id":request,"quantity":quantity,"correction_reason":"reweighed"}),
            )
            .await?;
        verify(
            "linear correction and exact stock delta",
            next["corrects_id"] == id(&previous)
                && next["inventory_transaction_id"].is_number() == posted
                && number(
                    &inv.get_stock_position(&admin, 1, item, location)
                        .await?
                        .quantity,
                ) == before - number(net),
        )?;
        if posted {
            let posting = inv
                .get_transaction(
                    &admin,
                    1,
                    next["inventory_transaction_id"].as_i64().unwrap(),
                )
                .await?;
            verify(
                "ledger corrections link root",
                posting.corrects_transaction_id == root["inventory_transaction_id"].as_i64(),
            )?;
        }
        let y = pe.get_run_yield(&admin, 1, run).await?;
        let b = y
            .balances
            .iter()
            .find(|b| b.unit.id == output.native_unit_id)
            .expect("unit balance");
        verify(
            "yield independently expected",
            number(&b.input) == number("15")
                && number(&b.product) == number("15") - number(net)
                && number(&b.wastage) == number(net),
        )?;
        previous = next;
    }
    refusal(
        "stale correction",
        pe.correct_wastage(
            &manager,
            1,
            id(&root),
            &json!({"request_id":"xp2-stale","quantity":"1","correction_reason":"stale"}),
        )
        .await,
        "Wastage already corrected",
    )?;
    let chain = pe.list_wastage(&manager, 1, run).await?;
    verify(
        "correction back links visible",
        chain.len() == 5
            && chain[0]["corrected_by_id"] == id(&chain[1])
            && chain.last().unwrap()["corrected_by_id"].is_null(),
    )?;
    pe.update_wastage_reason(&manager, 1, id(&reason), &json!({"active":true}))
        .await?;
    let before_failure = counts(&c).await?;
    c.batch_execute("CREATE FUNCTION xp2_fail_capture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'XP2 injected capture failure'; END $$; CREATE TRIGGER xp2_fail_capture BEFORE INSERT ON factory_wastage_records FOR EACH ROW EXECUTE FUNCTION xp2_fail_capture()").await?;
    let failed = pe
        .record_wastage(&manager, 1, run, &waste("xp2-rollback", "1"))
        .await;
    c.batch_execute("DROP TRIGGER xp2_fail_capture ON factory_wastage_records; DROP FUNCTION xp2_fail_capture()").await?;
    refusal(
        "capture failure after Inventory posting",
        failed,
        "Database unavailable",
    )?;
    verify(
        "atomic rollback capture posting audits and stock",
        counts(&c).await? == before_failure
            && number(
                &inv.get_stock_position(&admin, 1, item, location)
                    .await?
                    .quantity,
            ) == before,
    )?;
    let race = waste("xp2-race", "1");
    let (a, b) = tokio::join!(
        pe.record_wastage(&manager, 1, run, &race),
        pe.record_wastage(&manager, 1, run, &race)
    );
    verify(
        "concurrent wastage one row",
        id(&a?) == id(&b?) && pe.list_wastage(&manager, 1, run).await?.len() == 6,
    )?;
    let before_race = counts(&c).await?;
    let (small, large) = (
        waste("xp2-conflicting-race", "1"),
        waste("xp2-conflicting-race", "2"),
    );
    let (a, b) = tokio::join!(
        pe.record_wastage(&manager, 1, run, &small),
        pe.record_wastage(&manager, 1, run, &large)
    );
    let winner = match (a, b) {
        (Ok(v), Err(e)) | (Err(e), Ok(v)) => {
            verify(
                "conflicting race loser",
                e.to_string().contains("Request id conflict"),
            )?;
            v
        }
        pair => return Err(format!("conflicting race must save exactly one: {pair:?}").into()),
    };
    let after_race = counts(&c).await?;
    verify(
        "conflicting race posts one consumption",
        after_race[0] == before_race[0] + 1
            && after_race[1] == before_race[1] + 1
            && number(
                &inv.get_stock_position(&admin, 1, item, location)
                    .await?
                    .quantity,
            ) == before - number("1") - number(winner["quantity"].as_str().unwrap()),
    )?;
    let unit_yield = pe.get_unit_yield(&admin, 1, identity).await?;
    verify(
        "identified output yield includes run",
        unit_yield.identity.id == identity && unit_yield.runs.iter().any(|r| r.execution_id == run),
    )?;
    let trace = pe.trace_backward(&admin, 1, identity).await?;
    verify(
        "production trace retains original run",
        trace
            .runs
            .iter()
            .any(|r| r.id == run && r.order.id == order),
    )?;
    let free_input = tx
        .entries
        .iter()
        .find(|e| e.role == "stock" && number(&e.native_quantity) == number("-5"))
        .expect("XP1 free input");
    let consumed = pe
        .complete_operation(
            &admin,
            1,
            order,
            Source::Live,
            &Completion {
                request_id: "xp2-consumption-only".into(),
                operation_code: execution.operation_code.clone(),
                resource_id: execution.resource_id,
                operator_type: "user".into(),
                operator_id: admin.user(),
                started_at: std::time::SystemTime::now() - std::time::Duration::from_secs(600),
                completed_at: std::time::SystemTime::now() - std::time::Duration::from_secs(60),
                inputs: vec![StockLine {
                    item_id: free_input.item_id.unwrap(),
                    identity_id: free_input.identity_id,
                    location_id: free_input.location_id,
                    quantity: number("1"),
                    ..Default::default()
                }],
                outputs: vec![],
                variance: None,
                evidence: "capture proof consumption-only run".into(),
                hold_override: None,
            },
        )
        .await?;
    let forward = pe
        .trace_forward(&admin, 1, free_input.identity_id.unwrap())
        .await?;
    verify(
        "forward trace includes consuming run without outputs",
        forward.runs.iter().any(|r| r.id == consumed.id),
    )?;
    let consumed_yield = pe.get_run_yield(&admin, 1, consumed.id).await?;
    verify(
        "consumption-only yield",
        consumed_yield.balances.len() == 1
            && number(&consumed_yield.balances[0].input) == number("1")
            && number(&consumed_yield.balances[0].product) == Decimal::ZERO,
    )?;
    let input_loss=pe.record_wastage(&manager,1,run,&json!({"request_id":"xp2-input-loss","reason_id":id(&reason),"item_id":input.item_id,"location_id":input.location_id,"identity_id":input.identity_id,"quantity":"0.25","observation":"measured"})).await?;
    let y = pe.get_run_yield(&admin, 1, run).await?;
    let b = y
        .balances
        .iter()
        .find(|b| b.unit.id == input.native_unit_id)
        .unwrap();
    verify(
        "input wastage increases input and waste",
        number(&b.input) == number("15.25") && number(&b.waste) == number(&b.wastage),
    )?;
    let u = pe
        .get_unit_yield(&admin, 1, input.identity_id.unwrap())
        .await?;
    let r = u.runs.iter().find(|r| r.execution_id == run).unwrap();
    let b = r
        .balances
        .iter()
        .find(|b| b.unit.id == input.native_unit_id)
        .unwrap();
    verify(
        "unit yield keeps whole-run and exact identity input",
        number(&b.input) == number("15.25")
            && number(b.unit_input.as_deref().unwrap()) == number("10.25")
            && input_loss["inventory_transaction_id"].is_number(),
    )?;

    println!("== measurements and snapshots");
    let measure = |request: &str, value: &str, identity: Option<i64>| json!({"request_id":request,"measurement_type_id":id(&kind),"identity_id":identity,"value":value});
    let mut first = Value::Null;
    for (request, value, flag) in [
        ("xp2-low", "2", false),
        ("xp2-high", "4", false),
        ("xp2-below", "1.999", true),
        ("xp2-above", "4.001", true),
    ] {
        let row = pe
            .record_measurement(&manager, 1, run, &measure(request, value, Some(identity)))
            .await?;
        verify(
            "inclusive measurement bounds and recorder",
            row["out_of_range"] == flag
                && row["value"].is_string()
                && row["recorded_by_id"] == manager.user(),
        )?;
        if first.is_null() {
            first = row;
        }
    }
    let unconstrained = pe
        .record_measurement(
            &manager,
            1,
            run,
            &json!({"request_id":"xp2-unbounded","measurement_type_id":id(&unbounded),"value":999}),
        )
        .await?;
    verify(
        "missing limits mean unknown not false",
        unconstrained["out_of_range"].is_null(),
    )?;
    let before_replay = counts(&c).await?;
    let replay=pe.record_measurement(&manager,1,run,&json!({"request_id":"xp2-unbounded","measurement_type_id":id(&unbounded),"value":999,"note":null,"identity_id":null})).await?;
    verify(
        "omitted and explicit null normalize before replay",
        id(&replay) == id(&unconstrained) && counts(&c).await? == before_replay,
    )?;
    let race =
        json!({"request_id":"xp2-measure-race","measurement_type_id":id(&unbounded),"value":7});
    let (a, b) = tokio::join!(
        pe.record_measurement(&manager, 1, run, &race),
        pe.record_measurement(&manager, 1, run, &race)
    );
    verify("concurrent measurements one row", id(&a?) == id(&b?))?;
    refusal(
        "input identity not output",
        pe.record_measurement(
            &manager,
            1,
            run,
            &measure("xp2-input", "3", Some(input_identity)),
        )
        .await,
        "Identity not run output",
    )?;
    refusal(
        "unknown output identity",
        pe.record_measurement(
            &manager,
            1,
            run,
            &measure("xp2-unknown", "3", Some(i64::MAX)),
        )
        .await,
        "Identity not run output",
    )?;
    refusal(
        "invalid decimal",
        pe.record_measurement(
            &manager,
            1,
            run,
            &measure("xp2-invalid", "not numeric", None),
        )
        .await,
        "Invalid measurement value",
    )?;
    pe.update_measurement_type(
        &manager,
        1,
        id(&kind),
        &json!({"minimum":"10","maximum":"20","active":false}),
    )
    .await?;
    refusal(
        "inactive measurement new capture",
        pe.record_measurement(&manager, 1, run, &measure("xp2-type-inactive", "3", None))
            .await,
        "Measurement type inactive",
    )?;
    let corrected=pe.correct_measurement(&manager,1,id(&first),&json!({"request_id":"xp2-measure-correct","value":"3","correction_reason":"read again"})).await?;
    verify(
        "correction retains original limit snapshot even inactive",
        exact(&corrected, "minimum", "2")
            && exact(&corrected, "maximum", "4")
            && corrected["out_of_range"] == false
            && corrected["corrects_id"] == id(&first),
    )?;
    let original = pe
        .list_measurements(&manager, 1, run)
        .await?
        .into_iter()
        .find(|r| id(r) == id(&first))
        .expect("original observation");
    verify(
        "stored measurement snapshot and successor",
        exact(&original, "minimum", "2") && original["corrected_by_id"] == id(&corrected),
    )?;
    verify(
        "measurement config getters and active filters",
        pe.get_measurement_type(&manager, 1, id(&kind)).await?["active"] == false
            && pe
                .list_measurement_types(&manager, 1, Some(false))
                .await?
                .iter()
                .any(|r| id(r) == id(&kind)),
    )?;
    let before_replay = counts(&c).await?;
    refusal(
        "measurement replay validates retired type",
        pe.record_measurement(&manager, 1, run, &measure("xp2-high", "4", Some(identity)))
            .await,
        "Measurement type inactive",
    )?;
    verify(
        "retired replay writes nothing",
        counts(&c).await? == before_replay,
    )?;
    pe.update_measurement_type(&manager, 1, id(&kind), &json!({"active":true}))
        .await?;
    refusal(
        "measurement conflicting replay",
        pe.record_measurement(&manager, 1, run, &measure("xp2-high", "3", Some(identity)))
            .await,
        "Request id conflict",
    )?;

    println!("== labour ownership intervals and clocks");
    // Fixed historical timestamps make expected durations independent of runtime.
    let manual = |request: &str, start: &str, stop: &str, worker: i64| json!({"request_id":request,"role_id":id(&role),"worker_user_id":worker,"started_at":start,"stopped_at":stop});
    let worked = pe
        .record_labour(
            &manager,
            1,
            order,
            &manual(
                "xp2-work",
                "2026-01-01T01:00:00Z",
                "2026-01-01T02:00:00Z",
                manager.user(),
            ),
        )
        .await?;
    verify(
        "own manual labour exact seconds",
        worked["seconds"] == 3600
            && worked["recorded_by_id"] == manager.user()
            && worked["stopped_by_id"] == manager.user(),
    )?;
    refusal(
        "others require management",
        pe.record_labour(
            &manager,
            1,
            order,
            &manual(
                "xp2-others",
                "2026-01-01T01:00:00Z",
                "2026-01-01T02:00:00Z",
                staff.user(),
            ),
        )
        .await,
        "Capture not authorized",
    )?;
    let managed = pe
        .record_labour(
            &admin,
            1,
            order,
            &manual(
                "xp2-managed",
                "2026-01-01T01:00:00Z",
                "2026-01-01T01:30:00Z",
                staff.user(),
            ),
        )
        .await?;
    verify(
        "management records distinct worker",
        managed["worker_user_id"] == staff.user() && managed["recorded_by_id"] == admin.user(),
    )?;
    refusal(
        "overlap across orders",
        pe.record_labour(
            &manager,
            1,
            batch,
            &manual(
                "xp2-overlap",
                "2026-01-01T01:30:00Z",
                "2026-01-01T02:30:00Z",
                manager.user(),
            ),
        )
        .await,
        "Labour overlap",
    )?;
    let adjacent = pe
        .record_labour(
            &manager,
            1,
            batch,
            &manual(
                "xp2-adjacent",
                "2026-01-01T02:00:00Z",
                "2026-01-01T03:00:00Z",
                manager.user(),
            ),
        )
        .await?;
    verify(
        "adjacency across orders allowed",
        adjacent["seconds"] == 3600,
    )?;
    refusal("manual requires stop",pe.record_labour(&manager,1,order,&json!({"request_id":"xp2-open-manual","role_id":id(&role),"started_at":"2026-01-02T00:00:00Z"})).await,"Invalid labour times")?;
    refusal(
        "own corrections require management",
        pe.correct_labour(
            &manager,
            1,
            id(&worked),
            &json!({"request_id":"xp2-denied-correction","correction_reason":"note","note":"new"}),
        )
        .await,
        "Capture not authorized",
    )?;
    pe.update_labour_role(&manager, 1, id(&role), &json!({"active":false}))
        .await?;
    let short=pe.correct_labour(&admin,1,id(&worked),&json!({"request_id":"xp2-short","correction_reason":"time corrected","stopped_at":"2026-01-01T01:15:00Z"})).await?;
    let retained=pe.correct_labour(&admin,1,id(&short),&json!({"request_id":"xp2-retain-stop","correction_reason":"note corrected","note":"retained"})).await?;
    refusal("nonfinite correction start",pe.correct_labour(&admin,1,id(&retained),&json!({"request_id":"xp2-nonfinite","correction_reason":"bad start","started_at":"-infinity","stopped_at":null})).await,"Invalid labour times")?;
    refusal("relative correction time",pe.correct_labour(&admin,1,id(&retained),&json!({"request_id":"xp2-relative","correction_reason":"bad start","started_at":"yesterday","stopped_at":null})).await,"Invalid labour times")?;
    let before_replay = counts(&c).await?;
    let replay=pe.correct_labour(&admin,1,id(&short),&json!({"request_id":"xp2-retain-stop","correction_reason":"note corrected","note":"retained","role_id":id(&role),"stopped_at":retained["stopped_at"]})).await?;
    verify(
        "correction defaults normalize before replay",
        id(&replay) == id(&retained) && counts(&c).await? == before_replay,
    )?;
    verify(
        "omitted stop retained; summary excludes predecessors",
        retained["seconds"] == 900
            && pe.labour_summary(&admin, 1, order).await?["total_seconds"] == 2700,
    )?;
    refusal(
        "inactive role new labour",
        pe.record_labour(
            &manager,
            1,
            order,
            &manual(
                "xp2-inactive-role",
                "2026-01-03T00:00:00Z",
                "2026-01-03T01:00:00Z",
                manager.user(),
            ),
        )
        .await,
        "Labour role inactive",
    )?;
    verify(
        "labour role active filter",
        pe.list_labour_roles(&manager, 1, Some(false))
            .await?
            .iter()
            .any(|r| id(r) == id(&role)),
    )?;
    pe.update_labour_role(&manager, 1, id(&role), &json!({"active":true}))
        .await?;
    let clock = pe
        .clock_in(
            &manager,
            1,
            order,
            &json!({"request_id":"xp2-clock","role_id":id(&role)}),
        )
        .await?;
    refusal(
        "second clock across orders",
        pe.clock_in(
            &manager,
            1,
            batch,
            &json!({"request_id":"xp2-clock-again","role_id":id(&role)}),
        )
        .await,
        "Worker already clocked in",
    )?;
    refusal(
        "others clock out requires management",
        pe.clock_out(&manager, 1, id(&managed)).await,
        "Capture not authorized",
    )?;
    let closed = pe.clock_out(&manager, 1, id(&clock)).await?;
    verify(
        "clock out stamps stopper",
        closed["stopped_at"].is_string() && closed["stopped_by_id"] == manager.user(),
    )?;
    refusal(
        "double clock out",
        pe.clock_out(&manager, 1, id(&clock)).await,
        "Labour entry closed",
    )?;
    let reopened=pe.correct_labour(&admin,1,id(&closed),&json!({"request_id":"xp2-reopen","correction_reason":"still working","stopped_at":null})).await?;
    verify(
        "explicit null reopens and clears stopper",
        reopened["stopped_at"].is_null()
            && reopened["stopped_by_id"].is_null()
            && reopened["seconds"].is_null()
            && pe.labour_summary(&admin, 1, order).await?["open"]
                .as_array()
                .unwrap()
                .len()
                == 1,
    )?;
    pe.clock_out(&admin, 1, id(&reopened)).await?;
    // C11 public labour path must consult live company-account eligibility.
    c.execute(
        "UPDATE users SET status='inactive' WHERE tenant_id=$1 AND id=$2",
        &[&admin.tenant(), &staff.user()],
    )
    .await?;
    let inactive = pe
        .record_labour(
            &admin,
            1,
            order,
            &manual(
                "xp2-inactive-worker",
                "2026-01-04T00:00:00Z",
                "2026-01-04T01:00:00Z",
                staff.user(),
            ),
        )
        .await;
    c.execute(
        "UPDATE users SET status='active' WHERE tenant_id=$1 AND id=$2",
        &[&admin.tenant(), &staff.user()],
    )
    .await?;
    refusal("C11 refuses inactive worker", inactive, "Worker not found")?;
    let before_replay = counts(&c).await?;
    verify(
        "labour replay returns original without audit",
        id(&pe
            .record_labour(
                &manager,
                1,
                order,
                &manual(
                    "xp2-work",
                    "2026-01-01T01:00:00Z",
                    "2026-01-01T02:00:00Z",
                    manager.user(),
                ),
            )
            .await?)
            == id(&worked)
            && counts(&c).await? == before_replay,
    )?;
    refusal(
        "labour conflicting retry",
        pe.record_labour(
            &manager,
            1,
            order,
            &manual(
                "xp2-work",
                "2026-01-01T01:00:00Z",
                "2026-01-01T01:59:00Z",
                manager.user(),
            ),
        )
        .await,
        "Request id conflict",
    )?;
    let race = manual(
        "xp2-labour-race",
        "2026-01-05T00:00:00Z",
        "2026-01-05T00:10:00Z",
        manager.user(),
    );
    let (a, b) = tokio::join!(
        pe.record_labour(&manager, 1, order, &race),
        pe.record_labour(&manager, 1, order, &race)
    );
    verify("concurrent labour one row", id(&a?) == id(&b?))?;

    println!("== authorization tenant boundaries and storage guards");
    refusal(
        "capture without grant",
        pe.record_wastage(&viewer, 1, run, &waste("xp2-viewer", "1"))
            .await,
        "Capture not authorized",
    )?;
    refusal(
        "wrong recorder company",
        pe.record_wastage(&manager, 2, run, &waste("xp2-wrong-company", "1"))
            .await,
        "Recorder company mismatch",
    )?;
    refusal(
        "other tenant cannot list captures",
        pe.list_measurements(&other, 1, run).await,
        "Not found",
    )?;
    refusal(
        "other tenant cannot read config",
        pe.get_wastage_reason(&other, 1, id(&reason)).await,
        "Not found",
    )?;
    let _ = app
        .user
        .impersonate(&token, &admin.csrf, manager.user(), &RequestInfo::default())
        .await
        .map_err(|e| format!("impersonation: {e:?}"))?;
    let impersonated = app.security.guard(&token).await?;
    verify(
        "real impersonation session",
        impersonated.impersonator().is_some(),
    )?;
    refusal(
        "impersonated capture",
        pe.record_wastage(&impersonated, 1, run, &waste("xp2-impersonated", "1"))
            .await,
        "Capture refused under impersonation",
    )?;
    for table in [
        "factory_wastage_records",
        "factory_measurements",
        "factory_labour_entries",
    ] {
        for sql in [
            format!("UPDATE {table} SET note='tampered'"),
            format!("DELETE FROM {table}"),
            format!("TRUNCATE {table} CASCADE"),
        ] {
            c.batch_execute("BEGIN").await?;
            let r = c.batch_execute(&sql).await;
            c.batch_execute("ROLLBACK").await?;
            // Database errors render generically; inspect server message explicitly.
            let message = r
                .err()
                .and_then(|e| e.as_db_error().map(|d| d.message().to_owned()))
                .unwrap_or_default();
            verify(
                "SQL update delete truncate refused",
                message.contains("immutable"),
            )?;
        }
    }
    println!("shop floor walkthrough complete");
    Ok(())
}
