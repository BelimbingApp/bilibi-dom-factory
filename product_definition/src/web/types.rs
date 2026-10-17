//! `/factory/resource-types`: company resource type administration, mirroring
//! Bilimbi `ProductDefinition.Web.ResourceTypesLive`.
use super::shared::{self as s, Notice, PageView, Result};
use super::Pages;
use crate::{contributions::MANAGE, NewResourceType, ResourceType, ResourceTypeUpdate};
use bilibi_factory_inventory::PropertyDefinition;
use bilibi_tenancy::Scope;
use bilibi_ui::{Button, ButtonVariant, ConfirmDialog, FlexRow, Html, ListColumns};
use serde_json::{json, Value};

const DENIED: &str = "You do not have permission to manage resource types.";
const FORM: &str = "type";

#[derive(Clone, Copy, PartialEq)]
enum Editing {
    New,
    Type(i64),
}

#[derive(Clone)]
struct Draft {
    code: String,
    name: String,
    property_definitions: String,
}
impl Default for Draft {
    fn default() -> Self {
        Self {
            code: String::new(),
            name: String::new(),
            property_definitions: "[]".into(),
        }
    }
}

pub(super) struct Types {
    pages: Pages,
    columns: ListColumns,
    types: Vec<ResourceType>,
    editing: Option<Editing>,
    draft: Draft,
    pending_retire: Option<i64>,
    can_manage: bool,
    notice: Notice,
}

impl Types {
    pub(super) fn new(pages: &Pages) -> Result<Self> {
        Ok(Self {
            pages: pages.clone(),
            columns: s::mount_columns(
                "resource-types-table",
                vec![
                    s::column("code", "Code"),
                    s::column("name", "Name"),
                    s::count_column("properties", "Properties"),
                    s::column("state", "State"),
                ],
            )?,
            types: Vec::new(),
            editing: None,
            draft: Draft::default(),
            pending_retire: None,
            can_manage: false,
            notice: Notice::default(),
        })
    }

    /// Reads the list and the actor's standing; the columns forget an
    /// unfinished add box only when the rows are replaced.
    async fn load(&mut self, scope: &Scope) -> Result<()> {
        self.can_manage = s::can(&self.pages.db, scope, MANAGE).await?;
        self.types = Vec::new();
        match s::company(scope) {
            Some(company) => {
                let listed = self
                    .pages
                    .definitions
                    .list_resource_types(scope, company, None)
                    .await;
                match s::refusal(listed)? {
                    Ok(types) => self.types = types,
                    Err(message) => self.notice.fail(message),
                }
            }
            None => self.notice.fail("Company not found"),
        }
        self.columns.load(&self.types, |t| t.id.to_string());
        Ok(())
    }

    pub(super) async fn refresh(&mut self, scope: &Scope) -> Result<()> {
        self.load(scope).await
    }

    pub(super) async fn event(&mut self, scope: &Scope, payload: &Value) -> Result<()> {
        self.notice.begin();
        let kind = s::text(payload, "type").ok_or(bilibi_database::Error::Invalid)?;
        match kind {
            "grid" => {
                s::grid(&mut self.columns, payload);
                return Ok(());
            }
            "cancel" => {
                self.editing = None;
                self.notice.error = None;
                return Ok(());
            }
            "cancel_retire" => {
                self.pending_retire = None;
                return Ok(());
            }
            "new" | "edit" | "save_type" | "request_retire" | "retire" => {}
            _ => return Err(bilibi_database::Error::Invalid),
        }
        // Every write-shaped event asks again; the facade asks once more.
        if !s::can(&self.pages.db, scope, MANAGE).await? {
            self.pending_retire = None;
            self.notice.fail(DENIED);
            return self.load(scope).await;
        }
        let company = s::company(scope);
        match kind {
            "new" => {
                self.editing = Some(Editing::New);
                self.draft = Draft::default();
                self.notice.error = None;
            }
            "edit" => self.edit(scope, company, payload).await?,
            "save_type" => self.save(scope, company, payload).await?,
            "request_retire" => match s::id(payload) {
                Some(id) if self.types.iter().any(|t| t.id == id && !t.retired) => {
                    self.pending_retire = Some(id);
                }
                _ => self.notice.fail("Resource type not found"),
            },
            _ => self.retire(scope, company).await?,
        }
        self.load(scope).await
    }

    async fn edit(&mut self, scope: &Scope, company: Option<i64>, payload: &Value) -> Result<()> {
        let (Some(company), Some(id)) = (company, s::id(payload)) else {
            self.notice.fail("Resource type not found");
            return Ok(());
        };
        let found = self
            .pages
            .definitions
            .get_resource_type(scope, company, id)
            .await;
        match s::refusal(found)? {
            Ok(ty) => {
                self.editing = Some(Editing::Type(ty.id));
                self.draft = Draft {
                    code: ty.code,
                    name: ty.name,
                    property_definitions: s::json(&json!(ty.property_definitions), true),
                };
                self.notice.error = None;
            }
            Err(message) => self.notice.fail(message),
        }
        Ok(())
    }

