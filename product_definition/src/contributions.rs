//! Bilimbi.Factory.ProductDefinition.Contributions vocabulary. Menu/routes
//! arrive with XD2, rather than advertising pages that do not exist yet.
use bilibi_authz::{CapabilityDefinition as C, RoleDefinition, VocabularyContribution, VOCABULARY};
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
