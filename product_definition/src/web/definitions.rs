//! `/factory/definitions`: product definitions with their immutable formula
//! and routing revisions, mirroring Bilimbi `ProductDefinition.Web.DefinitionsLive`.
use super::shared::{self as s, Notice, PageView, Result};
use super::Pages;
use crate::{contributions::MANAGE, FormulaRevision, NewProduct, Product, RoutingRevision};
use askama::Template;
use bilibi_factory_inventory::Item;
use bilibi_tenancy::Scope;
use bilibi_ui::{Button, Card, Choice, FlexRow, Html, InputKind, ListColumns, RowMode};
use serde_json::{json, Value};

const DENIED: &str = "You do not have permission to manage definitions.";
const PRODUCT: &str = "product";
const FORMULA: &str = "formula";
const ROUTING: &str = "routing";

/// A revision's JSON, one block in a normal row and one line in a compact one.
#[derive(Template)]
#[template(
    source = r#"<pre class="whitespace-pre-wrap text-xs">{{ text }}</pre>"#,
    ext = "html"
)]
struct Block<'a> {
    text: &'a str,
}

#[derive(Clone, Default)]
struct Draft {
    values: [String; 3],
}

pub(super) struct Definitions {
    pages: Pages,
    products_columns: ListColumns,
    formulas_columns: ListColumns,
    routings_columns: ListColumns,
    products: Vec<Product>,
    items: Vec<Item>,
    selected: Option<i64>,
    formulas: Vec<FormulaRevision>,
    routings: Vec<RoutingRevision>,
    product_draft: Draft,
    formula_draft: Draft,
    routing_draft: Draft,
    can_manage: bool,
    notice: Notice,
}

fn blank_publish() -> Draft {
    Draft {
        values: ["[]".into(), "{}".into(), String::new()],
    }
}

impl Definitions {
    pub(super) fn new(pages: &Pages) -> Result<Self> {
        let revisions = |id: &str, body: &str| {
            s::mount_columns(
                id,
                vec![
                    s::column("version", "Version"),
                    s::column("body", body),
                    s::column("process_config", "Process configuration"),
                ],
            )
        };
        Ok(Self {
            pages: pages.clone(),
            products_columns: s::mount_columns(
                "product-definitions-table",
                vec![s::column("code", "Code"), s::column("name", "Name")],
            )?,
            formulas_columns: revisions("formula-revisions-table", "Lines")?,
            routings_columns: revisions("routing-revisions-table", "Operations")?,
            products: Vec::new(),
            items: Vec::new(),
            selected: None,
            formulas: Vec::new(),
            routings: Vec::new(),
            product_draft: Draft::default(),
            formula_draft: blank_publish(),
            routing_draft: blank_publish(),
            can_manage: false,
            notice: Notice::default(),
        })
    }

    fn selected(&self) -> Option<&Product> {
        self.products.iter().find(|p| Some(p.id) == self.selected)
    }

    async fn load(&mut self, scope: &Scope) -> Result<()> {
        self.can_manage = s::can(&self.pages.db, scope, MANAGE).await?;
        self.products = Vec::new();
        self.items = Vec::new();
        let Some(company) = s::company(scope) else {
            self.notice.fail("Company not found");
            return Ok(());
        };
        let products = self
            .pages
            .definitions
            .list_products(scope, company, None)
            .await;
        let items = self
            .pages
            .inventory
            .list_items(scope, company, None, Some(500))
            .await
            .map_err(crate::Error::from);
        match (s::refusal(products)?, s::refusal(items)?) {
            (Ok(products), Ok(items)) => {
                self.products = products;
                self.items = items;
            }
            (Err(message), _) | (_, Err(message)) => self.notice.fail(message),
        }
        self.products_columns
            .load(&self.products, |p| p.id.to_string());
        self.select(scope, self.selected).await
    }

