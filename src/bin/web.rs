//! Read-only dashboard API. Serves the persisted decision trail and backtest results
//! as JSON, and (in prod) the built SPA. It ONLY reads files — no broker, no orders,
//! no writes — so looking at the dashboard can never move money.
//!
//! Dev:  `cargo run --bin web` here + `npm run dev` in frontend/ (Next.js proxies /api).
//! Prod: `npm run build` in frontend/ (static export), then this bin serves frontend/out too.
//!
//! `GET /api/bars/{symbol}?timeframe=1Day&limit=500` serves the local bar store the
//! backfill writes. A crypto pair goes in the path URL-encoded: `/api/bars/BTC%2FUSD`.
//!
//! `POST /api/backtest/run` starts a backtest from the page (#50): the rule arm runs
//! to completion before the response, an LLM arm runs in the background and the page
//! polls `GET /api/backtest/runs/{id}/status`, then fetches `/api/backtest/runs/{id}`.
//! `GET /api/backtest/runs` lists every run, in flight or on disk. The run handler can
//! reach bar **files** and a model; it cannot reach the broker, by construction: the
//! only `BarSource` it constructs is `FileBars` and no Alpaca client exists in this
//! binary. An LLM run above `LLM_CALL_LIMIT` estimated calls is refused unless the
//! request carries `confirm: true`.
//!
//! `GET /api/screen?date=2026-06-02&top=8&window=60` runs the screener and the regime
//! read **as of that morning** against the bar files — the replay behind the page's
//! date box. `GET /api/reports`, `/api/reports/latest`, `/api/reports/{date}` serve
//! the written reports. Everything here reads files; nothing fetches.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;

use axum::extract::{FromRef, Path, Query};
use axum::routing::post;
use axum::{extract::State, routing::get, Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use std::sync::Arc;

use alpaca_trading_system::domain::dto::{
    Arm, BacktestResult, EventRow, RunConfig, RunRequest, RunState, RunStatus, RunSummary,
};
use alpaca_trading_system::domain::report::{MorningReport, Regime, Screen};
use alpaca_trading_system::domain::types::{Bar, Symbol, Timeframe};
use alpaca_trading_system::domain::universe::Universe;
use alpaca_trading_system::engine::backtest::{self, BacktestConfig, RunStamp, Window};
use alpaca_trading_system::engine::regime;
use alpaca_trading_system::engine::screener::{self, ScreenerConfig};
use alpaca_trading_system::infra::{AnthropicClient, Claude, NoLlm, SONNET};
use alpaca_trading_system::infra::{BacktestStore, BarFileStore, FileBars, ReportStore};
use alpaca_trading_system::traits::LlmClient;
use axum::http::StatusCode;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Value};

/// Where the inputs live. v1 reads files the trading runs produce; swapping to a
/// Scylla read later means changing only these handlers.
#[derive(Clone)]
struct Paths {
    /// Directory of run files (`BacktestStore`).
    backtest: String,
    events: String,
    bars: String,
    reports: String,
    universe: String,
}

/// Everything the handlers share: file locations plus the in-flight run table.
#[derive(Clone)]
struct AppState {
    paths: Paths,
    runs: Runs,
}

impl FromRef<AppState> for Paths {
    fn from_ref(s: &AppState) -> Paths {
        s.paths.clone()
    }
}

impl FromRef<AppState> for Runs {
    fn from_ref(s: &AppState) -> Runs {
        s.runs.clone()
    }
}

/// Runs started through the API, by id. A finished run is also on disk; this table
/// is what answers `status` while it is still going and what remembers an error.
type Runs = Arc<Mutex<HashMap<String, RunEntry>>>;

