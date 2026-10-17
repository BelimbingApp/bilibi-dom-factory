//! Bilimbi.Factory.ProductDefinition: company resources, products and exact,
//! immutable formula/routing revisions, with their pages (`web`).
use bilibi_database::{Db, MutationBoundary};
use bilibi_factory_inventory::Inventory;
use bilibi_module_registry::{Layer, Module, Service};
use bilibi_tenancy::Scope;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;
use store::Kind;

mod contributions;
mod error;
mod model;
mod resources;
mod revisions;
mod store;
mod web;
pub use error::Error;
pub use model::{
    FormulaLine, FormulaRevision, NewProduct, NewResource, NewResourceType, Operation, Product,
    Resource, ResourceType, ResourceTypeUpdate, ResourceUpdate, RoutingRevision, SelectedRevisions,
};

pub const SERVICE: &str = "factory/product_definition.product_definition";
const PAGES: &str = "factory/product_definition.pages";
type Result<T> = std::result::Result<T, Error>;
pub struct ProductDefinition {
    db: Db,
    mutations: Arc<MutationBoundary>,
    inventory: Arc<Inventory>,
}

// Check authorization inside the transaction, before validation or mutation.
// The company lock is also Inventory's write lock, so publication cannot race
// retirement, and no company can become archived between check and commit.
macro_rules! audited {
    ($self:ident,$scope:ident,$company:ident,$action:literal,|$tx:ident| $body:expr) => {{
        let mut client = $self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut $tx = $self
            .mutations
            .begin(&mut client, $scope.mutation_context($action))
            .await?;
        bilibi_company::organisation::authorize_target(
            $tx.sql(),
            $scope,
            $company,
            contributions::MANAGE,
        )
        .await?;
        bilibi_company::organisation::require_writable($tx.sql(), $scope, $company, true).await?;
        let result: Result<_> = async { $body }.await;
        match result {
            Ok(value) => {
                $tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                $tx.rollback().await?;
                Err(error)
            }
        }
    }};
}

