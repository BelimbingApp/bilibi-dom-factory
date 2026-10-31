//! What the three pages share: fragments, the notice (flash and error alert),
//! error wording, and the FlexTable and form helpers. Private to the module.
use crate::Error;
use askama::Template;
use bilibi_database::{Db, Error as DbError};
use bilibi_tenancy::Scope;
use bilibi_ui::{
    Alert, BuiltinColumn, ColumnType, EmptyState, Field, FlashMap, FlexRow, FlexTable, Html,
    InputKind, ListColumns, StatusKind, TableEmpty, NO_FLASH,
};
use std::collections::HashMap;

pub(super) type Result<T> = std::result::Result<T, DbError>;

pub(super) fn render(fragment: impl Template) -> Result<Html> {
    Html::render(&fragment).map_err(|_| DbError::Internal)
}

/// The company of the signed-in account, as Bilimbi's `user["company_id"]`.
pub(super) fn company(scope: &Scope) -> Option<i64> {
    scope.current_company().map(|id| id.get())
}

/// Whether the actor holds `key` now (Bilimbi `Authz.can`).
pub(super) async fn can(db: &Db, scope: &Scope, key: &str) -> Result<bool> {
    let client = db.get().await?;
    Ok(bilibi_authz::decide(&**client, scope, key).await?.allowed)
}

/// A facade refusal as the page words it. Only a lost sign-in ends the page:
/// every other refusal is text the person reads, as Bilimbi's `error_text`.
pub(super) fn refusal<T>(
    outcome: std::result::Result<T, Error>,
) -> Result<std::result::Result<T, String>> {
    match outcome {
        Ok(value) => Ok(Ok(value)),
        Err(Error::Access(DbError::Unauthorized)) => Err(DbError::Unauthorized),
        Err(error) => Ok(Err(error.to_string())),
    }
}

/// The flash and the inline error a screen shows for the render that follows
/// its event. The next intent clears both, so a dismissed message never returns.
#[derive(Default)]
pub(super) struct Notice {
    flash: Option<(StatusKind, String)>,
    pub(super) error: Option<String>,
}
impl Notice {
    pub(super) fn begin(&mut self) {
        self.flash = None;
    }
    pub(super) fn success(&mut self, message: &str) {
        self.flash = Some((StatusKind::Success, message.into()));
    }
    pub(super) fn fail(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
    }
    pub(super) fn flash_map(&self) -> FlashMap<'_> {
        let mut map = NO_FLASH;
        if let Some((kind, message)) = &self.flash {
            match kind {
                StatusKind::Success => map.success = message,
                StatusKind::Error => map.error = message,
                StatusKind::Warning => map.warning = message,
                StatusKind::Info => map.info = message,
            }
        }
        map
    }
    /// Bilimbi's `<p id=… role="alert">` line, drawn with the shared alert.
    pub(super) fn alert(&self, id: &str) -> Result<Html> {
        match &self.error {
            Some(message) => render(Alert {
                kind: StatusKind::Error,
                id,
                class: "mt-4",
                body: Html::text(message),
            }),
            None => Ok(Html::EMPTY),
        }
    }
}

#[derive(Template)]
#[template(path = "page.html")]
pub(super) struct PageView<'a> {
    pub(super) flash: FlashMap<'a>,
    pub(super) id: &'a str,
    pub(super) title: &'a str,
    pub(super) subtitle: &'a str,
    pub(super) actions: Html,
    pub(super) body: Html,
    pub(super) dialog: Html,
}

#[derive(Template)]
#[template(path = "form_card.html")]
pub(super) struct FormCard<'a> {
    pub(super) form_id: &'a str,
    pub(super) event: &'a str,
    pub(super) fields: Vec<Field>,
    pub(super) submit: &'a str,
    pub(super) cancel: bool,
}

/// A text control of a form namespace (`type[code]`), id `type-code`.
pub(super) fn text_field(form: &str, key: &str, label: &str, value: &str) -> Field {
    Field::in_form(form, key)
        .id(format!("{form}-{}", key.replace('_', "-")))
        .label(label)
        .value(value)
}

pub(super) fn textarea_field(form: &str, key: &str, label: &str, hint: &str, value: &str) -> Field {
    text_field(form, key, label, value)
        .kind(InputKind::Textarea)
        .hint(hint)
}

pub(super) fn column(id: &str, label: &str) -> BuiltinColumn {
    BuiltinColumn::new(id, label)
}

pub(super) fn count_column(id: &str, label: &str) -> BuiltinColumn {
    let mut column = BuiltinColumn::new(id, label).right();
    column.kind = ColumnType::Integer;
    column
}

pub(super) fn mount_columns(id: &str, builtins: Vec<BuiltinColumn>) -> Result<ListColumns> {
    ListColumns::mount(id, builtins).map_err(|_| DbError::Internal)
}

/// The grid events of a flex table. No Product Definition column sorts, so
/// the outcome has no sort to answer.
pub(super) fn grid(columns: &mut ListColumns, payload: &serde_json::Value) {
    let _ = columns.handle(&bilibi_ui::params::string_values(payload));
}

/// One list, framed by its card as Bilimbi's `table framed={false}`.
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
    render(table)
}

/// An event's string value, as `data-*`/`name`/`value` controls post it.
pub(super) fn text<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(serde_json::Value::as_str)
}

/// The id a row control posted: a whole positive integer, or none.
pub(super) fn id(payload: &serde_json::Value) -> Option<i64> {
    bilibi_ui::params::positive_id(text(payload, "id"))
}

pub(super) fn form(payload: &serde_json::Value, name: &str) -> HashMap<String, String> {
    bilibi_ui::params::form_values(payload, name)
}

/// Pretty JSON with keys in order, as Bilimbi's `Jason.encode!(pretty: true)`.
pub(super) fn json(value: &serde_json::Value, pretty: bool) -> String {
    let rendered = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    rendered.unwrap_or_default()
}

/// A value the textarea held, decoded; undecodable text stays text, so the
/// facade refuses it by name as it does for Bilimbi's `decode_json`.
pub(super) fn decode(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.into()))
}

/// A row slot drawn from request-independent text.
pub(super) fn slot(row: FlexRow, column: &str, text: &str) -> FlexRow {
    row.slot(column, Html::text(text))
}

/// One posted value of a form namespace, empty when absent.
pub(super) fn pick(posted: &HashMap<String, String>, key: &str) -> String {
    posted.get(key).cloned().unwrap_or_default()
}
