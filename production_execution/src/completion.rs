//! Operation completion: validation against the order's selected revisions,
//! material hold checks, and the one transaction that posts the material
//! effects through Inventory and records the execution with its override
//! evidence. The crate's private contributed authority also serves wastage;
//! Inventory accepts production only from that static's address.
use crate::AUTHORITY;
use crate::{
    contributions::{HOLD_OVERRIDE, MANAGE},
    model::{Approver, Completion, HoldOverrideRequest, Order, Source},
    orders, Error, Result,
};
use bilibi_database::{Change, Db, MutationBoundary};
use bilibi_factory_inventory::{
    Decimal, Inventory, MaterialTransaction, PostingRequest, StockLine,
};
use bilibi_factory_product_definition::{ProductDefinition, SelectedRevisions};
use bilibi_tenancy::Scope;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, SystemTime},
};

const ACTION: &str = "factory.production_execution.complete_operation";

/// Who is recording, from the Scope alone: the user, or an agent acting for
/// the user. Callers cannot name a recorder.
#[derive(Clone, Debug, Serialize)]
struct Recorder {
    kind: &'static str,
    id: i64,
    acting_for_user_id: Option<i64>,
}
fn recorder(scope: &Scope) -> Recorder {
    match scope.agent() {
        Some(agent) => Recorder {
            kind: "agent",
            id: agent.connection(),
            acting_for_user_id: Some(scope.user()),
        },
        None => Recorder {
            kind: "user",
            id: scope.user(),
            acting_for_user_id: None,
        },
    }
}

/// A hold override that passed shape checks, before Authz decides it.
#[derive(Clone, Debug, Serialize)]
struct Override {
    approver: Option<Approver>,
    recorder: Recorder,
    reason: String,
    evidence: Option<String>,
    occurred_at: Option<SystemTime>,
}

/// A request that passed validation, in canonical form.
#[derive(Clone, Debug, Serialize)]
struct Request {
    request_id: String,
    source: Source,
    operation_code: String,
    resource_id: i64,
    operator_type: String,
    operator_id: i64,
    started_at: SystemTime,
    completed_at: SystemTime,
    inputs: Vec<StockLine>,
    outputs: Vec<StockLine>,
    variance: Option<bilibi_factory_inventory::Variance>,
    hold_override: Option<Override>,
    evidence: String,
    /// Input line index and the minimum hold in hours that governs it.
    #[serde(skip)]
    holds: Vec<(usize, i64)>,
}

/// The override evidence one held input needs recorded.
#[derive(Clone)]
struct Applied {
    approver: Option<Approver>,
    recorder: Recorder,
    reason: String,
    evidence: Option<String>,
    occurred_at: SystemTime,
    source_transaction_id: i64,
    identity_id: i64,
    item_id: i64,
}

pub(crate) struct Completer {
    pub db: Db,
    pub mutations: std::sync::Arc<MutationBoundary>,
    pub inventory: std::sync::Arc<Inventory>,
    pub product_definition: std::sync::Arc<ProductDefinition>,
}