struct RunEntry {
    status: RunStatus,
    summary: RunSummary,
    results: Option<Vec<BacktestResult>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `.env` for ANTHROPIC_API_KEY (LLM arms only); real env vars win.
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let paths = Paths {
        backtest: std::env::var("BACKTEST_STORE").unwrap_or_else(|_| "data/backtests".into()),
        events: std::env::var("EVENTS_PATH").unwrap_or_else(|_| "events.jsonl".into()),
        bars: std::env::var("BAR_STORE").unwrap_or_else(|_| "data/bars".into()),
        reports: std::env::var("REPORT_STORE").unwrap_or_else(|_| "data/reports".into()),
        universe: std::env::var("UNIVERSE").unwrap_or_else(|_| "config/universe.csv".into()),
    };

    let state = AppState {
        paths,
        runs: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/api/backtest", get(get_backtest))
        .route("/api/backtest/run", post(post_run))
        .route("/api/backtest/runs", get(get_runs))
        .route("/api/backtest/runs/{id}", get(get_run))
        .route("/api/backtest/runs/{id}/status", get(get_run_status))
        .route("/api/events", get(get_events))
        .route("/api/bars/{symbol}", get(get_bars))
        .route("/api/screen", get(get_screen))
        .route("/api/reports", get(get_report_dates))
        .route("/api/reports/latest", get(get_report_latest))
        .route("/api/reports/{date}", get(get_report))
        // Prod: same origin serves the built SPA. Missing out/ (dev, or before a
        // build) just 404s the fallback — the API routes still work.
        .fallback_service(ServeDir::new("frontend/out"))
        // Dev: Next.js serves the SPA on :3000 and proxies /api here; CORS stays
        // permissive so a direct cross-origin call still works.
        .layer(CorsLayer::permissive())
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "dashboard API listening (read-only)");
    println!(
        "dashboard API : http://{addr}  (GET /api/backtest, POST /api/backtest/run, /api/events, /api/bars/{{symbol}})"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// The newest run's results (every symbol), or an empty list before any run exists.
async fn get_backtest(State(p): State<Paths>) -> Json<Vec<BacktestResult>> {
    let latest = BacktestStore::new(&p.backtest)
        .latest()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read backtest store");
            None
        });
    Json(latest.map(|r| r.results).unwrap_or_default())
}

async fn get_events(State(p): State<Paths>) -> Json<Vec<EventRow>> {
    Json(read_jsonl(&p.events))
}

#[derive(Deserialize)]
struct BarsQuery {
    /// `1Hour` | `1Day` | `1Week`; default daily.
    timeframe: Option<String>,
    /// Most recent N bars; default 500, which is two years of daily.
    limit: Option<usize>,
}

/// The stored bars for one symbol, oldest first. Read-only: a symbol with no file is
/// an empty list, never a fetch — the dashboard must not become a second backfill.
async fn get_bars(
    State(p): State<Paths>,
    Path(symbol): Path<String>,
    Query(q): Query<BarsQuery>,
) -> Json<Vec<Bar>> {
    let timeframe = match q.timeframe.as_deref().unwrap_or("1Day") {
        "1Hour" => Timeframe::Hour1,
        "1Week" => Timeframe::Week1,
        _ => Timeframe::Day1,
    };
    let limit = q.limit.unwrap_or(500);
    let store = BarFileStore::new(&p.bars);
    let mut all = store
        .read_all(&Symbol::from(symbol.as_str()), timeframe)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, %symbol, "could not read bar file");
            Vec::new()
        });
    let skip = all.len().saturating_sub(limit);
    Json(all.split_off(skip))
}

/// Parse a JSON-lines file into `Vec<T>`. Missing file ⇒ empty vec (a fresh checkout
/// has no runs yet — not an error). A malformed line is skipped with a warning, never
/// fatal: a read-only viewer must not 500 because one row is bad.
fn read_jsonl<T: DeserializeOwned>(path: &str) -> Vec<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, path, "could not read jsonl");
            return Vec::new();
        }
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str::<T>(l) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(error = %e, "skipping malformed jsonl line");
                None
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct ScreenQuery {
    /// The morning to replay, YYYY-MM-DD; default today. Only bars before this date
    /// are visible, exactly as a live run that morning would have seen.
    date: Option<NaiveDate>,
    top: Option<usize>,
    window: Option<usize>,
}

