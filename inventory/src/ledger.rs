//! Bilimbi Inventory.Ledger: locked planning, append-only posting and reads.
//! Native arithmetic stays in PostgreSQL: numeric(36,12) exceeds Decimal's
//! 96-bit mantissa. Read models carry exact decimal strings, never floats.
use crate::{
    access,
    authority::PostingAuthority,
    ledger_request::{
        self, invalid, NewIdentity, Observation, PostingRequest, ReceiptMeasurement, StockLine,
        TransactionKind, Variance,
    },
    materials, Decimal, Error, Material, Unit,
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
    /// The lot or unit identity a stock entry moves, if any.
    pub identity_id: Option<i64>,
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
    /// A transform links each input entry to each output entry it produced;
    /// every other kind has none.
    pub genealogy: Vec<GenealogyLink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenealogyLink {
    pub input_entry_id: i64,
    pub output_entry_id: i64,
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

pub(crate) async fn read(
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
        genealogy: client
            .query("SELECT input_entry_id,output_entry_id FROM factory_inventory_genealogy_links WHERE tenant_id=$1 AND company_id=$2 AND transaction_id=$3 ORDER BY id", &[&tenant,&company,&id])
            .await?
            .iter()
            .map(|row| GenealogyLink { input_entry_id: row.get(0), output_entry_id: row.get(1) })
            .collect(),
    })
}

/// Reads the transactions `sql` selects as `id` (`$1` tenant, `$2` company,
/// then `params`), in the order it gives.
pub(crate) async fn read_selected(
    client: &impl GenericClient,
    tenant: i64,
    company: i64,
    sql: &str,
    params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
) -> Result<Vec<MaterialTransaction>, Error> {
    let mut all: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&tenant, &company];
    all.extend_from_slice(params);
    let mut result = Vec::new();
    for row in client.query(sql, &all).await? {
        result.push(read(client, tenant, company, row.get(0)).await?);
    }
    Ok(result)
}

