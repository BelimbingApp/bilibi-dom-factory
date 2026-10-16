//! Factory Inventory's initial durable vocabulary and company-owned catalogs.
pub mod contributions;

pub const ITEM_STATUSES_KEY: &str = "factory.inventory.item_statuses";
pub const DEFAULT_CURRENCY_CODE_KEY: &str = "factory.inventory.default_currency_code";
pub const ITEM_CREATE: &str = "commerce.inventory.item.create";
pub const ITEM_LIST: &str = "commerce.inventory.item.list";
pub const ITEM_UPDATE: &str = "commerce.inventory.item.update";
pub const ITEM_VIEW: &str = "commerce.inventory.item.view";
pub const MANAGE: &str = "commerce.inventory.manage";
pub const CONFIGURATION_VIEW: &str = "factory.inventory.configuration.view";
pub const CONFIGURATION_MANAGE: &str = "factory.inventory.configuration.manage";
pub const CAPABILITIES: [&str; 7] = [
    ITEM_CREATE,
    ITEM_LIST,
    ITEM_UPDATE,
    ITEM_VIEW,
    MANAGE,
    CONFIGURATION_VIEW,
    CONFIGURATION_MANAGE,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unit {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub name: String,
    pub retired: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Location {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub name: String,
}

/// Input validation shared by the Unit and Location facades.
pub fn validate_code_name(code: &str, name: &str, max_code_len: usize) -> Result<(), &'static str> {
    let code = code.trim();
    if code.is_empty() || code.len() > max_code_len || name.trim().is_empty() || name.len() > 255 {
        return Err("invalid_code_or_name");
    }
    Ok(())
}

inventory::submit! { bilibi_module_registry::Module {
    id: "factory/inventory",
    layer: bilibi_module_registry::Layer::Domains,
    dependencies: &["base/module_registry", "base/database", "base/authz", "base/tenancy", "base/settings", "base/ui", "core/company"],
    migrations: &[("factory-inventory-001-units-locations", include_str!("../migrations/001-units-locations.sql"))],
    services: &[],
} }
