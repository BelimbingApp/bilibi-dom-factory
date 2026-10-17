//! Bilimbi FloorLive's order/run picker, yield and capture panels.
use super::{floor::Floor, shared as s};
use crate::contributions as caps;
use bilibi_database::{Error, Result};
use bilibi_datetime::{Format, Precision};
use bilibi_ui::{
    Alert, Badge, BadgeKind, Card, Cell, Choice, EmptyState, Field, FlexRow, FlexTable, Html,
    InputKind, ListColumns, StatItem, StatStrip, StatusKind, TableEmpty, NO_FLASH,
};
use serde_json::Value;

impl Floor {
    pub(super) fn card(&self, id: &str, title: &str, body: Html) -> Result<Html> {
        s::render(Card {
            id,
            title,
            body,
            class: "mt-5",
            inner_class: "p-0",
            ..Card::default()
        })
    }
    pub(super) fn view(&self) -> Result<String> {
        let mut body = Html::EMPTY;
        match (&self.order, &self.run) {
            (None, _) => {
                let mut choices = Html::EMPTY;
                for order in &self.orders {
                    choices.push(&s::button(
                        &format!("{} · {}", order.code, order.kind.name()),
                        "pick_order",
                        &order.id.to_string(),
                        false,
                    )?);
                }
                body.push(&s::render(s::Picker {
                    id: "floor-orders",
                    title: "Choose an order",
                    actions: Html::EMPTY,
                    buttons: choices,
                    empty: s::render(EmptyState::missing(
                        "No orders",
                        "Orders appear here once they are created.",
                    ))?,
                })?);
            }
            (Some(order), None) => {
                let mut choices = Html::EMPTY;
                for run in &self.runs {
                    let mut label = Html::text(&format!("{} · ", run.operation_code));
                    label.push(&self.instant(
                        &format!("run-{}-completed", run.id),
                        jiff::Timestamp::try_from(run.completed_at).map_err(|_| Error::Invalid)?,
                    )?);
                    choices.push(&s::render(bilibi_ui::Button {
                        button_type: "button",
                        event: "pick_run",
                        name: "id",
                        value: &run.id.to_string(),
                        class: s::TOUCH,
                        body: label,
                        ..bilibi_ui::Button::default()
                    })?);
                }
                body.push(&s::render(s::Picker {
                    id: "floor-runs",
                    title: &format!("Runs of {}", order.code),
                    actions: s::button("Orders", "back_to_orders", "", false)?,
                    buttons: choices,
                    empty: s::render(EmptyState::missing(
                        "No runs",
                        "A run appears here once an operation of this order is completed.",
                    ))?,
                })?);
            }
            (Some(order), Some(run)) => {
                let mut heading = s::button("Runs", "back_to_runs", "", false)?;
                heading.push(&Html::text(&format!(
                    " {} · {} · ",
                    order.code, run.operation_code
                )));
                heading.push(&self.instant(
                    "run-completed",
                    jiff::Timestamp::try_from(run.completed_at).map_err(|_| Error::Invalid)?,
                )?);
                body.push(&self.card("floor-run", "", heading)?);
                if let Some(yield_) = &self.run_yield {
                    let mut stats = Html::EMPTY;
                    for balance in &yield_.balances {
                        stats.push(&s::render(StatStrip {
                            id: &format!("floor-yield-unit-{}", balance.unit.id),
                            title: &balance.unit.code,
                            href: "",
                            items: [
                                ("Input", &balance.input),
                                ("Product", &balance.product),
                                ("Trim", &balance.trim),
                                ("Waste", &balance.waste),
                                ("Recorded wastage", &balance.wastage),
                                ("Variance", &balance.variance),
                            ]
                            .into_iter()
                            .map(|(label, value)| {
                                StatItem::number(
                                    label,
                                    format!("{} {}", quantity(value), balance.unit.code),
                                )
                            })
                            .collect(),
                        })?);
                    }
                    body.push(&self.card("floor-yield", "Run yield", stats)?);
                }
                if self.allowed.contains(caps::WASTAGE_RECORD) {
                    body.push(&self.waste_editor()?);
                } else {
                    body.push(&s::render(EmptyState::missing(
                        "Record wastage unavailable",
                        "You do not have permission to record wastage.",
                    ))?);
                }
                body.push(&self.waste_table()?);
                if self.correction.is_some() {
                    body.push(&self.waste_correction()?);
                }
                if self.allowed.contains(caps::MEASUREMENT_RECORD) {
                    body.push(&self.measure_editor()?);
                } else {
                    body.push(&s::render(EmptyState::missing(
                        "Measure output unavailable",
                        "You do not have permission to record measurements.",
                    ))?);
                }
                body.push(&self.measure_table()?);
                if self.measure_correction.is_some() {
                    body.push(&self.measure_correction_editor()?);
                }
            }
        }
        if self.order.is_some() {
            body.push(&self.labour_panel()?);
        }
        let error = match &self.error {
            Some(error) => s::render(Alert {
                kind: StatusKind::Error,
                id: "floor-error",
                class: "mt-4",
                body: Html::text(error),
            })?,
            None => Html::EMPTY,
        };
        let mut flash = NO_FLASH;
        if let Some(success) = &self.success {
            flash.success = success;
        }
        Ok(s::render(s::PageView { body, error, flash })?
            .as_str()
            .to_string())
    }
    pub(super) fn instant(&self, id: &str, at: jiff::Timestamp) -> Result<Html> {
        s::render(s::InstantView {
            id,
            value: {
                let r = self.display.render(at, Format::DateTime, Precision::Minute);
                bilibi_ui::flex_table::Instant::new(
                    &r.iso,
                    &r.text,
                    &r.company,
                    &r.utc,
                    r.mode.as_str(),
                )
            },
        })
    }
    pub(super) fn time_cell(&self, v: &Value, key: &str) -> Result<Cell> {
        let at = s::text(v, key)
            .parse::<jiff::Timestamp>()
            .map_err(|_| Error::Invalid)?;
        let r = self.display.render(at, Format::DateTime, Precision::Minute);
        Ok(Cell::instant(bilibi_ui::flex_table::Instant::new(
            &r.iso,
            &r.text,
            &r.company,
            &r.utc,
            r.mode.as_str(),
        )))
    }
    fn reason_choices(&self, correcting: bool) -> Vec<Choice> {
        self.reasons
            .iter()
            .filter(|r| {
                r["active"] == true
                    || (correcting
                        && self
                            .correction
                            .as_ref()
                            .is_some_and(|c| c["reason_id"] == r["id"]))
            })
            .map(|r| Choice::new(s::text(r, "label"), r["id"].to_string()))
            .collect()
    }
    pub(super) fn choices(
        &self,
        values: &[Value],
        legend: &str,
        event: &str,
        selected: &str,
        empty: &str,
    ) -> Result<Html> {
        let mut buttons = Html::EMPTY;
        for v in values.iter().filter(|v| v["active"] == true) {
            let id = v["id"].to_string();
            let label = if let Some(unit) = v["unit"].as_str() {
                format!("{} ({unit})", s::text(v, "label"))
            } else {
                s::text(v, "label").to_string()
            };
            buttons.push(&s::button(&label, event, &id, id == selected)?);
        }
        s::render(s::Choices {
            legend,
            buttons,
            empty,
        })
    }
    fn waste_editor(&self) -> Result<Html> {
        let d = &self.waste_draft;
        let fields = vec![
            hidden("wastage", "request_id", d),
            hidden("wastage", "reason_id", d),
            s::select(
                "wastage",
                "line",
                "Material",
                "Choose material",
                d,
                self.lines
                    .iter()
                    .map(|l| {
                        Choice::new(
                            format!(
                                "{}: {} ({} {})",
                                if l.output { "Output" } else { "Input" },
                                l.label,
                                quantity(&l.quantity),
                                l.unit
                            ),
                            &l.key,
                        )
                    })
                    .collect(),
            ),
            s::select(
                "wastage",
                "location_id",
                "Drawn from",
                "The run's location",
                d,
                self.locations
                    .iter()
                    .map(|l| Choice::new(format!("{} · {}", l.code, l.name), l.id.to_string()))
                    .collect(),
            ),
            numeric(
                s::field("wastage", "quantity", "Quantity (native unit)", d),
                "decimal",
            ),
            s::select(
                "wastage",
                "observation",
                "Quantity was",
                "",
                d,
                [
                    ("measured", "measured"),
                    ("declared", "declared"),
                    ("counted", "counted"),
                    ("derived", "derived"),
                ]
                .into_iter()
                .map(|(label, value)| Choice::new(label, value))
                .collect(),
            ),
        ];
        let choices = self.choices(
            &self.reasons,
            "Reason",
            "pick_reason",
            s::get(d, "reason_id"),
            "No active wastage reasons. An administrator defines them under Wastage reasons.",
        )?;
        let form = s::render(s::FormView {
            id: "wastage-form",
            event: "save_wastage",
            change: "change_wastage",
            fields,
            choices,
            after: vec![s::field("wastage", "note", "Note (optional)", d)],
            submit: "Record wastage",
            cancel: "",
        })?;
        self.card("floor-wastage-entry", "Record wastage", form)
    }
    fn waste_correction(&self) -> Result<Html> {
        let d = &self.correction_draft;
        let form = s::render(s::FormView {
            id: "wastage-correction-form",
            event: "save_wastage_correction",
            change: "",
            fields: vec![
                hidden("correction", "request_id", d),
                numeric(
                    s::field(
                        "correction",
                        "quantity",
                        "Corrected quantity (0 voids it)",
                        d,
                    ),
                    "decimal",
                ),
                s::select(
                    "correction",
                    "reason_id",
                    "Reason",
                    "",
                    d,
                    self.reason_choices(true),
                ),
                s::field("correction", "note", "Note", d),
                s::field(
                    "correction",
                    "correction_reason",
                    "Why is this corrected?",
                    d,
                ),
            ],
            choices: Html::EMPTY,
            after: vec![],
            submit: "Save correction",
            cancel: "cancel_correction",
        })?;
        self.card("wastage-correction", "Correct wastage", form)
    }
    fn measurement_value(
        &self,
        form: &str,
        d: &std::collections::HashMap<String, String>,
        kind: &str,
        label: &str,
        hint: &str,
    ) -> Field {
        match kind {
            "boolean" => s::select(
                form,
                "value",
                label,
                "Choose",
                d,
                vec![Choice::new("Yes", "true"), Choice::new("No", "false")],
            ),
            "integer" | "decimal" => numeric(
                s::field(form, "value", label, d).hint(hint),
                if kind == "integer" {
                    "numeric"
                } else {
                    "decimal"
                },
            ),
            _ => s::field(form, "value", label, d),
        }
    }
    fn measure_editor(&self) -> Result<Html> {
        let d = &self.measure_draft;
        let mut after = vec![s::select(
            "measurement",
            "identity_id",
            "Output",
            "Run output as a whole",
            d,
            self.lines
                .iter()
                .filter(|l| l.output && l.identity.is_some())
                .map(|l| Choice::new(&l.label, l.identity.unwrap().to_string()))
                .collect(),
        )];
        if let Some(ty) = self.types.iter().find(|t| {
            t["id"].as_i64()
                == bilibi_ui::params::positive_id(Some(s::get(d, "measurement_type_id")))
        }) {
            let label = if let Some(unit) = ty["unit"].as_str() {
                format!("{} ({unit})", s::text(ty, "label"))
            } else {
                s::text(ty, "label").to_string()
            };
            after.push(self.measurement_value(
                "measurement",
                d,
                s::text(ty, "value_type"),
                &label,
                &s::limits(ty),
            ));
        }
        if !self.types.iter().any(|t| {
            t["id"].as_i64()
                == bilibi_ui::params::positive_id(Some(s::get(d, "measurement_type_id")))
        }) {
            after[0].hint = "Choose what was measured to enter its value.".into();
        }
        after.push(s::field("measurement", "note", "Note (optional)", d));
        let choices = self.choices(
            &self.types,
            "What was measured",
            "pick_measurement_type",
            s::get(d, "measurement_type_id"),
            "No active measurement types. An administrator defines them under Measurement types.",
        )?;
        let form = s::render(s::FormView {
            id: "measurement-form",
            event: "save_measurement",
            change: "change_measurement",
            fields: vec![
                hidden("measurement", "request_id", d),
                hidden("measurement", "measurement_type_id", d),
            ],
            choices,
            after,
            submit: "Record measurement",
            cancel: "",
        })?;
        self.card("floor-measurement-entry", "Measure output", form)
    }
    fn measure_correction_editor(&self) -> Result<Html> {
        let row = self.measure_correction.as_ref().ok_or(Error::Invalid)?;
        let d = &self.measure_correction_draft;
        let form = s::render(s::FormView {
            id: "measurement-correction-form",
            event: "save_measurement_correction",
            change: "",
            fields: vec![
                hidden("measurement_correction", "request_id", d),
                self.measurement_value(
                    "measurement_correction",
                    d,
                    s::text(row, "value_type"),
                    "Corrected value",
                    &s::limits(row),
                ),
                s::field("measurement_correction", "note", "Note", d),
                s::field(
                    "measurement_correction",
                    "correction_reason",
                    "Why is this corrected?",
                    d,
                ),
            ],
            choices: Html::EMPTY,
            after: vec![],
            submit: "Save correction",
            cancel: "cancel_measurement_correction",
        })?;
        self.card("measurement-correction", "Correct measurement", form)
    }
    fn line_label(&self, record: &Value) -> String {
        self.lines
            .iter()
            .find(|l| {
                Some(l.item) == record["item_id"].as_i64()
                    && l.identity == record["identity_id"].as_i64()
            })
            .map(|l| l.label.clone())
            .unwrap_or_else(|| format!("Item {}", record["item_id"]))
    }
    fn waste_table(&self) -> Result<Html> {
        let mut rows = vec![];
        for v in self
            .wastage
            .iter()
            .filter(|v| v["corrected_by_id"].is_null())
        {
            let id = v["id"].to_string();
            let unit = self
                .lines
                .iter()
                .find(|l| {
                    Some(l.item) == v["item_id"].as_i64() && l.identity == v["identity_id"].as_i64()
                })
                .map(|l| l.unit.as_str())
                .unwrap_or("");
            let reason = self
                .reasons
                .iter()
                .find(|r| r["id"] == v["reason_id"])
                .map(|r| format!("{} · {}", s::text(r, "code"), s::text(r, "label")))
                .unwrap_or_else(|| "—".into());
            let mut row = FlexRow::new(&id)
                .cell("material", Cell::text(self.line_label(v)))
                .cell(
                    "quantity",
                    Cell::text(format!("{} {unit}", quantity(s::text(v, "quantity")))),
                )
                .cell("reason", Cell::text(reason))
                .cell("when", self.time_cell(v, "occurred_at")?)
                .cell("note", Cell::text(s::text(v, "note")))
                .cell("corrected", Cell::text(s::text(v, "correction_reason")));
            row.dom_id = Some(format!("wastage-{id}"));
            if self.allowed.contains(caps::WASTAGE_CORRECT) {
                row.actions = s::button("Correct", "start_correction", &id, false)?;
            }
            rows.push(row);
        }
        self.card(
            "floor-wastage",
            "Wastage on this run",
            table(
                "wastage-table",
                "wastage_grid",
                "Current wastage",
                &self.waste_columns,
                rows,
                self.allowed.contains(caps::WASTAGE_CORRECT),
                ("No wastage", "Nothing has been scrapped on this run yet."),
            )?,
        )
    }
    fn measure_table(&self) -> Result<Html> {
        let mut rows = vec![];
        for v in self
            .measurements
            .iter()
            .filter(|v| v["corrected_by_id"].is_null())
        {
            let id = v["id"].to_string();
            let label = self
                .types
                .iter()
                .find(|t| t["id"] == v["measurement_type_id"])
                .map(|t| s::text(t, "label"))
                .unwrap_or("—");
            let output = match v["identity_id"].as_i64() {
                None => "Run output".into(),
                Some(id) => self
                    .lines
                    .iter()
                    .find(|l| l.identity == Some(id))
                    .map(|l| l.label.clone())
                    .unwrap_or_else(|| format!("Unit {id}")),
            };
            let mut row = FlexRow::new(&id)
                .cell("measured", Cell::text(label))
                .cell("output", Cell::text(output))
                .cell(
                    "value",
                    Cell::text(format!("{} {}", s::text(v, "value"), s::text(v, "unit"))),
                )
                .cell("limits", Cell::text(s::limits(v)))
                .cell("when", self.time_cell(v, "measured_at")?)
                .cell("corrected", Cell::text(s::text(v, "correction_reason")));
            if let Some(out) = v["out_of_range"].as_bool() {
                row = row.slot(
                    "check",
                    s::render(Badge {
                        kind: if out {
                            BadgeKind::Danger
                        } else {
                            BadgeKind::Success
                        },
                        body: Html::text(if out { "Out of range" } else { "In range" }),
                        ..Badge::default()
                    })?,
                );
            }
            row.dom_id = Some(format!("measurement-{id}"));
            if self.allowed.contains(caps::MEASUREMENT_CORRECT) {
                row.actions = s::button("Correct", "start_measurement_correction", &id, false)?;
            }
            rows.push(row);
        }
        self.card(
            "floor-measurements",
            "Output measurements",
            table(
                "measurements-table",
                "measurement_grid",
                "Current measurements",
                &self.measure_columns,
                rows,
                self.allowed.contains(caps::MEASUREMENT_CORRECT),
                (
                    "No measurements",
                    "Nothing has been measured on this run's output yet.",
                ),
            )?,
        )
    }
}
pub(super) fn hidden(
    form: &str,
    key: &str,
    d: &std::collections::HashMap<String, String>,
) -> Field {
    s::field(form, key, "", d).kind(InputKind::Hidden)
}
fn numeric(mut field: Field, mode: &str) -> Field {
    field.attrs.extra.push(("inputmode".into(), mode.into()));
    field.autocomplete("off")
}
fn quantity(value: &str) -> &str {
    if value.contains('.') {
        let v = value.trim_end_matches('0').trim_end_matches('.');
        if v == "-0" {
            "0"
        } else {
            v
        }
    } else {
        value
    }
}
pub(super) fn table(
    id: &str,
    event: &str,
    caption: &str,
    columns: &ListColumns,
    rows: Vec<FlexRow>,
    actions: bool,
    empty: (&str, &str),
) -> Result<Html> {
    let none = rows.is_empty();
    let mut table = FlexTable::new(id, columns.flex_columns(), rows);
    table.zoom = columns.zoom;
    table.suggestions = columns.flex_suggestions();
    table.add_query = &columns.add_query;
    table.framed = false;
    table.event = event;
    table.actions = actions;
    table.caption = caption;
    if none {
        table.empty = Some(TableEmpty::State(EmptyState::missing(empty.0, empty.1)));
    }
    s::render(table)
}
