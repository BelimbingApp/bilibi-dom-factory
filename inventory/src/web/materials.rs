//! `/factory/materials` (Bilimbi `Web.MaterialsLive`): register an item with a
//! native unit, a material type and typed values, and edit or retire it.
use super::*;
use crate::{ItemSettings, ItemUpdate, Map, Material, MaterialType, MaterialTyping, NewItem, Unit};
use bilibi_ui::{choices, BuiltinColumn, Cell, Field, InputKind};

#[derive(Clone)]
struct Form {
    sku: String,
    title: String,
    status: String,
    currency_code: String,
    description: String,
    native_unit_id: String,
    material_type_id: String,
    properties: String,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            sku: String::new(),
            title: String::new(),
            status: String::new(),
            currency_code: String::new(),
            description: String::new(),
            native_unit_id: String::new(),
            material_type_id: String::new(),
            properties: "{}".into(),
        }
    }
}

pub(super) struct Materials {
    notice: Notice,
    edit: Edit,
    form: Form,
    confirming: Option<i64>,
    materials: Vec<Material>,
    units: Vec<Unit>,
    types: Vec<MaterialType>,
    settings: ItemSettings,
    columns: ListColumns,
}

/// Bilimbi's `decode_properties`: a JSON object, else the facade refuses.
fn properties(json: &str) -> std::result::Result<Map<String, Value>, crate::Error> {
    match serde_json::from_str(json) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(crate::Error::InvalidProperties),
    }
}

fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

impl Materials {
    pub(super) async fn open(ctx: &Ctx<'_>) -> Result<Self> {
        let mut page = Self {
            notice: Notice::default(),
            edit: Edit::Closed,
            form: Form::default(),
            confirming: None,
            materials: Vec::new(),
            units: Vec::new(),
            types: Vec::new(),
            settings: ItemSettings::default(),
            columns: ListColumns::mount(
                "materials",
                vec![
                    BuiltinColumn::new("sku", "SKU"),
                    BuiltinColumn::new("native_unit", "Native unit"),
                    BuiltinColumn::new("type", "Type"),
                    BuiltinColumn::new("state", "State"),
                ],
            )
            .map_err(|_| Error::Internal)?,
        };
        page.load(ctx).await?;
        Ok(page)
    }

