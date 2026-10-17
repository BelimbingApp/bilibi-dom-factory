//! Factory Inventory (`factory/inventory`): the item master and its company
//! settings, units of measure, stock locations, material types, material
//! identity and versioned unit conversions behind one facade, [`Inventory`].
//! Schema, SQL, validation and helpers stay private to this crate; later
//! slices add genealogy behind the same facade. The Material Transaction
//! ledger records stock movements without changing earlier observations.
//!
//! Every operation takes the sealed [`Scope`] and a company ID. A read needs
//! its view capability (`factory.inventory.configuration.view`, or
//! `commerce.inventory.item.list`/`.view` for the item master) and a live
//! company of the scope's tenant; a write needs its manage capability
//! (`factory.inventory.configuration.manage`, or
//! `commerce.inventory.item.create`/`.update`), the company as an authorized
//! target, and a company that is not archived, and runs in one audited
//! `MutationBoundary` transaction. A record of another company is reported as
//! not found.
use bilibi_database::{Db, MutationBoundary};
use bilibi_module_registry::{Layer, Module, Service};
use bilibi_settings::Settings;
use bilibi_tenancy::Scope;
use std::sync::Arc;

mod access;
mod contributions;
mod conversions;
mod error;
mod items;
mod ledger;
mod ledger_request;
mod locations;
mod material_types;
mod materials;
mod property;
mod settings;
mod units;

