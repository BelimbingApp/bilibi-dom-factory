//! Factory Inventory (`factory/inventory`): the item master and its company
//! settings, units of measure, stock locations, material types, material
//! identity and versioned unit conversions behind one facade, [`Inventory`].
//! Schema, SQL, validation and helpers stay private to this crate. The
//! Material Transaction ledger records stock movements without changing
//! earlier observations, with lot and unit genealogy, and takes production
//! postings only from a registered [`PostingAuthority`].
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
mod authority;
mod balance;
mod contributions;
mod conversions;
mod error;
mod genealogy;
mod items;
mod ledger;
mod ledger_request;
mod locations;
mod material_types;
mod materials;
mod property;
mod settings;
mod units;

pub use authority::{PostingAuthority, POSTING_AUTHORITIES};
pub use balance::{Balance, BalanceConversion, CrossUnitBalance, UnitBalance};
pub use conversions::Conversion;
pub use error::{Error, FieldError};
pub use genealogy::{Identity, IdentityPosition, Trace, TraceLink};
pub use items::{Item, ItemUpdate, NewItem};
pub use ledger::{Entry, GenealogyLink, MaterialTransaction, StockPosition};
use ledger_request::TransactionKind;
pub use ledger_request::{
    Dimension, DimensionName, DimensionProvenance, IdentityKind, LengthUnit, NewIdentity,
    Observation, PostingRequest, ReceiptMeasurement, StockLine, UnitVariance, Variance,
};
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
    /// The production posting authorities contributed at link time.
    authorities: Vec<&'static PostingAuthority>,
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
    /// One posting in one audited transaction; `authority` is already proven
    /// registered when present.
    async fn post(
        &self,
        scope: &Scope,
        company_id: i64,
        kind: TransactionKind,
        request: &PostingRequest,
        authority: Option<&'static PostingAuthority>,
    ) -> Result<MaterialTransaction> {
        let action = match (kind, authority.is_some()) {
            (TransactionKind::Receipt, _) => "factory.inventory.record_receipt",
            (TransactionKind::Transfer, _) => "factory.inventory.record_transfer",
            (TransactionKind::Consumption, false) => "factory.inventory.record_consumption",
            (TransactionKind::Consumption, true) => {
                "factory.inventory.record_production_consumption"
            }
            (TransactionKind::Output, _) => "factory.inventory.record_output",
            (TransactionKind::Correction, false) => "factory.inventory.record_correction",
            (TransactionKind::Correction, true) => "factory.inventory.record_production_correction",
            (TransactionKind::Transform, _) => "factory.inventory.record_transform",
        };
        let mut client = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(&mut client, scope.mutation_context(action))
            .await?;
        match ledger::post(&mut tx, scope, company_id, kind, request, authority).await {
            Ok(saved) => {
                tx.commit().await?;
                Ok(saved)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }

    /// The authority itself when it is a registered contribution's payload.
    fn registered(
        &self,
        authority: &'static PostingAuthority,
    ) -> Result<Option<&'static PostingAuthority>> {
        if self.posting_authority_registered(authority) {
            Ok(Some(authority))
        } else {
            Err(Error::UnregisteredPostingAuthority)
        }
    }

    /// Whether `authority` is a production posting authority contributed to
    /// [`POSTING_AUTHORITIES`]: the contributed static itself, never an
    /// equal value built elsewhere. Only a module of Inventory's own
    /// container may contribute one; any other fails boot.
    pub fn posting_authority_registered(&self, authority: &PostingAuthority) -> bool {
        self.authorities
            .iter()
            .any(|registered| std::ptr::eq(*registered, authority))
    }

    /// Receives stock with source evidence and an optional weigh ticket. A
    /// line may create a lot or unit identity (`identity`).
    pub async fn record_receipt(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        self.post(scope, company_id, TransactionKind::Receipt, request, None)
            .await
    }

    /// Moves stock between distinct locations, preserving its recorded basis
    /// and any identity a line cites.
    pub async fn record_transfer(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        self.post(scope, company_id, TransactionKind::Transfer, request, None)
            .await
    }

    /// Draws stock under locks; concurrent requests cannot spend it twice.
    /// Consumption against production context goes through
    /// [`Inventory::record_production_consumption`].
    pub async fn record_consumption(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        self.post(
            scope,
            company_id,
            TransactionKind::Consumption,
            request,
            None,
        )
        .await
    }

    /// Consumption that may carry production context, posted by a
    /// registered `authority`.
    pub async fn record_production_consumption(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
        authority: &'static PostingAuthority,
    ) -> Result<MaterialTransaction> {
        let authority = self.registered(authority)?;
        self.post(
            scope,
            company_id,
            TransactionKind::Consumption,
            request,
            authority,
        )
        .await
    }

    /// Production output into stock, posted by a registered `authority`. A
    /// line may create a lot or unit identity.
    pub async fn record_output(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
        authority: &'static PostingAuthority,
    ) -> Result<MaterialTransaction> {
        let authority = self.registered(authority)?;
        self.post(
            scope,
            company_id,
            TransactionKind::Output,
            request,
            authority,
        )
        .await
    }

    /// Appends signed adjustments with a reason and an original transaction.
    /// Corrections may adjust a retired material; ordinary postings may not.
    /// A transaction that needed a posting authority is corrected through
    /// [`Inventory::record_production_correction`].
    pub async fn record_correction(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
    ) -> Result<MaterialTransaction> {
        self.post(
            scope,
            company_id,
            TransactionKind::Correction,
            request,
            None,
        )
        .await
    }

    /// Corrects any recorded transaction, including one that needed a
    /// posting authority, posted by a registered `authority`.
    pub async fn record_production_correction(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
        authority: &'static PostingAuthority,
    ) -> Result<MaterialTransaction> {
        let authority = self.registered(authority)?;
        self.post(
            scope,
            company_id,
            TransactionKind::Correction,
            request,
            authority,
        )
        .await
    }

    /// A material transform: `inputs` drawn from stock and `outputs` put
    /// into stock, committed with their genealogy links as one transaction,
    /// posted by a registered `authority`. Each native unit balances on its
    /// own; one whose inputs and outputs differ records the difference as a
    /// variance entry and needs `variance` evidence (`Variance required`),
    /// and evidence no difference uses is `No variance`.
    pub async fn record_transform(
        &self,
        scope: &Scope,
        company_id: i64,
        request: &PostingRequest,
        authority: &'static PostingAuthority,
    ) -> Result<MaterialTransaction> {
        let authority = self.registered(authority)?;
        self.post(
            scope,
            company_id,
            TransactionKind::Transform,
            request,
            authority,
        )
        .await
    }

    /// One lot or unit identity inside the company.
    pub async fn get_identity(
        &self,
        scope: &Scope,
        company_id: i64,
        identity_id: i64,
    ) -> Result<Identity> {
        connection!(self, client);
        genealogy::get(client, scope, company_id, identity_id).await
    }

    /// The identity's positive stock positions by location; empty once none
    /// remains in stock.
    pub async fn get_identity_positions(
        &self,
        scope: &Scope,
        company_id: i64,
        identity_id: i64,
    ) -> Result<Vec<IdentityPosition>> {
        connection!(self, client);
        genealogy::positions(client, scope, company_id, identity_id).await
    }

    /// Follows transform links from an output toward its source receipts.
    pub async fn trace_backward(
        &self,
        scope: &Scope,
        company_id: i64,
        identity_id: i64,
    ) -> Result<Trace> {
        connection!(self, client);
        genealogy::trace(
            client,
            scope,
            company_id,
            identity_id,
            genealogy::Direction::Backward,
        )
        .await
    }

    /// Follows transform links from a receipt toward descendant outputs.
    pub async fn trace_forward(
        &self,
        scope: &Scope,
        company_id: i64,
        identity_id: i64,
    ) -> Result<Trace> {
        connection!(self, client);
        genealogy::trace(
            client,
            scope,
            company_id,
            identity_id,
            genealogy::Direction::Forward,
        )
        .await
    }

    /// The consumptions and transforms that drew the identity, in ID order,
    /// including consumption that created no link.
    pub async fn list_identity_draws(
        &self,
        scope: &Scope,
        company_id: i64,
        identity_id: i64,
    ) -> Result<Vec<MaterialTransaction>> {
        connection!(self, client);
        genealogy::draws(client, scope, company_id, identity_id).await
    }

    /// The corrections of a transaction, following corrections of
    /// corrections, in ID order.
    pub async fn list_corrections(
        &self,
        scope: &Scope,
        company_id: i64,
        transaction_id: i64,
    ) -> Result<Vec<MaterialTransaction>> {
        connection!(self, client);
        ledger::corrections(client, scope, company_id, transaction_id).await
    }

    /// A transaction's material balance net of its corrections, per native
    /// unit and across units; see [`Balance`].
    pub async fn get_transaction_balance(
        &self,
        scope: &Scope,
        company_id: i64,
        transaction_id: i64,
    ) -> Result<Balance> {
        connection!(self, client);
        balance::read(client, scope, company_id, transaction_id).await
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

    /// The shared company-defined property vocabulary for material and resource types.
    pub fn normalize_property_definitions(
        definitions: &[PropertyDefinition],
    ) -> Result<Vec<PropertyDefinition>> {
        property::normalize(definitions)
    }

    /// Validates and normalizes values with the same rules as material properties.
    pub fn validate_property_values(
        definitions: &[PropertyDefinition],
        values: &Map<String, Value>,
    ) -> Result<Map<String, Value>> {
        property::validate(definitions, values)
    }

    /// Reads on a sibling's transaction, retaining Inventory authorization and
    /// seeing pending writes without acquiring another pool connection.
    pub async fn get_item_on(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company_id: i64,
        id: i64,
    ) -> Result<Item> {
        items::get(client, scope, company_id, id).await
    }

    /// Transaction-aware material read; see [`Inventory::get_item_on`].
    pub async fn get_material_on(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company_id: i64,
        id: i64,
    ) -> Result<Material> {
        materials::get(client, scope, company_id, id).await
    }

    /// Transaction-aware unit read; see [`Inventory::get_item_on`].
    pub async fn get_unit_on(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company_id: i64,
        id: i64,
    ) -> Result<Unit> {
        units::get(client, scope, company_id, id).await
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
            (
                "factory-inventory-004-genealogy",
                include_str!("../migrations/004-genealogy.sql"),
            ),
        ],
        services: &[Service {
            id: SERVICE,
            factory: |services| {
                Ok(Arc::new(Inventory {
                    db: services.get::<Db>("base/database.pool")?.as_ref().clone(),
                    mutations: services.get("base/database.mutations")?,
                    settings: services.get("base/settings.store")?,
                    authorities: authority::registered(
                        services.catalog()?.entries::<PostingAuthority>(POSTING_AUTHORITIES)?,
                    )?,
                }))
            },
        }],
    }
}
