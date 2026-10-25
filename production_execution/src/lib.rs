//! Bilimbi.Factory.ProductionExecution: production orders with exact
//! formula/routing revisions, completed operation executions posted
//! atomically through Inventory's production postings, and material hold
//! overrides, shop-floor capture, correction chains, traces and yield.
use bilibi_database::{Db, MutationBoundary};
use bilibi_factory_inventory::Inventory;
use bilibi_factory_product_definition::ProductDefinition;
use bilibi_module_registry::{Layer, Module, Service};
use bilibi_tenancy::Scope;
use std::sync::Arc;

mod agent_operations;
mod capture;
mod codes;
mod completion;
mod contributions;
mod error;
mod import;
mod labour;
mod measurements;
mod model;
mod orders;
mod outcomes;
mod wastage;
mod web;
pub use error::Error;
pub use import::Importer;
pub use model::{
    Approver, Completion, Execution, HoldOverride, HoldOverrideRequest, NewOrder, Order, OrderKind,
    Source,
};
pub use outcomes::{ProductionTrace, RunYield, TraceOrder, TraceRun, UnitYield, YieldBalance};

const PAGES: &str = "factory/production_execution.pages";
pub const SERVICE: &str = "factory/production_execution.production_execution";
type Result<T> = std::result::Result<T, Error>;

const DEFAULT_ORDER_LIMIT: i64 = 50;
const DEFAULT_LIMIT: i64 = 100;
const MAXIMUM_LIMIT: i64 = 500;

// Shared only inside this module: Inventory accepts this contributed address.
static AUTHORITY: bilibi_factory_inventory::PostingAuthority =
    bilibi_factory_inventory::PostingAuthority::declare("factory/production_execution")
        .with_import_capability(contributions::IMPORT);
inventory::submit! { bilibi_module_registry::Contribution::new(
    bilibi_factory_inventory::POSTING_AUTHORITIES,"factory/production_execution",
    bilibi_module_registry::Provenance {module:"factory/production_execution",source:file!(),line:line!()},
    &AUTHORITY
) }

pub struct ProductionExecution {
    db: Db,
    mutations: Arc<MutationBoundary>,
    product_definition: Arc<ProductDefinition>,
    completer: completion::Completer,
}

impl ProductionExecution {
    /// A pooled connection for a read, after the view capability and a live
    /// company are proven.
    async fn read_client(&self, scope: &Scope, company: i64) -> Result<deadpool_postgres::Object> {
        let client = self.db.get().await.map_err(bilibi_database::Error::from)?;
        bilibi_authz::authorize_key(&**client, scope, contributions::FLOOR_VIEW).await?;
        bilibi_company::organisation::require_live(&**client, scope, company).await?;
        Ok(client)
    }

