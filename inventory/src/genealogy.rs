//! Bilimbi Inventory.Genealogy: immutable lot and unit identities, their
//! current positions, and the transform links between them, walked in either
//! direction across any number of transforms.
use crate::{
    access,
    ledger::{self, MaterialTransaction},
    ledger_request::{Dimension, DimensionName, IdentityKind},
    Error, Location, Unit,
};
use bilibi_tenancy::Scope;
use std::collections::{BTreeMap, BTreeSet};
use tokio_postgres::GenericClient;

/// An identified lot or individual unit of one item's material. Its code,
/// kind, dimensions and source transaction never change.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub id: i64,
    pub item_id: i64,
    pub kind: IdentityKind,
    pub code: String,
    pub source_transaction_id: i64,
    pub dimensions: BTreeMap<DimensionName, Dimension>,
}

/// A positive stock balance of one identity at one location.
#[derive(Clone, Debug)]
pub struct IdentityPosition {
    pub location: Location,
    /// Exact sum of native quantities (decimal text).
    pub quantity: String,
    pub unit: Unit,
}

/// One transform edge between identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TraceLink {
    pub input_identity_id: i64,
    pub output_identity_id: i64,
    pub transaction_id: i64,
}

/// What a trace reached from its root: every visited identity (by ID), the
/// links walked, and the receipts among the identities' source transactions.
#[derive(Clone, Debug)]
pub struct Trace {
    pub root: Identity,
    pub identities: Vec<Identity>,
    pub links: Vec<TraceLink>,
    pub receipts: Vec<MaterialTransaction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Backward,
    Forward,
}

