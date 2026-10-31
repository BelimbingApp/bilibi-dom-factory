//! Company units of measure (Bilimbi `Inventory.list_units/3`, `get_unit/3`,
//! `create_unit/3`, `rename_unit/4`, `retire_unit/3`).
use crate::{
    access::{readable, writable},
    error::{Error, FieldError},
    limit,
};
use bilibi_database::{Change, MutationTransaction};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use tokio_postgres::{GenericClient, Row};

/// One company's unit of measure. A retired unit keeps its id in conversions
/// and postings but takes no new use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub name: String,
    pub retired: bool,
}

/// The attributes of a new unit: `code` (unique in the company) and `name`.
#[derive(Clone, Debug, Default)]
pub struct NewUnit {
    pub code: String,
    pub name: String,
}

const COLUMNS: &str = "id,company_id,code,name,retired_at IS NOT NULL AS retired";
fn read(row: &Row) -> Unit {
    Unit {
        id: row.get("id"),
        company_id: row.get("company_id"),
        code: row.get("code"),
        name: row.get("name"),
        retired: row.get("retired"),
    }
}
fn audited(unit: &Unit) -> Value {
    json!({"company_id":unit.company_id,"code":unit.code,"name":unit.name,"retired":unit.retired})
}

pub(crate) async fn list(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    requested: Option<i64>,
) -> Result<Vec<Unit>, Error> {
    let limit = limit(requested)?;
    readable(client, scope, company_id).await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM factory_inventory_units WHERE tenant_id=$1 AND company_id=$2 ORDER BY code LIMIT $3"
    );
    Ok(client
        .query(&sql, &[&scope.tenant(), &company_id, &limit])
        .await?
        .iter()
        .map(read)
        .collect())
}

pub(crate) async fn get(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    unit_id: i64,
) -> Result<Unit, Error> {
    readable(client, scope, company_id).await?;
    fetch(client, scope, company_id, unit_id, "").await
}

/// Reads a unit, optionally with a row lock (`FOR SHARE` while a material or
/// conversion takes it, `FOR UPDATE` while it changes).
pub(crate) async fn fetch(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    unit_id: i64,
    lock: &str,
) -> Result<Unit, Error> {
    let sql = format!(
        "SELECT {COLUMNS} FROM factory_inventory_units WHERE tenant_id=$1 AND company_id=$2 AND id=$3 {lock}"
    );
    client
        .query_opt(&sql, &[&scope.tenant(), &company_id, &unit_id])
        .await?
        .as_ref()
        .map(read)
        .ok_or(Error::UnitNotFound)
}

fn name_errors(name: &str, errors: &mut Vec<FieldError>) {
    if name.trim().is_empty() {
        errors.push(FieldError {
            field: "name",
            message: "can't be blank".into(),
        });
    } else if name.chars().count() > 255 {
        errors.push(FieldError {
            field: "name",
            message: "should be at most 255 character(s)".into(),
        });
    }
}

pub(crate) async fn create(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    new: &NewUnit,
) -> Result<Unit, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let code = new.code.trim().to_owned();
    let name = new.name.clone();
    let mut errors = Vec::new();
    if code.is_empty() {
        errors.push(FieldError {
            field: "code",
            message: "can't be blank".into(),
        });
    } else if code.chars().count() > 32 {
        errors.push(FieldError {
            field: "code",
            message: "should be at most 32 character(s)".into(),
        });
    } else if tx
        .sql()
        .query_opt(
            "SELECT 1 FROM factory_inventory_units WHERE company_id=$1 AND code=$2",
            &[&company_id, &code],
        )
        .await?
        .is_some()
    {
        errors.push(FieldError {
            field: "code",
            message: "has already been taken".into(),
        });
    }
    name_errors(&name, &mut errors);
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }
    let tenant = scope.tenant();
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let row = sql
                    .query_one(
                        &format!("INSERT INTO factory_inventory_units(tenant_id,company_id,code,name) VALUES($1,$2,$3,$4) RETURNING {COLUMNS}"),
                        &[&tenant, &company_id, &code, &name],
                    )
                    .await?;
                let unit = read(&row);
                let change = Change { kind: "factory_inventory_units".into(), id: unit.id, old: json!({}), new: audited(&unit) };
                Ok((unit, vec![change]))
            })
        })
        .await?)
}

/// Renames a unit. Its code stays stable in conversion and posting history.
pub(crate) async fn rename(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    unit_id: i64,
    name: &str,
) -> Result<Unit, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let before = fetch(tx.sql(), scope, company_id, unit_id, "FOR UPDATE").await?;
    if before.retired {
        return Err(Error::UnitRetired);
    }
    let mut errors = Vec::new();
    name_errors(name, &mut errors);
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }
    let name = name.to_owned();
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let row = sql
                    .query_one(
                        &format!("UPDATE factory_inventory_units SET name=$2,updated_at=now() WHERE id=$1 RETURNING {COLUMNS}"),
                        &[&unit_id, &name],
                    )
                    .await?;
                let unit = read(&row);
                let change = Change { kind: "factory_inventory_units".into(), id: unit_id, old: audited(&before), new: audited(&unit) };
                Ok((unit, vec![change]))
            })
        })
        .await?)
}

/// Retires a unit while preserving its id in conversions and postings. A
/// unit that is an active material's native unit is refused as in use.
pub(crate) async fn retire(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    unit_id: i64,
) -> Result<Unit, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let before = fetch(tx.sql(), scope, company_id, unit_id, "FOR UPDATE").await?;
    if before.retired {
        return Err(Error::UnitRetired);
    }
    if tx
        .sql()
        .query_opt(
            "SELECT 1 FROM factory_inventory_materials WHERE company_id=$1 AND native_unit_id=$2 AND retired_at IS NULL LIMIT 1",
            &[&company_id, &unit_id],
        )
        .await?
        .is_some()
    {
        return Err(Error::UnitInUse);
    }
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let row = sql
                    .query_one(
                        &format!("UPDATE factory_inventory_units SET retired_at=now(),updated_at=now() WHERE id=$1 RETURNING {COLUMNS}"),
                        &[&unit_id],
                    )
                    .await?;
                let unit = read(&row);
                let change = Change { kind: "factory_inventory_units".into(), id: unit_id, old: audited(&before), new: audited(&unit) };
                Ok((unit, vec![change]))
            })
        })
        .await?)
}
