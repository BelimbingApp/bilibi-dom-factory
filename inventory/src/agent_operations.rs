//! The Inventory operations agents may call, each a thin adapter over this
//! module's own code with the sealed scope the registry proved. Locations,
//! receipts, transfers, consumption and corrections have no page in the
//! reference, so these operations are their production reach; the stock
//! position, transaction and genealogy reads join them.
//!
//! Reads call the facade. A write is a reviewed safe write: its preview runs
//! the real posting inside a transaction of its own and rolls it back, so the
//! review shows exactly what the commit will decide (every refusal, the
//! converted native quantities, the stock before and after) without a write,
//! a generated ID or a timestamp that would differ at commit; its commit
//! posts inside the plan's audited transaction. Production postings are
//! deliberately absent: they need a registered `PostingAuthority`, which an
//! agent cannot supply, so context and corrections that need one are refused
//! as `Unregistered posting authority`.
use crate::{
    access,
    contributions::{CONFIGURATION_MANAGE, CONFIGURATION_VIEW},
    ledger::{self, MaterialTransaction},
    ledger_request::TransactionKind,
    locations, Balance, Entry, Error, Identity, Inventory, Location, NewLocation, PostingRequest,
    StockLine, StockPosition, Trace, Unit,
};
use bilibi_agent_api::{
    json::{Data, Output},
    Channels, Definitions, Guide, Handler, Operation, OperationClient, OperationFuture,
    ReadHandler, WriteHandler, DEFINITIONS,
};
use bilibi_database::MutationTransaction;
use bilibi_module_registry::{Contribution, Provenance};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

type Refusal = bilibi_agent_api::Error;
type Outcome<T> = Result<T, Refusal>;

/// Handlers are static; the facade they call is bound once by the
/// `factory/inventory.inventory` service factory. An unbound handler refuses.
static BOUND: OnceLock<Arc<Inventory>> = OnceLock::new();
pub(crate) fn bind(inventory: Arc<Inventory>) -> bilibi_module_registry::Result<()> {
    BOUND.set(inventory).map_err(|_| {
        bilibi_module_registry::RegistryError::Service("duplicate inventory agent binding".into())
    })
}
fn inventory() -> Outcome<&'static Arc<Inventory>> {
    BOUND
        .get()
        .ok_or(Refusal::Backend(bilibi_database::Error::Internal))
}

fn invalid(field: impl Into<String>, message: impl Into<String>) -> Refusal {
    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    fields.insert(field.into(), vec![message.into()]);
    Refusal::InvalidInput(fields)
}
/// An access failure keeps its kind (the plan reports a denial as a missing
/// capability); every refusal that is the input's own is an invalid input
/// carrying the text a person sees, under the field it concerns.
fn refuse(error: Error) -> Refusal {
    let text = error.to_string();
    match error {
        Error::Access(error) => Refusal::Backend(error),
        Error::Invalid(errors) => {
            let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for error in errors {
                fields
                    .entry(error.field.to_owned())
                    .or_default()
                    .push(error.message);
            }
            Refusal::InvalidInput(fields)
        }
        Error::CompanyNotFound | Error::CompanyArchived => invalid("company_id", text),
        Error::RequestIdConflict => invalid("request_id", text),
        Error::TransactionNotFound => invalid("transaction_id", text),
        _ => invalid("request", text),
    }
}

fn integer(input: &Value, field: &str) -> Outcome<i64> {
    input[field]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid(field, "must be a positive integer"))
}
fn optional(input: &Value, field: &str) -> Outcome<Option<i64>> {
    match &input[field] {
        Value::Null => Ok(None),
        _ => integer(input, field).map(Some),
    }
}
fn limit(input: &Value) -> Outcome<Option<i64>> {
    optional(input, "limit")
}

/// A timestamp as RFC 3339 UTC with microseconds, from the epoch by the
/// proleptic Gregorian calendar.
fn instant(at: SystemTime) -> String {
    let micros = match at.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_micros() as i64,
        Err(before) => -(before.duration().as_micros() as i64),
    };
    let seconds = micros.div_euclid(1_000_000);
    let (days, second) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        second / 3600,
        second % 3600 / 60,
        second % 60,
        micros.rem_euclid(1_000_000)
    )
}