pub use conversions::Conversion;
pub use error::{Error, FieldError};
pub use items::{Item, ItemUpdate, NewItem};
pub use ledger::{Entry, MaterialTransaction, StockPosition};
use ledger_request::TransactionKind;
pub use ledger_request::{Observation, PostingRequest, ReceiptMeasurement, StockLine};
pub use locations::{Location, NewLocation};
pub use material_types::{MaterialType, MaterialTypeUpdate, NewMaterialType};
pub use materials::{Material, MaterialTyping};
pub use property::{PropertyDefinition, VALUE_TYPES};
pub use rust_decimal::Decimal;
pub use serde_json::{Map, Value};
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
        let $name = $self.db.get().await.map_err(bilibi_database::Error::from)?;
        let $name: &tokio_postgres::Client = &$name;
    };
}
/// An audited write: one `MutationBoundary` transaction around `$write`,
/// committed only when it saved.
macro_rules! audited {
    ($self:ident, $scope:ident, $action:literal, |$tx:ident| $write:expr) => {{
        let mut client = $self.db.get().await.map_err(bilibi_database::Error::from)?;
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
    /// Receives stock with source evidence and an optional weigh ticket.
    pub async fn record_receipt(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        audited!(self, scope, "factory.inventory.record_receipt", |tx| {
            ledger::post(
                &mut tx,
                scope,
                company_id,
                TransactionKind::Receipt,
                request,
            )
            .await
        })
    }

    /// Moves stock between distinct locations, preserving its recorded basis.
    pub async fn record_transfer(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        audited!(self, scope, "factory.inventory.record_transfer", |tx| {
            ledger::post(
                &mut tx,
                scope,
                company_id,
                TransactionKind::Transfer,
                request,
            )
            .await
        })
    }

    /// Draws stock under locks; concurrent requests cannot spend it twice.
    pub async fn record_consumption(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        audited!(self, scope, "factory.inventory.record_consumption", |tx| {
            ledger::post(
                &mut tx,
                scope,
                company_id,
                TransactionKind::Consumption,
                request,
            )
            .await
        })
    }

    /// Appends signed adjustments with a reason and an original transaction.
    /// Corrections may adjust a retired material; ordinary postings may not.
    pub async fn record_correction(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        audited!(self, scope, "factory.inventory.record_correction", |tx| {
            ledger::post(
                &mut tx,
                scope,
                company_id,
                TransactionKind::Correction,
                request,
            )
            .await
        })
    }

    pub async fn get_transaction(
        &self,
        scope: &Scope,
        company_id: i64,
        transaction_id: i64,
    ) -> Result<MaterialTransaction> {
        connection!(self, client);
        ledger::get(client, scope, company_id, transaction_id).await
    }

    /// Most recent IDs first, optionally containing one item (100, at most 500).
    pub async fn list_transactions(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: Option<i64>,
        limit: Option<i64>,
    ) -> Result<Vec<MaterialTransaction>> {
        connection!(self, client);
        ledger::list(client, scope, company_id, item_id, limit).await
    }

    pub async fn get_stock_position(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
        location_id: i64,
    ) -> Result<StockPosition> {
        connection!(self, client);
        ledger::position(client, scope, company_id, item_id, location_id).await
    }

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
        audited!(
            self,
            scope,
            "factory.inventory.configure_item_settings",
            |tx| { settings::configure(&mut tx, &self.settings, scope, company_id, new).await }
        )
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

    pub async fn create_unit(&self, scope: &Scope, company_id: i64, new: &NewUnit) -> Result<Unit> {
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
    /// The company's items ordered by SKU, optionally of one `status`, at
    /// most `limit` (default 100, maximum 500).
    pub async fn list_items(
        &self,
        scope: &Scope,
        company_id: i64,
        status: Option<&str>,
        limit: Option<i64>,
    ) -> Result<Vec<Item>> {
        connection!(self, client);
        items::list(client, scope, company_id, status, limit).await
    }

    pub async fn get_item(&self, scope: &Scope, company_id: i64, item_id: i64) -> Result<Item> {
        connection!(self, client);
        items::get(client, scope, company_id, item_id).await
    }

    /// Reads an item by SKU. SKUs are stored upper-case, so the lookup is too.
    pub async fn get_item_by_sku(&self, scope: &Scope, company_id: i64, sku: &str) -> Result<Item> {
        connection!(self, client);
        items::get_by_sku(client, scope, company_id, sku).await
    }

    /// Creates an item master row with Belimbing's create rules and the
    /// company's item settings ([`Inventory::item_settings`]).
    pub async fn create_item(&self, scope: &Scope, company_id: i64, new: &NewItem) -> Result<Item> {
        audited!(self, scope, "factory.inventory.create_item", |tx| {
            items::create(&mut tx, &self.settings, scope, company_id, new).await
        })
    }

    /// Defines a company material type. Its property definitions stop
    /// changing once a material uses it, so recorded values keep their meaning.
    pub async fn create_material_type(
        &self,
        scope: &Scope,
        company_id: i64,
        new: &NewMaterialType,
    ) -> Result<MaterialType> {
        audited!(
            self,
            scope,
            "factory.inventory.create_material_type",
            |tx| { material_types::create(&mut tx, scope, company_id, new).await }
        )
    }

    pub async fn get_material_type(
        &self,
        scope: &Scope,
        company_id: i64,
        material_type_id: i64,
    ) -> Result<MaterialType> {
        connection!(self, client);
        material_types::get(client, scope, company_id, material_type_id).await
    }

    /// The company's material types ordered by code, at most `limit`
    /// (default 100, maximum 500).
    pub async fn list_material_types(
        &self,
        scope: &Scope,
        company_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<MaterialType>> {
        connection!(self, client);
        material_types::list(client, scope, company_id, limit).await
    }

    /// Updates a material type. Property definitions cannot change after a
    /// material uses the type; a retired type takes no edit.
    pub async fn update_material_type(
        &self,
        scope: &Scope,
        company_id: i64,
        material_type_id: i64,
        update: &MaterialTypeUpdate,
    ) -> Result<MaterialType> {
        audited!(
            self,
            scope,
            "factory.inventory.update_material_type",
            |tx| {
                material_types::update(&mut tx, scope, company_id, material_type_id, update).await
            }
        )
    }

    /// Retires a material type without removing historical references.
    pub async fn retire_material_type(
        &self,
        scope: &Scope,
        company_id: i64,
        material_type_id: i64,
    ) -> Result<MaterialType> {
        audited!(
            self,
            scope,
            "factory.inventory.retire_material_type",
            |tx| { material_types::retire(&mut tx, scope, company_id, material_type_id).await }
        )
    }

    /// Makes an item a stocked material held in `native_unit_id`. An item is
    /// registered once and its native unit never changes, because every
    /// quantity recorded for it is in that unit. A type of another company is
    /// not found; values the type does not define, a missing required value,
    /// or a value of the wrong type are invalid properties.
    pub async fn register_material(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
        native_unit_id: i64,
        typing: &MaterialTyping,
    ) -> Result<Material> {
        audited!(self, scope, "factory.inventory.register_material", |tx| {
            materials::register(&mut tx, scope, company_id, item_id, native_unit_id, typing).await
        })
    }

    /// Creates an item and registers it as a material atomically; needs
    /// `commerce.inventory.item.create` as well.
    pub async fn create_material(
        &self,
        scope: &Scope,
        company_id: i64,
        item: &NewItem,
        native_unit_id: i64,
        typing: &MaterialTyping,
    ) -> Result<Material> {
        audited!(self, scope, "factory.inventory.create_material", |tx| {
            materials::create(
                &mut tx,
                &self.settings,
                scope,
                company_id,
                item,
                native_unit_id,
                typing,
            )
            .await
        })
    }

    pub async fn get_material(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
    ) -> Result<Material> {
        connection!(self, client);
        materials::get(client, scope, company_id, item_id).await
    }

    /// The company's materials by item SKU, at most `limit` (default 100,
    /// maximum 500).
    pub async fn list_materials(
        &self,
        scope: &Scope,
        company_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Material>> {
        connection!(self, client);
        materials::list(client, scope, company_id, limit).await
    }

    /// Updates an item's editable details and the material's typed values,
    /// preserving its native unit and type; needs
    /// `commerce.inventory.item.update` as well. A retired material takes no
    /// update.
    pub async fn update_material(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
        item: &ItemUpdate,
        properties: &Map<String, Value>,
    ) -> Result<Material> {
        audited!(self, scope, "factory.inventory.update_material", |tx| {
            materials::update(
                &mut tx,
                &self.settings,
                scope,
                company_id,
                item_id,
                item,
                properties,
            )
            .await
        })
    }

    /// Retires a material, preserving its posting history.
    pub async fn retire_material(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
    ) -> Result<Material> {
        audited!(self, scope, "factory.inventory.retire_material", |tx| {
            materials::retire(&mut tx, scope, company_id, item_id).await
        })
    }

    /// Defines the next version of an item's conversion from `unit_id` to
    /// its native unit: one `unit_id` equals `factor` native units, a positive
    /// decimal with at most 12 decimal places. Earlier versions stay readable;
    /// the new version becomes current.
    pub async fn define_conversion(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
        unit_id: i64,
        factor: Decimal,
    ) -> Result<Conversion> {
        audited!(self, scope, "factory.inventory.define_conversion", |tx| {
            conversions::define(&mut tx, scope, company_id, item_id, unit_id, factor).await
        })
    }

    /// Every version of an item's conversions, by unit code then version.
    pub async fn list_conversions(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
    ) -> Result<Vec<Conversion>> {
        connection!(self, client);
        conversions::list(client, scope, company_id, item_id).await
    }

    /// An item's conversion from `unit_id`: the current version, or the one
    /// `version` names.
    pub async fn get_conversion(
        &self,
        scope: &Scope,
        company_id: i64,
        item_id: i64,
        unit_id: i64,
        version: Option<i32>,
    ) -> Result<Conversion> {
        connection!(self, client);
        conversions::get(client, scope, company_id, item_id, unit_id, version).await
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
        migrations: &[
            (
                "factory-inventory-001-units-locations",
                include_str!("../migrations/001-units-locations.sql"),
            ),
            (
                "factory-inventory-002-catalog",
                include_str!("../migrations/002-catalog.sql"),
            ),
            (
                "factory-inventory-003-ledger",
                include_str!("../migrations/003-ledger.sql"),
            ),
        ],
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
