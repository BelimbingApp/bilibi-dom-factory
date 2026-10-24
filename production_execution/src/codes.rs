//! Bilimbi CaptureCodes and Measurement's configuration, private behind the
//! ProductionExecution facade. Code, value type and unit are immutable.
use crate::{
    capture::{self, Table},
    contributions::{MANAGE, VIEW},
    Error, ProductionExecution, Result,
};
use bilibi_database::Change;
use bilibi_tenancy::Scope;
use serde_json::{json, Value};

pub(crate) fn decimal(v: &Value, error: &'static str) -> Result<bilibi_factory_inventory::Decimal> {
    use bilibi_factory_inventory::Decimal;
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(Error::Capture(error)),
    };
    let d = s
        .parse::<Decimal>()
        .map_err(|_| Error::Capture(error))?
        .normalize();
    if d.scale() > 12 || d.abs() >= Decimal::from(1_000_000_000_000_i64) {
        return Err(Error::Capture(error));
    }
    Ok(d)
}
fn configuration(table: Table, attrs: &Value, previous: Option<&Value>) -> Result<Value> {
    if !attrs.is_object() {
        return Err(Error::Capture("Invalid configuration"));
    }
    let mut row = previous.cloned().unwrap_or_else(|| json!({"active":true}));
    if previous.is_none() {
        let code = attrs["code"].as_str().unwrap_or("").trim().to_uppercase();
        if code.is_empty()
            || code.len() > 64
            || !code.as_bytes()[0].is_ascii_alphanumeric()
            || !code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(Error::Invalid {
                field: "Code",
                message: "is invalid",
            });
        }
        row["code"] = json!(code);
    }
    if previous.is_none() || attrs.get("label").is_some() {
        let label = attrs["label"].as_str().unwrap_or("").trim();
        if label.is_empty() || label.chars().count() > 255 {
            return Err(Error::Invalid {
                field: "Label",
                message: "is invalid",
            });
        }
        row["label"] = json!(label);
    }
    if let Some(active) = attrs.get("active") {
        if !active.is_boolean() {
            return Err(Error::Capture("Invalid active"));
        }
        row["active"] = active.clone();
    }
    if matches!(table, Table::Types) {
        if previous.is_none() {
            row["value_type"] = attrs["value_type"].clone();
            row["unit"] = capture::text(attrs, "unit", false, "Invalid measurement type")?;
        }
        let kind = row["value_type"].as_str().unwrap_or("");
        if !bilibi_factory_inventory::VALUE_TYPES.contains(&kind) {
            return Err(Error::Capture("Invalid measurement type"));
        }
        let numeric = matches!(kind, "decimal" | "integer");
        if row["unit"]
            .as_str()
            .is_some_and(|u| !numeric || u.chars().count() > 32)
        {
            return Err(Error::Capture("Invalid measurement type"));
        }
        for key in ["minimum", "maximum", "target"] {
            if let Some(v) = attrs.get(key) {
                row[key] = if v.is_null() || v == "" {
                    Value::Null
                } else {
                    if !numeric {
                        return Err(Error::Capture("Invalid measurement limits"));
                    }
                    json!(decimal(v, "Invalid measurement limits")?.to_string())
                };
            }
        }
        if !row["minimum"].is_null()
            && !row["maximum"].is_null()
            && decimal(&row["minimum"], "Invalid measurement limits")?
                > decimal(&row["maximum"], "Invalid measurement limits")?
        {
            return Err(Error::Capture("Invalid measurement limits"));
        }
    }
    Ok(row)
}
macro_rules! code_facade {
    ($list:ident,$get:ident,$create:ident,$update:ident,$table:ident) => {
        pub async fn $list(
            &self,
            scope: &Scope,
            company: i64,
            active: Option<bool>,
        ) -> Result<Vec<Value>> {
            let c = self.db.get().await.map_err(bilibi_database::Error::from)?;
            bilibi_company::authorize_target(&**c, scope, company, VIEW).await?;
            bilibi_company::require_live(&**c, scope, company).await?;
            capture::list(&**c, scope, company, Table::$table, None, active).await
        }
        pub async fn $get(&self, scope: &Scope, company: i64, id: i64) -> Result<Value> {
            let c = self.db.get().await.map_err(bilibi_database::Error::from)?;
            bilibi_company::authorize_target(&**c, scope, company, VIEW).await?;
            bilibi_company::require_live(&**c, scope, company).await?;
            Ok(capture::public(
                capture::get(&**c, scope, company, Table::$table, id).await?,
            ))
        }
        pub async fn $create(&self, scope: &Scope, company: i64, attrs: &Value) -> Result<Value> {
            self.save_code(scope, company, Table::$table, None, attrs)
                .await
        }
        pub async fn $update(
            &self,
            scope: &Scope,
            company: i64,
            id: i64,
            attrs: &Value,
        ) -> Result<Value> {
            self.save_code(scope, company, Table::$table, Some(id), attrs)
                .await
        }
    };
}
impl ProductionExecution {
    code_facade!(
        list_wastage_reasons,
        get_wastage_reason,
        create_wastage_reason,
        update_wastage_reason,
        Reasons
    );
    code_facade!(
        list_labour_roles,
        get_labour_role,
        create_labour_role,
        update_labour_role,
        Roles
    );
    code_facade!(
        list_measurement_types,
        get_measurement_type,
        create_measurement_type,
        update_measurement_type,
        Types
    );

