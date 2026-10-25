//! Bilimbi Inventory.Ledger.Request: typed inputs and canonical retry identity.
use crate::{Decimal, Error, FieldError};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    Receipt,
    Transfer,
    Consumption,
    Output,
    Correction,
    Transform,
}
impl TransactionKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Transfer => "transfer",
            Self::Consumption => "consumption",
            Self::Output => "output",
            Self::Correction => "correction",
            Self::Transform => "transform",
        }
    }
    /// Output and transform always need a posting authority.
    pub(crate) fn production(self) -> bool {
        matches!(self, Self::Output | Self::Transform)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Observation {
    Measured,
    Declared,
    Counted,
    Derived,
}
impl Observation {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Declared => "declared",
            Self::Counted => "counted",
            Self::Derived => "derived",
        }
    }
}

/// A stock observation, or a transfer from one location to another. Only a
/// correction accepts a negative quantity. Actors always come from Scope.
///
/// A receipt, production output or transform output line may create a lot
/// or unit `identity`; a transfer, consumption, correction, production output
/// or transform line cites an existing one by `identity_id`. A positive
/// output adds stock without replacing earlier postings or identity metadata.
/// A line cannot carry both fields. `output_role` is the caller's
/// opaque label for a transform output; other lines drop it, as Bilimbi's
/// cast does.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StockLine {
    pub item_id: i64,
    pub location_id: Option<i64>,
    pub from_location_id: Option<i64>,
    pub to_location_id: Option<i64>,
    pub quantity: Decimal,
    pub unit_id: Option<i64>,
    pub conversion_version: Option<i32>,
    pub observation: Observation,
    pub evidence: Option<String>,
    // Omitted when empty, so a request without them keeps its XI2 fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<NewIdentity>,
}
impl Default for StockLine {
    fn default() -> Self {
        Self {
            item_id: 0,
            location_id: None,
            from_location_id: None,
            to_location_id: None,
            quantity: Decimal::ZERO,
            unit_id: None,
            conversion_version: None,
            observation: Observation::Measured,
            evidence: None,
            output_role: None,
            identity_id: None,
            identity: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    Lot,
    Unit,
}
impl IdentityKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Lot => "lot",
            Self::Unit => "unit",
        }
    }
}

/// The dimensions an individual unit may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionName {
    Width,
    Length,
    Thickness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    Um,
    Mm,
    Cm,
    M,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionProvenance {
    Measured,
    Nominal,
}

/// A positive length measurement attached to a unit identity at creation.
/// Stored as `{"value": "<decimal>", "unit": …, "provenance": …}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    pub value: Decimal,
    pub unit: LengthUnit,
    pub provenance: DimensionProvenance,
}

/// A lot or unit identity a line creates: a code unique for its material in
/// the company and, on a unit only, one to three dimensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewIdentity {
    pub kind: IdentityKind,
    pub code: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dimensions: BTreeMap<DimensionName, Dimension>,
}

