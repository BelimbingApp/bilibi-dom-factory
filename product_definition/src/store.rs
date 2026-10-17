//! Private persistence for ProductDefinition. Identifiers come only from Kind,
//! never request data; every statement carries tenant and company predicates.
use crate::{Error, Result};
use bilibi_database::{Change, MutationTransaction};
use bilibi_tenancy::Scope;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio_postgres::GenericClient;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Product,
    ResourceType,
    Resource,
    Formula,
    Routing,
}
impl Kind {
    fn table(self) -> &'static str {
        match self {
            Self::Product => "factory_products",
            Self::ResourceType => "factory_resource_types",
            Self::Resource => "factory_resources",
            Self::Formula => "factory_formula_revisions",
            Self::Routing => "factory_routing_revisions",
        }
    }
    fn projection(self) -> &'static str {
        match self {
            Self::ResourceType | Self::Resource => {
                "to_jsonb(t) || jsonb_build_object('retired',t.retired_at IS NOT NULL)"
            }
            _ => "to_jsonb(t)",
        }
    }
    fn missing(self) -> Error {
        match self {
            Self::Product => Error::ProductNotFound,
            Self::ResourceType => Error::ResourceTypeNotFound,
            Self::Resource => Error::ResourceNotFound,
            Self::Formula => Error::FormulaRevisionNotFound,
            Self::Routing => Error::RoutingRevisionNotFound,
        }
    }
}

pub(crate) async fn fetch<T: DeserializeOwned>(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    kind: Kind,
    id: i64,
) -> Result<T> {
    let sql = format!(
        "SELECT {} FROM {} t WHERE tenant_id=$1 AND company_id=$2 AND id=$3",
        kind.projection(),
        kind.table()
    );
    let row = client
        .query_opt(&sql, &[&scope.tenant(), &company, &id])
        .await?
        .ok_or_else(|| kind.missing())?;
    Ok(serde_json::from_value(row.get(0))?)
}

pub(crate) async fn list<T: DeserializeOwned>(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    kind: Kind,
    product: Option<i64>,
    limit: i64,
) -> Result<Vec<T>> {
    let suffix = if product.is_some() {
        "AND product_id=$4 ORDER BY version"
    } else {
        "ORDER BY code"
    };
    let sql = format!(
        "SELECT {} FROM {} t WHERE tenant_id=$1 AND company_id=$2 {suffix} LIMIT $3",
        kind.projection(),
        kind.table()
    );
    let tenant = scope.tenant();
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        vec![&tenant, &company, &limit];
    if let Some(ref product) = product {
        params.push(product);
    }
    client
        .query(&sql, &params)
        .await?
        .iter()
        .map(|row| Ok(serde_json::from_value(row.get(0))?))
        .collect()
}

pub(crate) async fn revision<T: DeserializeOwned>(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    kind: Kind,
    product: i64,
    version: i32,
) -> Result<T> {
    let sql = format!("SELECT to_jsonb(t) FROM {} t WHERE tenant_id=$1 AND company_id=$2 AND product_id=$3 AND version=$4",kind.table());
    let row = client
        .query_opt(&sql, &[&scope.tenant(), &company, &product, &version])
        .await?
        .ok_or_else(|| kind.missing())?;
    Ok(serde_json::from_value(row.get(0))?)
}

pub(crate) async fn next_version(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    kind: Kind,
    product: i64,
) -> Result<i32> {
    // Publication already holds the company lock, shared with Inventory writes.
    client.query_one("SELECT id FROM factory_products WHERE tenant_id=$1 AND company_id=$2 AND id=$3 FOR UPDATE", &[&scope.tenant(),&company,&product]).await?;
    let sql = format!("SELECT COALESCE(max(version),0)+1 FROM {} WHERE tenant_id=$1 AND company_id=$2 AND product_id=$3",kind.table());
    Ok(client
        .query_one(&sql, &[&scope.tenant(), &company, &product])
        .await?
        .get(0))
}

