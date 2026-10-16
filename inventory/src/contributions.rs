use bilibi_authz::{CapabilityDefinition as C, RoleDefinition, VocabularyContribution, VOCABULARY};
use bilibi_menu::{MenuItem, Requirement};
use bilibi_module_registry::{Contribution, Provenance};

use crate::{CAPABILITIES, CONFIGURATION_VIEW};

const fn role() -> RoleDefinition {
    RoleDefinition {
        code: "tenant_owner",
        name: "Tenant Owner",
        description: "Tenant owner",
        grant_all: false,
        capabilities: &CAPABILITIES,
    }
}

inventory::submit! { Contribution::new(VOCABULARY, "factory/inventory", Provenance { module: "factory/inventory", source: file!(), line: line!() }, &VocabularyContribution {
    domains: &["commerce", "factory"], verbs: &[],
    capabilities: &[C::tenant("commerce.inventory.item.create"), C::tenant("commerce.inventory.item.list"), C::tenant("commerce.inventory.item.update"), C::tenant("commerce.inventory.item.view"), C::tenant("commerce.inventory.manage"), C::tenant("factory.inventory.configuration.view"), C::tenant("factory.inventory.configuration.manage")],
    roles: &[role()], system_principals: &[],
}) }

bilibi_menu::contribute!(
    "factory/inventory",
    MenuItem::group("admin.factory", "Factory")
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
