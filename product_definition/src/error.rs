//! Bilimbi ProductDefinition refusal names, rendered as its pages render them.
use bilibi_database::Error as DatabaseError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Product not found")]
    ProductNotFound,
    #[error("Resource type not found")]
    ResourceTypeNotFound,
    #[error("Resource type retired")]
    ResourceTypeRetired,
    #[error("Resource type in use")]
    ResourceTypeInUse,
    #[error("Resource not found")]
    ResourceNotFound,
    #[error("Resource retired")]
    ResourceRetired,
    #[error("Formula revision not found")]
    FormulaRevisionNotFound,
    #[error("Routing revision not found")]
    RoutingRevisionNotFound,
    #[error("Invalid lines")]
    InvalidLines,
    #[error("Invalid operations")]
    InvalidOperations,
    #[error("Invalid process config")]
    InvalidProcessConfig,
    #[error("Product output missing")]
    ProductOutputMissing,
    #[error("Routing formula mismatch")]
    RoutingFormulaMismatch,
    #[error("{field} {message}")]
    Invalid {
        field: &'static str,
        message: &'static str,
    },
    #[error(transparent)]
    Inventory(#[from] bilibi_factory_inventory::Error),
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
