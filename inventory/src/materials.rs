//! Material identity (Bilimbi `Inventory.register_material/5`,
//! `create_material/5`, `get_material/3`, `list_materials/3`,
//! `update_material/5`, `retire_material/3`). An item becomes a stocked
//! material once, with a native unit that never changes: every quantity
//! recorded for it is in that unit. A material may take one of the company's
//! types and hold values for that type's property definitions; one without a
//! type holds none.
use crate::{
    access::{readable, writable, writable_for},
    contributions::ITEM_UPDATE,
    error::{Error, FieldError},
    items::{self, Item, ItemUpdate, NewItem},
    limit, material_types, property, settings,
    units::{self, Unit},
};
use bilibi_database::{Change, MutationTransaction};
use bilibi_settings::Settings;
use bilibi_tenancy::Scope;
use serde_json::{json, Map, Value};
use tokio_postgres::{GenericClient, Row};

/// An item's material identity: the item master row it stocks (by `item_id`
/// and `sku`), the native unit its quantities are held in, and, when the
/// company gave it a type, that type and the property values validated
/// against it. A retired material refuses new postings; its history stays
/// readable.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub item_id: i64,
    pub company_id: i64,
    pub sku: String,
    pub native_unit: Unit,
    pub material_type_id: Option<i64>,
    pub properties: Map<String, Value>,
    pub retired: bool,
}

/// How a new material is typed: one of the company's types, or none, and
/// its values for that type's definitions. A material without a type holds
/// no properties.
#[derive(Clone, Debug, Default)]
pub struct MaterialTyping {
    pub material_type_id: Option<i64>,
    pub properties: Map<String, Value>,
}

/// Columns of a material joined with its item (`i`) and native unit (`u`).
const COLUMNS: &str = "m.id,m.item_id,m.company_id,i.sku,m.material_type_id,m.properties,m.retired_at IS NOT NULL AS retired,u.id AS unit_id,u.code AS unit_code,u.name AS unit_name,u.retired_at IS NOT NULL AS unit_retired";
const FROM: &str = "factory_inventory_materials m JOIN commerce_inventory_items i ON i.company_id=m.company_id AND i.id=m.item_id JOIN factory_inventory_units u ON u.company_id=m.company_id AND u.id=m.native_unit_id";
fn read(row: &Row) -> (i64, Material) {
    let company_id = row.get("company_id");
    let properties = match row.get::<_, Value>("properties") {
        Value::Object(properties) => properties,
        _ => Map::new(),
    };
    (
        row.get("id"),
        Material {
            item_id: row.get("item_id"),
            company_id,
            sku: row.get("sku"),
            native_unit: Unit {
                id: row.get("unit_id"),
                company_id,
                code: row.get("unit_code"),
                name: row.get("unit_name"),
                retired: row.get("unit_retired"),
            },
            material_type_id: row.get("material_type_id"),
            properties,
            retired: row.get("retired"),
        },
    )
}
fn audited(material: &Material) -> Value {
    json!({
        "company_id": material.company_id, "item_id": material.item_id,
        "native_unit_id": material.native_unit.id, "material_type_id": material.material_type_id,
        "properties": material.properties, "retired": material.retired,
    })
}

pub(crate) async fn list(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
    requested: Option<i64>,
) -> Result<Vec<Material>, Error> {
    let limit = limit(requested)?;
    readable(client, scope, company_id).await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM {FROM} WHERE m.tenant_id=$1 AND m.company_id=$2 ORDER BY i.sku LIMIT $3"
    );
    Ok(client
        .query(&sql, &[&scope.tenant(), &company_id, &limit])
        .await?
        .iter()
        .map(|row| read(row).1)
        .collect())
}

pub(crate) async fn get(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
    item_id: i64,
) -> Result<Material, Error> {
    readable(client, scope, company_id).await?;
    Ok(fetch(client, scope, company_id, item_id, "").await?.1)
}