    /// Creates an order or batch with exact published definition revisions,
    /// validated through Product Definition and retained on the order and
    /// every execution.
    pub async fn create_order(&self, scope: &Scope, company: i64, new: &NewOrder) -> Result<Order> {
        let mut client = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(
                &mut client,
                scope.mutation_context("factory.production_execution.create_order"),
            )
            .await?;
        match self.create_order_on(&mut tx, scope, company, new).await {
            Ok(order) => {
                tx.commit().await?;
                Ok(order)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }

    async fn create_order_on(
        &self,
        tx: &mut bilibi_database::MutationTransaction<'_>,
        scope: &Scope,
        company: i64,
        new: &NewOrder,
    ) -> Result<Order> {
        self.create_order_for_on(tx, scope, company, new, Source::Live)
            .await
    }

    async fn create_order_for_on(
        &self,
        tx: &mut bilibi_database::MutationTransaction<'_>,
        scope: &Scope,
        company: i64,
        new: &NewOrder,
        source: Source,
    ) -> Result<Order> {
        bilibi_company::organisation::authorize_target(
            tx.sql(),
            scope,
            company,
            if source == Source::Import {
                contributions::IMPORT
            } else {
                contributions::MANAGE
            },
        )
        .await?;
        bilibi_company::organisation::require_writable(tx.sql(), scope, company, true).await?;
        if source == Source::Import {
            let import = self
                .completer
                .inventory
                .production_import(scope, company, &AUTHORITY)?;
            self.product_definition
                .select_import_revisions_on(
                    tx.sql(),
                    &import,
                    new.product_id,
                    new.formula_version,
                    new.routing_version,
                )
                .await?;
        } else {
            self.product_definition
                .select_revisions_on(
                    tx.sql(),
                    scope,
                    company,
                    new.product_id,
                    new.formula_version,
                    new.routing_version,
                )
                .await?;
        }
        let field = |field, message| Error::Invalid { field, message };
        if new.code.trim().is_empty() {
            return Err(field("Code", "can't be blank"));
        }
        if new.code.chars().count() > 255 {
            return Err(field("Code", "should be at most 255 character(s)"));
        }
        if new
            .demand_ref
            .as_ref()
            .is_some_and(|reference| reference.chars().count() > 255)
        {
            return Err(field("Demand ref", "should be at most 255 character(s)"));
        }
        if tx
                .sql()
                .query_opt(
                    "SELECT 1 FROM factory_production_orders WHERE tenant_id=$1 AND company_id=$2 AND code=$3",
                    &[&scope.tenant(), &company, &new.code],
                )
                .await?
                .is_some()
            {
                return Err(field("Code", "has already been taken"));
            }
        let tenant = scope.tenant();
        let new = new.clone();
        let id = tx
                .mutate(|sql| {
                    Box::pin(async move {
                        let row = sql
                            .query_one(
                                "INSERT INTO factory_production_orders(tenant_id,company_id,code,kind,product_id,formula_version,routing_version,demand_ref) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id,to_jsonb(factory_production_orders) AS saved",
                                &[&tenant, &company, &new.code, &new.kind.name(), &new.product_id, &new.formula_version, &new.routing_version, &new.demand_ref],
                            )
                            .await?;
                        let id: i64 = row.get("id");
                        Ok((
                            id,
                            vec![bilibi_database::Change {
                                kind: "factory_production_orders".into(),
                                id,
                                old: serde_json::json!({}),
                                new: row.get("saved"),
                            }],
                        ))
                    })
                })
                .await?;
        orders::get_order(tx.sql(), scope, company, id).await
    }

    pub async fn get_order(&self, scope: &Scope, company: i64, order_id: i64) -> Result<Order> {
        let client = self.read_client(scope, company).await?;
        orders::get_order(&**client, scope, company, order_id).await
    }

    /// A company's orders and batches, most recently created first (50 by
    /// default, between 1 and 500).
    pub async fn list_orders(
        &self,
        scope: &Scope,
        company: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Order>> {
        let client = self.read_client(scope, company).await?;
        let limit = limit.unwrap_or(DEFAULT_ORDER_LIMIT).clamp(1, MAXIMUM_LIMIT);
        orders::list_orders(&**client, scope, company, limit).await
    }

    /// An order's executions (its runs), most recently completed first (100
    /// by default, at most 500).
    pub async fn list_executions(
        &self,
        scope: &Scope,
        company: i64,
        order_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<Execution>> {
        let limit = match limit.unwrap_or(DEFAULT_LIMIT) {
            limit @ 1..=MAXIMUM_LIMIT => limit,
            _ => return Err(bilibi_database::Error::Invalid.into()),
        };
        let client = self.read_client(scope, company).await?;
        orders::get_order(&**client, scope, company, order_id).await?;
        orders::list_executions(&**client, scope, company, order_id, limit).await
    }

    pub async fn get_execution(
        &self,
        scope: &Scope,
        company: i64,
        execution_id: i64,
    ) -> Result<Execution> {
        let client = self.read_client(scope, company).await?;
        orders::get_execution(&**client, scope, company, execution_id).await
    }

    /// Completes one routed operation. A live completion and a historical
    /// import validate and post identically, but an import first needs the
    /// `factory.production-execution.import` capability. The execution, its
    /// Inventory material effects and any hold override evidence commit in
    /// one transaction, or none does. An identical retry of a `request_id`
    /// returns the original execution; a different request with the same ID
    /// is refused.
    pub async fn complete_operation(
        &self,
        scope: &Scope,
        company: i64,
        order_id: i64,
        source: Source,
        completion: &Completion,
    ) -> Result<Execution> {
        self.completer
            .complete(scope, company, order_id, source, completion)
            .await
    }

    /// Immutable material hold override evidence for one execution.
    pub async fn list_hold_overrides(
        &self,
        scope: &Scope,
        company: i64,
        execution_id: i64,
    ) -> Result<Vec<HoldOverride>> {
        let client = self.read_client(scope, company).await?;
        orders::get_execution(&**client, scope, company, execution_id).await?;
        orders::hold_overrides(&**client, scope, company, execution_id).await
    }
}

inventory::submit! { Module {
    id: "factory/production_execution", layer: Layer::Domains,
    dependencies: &["base/agent_api","base/authz","base/database","base/datetime","base/locale","base/menu","base/module_registry","base/tenancy","base/ui","core/company","core/user","factory/inventory","factory/product_definition"],
    migrations: &[("factory-production-execution-001-orders-executions",include_str!("../migrations/001-orders-executions.sql")),
        ("factory-production-execution-002-shop-floor-capture",include_str!("../migrations/002-shop-floor-capture.sql"))],
    services: &[Service { id:SERVICE, factory: |services| {
        let db = services.get::<Db>("base/database.pool")?.as_ref().clone();
        let mutations: Arc<MutationBoundary> = services.get("base/database.mutations")?;
        let inventory: Arc<Inventory> = services.get(bilibi_factory_inventory::SERVICE)?;
        let product_definition: Arc<ProductDefinition> = services.get(bilibi_factory_product_definition::SERVICE)?;
        let owner = Arc::new(ProductionExecution {
            db: db.clone(),
            mutations: mutations.clone(),
            product_definition: product_definition.clone(),
            completer: completion::Completer { db, mutations, inventory, product_definition },
        });
        agent_operations::bind(owner.clone())?;
        Ok(owner)
    } }, Service { id:PAGES, factory: |services| Ok(Arc::new(bilibi_ui::page::PageService::<Scope, bilibi_database::Error>(Arc::new(web::Pages {
        db: services.get::<Db>("base/database.pool")?.as_ref().clone(),
        execution: services.get(SERVICE)?,
        inventory: services.get(bilibi_factory_inventory::SERVICE)?,
        display: services.get("base/datetime.display")?,
        locales: services.get("base/locale.locales")?,
    })))) }],
} }
