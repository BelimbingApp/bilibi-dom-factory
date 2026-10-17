//! Public values of Bilimbi.Factory.ProductDefinition; persistence stays private.
use bilibi_factory_inventory::PropertyDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Product {
    pub id: i64,
    pub company_id: i64,
    pub item_id: i64,
    pub code: String,
    pub name: String,
}
#[derive(Clone, Debug, Default)]
pub struct NewProduct {
    pub item_id: i64,
    pub code: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceType {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub name: String,
    pub property_definitions: Vec<PropertyDefinition>,
    pub retired: bool,
}
#[derive(Clone, Debug, Default)]
pub struct NewResourceType {
    pub code: String,
    pub name: String,
    pub property_definitions: Vec<PropertyDefinition>,
}
#[derive(Clone, Debug, Default)]
pub struct ResourceTypeUpdate {
    pub code: Option<String>,
    pub name: Option<String>,
    pub property_definitions: Option<Vec<PropertyDefinition>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub id: i64,
    pub company_id: i64,
    pub code: String,
    pub name: String,
    pub resource_type_id: i64,
    pub properties: Map<String, Value>,
    pub retired: bool,
}
#[derive(Clone, Debug, Default)]
pub struct NewResource {
    pub code: String,
    pub name: String,
    pub resource_type_id: i64,
    pub properties: Map<String, Value>,
}
/// None preserves a value. Properties replace, not merge; the type never changes.
#[derive(Clone, Debug, Default)]
pub struct ResourceUpdate {
    pub code: Option<String>,
    pub name: Option<String>,
    pub properties: Option<Map<String, Value>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormulaRevision {
    pub id: i64,
    pub company_id: i64,
    pub product_id: i64,
    pub version: i32,
    pub lines: Vec<FormulaLine>,
    pub process_config: Map<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormulaLine {
    pub item_id: i64,
    pub unit_id: i64,
    pub role: String,
    pub quantity: String,
    pub output_role: Option<String>,
    pub material_hold_rule: Option<Map<String, Value>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingRevision {
    pub id: i64,
    pub company_id: i64,
    pub product_id: i64,
    pub version: i32,
    pub operations: Vec<Operation>,
    pub process_config: Map<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub code: String,
    pub sequence: i64,
    pub inputs: Vec<i64>,
    pub outputs: Vec<i64>,
    pub allowed_resource_ids: Vec<i64>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedRevisions {
    pub product: Product,
    pub formula: FormulaRevision,
    pub routing: RoutingRevision,
}
