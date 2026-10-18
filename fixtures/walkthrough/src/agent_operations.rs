//! H5 live walkthrough: Inventory's agent operations (locations, receipts,
//! transfers, consumption, corrections, and the stock, transaction and
//! genealogy reads) driven through the real registry and the reviewed
//! safe-write plan service as signed-in accounts. Run once on a freshly
//! migrated disposable database; it checks each runtime outcome and exits
//! nonzero on an unexpected one. Plan codes are never printed.
use bilibi_agent_access::{AgentAccess, Decision, CLIENT_ID};
use bilibi_agent_api::{
    plans::{PlanAuthority, PlanIdentity, PlannedOperation, Plans},
    Registry, SearchOptions,
};
use bilibi_authz::{
    grants::Grants, CapabilityDefinition, RoleDefinition, VocabularyContribution, VOCABULARY,
};
use bilibi_factory_inventory::{Decimal, Inventory, MaterialTyping, NewItem, NewUnit};
use bilibi_tenancy::{ProofFuture, Scope};
use bilibi_walkthrough_fixture::registry::{Contribution, Provenance};
use serde_json::{json, Value};
use tokio_postgres::Transaction;

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
bilibi_walkthrough_fixture::inventory::submit! { Contribution::new(VOCABULARY,"factory.h5.roles",Provenance {module:"base/authz",source:file!(),line:line!()}, &VocabularyContribution {
    domains:&[],verbs:&[],capabilities:&[] as &[CapabilityDefinition],system_principals:&[],
    roles:&[
        RoleDefinition {code:"manager",name:"manager",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.inventory.configuration.view","factory.inventory.configuration.manage","commerce.inventory.item.create","commerce.inventory.item.view"]},
        RoleDefinition {code:"viewer",name:"viewer",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.inventory.configuration.view"]},
    ],
}) }

type Error = Box<dyn std::error::Error>;

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
            let scope = self.app.security.sessions.check(tx, &self.token).await?;
            Ok(PlanIdentity {
                scope,
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
/// A review with its plan code withheld from the printout.
async fn preview(plans: &Plans, who: &Actor, ops: &[PlannedOperation]) -> Result<Value, Error> {
    let review = plans.preview(&who.browser, ops).await?;
    let mut shown = review.clone();
    if let Some(object) = shown.as_object_mut() {
        object.remove("plan_code");
    }
    println!(
        "review: {shown}; code issued={}",
        review["plan_code"].is_string()
    );
    Ok(review)
}
async fn commit(
    plans: &Plans,
    who: &Actor,
    review: &Value,
    ops: &[PlannedOperation],
) -> Result<Value, bilibi_agent_api::Error> {
    plans
        .commit(
            &who.scope,
            &who.browser,
            review["plan_code"].as_str().unwrap_or_default(),
            ops,
        )
        .await
}
async fn read(
    registry: &Registry,
    db: &bilibi_database::Db,
    who: &Actor,
    key: &str,
    input: Value,
) -> Result<Value, Error> {
    let client = db.get().await?;
    let value = registry.call(&**client, &who.scope, key, &input).await?;
    Ok(value["result"].clone())
}
async fn count(db: &bilibi_database::Db, sql: &str) -> Result<i64, Error> {
    Ok(db.get().await?.query_one(sql, &[]).await?.get(0))
}
fn error_text(error: &bilibi_agent_api::Error) -> String {
    match error {
        bilibi_agent_api::Error::PlanRejected(results) => results.to_string(),
        other => other.to_string(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    bilibi_walkthrough_fixture::install(&db).await?;
    // One App per process: its services bind module-wide singletons.
    let app: &'static bilibi_web::App = Box::leak(Box::new(bilibi_web::App::new(
        db.clone(),
        "http://localhost:35592",
    )?));
    // Grants ask for Base Step-up and no fixture account enrolls an
    // authenticator: turn enforcement off, as `BILIBI_ENV=development` does.
    app.step_up.set_enforced(false);
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    grants.reconcile().await?;
    let plans = app.services.get::<Plans>("base/agent_api.plans")?;
    let registry = app.services.get::<Registry>("base/agent_api.registry")?;
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let manager = actor(app, "manager@example.test").await?;
    let viewer = actor(app, "viewer@example.test").await?;
    let other = actor(app, "manager@other.example.test").await?;
    let admin = actor(app, "admin@example.test").await?;
    if count(&db, "SELECT count(*) FROM factory_inventory_transactions").await? != 0 {
        return Err("use a fresh disposable database".into());
    }

    println!("== discovery");
    let keys = |found: Vec<Value>| -> Vec<String> {
        found
            .iter()
            .filter_map(|entry| entry["key"].as_str().map(str::to_owned))
            .collect()
    };
    let client = db.get().await?;
    let found = registry
        .search(
            &**client,
            &manager.scope,
            "receipt stock",
            SearchOptions::default(),
        )
        .await?;
    println!(
        "manager search 'receipt stock': {}",
        json!(keys(found.clone()))
    );
    verify(
        "a manager finds the receipt write and the stock read",
        keys(found)
            .iter()
            .any(|key| key == "factory.inventory.receipt.record"),
    )?;
    let all = registry
        .search(
            &**client,
            &viewer.scope,
            "",
            SearchOptions {
                limit: 50,
                ..Default::default()
            },
        )
        .await?;
    let viewer_keys = keys(all);
    println!("viewer browse: {}", json!(viewer_keys));
    verify(
        "a viewer reaches the reads and none of the writes",
        viewer_keys
            .iter()
            .any(|key| key == "factory.inventory.stock.position")
            && !viewer_keys
                .iter()
                .any(|key| key.ends_with(".record") || key.ends_with(".create")),
    )?;
    let described = registry
        .describe(
            &**client,
            &manager.scope,
            "factory.inventory.transfer.record",
        )
        .await?;
    println!(
        "describe transfer.record: kind={} safe_write={} disposition={} capability={}",
        described["kind"],
        described["safe_write"],
        described["disposition"],
        described["capability"]
    );
    println!(
        "viewer describe of a write: {:?}",
        registry
            .describe(
                &**client,
                &viewer.scope,
                "factory.inventory.transfer.record"
            )
            .await
            .map(|_| ())
    );
    let guide = registry
        .guide(&**client, &viewer.scope, "factory.inventory")
        .await?;
    println!(
        "guide: {} ({} bytes)",
        guide["title"],
        guide["body"].as_str().map_or(0, str::len)
    );
    drop(client);

    println!("== catalog (Materials page territory, set up through the facade)");
    let mut units = Vec::new();
    for code in ["kg", "g"] {
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
    let mut items = Vec::new();
    for sku in ["H5-ALPHA", "H5-BETA"] {
        items.push(
            inventory
                .create_material(
                    &manager.scope,
                    1,
                    &NewItem {
                        sku: sku.into(),
                        title: sku.into(),
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
    let (alpha, beta) = (items[0], items[1]);
    inventory
        .define_conversion(
            &manager.scope,
            1,
            alpha,
            units[1],
            "0.001".parse::<Decimal>()?,
        )
        .await?;

    println!("== locations");
    let direct = {
        let client = db.get().await?;
        registry
            .call(
                &**client,
                &manager.scope,
                "factory.inventory.location.create",
                &json!({"company_id":1,"code":"store","name":"Store"}),
            )
            .await
    };
    println!("direct write: {:?}", direct.as_ref().map(|_| ()));
    verify("a direct write is refused", direct.is_err())?;
    let bad = [op(
        "factory.inventory.location.create",
        json!({"company_id":1,"code":"  ","name":"Blank"}),
    )];
    let review = preview(&plans, &manager, &bad).await?;
    verify(
        "a blank code is refused in review, no code issued",
        review["valid"] == false && review["plan_code"].is_null(),
    )?;
    let ops = [
        op(
            "factory.inventory.location.create",
            json!({"company_id":1,"code":"store","name":"Store"}),
        ),
        op(
            "factory.inventory.location.create",
            json!({"company_id":1,"code":"floor","name":"Shop floor"}),
        ),
    ];
    let review = preview(&plans, &manager, &ops).await?;
    verify(
        "review made no location",
        review["valid"] == true
            && count(&db, "SELECT count(*) FROM factory_inventory_locations").await? == 0,
    )?;
    let viewer_try = plans.preview(&viewer.browser, &ops).await?;
    verify(
        "a viewer's review of the same write is invalid",
        viewer_try["valid"] == false,
    )?;
    let done = commit(&plans, &manager, &review, &ops).await?;
    println!("committed: {done}");
    verify(
        "commit made both locations, audited under the table",
        count(&db, "SELECT count(*) FROM factory_inventory_locations").await? == 2
            && count(&db, "SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_inventory_locations'").await? == 2,
    )?;
    println!(
        "replay of the code: {:?}",
        commit(&plans, &manager, &review, &ops).await.map(|_| ())
    );
    verify(
        "a used code cannot be replayed",
        commit(&plans, &manager, &review, &ops).await.is_err(),
    )?;
    let dup = [op(
        "factory.inventory.location.create",
        json!({"company_id":1,"code":"STORE","name":"Again"}),
    )];
    let review = preview(&plans, &manager, &dup).await?;
    verify(
        "a taken code is refused in review",
        review["valid"] == false,
    )?;
    let listed = read(
        &registry,
        &db,
        &viewer,
        "factory.inventory.location.list",
        json!({"company_id":1}),
    )
    .await?;
    println!("locations: {listed}");
    let floor_id = listed
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["code"] == "FLOOR"))
        .and_then(|row| row["id"].as_i64())
        .ok_or("floor")?;
    let store_id = listed
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["code"] == "STORE"))
        .and_then(|row| row["id"].as_i64())
        .ok_or("store")?;
    println!(
        "one location: {}",
        read(
            &registry,
            &db,
            &viewer,
            "factory.inventory.location.get",
            json!({"company_id":1,"location_id":store_id})
        )
        .await?
    );

    println!("== receipts");
    let receipt = |request: &str, quantity: &str| {
        op(
            "factory.inventory.receipt.record",
            json!({"company_id":1,"request_id":request,"evidence":"delivery note 1",
            "lines":[
                {"item_id":alpha,"location_id":store_id,"quantity":quantity,"observation":"counted",
                 "identity":{"kind":"lot","code":"LOT-1"}},
                {"item_id":alpha,"location_id":store_id,"quantity":"500","unit_id":units[1],"observation":"declared"}
            ]}),
        )
    };
    let ops = [receipt("h5-receipt-1", "100")];
    let review = preview(&plans, &manager, &ops).await?;
    verify(
        "review shows 100 + 500 g converted to 0.5 kg (100.5 kg after), writes nothing",
        review["valid"] == true
            && review["results"][0]["preview"]["result"]["stock"][0]["after"] == "100.500000000000"
            && count(&db, "SELECT count(*) FROM factory_inventory_transactions").await? == 0
            && count(&db, "SELECT count(*) FROM factory_inventory_identities").await? == 0,
    )?;
    let done = commit(&plans, &manager, &review, &ops).await?;
    let receipt_id = done["results"][0]["result"]["result"]["id"]
        .as_i64()
        .ok_or("receipt id")?;
    println!("committed transaction {receipt_id}");
    verify(
        "the receipt, its lot and the audit rows exist once",
        count(&db, "SELECT count(*) FROM factory_inventory_transactions").await? == 1
            && count(&db, "SELECT count(*) FROM factory_inventory_identities").await? == 1
            && count(&db, "SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_inventory_transactions'").await? == 1,
    )?;
    let again = preview(&plans, &manager, &ops).await?;
    verify(
        "the same request again reviews as a replay that changes nothing",
        again["results"][0]["preview"]["result"]["replay"] == true,
    )?;
    let replayed = commit(&plans, &manager, &again, &ops).await?;
    verify(
        "its commit returns the original transaction and adds no rows",
        replayed["results"][0]["result"]["result"]["id"] == receipt_id
            && count(&db, "SELECT count(*) FROM factory_inventory_transactions").await? == 1,
    )?;
    let conflict = [receipt("h5-receipt-1", "7")];
    let review = preview(&plans, &manager, &conflict).await?;
    verify(
        "the request id with other content is refused",
        review["valid"] == false && review.to_string().contains("Request id conflict"),
    )?;
    let weighed = [op(
        "factory.inventory.receipt.record",
        json!({"company_id":1,"request_id":"h5-weighed","evidence":"weigh ticket 7",
            "receipt_measurement":{"supplier_declared":"50","measured_gross":"52.5","tare":"2.5","net":"50","unit_id":units[0],"weighing_point_ref":"SCALE-1"},
            "lines":[{"item_id":beta,"location_id":store_id,"quantity":"50","unit_id":units[0],"observation":"measured"}]}),
    )];
    let review = preview(&plans, &manager, &weighed).await?;
    commit(&plans, &manager, &review, &weighed).await?;
    let wrong = [op(
        "factory.inventory.receipt.record",
        json!({"company_id":1,"request_id":"h5-weighed-bad","evidence":"weigh ticket 8",
            "receipt_measurement":{"supplier_declared":"50","measured_gross":"52.5","tare":"2.5","net":"49","unit_id":units[0],"weighing_point_ref":"SCALE-1"},
            "lines":[{"item_id":beta,"location_id":store_id,"quantity":"49","unit_id":units[0],"observation":"measured"}]}),
    )];
    verify(
        "a weigh ticket whose gross less tare is not net is refused",
        preview(&plans, &manager, &wrong).await?["valid"] == false,
    )?;
    let production = [op(
        "factory.inventory.receipt.record",
        json!({"company_id":1,"request_id":"h5-production","evidence":"x","context":{"operation_execution":"OP-1"},
            "lines":[{"item_id":alpha,"location_id":store_id,"quantity":"1","observation":"counted"}]}),
    )];
    verify(
        "production context is not accepted from an agent",
        preview(&plans, &manager, &production).await?["valid"] == false,
    )?;

    println!("== transfers, consumption, stock");
    let lot = read(
        &registry,
        &db,
        &viewer,
        "factory.inventory.transaction.get",
        json!({"company_id":1,"transaction_id":receipt_id}),
    )
    .await?;
    let lot_id = lot["entries"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find_map(|entry| entry["identity_id"].as_i64())
        })
        .ok_or("lot identity")?;
    let transfer = [op(
        "factory.inventory.transfer.record",
        json!({"company_id":1,"request_id":"h5-transfer","evidence":"move to the floor",
            "lines":[{"item_id":alpha,"from_location_id":store_id,"to_location_id":floor_id,"quantity":"30","observation":"counted","identity_id":lot_id}]}),
    )];
    let review = preview(&plans, &manager, &transfer).await?;
    commit(&plans, &manager, &review, &transfer).await?;
    let position = |location: i64| {
        read(
            &registry,
            &db,
            &viewer,
            "factory.inventory.stock.position",
            json!({"company_id":1,"item_id":alpha,"location_id":location}),
        )
    };
    let (store, floor) = (position(store_id).await?, position(floor_id).await?);
    println!("store {store}; floor {floor}");
    verify(
        "stock moved exactly: store 70.5, floor 30",
        store["quantity"] == "70.500000000000" && floor["quantity"] == "30.000000000000",
    )?;
    let same = [op(
        "factory.inventory.transfer.record",
        json!({"company_id":1,"request_id":"h5-same","evidence":"x","lines":[{"item_id":alpha,"from_location_id":store_id,"to_location_id":store_id,"quantity":"1","observation":"counted"}]}),
    )];
    verify(
        "a transfer to the same location is refused",
        preview(&plans, &manager, &same).await?["valid"] == false,
    )?;
    let too_much = [op(
        "factory.inventory.consumption.record",
        json!({"company_id":1,"request_id":"h5-too-much","evidence":"issue","lines":[{"item_id":alpha,"location_id":floor_id,"quantity":"80","observation":"measured"}]}),
    )];
    let review = preview(&plans, &manager, &too_much).await?;
    verify(
        "consuming more than the location holds is refused",
        review["valid"] == false && review.to_string().contains("Insufficient"),
    )?;
    let consume = |request: &str, quantity: &str, location: i64| {
        op(
            "factory.inventory.consumption.record",
            json!({"company_id":1,"request_id":request,"evidence":"issue to line","lines":[{"item_id":alpha,"location_id":location,"quantity":quantity,"observation":"measured","identity_id":lot_id}]}),
        )
    };
    let first = [consume("h5-consume-a", "60", store_id)];
    let stale = preview(&plans, &manager, &first).await?;
    let second = [consume("h5-consume-b", "20", store_id)];
    let review = preview(&plans, &manager, &second).await?;
    commit(&plans, &manager, &review, &second).await?;
    let late = commit(&plans, &manager, &stale, &first).await;
    println!(
        "commit of a review made before the stock changed: {:?}",
        late.as_ref().map_err(error_text).map(|_| ())
    );
    verify(
        "a review the stock has since outrun cannot commit",
        late.is_err(),
    )?;
    verify(
        "and left no transaction behind",
        count(
            &db,
            "SELECT count(*) FROM factory_inventory_transactions WHERE request_id='h5-consume-a'",
        )
        .await?
            == 0,
    )?;

    println!("== a grant revoked between review and commit");
    let ops = [consume("h5-consume-c", "1", store_id)];
    let review = preview(&plans, &manager, &ops).await?;
    grants
        .set_direct(
            &admin.scope,
            manager.scope.user(),
            "factory.inventory.configuration.manage",
            Some(false),
        )
        .await?;
    let revoked = commit(&plans, &manager, &review, &ops).await;
    println!(
        "commit after revocation: {:?}",
        revoked.as_ref().map_err(error_text).map(|_| ())
    );
    verify("the commit is refused and writes nothing", revoked.is_err() && count(&db, "SELECT count(*) FROM factory_inventory_transactions WHERE request_id='h5-consume-c'").await? == 0)?;
    grants
        .set_direct(
            &admin.scope,
            manager.scope.user(),
            "factory.inventory.configuration.manage",
            None,
        )
        .await?;

    println!("== corrections");
    let correction = [op(
        "factory.inventory.correction.record",
        json!({"company_id":1,"request_id":"h5-correction","evidence":"cycle count","reason":"short by 5 on recount","corrects_transaction_id":receipt_id,
            "lines":[{"item_id":alpha,"location_id":store_id,"quantity":"-5","observation":"counted","identity_id":lot_id}]}),
    )];
    let review = preview(&plans, &manager, &correction).await?;
    commit(&plans, &manager, &review, &correction).await?;
    let store = position(store_id).await?;
    verify(
        "the signed correction lowers the store to 45.5",
        store["quantity"] == "45.500000000000",
    )?;
    let unreasoned = [op(
        "factory.inventory.correction.record",
        json!({"company_id":1,"request_id":"h5-no-reason","evidence":"x","reason":" ","corrects_transaction_id":receipt_id,
            "lines":[{"item_id":alpha,"location_id":store_id,"quantity":"-1","observation":"counted"}]}),
    )];
    verify(
        "a correction without a reason is refused",
        preview(&plans, &manager, &unreasoned).await?["valid"] == false,
    )?;

    println!("== transaction and genealogy reads");
    let listed = read(
        &registry,
        &db,
        &viewer,
        "factory.inventory.transaction.list",
        json!({"company_id":1,"item_id":alpha,"limit":3}),
    )
    .await?;
    println!(
        "newest three for alpha: {}",
        listed.as_array().map_or(0, Vec::len)
    );
    verify(
        "newest first, limited",
        listed
            .as_array()
            .is_some_and(|rows| rows.len() == 3 && rows[0]["kind"] == "correction"),
    )?;
    let corrections = read(
        &registry,
        &db,
        &viewer,
        "factory.inventory.transaction.corrections",
        json!({"company_id":1,"transaction_id":receipt_id}),
    )
    .await?;
    verify(
        "the receipt lists its one correction",
        corrections.as_array().is_some_and(|rows| rows.len() == 1),
    )?;
    println!(
        "balance: {}",
        read(
            &registry,
            &db,
            &viewer,
            "factory.inventory.transaction.balance",
            json!({"company_id":1,"transaction_id":receipt_id})
        )
        .await?
    );
    println!(
        "identity: {}",
        read(
            &registry,
            &db,
            &viewer,
            "factory.inventory.identity.get",
            json!({"company_id":1,"identity_id":lot_id})
        )
        .await?
    );
    let positions = read(
        &registry,
        &db,
        &viewer,
        "factory.inventory.identity.positions",
        json!({"company_id":1,"identity_id":lot_id}),
    )
    .await?;
    println!("identity positions: {positions}");
    verify(
        "the lot is held on the floor",
        positions.as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row["location"]["code"] == "FLOOR" && row["quantity"] == "30.000000000000"
            })
        }),
    )?;
    let trace = read(
        &registry,
        &db,
        &viewer,
        "factory.inventory.identity.trace",
        json!({"company_id":1,"identity_id":lot_id,"direction":"backward"}),
    )
    .await?;
    verify(
        "a lot with no transform traces to itself and its receipt",
        trace["identities"]
            .as_array()
            .is_some_and(|rows| rows.len() == 1)
            && trace["receipts"]
                .as_array()
                .is_some_and(|rows| rows.len() == 1),
    )?;
    println!(
        "draws: {}",
        read(
            &registry,
            &db,
            &viewer,
            "factory.inventory.identity.draws",
            json!({"company_id":1,"identity_id":lot_id})
        )
        .await?
    );

    println!("== another tenant");
    let hidden = {
        let client = db.get().await?;
        registry
            .call(
                &**client,
                &other.scope,
                "factory.inventory.location.list",
                &json!({"company_id":1}),
            )
            .await
    };
    println!(
        "other tenant's read of company 1: {:?}",
        hidden
            .as_ref()
            .map(|_| ())
            .map_err(|error| error.to_string())
    );
    verify(
        "another tenant's account cannot read or review",
        hidden.is_err() && preview(&plans, &other, &ops).await?["valid"] == false,
    )?;

    println!("== totals");
    println!(
        "transactions={} entries={} audited transaction rows={}",
        count(&db, "SELECT count(*) FROM factory_inventory_transactions").await?,
        count(&db, "SELECT count(*) FROM factory_inventory_transaction_entries").await?,
        count(&db, "SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_inventory_transactions'").await?,
    );
    if let Ok(api) = std::env::var("BILIBI_FACTORY_API") {
        over_http(app, &manager, &api, alpha, store_id, floor_id, lot_id).await?;
    }
    Ok(())
}

