//! Product Definition's pages (Bilimbi `ProductDefinition.Web.*Live`): resource
//! types, resources, and BOM formulas & routings. Each is a live screen over the
//! facade; the route guard checks the view capability on every request and live
//! event, each write-shaped event asks for manage again, and the facade asks a
//! last time inside its transaction.
mod definitions;
mod resources;
mod shared;
mod types;

use crate::ProductDefinition;
use bilibi_database::{Db, Error, Result};
use bilibi_factory_inventory::Inventory;
use bilibi_tenancy::Scope;
use bilibi_ui::page::{
    LiveBody, LiveFuture, LiveInput, LiveRequest, LiveState, MountFuture, Page, PageBody,
    PageFuture, PageRequest,
};
use std::sync::Arc;

/// The page adapter; every field is shared by the mounted screens.
#[derive(Clone)]
pub(crate) struct Pages {
    pub(crate) db: Db,
    pub(crate) definitions: Arc<ProductDefinition>,
    pub(crate) inventory: Arc<Inventory>,
}

enum Screen {
    Types(types::Types),
    Resources(resources::Resources),
    Definitions(definitions::Definitions),
}

impl Screen {
    fn open(pages: &Pages, operation: &str) -> Result<Self> {
        match operation {
            "resource-types" => Ok(Self::Types(types::Types::new(pages)?)),
            "resources" => Ok(Self::Resources(resources::Resources::new(pages)?)),
            "definitions" => Ok(Self::Definitions(definitions::Definitions::new(pages)?)),
            _ => Err(Error::NotFound),
        }
    }

    async fn refresh(&mut self, scope: &Scope) -> Result<()> {
        match self {
            Self::Types(screen) => screen.refresh(scope).await,
            Self::Resources(screen) => screen.refresh(scope).await,
            Self::Definitions(screen) => screen.refresh(scope).await,
        }
    }

    async fn event(&mut self, scope: &Scope, payload: &serde_json::Value) -> Result<()> {
        match self {
            Self::Types(screen) => screen.event(scope, payload).await,
            Self::Resources(screen) => screen.event(scope, payload).await,
            Self::Definitions(screen) => screen.event(scope, payload).await,
        }
    }

    fn view(&self) -> Result<String> {
        match self {
            Self::Types(screen) => screen.view(),
            Self::Resources(screen) => screen.view(),
            Self::Definitions(screen) => screen.view(),
        }
    }
}

impl Page<Scope, Error> for Pages {
    fn title(&self, operation: &str) -> Option<&'static str> {
        match operation {
            "resource-types" => Some("Resource types"),
            "resources" => Some("Resources"),
            "definitions" => Some("BOM formulas & routings"),
            _ => None,
        }
    }

    fn render<'a>(&'a self, request: PageRequest<'a, Scope>) -> PageFuture<'a, Error> {
        Box::pin(async move {
            let scope = request.scope.ok_or(Error::Unauthorized)?;
            let mut screen = Screen::open(self, request.operation)?;
            screen.refresh(scope).await?;
            Ok(PageBody {
                html: screen.view()?,
                live: true,
                title: None,
            })
        })
    }

    fn mount<'a>(&'a self, request: PageRequest<'a, Scope>) -> MountFuture<'a, Scope, Error> {
        Box::pin(async move {
            let state: Box<dyn LiveState<Scope, Error>> =
                Box::new(Live(Screen::open(self, request.operation)?));
            Ok(Some(state))
        })
    }
}

struct Live(Screen);

impl LiveState<Scope, Error> for Live {
    fn update<'a>(&'a mut self, request: LiveRequest<'a, Scope>) -> LiveFuture<'a, Error> {
        Box::pin(async move {
            match request.input {
                LiveInput::Refresh | LiveInput::Params(_) => self.0.refresh(request.scope).await?,
                LiveInput::Event(payload) => self.0.event(request.scope, payload).await?,
            }
            Ok(LiveBody {
                html: self.0.view()?,
                ..LiveBody::default()
            })
        })
    }
}
