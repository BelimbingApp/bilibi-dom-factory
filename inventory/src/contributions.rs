//! What Inventory adds to the shared catalogs (Bilimbi
//! `Bilimbi.Factory.Inventory.Contributions`): Belimbing's item master
//! capabilities under their durable keys, the Factory menu, and the company
//! settings the item master reads.
use bilibi_authz::{CapabilityDefinition as C, RoleDefinition, VocabularyContribution, VOCABULARY};
use bilibi_menu::{MenuItem, Requirement};
use bilibi_module_registry::{
    contributions::{Access, Method, Route, ROUTES},
    Contribution, Provenance,
};
use bilibi_settings::{Definition, Editor, ScopeKind, Setting, Validator, ValueType, DEFINITIONS};
use serde_json::{json, Value};

pub(crate) const ITEM_CREATE: &str = "commerce.inventory.item.create";
pub(crate) const ITEM_LIST: &str = "commerce.inventory.item.list";
pub(crate) const ITEM_UPDATE: &str = "commerce.inventory.item.update";
pub(crate) const ITEM_VIEW: &str = "commerce.inventory.item.view";
pub(crate) const MANAGE_ALL: &str = "commerce.inventory.manage";
pub(crate) const CONFIGURATION_VIEW: &str = "factory.inventory.configuration.view";
pub(crate) const CONFIGURATION_MANAGE: &str = "factory.inventory.configuration.manage";
const CAPABILITIES: [&str; 7] = [
    ITEM_CREATE,
    ITEM_LIST,
    ITEM_UPDATE,
    ITEM_VIEW,
    MANAGE_ALL,
    CONFIGURATION_VIEW,
    CONFIGURATION_MANAGE,
];

/// The settings naming a company's item status vocabulary and default item
/// currency. Both default to nothing on purpose: the vocabulary and the
/// currency are a company's configuration, never Inventory's.
pub(crate) const ITEM_STATUSES_KEY: &str = "factory.inventory.item_statuses";
pub(crate) const DEFAULT_CURRENCY_KEY: &str = "factory.inventory.default_currency_code";

macro_rules! provenance {
    () => {
        Provenance {
            module: "factory/inventory",
            source: file!(),
            line: line!(),
        }
    };
}

// `tenant_owner` receives the whole set, as Bilimbi's role map grants it.
inventory::submit! { Contribution::new(VOCABULARY, "factory/inventory", provenance!(), &VocabularyContribution {
    domains: &["commerce", "factory"],
    verbs: &[],
    capabilities: &[C::tenant(ITEM_CREATE), C::tenant(ITEM_LIST), C::tenant(ITEM_UPDATE), C::tenant(ITEM_VIEW), C::tenant(MANAGE_ALL), C::tenant(CONFIGURATION_VIEW), C::tenant(CONFIGURATION_MANAGE)],
    roles: &[RoleDefinition { code: "tenant_owner", name: "Tenant Owner", description: "Contributed system role; reconciled explicitly", grant_all: false, capabilities: &CAPABILITIES }],
    system_principals: &[],
}) }

bilibi_menu::contribute!(
    "factory/inventory",
    MenuItem::container("admin.factory", "Factory")
        .icon("building-office-2")
        .under("admin")
        .order(80)
);
bilibi_menu::contribute!(
    "factory/inventory",
    MenuItem::page("admin.factory.units", "Units", "/factory/units")
        .under("admin.factory")
        .requires(Requirement::Capability(CONFIGURATION_VIEW))
        .order(20)
);
bilibi_menu::contribute!(
    "factory/inventory",
    MenuItem::page("admin.factory.materials", "Materials", "/factory/materials")
        .under("admin.factory")
        .requires(Requirement::Capability(CONFIGURATION_VIEW))
        .order(5)
);

const TENANT_OR_COMPANY: &[ScopeKind] = &[ScopeKind::Tenant, ScopeKind::Company];

/// A vocabulary is an ordered list of distinct, unpadded, non-blank statuses.
pub(crate) fn statuses_valid(value: &Value) -> bool {
    value.is_null()
        || value.as_array().is_some_and(|statuses| {
            !statuses.is_empty()
                && statuses.iter().enumerate().all(|(index, status)| {
                    status.as_str().is_some_and(|status| {
                        !status.is_empty()
                            && status == status.trim()
                            && status.chars().count() <= 255
                    }) && !statuses[..index].contains(status)
                })
        })
}

/// An ISO 4217 shaped code: three upper-case ASCII letters.
pub(crate) fn currency_valid(value: &Value) -> bool {
    value.is_null()
        || value.as_str().is_some_and(|code| {
            code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_uppercase())
        })
}

inventory::submit! { Contribution::new(DEFINITIONS, ITEM_STATUSES_KEY, provenance!(), &Setting::Definition(Definition {
    value_type: ValueType::Array, scopes: TENANT_OR_COMPANY, default: || Value::Null, nullable: true, encrypted: false, minimum: None, maximum: None,
    validator: Some(Validator { accepts: statuses_valid, message: "Give distinct, non-blank statuses, or none" }),
    editor: Some(Editor { group: "Factory", label: "Item statuses", help: "The statuses an item may hold, in order; the first is the default. Unset, any non-blank status is accepted.", capability: Some(CONFIGURATION_MANAGE), reveal_duration_ms: 5_000 }),
})) }
inventory::submit! { Contribution::new(DEFINITIONS, DEFAULT_CURRENCY_KEY, provenance!(), &Setting::Definition(Definition {
    value_type: ValueType::String, scopes: TENANT_OR_COMPANY, default: || json!(null), nullable: true, encrypted: false, minimum: None, maximum: None,
    validator: Some(Validator { accepts: currency_valid, message: "Give a three-letter currency code, or none" }),
    editor: Some(Editor { group: "Factory", label: "Default item currency", help: "ISO 4217 code an item takes when none is given. Unset, each item names its own.", capability: Some(CONFIGURATION_MANAGE), reveal_duration_ms: 5_000 }),
})) }

// Bilimbi `priv/web_routes.exs`: five pages behind the configuration view
// capability. Each is checked again on every live event.
macro_rules! page {
    ($id:literal, $path:literal, $operation:literal) => {
        inventory::submit! { Contribution::new(ROUTES, $id, provenance!(), &Route {
            method: Method::Get, path: $path, service: crate::web::SERVICE, operation: $operation,
            access: Access::Capability(CONFIGURATION_VIEW), framed: true,
        }) }
    };
}
page!(
    "factory/inventory.material_types",
    "/factory/material-types",
    "material_types"
);
page!("factory/inventory.units", "/factory/units", "units");
page!(
    "factory/inventory.item_settings",
    "/factory/item-settings",
    "item_settings"
);
page!(
    "factory/inventory.conversions",
    "/factory/conversions",
    "conversions"
);
page!(
    "factory/inventory.materials",
    "/factory/materials",
    "materials"
);