fn curl(
    api: &str,
    token: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, Error> {
    let mut command = std::process::Command::new("curl");
    command
        .args([
            "-sS",
            "-X",
            method,
            "-H",
            &format!("Authorization: Bearer {token}"),
        ])
        .args([
            "-H",
            "Content-Type: application/json",
            "-w",
            "\n%{http_code}",
        ]);
    if let Some(body) = body {
        command.args(["-d", &body.to_string()]);
    }
    let output = command.arg(format!("{api}{path}")).output()?;
    let text = String::from_utf8(output.stdout)?;
    let (body, status) = text.rsplit_once('\n').ok_or("no status")?;
    let mut value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    value["http_status"] = json!(status);
    Ok(value)
}

/// The same operations through the running app's `/api/v1` edge, with a real
/// OAuth device grant approved by the manager and redeemed for a bearer.
async fn over_http(
    app: &'static bilibi_web::App,
    manager: &Actor,
    api: &str,
    alpha: i64,
    store_id: i64,
    floor_id: i64,
    lot_id: i64,
) -> Result<(), Error> {
    println!("== over HTTP at {api}");
    let access = app
        .services
        .get::<AgentAccess>("core/agent_access.grants")?;
    let start = access.start_device(CLIENT_ID, "H5 walkthrough").await?;
    access
        .decide(
            &manager.scope,
            &start.user_code,
            Decision::Approve,
            access.connection_expiry().await?,
        )
        .await?;
    // Time travel on the disposable row: the client may poll now.
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
    let found = curl(api, &token, "GET", "/api/v1/search?q=transfer", None)?;
    println!("search: status {}", found["http_status"]);
    verify(
        "search over HTTP finds the transfer write",
        found["http_status"] == "200"
            && found
                .to_string()
                .contains("factory.inventory.transfer.record"),
    )?;
    let read = |location: i64| {
        curl(
            api,
            &token,
            "POST",
            "/api/v1/operations/factory.inventory.stock.position/call",
            Some(&json!({"input":{"company_id":1,"item_id":alpha,"location_id":location}})),
        )
    };
    let before = read(floor_id)?;
    println!("floor before: {}", before["result"]);
    let direct = curl(
        api,
        &token,
        "POST",
        "/api/v1/operations/factory.inventory.transfer.record/call",
        Some(&json!({"input":{"company_id":1}})),
    )?;
    println!("direct write: status {}", direct["http_status"]);
    verify(
        "a direct write over HTTP is refused",
        direct["http_status"] != "200",
    )?;
    let ops = json!([{"key":"factory.inventory.transfer.record","input":{
        "company_id":1,"request_id":"h5-http-transfer","evidence":"moved over HTTP",
        "lines":[{"item_id":alpha,"from_location_id":store_id,"to_location_id":floor_id,"quantity":"5","observation":"counted","identity_id":lot_id}]}}]);
    let review = curl(
        api,
        &token,
        "POST",
        "/api/v1/plans",
        Some(&json!({"operations":ops})),
    )?;
    println!(
        "plan review: status {} valid {}",
        review["http_status"], review["valid"]
    );
    verify(
        "the plan review is valid and issues a code",
        review["valid"] == true && review["plan_code"].is_string(),
    )?;
    let body = json!({"plan_code":review["plan_code"],"operations":ops});
    let done = curl(api, &token, "POST", "/api/v1/plans/commit", Some(&body))?;
    println!("commit: status {}", done["http_status"]);
    let after = read(floor_id)?;
    println!("floor after: {}", after["result"]);
    verify(
        "the committed transfer moved 5 kg to the floor",
        done["http_status"] == "200" && after["result"]["quantity"] == "35.000000000000",
    )?;
    let replay = curl(api, &token, "POST", "/api/v1/plans/commit", Some(&body))?;
    println!("replay: status {}", replay["http_status"]);
    verify("the used code is refused", replay["http_status"] != "200")?;
    Ok(())
}