fn unit_json(unit: &Unit) -> Value {
    json!({"id":unit.id,"code":unit.code,"name":unit.name,"retired":unit.retired})
}
fn location_json(location: &Location) -> Value {
    json!({"id":location.id,"code":location.code,"name":location.name})
}
fn entry_json(entry: &Entry) -> Value {
    json!({
        "id":entry.id,"role":entry.role,"item_id":entry.item_id,"location_id":entry.location_id,
        "native_quantity":entry.native_quantity,"native_unit_id":entry.native_unit_id,
        "recorded_quantity":entry.recorded_quantity.map(|quantity| quantity.normalize().to_string()),
        "recorded_unit_id":entry.recorded_unit_id,"conversion_id":entry.conversion_id,
        "conversion_version":entry.conversion_version,
        "observation":entry.observation.map(crate::Observation::name),
        "identity_id":entry.identity_id,"evidence":entry.evidence,
        "output_role":entry.output_role,"reconciliation_basis":entry.reconciliation_basis,
    })
}
fn transaction_json(transaction: &MaterialTransaction) -> Value {
    json!({
        "id":transaction.id,"company_id":transaction.company_id,"kind":transaction.kind,
        "request_id":transaction.request_id,"actor_type":transaction.actor_type,
        "actor_id":transaction.actor_id,"evidence":transaction.evidence,
        "receipt_measurement":transaction.receipt_measurement,"reason":transaction.reason,
        "corrects_transaction_id":transaction.corrects_transaction_id,
        "posting_authority":transaction.posting_authority,
        "effective_at":instant(transaction.effective_at),
        "recorded_at":instant(transaction.recorded_at),
        "context":transaction.context,
        "entries":transaction.entries.iter().map(entry_json).collect::<Vec<_>>(),
        "genealogy":transaction.genealogy.iter().map(|link| json!({"input_entry_id":link.input_entry_id,"output_entry_id":link.output_entry_id})).collect::<Vec<_>>(),
    })
}
fn position_json(position: &StockPosition) -> Value {
    json!({"item_id":position.item_id,"location_id":position.location_id,"quantity":position.quantity,"unit":unit_json(&position.unit)})
}
fn identity_json(identity: &Identity) -> Value {
    json!({"id":identity.id,"item_id":identity.item_id,"kind":identity.kind,"code":identity.code,"source_transaction_id":identity.source_transaction_id,"dimensions":identity.dimensions})
}
fn trace_json(trace: &Trace) -> Value {
    json!({
        "root":identity_json(&trace.root),
        "identities":trace.identities.iter().map(identity_json).collect::<Vec<_>>(),
        "links":trace.links.iter().map(|link| json!({"input_identity_id":link.input_identity_id,"output_identity_id":link.output_identity_id,"transaction_id":link.transaction_id})).collect::<Vec<_>>(),
        "receipts":trace.receipts.iter().map(transaction_json).collect::<Vec<_>>(),
    })
}
fn balance_json(balance: &Balance) -> Value {
    json!({
        "transaction_id":balance.transaction_id,
        "per_unit":balance.per_unit.iter().map(|group| json!({"unit":unit_json(&group.unit),"input":group.input,"output":group.output,"difference":group.difference,"variance":group.variance})).collect::<Vec<_>>(),
        "cross_unit":balance.cross_unit.iter().map(|group| json!({"unit":unit_json(&group.unit),"input":group.input,"output":group.output,"difference":group.difference,"conversions":group.conversions.iter().map(|c| json!({"item_id":c.item_id,"conversion_id":c.conversion_id,"version":c.version,"factor":c.factor.normalize().to_string()})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "correction_transaction_ids":balance.correction_transaction_ids,
    })
}

enum Read {
    LocationList,
    LocationGet,
    StockPosition,
    TransactionList,
    TransactionGet,
    TransactionCorrections,
    TransactionBalance,
    IdentityGet,
    IdentityPositions,
    IdentityTrace,
    IdentityDraws,
}
impl ReadHandler for Read {
    fn owner(&self) -> &'static str {
        "factory/inventory"
    }
    fn call<'a>(
        &'a self,
        _client: &'a dyn OperationClient,
        scope: &'a Scope,
        input: &'a Value,
    ) -> OperationFuture<'a> {
        Box::pin(async move {
            let inventory = inventory()?;
            let company = integer(input, "company_id")?;
            let transactions = |all: Vec<MaterialTransaction>| {
                Value::Array(all.iter().map(transaction_json).collect())
            };
            let value = match self {
                Self::LocationList => Value::Array(
                    inventory
                        .list_locations(scope, company, limit(input)?)
                        .await
                        .map_err(refuse)?
                        .iter()
                        .map(location_json)
                        .collect(),
                ),
                Self::LocationGet => location_json(
                    &inventory
                        .get_location(scope, company, integer(input, "location_id")?)
                        .await
                        .map_err(refuse)?,
                ),
                Self::StockPosition => position_json(
                    &inventory
                        .get_stock_position(
                            scope,
                            company,
                            integer(input, "item_id")?,
                            integer(input, "location_id")?,
                        )
                        .await
                        .map_err(refuse)?,
                ),
                Self::TransactionList => transactions(
                    inventory
                        .list_transactions(
                            scope,
                            company,
                            optional(input, "item_id")?,
                            limit(input)?,
                        )
                        .await
                        .map_err(refuse)?,
                ),
                Self::TransactionGet => transaction_json(
                    &inventory
                        .get_transaction(scope, company, integer(input, "transaction_id")?)
                        .await
                        .map_err(refuse)?,
                ),
                Self::TransactionCorrections => transactions(
                    inventory
                        .list_corrections(scope, company, integer(input, "transaction_id")?)
                        .await
                        .map_err(refuse)?,
                ),
                Self::TransactionBalance => balance_json(
                    &inventory
                        .get_transaction_balance(scope, company, integer(input, "transaction_id")?)
                        .await
                        .map_err(refuse)?,
                ),
                Self::IdentityGet => identity_json(
                    &inventory
                        .get_identity(scope, company, integer(input, "identity_id")?)
                        .await
                        .map_err(refuse)?,
                ),
                Self::IdentityPositions => Value::Array(
                    inventory
                        .get_identity_positions(scope, company, integer(input, "identity_id")?)
                        .await
                        .map_err(refuse)?
                        .iter()
                        .map(|position| json!({"location":location_json(&position.location),"quantity":position.quantity,"unit":unit_json(&position.unit)}))
                        .collect(),
                ),
                Self::IdentityTrace => {
                    let id = integer(input, "identity_id")?;
                    let trace = match input["direction"].as_str() {
                        Some("backward") => inventory.trace_backward(scope, company, id).await,
                        Some("forward") => inventory.trace_forward(scope, company, id).await,
                        _ => return Err(invalid("direction", "must be backward or forward")),
                    };
                    trace_json(&trace.map_err(refuse)?)
                }
                Self::IdentityDraws => transactions(
                    inventory
                        .list_identity_draws(scope, company, integer(input, "identity_id")?)
                        .await
                        .map_err(refuse)?,
                ),
            };
            Ok(Output::Data(Data::from(value)))
        })
    }
}

/// Parses a posting's wire input into the facade's request. Quantities and
/// factors arrive as decimal strings and stay exact; the schema has already
/// refused unknown keys.
fn parse_posting(input: &Value) -> Outcome<PostingRequest> {
    let text = |field: &str| input[field].as_str().map(str::to_owned);
    let mut request = PostingRequest {
        request_id: text("request_id").unwrap_or_default(),
        evidence: text("evidence").unwrap_or_default(),
        reason: text("reason"),
        corrects_transaction_id: optional(input, "corrects_transaction_id")?,
        ..PostingRequest::default()
    };
    if let Some(context) = input["context"].as_object() {
        for (key, value) in context {
            request
                .context
                .insert(key.clone(), value.as_str().unwrap_or_default().to_owned());
        }
    }
    if !input["receipt_measurement"].is_null() {
        request.receipt_measurement = Some(
            serde_json::from_value(input["receipt_measurement"].clone()).map_err(|error| {
                invalid("receipt_measurement", format!("is not valid: {error}"))
            })?,
        );
    }
    let lines = input["lines"]
        .as_array()
        .ok_or_else(|| invalid("lines", "must be a list of lines"))?;
    for (index, line) in lines.iter().enumerate() {
        let line: StockLine = serde_json::from_value(line.clone())
            .map_err(|error| invalid(format!("lines.{index}"), format!("is not valid: {error}")))?;
        request.lines.push(line);
    }
    Ok(request)
}

/// The effective time, resolved by PostgreSQL so no calendar library is
/// needed here; the schema has already checked the format.
async fn effective(
    client: &impl tokio_postgres::GenericClient,
    input: &Value,
) -> Outcome<Option<SystemTime>> {
    match input["effective_at"].as_str() {
        None => Ok(None),
        Some(text) => Ok(Some(
            client
                .query_one("SELECT $1::text::timestamptz", &[&text])
                .await
                .map_err(|_| invalid("effective_at", "is not a valid date and time"))?
                .get(0),
        )),
    }
}

/// Runs `$review` against a transaction that is always rolled back: the real
/// code path decides, and nothing it did survives.
macro_rules! rolled_back {
    ($inventory:ident, $scope:ident, $action:literal, |$tx:ident| $review:expr) => {{
        let mut client = $inventory
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        let mut $tx = $inventory
            .mutations
            .begin(&mut client, $scope.mutation_context($action))
            .await?;
        let outcome: Outcome<Value> = $review;
        $tx.rollback().await?;
        outcome
    }};
}

/// The (item, location) pairs a posting touches, in a fixed order.
fn touched(request: &PostingRequest) -> BTreeSet<(i64, i64)> {
    request
        .lines
        .iter()
        .flat_map(|line| {
            [line.location_id, line.from_location_id, line.to_location_id]
                .into_iter()
                .flatten()
                .map(|location| (line.item_id, location))
        })
        .collect()
}

async fn stock_figures(
    tx: &MutationTransaction<'_>,
    tenant: i64,
    company: i64,
    pairs: &BTreeSet<(i64, i64)>,
) -> Outcome<BTreeMap<(i64, i64), String>> {
    let mut figures = BTreeMap::new();
    for (item, location) in pairs {
        figures.insert(
            (*item, *location),
            ledger::stock_text(tx.sql(), tenant, company, *item, *location)
                .await
                .map_err(refuse)?,
        );
    }
    Ok(figures)
}

/// A posting's review: what it records and the stock it leaves, with no
/// generated ID or timestamp so the commit's own preview reproduces it.
async fn review_posting(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    kind: TransactionKind,
    input: &Value,
) -> Outcome<Value> {
    access::writable(tx.sql(), scope, company)
        .await
        .map_err(refuse)?;
    let tenant = scope.tenant();
    let mut request = parse_posting(input)?;
    request.effective_at = effective(tx.sql(), input).await?;
    let replay = tx
        .sql()
        .query_opt(
            "SELECT 1 FROM factory_inventory_transactions WHERE tenant_id=$1 AND company_id=$2 AND request_id=$3",
            &[&tenant, &company, &request.request_id],
        )
        .await
        .map_err(|error| refuse(error.into()))?
        .is_some();
    let pairs = touched(&request);
    let before = stock_figures(tx, tenant, company, &pairs).await?;
    let posted = ledger::post(tx, scope, company, kind, &request, None, None)
        .await
        .map_err(refuse)?;
    let after = stock_figures(tx, tenant, company, &pairs).await?;
    let entries: Vec<Value> = posted
        .entries
        .iter()
        .map(|entry| {
            let mut entry = entry_json(entry);
            // Identities a line creates have no ID until the commit.
            if let Some(object) = entry.as_object_mut() {
                object.remove("id");
                object.remove("identity_id");
                object.remove("conversion_id");
            }
            entry
        })
        .collect();
    Ok(json!({
        "summary": format!(
            "Record a {} with {} line(s); {}",
            kind.name(), request.lines.len(),
            if replay { "it repeats a recorded request and changes nothing" } else { "stock changes are listed under stock" },
        ),
        "kind": kind.name(),
        "request_id": request.request_id,
        "replay": replay,
        "lines": serde_json::to_value(&request.lines).map_err(|_| Refusal::Backend(bilibi_database::Error::Internal))?,
        "entries": entries,
        "stock": pairs.iter().map(|(item, location)| json!({"item_id":item,"location_id":location,"before":before[&(*item, *location)],"after":after[&(*item, *location)]})).collect::<Vec<_>>(),
    }))
}

enum Write {
    LocationCreate,
    Post(TransactionKind),
}
fn new_location(input: &Value) -> NewLocation {
    NewLocation {
        code: input["code"].as_str().unwrap_or_default().to_owned(),
        name: input["name"].as_str().unwrap_or_default().to_owned(),
    }
}
impl WriteHandler for Write {
    fn owner(&self) -> &'static str {
        "factory/inventory"
    }
    fn preview<'a>(
        &'a self,
        _client: &'a dyn OperationClient,
        scope: &'a Scope,
        input: &'a Value,
    ) -> OperationFuture<'a> {
        Box::pin(async move {
            let inventory = inventory()?;
            let company = integer(input, "company_id")?;
            let value = match self {
                Self::LocationCreate => {
                    let new = new_location(input);
                    rolled_back!(
                        inventory,
                        scope,
                        "factory.inventory.preview_location",
                        |tx| {
                            locations::create(&mut tx, scope, company, &new)
                            .await
                            .map(|location| {
                                json!({
                                    "summary": format!("Create stock location {} ({})", location.code, location.name),
                                    "location": {"code":location.code,"name":location.name},
                                })
                            })
                            .map_err(refuse)
                        }
                    )?
                }
                Self::Post(kind) => rolled_back!(
                    inventory,
                    scope,
                    "factory.inventory.preview_posting",
                    |tx| { review_posting(&mut tx, scope, company, *kind, input).await }
                )?,
            };
            Ok(value.into())
        })
    }
    fn commit<'a>(
        &'a self,
        tx: &'a mut MutationTransaction<'_>,
        scope: &'a Scope,
        input: &'a Value,
    ) -> OperationFuture<'a> {
        Box::pin(async move {
            let company = integer(input, "company_id")?;
            let value = match self {
                Self::LocationCreate => location_json(
                    &locations::create(tx, scope, company, &new_location(input))
                        .await
                        .map_err(refuse)?,
                ),
                Self::Post(kind) => {
                    let mut request = parse_posting(input)?;
                    request.effective_at = effective(tx.sql(), input).await?;
                    transaction_json(
                        &ledger::post(tx, scope, company, *kind, &request, None, None)
                            .await
                            .map_err(refuse)?,
                    )
                }
            };
            Ok(Output::Data(Data::from(value)))
        })
    }
}

