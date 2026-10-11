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
//! `GET /api/screen?date=2026-06-02&top=8&window=60` runs the screener and the regime
//! read **as of that morning** against the bar files — the replay behind the page's
//! date box. `GET /api/reports`, `/api/reports/latest`, `/api/reports/{date}` serve
//! the written reports. Everything here reads files; nothing fetches.

use std::net::SocketAddr;

use axum::extract::{Path, Query};
use axum::{extract::State, routing::get, Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use std::sync::Arc;

use alpaca_trading_system::domain::dto::{BacktestResult, EventRow};
use alpaca_trading_system::domain::report::{MorningReport, Regime, Screen};
use alpaca_trading_system::domain::types::{Bar, Symbol, Timeframe};
use alpaca_trading_system::domain::universe::Universe;
use alpaca_trading_system::engine::regime;
use alpaca_trading_system::engine::screener::{self, ScreenerConfig};
use alpaca_trading_system::infra::{BacktestStore, BarFileStore, FileBars, ReportStore};
use axum::http::StatusCode;
use chrono::NaiveDate;
use serde::Serialize;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let app = Router::new()
        .route("/api/backtest", get(get_backtest))
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
        .with_state(paths);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "dashboard API listening (read-only)");
    println!(
        "dashboard API : http://{addr}  (GET /api/backtest, /api/events, /api/bars/{{symbol}})"
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
