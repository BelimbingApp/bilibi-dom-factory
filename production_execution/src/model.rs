//! Public values of Bilimbi.Factory.ProductionExecution; persistence stays private.
use bilibi_factory_inventory::{StockLine, Variance};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// What a production order stands for: one customer order or one batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderKind {
    Order,
    Batch,
}
impl OrderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Order => "order",
            Self::Batch => "batch",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "order" => Some(Self::Order),
            "batch" => Some(Self::Batch),
            _ => None,
        }
    }
}

/// An order or batch carrying the exact formula and routing revisions it was
/// created with; they never change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub kind: OrderKind,
    pub product_id: i64,
    pub formula_version: i32,
    pub routing_version: i32,
    pub demand_ref: Option<String>,
}

/// Attributes of a new order. `demand_ref` is an opaque reference to the
/// demand source.
#[derive(Clone, Debug)]
pub struct NewOrder {
    pub code: String,
    pub kind: OrderKind,
    pub product_id: i64,
    pub formula_version: i32,
    pub routing_version: i32,
    pub demand_ref: Option<String>,
}

/// Where a completion came from. An import needs the import capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Live,
    Import,
}
impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Import => "import",
        }
    }
}

/// An imported override's historical approver, source evidence only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approver {
    /// `user`, or `agent` with `acting_for_user_id`.
    pub kind: String,
    pub id: i64,
    pub acting_for_user_id: Option<i64>,
}

/// A request to consume held material early. The recorder is always the
/// Scope's user; a live override is approved by the same user. An import also
/// carries its source `evidence`, the historical `occurred_at` (not after
/// completion) and optionally the historical `approver`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HoldOverrideRequest {
    pub reason: String,
    pub evidence: Option<String>,
    pub occurred_at: Option<SystemTime>,
    pub approver: Option<Approver>,
}

/// One completed routed operation, live or imported. `request_id` is the
/// company-unique retry key.
#[derive(Clone, Debug)]
pub struct Completion {
    pub request_id: String,
    pub operation_code: String,
    pub resource_id: i64,
    pub operator_type: String,
    pub operator_id: i64,
    pub started_at: SystemTime,
    pub completed_at: SystemTime,
    pub evidence: String,
    pub inputs: Vec<StockLine>,
    pub outputs: Vec<StockLine>,
    pub variance: Option<Variance>,
    pub hold_override: Option<HoldOverrideRequest>,
}

#[derive(Clone, Debug)]
pub struct Execution {
    pub id: i64,
    pub company_id: i64,
    pub order_id: i64,
    pub request_id: String,
    pub source: Source,
    pub operation_code: String,
    pub resource_id: i64,
    pub operator_type: String,
    pub operator_id: i64,
    pub started_at: SystemTime,
    pub completed_at: SystemTime,
    pub inputs: Vec<StockLine>,
    pub outputs: Vec<StockLine>,
    pub variance: Option<Variance>,
    pub evidence: String,
    pub inventory_transaction_id: i64,
    pub formula_version: i32,
    pub routing_version: i32,
}

/// Immutable evidence that held material was consumed early.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldOverride {
    pub id: i64,
    pub execution_id: i64,
    pub inventory_transaction_id: i64,
    pub source_transaction_id: i64,
    pub identity_id: i64,
    pub item_id: i64,
    pub source: Source,
    pub actor_type: Option<String>,
    pub actor_id: Option<i64>,
    pub acting_for_user_id: Option<i64>,
    pub recorded_by_type: String,
    pub recorded_by_id: i64,
    pub recorded_by_acting_for_user_id: Option<i64>,
    pub reason: String,
    pub evidence: Option<String>,
    pub occurred_at: SystemTime,
}
