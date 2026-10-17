//! Headless reach for Bilimbi ProductionExecution's facade-only writes.
//! Previews execute the owner path and roll back; commits share the plan's
//! MutationTransaction. No input or handler can supply a posting authority.
use crate::{
    contributions::{FLOOR_VIEW, MANAGE},
    orders, Approver, Completion, Error, Execution, HoldOverride, HoldOverrideRequest, NewOrder,
    Order, OrderKind, ProductionExecution, Source,
};
use bilibi_agent_api::{
    Definitions, Guide, Handler, Operation, OperationClient, OperationFuture, ReadHandler,
    WriteHandler, DEFINITIONS,
};
use bilibi_database::MutationTransaction;
use bilibi_module_registry::{Contribution, Provenance};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
    time::SystemTime,
};

type Refusal = bilibi_agent_api::Error;
type Result<T> = std::result::Result<T, Refusal>;
static BOUND: OnceLock<Arc<ProductionExecution>> = OnceLock::new();
pub(crate) fn bind(owner: Arc<ProductionExecution>) -> bilibi_module_registry::Result<()> {
    BOUND.set(owner).map_err(|_| {
        bilibi_module_registry::RegistryError::Service("duplicate production agent binding".into())
    })
}
fn owner() -> Result<&'static ProductionExecution> {
    BOUND
        .get()
        .map(Arc::as_ref)
        .ok_or(Refusal::Backend(bilibi_database::Error::Internal))
}
fn invalid(field: impl Into<String>, message: impl Into<String>) -> Refusal {
    Refusal::InvalidInput(BTreeMap::from([(field.into(), vec![message.into()])]))
}
fn refuse(error: Error) -> Refusal {
    match error {
        Error::Access(error) => Refusal::Backend(error),
        Error::Inventory(bilibi_factory_inventory::Error::Access(error))
        | Error::ProductDefinition(bilibi_factory_product_definition::Error::Access(error)) => {
            Refusal::Backend(error)
        }
        Error::RequestIdConflict => invalid("request_id", error.to_string()),
        Error::ImportNotAuthorized | Error::HoldOverrideDenied => Refusal::Forbidden,
        _ => invalid("request", error.to_string()),
    }
}
fn id(input: &Value, field: &str) -> Result<i64> {
    input[field]
        .as_i64()
        .filter(|id| *id > 0)
        .ok_or_else(|| invalid(field, "must be a positive integer"))
}
fn text(input: &Value, field: &str) -> String {
    input[field].as_str().unwrap_or_default().into()
}
fn instant(at: SystemTime) -> String {
    jiff::Timestamp::try_from(at)
        .expect("database timestamp in range")
        .to_string()
}
fn time(input: &Value, field: &str) -> Result<SystemTime> {
    input[field]
        .as_str()
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .map(SystemTime::from)
        .ok_or_else(|| invalid(field, "must be a date-time with offset"))
}
fn new_order(input: &Value) -> Result<NewOrder> {
    Ok(NewOrder {
        code: text(input, "code"),
        kind: OrderKind::parse(&text(input, "kind"))
            .ok_or_else(|| invalid("kind", "must be order or batch"))?,
        product_id: id(input, "product_id")?,
        formula_version: i32::try_from(id(input, "formula_version")?)
            .map_err(|_| invalid("formula_version", "is too large"))?,
        routing_version: i32::try_from(id(input, "routing_version")?)
            .map_err(|_| invalid("routing_version", "is too large"))?,
        demand_ref: input["demand_ref"].as_str().map(str::to_owned),
    })
}
fn completion(input: &Value) -> Result<(Source, Completion)> {
    let source = match input["source"].as_str() {
        Some("live") => Source::Live,
        Some("import") => Source::Import,
        _ => return Err(invalid("source", "must be live or import")),
    };
    let parse = |field: &str| {
        serde_json::from_value(input[field].clone())
            .map_err(|error| invalid(field, format!("is not valid: {error}")))
    };
    let hold_override = input
        .get("hold_override")
        .map(|hold| -> Result<_> {
            Ok(HoldOverrideRequest {
                reason: text(hold, "reason"),
                evidence: hold["evidence"].as_str().map(str::to_owned),
                occurred_at: hold
                    .get("occurred_at")
                    .map(|_| time(hold, "occurred_at"))
                    .transpose()?,
                approver: hold
                    .get("approver")
                    .map(|v| {
                        serde_json::from_value::<Approver>(v.clone())
                            .map_err(|e| invalid("hold_override.approver", e.to_string()))
                    })
                    .transpose()?,
            })
        })
        .transpose()?;
    Ok((
        source,
        Completion {
            request_id: text(input, "request_id"),
            operation_code: text(input, "operation_code"),
            resource_id: id(input, "resource_id")?,
            operator_type: text(input, "operator_type"),
            operator_id: input["operator_id"]
                .as_i64()
                .ok_or_else(|| invalid("operator_id", "must be an integer"))?,
            started_at: time(input, "started_at")?,
            completed_at: time(input, "completed_at")?,
            evidence: text(input, "evidence"),
            inputs: parse("inputs")?,
            outputs: parse("outputs")?,
            variance: input
                .get("variance")
                .map(|v| {
                    serde_json::from_value(v.clone())
                        .map_err(|e| invalid("variance", e.to_string()))
                })
                .transpose()?,
            hold_override,
        },
    ))
}
fn order_json(order: &Order) -> Value {
    json!({"id":order.id,"company_id":order.company_id,"code":order.code,"kind":order.kind,
        "product_id":order.product_id,"formula_version":order.formula_version,
        "routing_version":order.routing_version,"demand_ref":order.demand_ref})
}
fn execution_json(run: &Execution) -> Value {
    json!({"id":run.id,"company_id":run.company_id,"order_id":run.order_id,"request_id":run.request_id,
        "source":run.source,"operation_code":run.operation_code,"resource_id":run.resource_id,
        "operator_type":run.operator_type,"operator_id":run.operator_id,
        "started_at":instant(run.started_at),"completed_at":instant(run.completed_at),
        "inputs":run.inputs,"outputs":run.outputs,"variance":run.variance,"evidence":run.evidence,
        "inventory_transaction_id":run.inventory_transaction_id,
        "formula_version":run.formula_version,"routing_version":run.routing_version})
}
fn override_json(hold: &HoldOverride) -> Value {
    json!({"id":hold.id,"execution_id":hold.execution_id,"inventory_transaction_id":hold.inventory_transaction_id,
        "source_transaction_id":hold.source_transaction_id,"identity_id":hold.identity_id,"item_id":hold.item_id,
        "source":hold.source,"actor_type":hold.actor_type,"actor_id":hold.actor_id,
        "acting_for_user_id":hold.acting_for_user_id,"recorded_by_type":hold.recorded_by_type,
        "recorded_by_id":hold.recorded_by_id,"recorded_by_acting_for_user_id":hold.recorded_by_acting_for_user_id,
        "reason":hold.reason,"evidence":hold.evidence,"occurred_at":instant(hold.occurred_at)})
}