pub(crate) async fn get(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Identity, Error> {
    access::readable(client, scope, company).await?;
    get_authorized(client, scope, company, id).await
}

pub(crate) async fn get_authorized(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Identity, Error> {
    fetch(client, scope.tenant(), company, &[id])
        .await?
        .pop()
        .ok_or(Error::IdentityNotFound)
}

/// When stock was last added to the identity at or before `until`: the latest
/// effective time of a receipt, output or transform that put a positive
/// quantity of it into stock. An addition to an existing identity counts, so
/// the identity's newest quantity is never aged from its first posting.
pub(crate) async fn last_added(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
    until: std::time::SystemTime,
) -> Result<Option<std::time::SystemTime>, Error> {
    access::readable(client, scope, company).await?;
    last_added_authorized(client, scope, company, id, until).await
}

pub(crate) async fn last_added_authorized(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
    until: std::time::SystemTime,
) -> Result<Option<std::time::SystemTime>, Error> {
    Ok(client.query_one("SELECT max(t.effective_at) FROM factory_inventory_transactions t JOIN factory_inventory_transaction_entries e ON e.tenant_id=t.tenant_id AND e.company_id=t.company_id AND e.transaction_id=t.id WHERE t.tenant_id=$1 AND t.company_id=$2 AND e.identity_id=$3 AND e.role='stock' AND e.native_quantity>0 AND t.kind IN ('receipt','output','transform') AND t.effective_at<=$4", &[&scope.tenant(), &company, &id, &until]).await?.get(0))
}

async fn fetch(
    client: &(impl GenericClient + Sync),
    tenant: i64,
    company: i64,
    ids: &[i64],
) -> Result<Vec<Identity>, Error> {
    let rows = client.query("SELECT i.id,m.item_id,i.kind,i.code,i.source_transaction_id,i.dimensions FROM factory_inventory_identities i JOIN factory_inventory_materials m ON m.company_id=i.company_id AND m.id=i.material_id WHERE i.tenant_id=$1 AND i.company_id=$2 AND i.id=ANY($3) ORDER BY i.id", &[&tenant, &company, &ids]).await?;
    rows.iter()
        .map(|row| {
            Ok(Identity {
                id: row.get("id"),
                item_id: row.get("item_id"),
                kind: if row.get::<_, &str>("kind") == "lot" {
                    IdentityKind::Lot
                } else {
                    IdentityKind::Unit
                },
                code: row.get("code"),
                source_transaction_id: row.get("source_transaction_id"),
                dimensions: serde_json::from_value(row.get("dimensions"))
                    .map_err(|_| bilibi_database::Error::Invalid)?,
            })
        })
        .collect()
}

pub(crate) async fn positions(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Vec<IdentityPosition>, Error> {
    let identity = get(client, scope, company, id).await?;
    let rows = client.query("SELECT l.id AS location_id,l.code AS location_code,l.name AS location_name,u.id AS unit_id,u.code AS unit_code,u.name AS unit_name,u.retired_at IS NOT NULL AS unit_retired,sum(e.native_quantity)::text AS quantity FROM factory_inventory_transaction_entries e JOIN factory_inventory_locations l ON l.company_id=e.company_id AND l.id=e.location_id JOIN factory_inventory_units u ON u.company_id=e.company_id AND u.id=e.native_unit_id WHERE e.tenant_id=$1 AND e.company_id=$2 AND e.identity_id=$3 AND e.role='stock' GROUP BY l.id,u.id HAVING sum(e.native_quantity)>0 ORDER BY l.id", &[&scope.tenant(), &company, &identity.id]).await?;
    Ok(rows
        .iter()
        .map(|row| IdentityPosition {
            location: Location {
                id: row.get("location_id"),
                company_id: company,
                code: row.get("location_code"),
                name: row.get("location_name"),
            },
            quantity: row.get("quantity"),
            unit: Unit {
                id: row.get("unit_id"),
                company_id: company,
                code: row.get("unit_code"),
                name: row.get("unit_name"),
                retired: row.get("unit_retired"),
            },
        })
        .collect())
}

pub(crate) async fn trace(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
    direction: Direction,
) -> Result<Trace, Error> {
    access::readable(client, scope, company).await?;
    trace_authorized(client, scope, company, id, direction).await
}

pub(crate) async fn trace_authorized(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
    direction: Direction,
) -> Result<Trace, Error> {
    let root = get_authorized(client, scope, company, id).await?;
    let tenant = scope.tenant();
    let (from, to) = match direction {
        Direction::Backward => ("o", "i"),
        Direction::Forward => ("i", "o"),
    };
    let sql = format!(
        "SELECT i.identity_id,o.identity_id,l.transaction_id FROM factory_inventory_genealogy_links l JOIN factory_inventory_transaction_entries i ON i.company_id=l.company_id AND i.id=l.input_entry_id JOIN factory_inventory_transaction_entries o ON o.company_id=l.company_id AND o.id=l.output_entry_id WHERE l.tenant_id=$1 AND l.company_id=$2 AND {from}.identity_id=ANY($3) AND {to}.identity_id IS NOT NULL"
    );
    let mut visited = BTreeSet::from([root.id]);
    let mut frontier = vec![root.id];
    let mut links = BTreeSet::new();
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for row in client.query(&sql, &[&tenant, &company, &frontier]).await? {
            let link = TraceLink {
                input_identity_id: row.get(0),
                output_identity_id: row.get(1),
                transaction_id: row.get(2),
            };
            let reached = match direction {
                Direction::Backward => link.input_identity_id,
                Direction::Forward => link.output_identity_id,
            };
            if visited.insert(reached) {
                next.push(reached);
            }
            links.insert(link);
        }
        frontier = next;
    }
    let ids: Vec<i64> = visited.into_iter().collect();
    let identities = fetch(client, tenant, company, &ids).await?;
    let sources: Vec<i64> = identities
        .iter()
        .map(|identity| identity.source_transaction_id)
        .collect();
    let receipts = ledger::read_selected(client, tenant, company, "SELECT id FROM factory_inventory_transactions WHERE tenant_id=$1 AND company_id=$2 AND kind='receipt' AND id=ANY($3) ORDER BY id", &[&sources]).await?;
    Ok(Trace {
        root,
        identities,
        links: links.into_iter().collect(),
        receipts,
    })
}

/// The consumptions and transforms that drew the identity, in ID order.
pub(crate) async fn draws(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Vec<MaterialTransaction>, Error> {
    let identity = get(client, scope, company, id).await?;
    ledger::read_selected(client, scope.tenant(), company, "SELECT t.id FROM factory_inventory_transactions t WHERE t.tenant_id=$1 AND t.company_id=$2 AND t.kind IN ('consumption','transform') AND EXISTS(SELECT 1 FROM factory_inventory_transaction_entries e WHERE e.company_id=t.company_id AND e.transaction_id=t.id AND e.role='stock' AND e.identity_id=$3 AND e.native_quantity<0) ORDER BY t.id", &[&identity.id]).await
}
