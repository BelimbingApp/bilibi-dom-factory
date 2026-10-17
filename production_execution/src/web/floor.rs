//! Selection and capture intents of Bilimbi `FloorLive`; SQL stays in facades.
use super::{shared as s, Pages};
use crate::{contributions as caps, Execution, Order, RunYield};
use bilibi_database::{Error, Result};
use bilibi_datetime::DisplayContext;
use bilibi_factory_inventory::Location;
use bilibi_tenancy::Scope;
use bilibi_ui::{BuiltinColumn, ListColumns};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub(super) struct Line {
    pub(super) key: String,
    pub(super) label: String,
    pub(super) item: i64,
    pub(super) identity: Option<i64>,
    pub(super) location: Option<i64>,
    pub(super) quantity: String,
    pub(super) unit: String,
    pub(super) output: bool,
}
pub(super) struct Floor {
    pub(super) pages: Pages,
    pub(super) labour: super::labour::Labour,
    pub(super) orders: Vec<Order>,
    pub(super) order: Option<Order>,
    pub(super) runs: Vec<Execution>,
    pub(super) run: Option<Execution>,
    pub(super) lines: Vec<Line>,
    pub(super) locations: Vec<Location>,
    pub(super) reasons: Vec<Value>,
    pub(super) types: Vec<Value>,
    pub(super) wastage: Vec<Value>,
    pub(super) measurements: Vec<Value>,
    pub(super) run_yield: Option<RunYield>,
    pub(super) waste_draft: HashMap<String, String>,
    pub(super) measure_draft: HashMap<String, String>,
    pub(super) correction: Option<Value>,
    pub(super) correction_draft: HashMap<String, String>,
    pub(super) measure_correction: Option<Value>,
    pub(super) measure_correction_draft: HashMap<String, String>,
    pub(super) waste_columns: ListColumns,
    pub(super) measure_columns: ListColumns,
    pub(super) allowed: HashSet<&'static str>,
    pub(super) display: DisplayContext,
    pub(super) error: Option<String>,
    pub(super) success: Option<String>,
}
impl Floor {
    pub(super) fn new(pages: &Pages) -> Result<Self> {
        let columns = |id, cols: Vec<(&str, &str)>| {
            ListColumns::mount(
                id,
                cols.into_iter()
                    .map(|(id, label)| BuiltinColumn::new(id, label))
                    .collect(),
            )
            .map_err(|_| Error::Internal)
        };
        Ok(Self {
            pages: pages.clone(),
            labour: super::labour::Labour::new()?,
            orders: vec![],
            order: None,
            runs: vec![],
            run: None,
            lines: vec![],
            locations: vec![],
            reasons: vec![],
            types: vec![],
            wastage: vec![],
            measurements: vec![],
            run_yield: None,
            waste_draft: Self::waste_draft(),
            measure_draft: Self::measure_draft(),
            correction: None,
            correction_draft: HashMap::new(),
            measure_correction: None,
            measure_correction_draft: HashMap::new(),
            waste_columns: columns(
                "wastage-table",
                vec![
                    ("material", "Material"),
                    ("quantity", "Quantity"),
                    ("reason", "Reason"),
                    ("when", "When"),
                    ("note", "Note"),
                    ("corrected", "Corrected"),
                ],
            )?,
            measure_columns: columns(
                "measurements-table",
                vec![
                    ("measured", "Measured"),
                    ("output", "Output"),
                    ("value", "Value"),
                    ("limits", "Limits"),
                    ("check", "Check"),
                    ("when", "When"),
                    ("corrected", "Corrected"),
                ],
            )?,
            allowed: HashSet::new(),
            display: DisplayContext::anonymous(),
            error: None,
            success: None,
        })
    }
    fn waste_draft() -> HashMap<String, String> {
        s::draft(&[
            ("line", ""),
            ("location_id", ""),
            ("quantity", ""),
            ("observation", "measured"),
            ("reason_id", ""),
            ("note", ""),
        ])
    }
    fn measure_draft() -> HashMap<String, String> {
        s::draft(&[
            ("measurement_type_id", ""),
            ("identity_id", ""),
            ("value", ""),
            ("note", ""),
        ])
    }
    pub(super) fn query(&self) -> HashMap<String, String> {
        let mut q = HashMap::new();
        if let Some(o) = &self.order {
            q.insert("order".into(), o.id.to_string());
        }
        if let Some(r) = &self.run {
            q.insert("run".into(), r.id.to_string());
        }
        q
    }
    pub(super) async fn select(
        &mut self,
        scope: &Scope,
        q: &HashMap<String, String>,
    ) -> Result<()> {
        self.error = None;
        self.success = None;
        self.correction = None;
        self.measure_correction = None;
        self.order = None;
        self.run = None;
        self.labour = super::labour::Labour::new()?;
        self.runs.clear();
        self.clear_run();
        let Ok(company) = s::company(scope) else {
            self.error = Some("Company not found".into());
            return Ok(());
        };
        if let Some(order) = bilibi_ui::params::positive_id(q.get("order").map(String::as_str)) {
            match self.pages.execution.get_order(scope, company, order).await {
                Ok(order) => self.order = Some(order),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.load(scope).await?;
        if let Some(run) = bilibi_ui::params::positive_id(q.get("run").map(String::as_str)) {
            self.run = self.runs.iter().find(|r| r.id == run).cloned();
            self.load_run(scope, company).await?;
        }
        self.waste_draft = Self::waste_draft();
        self.measure_draft = Self::measure_draft();
        Ok(())
    }
    fn clear_run(&mut self) {
        self.lines.clear();
        self.locations.clear();
        self.reasons.clear();
        self.types.clear();
        self.wastage.clear();
        self.measurements.clear();
        self.run_yield = None;
    }
    pub(super) async fn load(&mut self, scope: &Scope) -> Result<()> {
        self.allowed.clear();
        for cap in [
            caps::WASTAGE_RECORD,
            caps::WASTAGE_CORRECT,
            caps::MEASUREMENT_RECORD,
            caps::MEASUREMENT_CORRECT,
            caps::LABOUR_RECORD,
            caps::LABOUR_MANAGE,
        ] {
            if s::can(&self.pages.db, scope, cap).await? {
                self.allowed.insert(cap);
            }
        }
        let Ok(company) = s::company(scope) else {
            self.orders.clear();
            self.runs.clear();
            self.order = None;
            self.run = None;
            self.clear_run();
            self.error = Some("Company not found".into());
            return Ok(());
        };
        let c = self.pages.db.get().await?;
        let locale = self.pages.locales.resolve(&**c, scope, None).await?.locale;
        self.display = self
            .pages
            .display
            .display(&**c, scope, scope.actor().organisation(), locale)
            .await?;
        drop(c);
        match self.pages.execution.list_orders(scope, company, None).await {
            Ok(v) => self.orders = v,
            Err(e) => {
                self.orders.clear();
                self.error = Some(e.to_string());
            }
        }
        if let Some(order) = &self.order {
            match self
                .pages
                .execution
                .list_executions(scope, company, order.id, None)
                .await
            {
                Ok(v) => self.runs = v,
                Err(e) => {
                    self.runs.clear();
                    self.run = None;
                    self.clear_run();
                    self.error = Some(e.to_string());
                }
            }
        }
        self.load_labour(scope, company).await?;
        self.load_run(scope, company).await
    }
    async fn load_run(&mut self, scope: &Scope, company: i64) -> Result<()> {
        let Some(run) = &self.run else {
            return Ok(());
        };
        let id = run.id;
        let result = self.read_run(scope, company, id).await;
        if let Err(e) = result {
            self.clear_run();
            self.error = Some(e.to_string());
        }
        self.waste_columns
            .load(&self.wastage, |v| v["id"].to_string());
        self.measure_columns
            .load(&self.measurements, |v| v["id"].to_string());
        Ok(())
    }
    async fn read_run(
        &mut self,
        scope: &Scope,
        company: i64,
        id: i64,
    ) -> std::result::Result<(), crate::Error> {
        let run = self
            .pages
            .execution
            .get_execution(scope, company, id)
            .await?;
        let inv = &self.pages.inventory;
        let tx = inv
            .get_transaction(scope, company, run.inventory_transaction_id)
            .await?;
        let mut lines = vec![];
        let mut seen = HashSet::new();
        // Own entries before awaiting facade calls (all facade futures must be Send).
        let entries: Vec<_> = tx
            .entries
            .into_iter()
            .filter(|e| e.role == "stock")
            .collect();
        for e in entries {
            let Some(item) = e.item_id else {
                continue;
            };
            if !seen.insert((item, e.identity_id)) {
                continue;
            }
            let item_row = inv.get_item(scope, company, item).await?;
            let mut label = format!("{} · {}", item_row.sku, item_row.title);
            if let Some(identity) = e.identity_id {
                label.push_str(&format!(
                    " · {}",
                    inv.get_identity(scope, company, identity).await?.code
                ));
            }
            let unit = inv.get_unit(scope, company, e.native_unit_id).await?;
            lines.push(Line {
                key: format!(
                    "{item}:{}",
                    e.identity_id.map(|i| i.to_string()).unwrap_or_default()
                ),
                label,
                item,
                identity: e.identity_id,
                location: e.location_id,
                quantity: e.native_quantity.trim_start_matches('-').to_string(),
                unit: unit.code,
                output: !e.native_quantity.starts_with('-'),
            });
        }
        self.lines = lines;
        self.locations = inv.list_locations(scope, company, Some(500)).await?;
        self.reasons = self
            .pages
            .execution
            .list_wastage_reasons(scope, company, None)
            .await?;
        self.wastage = self
            .pages
            .execution
            .list_wastage(scope, company, id)
            .await?;
        self.run_yield = Some(
            self.pages
                .execution
                .get_run_yield(scope, company, id)
                .await?,
        );
        self.types = self
            .pages
            .execution
            .list_measurement_types(scope, company, None)
            .await?;
        self.measurements = self
            .pages
            .execution
            .list_measurements(scope, company, id)
            .await?;
        Ok(())
    }
    pub(super) async fn event(&mut self, scope: &Scope, p: &Value) -> Result<()> {
        self.success = None;
        if self.labour_event(scope, p).await? {
            return Ok(());
        }
        let kind = s::text(p, "type");
        match kind {
            "wastage_grid" => {
                let _ = self
                    .waste_columns
                    .handle(&bilibi_ui::params::string_values(p));
                return Ok(());
            }
            "measurement_grid" => {
                let _ = self
                    .measure_columns
                    .handle(&bilibi_ui::params::string_values(p));
                return Ok(());
            }
            "pick_order" | "pick_run" | "back_to_orders" | "back_to_runs" => {
                let mut q = self.query();
                match kind {
                    "pick_order" => {
                        q.clear();
                        if let Some(id) = s::id(p, "id") {
                            q.insert("order".into(), id.to_string());
                        }
                    }
                    "pick_run" => {
                        if let Some(id) = s::id(p, "id") {
                            q.insert("run".into(), id.to_string());
                        }
                    }
                    "back_to_orders" => q.clear(),
                    _ => {
                        q.remove("run");
                    }
                }
                return self.select(scope, &q).await;
            }
            "cancel_correction" => {
                self.correction = None;
                return Ok(());
            }
            "cancel_measurement_correction" => {
                self.measure_correction = None;
                return Ok(());
            }
            _ => {}
        }
        let (cap, denied) = match kind {
            "pick_reason" | "change_wastage" | "save_wastage" => (
                caps::WASTAGE_RECORD,
                "You do not have permission to record wastage.",
            ),
            "start_correction" | "save_wastage_correction" => (
                caps::WASTAGE_CORRECT,
                "You do not have permission to correct wastage.",
            ),
            "pick_measurement_type" | "change_measurement" | "save_measurement" => (
                caps::MEASUREMENT_RECORD,
                "You do not have permission to record measurements.",
            ),
            "start_measurement_correction" | "save_measurement_correction" => (
                caps::MEASUREMENT_CORRECT,
                "You do not have permission to correct measurements.",
            ),
            _ => return Err(Error::Invalid),
        };
        if !s::can(&self.pages.db, scope, cap).await? {
            self.error = Some(denied.into());
            return self.load(scope).await;
        }
        self.error = None;
        match kind {
            "pick_reason" => {
                self.waste_draft
                    .insert("reason_id".into(), s::text(p, "id").into());
            }
            "pick_measurement_type" => {
                self.measure_draft
                    .insert("measurement_type_id".into(), s::text(p, "id").into());
                self.measure_draft.insert("value".into(), String::new());
            }
            "change_wastage" => self.waste_draft.extend(s::form(p, "wastage")),
            "change_measurement" => self.measure_draft.extend(s::form(p, "measurement")),
            "start_correction" => {
                self.correction = self
                    .wastage
                    .iter()
                    .find(|v| s::id(v, "id") == s::id(p, "id") && v["corrected_by_id"].is_null())
                    .cloned();
                if let Some(row) = &self.correction {
                    self.correction_draft = s::draft(&[
                        ("quantity", s::text(row, "quantity")),
                        ("note", s::text(row, "note")),
                        ("correction_reason", ""),
                    ]);
                    self.correction_draft
                        .insert("reason_id".into(), row["reason_id"].to_string());
                } else {
                    self.error = Some("Wastage record not found.".into());
                }
            }
            "start_measurement_correction" => {
                self.measure_correction = self
                    .measurements
                    .iter()
                    .find(|v| s::id(v, "id") == s::id(p, "id") && v["corrected_by_id"].is_null())
                    .cloned();
                if let Some(row) = &self.measure_correction {
                    self.measure_correction_draft = s::draft(&[
                        ("value", s::text(row, "value")),
                        ("note", s::text(row, "note")),
                        ("correction_reason", ""),
                    ]);
                } else {
                    self.error = Some("Measurement not found.".into());
                }
            }
            _ => self.save(scope, kind, p).await?,
        }
        Ok(())
    }
    async fn save(&mut self, scope: &Scope, kind: &str, p: &Value) -> Result<()> {
        let Ok(company) = s::company(scope) else {
            self.error = Some("Company not found".into());
            return Ok(());
        };
        let Some(run) = self.run.as_ref().map(|r| r.id) else {
            self.error = Some("Execution not found".into());
            return Ok(());
        };
        let pe = &self.pages.execution;
        let (result, message) = match kind {
            "save_wastage" => {
                self.waste_draft.extend(s::form(p, "wastage"));
                let d = &self.waste_draft;
                let Some(line) = self.lines.iter().find(|l| l.key == s::get(d, "line")) else {
                    self.error = Some("Choose the material that was scrapped.".into());
                    return Ok(());
                };
                let Some(reason) = bilibi_ui::params::positive_id(Some(s::get(d, "reason_id")))
                else {
                    self.error = Some("Choose a wastage reason.".into());
                    return Ok(());
                };
                let attrs = json!({"request_id":s::get(d,"request_id"),"reason_id":reason,"item_id":line.item,"identity_id":line.identity,"location_id":bilibi_ui::params::positive_id(Some(s::get(d,"location_id"))).or(line.location),"quantity":s::get(d,"quantity"),"observation":s::get(d,"observation"),"note":s::get(d,"note")});
                (
                    pe.record_wastage(scope, company, run, &attrs).await,
                    "Wastage recorded.",
                )
            }
            "save_wastage_correction" => {
                self.correction_draft.extend(s::form(p, "correction"));
                let d = &self.correction_draft;
                let Some(id) = self.correction.as_ref().and_then(|v| s::id(v, "id")) else {
                    self.error = Some("Wastage record not found.".into());
                    return Ok(());
                };
                let attrs = json!({"request_id":s::get(d,"request_id"),"quantity":s::get(d,"quantity"),"reason_id":bilibi_ui::params::positive_id(Some(s::get(d,"reason_id"))),"note":s::get(d,"note"),"correction_reason":s::get(d,"correction_reason")});
                (
                    pe.correct_wastage(scope, company, id, &attrs).await,
                    "Wastage corrected.",
                )
            }
            "save_measurement" => {
                self.measure_draft.extend(s::form(p, "measurement"));
                let d = &self.measure_draft;
                let Some(ty) = self.types.iter().find(|t| {
                    t["id"].as_i64()
                        == bilibi_ui::params::positive_id(Some(s::get(d, "measurement_type_id")))
                }) else {
                    self.error = Some("Choose what was measured.".into());
                    return Ok(());
                };
                let attrs = json!({"request_id":s::get(d,"request_id"),"measurement_type_id":ty["id"],"identity_id":bilibi_ui::params::positive_id(Some(s::get(d,"identity_id"))),"value":s::typed(s::text(ty,"value_type"),s::get(d,"value")),"note":s::get(d,"note")});
                (
                    pe.record_measurement(scope, company, run, &attrs).await,
                    "Measurement recorded.",
                )
            }
            _ => {
                self.measure_correction_draft
                    .extend(s::form(p, "measurement_correction"));
                let d = &self.measure_correction_draft;
                let Some(row) = &self.measure_correction else {
                    self.error = Some("Measurement not found.".into());
                    return Ok(());
                };
                let attrs = json!({"request_id":s::get(d,"request_id"),"value":s::typed(s::text(row,"value_type"),s::get(d,"value")),"note":s::get(d,"note"),"correction_reason":s::get(d,"correction_reason")});
                (
                    pe.correct_measurement(
                        scope,
                        company,
                        s::id(row, "id").ok_or(Error::Invalid)?,
                        &attrs,
                    )
                    .await,
                    "Measurement corrected.",
                )
            }
        };
        match result {
            Ok(_) => {
                self.success = Some(message.into());
                self.correction = None;
                self.measure_correction = None;
                self.waste_draft = Self::waste_draft();
                self.measure_draft = Self::measure_draft();
                self.load(scope).await?;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        Ok(())
    }
}