enum Read {
    Orders,
    Order,
    Executions,
    Execution,
    Overrides,
}
impl ReadHandler for Read {
    fn owner(&self) -> &'static str {
        "factory/production_execution"
    }
    fn call<'a>(
        &'a self,
        _client: &'a dyn OperationClient,
        scope: &'a Scope,
        input: &'a Value,
    ) -> OperationFuture<'a> {
        Box::pin(async move {
            let owner = owner()?;
            let company = id(input, "company_id")?;
            let limit = input.get("limit").map(|_| id(input, "limit")).transpose()?;
            let value = match self {
                Self::Orders => json!(owner
                    .list_orders(scope, company, limit)
                    .await
                    .map_err(refuse)?
                    .iter()
                    .map(order_json)
                    .collect::<Vec<_>>()),
                Self::Order => order_json(
                    &owner
                        .get_order(scope, company, id(input, "order_id")?)
                        .await
                        .map_err(refuse)?,
                ),
                Self::Executions => json!(owner
                    .list_executions(scope, company, id(input, "order_id")?, limit)
                    .await
                    .map_err(refuse)?
                    .iter()
                    .map(execution_json)
                    .collect::<Vec<_>>()),
                Self::Execution => execution_json(
                    &owner
                        .get_execution(scope, company, id(input, "execution_id")?)
                        .await
                        .map_err(refuse)?,
                ),
                Self::Overrides => json!(owner
                    .list_hold_overrides(scope, company, id(input, "execution_id")?)
                    .await
                    .map_err(refuse)?
                    .iter()
                    .map(override_json)
                    .collect::<Vec<_>>()),
            };
            Ok(value.into())
        })
    }
}

