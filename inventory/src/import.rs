//! Import validation and postings through a module's private registered
//! authority; ordinary Inventory APIs retain their own capabilities.
use crate::{
    balance, conversions, genealogy, ledger, materials, units, Balance, Conversion, Error,
    Identity, Inventory, Material, MaterialTransaction, PostingAuthority, PostingRequest,
    ProductionImport, Trace, TransactionKind, Unit,
};
use bilibi_database::MutationTransaction;
use bilibi_tenancy::Scope;

impl Inventory {
    /// Bind an import path to its scope, company and registered authority.
    /// Every use rechecks the grant; this is not a cached authorization.
    pub fn production_import<'a>(
        &self,
        scope: &'a Scope,
        company: i64,
        authority: &'static PostingAuthority,
    ) -> Result<ProductionImport<'a>, Error> {
        self.registered(authority)?;
        Ok(ProductionImport::new(scope, company, authority))
    }

    /// Read immutable identity evidence needed by an import.
    pub async fn get_import_identity_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        id: i64,
    ) -> Result<Identity, Error> {
        import.authorize_on(client).await?;
        genealogy::get_authorized(client, import.scope(), import.company(), id).await
    }

    /// When stock was last added to an identity at or before `until`, for an
    /// import's hold age.
    pub async fn get_import_identity_last_added_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        id: i64,
        until: std::time::SystemTime,
    ) -> Result<Option<std::time::SystemTime>, Error> {
        import.authorize_on(client).await?;
        genealogy::last_added_authorized(client, import.scope(), import.company(), id, until).await
    }

    /// Read immutable posting evidence needed by an import.
    pub async fn get_import_transaction_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        id: i64,
    ) -> Result<MaterialTransaction, Error> {
        import.authorize_on(client).await?;
        ledger::read(client, import.scope().tenant(), import.company(), id).await
    }

    /// Material lookup needed to prepare an import, without configuration view.
    pub async fn get_import_material_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        item: i64,
    ) -> Result<Material, Error> {
        import.authorize_on(client).await?;
        Ok(
            materials::fetch(client, import.scope(), import.company(), item, "")
                .await?
                .1,
        )
    }

    /// An exact immutable conversion version for an import line.
    pub async fn get_import_conversion_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        item: i64,
        unit: i64,
        version: i32,
    ) -> Result<Conversion, Error> {
        import.authorize_on(client).await?;
        conversions::get_authorized(
            client,
            import.scope(),
            import.company(),
            item,
            unit,
            Some(version),
        )
        .await
    }

    /// Backward genealogy needed to recover an import's source lots.
    pub async fn trace_import_backward_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        identity: i64,
    ) -> Result<Trace, Error> {
        import.authorize_on(client).await?;
        genealogy::trace_authorized(
            client,
            import.scope(),
            import.company(),
            identity,
            genealogy::Direction::Backward,
        )
        .await
    }

    /// Correction evidence used by imported run yield, including corrections
    /// of corrections. This grants no correction write access.
    pub async fn list_import_corrections_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        transaction: i64,
    ) -> Result<Vec<MaterialTransaction>, Error> {
        import.authorize_on(client).await?;
        ledger::corrections_authorized(client, import.scope(), import.company(), transaction).await
    }

    /// Recorded balance and conversion evidence used by imported run yield.
    pub async fn get_import_transaction_balance_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        transaction: i64,
    ) -> Result<Balance, Error> {
        import.authorize_on(client).await?;
        balance::read_authorized(client, import.scope(), import.company(), transaction).await
    }

    /// Native unit metadata used by imported run yield.
    pub async fn get_import_unit_on(
        &self,
        client: &(impl tokio_postgres::GenericClient + Sync),
        import: &ProductionImport<'_>,
        unit: i64,
    ) -> Result<Unit, Error> {
        import.authorize_on(client).await?;
        units::fetch(client, import.scope(), import.company(), unit, "").await
    }

    /// Post imported completion lines atomically with their execution. Only
    /// output, production consumption and transform are accepted; an import
    /// grant cannot post receipts, transfers or corrections.
    pub async fn record_import_on(
        &self,
        tx: &mut MutationTransaction<'_>,
        import: &ProductionImport<'_>,
        inputs: &[crate::StockLine],
        outputs: &[crate::StockLine],
        request: &PostingRequest,
    ) -> Result<MaterialTransaction, Error> {
        self.registered(import.authority)?;
        let mut request = request.clone();
        let kind = match (inputs.is_empty(), outputs.is_empty()) {
            (false, false) => {
                request.inputs = inputs.to_vec();
                request.outputs = outputs.to_vec();
                TransactionKind::Transform
            }
            (true, false) => {
                request.lines = outputs.to_vec();
                TransactionKind::Output
            }
            (false, true) => {
                request.lines = inputs.to_vec();
                TransactionKind::Consumption
            }
            (true, true) => return Err(bilibi_database::Error::Invalid.into()),
        };
        ledger::post(
            tx,
            import.scope(),
            import.company(),
            kind,
            &request,
            Some(import.authority),
            Some(import),
        )
        .await
    }
}
