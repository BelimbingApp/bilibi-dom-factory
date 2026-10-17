//! `/factory/resources`: company resource administration, mirroring Bilimbi
//! `ProductDefinition.Web.ResourcesLive`.
use super::shared::{self as s, Notice, PageView, Result};
use super::Pages;
use crate::{contributions::MANAGE, NewResource, Resource, ResourceType, ResourceUpdate};
use bilibi_tenancy::Scope;
use bilibi_ui::{
    Button, ButtonVariant, Choice, ConfirmDialog, Field, FlexRow, Html, InputKind, ListColumns,
};
use serde_json::{Map, Value};

const DENIED: &str = "You do not have permission to manage resources.";
const FORM: &str = "resource";

#[derive(Clone, Copy, PartialEq)]
enum Editing {
    New,
    Resource(i64),
}

#[derive(Clone, Default)]
struct Draft {
    code: String,
    name: String,
    resource_type_id: String,
    properties: String,
}
impl Draft {
    fn blank() -> Self {
        Self {
            properties: "{}".into(),
            ..Self::default()
        }
    }
}

pub(super) struct Resources {
    pages: Pages,
    columns: ListColumns,
    resources: Vec<Resource>,
    types: Vec<ResourceType>,
    editing: Option<Editing>,
    draft: Draft,
    pending_retire: Option<i64>,
    can_manage: bool,
    notice: Notice,
}

impl Resources {
    pub(super) fn new(pages: &Pages) -> Result<Self> {
        Ok(Self {
            pages: pages.clone(),
            columns: s::mount_columns(
                "resources-table",
                vec![
                    s::column("code", "Code"),
                    s::column("name", "Name"),
                    s::column("type", "Type"),
                    s::column("state", "State"),
                ],
            )?,
            resources: Vec::new(),
            types: Vec::new(),
            editing: None,
            draft: Draft::blank(),
            pending_retire: None,
            can_manage: false,
            notice: Notice::default(),
        })
    }