    async fn save(&mut self, scope: &Scope, company: Option<i64>, payload: &Value) -> Result<()> {
        let posted = s::form(payload, FORM);
        let draft = Draft {
            code: s::pick(&posted, "code"),
            name: s::pick(&posted, "name"),
            property_definitions: s::pick(&posted, "property_definitions"),
        };
        let Some(company) = company else {
            self.draft = draft;
            self.notice.fail("Company not found");
            return Ok(());
        };
        let definitions: Option<Vec<PropertyDefinition>> =
            serde_json::from_str(&draft.property_definitions).ok();
        let Some(definitions) = definitions else {
            self.draft = draft;
            self.notice.fail("Property definitions is invalid");
            return Ok(());
        };
        let done = match self.editing {
            Some(Editing::New) => {
                let new = NewResourceType {
                    code: draft.code.clone(),
                    name: draft.name.clone(),
                    property_definitions: definitions,
                };
                self.pages
                    .definitions
                    .create_resource_type(scope, company, &new)
                    .await
            }
            Some(Editing::Type(id)) => {
                let update = ResourceTypeUpdate {
                    code: Some(draft.code.clone()),
                    name: Some(draft.name.clone()),
                    property_definitions: Some(definitions),
                };
                self.pages
                    .definitions
                    .update_resource_type(scope, company, id, &update)
                    .await
            }
            None => {
                self.notice.fail("Resource type not found");
                return Ok(());
            }
        };
        match s::refusal(done)? {
            Ok(_) => {
                self.editing = None;
                self.notice.error = None;
                self.notice.success("Resource type saved.");
            }
            Err(message) => {
                self.draft = draft;
                self.notice.fail(message);
            }
        }
        Ok(())
    }

    async fn retire(&mut self, scope: &Scope, company: Option<i64>) -> Result<()> {
        let (Some(company), Some(id)) = (company, self.pending_retire.take()) else {
            self.notice.fail("Resource type not found");
            return Ok(());
        };
        let done = self
            .pages
            .definitions
            .retire_resource_type(scope, company, id)
            .await;
        match s::refusal(done)? {
            Ok(_) => self.notice.success("Resource type retired."),
            Err(message) => self.notice.fail(message),
        }
        Ok(())
    }

    fn rows(&self) -> Result<Vec<FlexRow>> {
        let mut rows = Vec::with_capacity(self.types.len());
        for ty in &self.types {
            let mut row = FlexRow::new(ty.id.to_string());
            row.dom_id = Some(format!("resource-type-{}", ty.id));
            row = s::slot(row, "code", &ty.code);
            row = s::slot(row, "name", &ty.name);
            row = s::slot(
                row,
                "properties",
                &ty.property_definitions.len().to_string(),
            );
            row = s::slot(row, "state", if ty.retired { "Retired" } else { "Active" });
            if self.can_manage && !ty.retired {
                let id = ty.id.to_string();
                let mut actions = s::render(Button {
                    id: &format!("resource-type-{}-edit", ty.id),
                    button_type: "button",
                    event: "edit",
                    name: "id",
                    value: &id,
                    body: Html::text("Edit"),
                    ..Button::default()
                })?;
                actions.push(&s::render(Button {
                    id: &format!("resource-type-{}-retire", ty.id),
                    button_type: "button",
                    event: "request_retire",
                    name: "id",
                    value: &id,
                    body: Html::text("Retire"),
                    ..Button::default()
                })?);
                row.actions = actions;
            }
            rows.push(row);
        }
        Ok(rows)
    }

    fn editor(&self) -> Result<Html> {
        let Some(editing) = self.editing else {
            return Ok(Html::EMPTY);
        };
        let form = s::render(s::FormCard {
            form_id: "resource-type-form",
            event: "save_type",
            fields: vec![
                s::text_field(FORM, "code", "Code", &self.draft.code),
                s::text_field(FORM, "name", "Name", &self.draft.name),
                s::textarea_field(
                    FORM,
                    "property_definitions",
                    "Property definitions (JSON)",
                    "Each definition has key, label, value_type, required, and optional unit.",
                    &self.draft.property_definitions,
                ),
            ],
            submit: "Save",
            cancel: true,
        })?;
        s::render(bilibi_ui::Card {
            id: "resource-type-editor",
            title: if editing == Editing::New {
                "New resource type"
            } else {
                "Resource type"
            },
            class: "mt-5",
            body: form,
            ..bilibi_ui::Card::default()
        })
    }

    pub(super) fn view(&self) -> Result<String> {
        let table = s::table(
            "resource-types-table",
            "grid",
            "Resource types",
            &self.columns,
            self.rows()?,
            true,
            (
                "No resource types",
                "Create a type to define its properties.",
            ),
        )?;
        let mut body = self.notice.alert("resource-type-error")?;
        body.push(&self.editor()?);
        body.push(&s::render(bilibi_ui::Card {
            id: "resource-types",
            inner_class: "p-0",
            class: "mt-5",
            body: table,
            ..bilibi_ui::Card::default()
        })?);
        let actions = if self.can_manage {
            s::render(Button {
                variant: ButtonVariant::Primary,
                id: "new-resource-type",
                button_type: "button",
                event: "new",
                body: Html::text("New type"),
                ..Button::default()
            })?
        } else {
            Html::EMPTY
        };
        let dialog = match self.pending_retire {
            Some(_) => s::render(ConfirmDialog {
                id: "resource-type-retire-confirm",
                consequence: "Retire this resource type?",
                detail:
                    "A retired resource type cannot be edited, and no resource can be added to it.",
                confirm: "Retire",
                working: "Retiring…",
                confirm_event: "retire",
                cancel_event: "cancel_retire",
            })?,
            None => Html::EMPTY,
        };
        Ok(s::render(PageView {
            flash: self.notice.flash_map(),
            id: "resource-types-index",
            title: "Resource types",
            subtitle: "Define the properties that a company's resources record.",
            actions,
            body,
            dialog,
        })?
        .as_str()
        .to_owned())
    }
}
