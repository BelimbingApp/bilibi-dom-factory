//! The Inventory pages (`factory/inventory.pages`), ported from Bilimbi's
//! `Bilimbi.Factory.Inventory.Web.{Units,MaterialTypes,Materials,Conversions,
//! ItemSettings}Live`: `/factory/units`, `/factory/material-types`,
//! `/factory/materials`, `/factory/conversions` and `/factory/item-settings`.
//!
//! Every page is a mounted state over the [`Inventory`] facade. The route guard
//! checks `factory.inventory.configuration.view` on each request and live event;
//! each write-shaped event checks `…configuration.manage` again before the
//! facade does. Lists are the shared `FlexTable` over built-in `ListColumns`
//! (Bilimbi's tables have no sort, filter or pager, so neither do these), and a
//! retirement waits for a `ConfirmDialog`, which replaces `data-confirm`.
use crate::{
    contributions::{CONFIGURATION_MANAGE, CONFIGURATION_VIEW},
    Error as InventoryError, Inventory,
};
use askama::Template;
use bilibi_database::{Db, Error, Result};
use bilibi_tenancy::Scope;
use bilibi_ui::{
    page::{
        LiveBody, LiveFuture, LiveInput, LiveRequest, LiveState, MountFuture, Page, PageBody,
        PageFuture, PageRequest,
    },
    params, Button, ButtonVariant, Card, ColumnsOutcome, ConfirmDialog, EmptyState, FlashGroup,
    FlashMap, FlexRow, FlexTable, Header, Html, ListColumns, PageVariant, StatusKind, TableEmpty,
};
use serde_json::Value;
use std::sync::Arc;

mod conversions;
mod item_settings;
mod material_types;
mod materials;
mod units;

/// The service ID of the page adapter.
pub(crate) const SERVICE: &str = "factory/inventory.pages";

/// The rows a list loads: Bilimbi asks for the facade's maximum.
const ROWS: Option<i64> = Some(500);

/// The module's page adapter; every field is shared by mounted states.
#[derive(Clone)]
pub(crate) struct Pages {
    pub(crate) db: Db,
    pub(crate) inventory: Arc<Inventory>,
}

/// What one update works with: the adapter, the freshly guarded scope and the
/// company the actor belongs to (Bilimbi's `current_scope.user["company_id"]`);
/// an actor with no company reaches the facade's "Company not found".
struct Ctx<'a> {
    pages: &'a Pages,
    scope: &'a Scope,
    company: i64,
}

impl<'a> Ctx<'a> {
    fn new(pages: &'a Pages, scope: &'a Scope) -> Self {
        Self {
            pages,
            scope,
            company: scope
                .actor()
                .organisation()
                .map_or(0, |company| company.get()),
        }
    }

    fn inventory(&self) -> &Inventory {
        &self.pages.inventory
    }

    async fn can(&self, capability: &str) -> Result<bool> {
        let client = self.pages.db.get().await?;
        Ok(bilibi_authz::decide(&**client, self.scope, capability)
            .await?
            .allowed)
    }

    async fn can_view(&self) -> Result<bool> {
        self.can(CONFIGURATION_VIEW).await
    }

    async fn can_manage(&self) -> Result<bool> {
        self.can(CONFIGURATION_MANAGE).await
    }
}

/// The page's alert line and its flash message.
#[derive(Default)]
struct Notice {
    error: Option<String>,
    flash: Option<(StatusKind, String)>,
}

impl Notice {
    /// A completed write: the flash, and no alert line left over.
    fn saved(&mut self, message: &str) {
        self.error = None;
        self.flash = Some((StatusKind::Success, message.to_owned()));
    }

    fn say(&mut self, message: &str) {
        self.error = Some(message.to_owned());
    }

    fn denied(&mut self, what: &str) {
        self.say(&format!("You do not have permission to {what}."));
    }

    /// A refused facade call becomes the alert line; only a lapsed session
    /// ends the page.
    fn refused(&mut self, error: &InventoryError) -> Result<()> {
        if matches!(error, InventoryError::Access(Error::Unauthorized)) {
            return Err(Error::Unauthorized);
        }
        self.error = Some(error.to_string());
        Ok(())
    }

    /// The facade's answer, or `None` after recording its refusal.
    fn take<T>(&mut self, result: std::result::Result<T, InventoryError>) -> Result<Option<T>> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(error) => self.refused(&error).map(|()| None),
        }
    }

    fn flash_map(&self) -> FlashMap<'_> {
        let mut flash = FlashMap::default();
        if let Some((kind, message)) = &self.flash {
            match kind {
                StatusKind::Success => flash.success = message,
                StatusKind::Error => flash.error = message,
                StatusKind::Warning => flash.warning = message,
                StatusKind::Info => flash.info = message,
            }
        }
        flash
    }
}

