//! Bilimbi.Factory.ProductDefinition.Contributions: the vocabulary, the two
//! Factory menu items and the three page routes. Resource types has no menu
//! item in Bilimbi either: the Resources page links to it.
use bilibi_authz::{CapabilityDefinition as C, RoleDefinition, VocabularyContribution, VOCABULARY};
use bilibi_menu::{MenuItem, Requirement};
use bilibi_module_registry::contributions::{Access, Method, Route, ROUTES};
use bilibi_module_registry::{Contribution, Provenance};
pub(crate) const VIEW: &str = "factory.product-definition.configuration.view";
pub(crate) const MANAGE: &str = "factory.product-definition.configuration.manage";
inventory::submit! { Contribution::new(VOCABULARY,"factory/product_definition",Provenance {
    module:"factory/product_definition",source:file!(),line:line!(),
}, &VocabularyContribution {
    domains:&["factory"],verbs:&[],capabilities:&[C::tenant(VIEW),C::tenant(MANAGE)],
    roles:&[RoleDefinition {code:"tenant_owner",name:"Tenant Owner",description:"Contributed system role; reconciled explicitly",grant_all:false,capabilities:&[VIEW,MANAGE]}],
    system_principals:&[],
}) }

bilibi_menu::contribute!(
    "factory/product_definition",
    MenuItem::page("admin.factory.resources", "Resources", "/factory/resources")
        .company_scoped()
        .under("admin.factory")
        .requires(Requirement::Capability(VIEW))
        .order(60)
);
bilibi_menu::contribute!(
    "factory/product_definition",
    MenuItem::page(
        "admin.factory.definitions",
        "BOM formulas & routings",
        "/factory/definitions"
    )
    .company_scoped()
    .under("admin.factory")
    .requires(Requirement::Capability(VIEW))
    .order(70)
);

macro_rules! page_route {
    ($id:literal, $path:literal, $operation:literal) => {
        inventory::submit! { Contribution::new(ROUTES, $id, Provenance {
            module:"factory/product_definition",source:file!(),line:line!(),
        }, &Route { company_scope: bilibi_module_registry::contributions::CompanyScope::Company,
            method:Method::Get, path:$path, service:crate::PAGES, operation:$operation,
            access:Access::Capability(VIEW), framed:true,
        }) }
    };
}
page_route!(
    "factory/product_definition.resource-types",
    "/factory/resource-types",
    "resource-types"
);
page_route!(
    "factory/product_definition.resources",
    "/factory/resources",
    "resources"
);
page_route!(
    "factory/product_definition.definitions",
    "/factory/definitions",
    "definitions"
);

// ADR 01 gap, listed so it stays visible: these pages still write through
// the Product Definition facade rather than an operation contract.
inventory::submit! { Contribution::new(bilibi_module_registry::contributions::PAGE_WRITES,crate::PAGES,Provenance {
    module:"factory/product_definition",source:file!(),line:line!(),
}, &bilibi_module_registry::contributions::PageWrites {
    service:crate::PAGES,contracts:&[],
    pending:bilibi_module_registry::contributions::Pending::Every {reason:"Resource types, resources, products and formula and routing publication write through the Product Definition facade; no contract yet."},
}) }
