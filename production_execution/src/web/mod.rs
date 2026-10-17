//! Bilimbi `ProductionExecution.Web.FloorLive`, behind one page adapter.
mod floor;
mod shared;
mod view;

use bilibi_database::{Db, Error, Result};
use bilibi_datetime::DisplayPolicy;
use bilibi_factory_inventory::Inventory;
use bilibi_tenancy::Scope;
use bilibi_ui::page::{LiveBody, LiveFuture, LiveInput, LiveRequest, LiveState, MountFuture, Page, PageBody, PageFuture, PageRequest};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Pages {
    pub(crate) db: Db,
    pub(crate) execution: Arc<crate::ProductionExecution>,
    pub(crate) inventory: Arc<Inventory>,
    pub(crate) display: Arc<DisplayPolicy>,
}
impl Page<Scope, Error> for Pages {
    fn title(&self, _: &str) -> Option<&'static str> { Some("Shop floor") }
    fn render<'a>(&'a self, request: PageRequest<'a, Scope>) -> PageFuture<'a, Error> {
        Box::pin(async move {
            let scope = request.scope.ok_or(Error::Unauthorized)?;
            let mut floor = floor::Floor::new(self)?;
            floor.select(scope, request.query).await?;
            Ok(PageBody { html: floor.view()?, live: true, title: None })
        })
    }
    fn mount<'a>(&'a self, request: PageRequest<'a, Scope>) -> MountFuture<'a, Scope, Error> {
        Box::pin(async move {
            let scope = request.scope.ok_or(Error::Unauthorized)?;
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
            Ok(LiveBody { html: self.view()?, query: self.query(), ..LiveBody::default() })
        })
    }
}
