//! Why an Inventory operation did not happen. The texts are Bilimbi's
//! (`reason |> to_string() |> String.replace("_", " ") |> String.capitalize()`
//! for a named reason, "Field message" for a refused attribute).
use bilibi_company::WriteError;
use bilibi_database::Error as DatabaseError;

/// One refused attribute, worded as Bilimbi's changesets word it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Company not found")]
    CompanyNotFound,
    #[error("This company is archived and read-only")]
    CompanyArchived,
    #[error("Unit not found")]
    UnitNotFound,
    #[error("Unit retired")]
    UnitRetired,
    #[error("Unit in use")]
    UnitInUse,
    #[error("Location not found")]
    LocationNotFound,
    #[error("Item not found")]
    ItemNotFound,
    #[error("Material type not found")]
    MaterialTypeNotFound,
    #[error("Material type retired")]
    MaterialTypeRetired,
    #[error("Material type in use")]
    MaterialTypeInUse,
    #[error("Invalid property definitions")]
    InvalidPropertyDefinitions,
    #[error("Invalid properties")]
    InvalidProperties,
    #[error("Material not found")]
    MaterialNotFound,
    #[error("Material retired")]
    MaterialRetired,
    #[error("Material has stock")]
    MaterialHasStock,
    #[error("Transaction not found")]
    TransactionNotFound,
    #[error("Request id conflict")]
    RequestIdConflict,
    #[error("Insufficient stock")]
    InsufficientStock,
    #[error("Unregistered posting authority")]
    UnregisteredPostingAuthority,
    #[error("Variance required")]
    VarianceRequired,
    #[error("No variance")]
    NoVariance,
    #[error("Identity not found")]
    IdentityNotFound,
    #[error("Identity required")]
    IdentityRequired,
    #[error("Invalid identity")]
    InvalidIdentity,
    #[error("Insufficient identity stock")]
    InsufficientIdentityStock,
    #[error("Insufficient unidentified stock")]
    InsufficientUnidentifiedStock,
    #[error("Conversion not found")]
    ConversionNotFound,
    #[error("Native unit")]
    NativeUnit,
    #[error("Invalid item statuses")]
    InvalidItemStatuses,
    #[error("Invalid default currency")]
    InvalidDefaultCurrency,
    #[error("{}", fields(.0))]
    Invalid(Vec<FieldError>),
    /// A denied capability, a failed write or a database fault.
    #[error(transparent)]
    Access(#[from] DatabaseError),
}
impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Access(error.into())
    }
}
fn fields(errors: &[FieldError]) -> String {
    errors
        .iter()
        .map(|error| {
            let mut field = error.field.replace('_', " ");
            if let Some(first) = field.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            format!("{field} {}", error.message)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A company the caller cannot reach is not found, whatever the reason.
pub(crate) fn company(error: DatabaseError) -> Error {
    match error {
        DatabaseError::NotFound => Error::CompanyNotFound,
        other => Error::Access(other),
    }
}
impl From<WriteError> for Error {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::NotFound => Self::CompanyNotFound,
            WriteError::Archived => Self::CompanyArchived,
            WriteError::Access(error) => Self::Access(error),
            WriteError::InUse | WriteError::Stale | WriteError::Invalid(_) => {
                Self::Access(DatabaseError::Invalid)
            }
        }
    }
}