enum Write {
    Order,
    Complete,
}
impl Write {
    async fn execute(
        &self,
        tx: &mut MutationTransaction<'_>,
        scope: &Scope,
        input: &Value,
        review: bool,
    ) -> Result<Value> {
        let owner = owner()?;
        let company = id(input, "company_id")?;
        match self {
            Self::Order => {
                let order = owner
                    .create_order_on(tx, scope, company, &new_order(input)?)
                    .await
                    .map_err(refuse)?;
                let mut value = order_json(&order);
                if review {
                    value.as_object_mut().expect("object").remove("id");
                }
                Ok(
                    json!({"summary":"Create a production order with exact definition revisions", "order":value}),
                )
            }
            Self::Complete => {
                let (source, completion) = completion(input)?;
                // A review cannot authorize an unspecified future conversion.
                // Native units are immutable; non-native conversions must be pinned.
                for line in completion
                    .inputs
                    .iter()
                    .chain(&completion.outputs)
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    if let Some(unit) = line.unit_id {
                        let material = owner
                            .completer
                            .inventory
                            .get_material_on(tx.sql(), scope, company, line.item_id)
                            .await
                            .map_err(|e| refuse(e.into()))?;
                        if unit != material.native_unit.id && line.conversion_version.is_none() {
                            return Err(invalid(
                                "conversion_version",
                                "pin the conversion version for every non-native unit",
                            ));
                        }
                    }
                }
                let (run, created) = owner
                    .completer
                    .complete_on(
                        tx,
                        scope,
                        company,
                        id(input, "order_id")?,
                        source,
                        &completion,
                    )
                    .await
                    .map_err(refuse)?;
                let posting = owner
                    .completer
                    .inventory
                    .get_transaction_on(tx.sql(), scope, company, run.inventory_transaction_id)
                    .await
                    .map_err(|e| refuse(e.into()))?;
                let holds = orders::hold_overrides(tx.sql(), scope, company, run.id)
                    .await
                    .map_err(refuse)?;
                let mut entries = Vec::new();
                let existing: Vec<i64> = run
                    .inputs
                    .iter()
                    .chain(&run.outputs)
                    .filter_map(|line| line.identity_id)
                    .collect();
                for entry in &posting.entries {
                    let mut value = serde_json::to_value(entry)
                        .map_err(|_| Refusal::Backend(bilibi_database::Error::Internal))?;
                    if review {
                        value.as_object_mut().expect("object").remove("id");
                        if let Some(identity) =
                            entry.identity_id.filter(|id| !existing.contains(id))
                        {
                            let identity = owner
                                .completer
                                .inventory
                                .get_identity_on(tx.sql(), scope, company, identity)
                                .await
                                .map_err(|e| refuse(e.into()))?;
                            value["identity_id"] = json!({"created":{"kind":identity.kind,"code":identity.code,"dimensions":identity.dimensions}});
                        }
                    }
                    entries.push(value);
                }
                let mut overrides: Vec<Value> = holds.iter().map(override_json).collect();
                if review {
                    for hold in &mut overrides {
                        for key in ["id", "execution_id", "inventory_transaction_id"] {
                            hold.as_object_mut().expect("object").remove(key);
                        }
                        if source == Source::Live && created {
                            hold["occurred_at"] = json!("assigned when recorded");
                        }
                    }
                }
                let mut execution = execution_json(&run);
                if review {
                    for key in ["id", "inventory_transaction_id"] {
                        execution.as_object_mut().expect("object").remove(key);
                    }
                }
                Ok(
                    json!({"summary":if created {"Complete the operation and post its material effects atomically"} else {"Repeat a recorded request; no new material effects"},
                    "replay":!created,"execution":execution,"posting_kind":posting.kind,"entries":entries,"hold_overrides":overrides}),
                )
            }
        }
    }
}
impl WriteHandler for Write {
    fn owner(&self) -> &'static str {
        "factory/production_execution"
    }
    fn preview<'a>(
        &'a self,
        _client: &'a dyn OperationClient,
        scope: &'a Scope,
        input: &'a Value,
    ) -> OperationFuture<'a> {
        Box::pin(async move {
            let owner = owner()?;
            let mut client = owner.db.get().await.map_err(bilibi_database::Error::from)?;
            let mut tx = owner
                .mutations
                .begin(
                    &mut client,
                    scope.mutation_context("factory.production_execution.preview"),
                )
                .await?;
            let result = self.execute(&mut tx, scope, input, true).await;
            tx.rollback().await?;
            Ok(result?.into())
        })
    }
    fn commit<'a>(
        &'a self,
        tx: &'a mut MutationTransaction<'_>,
        scope: &'a Scope,
        input: &'a Value,
    ) -> OperationFuture<'a> {
        Box::pin(async move { Ok(self.execute(tx, scope, input, false).await?.into()) })
    }
}