    pub(super) async fn load(&mut self, ctx: &Ctx<'_>) -> Result<()> {
        self.materials.clear();
        self.units.clear();
        self.types.clear();
        self.settings = ItemSettings::default();
        if ctx.can_view().await? {
            let inventory = ctx.inventory();
            let loaded = async {
                Ok::<_, crate::Error>((
                    inventory
                        .list_materials(ctx.scope, ctx.company, ROWS)
                        .await?,
                    inventory.list_units(ctx.scope, ctx.company, ROWS).await?,
                    inventory
                        .list_material_types(ctx.scope, ctx.company, ROWS)
                        .await?,
                    inventory.item_settings(ctx.scope, ctx.company).await?,
                ))
            }
            .await;
            if let Some((materials, units, types, settings)) = self.notice.take(loaded)? {
                self.materials = materials;
                self.units = units;
                self.types = types;
                self.settings = settings;
            }
        } else {
            self.notice
                .say("You do not have permission to view materials.");
        }
        self.columns
            .load(&self.materials, |material| material.item_id.to_string());
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
            kind @ ("new" | "edit" | "save_material" | "retire" | "retire_confirmed") => {
                if !ctx.can_manage().await? {
                    self.confirming = None;
                    self.notice.denied("manage materials");
                    return Ok(());
                }
                match kind {
                    "new" => {
                        self.edit = Edit::New;
                        self.form = Form::default();
                        self.notice.error = None;
                    }
                    "edit" => self.start_edit(ctx, value).await?,
                    "save_material" => self.save(ctx, value).await?,
                    "retire" => match record_id(value) {
                        Some(id) => self.confirming = Some(id),
                        None => self.notice.say("Material not found."),
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
            self.notice.say("Material not found.");
            return Ok(());
        };
        let inventory = ctx.inventory();
        let found = async {
            Ok::<_, crate::Error>((
                inventory.get_material(ctx.scope, ctx.company, id).await?,
                inventory.get_item(ctx.scope, ctx.company, id).await?,
            ))
        }
        .await;
        if let Some((material, item)) = self.notice.take(found)? {
            self.form = Form {
                sku: item.sku,
                title: item.title,
                status: item.status,
                currency_code: item.currency_code,
                description: item.description.unwrap_or_default(),
                native_unit_id: material.native_unit.id.to_string(),
                material_type_id: material
                    .material_type_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                properties: serde_json::to_string_pretty(&material.properties)
                    .map_err(|_| Error::Internal)?,
            };
            self.edit = Edit::Record(id);
            self.notice.error = None;
        }
        Ok(())
    }

    async fn save(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        let posted = params::form_values(value, "material");
        let get = |key: &str| posted.get(key).cloned().unwrap_or_default();
        let form = Form {
            sku: get("sku"),
            title: get("title"),
            status: get("status"),
            currency_code: get("currency_code"),
            description: get("description"),
            native_unit_id: get("native_unit_id"),
            material_type_id: get("material_type_id"),
            properties: get("properties"),
        };
        let inventory = ctx.inventory();
        let saved = match self.edit {
            Edit::New => {
                let unit = params::positive_id(Some(&form.native_unit_id));
                let kind = match form.material_type_id.as_str() {
                    "" => Some(None),
                    id => params::positive_id(Some(id)).map(Some),
                };
                match (unit, kind) {
                    (Some(unit), Some(material_type_id)) => match properties(&form.properties) {
                        Ok(properties) => {
                            let item = NewItem {
                                sku: form.sku.clone(),
                                title: form.title.clone(),
                                status: optional(&form.status),
                                description: optional(&form.description),
                                currency_code: optional(&form.currency_code),
                                ..NewItem::default()
                            };
                            inventory
                                .create_material(
                                    ctx.scope,
                                    ctx.company,
                                    &item,
                                    unit,
                                    &MaterialTyping {
                                        material_type_id,
                                        properties,
                                    },
                                )
                                .await
                        }
                        Err(error) => Err(error),
                    },
                    _ => {
                        self.notice.say("Invalid material reference");
                        self.form = form;
                        return Ok(());
                    }
                }
            }
            Edit::Record(id) => match properties(&form.properties) {
                Ok(properties) => {
                    let item = ItemUpdate {
                        title: Some(form.title.clone()),
                        description: Some(form.description.clone()),
                        status: optional(&form.status),
                        notes: None,
                        currency_code: optional(&form.currency_code),
                    };
                    inventory
                        .update_material(ctx.scope, ctx.company, id, &item, &properties)
                        .await
                }
                Err(error) => Err(error),
            },
            Edit::Closed => Err(crate::Error::MaterialNotFound),
        };
        if self.notice.take(saved)?.is_some() {
            self.edit = Edit::Closed;
            self.load(ctx).await?;
            self.notice.saved("Material saved.");
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
            .retire_material(ctx.scope, ctx.company, id)
            .await;
        if self.notice.take(retired)?.is_some() {
            self.load(ctx).await?;
            self.notice.saved("Material retired.");
        }
        Ok(())
    }

    fn form_fields(&self) -> Result<Vec<Html>> {
        let existing = !self.edit.is_new();
        let mut sku = Field::in_form("material", "sku")
            .label("SKU")
            .value(&self.form.sku);
        if existing {
            sku = sku.readonly();
        }
        let units: Vec<(String, String)> = self
            .units
            .iter()
            .filter(|unit| !unit.retired)
            .map(|unit| {
                (
                    format!("{} ({})", unit.name, unit.code),
                    unit.id.to_string(),
                )
            })
            .collect();
        let types: Vec<(String, String)> = self
            .types
            .iter()
            .filter(|kind| !kind.retired)
            .map(|kind| (kind.name.clone(), kind.id.to_string()))
            .collect();
        let mut unit = Field::in_form("material", "native_unit_id")
            .kind(InputKind::Select)
            .label("Native unit")
            .prompt("Choose a unit")
            .options(choices(&units))
            .value(&self.form.native_unit_id);
        let mut kind = Field::in_form("material", "material_type_id")
            .kind(InputKind::Select)
            .label("Material type")
            .prompt("No type")
            .options(choices(&types))
            .value(&self.form.material_type_id);
        if existing {
            unit = unit.disabled();
            kind = kind.disabled();
        }
        Ok(vec![
            render(sku)?,
            render(
                Field::in_form("material", "title")
                    .label("Title")
                    .value(&self.form.title),
            )?,
            render(
                Field::in_form("material", "description")
                    .kind(InputKind::Textarea)
                    .label("Description")
                    .value(&self.form.description),
            )?,
            render(
                Field::in_form("material", "status")
                    .label("Status")
                    .hint(format!(
                        "Configured statuses: {}",
                        self.settings
                            .statuses
                            .as_deref()
                            .unwrap_or_default()
                            .join(", ")
                    ))
                    .value(&self.form.status),
            )?,
            render(
                Field::in_form("material", "currency_code")
                    .label("Currency")
                    .hint(format!(
                        "Default: {}",
                        self.settings
                            .default_currency_code
                            .as_deref()
                            .unwrap_or("none")
                    ))
                    .value(&self.form.currency_code),
            )?,
            render(unit)?,
            render(kind)?,
            render(
                Field::in_form("material", "properties")
                    .kind(InputKind::Textarea)
                    .label("Property values (JSON)")
                    .hint("Keys and value types are defined by the selected material type.")
                    .value(&self.form.properties),
            )?,
        ])
    }

    pub(super) async fn render(&self, ctx: &Ctx<'_>) -> Result<String> {
        let manage = ctx.can_manage().await?;
        let mut actions = link("Material types", "/factory/material-types")?;
        actions.push(&text(" "));
        actions.push(&link("Conversions", "/factory/conversions")?);
        if manage {
            actions.push(&text(" "));
            actions.push(&primary("new-material", "New material", "new")?);
        }
        let mut body = Html::EMPTY;
        if manage && self.edit != Edit::Closed {
            body.push(&editor(
                "material-editor",
                if self.edit.is_new() {
                    "New material"
                } else {
                    "Material"
                },
                "material-form",
                "save_material",
                self.form_fields()?,
            )?);
        }
        let rows = self
            .materials
            .iter()
            .map(|material| {
                let kind = self
                    .types
                    .iter()
                    .find(|kind| Some(kind.id) == material.material_type_id)
                    .map_or("—", |kind| kind.name.as_str());
                let mut row = FlexRow::new(material.item_id.to_string())
                    .cell("sku", Cell::text(&material.sku))
                    .cell("native_unit", Cell::text(&material.native_unit.code))
                    .cell("type", Cell::text(kind))
                    .cell(
                        "state",
                        Cell::text(if material.retired {
                            "Retired"
                        } else {
                            "Active"
                        }),
                    );
                row.dom_id = Some(format!("material-{}", material.item_id));
                if manage && !material.retired {
                    row.actions = row_actions("material", material.item_id)?;
                }
                Ok(row)
            })
            .collect::<Result<Vec<_>>>()?;
        body.push(&list(
            "materials",
            "materials-table",
            "Materials",
            &self.columns,
            "grid",
            rows,
            EmptyState::missing("No materials", "Create a unit and a material to begin."),
            true,
        )?);
        if self.confirming.is_some() {
            body.push(&confirm(
                "material-retire-confirm",
                "Retire this material?",
                "Retire",
                "Retiring…",
            )?);
        }
        layout(
            "materials-index",
            PageVariant::List,
            Header {
                title: text("Materials"),
                subtitle: "Register an item with a native unit, material type, and typed values.",
                actions,
                ..Header::default()
            },
            "material-error",
            &self.notice,
            body,
        )
    }
}
