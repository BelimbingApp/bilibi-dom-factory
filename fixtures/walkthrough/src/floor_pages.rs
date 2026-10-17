//! XP3 runtime proof through the registered PageService and real facade writes.
//! Run after production-execution and shop-floor on the same disposable DB.
use bilibi_authz::grants::Grants;
use bilibi_database::Error as DbError;
use bilibi_factory_inventory::Inventory;
use bilibi_factory_production_execution::ProductionExecution;
use bilibi_tenancy::Scope;
use bilibi_ui::page::{LiveBody, LiveInput, LiveRequest, LiveState, PageRequest, PageService};
use serde_json::{json, Value};
use std::collections::HashMap;

#[path = "../../../production_execution/src/web/time.rs"]
mod entry_time;

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
) -> Result<State, Error> {
    Ok(page
        .0
        .mount(PageRequest {
            operation: "floor",
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
fn clock_proof() -> Result<(), Error> {
    use bilibi_datetime::{DisplayContext, Mode};
    let mut display = DisplayContext {
        mode: Mode::Company,
        timezone: "Asia/Kuala_Lumpur".into(),
        locale: "en-MY",
    };
    verify(
        "company wall time becomes UTC",
        entry_time::parse(&display, "2026-01-05T08:00")
            .map(|t| t.to_string())
            .as_deref()
            == Some("2026-01-05T00:00:00Z"),
    )?;
    let original = "2026-01-05T00:00:42.123456Z".parse::<jiff::Timestamp>()?;
    verify(
        "unchanged minute preserves original precision",
        entry_time::corrected(&display, "2026-01-05T08:00", Some(original)) == Some(Some(original)),
    )?;
    verify(
        "empty correction keeps an open stop",
        entry_time::corrected(&display, "", None) == Some(None),
    )?;
    display.mode = Mode::Utc;
    verify(
        "UTC entry follows saved mode",
        entry_time::parse(&display, "2026-01-05T08:00")
            .map(|t| t.to_string())
            .as_deref()
            == Some("2026-01-05T08:00:00Z"),
    )?;
    display.mode = Mode::Local;
    verify(
        "local display entry uses company zone",
        entry_time::parse(&display, "2026-01-05T08:00")
            .map(|t| t.to_string())
            .as_deref()
            == Some("2026-01-05T00:00:00Z"),
    )?;
    display.timezone = "America/New_York".into();
    verify(
        "daylight-saving gap refused",
        entry_time::parse(&display, "2026-03-08T02:30").is_none(),
    )?;
    verify(
        "daylight-saving fold picks earlier instant",
        entry_time::parse(&display, "2026-11-01T01:30")
            .map(|t| t.to_string())
            .as_deref()
            == Some("2026-11-01T05:30:00Z"),
    )?;
    verify(
        "malformed entry refused",
        entry_time::parse(&display, "not a date").is_none(),
    )
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    clock_proof()?;
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    let origin = std::env::var("APP_ORIGIN").unwrap_or_else(|_| "http://localhost:35593".into());
    let app = bilibi_web::App::new(db.clone(), &origin)?;
    let manager = login(&app, "manager@example.test").await?;
    let admin = login(&app, "admin@example.test").await?;
    let viewer = login(&app, "viewer@example.test").await?;
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    let pe = app
        .services
        .get::<ProductionExecution>(bilibi_factory_production_execution::SERVICE)?;
    let inv = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let page = app
        .services
        .get::<PageService<Scope, DbError>>("factory/production_execution.pages")?;
    let company = manager
        .actor()
        .organisation()
        .ok_or("manager company")?
        .get();
    let settings = app
        .services
        .get::<bilibi_settings::Settings>("base/settings.store")?;
    let boundary = app
        .services
        .get::<bilibi_database::MutationBoundary>("base/database.mutations")?;
    let mut c = db.get().await?;
    let mut tx = boundary
        .begin(
            &mut c,
            manager.mutation_context("factory.walkthrough.floor_timezone"),
        )
        .await?;
    settings
        .put(
            &mut tx,
            &manager,
            bilibi_settings::Target::Company(
                manager.actor().organisation().ok_or("manager company")?,
            ),
            bilibi_datetime::TIMEZONE_KEY,
            json!("Asia/Kuala_Lumpur"),
        )
        .await?;
    app.services
        .get::<bilibi_datetime::DisplayPolicy>("base/datetime.display")?
        .put_mode(&mut tx, &manager, "company")
        .await?;
    tx.commit().await?;
    drop(c);
    let c = db.get().await?;
    let order:i64=c.query_one("SELECT id FROM factory_production_orders WHERE tenant_id=$1 AND company_id=$2 AND code='ORD-1'",&[&manager.tenant(),&company]).await?.get(0);
    let run:i64=c.query_one("SELECT id FROM factory_operation_executions WHERE tenant_id=$1 AND company_id=$2 AND request_id='run-1'",&[&manager.tenant(),&company]).await?.get(0);
    drop(c);
    let execution = pe.get_execution(&manager, company, run).await?;
    let posting = inv
        .get_transaction(&manager, company, execution.inventory_transaction_id)
        .await?;
    let output = posting
        .entries
        .iter()
        .find(|e| {
            e.role == "stock" && !e.native_quantity.starts_with('-') && e.identity_id.is_some()
        })
        .ok_or("run output")?;
    let reason = pe
        .create_wastage_reason(
            &manager,
            company,
            &json!({"code":"XP3_REASON","label":"Capture reason"}),
        )
        .await?;
    let decimal=pe.create_measurement_type(&manager,company,&json!({"code":"XP3_DECIMAL","label":"Output measure","value_type":"decimal","unit":"mm","minimum":"1","maximum":"3","target":"2"})).await?;
    let boolean = pe
        .create_measurement_type(
            &manager,
            company,
            &json!({"code":"XP3_BOOLEAN","label":"Output check","value_type":"boolean"}),
        )
        .await?;
    let integer = pe
        .create_measurement_type(
            &manager,
            company,
            &json!({"code":"XP3_INTEGER","label":"Output count","value_type":"integer"}),
        )
        .await?;
    let text = pe
        .create_measurement_type(
            &manager,
            company,
            &json!({"code":"XP3_TEXT","label":"Output description","value_type":"string"}),
        )
        .await?;
    let empty = HashMap::new();
    let mut state = mount(&page, &manager, &empty).await?;
    let initial = state
        .update(LiveRequest {
            scope: &manager,
            credential: "",
            input: LiveInput::Refresh,
        })
        .await?;
    verify(
        "order picker renders component markup",
        initial.html.contains("id=\"floor-orders\"")
            && initial.html.contains("ORD-1")
            && !initial.html.contains("&lt;button"),
    )?;
    let body = event(
        &mut state,
        &manager,
        json!({"type":"pick_order","id":order.to_string()}),
    )
    .await?;
    verify(
        "order selection updates URL and run picker",
        body.query.get("order") == Some(&order.to_string()) && body.html.contains("floor-runs"),
    )?;
    let body = event(
        &mut state,
        &manager,
        json!({"type":"pick_run","id":run.to_string()}),
    )
    .await?;
    verify(
        "run selection has yield and large controls",
        body.query.get("run") == Some(&run.to_string())
            && body.html.contains("floor-yield")
            && body.html.contains("min-h-14")
            && body.html.contains("wastage-form"),
    )?;
    verify(
        "stored company timezone reaches timestamp components",
        body.html.contains("data-text-company=")
            && body.html.contains("data-mode=\"company\"")
            && body.html.contains("+08"),
    )?;
    let query = body.query;
    let mut restored = mount(&page, &manager, &query).await?;
    let body = restored
        .update(LiveRequest {
            scope: &manager,
            credential: "",
            input: LiveInput::Refresh,
        })
        .await?;
    verify(
        "bookmark restores order and run",
        body.query == query && body.html.contains("wastage-form"),
    )?;
    let invalid = event(
        &mut state,
        &manager,
        form("save_wastage", "wastage", &[("quantity", json!("1"))]),
    )
    .await?;
    verify(
        "missing material is an inline refusal",
        invalid
            .html
            .contains("Choose the material that was scrapped."),
    )?;
    let waste = form(
        "save_wastage",
        "wastage",
        &[
            ("request_id", json!("xp3-page-waste")),
            (
                "line",
                json!(format!(
                    "{}:{}",
                    output.item_id.unwrap(),
                    output.identity_id.unwrap()
                )),
            ),
            ("quantity", json!("1")),
            ("reason_id", json!(id(&reason).to_string())),
            ("observation", json!("counted")),
            ("note", json!("Page capture <proof>")),
        ],
    );
    let body = event(&mut state, &manager, waste.clone()).await?;
    verify(
        "wastage recorded and escaped",
        body.html.contains("Wastage recorded.")
            && body.html.contains("Page capture &#60;proof&#62;"),
    )?;
    event(&mut state, &manager, waste).await?;
    let records = pe.list_wastage(&manager, company, run).await?;
    verify(
        "double submit creates one record",
        records
            .iter()
            .filter(|r| r["request_id"] == "xp3-page-waste")
            .count()
            == 1,
    )?;
    let recorded = records
        .iter()
        .find(|r| r["request_id"] == "xp3-page-waste")
        .ok_or("record")?;
    event(
        &mut state,
        &manager,
        json!({"type":"start_correction","id":id(recorded).to_string()}),
    )
    .await?;
    let body = event(
        &mut state,
        &manager,
        form(
            "save_wastage_correction",
            "correction",
            &[
                ("quantity", json!("0.5")),
                ("correction_reason", json!("Recounted on page")),
            ],
        ),
    )
    .await?;
    verify(
        "wastage correction returns to table",
        body.html.contains("Wastage corrected.")
            && body.html.contains("Recounted on page")
            && !body.html.contains("id=\"wastage-correction-form\""),
    )?;
    for (ty, value) in [
        (&decimal, "4"),
        (&boolean, "true"),
        (&integer, "3"),
        (&text, "Observed output"),
    ] {
        event(
            &mut state,
            &manager,
            json!({"type":"pick_measurement_type","id":id(ty).to_string()}),
        )
        .await?;
        let body = event(
            &mut state,
            &manager,
            form(
                "save_measurement",
                "measurement",
                &[
                    ("value", json!(value)),
                    (
                        "identity_id",
                        json!(output.identity_id.unwrap().to_string()),
                    ),
                ],
            ),
        )
        .await?;
        verify(
            &format!("{} measurement captured", ty["value_type"]),
            body.html.contains("Measurement recorded."),
        )?;
    }
    let measurements = pe.list_measurements(&manager, company, run).await?;
    let measured = measurements
        .iter()
        .find(|v| v["measurement_type_id"] == decimal["id"])
        .ok_or("measurement")?;
    verify("range flag saved", measured["out_of_range"] == true)?;
    let body = event(
        &mut state,
        &manager,
        json!({"type":"start_measurement_correction","id":id(measured).to_string()}),
    )
    .await?;
    verify("range flag rendered", body.html.contains("Out of range"))?;
    let body = event(
        &mut state,
        &manager,
        form(
            "save_measurement_correction",
            "measurement_correction",
            &[
                ("value", json!("2")),
                ("correction_reason", json!("Remeasured on page")),
            ],
        ),
    )
    .await?;
    verify(
        "measurement corrected with in-range badge",
        body.html.contains("Measurement corrected.")
            && body.html.contains("In range")
            && body.html.contains("Remeasured on page"),
    )?;
    let body = event(
        &mut state,
        &manager,
        json!({"type":"wastage_grid","op":"zoom_preset","preset":"compact"}),
    )
    .await?;
    verify(
        "column host retains both tables",
        body.html.contains("wastage-table")
            && body.html.contains("measurements-table")
            && body.html.contains("data-mode=\"compact\""),
    )?;
    for key in [
        "factory.production-execution.floor.view",
        "factory.production-execution.configuration.view",
        "factory.inventory.configuration.view",
        "commerce.inventory.item.view",
    ] {
        grants
            .set_direct(&admin, viewer.user(), key, Some(true))
            .await?;
    }
    let mut view = mount(&page, &viewer, &query).await?;
    let body = event(
        &mut view,
        &viewer,
        form("save_measurement", "measurement", &[("value", json!("1"))]),
    )
    .await?;
    verify(
        "viewer forged write refused without dropping page",
        body.html.contains("permission to record measurements")
            && body.html.contains("measurements-table")
            && !body.html.contains("id=\"measurement-form\""),
    )?;
    let before = pe.list_measurements(&manager, company, run).await?.len();
    grants
        .set_direct(
            &admin,
            manager.user(),
            "factory.production-execution.measurement.record",
            Some(false),
        )
        .await?;
    let body = event(
        &mut state,
        &manager,
        form("save_measurement", "measurement", &[("value", json!("2"))]),
    )
    .await?;
    verify(
        "revoked mid-page grant refused",
        body.html
            .contains("You do not have permission to record measurements.")
            && pe.list_measurements(&manager, company, run).await?.len() == before,
    )?;
    grants
        .set_direct(
            &admin,
            manager.user(),
            "factory.production-execution.measurement.record",
            Some(true),
        )
        .await?;
    // Disposable-only fixture change simulates the company being archived by
    // another session between mount and submit; the facade must refuse it.
    let c = db.get().await?;
    c.execute(
        "UPDATE companies SET status='archived' WHERE tenant_id=$1 AND id=$2",
        &[&manager.tenant(), &company],
    )
    .await?;
    let archived = event(
        &mut state,
        &manager,
        form("save_measurement", "measurement", &[("value", json!("2"))]),
    )
    .await;
    c.execute(
        "UPDATE companies SET status='active' WHERE tenant_id=$1 AND id=$2",
        &[&manager.tenant(), &company],
    )
    .await?;
    let body = archived?;
    verify(
        "archived mid-page company refused",
        !body.html.contains("Measurement recorded.") && body.html.contains("floor-error"),
    )?;
    drop(c);
    let mut wrong = query.clone();
    wrong.insert("run".into(), i64::MAX.to_string());
    let mut wrong = mount(&page, &manager, &wrong).await?;
    let body = wrong
        .update(LiveRequest {
            scope: &manager,
            credential: "",
            input: LiveInput::Refresh,
        })
        .await?;
    verify(
        "unrelated run never selected",
        !body.query.contains_key("run") && !body.html.contains("wastage-form"),
    )?;
    println!("floor page walkthrough complete; order={order} run={run}");
    if std::env::var("BILIBI_FLOOR_SERVE").as_deref() == Ok("yes") {
        let bind = std::env::var("BIND_ADDR")?;
        let listener = tokio::net::TcpListener::bind(&bind).await?;
        println!("Floor browser proof listening at {origin}/factory/floor?order={order}&run={run}");
        axum::serve(listener, bilibi_web::router(app)).await?;
    }
    Ok(())
}
