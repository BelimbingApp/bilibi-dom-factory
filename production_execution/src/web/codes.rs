//! Bilimbi CodeList and MeasurementTypesLive share this private page adapter.
use super::{shared as s, Pages};
use crate::contributions as caps;
use bilibi_database::{Error, Result};
use bilibi_tenancy::Scope;
use bilibi_ui::{
    Alert, BuiltinColumn, Button, ButtonVariant, Card, Cell, Choice, EmptyState, Field, FlexRow,
    FlexTable, Html, ListColumns, StatusKind, TableEmpty, NO_FLASH,
};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Wastage,
    Labour,
    Measurement,
}
impl Kind {
    pub(super) fn for_operation(operation: &str) -> Result<Self> {
        match operation {
            "wastage-reasons" => Ok(Self::Wastage),
            "labour-roles" => Ok(Self::Labour),
            "measurement-types" => Ok(Self::Measurement),
            _ => Err(Error::NotFound),
        }
    }
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Wastage => "Wastage reasons",
            Self::Labour => "Labour roles",
            Self::Measurement => "Measurement types",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Wastage => "wastage-reasons",
            Self::Labour => "labour-roles",
            Self::Measurement => "measurement-types",
        }
    }
    fn editor_id(self) -> &'static str {
        if self.measurement() {
            "measurement-type"
        } else {
            self.id()
        }
    }
    fn noun(self) -> &'static str {
        match self {
            Self::Wastage => "Wastage reason",
            Self::Labour => "Labour role",
            Self::Measurement => "Measurement type",
        }
    }
    fn plural(self) -> &'static str {
        match self {
            Self::Wastage => "wastage reasons",
            Self::Labour => "labour roles",
            Self::Measurement => "measurement types",
        }
    }
    fn subtitle(self) -> &'static str {
        match self { Self::Wastage => "Why material is scrapped on a run, as operators choose it on the shop floor.", Self::Labour => "The roles people work in on a run, as they clock in on the shop floor.", Self::Measurement => "What operators measure on a run's output, with its unit and the limits a value is flagged outside." }
    }
    fn empty(self) -> &'static str {
        match self {
            Self::Wastage => "Add the reasons operators may give for scrapped material.",
            Self::Labour => "Add the roles people may clock in as.",
            Self::Measurement => "Add what operators measure on a run's output.",
        }
    }
    fn measurement(self) -> bool {
        matches!(self, Self::Measurement)
    }
    fn form(self) -> &'static str {
        if self.measurement() {
            "type"
        } else {
            "entry"
        }
    }
}
pub(super) struct Codes {
    pages: Pages,
    kind: Kind,
    rows: Vec<Value>,
    columns: ListColumns,
    // None = reading, Some(None) = create, Some(Some(id)) = change label/limits.
    editing: Option<Option<i64>>,
    draft: HashMap<String, String>,
    manage: bool,
    error: Option<String>,
    success: Option<String>,
}
impl Codes {
    pub(super) fn new(pages: &Pages, operation: &str) -> Result<Self> {
        let kind = Kind::for_operation(operation)?;
        let mut cols = vec![("code", "Code"), ("label", "Label")];
        if kind.measurement() {
            cols.extend([
                ("value", "Value"),
                ("minimum", "Minimum"),
                ("target", "Target"),
                ("maximum", "Maximum"),
            ]);
        }
        cols.push(("state", "State"));
        Ok(Self {
            pages: pages.clone(),
            kind,
            rows: vec![],
            columns: ListColumns::mount(
                &format!("{}-table", kind.id()),
                cols.into_iter()
                    .map(|(id, label)| BuiltinColumn::new(id, label))
                    .collect(),
            )
            .map_err(|_| Error::Internal)?,
            editing: None,
            draft: HashMap::new(),
            manage: false,
            error: None,
            success: None,
        })
    }
    pub(super) async fn load(&mut self, scope: &Scope) -> Result<()> {
        self.manage = s::can(&self.pages.db, scope, caps::MANAGE).await?;
        let company = s::company(scope)?;
        let pe = &self.pages.execution;
        let result = match self.kind {
            Kind::Wastage => pe.list_wastage_reasons(scope, company, None).await,
            Kind::Labour => pe.list_labour_roles(scope, company, None).await,
            Kind::Measurement => pe.list_measurement_types(scope, company, None).await,
        };
        match result {
            Ok(rows) => self.rows = rows,
            Err(e) => {
                self.rows.clear();
                self.error = Some(e.to_string());
            }
        }
        self.columns.load(&self.rows, |v| v["id"].to_string());
        Ok(())
    }
    pub(super) async fn event(&mut self, scope: &Scope, p: &Value) -> Result<()> {
        self.success = None;
        let event = s::text(p, "type");
        if event == "clear-flash" {
            return Ok(());
        }
        if event == "grid" {
            let _ = self.columns.handle(&bilibi_ui::params::string_values(p));
            return Ok(());
        }
        if event == "cancel" {
            self.editing = None;
            self.error = None;
            return Ok(());
        }
        if !s::can(&self.pages.db, scope, caps::MANAGE).await? {
            self.error = Some(format!(
                "You do not have permission to manage {}.",
                self.kind.plural()
            ));
            return self.load(scope).await;
        }
        self.error = None;
        match event {
            "new" => {
                self.editing = Some(None);
                self.draft = HashMap::from([("value_type".into(), "decimal".into())]);
            }
            "edit" => {
                if let Some(row) = self.rows.iter().find(|v| s::id(v, "id") == s::id(p, "id")) {
                    self.editing = Some(s::id(row, "id"));
                    self.draft = [
                        "code",
                        "label",
                        "value_type",
                        "unit",
                        "minimum",
                        "maximum",
                        "target",
                    ]
                    .into_iter()
                    .map(|k| (k.into(), s::text(row, k).into()))
                    .collect();
                } else {
                    self.error = Some(format!("{} not found.", self.kind.noun()));
                }
            }
            "save_entry" => {
                self.draft.extend(s::form(p, self.kind.form()));
                let mut attrs = json!({"label":s::get(&self.draft,"label")});
                if self.kind.measurement() {
                    for key in ["minimum", "maximum", "target"] {
                        attrs[key] = json!(s::get(&self.draft, key));
                    }
                }
                if self.editing == Some(None) {
                    attrs["code"] = json!(s::get(&self.draft, "code"));
                    if self.kind.measurement() {
                        for key in ["value_type", "unit"] {
                            attrs[key] = json!(s::get(&self.draft, key));
                        }
                    }
                }
                let result = self.write(scope, self.editing, &attrs).await;
                match result {
                    Ok(_) => {
                        self.editing = None;
                        self.success = Some(format!("{} saved.", self.kind.noun()));
                        self.load(scope).await?;
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            "toggle_active" => {
                if let Some(row) = self.rows.iter().find(|v| s::id(v, "id") == s::id(p, "id")) {
                    let attrs = json!({"active":row["active"]!=true});
                    match self.write(scope, Some(s::id(row, "id")), &attrs).await {
                        Ok(_) => self.load(scope).await?,
                        Err(e) => self.error = Some(e.to_string()),
                    }
                } else {
                    self.error = Some(format!("{} not found.", self.kind.noun()));
                }
            }
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }
    async fn write(
        &self,
        scope: &Scope,
        editing: Option<Option<i64>>,
        attrs: &Value,
    ) -> crate::Result<Value> {
        let company = s::company(scope)?;
        let pe = &self.pages.execution;
        match (self.kind, editing) {
            (Kind::Wastage, Some(None)) => pe.create_wastage_reason(scope, company, attrs).await,
            (Kind::Wastage, Some(Some(id))) => {
                pe.update_wastage_reason(scope, company, id, attrs).await
            }
            (Kind::Labour, Some(None)) => pe.create_labour_role(scope, company, attrs).await,
            (Kind::Labour, Some(Some(id))) => {
                pe.update_labour_role(scope, company, id, attrs).await
            }
            (Kind::Measurement, Some(None)) => {
                pe.create_measurement_type(scope, company, attrs).await
            }
            (Kind::Measurement, Some(Some(id))) => {
                pe.update_measurement_type(scope, company, id, attrs).await
            }
            _ => Err(crate::Error::Capture("Entry not found")),
        }
    }
    fn editor(&self) -> Result<Html> {
        let name = self.kind.form();
        let d = &self.draft;
        let existing = self.editing.flatten().is_some();
        let mut fields = vec![
            s::field(name, "code", "Code", d),
            s::field(name, "label", "Label", d),
        ];
        let hint = if self.kind.measurement() {
            fields.push(s::select(
                name,
                "value_type",
                "Value type",
                "",
                d,
                bilibi_factory_inventory::VALUE_TYPES
                    .iter()
                    .map(|v| Choice::new(*v, *v))
                    .collect(),
            ));
            fields.push(s::field(name, "unit", "Unit (numeric types)", d));
            for (key, label) in [
                ("minimum", "Minimum"),
                ("maximum", "Maximum"),
                ("target", "Target"),
            ] {
                let mut f = s::field(name, key, label, d);
                f.attrs.extra.push(("inputmode".into(), "decimal".into()));
                fields.push(f);
            }
            "Code, value type, and unit are fixed once created. Changed limits apply to new measurements only."
        } else {
            fields[0].hint = "Fixed once created, so recorded history keeps its meaning.".into();
            ""
        };
        if existing {
            for field in &mut fields {
                if matches!(
                    field.name.as_str(),
                    "entry[code]" | "type[code]" | "type[value_type]" | "type[unit]"
                ) {
                    field.attrs.disabled = true;
                }
            }
        }
        let id = format!("{}-form", self.kind.editor_id());
        let body = s::render(Editor {
            id: &id,
            fields,
            hint,
        })?;
        let title = if existing {
            self.kind.noun().into()
        } else {
            format!("New {}", self.kind.noun().to_lowercase())
        };
        s::render(Card {
            id: &format!("{}-editor", self.kind.editor_id()),
            title: &title,
            body,
            class: "mt-5",
            ..Card::default()
        })
    }
    pub(super) fn view(&self) -> Result<String> {
        let mut rows = vec![];
        for v in &self.rows {
            let id = v["id"].to_string();
            let mut row = FlexRow::new(&id)
                .cell("code", Cell::text(s::text(v, "code")))
                .cell("label", Cell::text(s::text(v, "label")))
                .cell(
                    "state",
                    Cell::text(if v["active"] == true {
                        "Active"
                    } else {
                        "Inactive"
                    }),
                );
            row.dom_id = Some(format!("{}-{id}", self.kind.id()));
            if self.kind.measurement() {
                row = row.cell(
                    "value",
                    Cell::text(format!(
                        "{}{}",
                        s::text(v, "value_type"),
                        v["unit"]
                            .as_str()
                            .map(|u| format!(" ({u})"))
                            .unwrap_or_default()
                    )),
                );
                for key in ["minimum", "target", "maximum"] {
                    row = row.cell(key, Cell::text(s::text(v, key)));
                }
            }
            if self.manage {
                row.actions = action("Edit", "edit", &id, "")?;
                row.actions.push(&action(
                    if v["active"] == true {
                        "Deactivate"
                    } else {
                        "Activate"
                    },
                    "toggle_active",
                    &id,
                    "",
                )?);
            }
            rows.push(row);
        }
        let table_id = format!("{}-table", self.kind.id());
        let mut table = FlexTable::new(&table_id, self.columns.flex_columns(), rows);
        table.zoom = self.columns.zoom;
        table.suggestions = self.columns.flex_suggestions();
        table.add_query = &self.columns.add_query;
        table.framed = false;
        table.event = "grid";
        table.actions = self.manage;
        table.caption = self.kind.title();
        if self.rows.is_empty() {
            table.empty = Some(TableEmpty::State(EmptyState::missing(
                format!("No {}", self.kind.plural()),
                self.kind.empty(),
            )));
        }
        let list = s::render(Card {
            id: self.kind.id(),
            body: s::render(table)?,
            inner_class: "p-0",
            class: "mt-5",
            ..Card::default()
        })?;
        let error = match &self.error {
            Some(e) => s::render(Alert {
                id: &format!("{}-error", self.kind.editor_id()),
                kind: StatusKind::Error,
                body: Html::text(e),
                class: "mt-4",
            })?,
            None => Html::EMPTY,
        };
        let editor = if self.editing.is_some() && self.manage {
            self.editor()?
        } else {
            Html::EMPTY
        };
        let actions = if self.manage {
            action(
                if self.kind.measurement() {
                    "New type"
                } else {
                    "New"
                },
                "new",
                "",
                &format!("new-{}", self.kind.editor_id()),
            )?
        } else {
            Html::EMPTY
        };
        let withheld = if !self.manage {
            s::render(EmptyState::missing(
                "Management unavailable",
                format!(
                    "You do not have permission to manage {}.",
                    self.kind.plural()
                ),
            ))?
        } else {
            Html::EMPTY
        };
        let mut flash = NO_FLASH;
        if let Some(message) = &self.success {
            flash.success = message;
        }
        Ok(s::render(CodeView {
            title: self.kind.title(),
            subtitle: self.kind.subtitle(),
            actions,
            error,
            editor,
            list,
            withheld,
            flash,
        })?
        .as_str()
        .into())
    }
}
fn action(label: &str, event: &str, id: &str, dom_id: &str) -> Result<Html> {
    s::render(Button {
        id: dom_id,
        button_type: "button",
        event,
        name: "id",
        value: id,
        body: Html::text(label),
        variant: if event == "new" {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Secondary
        },
        ..Button::default()
    })
}
#[derive(askama::Template)]
#[template(path = "code_list.html")]
struct CodeView<'a> {
    title: &'a str,
    subtitle: &'a str,
    actions: Html,
    error: Html,
    editor: Html,
    list: Html,
    withheld: Html,
    flash: bilibi_ui::FlashMap<'a>,
}
#[derive(askama::Template)]
#[template(path = "code_editor.html")]
struct Editor<'a> {
    id: &'a str,
    fields: Vec<Field>,
    hint: &'a str,
}
