//! Bilimbi ProductDefinition formula/routing normalization and compatibility.
use crate::{
    store::{self, Kind},
    *,
};
use bilibi_database::MutationTransaction;
use bilibi_factory_inventory::{Error as InventoryError, Inventory};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::{collections::HashSet, str::FromStr};

fn config(value: &Value) -> Result<Value> {
    let config = value.as_object().ok_or(Error::InvalidProcessConfig)?;
    for (key, value) in config {
        if !match key.as_str() {
            "process_family" => value.is_null() || value.is_string(),
            "tolerances" | "output_roles" | "material_hold_rules" => {
                value.is_null() || value.is_object()
            }
            _ => false,
        } {
            return Err(Error::InvalidProcessConfig);
        }
    }
    Ok(value.clone())
}
fn lines(value: &Value) -> Result<Vec<FormulaLine>> {
    let values = value
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or(Error::InvalidLines)?;
    let mut lines = Vec::new();
    for value in values {
        let mut value = value.as_object().cloned().ok_or(Error::InvalidLines)?;
        let quantity = match value.get("quantity") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => return Err(Error::InvalidLines),
        };
        let quantity = Decimal::from_str(&quantity)
            .or_else(|_| Decimal::from_scientific(&quantity))
            .map_err(|_| Error::InvalidLines)?;
        if quantity <= Decimal::ZERO {
            return Err(Error::InvalidLines);
        }
        value.insert("quantity".into(), json!(quantity.to_string()));
        let line: FormulaLine =
            serde_json::from_value(Value::Object(value)).map_err(|_| Error::InvalidLines)?;
        if line.item_id <= 0
            || line.unit_id <= 0
            || !["input", "output"].contains(&line.role.as_str())
            || line
                .output_role
                .as_ref()
                .is_some_and(|r| line.role != "output" || r.is_empty())
            || (line.material_hold_rule.is_some() && line.role != "input")
        {
            return Err(Error::InvalidLines);
        }
        lines.push(line);
    }
    if !lines.iter().any(|l| l.role == "output") {
        return Err(Error::InvalidLines);
    }
    Ok(lines)
}
fn operations(value: &Value) -> Result<Vec<Operation>> {
    let operations: Vec<Operation> =
        serde_json::from_value(value.clone()).map_err(|_| Error::InvalidOperations)?;
    let mut codes = HashSet::new();
    let mut sequences = HashSet::new();
    if operations.is_empty()
        || operations.iter().any(|op| {
            op.code.is_empty()
                || op.sequence <= 0
                || !codes.insert(&op.code)
                || !sequences.insert(op.sequence)
                || op.outputs.is_empty()
                || op.allowed_resource_ids.is_empty()
                || op
                    .inputs
                    .iter()
                    .chain(&op.outputs)
                    .chain(&op.allowed_resource_ids)
                    .any(|id| *id <= 0)
        })
    {
        return Err(Error::InvalidOperations);
    }
    Ok(operations)
}
async fn items(
    client: &(impl tokio_postgres::GenericClient + Sync),
    inventory: &Inventory,
    scope: &Scope,
    company: i64,
    ids: HashSet<i64>,
) -> Result<()> {
    for id in ids {
        match inventory.get_material_on(client, scope, company, id).await {
            Ok(material) if material.retired => return Err(InventoryError::MaterialRetired.into()),
            Ok(_) | Err(InventoryError::MaterialNotFound) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
pub(crate) async fn formula(
    tx: &mut MutationTransaction<'_>,
    inventory: &Inventory,
    scope: &Scope,
    company: i64,
    product: i64,
    attrs: &Value,
) -> Result<FormulaRevision> {
    let product: Product = store::fetch(tx.sql(), scope, company, Kind::Product, product).await?;
    let lines = lines(&attrs["lines"])?;
    items(
        tx.sql(),
        inventory,
        scope,
        company,
        lines.iter().map(|l| l.item_id).collect(),
    )
    .await?;
    for id in lines.iter().map(|l| l.unit_id).collect::<HashSet<_>>() {
        if inventory
            .get_unit_on(tx.sql(), scope, company, id)
            .await?
            .retired
        {
            return Err(InventoryError::UnitRetired.into());
        }
    }
    if !lines
        .iter()
        .any(|l| l.role == "output" && l.item_id == product.item_id)
    {
        return Err(Error::ProductOutputMissing);
    }
    let config = config(attrs.get("process_config").unwrap_or(&json!({})))?;
    let version = store::next_version(tx.sql(), scope, company, Kind::Formula, product.id).await?;
    store::insert(
        tx,
        scope,
        company,
        Kind::Formula,
        json!({"product_id":product.id,"version":version,"lines":lines,"process_config":config}),
    )
    .await
}
pub(crate) async fn routing(
    tx: &mut MutationTransaction<'_>,
    inventory: &Inventory,
    scope: &Scope,
    company: i64,
    product: i64,
    attrs: &Value,
) -> Result<RoutingRevision> {
    let product: Product = store::fetch(tx.sql(), scope, company, Kind::Product, product).await?;
    let operations = operations(&attrs["operations"])?;
    items(
        tx.sql(),
        inventory,
        scope,
        company,
        operations
            .iter()
            .flat_map(|op| op.inputs.iter().chain(&op.outputs).copied())
            .collect(),
    )
    .await?;
    // Collected before the first await: an iterator over `operations` held
    // across one would make the facade's future unprovably `Send` for pages.
    let allowed: Vec<i64> = operations
        .iter()
        .flat_map(|op| op.allowed_resource_ids.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    for id in allowed {
        let resource: Resource = store::fetch(tx.sql(), scope, company, Kind::Resource, id).await?;
        if resource.retired {
            return Err(Error::ResourceRetired);
        }
    }
    let config = config(attrs.get("process_config").unwrap_or(&json!({})))?;
    let version = store::next_version(tx.sql(), scope, company, Kind::Routing, product.id).await?;
    store::insert(tx,scope,company,Kind::Routing,json!({"product_id":product.id,"version":version,"operations":operations,"process_config":config})).await
}
pub(crate) fn compatible(
    product: &Product,
    formula: &FormulaRevision,
    routing: &RoutingRevision,
) -> Result<()> {
    let inputs: HashSet<_> = formula
        .lines
        .iter()
        .filter(|l| l.role == "input")
        .map(|l| l.item_id)
        .collect();
    let outputs: HashSet<_> = formula
        .lines
        .iter()
        .filter(|l| l.role == "output")
        .map(|l| l.item_id)
        .collect();
    if routing.operations.iter().all(|op| {
        op.inputs.iter().all(|id| inputs.contains(id))
            && op.outputs.iter().all(|id| outputs.contains(id))
    }) && routing
        .operations
        .iter()
        .any(|op| op.outputs.contains(&product.item_id))
    {
        Ok(())
    } else {
        Err(Error::RoutingFormulaMismatch)
    }
}
