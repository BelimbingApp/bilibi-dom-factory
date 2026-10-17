//! XP4 runtime proof through registered pages and audited facade mutations.
//! Run after production-execution, shop-floor and floor-pages.
use bilibi_authz::grants::Grants;
use bilibi_database::Error as DbError;
use bilibi_factory_production_execution::ProductionExecution;
use bilibi_tenancy::Scope;
use bilibi_ui::page::{LiveBody, LiveInput, LiveRequest, LiveState, PageRequest, PageService};
use serde_json::{json, Value};
use std::collections::HashMap;

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
type Error = Box<dyn std::error::Error>;
type State = Box<dyn LiveState<Scope, DbError>>;
fn verify(label: &str, yes: bool) -> Result<(), Error> {
    if !yes {
        return Err(format!("{label}: unexpected runtime result").into());
    }
    println!("{label}: verified");
    Ok(())
}
async fn login(app: &bilibi_web::App, email: &str) -> Result<Scope, Error> {
    let login = app
        .user
        .login(email, bilibi_walkthrough_fixture::PASSWORD)
        .await?;
    Ok(app.security.guard(&login.token).await?)
}
async fn event(state: &mut State, scope: &Scope, p: Value) -> Result<LiveBody, Error> {
    Ok(state
        .update(LiveRequest {
            scope,
            credential: "",
            input: LiveInput::Event(&p),
        })
        .await?)
}
async fn mount(
    page: &PageService<Scope, DbError>,
    scope: &Scope,
    query: &HashMap<String, String>,
    operation: &'static str,
) -> Result<State, Error> {
    Ok(page
        .0
        .mount(PageRequest {
            operation,
            scope: Some(scope),
            path: &HashMap::new(),
            query,
            body: &[],
            framed: false,
        })
        .await?
        .ok_or("floor not live")?)
}
fn form(event: &str, name: &str, fields: &[(&str, Value)]) -> Value {
    let mut p = json!({"type":event});
    for (key, value) in fields {
        p[format!("{name}[{key}]")] = value.clone();
    }
    p
}
fn id(v: &Value) -> i64 {
    v["id"].as_i64().expect("id")
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    let origin = std::env::var("APP_ORIGIN").unwrap_or_else(|_| "http://localhost:35594".into());
    let app = bilibi_web::App::new(db.clone(), &origin)?;
    let manager = login(&app, "manager@example.test").await?;
    let admin = login(&app, "admin@example.test").await?;
    let viewer = login(&app, "viewer@example.test").await?;
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    let pe = app
        .services
        .get::<ProductionExecution>(bilibi_factory_production_execution::SERVICE)?;
    let page = app
        .services
        .get::<PageService<Scope, DbError>>("factory/production_execution.pages")?;
    let company = manager
        .actor()
        .organisation()
        .ok_or("manager company")?
        .get();
    let c = db.get().await?;
    let order:i64=c.query_one("SELECT id FROM factory_production_orders WHERE tenant_id=$1 AND company_id=$2 AND code='ORD-1'",&[&manager.tenant(),&company]).await?.get(0);
    let run:i64=c.query_one("SELECT id FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=$2 AND request_id='run-1'",&[&manager.tenant(),&company]).await?.get(0);
    drop(c);
    let empty = HashMap::new();
    for (operation, code, noun) in [
        ("wastage-reasons", "XP4_REASON", "Wastage reason"),
        ("labour-roles", "XP4_ROLE", "Labour role"),
        ("measurement-types", "XP4_TYPE", "Measurement type"),
    ] {
        let mut state = mount(&page, &manager, &empty, operation).await?;
        let initial = event(&mut state, &manager, json!({"type":"new"})).await?;
        verify(
            &format!("{operation} opens editor"),
            initial.html.contains("data-event=\"save_entry\""),
        )?;
        let name = if operation == "measurement-types" {
            "type"
        } else {
            "entry"
        };
        let body = event(
            &mut state,
            &manager,
            form(
                "save_entry",
                name,
                &[
                    ("code", json!(code.to_lowercase())),
                    ("label", json!("Page <entry>")),
                    ("value_type", json!("decimal")),
                    ("unit", json!("mm")),
                    ("minimum", json!("1")),
                    ("maximum", json!("3")),
                    ("target", json!("2")),
                ],
            ),
        )
        .await?;
        verify(
            &format!("{operation} saves escaped normalized data"),
            body.html.contains(&format!("{noun} saved."))
                && body.html.contains("Page &#60;entry&#62;")
                && body.html.contains(code),
        )?;
        let rows = match operation {
            "wastage-reasons" => pe.list_wastage_reasons(&manager, company, None).await?,
            "labour-roles" => pe.list_labour_roles(&manager, company, None).await?,
            _ => pe.list_measurement_types(&manager, company, None).await?,
        };
        let row = rows
            .iter()
            .find(|r| r["code"] == code)
            .ok_or("created code")?;
        let entry = id(row);
        let edited = event(
            &mut state,
            &manager,
            json!({"type":"edit","id":entry.to_string()}),
        )
        .await?;
        verify(
            &format!("{operation} immutable code disabled"),
            edited.html.contains("disabled"),
        )?;
        let body = event(
            &mut state,
            &manager,
            form(
                "save_entry",
                name,
                &[
                    ("code", json!("FORGED")),
                    ("label", json!("Page entry updated")),
                    ("value_type", json!("boolean")),
                    ("unit", json!("forged")),
                    ("minimum", json!("0")),
                    ("maximum", json!("4")),
                    ("target", json!("2")),
                ],
            ),
        )
        .await?;
        verify(
            &format!("{operation} update ignores immutable input"),
            body.html.contains(code)
                && !body.html.contains("FORGED")
                && body.html.contains("Page entry updated"),
        )?;
        for _ in 0..2 {
            event(
                &mut state,
                &manager,
                json!({"type":"toggle_active","id":entry.to_string()}),
            )
            .await?;
        }
        let body = event(
            &mut state,
            &manager,
            json!({"type":"grid","op":"zoom_preset","preset":"compact"}),
        )
        .await?;
        verify(
            &format!("{operation} compact columns"),
            body.html.contains("data-mode=\"compact\""),
        )?;
        let mut restricted = mount(&page, &viewer, &empty, operation).await?;
        let denied = event(&mut restricted, &viewer, json!({"type":"new"})).await?;
        verify(
            &format!("{operation} forged editor refused"),
            denied.html.contains("permission to manage")
                && !denied.html.contains("data-event=\"save_entry\""),
        )?;
        // A mounted manager loses the grant before a write-shaped event.
        grants
            .set_direct(
                &admin,
                manager.user(),
                "factory.production-execution.configuration.manage",
                Some(false),
            )
            .await?;
        let denied = event(
            &mut state,
            &manager,
            json!({"type":"toggle_active","id":entry.to_string()}),
        )
        .await?;
        verify(
            &format!("{operation} stale grant refused"),
            denied.html.contains("permission to manage"),
        )?;
        grants
            .set_direct(
                &admin,
                manager.user(),
                "factory.production-execution.configuration.manage",
                Some(true),
            )
            .await?;
    }
    let role = pe
        .list_labour_roles(&manager, company, None)
        .await?
        .into_iter()
        .find(|r| r["code"] == "XP4_ROLE")
        .ok_or("role")?;
    let query = HashMap::from([("order".into(), order.to_string())]);
    let mut floor = mount(&page, &manager, &query, "floor").await?;
    let body = event(
        &mut floor,
        &manager,
        json!({"type":"pick_role","id":id(&role).to_string()}),
    )
    .await?;
    verify(
        "labour before run selection",
        body.html.contains("labour-table")
            && body.html.contains("labour-form")
            && !body.html.contains("labour-entry-form"),
    )?;
    let clock = form(
        "clock_in",
        "labour",
        &[
            ("request_id", json!("xp4-own-clock")),
            ("role_id", json!(id(&role).to_string())),
        ],
    );
    let body = event(&mut floor, &manager, clock.clone()).await?;
    verify("own clock in", body.html.contains("Clocked in."))?;
    event(&mut floor, &manager, clock).await?;
    let rows = pe.list_labour(&manager, company, order).await?;
    verify(
        "clock retry one row",
        rows.iter()
            .filter(|v| v["request_id"] == "xp4-own-clock")
            .count()
            == 1,
    )?;
    let clock = rows
        .iter()
        .find(|v| v["request_id"] == "xp4-own-clock")
        .ok_or("clock")?;
    let body = event(
        &mut floor,
        &manager,
        json!({"type":"clock_out","id":id(clock).to_string()}),
    )
    .await?;
    verify("own clock out", body.html.contains("Clocked out."))?;
    let denied_other = event(
        &mut floor,
        &manager,
        form(
            "clock_in",
            "labour",
            &[
                ("request_id", json!("xp4-forged-worker")),
                ("role_id", json!(id(&role).to_string())),
                ("worker_user_id", json!(viewer.user().to_string())),
            ],
        ),
    )
    .await?;
    verify(
        "own grant cannot clock another worker",
        !denied_other.html.contains("Clocked in.")
            && pe
                .list_labour(&manager, company, order)
                .await?
                .iter()
                .all(|r| r["request_id"] != "xp4-forged-worker"),
    )?;
    let mut restricted = mount(&page, &viewer, &query, "floor").await?;
    let body = event(&mut restricted, &viewer, json!({"type":"clock_in"})).await?;
    verify(
        "ungranted own clock refused",
        body.html.contains("permission to record labour")
            && !body.html.contains("id=\"labour-form\""),
    )?;
    grants
        .set_direct(
            &admin,
            manager.user(),
            "factory.production-execution.labour.manage",
            Some(true),
        )
        .await?;
    let body = event(
        &mut floor,
        &manager,
        json!({"type":"pick_run","id":run.to_string()}),
    )
    .await?;
    verify(
        "supervisor run controls",
        body.html.contains("labour-entry-form") && body.html.contains("Clock in on this run"),
    )?;
    let clock = form(
        "clock_in",
        "labour",
        &[
            ("request_id", json!("xp4-supervisor-clock")),
            ("role_id", json!(id(&role).to_string())),
            ("worker_user_id", json!(viewer.user().to_string())),
        ],
    );
    let body = event(&mut floor, &manager, clock).await?;
    verify(
        "supervisor clocks another worker",
        body.html.contains("Clocked in."),
    )?;
    let rows = pe.list_labour(&manager, company, order).await?;
    let clock = rows
        .iter()
        .find(|v| v["request_id"] == "xp4-supervisor-clock")
        .ok_or("supervisor clock")?;
    verify(
        "supervisor clock assigned run and worker",
        clock["execution_id"] == run && clock["worker_user_id"] == viewer.user(),
    )?;
    event(
        &mut floor,
        &manager,
        json!({"type":"clock_out","id":id(clock).to_string()}),
    )
    .await?;
    let before = pe.labour_summary(&manager, company, order).await?["total_seconds"]
        .as_i64()
        .ok_or("total")?;
    let entry = form(
        "save_labour_entry",
        "entry",
        &[
            ("request_id", json!("xp4-finished")),
            ("worker_user_id", json!(viewer.user().to_string())),
            ("role_id", json!(id(&role).to_string())),
            ("started_at", json!("2026-01-05T08:00")),
            ("stopped_at", json!("2026-01-05T09:30")),
            ("note", json!("Supervisor <entry>")),
        ],
    );
    let body = event(&mut floor, &manager, entry.clone()).await?;
    verify(
        "supervisor finished entry",
        body.html.contains("Labour recorded.") && body.html.contains("1h 30m"),
    )?;
    event(&mut floor, &manager, entry).await?;
    let rows = pe.list_labour(&manager, company, order).await?;
    let worked = rows
        .iter()
        .find(|v| v["request_id"] == "xp4-finished")
        .ok_or("finished entry")?;
    verify(
        "company time and exact total",
        s_timestamp(&worked["started_at"])? == "2026-01-05T00:00:00Z"
            && pe.labour_summary(&manager, company, order).await?["total_seconds"].as_i64()
                == Some(before + 5400),
    )?;
    let body = event(
        &mut floor,
        &manager,
        json!({"type":"start_labour_correction","id":id(worked).to_string()}),
    )
    .await?;
    verify(
        "correction displays company wall time",
        body.html.contains("2026-01-05T08:00"),
    )?;
    let body = event(
        &mut floor,
        &manager,
        form(
            "save_labour_correction",
            "labour_correction",
            &[
                ("stopped_at", json!("2026-01-05T09:00")),
                ("correction_reason", json!("Checked time")),
            ],
        ),
    )
    .await?;
    verify(
        "labour correction and revised total",
        body.html.contains("Labour corrected.")
            && body.html.contains("Checked time")
            && pe.labour_summary(&manager, company, order).await?["total_seconds"].as_i64()
                == Some(before + 3600),
    )?;
    let rows = pe.list_labour(&manager, company, order).await?;
    let corrected = rows
        .iter()
        .find(|v| v["corrects_id"] == id(worked))
        .ok_or("correction")?;
    verify(
        "correction preserves run worker start",
        corrected["execution_id"] == run
            && corrected["worker_user_id"] == viewer.user()
            && corrected["started_at"] == worked["started_at"],
    )?;
    let precise=pe.record_labour(&manager,company,order,&json!({"request_id":"xp4-precise","worker_user_id":viewer.user(),"role_id":id(&role),"execution_id":run,"started_at":"2026-01-06T00:00:42.123456Z","stopped_at":"2026-01-06T01:30:55.123456Z"})).await?;
    floor
        .update(LiveRequest {
            scope: &manager,
            credential: "",
            input: LiveInput::Refresh,
        })
        .await?;
    event(
        &mut floor,
        &manager,
        json!({"type":"start_labour_correction","id":id(&precise).to_string()}),
    )
    .await?;
    let body = event(
        &mut floor,
        &manager,
        form(
            "save_labour_correction",
            "labour_correction",
            &[
                ("started_at", json!("2026-01-06T08:00")),
                ("stopped_at", json!("2026-01-06T09:30")),
                ("correction_reason", json!("Keep recorded instants")),
            ],
        ),
    )
    .await?;
    let rows = pe.list_labour(&manager, company, order).await?;
    let kept = rows
        .iter()
        .find(|r| r["corrects_id"] == id(&precise))
        .ok_or("precision correction")?;
    verify(
        "unchanged minute correction retains seconds and subseconds",
        body.html.contains("Labour corrected.")
            && kept["started_at"] == precise["started_at"]
            && kept["stopped_at"] == precise["stopped_at"],
    )?;
    grants
        .set_direct(
            &admin,
            manager.user(),
            "factory.production-execution.labour.manage",
            Some(false),
        )
        .await?;
    let denied = event(
        &mut floor,
        &manager,
        json!({"type":"start_labour_correction","id":id(corrected).to_string()}),
    )
    .await?;
    verify(
        "correction start rechecks revoked grant",
        denied.html.contains("permission to manage labour")
            && !denied.html.contains("id=\"labour-correction-form\""),
    )?;
    grants
        .set_direct(
            &admin,
            manager.user(),
            "factory.production-execution.labour.manage",
            Some(true),
        )
        .await?;
    event(
        &mut floor,
        &manager,
        json!({"type":"pick_run","id":run.to_string()}),
    )
    .await?;
    let invalid = event(
        &mut floor,
        &manager,
        form(
            "save_labour_entry",
            "entry",
            &[("started_at", json!("bad"))],
        ),
    )
    .await?;
    verify(
        "bad datetime is nonfatal",
        invalid.html.contains("Enter a start and stop time."),
    )?;
    let c = db.get().await?;
    c.execute(
        "UPDATE companies SET status='archived' WHERE tenant_id=$1 AND id=$2",
        &[&manager.tenant(), &company],
    )
    .await?;
    let archived = event(
        &mut floor,
        &manager,
        form(
            "clock_in",
            "labour",
            &[("role_id", json!(id(&role).to_string()))],
        ),
    )
    .await;
    c.execute(
        "UPDATE companies SET status='active' WHERE tenant_id=$1 AND id=$2",
        &[&manager.tenant(), &company],
    )
    .await?;
    verify(
        "archived company clock refused",
        !archived?.html.contains("Clocked in."),
    )?;
    drop(c);
    println!("labour/configuration page walkthrough complete; order={order} run={run}");
    if std::env::var("BILIBI_FLOOR_SERVE").as_deref() == Ok("yes") {
        let listener = tokio::net::TcpListener::bind(std::env::var("BIND_ADDR")?).await?;
        println!("Browser proof listening at {origin}/factory/floor?order={order}&run={run}");
        axum::serve(listener, bilibi_web::router(app)).await?;
    }
    Ok(())
}
fn s_timestamp(value: &Value) -> Result<String, Error> {
    Ok(value
        .as_str()
        .ok_or("timestamp")?
        .parse::<jiff::Timestamp>()?
        .to_string())
}
