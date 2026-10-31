//! A company's material types with their property definitions (Bilimbi
//! `Inventory.create_material_type/3`, `get_material_type/3`,
//! `list_material_types/3`, `update_material_type/4`,
//! `retire_material_type/3`). A type's definitions can change until a
//! material uses it; its code and name can change later too, and retirement
//! keeps existing references readable.
use crate::{
    access::{readable, writable},
    error::{Error, FieldError},
    limit,
    property::{self, PropertyDefinition},
};
use bilibi_database::{Change, MutationTransaction};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use tokio_postgres::{GenericClient, Row};

/// One company's material type. A retired type keeps its materials readable
/// but takes no new material and no edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialType {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub name: String,
    pub property_definitions: Vec<PropertyDefinition>,
    pub retired: bool,
}

/// A new material type: `code` (upper-cased, unique in the company), `name`
/// and its `property_definitions` (empty for a type with no properties).
#[derive(Clone, Debug, Default)]
pub struct NewMaterialType {
    pub code: String,
    pub name: String,
    pub property_definitions: Vec<PropertyDefinition>,
}

/// A change to a material type; `None` keeps the current value.
#[derive(Clone, Debug, Default)]
pub struct MaterialTypeUpdate {
    pub code: Option<String>,
    pub name: Option<String>,
    pub property_definitions: Option<Vec<PropertyDefinition>>,
}

const COLUMNS: &str =
    "id,company_id,code,name,property_definitions,retired_at IS NOT NULL AS retired";
fn read(row: &Row) -> MaterialType {
    MaterialType {
        id: row.get("id"),
        company_id: row.get("company_id"),
        code: row.get("code"),
        name: row.get("name"),
        property_definitions: property::stored(row.get("property_definitions")),
        retired: row.get("retired"),
    }
}
fn audited(kind: &MaterialType) -> Value {
    json!({
        "company_id": kind.company_id, "code": kind.code, "name": kind.name,
        "property_definitions": property::to_value(&kind.property_definitions), "retired": kind.retired,
    })
}

pub(crate) async fn list(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    requested: Option<i64>,
) -> Result<Vec<MaterialType>, Error> {
    let limit = limit(requested)?;
    readable(client, scope, company_id).await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM factory_inventory_material_types WHERE tenant_id=$1 AND company_id=$2 ORDER BY code LIMIT $3"
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
    material_type_id: i64,
) -> Result<MaterialType, Error> {
    readable(client, scope, company_id).await?;
    fetch(client, scope, company_id, material_type_id, "").await
}

/// Reads a type, optionally with a row lock (`FOR SHARE` while a material
/// takes it, `FOR UPDATE` while it changes).
pub(crate) async fn fetch(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    material_type_id: i64,
    lock: &str,
) -> Result<MaterialType, Error> {
    let sql = format!(
        "SELECT {COLUMNS} FROM factory_inventory_material_types WHERE tenant_id=$1 AND company_id=$2 AND id=$3 {lock}"
    );
    client
        .query_opt(&sql, &[&scope.tenant(), &company_id, &material_type_id])
        .await?
        .as_ref()
        .map(read)
        .ok_or(Error::MaterialTypeNotFound)
}

async fn field_errors(
    client: &(impl GenericClient + Sync),
    company_id: i64,
    except: Option<i64>,
    code: &str,
    name: &str,
) -> Result<(), Error> {
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
    } else if client
        .query_opt(
            "SELECT 1 FROM factory_inventory_material_types WHERE company_id=$1 AND code=$2 AND id IS DISTINCT FROM $3",
            &[&company_id, &code, &except],
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
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(errors))
    }
}

pub(crate) async fn create(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    new: &NewMaterialType,
) -> Result<MaterialType, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let definitions = property::normalize(&new.property_definitions)?;
    let code = new.code.trim().to_uppercase();
    let name = new.name.clone();
    field_errors(tx.sql(), company_id, None, &code, &name).await?;
    let tenant = scope.tenant();
    let definitions = property::to_value(&definitions);
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let row = sql
                    .query_one(
                        &format!("INSERT INTO factory_inventory_material_types(tenant_id,company_id,code,name,property_definitions) VALUES($1,$2,$3,$4,$5) RETURNING {COLUMNS}"),
                        &[&tenant, &company_id, &code, &name, &definitions],
                    )
                    .await?;
                let kind = read(&row);
                let change = Change { kind: "factory_inventory_material_types".into(), id: kind.id, old: json!({}), new: audited(&kind) };
                Ok((kind, vec![change]))
            })
        })
        .await?)
}

/// Updates a type. Its property definitions cannot change once a material
/// uses it, so recorded values keep their meaning.
pub(crate) async fn update(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    material_type_id: i64,
    update: &MaterialTypeUpdate,
) -> Result<MaterialType, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let before = fetch(tx.sql(), scope, company_id, material_type_id, "FOR UPDATE").await?;
    if before.retired {
        return Err(Error::MaterialTypeRetired);
    }
    let definitions = match &update.property_definitions {
        Some(definitions) => property::normalize(definitions)?,
        None => before.property_definitions.clone(),
    };
    if definitions != before.property_definitions
        && tx
            .sql()
            .query_opt(
                "SELECT 1 FROM factory_inventory_materials WHERE company_id=$1 AND material_type_id=$2 LIMIT 1",
                &[&company_id, &material_type_id],
            )
            .await?
            .is_some()
    {
        return Err(Error::MaterialTypeInUse);
    }
    let code = update
        .code
        .as_deref()
        .map_or_else(|| before.code.clone(), |code| code.trim().to_uppercase());
    let name = update.name.clone().unwrap_or_else(|| before.name.clone());
    field_errors(tx.sql(), company_id, Some(material_type_id), &code, &name).await?;
    let definitions = property::to_value(&definitions);
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let row = sql
                    .query_one(
                        &format!("UPDATE factory_inventory_material_types SET code=$2,name=$3,property_definitions=$4,updated_at=now() WHERE id=$1 RETURNING {COLUMNS}"),
                        &[&material_type_id, &code, &name, &definitions],
                    )
                    .await?;
                let kind = read(&row);
                let change = Change { kind: "factory_inventory_material_types".into(), id: material_type_id, old: audited(&before), new: audited(&kind) };
                Ok((kind, vec![change]))
            })
        })
        .await?)
}

/// Retires a type without removing historical references.
pub(crate) async fn retire(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    material_type_id: i64,
) -> Result<MaterialType, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let before = fetch(tx.sql(), scope, company_id, material_type_id, "FOR UPDATE").await?;
    if before.retired {
        return Err(Error::MaterialTypeRetired);
    }
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let row = sql
                    .query_one(
                        &format!("UPDATE factory_inventory_material_types SET retired_at=now(),updated_at=now() WHERE id=$1 RETURNING {COLUMNS}"),
                        &[&material_type_id],
                    )
                    .await?;
                let kind = read(&row);
                let change = Change { kind: "factory_inventory_material_types".into(), id: material_type_id, old: audited(&before), new: audited(&kind) };
                Ok((kind, vec![change]))
            })
        })
        .await?)
}