fn id_schema() -> Value {
    json!({"type":"integer","minimum":1})
}
fn company_schema() -> Value {
    json!({"type":"integer","minimum":1,"description":"The company whose inventory this is."})
}
fn limit_schema() -> Value {
    json!({"type":"integer","minimum":1,"maximum":500,"description":"Most rows to return; 100 by default."})
}
fn object(required: &[&str], properties: Value) -> Value {
    json!({"type":"object","required":required,"properties":properties})
}
fn quantity_schema(signed: bool) -> Value {
    let note = if signed {
        "A non-zero signed decimal string, at most 12 decimal places: the adjustment, positive to add stock and negative to remove it."
    } else {
        "A positive decimal string, at most 12 decimal places and 12 integer digits."
    };
    json!({"type":"string","maxLength":40,"description":note})
}
fn dimension_schema() -> Value {
    object(
        &["value", "unit", "provenance"],
        json!({
            "value":{"type":"string","maxLength":40,"description":"A positive decimal string."},
            "unit":{"type":"string","enum":["um","mm","cm","m"]},
            "provenance":{"type":"string","enum":["measured","nominal"]},
        }),
    )
}
fn identity_schema() -> Value {
    let mut schema = object(
        &["kind", "code"],
        json!({
            "kind":{"type":"string","enum":["lot","unit"]},
            "code":{"type":"string","maxLength":255,"description":"Unique for the item's material in the company."},
            "dimensions":{"type":"object","description":"A unit only: width, length and thickness.","properties":{
                "width":dimension_schema(),"length":dimension_schema(),"thickness":dimension_schema()
            }},
        }),
    );
    schema["description"] = json!("The lot or unit this receipt line creates.");
    schema
}
fn line_schema(kind: TransactionKind) -> Value {
    let mut properties = json!({
        "item_id":id_schema(),
        "quantity":quantity_schema(kind == TransactionKind::Correction),
        "unit_id":{"type":"integer","minimum":1,"description":"The unit the quantity is in; the material's native unit when omitted."},
        "conversion_version":{"type":"integer","minimum":1,"description":"Pin an earlier conversion version; the latest when omitted."},
        "observation":{"type":"string","enum":["measured","declared","counted","derived"]},
        "evidence":{"type":"string","maxLength":10000},
    });
    let mut required = vec!["item_id", "quantity", "observation"];
    let map = properties.as_object_mut().expect("object");
    if kind == TransactionKind::Transfer {
        map.insert("from_location_id".into(), id_schema());
        map.insert("to_location_id".into(), id_schema());
        required.extend(["from_location_id", "to_location_id"]);
    } else {
        map.insert("location_id".into(), id_schema());
        required.push("location_id");
    }
    if kind == TransactionKind::Receipt {
        map.insert("identity".into(), identity_schema());
    } else {
        map.insert(
            "identity_id".into(),
            json!({"type":"integer","minimum":1,"description":"Cite a lot or unit identity of this item's material."}),
        );
    }
    object(&required, properties)
}
fn measurement_schema() -> Value {
    object(
        &[
            "supplier_declared",
            "measured_gross",
            "tare",
            "net",
            "unit_id",
            "weighing_point_ref",
        ],
        json!({
            "supplier_declared":{"type":"string","maxLength":40,"description":"The supplier's declared net, a positive decimal string."},
            "measured_gross":{"type":"string","maxLength":40},
            "tare":{"type":"string","maxLength":40,"description":"Not negative."},
            "net":{"type":"string","maxLength":40,"description":"Must equal measured_gross minus tare, and the single measured line's quantity."},
            "unit_id":id_schema(),
            "weighing_point_ref":{"type":"string","maxLength":255},
        }),
    )
}
fn posting_schema(kind: TransactionKind) -> Value {
    let mut properties = json!({
        "company_id":company_schema(),
        "request_id":{"type":"string","maxLength":255,"description":"Unique in the company; repeating a request with identical content returns the original and changes nothing."},
        "evidence":{"type":"string","maxLength":10000,"description":"The source evidence for the posting."},
        "effective_at":{"type":"string","format":"date-time","description":"When it happened; not later than now. Recorded time is the database's."},
        "context":{"type":"object","description":"Opaque references.","properties":{
            "shipment":{"type":"string","maxLength":255},"destination":{"type":"string","maxLength":255}
        }},
        "lines":{"type":"array","items":line_schema(kind),"description":"At least one line."},
    });
    let mut required = vec!["company_id", "request_id", "evidence", "lines"];
    let map = properties.as_object_mut().expect("object");
    match kind {
        TransactionKind::Receipt => {
            map.insert("receipt_measurement".into(), measurement_schema());
        }
        TransactionKind::Correction => {
            map.insert("reason".into(), json!({"type":"string","maxLength":10000}));
            map.insert("corrects_transaction_id".into(), id_schema());
            required.extend(["reason", "corrects_transaction_id"]);
        }
        _ => {}
    }
    object(&required, properties)
}
fn receipt_schema() -> Value {
    posting_schema(TransactionKind::Receipt)
}
fn transfer_schema() -> Value {
    posting_schema(TransactionKind::Transfer)
}
fn consumption_schema() -> Value {
    posting_schema(TransactionKind::Consumption)
}
fn correction_schema() -> Value {
    posting_schema(TransactionKind::Correction)
}
fn location_list_schema() -> Value {
    object(
        &["company_id"],
        json!({"company_id":company_schema(),"limit":limit_schema()}),
    )
}
fn location_get_schema() -> Value {
    object(
        &["company_id", "location_id"],
        json!({"company_id":company_schema(),"location_id":id_schema()}),
    )
}
fn location_create_schema() -> Value {
    object(
        &["company_id", "code", "name"],
        json!({
            "company_id":company_schema(),
            "code":{"type":"string","maxLength":64,"description":"Trimmed and upper-cased; unique in the company."},
            "name":{"type":"string","maxLength":255},
        }),
    )
}
fn position_schema() -> Value {
    object(
        &["company_id", "item_id", "location_id"],
        json!({"company_id":company_schema(),"item_id":id_schema(),"location_id":id_schema()}),
    )
}
fn transaction_list_schema() -> Value {
    object(
        &["company_id"],
        json!({"company_id":company_schema(),"item_id":id_schema(),"limit":limit_schema()}),
    )
}
fn transaction_schema() -> Value {
    object(
        &["company_id", "transaction_id"],
        json!({"company_id":company_schema(),"transaction_id":id_schema()}),
    )
}
fn identity_only() -> Value {
    object(
        &["company_id", "identity_id"],
        json!({"company_id":company_schema(),"identity_id":id_schema()}),
    )
}
fn trace_schema() -> Value {
    object(
        &["company_id", "identity_id", "direction"],
        json!({
            "company_id":company_schema(),"identity_id":id_schema(),
            "direction":{"type":"string","enum":["backward","forward"],"description":"Backward follows transforms toward source receipts; forward toward descendant outputs."},
        }),
    )
}
fn one(description: &'static str) -> Value {
    json!({"type":"object","description":description})
}
fn location_output() -> Value {
    one("A stock location: id, code and name.")
}
fn locations_output() -> Value {
    json!({"type":"array","description":"Stock locations ordered by code."})
}
fn transaction_output() -> Value {
    one("A Material Transaction: id, kind, request_id, actor, evidence, reason, effective_at and recorded_at, context, its signed entries (exact native_quantity strings; role stock, boundary or variance) and genealogy links.")
}
fn transactions_output() -> Value {
    json!({"type":"array","description":"Material Transactions, each as transaction.get returns, newest first unless stated."})
}
fn position_output() -> Value {
    one("The exact stock of an item at a location as a decimal string in the material's native unit.")
}
fn balance_output() -> Value {
    one("A transaction's balance net of its corrections: per native unit the observed input, output, difference and variance, and cross-unit agreement, as exact decimal strings.")
}
fn identity_output() -> Value {
    one("A lot or unit identity: item_id, kind, code, source_transaction_id and dimensions.")
}
fn positions_output() -> Value {
    json!({"type":"array","description":"The identity's positive stock by location, as exact decimal strings."})
}
fn trace_output() -> Value {
    one("A genealogy trace: the root, every identity reached, the transform links walked and the receipts among their sources.")
}
fn posted_output() -> Value {
    one("The recorded Material Transaction, as transaction.get returns it.")
}

