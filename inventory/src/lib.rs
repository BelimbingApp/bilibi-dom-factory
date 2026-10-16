//! Factory Inventory (`factory/inventory`): the company item master settings,
//! units of measure and stock locations behind one facade, [`Inventory`].
//! Schema, SQL, validation and helpers stay private to this crate; later
//! slices add the catalog, the Material Transaction ledger and genealogy
//! behind the same facade.
//!
//! Every operation takes the sealed [`Scope`] and a company ID. A read needs
//! `factory.inventory.configuration.view` and a live company of the scope's
//! tenant; a write needs `factory.inventory.configuration.manage`, the
//! company as an authorized target, and a company that is not archived, and
//! runs in one audited `MutationBoundary` transaction. A record of another
//! company is reported as not found.
use bilibi_database::{Db, MutationBoundary};
use bilibi_module_registry::{Layer, Module, Service};
use bilibi_settings::Settings;
use bilibi_tenancy::Scope;
use std::sync::Arc;

mod access;
mod contributions;
mod error;
mod locations;
mod settings;
mod units;

pub use error::{Error, FieldError};
pub use locations::{Location, NewLocation};
pub use settings::ItemSettings;
pub use units::{NewUnit, Unit};

/// The service ID of [`Inventory`].
pub const SERVICE: &str = "factory/inventory.inventory";

const DEFAULT_LIMIT: i64 = 100;
const MAXIMUM_LIMIT: i64 = 500;

/// A list's row cap: 100 by default, 500 at most.
fn limit(requested: Option<i64>) -> Result<i64> {
    match requested.unwrap_or(DEFAULT_LIMIT) {
        limit @ 1..=MAXIMUM_LIMIT => Ok(limit),
        _ => Err(bilibi_database::Error::Invalid.into()),
    }
}

pub struct Inventory {
    db: Db,
    mutations: Arc<MutationBoundary>,
    settings: Arc<Settings>,
}

type Result<T> = std::result::Result<T, Error>;

/// A pooled connection for a read, as `&tokio_postgres::Client`.
macro_rules! connection {
    ($self:ident, $name:ident) => {
        let $name = $self
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        let $name: &tokio_postgres::Client = &$name;
    };
}
/// An audited write: one `MutationBoundary` transaction around `$write`,
/// committed only when it saved.
macro_rules! audited {
    ($self:ident, $scope:ident, $action:literal, |$tx:ident| $write:expr) => {{
        let mut client = $self
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        let mut $tx = $self
            .mutations
            .begin(&mut client, $scope.mutation_context($action))
            .await?;
        let saved = $write;
        match saved {
            Ok(saved) => {
                $tx.commit().await?;
                Ok(saved)
            }
            Err(error) => {
                $tx.rollback().await?;
                Err(error)
            }
        }
    }};
}

impl Inventory {
    /// The company's item settings: its own overrides, else its tenant's.
    pub async fn item_settings(&self, scope: &Scope, company_id: i64) -> Result<ItemSettings> {
        connection!(self, client);
        settings::read(client, &self.settings, scope, company_id).await
    }

    /// The company's own overrides; a field the company inherits is `None`.
    pub async fn item_setting_overrides(
        &self,
        scope: &Scope,
        company_id: i64,
    ) -> Result<ItemSettings> {
        connection!(self, client);
        settings::overrides(client, &self.settings, scope, company_id).await
    }

    /// Configures the company's item statuses and default currency as one
    /// write. `None` removes an override.
    pub async fn configure_item_settings(
        &self,
        scope: &Scope,
        company_id: i64,
        new: &ItemSettings,
    ) -> Result<ItemSettings> {
        audited!(self, scope, "factory.inventory.configure_item_settings", |tx| {
            settings::configure(&mut tx, &self.settings, scope, company_id, new).await
        })
    }

    /// The company's units ordered by code, at most `limit` (default 100,
    /// maximum 500).
    pub async fn list_units(
        &self,
        scope: &Scope,
        company_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Unit>> {
        connection!(self, client);
        units::list(client, scope, company_id, limit).await
    }

    pub async fn get_unit(&self, scope: &Scope, company_id: i64, unit_id: i64) -> Result<Unit> {
        connection!(self, client);
        units::get(client, scope, company_id, unit_id).await
    }

    pub async fn create_unit(
        &self,
        scope: &Scope,
        company_id: i64,
        new: &NewUnit,
    ) -> Result<Unit> {
        audited!(self, scope, "factory.inventory.create_unit", |tx| {
            units::create(&mut tx, scope, company_id, new).await
        })
    }

    /// Renames a unit. Its code stays stable in conversion and posting history.
    pub async fn rename_unit(
        &self,
        scope: &Scope,
        company_id: i64,
        unit_id: i64,
        name: &str,
    ) -> Result<Unit> {
        audited!(self, scope, "factory.inventory.rename_unit", |tx| {
            units::rename(&mut tx, scope, company_id, unit_id, name).await
        })
    }

    /// Retires a unit while preserving its id in conversions and postings.
    pub async fn retire_unit(&self, scope: &Scope, company_id: i64, unit_id: i64) -> Result<Unit> {
        audited!(self, scope, "factory.inventory.retire_unit", |tx| {
            units::retire(&mut tx, scope, company_id, unit_id).await
        })
    }

    /// The company's stock locations ordered by code, at most `limit`
    /// (default 100, maximum 500).
    pub async fn list_locations(
        &self,
        scope: &Scope,
        company_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Location>> {
        connection!(self, client);
        locations::list(client, scope, company_id, limit).await
    }

    pub async fn get_location(
        &self,
        scope: &Scope,
        company_id: i64,
        location_id: i64,
    ) -> Result<Location> {
        connection!(self, client);
        locations::get(client, scope, company_id, location_id).await
    }

    pub async fn create_location(
        &self,
        scope: &Scope,
        company_id: i64,
        new: &NewLocation,
    ) -> Result<Location> {
        audited!(self, scope, "factory.inventory.create_location", |tx| {
            locations::create(&mut tx, scope, company_id, new).await
        })
    }
}

inventory::submit! {
    Module {
        id: "factory/inventory",
        layer: Layer::Domains,
        dependencies: &[
            "base/authz", "base/database", "base/menu", "base/module_registry", "base/party_ids",
            "base/settings", "base/tenancy", "core/company",
        ],
        migrations: &[(
            "factory-inventory-001-units-locations",
            include_str!("../migrations/001-units-locations.sql"),
        )],
        services: &[Service {
            id: SERVICE,
            factory: |services| {
                Ok(Arc::new(Inventory {
                    db: services.get::<Db>("base/database.pool")?.as_ref().clone(),
                    mutations: services.get("base/database.mutations")?,
                    settings: services.get("base/settings.store")?,
                }))
            },
        }],
    }
}
