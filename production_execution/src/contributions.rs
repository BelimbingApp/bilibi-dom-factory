//! Bilimbi.Factory.ProductionExecution.Contributions vocabulary. Menu and
//! routes arrive with the shop-floor pages (XP3/XP4), rather than advertising
//! pages that do not exist yet; the shop-floor capture capabilities arrive
//! with the capture they gate (XP2).
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
