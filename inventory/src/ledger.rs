//! Bilimbi Inventory.Ledger: locked planning, append-only posting and reads.
//! Native arithmetic stays in PostgreSQL: numeric(36,12) exceeds Decimal's
//! 96-bit mantissa. Read models carry exact decimal strings, never floats.
use crate::{
    access,
    ledger_request::{self, invalid, PostingRequest, ReceiptMeasurement, TransactionKind},
    materials, Decimal, Error, Unit,
};
use bilibi_database::{Change, MutationTransaction};
use bilibi_tenancy::Scope;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::SystemTime};
use tokio_postgres::GenericClient;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub role: String,
    pub item_id: Option<i64>,
    pub location_id: Option<i64>,
    /// Exact signed numeric(36,12), represented as decimal text.
    pub native_quantity: String,
    pub native_unit_id: i64,
    pub recorded_quantity: Option<Decimal>,
    pub recorded_unit_id: Option<i64>,
    pub conversion_id: Option<i64>,
    pub conversion_version: Option<i32>,
    pub observation: Option<crate::Observation>,
    pub evidence: Option<String>,
    pub output_role: Option<String>,
    pub reconciliation_basis: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MaterialTransaction {
    pub id: i64,
    pub company_id: i64,
    pub kind: String,
    pub request_id: String,
    pub actor_type: String,
    pub actor_id: i64,
    pub evidence: String,
    pub receipt_measurement: Option<ReceiptMeasurement>,
    pub reason: Option<String>,
    pub corrects_transaction_id: Option<i64>,
    pub posting_authority: Option<String>,
    pub effective_at: SystemTime,
    pub recorded_at: SystemTime,
    pub context: BTreeMap<String, String>,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct StockPosition {
    pub item_id: i64,
    pub location_id: i64,
    /// Exact sum of native quantities (decimal text).
    pub quantity: String,
    pub unit: Unit,
}

pub(crate) async fn get(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<MaterialTransaction, Error> {
    access::readable(client, scope, company).await?;
    read(client, scope.tenant(), company, id).await
}

async fn read(
    client: &impl GenericClient,
    tenant: i64,
    company: i64,
    id: i64,
) -> Result<MaterialTransaction, Error> {
    let row = client.query_opt("SELECT * FROM factory_inventory_transactions WHERE tenant_id=$1 AND company_id=$2 AND id=$3", &[&tenant,&company,&id]).await?.ok_or(Error::TransactionNotFound)?;
    let entries = client.query("SELECT to_jsonb(e) || jsonb_build_object('native_quantity',e.native_quantity::text,'recorded_quantity',e.recorded_quantity::text,'item_id',m.item_id,'conversion_version',c.version) AS entry FROM factory_inventory_transaction_entries e LEFT JOIN factory_inventory_materials m ON m.company_id=e.company_id AND m.id=e.material_id LEFT JOIN factory_inventory_unit_conversions c ON c.company_id=e.company_id AND c.id=e.conversion_id WHERE e.tenant_id=$1 AND e.company_id=$2 AND e.transaction_id=$3 ORDER BY e.id", &[&tenant,&company,&id]).await?.iter().map(|r| serde_json::from_value(r.get("entry")).map_err(|_| bilibi_database::Error::Invalid.into())).collect::<Result<Vec<Entry>,Error>>()?;
    let mut context = BTreeMap::new();
    for (key, column) in [
        ("operation_execution", "operation_execution_ref"),
        ("order_or_batch", "order_or_batch_ref"),
        ("work_centre", "work_centre_ref"),
        ("shipment", "shipment_ref"),
        ("destination", "destination_ref"),
    ] {
        if let Some(value) = row.get::<_, Option<String>>(column) {
            context.insert(key.into(), value);
        }
    }
    let measurement = row
        .get::<_, Option<Value>>("receipt_measurement")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| bilibi_database::Error::Invalid)?;
    Ok(MaterialTransaction {
        id,
        company_id: company,
        kind: row.get("kind"),
        request_id: row.get("request_id"),
        actor_type: row.get("actor_type"),
        actor_id: row.get("actor_id"),
        evidence: row.get("evidence"),
        receipt_measurement: measurement,
        reason: row.get("reason"),
        corrects_transaction_id: row.get("corrects_transaction_id"),
        posting_authority: row.get("posting_authority"),
        effective_at: row.get("effective_at"),
        recorded_at: row.get("recorded_at"),
        context,
        entries,
    })
}

pub(crate) async fn list(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    item: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<MaterialTransaction>, Error> {
    access::readable(client, scope, company).await?;
    let limit = crate::limit(limit)?;
    let rows = client.query("SELECT t.id FROM factory_inventory_transactions t WHERE t.tenant_id=$1 AND t.company_id=$2 AND ($3::bigint IS NULL OR EXISTS(SELECT 1 FROM factory_inventory_transaction_entries e JOIN factory_inventory_materials m ON m.company_id=e.company_id AND m.id=e.material_id WHERE e.tenant_id=t.tenant_id AND e.company_id=t.company_id AND e.transaction_id=t.id AND m.item_id=$3)) ORDER BY t.id DESC LIMIT $4", &[&scope.tenant(),&company,&item,&limit]).await?;
    let mut result = Vec::new();
    for row in rows {
        result.push(read(client, scope.tenant(), company, row.get(0)).await?);
    }
    Ok(result)
}

pub(crate) async fn position(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    item: i64,
    location: i64,
) -> Result<StockPosition, Error> {
    access::readable(client, scope, company).await?;
    let (id, material) = materials::fetch(client, scope, company, item, "").await?;
    location_exists(client, scope.tenant(), company, location).await?;
    let quantity = client.query_one("SELECT COALESCE(sum(native_quantity),0)::text FROM factory_inventory_transaction_entries WHERE tenant_id=$1 AND company_id=$2 AND material_id=$3 AND location_id=$4 AND role='stock'", &[&scope.tenant(),&company,&id,&location]).await?.get(0);
    Ok(StockPosition {
        item_id: item,
        location_id: location,
        quantity,
        unit: material.native_unit,
    })
}

async fn location_exists(
    client: &impl GenericClient,
    tenant: i64,
    company: i64,
    location: i64,
) -> Result<(), Error> {
    if client.query_opt("SELECT id FROM factory_inventory_locations WHERE tenant_id=$1 AND company_id=$2 AND id=$3", &[&tenant,&company,&location]).await?.is_none() { return Err(Error::LocationNotFound); }
    Ok(())
}

pub(crate) async fn post(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    kind: TransactionKind,
    request: &PostingRequest,
) -> Result<MaterialTransaction, Error> {
    access::writable(tx.sql(), scope, company).await?;
    let request = ledger_request::validate(kind, request)?;
    let tenant = scope.tenant();
    let actor = scope.actor().user();
    if request
        .context
        .keys()
        .any(|key| ["operation_execution", "order_or_batch", "work_centre"].contains(&key.as_str()))
    {
        return Err(Error::UnregisteredPostingAuthority);
    }
    if let Some(original) = request.corrects_transaction_id {
        let original = read(tx.sql(), tenant, company, original).await?;
        if original.posting_authority.is_some() {
            return Err(Error::UnregisteredPostingAuthority);
        }
    }
    // The Core writable-company lock serialises posts even when a conflicting
    // request names disjoint materials. Material locks also order retirement
    // and conversion versions. Never call mutate on a replay (no audit write).
    let fingerprint: String = tx
        .sql()
        .query_one(
            "SELECT encode(sha256(convert_to($1::jsonb::text,'UTF8')),'hex')",
            &[&json!({"kind":kind,"actor_id":actor,"request":request})],
        )
        .await?
        .get(0);
    if let Some(row) = tx.sql().query_opt("SELECT id,request_fingerprint FROM factory_inventory_transactions WHERE tenant_id=$1 AND company_id=$2 AND request_id=$3", &[&tenant,&company,&request.request_id]).await? {
        if row.get::<_,String>(1) != fingerprint { return Err(Error::RequestIdConflict); }
        return read(tx.sql(),tenant,company,row.get(0)).await;
    }
    let mut items: Vec<_> = request.lines.iter().map(|l| l.item_id).collect();
    items.sort_unstable();
    items.dedup();
    let mut materials = BTreeMap::new();
    for item in items {
        let material = materials::fetch(tx.sql(), scope, company, item, "FOR UPDATE").await?;
        if material.1.retired && kind != TransactionKind::Correction {
            return Err(Error::MaterialRetired);
        }
        materials.insert(item, material);
    }
    let mut entries = Vec::new();
    for line in &request.lines {
        let (material_id, material) = &materials[&line.item_id];
        for location in [line.location_id, line.from_location_id, line.to_location_id]
            .into_iter()
            .flatten()
        {
            location_exists(tx.sql(), tenant, company, location).await?;
        }
        let unit_id = line.unit_id.unwrap_or(material.native_unit.id);
        let (conversion_id, factor) = if unit_id == material.native_unit.id {
            (None, Decimal::ONE)
        } else {
            let row = tx.sql().query_opt("SELECT id,factor FROM factory_inventory_unit_conversions WHERE tenant_id=$1 AND company_id=$2 AND material_id=$3 AND unit_id=$4 AND ($5::integer IS NULL OR version=$5) ORDER BY version DESC LIMIT 1", &[&tenant,&company,material_id,&unit_id,&line.conversion_version]).await?.ok_or(Error::ConversionNotFound)?;
            (Some(row.get::<_, i64>(0)), row.get::<_, Decimal>(1))
        };
        let sign: i32 = if kind == TransactionKind::Consumption
            || kind == TransactionKind::Transfer
            || line.quantity < Decimal::ZERO
        {
            -1
        } else {
            1
        };
        let native: String = tx
            .sql()
            .query_one(
                "SELECT (round($1::numeric*$2::numeric,12)*$3::integer)::text",
                &[&line.quantity.abs(), &factor, &sign],
            )
            .await?
            .get(0);
        // Refuse a quantity that rounds to zero before entering the boundary.
        let zero: bool = tx
            .sql()
            .query_one("SELECT $1::text::numeric=0", &[&native])
            .await?
            .get(0);
        if zero {
            return Err(invalid("lines", "native quantity rounds to zero"));
        }
        let stock = json!({"role":"stock","material_id":material_id,"location_id":line.location_id.or(line.from_location_id),"native_quantity":native,"native_unit_id":material.native_unit.id,"recorded_quantity":line.quantity.abs(),"recorded_unit_id":unit_id,"conversion_id":conversion_id,"observation":line.observation,"evidence":line.evidence});
        let opposite: String = tx
            .sql()
            .query_one("SELECT (-$1::text::numeric)::text", &[&native])
            .await?
            .get(0);
        let counterpart = if kind == TransactionKind::Transfer {
            let mut dest = stock.clone();
            dest["location_id"] = json!(line.to_location_id);
            dest["native_quantity"] = json!(opposite);
            dest
        } else {
            json!({"role":"boundary","material_id":material_id,"native_quantity":opposite,"native_unit_id":material.native_unit.id})
        };
        entries.push(stock);
        entries.push(counterpart);
    }
    let entries = Value::Array(entries);
    // Sum withdrawals, not net movement: additions in this request cannot
    // fund its own withdrawals, even across duplicate lines or transfer cycles.
    let sufficient: bool = tx.sql().query_one("SELECT NOT EXISTS(SELECT 1 FROM jsonb_to_recordset($3) AS p(role text,material_id bigint,location_id bigint,native_quantity numeric) WHERE p.role='stock' AND p.native_quantity<0 GROUP BY p.material_id,p.location_id HAVING -sum(p.native_quantity) > (SELECT COALESCE(sum(e.native_quantity),0) FROM factory_inventory_transaction_entries e WHERE e.tenant_id=$1 AND e.company_id=$2 AND e.material_id=p.material_id AND e.location_id=p.location_id AND e.role='stock'))", &[&tenant,&company,&entries]).await?.get(0);
    if !sufficient {
        return Err(Error::InsufficientStock);
    }
    let effective = request.effective_at;
    let id = tx.mutate(move |sql| Box::pin(async move {
        let measurement = request.receipt_measurement.as_ref().map(|m| json!(m));
        let row = sql.query_one("INSERT INTO factory_inventory_transactions(tenant_id,company_id,kind,request_id,request_fingerprint,actor_type,actor_id,evidence,receipt_measurement,reason,corrects_transaction_id,shipment_ref,destination_ref,effective_at,recorded_at) VALUES($1,$2,$3,$4,$5,'user',$6,$7,$8,$9,$10,$11,$12,COALESCE($13,clock_timestamp()),clock_timestamp()) RETURNING id,to_jsonb(factory_inventory_transactions) AS saved", &[&tenant,&company,&kind.name(),&request.request_id,&fingerprint,&actor,&request.evidence,&measurement,&request.reason,&request.corrects_transaction_id,&request.context.get("shipment"),&request.context.get("destination"),&effective]).await?;
        let id: i64 = row.get("id");
        let mut changes = vec![Change { kind:"factory_inventory_transactions".into(),id,old:json!({}),new:row.get("saved") }];
        let rows = sql.query("INSERT INTO factory_inventory_transaction_entries(tenant_id,company_id,transaction_id,role,material_id,location_id,native_quantity,native_unit_id,recorded_quantity,recorded_unit_id,conversion_id,observation,evidence) SELECT $1,$2,$3,p.role,p.material_id,p.location_id,p.native_quantity,p.native_unit_id,p.recorded_quantity,p.recorded_unit_id,p.conversion_id,p.observation,p.evidence FROM jsonb_to_recordset($4) AS p(role text,material_id bigint,location_id bigint,native_quantity numeric,native_unit_id bigint,recorded_quantity numeric,recorded_unit_id bigint,conversion_id bigint,observation text,evidence text) RETURNING id,to_jsonb(factory_inventory_transaction_entries) AS saved", &[&tenant,&company,&id,&entries]).await?;
        for row in rows { changes.push(Change {kind:"factory_inventory_transaction_entries".into(),id:row.get("id"),old:json!({}),new:row.get("saved")}); }
        Ok((id,changes))
    })).await?;
    read(tx.sql(), tenant, company, id).await
}