/// Reads the material of an item (`ItemNotFound` without the item,
/// `MaterialNotFound` when it is not stocked), with its row id; `FOR UPDATE`
/// locks the material row while it changes and serialises its conversion
/// versions.
pub(crate) async fn fetch(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
    item_id: i64,
    lock: &str,
) -> Result<(i64, Material), Error> {
    let sql =
        format!(
        "SELECT {COLUMNS} FROM {FROM} WHERE m.tenant_id=$1 AND m.company_id=$2 AND m.item_id=$3 {}",
        if lock.is_empty() { "" } else { "FOR UPDATE OF m" }
    );
    match client
        .query_opt(&sql, &[&scope.tenant(), &company_id, &item_id])
        .await?
    {
        Some(row) => Ok(read(&row)),
        None => {
            items::fetch(client, scope, company_id, item_id, false).await?;
            Err(Error::MaterialNotFound)
        }
    }
}

/// The type a new material takes and its validated values; a retired type
/// takes no new material. The type is held `FOR SHARE` so its definitions
/// cannot change before the material commits.
async fn typed(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
    typing: &MaterialTyping,
) -> Result<(Option<i64>, Map<String, Value>), Error> {
    match typing.material_type_id {
        None => Ok((None, property::validate(&[], &typing.properties)?)),
        Some(type_id) => {
            let kind =
                material_types::fetch(client, scope, company_id, type_id, "FOR SHARE").await?;
            if kind.retired {
                return Err(Error::MaterialTypeRetired);
            }
            Ok((
                Some(kind.id),
                property::validate(&kind.property_definitions, &typing.properties)?,
            ))
        }
    }
}

/// Everything `register` decides before its write: the native unit (active,
/// held `FOR SHARE` against retirement), the type and values, and that the
/// item is not yet a material.
async fn accept(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
    item_id: Option<i64>,
    native_unit_id: i64,
    typing: &MaterialTyping,
) -> Result<(Unit, Option<i64>, Map<String, Value>), Error> {
    let unit = units::fetch(client, scope, company_id, native_unit_id, "FOR SHARE").await?;
    if unit.retired {
        return Err(Error::UnitRetired);
    }
    let (type_id, properties) = typed(client, scope, company_id, typing).await?;
    if let Some(item_id) = item_id {
        if client
            .query_opt(
                "SELECT 1 FROM factory_inventory_materials WHERE item_id=$1",
                &[&item_id],
            )
            .await?
            .is_some()
        {
            return Err(Error::Invalid(vec![FieldError {
                field: "item",
                message: "has already been taken".into(),
            }]));
        }
    }
    Ok((unit, type_id, properties))
}

async fn insert(
    sql: &tokio_postgres::Transaction<'_>,
    tenant: i64,
    item: &Item,
    unit: Unit,
    type_id: Option<i64>,
    properties: Map<String, Value>,
) -> Result<(Material, Change), bilibi_database::Error> {
    let row = sql
        .query_one(
            "INSERT INTO factory_inventory_materials(tenant_id,company_id,item_id,native_unit_id,material_type_id,properties) VALUES($1,$2,$3,$4,$5,$6) RETURNING id",
            &[&tenant, &item.company_id, &item.id, &unit.id, &type_id, &Value::Object(properties.clone())],
        )
        .await?;
    let material = Material {
        item_id: item.id,
        company_id: item.company_id,
        sku: item.sku.clone(),
        native_unit: unit,
        material_type_id: type_id,
        properties,
        retired: false,
    };
    let change = Change {
        kind: "factory_inventory_materials".into(),
        id: row.get(0),
        old: json!({}),
        new: audited(&material),
    };
    Ok((material, change))
}

/// Makes an item a stocked material held in `native_unit_id`, once.
pub(crate) async fn register(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    item_id: i64,
    native_unit_id: i64,
    typing: &MaterialTyping,
) -> Result<Material, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let item = items::fetch(tx.sql(), scope, company_id, item_id, true).await?;
    let (unit, type_id, properties) = accept(
        tx.sql(),
        scope,
        company_id,
        Some(item_id),
        native_unit_id,
        typing,
    )
    .await?;
    let tenant = scope.tenant();
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let (material, change) =
                    insert(sql, tenant, &item, unit, type_id, properties).await?;
                Ok((material, vec![change]))
            })
        })
        .await?)
}