pub(crate) async fn names(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    kind: Kind,
    except: Option<i64>,
    code: &str,
    name: &str,
) -> Result<()> {
    for (field, value, max) in [("Code", code, 64), ("Name", name, 255)] {
        let message = if value.trim().is_empty() {
            Some("can't be blank")
        } else if value.chars().count() > max {
            Some(if max == 64 {
                "should be at most 64 character(s)"
            } else {
                "should be at most 255 character(s)"
            })
        } else {
            None
        };
        if let Some(message) = message {
            return Err(Error::Invalid { field, message });
        }
    }
    let sql = format!("SELECT 1 FROM {} WHERE tenant_id=$1 AND company_id=$2 AND code=$3 AND id IS DISTINCT FROM $4",kind.table());
    if client
        .query_opt(&sql, &[&scope.tenant(), &company, &code, &except])
        .await?
        .is_some()
    {
        return Err(Error::Invalid {
            field: "Code",
            message: "has already been taken",
        });
    }
    Ok(())
}

/// Insert one already validated row; audit the complete stored row.
pub(crate) async fn insert<T: DeserializeOwned>(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    kind: Kind,
    data: Value,
) -> Result<T> {
    let (columns,values) = match kind {
        Kind::Product => ("item_id,code,name", "($3::jsonb->>'item_id')::bigint,$3::jsonb->>'code',$3::jsonb->>'name'"),
        Kind::ResourceType => ("code,name,property_definitions", "$3::jsonb->>'code',$3::jsonb->>'name',$3::jsonb->'property_definitions'"),
        Kind::Resource => ("code,name,resource_type_id,properties", "$3::jsonb->>'code',$3::jsonb->>'name',($3::jsonb->>'resource_type_id')::bigint,$3::jsonb->'properties'"),
        Kind::Formula => ("product_id,version,lines,process_config", "($3::jsonb->>'product_id')::bigint,($3::jsonb->>'version')::integer,$3::jsonb->'lines',$3::jsonb->'process_config'"),
        Kind::Routing => ("product_id,version,operations,process_config", "($3::jsonb->>'product_id')::bigint,($3::jsonb->>'version')::integer,$3::jsonb->'operations',$3::jsonb->'process_config'"),
    };
    let sql = format!("INSERT INTO {} AS t (tenant_id,company_id,{columns}) VALUES($1,$2,{values}) RETURNING {},id",kind.table(),kind.projection());
    let tenant = scope.tenant();
    let value = tx
        .mutate(move |client| {
            Box::pin(async move {
                let row = client.query_one(&sql, &[&tenant, &company, &data]).await?;
                let value: Value = row.get(0);
                Ok((
                    value.clone(),
                    vec![Change {
                        kind: kind.table().into(),
                        id: row.get(1),
                        old: json!({}),
                        new: value,
                    }],
                ))
            })
        })
        .await?;
    Ok(serde_json::from_value(value)?)
}

/// Only resource types/resources have an update path. Revisions have none.
pub(crate) async fn update<T: DeserializeOwned>(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    kind: Kind,
    id: i64,
    data: Value,
) -> Result<T> {
    let fields = match kind {
        Kind::ResourceType => {
            "code=$4::jsonb->>'code',name=$4::jsonb->>'name',property_definitions=$4::jsonb->'property_definitions'"
        }
        Kind::Resource => "code=$4::jsonb->>'code',name=$4::jsonb->>'name',properties=$4::jsonb->'properties'",
        _ => unreachable!("only resource types and resources are mutable"),
    };
    let old: Value = fetch(tx.sql(), scope, company, kind, id).await?;
    let sql=format!("UPDATE {} t SET {fields},retired_at=CASE WHEN ($4->>'retired')::boolean THEN now() ELSE NULL END,updated_at=now() WHERE tenant_id=$1 AND company_id=$2 AND id=$3 RETURNING {}",kind.table(),kind.projection());
    let tenant = scope.tenant();
    let value = tx
        .mutate(move |client| {
            Box::pin(async move {
                let row = client
                    .query_one(&sql, &[&tenant, &company, &id, &data])
                    .await?;
                let value: Value = row.get(0);
                Ok((
                    value.clone(),
                    vec![Change {
                        kind: kind.table().into(),
                        id,
                        old,
                        new: value,
                    }],
                ))
            })
        })
        .await?;
    Ok(serde_json::from_value(value)?)
}
