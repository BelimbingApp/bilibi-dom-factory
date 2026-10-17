//! The two gates every operation passes, in the facade, before its own work:
//! a read needs its view capability and a live company of the scope's tenant
//! (Bilimbi `live_company/2`); a write needs its manage capability, the
//! company as an authorized target, and takes the company row so an archived
//! company is judged as the write will see it (`lock_writable_company`).
//!
//! Units, locations, material types, materials, conversions and item
//! settings are configuration (`factory.inventory.configuration.*`); the item
//! master is gated by Belimbing's `commerce.inventory.item.*` keys. An
//! operation that writes both, such as creating a material with its item,
//! needs both.
use crate::{
    contributions::{CONFIGURATION_MANAGE, CONFIGURATION_VIEW},
    error::{company, Error},
};
use bilibi_authz::authorize_key;
use bilibi_company::organisation::{authorize_target, require_live, require_writable};
use bilibi_tenancy::Scope;
use tokio_postgres::GenericClient;

pub(crate) async fn readable(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
) -> Result<(), Error> {
    readable_for(client, scope, company_id, CONFIGURATION_VIEW).await
}

pub(crate) async fn readable_for(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
    capability: &str,
) -> Result<(), Error> {
    authorize_key(client, scope, capability).await?;
    require_live(client, scope, company_id)
        .await
        .map_err(company)?;
    Ok(())
}

pub(crate) async fn writable(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
) -> Result<(), Error> {
    writable_for(client, scope, company_id, CONFIGURATION_MANAGE).await
}

pub(crate) async fn writable_for(
    client: &impl GenericClient,
    scope: &Scope,
    company_id: i64,
    capability: &str,
) -> Result<(), Error> {
    authorize_target(client, scope, company_id, capability)
        .await
        .map_err(company)?;
    require_writable(client, scope, company_id, true).await?;
    Ok(())
}
