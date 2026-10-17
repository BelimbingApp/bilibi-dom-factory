//! Private floor fragments and event decoding; mirrors Bilimbi FloorLive.
use askama::Template;
use bilibi_database::{Db, Error, Result};
use bilibi_tenancy::Scope;
use bilibi_ui::{Button, ButtonVariant, Choice, Field, Html, InputKind};
use serde_json::Value;
use std::collections::HashMap;

pub(super) const TOUCH: &str = "min-h-14 px-6 text-base";
pub(super) fn render(value: impl Template) -> Result<Html> {
    Html::render(&value).map_err(|_| Error::Internal)
}
pub(super) fn company(scope: &Scope) -> Result<i64> {
    scope
        .actor()
        .organisation()
        .map(|id| id.get())
        .ok_or(Error::NotFound)
}
pub(super) async fn can(db: &Db, scope: &Scope, cap: &str) -> Result<bool> {
    let c = db.get().await?;
    Ok(bilibi_authz::decide(&**c, scope, cap).await?.allowed)
}
pub(super) fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
pub(super) fn id(value: &Value, key: &str) -> Option<i64> {
    value[key]
        .as_i64()
        .filter(|v| *v > 0)
        .or_else(|| bilibi_ui::params::positive_id(value[key].as_str()))
}
pub(super) fn form(value: &Value, name: &str) -> HashMap<String, String> {
    bilibi_ui::params::form_values(value, name)
}
pub(super) fn get<'a>(draft: &'a HashMap<String, String>, key: &str) -> &'a str {
    draft.get(key).map(String::as_str).unwrap_or("")
}
pub(super) fn field(name: &str, key: &str, label: &str, draft: &HashMap<String, String>) -> Field {
    Field::in_form(name, key)
        .id(format!("{name}-{}", key.replace('_', "-")))
        .label(label)
        .value(get(draft, key))
}
pub(super) fn select(
    name: &str,
    key: &str,
    label: &str,
    prompt: &str,
    draft: &HashMap<String, String>,
    options: Vec<Choice>,
) -> Field {
    field(name, key, label, draft)
        .kind(InputKind::Select)
        .prompt(prompt)
        .options(options)
}
pub(super) fn button(label: &str, event: &str, id: &str, primary: bool) -> Result<Html> {
    render(Button {
        button_type: "button",
        class: TOUCH,
        event,
        name: "id",
        value: id,
        body: Html::text(label),
        variant: if primary {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Secondary
        },
        ..Button::default()
    })
}
pub(super) fn limits(value: &Value) -> String {
    [("min", "minimum"), ("target", "target"), ("max", "maximum")]
        .into_iter()
        .filter_map(|(label, key)| value[key].as_str().map(|v| format!("{label} {v}")))
        .collect::<Vec<_>>()
        .join(" · ")
}
pub(super) fn typed(kind: &str, value: &str) -> Value {
    match kind {
        "integer" => value
            .trim()
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::from(value)),
        "boolean" if value == "true" || value == "false" => Value::from(value == "true"),
        _ => Value::from(value),
    }
}
pub(super) fn request() -> String {
    format!("floor:{}", jiff::Timestamp::now().as_nanosecond())
}
pub(super) fn draft(keys: &[(&str, &str)]) -> HashMap<String, String> {
    let mut result: HashMap<String, String> = keys
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    result.insert("request_id".into(), request());
    result
}

#[derive(Template)]
#[template(path = "floor.html")]
pub(super) struct PageView<'a> {
    pub(super) body: Html,
    pub(super) error: Html,
    pub(super) flash: bilibi_ui::FlashMap<'a>,
}
#[derive(Template)]
#[template(path = "floor_form.html")]
pub(super) struct FormView<'a> {
    pub(super) id: &'a str,
    pub(super) event: &'a str,
    pub(super) change: &'a str,
    pub(super) fields: Vec<Field>,
    pub(super) choices: Html,
    pub(super) after: Vec<Field>,
    pub(super) submit: &'a str,
    pub(super) cancel: &'a str,
}
#[derive(Template)]
#[template(path = "floor_choices.html")]
pub(super) struct Choices<'a> {
    pub(super) legend: &'a str,
    pub(super) buttons: Html,
    pub(super) empty: &'a str,
}
#[derive(Template)]
#[template(path = "floor_instant.html")]
pub(super) struct InstantView<'a> {
    pub(super) id: &'a str,
    pub(super) value: bilibi_ui::flex_table::Instant,
}

#[derive(Template)]
#[template(path = "floor_picker.html")]
pub(super) struct Picker<'a> {
    pub(super) id: &'a str,
    pub(super) title: &'a str,
    pub(super) actions: Html,
    pub(super) buttons: Html,
    pub(super) empty: Html,
}