#[derive(Serialize)]
struct ScreenReply {
    date: NaiveDate,
    regime: Option<Regime>,
    screen: Screen,
}

/// Replay the screener as of `date` against the bar files. No network: a name with no
/// file is simply skipped, and the response says so in `screen.skipped`.
async fn get_screen(
    State(p): State<Paths>,
    Query(q): Query<ScreenQuery>,
) -> Result<Json<ScreenReply>, (StatusCode, String)> {
    let universe = Universe::load(&p.universe)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("universe: {e}")))?;
    let date = q.date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let as_of = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp_millis();
    let bars = Arc::new(FileBars::new(Arc::new(BarFileStore::new(&p.bars))));
    let cfg = ScreenerConfig {
        top_n: q.top.unwrap_or(8),
        bar_window: q.window.unwrap_or(60),
        as_of: Some(as_of),
        ..Default::default()
    };
    let spy = Symbol::from(regime::DEFAULT_SYMBOL);
    let (regime, screen) = tokio::join!(
        regime::regime(&*bars, &spy, regime::DEFAULT_WINDOW, Some(as_of)),
        screener::screen(bars.clone(), &universe, &cfg),
    );
    Ok(Json(ScreenReply {
        date,
        regime: regime.unwrap_or_default(),
        screen,
    }))
}

async fn get_report_dates(State(p): State<Paths>) -> Json<Vec<NaiveDate>> {
    Json(ReportStore::new(&p.reports).dates().unwrap_or_default())
}

