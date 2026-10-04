//! Read-only dashboard API. Serves the persisted decision trail and backtest results
//! as JSON, and (in prod) the built SPA. It ONLY reads files — no broker, no orders,
//! no writes — so looking at the dashboard can never move money.
//!
//! Dev:  `cargo run --bin web` here + `npm run dev` in frontend/ (Vite proxies /api).
//! Prod: `npm run build` in frontend/, then this bin serves frontend/dist too.
//!
//! `GET /api/bars/{symbol}?timeframe=1Day&limit=500` serves the local bar store the
//! backfill writes. A crypto pair goes in the path URL-encoded: `/api/bars/BTC%2FUSD`.

use std::net::SocketAddr;

use axum::extract::{Path, Query};
use axum::{extract::State, routing::get, Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use alpaca_trading_system::domain::dto::{BacktestResult, EventRow};
use alpaca_trading_system::domain::types::{Bar, Symbol, Timeframe};
use alpaca_trading_system::infra::BarFileStore;

/// Where the two JSON-lines inputs live. v1 reads files the trading runs produce;
/// swapping to a Scylla read later means changing only these handlers.
#[derive(Clone)]
struct Paths {
    backtest: String,
    events: String,
    bars: String,
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
        backtest: std::env::var("BACKTEST_PATH").unwrap_or_else(|_| "backtest.jsonl".into()),
        events: std::env::var("EVENTS_PATH").unwrap_or_else(|_| "events.jsonl".into()),
        bars: std::env::var("BAR_STORE").unwrap_or_else(|_| "data/bars".into()),
    };

    let app = Router::new()
        .route("/api/backtest", get(get_backtest))
        .route("/api/events", get(get_events))
        .route("/api/bars/{symbol}", get(get_bars))
        // Prod: same origin serves the built SPA. Missing dist/ (dev, or before a
        // build) just 404s the fallback — the API routes still work.
        .fallback_service(ServeDir::new("frontend/dist"))
        // Dev: Vite serves the SPA on another port and calls this API cross-origin.
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

async fn get_backtest(State(p): State<Paths>) -> Json<Vec<BacktestResult>> {
    Json(read_jsonl(&p.backtest))
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