/// A transform's variance evidence: shared by every native unit whose
/// inputs and outputs differ, or named per unit; never both.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Variance {
    Shared {
        evidence: String,
        reconciliation_basis: String,
    },
    Units(Vec<UnitVariance>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitVariance {
    pub unit_id: i64,
    pub evidence: String,
    pub reconciliation_basis: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiptMeasurement {
    pub supplier_declared: Decimal,
    pub measured_gross: Decimal,
    pub tare: Decimal,
    pub net: Decimal,
    pub unit_id: i64,
    pub weighing_point_ref: String,
}
impl ReceiptMeasurement {
    pub fn supplier_variance(&self) -> Decimal {
        self.supplier_declared - self.net
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PostingRequest {
    pub request_id: String,
    pub evidence: String,
    pub effective_at: Option<SystemTime>,
    pub context: BTreeMap<String, String>,
    pub lines: Vec<StockLine>,
    pub receipt_measurement: Option<ReceiptMeasurement>,
    pub reason: Option<String>,
    pub corrects_transaction_id: Option<i64>,
    /// A transform's lines drawn from stock; every other posting uses `lines`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<StockLine>,
    /// A transform's lines put into stock.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<StockLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance: Option<Variance>,
}
impl PostingRequest {
    /// Every line, inputs before outputs for a transform.
    pub(crate) fn all_lines(&self) -> impl Iterator<Item = &StockLine> {
        self.lines
            .iter()
            .chain(self.inputs.iter())
            .chain(self.outputs.iter())
    }
}

pub(crate) fn invalid(field: &'static str, message: impl Into<String>) -> Error {
    Error::Invalid(vec![FieldError {
        field,
        message: message.into(),
    }])
}
fn text(field: &'static str, value: &str, max: usize) -> Result<(), Error> {
    if value.trim().is_empty() {
        return Err(invalid(field, "can't be blank"));
    }
    if value.chars().count() > max {
        return Err(invalid(
            field,
            format!("should be at most {max} character(s)"),
        ));
    }
    Ok(())
}

pub(crate) fn validate(
    kind: TransactionKind,
    request: &PostingRequest,
) -> Result<PostingRequest, Error> {
    let mut r = request.clone();
    text("request_id", &r.request_id, 255)?;
    text("evidence", &r.evidence, 10_000)?;
    if r.effective_at.is_some_and(|at| at > SystemTime::now()) {
        return Err(invalid(
            "effective_at",
            "cannot be later than the time it is recorded",
        ));
    }
    if kind == TransactionKind::Correction {
        text("reason", r.reason.as_deref().unwrap_or_default(), 10_000)?;
        if r.corrects_transaction_id.is_none() {
            return Err(invalid("corrects_transaction_id", "can't be blank"));
        }
    } else if r.reason.is_some() || r.corrects_transaction_id.is_some() {
        return Err(invalid("reason", "is not accepted for this transaction"));
    }
    for (key, value) in &r.context {
        if ![
            "operation_execution",
            "order_or_batch",
            "work_centre",
            "shipment",
            "destination",
        ]
        .contains(&key.as_str())
        {
            return Err(invalid(
                "context",
                format!("has an unknown reference {key:?}"),
            ));
        }
        text("context", value, 255)?;
    }
    if kind == TransactionKind::Transform {
        if !r.lines.is_empty() {
            return Err(invalid("lines", "is not accepted for this transaction"));
        }
        validate_lines(kind, "inputs", &mut r.inputs, false)?;
        validate_lines(kind, "outputs", &mut r.outputs, true)?;
        if let Some(variance) = &r.variance {
            validate_variance(variance)?;
        }
    } else {
        if !r.inputs.is_empty() || !r.outputs.is_empty() {
            return Err(invalid("inputs", "is not accepted for this transaction"));
        }
        if r.variance.is_some() {
            return Err(invalid("variance", "is not accepted for this transaction"));
        }
        validate_lines(kind, "lines", &mut r.lines, false)?;
    }
    if let Some(m) = &mut r.receipt_measurement {
        if kind != TransactionKind::Receipt {
            return Err(invalid(
                "receipt_measurement",
                "is not accepted for this transaction",
            ));
        }
        m.supplier_declared = m.supplier_declared.normalize();
        m.measured_gross = m.measured_gross.normalize();
        m.tare = m.tare.normalize();
        m.net = m.net.normalize();
        if m.supplier_declared <= Decimal::ZERO
            || m.measured_gross <= Decimal::ZERO
            || m.net <= Decimal::ZERO
            || m.tare < Decimal::ZERO
            || m.unit_id <= 0
        {
            return Err(invalid(
                "receipt_measurement",
                "needs positive quantities and unit_id, and non-negative tare",
            ));
        }
        text("receipt_measurement", &m.weighing_point_ref, 255)?;
        if m.measured_gross.checked_sub(m.tare) != Some(m.net) {
            return Err(invalid(
                "receipt_measurement",
                "measured_gross minus tare must equal net",
            ));
        }
        if r.lines.len() != 1 {
            return Err(invalid(
                "receipt_measurement",
                "requires exactly one measured stock line",
            ));
        }
        let line = &r.lines[0];
        if line.observation != Observation::Measured
            || line.unit_id != Some(m.unit_id)
            || line.quantity != m.net
        {
            return Err(invalid(
                "receipt_measurement",
                "net and unit must match the measured stock line",
            ));
        }
    }
    Ok(r)
}

fn validate_lines(
    kind: TransactionKind,
    field: &'static str,
    lines: &mut [StockLine],
    outputs: bool,
) -> Result<(), Error> {
    if lines.is_empty() {
        return Err(invalid(field, "needs at least one line"));
    }
    for (index, line) in lines.iter_mut().enumerate() {
        let fail = |message: &str| invalid(field, format!("line {}: {message}", index + 1));
        line.quantity = line.quantity.normalize();
        if line.quantity.is_zero() {
            return Err(fail("quantity must not be zero"));
        }
        if kind != TransactionKind::Correction && line.quantity < Decimal::ZERO {
            return Err(fail("quantity must be greater than 0"));
        }
        if line.quantity.scale() > 12 {
            return Err(fail("quantity has more than 12 decimal places"));
        }
        if line.quantity.abs() >= Decimal::from(1_000_000_000_000_i64) {
            return Err(fail("quantity is too large"));
        }
        if line.conversion_version.is_some_and(|v| v <= 0) {
            return Err(fail("conversion_version must be greater than 0"));
        }
        if line.identity_id.is_some_and(|id| id <= 0) {
            return Err(fail("identity_id must be greater than 0"));
        }
        if let Some(identity) = &line.identity {
            if !identity_valid(identity) {
                return Err(fail("identity needs kind (lot or unit), a non-empty code of at most 255 bytes, and valid unit dimensions"));
            }
        }
        if kind == TransactionKind::Transfer {
            if line.from_location_id.is_none() || line.to_location_id.is_none() {
                return Err(fail("from_location_id and to_location_id can't be blank"));
            }
            if line.from_location_id == line.to_location_id {
                return Err(fail("to_location_id must differ from from_location_id"));
            }
            line.location_id = None;
        } else {
            if line.location_id.is_none() {
                return Err(fail("location_id can't be blank"));
            }
            line.from_location_id = None;
            line.to_location_id = None;
        }
        if outputs {
            if let Some(role) = &line.output_role {
                if role.chars().count() > 64 {
                    return Err(fail("output_role should be at most 64 character(s)"));
                }
            }
        } else {
            line.output_role = None;
        }
        if let Some(evidence) = &line.evidence {
            text(field, evidence, 10_000)?;
        }
    }
    Ok(())
}

/// Dimensions only on a unit, each a positive value of at most 12 places.
fn identity_valid(identity: &NewIdentity) -> bool {
    !identity.code.trim().is_empty()
        && identity.code.len() <= 255
        && (identity.dimensions.is_empty() || identity.kind == IdentityKind::Unit)
        && identity
            .dimensions
            .values()
            .all(|d| d.value > Decimal::ZERO && d.value.normalize().scale() <= 12)
}

fn validate_variance(variance: &Variance) -> Result<(), Error> {
    let fail = |message: String| Err(invalid("variance", message));
    let texts = |evidence: &str, basis: &str, prefix: &str| -> Result<(), Error> {
        for (key, value) in [("evidence", evidence), ("reconciliation_basis", basis)] {
            if value.is_empty() || value.len() > 10_000 {
                return Err(invalid("variance", format!("{prefix} {key}")));
            }
        }
        Ok(())
    };
    match variance {
        Variance::Shared {
            evidence,
            reconciliation_basis,
        } => texts(evidence, reconciliation_basis, "needs"),
        Variance::Units(units) if units.is_empty() => {
            fail("needs evidence and reconciliation_basis, or units".into())
        }
        Variance::Units(units) => {
            for (index, unit) in units.iter().enumerate() {
                if unit.unit_id <= 0 {
                    return fail(format!("unit {} needs unit_id", index + 1));
                }
                texts(
                    &unit.evidence,
                    &unit.reconciliation_basis,
                    &format!("unit {} needs", index + 1),
                )?;
            }
            let mut ids: Vec<_> = units.iter().map(|unit| unit.unit_id).collect();
            ids.sort_unstable();
            ids.dedup();
            if ids.len() != units.len() {
                return fail("units must name each unit once".into());
            }
            Ok(())
        }
    }
}