async fn get_report_latest(State(p): State<Paths>) -> Result<Json<MorningReport>, StatusCode> {
    ReportStore::new(&p.reports)
        .latest()
        .ok()
        .flatten()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn get_report(
    State(p): State<Paths>,
    Path(date): Path<NaiveDate>,
) -> Result<Json<MorningReport>, StatusCode> {
    ReportStore::new(&p.reports)
        .read(date)
        .ok()
        .flatten()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

// ---------------------------------------------------------------------------------
// Backtest run API (#50)

type ApiError = (StatusCode, Json<Value>);

fn bad(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (status, Json(json!({ "error": msg.into() })))
}

/// How many LLM calls a request would make, so the page can be told before paying.
/// A date range is estimated at five sessions a week.
fn estimate_calls(req: &RunRequest, window: Window) -> usize {
    let days = match window {
        Window::Days(d) => d,
        Window::Range { start, end } => ((end - start) / 86_400_000).max(0) as usize * 5 / 7,
    };
    req.symbols.len() * days * req.arm.calls_per_day(1)
}

fn parse_window(req: &RunRequest) -> Result<Window, ApiError> {
    match (req.days, &req.start, &req.end) {
        (Some(d), None, None) if (1..=5000).contains(&d) => Ok(Window::Days(d)),
        (Some(_), None, None) => Err(bad(StatusCode::BAD_REQUEST, "days must be 1..=5000")),
        (None, Some(s), Some(e)) => {
            let day = |t: &str| {
                t.parse::<NaiveDate>()
                    .map_err(|_| bad(StatusCode::BAD_REQUEST, format!("bad date '{t}'")))
            };
            let (s, e) = (day(s)?, day(e)?);
            if e < s {
                return Err(bad(StatusCode::BAD_REQUEST, "end is before start"));
            }
            let ms = |d: NaiveDate| {
                d.and_hms_opt(0, 0, 0)
                    .expect("midnight")
                    .and_utc()
                    .timestamp_millis()
            };
            // `end` is inclusive in the request, exclusive in the window.
            Ok(Window::Range {
                start: ms(s),
                end: ms(e) + 86_400_000,
            })
        }
        _ => Err(bad(
            StatusCode::BAD_REQUEST,
            "give either days or start and end",
        )),
    }
}

/// The model an LLM arm will call, or `None` for the rule arm. Built here, from the
/// web server's own environment: the page never sees a key.
enum Model {
    None,
    Claude(Arc<Claude>),
}

#[derive(Serialize)]
struct RunStarted {
    run_id: String,
}

/// Start a run. Validates, estimates the LLM cost and refuses to pay without
/// `confirm`, registers the run, then runs it: the rule arm is awaited (it takes
/// milliseconds, so the page's first status poll already sees `done`), an LLM arm is
/// left running and polled.
async fn post_run(
    State(s): State<AppState>,
    Json(req): Json<RunRequest>,
) -> Result<Json<RunStarted>, ApiError> {
    if req.symbols.is_empty() {
        return Err(bad(StatusCode::BAD_REQUEST, "at least one symbol"));
    }
    if req.starting_cash.is_nan() || req.starting_cash <= 0.0 {
        return Err(bad(
            StatusCode::BAD_REQUEST,
            "starting_cash must be positive",
        ));
    }
    let window = parse_window(&req)?;

    let (model, model_id, adapter) = match req.arm {
        Arm::Rule => (Model::None, String::new(), String::new()),
        Arm::Llm | Arm::LlmDebate => {
            let key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
            if key.is_empty() {
                return Err(bad(
                    StatusCode::BAD_REQUEST,
                    "an LLM arm needs ANTHROPIC_API_KEY on the web server; the rule arm needs no key",
                ));
            }
            let claude = Claude::new(Arc::new(AnthropicClient::new(key)), SONNET);
            (
                Model::Claude(Arc::new(claude)),
                SONNET.to_string(),
                "anthropic".to_string(),
            )
        }
    };

    let estimate = estimate_calls(&req, window);
    let limit: usize = std::env::var("LLM_CALL_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    if estimate > limit && !req.confirm {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("this run would make about {estimate} LLM calls (limit {limit}); resend with confirm: true"),
                "estimate": estimate,
                "limit": limit,
            })),
        ));
    }

    let cfg = BacktestConfig {
        arm: req.arm,
        model: model_id.clone(),
        adapter: adapter.clone(),
        starting_cash: req.starting_cash,
        // One clip is the whole account: the page compares against a fully invested
        // benchmark, so the strategy gets the same capital to deploy.
        max_notional: Decimal::from_f64_retain(req.starting_cash).unwrap_or_default(),
        days: 0, // set per symbol from the window
        debate_rounds: 1,
        fee_bps: req.fee_bps,
        slippage_bps: req.slippage_bps,
    };
    let stamp = RunStamp::new(req.symbols.clone(), req.context_symbols.clone());
    let run_id = stamp.run_id.clone();
    let summary = RunSummary {
        run_id: run_id.clone(),
        created_at: chrono::Utc::now().timestamp_millis(),
        config: RunConfig {
            arm: req.arm,
            model: model_id,
            adapter,
            fee_bps: req.fee_bps,
            slippage_bps: req.slippage_bps,
            starting_cash: req.starting_cash,
            window_start: req.start.clone().unwrap_or_default(),
            window_end: req.end.clone().unwrap_or_default(),
            symbols: req.symbols.clone(),
            context_symbols: req.context_symbols.clone(),
            git_sha: backtest::git_sha().to_string(),
            error_days: 0,
        },
    };
    s.runs.lock().unwrap().insert(
        run_id.clone(),
        RunEntry {
            status: RunStatus {
                run_id: run_id.clone(),
                state: RunState::Queued,
                progress: Some(0.0),
                message: "queued".into(),
            },
            summary,
            results: None,
        },
    );

    // Read-only by construction: the only bar source this task can touch is the
    // file store, and there is no broker in this binary.
    let bars = Arc::new(FileBars::new(Arc::new(BarFileStore::new(&s.paths.bars))));
    let store_dir = s.paths.backtest.clone();
    let runs = s.runs.clone();
    let id = run_id.clone();
    let handle = tokio::spawn(async move {
        set_status(&runs, &id, RunState::Running, Some(0.0), "running");
        let progress = {
            let (runs, id) = (runs.clone(), id.clone());
            move |f: f64| set_status(&runs, &id, RunState::Running, Some(f), "running")
        };
        let outcome = match &model {
            Model::None => {
                backtest::run_many(&*bars, &NoLlm, &cfg, &stamp, window, &progress).await
            }
            Model::Claude(c) => {
                let llm: &dyn LlmClient = &**c;
                backtest::run_many(&*bars, &llm, &cfg, &stamp, window, &progress).await
            }
        };
        match outcome {
            Ok(results) => {
                if let Err(e) = BacktestStore::new(&store_dir).write(&results) {
                    tracing::warn!(error = %e, run = %id, "could not persist run");
                }
                let mut map = runs.lock().unwrap();
                if let Some(entry) = map.get_mut(&id) {
                    entry.summary.config = results
                        .first()
                        .map(|r| r.config.clone())
                        .unwrap_or_else(|| entry.summary.config.clone());
                    entry.results = Some(results);
                    entry.status.state = RunState::Done;
                    entry.status.progress = Some(1.0);
                    entry.status.message = "done".into();
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, run = %id, "run failed");
                set_status(&runs, &id, RunState::Error, None, e.to_string());
            }
        }
    });
    if req.arm == Arm::Rule {
        let _ = handle.await;
    }
    Ok(Json(RunStarted { run_id }))
}