    /// Reads the selected product's revisions; an unknown selection clears.
    async fn select(&mut self, scope: &Scope, id: Option<i64>) -> Result<()> {
        self.selected = id.filter(|id| self.products.iter().any(|p| p.id == *id));
        self.formulas = Vec::new();
        self.routings = Vec::new();
        let (Some(product), Some(company)) = (self.selected, s::company(scope)) else {
            return Ok(());
        };
        let facade = &self.pages.definitions;
        let formulas = facade
            .list_formula_revisions(scope, company, product, None)
            .await;
        let routings = facade
            .list_routing_revisions(scope, company, product, None)
            .await;
        match (s::refusal(formulas)?, s::refusal(routings)?) {
            (Ok(formulas), Ok(routings)) => {
                self.formulas = formulas;
                self.routings = routings;
                self.formulas_columns
                    .load(&self.formulas, |f| f.version.to_string());
                self.routings_columns
                    .load(&self.routings, |r| r.version.to_string());
                self.notice.error = None;
            }
            (Err(message), _) | (_, Err(message)) => self.notice.fail(message),
        }
        Ok(())
    }

    pub(super) async fn refresh(&mut self, scope: &Scope) -> Result<()> {
        self.load(scope).await
    }

    pub(super) async fn event(&mut self, scope: &Scope, payload: &Value) -> Result<()> {
        self.notice.begin();
        let kind = s::text(payload, "type").ok_or(bilibi_database::Error::Invalid)?;
        match kind {
            "clear-flash" => return Ok(()),
            // Each table raises its own event, so its columns are the ones touched.
            "grid_products" => {
                s::grid(&mut self.products_columns, payload);
                return Ok(());
            }
            "grid_formulas" => {
                s::grid(&mut self.formulas_columns, payload);
                return Ok(());
            }
            "grid_routings" => {
                s::grid(&mut self.routings_columns, payload);
                return Ok(());
            }
            "select" => {
                // Reading is already allowed by the route; the facade asks again.
                let id = s::id(payload);
                if id.is_some_and(|id| self.products.iter().any(|p| p.id == id)) {
                    return self.select(scope, id).await;
                }
                self.notice.fail("Product definition not found.");
                return Ok(());
            }
            "create_product" | "publish_formula" | "publish_routing" => {}
            _ => return Err(bilibi_database::Error::Invalid),
        }
        if !s::can(&self.pages.db, scope, MANAGE).await? {
            self.notice.fail(DENIED);
            return self.load(scope).await;
        }
        match kind {
            "create_product" => self.create_product(scope, payload).await?,
            "publish_formula" => self.publish(scope, payload, true).await?,
            _ => self.publish(scope, payload, false).await?,
        }
        Ok(())
    }

    async fn create_product(&mut self, scope: &Scope, payload: &Value) -> Result<()> {
        let posted = s::form(payload, PRODUCT);
        let draft = Draft {
            values: [
                s::pick(&posted, "item_id"),
                s::pick(&posted, "code"),
                s::pick(&posted, "name"),
            ],
        };
        let Some(company) = s::company(scope) else {
            self.product_draft = draft;
            self.notice.fail("Company not found");
            return Ok(());
        };
        let item_id = draft.values[0].trim().parse::<i64>().ok();
        let new = NewProduct {
            item_id: item_id.unwrap_or_default(),
            code: draft.values[1].clone(),
            name: draft.values[2].clone(),
        };
        let done = if item_id.is_some() {
            self.pages
                .definitions
                .create_product(scope, company, &new)
                .await
        } else {
            Err(crate::Error::Inventory(
                bilibi_factory_inventory::Error::ItemNotFound,
            ))
        };
        let created = s::refusal(done)?;
        match created {
            Ok(product) => {
                let id = product.id;
                drop(product);
                self.product_draft = Draft::default();
                self.load(scope).await?;
                self.select(scope, Some(id)).await?;
                self.notice.success("Product definition created.");
            }
            Err(message) => {
                self.product_draft = draft;
                self.notice.fail(message);
            }
        }
        Ok(())
    }

