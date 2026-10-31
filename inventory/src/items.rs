//! Belimbing's item master, `commerce_inventory_items` (Bilimbi
//! `Inventory.list_items/3`, `get_item/3`, `get_item_by_sku/3`,
//! `create_item/3`, and the item half of `update_material/5`). The status
//! vocabulary and the default currency are the company's item settings; the
//! table carries no default for either.
use crate::{
    access::{readable_for, writable_for},
    contributions::{ITEM_CREATE, ITEM_LIST, ITEM_VIEW},
    error::{Error, FieldError},
    limit, settings, ItemSettings,
};
use bilibi_database::{Change, MutationTransaction};
use bilibi_settings::Settings;
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use tokio_postgres::{GenericClient, Row};

/// One item master row. `category_id` and `product_template_id` are opaque
/// references to a catalog Inventory does not own; `quantity_on_hand` and
/// `storage_location` are the item master's own location-less values, never
/// stock positions. Money amounts are minor units of `currency_code`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub company_id: i64,
    pub category_id: Option<i64>,
    pub product_template_id: Option<i64>,
    pub sku: String,
    pub status: String,
    pub title: String,
    pub description: Option<String>,
    pub quantity_on_hand: i32,
    pub storage_location: Option<String>,
    pub notes: Option<String>,
    pub unit_cost_amount: Option<i64>,
    pub target_price_amount: Option<i64>,
    pub currency_code: String,
}

/// A new item, with Belimbing's create rules: the SKU and currency are
/// upper-cased, a blank storage location is none, and the quantity defaults
/// to one. An omitted `status` is the configured vocabulary's first entry and
/// an omitted `currency_code` the configured default; without the setting,
/// the value is required. Catalog references are not accepted: Inventory
/// cannot validate a catalog it does not own.
#[derive(Clone, Debug, Default)]
pub struct NewItem {
    pub sku: String,
    pub title: String,
    pub status: Option<String>,
    pub description: Option<String>,
    pub quantity_on_hand: Option<i32>,
    pub storage_location: Option<String>,
    pub notes: Option<String>,
    pub unit_cost_amount: Option<i64>,
    pub target_price_amount: Option<i64>,
    pub currency_code: Option<String>,
}

/// The editable details of an item; `None` keeps the current value, and an
/// empty `description` or `notes` clears it. The SKU never changes.
#[derive(Clone, Debug, Default)]
pub struct ItemUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub notes: Option<String>,
    pub currency_code: Option<String>,
}

const COLUMNS: &str = "id,company_id,category_id,product_template_id,sku,status,title,description,quantity_on_hand,storage_location,notes,unit_cost_amount,target_price_amount,currency_code";
fn read(row: &Row) -> Item {
    Item {
        id: row.get("id"),
        company_id: row.get("company_id"),
        category_id: row.get("category_id"),
        product_template_id: row.get("product_template_id"),
        sku: row.get("sku"),
        status: row.get("status"),
        title: row.get("title"),
        description: row.get("description"),
        quantity_on_hand: row.get("quantity_on_hand"),
        storage_location: row.get("storage_location"),
        notes: row.get("notes"),
        unit_cost_amount: row.get("unit_cost_amount"),
        target_price_amount: row.get("target_price_amount"),
        currency_code: row.get("currency_code"),
    }
}
fn audited(item: &Item) -> Value {
    json!({
        "company_id": item.company_id, "sku": item.sku, "status": item.status, "title": item.title,
        "description": item.description, "quantity_on_hand": item.quantity_on_hand,
        "storage_location": item.storage_location, "notes": item.notes,
        "unit_cost_amount": item.unit_cost_amount, "target_price_amount": item.target_price_amount,
        "currency_code": item.currency_code,
    })
}

pub(crate) async fn list(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    status: Option<&str>,
    requested: Option<i64>,
) -> Result<Vec<Item>, Error> {
    let limit = limit(requested)?;
    readable_for(client, scope, company_id, ITEM_LIST).await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM commerce_inventory_items WHERE tenant_id=$1 AND company_id=$2 AND ($3::text IS NULL OR status=$3) ORDER BY sku LIMIT $4"
    );
    Ok(client
        .query(&sql, &[&scope.tenant(), &company_id, &status, &limit])
        .await?
        .iter()
        .map(read)
        .collect())
}

pub(crate) async fn get(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    item_id: i64,
) -> Result<Item, Error> {
    readable_for(client, scope, company_id, ITEM_VIEW).await?;
    fetch(client, scope, company_id, item_id, false).await
}

