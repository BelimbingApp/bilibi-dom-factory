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
    Correction,
}
impl TransactionKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Transfer => "transfer",
            Self::Consumption => "consumption",
            Self::Correction => "correction",
        }
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

/// A stock observation, or a transfer from one location to another. Only a
/// correction accepts a negative quantity. Actors always come from Scope.
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
    if r.lines.is_empty() {
        return Err(invalid("lines", "needs at least one line"));
    }
    for (index, line) in r.lines.iter_mut().enumerate() {
        let fail = |message: &str| invalid("lines", format!("line {}: {message}", index + 1));
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
        if let Some(evidence) = &line.evidence {
            text("lines", evidence, 10_000)?;
        }
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