/// Which record an editor is open on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Edit {
    #[default]
    Closed,
    New,
    Record(i64),
}

impl Edit {
    fn is_new(self) -> bool {
        self == Self::New
    }
}

fn render(fragment: impl Template) -> Result<Html> {
    Html::render(&fragment).map_err(|_| Error::Internal)
}

fn text(value: &str) -> Html {
    Html::text(value)
}

fn event(value: &Value) -> Result<&str> {
    value
        .get("type")
        .and_then(Value::as_str)
        .ok_or(Error::Invalid)
}

/// A string field of the event, as Bilimbi's `phx-value-*`.
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// A record ID the event names; anything else is the page's "not found".
fn record_id(value: &Value) -> Option<i64> {
    params::positive_id(field(value, "id"))
}

fn buttons(buttons: Vec<Html>) -> Result<Html> {
    #[derive(Template)]
    #[template(path = "button_row.html")]
    struct Row {
        buttons: Vec<Html>,
    }
    render(Row { buttons })
}

/// A button that raises `event`, carrying a record's `id` when it has one.
fn action(id: &str, label: &str, event: &str, record: Option<i64>) -> Result<Html> {
    let value = record.map(|record| record.to_string()).unwrap_or_default();
    render(Button {
        id,
        button_type: "button",
        event,
        name: if record.is_some() { "id" } else { "" },
        value: &value,
        body: text(label),
        ..Button::default()
    })
}

fn primary(id: &str, label: &str, event: &str) -> Result<Html> {
    render(Button {
        variant: ButtonVariant::Primary,
        id,
        button_type: "button",
        event,
        body: text(label),
        ..Button::default()
    })
}

fn link(label: &str, href: &str) -> Result<Html> {
    render(Button {
        href,
        body: text(label),
        ..Button::default()
    })
}

/// The Edit and Retire buttons of an active row (a retired row has none).
fn row_actions(prefix: &str, id: i64) -> Result<Html> {
    buttons(vec![
        action(&format!("{prefix}-edit-{id}"), "Edit", "edit", Some(id))?,
        action(
            &format!("{prefix}-retire-{id}"),
            "Retire",
            "retire",
            Some(id),
        )?,
    ])
}

/// A form's Save and Cancel, and its controls, inside a titled card.
fn editor(card: &str, title: &str, form: &str, submit: &str, fields: Vec<Html>) -> Result<Html> {
    #[derive(Template)]
    #[template(path = "editor.html")]
    struct Form<'a> {
        id: &'a str,
        event: &'a str,
        fields: Vec<Html>,
        save: Html,
        cancel: Html,
    }
    let save = render(Button {
        variant: ButtonVariant::Primary,
        button_type: "submit",
        body: text("Save"),
        ..Button::default()
    })?;
    let cancel = action("", "Cancel", "cancel", None)?;
    let body = render(Form {
        id: form,
        event: submit,
        fields,
        save,
        cancel,
    })?;
    render(Card {
        id: card,
        title,
        class: "mt-5",
        body,
        ..Card::default()
    })
}

/// A list of rows in a flexible table inside a card with no padding of its own.
#[allow(clippy::too_many_arguments)]
fn list(
    card: &str,
    table: &str,
    caption: &str,
    columns: &ListColumns,
    grid_event: &str,
    rows: Vec<FlexRow>,
    empty: EmptyState,
    actions: bool,
) -> Result<Html> {
    let none = rows.is_empty();
    let mut flex = FlexTable::new(table, columns.flex_columns(), rows);
    flex.event = grid_event;
    flex.zoom = columns.zoom;
    flex.suggestions = columns.flex_suggestions();
    flex.add_query = &columns.add_query;
    flex.caption = caption;
    flex.framed = false;
    flex.actions = actions;
    if none {
        flex.empty = Some(TableEmpty::State(empty));
    }
    render(Card {
        id: card,
        inner_class: "p-0",
        class: "mt-5",
        body: render(flex)?,
        ..Card::default()
    })
}

/// A flex table's own `grid` event (column settings, row height).
fn grid(columns: &mut ListColumns, value: &Value) {
    // No column of these lists sorts, so only the arrangement can change.
    let _: ColumnsOutcome = columns.handle(&params::string_values(value));
}

fn confirm(id: &str, consequence: &str, confirm: &str, working: &str) -> Result<Html> {
    render(ConfirmDialog {
        id,
        consequence,
        detail: "",
        confirm,
        working,
        confirm_event: "retire_confirmed",
        cancel_event: "cancel_retire",
    })
}

