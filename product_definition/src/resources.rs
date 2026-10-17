//! Bilimbi ProductDefinition resource lifecycle; the company lock serializes
//! definition changes, new resources and retirement against each other.
use crate::{
    store::{self, Kind},
    *,
};
use bilibi_database::MutationTransaction;
use bilibi_factory_inventory::Inventory;
use serde_json::json;

pub(crate) async fn create_type(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    new: &NewResourceType,
) -> Result<ResourceType> {
    let code = new.code.trim().to_uppercase();
    let definitions = Inventory::normalize_property_definitions(&new.property_definitions)?;
    store::names(
        tx.sql(),
        scope,
        company,
        Kind::ResourceType,
        None,
        &code,
        &new.name,
    )
    .await?;
    store::insert(
        tx,
        scope,
        company,
        Kind::ResourceType,
        json!({"code":code,"name":new.name,"property_definitions":definitions}),
    )
    .await
}
pub(crate) async fn update_type(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    id: i64,
    update: &ResourceTypeUpdate,
) -> Result<ResourceType> {
    let mut row: ResourceType =
        store::fetch(tx.sql(), scope, company, Kind::ResourceType, id).await?;
    if row.retired {
        return Err(Error::ResourceTypeRetired);
    }
    if let Some(definitions) = &update.property_definitions {
        let definitions = Inventory::normalize_property_definitions(definitions)?;
        if definitions != row.property_definitions && in_use(tx, scope, company, id, false).await? {
            return Err(Error::ResourceTypeInUse);
        }
        row.property_definitions = definitions;
    }
    if let Some(code) = &update.code {
        row.code = code.trim().to_uppercase();
    }
    if let Some(name) = &update.name {
        row.name = name.clone();
    }
    store::names(
        tx.sql(),
        scope,
        company,
        Kind::ResourceType,
        Some(id),
        &row.code,
        &row.name,
    )
    .await?;
    store::update(tx, scope, company, Kind::ResourceType, id, json!(row)).await
}
pub(crate) async fn retire_type(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<ResourceType> {
    let mut row: ResourceType =
        store::fetch(tx.sql(), scope, company, Kind::ResourceType, id).await?;
    if row.retired {
        return Err(Error::ResourceTypeRetired);
    }
    if in_use(tx, scope, company, id, true).await? {
        return Err(Error::ResourceTypeInUse);
    }
    row.retired = true;
    store::update(tx, scope, company, Kind::ResourceType, id, json!(row)).await
}
async fn in_use(
    tx: &MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    id: i64,
    active_only: bool,
) -> Result<bool> {
    Ok(tx.sql().query_opt("SELECT 1 FROM factory_resources WHERE tenant_id=$1 AND company_id=$2 AND resource_type_id=$3 AND (NOT $4 OR retired_at IS NULL) LIMIT 1",&[&scope.tenant(),&company,&id,&active_only]).await?.is_some())
}
pub(crate) async fn create(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    new: &NewResource,
) -> Result<Resource> {
    let kind: ResourceType = store::fetch(
        tx.sql(),
        scope,
        company,
        Kind::ResourceType,
        new.resource_type_id,
    )
    .await?;
    if kind.retired {
        return Err(Error::ResourceTypeRetired);
    }
    let properties =
        Inventory::validate_property_values(&kind.property_definitions, &new.properties)?;
    store::names(
        tx.sql(),
        scope,
        company,
        Kind::Resource,
        None,
        &new.code,
        &new.name,
    )
    .await?;
    store::insert(
        tx,
        scope,
        company,
        Kind::Resource,
        json!({"code":new.code,"name":new.name,"resource_type_id":kind.id,"properties":properties}),
    )
    .await
}
pub(crate) async fn update(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    id: i64,
    update: &ResourceUpdate,
) -> Result<Resource> {
    let mut row: Resource = store::fetch(tx.sql(), scope, company, Kind::Resource, id).await?;
    if row.retired {
        return Err(Error::ResourceRetired);
    }
    let kind: ResourceType = store::fetch(
        tx.sql(),
        scope,
        company,
        Kind::ResourceType,
        row.resource_type_id,
    )
    .await?;
    if let Some(values) = &update.properties {
        row.properties = Inventory::validate_property_values(&kind.property_definitions, values)?;
    }
    if let Some(code) = &update.code {
        row.code = code.clone();
    }
    if let Some(name) = &update.name {
        row.name = name.clone();
    }
    store::names(
        tx.sql(),
        scope,
        company,
        Kind::Resource,
        Some(id),
        &row.code,
        &row.name,
    )
    .await?;
    store::update(tx, scope, company, Kind::Resource, id, json!(row)).await
}
pub(crate) async fn retire(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Resource> {
    let mut row: Resource = store::fetch(tx.sql(), scope, company, Kind::Resource, id).await?;
    if row.retired {
        return Err(Error::ResourceRetired);
    }
    row.retired = true;
    store::update(tx, scope, company, Kind::Resource, id, json!(row)).await
}