fn object(required: &[&str], properties: Value) -> Value {
    json!({"type":"object","required":required,"properties":properties})
}
fn ident() -> Value {
    json!({"type":"integer","minimum":1})
}
fn string() -> Value {
    json!({"type":"string","maxLength":10000})
}
fn date_time() -> Value {
    json!({"type":"string","format":"date-time"})
}
fn company() -> Value {
    object(
        &["company_id"],
        json!({"company_id":ident(),"limit":{"type":"integer","minimum":1,"maximum":500}}),
    )
}
fn order_id() -> Value {
    object(
        &["company_id", "order_id"],
        json!({"company_id":ident(),"order_id":ident(),"limit":{"type":"integer","minimum":1,"maximum":500}}),
    )
}
fn execution_id() -> Value {
    object(
        &["company_id", "execution_id"],
        json!({"company_id":ident(),"execution_id":ident()}),
    )
}
fn order_input() -> Value {
    object(
        &[
            "company_id",
            "code",
            "kind",
            "product_id",
            "formula_version",
            "routing_version",
        ],
        json!({
        "company_id":ident(),"code":{"type":"string","maxLength":255},"kind":{"type":"string","enum":["order","batch"]},
        "product_id":ident(),"formula_version":{"type":"integer","minimum":1,"maximum":2147483647},
        "routing_version":{"type":"integer","minimum":1,"maximum":2147483647},"demand_ref":{"type":"string","maxLength":255}}),
    )
}
fn dimension() -> Value {
    object(
        &["value", "unit", "provenance"],
        json!({"value":{"type":"string","maxLength":40},
        "unit":{"type":"string","enum":["um","mm","cm","m"]},"provenance":{"type":"string","enum":["measured","nominal"]}}),
    )
}
fn stock_line(output: bool) -> Value {
    let mut fields = json!({"item_id":ident(),"location_id":ident(),"quantity":{"type":"string","maxLength":40,"description":"Positive exact decimal string."},
        "unit_id":ident(),"conversion_version":{"type":"integer","minimum":1,"maximum":2147483647,"description":"Required for non-native units; pins the reviewed conversion."},
        "observation":{"type":"string","enum":["measured","declared","counted","derived"]},"evidence":string(),"identity_id":ident()});
    if output {
        fields["output_role"] = json!({"type":"string","maxLength":255});
        fields["identity"] = object(
            &["kind", "code"],
            json!({"kind":{"type":"string","enum":["lot","unit"]},"code":{"type":"string","maxLength":255},
            "dimensions":{"type":"object","properties":{"width":dimension(),"length":dimension(),"thickness":dimension()}}}),
        );
    }
    object(
        &["item_id", "location_id", "quantity", "observation"],
        fields,
    )
}
fn complete_input() -> Value {
    let variance = object(
        &["evidence", "reconciliation_basis"],
        json!({"evidence":string(),"reconciliation_basis":string()}),
    );
    let mut unit_variance = variance.clone();
    unit_variance["required"] = json!(["unit_id", "evidence", "reconciliation_basis"]);
    unit_variance["properties"]["unit_id"] = ident();
    object(
        &[
            "company_id",
            "order_id",
            "source",
            "request_id",
            "operation_code",
            "resource_id",
            "operator_type",
            "operator_id",
            "started_at",
            "completed_at",
            "evidence",
            "inputs",
            "outputs",
        ],
        json!({
            "company_id":ident(),"order_id":ident(),"source":{"type":"string","enum":["live","import"]},
            "request_id":{"type":"string","maxLength":240},"operation_code":{"type":"string","maxLength":255},"resource_id":ident(),
            "operator_type":{"type":"string","maxLength":255,"description":"Work attribution, never the authenticated recorder."},"operator_id":{"type":"integer","minimum":0},
            "started_at":date_time(),"completed_at":date_time(),"evidence":string(),
            "inputs":{"type":"array","items":stock_line(false)},"outputs":{"type":"array","items":stock_line(true)},
            "variance":{"type":"object","description":"Exactly one of shared or units, only with both input and output lines.","properties":{"shared":variance,"units":{"type":"array","items":unit_variance}}},
            "hold_override":object(&["reason"], json!({"reason":string(),"evidence":string(),"occurred_at":date_time(),
                "approver":object(&["kind","id"],json!({"kind":{"type":"string","enum":["user","agent"]},"id":ident(),"acting_for_user_id":ident()}))}))
        }),
    )
}
fn record_output() -> Value {
    json!({"type":"object","description":"Order or execution with exact decimal strings, RFC3339 instants and committed IDs. Completion also returns signed posting entries, resolved identity IDs, replay and hold evidence."})
}
fn list_output() -> Value {
    json!({"type":"array","description":"Company-scoped orders, executions or immutable hold-override evidence."})
}