    async fn publish(&mut self, scope: &Scope, payload: &Value, formula: bool) -> Result<()> {
        let (form, first) = if formula {
            (FORMULA, "lines")
        } else {
            (ROUTING, "operations")
        };
        let posted = s::form(payload, form);
        let draft = Draft {
            values: [
                s::pick(&posted, first),
                s::pick(&posted, "process_config"),
                String::new(),
            ],
        };
        let (Some(product), Some(company)) = (self.selected, s::company(scope)) else {
            self.notice.fail("Select a product definition.");
            return Ok(());
        };
        let attrs = json!({
            first: s::decode(&draft.values[0]),
            "process_config": s::decode(&draft.values[1]),
        });
        let facade = &self.pages.definitions;
        let done = if formula {
            facade
                .publish_formula(scope, company, product, &attrs)
                .await
                .map(drop)
        } else {
            facade
                .publish_routing(scope, company, product, &attrs)
                .await
                .map(drop)
        };
        match s::refusal(done)? {
            Ok(()) => {
                if formula {
                    self.formula_draft = blank_publish();
                } else {
                    self.routing_draft = blank_publish();
                }
                self.select(scope, Some(product)).await?;
                self.notice.success(if formula {
                    "Formula revision published."
                } else {
                    "Routing revision published."
                });
            }
            Err(message) => {
                if formula {
                    self.formula_draft = draft;
                } else {
                    self.routing_draft = draft;
                }
                self.notice.fail(message);
            }
        }
        Ok(())
    }

    fn block(columns: &ListColumns, value: &Value) -> Result<Html> {
        let text = s::json(value, columns.mode() != RowMode::Compact);
        s::render(Block { text: &text })
    }

    fn product_rows(&self) -> Result<Vec<FlexRow>> {
        let mut rows = Vec::with_capacity(self.products.len());
        for product in &self.products {
            let mut row = FlexRow::new(product.id.to_string());
            row.dom_id = Some(format!("product-definition-{}", product.id));
            row = s::slot(row, "code", &product.code);
            row = s::slot(row, "name", &product.name);
            row.actions = s::render(Button {
                id: &format!("product-definition-{}-revisions", product.id),
                button_type: "button",
                event: "select",
                name: "id",
                value: &product.id.to_string(),
                body: Html::text("Revisions"),
                ..Button::default()
            })?;
            rows.push(row);
        }
        Ok(rows)
    }

    /// A line as stored: absent optional keys stay absent, not `null`.
    fn lines(revision: &FormulaRevision) -> Value {
        let lines = revision
            .lines
            .iter()
            .map(|line| {
                let mut value = json!(line);
                if let Some(map) = value.as_object_mut() {
                    map.retain(|_, v| !v.is_null());
                }
                value
            })
            .collect();
        Value::Array(lines)
    }

    fn formula_rows(&self) -> Result<Vec<FlexRow>> {
        self.formulas
            .iter()
            .map(|formula| {
                let mut row = FlexRow::new(formula.version.to_string());
                row.dom_id = Some(format!("formula-revision-{}", formula.version));
                row = s::slot(row, "version", &formula.version.to_string());
                Ok(row
                    .slot(
                        "body",
                        Self::block(&self.formulas_columns, &Self::lines(formula))?,
                    )
                    .slot(
                        "process_config",
                        Self::block(
                            &self.formulas_columns,
                            &Value::Object(formula.process_config.clone()),
                        )?,
                    ))
            })
            .collect()
    }

    fn routing_rows(&self) -> Result<Vec<FlexRow>> {
        self.routings
            .iter()
            .map(|routing| {
                let mut row = FlexRow::new(routing.version.to_string());
                row.dom_id = Some(format!("routing-revision-{}", routing.version));
                row = s::slot(row, "version", &routing.version.to_string());
                Ok(row
                    .slot(
                        "body",
                        Self::block(&self.routings_columns, &json!(routing.operations))?,
                    )
                    .slot(
                        "process_config",
                        Self::block(
                            &self.routings_columns,
                            &Value::Object(routing.process_config.clone()),
                        )?,
                    ))
            })
            .collect()
    }

