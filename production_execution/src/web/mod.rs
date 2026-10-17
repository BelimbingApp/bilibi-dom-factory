//! Bilimbi `ProductionExecution.Web.FloorLive`, behind one page adapter.
mod codes;
mod floor;
mod labour;
mod shared;
mod time;
mod view;

use bilibi_database::{Db, Error};
use bilibi_datetime::DisplayPolicy;
use bilibi_factory_inventory::Inventory;
use bilibi_tenancy::Scope;
use bilibi_ui::page::{
    LiveBody, LiveFuture, LiveInput, LiveRequest, LiveState, MountFuture, Page, PageBody,
    PageFuture, PageRequest,
};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Pages {
    pub(crate) db: Db,
    pub(crate) execution: Arc<crate::ProductionExecution>,
    pub(crate) inventory: Arc<Inventory>,
    pub(crate) display: Arc<DisplayPolicy>,
    pub(crate) locales: Arc<bilibi_locale::Locales>,
}
impl Page<Scope, Error> for Pages {
    fn title(&self, operation: &str) -> Option<&'static str> {
        if operation == "floor" {
            Some("Shop floor")
        } else {
            codes::Kind::for_operation(operation)
                .ok()
                .map(|kind| kind.title())
        }
    }
    fn render<'a>(&'a self, request: PageRequest<'a, Scope>) -> PageFuture<'a, Error> {
        Box::pin(async move {
            let scope = request.scope.ok_or(Error::Unauthorized)?;
            if request.operation != "floor" {
                let mut codes = codes::Codes::new(self, request.operation)?;
                codes.load(scope).await?;
                return Ok(PageBody {
                    html: codes.view()?,
                    live: true,
                    title: None,
                });
            }
            let mut floor = floor::Floor::new(self)?;
            floor.select(scope, request.query).await?;
            Ok(PageBody {
                html: floor.view()?,
                live: true,
                title: None,
            })
        })
    }
    fn mount<'a>(&'a self, request: PageRequest<'a, Scope>) -> MountFuture<'a, Scope, Error> {
        Box::pin(async move {
            let scope = request.scope.ok_or(Error::Unauthorized)?;
            if request.operation != "floor" {
                let mut codes = codes::Codes::new(self, request.operation)?;
                codes.load(scope).await?;
                return Ok(Some(Box::new(codes) as Box<dyn LiveState<Scope, Error>>));
            }
            let mut floor = floor::Floor::new(self)?;
            floor.select(scope, request.query).await?;
            Ok(Some(Box::new(floor) as Box<dyn LiveState<Scope, Error>>))
        })
    }
}
impl LiveState<Scope, Error> for floor::Floor {
    fn update<'a>(&'a mut self, request: LiveRequest<'a, Scope>) -> LiveFuture<'a, Error> {
        Box::pin(async move {
            match request.input {
                LiveInput::Params(query) => self.select(request.scope, query).await?,
                LiveInput::Refresh => self.load(request.scope).await?,
                LiveInput::Event(payload) => self.event(request.scope, payload).await?,
            }
            Ok(LiveBody {
                html: self.view()?,
                query: self.query(),
                ..LiveBody::default()
            })
        })
    }
}

impl LiveState<Scope, Error> for codes::Codes {
    fn update<'a>(&'a mut self, request: LiveRequest<'a, Scope>) -> LiveFuture<'a, Error> {
        Box::pin(async move {
            match request.input {
                LiveInput::Params(_) | LiveInput::Refresh => self.load(request.scope).await?,
                LiveInput::Event(payload) => self.event(request.scope, payload).await?,
            }
            Ok(LiveBody {
                html: self.view()?,
                ..LiveBody::default()
            })
        })
    }
}