macro_rules! read {
    ($key:literal,$title:literal,$input:ident,$output:ident,$handler:ident) => {
        Operation {
            key: $key,
            title: $title,
            summary: $title,
            keywords: &["production", "order", "operation", "run", "hold"],
            capability: FLOOR_VIEW,
            input: $input,
            output: $output,
            handler: Handler::Read(&Read::$handler),
        }
    };
}
inventory::submit! { Contribution::new(DEFINITIONS,"factory/production_execution",Provenance {
    module:"factory/production_execution",source:file!(),line:line!(),
}, &Definitions {
    operations:&[
        read!("factory.production_execution.order.list","List production orders",company,list_output,Orders),
        read!("factory.production_execution.order.get","Read a production order",order_id,record_output,Order),
        read!("factory.production_execution.execution.list","List an order's completed operations",order_id,list_output,Executions),
        read!("factory.production_execution.execution.get","Read a completed operation",execution_id,record_output,Execution),
        read!("factory.production_execution.hold_override.list","Read an execution's material hold overrides",execution_id,list_output,Overrides),
        Operation {key:"factory.production_execution.order.create",title:"Create a production order",summary:"Create an order or batch with exact published formula and routing versions.",keywords:&["production","order","batch","create"],capability:MANAGE,input:order_input,output:record_output,handler:Handler::SafeWrite(&Write::Order)},
        Operation {key:"factory.production_execution.operation.complete",title:"Complete a production operation",summary:"Record live or imported completion, material effects and hold evidence in one audited transaction, idempotently by request_id.",keywords:&["production","operation","complete","import","run","hold","override"],capability:MANAGE,input:complete_input,output:record_output,handler:Handler::SafeWrite(&Write::Complete)},
    ],
    guides:&[Guide {key:"factory.production_execution",title:"Production operations",summary:"Create orders, complete routed operations and read hold evidence.",keywords:&["production","order","operation","run","import","hold"],capability:FLOOR_VIEW,
        body:"Every operation names company_id. Find orders with order.list and order.get; find completed runs with execution.list and execution.get (all under factory.production_execution). order.create and operation.complete are reviewed safe writes: preview, obtain the person's approval, then commit the exact inputs with the single-use plan code. Direct writes are refused. Creating an order requires exact published product_id/formula_version/routing_version; its code is company-unique, not an idempotency key. Commit it before completing its generated order_id.\n\nCompletion source is live or import; both use the same validations. Import additionally requires factory.production-execution.import, including on retries. Writes require configuration.manage and the Product Definition and Inventory view/manage grants used by the facade; reads require floor.view. operator_type/operator_id attribute the work, never authorize it. started_at and completed_at have offsets; completion cannot be in the future. Inputs and outputs name allowed operation items and a resource allowed by its routing. At least one side is nonempty. Quantities are exact decimal strings. Omitted unit_id means the immutable native unit; every non-native unit requires an explicit conversion_version so a later conversion cannot change the review. A transform's variance is either {shared:{evidence,reconciliation_basis}} or {units:[{unit_id,evidence,reconciliation_basis}]}.\n\nOutput lines may create a lot/unit identity with kind, code and dimensions; later lines cite identity_id. Previews use symbolic created identities, never predicted IDs. Commit returns resolved identity IDs in entries; inventory_transaction_id also leads to factory.inventory.transaction.get.\n\nHeld inputs must name an identity with valid source evidence. An early draw requires material-hold.override. A live hold_override carries only reason: recorder and approver come from Scope and its time is assigned at recording. Import also requires historical occurred_at (not after completion) and evidence, and may name an historical approver (kind user with id, or agent with id and acting_for_user_id). Historical approver is evidence only. No caller may name the recorder; impersonated overrides are refused. hold_override.list returns the immutable evidence.\n\nCompletion request_id is company-unique (240 bytes). Identical retries return the original without reposting; different content conflicts. Import authorization is always rechecked, while recorded retries do not reauthorize historical holds. Plans commit all operations or none, but preview each against committed data; commit newly generated IDs in an earlier plan. Multiple individually feasible draws can fail collectively, rolling back the whole plan. Reviews execute the real owner path then roll back business and audit writes; PostgreSQL sequence allocations may leave gaps."}],
}) }
