//! Bilimbi ProductionExecution.Labour: time belongs to a worker and order,
//! optionally a run. The company lock serializes intervals across all orders.
use crate::{
    capture::{self, Table},
    contributions::{LABOUR_MANAGE, LABOUR_RECORD},
    orders, Error, ProductionExecution, Result,
};
use bilibi_database::Change;
use bilibi_tenancy::Scope;
use serde_json::{json, Value};

fn capability(scope: &Scope, worker: i64) -> &'static str {
    if scope.agent().is_none() && worker == scope.user() {
        LABOUR_RECORD
    } else {
        LABOUR_MANAGE
    }
}
impl ProductionExecution {
    pub async fn clock_in(
        &self,
        scope: &Scope,
        company: i64,
        order: i64,
        attrs: &Value,
    ) -> Result<Value> {
        self.labour(scope, company, order, None, true, attrs).await
    }
    pub async fn record_labour(
        &self,
        scope: &Scope,
        company: i64,
        order: i64,
        attrs: &Value,
    ) -> Result<Value> {
        self.labour(scope, company, order, None, false, attrs).await
    }
    pub async fn correct_labour(
        &self,
        scope: &Scope,
        company: i64,
        entry: i64,
        attrs: &Value,
    ) -> Result<Value> {
        self.labour(scope, company, entry, Some(entry), false, attrs)
            .await
    }
    pub async fn list_labour(&self, scope: &Scope, company: i64, order: i64) -> Result<Vec<Value>> {
        let c = self.read_client(scope, company).await?;
        orders::get_order(&**c, scope, company, order).await?;
        capture::list(&**c, scope, company, Table::Labour, Some(order), None).await
    }
    pub async fn labour_summary(&self, scope: &Scope, company: i64, order: i64) -> Result<Value> {
        let entries = self.list_labour(scope, company, order).await?;
        let mut total = 0_i64;
        let mut open = Vec::new();
        let mut runs = std::collections::BTreeMap::<Option<i64>, (i64, i64)>::new();
        let mut workers = std::collections::BTreeMap::<i64, (i64, i64)>::new();
        for e in entries
            .into_iter()
            .filter(|e| e["corrected_by_id"].is_null())
        {
            if let Some(seconds) = e["seconds"].as_i64() {
                total += seconds;
                let r = runs.entry(e["execution_id"].as_i64()).or_default();
                r.0 += seconds;
                r.1 += 1;
                let w = workers
                    .entry(capture::id(&e, "worker_user_id")?)
                    .or_default();
                w.0 += seconds;
                w.1 += 1;
            } else {
                open.push(e);
            }
        }
        Ok(
            json!({"total_seconds":total,"open":open,"runs":runs.into_iter().map(|(id,(seconds,entries))|json!({"execution_id":id,"seconds":seconds,"entries":entries})).collect::<Vec<_>>(),"workers":workers.into_iter().map(|(id,(seconds,entries))|json!({"worker_user_id":id,"seconds":seconds,"entries":entries})).collect::<Vec<_>>()}),
        )
    }
    async fn labour(
        &self,
        scope: &Scope,
        company: i64,
        target: i64,
        correction: Option<i64>,
        clock: bool,
        attrs: &Value,
    ) -> Result<Value> {
        let worker = if correction.is_none() {
            if attrs["worker_user_id"].is_null() && scope.agent().is_none() {
                scope.user()
            } else {
                capture::id(attrs, "worker_user_id")
                    .map_err(|_| Error::Capture("Worker not found"))?
            }
        } else {
            0
        };
        let cap = if correction.is_some() {
            LABOUR_MANAGE
        } else {
            capability(scope, worker)
        };
        let recorder = self.capture_authorize(scope, company, cap).await?;
        let request = capture::request(attrs)?.to_owned();
        let mut c = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(
                &mut c,
                scope.mutation_context(if correction.is_some() {
                    "factory.production_execution.correct_labour"
                } else if clock {
                    "factory.production_execution.clock_in"
                } else {
                    "factory.production_execution.record_labour"
                }),
            )
            .await?;
        let result=async{
            capture::writable(tx.sql(),scope,company,cap).await?;
            let mut row=if let Some(id)=correction{
                capture::get(tx.sql(),scope,company,Table::Labour,id).await?
            }else{
                orders::get_order(tx.sql(),scope,company,target).await?;
                bilibi_user::UserService::company_account_on(tx.sql(),scope,company,worker).await.map_err(|_|Error::Capture("Worker not found"))?;
                json!({"order_id":target,"worker_user_id":worker})
            };
            let old_role=row["role_id"].as_i64();
            if correction.is_none() || attrs.get("role_id").is_some(){row["role_id"]=json!(capture::id(attrs,"role_id")?);}
            let role=capture::get(tx.sql(),scope,company,Table::Roles,capture::id(&row,"role_id")?).await?;
            if role["active"]!=true && old_role!=row["role_id"].as_i64(){return Err(Error::Capture("Labour role inactive"));}
            if correction.is_none() || attrs.get("execution_id").is_some(){row["execution_id"]=json!(capture::optional_id(attrs,"execution_id")?);}
            if let Some(run)=row["execution_id"].as_i64(){
                if orders::get_execution(tx.sql(),scope,company,run).await?.order_id!=capture::id(&row,"order_id")?{return Err(Error::ExecutionNotFound);}
            }
            if clock{
                if !attrs["started_at"].is_null() || !attrs["stopped_at"].is_null(){return Err(Error::Capture("Invalid labour times"));}
                row["started_at"]=capture::time(tx.sql(),&Value::Null,None,"Invalid labour times").await?;row["stopped_at"]=Value::Null;
            }else{
                if correction.is_none() && (attrs["started_at"].is_null() || attrs["stopped_at"].is_null()){return Err(Error::Capture("Invalid labour times"));}
                for key in ["started_at","stopped_at"]{
                    if let Some(v)=attrs.get(key){
                        row[key]=if v.is_null() && key=="stopped_at"{Value::Null}else{
                            if v.is_null(){return Err(Error::Capture("Invalid labour times"));}
                            capture::time(tx.sql(),v,None,"Invalid labour times").await?
                        };
                    }
                }
            }
            if correction.is_none() || attrs.get("note").is_some(){row["note"]=capture::text(attrs,"note",false,"Invalid note")?;}
            row["request_id"]=json!(request);row["corrects_id"]=json!(correction);
            row["correction_reason"]=if correction.is_some(){capture::text(attrs,"correction_reason",true,"Correction reason required")?}else{Value::Null};
            let mut command=capture::intent(&row,&["request_id","order_id","worker_user_id","role_id","note","corrects_id","correction_reason","started_at","stopped_at"])?;
            command["execution_id"]=row["execution_id"].clone();
            if clock{command["started_at"]=Value::Null;}
            let fp=capture::fingerprint(tx.sql(),if correction.is_some(){"labour_correction"}else if clock{"clock_in"}else{"labour"},target,&command).await?;
            if let Some(v)=capture::replay(tx.sql(),scope,company,Table::Labour,&request,&fp).await?{return Ok((v,false));}
            if !row["corrected_by_id"].is_null(){return Err(Error::Capture("Labour entry corrected"));}
            let worker=capture::id(&row,"worker_user_id")?;
            let start=row["started_at"].as_str().ok_or(Error::Capture("Invalid labour times"))?;
            let stop=row["stopped_at"].as_str();
            if let Some(stop)=stop{
                if !tx.sql().query_one("SELECT $1::text::timestamptz < $2::text::timestamptz",&[&start,&stop]).await?.get::<_,bool>(0){return Err(Error::Capture("Invalid labour times"));}
            }
            let conflict=tx.sql().query_opt("SELECT e.stopped_at IS NULL FROM factory_labour_entries e WHERE e.tenant_id=$1 AND e.company_id=$2 AND e.worker_user_id=$3 AND ($4::bigint IS NULL OR e.id<>$4) AND NOT EXISTS(SELECT 1 FROM factory_labour_entries n WHERE n.tenant_id=e.tenant_id AND n.company_id=e.company_id AND n.corrects_id=e.id) AND (($6::text IS NULL AND e.stopped_at IS NULL) OR (e.started_at<COALESCE($6::text::timestamptz,clock_timestamp()) AND COALESCE(e.stopped_at,clock_timestamp())>$5::text::timestamptz)) ORDER BY e.id LIMIT 1",&[&scope.tenant(),&company,&worker,&correction,&start,&stop]).await?;
            if let Some(conflict)=conflict{return Err(Error::Capture(if stop.is_none() && conflict.get::<_,bool>(0){"Worker already clocked in"}else{"Labour overlap"}));}
            let closed=stop.is_some();
            row["request_fingerprint"]=json!(fp);
            for (stopper,recorded) in [("stopped_by_type","recorded_by_type"),("stopped_by_id","recorded_by_id"),("stopped_by_acting_for_user_id","recorded_by_acting_for_user_id")]{row[stopper]=if closed{recorder[recorded].clone()}else{Value::Null};}
            capture::merge(&mut row,recorder);
            Ok((capture::insert(&mut tx,scope,company,Table::Labour,row).await?,true))
        }.await;
        capture::finish(tx, result).await
    }
    pub async fn clock_out(&self, scope: &Scope, company: i64, entry: i64) -> Result<Value> {
        capture::recorded(scope, company)?;
        let c = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let old = capture::get(&**c, scope, company, Table::Labour, entry).await?;
        drop(c);
        let cap = capability(scope, capture::id(&old, "worker_user_id")?);
        let recorder = self.capture_authorize(scope, company, cap).await?;
        let mut c = self.db.get().await.map_err(bilibi_database::Error::from)?;
        let mut tx = self
            .mutations
            .begin(
                &mut c,
                scope.mutation_context("factory.production_execution.clock_out"),
            )
            .await?;
        let result=async{
            capture::writable(tx.sql(),scope,company,cap).await?;
            let old=capture::get(tx.sql(),scope,company,Table::Labour,entry).await?;
            if !old["corrected_by_id"].is_null(){return Err(Error::Capture("Labour entry corrected"));}
            if !old["stopped_at"].is_null(){return Err(Error::Capture("Labour entry closed"));}
            let tenant=scope.tenant();
            let old:Value=tx.sql().query_one("SELECT to_jsonb(t) FROM factory_labour_entries t WHERE tenant_id=$1 AND company_id=$2 AND id=$3",&[&tenant,&company,&entry]).await?.get(0);
            tx.mutate(move |c|Box::pin(async move{
                let kind=recorder["recorded_by_type"].as_str().unwrap();let who=recorder["recorded_by_id"].as_i64().unwrap();let acting=recorder["recorded_by_acting_for_user_id"].as_i64();
                let new:Value=c.query_one("UPDATE factory_labour_entries SET stopped_at=clock_timestamp(),stopped_by_type=$4,stopped_by_id=$5,stopped_by_acting_for_user_id=$6 WHERE tenant_id=$1 AND company_id=$2 AND id=$3 RETURNING to_jsonb(factory_labour_entries)",&[&tenant,&company,&entry,&kind,&who,&acting]).await?.get(0);
                Ok(((),vec![Change{kind:"factory_labour_entries".into(),id:entry,old,new}]))
            })).await?;
            Ok((capture::public(capture::get(tx.sql(),scope,company,Table::Labour,entry).await?),true))
        }.await;
        capture::finish(tx, result).await
    }
}