macro_rules! read {
    ($key:literal, $title:literal, $summary:literal, [$($keyword:literal),*], $input:ident, $output:ident, $handler:expr) => {
        Operation { key:$key, title:$title, summary:$summary, keywords:&[$($keyword),*], capability:CONFIGURATION_VIEW, input:$input, output:$output, handler:Handler::Read(&$handler), channels:Channels::Every, step_up:&[] }
    };
}
macro_rules! write {
    ($key:literal, $title:literal, $summary:literal, [$($keyword:literal),*], $input:ident, $output:ident, $handler:expr) => {
        Operation { key:$key, title:$title, summary:$summary, keywords:&[$($keyword),*], capability:CONFIGURATION_MANAGE, input:$input, output:$output, handler:Handler::SafeWrite(&$handler), channels:Channels::Every, step_up:&[] }
    };
}

inventory::submit! { Contribution::new(DEFINITIONS, "factory/inventory.agent", Provenance { module: "factory/inventory", source: file!(), line: line!() }, &Definitions {
    operations: &[
        read!("factory.inventory.location.list", "List stock locations", "List the company's stock locations ordered by code.", ["location","warehouse","stock","bin"], location_list_schema, locations_output, Read::LocationList),
        read!("factory.inventory.location.get", "Read a stock location", "Read one stock location by ID.", ["location","stock"], location_get_schema, location_output, Read::LocationGet),
        read!("factory.inventory.stock.position", "Read a stock position", "Read the exact stock of one item at one location: the sum of its signed stock entries in the material's native unit.", ["stock","position","on hand","quantity","balance","location"], position_schema, position_output, Read::StockPosition),
        read!("factory.inventory.transaction.list", "List material transactions", "List the company's Material Transactions, newest first, optionally those containing one item.", ["transaction","ledger","movement","history","receipt","transfer","consumption","correction"], transaction_list_schema, transactions_output, Read::TransactionList),
        read!("factory.inventory.transaction.get", "Read a material transaction", "Read one Material Transaction with its signed entries and genealogy links.", ["transaction","ledger","entries"], transaction_schema, transaction_output, Read::TransactionGet),
        read!("factory.inventory.transaction.corrections", "List a transaction's corrections", "List the corrections of a Material Transaction, following corrections of corrections, in ID order.", ["correction","adjustment","transaction"], transaction_schema, transactions_output, Read::TransactionCorrections),
        read!("factory.inventory.transaction.balance", "Read a transaction's balance", "Read a transaction's material balance net of its corrections, per native unit and across units.", ["balance","variance","yield","transaction","agreement"], transaction_schema, balance_output, Read::TransactionBalance),
        read!("factory.inventory.identity.get", "Read a lot or unit identity", "Read one lot or unit identity with its code, dimensions and source transaction.", ["lot","unit","identity","genealogy","serial"], identity_only, identity_output, Read::IdentityGet),
        read!("factory.inventory.identity.positions", "Read an identity's stock positions", "Read where an identity's stock is held: positive balances by location.", ["lot","unit","identity","position","stock","location"], identity_only, positions_output, Read::IdentityPositions),
        read!("factory.inventory.identity.trace", "Trace an identity's genealogy", "Follow transform links from an identity backward to its source receipts or forward to descendant outputs.", ["genealogy","trace","traceability","recall","lot","parent","child"], trace_schema, trace_output, Read::IdentityTrace),
        read!("factory.inventory.identity.draws", "List the draws on an identity", "List the consumptions and transforms that drew an identity, in ID order.", ["genealogy","draw","consumption","lot","unit"], identity_only, transactions_output, Read::IdentityDraws),
        write!("factory.inventory.location.create", "Create a stock location", "Create a stock location with a code (upper-cased, unique in the company) and a name.", ["location","warehouse","stock","add"], location_create_schema, location_output, Write::LocationCreate),
        write!("factory.inventory.receipt.record", "Record a receipt", "Receive stock into locations with source evidence, an optional weigh ticket and optional lot or unit identities. Append-only and idempotent by request_id.", ["receipt","receive","stock","goods","weigh","lot","unit"], receipt_schema, posted_output, Write::Post(TransactionKind::Receipt)),
        write!("factory.inventory.transfer.record", "Record a transfer", "Move stock between two distinct locations, keeping its recorded basis and any identity a line cites.", ["transfer","move","stock","location"], transfer_schema, posted_output, Write::Post(TransactionKind::Transfer)),
        write!("factory.inventory.consumption.record", "Record a consumption", "Draw stock from locations under locks; stock can never go negative and concurrent requests cannot spend it twice.", ["consumption","consume","issue","draw","stock"], consumption_schema, posted_output, Write::Post(TransactionKind::Consumption)),
        write!("factory.inventory.correction.record", "Record a correction", "Append signed adjustments to an earlier transaction with a reason. The original never changes.", ["correction","adjust","fix","count","stock"], correction_schema, posted_output, Write::Post(TransactionKind::Correction)),
    ],
    guides: &[Guide { key:"factory.inventory", title:"Inventory", summary:"Stock locations, the append-only Material Transaction ledger and lot or unit genealogy.", keywords:&["inventory","stock","ledger","receipt","transfer","consumption","correction","location","genealogy"], capability:CONFIGURATION_VIEW,
        body:"Inventory keeps an append-only, signed Material Transaction ledger: stock at a location is the exact sum of its stock entries in the material's native unit. Every operation names a company_id. Quantities, native quantities and factors are decimal strings, never floats.\n\nA material is an item registered once with a fixed native unit; the item master and materials are managed on the Materials page, not here. Find locations with factory.inventory.location.list (create one with factory.inventory.location.create, in its own plan before any posting that names it, because a plan's reviews read committed data). Read stock with factory.inventory.stock.position and history with factory.inventory.transaction.list and .get.\n\nPostings are factory.inventory.receipt.record, transfer.record, consumption.record and correction.record. Each is a reviewed safe write: the review runs the real posting and rolls it back, so it shows every entry with its exact native quantity and the stock before and after, and refuses what the commit would refuse (insufficient stock, retired material, unknown location or conversion, a request_id reused with different content). Give every posting a request_id that is unique in the company; repeating an identical request returns the original transaction and changes nothing. A quantity is positive and may name unit_id (and conversion_version) to be converted to the native unit; the observed quantity and the conversion used are kept. observation says how the quantity was known: measured, declared, counted or derived. A receipt may carry receipt_measurement (supplier_declared, measured_gross, tare, net where gross minus tare equals net, unit_id and weighing_point_ref) with exactly one measured line of the same net and unit, and a line may create a lot or unit identity with a code (a unit may carry width, length and thickness). A later transfer, consumption or correction cites that identity with identity_id. A transfer line names from_location_id and to_location_id; the others name location_id. A correction names corrects_transaction_id and a reason, and its quantities are signed adjustments.\n\nIdentities are read with factory.inventory.identity.get, .positions and .draws; factory.inventory.identity.trace follows transforms backward to source receipts or forward to descendant outputs. factory.inventory.transaction.balance and .corrections show a transaction net of its corrections.\n\nProduction postings (outputs, transforms, and consumption or corrections that carry production context) belong to Production Execution and are refused here as an unregistered posting authority." }],
}) }