    fn product_form(&self) -> Result<Html> {
        let items = self
            .items
            .iter()
            .map(|item| {
                Choice::new(
                    format!("{} — {}", item.sku, item.title),
                    item.id.to_string(),
                )
            })
            .collect();
        let item = s::text_field(
            PRODUCT,
            "item_id",
            "Output item",
            &self.product_draft.values[0],
        )
        .kind(InputKind::Select)
        .prompt("Choose an item")
        .options(items);
        let form = s::render(s::FormCard {
            form_id: "product-form",
            event: "create_product",
            fields: vec![
                item,
                s::text_field(PRODUCT, "code", "Code", &self.product_draft.values[1]),
                s::text_field(PRODUCT, "name", "Name", &self.product_draft.values[2]),
            ],
            submit: "Create product definition",
            cancel: false,
        })?;
        s::render(Card {
            id: "new-product",
            title: "New product definition",
            class: "mt-5",
            body: form,
            ..Card::default()
        })
    }

    /// One revision card: its table, then the publish form when allowed.
    #[allow(clippy::too_many_arguments)]
    fn revisions_card(&self, product: &Product, formula: bool, rows: Vec<FlexRow>) -> Result<Html> {
        let (id, noun, columns, draft, form_id, event, label, hint, button) = if formula {
            (
                "formula-revisions",
                "formulas",
                &self.formulas_columns,
                &self.formula_draft,
                "formula-form",
                "publish_formula",
                "Lines (JSON)",
                "Each line names item_id, unit_id, role, and quantity.",
                "Publish formula version",
            )
        } else {
            (
                "routing-revisions",
                "routings",
                &self.routings_columns,
                &self.routing_draft,
                "routing-form",
                "publish_routing",
                "Operations (JSON)",
                "Each operation names code, sequence, inputs, outputs, and allowed_resource_ids.",
                "Publish routing version",
            )
        };
        let (empty, reason, caption) = if formula {
            (
                "No formulas",
                "Publish a formula revision to define inputs and outputs.",
                "Formula revisions",
            )
        } else {
            (
                "No routings",
                "Publish a routing revision to define operations.",
                "Routing revisions",
            )
        };
        let namespace = if formula { FORMULA } else { ROUTING };
        let first = if formula { "lines" } else { "operations" };
        let mut body = s::table(
            &format!("{id}-table"),
            if formula {
                "grid_formulas"
            } else {
                "grid_routings"
            },
            caption,
            columns,
            rows,
            false,
            (empty, reason),
        )?;
        if self.can_manage {
            body.push(&s::render(s::FormCard {
                form_id,
                event,
                fields: vec![
                    s::textarea_field(namespace, first, label, hint, &draft.values[0]),
                    s::textarea_field(
                        namespace,
                        "process_config",
                        "Process configuration (JSON)",
                        "",
                        &draft.values[1],
                    ),
                ],
                submit: button,
                cancel: false,
            })?);
        }
        s::render(Card {
            id,
            title: &format!("{} {noun}", product.name),
            body,
            ..Card::default()
        })
    }

    pub(super) fn view(&self) -> Result<String> {
        let mut body = self.notice.alert("definition-error")?;
        if self.can_manage {
            body.push(&self.product_form()?);
        }
        let products = s::table(
            "product-definitions-table",
            "grid_products",
            "Product definitions",
            &self.products_columns,
            self.product_rows()?,
            true,
            ("No product definitions", "Create one for a company item."),
        )?;
        body.push(&s::render(Card {
            id: "product-definitions",
            inner_class: "p-0",
            class: "mt-5",
            body: products,
            ..Card::default()
        })?);
        if let Some(product) = self.selected() {
            let mut cards = self.revisions_card(product, true, self.formula_rows()?)?;
            cards.push(&self.revisions_card(product, false, self.routing_rows()?)?);
            body.push(&s::render(Revisions { body: cards })?);
        }
        Ok(s::render(PageView {
            flash: self.notice.flash_map(),
            id: "definitions-index",
            title: "BOM formulas & routings",
            subtitle: "Published revisions are immutable. New versions use the current company items, units, and resources.",
            actions: Html::EMPTY,
            body,
            dialog: Html::EMPTY,
        })?
        .as_str()
        .to_owned())
    }
}

/// The selected product's two cards, spaced as Bilimbi's `mt-5 space-y-5`.
#[derive(Template)]
#[template(
    source = r#"<div class="mt-5 space-y-5">{{ body|safe }}</div>"#,
    ext = "html"
)]
struct Revisions {
    body: Html,
}
