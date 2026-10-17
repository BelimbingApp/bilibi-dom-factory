//! XI2's running facade walkthrough. Only runs on the disposable fixture DB.
use super::{refused, show, Error};
use bilibi_database::Db;
use bilibi_factory_inventory::{
    Decimal, Inventory, MaterialTyping, NewItem, NewLocation, NewUnit, Observation, PostingRequest,
    ReceiptMeasurement, StockLine,
};
use bilibi_tenancy::Scope;
use std::time::{Duration, SystemTime};

fn line(item: i64, location: i64, quantity: i64) -> StockLine {
    StockLine {
        item_id: item,
        location_id: Some(location),
        from_location_id: None,
        to_location_id: None,
        quantity: Decimal::from(quantity),
        unit_id: None,
        conversion_version: None,
        observation: Observation::Measured,
        evidence: None,
    }
}
fn request(id: &str, lines: Vec<StockLine>) -> PostingRequest {
    PostingRequest {
        request_id: id.into(),
        evidence: "walkthrough source evidence".into(),
        lines,
        ..Default::default()
    }
}

pub(super) async fn run(
    inventory: &Inventory,
    db: &Db,
    manager: &Scope,
    viewer: &Scope,
    staff: &Scope,
    other: &Scope,
) -> Result<(), Error> {
    println!("== XI2 Material Transaction ledger");
    let unit = inventory
        .create_unit(
            manager,
            1,
            &NewUnit {
                code: "native".into(),
                name: "Native unit".into(),
            },
        )
        .await?;
    let alternate = inventory
        .create_unit(
            manager,
            1,
            &NewUnit {
                code: "recorded".into(),
                name: "Recorded unit".into(),
            },
        )
        .await?;
    let a = inventory
        .create_location(
            manager,
            1,
            &NewLocation {
                code: "LEDGER-A".into(),
                name: "Ledger A".into(),
            },
        )
        .await?
        .id;
    let b = inventory
        .create_location(
            manager,
            1,
            &NewLocation {
                code: "LEDGER-B".into(),
                name: "Ledger B".into(),
            },
        )
        .await?
        .id;
    let item = inventory
        .create_material(
            manager,
            1,
            &NewItem {
                sku: "LEDGER-1".into(),
                title: "Ledger material".into(),
                currency_code: Some("MYR".into()),
                ..Default::default()
            },
            unit.id,
            &MaterialTyping::default(),
        )
        .await?
        .item_id;
    let second = inventory
        .create_material(
            manager,
            1,
            &NewItem {
                sku: "LEDGER-2".into(),
                title: "Second ledger material".into(),
                currency_code: Some("MYR".into()),
                ..Default::default()
            },
            alternate.id,
            &MaterialTyping::default(),
        )
        .await?
        .item_id;
    show(
        "empty position",
        inventory.get_stock_position(viewer, 1, item, a).await?,
    );
    inventory
        .define_conversion(manager, 1, item, alternate.id, Decimal::new(25, 1))
        .await?;
    let mut receipt = request("weighed", vec![line(item, a, 10)]);
    receipt.lines[0].unit_id = Some(alternate.id);
    receipt.receipt_measurement = Some(ReceiptMeasurement {
        supplier_declared: Decimal::from(11),
        measured_gross: Decimal::from(12),
        tare: Decimal::from(2),
        net: Decimal::from(10),
        unit_id: alternate.id,
        weighing_point_ref: "ticket-1".into(),
    });
    receipt.effective_at = Some(SystemTime::now() - Duration::from_secs(3600));
    let received = inventory.record_receipt(manager, 1, &receipt).await?;
    show("weighed receipt: 25 native, variance 1", &received);
    show(
        "supplier variance",
        received
            .receipt_measurement
            .as_ref()
            .map(|m| m.supplier_variance()),
    );
    let client = db.get().await?;
    let audit_before: i64 = client.query_one("SELECT count(*) FROM base_audit_mutations WHERE auditable_type IN ('factory_inventory_transactions','factory_inventory_transaction_entries')",&[]).await?.get(0);
    inventory
        .define_conversion(manager, 1, item, alternate.id, Decimal::from(3))
        .await?;
    let mut equivalent = receipt.clone();
    equivalent.lines[0].quantity = Decimal::new(100, 1);
    let replay = inventory.record_receipt(manager, 1, &equivalent).await?;
    let audit_after: i64 = client.query_one("SELECT count(*) FROM base_audit_mutations WHERE auditable_type IN ('factory_inventory_transactions','factory_inventory_transaction_entries')",&[]).await?.get(0);
    show(
        "retry keeps original id and conversion; no duplicate audit",
        (
            received.id,
            replay.id,
            replay.entries[0].native_quantity.clone(),
            audit_after - audit_before,
        ),
    );
    let mut conflict = receipt.clone();
    conflict.evidence = "different".into();
    refused(
        "request id conflict",
        inventory.record_receipt(manager, 1, &conflict).await,
    );
    let mut malformed = receipt.clone();
    malformed.request_id = "bad-ticket".into();
    malformed.receipt_measurement.as_mut().unwrap().tare = Decimal::from(3);
    refused(
        "gross minus tare mismatch",
        inventory.record_receipt(manager, 1, &malformed).await,
    );
    malformed.receipt_measurement.as_mut().unwrap().tare = Decimal::from(2);
    malformed.lines[0].observation = Observation::Declared;
    refused(
        "ticket needs measured line",
        inventory.record_receipt(manager, 1, &malformed).await,
    );
    malformed.lines.push(malformed.lines[0].clone());
    refused(
        "ticket needs one line",
        inventory.record_receipt(manager, 1, &malformed).await,
    );
    let mut old = request("old-conversion", vec![line(item, a, 2)]);
    old.lines[0].unit_id = Some(alternate.id);
    old.lines[0].conversion_version = Some(1);
    show(
        "explicit old conversion adds 5",
        inventory.record_receipt(manager, 1, &old).await?,
    );
    old.request_id = "current-conversion".into();
    old.lines[0].conversion_version = None;
    show(
        "current conversion adds 6",
        inventory.record_receipt(manager, 1, &old).await?,
    );
    let mut transfer = request("transfer", vec![line(item, a, 9)]);
    transfer.lines[0].location_id = None;
    transfer.lines[0].from_location_id = Some(a);
    transfer.lines[0].to_location_id = Some(b);
    show(
        "transfer 9",
        inventory.record_transfer(manager, 1, &transfer).await?,
    );
    show(
        "source 27",
        inventory.get_stock_position(viewer, 1, item, a).await?,
    );
    show(
        "destination 9",
        inventory.get_stock_position(viewer, 1, item, b).await?,
    );
    refused(
        "retirement with stock",
        inventory.retire_material(manager, 1, item).await,
    );
    let consume = request("consume", vec![line(item, b, 4)]);
    show(
        "consume 4",
        inventory.record_consumption(manager, 1, &consume).await?,
    );
    let double = request("duplicate-draw", vec![line(item, b, 3), line(item, b, 3)]);
    refused(
        "aggregate withdrawals exceed 5",
        inventory.record_consumption(manager, 1, &double).await,
    );
    let mut correction = request("correct", vec![line(item, b, -2)]);
    correction.reason = Some("measurement corrected".into());
    correction.corrects_transaction_id = Some(received.id);
    show(
        "correction subtracts 2",
        inventory.record_correction(manager, 1, &correction).await?,
    );
    correction.request_id = "self-funded".into();
    correction.lines = vec![line(item, b, 20), line(item, b, -10)];
    refused(
        "own additions cannot fund withdrawals",
        inventory.record_correction(manager, 1, &correction).await,
    );
    let c1 = request("concurrent-1", vec![line(item, b, 2)]);
    let c2 = request("concurrent-2", vec![line(item, b, 2)]);
    let (one, two) = tokio::join!(
        inventory.record_consumption(manager, 1, &c1),
        inventory.record_consumption(manager, 1, &c2)
    );
    show("concurrent draws of 2 from 3 (one success)", (one, two));
    show(
        "after race: 1",
        inventory.get_stock_position(viewer, 1, item, b).await?,
    );
    let retry = request("parallel-retry", vec![line(item, b, 1)]);
    let (one, two) = tokio::join!(
        inventory.record_consumption(manager, 1, &retry),
        inventory.record_consumption(manager, 1, &retry)
    );
    show("parallel retry: same transaction id", (one?.id, two?.id));
    show(
        "after retry: 0",
        inventory.get_stock_position(viewer, 1, item, b).await?,
    );
    let mut disjoint = request("disjoint-id", vec![line(item, a, 1)]);
    let mut disjoint2 = disjoint.clone();
    disjoint2.lines = vec![line(second, a, 1)];
    let (one, two) = tokio::join!(
        inventory.record_receipt(manager, 1, &disjoint),
        inventory.record_receipt(manager, 1, &disjoint2)
    );
    show(
        "same key different materials: one success, one conflict",
        (one, two),
    );
    disjoint.request_id = "production-context".into();
    disjoint
        .context
        .insert("order_or_batch".into(), "order-1".into());
    refused(
        "production context needs authority",
        inventory.record_receipt(manager, 1, &disjoint).await,
    );
    refused(
        "viewer cannot post (even a replay)",
        inventory.record_receipt(viewer, 1, &receipt).await,
    );
    client.execute("INSERT INTO principal_capabilities(tenant_id,user_id,capability,is_allowed) VALUES($1,$2,'factory.inventory.configuration.manage',false)", &[&manager.tenant(),&manager.user()]).await?;
    refused(
        "revoked grant on existing scope refuses replay",
        inventory.record_receipt(manager, 1, &receipt).await,
    );
    client.execute("DELETE FROM principal_capabilities WHERE tenant_id=$1 AND user_id=$2 AND capability='factory.inventory.configuration.manage'", &[&manager.tenant(),&manager.user()]).await?;
    show(
        "restored grant on same scope replays",
        inventory.record_receipt(manager, 1, &receipt).await?.id,
    );
    refused(
        "staff cannot read",
        inventory.get_transaction(staff, 1, received.id).await,
    );
    refused(
        "cross tenant read",
        inventory.get_transaction(other, 1, received.id).await,
    );
    refused(
        "cross company transaction",
        inventory.get_transaction(manager, 2, received.id).await,
    );
    show(
        "filtered newest transaction",
        inventory
            .list_transactions(viewer, 1, Some(item), Some(1))
            .await?,
    );
    refused(
        "invalid list bound",
        inventory
            .list_transactions(viewer, 1, None, Some(501))
            .await,
    );
    let mut future = request("future", vec![line(item, a, 1)]);
    future.effective_at = Some(SystemTime::now() + Duration::from_secs(3600));
    refused(
        "future effective time",
        inventory.record_receipt(manager, 1, &future).await,
    );
    client
        .execute(
            "UPDATE companies SET status='archived' WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    refused(
        "archived company cannot replay",
        inventory.record_receipt(manager, 1, &receipt).await,
    );
    client
        .execute(
            "UPDATE companies SET status='active' WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;

    // Drain exactly the current balance, retire, and prove corrections remain
    // possible while new receipts are refused, as in Bilimbi.
    let balance = inventory
        .get_stock_position(manager, 1, item, a)
        .await?
        .quantity
        .parse::<Decimal>()?;
    let mut drain = request("drain", vec![line(item, a, 1)]);
    drain.lines[0].quantity = balance;
    inventory.record_consumption(manager, 1, &drain).await?;
    show(
        "retire empty material",
        inventory.retire_material(manager, 1, item).await?,
    );
    show(
        "retry after retirement keeps id",
        inventory.record_receipt(manager, 1, &receipt).await?.id,
    );
    refused(
        "new receipt to retired material",
        inventory
            .record_receipt(manager, 1, &request("retired", vec![line(item, a, 1)]))
            .await,
    );
    correction.request_id = "retired-correction".into();
    correction.lines = vec![line(item, a, 1)];
    show(
        "correction of retired material",
        inventory.record_correction(manager, 1, &correction).await?,
    );

    let maximum: Decimal = "999999999999.999999999999".parse()?;
    inventory
        .define_conversion(manager, 1, second, unit.id, maximum)
        .await?;
    let mut precise = request("wide-native", vec![line(second, a, 1)]);
    precise.lines[0].unit_id = Some(unit.id);
    precise.lines[0].quantity = maximum;
    show(
        "36-digit native quantity stays exact",
        inventory
            .record_receipt(manager, 1, &precise)
            .await?
            .entries[0]
            .native_quantity
            .clone(),
    );

    // An audit failure must roll back the header and every entry, not merely
    // return an error after stock has changed. This trigger is fixture-only.
    client.batch_execute("CREATE FUNCTION xi2_refuse_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.auditable_type='factory_inventory_transaction_entries' THEN RAISE EXCEPTION 'XI2 audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER xi2_refuse_audit BEFORE INSERT ON base_audit_mutations FOR EACH ROW EXECUTE FUNCTION xi2_refuse_audit()").await?;
    refused(
        "audit failure rolls back posting",
        inventory
            .record_receipt(
                manager,
                1,
                &request("audit-failure", vec![line(second, b, 1)]),
            )
            .await,
    );
    client.batch_execute("DROP TRIGGER xi2_refuse_audit ON base_audit_mutations; DROP FUNCTION xi2_refuse_audit()").await?;
    show("audit-failure transaction count: 0",client.query_one("SELECT count(*) FROM factory_inventory_transactions WHERE request_id='audit-failure'", &[]).await?.get::<_,i64>(0));

    println!("== ledger database backstops");
    refused("zero overall but unbalanced per unit",client.execute("WITH t AS (INSERT INTO factory_inventory_transactions(tenant_id,company_id,kind,request_id,request_fingerprint,actor_type,actor_id,evidence,effective_at,recorded_at) VALUES(1,1,'transform','per-unit','x','user',1,'proof',now(),now()) RETURNING id) INSERT INTO factory_inventory_transaction_entries(tenant_id,company_id,transaction_id,role,native_quantity,native_unit_id,evidence,reconciliation_basis) SELECT 1,1,t.id,'variance',v.quantity,v.unit,'proof','proof' FROM t CROSS JOIN (VALUES($1::bigint,1),($2::bigint,-1)) AS v(unit,quantity)",&[&unit.id,&alternate.id]).await);
    for sql in [
        "UPDATE factory_inventory_transactions SET evidence='changed'",
        "DELETE FROM factory_inventory_transactions",
        "UPDATE factory_inventory_transaction_entries SET native_quantity=1",
        "DELETE FROM factory_inventory_transaction_entries",
        "TRUNCATE factory_inventory_transaction_entries",
        "TRUNCATE factory_inventory_transactions,factory_inventory_transaction_entries",
        "INSERT INTO factory_inventory_transactions(tenant_id,company_id,kind,request_id,request_fingerprint,actor_type,actor_id,evidence,effective_at,recorded_at) VALUES(1,1,'receipt','empty','x','user',1,'proof',now(),now())",
        "INSERT INTO factory_inventory_transaction_entries(tenant_id,company_id,transaction_id,role,material_id,location_id,native_quantity,native_unit_id,recorded_quantity,recorded_unit_id,observation) SELECT tenant_id,company_id,transaction_id,role,material_id,location_id,1,native_unit_id,1,recorded_unit_id,observation FROM factory_inventory_transaction_entries WHERE role='stock' LIMIT 1",
        "INSERT INTO factory_inventory_transaction_entries(tenant_id,company_id,transaction_id,role,material_id,location_id,native_quantity,native_unit_id,recorded_quantity,recorded_unit_id,observation) SELECT tenant_id,2,transaction_id,role,material_id,location_id,1,native_unit_id,1,recorded_unit_id,observation FROM factory_inventory_transaction_entries WHERE role='stock' LIMIT 1",
    ] { refused(sql,client.execute(sql,&[]).await); }
    let counts=client.query_one("SELECT (SELECT count(*) FROM factory_inventory_transactions),(SELECT count(*) FROM factory_inventory_transaction_entries),(SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_inventory_transactions'),(SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_inventory_transaction_entries')",&[]).await?;
    show(
        "ledger rows and audit rows",
        (
            counts.get::<_, i64>(0),
            counts.get::<_, i64>(1),
            counts.get::<_, i64>(2),
            counts.get::<_, i64>(3),
        ),
    );
    Ok(())
}
