//! Company stock locations (Bilimbi `Inventory.list_locations/3`,
//! `get_location/3`, `create_location/3`). Locations have no page: the
//! Material Transaction ledger names them in postings.
use crate::{
    access::{readable, writable},
    error::{Error, FieldError},
    limit,
};
use bilibi_database::{Change, MutationTransaction};
use bilibi_tenancy::Scope;
use serde_json::json;
use tokio_postgres::{GenericClient, Row};

/// One company's stock location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub name: String,
}

/// The attributes of a new location: `code` (upper-cased, unique in the
/// company) and `name`.
#[derive(Clone, Debug, Default)]
pub struct NewLocation {
    pub code: String,
    pub name: String,
}

const COLUMNS: &str = "id,company_id,code,name";
fn read(row: &Row) -> Location {
    Location {
        id: row.get("id"),
        company_id: row.get("company_id"),
        code: row.get("code"),
        name: row.get("name"),
    }
}

pub(crate) async fn list(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    requested: Option<i64>,
) -> Result<Vec<Location>, Error> {
    let limit = limit(requested)?;
    readable(client, scope, company_id).await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM factory_inventory_locations WHERE tenant_id=$1 AND company_id=$2 ORDER BY code LIMIT $3"
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
    location_id: i64,
) -> Result<Location, Error> {
    readable(client, scope, company_id).await?;
    client
        .query_opt(
            &format!("SELECT {COLUMNS} FROM factory_inventory_locations WHERE tenant_id=$1 AND company_id=$2 AND id=$3"),
            &[&scope.tenant(), &company_id, &location_id],
        )
        .await?
        .as_ref()
        .map(read)
        .ok_or(Error::LocationNotFound)
}

pub(crate) async fn create(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    new: &NewLocation,
) -> Result<Location, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let code = new.code.trim().to_uppercase();
    let name = new.name.clone();
    let mut errors = Vec::new();
    if code.is_empty() {
        errors.push(FieldError {
            field: "code",
            message: "can't be blank".into(),
        });
    } else if code.chars().count() > 64 {
        errors.push(FieldError {
            field: "code",
            message: "should be at most 64 character(s)".into(),
        });
    } else if tx
        .sql()
        .query_opt(
            "SELECT 1 FROM factory_inventory_locations WHERE company_id=$1 AND code=$2",
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
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }
    let tenant = scope.tenant();
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let row = sql
                    .query_one(
                        &format!("INSERT INTO factory_inventory_locations(tenant_id,company_id,code,name) VALUES($1,$2,$3,$4) RETURNING {COLUMNS}"),
                        &[&tenant, &company_id, &code, &name],
                    )
                    .await?;
                let location = read(&row);
                let change = Change {
                    kind: "factory_inventory_locations".into(),
                    id: location.id,
                    old: json!({}),
                    new: json!({"company_id":location.company_id,"code":location.code,"name":location.name}),
                };
                Ok((location, vec![change]))
            })
        })
        .await?)
}