/// Creates an item and registers it as a material in one write.
pub(crate) async fn create(
    tx: &mut MutationTransaction<'_>,
    store: &Settings,
    scope: &Scope,
    company_id: i64,
    new: &NewItem,
    native_unit_id: i64,
    typing: &MaterialTyping,
) -> Result<Material, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let new = items::validate_new(tx, store, scope, company_id, new).await?;
    let (unit, type_id, properties) =
        accept(tx.sql(), scope, company_id, None, native_unit_id, typing).await?;
    let tenant = scope.tenant();
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let (item, item_change) = items::insert(sql, tenant, company_id, &new).await?;
                let (material, change) =
                    insert(sql, tenant, &item, unit, type_id, properties).await?;
                Ok((material, vec![item_change, change]))
            })
        })
        .await?)
}

/// Updates an item's editable details and the material's values for its
/// type, keeping its native unit and type. Values replace the recorded ones
/// whole; an empty map clears them.
pub(crate) async fn update(
    tx: &mut MutationTransaction<'_>,
    store: &Settings,
    scope: &Scope,
    company_id: i64,
    item_id: i64,
    update: &ItemUpdate,
    properties: &Map<String, Value>,
) -> Result<Material, Error> {
    writable(tx.sql(), scope, company_id).await?;
    writable_for(tx.sql(), scope, company_id, ITEM_UPDATE).await?;
    let (id, before) = fetch(tx.sql(), scope, company_id, item_id, "FOR UPDATE").await?;
    if before.retired {
        return Err(Error::MaterialRetired);
    }
    let definitions = match before.material_type_id {
        None => Vec::new(),
        Some(type_id) => {
            material_types::fetch(tx.sql(), scope, company_id, type_id, "FOR SHARE")
                .await?
                .property_definitions
        }
    };
    let properties = property::validate(&definitions, properties)?;
    let item_before = items::fetch(tx.sql(), scope, company_id, item_id, true).await?;
    let settings = settings::effective(tx.sql(), store, scope, company_id).await?;
    let item = items::updated(&item_before, update, &settings)?;
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let (_, item_change) = items::write_update(sql, &item_before, &item).await?;
                sql.execute(
                    "UPDATE factory_inventory_materials SET properties=$2,updated_at=now() WHERE id=$1",
                    &[&id, &Value::Object(properties.clone())],
                )
                .await?;
                let material = Material {
                    properties,
                    ..before.clone()
                };
                let change = Change {
                    kind: "factory_inventory_materials".into(),
                    id,
                    old: audited(&before),
                    new: audited(&material),
                };
                Ok((material, vec![item_change, change]))
            })
        })
        .await?)
}

/// Retires a material, preserving its posting history.
pub(crate) async fn retire(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company_id: i64,
    item_id: i64,
) -> Result<Material, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let (id, before) = fetch(tx.sql(), scope, company_id, item_id, "FOR UPDATE").await?;
    if before.retired {
        return Err(Error::MaterialRetired);
    }
    let has_stock: bool = tx.sql().query_one(
        "SELECT EXISTS(SELECT 1 FROM factory_inventory_transaction_entries WHERE tenant_id=$1 AND company_id=$2 AND material_id=$3 AND role='stock' GROUP BY location_id HAVING sum(native_quantity)<>0)",
        &[&scope.tenant(), &company_id, &id],
    ).await?.get(0);
    if has_stock {
        return Err(Error::MaterialHasStock);
    }
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                sql.execute(
                    "UPDATE factory_inventory_materials SET retired_at=now(),updated_at=now() WHERE id=$1",
                    &[&id],
                )
                .await?;
                let material = Material {
                    retired: true,
                    ..before.clone()
                };
                let change = Change {
                    kind: "factory_inventory_materials".into(),
                    id,
                    old: audited(&before),
                    new: audited(&material),
                };
                Ok((material, vec![change]))
            })
        })
        .await?)
}
