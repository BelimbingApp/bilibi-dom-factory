//! Mirrors Bilimbi ProductionExecution.Trace and ProductionExecution.Yield.
//! Inventory owns all material reads; PostgreSQL sums facade-provided exact text.
use crate::{orders, Execution, OrderKind, ProductionExecution, Result, Source};
use bilibi_factory_inventory::{CrossUnitBalance, Identity, ProductionImport, Trace, Unit};
use bilibi_factory_product_definition::Resource;
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::SystemTime,
};

#[derive(Clone, Debug)]
pub struct TraceOrder {
    pub id: i64,
    pub code: String,
    pub kind: OrderKind,
    pub product_id: i64,
    pub formula_version: i32,
    pub routing_version: i32,
}

#[derive(Clone, Debug)]
pub struct TraceRun {
    pub id: i64,
    pub order: TraceOrder,
    pub operation_code: String,
    pub resource: Resource,
    pub started_at: SystemTime,
    pub completed_at: SystemTime,
    pub source: Source,
    pub inventory_transaction_id: i64,
}

#[derive(Clone, Debug)]
pub struct ProductionTrace {
    pub material: Trace,
    pub runs: Vec<TraceRun>,
}

/// Exact native quantities. Unit subtotals are present only for a unit read;
/// shared run effects are never allocated to an identity.
#[derive(Clone, Debug)]
pub struct YieldBalance {
    pub unit: Unit,
    pub input: String,
    pub product: String,
    pub trim: String,
    pub waste: String,
    pub wastage: String,
    pub variance: String,
    pub unit_input: Option<String>,
    pub unit_output: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunYield {
    pub execution_id: i64,
    pub transaction_id: i64,
    pub operation_code: String,
    pub resource_id: i64,
    pub corrected: bool,
    pub correction_transaction_ids: Vec<i64>,
    pub wastage_transaction_ids: Vec<i64>,
    pub balances: Vec<YieldBalance>,
    pub cross_unit: Vec<CrossUnitBalance>,
}

#[derive(Clone, Debug)]
pub struct UnitYield {
    pub identity: Identity,
    pub runs: Vec<RunYield>,
}

impl ProductionExecution {
    pub async fn trace_backward(
        &self,
        scope: &Scope,
        company: i64,
        identity: i64,
    ) -> Result<ProductionTrace> {
        self.production_trace(scope, company, identity, false, None)
            .await
    }

    pub async fn trace_forward(
        &self,
        scope: &Scope,
        company: i64,
        identity: i64,
    ) -> Result<ProductionTrace> {
        self.production_trace(scope, company, identity, true, None)
            .await
    }

    // Import reads substitute only the authorization path. Both callers use
    // the same trace traversal, correction chains and PostgreSQL yield sums.
    async fn outcome_client(
        &self,
        scope: &Scope,
        company: i64,
        import: Option<&ProductionImport<'_>>,
    ) -> Result<deadpool_postgres::Object> {
        if let Some(import) = import {
            let client = self.db.get().await.map_err(bilibi_database::Error::from)?;
            import.authorize_on(&**client).await?;
            Ok(client)
        } else {
            self.read_client(scope, company).await
        }
    }

    async fn outcome_corrections(
        &self,
        scope: &Scope,
        company: i64,
        transaction: i64,
        import: Option<&ProductionImport<'_>>,
    ) -> Result<Vec<bilibi_factory_inventory::MaterialTransaction>> {
        let inventory = &self.completer.inventory;
        Ok(if let Some(import) = import {
            let client = self.outcome_client(scope, company, Some(import)).await?;
            inventory
                .list_import_corrections_on(&**client, import, transaction)
                .await?
        } else {
            inventory
                .list_corrections(scope, company, transaction)
                .await?
        })
    }

    pub(crate) async fn production_trace(
        &self,
        scope: &Scope,
        company: i64,
        identity: i64,
        forward: bool,
        import: Option<&ProductionImport<'_>>,
    ) -> Result<ProductionTrace> {
        drop(self.outcome_client(scope, company, import).await?);
        let inventory = &self.completer.inventory;
        let material = if let Some(import) = import {
            let client = self.outcome_client(scope, company, Some(import)).await?;
            inventory
                .trace_import_backward_on(&**client, import, identity)
                .await?
        } else if forward {
            inventory.trace_forward(scope, company, identity).await?
        } else {
            inventory.trace_backward(scope, company, identity).await?
        };
        let mut ids: BTreeSet<i64> = material
            .identities
            .iter()
            .map(|i| i.source_transaction_id)
            .chain(material.links.iter().map(|l| l.transaction_id))
            .collect();
        if forward {
            for identity in &material.identities {
                let draws = inventory
                    .list_identity_draws(scope, company, identity.id)
                    .await?;
                ids.extend(draws.into_iter().map(|t| t.id));
            }
        }
        let ids: Vec<i64> = ids.into_iter().collect();
        let executions = self
            .outcome_executions(scope, company, &ids, import)
            .await?;
        let client = self.outcome_client(scope, company, import).await?;
        let mut paired = Vec::new();
        for execution in executions {
            let order = orders::get_order(&**client, scope, company, execution.order_id).await?;
            paired.push((execution, order));
        }
        drop(client);
        let mut resources = BTreeMap::new();
        let mut runs = Vec::new();
        for (execution, order) in paired {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                resources.entry(execution.resource_id)
            {
                entry.insert(if let Some(import) = import {
                    let client = self.outcome_client(scope, company, Some(import)).await?;
                    self.product_definition
                        .get_import_resource_on(&**client, import, execution.resource_id)
                        .await?
                } else {
                    self.product_definition
                        .get_resource(scope, company, execution.resource_id)
                        .await?
                });
            }
            runs.push(TraceRun {
                id: execution.id,
                order: TraceOrder {
                    id: order.id,
                    code: order.code,
                    kind: order.kind,
                    product_id: order.product_id,
                    formula_version: order.formula_version,
                    routing_version: order.routing_version,
                },
                operation_code: execution.operation_code,
                resource: resources[&execution.resource_id].clone(),
                started_at: execution.started_at,
                completed_at: execution.completed_at,
                source: execution.source,
                inventory_transaction_id: execution.inventory_transaction_id,
            });
        }
        Ok(ProductionTrace { material, runs })
    }

