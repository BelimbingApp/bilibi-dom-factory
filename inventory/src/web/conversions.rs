//! `/factory/conversions` (Bilimbi `Web.ConversionsLive`): pick a material to
//! read its conversion versions and publish the next factor for a unit.
use super::*;
use crate::{Conversion, Decimal, FieldError, Material, Unit};
use bilibi_ui::{choices, BuiltinColumn, Cell, Field, InputKind};
use std::str::FromStr;

#[derive(Clone, Default)]
struct Form {
    unit_id: String,
    factor: String,
}

pub(super) struct Conversions {
    notice: Notice,
    form: Form,
    materials: Vec<Material>,
    units: Vec<Unit>,
    selected: Option<Material>,
    conversions: Vec<Conversion>,
    material_columns: ListColumns,
    version_columns: ListColumns,
}

impl Conversions {
    pub(super) async fn open(ctx: &Ctx<'_>) -> Result<Self> {
        let mount = |id: &str, columns: Vec<BuiltinColumn>| {
            ListColumns::mount(id, columns).map_err(|_| Error::Internal)
        };
        let mut page = Self {
            notice: Notice::default(),
            form: Form::default(),
            materials: Vec::new(),
            units: Vec::new(),
            selected: None,
            conversions: Vec::new(),
            material_columns: mount(
                "conversion-materials",
                vec![
                    BuiltinColumn::new("material", "Material"),
                    BuiltinColumn::new("native_unit", "Native unit"),
                ],
            )?,
            version_columns: mount(
                "conversion-versions",
                vec![
                    BuiltinColumn::new("unit", "Unit"),
                    BuiltinColumn::new("version", "Version"),
                    BuiltinColumn::new("factor", "Factor"),
                ],
            )?,
        };
        page.load(ctx).await?;
        Ok(page)
    }

    async fn load(&mut self, ctx: &Ctx<'_>) -> Result<()> {
        self.materials.clear();
        self.units.clear();
        if ctx.can_view().await? {
            let inventory = ctx.inventory();
            let loaded = async {
                Ok::<_, crate::Error>((
                    inventory
                        .list_materials(ctx.scope, ctx.company, ROWS)
                        .await?,
                    inventory.list_units(ctx.scope, ctx.company, ROWS).await?,
                ))
            }
            .await;
            if let Some((materials, units)) = self.notice.take(loaded)? {
                self.materials = materials;
                self.units = units;
            }
        } else {
            self.notice
                .say("You do not have permission to view conversions.");
        }
        self.material_columns
            .load(&self.materials, |material| material.item_id.to_string());
        Ok(())
    }

    /// A refresh re-reads the lists and the open material's versions.
    pub(super) async fn refresh(&mut self, ctx: &Ctx<'_>) -> Result<()> {
        self.load(ctx).await?;
        if let Some(item_id) = self.selected.as_ref().map(|material| material.item_id) {
            self.selected = self
                .materials
                .iter()
                .find(|material| material.item_id == item_id)
                .cloned();
            if let Some(material) = self.selected.clone() {
                self.select(ctx, material).await?;
            } else {
                self.conversions.clear();
            }
        }
        Ok(())
    }

    async fn select(&mut self, ctx: &Ctx<'_>, material: Material) -> Result<()> {
        let listed = ctx
            .inventory()
            .list_conversions(ctx.scope, ctx.company, material.item_id)
            .await;
        if let Some(conversions) = self.notice.take(listed)? {
            self.conversions = conversions;
            self.version_columns
                .load(&self.conversions, |conversion| conversion.id.to_string());
            self.selected = Some(material);
            self.notice.error = None;
        }
        Ok(())
    }

