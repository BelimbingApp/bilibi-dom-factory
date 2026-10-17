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
}
impl PostingAuthority {
    pub const fn declare(module: &'static str) -> Self {
        Self { module }
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