/// The page: flash, header, alert line and body, in Bilimbi's order.
fn layout(
    id: &str,
    variant: PageVariant,
    header: Header<'_>,
    error_id: &str,
    notice: &Notice,
    body: Html,
) -> Result<String> {
    #[derive(Template)]
    #[template(path = "error_line.html")]
    struct Line<'a> {
        id: &'a str,
        error: Option<&'a str>,
    }
    let mut content = render(header)?;
    content.push(&render(Line {
        id: error_id,
        error: notice.error.as_deref(),
    })?);
    content.push(&body);
    let mut html = render(FlashGroup {
        id: "flash-group",
        flash: notice.flash_map(),
    })?;
    html.push(&render(bilibi_ui::Page {
        id,
        variant,
        body: content,
        ..bilibi_ui::Page::default()
    })?);
    Ok(html.as_str().to_owned())
}

/// A mounted page.
enum Screen {
    Units(units::Units),
    MaterialTypes(material_types::MaterialTypes),
    Materials(materials::Materials),
    Conversions(conversions::Conversions),
    ItemSettings(item_settings::ItemSettings),
}

impl Screen {
    async fn open(operation: &str, ctx: &Ctx<'_>) -> Result<Self> {
        Ok(match operation {
            "units" => Self::Units(units::Units::open(ctx).await?),
            "material_types" => {
                Self::MaterialTypes(material_types::MaterialTypes::open(ctx).await?)
            }
            "materials" => Self::Materials(materials::Materials::open(ctx).await?),
            "conversions" => Self::Conversions(conversions::Conversions::open(ctx).await?),
            "item_settings" => Self::ItemSettings(item_settings::ItemSettings::open(ctx).await?),
            _ => return Err(Error::NotFound),
        })
    }

    async fn refresh(&mut self, ctx: &Ctx<'_>) -> Result<()> {
        match self {
            Self::Units(page) => page.load(ctx).await,
            Self::MaterialTypes(page) => page.load(ctx).await,
            Self::Materials(page) => page.load(ctx).await,
            Self::Conversions(page) => page.refresh(ctx).await,
            Self::ItemSettings(page) => page.load(ctx).await,
        }
    }

    async fn event(&mut self, ctx: &Ctx<'_>, value: &Value) -> Result<()> {
        match self {
            Self::Units(page) => page.event(ctx, value).await,
            Self::MaterialTypes(page) => page.event(ctx, value).await,
            Self::Materials(page) => page.event(ctx, value).await,
            Self::Conversions(page) => page.event(ctx, value).await,
            Self::ItemSettings(page) => page.event(ctx, value).await,
        }
    }

    async fn render(&self, ctx: &Ctx<'_>) -> Result<String> {
        match self {
            Self::Units(page) => page.render(ctx).await,
            Self::MaterialTypes(page) => page.render(ctx).await,
            Self::Materials(page) => page.render(ctx).await,
            Self::Conversions(page) => page.render(ctx).await,
            Self::ItemSettings(page) => page.render(ctx).await,
        }
    }
}

struct Mounted {
    pages: Pages,
    screen: Screen,
}

impl LiveState<Scope, Error> for Mounted {
    fn update<'a>(&'a mut self, request: LiveRequest<'a, Scope>) -> LiveFuture<'a, Error> {
        Box::pin(async move {
            let ctx = Ctx::new(&self.pages, request.scope);
            match request.input {
                LiveInput::Refresh => self.screen.refresh(&ctx).await?,
                LiveInput::Params(_) => {}
                LiveInput::Event(value) => self.screen.event(&ctx, value).await?,
            }
            Ok(LiveBody {
                html: self.screen.render(&ctx).await?,
                ..LiveBody::default()
            })
        })
    }
}

impl Page<Scope, Error> for Pages {
    fn title(&self, operation: &str) -> Option<&'static str> {
        Some(match operation {
            "units" => "Units",
            "material_types" => "Material types",
            "materials" => "Materials",
            "conversions" => "Conversions",
            "item_settings" => "Item settings",
            _ => return None,
        })
    }

    fn render<'a>(&'a self, request: PageRequest<'a, Scope>) -> PageFuture<'a, Error> {
        Box::pin(async move {
            let scope = request.scope.ok_or(Error::Unauthorized)?;
            let ctx = Ctx::new(self, scope);
            let screen = Screen::open(request.operation, &ctx).await?;
            Ok(PageBody {
                html: screen.render(&ctx).await?,
                live: true,
                title: None,
            })
        })
    }

    fn mount<'a>(&'a self, request: PageRequest<'a, Scope>) -> MountFuture<'a, Scope, Error> {
        Box::pin(async move {
            let scope = request.scope.ok_or(Error::Unauthorized)?;
            let ctx = Ctx::new(self, scope);
            let screen = Screen::open(request.operation, &ctx).await?;
            let state: Box<dyn LiveState<Scope, Error>> = Box::new(Mounted {
                pages: self.clone(),
                screen,
            });
            Ok(Some(state))
        })
    }
}