    async fn save_code(
        &self,
        scope: &Scope,
        company: i64,
        table: Table,
        key: Option<i64>,
        attrs: &Value,
    ) -> Result<Value> {
        let mut c = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(
                &mut c,
                scope.mutation_context("factory.production_execution.configure_capture"),
            )
            .await?;
        let result=async {
            capture::writable(tx.sql(),scope,company,MANAGE).await?;
            let previous=match key {Some(id)=>Some(capture::get(tx.sql(),scope,company,table,id).await?),None=>None};
            let mut data=configuration(table,attrs,previous.as_ref())?;
            if let Some(id)=key {
                let name=table.name();
                let old:Value=tx.sql().query_one(&format!("SELECT to_jsonb(t) FROM {name} t WHERE tenant_id=$1 AND company_id=$2 AND id=$3"),&[&scope.tenant(),&company,&id]).await?.get(0);
                let saved=tx.mutate(move |c|Box::pin(async move {
                    let extra=if matches!(table,Table::Types) {",minimum=r.minimum,maximum=r.maximum,target=r.target"} else {""};
                    let new=c.query_one(&format!("UPDATE {name} t SET label=r.label,active=r.active{extra} FROM jsonb_populate_record(NULL::{name},$1) r WHERE t.tenant_id=r.tenant_id AND t.company_id=r.company_id AND t.id=r.id RETURNING to_jsonb(t)"),&[&data]).await?.get::<_,Value>(0);
                    Ok((new.clone(),vec![Change{kind:name.into(),id,old,new}]))
                })).await?;
                // Render numeric settings as exact decimal text, consistently with reads.
                let _=saved;
                Ok(capture::public(capture::get(tx.sql(),scope,company,table,id).await?))
            } else {
                if tx.sql().query_opt(&format!("SELECT 1 FROM {} WHERE tenant_id=$1 AND company_id=$2 AND code=$3",table.name()),&[&scope.tenant(),&company,&data["code"].as_str().unwrap()]).await?.is_some() { return Err(Error::Invalid{field:"Code",message:"has already been taken"}); }
                data["tenant_id"]=json!(scope.tenant()); data["company_id"]=json!(company);
                capture::insert(&mut tx,scope,company,table,data).await
            }
        }.await;
        match result {
            Ok(v) => {
                tx.commit().await?;
                Ok(v)
            }
            Err(e) => {
                tx.rollback().await?;
                Err(e)
            }
        }
    }
}
