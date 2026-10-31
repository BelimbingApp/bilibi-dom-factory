//! Bilimbi.Factory.ProductionExecution.Contributions vocabulary. Menu and
//! shop-floor and configuration pages.
use bilibi_authz::{CapabilityDefinition as C, RoleDefinition, VocabularyContribution, VOCABULARY};
use bilibi_module_registry::{Contribution, Provenance};

/// Reads of orders, executions and hold overrides (Bilimbi's shop floor view).
pub(crate) const FLOOR_VIEW: &str = "factory.production-execution.floor.view";
pub(crate) const VIEW: &str = "factory.production-execution.configuration.view";
/// Creating orders and completing operations.
pub(crate) const MANAGE: &str = "factory.production-execution.configuration.manage";
/// Completing an operation from a historical source.
pub(crate) const IMPORT: &str = "factory.production-execution.import";
/// Consuming material before its hold has elapsed.
pub(crate) const HOLD_OVERRIDE: &str = "factory.production-execution.material-hold.override";

pub(crate) const WASTAGE_RECORD: &str = "factory.production-execution.wastage.record";
pub(crate) const WASTAGE_CORRECT: &str = "factory.production-execution.wastage.correct";
pub(crate) const LABOUR_RECORD: &str = "factory.production-execution.labour.record";
pub(crate) const LABOUR_MANAGE: &str = "factory.production-execution.labour.manage";
pub(crate) const MEASUREMENT_RECORD: &str = "factory.production-execution.measurement.record";
pub(crate) const MEASUREMENT_CORRECT: &str = "factory.production-execution.measurement.correct";

// `tenant_owner` receives the floor, view and manage keys, as Bilimbi's role
// map grants them; an administrator grants import and override explicitly.
inventory::submit! { Contribution::new(VOCABULARY,"factory/production_execution",Provenance {
    module:"factory/production_execution",source:file!(),line:line!(),
}, &VocabularyContribution {
    domains:&["factory"],verbs:&["import","override","record","correct"],
    capabilities:&[C::tenant(IMPORT),C::tenant(HOLD_OVERRIDE),C::tenant(FLOOR_VIEW),C::tenant(VIEW),C::tenant(MANAGE),
        C::tenant(WASTAGE_RECORD),C::tenant(WASTAGE_CORRECT),C::tenant(LABOUR_RECORD),C::tenant(LABOUR_MANAGE),C::tenant(MEASUREMENT_RECORD),C::tenant(MEASUREMENT_CORRECT)],
    roles:&[RoleDefinition {code:"tenant_owner",name:"Tenant Owner",description:"Contributed system role; reconciled explicitly",grant_all:false,capabilities:&[FLOOR_VIEW,VIEW,MANAGE]}],
    system_principals:&[],
}) }

bilibi_menu::contribute!(
    "factory/production_execution",
    bilibi_menu::MenuItem::page("production.factory.floor", "Shop floor", "/factory/floor")
        .company_scoped()
        .under("production")
        .requires(bilibi_menu::Requirement::Capability(FLOOR_VIEW))
        .order(10)
);
inventory::submit! { Contribution::new(
    bilibi_module_registry::contributions::ROUTES,"factory/production_execution.floor",
    Provenance {module:"factory/production_execution",source:file!(),line:line!()},
    &bilibi_module_registry::contributions::Route { company_scope: bilibi_module_registry::contributions::CompanyScope::Company,
        method:bilibi_module_registry::contributions::Method::Get,path:"/factory/floor",
        service:crate::PAGES,operation:"floor",
        access:bilibi_module_registry::contributions::Access::Capability(FLOOR_VIEW),framed:true,
    }
) }

bilibi_menu::contribute!(
    "factory/production_execution",
    bilibi_menu::MenuItem::page(
        "admin.factory.wastage-reasons",
        "Wastage reasons",
        "/factory/wastage-reasons"
    )
    .company_scoped()
    .under("admin.factory")
    .requires(bilibi_menu::Requirement::Capability(VIEW))
    .order(80)
);
inventory::submit! { Contribution::new(
    bilibi_module_registry::contributions::ROUTES,"factory/production_execution.wastage-reasons",
    Provenance {module:"factory/production_execution",source:file!(),line:line!()},
    &bilibi_module_registry::contributions::Route { company_scope: bilibi_module_registry::contributions::CompanyScope::Company,
        method:bilibi_module_registry::contributions::Method::Get,path:"/factory/wastage-reasons",
        service:crate::PAGES,operation:"wastage-reasons",
        access:bilibi_module_registry::contributions::Access::Capability(VIEW),framed:true,
    }
) }

bilibi_menu::contribute!(
    "factory/production_execution",
    bilibi_menu::MenuItem::page(
        "admin.factory.labour-roles",
        "Labour roles",
        "/factory/labour-roles"
    )
    .company_scoped()
    .under("admin.factory")
    .requires(bilibi_menu::Requirement::Capability(VIEW))
    .order(85)
);
inventory::submit! { Contribution::new(
    bilibi_module_registry::contributions::ROUTES,"factory/production_execution.labour-roles",
    Provenance {module:"factory/production_execution",source:file!(),line:line!()},
    &bilibi_module_registry::contributions::Route { company_scope: bilibi_module_registry::contributions::CompanyScope::Company,
        method:bilibi_module_registry::contributions::Method::Get,path:"/factory/labour-roles",
        service:crate::PAGES,operation:"labour-roles",
        access:bilibi_module_registry::contributions::Access::Capability(VIEW),framed:true,
    }
) }

bilibi_menu::contribute!(
    "factory/production_execution",
    bilibi_menu::MenuItem::page(
        "admin.factory.measurement-types",
        "Measurement types",
        "/factory/measurement-types"
    )
    .company_scoped()
    .under("admin.factory")
    .requires(bilibi_menu::Requirement::Capability(VIEW))
    .order(90)
);
inventory::submit! { Contribution::new(
    bilibi_module_registry::contributions::ROUTES,"factory/production_execution.measurement-types",
    Provenance {module:"factory/production_execution",source:file!(),line:line!()},
    &bilibi_module_registry::contributions::Route { company_scope: bilibi_module_registry::contributions::CompanyScope::Company,
        method:bilibi_module_registry::contributions::Method::Get,path:"/factory/measurement-types",
        service:crate::PAGES,operation:"measurement-types",
        access:bilibi_module_registry::contributions::Access::Capability(VIEW),framed:true,
    }
) }

// ADR 01 gap, listed so it stays visible: these pages still write through
// the Production Execution facade rather than an operation contract.
inventory::submit! { Contribution::new(
    bilibi_module_registry::contributions::PAGE_WRITES,crate::PAGES,
    Provenance {module:"factory/production_execution",source:file!(),line:line!()},
    &bilibi_module_registry::contributions::PageWrites {
        service:crate::PAGES,contracts:&[],
        pending:bilibi_module_registry::contributions::Pending::Every {reason:"Labour, wastage and measurement capture and their corrections, and wastage reason, labour role and measurement type configuration, write through the Production Execution facade; no contract yet."},
    }
) }