    async fn outcome_executions(
        &self,
        scope: &Scope,
        company: i64,
        ids: &[i64],
        import: Option<&ProductionImport<'_>>,
    ) -> Result<Vec<Execution>> {
        let client = self.outcome_client(scope, company, import).await?;
        let sql = format!(
            "SELECT {} FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=$2 AND inventory_transaction_id=ANY($3) ORDER BY completed_at,id",
            orders::EXECUTION_COLUMNS
        );
        let rows = client
            .query(&sql, &[&scope.tenant(), &company, &ids])
            .await?;
        rows.iter()
            .map(|row| Ok(orders::execution(row)?.0))
            .collect()
    }

    pub async fn get_run_yield(
        &self,
        scope: &Scope,
        company: i64,
        execution: i64,
    ) -> Result<RunYield> {
        let execution = self.get_execution(scope, company, execution).await?;
        self.summarize_yield(scope, company, execution, None, None)
            .await
    }

    pub async fn get_unit_yield(
        &self,
        scope: &Scope,
        company: i64,
        identity: i64,
    ) -> Result<UnitYield> {
        drop(self.read_client(scope, company).await?);
        let inventory = &self.completer.inventory;
        let identity = inventory.get_identity(scope, company, identity).await?;
        let draws = inventory
            .list_identity_draws(scope, company, identity.id)
            .await?;
        let mut ids = BTreeSet::from([identity.source_transaction_id]);
        ids.extend(draws.into_iter().map(|t| t.id));
        let ids: Vec<i64> = ids.into_iter().collect();
        let executions = self.outcome_executions(scope, company, &ids, None).await?;
        let mut runs = Vec::new();
        for execution in executions {
            runs.push(
                self.summarize_yield(scope, company, execution, Some(identity.id), None)
                    .await?,
            );
        }
        Ok(UnitYield { identity, runs })
    }

