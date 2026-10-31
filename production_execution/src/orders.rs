//! Private persistence for orders, executions and hold overrides. Every
//! statement carries tenant and company predicates.
use crate::{
    model::{Approver, Execution, HoldOverride, Order, OrderKind, Source},
    Error, Result,
};
use bilibi_tenancy::Scope;
use tokio_postgres::{GenericClient, Row};

pub(crate) const ORDER_COLUMNS: &str =
    "id,company_id,code,kind,product_id,formula_version,routing_version,demand_ref";
pub(crate) const EXECUTION_COLUMNS: &str = "id,company_id,order_id,request_id,request_fingerprint,source,operation_code,resource_id,operator_type,operator_id,started_at,completed_at,inputs,outputs,variance,evidence,inventory_transaction_id,formula_version,routing_version";

fn source(name: &str) -> Result<Source> {
    match name {
        "live" => Ok(Source::Live),
        "import" => Ok(Source::Import),
        _ => Err(bilibi_database::Error::Invalid.into()),
    }
}

pub(crate) fn order(row: &Row) -> Result<Order> {
    Ok(Order {
        id: row.get("id"),
        company_id: row.get("company_id"),
        code: row.get("code"),
        kind: OrderKind::parse(row.get("kind")).ok_or(bilibi_database::Error::Invalid)?,
        product_id: row.get("product_id"),
        formula_version: row.get("formula_version"),
        routing_version: row.get("routing_version"),
        demand_ref: row.get("demand_ref"),
    })
}

/// The stored execution, with the fingerprint of the request that made it.
pub(crate) fn execution(row: &Row) -> Result<(Execution, String)> {
    let variance: Option<serde_json::Value> = row.get("variance");
    Ok((
        Execution {
            id: row.get("id"),
            company_id: row.get("company_id"),
            order_id: row.get("order_id"),
            request_id: row.get("request_id"),
            source: source(row.get("source"))?,
            operation_code: row.get("operation_code"),
            resource_id: row.get("resource_id"),
            operator_type: row.get("operator_type"),
            operator_id: row.get("operator_id"),
            started_at: row.get("started_at"),
            completed_at: row.get("completed_at"),
            inputs: serde_json::from_value(row.get("inputs"))?,
            outputs: serde_json::from_value(row.get("outputs"))?,
            variance: variance.map(serde_json::from_value).transpose()?,
            evidence: row.get("evidence"),
            inventory_transaction_id: row.get("inventory_transaction_id"),
            formula_version: row.get("formula_version"),
            routing_version: row.get("routing_version"),
        },
        row.get("request_fingerprint"),
    ))
}

fn hold_override(row: &Row) -> Result<HoldOverride> {
    Ok(HoldOverride {
        id: row.get("id"),
        execution_id: row.get("execution_id"),
        inventory_transaction_id: row.get("inventory_transaction_id"),
        source_transaction_id: row.get("source_transaction_id"),
        identity_id: row.get("identity_id"),
        item_id: row.get("item_id"),
        source: source(row.get("source"))?,
        actor_type: row.get("actor_type"),
        actor_id: row.get("actor_id"),
        acting_for_user_id: row.get("acting_for_user_id"),
        recorded_by_type: row.get("recorded_by_type"),
        recorded_by_id: row.get("recorded_by_id"),
        recorded_by_acting_for_user_id: row.get("recorded_by_acting_for_user_id"),
        reason: row.get("reason"),
        evidence: row.get("evidence"),
        occurred_at: row.get("occurred_at"),
    })
}

pub(crate) async fn get_order(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Order> {
    let sql = format!(
        "SELECT {ORDER_COLUMNS} FROM factory_production_orders WHERE tenant_id=$1 AND company_id=$2 AND id=$3"
    );
    let row = client
        .query_opt(&sql, &[&scope.tenant(), &company, &id])
        .await?
        .ok_or(Error::OrderNotFound)?;
    order(&row)
}

pub(crate) async fn list_orders(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    limit: i64,
) -> Result<Vec<Order>> {
    let sql = format!(
        "SELECT {ORDER_COLUMNS} FROM factory_production_orders WHERE tenant_id=$1 AND company_id=$2 ORDER BY id DESC LIMIT $3"
    );
    client
        .query(&sql, &[&scope.tenant(), &company, &limit])
        .await?
        .iter()
        .map(order)
        .collect()
}

pub(crate) async fn get_execution(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Execution> {
    let sql = format!(
        "SELECT {EXECUTION_COLUMNS} FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=$2 AND id=$3"
    );
    let row = client
        .query_opt(&sql, &[&scope.tenant(), &company, &id])
        .await?
        .ok_or(Error::ExecutionNotFound)?;
    Ok(execution(&row)?.0)
}

/// Most recently completed first.
pub(crate) async fn list_executions(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    order: i64,
    limit: i64,
) -> Result<Vec<Execution>> {
    let sql = format!(
        "SELECT {EXECUTION_COLUMNS} FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=$2 AND order_id=$3 ORDER BY completed_at DESC,id DESC LIMIT $4"
    );
    client
        .query(&sql, &[&scope.tenant(), &company, &order, &limit])
        .await?
        .iter()
        .map(|row| Ok(execution(row)?.0))
        .collect()
}

pub(crate) async fn recorded(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    request_id: &str,
) -> Result<bool> {
    Ok(client
        .query_opt(
            "SELECT 1 FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=$2 AND request_id=$3",
            &[&scope.tenant(), &company, &request_id],
        )
        .await?
        .is_some())
}

pub(crate) async fn hold_overrides(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    execution: i64,
) -> Result<Vec<HoldOverride>> {
    client
        .query(
            "SELECT * FROM factory_material_hold_overrides WHERE tenant_id=$1 AND company_id=$2 AND execution_id=$3 ORDER BY id",
            &[&scope.tenant(), &company, &execution],
        )
        .await?
        .iter()
        .map(hold_override)
        .collect()
}

impl Approver {
    /// A user, or an agent acting for a user.
    pub(crate) fn valid(&self) -> bool {
        self.id > 0
            && ((self.kind == "user" && self.acting_for_user_id.is_none())
                || (self.kind == "agent" && self.acting_for_user_id.is_some_and(|id| id > 0)))
    }
}