impl ProductDefinition {
    async fn read_client(&self, scope: &Scope, company: i64) -> Result<deadpool_postgres::Object> {
        let client = self.db.get().await.map_err(bilibi_database::Error::from)?;
        bilibi_authz::authorize_key(&**client, scope, contributions::VIEW).await?;
        bilibi_company::organisation::require_live(&**client, scope, company).await?;
        Ok(client)
    }
    async fn get<T: DeserializeOwned>(
        &self,
        scope: &Scope,
        company: i64,
        kind: Kind,
        id: i64,
    ) -> Result<T> {
        let client = self.read_client(scope, company).await?;
        store::fetch(&**client, scope, company, kind, id).await
    }
    async fn list<T: DeserializeOwned>(
        &self,
        scope: &Scope,
        company: i64,
        kind: Kind,
        product: Option<i64>,
        limit: Option<i64>,
    ) -> Result<Vec<T>> {
        let limit = match limit.unwrap_or(100) {
            n @ 1..=500 => n,
            _ => return Err(bilibi_database::Error::Invalid.into()),
        };
        let client = self.read_client(scope, company).await?;
        if let Some(product) = product {
            let _: Product =
                store::fetch(&**client, scope, company, Kind::Product, product).await?;
        }
        store::list(&**client, scope, company, kind, product, limit).await
    }
    pub async fn create_product(
        &self,
        scope: &Scope,
        company: i64,
        new: &NewProduct,
    ) -> Result<Product> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.create_product",
            |tx| {
                self.inventory
                    .get_item_on(tx.sql(), scope, company, new.item_id)
                    .await?;
                store::names(
                    tx.sql(),
                    scope,
                    company,
                    Kind::Product,
                    None,
                    &new.code,
                    &new.name,
                )
                .await?;
                if tx.sql().query_opt("SELECT 1 FROM factory_products WHERE tenant_id=$1 AND company_id=$2 AND item_id=$3",&[&scope.tenant(),&company,&new.item_id]).await?.is_some() {
                return Err(Error::Invalid {field:"Item id",message:"has already been taken"});
            }
                store::insert(
                    &mut tx,
                    scope,
                    company,
                    Kind::Product,
                    json!({"item_id":new.item_id,"code":new.code,"name":new.name}),
                )
                .await
            }
        )
    }
    pub async fn get_product(&self, scope: &Scope, company: i64, id: i64) -> Result<Product> {
        self.get(scope, company, Kind::Product, id).await
    }
    pub async fn list_products(
        &self,
        scope: &Scope,
        company: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Product>> {
        self.list(scope, company, Kind::Product, None, limit).await
    }
    pub async fn create_resource_type(
        &self,
        scope: &Scope,
        company: i64,
        new: &NewResourceType,
    ) -> Result<ResourceType> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.create_resource_type",
            |tx| resources::create_type(&mut tx, scope, company, new).await
        )
    }
    pub async fn update_resource_type(
        &self,
        scope: &Scope,
        company: i64,
        id: i64,
        update: &ResourceTypeUpdate,
    ) -> Result<ResourceType> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.update_resource_type",
            |tx| resources::update_type(&mut tx, scope, company, id, update).await
        )
    }
    pub async fn retire_resource_type(
        &self,
        scope: &Scope,
        company: i64,
        id: i64,
    ) -> Result<ResourceType> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.retire_resource_type",
            |tx| resources::retire_type(&mut tx, scope, company, id).await
        )
    }
    pub async fn get_resource_type(
        &self,
        scope: &Scope,
        company: i64,
        id: i64,
    ) -> Result<ResourceType> {
        self.get(scope, company, Kind::ResourceType, id).await
    }
    pub async fn list_resource_types(
        &self,
        scope: &Scope,
        company: i64,
        limit: Option<i64>,
    ) -> Result<Vec<ResourceType>> {
        self.list(scope, company, Kind::ResourceType, None, limit)
            .await
    }
    pub async fn create_resource(
        &self,
        scope: &Scope,
        company: i64,
        new: &NewResource,
    ) -> Result<Resource> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.create_resource",
            |tx| resources::create(&mut tx, scope, company, new).await
        )
    }
    pub async fn update_resource(
        &self,
        scope: &Scope,
        company: i64,
        id: i64,
        update: &ResourceUpdate,
    ) -> Result<Resource> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.update_resource",
            |tx| resources::update(&mut tx, scope, company, id, update).await
        )
    }
    pub async fn retire_resource(&self, scope: &Scope, company: i64, id: i64) -> Result<Resource> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.retire_resource",
            |tx| resources::retire(&mut tx, scope, company, id).await
        )
    }
    pub async fn get_resource(&self, scope: &Scope, company: i64, id: i64) -> Result<Resource> {
        self.get(scope, company, Kind::Resource, id).await
    }
    /// Scoped resource read on a sibling's mutation connection.
    pub async fn get_resource_on(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company: i64,
        id: i64,
    ) -> Result<Resource> {
        bilibi_authz::authorize_key(client, scope, contributions::VIEW).await?;
        bilibi_company::organisation::require_live(client, scope, company).await?;
        store::fetch(client, scope, company, Kind::Resource, id).await
    }
    pub async fn list_resources(
        &self,
        scope: &Scope,
        company: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Resource>> {
        self.list(scope, company, Kind::Resource, None, limit).await
    }
    /// JSON attributes match Bilimbi: lines and optional process_config.
    pub async fn publish_formula(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        attrs: &Value,
    ) -> Result<FormulaRevision> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.publish_formula",
            |tx| revisions::formula(&mut tx, &self.inventory, scope, company, product, attrs).await
        )
    }
    /// JSON attributes match Bilimbi: operations and optional process_config.
    pub async fn publish_routing(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        attrs: &Value,
    ) -> Result<RoutingRevision> {
        audited!(
            self,
            scope,
            company,
            "factory.product_definition.publish_routing",
            |tx| revisions::routing(&mut tx, &self.inventory, scope, company, product, attrs).await
        )
    }
    async fn revision<T: DeserializeOwned>(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        version: i32,
        kind: Kind,
    ) -> Result<T> {
        let client = self.read_client(scope, company).await?;
        let _: Product = store::fetch(&**client, scope, company, Kind::Product, product).await?;
        store::revision(&**client, scope, company, kind, product, version).await
    }
    pub async fn get_formula_revision(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        version: i32,
    ) -> Result<FormulaRevision> {
        self.revision(scope, company, product, version, Kind::Formula)
            .await
    }
    pub async fn get_routing_revision(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        version: i32,
    ) -> Result<RoutingRevision> {
        self.revision(scope, company, product, version, Kind::Routing)
            .await
    }
    pub async fn list_formula_revisions(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        limit: Option<i64>,
    ) -> Result<Vec<FormulaRevision>> {
        self.list(scope, company, Kind::Formula, Some(product), limit)
            .await
    }
    pub async fn list_routing_revisions(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        limit: Option<i64>,
    ) -> Result<Vec<RoutingRevision>> {
        self.list(scope, company, Kind::Routing, Some(product), limit)
            .await
    }
    /// Exact versions, not latest. Retirement does not invalidate history.
    pub async fn select_revisions(
        &self,
        scope: &Scope,
        company: i64,
        product: i64,
        formula_version: i32,
        routing_version: i32,
    ) -> Result<SelectedRevisions> {
        let client = self.read_client(scope, company).await?;
        self.select_revisions_on(
            &**client,
            scope,
            company,
            product,
            formula_version,
            routing_version,
        )
        .await
    }

    /// Exact revisions on the caller's connection, including pending data.
    pub async fn select_revisions_on(
        &self,
        client: &impl tokio_postgres::GenericClient,
        scope: &Scope,
        company: i64,
        product: i64,
        formula_version: i32,
        routing_version: i32,
    ) -> Result<SelectedRevisions> {
        bilibi_authz::authorize_key(client, scope, contributions::VIEW).await?;
        bilibi_company::organisation::require_live(client, scope, company).await?;
        let product: Product = store::fetch(client, scope, company, Kind::Product, product).await?;
        let formula = store::revision(
            client,
            scope,
            company,
            Kind::Formula,
            product.id,
            formula_version,
        )
        .await?;
        let routing = store::revision(
            client,
            scope,
            company,
            Kind::Routing,
            product.id,
            routing_version,
        )
        .await?;
        revisions::compatible(&product, &formula, &routing)?;
        Ok(SelectedRevisions {
            product,
            formula,
            routing,
        })
    }
}

inventory::submit! { Module {
    id: "factory/product_definition", layer: Layer::Domains,
    dependencies: &["base/authz","base/database","base/menu","base/module_registry","base/tenancy","base/ui","core/company","factory/inventory"],
    migrations: &[("factory-product-definition-001-definitions",include_str!("../migrations/001-definitions.sql"))],
    services: &[
        Service { id:SERVICE, factory: |services| Ok(Arc::new(ProductDefinition {
            db:services.get::<Db>("base/database.pool")?.as_ref().clone(),
            mutations:services.get("base/database.mutations")?,
            inventory:services.get(bilibi_factory_inventory::SERVICE)?,
        })) },
        Service { id:PAGES, factory: |services| Ok(Arc::new(bilibi_ui::page::PageService::<bilibi_tenancy::Scope, bilibi_database::Error>(Arc::new(web::Pages {
            db:services.get::<Db>("base/database.pool")?.as_ref().clone(),
            definitions:services.get(SERVICE)?,
            inventory:services.get(bilibi_factory_inventory::SERVICE)?,
        })))) },
    ],
} }