fn set_status(
    runs: &Runs,
    id: &str,
    state: RunState,
    progress: Option<f64>,
    msg: impl Into<String>,
) {
    if let Some(entry) = runs.lock().unwrap().get_mut(id) {
        entry.status.state = state;
        entry.status.progress = progress;
        entry.status.message = msg.into();
    }
}

/// Every run: in flight (from the table) and on disk (from the store), newest first.
async fn get_runs(State(s): State<AppState>) -> Json<Vec<RunSummary>> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<RunSummary> = s
        .runs
        .lock()
        .unwrap()
        .values()
        .map(|e| e.summary.clone())
        .collect();
    out.iter().for_each(|r| {
        seen.insert(r.run_id.clone());
    });
    for r in BacktestStore::new(&s.paths.backtest)
        .list()
        .unwrap_or_default()
    {
        if seen.insert(r.run_id.clone()) {
            out.push(r);
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    Json(out)
}

async fn get_run_status(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RunStatus>, ApiError> {
    if let Some(e) = s.runs.lock().unwrap().get(&id) {
        return Ok(Json(e.status.clone()));
    }
    match BacktestStore::new(&s.paths.backtest).read(&id) {
        Ok(Some(_)) => Ok(Json(RunStatus {
            run_id: id,
            state: RunState::Done,
            progress: Some(1.0),
            message: "done".into(),
        })),
        _ => Err(bad(StatusCode::NOT_FOUND, "no such run")),
    }
}

async fn get_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<BacktestResult>>, ApiError> {
    if let Some(e) = s.runs.lock().unwrap().get(&id) {
        return match (&e.results, e.status.state) {
            (Some(r), _) => Ok(Json(r.clone())),
            (None, RunState::Error) => Err(bad(
                StatusCode::INTERNAL_SERVER_ERROR,
                e.status.message.clone(),
            )),
            (None, _) => Err(bad(StatusCode::CONFLICT, "still running")),
        };
    }
    match BacktestStore::new(&s.paths.backtest).read(&id) {
        Ok(Some(r)) => Ok(Json(r.results)),
        _ => Err(bad(StatusCode::NOT_FOUND, "no such run")),
    }
}
