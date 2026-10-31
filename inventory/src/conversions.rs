//! Item-level unit conversions (Bilimbi `Inventory.define_conversion/5`,
//! `list_conversions/3`, `get_conversion/5`). One conversion unit equals
//! `factor` native units. A version never changes once defined (PostgreSQL
//! refuses UPDATE, DELETE and TRUNCATE), so a changed factor is the next
//! version and a converted quantity that names its version can always be
//! recomputed.
use crate::{
    access::{readable, writable},
    error::{Error, FieldError},
    materials,
    units::{self, Unit},
};
use bilibi_database::{Change, MutationTransaction};
use bilibi_tenancy::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use tokio_postgres::{GenericClient, Row};

/// One version of an item's conversion: one `unit` equals `factor` of the
/// item's `native_unit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversion {
    pub id: i64,
    pub item_id: i64,
    pub unit: Unit,
    pub native_unit: Unit,
    pub version: i32,
    pub factor: Decimal,
}

const COLUMNS: &str = "c.id,c.version,c.factor,u.id AS unit_id,u.company_id,u.code,u.name,u.retired_at IS NOT NULL AS retired";
const FROM: &str = "factory_inventory_unit_conversions c JOIN factory_inventory_units u ON u.company_id=c.company_id AND u.id=c.unit_id";
fn read(row: &Row, item_id: i64, native_unit: &Unit) -> Conversion {
    Conversion {
        id: row.get("id"),
        item_id,
        unit: Unit {
            id: row.get("unit_id"),
            company_id: row.get("company_id"),
            code: row.get("code"),
            name: row.get("name"),
            retired: row.get("retired"),
        },
        native_unit: native_unit.clone(),
        version: row.get("version"),
        factor: row.get("factor"),
    }
}

/// Every version of an item's conversions, by unit code then version.
pub(crate) async fn list(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    item_id: i64,
) -> Result<Vec<Conversion>, Error> {
    readable(client, scope, company_id).await?;
    let (material_id, material) = materials::fetch(client, scope, company_id, item_id, "").await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM {FROM} WHERE c.company_id=$1 AND c.material_id=$2 ORDER BY u.code, c.version"
    );
    Ok(client
        .query(&sql, &[&company_id, &material_id])
        .await?
        .iter()
        .map(|row| read(row, item_id, &material.native_unit))
        .collect())
}

/// The conversion from `unit_id`: the current version, or `version`.
pub(crate) async fn get(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    item_id: i64,
    unit_id: i64,
    version: Option<i32>,
) -> Result<Conversion, Error> {
    readable(client, scope, company_id).await?;
    get_authorized(client, scope, company_id, item_id, unit_id, version).await
}

pub(crate) async fn get_authorized(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    item_id: i64,
    unit_id: i64,
    version: Option<i32>,
) -> Result<Conversion, Error> {
    let (material_id, material) = materials::fetch(client, scope, company_id, item_id, "").await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM {FROM} WHERE c.company_id=$1 AND c.material_id=$2 AND c.unit_id=$3 AND ($4::integer IS NULL OR c.version=$4) ORDER BY c.version DESC LIMIT 1"
    );
    client
        .query_opt(&sql, &[&company_id, &material_id, &unit_id, &version])
        .await?
        .map(|row| read(&row, item_id, &material.native_unit))
        .ok_or(Error::ConversionNotFound)
}

/// numeric(24,12): positive, at most 12 decimal places and 12 integer digits.
fn factor_errors(factor: Decimal) -> Result<Decimal, Error> {
    let message = if factor <= Decimal::ZERO {
        "must be greater than 0"
    } else if factor.normalize().scale() > 12 {
        "has more than 12 decimal places"
    } else if factor >= Decimal::from(1_000_000_000_000_i64) {
        "is too large"
    } else {
        return Ok(factor.normalize());
    };
    Err(Error::Invalid(vec![FieldError {
        field: "factor",
        message: message.into(),
    }]))
}

/// Defines the next version of an item's conversion from `unit_id`. Locking
/// the material row serialises the versions; the unique version index is the
/// backstop.
pub(crate) async fn define(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    item_id: i64,
    unit_id: i64,
    factor: Decimal,
) -> Result<Conversion, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let (material_id, material) =
        materials::fetch(tx.sql(), scope, company_id, item_id, "FOR UPDATE").await?;
    let unit = units::fetch(tx.sql(), scope, company_id, unit_id, "FOR SHARE").await?;
    if unit.retired {
        return Err(Error::UnitRetired);
    }
    if unit.id == material.native_unit.id {
        return Err(Error::NativeUnit);
    }
    let factor = factor_errors(factor)?;
    let version: i32 = tx
        .sql()
        .query_one(
            "SELECT COALESCE(max(version),0)+1 FROM factory_inventory_unit_conversions WHERE material_id=$1 AND unit_id=$2",
            &[&material_id, &unit_id],
        )
        .await?
        .get(0);
    let tenant = scope.tenant();
    let native_unit = material.native_unit;
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                // `RETURNING` reads the factor back at the column's scale, so
                // the result equals what a later read returns.
                let row = sql
                    .query_one(
                        "INSERT INTO factory_inventory_unit_conversions(tenant_id,company_id,material_id,unit_id,version,factor) VALUES($1,$2,$3,$4,$5,$6) RETURNING id,factor",
                        &[&tenant, &company_id, &material_id, &unit_id, &version, &factor],
                    )
                    .await?;
                let conversion = Conversion {
                    id: row.get(0),
                    item_id,
                    unit,
                    native_unit,
                    version,
                    factor: row.get(1),
                };
                let change = Change {
                    kind: "factory_inventory_unit_conversions".into(),
                    id: conversion.id,
                    old: json!({}),
                    new: json!({
                        "company_id": company_id, "material_id": material_id, "item_id": item_id,
                        "unit_id": conversion.unit.id, "version": version, "factor": conversion.factor.to_string(),
                    }),
                };
                Ok((conversion, vec![change]))
            })
        })
        .await?)
}