    async fn load(&mut self, scope: &Scope) -> Result<()> {
        self.can_manage = s::can(&self.pages.db, scope, MANAGE).await?;
        self.resources = Vec::new();
        self.types = Vec::new();
        match s::company(scope) {
            Some(company) => {
                let facade = &self.pages.definitions;
                let resources = facade.list_resources(scope, company, None).await;
                let types = facade.list_resource_types(scope, company, None).await;
                match (s::refusal(resources)?, s::refusal(types)?) {
                    (Ok(resources), Ok(types)) => {
                        self.resources = resources;
                        self.types = types;
                    }
                    (Err(message), _) | (_, Err(message)) => self.notice.fail(message),
                }
            }
            None => self.notice.fail("Company not found"),
        }
        self.columns.load(&self.resources, |r| r.id.to_string());
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
            "new" | "edit" | "save_resource" | "request_retire" | "retire" => {}
            _ => return Err(bilibi_database::Error::Invalid),
        }
        if !s::can(&self.pages.db, scope, MANAGE).await? {
            self.pending_retire = None;
            self.notice.fail(DENIED);
            return self.load(scope).await;
        }
        let company = s::company(scope);
        match kind {
            "new" => {
                self.editing = Some(Editing::New);
                self.draft = Draft::blank();
                self.notice.error = None;
            }
            "edit" => self.edit(scope, company, payload).await?,
            "save_resource" => self.save(scope, company, payload).await?,
            "request_retire" => match s::id(payload) {
                Some(id) if self.resources.iter().any(|r| r.id == id && !r.retired) => {
                    self.pending_retire = Some(id);
                }
                _ => self.notice.fail("Resource not found"),
            },
            _ => self.retire(scope, company).await?,
        }
        self.load(scope).await
    }

    async fn edit(&mut self, scope: &Scope, company: Option<i64>, payload: &Value) -> Result<()> {
        let (Some(company), Some(id)) = (company, s::id(payload)) else {
            self.notice.fail("Resource not found");
            return Ok(());
        };
        let found = self
            .pages
            .definitions
            .get_resource(scope, company, id)
            .await;
        match s::refusal(found)? {
            Ok(resource) => {
                self.editing = Some(Editing::Resource(resource.id));
                self.draft = Draft {
                    code: resource.code,
                    name: resource.name,
                    resource_type_id: resource.resource_type_id.to_string(),
                    properties: s::json(&Value::Object(resource.properties), true),
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
            resource_type_id: s::pick(&posted, "resource_type_id"),
            properties: s::pick(&posted, "properties"),
        };
        let Some(company) = company else {
            self.draft = draft;
            self.notice.fail("Company not found");
            return Ok(());
        };
        // Bilimbi hands undecodable text to the facade, which refuses it as
        // an invalid property map; so does a value that is not an object.
        let properties: Option<Map<String, Value>> = serde_json::from_str(&draft.properties).ok();
        let Some(properties) = properties else {
            self.draft = draft;
            self.notice.fail("Properties is invalid");
            return Ok(());
        };
        let facade = &self.pages.definitions;
        let done = match self.editing {
            Some(Editing::New) => match draft.resource_type_id.trim().parse::<i64>() {
                Ok(resource_type_id) => {
                    let new = NewResource {
                        code: draft.code.clone(),
                        name: draft.name.clone(),
                        resource_type_id,
                        properties,
                    };
                    facade.create_resource(scope, company, &new).await
                }
                Err(_) => Err(crate::Error::ResourceTypeNotFound),
            },
            Some(Editing::Resource(id)) => {
                let update = ResourceUpdate {
                    code: Some(draft.code.clone()),
                    name: Some(draft.name.clone()),
                    properties: Some(properties),
                };
                facade.update_resource(scope, company, id, &update).await
            }
            None => Err(crate::Error::ResourceNotFound),
        };
        match s::refusal(done)? {
            Ok(_) => {
                self.editing = None;
                self.notice.error = None;
                self.notice.success("Resource saved.");
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
            self.notice.fail("Resource not found");
            return Ok(());
        };
        let done = self
            .pages
            .definitions
            .retire_resource(scope, company, id)
            .await;
        match s::refusal(done)? {
            Ok(_) => self.notice.success("Resource retired."),
            Err(message) => self.notice.fail(message),
        }
        Ok(())
    }

    fn rows(&self) -> Result<Vec<FlexRow>> {
        let mut rows = Vec::with_capacity(self.resources.len());
        for resource in &self.resources {
            let type_name = self
                .types
                .iter()
                .find(|t| t.id == resource.resource_type_id)
                .map_or("—", |t| t.name.as_str());
            let mut row = FlexRow::new(resource.id.to_string());
            row.dom_id = Some(format!("resource-{}", resource.id));
            row = s::slot(row, "code", &resource.code);
            row = s::slot(row, "name", &resource.name);
            row = s::slot(row, "type", type_name);
            row = s::slot(
                row,
                "state",
                if resource.retired {
                    "Retired"
                } else {
                    "Active"
                },
            );
            if self.can_manage && !resource.retired {
                let id = resource.id.to_string();
                let mut actions = s::render(Button {
                    id: &format!("resource-{}-edit", resource.id),
                    button_type: "button",
                    event: "edit",
                    name: "id",
                    value: &id,
                    body: Html::text("Edit"),
                    ..Button::default()
                })?;
                actions.push(&s::render(Button {
                    id: &format!("resource-{}-retire", resource.id),
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
        let mut kind: Field = s::text_field(
            FORM,
            "resource_type_id",
            "Resource type",
            &self.draft.resource_type_id,
        )
        .kind(InputKind::Select)
        .prompt("Choose a type")
        .options(
            self.types
                .iter()
                .filter(|t| !t.retired)
                .map(|t| Choice::new(t.name.clone(), t.id.to_string()))
                .collect(),
        );
        if editing != Editing::New {
            kind = kind.disabled();
        }
        let form = s::render(s::FormCard {
            form_id: "resource-form",
            event: "save_resource",
            fields: vec![
                s::text_field(FORM, "code", "Code", &self.draft.code),
                s::text_field(FORM, "name", "Name", &self.draft.name),
                kind,
                s::textarea_field(
                    FORM,
                    "properties",
                    "Property values (JSON)",
                    "Keys and value types are defined by the selected resource type.",
                    &self.draft.properties,
                ),
            ],
            submit: "Save",
            cancel: true,
        })?;
        s::render(bilibi_ui::Card {
            id: "resource-editor",
            title: if editing == Editing::New {
                "New resource"
            } else {
                "Resource"
            },
            class: "mt-5",
            body: form,
            ..bilibi_ui::Card::default()
        })
    }

    pub(super) fn view(&self) -> Result<String> {
        let table = s::table(
            "resources-table",
            "grid",
            "Resources",
            &self.columns,
            self.rows()?,
            true,
            (
                "No resources",
                "Create a resource type, then register a resource.",
            ),
        )?;
        let mut body = self.notice.alert("resource-error")?;
        body.push(&self.editor()?);
        body.push(&s::render(bilibi_ui::Card {
            id: "resources",
            inner_class: "p-0",
            class: "mt-5",
            body: table,
            ..bilibi_ui::Card::default()
        })?);
        let mut actions = s::render(Button {
            id: "resource-types-link",
            href: "/factory/resource-types",
            body: Html::text("Resource types"),
            ..Button::default()
        })?;
        if self.can_manage {
            actions.push(&Html::raw(" "));
            actions.push(&s::render(Button {
                variant: ButtonVariant::Primary,
                id: "new-resource",
                button_type: "button",
                event: "new",
                body: Html::text("New resource"),
                ..Button::default()
            })?);
        }
        let dialog = match self.pending_retire {
            Some(_) => s::render(ConfirmDialog {
                id: "resource-retire-confirm",
                consequence: "Retire this resource?",
                detail:
                    "A retired resource cannot be edited, and no routing can allow it afterwards.",
                confirm: "Retire",
                working: "Retiring…",
                confirm_event: "retire",
                cancel_event: "cancel_retire",
            })?,
            None => Html::EMPTY,
        };
        Ok(s::render(PageView {
            flash: self.notice.flash_map(),
            id: "resources-index",
            title: "Resources",
            subtitle: "Register a resource against a company type and its property definitions.",
            actions,
            body,
            dialog,
        })?
        .as_str()
        .to_owned())
    }
}
