//! H6 runtime proof through real authenticated registry/plans and optional
//! HTTP/MCP edges. No unit-test harness or production data. Run once against
//! a freshly migrated disposable database; plan codes and tokens stay private.
use bilibi_agent_access::{
    AgentAccess, AuthorizeParams, ClientRegistration, CodeExchange, Decision, CLIENT_ID,
};
use bilibi_agent_api::{
    plans::{PlanAuthority, PlanIdentity, PlannedOperation, Plans},
    Registry, SearchOptions,
};
use bilibi_authz::{
    grants::Grants, CapabilityDefinition, RoleDefinition, VocabularyContribution, VOCABULARY,
};
use bilibi_factory_inventory::{
    Decimal, Inventory, MaterialTyping, NewItem, NewLocation, NewUnit, PostingRequest, StockLine,
};
use bilibi_factory_product_definition::{
    NewProduct, NewResource, NewResourceType, ProductDefinition,
};
use bilibi_tenancy::{ProofFuture, Scope};
use bilibi_walkthrough_fixture::registry::{Contribution, Provenance};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime};
use tokio_postgres::Transaction;

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
bilibi_walkthrough_fixture::inventory::submit! { Contribution::new(VOCABULARY,"factory.h6.roles",Provenance {module:"base/authz",source:file!(),line:line!()}, &VocabularyContribution {
    domains:&[],verbs:&[],capabilities:&[] as &[CapabilityDefinition],system_principals:&[],
    roles:&[
        RoleDefinition {code:"manager",name:"manager",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.production-execution.floor.view","factory.production-execution.configuration.view","factory.production-execution.configuration.manage","factory.product-definition.configuration.view","factory.product-definition.configuration.manage","factory.inventory.configuration.view","factory.inventory.configuration.manage","commerce.inventory.item.create","commerce.inventory.item.view"]},
        RoleDefinition {code:"viewer",name:"viewer",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.production-execution.floor.view"]},
    ],
}) }
type Error = Box<dyn std::error::Error>;
const CREATE: &str = "factory.production_execution.order.create";
const COMPLETE: &str = "factory.production_execution.operation.complete";
const OVERRIDES: &str = "factory.production_execution.hold_override.list";
const IMPORT: &str = "factory.production-execution.import";
const OVERRIDE: &str = "factory.production-execution.material-hold.override";
struct Browser {
    app: &'static bilibi_web::App,
    token: String,
}
impl PlanAuthority for Browser {
    fn verify<'a>(&'a self, tx: &'a Transaction<'_>) -> ProofFuture<'a, PlanIdentity> {
        Box::pin(async move {
            tx.query_opt(
                "SELECT id FROM sessions WHERE id=$1 AND expires_at>clock_timestamp() FOR UPDATE",
                &[&self.token],
            )
            .await?
            .ok_or(bilibi_database::Error::Unauthorized)?;
            Ok(PlanIdentity {
                scope: self.app.security.sessions.check(tx, &self.token).await?,
                credential_binding: format!("browser:{}", self.token),
            })
        })
    }
}
struct Actor {
    scope: Scope,
    browser: Browser,
}
async fn actor(app: &'static bilibi_web::App, email: &str) -> Result<Actor, Error> {
    let login = app
        .user
        .login(email, bilibi_walkthrough_fixture::PASSWORD)
        .await?;
    Ok(Actor {
        scope: app.security.guard(&login.token).await?,
        browser: Browser {
            app,
            token: login.token,
        },
    })
}
fn verify(label: &str, correct: bool) -> Result<(), Error> {
    if !correct {
        return Err(format!("{label}: unexpected runtime result").into());
    }
    println!("{label}: verified");
    Ok(())
}
fn op(key: &str, input: Value) -> PlannedOperation {
    PlannedOperation {
        key: key.into(),
        input,
    }
}
async fn preview(plans: &Plans, who: &Actor, ops: &[PlannedOperation]) -> Result<Value, Error> {
    let review = plans.preview(&who.browser, ops).await?;
    if review["valid"] != true {
        return Err(format!("unexpected invalid review: {}", review["results"]).into());
    }
    Ok(review)
}
async fn commit(
    plans: &Plans,
    who: &Actor,
    review: &Value,
    ops: &[PlannedOperation],
) -> bilibi_agent_api::Result<Value> {
    plans
        .commit(
            &who.scope,
            &who.browser,
            review["plan_code"].as_str().unwrap_or_default(),
            ops,
        )
        .await
}
async fn run(plans: &Plans, who: &Actor, ops: &[PlannedOperation]) -> Result<Value, Error> {
    let review = preview(plans, who, ops).await?;
    Ok(commit(plans, who, &review, ops).await?["results"][0]["result"]["result"].clone())
}
async fn read(
    registry: &Registry,
    db: &bilibi_database::Db,
    who: &Actor,
    key: &str,
    input: Value,
) -> bilibi_agent_api::Result<Value> {
    let client = db.get().await.map_err(bilibi_database::Error::from)?;
    Ok(registry.call(&**client, &who.scope, key, &input).await?["result"].clone())
}
async fn counts(db: &bilibi_database::Db) -> Result<Vec<i64>, Error> {
    let client = db.get().await?;
    let mut counts = Vec::new();
    for table in [
        "factory_production_orders",
        "factory_operation_executions",
        "factory_inventory_transactions",
        "factory_inventory_transaction_entries",
        "factory_inventory_identities",
        "factory_material_hold_overrides",
        "base_audit_mutations",
    ] {
        counts.push(
            client
                .query_one(&format!("SELECT count(*) FROM {table}"), &[])
                .await?
                .get(0),
        );
    }
    Ok(counts)
}
async fn denials(db: &bilibi_database::Db, capability: &str) -> Result<i64, Error> {
    Ok(db
        .get()
        .await?
        .query_one(
            "SELECT count(*) FROM authz_decision_log WHERE tenant_id=1 AND user_id=1 AND capability=$1 AND NOT allowed",
            &[&capability],
        )
        .await?
        .get(0))
}
async fn reject(
    plans: &Plans,
    who: &Actor,
    key: &str,
    input: Value,
    reason: &str,
) -> Result<(), Error> {
    let review = plans.preview(&who.browser, &[op(key, input)]).await?;
    verify(
        reason,
        review["valid"] == false
            && review["plan_code"].is_null()
            && review["results"].to_string().contains(reason),
    )
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    if counts(&db).await?[0] != 0 {
        return Err("use a freshly migrated disposable database".into());
    }
    bilibi_walkthrough_fixture::install(&db).await?;
    let app: &'static bilibi_web::App = Box::leak(Box::new(bilibi_web::App::new(
        db.clone(),
        "http://localhost:35762",
    )?));
    // Grants ask for Base Step-up and no fixture account enrolls an
    // authenticator: turn enforcement off, as `BILIBI_ENV=development` does.
    app.step_up.set_enforced(false);
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    grants.reconcile().await?;
    let registry = app.services.get::<Registry>("base/agent_api.registry")?;
    let plans = app.services.get::<Plans>("base/agent_api.plans")?;
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let pd = app
        .services
        .get::<ProductDefinition>(bilibi_factory_product_definition::SERVICE)?;
    let manager = actor(app, "manager@example.test").await?;
    let viewer = actor(app, "viewer@example.test").await?;
    let other = actor(app, "manager@other.example.test").await?;
    let admin = actor(app, "admin@example.test").await?;

    let client = db.get().await?;
    let found = registry
        .search(
            &**client,
            &manager.scope,
            "production",
            SearchOptions {
                limit: 50,
                ..Default::default()
            },
        )
        .await?;
    verify(
        "discovery finds the reviewed completion",
        found.iter().any(|v| v["key"] == COMPLETE),
    )?;
    let described = registry
        .describe(&**client, &manager.scope, COMPLETE)
        .await?;
    verify(
        "completion is a safe write",
        described["safe_write"] == true && described["disposition"] == "plan",
    )?;
    verify(
        "the production guide is available",
        registry
            .guide(&**client, &viewer.scope, "factory.production_execution")
            .await?["body"]
            .as_str()
            .is_some_and(|s| s.contains("conversion_version")),
    )?;
    drop(client);

    let mut units = Vec::new();
    for code in ["native", "observed"] {
        units.push(
            inventory
                .create_unit(
                    &manager.scope,
                    1,
                    &NewUnit {
                        code: code.into(),
                        name: code.into(),
                    },
                )
                .await?
                .id,
        );
    }
    let location = inventory
        .create_location(
            &manager.scope,
            1,
            &NewLocation {
                code: "H6".into(),
                name: "Operation location".into(),
            },
        )
        .await?
        .id;
    let mut items = Vec::new();
    for code in ["H6-INPUT", "H6-OUTPUT"] {
        items.push(
            inventory
                .create_material(
                    &manager.scope,
                    1,
                    &NewItem {
                        sku: code.into(),
                        title: code.into(),
                        status: Some("active".into()),
                        currency_code: Some("MYR".into()),
                        ..Default::default()
                    },
                    units[0],
                    &MaterialTyping::default(),
                )
                .await?
                .item_id,
        );
    }
    let conversion = inventory
        .define_conversion(
            &manager.scope,
            1,
            items[0],
            units[1],
            "0.5".parse::<Decimal>()?,
        )
        .await?;
    let resource_type = pd
        .create_resource_type(
            &manager.scope,
            1,
            &NewResourceType {
                code: "h6".into(),
                name: "Operation resources".into(),
                property_definitions: vec![],
            },
        )
        .await?;
    let resource = pd
        .create_resource(
            &manager.scope,
            1,
            &NewResource {
                code: "h6".into(),
                name: "Operation resource".into(),
                resource_type_id: resource_type.id,
                properties: Default::default(),
            },
        )
        .await?
        .id;
    let product = pd
        .create_product(
            &manager.scope,
            1,
            &NewProduct {
                item_id: items[1],
                code: "H6".into(),
                name: "Operation definition".into(),
            },
        )
        .await?
        .id;
    pd.publish_formula(&manager.scope,1,product,&json!({"lines":[
        {"item_id":items[0],"unit_id":units[0],"role":"input","quantity":"3","material_hold_rule":{"hours":2}},
        {"item_id":items[1],"unit_id":units[0],"role":"output","quantity":"3"}]})).await?;
    pd.publish_routing(&manager.scope,1,product,&json!({"operations":[{"code":"operate","sequence":1,"inputs":[items[0]],"outputs":[items[1]],"allowed_resource_ids":[resource]}]})).await?;
    let receipt = inventory
        .record_receipt(
            &manager.scope,
            1,
            &PostingRequest {
                request_id: "h6-source".into(),
                evidence: "source observation".into(),
                effective_at: Some(SystemTime::now() - Duration::from_secs(3600)),
                lines: vec![StockLine {
                    item_id: items[0],
                    location_id: Some(location),
                    quantity: "30".parse()?,
                    identity: Some(serde_json::from_value(
                        json!({"kind":"lot","code":"H6-SOURCE"}),
                    )?),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await?;
    let identity = receipt
        .entries
        .iter()
        .find_map(|e| e.identity_id)
        .ok_or("source identity")?;
    // The Z suffix denotes UTC, independent of the PostgreSQL session zone.
    let times = db.get().await?.query_one("SELECT to_char((now()-interval '30 minutes') AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'),to_char((now()-interval '1 minute') AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",&[]).await?;
    let (started, completed): (String, String) = (times.get(0), times.get(1));
    let order_input = json!({"company_id":1,"code":"H6-ORDER","kind":"order","product_id":product,"formula_version":1,"routing_version":1,"demand_ref":"source ref"});
    let ops = [op(CREATE, order_input.clone())];
    let before = counts(&db).await?;
    let review = preview(&plans, &manager, &ops).await?;
    verify(
        "order preview leaves business and audit rows unchanged",
        before == counts(&db).await?,
    )?;
    verify(
        "direct write refused",
        read(&registry, &db, &manager, CREATE, order_input.clone())
            .await
            .is_err(),
    )?;
    reject(
        &plans,
        &viewer,
        CREATE,
        order_input.clone(),
        "missing_capability",
    )
    .await?;
    let done = commit(&plans, &manager, &review, &ops).await?;
    let order = done["results"][0]["result"]["result"]["order"]["id"]
        .as_i64()
        .ok_or("order id")?;
    verify(
        "single-use plan",
        commit(&plans, &manager, &review, &ops).await.is_err(),
    )?;
    reject(
        &plans,
        &manager,
        CREATE,
        order_input.clone(),
        "has already been taken",
    )
    .await?;
    let mut bad = order_input.clone();
    bad["code"] = json!("BAD");
    bad["formula_version"] = json!(99);
    reject(&plans, &manager, CREATE, bad, "revision").await?;
    let base = json!({"company_id":1,"order_id":order,"source":"live","request_id":"h6-live","operation_code":"operate","resource_id":resource,
        "operator_type":"user","operator_id":99,"started_at":started,"completed_at":completed,"evidence":"operation observation",
        "inputs":[{"item_id":items[0],"location_id":location,"identity_id":identity,"quantity":"6","unit_id":units[1],"conversion_version":conversion.version,"observation":"measured"}],
        "outputs":[{"item_id":items[1],"location_id":location,"quantity":"3","observation":"counted","identity":{"kind":"lot","code":"H6-OUTPUT"}}]});
    reject(&plans, &manager, COMPLETE, base.clone(), "Material held").await?;
    let mut live = base.clone();
    live["hold_override"] = json!({"reason":"authorized early use"});
    let before = counts(&db).await?;
    let denied = denials(&db, OVERRIDE).await?;
    reject(
        &plans,
        &manager,
        COMPLETE,
        live.clone(),
        "missing_capability",
    )
    .await?;
    verify(
        "refused hold override keeps its authorization decision, not business/audit effects",
        denials(&db, OVERRIDE).await? == denied + 1 && before == counts(&db).await?,
    )?;
    grants
        .set_direct(&admin.scope, manager.scope.user(), OVERRIDE, Some(true))
        .await?;
    let ops = [op(COMPLETE, live.clone())];
    let before = counts(&db).await?;
    let review = preview(&plans, &manager, &ops).await?;
    let repeat = preview(&plans, &manager, &ops).await?;
    verify(
        "live hold and generated identity reviews are stable and leave no business/audit rows",
        review["results"] == repeat["results"] && before == counts(&db).await?,
    )?;
    let reviewed = &review["results"][0]["preview"]["result"];
    verify(
        "review preserves conversion and native effects",
        reviewed["entries"].as_array().is_some_and(|es| {
            es.iter().any(|e| {
                e["native_quantity"] == "-3.000000000000"
                    && e["conversion_version"] == conversion.version
            })
        }),
    )?;
    // A newer conversion must not silently alter the reviewed completion.
    inventory
        .define_conversion(
            &manager.scope,
            1,
            items[0],
            units[1],
            "0.75".parse::<Decimal>()?,
        )
        .await?;
    let done = commit(&plans, &manager, &review, &ops).await?;
    let result = &done["results"][0]["result"]["result"];
    let execution = result["execution"]["id"].as_i64().ok_or("execution id")?;
    verify(
        "pinned conversion survives a newly published version",
        result["entries"].as_array().is_some_and(|es| {
            es.iter().any(|e| {
                e["native_quantity"] == "-3.000000000000"
                    && e["conversion_version"] == conversion.version
            })
        }),
    )?;
    let output_identity = result["entries"]
        .as_array()
        .and_then(|es| {
            es.iter()
                .find(|e| e["item_id"] == items[1] && e["role"] == "stock")
        })
        .and_then(|e| e["identity_id"].as_i64())
        .ok_or("output identity id")?;
    verify(
        "commit returns a resolvable generated identity",
        inventory
            .get_identity(&manager.scope, 1, output_identity)
            .await?
            .code
            == "H6-OUTPUT",
    )?;
    let holds = read(
        &registry,
        &db,
        &viewer,
        OVERRIDES,
        json!({"company_id":1,"execution_id":execution}),
    )
    .await?;
    verify(
        "hold read keeps authenticated recorder, not supplied operator",
        holds[0]["actor_id"] == manager.scope.user()
            && holds[0]["recorded_by_id"] == manager.scope.user()
            && holds[0]["source_transaction_id"] == receipt.id,
    )?;
    grants
        .set_direct(&admin.scope, manager.scope.user(), OVERRIDE, Some(false))
        .await?;
    let before = counts(&db).await?;
    let replay = run(&plans, &manager, &ops).await?;
    verify(
        "historical hold retry needs no new override grant and posts nothing",
        replay["replay"] == true && before == counts(&db).await?,
    )?;
    let mut conflict = live.clone();
    conflict["evidence"] = json!("different");
    reject(&plans, &manager, COMPLETE, conflict, "Request id conflict").await?;
    grants
        .set_direct(&admin.scope, manager.scope.user(), OVERRIDE, Some(true))
        .await?;
    let mut unpinned = live.clone();
    unpinned["request_id"] = json!("h6-unpinned");
    unpinned["inputs"][0]
        .as_object_mut()
        .unwrap()
        .remove("conversion_version");
    reject(
        &plans,
        &manager,
        COMPLETE,
        unpinned,
        "pin the conversion version",
    )
    .await?;
    let mut imported = base.clone();
    imported["request_id"] = json!("h6-import");
    imported["source"] = json!("import");
    imported["outputs"][0]["identity"]["code"] = json!("H6-IMPORTED");
    imported["hold_override"] = json!({"reason":"historical authorization","evidence":"source log","occurred_at":started,"approver":{"kind":"user","id":42}});
    let before = counts(&db).await?;
    let denied = denials(&db, IMPORT).await?;
    reject(
        &plans,
        &manager,
        COMPLETE,
        imported.clone(),
        "missing_capability",
    )
    .await?;
    verify(
        "refused import keeps its authorization decision, not business/audit effects",
        denials(&db, IMPORT).await? == denied + 1 && before == counts(&db).await?,
    )?;
    for cap in [IMPORT, OVERRIDE] {
        grants
            .set_direct(&admin.scope, manager.scope.user(), cap, Some(true))
            .await?;
    }
    // Each grant is rechecked when committing an already reviewed new write.
    for cap in [IMPORT, OVERRIDE] {
        let ops = [op(COMPLETE, imported.clone())];
        let review = preview(&plans, &manager, &ops).await?;
        grants
            .set_direct(&admin.scope, manager.scope.user(), cap, Some(false))
            .await?;
        let before = counts(&db).await?;
        verify(
            "revoked additional grant refuses commit with no partial effects",
            commit(&plans, &manager, &review, &ops).await.is_err() && before == counts(&db).await?,
        )?;
        grants
            .set_direct(&admin.scope, manager.scope.user(), cap, Some(true))
            .await?;
    }
    let done = run(&plans, &manager, &[op(COMPLETE, imported.clone())]).await?;
    let imp_execution = done["execution"]["id"].as_i64().ok_or("import execution")?;
    let holds = read(
        &registry,
        &db,
        &viewer,
        OVERRIDES,
        json!({"company_id":1,"execution_id":imp_execution}),
    )
    .await?;
    verify(
        "import keeps source approver separate from recorder",
        holds[0]["actor_id"] == 42
            && holds[0]["recorded_by_id"] == manager.scope.user()
            && holds[0]["evidence"] == "source log",
    )?;
    grants
        .set_direct(&admin.scope, manager.scope.user(), IMPORT, Some(false))
        .await?;
    reject(&plans, &manager, COMPLETE, imported, "missing_capability").await?;
    grants
        .set_direct(&admin.scope, manager.scope.user(), IMPORT, Some(true))
        .await?;

    // Real individually feasible completions that overdraw together must
    // roll back all their entries, executions, identities, overrides and audit.
    let mut both = Vec::new();
    for suffix in ["a", "b"] {
        let mut input = base.clone();
        input["request_id"] = json!(format!("h6-atomic-{suffix}"));
        input["inputs"][0]["quantity"] = json!("40");
        input["outputs"][0]["quantity"] = json!("20");
        input["outputs"][0]["identity"]["code"] = json!(format!("H6-ATOMIC-{suffix}"));
        input["hold_override"] = json!({"reason":"authorized draw"});
        both.push(op(COMPLETE, input));
    }
    let review = preview(&plans, &manager, &both).await?;
    let before = counts(&db).await?;
    verify(
        "second operation failure rolls back every business and audit row",
        commit(&plans, &manager, &review, &both).await.is_err() && before == counts(&db).await?,
    )?;
    // The same code was not consumed by failure (now both lines can be drawn).
    inventory
        .record_correction(
            &manager.scope,
            1,
            &PostingRequest {
                request_id: "h6-replenish".into(),
                evidence: "source quantity correction".into(),
                reason: Some("additional quantity on the source observation".into()),
                corrects_transaction_id: Some(receipt.id),
                lines: vec![StockLine {
                    item_id: items[0],
                    location_id: Some(location),
                    quantity: "30".parse()?,
                    identity_id: Some(identity),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await?;
    verify(
        "failed plan code remains usable after replenishment",
        commit(&plans, &manager, &review, &both).await.is_ok(),
    )?;
    let mut malformed = base.clone();
    malformed["posting_authority"] = json!("factory/production_execution");
    reject(&plans, &manager, COMPLETE, malformed, "posting_authority").await?;
    let mut malformed = base.clone();
    malformed["hold_override"] = json!({"reason":"x","recorded_by_id":42});
    reject(&plans, &manager, COMPLETE, malformed, "recorded_by_id").await?;
    let mut malformed = base.clone();
    malformed["inputs"][0]["quantity"] = json!(6.5);
    reject(&plans, &manager, COMPLETE, malformed, "quantity").await?;
    let mut historical = base.clone();
    historical["request_id"] = json!("h6-pre-epoch");
    historical["started_at"] = json!("1969-12-31T23:59:59Z");
    historical["inputs"] = json!([]);
    historical["outputs"][0]["identity"]["code"] = json!("H6-HISTORICAL");
    let before = counts(&db).await?;
    reject(&plans, &manager, COMPLETE, historical, "Invalid execution").await?;
    verify(
        "unserializable historical time refuses without effects or panic",
        before == counts(&db).await?,
    )?;
    verify(
        "another tenant cannot read hold evidence",
        read(
            &registry,
            &db,
            &other,
            OVERRIDES,
            json!({"company_id":1,"execution_id":execution}),
        )
        .await
        .is_err(),
    )?;
    let before = counts(&db).await?;
    let review = plans.preview(&other.browser, &[op(CREATE,json!({"company_id":1,"code":"CROSS","kind":"order","product_id":product,"formula_version":1,"routing_version":1}))]).await?;
    verify(
        "another tenant's write is rejected without disclosing record existence or effects",
        review["valid"] == false
            && review["plan_code"].is_null()
            && review["results"][0]["error"]["code"] == "rejected"
            && before == counts(&db).await?,
    )?;
    let client = db.get().await?;
    let rows = client.query_one("SELECT (SELECT count(*) FROM factory_operation_executions),(SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_operation_executions')",&[]).await?;
    verify(
        "every execution has exactly one audit row",
        rows.get::<_, i64>(0) == rows.get::<_, i64>(1),
    )?;
    drop(client);
    if let Ok(api) = std::env::var("BILIBI_FACTORY_API") {
        over_http(app, &manager, &api, order_input, base).await?;
    }
    println!(
        "production operations walkthrough complete; counts orders/executions/postings/entries/identities/overrides/audit={:?}",
        counts(&db).await?
    );
    Ok(())
}

fn curl(api: &str, token: &str, path: &str, body: &Value) -> Result<Value, Error> {
    let output = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "30",
            "-X",
            "POST",
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "Content-Type: application/json",
            "-H",
            "Accept: application/json, text/event-stream",
            "-d",
            &body.to_string(),
            "-w",
            "\n%{http_code}",
        ])
        .arg(format!("{api}{path}"))
        .output()?;
    let text = String::from_utf8(output.stdout)?;
    let (body, status) = text.rsplit_once('\n').ok_or("no HTTP status")?;
    let mut result: Value = serde_json::from_str(body)?;
    result["http_status"] = json!(status);
    Ok(result)
}
async fn over_http(
    app: &'static bilibi_web::App,
    manager: &Actor,
    api: &str,
    mut order: Value,
    mut completion: Value,
) -> Result<(), Error> {
    let mut consume_line = completion["inputs"][0].clone();
    consume_line["quantity"] = json!("1");
    for key in ["unit_id", "conversion_version"] {
        consume_line.as_object_mut().unwrap().remove(key);
    }
    let access = app
        .services
        .get::<AgentAccess>("core/agent_access.grants")?;
    let start = access.start_device(CLIENT_ID, "H6 walkthrough").await?;
    access
        .decide(
            &manager.scope,
            &start.user_code,
            Decision::Approve,
            access.connection_expiry().await?,
        )
        .await?;
    let db = app
        .services
        .get::<bilibi_database::Db>("base/database.pool")?;
    db.get()
        .await?
        .execute(
            "UPDATE core_agent_devices SET next_poll_at=now() WHERE status='approved'",
            &[],
        )
        .await?;
    let token = access
        .exchange(CLIENT_ID, &start.device_code)
        .await?
        .access_token;
    order["code"] = json!("H6-HTTP");
    let ops = json!([{"key":CREATE,"input":order}]);
    let direct = curl(
        api,
        &token,
        &format!("/api/v1/operations/{CREATE}/call"),
        &json!({"input":ops[0]["input"]}),
    )?;
    verify(
        "HTTP refuses direct production writes",
        direct["http_status"] != "200",
    )?;
    let review = curl(api, &token, "/api/v1/plans", &json!({"operations":ops}))?;
    verify(
        "HTTP reviews order creation",
        review["http_status"] == "200" && review["valid"] == true,
    )?;
    let done = curl(
        api,
        &token,
        "/api/v1/plans/commit",
        &json!({"operations":ops,"plan_code":review["plan_code"]}),
    )?;
    verify("HTTP commits order creation", done["http_status"] == "200")?;
    completion["order_id"] = done["results"][0]["result"]["result"]["order"]["id"].clone();
    completion["request_id"] = json!("h6-http-complete");
    completion["inputs"] = json!([]);
    completion["outputs"][0]["identity"]["code"] = json!("H6-HTTP-OUTPUT");
    let ops = json!([{"key":COMPLETE,"input":completion}]);
    let review = curl(api, &token, "/api/v1/plans", &json!({"operations":ops}))?;
    verify(
        "HTTP reviews output-only completion",
        review["valid"] == true,
    )?;
    let done = curl(
        api,
        &token,
        "/api/v1/plans/commit",
        &json!({"operations":ops,"plan_code":review["plan_code"]}),
    )?;
    verify(
        "HTTP commits output-only completion",
        done["http_status"] == "200"
            && done["results"][0]["result"]["result"]["posting_kind"] == "output",
    )?;
    let execution = done["results"][0]["result"]["result"]["execution"]["id"].clone();
    // MCP has a distinct resource audience and a public, dynamically registered
    // PKCE client, not the HTTP API's native-device token (Platform Agent Access).
    let registration = access
        .register_client(&serde_json::from_value::<ClientRegistration>(json!({
            "client_name": "H6 proof", "redirect_uris": ["https://proof.example.test/callback"],
            "token_endpoint_auth_method": "none"
        }))?)
        .await?;
    let client_id = registration["client_id"]
        .as_str()
        .ok_or("registered client ID")?;
    let metadata = std::process::Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "30",
            &format!("{api}/.well-known/oauth-protected-resource/mcp"),
        ])
        .output()?;
    let metadata: Value = serde_json::from_slice(&metadata.stdout)?;
    let resource = metadata["resource"].as_str().ok_or("MCP resource")?;
    // RFC7636 example verifier is public proof-fixture data, not a credential.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let params = AuthorizeParams {
        client_id: client_id.into(),
        redirect_uri: "https://proof.example.test/callback".into(),
        response_type: "code".into(),
        resource: resource.into(),
        scope: None,
        code_challenge: bilibi_agent_access::code_challenge(verifier),
        code_challenge_method: "S256".into(),
        state: None,
    };
    let code = access
        .authorize(
            &manager.scope,
            &params,
            resource,
            access.connection_expiry().await?,
        )
        .await?;
    let token = access
        .exchange_code(
            &CodeExchange {
                code,
                client_id: params.client_id,
                redirect_uri: params.redirect_uri,
                code_verifier: verifier.into(),
                resource: resource.into(),
            },
            resource,
        )
        .await?
        .access_token;
    let mcp = |name: &str, arguments: Value| {
        curl(
            api,
            &token,
            "/mcp",
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        )
    };
    let result = mcp("search", json!({"query":"production"}))?;
    verify(
        "MCP discovers the same production operations",
        result["http_status"] == "200" && result.to_string().contains(COMPLETE),
    )?;
    let result = mcp(
        "read",
        json!({"key":OVERRIDES,"input":{"company_id":1,"execution_id":execution}}),
    )?;
    verify(
        "MCP reads hold evidence",
        result["http_status"] == "200" && result["result"]["isError"] != true,
    )?;
    // OAuth recorder is an agent acting for the approving person. Consume a
    // small held quantity through MCP and inspect the immutable evidence.
    let mut input = ops[0]["input"].clone();
    input["request_id"] = json!("h6-mcp-consume");
    input["outputs"] = json!([]);
    input["inputs"] = json!([consume_line]);
    input["hold_override"] = json!({"reason":"MCP authorized draw"});
    let ops = json!([{"key":COMPLETE,"input":input}]);
    let review = mcp("plan", json!({"operations":ops}))?;
    let payload = mcp_payload(&review)?;
    verify(
        "MCP reviews consumption-only completion",
        payload["valid"] == true,
    )?;
    let done = mcp(
        "commit",
        json!({"operations":ops,"plan_code":payload["plan_code"]}),
    )?;
    let done = mcp_payload(&done)?;
    let result = &done["results"][0]["result"]["result"];
    verify(
        "MCP commits consumption-only with agent recorder",
        result["posting_kind"] == "consumption"
            && result["hold_overrides"][0]["recorded_by_type"] == "agent"
            && result["hold_overrides"][0]["recorded_by_acting_for_user_id"]
                == manager.scope.user(),
    )?;
    println!("HTTP and MCP production reach complete (credentials and plan codes withheld)");
    Ok(())
}
fn mcp_payload(value: &Value) -> Result<Value, Error> {
    if value["result"]["isError"] == true {
        return Err(format!("MCP error: {}", value["result"]["content"]).into());
    }
    Ok(serde_json::from_str(
        value["result"]["content"][0]["text"]
            .as_str()
            .ok_or("MCP text")?,
    )?)
}