/// SKUs are stored upper-case, so the lookup is too.
pub(crate) async fn get_by_sku(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    sku: &str,
) -> Result<Item, Error> {
    readable_for(client, scope, company_id, ITEM_VIEW).await?;
    let sql = format!(
        "SELECT {COLUMNS} FROM commerce_inventory_items WHERE tenant_id=$1 AND company_id=$2 AND sku=$3"
    );
    client
        .query_opt(
            &sql,
            &[&scope.tenant(), &company_id, &sku.trim().to_uppercase()],
        )
        .await?
        .as_ref()
        .map(read)
        .ok_or(Error::ItemNotFound)
}

pub(crate) async fn fetch(
    client: &(impl GenericClient + Sync),
    scope: &Scope,
    company_id: i64,
    item_id: i64,
    lock: bool,
) -> Result<Item, Error> {
    let sql = format!(
        "SELECT {COLUMNS} FROM commerce_inventory_items WHERE tenant_id=$1 AND company_id=$2 AND id=$3{}",
        if lock { " FOR UPDATE" } else { "" }
    );
    client
        .query_opt(&sql, &[&scope.tenant(), &company_id, &item_id])
        .await?
        .as_ref()
        .map(read)
        .ok_or(Error::ItemNotFound)
}

fn error(errors: &mut Vec<FieldError>, field: &'static str, message: impl Into<String>) {
    errors.push(FieldError {
        field,
        message: message.into(),
    });
}
/// A value the caller gave, trimmed; blank is none (Ecto's empty values).
fn given(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
fn length(errors: &mut Vec<FieldError>, field: &'static str, value: Option<&str>, max: usize) {
    if value.is_some_and(|value| value.chars().count() > max) {
        error(
            errors,
            field,
            format!("should be at most {max} character(s)"),
        );
    }
}
fn status_errors(errors: &mut Vec<FieldError>, status: Option<&str>, settings: &ItemSettings) {
    match (status, &settings.statuses) {
        (None, _) => error(errors, "status", "can't be blank"),
        (Some(status), Some(statuses)) if !statuses.iter().any(|known| known == status) => {
            error(errors, "status", "is invalid");
        }
        (Some(status), _) => length(errors, "status", Some(status), 255),
    }
}
fn currency_errors(errors: &mut Vec<FieldError>, currency: Option<&str>) {
    match currency {
        None => error(errors, "currency_code", "can't be blank"),
        Some(code) if code.chars().count() != 3 => {
            error(errors, "currency_code", "should be 3 character(s)");
        }
        Some(code) if !code.bytes().all(|byte| byte.is_ascii_uppercase()) => {
            error(errors, "currency_code", "is invalid");
        }
        Some(_) => {}
    }
}
fn title_errors(errors: &mut Vec<FieldError>, title: Option<&str>) {
    match title {
        None => error(errors, "title", "can't be blank"),
        title => length(errors, "title", title, 255),
    }
}

/// Checks a new item against Belimbing's create rules and the company's item
/// settings, deciding every refusal before the write.
pub(crate) async fn validate_new(
    tx: &mut MutationTransaction<'_>,
    store: &Settings,
    scope: &Scope,
    company_id: i64,
    new: &NewItem,
) -> Result<NewItem, Error> {
    writable_for(tx.sql(), scope, company_id, ITEM_CREATE).await?;
    let settings = settings::effective(tx.sql(), store, scope, company_id).await?;
    let sku = given(Some(&new.sku)).map(|sku| sku.to_uppercase());
    let title = given(Some(&new.title)).map(|_| new.title.clone());
    let status = given(new.status.as_ref()).or_else(|| {
        settings
            .statuses
            .as_ref()
            .and_then(|statuses| statuses.first().cloned())
    });
    let currency = given(new.currency_code.as_ref())
        .map(|code| code.to_uppercase())
        .or_else(|| settings.default_currency_code.clone());
    let quantity = new.quantity_on_hand.unwrap_or(1);
    let mut errors = Vec::new();
    match &sku {
        None => error(&mut errors, "sku", "can't be blank"),
        Some(sku) if sku.chars().count() > 64 => {
            error(&mut errors, "sku", "should be at most 64 character(s)");
        }
        Some(sku) => {
            if tx
                .sql()
                .query_opt(
                    "SELECT 1 FROM commerce_inventory_items WHERE company_id=$1 AND sku=$2",
                    &[&company_id, sku],
                )
                .await?
                .is_some()
            {
                error(&mut errors, "sku", "has already been taken");
            }
        }
    }
    status_errors(&mut errors, status.as_deref(), &settings);
    title_errors(&mut errors, title.as_deref());
    if !(0..=999_999).contains(&quantity) {
        error(
            &mut errors,
            "quantity_on_hand",
            if quantity < 0 {
                "must be greater than or equal to 0"
            } else {
                "must be less than or equal to 999999"
            },
        );
    }
    let storage_location = given(new.storage_location.as_ref());
    length(
        &mut errors,
        "storage_location",
        storage_location.as_deref(),
        255,
    );
    let notes = new.notes.clone().filter(|notes| !notes.is_empty());
    length(&mut errors, "notes", notes.as_deref(), 5000);
    for (field, amount) in [
        ("unit_cost_amount", new.unit_cost_amount),
        ("target_price_amount", new.target_price_amount),
    ] {
        if amount.is_some_and(|amount| amount < 0) {
            error(&mut errors, field, "must be greater than or equal to 0");
        }
    }
    currency_errors(&mut errors, currency.as_deref());
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }
    Ok(NewItem {
        sku: sku.unwrap_or_default(),
        title: title.unwrap_or_default(),
        status,
        description: new.description.clone().filter(|text| !text.is_empty()),
        quantity_on_hand: Some(quantity),
        storage_location,
        notes,
        unit_cost_amount: new.unit_cost_amount,
        target_price_amount: new.target_price_amount,
        currency_code: currency,
    })
}

