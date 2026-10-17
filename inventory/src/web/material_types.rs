//! `/factory/material-types` (Bilimbi `Web.MaterialTypesLive`): the company's
//! material types and their property definitions, edited as JSON.
use super::*;
use crate::{MaterialType, MaterialTypeUpdate, NewMaterialType, PropertyDefinition};
use bilibi_ui::{BuiltinColumn, Cell, Field, InputKind};

#[derive(Clone)]
struct Form {
    code: String,
    name: String,
    property_definitions: String,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            code: String::new(),
            name: String::new(),
            property_definitions: "[]".into(),
        }
    }
}

pub(super) struct MaterialTypes {
    notice: Notice,
    edit: Edit,
    form: Form,
    confirming: Option<i64>,
    types: Vec<MaterialType>,
    columns: ListColumns,
}

impl MaterialTypes {
    pub(super) async fn open(ctx: &Ctx<'_>) -> Result<Self> {
        let mut page = Self {
            notice: Notice::default(),
            edit: Edit::Closed,
            form: Form::default(),
            confirming: None,
            types: Vec::new(),
            columns: ListColumns::mount(
                "material-types",
                vec![
                    BuiltinColumn::new("code", "Code"),
                    BuiltinColumn::new("name", "Name"),
                    BuiltinColumn::new("properties", "Properties"),
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
                .list_material_types(ctx.scope, ctx.company, ROWS)
                .await;
            self.types = self.notice.take(listed)?.unwrap_or_default();
        } else {
            self.types.clear();
            self.notice
                .say("You do not have permission to view material types.");
        }
        self.columns.load(&self.types, |kind| kind.id.to_string());
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
            kind @ ("new" | "edit" | "save_type" | "retire" | "retire_confirmed") => {
                if !ctx.can_manage().await? {
                    self.confirming = None;
                    self.notice.denied("manage material types");
                    return Ok(());
                }
                match kind {
                    "new" => {
                        self.edit = Edit::New;
                        self.form = Form::default();
                        self.notice.error = None;
                    }
                    "edit" => self.start_edit(ctx, value).await?,
                    "save_type" => self.save(ctx, value).await?,
                    "retire" => match record_id(value) {
                        Some(id) => self.confirming = Some(id),
                        None => self.notice.say("Material type not found."),
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
            self.notice.say("Material type not found.");
            return Ok(());
        };
        let found = ctx
            .inventory()
            .get_material_type(ctx.scope, ctx.company, id)
            .await;
        if let Some(kind) = self.notice.take(found)? {
            self.form = Form {
                code: kind.code,
                name: kind.name,
                property_definitions: serde_json::to_string_pretty(&kind.property_definitions)
                    .map_err(|_| Error::Internal)?,
            };
            self.edit = Edit::Record(id);
            self.notice.error = None;
        }
        Ok(())
    }

    async fn save(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        let posted = params::form_values(value, "type");
        let form = Form {
            code: posted.get("code").cloned().unwrap_or_default(),
            name: posted.get("name").cloned().unwrap_or_default(),
            property_definitions: posted
                .get("property_definitions")
                .cloned()
                .unwrap_or_default(),
        };
        let saved =
            match serde_json::from_str::<Vec<PropertyDefinition>>(&form.property_definitions) {
                Err(_) => Err(crate::Error::InvalidPropertyDefinitions),
                Ok(definitions) => match self.edit {
                    Edit::New => {
                        let new = NewMaterialType {
                            code: form.code.clone(),
                            name: form.name.clone(),
                            property_definitions: definitions,
                        };
                        ctx.inventory()
                            .create_material_type(ctx.scope, ctx.company, &new)
                            .await
                    }
                    Edit::Record(id) => {
                        let update = MaterialTypeUpdate {
                            code: Some(form.code.clone()),
                            name: Some(form.name.clone()),
                            property_definitions: Some(definitions),
                        };
                        ctx.inventory()
                            .update_material_type(ctx.scope, ctx.company, id, &update)
                            .await
                    }
                    Edit::Closed => Err(crate::Error::MaterialTypeNotFound),
                },
            };
        if self.notice.take(saved)?.is_some() {
            self.edit = Edit::Closed;
            self.load(ctx).await?;
            self.notice.saved("Material type saved.");
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
            .retire_material_type(ctx.scope, ctx.company, id)
            .await;
        if self.notice.take(retired)?.is_some() {
            self.load(ctx).await?;
            self.notice.saved("Material type retired.");
        }
        Ok(())
    }

    pub(super) async fn render(&self, ctx: &Ctx<'_>) -> Result<String> {
        let manage = ctx.can_manage().await?;
        let mut actions = Html::EMPTY;
        if manage {
            actions.push(&primary("new-material-type", "New type", "new")?);
        }
        let mut body = Html::EMPTY;
        if manage && self.edit != Edit::Closed {
            body.push(&editor(
                "material-type-editor",
                if self.edit.is_new() {
                    "New material type"
                } else {
                    "Material type"
                },
                "material-type-form",
                "save_type",
                vec![
                    render(
                        Field::in_form("type", "code")
                            .label("Code")
                            .value(&self.form.code),
                    )?,
                    render(
                        Field::in_form("type", "name")
                            .label("Name")
                            .value(&self.form.name),
                    )?,
                    render(
                        Field::in_form("type", "property_definitions")
                            .kind(InputKind::Textarea)
                            .label("Property definitions (JSON)")
                            .hint("Each definition has key, label, value_type, required, and optional unit.")
                            .value(&self.form.property_definitions),
                    )?,
                ],
            )?);
        }
        let rows = self
            .types
            .iter()
            .map(|kind| {
                let mut row = FlexRow::new(kind.id.to_string())
                    .cell("code", Cell::text(&kind.code))
                    .cell("name", Cell::text(&kind.name))
                    .cell(
                        "properties",
                        Cell::text(kind.property_definitions.len().to_string()),
                    )
                    .cell(
                        "state",
                        Cell::text(if kind.retired { "Retired" } else { "Active" }),
                    );
                row.dom_id = Some(format!("material-type-{}", kind.id));
                if manage && !kind.retired {
                    row.actions = row_actions("material-type", kind.id)?;
                }
                Ok(row)
            })
            .collect::<Result<Vec<_>>>()?;
        body.push(&list(
            "material-types",
            "material-types-table",
            "Material types",
            &self.columns,
            "grid",
            rows,
            EmptyState::missing(
                "No material types",
                "Create a type to define its properties.",
            ),
            true,
        )?);
        if self.confirming.is_some() {
            body.push(&confirm(
                "material-type-retire-confirm",
                "Retire this material type?",
                "Retire",
                "Retiring…",
            )?);
        }
        layout(
            "material-types-index",
            PageVariant::List,
            Header {
                title: text("Material types"),
                subtitle: "Define the properties that a company's materials record.",
                actions,
                ..Header::default()
            },
            "material-type-error",
            &self.notice,
            body,
        )
    }
}
