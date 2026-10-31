//! The production posting-authority registry (Bilimbi
//! `Inventory.PostingAuthority`). Bilimbi declares an authority in its OTP
//! application metadata; here a module contributes a [`PostingAuthority`] to
//! [`POSTING_AUTHORITIES`] at link time, and boot refuses one from any module
//! outside Inventory's own container. Nothing registers at runtime, and
//! Inventory names no authority.
//!
//! The authority is the contributed `static` itself: a production posting
//! passes `&'static PostingAuthority`, and Inventory accepts only a reference
//! to a registered contribution's payload. A module keeps its static private,
//! so no other crate can name it; a value it builds itself is not registered.
use bilibi_module_registry::{Consumer, Entry, RegistryError};

/// The catalog a production posting authority is contributed to, with the
/// contributing module's ID as the contribution ID:
///
/// ```ignore
/// static AUTHORITY: PostingAuthority = PostingAuthority::declare("factory/production_execution");
/// inventory::submit! { Contribution::new(POSTING_AUTHORITIES, "factory/production_execution", provenance!(), &AUTHORITY) }
/// ```
pub const POSTING_AUTHORITIES: &str = "factory/inventory.posting_authorities";

/// The container whose Domain modules may declare an authority.
const CONTAINER: &str = "factory";

/// A module's declaration that it posts production. Keep the `static` that
/// holds it private to the declaring module.
#[derive(Debug)]
pub struct PostingAuthority {
    module: &'static str,
    import_capability: Option<&'static str>,
}
impl PostingAuthority {
    pub const fn declare(module: &'static str) -> Self {
        Self {
            module,
            import_capability: None,
        }
    }
    /// Opt this authority into its own import capability. Ordinary postings
    /// keep their configuration capability; only explicit import calls use it.
    pub const fn with_import_capability(mut self, capability: &'static str) -> Self {
        self.import_capability = Some(capability);
        self
    }
    /// The declaring module, recorded on every transaction it posts.
    pub fn module(&self) -> &'static str {
        self.module
    }
}

inventory::submit! { Consumer::new::<PostingAuthority>(POSTING_AUTHORITIES, "factory/inventory", validate) }

/// Accepts a declaration only from a module of Inventory's own container,
/// naming that module, once per module.
fn validate(entries: &[Entry]) -> Result<(), RegistryError> {
    for entry in entries {
        let authority = entry.get::<PostingAuthority>()?;
        let module = entry.provenance.module;
        if module.split_once('/').map(|(container, _)| container) != Some(CONTAINER)
            || entry.id != module
            || authority.module != module
            || authority.import_capability.is_some_and(|capability| {
                capability != format!("{}.import", module.replace('/', ".").replace('_', "-"))
            })
        {
            return Err(RegistryError::Contribution(format!(
                "{module} declares {} as a posting authority at {}:{}; only a Domain module of Inventory's own container may declare itself",
                authority.module, entry.provenance.source, entry.provenance.line
            )));
        }
    }
    Ok(())
}

/// The registered authorities, read once when the Inventory service starts.
pub(crate) fn registered(
    entries: &[Entry],
) -> Result<Vec<&'static PostingAuthority>, RegistryError> {
    entries.iter().map(Entry::get::<PostingAuthority>).collect()
}

/// Import-only access, sealed by a registered production authority. It is
/// bound to one scope and company, and reauthorizes on every facade call;
/// retaining it never retains a grant. It cannot authorize configuration,
/// ordinary postings, or corrections.
pub struct ProductionImport<'a> {
    scope: &'a bilibi_tenancy::Scope,
    company: i64,
    pub(crate) authority: &'static PostingAuthority,
}
impl<'a> ProductionImport<'a> {
    pub(crate) fn new(
        scope: &'a bilibi_tenancy::Scope,
        company: i64,
        authority: &'static PostingAuthority,
    ) -> Self {
        Self {
            scope,
            company,
            authority,
        }
    }
    pub fn scope(&self) -> &bilibi_tenancy::Scope {
        self.scope
    }
    pub fn company(&self) -> i64 {
        self.company
    }
    /// Recheck the authority's import grant and the live company on the
    /// caller's connection. Sibling facades use this for import validation.
    pub async fn authorize_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
    ) -> Result<(), bilibi_database::Error> {
        let capability = self
            .authority
            .import_capability
            .ok_or(bilibi_database::Error::Forbidden)?;
        bilibi_company::authorize_work_target(client, self.scope, self.company, capability).await?;
        bilibi_company::require_live(client, self.scope, self.company).await?;
        Ok(())
    }
}
