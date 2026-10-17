//! Bilimbi ProductionExecution.Wastage: capture and material effects commit
//! together. Capture chains point to predecessors; ledger deltas target root.
use crate::{
    capture::{self, Table},
    codes::decimal,
    contributions::{WASTAGE_CORRECT, WASTAGE_RECORD},
    orders, Error, ProductionExecution, Result, AUTHORITY,
};
use bilibi_factory_inventory::{Decimal, Observation, PostingRequest, StockLine};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl ProductionExecution {
    pub async fn record_wastage(
        &self,
        scope: &Scope,
        company: i64,
        execution: i64,
        attrs: &Value,
    ) -> Result<Value> {
        self.wastage(scope, company, execution, None, attrs).await
    }
    pub async fn correct_wastage(
        &self,
        scope: &Scope,
        company: i64,
        record: i64,
        attrs: &Value,
    ) -> Result<Value> {
        self.wastage(scope, company, record, Some(record), attrs)
            .await
    }
    pub async fn list_wastage(
        &self,
        scope: &Scope,
        company: i64,
        execution: i64,
    ) -> Result<Vec<Value>> {
        let c = self.read_client(scope, company).await?;
        orders::get_execution(&**c, scope, company, execution).await?;
        capture::list(&**c, scope, company, Table::Wastage, Some(execution), None).await
    }
    async fn wastage(
        &self,
        scope: &Scope,
        company: i64,
        target: i64,
        correction: Option<i64>,
        attrs: &Value,
    ) -> Result<Value> {
        let cap = if correction.is_some() {
            WASTAGE_CORRECT
        } else {
            WASTAGE_RECORD
        };
        let recorder = self.capture_authorize(scope, company, cap).await?;
        let request = capture::request(attrs)?.to_owned();
        let quantity = decimal(&attrs["quantity"], "Invalid quantity")?;
        if quantity < Decimal::ZERO || (correction.is_none() && quantity.is_zero()) {
            return Err(Error::Capture("Invalid quantity"));
        }
        let mut intent = capture::intent(
            attrs,
            &[
                "request_id",
                "reason_id",
                "item_id",
                "location_id",
                "identity_id",
                "quantity",
                "observation",
                "note",
                "occurred_at",
                "correction_reason",
            ],
        )?;
        intent["quantity"] = json!(quantity.to_string());
        if intent.get("note").is_some() {
            intent["note"] = capture::text(attrs, "note", false, "Invalid note")?;
        }
        if correction.is_some() {
            intent["correction_reason"] = capture::text(
                attrs,
                "correction_reason",
                true,
                "Correction reason required",
            )?;
        }
        let mut c = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(
                &mut c,
                scope.mutation_context(if correction.is_some() {
                    "factory.production_execution.correct_wastage"
                } else {
                    "factory.production_execution.record_wastage"
                }),
            )
            .await?;
        let result=async{
            capture::writable(tx.sql(),scope,company,cap).await?;
            let fp=capture::fingerprint(tx.sql(),if correction.is_some(){"wastage_correction"}else{"wastage"},target,&intent).await?;
            if let Some(v)=capture::replay(tx.sql(),scope,company,Table::Wastage,&request,&fp).await?{return Ok((v,false));}
            let mut row=if let Some(id)=correction{
                let old=capture::get(tx.sql(),scope,company,Table::Wastage,id).await?;
                if !old["corrected_by_id"].is_null(){return Err(Error::Capture("Wastage already corrected"));}old
            }else{
                let execution=orders::get_execution(tx.sql(),scope,company,target).await?;
                let item=capture::id(attrs,"item_id")?;let identity=capture::optional_id(attrs,"identity_id")?;
                let posting=self.completer.inventory.get_transaction_on(tx.sql(),scope,company,execution.inventory_transaction_id).await?;
                let entry=posting.entries.iter().find(|e|e.role=="stock" && e.item_id==Some(item) && e.identity_id==identity).ok_or(Error::Capture("Wastage material not in run"))?;
                let occurred=capture::time(tx.sql(),&attrs["occurred_at"],Some(execution.started_at),"Invalid occurred at").await?;
                json!({"order_id":execution.order_id,"execution_id":target,"item_id":item,"identity_id":identity,"location_id":capture::id(attrs,"location_id")?,"unit_id":entry.native_unit_id,"observation":attrs["observation"],"occurred_at":occurred})
            };
            let previous_reason=row["reason_id"].as_i64();
            if correction.is_none() || attrs.get("reason_id").is_some(){row["reason_id"]=json!(capture::id(attrs,"reason_id")?);}
            let reason=capture::get(tx.sql(),scope,company,Table::Reasons,capture::id(&row,"reason_id")?).await?;
            if reason["active"]!=true && previous_reason!=row["reason_id"].as_i64(){return Err(Error::Capture("Wastage reason inactive"));}
            if correction.is_none() || attrs.get("note").is_some(){row["note"]=capture::text(attrs,"note",false,"Invalid note")?;}
            let delta=if correction.is_some(){decimal(&row["quantity"],"Invalid quantity")?-quantity}else{quantity};
            let mut root=row.clone();
            while let Some(previous)=root["corrects_id"].as_i64(){root=capture::get(tx.sql(),scope,company,Table::Wastage,previous).await?;}
            let observation:Observation=serde_json::from_value(row["observation"].clone()).map_err(|_|Error::Capture("Invalid observation"))?;
            let posted=if correction.is_some() && delta.is_zero(){None}else{
                let execution=orders::get_execution(tx.sql(),scope,company,capture::id(&row,"execution_id")?).await?;
                let order=orders::get_order(tx.sql(),scope,company,execution.order_id).await?;
                let mut context=BTreeMap::new();context.insert("operation_execution".into(),execution.request_id);context.insert("order_or_batch".into(),order.code);context.insert("work_centre".into(),execution.resource_id.to_string());
                let evidence=format!("Wastage {}: {}{}",reason["code"].as_str().unwrap(),reason["label"].as_str().unwrap(),row["note"].as_str().map(|n|format!(" — {n}")).unwrap_or_default());
                let at=row["occurred_at"].as_str().unwrap();
                let effective_at=tx.sql().query_one("SELECT $1::text::timestamptz",&[&at]).await?.get(0);
                let posting=PostingRequest{request_id:format!("pe-wastage:{request}"),evidence,effective_at:Some(effective_at),context,lines:vec![StockLine{item_id:capture::id(&row,"item_id")?,location_id:Some(capture::id(&row,"location_id")?),quantity:delta,unit_id:Some(capture::id(&row,"unit_id")?),identity_id:row["identity_id"].as_i64(),observation,..Default::default()}],reason:if correction.is_some(){intent["correction_reason"].as_str().map(str::to_owned)}else{None},corrects_transaction_id:if correction.is_some(){Some(capture::id(&root,"inventory_transaction_id")?)}else{None},..Default::default()};
                let saved=if correction.is_some(){self.completer.inventory.record_production_correction_on(&mut tx,scope,company,&posting,&AUTHORITY).await?}else{self.completer.inventory.record_production_consumption_on(&mut tx,scope,company,&posting,&AUTHORITY).await?};
                Some(saved.id)
            };
            row["quantity"]=json!(quantity.to_string());row["inventory_transaction_id"]=json!(posted);
            row["corrects_id"]=json!(correction);row["correction_reason"]=intent["correction_reason"].clone();row["request_id"]=json!(request);row["request_fingerprint"]=json!(fp);capture::merge(&mut row,recorder);
            Ok((capture::insert(&mut tx,scope,company,Table::Wastage,row).await?,true))
        }.await;
        capture::finish(tx, result).await
    }
}