    pub(crate) async fn summarize_yield(
        &self,
        scope: &Scope,
        company: i64,
        execution: Execution,
        identity: Option<i64>,
        import: Option<&ProductionImport<'_>>,
    ) -> Result<RunYield> {
        let inventory = &self.completer.inventory;
        let transaction = if let Some(import) = import {
            let client = self.outcome_client(scope, company, Some(import)).await?;
            inventory
                .get_import_transaction_on(&**client, import, execution.inventory_transaction_id)
                .await?
        } else {
            inventory
                .get_transaction(scope, company, execution.inventory_transaction_id)
                .await?
        };
        let corrections = self
            .outcome_corrections(scope, company, transaction.id, import)
            .await?;
        let balance = if let Some(import) = import {
            let client = self.outcome_client(scope, company, Some(import)).await?;
            inventory
                .get_import_transaction_balance_on(&**client, import, transaction.id)
                .await?
        } else {
            inventory
                .get_transaction_balance(scope, company, transaction.id)
                .await?
        };
        let client = self.outcome_client(scope, company, import).await?;
        // Only root captures: Inventory supplies the full correction chain once.
        let wastage_ids: Vec<i64> = client.query("SELECT DISTINCT inventory_transaction_id FROM factory_wastage_records WHERE tenant_id=$1 AND company_id=$2 AND execution_id=$3 AND corrects_id IS NULL AND inventory_transaction_id IS NOT NULL ORDER BY inventory_transaction_id", &[&scope.tenant(), &company, &execution.id]).await?.iter().map(|r| r.get(0)).collect();
        drop(client);
        let mut wastage = Vec::new();
        let mut seen = BTreeSet::new();
        for id in wastage_ids {
            let root = if let Some(import) = import {
                let client = self.outcome_client(scope, company, Some(import)).await?;
                inventory
                    .get_import_transaction_on(&**client, import, id)
                    .await?
            } else {
                inventory.get_transaction(scope, company, id).await?
            };
            let changes = self.outcome_corrections(scope, company, id, import).await?;
            for posting in std::iter::once(root).chain(changes) {
                if seen.insert(posting.id) {
                    wastage.push(posting);
                }
            }
        }
        let mut payload = Vec::<Value>::new();
        for entry in &transaction.entries {
            if entry.role == "stock" || entry.role == "variance" {
                payload.push(json!({"origin":"original", "entry":entry}));
            }
        }
        for posting in &corrections {
            for entry in &posting.entries {
                if entry.role == "stock" {
                    payload.push(json!({"origin":"correction", "entry":entry}));
                }
            }
        }
        for posting in &wastage {
            for entry in &posting.entries {
                if entry.role == "stock" {
                    payload.push(json!({"origin":"wastage", "entry":entry}));
                }
            }
        }
        let unit_ids: BTreeSet<i64> = payload
            .iter()
            .filter_map(|v| v["entry"]["native_unit_id"].as_i64())
            .collect();
        let mut units = BTreeMap::new();
        for id in unit_ids {
            let unit = if let Some(import) = import {
                let client = self.outcome_client(scope, company, Some(import)).await?;
                inventory.get_import_unit_on(&**client, import, id).await?
            } else {
                inventory.get_unit(scope, company, id).await?
            };
            units.insert(id, unit);
        }
        let payload = Value::Array(payload);
        let client = self.outcome_client(scope, company, import).await?;
        let rows = client.query(YIELD_SQL, &[&payload, &identity]).await?;
        let mut balances = Vec::new();
        for row in rows {
            let id: i64 = row.get("unit_id");
            let unit = units.remove(&id).ok_or(bilibi_database::Error::Invalid)?;
            balances.push(YieldBalance {
                unit,
                input: row.get("input"),
                product: row.get("product"),
                trim: row.get("trim"),
                waste: row.get("waste"),
                wastage: row.get("wastage"),
                variance: row.get("variance"),
                unit_input: identity.map(|_| row.get("unit_input")),
                unit_output: identity.map(|_| row.get("unit_output")),
            });
        }
        Ok(RunYield {
            execution_id: execution.id,
            transaction_id: transaction.id,
            operation_code: execution.operation_code,
            resource_id: execution.resource_id,
            corrected: !corrections.is_empty(),
            correction_transaction_ids: corrections.iter().map(|t| t.id).collect(),
            wastage_transaction_ids: wastage.iter().map(|t| t.id).collect(),
            balances,
            cross_unit: balance.cross_unit,
        })
    }
}

// No Inventory SQL: classify and sum the exact entries returned by its facade.
const YIELD_SQL: &str = r#"
WITH entries AS (
 SELECT ord, v->>'origin' AS origin, v->'entry' AS e
 FROM jsonb_array_elements($1::jsonb) WITH ORDINALITY AS x(v,ord)
), classified AS (
 SELECT *, CASE WHEN e->>'role'='variance' THEN 'variance'
 WHEN (e->>'native_quantity')::numeric<0 THEN 'input'
 WHEN e->>'output_role'='trim' THEN 'trim'
 WHEN e->>'output_role'='waste' THEN 'waste' ELSE 'product' END AS own_side
 FROM entries
), sided AS (
 SELECT (c.e->>'native_unit_id')::bigint AS unit_id,
 (c.e->>'identity_id')::bigint AS identity_id,
 (c.e->>'native_quantity')::numeric AS quantity,
 CASE WHEN c.origin='original' THEN c.own_side ELSE COALESCE(
 (SELECT o.own_side FROM classified o WHERE o.origin='original' AND o.e->>'role'='stock'
 AND o.e->'item_id'=c.e->'item_id' AND o.e->'identity_id'=c.e->'identity_id' ORDER BY o.ord LIMIT 1),c.own_side) END AS side
 FROM classified c
 UNION ALL
 SELECT (e->>'native_unit_id')::bigint,(e->>'identity_id')::bigint,
 -(e->>'native_quantity')::numeric,'wastage' FROM classified WHERE origin='wastage'
)
SELECT unit_id,
 (-COALESCE(sum(quantity) FILTER(WHERE side='input'),0))::text AS input,
 COALESCE(sum(quantity) FILTER(WHERE side='product'),0)::text AS product,
 COALESCE(sum(quantity) FILTER(WHERE side='trim'),0)::text AS trim,
 COALESCE(sum(quantity) FILTER(WHERE side IN ('waste','wastage')),0)::text AS waste,
 COALESCE(sum(quantity) FILTER(WHERE side='wastage'),0)::text AS wastage,
 COALESCE(sum(quantity) FILTER(WHERE side='variance'),0)::text AS variance,
 (-COALESCE(sum(quantity) FILTER(WHERE side='input' AND identity_id=$2::bigint),0))::text AS unit_input,
 COALESCE(sum(quantity) FILTER(WHERE side IN ('product','trim','waste','wastage') AND identity_id=$2::bigint),0)::text AS unit_output
FROM sided GROUP BY unit_id ORDER BY unit_id
"#;