fn nonblank(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Lines must name an item of the operation, a location and a positive
/// quantity. Only the fields an execution line carries are kept.
fn lines(lines: &[StockLine], allowed: &[i64]) -> Result<Vec<StockLine>> {
    lines
        .iter()
        .map(|line| {
            if !allowed.contains(&line.item_id)
                || line.location_id.is_none_or(|id| id <= 0)
                || line.quantity <= Decimal::ZERO
            {
                return Err(Error::InvalidLines);
            }
            Ok(StockLine {
                from_location_id: None,
                to_location_id: None,
                quantity: line.quantity.normalize(),
                ..line.clone()
            })
        })
        .collect()
}

/// A Formula input's `material_hold_rule`, or a Formula or routing
/// `process_config.material_hold_rules` entry keyed by the input's item ID;
/// the longest applicable minimum governs.
fn hold_rules(selected: &SelectedRevisions, inputs: &[StockLine]) -> Result<Vec<(usize, i64)>> {
    let mut holds = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let mut rules: Vec<Value> = selected
            .formula
            .lines
            .iter()
            .filter(|line| line.role == "input" && line.item_id == input.item_id)
            .filter_map(|line| line.material_hold_rule.clone().map(Value::Object))
            .collect();
        let key = input.item_id.to_string();
        for config in [
            &selected.formula.process_config,
            &selected.routing.process_config,
        ] {
            match config
                .get("material_hold_rules")
                .and_then(Value::as_object)
                .and_then(|rules| rules.get(&key))
            {
                None | Some(Value::Null) => {}
                Some(Value::Array(list)) => rules.extend(list.iter().cloned()),
                Some(rule) => rules.push(rule.clone()),
            }
        }
        if rules.is_empty() {
            continue;
        }
        let hours = rules
            .iter()
            .map(|rule| {
                rule.as_object()
                    .and_then(|rule| rule.get("hours"))
                    .and_then(Value::as_i64)
                    .filter(|hours| *hours > 0)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(Error::InvalidMaterialHoldRule)?;
        holds.push((index, hours.into_iter().max().unwrap_or_default()));
    }
    Ok(holds)
}

/// Shape and recorder checks for a hold override; Authz decides it later.
fn override_request(
    scope: &Scope,
    source: Source,
    completed_at: SystemTime,
    request: &HoldOverrideRequest,
) -> Result<Override> {
    if !nonblank(&request.reason) {
        return Err(Error::InvalidHoldOverride);
    }
    if scope.impersonator().is_some() {
        return Err(Error::OverrideRefusedUnderImpersonation);
    }
    let recorder = recorder(scope);
    let (approver, evidence) = match source {
        Source::Live => {
            if request.approver.is_some()
                || request.evidence.is_some()
                || request.occurred_at.is_some()
            {
                return Err(Error::InvalidHoldOverride);
            }
            (
                Some(Approver {
                    kind: recorder.kind.into(),
                    id: recorder.id,
                    acting_for_user_id: recorder.acting_for_user_id,
                }),
                None,
            )
        }
        Source::Import => {
            let evidence = request
                .evidence
                .as_deref()
                .filter(|evidence| nonblank(evidence))
                .ok_or(Error::InvalidHoldOverride)?;
            if request.occurred_at.is_none_or(|at| at > completed_at) {
                return Err(Error::InvalidHoldOverride);
            }
            if request
                .approver
                .as_ref()
                .is_some_and(|approver| !approver.valid())
            {
                return Err(Error::InvalidHoldOverride);
            }
            (request.approver.clone(), Some(evidence.trim().to_owned()))
        }
    };
    Ok(Override {
        approver,
        recorder,
        reason: request.reason.trim().to_owned(),
        evidence,
        occurred_at: request.occurred_at,
    })
}

fn validate(
    scope: &Scope,
    selected: &SelectedRevisions,
    source: Source,
    completion: &Completion,
) -> Result<(Request, Option<i64>)> {
    let operation = selected
        .routing
        .operations
        .iter()
        .find(|operation| operation.code == completion.operation_code)
        .filter(|operation| {
            operation
                .allowed_resource_ids
                .contains(&completion.resource_id)
        })
        .ok_or(Error::InvalidExecution)?;
    if completion.request_id.is_empty()
        || completion.request_id.len() > 240
        || completion.evidence.is_empty()
        || completion.operator_type.is_empty()
        || completion.operator_id < 0
        || completion.started_at > completion.completed_at
        || completion.completed_at > SystemTime::now()
    {
        return Err(Error::InvalidExecution);
    }
    let inputs = lines(&completion.inputs, &operation.inputs)?;
    let outputs = lines(&completion.outputs, &operation.outputs)?;
    let holds = hold_rules(selected, &inputs)?;
    let hold_override = completion
        .hold_override
        .as_ref()
        .map(|request| override_request(scope, source, completion.completed_at, request))
        .transpose()?;
    if (inputs.is_empty() && outputs.is_empty())
        || (completion.variance.is_some() && (inputs.is_empty() || outputs.is_empty()))
    {
        return Err(Error::InvalidExecution);
    }
    Ok((
        Request {
            request_id: completion.request_id.clone(),
            source,
            operation_code: completion.operation_code.clone(),
            resource_id: completion.resource_id,
            operator_type: completion.operator_type.clone(),
            operator_id: completion.operator_id,
            started_at: completion.started_at,
            completed_at: completion.completed_at,
            inputs,
            outputs,
            variance: completion.variance.clone(),
            hold_override,
            evidence: completion.evidence.clone(),
            holds,
        },
        Some(completion.resource_id),
    ))
}

/// A stock entry that moved stock into the identity: positive, not a
/// reversal or a variance.
fn positive(quantity: &str) -> bool {
    !quantity.starts_with('-') && quantity.bytes().any(|byte| matches!(byte, b'1'..=b'9'))
}

impl Completer {
    async fn read_client(&self) -> Result<deadpool_postgres::Object> {
        Ok(self.db.get().await.map_err(bilibi_database::Error::from)?)
    }

    /// Authorizes an import against the order's company, before anything is
    /// posted. A live completion needs no import grant.
    async fn authorize_import(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company: i64,
        source: Source,
    ) -> Result<()> {
        if source == Source::Live {
            return Ok(());
        }
        match bilibi_company::organisation::authorize_target(
            client,
            scope,
            company,
            crate::contributions::IMPORT,
        )
        .await
        {
            Err(bilibi_database::Error::Forbidden) => Err(Error::ImportNotAuthorized),
            other => Ok(other.map(|_| ())?),
        }
    }

    /// Checks every held input against its immutable source transaction. The
    /// Authz decision for an override runs here, before the posting
    /// transaction, so a refused attempt keeps its decision log.
    async fn check_holds(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company: i64,
        request: &Request,
    ) -> Result<Vec<Applied>> {
        if orders::recorded(client, scope, company, &request.request_id).await? {
            return Ok(Vec::new());
        }
        let mut seen = HashSet::new();
        let mut applied = Vec::new();
        for (index, hours) in &request.holds {
            let line = &request.inputs[*index];
            if !seen.insert((line.identity_id, line.item_id)) {
                continue;
            }
            if let Some(override_) = self
                .check_hold(client, scope, company, request, line, *hours)
                .await?
            {
                applied.push(override_);
            }
        }
        Ok(applied)
    }

    async fn check_hold(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company: i64,
        request: &Request,
        line: &StockLine,
        hours: i64,
    ) -> Result<Option<Applied>> {
        let invalid = Error::InvalidHoldSource;
        let identity_id = line.identity_id.filter(|id| *id > 0).ok_or(invalid)?;
        // A missing identity or source is a hold-source refusal; a denied
        // capability or database fault is reported as it is.
        let identity_result = if request.source == Source::Import {
            let import = self
                .inventory
                .production_import(scope, company, &AUTHORITY)?;
            self.inventory
                .get_import_identity_on(client, &import, identity_id)
                .await
        } else {
            self.inventory
                .get_identity_on(client, scope, company, identity_id)
                .await
        };
        let identity = match identity_result {
            Ok(identity) => identity,
            Err(bilibi_factory_inventory::Error::Access(error)) => return Err(error.into()),
            Err(_) => return Err(Error::InvalidHoldSource),
        };
        if identity.item_id != line.item_id {
            return Err(Error::InvalidHoldSource);
        }
        let source_result = if request.source == Source::Import {
            let import = self
                .inventory
                .production_import(scope, company, &AUTHORITY)?;
            self.inventory
                .get_import_transaction_on(client, &import, identity.source_transaction_id)
                .await
        } else {
            self.inventory
                .get_transaction_on(client, scope, company, identity.source_transaction_id)
                .await
        };
        let source = match source_result {
            Ok(source) => source,
            Err(bilibi_factory_inventory::Error::Access(error)) => return Err(error.into()),
            Err(_) => return Err(Error::InvalidHoldSource),
        };
        if !["receipt", "output", "transform"].contains(&source.kind.as_str())
            || !source.entries.iter().any(|entry| {
                entry.identity_id == Some(identity_id)
                    && entry.role == "stock"
                    && entry.item_id == Some(line.item_id)
                    && positive(&entry.native_quantity)
            })
            || source.effective_at > request.completed_at
        {
            return Err(Error::InvalidHoldSource);
        }
        let age = request
            .completed_at
            .duration_since(source.effective_at)
            .unwrap_or_default();
        if age >= Duration::from_secs(hours as u64 * 3600) {
            return Ok(None);
        }
        let override_ = request.hold_override.as_ref().ok_or(Error::MaterialHeld)?;
        match bilibi_company::organisation::authorize_target(client, scope, company, HOLD_OVERRIDE)
            .await
        {
            Ok(_) => Ok(Some(Applied {
                approver: override_.approver.clone(),
                recorder: override_.recorder.clone(),
                reason: override_.reason.clone(),
                evidence: override_.evidence.clone(),
                occurred_at: override_.occurred_at.unwrap_or_else(SystemTime::now),
                source_transaction_id: identity.source_transaction_id,
                identity_id,
                item_id: line.item_id,
            })),
            Err(bilibi_database::Error::Forbidden) => Err(Error::HoldOverrideDenied),
            Err(error) => Err(error.into()),
        }
    }

    /// Posts the execution's material effects through the Inventory function
    /// its lines call for, on the open transaction.
    async fn post(
        &self,
        tx: &mut bilibi_database::MutationTransaction<'_>,
        scope: &Scope,
        company: i64,
        order: &Order,
        request: &Request,
    ) -> Result<MaterialTransaction> {
        let mut posting = PostingRequest {
            request_id: format!("pe:{}", request.request_id),
            evidence: request.evidence.clone(),
            effective_at: Some(request.completed_at),
            context: BTreeMap::from([
                ("operation_execution".to_owned(), request.request_id.clone()),
                ("order_or_batch".to_owned(), order.code.clone()),
                ("work_centre".to_owned(), request.resource_id.to_string()),
            ]),
            ..PostingRequest::default()
        };
        let inventory = &self.inventory;
        if request.source == Source::Import {
            posting.variance.clone_from(&request.variance);
            let import = inventory.production_import(scope, company, &AUTHORITY)?;
            return Ok(inventory
                .record_import_on(tx, &import, &request.inputs, &request.outputs, &posting)
                .await?);
        }
        Ok(
            match (request.inputs.is_empty(), request.outputs.is_empty()) {
                (false, false) => {
                    posting.inputs.clone_from(&request.inputs);
                    posting.outputs.clone_from(&request.outputs);
                    posting.variance.clone_from(&request.variance);
                    inventory
                        .record_transform_on(tx, scope, company, &posting, &AUTHORITY)
                        .await?
                }
                (true, false) => {
                    posting.lines.clone_from(&request.outputs);
                    inventory
                        .record_output_on(tx, scope, company, &posting, &AUTHORITY)
                        .await?
                }
                _ => {
                    posting.lines.clone_from(&request.inputs);
                    inventory
                        .record_production_consumption_on(tx, scope, company, &posting, &AUTHORITY)
                        .await?
                }
            },
        )
    }

    pub(crate) async fn complete(
        &self,
        scope: &Scope,
        company: i64,
        order_id: i64,
        source: Source,
        completion: &Completion,
    ) -> Result<crate::Execution> {
        // Preserve durable refusal decisions before opening the write
        // transaction. Nothing from this preflight authorizes that write.
        {
            let client = self.read_client().await?;
            self.preflight(&**client, scope, company, order_id, source, completion)
                .await?;
        }
        let mut client = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(&mut client, scope.mutation_context(ACTION))
            .await?;
        let result = self
            .complete_on(&mut tx, scope, company, order_id, source, completion)
            .await;
        match result {
            Ok((execution, true)) => {
                tx.commit().await?;
                Ok(execution)
            }
            // A retry of a recorded request wrote nothing: nothing to audit.
            Ok((execution, false)) => {
                tx.rollback().await?;
                Ok(execution)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }

    /// Keep refusal decisions on an autocommit connection. The transaction
    /// repeats these checks; this result never authorizes a later write.
    pub(crate) async fn preflight(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company: i64,
        order_id: i64,
        source: Source,
        completion: &Completion,
    ) -> Result<()> {
        let (_, request) = self
            .prepare(client, scope, company, order_id, source, completion)
            .await?;
        self.check_holds(client, scope, company, &request).await?;
        Ok(())
    }

    async fn prepare(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company: i64,
        order_id: i64,
        source: Source,
        completion: &Completion,
    ) -> Result<(Order, Request)> {
        self.authorize_import(client, scope, company, source)
            .await?;
        if source == Source::Live {
            bilibi_authz::authorize_key(client, scope, crate::contributions::FLOOR_VIEW).await?;
        }
        bilibi_company::organisation::require_live(client, scope, company).await?;
        let order = orders::get_order(client, scope, company, order_id).await?;
        let selected = if source == Source::Import {
            let import = self
                .inventory
                .production_import(scope, company, &AUTHORITY)?;
            self.product_definition
                .select_import_revisions_on(
                    client,
                    &import,
                    order.product_id,
                    order.formula_version,
                    order.routing_version,
                )
                .await?
        } else {
            self.product_definition
                .select_revisions_on(
                    client,
                    scope,
                    company,
                    order.product_id,
                    order.formula_version,
                    order.routing_version,
                )
                .await?
        };
        let (request, resource) = validate(scope, &selected, source, completion)?;
        if let Some(resource) = resource {
            if source == Source::Import {
                let import = self
                    .inventory
                    .production_import(scope, company, &AUTHORITY)?;
                self.product_definition
                    .get_import_resource_on(client, &import, resource)
                    .await?;
            } else {
                self.product_definition
                    .get_resource_on(client, scope, company, resource)
                    .await?;
            }
        }
        Ok((order, request))
    }

    /// The single owner path for facade, agent preview and plan commit. It
    /// owns no transaction lifecycle and reacquires all authorization and
    /// hold facts on the caller's connection under the writable-company lock.
    pub(crate) async fn complete_on(
        &self,
        tx: &mut bilibi_database::MutationTransaction<'_>,
        scope: &Scope,
        company: i64,
        order_id: i64,
        source: Source,
        completion: &Completion,
    ) -> Result<(crate::Execution, bool)> {
        if source == Source::Live {
            bilibi_company::organisation::authorize_target(tx.sql(), scope, company, MANAGE)
                .await?;
        }
        self.authorize_import(tx.sql(), scope, company, source)
            .await?;
        bilibi_company::organisation::require_writable(tx.sql(), scope, company, true).await?;
        let (order, request) = self
            .prepare(tx.sql(), scope, company, order_id, source, completion)
            .await?;
        let overrides = self.check_holds(tx.sql(), scope, company, &request).await?;
        self.commit(tx, scope, company, &order, &request, &overrides)
            .await
    }

    /// The execution, and whether this call recorded it (`false` for an
    /// identical retry of a recorded request).
    async fn commit(
        &self,
        tx: &mut bilibi_database::MutationTransaction<'_>,
        scope: &Scope,
        company: i64,
        order: &Order,
        request: &Request,
        overrides: &[Applied],
    ) -> Result<(crate::Execution, bool)> {
        use bilibi_company::organisation::{authorize_target, require_writable};
        if request.source == Source::Live {
            authorize_target(tx.sql(), scope, company, MANAGE).await?;
        }
        if request.source == Source::Import {
            match authorize_target(tx.sql(), scope, company, crate::contributions::IMPORT).await {
                Err(bilibi_database::Error::Forbidden) => return Err(Error::ImportNotAuthorized),
                other => other.map(|_| ())?,
            }
        }
        // The company row lock also serializes every Inventory posting.
        require_writable(tx.sql(), scope, company, true).await?;
        tx.sql()
            .query_opt(
                "SELECT id FROM factory_production_orders WHERE tenant_id=$1 AND company_id=$2 AND id=$3 FOR UPDATE",
                &[&scope.tenant(), &company, &order.id],
            )
            .await?
            .ok_or(Error::OrderNotFound)?;
        // SystemTime's serde representation cannot encode pre-epoch times.
        // Refuse fallibly, retaining the fingerprint of every valid retry.
        let serialized = serde_json::to_value(request).map_err(|_| Error::InvalidExecution)?;
        let fingerprint: String = tx
            .sql()
            .query_one(
                "SELECT encode(sha256(convert_to($1::jsonb::text,'UTF8')),'hex')",
                &[&json!({"order_id":order.id,"request":serialized})],
            )
            .await?
            .get(0);
        let existing = tx
            .sql()
            .query_opt(
                &format!("SELECT {} FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=$2 AND request_id=$3", orders::EXECUTION_COLUMNS),
                &[&scope.tenant(), &company, &request.request_id],
            )
            .await?;
        if let Some(row) = existing {
            let (execution, stored) = orders::execution(&row)?;
            if stored != fingerprint {
                return Err(Error::RequestIdConflict);
            }
            return Ok((execution, false));
        }
        let transaction = self.post(tx, scope, company, order, request).await?;
        let (tenant, source) = (scope.tenant(), request.source.name());
        let inputs = serde_json::to_value(&request.inputs)?;
        let outputs = serde_json::to_value(&request.outputs)?;
        let variance = request
            .variance
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let (request, overrides) = (request.clone(), overrides.to_vec());
        let (order_id, transaction_id) = (order.id, transaction.id);
        let (formula, routing) = (order.formula_version, order.routing_version);
        let execution_id = tx
            .mutate(|sql| {
                Box::pin(async move {
                    let row = sql
                        .query_one(
                            "INSERT INTO factory_operation_executions(tenant_id,company_id,order_id,request_id,request_fingerprint,source,operation_code,resource_id,operator_type,operator_id,started_at,completed_at,inputs,outputs,variance,evidence,inventory_transaction_id,formula_version,routing_version) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19) RETURNING id,to_jsonb(factory_operation_executions) AS saved",
                            &[&tenant, &company, &order_id, &request.request_id, &fingerprint, &source, &request.operation_code, &request.resource_id, &request.operator_type, &request.operator_id, &request.started_at, &request.completed_at, &inputs, &outputs, &variance, &request.evidence, &transaction_id, &formula, &routing],
                        )
                        .await?;
                    let id: i64 = row.get("id");
                    let mut changes = vec![Change {
                        kind: "factory_operation_executions".into(),
                        id,
                        old: json!({}),
                        new: row.get("saved"),
                    }];
                    for applied in &overrides {
                        let (actor_type, actor_id, acting_for) = applied.approver.as_ref().map_or((None, None, None), |a| (Some(a.kind.as_str()), Some(a.id), a.acting_for_user_id));
                        let row = sql
                            .query_one(
                                "INSERT INTO factory_material_hold_overrides(tenant_id,company_id,execution_id,inventory_transaction_id,source_transaction_id,identity_id,item_id,source,actor_type,actor_id,acting_for_user_id,recorded_by_type,recorded_by_id,recorded_by_acting_for_user_id,reason,evidence,occurred_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) RETURNING id,to_jsonb(factory_material_hold_overrides) AS saved",
                                &[&tenant, &company, &id, &transaction_id, &applied.source_transaction_id, &applied.identity_id, &applied.item_id, &source, &actor_type, &actor_id, &acting_for, &applied.recorder.kind, &applied.recorder.id, &applied.recorder.acting_for_user_id, &applied.reason, &applied.evidence, &applied.occurred_at],
                            )
                            .await?;
                        changes.push(Change {
                            kind: "factory_material_hold_overrides".into(),
                            id: row.get("id"),
                            old: json!({}),
                            new: row.get("saved"),
                        });
                    }
                    Ok((id, changes))
                })
            })
            .await?;
        Ok((
            orders::get_execution(tx.sql(), scope, company, execution_id).await?,
            true,
        ))
    }
}
