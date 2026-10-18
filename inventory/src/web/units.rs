//! `/factory/units` (Bilimbi `Web.UnitsLive`): company units of measure.
use super::*;
use crate::{NewUnit, Unit};
use bilibi_ui::{BuiltinColumn, Cell, Field};

#[derive(Clone, Default)]
struct Form {
    code: String,
    name: String,
}

pub(super) struct Units {
    notice: Notice,
    edit: Edit,
    form: Form,
    /// The unit whose retirement waits for the dialog.
    confirming: Option<i64>,
    units: Vec<Unit>,
    columns: ListColumns,
}

impl Units {
    pub(super) async fn open(ctx: &Ctx<'_>) -> Result<Self> {
        let mut page = Self {
            notice: Notice::default(),
            edit: Edit::Closed,
            form: Form::default(),
            confirming: None,
            units: Vec::new(),
            columns: ListColumns::mount(
                "units",
                vec![
                    BuiltinColumn::new("code", "Code"),
                    BuiltinColumn::new("name", "Name"),
                    BuiltinColumn::new("state", "State"),
                ],
            )
            .map_err(|_| Error::Internal)?,
        };
        page.load(ctx).await?;
        Ok(page)
    }

    pub(super) async fn load(&mut self, ctx: &Ctx<'_>) -> Result<()> {
        if ctx.can_view().await? {
            let listed = ctx
                .inventory()
                .list_units(ctx.scope, ctx.company, ROWS)
                .await;
            self.units = self.notice.take(listed)?.unwrap_or_default();
        } else {
            self.units.clear();
            self.notice.say("You do not have permission to view units.");
        }
        self.columns.load(&self.units, |unit| unit.id.to_string());
        Ok(())
    }

    pub(super) async fn event(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        match event(value)? {
            "grid" => grid(&mut self.columns, value),
            "clear-flash" => self.notice.flash = None,
            "cancel" => {
                self.edit = Edit::Closed;
                self.notice.error = None;
            }
            "cancel_retire" => self.confirming = None,
            kind @ ("new" | "edit" | "save_unit" | "retire" | "retire_confirmed") => {
                if !ctx.can_manage().await? {
                    self.confirming = None;
                    self.notice.denied("manage units");
                    return Ok(());
                }
                match kind {
                    "new" => {
                        self.edit = Edit::New;
                        self.form = Form::default();
                        self.notice.error = None;
                    }
                    "edit" => self.start_edit(ctx, value).await?,
                    "save_unit" => self.save(ctx, value).await?,
                    "retire" => match record_id(value) {
                        Some(id) => self.confirming = Some(id),
                        None => self.notice.say("Unit not found."),
                    },
                    _ => self.retire(ctx).await?,
                }
            }
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }

    async fn start_edit(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        let Some(id) = record_id(value) else {
            self.notice.say("Unit not found.");
            return Ok(());
        };
        let found = ctx.inventory().get_unit(ctx.scope, ctx.company, id).await;
        if let Some(unit) = self.notice.take(found)? {
            self.form = Form {
                code: unit.code,
                name: unit.name,
            };
            self.edit = Edit::Record(id);
            self.notice.error = None;
        }
        Ok(())
    }

    async fn save(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        let posted = params::form_values(value, "unit");
        let form = Form {
            code: posted.get("code").cloned().unwrap_or_default(),
            name: posted.get("name").cloned().unwrap_or_default(),
        };
        let saved = match self.edit {
            Edit::New => {
                let new = NewUnit {
                    code: form.code.clone(),
                    name: form.name.clone(),
                };
                ctx.inventory()
                    .create_unit(ctx.scope, ctx.company, &new)
                    .await
            }
            Edit::Record(id) => {
                ctx.inventory()
                    .rename_unit(ctx.scope, ctx.company, id, &form.name)
                    .await
            }
            Edit::Closed => Err(crate::Error::UnitNotFound),
        };
        if self.notice.take(saved)?.is_some() {
            self.edit = Edit::Closed;
            self.load(ctx).await?;
            self.notice.saved("Unit saved.");
        } else {
            self.form = form;
        }
        Ok(())
    }

    async fn retire(&mut self, ctx: &Ctx<'_>) -> Result<()> {
        let Some(id) = self.confirming.take() else {
            return Ok(());
        };
        let retired = ctx
            .inventory()
            .retire_unit(ctx.scope, ctx.company, id)
            .await;
        if self.notice.take(retired)?.is_some() {
            self.load(ctx).await?;
            self.notice.saved("Unit retired.");
        }
        Ok(())
    }

    pub(super) async fn render(&self, ctx: &Ctx<'_>) -> Result<String> {
        let manage = ctx.can_manage().await?;
        let mut actions = link("Item settings", "/factory/item-settings")?;
        if manage {
            actions.push(&text(" "));
            actions.push(&primary("new-unit", "New unit", "new")?);
        }
        let mut body = Html::EMPTY;
        if manage && self.edit != Edit::Closed {
            let existing = !self.edit.is_new();
            let mut code = Field::in_form("unit", "code")
                .label("Code")
                .value(&self.form.code);
            if existing {
                code = code.readonly();
            }
            body.push(&editor(
                "unit-editor",
                if existing { "Unit" } else { "New unit" },
                "unit-form",
                "save_unit",
                vec![
                    render(code)?,
                    render(
                        Field::in_form("unit", "name")
                            .label("Name")
                            .value(&self.form.name),
                    )?,
                ],
            )?);
        }
        let rows = self
            .units
            .iter()
            .map(|unit| {
                let mut row = FlexRow::new(unit.id.to_string())
                    .cell("code", Cell::text(&unit.code))
                    .cell("name", Cell::text(&unit.name))
                    .cell(
                        "state",
                        Cell::text(if unit.retired { "Retired" } else { "Active" }),
                    );
                row.dom_id = Some(format!("unit-{}", unit.id));
                if manage && !unit.retired {
                    row.actions = row_actions("unit", unit.id)?;
                }
                Ok(row)
            })
            .collect::<Result<Vec<_>>>()?;
        body.push(&list(
            "units",
            "units-table",
            "Units",
            &self.columns,
            "grid",
            rows,
            EmptyState::missing("No units", "Create a unit to use it with a material."),
            true,
        )?);
        if self.confirming.is_some() {
            body.push(&confirm(
                "unit-retire-confirm",
                "Retire this unit?",
                "Retire",
                "Retiring…",
            )?);
        }
        layout(
            "units-index",
            PageVariant::List,
            Header {
                title: text("Units"),
                subtitle:
                    "Company units used by materials and conversions. Codes stay stable after creation.",
                actions,
                ..Header::default()
            },
            "unit-error",
            &self.notice,
            body,
        )
    }
}
