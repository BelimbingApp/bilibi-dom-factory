//! Bilimbi ProductionExecution.Measurement: observed values, immutable limit
//! snapshots and linear correction chains; range flags never reject a value.
use crate::{
    capture::{self, Table},
    contributions::{MEASUREMENT_CORRECT, MEASUREMENT_RECORD},
    orders, Error, ProductionExecution, Result,
};
use bilibi_factory_inventory::{Inventory, PropertyDefinition};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};

fn value(kind: &Value, raw: &Value) -> Result<String> {
    let definition = PropertyDefinition {
        key: "value".into(),
        label: "Value".into(),
        value_type: kind["value_type"].as_str().unwrap_or("").into(),
        unit: kind["unit"].as_str().map(str::to_owned),
        required: true,
    };
    let values = Inventory::validate_property_values(
        &[definition],
        json!({"value":raw}).as_object().unwrap(),
    )
    .map_err(|_| Error::Capture("Invalid measurement value"))?;
    let v = &values["value"];
    Ok(v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string()))
}
impl ProductionExecution {
    pub async fn record_measurement(
        &self,
        scope: &Scope,
        company: i64,
        execution: i64,
        attrs: &Value,
    ) -> Result<Value> {
        self.measurement(scope, company, execution, None, attrs)
            .await
    }
    pub async fn correct_measurement(
        &self,
        scope: &Scope,
        company: i64,
        measurement: i64,
        attrs: &Value,
    ) -> Result<Value> {
        self.measurement(scope, company, measurement, Some(measurement), attrs)
            .await
    }
    pub async fn list_measurements(
        &self,
        scope: &Scope,
        company: i64,
        execution: i64,
    ) -> Result<Vec<Value>> {
        let c = self.read_client(scope, company).await?;
        orders::get_execution(&**c, scope, company, execution).await?;
        capture::list(
            &**c,
            scope,
            company,
            Table::Measurements,
            Some(execution),
            None,
        )
        .await
    }
    async fn measurement(
        &self,
        scope: &Scope,
        company: i64,
        target: i64,
        correction: Option<i64>,
        attrs: &Value,
    ) -> Result<Value> {
        let cap = if correction.is_some() {
            MEASUREMENT_CORRECT
        } else {
            MEASUREMENT_RECORD
        };
        let recorder = self.capture_authorize(scope, company, cap).await?;
        let request = capture::request(attrs)?.to_owned();
        let mut c = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(
                &mut c,
                scope.mutation_context(if correction.is_some() {
                    "factory.production_execution.correct_measurement"
                } else {
                    "factory.production_execution.record_measurement"
                }),
            )
            .await?;
        let result=async {
            capture::writable(tx.sql(),scope,company,cap).await?;
            let mut row=if let Some(id)=correction {
                capture::get(tx.sql(),scope,company,Table::Measurements,id).await?
            }else{
                let execution=orders::get_execution(tx.sql(),scope,company,target).await?;
                let type_id=capture::id(attrs,"measurement_type_id")?;
                let kind=capture::get(tx.sql(),scope,company,Table::Types,type_id).await?;
                if kind["active"]!=true{return Err(Error::Capture("Measurement type inactive"));}
                let identity=capture::optional_id(attrs,"identity_id")?;
                if let Some(id)=identity {
                    let posting=self.completer.inventory.get_transaction_on(tx.sql(),scope,company,execution.inventory_transaction_id).await?;
                    if !posting.entries.iter().any(|e|e.role=="stock" && e.identity_id==Some(id) && !e.native_quantity.starts_with('-') && e.native_quantity.bytes().any(|b|b.is_ascii_digit() && b!=b'0')){return Err(Error::Capture("Identity not run output"));}
                }
                let measured=capture::time(tx.sql(),&attrs["measured_at"],Some(execution.started_at),"Invalid measured at").await?;
                json!({"order_id":execution.order_id,"execution_id":target,"measurement_type_id":type_id,"identity_id":identity,"value_type":kind["value_type"],"unit":kind["unit"],"minimum":kind["minimum"],"maximum":kind["maximum"],"target":kind["target"],"measured_at":measured})
            };
            let valid=value(&row,&attrs["value"])?;
            row["value"]=json!(valid);
            row["out_of_range"]=if matches!(row["value_type"].as_str(),Some("integer"|"decimal")) && (!row["minimum"].is_null() || !row["maximum"].is_null()) {
                let min=row["minimum"].as_str();let max=row["maximum"].as_str();
                json!(tx.sql().query_one("SELECT COALESCE($1::text::numeric < $2::text::numeric,false) OR COALESCE($1::text::numeric > $3::text::numeric,false)",&[&valid,&min,&max]).await?.get::<_,bool>(0))
            }else{Value::Null};
            if correction.is_none() || attrs.get("note").is_some(){row["note"]=capture::text(attrs,"note",false,"Invalid note")?;}
            row["request_id"]=json!(request);row["corrects_id"]=json!(correction);
            row["correction_reason"]=if correction.is_some(){capture::text(attrs,"correction_reason",true,"Correction reason required")?}else{Value::Null};
            let mut command=capture::intent(&row,&["request_id","order_id","execution_id","identity_id","measurement_type_id","value","note","corrects_id","correction_reason"])?;
            if correction.is_some(){
                for key in ["unit","minimum","maximum","target","measured_at"]{command[key]=row[key].clone();}
            }else{
                command["measured_at"]=if attrs["measured_at"].is_null(){Value::Null}else{row["measured_at"].clone()};
            }
            let fp=capture::fingerprint(tx.sql(),if correction.is_some(){"measurement_correction"}else{"measurement"},target,&command).await?;
            if let Some(v)=capture::replay(tx.sql(),scope,company,Table::Measurements,&request,&fp).await?{return Ok((v,false));}
            if !row["corrected_by_id"].is_null(){return Err(Error::Capture("Measurement already corrected"));}
            row["request_fingerprint"]=json!(fp);capture::merge(&mut row,recorder);
            Ok((capture::insert(&mut tx,scope,company,Table::Measurements,row).await?,true))
        }.await;
        capture::finish(tx, result).await
    }
}