    pub(super) async fn event(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        match event(value)? {
            "grid_materials" => grid(&mut self.material_columns, value),
            "grid_versions" => grid(&mut self.version_columns, value),
            "clear-flash" => self.notice.flash = None,
            "select" => {
                if !ctx.can_view().await? {
                    self.notice
                        .say("You do not have permission to view conversions.");
                    return Ok(());
                }
                let found = record_id(value).and_then(|id| {
                    self.materials
                        .iter()
                        .find(|material| material.item_id == id)
                        .cloned()
                });
                match found {
                    Some(material) => self.select(ctx, material).await?,
                    None => self.notice.say("Material not found."),
                }
            }
            "save_conversion" => {
                if !ctx.can_manage().await? {
                    self.notice.denied("manage conversions");
                    return Ok(());
                }
                self.save(ctx, value).await?;
            }
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }

    async fn save(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        let posted = params::form_values(value, "conversion");
        let form = Form {
            unit_id: posted.get("unit_id").cloned().unwrap_or_default(),
            factor: posted.get("factor").cloned().unwrap_or_default(),
        };
        let (Some(material), Some(unit_id)) = (
            self.selected.clone(),
            params::positive_id(Some(&form.unit_id)),
        ) else {
            self.notice.say("Select a material and unit.");
            return Ok(());
        };
        let defined = match Decimal::from_str(form.factor.trim()) {
            Ok(factor) => {
                ctx.inventory()
                    .define_conversion(ctx.scope, ctx.company, material.item_id, unit_id, factor)
                    .await
            }
            Err(_) => Err(crate::Error::Invalid(vec![FieldError {
                field: "factor",
                message: "is invalid".into(),
            }])),
        };
        if self.notice.take(defined)?.is_some() {
            self.select(ctx, material).await?;
            self.form = Form::default();
            self.notice.saved("Conversion version saved.");
        } else {
            self.form = form;
        }
        Ok(())
    }

    pub(super) async fn render(&self, ctx: &Ctx<'_>) -> Result<String> {
        let manage = ctx.can_manage().await?;
        let rows = self
            .materials
            .iter()
            .map(|material| {
                let mut row = FlexRow::new(material.item_id.to_string())
                    .cell("material", Cell::text(&material.sku))
                    .cell("native_unit", Cell::text(&material.native_unit.code));
                row.dom_id = Some(format!("conversion-material-{}", material.item_id));
                row.actions = action(
                    &format!("conversion-select-{}", material.item_id),
                    "Versions",
                    "select",
                    Some(material.item_id),
                )?;
                Ok(row)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut body = list(
            "conversion-materials",
            "conversion-materials-table",
            "Materials",
            &self.material_columns,
            "grid_materials",
            rows,
            EmptyState::missing(
                "No materials",
                "Register a material before defining conversions.",
            ),
            true,
        )?;
        if let Some(selected) = &self.selected {
            body.push(&self.versions(selected, manage)?);
        }
        layout(
            "conversions-index",
            PageVariant::List,
            Header {
                title: text("Conversions"),
                subtitle: "Each change publishes a new factor version. Earlier versions remain available for posted transactions.",
                ..Header::default()
            },
            "conversion-error",
            &self.notice,
            body,
        )
    }

    fn versions(&self, selected: &Material, manage: bool) -> Result<Html> {
        let rows = self
            .conversions
            .iter()
            .map(|conversion| {
                let mut row = FlexRow::new(conversion.id.to_string())
                    .cell("unit", Cell::text(&conversion.unit.code))
                    .cell("version", Cell::text(conversion.version.to_string()))
                    .cell("factor", Cell::text(conversion.factor.to_string()));
                row.dom_id = Some(format!("conversion-version-{}", conversion.id));
                row
            })
            .collect::<Vec<_>>();
        let none = rows.is_empty();
        let mut table = FlexTable::new(
            "conversion-versions-table",
            self.version_columns.flex_columns(),
            rows,
        );
        table.event = "grid_versions";
        table.zoom = self.version_columns.zoom;
        table.suggestions = self.version_columns.flex_suggestions();
        table.add_query = &self.version_columns.add_query;
        table.caption = "Conversion versions";
        table.framed = false;
        if none {
            table.empty = Some(TableEmpty::State(EmptyState::missing(
                "No conversions",
                "Publish the first factor for this material.",
            )));
        }
        let mut body = render(table)?;
        if manage {
            let units: Vec<(String, String)> = self
                .units
                .iter()
                .filter(|unit| !unit.retired && unit.id != selected.native_unit.id)
                .map(|unit| {
                    (
                        format!("{} ({})", unit.name, unit.code),
                        unit.id.to_string(),
                    )
                })
                .collect();
            body.push(&conversion_form(vec![
                render(
                    Field::in_form("conversion", "unit_id")
                        .kind(InputKind::Select)
                        .label("Unit")
                        .prompt("Choose a unit")
                        .options(choices(&units))
                        .value(&self.form.unit_id),
                )?,
                render(
                    Field::in_form("conversion", "factor")
                        .label("Native units per one selected unit")
                        .value(&self.form.factor),
                )?,
            ])?);
        }
        render(Card {
            id: "conversion-versions",
            title: &format!("{} conversions", selected.sku),
            class: "mt-5",
            body,
            ..Card::default()
        })
    }
}

/// The publish form: Bilimbi's has a single Publish conversion button.
fn conversion_form(fields: Vec<Html>) -> Result<Html> {
    #[derive(Template)]
    #[template(
        source = r#"<form id="conversion-form" data-event="save_conversion" class="space-y-4 p-4">{% for field in fields %}{{ field|safe }}{% endfor %}{{ save|safe }}</form>"#,
        ext = "html"
    )]
    struct Form {
        fields: Vec<Html>,
        save: Html,
    }
    let save = render(Button {
        variant: ButtonVariant::Primary,
        button_type: "submit",
        body: text("Publish conversion"),
        ..Button::default()
    })?;
    render(Form { fields, save })
}
