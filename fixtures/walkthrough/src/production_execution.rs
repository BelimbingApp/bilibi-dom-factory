//! XP1 live-facade walkthrough: production orders, live and imported
//! operation completion posted atomically through the Inventory posting
//! authority, material hold rules and overrides. Run once on a freshly
//! migrated disposable database; it checks each runtime outcome and exits
//! nonzero on an unexpected one. No unit-test harness, product seeds, or
//! production entry point.
use bilibi_audit::RequestInfo;
use bilibi_authz::{
    grants::Grants, CapabilityDefinition, RoleDefinition, VocabularyContribution, VOCABULARY,
};
use bilibi_factory_inventory::{
    Decimal, IdentityKind, Inventory, MaterialTyping, NewIdentity, NewItem, NewLocation, NewUnit,
    PostingRequest, StockLine, UnitVariance, Variance,
};
use bilibi_factory_product_definition::{
    NewProduct, NewResource, NewResourceType, ProductDefinition,
};
use bilibi_factory_production_execution::{
    Approver, Completion, HoldOverrideRequest, NewOrder, OrderKind, ProductionExecution, Source,
};
use bilibi_tenancy::Scope;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fmt::Debug,
    time::{Duration, SystemTime},
};

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
bilibi_walkthrough_fixture::inventory::submit! { bilibi_walkthrough_fixture::registry::Contribution::new(VOCABULARY,"factory.xp1.roles",bilibi_walkthrough_fixture::registry::Provenance {module:"base/authz",source:file!(),line:line!()}, &VocabularyContribution {
    domains:&[],verbs:&[],capabilities:&[] as &[CapabilityDefinition],system_principals:&[],
    roles:&[
        RoleDefinition {code:"manager",name:"manager",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.production-execution.floor.view","factory.production-execution.configuration.view","factory.production-execution.configuration.manage","factory.product-definition.configuration.view","factory.product-definition.configuration.manage","factory.inventory.configuration.view","factory.inventory.configuration.manage","commerce.inventory.item.create","commerce.inventory.item.view"]},
        RoleDefinition {code:"viewer",name:"viewer",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.production-execution.floor.view"]},
    ],
}) }

type Error = Box<dyn std::error::Error>;
fn refusal<T: Debug, E: std::error::Error + 'static>(
    label: &str,
    value: Result<T, E>,
    expected: &str,
) -> Result<(), Error> {
    match value {
        Err(error) => {
            let reason = (&error as &dyn std::error::Error)
                .downcast_ref::<tokio_postgres::Error>()
                .and_then(|error| error.as_db_error())
                .map_or_else(|| error.to_string(), |db| db.message().to_owned());
            if reason.contains(expected) {
                println!("{label}: refused ({reason})");
                return Ok(());
            }
            Err(format!("{label}: expected {expected}, got {reason}").into())
        }
        Ok(value) => Err(format!("{label}: expected {expected}, got {value:?}").into()),
    }
}
fn verify(label: &str, correct: bool) -> Result<(), Error> {
    if !correct {
        return Err(format!("{label}: unexpected runtime result").into());
    }
    println!("{label}: verified");
    Ok(())
}
async fn login(app: &bilibi_web::App, email: &str) -> Result<(Scope, String), Error> {
    let login = app
        .user
        .login(email, bilibi_walkthrough_fixture::PASSWORD)
        .await?;
    Ok((app.security.guard(&login.token).await?, login.token))
}
fn decimal(value: &str) -> Decimal {
    value.parse().expect("decimal")
}
fn line(item: i64, location: i64, quantity: &str) -> StockLine {
    StockLine {
        item_id: item,
        location_id: Some(location),
        quantity: decimal(quantity),
        ..Default::default()
    }
}
fn lot(code: &str) -> Option<NewIdentity> {
    Some(NewIdentity {
        kind: IdentityKind::Lot,
        code: code.into(),
        dimensions: BTreeMap::new(),
    })
}
fn ago(hours: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(hours * 3600)
}
fn identity(transaction: &bilibi_factory_inventory::MaterialTransaction, index: usize) -> i64 {
    transaction
        .entries
        .iter()
        .filter(|entry| entry.role == "stock")
        .nth(index)
        .and_then(|entry| entry.identity_id)
        .expect("identified entry")
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
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let pd = app
        .services
        .get::<ProductDefinition>(bilibi_factory_product_definition::SERVICE)?;
    let pe = app
        .services
        .get::<ProductionExecution>(bilibi_factory_production_execution::SERVICE)?;
    let (manager, _) = login(&app, "manager@example.test").await?;
    let (viewer, _) = login(&app, "viewer@example.test").await?;
    let (staff, _) = login(&app, "staff@example.test").await?;
    let (admin, admin_token) = login(&app, "admin@example.test").await?;
    let (other, _) = login(&app, "manager@other.example.test").await?;
    let client = db.get().await?;
    let count = |table: &'static str| {
        let client = &client;
        async move {
            client
                .query_one(&format!("SELECT count(*) FROM {table}"), &[])
                .await
                .map(|row| row.get::<_, i64>(0))
        }
    };
    if count("factory_production_orders").await? != 0 {
        return Err("use a fresh disposable database".into());
    }

    println!("== catalog and definitions");
    let kg = inventory
        .create_unit(
            &manager,
            1,
            &NewUnit {
                code: "kg".into(),
                name: "Kilogram".into(),
            },
        )
        .await?;
    let mut locations = Vec::new();
    for code in ["STORE", "FLOOR"] {
        locations.push(
            inventory
                .create_location(
                    &manager,
                    1,
                    &NewLocation {
                        code: code.into(),
                        name: code.into(),
                    },
                )
                .await?
                .id,
        );
    }
    let (store, floor) = (locations[0], locations[1]);
    let mut items = BTreeMap::new();
    for sku in ["XP-HELD", "XP-FREE", "XP-PRODUCT", "XP-TRIM", "XP-RULE"] {
        let material = inventory
            .create_material(
                &manager,
                1,
                &NewItem {
                    sku: sku.into(),
                    title: sku.into(),
                    status: Some("active".into()),
                    currency_code: Some("MYR".into()),
                    ..Default::default()
                },
                kg.id,
                &MaterialTyping::default(),
            )
            .await?;
        items.insert(sku, material.item_id);
    }
    let (held, free, product_item, trim, rule) = (
        items["XP-HELD"],
        items["XP-FREE"],
        items["XP-PRODUCT"],
        items["XP-TRIM"],
        items["XP-RULE"],
    );
    let kind = pd
        .create_resource_type(
            &manager,
            1,
            &NewResourceType {
                code: "line".into(),
                name: "Line".into(),
                property_definitions: Vec::new(),
            },
        )
        .await?;
    let mut resources = Vec::new();
    for code in ["R1", "R2"] {
        resources.push(
            pd.create_resource(
                &manager,
                1,
                &NewResource {
                    code: code.into(),
                    name: code.into(),
                    resource_type_id: kind.id,
                    properties: Default::default(),
                },
            )
            .await?
            .id,
        );
    }
    let (r1, r2) = (resources[0], resources[1]);
    let product = pd
        .create_product(
            &manager,
            1,
            &NewProduct {
                item_id: product_item,
                code: "P-1".into(),
                name: "Product one".into(),
            },
        )
        .await?;
    pd.publish_formula(
        &manager,
        1,
        product.id,
        &json!({"lines":[
            {"item_id":held,"unit_id":kg.id,"role":"input","quantity":"10","material_hold_rule":{"hours":2}},
            {"item_id":free,"unit_id":kg.id,"role":"input","quantity":"5"},
            {"item_id":product_item,"unit_id":kg.id,"role":"output","quantity":"14"},
            {"item_id":trim,"unit_id":kg.id,"role":"output","quantity":"1"}
        ]}),
    )
    .await?;
    pd.publish_routing(
        &manager,
        1,
        product.id,
        &json!({"operations":[
            {"code":"blend","sequence":1,"inputs":[held,free],"outputs":[product_item,trim],"allowed_resource_ids":[r1]},
            {"code":"make","sequence":2,"inputs":[],"outputs":[product_item],"allowed_resource_ids":[r1,r2]},
            {"code":"fails","sequence":3,"inputs":[free],"outputs":[trim],"allowed_resource_ids":[r1]}
        ]}),
    )
    .await?;
    // A second product whose input carries a hold rule without hours, and a
    // third whose routing-level rule only names the item.
    let rule_product_item = items["XP-TRIM"];
    let _ = rule_product_item;
    let bad = pd
        .create_product(
            &manager,
            1,
            &NewProduct {
                item_id: rule,
                code: "P-RULE".into(),
                name: "Rule product".into(),
            },
        )
        .await?;
    pd.publish_formula(
        &manager,
        1,
        bad.id,
        &json!({"lines":[
            {"item_id":free,"unit_id":kg.id,"role":"input","quantity":"1"},
            {"item_id":rule,"unit_id":kg.id,"role":"output","quantity":"1"}
        ],"process_config":{"material_hold_rules":{free.to_string():{"hours":0}}}}),
    )
    .await?;
    pd.publish_routing(
        &manager,
        1,
        bad.id,
        &json!({"operations":[{"code":"go","sequence":1,"inputs":[free],"outputs":[rule],"allowed_resource_ids":[r1]}]}),
    )
    .await?;

    println!("== stock to produce from");
    let receive = |id: &str, item, code: &str, at| {
        let request = PostingRequest {
            request_id: id.into(),
            evidence: "walkthrough receipt".into(),
            effective_at: Some(at),
            lines: vec![StockLine {
                identity: lot(code),
                ..line(item, store, "100")
            }],
            ..Default::default()
        };
        let (inventory, manager) = (inventory.clone(), manager.clone());
        async move { inventory.record_receipt(&manager, 1, &request).await }
    };
    let fresh = identity(
        &receive("rc-held-fresh", held, "HELD-FRESH", ago(1)).await?,
        0,
    );
    let aged = identity(
        &receive("rc-held-aged", held, "HELD-AGED", ago(3)).await?,
        0,
    );
    let aged_2 = identity(
        &receive("rc-held-aged-2", held, "HELD-AGED-2", ago(3)).await?,
        0,
    );
    let aged_3 = identity(
        &receive("rc-held-aged-3", held, "HELD-AGED-3", ago(3)).await?,
        0,
    );
    let free_lot = identity(&receive("rc-free", free, "FREE-1", ago(1)).await?, 0);

    println!("== orders");
    let new_order = NewOrder {
        code: "ORD-1".into(),
        kind: OrderKind::Order,
        product_id: product.id,
        formula_version: 1,
        routing_version: 1,
        demand_ref: Some("demand-7".into()),
    };
    let order = pe.create_order(&manager, 1, &new_order).await?;
    verify(
        "order keeps its exact revisions and demand reference",
        order.code == "ORD-1"
            && order.kind == OrderKind::Order
            && (order.formula_version, order.routing_version) == (1, 1)
            && order.demand_ref.as_deref() == Some("demand-7")
            && pe.get_order(&viewer, 1, order.id).await? == order,
    )?;
    let batch = pe
        .create_order(
            &manager,
            1,
            &NewOrder {
                code: "BATCH-1".into(),
                kind: OrderKind::Batch,
                demand_ref: None,
                ..new_order.clone()
            },
        )
        .await?;
    verify(
        "orders list newest first",
        pe.list_orders(&viewer, 1, None)
            .await?
            .iter()
            .map(|o| o.id)
            .collect::<Vec<_>>()
            == [batch.id, order.id],
    )?;
    refusal(
        "duplicate order code",
        pe.create_order(&manager, 1, &new_order).await,
        "Code has already been taken",
    )?;
    refusal(
        "blank order code",
        pe.create_order(
            &manager,
            1,
            &NewOrder {
                code: " ".into(),
                ..new_order.clone()
            },
        )
        .await,
        "Code can't be blank",
    )?;
    refusal(
        "unpublished revision",
        pe.create_order(
            &manager,
            1,
            &NewOrder {
                code: "ORD-X".into(),
                formula_version: 9,
                ..new_order.clone()
            },
        )
        .await,
        "Formula revision not found",
    )?;
    refusal(
        "order creation without the manage capability",
        pe.create_order(
            &viewer,
            1,
            &NewOrder {
                code: "ORD-V".into(),
                ..new_order.clone()
            },
        )
        .await,
        "Access denied",
    )?;
    refusal(
        "reading without the view capability",
        pe.get_order(&staff, 1, order.id).await,
        "Access denied",
    )?;
    refusal(
        "another tenant's company",
        pe.get_order(&other, 1, order.id).await,
        "Not found",
    )?;
    refusal(
        "an order that is not there",
        pe.get_order(&manager, 1, 9999).await,
        "Order not found",
    )?;

    println!("== live completion");
    let (started, completed) = (ago(1), SystemTime::now() - Duration::from_secs(60));
    let blend = |request_id: &str, held_lot: i64| Completion {
        request_id: request_id.into(),
        operation_code: "blend".into(),
        resource_id: r1,
        operator_type: "user".into(),
        operator_id: 1,
        started_at: started,
        completed_at: completed,
        evidence: "work order 1".into(),
        inputs: vec![
            StockLine {
                identity_id: Some(held_lot),
                ..line(held, store, "10")
            },
            StockLine {
                identity_id: Some(free_lot),
                ..line(free, store, "5")
            },
        ],
        outputs: vec![
            StockLine {
                identity: lot(&format!("P-{request_id}")),
                ..line(product_item, floor, "14")
            },
            StockLine {
                identity: lot(&format!("T-{request_id}")),
                ..line(trim, floor, "1")
            },
        ],
        variance: None,
        hold_override: None,
    };
    let first = pe
        .complete_operation(&manager, 1, order.id, Source::Live, &blend("run-1", aged))
        .await?;
    let posted = inventory
        .get_transaction(&manager, 1, first.inventory_transaction_id)
        .await?;
    verify(
        "a transform posted by Production Execution with the run's context",
        posted.kind == "transform"
            && posted.posting_authority.as_deref() == Some("factory/production_execution")
            && posted.request_id == "pe:run-1"
            && posted
                .context
                .get("operation_execution")
                .map(String::as_str)
                == Some("run-1")
            && posted.context.get("order_or_batch").map(String::as_str) == Some("ORD-1")
            && posted.context.get("work_centre") == Some(&r1.to_string()),
    )?;
    verify(
        "the execution keeps the order's revisions and its inventory transaction",
        first.order_id == order.id
            && first.source == Source::Live
            && (first.formula_version, first.routing_version) == (1, 1)
            && first.inputs.len() == 2
            && first.outputs.len() == 2
            && pe.get_execution(&viewer, 1, first.id).await?.id == first.id,
    )?;
    verify(
        "the stock moved: 10 of the held material drawn, product at the floor",
        inventory
            .get_stock_position(&manager, 1, held, store)
            .await?
            .quantity
            .starts_with("390")
            && inventory
                .get_stock_position(&manager, 1, product_item, floor)
                .await?
                .quantity
                .starts_with("14"),
    )?;
    let retried = pe
        .complete_operation(&manager, 1, order.id, Source::Live, &blend("run-1", aged))
        .await?;
    verify(
        "an identical retry returns the original and posts nothing more",
        retried.id == first.id
            && count("factory_operation_executions").await? == 1
            && inventory
                .list_transactions(&manager, 1, Some(product_item), None)
                .await?
                .len()
                == 1,
    )?;
    refusal(
        "the same request id with different content",
        pe.complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                evidence: "changed".into(),
                ..blend("run-1", aged)
            },
        )
        .await,
        "Request id conflict",
    )?;
    let race = blend("run-race", aged_2);
    let (a, b) = tokio::join!(
        pe.complete_operation(&manager, 1, order.id, Source::Live, &race),
        pe.complete_operation(&manager, 1, order.id, Source::Live, &race),
    );
    let (a, b) = (a?, b?);
    verify(
        "concurrent identical retries record one execution",
        a.id == b.id && count("factory_operation_executions").await? == 2,
    )?;
    let listed = pe.list_executions(&viewer, 1, order.id, None).await?;
    verify(
        "executions list most recently completed first",
        listed.len() == 2 && listed[0].completed_at >= listed[1].completed_at,
    )?;
    let made = pe
        .complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                request_id: "run-make".into(),
                operation_code: "make".into(),
                resource_id: r2,
                inputs: vec![],
                outputs: vec![StockLine {
                    identity: lot("P-make"),
                    ..line(product_item, floor, "3")
                }],
                ..blend("run-make", aged)
            },
        )
        .await?;
    verify(
        "an outputs-only completion posts an output",
        inventory
            .get_transaction(&manager, 1, made.inventory_transaction_id)
            .await?
            .kind
            == "output",
    )?;
    let drawn = pe
        .complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                request_id: "run-draw".into(),
                outputs: vec![],
                inputs: vec![StockLine {
                    identity_id: Some(free_lot),
                    ..line(free, store, "1")
                }],
                operation_code: "fails".into(),
                ..blend("run-draw", aged)
            },
        )
        .await?;
    verify(
        "an inputs-only completion posts a production consumption",
        inventory
            .get_transaction(&manager, 1, drawn.inventory_transaction_id)
            .await?
            .kind
            == "consumption",
    )?;
    let mixed = pe
        .complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                request_id: "run-variance".into(),
                inputs: vec![
                    StockLine {
                        identity_id: Some(aged_3),
                        ..line(held, store, "10")
                    },
                    StockLine {
                        identity_id: Some(free_lot),
                        ..line(free, store, "5")
                    },
                ],
                outputs: vec![
                    StockLine {
                        identity: lot("P-var"),
                        ..line(product_item, floor, "13")
                    },
                    StockLine {
                        identity: lot("T-var"),
                        ..line(trim, floor, "1")
                    },
                ],
                variance: Some(Variance::Units(vec![UnitVariance {
                    unit_id: kg.id,
                    evidence: "scale drift".into(),
                    reconciliation_basis: "weighed".into(),
                }])),
                ..blend("run-variance", aged)
            },
        )
        .await?;
    let balance = inventory
        .get_transaction_balance(&manager, 1, mixed.inventory_transaction_id)
        .await?;
    verify(
        "a short run records its variance through Inventory",
        balance
            .per_unit
            .iter()
            .any(|unit| unit.variance.starts_with('-') || unit.variance.starts_with('1')),
    )?;
    refusal(
        "a short run without variance evidence",
        pe.complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                request_id: "run-novariance".into(),
                outputs: vec![StockLine {
                    identity: lot("P-nv"),
                    ..line(product_item, floor, "13")
                }],
                ..blend("run-novariance", aged)
            },
        )
        .await,
        "Variance required",
    )?;

    println!("== validation and atomicity");
    for (label, completion, reason) in [
        (
            "an operation the routing lacks",
            Completion {
                operation_code: "nope".into(),
                ..blend("v-1", aged)
            },
            "Invalid execution",
        ),
        (
            "a resource the operation does not allow",
            Completion {
                resource_id: r2,
                ..blend("v-2", aged)
            },
            "Invalid execution",
        ),
        (
            "completion in the future",
            Completion {
                completed_at: SystemTime::now() + Duration::from_secs(3600),
                ..blend("v-3", aged)
            },
            "Invalid execution",
        ),
        (
            "start after completion",
            Completion {
                started_at: SystemTime::now(),
                completed_at: ago(1),
                ..blend("v-4", aged)
            },
            "Invalid execution",
        ),
        (
            "variance without both sides",
            Completion {
                inputs: vec![],
                variance: Some(Variance::Shared {
                    evidence: "e".into(),
                    reconciliation_basis: "b".into(),
                }),
                ..blend("v-5", aged)
            },
            "Invalid execution",
        ),
        (
            "nothing consumed or produced",
            Completion {
                inputs: vec![],
                outputs: vec![],
                ..blend("v-6", aged)
            },
            "Invalid execution",
        ),
        (
            "an item the operation does not take",
            Completion {
                inputs: vec![StockLine {
                    identity_id: Some(free_lot),
                    ..line(product_item, store, "1")
                }],
                ..blend("v-7", aged)
            },
            "Invalid lines",
        ),
        (
            "a non-positive quantity",
            Completion {
                outputs: vec![StockLine {
                    identity: lot("P-zero"),
                    ..line(product_item, floor, "0")
                }],
                ..blend("v-8", aged)
            },
            "Invalid lines",
        ),
        (
            "a blank request id",
            Completion {
                request_id: String::new(),
                ..blend("v-9", aged)
            },
            "Invalid execution",
        ),
    ] {
        refusal(
            label,
            pe.complete_operation(&manager, 1, order.id, Source::Live, &completion)
                .await,
            reason,
        )?;
    }
    let before = (
        count("factory_operation_executions").await?,
        count("factory_inventory_transactions").await?,
    );
    refusal(
        "Inventory refuses the posting (more than the lot holds)",
        pe.complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                inputs: vec![
                    StockLine {
                        identity_id: Some(aged),
                        ..line(held, store, "1000")
                    },
                    StockLine {
                        identity_id: Some(free_lot),
                        ..line(free, store, "5")
                    },
                ],
                variance: Some(Variance::Shared {
                    evidence: "walkthrough".into(),
                    reconciliation_basis: "counted".into(),
                }),
                ..blend("fail-stock", aged)
            },
        )
        .await,
        "Insufficient",
    )?;
    verify(
        "nothing was kept, and the request id is reusable",
        (
            count("factory_operation_executions").await?,
            count("factory_inventory_transactions").await?,
        ) == before,
    )?;
    client
        .batch_execute("ALTER TABLE factory_operation_executions ADD CONSTRAINT fixture_refuse CHECK(operation_code <> 'fails' OR operator_id <> 99)")
        .await?;
    refusal(
        "the execution record fails after Inventory posted",
        pe.complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                request_id: "fail-record".into(),
                operation_code: "fails".into(),
                operator_id: 99,
                inputs: vec![StockLine {
                    identity_id: Some(free_lot),
                    ..line(free, store, "1")
                }],
                outputs: vec![StockLine {
                    identity: lot("T-fail"),
                    ..line(trim, floor, "1")
                }],
                ..blend("fail-record", aged)
            },
        )
        .await,
        "Database unavailable",
    )?;
    client
        .batch_execute("ALTER TABLE factory_operation_executions DROP CONSTRAINT fixture_refuse")
        .await?;
    verify(
        "the posting rolled back with it: no execution, no transaction, no lot",
        (
            count("factory_operation_executions").await?,
            count("factory_inventory_transactions").await?,
        ) == before
            && client
                .query_one(
                    "SELECT count(*) FROM factory_inventory_identities WHERE code='T-fail'",
                    &[],
                )
                .await?
                .get::<_, i64>(0)
                == 0,
    )?;
    refusal(
        "an invalid hold rule on the input",
        pe.complete_operation(
            &manager,
            1,
            pe.create_order(
                &manager,
                1,
                &NewOrder {
                    code: "ORD-RULE".into(),
                    product_id: bad.id,
                    ..new_order.clone()
                },
            )
            .await?
            .id,
            Source::Live,
            &Completion {
                request_id: "rule-1".into(),
                operation_code: "go".into(),
                inputs: vec![StockLine {
                    identity_id: Some(free_lot),
                    ..line(free, store, "1")
                }],
                outputs: vec![StockLine {
                    identity: lot("RULE-OUT"),
                    ..line(rule, floor, "1")
                }],
                ..blend("rule-1", aged)
            },
        )
        .await,
        "Invalid material hold rule",
    )?;
    refusal(
        "completion without the manage capability",
        pe.complete_operation(
            &viewer,
            1,
            order.id,
            Source::Live,
            &blend("viewer-run", aged),
        )
        .await,
        "Access denied",
    )?;

    println!("== material holds");
    refusal(
        "a held input without an identity",
        pe.complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &Completion {
                inputs: vec![
                    line(held, store, "10"),
                    StockLine {
                        identity_id: Some(free_lot),
                        ..line(free, store, "5")
                    },
                ],
                ..blend("hold-1", aged)
            },
        )
        .await,
        "Invalid hold source",
    )?;
    refusal(
        "a held input still inside its hold",
        pe.complete_operation(&manager, 1, order.id, Source::Live, &blend("hold-2", fresh))
            .await,
        "Material held",
    )?;
    let override_request = |reason: &str| {
        Some(HoldOverrideRequest {
            reason: reason.into(),
            ..Default::default()
        })
    };
    let with_override = |request_id: &str| Completion {
        hold_override: override_request("urgent customer order"),
        ..blend(request_id, fresh)
    };
    refusal(
        "an override without the override capability",
        pe.complete_operation(
            &manager,
            1,
            order.id,
            Source::Live,
            &with_override("hold-3"),
        )
        .await,
        "Hold override denied",
    )?;
    refusal(
        "a live override carrying import evidence",
        pe.complete_operation(
            &admin,
            1,
            order.id,
            Source::Live,
            &Completion {
                hold_override: Some(HoldOverrideRequest {
                    reason: "r".into(),
                    evidence: Some("e".into()),
                    ..Default::default()
                }),
                ..blend("hold-4", fresh)
            },
        )
        .await,
        "Invalid hold override",
    )?;
    refusal(
        "a blank override reason",
        pe.complete_operation(
            &admin,
            1,
            order.id,
            Source::Live,
            &Completion {
                hold_override: override_request("  "),
                ..blend("hold-5", fresh)
            },
        )
        .await,
        "Invalid hold override",
    )?;
    let impersonated_token = {
        let csrf = admin.csrf.clone();
        let staff_id = 1;
        app.user
            .impersonate(&admin_token, &csrf, staff_id, &RequestInfo::default())
            .await
            .map_err(|refusal| format!("impersonation: {refusal:?}"))?
    };
    let _ = impersonated_token;
    let impersonated = app.security.guard(&admin_token).await?;
    verify(
        "the session is impersonated",
        impersonated.impersonator().is_some(),
    )?;
    refusal(
        "an override under impersonation",
        pe.complete_operation(
            &impersonated,
            1,
            order.id,
            Source::Live,
            &with_override("hold-6"),
        )
        .await,
        "Override refused under impersonation",
    )?;
    let early = pe
        .complete_operation(&admin, 1, order.id, Source::Live, &with_override("hold-7"))
        .await?;
    let overrides = pe.list_hold_overrides(&viewer, 1, early.id).await?;
    verify(
        "a live override records the user as approver and recorder",
        overrides.len() == 1
            && overrides[0].identity_id == fresh
            && overrides[0].item_id == held
            && overrides[0].source == Source::Live
            && overrides[0].actor_type.as_deref() == Some("user")
            && overrides[0].actor_id == Some(admin.user())
            && overrides[0].recorded_by_id == admin.user()
            && overrides[0].reason == "urgent customer order"
            && overrides[0].evidence.is_none()
            && overrides[0].inventory_transaction_id == early.inventory_transaction_id,
    )?;
    verify(
        "an execution with no override lists none",
        pe.list_hold_overrides(&viewer, 1, first.id)
            .await?
            .is_empty(),
    )?;
    refusal(
        "an immutable override",
        client
            .batch_execute("UPDATE factory_material_hold_overrides SET reason='changed'")
            .await,
        "factory_material_hold_overrides are immutable",
    )?;
    refusal(
        "an immutable execution",
        client
            .batch_execute("DELETE FROM factory_operation_executions")
            .await,
        "factory_operation_executions are immutable",
    )?;

    println!("== historical import");
    let historical = |request_id: &str, held_lot: i64| Completion {
        request_id: request_id.into(),
        started_at: ago(72),
        completed_at: ago(48),
        inputs: vec![
            StockLine {
                identity_id: Some(held_lot),
                ..line(held, store, "10")
            },
            StockLine {
                identity_id: Some(free_lot),
                ..line(free, store, "5")
            },
        ],
        outputs: vec![
            StockLine {
                identity: lot(&format!("P-{request_id}")),
                ..line(product_item, floor, "14")
            },
            StockLine {
                identity: lot(&format!("T-{request_id}")),
                ..line(trim, floor, "1")
            },
        ],
        ..blend(request_id, held_lot)
    };
    // The aged lots were received three hours ago: older than the hold at
    // that time of completion would be 48 hours ago, which predates the
    // receipt. The source must not be in the future of the completion.
    refusal(
        "an import without the import capability",
        pe.complete_operation(
            &manager,
            1,
            order.id,
            Source::Import,
            &historical("imp-1", aged_2),
        )
        .await,
        "Import not authorized",
    )?;
    refusal(
        "an import whose source postdates its completion",
        pe.complete_operation(
            &admin,
            1,
            order.id,
            Source::Import,
            &historical("imp-2", aged_2),
        )
        .await,
        "Invalid hold source",
    )?;
    let (imp_started, imp_completed, imp_occurred) =
        (ago(1), SystemTime::now() - Duration::from_secs(60), ago(1));
    let import_override = || HoldOverrideRequest {
        reason: "migrated from the paper log".into(),
        evidence: Some("log page 12".into()),
        occurred_at: Some(imp_occurred),
        approver: Some(Approver {
            kind: "user".into(),
            id: 42,
            acting_for_user_id: None,
        }),
    };
    let import_3 = Completion {
        started_at: imp_started,
        completed_at: imp_completed,
        hold_override: Some(import_override()),
        ..historical("imp-3", fresh)
    };
    let imported = pe
        .complete_operation(&admin, 1, order.id, Source::Import, &import_3)
        .await?;
    let imported_overrides = pe.list_hold_overrides(&viewer, 1, imported.id).await?;
    verify(
        "an import keeps its source approver apart from the recorder",
        imported.source == Source::Import
            && imported_overrides.len() == 1
            && imported_overrides[0].source == Source::Import
            && imported_overrides[0].actor_id == Some(42)
            && imported_overrides[0].recorded_by_id == admin.user()
            && imported_overrides[0].evidence.as_deref() == Some("log page 12"),
    )?;
    refusal(
        "an import override that postdates its completion",
        pe.complete_operation(
            &admin,
            1,
            order.id,
            Source::Import,
            &Completion {
                hold_override: Some(HoldOverrideRequest {
                    reason: "r".into(),
                    evidence: Some("e".into()),
                    occurred_at: Some(SystemTime::now() + Duration::from_secs(10)),
                    approver: None,
                }),
                completed_at: SystemTime::now() - Duration::from_secs(60),
                started_at: ago(1),
                ..historical("imp-4", fresh)
            },
        )
        .await,
        "Invalid hold override",
    )?;
    let replayed = pe
        .complete_operation(&admin, 1, order.id, Source::Import, &import_3)
        .await?;
    verify(
        "an identical import retry returns the original",
        replayed.id == imported.id
            && pe.list_hold_overrides(&viewer, 1, imported.id).await?.len() == 1,
    )?;
    refusal(
        "an import retry once the import grant is gone",
        pe.complete_operation(&manager, 1, order.id, Source::Import, &import_3)
            .await,
        "Import not authorized",
    )?;

    println!("== audit and isolation");
    let audited = client
        .query_one(
            "SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_operation_executions'",
            &[],
        )
        .await?
        .get::<_, i64>(0);
    verify(
        "every saved execution is audited",
        audited == count("factory_operation_executions").await?,
    )?;
    refusal(
        "another tenant cannot read an execution",
        pe.get_execution(&other, 1, first.id).await,
        "Not found",
    )?;
    println!("production execution walkthrough complete");
    Ok(())
}