/// Inserts an item `validate_new` accepted, returning it and its `Change`.
pub(crate) async fn insert(
    sql: &tokio_postgres::Transaction<'_>,
    tenant: i64,
    company_id: i64,
    item: &NewItem,
) -> Result<(Item, Change), bilibi_database::Error> {
    let row = sql
        .query_one(
            &format!("INSERT INTO commerce_inventory_items(tenant_id,company_id,sku,status,title,description,quantity_on_hand,storage_location,notes,unit_cost_amount,target_price_amount,currency_code) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING {COLUMNS}"),
            &[&tenant, &company_id, &item.sku, &item.status, &item.title, &item.description, &item.quantity_on_hand, &item.storage_location, &item.notes, &item.unit_cost_amount, &item.target_price_amount, &item.currency_code],
        )
        .await?;
    let item = read(&row);
    let change = Change {
        kind: "commerce_inventory_items".into(),
        id: item.id,
        old: json!({}),
        new: audited(&item),
    };
    Ok((item, change))
}

pub(crate) async fn create(
    tx: &mut MutationTransaction<'_>,
    store: &Settings,
    scope: &Scope,
    company_id: i64,
    new: &NewItem,
) -> Result<Item, Error> {
    let item = validate_new(tx, store, scope, company_id, new).await?;
    let tenant = scope.tenant();
    Ok(tx
        .mutate(move |sql| {
            Box::pin(async move {
                let (item, change) = insert(sql, tenant, company_id, &item).await?;
                Ok((item, vec![change]))
            })
        })
        .await?)
}

/// Applies an update to `before`, refusing what Belimbing's update rules
/// refuse; the caller holds the row lock and the capability.
pub(crate) fn updated(
    before: &Item,
    update: &ItemUpdate,
    settings: &ItemSettings,
) -> Result<Item, Error> {
    let mut item = before.clone();
    if let Some(title) = &update.title {
        item.title = title.clone();
    }
    if let Some(status) = &update.status {
        item.status = status.trim().to_owned();
    }
    if let Some(code) = &update.currency_code {
        item.currency_code = code.trim().to_uppercase();
    }
    if let Some(description) = &update.description {
        item.description = Some(description.clone()).filter(|text| !text.is_empty());
    }
    if let Some(notes) = &update.notes {
        item.notes = Some(notes.clone()).filter(|text| !text.is_empty());
    }
    let mut errors = Vec::new();
    title_errors(
        &mut errors,
        Some(item.title.as_str()).filter(|title| !title.trim().is_empty()),
    );
    length(&mut errors, "notes", item.notes.as_deref(), 5000);
    status_errors(
        &mut errors,
        Some(item.status.as_str()).filter(|status| !status.is_empty()),
        settings,
    );
    currency_errors(
        &mut errors,
        Some(item.currency_code.as_str()).filter(|code| !code.is_empty()),
    );
    if errors.is_empty() {
        Ok(item)
    } else {
        Err(Error::Invalid(errors))
    }
}

/// Writes an accepted update, returning the item and its `Change`.
pub(crate) async fn write_update(
    sql: &tokio_postgres::Transaction<'_>,
    before: &Item,
    item: &Item,
) -> Result<(Item, Change), bilibi_database::Error> {
    let row = sql
        .query_one(
            &format!("UPDATE commerce_inventory_items SET title=$2,description=$3,status=$4,notes=$5,currency_code=$6,updated_at=now() WHERE id=$1 RETURNING {COLUMNS}"),
            &[&item.id, &item.title, &item.description, &item.status, &item.notes, &item.currency_code],
        )
        .await?;
    let item = read(&row);
    let change = Change {
        kind: "commerce_inventory_items".into(),
        id: item.id,
        old: audited(before),
        new: audited(&item),
    };
    Ok((item, change))
}
