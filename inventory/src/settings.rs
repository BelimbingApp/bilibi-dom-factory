//! A company's item master settings (Bilimbi `Inventory.item_settings/2`,
//! `configure_item_settings/3`, `item_setting_overrides/2`), resolved through
//! Base Settings for the company, then its tenant.
use crate::{
    access::{readable, writable},
    contributions::{currency_valid, statuses_valid, DEFAULT_CURRENCY_KEY, ITEM_STATUSES_KEY},
    error::Error,
};
use bilibi_database::{Error as DatabaseError, MutationTransaction};
use bilibi_party_ids::OrganisationId;
use bilibi_settings::{Resolved, Settings, Target};
use bilibi_tenancy::Scope;
use serde_json::Value;
use tokio_postgres::GenericClient;

/// The company's item settings.
///
/// * `statuses`: the status vocabulary, an ordered list whose first entry is
///   the default status, or `None` when none is configured and any non-blank
///   status is accepted.
/// * `default_currency_code`: the currency an item takes when none is given,
///   or `None`.
///
/// Inventory holds no vocabulary or currency of its own; both are the
/// company's configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemSettings {
    pub statuses: Option<Vec<String>>,
    pub default_currency_code: Option<String>,
}

fn target(company_id: i64) -> Result<Target, Error> {
    OrganisationId::new(company_id)
        .map(Target::Company)
        .map_err(|_| Error::CompanyNotFound)
}

fn statuses_of(value: &Value) -> Result<Option<Vec<String>>, Error> {
    if !statuses_valid(value) {
        return Err(DatabaseError::Internal.into());
    }
    Ok(value.as_array().map(|statuses| {
        statuses
            .iter()
            .filter_map(|status| status.as_str().map(str::to_owned))
            .collect()
    }))
}
fn currency_of(value: &Value) -> Result<Option<String>, Error> {
    if !currency_valid(value) {
        return Err(DatabaseError::Internal.into());
    }
    Ok(value.as_str().map(str::to_owned))
}

async fn resolve(
    client: &(impl GenericClient + Sync),
    settings: &Settings,
    scope: &Scope,
    company_id: i64,
) -> Result<(Resolved, Resolved), Error> {
    let target = target(company_id)?;
    Ok((
        settings
            .get(client, scope, target, ITEM_STATUSES_KEY)
            .await?,
        settings
            .get(client, scope, target, DEFAULT_CURRENCY_KEY)
            .await?,
    ))
}

pub(crate) async fn effective(
    client: &(impl GenericClient + Sync),
    settings: &Settings,
    scope: &Scope,
    company_id: i64,
) -> Result<ItemSettings, Error> {
    let (statuses, currency) = resolve(client, settings, scope, company_id).await?;
    Ok(ItemSettings {
        statuses: statuses_of(&statuses.value)?,
        default_currency_code: currency_of(&currency.value)?,
    })
}

pub(crate) async fn read(
    client: &(impl GenericClient + Sync),
    settings: &Settings,
    scope: &Scope,
    company_id: i64,
) -> Result<ItemSettings, Error> {
    readable(client, scope, company_id).await?;
    effective(client, settings, scope, company_id).await
}

/// The company's own overrides: each field is the company's value, or `None`
/// where the company inherits its tenant's setting.
pub(crate) async fn overrides(
    client: &(impl GenericClient + Sync),
    settings: &Settings,
    scope: &Scope,
    company_id: i64,
) -> Result<ItemSettings, Error> {
    readable(client, scope, company_id).await?;
    let (statuses, currency) = resolve(client, settings, scope, company_id).await?;
    Ok(ItemSettings {
        statuses: if statuses.overridden {
            statuses_of(&statuses.value)?
        } else {
            None
        },
        default_currency_code: if currency.overridden {
            currency_of(&currency.value)?
        } else {
            None
        },
    })
}

/// Configures the company's item statuses and default currency as one write.
/// `None` removes the company's override.
pub(crate) async fn configure(
    tx: &mut MutationTransaction<'_>,
    settings: &Settings,
    scope: &Scope,
    company_id: i64,
    new: &ItemSettings,
) -> Result<ItemSettings, Error> {
    writable(tx.sql(), scope, company_id).await?;
    let statuses = new
        .statuses
        .as_ref()
        .map_or(Value::Null, |statuses| Value::from(statuses.clone()));
    if !statuses_valid(&statuses) {
        return Err(Error::InvalidItemStatuses);
    }
    let currency = new
        .default_currency_code
        .as_ref()
        .map_or(Value::Null, |code| Value::from(code.clone()));
    if !currency_valid(&currency) {
        return Err(Error::InvalidDefaultCurrency);
    }
    let target = target(company_id)?;
    for (key, value) in [
        (ITEM_STATUSES_KEY, statuses),
        (DEFAULT_CURRENCY_KEY, currency),
    ] {
        if value.is_null() {
            settings.delete(tx, scope, target, key).await?;
        } else {
            settings.put(tx, scope, target, key, value).await?;
        }
    }
    effective(tx.sql(), settings, scope, company_id).await
}
