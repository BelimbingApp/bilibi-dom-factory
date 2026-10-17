//! Order/run labour panel from Bilimbi FloorLive. All writes use its facade.
use super::{floor::Floor, shared as s, time, view};
use crate::contributions as caps;
use bilibi_database::{Error, Result};
use bilibi_tenancy::Scope;
use bilibi_ui::{
    BuiltinColumn, Cell, Choice, EmptyState, Field, FlexRow, Html, InputKind, ListColumns,
    StatItem, StatStrip,
};
use serde_json::{json, Value};
use std::collections::HashMap;

pub(super) struct Labour {
    roles: Vec<Value>,
    rows: Vec<Value>,
    summary: Value,
    users: Vec<bilibi_user::company_accounts::CompanyAccount>,
    own: i64,
    clock: HashMap<String, String>,
    entry: HashMap<String, String>,
    correcting: Option<Value>,
    correction: HashMap<String, String>,
    columns: ListColumns,
}
impl Labour {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            roles: vec![],
            rows: vec![],
            summary: Value::Null,
            users: vec![],
            own: 0,
            clock: s::draft(&[("role_id", ""), ("worker_user_id", "")]),
            entry: s::draft(&[
                ("role_id", ""),
                ("worker_user_id", ""),
                ("started_at", ""),
                ("stopped_at", ""),
                ("note", ""),
            ]),
            correcting: None,
            correction: HashMap::new(),
            columns: ListColumns::mount(
                "labour-table",
                [
                    ("worker", "Worker"),
                    ("role", "Role"),
                    ("run", "Run"),
                    ("start", "Start"),
                    ("stop", "Stop"),
                    ("time", "Time"),
                    ("corrected", "Corrected"),
                ]
                .into_iter()
                .map(|(id, label)| BuiltinColumn::new(id, label))
                .collect(),
            )
            .map_err(|_| Error::Internal)?,
        })
    }
    fn worker(&self, id: i64) -> String {
        if id == self.own {
            "You".into()
        } else {
            self.users
                .iter()
                .find(|u| u.id == id)
                .map(|u| u.name.clone())
                .unwrap_or_else(|| format!("User {id}"))
        }
    }
    fn roles(&self, previous: Option<i64>) -> Vec<Choice> {
        self.roles
            .iter()
            .filter(|r| r["active"] == true || s::id(r, "id") == previous)
            .map(|r| Choice::new(s::text(r, "label"), r["id"].to_string()))
            .collect()
    }
    fn workers(&self, others: bool) -> Vec<Choice> {
        self.users
            .iter()
            .filter(|u| !others || u.id != self.own)
            .map(|u| Choice::new(&u.name, u.id.to_string()))
            .collect()
    }
}
impl Floor {
    pub(super) async fn load_labour(&mut self, scope: &Scope, company: i64) -> Result<()> {
        self.labour.rows.clear();
        self.labour.roles.clear();
        self.labour.users.clear();
        self.labour.summary = Value::Null;
        self.labour.own = scope.user();
        let Some(order) = self.order.as_ref() else {
            return Ok(());
        };
        let pe = &self.pages.execution;
        let result = async {
            let rows = pe.list_labour(scope, company, order.id).await?;
            let summary = pe.labour_summary(scope, company, order.id).await?;
            let roles = pe.list_labour_roles(scope, company, None).await?;
            Ok::<_, crate::Error>((rows, summary, roles))
        }
        .await;
        match result {
            Ok((rows, summary, roles)) => {
                self.labour.rows = rows;
                self.labour.summary = summary;
                self.labour.roles = roles;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                return Ok(());
            }
        }
        if self.allowed.contains(caps::LABOUR_MANAGE) {
            let c = self.pages.db.get().await?;
            match bilibi_user::company_accounts::list(&**c, scope, company).await {
                Ok(users) => self.labour.users = users,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.labour
            .columns
            .load(&self.labour.rows, |v| v["id"].to_string());
        Ok(())
    }
    pub(super) async fn labour_event(&mut self, scope: &Scope, p: &Value) -> Result<bool> {
        let kind = s::text(p, "type");
        if kind == "labour_grid" {
            let _ = self
                .labour
                .columns
                .handle(&bilibi_ui::params::string_values(p));
            return Ok(true);
        }
        if kind == "cancel_labour_correction" {
            self.labour.correcting = None;
            return Ok(true);
        }
        let manage = match kind {
            "pick_role" | "change_labour" | "clock_in" | "clock_out" => false,
            "save_labour_entry" | "start_labour_correction" | "save_labour_correction" => true,
            _ => return Ok(false),
        };
        let allowed = if manage {
            s::can(&self.pages.db, scope, caps::LABOUR_MANAGE).await?
        } else {
            s::can(&self.pages.db, scope, caps::LABOUR_RECORD).await?
                || s::can(&self.pages.db, scope, caps::LABOUR_MANAGE).await?
        };
        if !allowed {
            self.error = Some(
                if manage {
                    "You do not have permission to manage labour."
                } else {
                    "You do not have permission to record labour."
                }
                .into(),
            );
            self.load(scope).await?;
            return Ok(true);
        }
        self.error = None;
        if kind == "pick_role" {
            self.labour
                .clock
                .insert("role_id".into(), s::text(p, "id").into());
            return Ok(true);
        }
        if kind == "change_labour" {
            self.labour.clock.extend(s::form(p, "labour"));
            return Ok(true);
        }
        if kind == "start_labour_correction" {
            self.labour.correcting = self
                .labour
                .rows
                .iter()
                .find(|v| s::id(v, "id") == s::id(p, "id") && v["corrected_by_id"].is_null())
                .cloned();
            if let Some(row) = &self.labour.correcting {
                self.labour.correction = s::draft(&[("correction_reason", "")]);
                self.labour
                    .correction
                    .insert("role_id".into(), row["role_id"].to_string());
                for key in ["started_at", "stopped_at"] {
                    self.labour.correction.insert(
                        key.into(),
                        s::text(row, key)
                            .parse()
                            .ok()
                            .and_then(|t| time::input(&self.display, t))
                            .unwrap_or_default(),
                    );
                }
            } else {
                self.error = Some("Labour entry not found.".into());
            }
            return Ok(true);
        }
        let company = s::company(scope)?;
        let Some(order) = self.order.as_ref().map(|o| o.id) else {
            self.error = Some("Order not found".into());
            return Ok(true);
        };
        let pe = &self.pages.execution;
        let (result, message) = match kind {
            "clock_in" => {
                self.labour.clock.extend(s::form(p, "labour"));
                let d = &self.labour.clock;
                let Some(role) = bilibi_ui::params::positive_id(Some(s::get(d, "role_id"))) else {
                    self.error = Some("Choose a labour role.".into());
                    return Ok(true);
                };
                let attrs = json!({"request_id":s::get(d,"request_id"),"role_id":role,"worker_user_id":bilibi_ui::params::positive_id(Some(s::get(d,"worker_user_id"))).unwrap_or(scope.user()),"execution_id":self.run.as_ref().map(|r|r.id)});
                (
                    pe.clock_in(scope, company, order, &attrs).await,
                    "Clocked in.",
                )
            }
            "clock_out" => {
                let Some(id) = s::id(p, "id")
                    .filter(|id| self.labour.rows.iter().any(|r| s::id(r, "id") == Some(*id)))
                else {
                    self.error = Some("Labour entry not found.".into());
                    return Ok(true);
                };
                (pe.clock_out(scope, company, id).await, "Clocked out.")
            }
            "save_labour_entry" => {
                self.labour.entry.extend(s::form(p, "entry"));
                let d = &self.labour.entry;
                let (Some(start), Some(stop)) = (
                    time::parse(&self.display, s::get(d, "started_at")),
                    time::parse(&self.display, s::get(d, "stopped_at")),
                ) else {
                    self.error = Some("Enter a start and stop time.".into());
                    return Ok(true);
                };
                let attrs = json!({"request_id":s::get(d,"request_id"),"worker_user_id":bilibi_ui::params::positive_id(Some(s::get(d,"worker_user_id"))),"role_id":bilibi_ui::params::positive_id(Some(s::get(d,"role_id"))),"execution_id":self.run.as_ref().map(|r|r.id),"started_at":start.to_string(),"stopped_at":stop.to_string(),"note":s::get(d,"note")});
                (
                    pe.record_labour(scope, company, order, &attrs).await,
                    "Labour recorded.",
                )
            }
            _ => {
                self.labour
                    .correction
                    .extend(s::form(p, "labour_correction"));
                let d = &self.labour.correction;
                let Some(row) = &self.labour.correcting else {
                    self.error = Some("Labour entry not found.".into());
                    return Ok(true);
                };
                let mut attrs = json!({"request_id":s::get(d,"request_id"),"role_id":bilibi_ui::params::positive_id(Some(s::get(d,"role_id"))),"correction_reason":s::get(d,"correction_reason")});
                for key in ["started_at", "stopped_at"] {
                    let original = s::text(row, key).parse::<jiff::Timestamp>().ok();
                    let Some(value) = time::corrected(&self.display, s::get(d, key), original)
                    else {
                        self.error = Some("Enter a valid start and stop time.".into());
                        return Ok(true);
                    };
                    if key == "started_at" && value.is_none() {
                        self.error = Some("Enter a valid start and stop time.".into());
                        return Ok(true);
                    }
                    // Omit unchanged instants: facade retains original database precision.
                    if value != original {
                        attrs[key] = value.map(|t| json!(t.to_string())).unwrap_or(Value::Null);
                    }
                }
                (
                    pe.correct_labour(
                        scope,
                        company,
                        s::id(row, "id").ok_or(Error::Invalid)?,
                        &attrs,
                    )
                    .await,
                    "Labour corrected.",
                )
            }
        };
        match result {
            Ok(_) => {
                self.success = Some(message.into());
                // Reset only the submitted draft. Other in-progress forms retain their values.
                match kind {
                    "clock_in" => self.labour.clock = Labour::new()?.clock,
                    "save_labour_entry" => self.labour.entry = Labour::new()?.entry,
                    "save_labour_correction" => self.labour.correcting = None,
                    _ => {}
                }
                self.load(scope).await?;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        Ok(true)
    }
    fn run_label(&self, id: Option<i64>) -> String {
        match id {
            None => "Order".into(),
            Some(id) => self
                .runs
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.operation_code.clone())
                .unwrap_or_else(|| format!("Run {id}")),
        }
    }
    fn labour_time(
        &self,
        name: &str,
        key: &str,
        label: String,
        d: &HashMap<String, String>,
    ) -> Field {
        s::field(name, key, &label, d).kind(InputKind::DatetimeLocal)
    }
    pub(super) fn labour_panel(&self) -> Result<Html> {
        let l = &self.labour;
        let manage = self.allowed.contains(caps::LABOUR_MANAGE);
        let record = self.allowed.contains(caps::LABOUR_RECORD);
        let mut body = Html::EMPTY;
        if record || manage {
            let mut fields = vec![
                view::hidden("labour", "request_id", &l.clock),
                view::hidden("labour", "role_id", &l.clock),
            ];
            if manage {
                fields.push(s::select(
                    "labour",
                    "worker_user_id",
                    "Worker",
                    "Me",
                    &l.clock,
                    l.workers(true),
                ));
            }
            let choices = self.choices(
                &l.roles,
                "Role",
                "pick_role",
                s::get(&l.clock, "role_id"),
                "No active labour roles. An administrator defines them under Labour roles.",
            )?;
            let form = s::render(s::FormView {
                id: "labour-form",
                event: "clock_in",
                change: "change_labour",
                fields,
                choices,
                after: vec![],
                submit: "Clock in",
                cancel: "",
            })?;
            let title = if self.run.is_some() {
                "Clock in on this run".into()
            } else {
                format!(
                    "Clock in on {}",
                    self.order.as_ref().map(|o| o.code.as_str()).unwrap_or("")
                )
            };
            body.push(&self.card("floor-clock", &title, form)?);
        } else {
            body.push(&s::render(EmptyState::missing(
                "Clock in unavailable",
                "You do not have permission to record labour.",
            ))?);
        }
        let mut items = vec![StatItem::number(
            "Order total",
            duration(l.summary["total_seconds"].as_i64().or(Some(0))),
        )];
        if let Some(runs) = l.summary["runs"].as_array() {
            for run in runs {
                items.push(StatItem::number(
                    self.run_label(run["execution_id"].as_i64()),
                    duration(run["seconds"].as_i64()),
                ));
            }
        }
        let mut content = s::render(StatStrip {
            id: "labour-total",
            title: "",
            href: "",
            items,
        })?;
        let mut rows = vec![];
        for v in l.rows.iter().filter(|v| v["corrected_by_id"].is_null()) {
            let id = v["id"].to_string();
            let role = l
                .roles
                .iter()
                .find(|r| r["id"] == v["role_id"])
                .map(|r| s::text(r, "label"))
                .unwrap_or("—");
            let mut row = FlexRow::new(&id)
                .cell(
                    "worker",
                    Cell::text(l.worker(s::id(v, "worker_user_id").ok_or(Error::Invalid)?)),
                )
                .cell("role", Cell::text(role))
                .cell(
                    "run",
                    Cell::text(self.run_label(v["execution_id"].as_i64())),
                )
                .cell("start", self.time_cell(v, "started_at")?)
                .cell("time", Cell::text(duration(v["seconds"].as_i64())))
                .cell("corrected", Cell::text(s::text(v, "correction_reason")));
            if !v["stopped_at"].is_null() {
                row = row.cell("stop", self.time_cell(v, "stopped_at")?);
            }
            row.dom_id = Some(format!("labour-{id}"));
            if v["stopped_at"].is_null() && (manage || (record && v["worker_user_id"] == l.own)) {
                row.actions
                    .push(&s::button("Clock out", "clock_out", &id, true)?);
            }
            if manage {
                row.actions.push(&s::button(
                    "Correct",
                    "start_labour_correction",
                    &id,
                    false,
                )?);
            }
            rows.push(row);
        }
        content.push(&view::table(
            "labour-table",
            "labour_grid",
            "Current labour",
            &l.columns,
            rows,
            manage || record,
            ("No labour", "Nobody has clocked in on this order yet."),
        )?);
        body.push(&self.card("floor-labour", "Labour", content)?);
        let zone = time::zone_name(&self.display);
        if manage {
            if let Some(row) = &l.correcting {
                let d = &l.correction;
                let form = s::render(s::FormView {
                    id: "labour-correction-form",
                    event: "save_labour_correction",
                    change: "",
                    fields: vec![
                        view::hidden("labour_correction", "request_id", d),
                        s::select(
                            "labour_correction",
                            "role_id",
                            "Role",
                            "",
                            d,
                            l.roles(s::id(row, "role_id")),
                        ),
                        self.labour_time(
                            "labour_correction",
                            "started_at",
                            format!("Start ({zone})"),
                            d,
                        ),
                        self.labour_time(
                            "labour_correction",
                            "stopped_at",
                            format!("Stop ({zone}); empty keeps it open"),
                            d,
                        ),
                        s::field(
                            "labour_correction",
                            "correction_reason",
                            "Why is this corrected?",
                            d,
                        ),
                    ],
                    choices: Html::EMPTY,
                    after: vec![],
                    submit: "Save correction",
                    cancel: "cancel_labour_correction",
                })?;
                body.push(&self.card("labour-correction", "Correct labour", form)?);
            }
            let d = &l.entry;
            let form = s::render(s::FormView {
                id: "labour-entry-form",
                event: "save_labour_entry",
                change: "",
                fields: vec![
                    view::hidden("entry", "request_id", d),
                    s::select(
                        "entry",
                        "worker_user_id",
                        "Worker",
                        "Choose a worker",
                        d,
                        l.workers(false),
                    ),
                    s::select(
                        "entry",
                        "role_id",
                        "Role",
                        "Choose a role",
                        d,
                        l.roles(None),
                    ),
                    self.labour_time("entry", "started_at", format!("Start ({zone})"), d),
                    self.labour_time("entry", "stopped_at", format!("Stop ({zone})"), d),
                    s::field("entry", "note", "Note (optional)", d),
                ],
                choices: Html::EMPTY,
                after: vec![],
                submit: "Add entry",
                cancel: "",
            })?;
            body.push(&self.card("labour-entry", "Add a finished entry", form)?);
        }
        Ok(body)
    }
}
fn duration(seconds: Option<i64>) -> String {
    match seconds {
        None => "open".into(),
        Some(s) => format!("{}h {:02}m", s / 3600, s / 60 % 60),
    }
}
