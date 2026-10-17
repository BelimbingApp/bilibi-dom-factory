//! Bilimbi ProductionExecution refusal names, worded as Bilimbi words them
//! (`reason |> to_string() |> String.replace("_", " ") |> String.capitalize()`).
use bilibi_database::Error as DatabaseError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Capture(&'static str),
    #[error("Order not found")]
    OrderNotFound,
    #[error("Execution not found")]
    ExecutionNotFound,
    #[error("Import not authorized")]
    ImportNotAuthorized,
    #[error("Invalid execution")]
    InvalidExecution,
    #[error("Invalid lines")]
    InvalidLines,
    #[error("Invalid material hold rule")]
    InvalidMaterialHoldRule,
    #[error("Invalid hold override")]
    InvalidHoldOverride,
    #[error("Invalid hold source")]
    InvalidHoldSource,
    #[error("Material held")]
    MaterialHeld,
    #[error("Hold override denied")]
    HoldOverrideDenied,
    #[error("Override refused under impersonation")]
    OverrideRefusedUnderImpersonation,
    #[error("Request id conflict")]
    RequestIdConflict,
    #[error("{field} {message}")]
    Invalid {
        field: &'static str,
        message: &'static str,
    },
    #[error(transparent)]
    Inventory(#[from] bilibi_factory_inventory::Error),
    #[error(transparent)]
    ProductDefinition(#[from] bilibi_factory_product_definition::Error),
    #[error(transparent)]
    Company(#[from] bilibi_company::WriteError),
    #[error(transparent)]
    Access(#[from] DatabaseError),
}
impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Access(error.into())
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Access(DatabaseError::Invalid)
    }
}
