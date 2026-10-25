//! Explicit import facade. Bilimbi's import needs no configuration grants;
//! keep that reach here while normal facade and agent paths retain theirs.
use crate::{
    orders, Completion, Execution, NewOrder, Order, ProductionExecution, Result, Source, AUTHORITY,
};
use bilibi_factory_inventory::{Identity, MaterialTransaction, ProductionImport};
use bilibi_tenancy::Scope;

/// An import's orders and immutable result evidence. Every call rechecks the
/// import grant against this scope and company; no general configuration or
/// live production operation is available through this facade.
pub struct Importer<'a> {
    owner: &'a ProductionExecution,
    access: ProductionImport<'a>,
}
impl ProductionExecution {
    /// Open the explicit import path. Authorization is repeated by each call,
    /// including retries and reads, and inside each write transaction.
    pub fn importer<'a>(&'a self, scope: &'a Scope, company: i64) -> Result<Importer<'a>> {
        Ok(Importer {
            owner: self,
            access: self
                .completer
                .inventory
                .production_import(scope, company, &AUTHORITY)?,
        })
    }
}
impl Importer<'_> {
    pub async fn create_order(&self, new: &NewOrder) -> Result<Order> {
        let mut client = self
            .owner
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .owner
            .mutations
            .begin(
                &mut client,
                self.access
                    .scope()
                    .mutation_context("factory.production_execution.import_order"),
            )
            .await?;
        let order = self
            .owner
            .create_order_for_on(
                &mut tx,
                self.access.scope(),
                self.access.company(),
                new,
                Source::Import,
            )
            .await?;
        tx.commit().await?;
        Ok(order)
    }
    pub async fn complete_operation(
        &self,
        order: i64,
        completion: &Completion,
    ) -> Result<Execution> {
        self.owner
            .complete_operation(
                self.access.scope(),
                self.access.company(),
                order,
                Source::Import,
                completion,
            )
            .await
    }
    /// Order recovery after a separately committed header (including a taken
    /// code). Same bounded listing as the ordinary order facade.
    pub async fn list_orders(&self, limit: Option<i64>) -> Result<Vec<Order>> {
        let client = self
            .owner
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        self.access.authorize_on(&**client).await?;
        orders::list_orders(
            &**client,
            self.access.scope(),
            self.access.company(),
            limit
                .unwrap_or(crate::DEFAULT_ORDER_LIMIT)
                .clamp(1, crate::MAXIMUM_LIMIT),
        )
        .await
    }
    pub async fn get_execution(&self, id: i64) -> Result<Execution> {
        let client = self
            .owner
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        self.access.authorize_on(&**client).await?;
        orders::get_execution(&**client, self.access.scope(), self.access.company(), id).await
    }
    pub async fn get_transaction(&self, id: i64) -> Result<MaterialTransaction> {
        let client = self
            .owner
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        Ok(self
            .owner
            .completer
            .inventory
            .get_import_transaction_on(&**client, &self.access, id)
            .await?)
    }
    pub async fn get_identity(&self, id: i64) -> Result<Identity> {
        let client = self
            .owner
            .db
            .get()
            .await
            .map_err(bilibi_database::Error::from)?;
        Ok(self
            .owner
            .completer
            .inventory
            .get_import_identity_on(&**client, &self.access, id)
            .await?)
    }
}