/// The corrections of a transaction, following corrections of corrections,
/// in ID order.
pub(crate) async fn corrections(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Vec<MaterialTransaction>, Error> {
    access::readable(client, scope, company).await?;
    read(client, scope.tenant(), company, id).await?;
    read_selected(client, scope.tenant(), company, "WITH RECURSIVE c(id) AS (SELECT id FROM factory_inventory_transactions WHERE tenant_id=$1 AND company_id=$2 AND kind='correction' AND corrects_transaction_id=$3 UNION SELECT t.id FROM factory_inventory_transactions t JOIN c ON t.corrects_transaction_id=c.id WHERE t.tenant_id=$1 AND t.company_id=$2 AND t.kind='correction') SELECT id FROM c ORDER BY id", &[&id]).await
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

/// The exact stock of `item` at `location` (decimal text), 0 when none. The
/// caller has already proven access; this serves a review made inside a
/// write's own transaction.
pub(crate) async fn stock_text(
    client: &impl GenericClient,
    tenant: i64,
    company: i64,
    item: i64,
    location: i64,
) -> Result<String, Error> {
    Ok(client.query_one("SELECT COALESCE(sum(e.native_quantity),0)::text FROM factory_inventory_transaction_entries e JOIN factory_inventory_materials m ON m.company_id=e.company_id AND m.id=e.material_id WHERE e.tenant_id=$1 AND e.company_id=$2 AND m.item_id=$3 AND e.location_id=$4 AND e.role='stock'", &[&tenant,&company,&item,&location]).await?.get(0))
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

/// Context only a production posting may carry.
const PRODUCTION_CONTEXT: [&str; 3] = ["operation_execution", "order_or_batch", "work_centre"];

/// One entry to insert, in plan order. A stock entry may cite an identity or
/// create one; `position` orders the plan for SQL sums.
#[derive(Clone, Debug, Serialize)]
struct Planned {
    position: usize,
    role: &'static str,
    material_id: Option<i64>,
    location_id: Option<i64>,
    /// Exact signed numeric text.
    native_quantity: String,
    native_unit_id: i64,
    recorded_quantity: Option<Decimal>,
    recorded_unit_id: Option<i64>,
    conversion_id: Option<i64>,
    observation: Option<Observation>,
    identity_id: Option<i64>,
    #[serde(skip)]
    new_identity: Option<NewIdentity>,
    output_role: Option<String>,
    evidence: Option<String>,
    reconciliation_basis: Option<String>,
}
impl Planned {
    fn negative(&self) -> bool {
        self.native_quantity.starts_with('-')
    }
    fn stock(&self) -> bool {
        self.role == "stock"
    }
    fn signed(mut self, sign: i32) -> Self {
        if sign < 0 {
            self.native_quantity = negate(&self.native_quantity);
        }
        self
    }
    /// The counterpart outside stock.
    fn boundary(&self) -> Self {
        Self {
            position: 0,
            role: "boundary",
            material_id: self.material_id,
            location_id: None,
            native_quantity: negate(&self.native_quantity),
            native_unit_id: self.native_unit_id,
            recorded_quantity: None,
            recorded_unit_id: None,
            conversion_id: None,
            observation: None,
            identity_id: None,
            new_identity: None,
            output_role: None,
            evidence: None,
            reconciliation_basis: None,
        }
    }
}
fn negate(quantity: &str) -> String {
    quantity
        .strip_prefix('-')
        .map_or_else(|| format!("-{quantity}"), str::to_owned)
}

pub(crate) async fn post(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    kind: TransactionKind,
    request: &PostingRequest,
    authority: Option<&'static PostingAuthority>,
    import: Option<&crate::ProductionImport<'_>>,
) -> Result<MaterialTransaction, Error> {
    if let Some(import) = import {
        import.authorize_on(tx.sql()).await?;
        bilibi_company::organisation::require_writable(tx.sql(), scope, company, true).await?;
    } else {
        access::writable(tx.sql(), scope, company).await?;
    }
    let request = ledger_request::validate(kind, request)?;
    let tenant = scope.tenant();
    let actor = scope.actor().user();
    // Production or transform context needs a registered authority, and so
    // does correcting a posting that needed one.
    let original = match request.corrects_transaction_id {
        Some(original) => Some(read(tx.sql(), tenant, company, original).await?),
        None => None,
    };
    let production = kind.production()
        || request
            .context
            .keys()
            .any(|key| PRODUCTION_CONTEXT.contains(&key.as_str()))
        || original
            .as_ref()
            .is_some_and(|original| original.posting_authority.is_some());
    if production && authority.is_none() {
        return Err(Error::UnregisteredPostingAuthority);
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
    let mut items: Vec<_> = request.all_lines().map(|l| l.item_id).collect();
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
    for line in request.all_lines() {
        for location in [line.location_id, line.from_location_id, line.to_location_id]
            .into_iter()
            .flatten()
        {
            location_exists(tx.sql(), tenant, company, location).await?;
        }
    }
    let (mut entries, links) = plan(tx, scope, kind, &request, &materials).await?;
    for (position, entry) in entries.iter_mut().enumerate() {
        entry.position = position;
    }
    validate_identities(tx, tenant, company, kind, &entries).await?;
    stock_suffices(tx, tenant, company, &entries).await?;
    let effective = request.effective_at;
    let module = authority.map(PostingAuthority::module);
    let id = tx.mutate(move |sql| Box::pin(async move {
        let measurement = request.receipt_measurement.as_ref().map(|m| json!(m));
        let context = |key: &str| request.context.get(key);
        let row = sql.query_one("INSERT INTO factory_inventory_transactions(tenant_id,company_id,kind,request_id,request_fingerprint,actor_type,actor_id,evidence,receipt_measurement,reason,corrects_transaction_id,posting_authority,operation_execution_ref,order_or_batch_ref,work_centre_ref,shipment_ref,destination_ref,effective_at,recorded_at) VALUES($1,$2,$3,$4,$5,'user',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,COALESCE($17,clock_timestamp()),clock_timestamp()) RETURNING id,to_jsonb(factory_inventory_transactions) AS saved", &[&tenant,&company,&kind.name(),&request.request_id,&fingerprint,&actor,&request.evidence,&measurement,&request.reason,&request.corrects_transaction_id,&module,&context("operation_execution"),&context("order_or_batch"),&context("work_centre"),&context("shipment"),&context("destination"),&effective]).await?;
        let id: i64 = row.get("id");
        let mut changes = vec![Change { kind:"factory_inventory_transactions".into(),id,old:json!({}),new:row.get("saved") }];
        let mut entry_ids = Vec::with_capacity(entries.len());
        for entry in &entries {
            let mut identity_id = entry.identity_id;
            if let Some(identity) = &entry.new_identity {
                let row = sql.query_one("INSERT INTO factory_inventory_identities(tenant_id,company_id,material_id,source_transaction_id,kind,code,dimensions) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id,to_jsonb(factory_inventory_identities) AS saved", &[&tenant,&company,&entry.material_id,&id,&identity.kind.name(),&identity.code,&json!(identity.dimensions)]).await?;
                identity_id = Some(row.get("id"));
                changes.push(Change { kind:"factory_inventory_identities".into(),id:row.get("id"),old:json!({}),new:row.get("saved") });
            }
            let row = sql.query_one("INSERT INTO factory_inventory_transaction_entries(tenant_id,company_id,transaction_id,role,material_id,location_id,native_quantity,native_unit_id,recorded_quantity,recorded_unit_id,conversion_id,observation,identity_id,output_role,evidence,reconciliation_basis) VALUES($1,$2,$3,$4,$5,$6,$7::text::numeric,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING id,to_jsonb(factory_inventory_transaction_entries) AS saved", &[&tenant,&company,&id,&entry.role,&entry.material_id,&entry.location_id,&entry.native_quantity,&entry.native_unit_id,&entry.recorded_quantity,&entry.recorded_unit_id,&entry.conversion_id,&entry.observation.map(Observation::name),&identity_id,&entry.output_role,&entry.evidence,&entry.reconciliation_basis]).await?;
            entry_ids.push(row.get::<_, i64>("id"));
            changes.push(Change {kind:"factory_inventory_transaction_entries".into(),id:row.get("id"),old:json!({}),new:row.get("saved")});
        }
        for (input, output) in links {
            let row = sql.query_one("INSERT INTO factory_inventory_genealogy_links(tenant_id,company_id,transaction_id,input_entry_id,output_entry_id) VALUES($1,$2,$3,$4,$5) RETURNING id,to_jsonb(factory_inventory_genealogy_links) AS saved", &[&tenant,&company,&id,&entry_ids[input],&entry_ids[output]]).await?;
            changes.push(Change {kind:"factory_inventory_genealogy_links".into(),id:row.get("id"),old:json!({}),new:row.get("saved")});
        }
        Ok((id,changes))
    })).await?;
    read(tx.sql(), tenant, company, id).await
}

type Materials = BTreeMap<i64, (i64, Material)>;

/// The entries to insert, in order, and the genealogy links between them as
/// (input position, output position).
async fn plan(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    kind: TransactionKind,
    request: &PostingRequest,
    materials: &Materials,
) -> Result<(Vec<Planned>, Vec<(usize, usize)>), Error> {
    let mut entries = Vec::new();
    match kind {
        TransactionKind::Transfer => {
            for line in &request.lines {
                let stock = stock(tx, scope, line, materials).await?;
                let mut from = stock.clone().signed(-1);
                from.location_id = line.from_location_id;
                let mut to = stock;
                to.location_id = line.to_location_id;
                entries.push(from);
                entries.push(to);
            }
            Ok((entries, Vec::new()))
        }
        TransactionKind::Transform => {
            for line in &request.inputs {
                entries.push(stock(tx, scope, line, materials).await?.signed(-1));
            }
            for line in &request.outputs {
                entries.push(stock(tx, scope, line, materials).await?);
            }
            let (inputs, outputs) = (request.inputs.len(), request.outputs.len());
            let variance = variance(tx, &entries, request.variance.as_ref()).await?;
            entries.extend(variance);
            let links = (0..inputs)
                .flat_map(|input| (inputs..inputs + outputs).map(move |output| (input, output)))
                .collect();
            Ok((entries, links))
        }
        _ => {
            let sign = if kind == TransactionKind::Consumption {
                -1
            } else {
                1
            };
            for line in &request.lines {
                let line_sign = if line.quantity < Decimal::ZERO {
                    -sign
                } else {
                    sign
                };
                let stock = stock(tx, scope, line, materials).await?.signed(line_sign);
                let boundary = stock.boundary();
                entries.push(stock);
                entries.push(boundary);
            }
            Ok((entries, Vec::new()))
        }
    }
}

/// A positive stock entry for `line`. The recorded quantity is kept as
/// observed; the native quantity is derived from it through the named
/// conversion version and never replaces it.
async fn stock(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    line: &StockLine,
    materials: &Materials,
) -> Result<Planned, Error> {
    let (material_id, material) = &materials[&line.item_id];
    let (tenant, company) = (scope.tenant(), material.company_id);
    let unit_id = line.unit_id.unwrap_or(material.native_unit.id);
    let (conversion_id, factor) = if unit_id == material.native_unit.id {
        (None, Decimal::ONE)
    } else {
        let row = tx.sql().query_opt("SELECT id,factor FROM factory_inventory_unit_conversions WHERE tenant_id=$1 AND company_id=$2 AND material_id=$3 AND unit_id=$4 AND ($5::integer IS NULL OR version=$5) ORDER BY version DESC LIMIT 1", &[&tenant,&company,material_id,&unit_id,&line.conversion_version]).await?.ok_or(Error::ConversionNotFound)?;
        (Some(row.get::<_, i64>(0)), row.get::<_, Decimal>(1))
    };
    let native: String = tx
        .sql()
        .query_one(
            "SELECT round($1::numeric*$2::numeric,12)::text",
            &[&line.quantity.abs(), &factor],
        )
        .await?
        .get(0);
    // Refuse a quantity that rounds to zero before entering the boundary.
    if native.chars().all(|c| c == '0' || c == '.') {
        return Err(invalid("lines", "native quantity rounds to zero"));
    }
    Ok(Planned {
        position: 0,
        role: "stock",
        material_id: Some(*material_id),
        location_id: line.location_id,
        native_quantity: native,
        native_unit_id: material.native_unit.id,
        recorded_quantity: Some(line.quantity.abs()),
        recorded_unit_id: Some(unit_id),
        conversion_id,
        observation: Some(line.observation),
        identity_id: line.identity_id,
        new_identity: line.identity.clone(),
        output_role: line.output_role.clone(),
        evidence: line.evidence.clone(),
        reconciliation_basis: None,
    })
}

/// A transform balances per native unit; its observations are compared,
/// never adjusted, and nothing is converted between units. Each unit whose
/// inputs and outputs differ records the difference as a variance entry in
/// that unit, carrying the shared evidence or the evidence named for that
/// unit. Evidence that no difference uses is refused.
async fn variance(
    tx: &mut MutationTransaction<'_>,
    stock: &[Planned],
    evidence: Option<&Variance>,
) -> Result<Vec<Planned>, Error> {
    let positioned: Vec<_> = stock
        .iter()
        .enumerate()
        .map(|(position, entry)| json!({"position":position,"native_unit_id":entry.native_unit_id,"native_quantity":entry.native_quantity}))
        .collect();
    let differences: Vec<(i64, String)> = tx.sql().query("SELECT native_unit_id,(-sum(native_quantity))::text FROM jsonb_to_recordset($1) AS p(position bigint,native_unit_id bigint,native_quantity numeric) GROUP BY native_unit_id HAVING sum(native_quantity)<>0 ORDER BY min(position)", &[&Value::Array(positioned)]).await?.iter().map(|row| (row.get(0), row.get(1))).collect();
    let named: BTreeMap<i64, (&str, &str)> = match evidence {
        None => BTreeMap::new(),
        Some(Variance::Shared {
            evidence,
            reconciliation_basis,
        }) => differences
            .iter()
            .map(|(unit, _)| (*unit, (evidence.as_str(), reconciliation_basis.as_str())))
            .collect(),
        Some(Variance::Units(units)) => units
            .iter()
            .map(|unit| {
                (
                    unit.unit_id,
                    (unit.evidence.as_str(), unit.reconciliation_basis.as_str()),
                )
            })
            .collect(),
    };
    if differences
        .iter()
        .any(|(unit, _)| !named.contains_key(unit))
    {
        return Err(Error::VarianceRequired);
    }
    if evidence.is_some() && (differences.is_empty() || named.len() > differences.len()) {
        return Err(Error::NoVariance);
    }
    Ok(differences
        .into_iter()
        .map(|(unit, difference)| {
            let (evidence, basis) = named[&unit];
            Planned {
                position: 0,
                role: "variance",
                material_id: None,
                location_id: None,
                native_quantity: difference,
                native_unit_id: unit,
                recorded_quantity: None,
                recorded_unit_id: None,
                conversion_id: None,
                observation: None,
                identity_id: None,
                new_identity: None,
                output_role: None,
                evidence: Some(evidence.to_owned()),
                reconciliation_basis: Some(basis.to_owned()),
            }
        })
        .collect())
}

/// An identity is created only by a positive receipt, output or transform
/// output, and cited only by a later movement of its own material. A
/// transform that uses any identity identifies every line.
async fn validate_identities(
    tx: &mut MutationTransaction<'_>,
    tenant: i64,
    company: i64,
    kind: TransactionKind,
    entries: &[Planned],
) -> Result<(), Error> {
    use TransactionKind::{Output, Receipt, Transform};
    let stock: Vec<_> = entries.iter().filter(|entry| entry.stock()).collect();
    let creates = |entry: &&Planned| entry.new_identity.is_some();
    let cites = |entry: &&Planned| entry.identity_id.is_some();
    if stock.iter().any(|e| creates(e) && cites(e))
        || stock
            .iter()
            .any(|e| creates(e) && !(!e.negative() && matches!(kind, Receipt | Output | Transform)))
        || (matches!(kind, Receipt | Output) && stock.iter().any(cites))
        || (kind == Transform && stock.iter().any(|e| !e.negative() && cites(e)))
    {
        return Err(Error::InvalidIdentity);
    }
    if kind == Transform
        && stock.iter().any(|e| cites(e) || creates(e))
        && stock.iter().any(|e| !cites(e) && !creates(e))
    {
        return Err(Error::IdentityRequired);
    }
    for entry in stock.iter().filter(|e| cites(e)) {
        if tx.sql().query_opt("SELECT 1 FROM factory_inventory_identities WHERE tenant_id=$1 AND company_id=$2 AND id=$3 AND material_id=$4", &[&tenant,&company,&entry.identity_id,&entry.material_id]).await?.is_none() {
            return Err(Error::IdentityNotFound);
        }
    }
    // A code is unique for its material in the company; decided here, before
    // the write, rather than by the unique index.
    let mut codes = Vec::new();
    for entry in stock.iter().filter(|e| creates(e)) {
        let code = &entry.new_identity.as_ref().expect("created").code;
        let taken = codes.contains(&(entry.material_id, code))
            || tx.sql().query_opt("SELECT 1 FROM factory_inventory_identities WHERE tenant_id=$1 AND company_id=$2 AND material_id=$3 AND code=$4", &[&tenant,&company,&entry.material_id,code]).await?.is_some();
        if taken {
            return Err(invalid("code", "has already been taken"));
        }
        codes.push((entry.material_id, code));
    }
    Ok(())
}

/// Every position a posting draws down must hold, before the posting, all it
/// draws there: the location's whole stock, then the cited identity's stock
/// there, then the unidentified pool for an unidentified draw. The posting's
/// own additions never cover its draws; the material locks make these reads
/// current.
async fn stock_suffices(
    tx: &mut MutationTransaction<'_>,
    tenant: i64,
    company: i64,
    entries: &[Planned],
) -> Result<(), Error> {
    let draws = Value::Array(
        entries
            .iter()
            .filter(|entry| entry.stock() && entry.negative())
            .map(|entry| json!(entry))
            .collect(),
    );
    let checks = [
        (
            "p.material_id,p.location_id",
            "e.material_id=p.material_id AND e.location_id=p.location_id",
            "",
            Error::InsufficientStock,
        ),
        (
            "p.identity_id,p.location_id",
            "e.identity_id=p.identity_id AND e.location_id=p.location_id",
            "AND p.identity_id IS NOT NULL",
            Error::InsufficientIdentityStock,
        ),
        (
            "p.material_id,p.location_id",
            "e.material_id=p.material_id AND e.location_id=p.location_id AND e.identity_id IS NULL",
            "AND p.identity_id IS NULL",
            Error::InsufficientUnidentifiedStock,
        ),
    ];
    for (group, matched, filter, error) in checks {
        let sql = format!("SELECT NOT EXISTS(SELECT 1 FROM jsonb_to_recordset($3) AS p(material_id bigint,location_id bigint,identity_id bigint,native_quantity numeric) WHERE true {filter} GROUP BY {group} HAVING -sum(p.native_quantity) > (SELECT COALESCE(sum(e.native_quantity),0) FROM factory_inventory_transaction_entries e WHERE e.tenant_id=$1 AND e.company_id=$2 AND e.role='stock' AND {matched}))");
        let sufficient: bool = tx
            .sql()
            .query_one(&sql, &[&tenant, &company, &draws])
            .await?
            .get(0);
        if !sufficient {
            return Err(error);
        }
    }
    Ok(())
}
