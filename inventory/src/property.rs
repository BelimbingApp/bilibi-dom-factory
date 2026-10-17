//! Company-defined property definitions and the values held against them
//! (Bilimbi `Inventory.PropertyDefinition`). A material type carries a list of
//! definitions; each material of that type holds one value per definition.
//! Nothing here names a particular property: what a type measures is that
//! company's configuration.
//!
//! Definitions are stored and read back as Bilimbi stores them, a JSON array
//! of `{key, label, value_type, unit, required}`. Values are validated against
//! the definitions that own them: no unknown key, every required key present,
//! and each value of its declared type. A decimal is stored as a plain decimal
//! string at the precision it was given, so a value reads back as recorded.
use crate::error::Error;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::str::FromStr;

/// The value types a definition may declare.
pub const VALUE_TYPES: [&str; 4] = ["string", "integer", "decimal", "boolean"];
const NUMERIC_TYPES: [&str; 2] = ["integer", "decimal"];
const LABEL_LIMIT: usize = 255;
const UNIT_LIMIT: usize = 32;
const STRING_LIMIT: usize = 1000;

/// One property a type defines: a `key` (a lower-case identifier unique in
/// the type), a `label`, a `value_type` (one of [`VALUE_TYPES`]), an optional
/// `unit` label for a numeric value, and whether a value is `required`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyDefinition {
    pub key: String,
    pub label: String,
    pub value_type: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub required: bool,
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= limit
}

fn key_valid(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && key.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Normalizes definitions, refusing a malformed one, a duplicate key, or a
/// unit on a non-numeric value type. An empty list is a type with no
/// properties.
pub(crate) fn normalize(
    definitions: &[PropertyDefinition],
) -> Result<Vec<PropertyDefinition>, Error> {
    let mut normalized: Vec<PropertyDefinition> = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let unit_valid = definition.unit.as_deref().is_none_or(|unit| {
            NUMERIC_TYPES.contains(&definition.value_type.as_str()) && text(unit, UNIT_LIMIT)
        });
        if !key_valid(&definition.key)
            || !text(&definition.label, LABEL_LIMIT)
            || !VALUE_TYPES.contains(&definition.value_type.as_str())
            || !unit_valid
            || normalized.iter().any(|seen| seen.key == definition.key)
        {
            return Err(Error::InvalidPropertyDefinitions);
        }
        normalized.push(PropertyDefinition {
            key: definition.key.clone(),
            label: definition.label.trim().to_owned(),
            value_type: definition.value_type.clone(),
            unit: definition
                .unit
                .as_deref()
                .map(|unit| unit.trim().to_owned()),
            required: definition.required,
        });
    }
    Ok(normalized)
}

/// Definitions as stored: they were normalized on the way in.
pub(crate) fn stored(value: Value) -> Vec<PropertyDefinition> {
    serde_json::from_value(value).unwrap_or_default()
}
pub(crate) fn to_value(definitions: &[PropertyDefinition]) -> Value {
    serde_json::to_value(definitions).unwrap_or(Value::Array(Vec::new()))
}

/// Validates property values against their definitions; the result holds
/// only defined keys, normalized.
pub(crate) fn validate(
    definitions: &[PropertyDefinition],
    values: &Map<String, Value>,
) -> Result<Map<String, Value>, Error> {
    if values
        .keys()
        .any(|key| !definitions.iter().any(|definition| &definition.key == key))
    {
        return Err(Error::InvalidProperties);
    }
    let mut normalized = Map::new();
    for definition in definitions {
        match values.get(&definition.key) {
            None if definition.required => return Err(Error::InvalidProperties),
            None => {}
            Some(value) => {
                let value = typed(&definition.value_type, value).ok_or(Error::InvalidProperties)?;
                normalized.insert(definition.key.clone(), value);
            }
        }
    }
    Ok(normalized)
}

fn typed(value_type: &str, value: &Value) -> Option<Value> {
    match (value_type, value) {
        ("string", Value::String(text_value)) if text(text_value, STRING_LIMIT) => {
            Some(Value::String(text_value.trim().to_owned()))
        }
        ("integer", Value::Number(number)) if number.is_i64() || number.is_u64() => {
            Some(value.clone())
        }
        ("boolean", Value::Bool(_)) => Some(value.clone()),
        ("decimal", Value::Number(number)) if number.is_i64() || number.is_u64() => {
            Some(Value::String(number.to_string()))
        }
        ("decimal", Value::String(text_value)) => {
            let text_value = text_value.trim();
            Decimal::from_str(text_value)
                .or_else(|_| Decimal::from_scientific(text_value))
                .ok()
                .map(|decimal| Value::String(decimal.to_string()))
        }
        _ => None,
    }
}
