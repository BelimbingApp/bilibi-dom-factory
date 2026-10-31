//! Bilimbi ProductionExecution.Capture: shared recorder, retry and evidence
//! persistence. The public facade accepts JSON attributes just as Bilimbi
//! accepts maps; only the owning capture copies its whitelisted fields.
use crate::{Error, ProductionExecution, Result};
use bilibi_database::{Change, MutationTransaction};
use bilibi_tenancy::Scope;
use serde_json::{json, Value};
use tokio_postgres::GenericClient;

#[derive(Clone, Copy)]
pub(crate) enum Table {
    Reasons,
    Roles,
    Types,
    Wastage,
    Labour,
    Measurements,
}
impl Table {
    pub fn name(self) -> &'static str {
        match self {
            Self::Reasons => "factory_wastage_reasons",
            Self::Roles => "factory_labour_roles",
            Self::Types => "factory_measurement_types",
            Self::Wastage => "factory_wastage_records",
            Self::Labour => "factory_labour_entries",
            Self::Measurements => "factory_measurements",
        }
    }
    pub fn missing(self) -> Error {
        Error::Capture(match self {
            Self::Reasons => "Wastage reason not found",
            Self::Roles => "Labour role not found",
            Self::Types => "Measurement type not found",
            Self::Wastage => "Wastage not found",
            Self::Labour => "Labour entry not found",
            Self::Measurements => "Measurement not found",
        })
    }
    fn fields(self) -> &'static str {
        match self {
            Self::Reasons | Self::Roles => "tenant_id,company_id,code,label,active",
            Self::Types => {
                "tenant_id,company_id,code,label,active,value_type,unit,minimum,maximum,target"
            }
            Self::Wastage => {
                "tenant_id,company_id,order_id,execution_id,request_id,request_fingerprint,reason_id,item_id,location_id,identity_id,quantity,unit_id,observation,note,occurred_at,inventory_transaction_id,corrects_id,correction_reason,recorded_by_type,recorded_by_id,recorded_by_acting_for_user_id"
            }
            Self::Labour => {
                "tenant_id,company_id,order_id,execution_id,request_id,request_fingerprint,worker_user_id,role_id,started_at,stopped_at,note,corrects_id,correction_reason,recorded_by_type,recorded_by_id,recorded_by_acting_for_user_id,stopped_by_type,stopped_by_id,stopped_by_acting_for_user_id"
            }
            Self::Measurements => {
                "tenant_id,company_id,order_id,execution_id,request_id,request_fingerprint,measurement_type_id,identity_id,value_type,value,unit,minimum,maximum,target,out_of_range,measured_at,note,corrects_id,correction_reason,recorded_by_type,recorded_by_id,recorded_by_acting_for_user_id"
            }
        }
    }
    fn expression(self) -> String {
        let numeric = match self {
            Self::Wastage => "quantity",
            Self::Types | Self::Measurements => "minimum,maximum,target",
            _ => "",
        };
        let mut expr = "to_jsonb(t)".to_owned();
        for field in numeric.split(',').filter(|s| !s.is_empty()) {
            expr.push_str(&format!(
                " || jsonb_build_object('{field}',t.{field}::text)"
            ));
        }
        if matches!(self, Self::Wastage | Self::Labour | Self::Measurements) {
            expr.push_str(&format!(" || jsonb_build_object('corrected_by_id',(SELECT n.id FROM {} n WHERE n.tenant_id=t.tenant_id AND n.company_id=t.company_id AND n.corrects_id=t.id))",self.name()));
        }
        if matches!(self, Self::Labour) {
            expr.push_str(" || jsonb_build_object('seconds',trunc(extract(epoch FROM t.stopped_at-t.started_at))::bigint)");
        }
        expr
    }
}
pub(crate) fn public(mut row: Value) -> Value {
    let map = row.as_object_mut().expect("stored row");
    for key in ["tenant_id", "company_id", "request_fingerprint"] {
        map.remove(key);
    }
    row
}
pub(crate) fn id(v: &Value, key: &str) -> Result<i64> {
    v[key]
        .as_i64()
        .filter(|id| *id > 0)
        .ok_or(Error::Capture("Invalid capture reference"))
}
pub(crate) fn optional_id(v: &Value, key: &str) -> Result<Option<i64>> {
    if v[key].is_null() {
        Ok(None)
    } else {
        id(v, key).map(Some)
    }
}
pub(crate) fn text(v: &Value, key: &str, required: bool, error: &'static str) -> Result<Value> {
    match &v[key] {
        Value::Null if !required => Ok(Value::Null),
        Value::String(s) if s.trim().is_empty() && !required => Ok(Value::Null),
        Value::String(s) if !s.trim().is_empty() && s.trim().len() <= 10_000 => Ok(json!(s.trim())),
        _ => Err(Error::Capture(error)),
    }
}
pub(crate) fn request(v: &Value) -> Result<&str> {
    v["request_id"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 240)
        .ok_or(Error::Capture("Invalid request id"))
}
pub(crate) async fn fingerprint(
    c: &(impl GenericClient + Sync),
    action: &str,
    target: i64,
    attrs: &Value,
) -> Result<String> {
    let value = json!([action, target, attrs]).to_string();
    Ok(c.query_one(
        "SELECT encode(sha256(convert_to($1,'UTF8')),'hex')",
        &[&value],
    )
    .await?
    .get(0))
}
pub(crate) async fn get(
    c: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    table: Table,
    key: i64,
) -> Result<Value> {
    c.query_opt(
        &format!(
            "SELECT {} FROM {} t WHERE tenant_id=$1 AND company_id=$2 AND id=$3",
            table.expression(),
            table.name()
        ),
        &[&scope.tenant(), &company, &key],
    )
    .await?
    .map(|r| r.get(0))
    .ok_or(table.missing())
}
pub(crate) async fn list(
    c: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    table: Table,
    parent: Option<i64>,
    active: Option<bool>,
) -> Result<Vec<Value>> {
    let (filter, order) = match table {
        Table::Wastage | Table::Measurements => ("execution_id=$3", "id"),
        Table::Labour => ("order_id=$3", "started_at,id"),
        _ => ("($4::boolean IS NULL OR active=$4)", "code"),
    };
    // Both optional parameters are typed even when the table doesn't use one.
    let sql = format!(
        "SELECT {} FROM {} t WHERE tenant_id=$1 AND company_id=$2 AND ($3::bigint IS NULL OR true) AND ($4::boolean IS NULL OR true) AND {filter} ORDER BY {order}",
        table.expression(),
        table.name()
    );
    Ok(
        c.query(&sql, &[&scope.tenant(), &company, &parent, &active])
            .await?
            .iter()
            .map(|r| public(r.get(0)))
            .collect(),
    )
}
pub(crate) async fn replay(
    c: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    table: Table,
    request: &str,
    fingerprint: &str,
) -> Result<Option<Value>> {
    let row = c
        .query_opt(
            &format!(
                "SELECT {} FROM {} t WHERE tenant_id=$1 AND company_id=$2 AND request_id=$3",
                table.expression(),
                table.name()
            ),
            &[&scope.tenant(), &company, &request],
        )
        .await?;
    if let Some(row) = row {
        let value: Value = row.get(0);
        if value["request_fingerprint"] != fingerprint {
            return Err(Error::RequestIdConflict);
        }
        Ok(Some(public(value)))
    } else {
        Ok(None)
    }
}
pub(crate) async fn insert(
    tx: &mut MutationTransaction<'_>,
    scope: &Scope,
    company: i64,
    table: Table,
    mut data: Value,
) -> Result<Value> {
    data["tenant_id"] = json!(scope.tenant());
    data["company_id"] = json!(company);
    let id = tx.mutate(move |c| Box::pin(async move {
        let name=table.name(); let fields=table.fields();
        let row=c.query_one(&format!("INSERT INTO {name}({fields}) SELECT {fields} FROM jsonb_populate_record(NULL::{name},$1) RETURNING id,to_jsonb({name})"),&[&data]).await?;
        let id:i64=row.get(0);
        Ok((id,vec![Change{kind:name.into(),id,old:json!({}),new:row.get(1)}]))
    })).await?;
    Ok(public(get(tx.sql(), scope, company, table, id).await?))
}
pub(crate) fn recorded(scope: &Scope) -> Result<Value> {
    if scope.impersonator().is_some() {
        return Err(Error::Capture("Capture refused under impersonation"));
    }
    if scope.principal().is_some() || scope.actor().person().is_none() {
        return Err(Error::Capture("Capture not authorized"));
    }
    Ok(match scope.agent() {
        Some(agent) => {
            json!({"recorded_by_type":"agent","recorded_by_id":agent.connection(),"recorded_by_acting_for_user_id":scope.user()})
        }
        None => {
            json!({"recorded_by_type":"user","recorded_by_id":scope.user(),"recorded_by_acting_for_user_id":null})
        }
    })
}
pub(crate) async fn writable(
    c: &(impl GenericClient + Sync),
    scope: &Scope,
    company: i64,
    cap: &str,
) -> Result<()> {
    bilibi_company::authorize_work_target(c, scope, company, cap).await?;
    bilibi_company::require_writable(c, scope, company, true).await?;
    Ok(())
}
pub(crate) async fn time(
    c: &(impl GenericClient + Sync),
    value: &Value,
    lower: Option<std::time::SystemTime>,
    error: &'static str,
) -> Result<Value> {
    if !value.is_null() && !value.is_string() {
        return Err(Error::Capture(error));
    }
    let value = value.as_str().map(str::to_owned);
    if let Some(value) = &value {
        let valid: bool = c.query_one(
            "SELECT $1::text ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]+)?([Zz]|[+-][0-9]{2}:[0-9]{2})$'",
            &[value],
        ).await?.get(0);
        if !valid {
            return Err(Error::Capture(error));
        }
    }
    let row=c.query_one("SELECT to_jsonb(t),isfinite(t) AND t<=clock_timestamp() AND ($2::timestamptz IS NULL OR t >= $2) FROM (SELECT COALESCE($1::text::timestamptz,clock_timestamp()) t) s", &[&value,&lower]).await.map_err(|_| Error::Capture(error))?;
    if !row.get::<_, bool>(1) {
        return Err(Error::Capture(error));
    }
    Ok(row.get(0))
}
pub(crate) fn merge(row: &mut Value, values: Value) {
    row.as_object_mut()
        .expect("row")
        .extend(values.as_object().expect("values").clone());
}
pub(crate) async fn finish(
    tx: MutationTransaction<'_>,
    result: Result<(Value, bool)>,
) -> Result<Value> {
    match result {
        Ok((v, true)) => {
            tx.commit().await?;
            Ok(v)
        }
        Ok((v, false)) => {
            tx.rollback().await?;
            Ok(v)
        }
        Err(e) => {
            tx.rollback().await?;
            Err(e)
        }
    }
}
pub(crate) fn intent(attrs: &Value, keys: &[&str]) -> Result<Value> {
    let map = attrs.as_object().ok_or(Error::Capture("Invalid capture"))?;
    Ok(Value::Object(
        map.iter()
            .filter(|(k, _)| keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ))
}
impl ProductionExecution {
    pub(crate) async fn capture_authorize(
        &self,
        scope: &Scope,
        company: i64,
        cap: &str,
    ) -> Result<Value> {
        let recorder = recorded(scope)?;
        let c = self.db.get().await.map_err(bilibi_database::Error::from)?;
        bilibi_company::authorize_work_target(&**c, scope, company, cap)
            .await
            .map_err(|_| Error::Capture("Capture not authorized"))?;
        Ok(recorder)
    }
}
