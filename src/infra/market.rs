//! `BarSource` / `MarketStream` adapters.
//!   - `AlpacaBars` — historical bars (crypto and stocks) over a shared `AlpacaClient`.
//!   - `LocalBars`  — a `BarSource` over the JSONL store with fall-through to an
//!     upstream `BarSource` and write-through, so backtest and live read the same
//!     files and Alpaca is asked only for what the files lack.
//!   - `CryptoWs`   — the 24/7 crypto trade websocket. In-band auth (a message, not a
//!     header): connect, send `auth`, wait for `authenticated`, send `subscribe`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use futures::SinkExt;
use serde::Deserialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::domain::error::MarketError;
use crate::domain::types::{AssetClass, Bar, Symbol, Tick, Timeframe};
use crate::traits::{BarSource, MarketStream};

use super::bar_store::BarFileStore;
use super::clients::alpaca::{BarsResponse, Host, RawBar};
use super::clients::{parse_rfc3339_millis, AlpacaClient};

// ---- AlpacaBars -------------------------------------------------------------------

pub struct AlpacaBars {
    client: Arc<AlpacaClient>,
}

impl AlpacaBars {
    pub fn new(client: Arc<AlpacaClient>) -> Self {
        Self { client }
    }
}

/// Calendar days to look back so that `limit` bars of `timeframe` fit in the window.
/// Crypto trades every hour of every day; stocks five days a week, and hourly stock
/// bars are counted at the 7 regular-session bars a day, so extended hours only add
/// margin. Always generous: a short window silently returns fewer bars than asked.
fn lookback_days(class: AssetClass, timeframe: Timeframe, limit: usize) -> usize {
    let bars_per_day = match (class, timeframe) {
        (AssetClass::Crypto, Timeframe::Hour1) => 24,
        (AssetClass::Stock, Timeframe::Hour1) => 7,
        (_, Timeframe::Day1) => 1,
        (_, Timeframe::Week1) => return limit * 7 + 14,
    };
    let days = limit.div_ceil(bars_per_day);
    match class {
        AssetClass::Crypto => days + 5,
        AssetClass::Stock => days * 7 / 5 + 10,
    }
}

/// Alpaca caps one bars page at this many rows.
const MAX_PAGE: usize = 10_000;

/// Endpoint and the class-specific query parameters shared by both fetch shapes.
/// `adjustment=all` folds splits and dividends into history so a 4:1 split is not a
/// -75% bar; the crypto endpoint has no such parameter.
fn endpoint(class: AssetClass) -> (&'static str, &'static [(&'static str, &'static str)]) {
    match class {
        AssetClass::Crypto => ("/v1beta3/crypto/us/bars", &[]),
        AssetClass::Stock => ("/v2/stocks/bars", &[("adjustment", "all")]),
    }
}

/// Wire → domain for one symbol's rows; a row whose timestamp does not parse is
/// dropped rather than given a made-up time.
fn into_bars(symbol: &Symbol, raws: Vec<RawBar>) -> Vec<Bar> {
    raws.into_iter()
        .filter_map(|r| {
            Some(Bar {
                symbol: symbol.clone(),
                ts: parse_rfc3339_millis(&r.t)?,
                open: r.o,
                high: r.h,
                low: r.l,
                close: r.c,
                volume: r.v,
            })
        })
        .collect()
}

fn rfc3339(ms: i64) -> Result<String, MarketError> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .ok_or_else(|| MarketError::Request(format!("timestamp out of range: {ms}")))
}

#[async_trait]
impl BarSource for AlpacaBars {
    /// Most-recent `limit` bars of `timeframe`, returned oldest→newest (what
    /// `signals::compute` wants). One symbol per request: Alpaca fills a multi-symbol
    /// `limit` symbol by symbol, so batching buys nothing here. Callers wanting
    /// several symbols run these concurrently.
    async fn bars(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Bar>, MarketError> {
        let class = symbol.asset_class();
        let (path, extra) = endpoint(class);

        // Without an explicit `start`, Alpaca returns only the current day's bars. Look
        // back far enough in calendar days that `limit` bars actually come back;
        // over-fetching is harmless because `sort=desc` + `limit` keeps the recent end.
        let start = (chrono::Utc::now()
            - chrono::Duration::days(lookback_days(class, timeframe, limit) as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let limit_s = limit.to_string();

        let mut params: Vec<(&str, &str)> = vec![
            ("symbols", symbol.as_str()),
            ("timeframe", timeframe.as_str()),
            ("limit", &limit_s),
            ("start", &start),
            ("sort", "desc"), // newest first, so `limit` gives the RECENT window
        ];
        params.extend_from_slice(extra);

        let mut parsed: BarsResponse = self.client.get_json(Host::Data, path, &params).await?;
        let mut bars = into_bars(
            symbol,
            parsed.bars.remove(symbol.as_str()).unwrap_or_default(),
        );
        bars.reverse(); // desc -> asc (chronological)
        Ok(bars)
    }

    /// Everything in `[start, end)`, oldest first, following `next_page_token` until
    /// the vendor runs out. Pages are 10,000 rows, so ten years of daily bars is one
    /// request and five years of hourly is two. The range is also enforced locally:
    /// Alpaca's `end` is inclusive and we want half-open.
    async fn bars_range(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        start: i64,
        end: i64,
    ) -> Result<Vec<Bar>, MarketError> {
        if start >= end {
            return Ok(Vec::new());
        }
        let class = symbol.asset_class();
        let (path, extra) = endpoint(class);
        let (start_s, end_s) = (rfc3339(start)?, rfc3339(end)?);
        let page = MAX_PAGE.to_string();

        let mut base: Vec<(&str, &str)> = vec![
            ("symbols", symbol.as_str()),
            ("timeframe", timeframe.as_str()),
            ("start", &start_s),
            ("end", &end_s),
            ("limit", &page),
            ("sort", "asc"),
        ];
        base.extend_from_slice(extra);

        let mut out: Vec<Bar> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut parsed: BarsResponse = {
                let mut params = base.clone();
                if let Some(tok) = &page_token {
                    params.push(("page_token", tok.as_str()));
                }
                self.client.get_json(Host::Data, path, &params).await?
            };
            let rows = parsed.bars.remove(symbol.as_str()).unwrap_or_default();
            let got = rows.len();
            out.extend(into_bars(symbol, rows));
            match parsed.next_page_token {
                Some(tok) if !tok.is_empty() && got > 0 => page_token = Some(tok),
                _ => break,
            }
        }

        out.retain(|b| b.ts >= start && b.ts < end);
        out.sort_by_key(|b| b.ts);
        out.dedup_by_key(|b| b.ts);
        Ok(out)
    }
}

#[cfg(test)]
mod lookback_tests {
    use super::*;

    #[test]
    fn stocks_look_back_further_than_crypto_for_the_same_bar_count() {
        let c = lookback_days(AssetClass::Crypto, Timeframe::Day1, 60);
        let s = lookback_days(AssetClass::Stock, Timeframe::Day1, 60);
        assert_eq!(c, 65);
        assert!(s > c, "stock={s} crypto={c}");
        assert!(
            s >= 60 * 7 / 5,
            "60 trading days need at least 84 calendar days"
        );
    }

    #[test]
    fn hourly_and_weekly_scale_with_the_timeframe() {
        assert_eq!(lookback_days(AssetClass::Crypto, Timeframe::Hour1, 48), 7); // 2 days + 5
        assert!(lookback_days(AssetClass::Stock, Timeframe::Hour1, 70) >= 14); // 10 sessions
        assert_eq!(lookback_days(AssetClass::Stock, Timeframe::Week1, 10), 84);
    }
}

// ---- LocalBars --------------------------------------------------------------------

const MS_PER_DAY: i64 = 86_400_000;

/// Clock injection so the freshness rules are testable at a fixed "now".
type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// A `BarSource` that answers from `data/bars/` and goes upstream only for what the
/// files lack: a cold symbol, bars newer than the last one stored, history older than
/// the first one stored, or a periodic full refresh (a split rewrites every adjusted
/// price, and nothing in the bars says so). Everything fetched is written through, so
/// the second caller of the day pays nothing. Serves **completed** bars only: the
/// upstream range is always capped at midnight UTC today.
pub struct LocalBars<U> {
    store: Arc<BarFileStore>,
    upstream: U,
    /// Rebuild a symbol's file from scratch once its sidecar is older than this.
    full_refresh_after: Duration,
    now: Clock,
}

impl<U: BarSource> LocalBars<U> {
    /// Default full refresh every 7 days: a split is rare and a week of stale
    /// adjustment is tolerable for a daily screener; `backfill --full` is the manual
    /// override when it is not.
    pub fn new(store: Arc<BarFileStore>, upstream: U) -> Self {
        Self {
            store,
            upstream,
            full_refresh_after: Duration::from_secs(7 * 86_400),
            now: Arc::new(|| chrono::Utc::now().timestamp_millis()),
        }
    }

    pub fn with_full_refresh_after(mut self, d: Duration) -> Self {
        self.full_refresh_after = d;
        self
    }

    #[cfg(test)]
    fn with_clock(mut self, now: Clock) -> Self {
        self.now = now;
        self
    }

    fn now_ms(&self) -> i64 {
        (self.now)()
    }

    fn today_midnight(&self) -> i64 {
        self.now_ms() - self.now_ms().rem_euclid(MS_PER_DAY)
    }

    /// Is the newest stored bar old enough that newer completed bars may exist?
    /// Crypto prints a bar every day, so yesterday's must be there. Stocks skip
    /// weekends and holidays: allow three calendar days before asking again, which
    /// costs one wasted request after a long weekend and none on a normal one.
    /// Weekly bars are stamped on their Monday and complete at Friday's close.
    fn stale(&self, symbol: &Symbol, timeframe: Timeframe, last_ts: i64) -> bool {
        let slack_days = match (symbol.asset_class(), timeframe) {
            (_, Timeframe::Week1) => 8,
            (AssetClass::Crypto, _) => 1,
            (AssetClass::Stock, _) => 3,
        };
        last_ts < self.today_midnight() - slack_days * MS_PER_DAY
    }

    /// Does the file start late enough that a request for `need_start` is not served?
    /// `need_start` can land on a weekend while the first bar is a Monday, so allow a
    /// period's worth of slack before calling the history too short.
    fn too_short(first_ts: i64, need_start: i64, timeframe: Timeframe) -> bool {
        let slack = match timeframe {
            Timeframe::Week1 => 14 * MS_PER_DAY,
            _ => 7 * MS_PER_DAY,
        };
        first_ts > need_start + slack
    }

    /// Make the file cover `[need_start, today)` and be fresh, with at most one
    /// upstream request, then return every stored bar.
    async fn ensure(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        need_start: i64,
    ) -> Result<Vec<Bar>, MarketError> {
        let end = self.today_midnight();
        let io = |e: std::io::Error| MarketError::Request(format!("bar store {symbol}: {e}"));

        let stored = self.store.read_all(symbol, timeframe).map_err(io)?;
        let span = stored.first().zip(stored.last()).map(|(a, b)| (a.ts, b.ts));
        let refresh_due = match self.store.meta(symbol, timeframe) {
            Some(m) => self.now_ms() - m.fetched_at > self.full_refresh_after.as_millis() as i64,
            None => !stored.is_empty(), // bars without a sidecar: age unknown, rebuild
        };

        let fetch_from = match span {
            None => Some(need_start),
            Some((first, _)) if refresh_due || Self::too_short(first, need_start, timeframe) => {
                tracing::info!(%symbol, ?timeframe, refresh_due, "rebuilding bar file");
                self.store.reset(symbol, timeframe).map_err(io)?;
                Some(need_start.min(first))
            }
            Some((_, last)) if self.stale(symbol, timeframe, last) => Some(last + 1),
            Some(_) => None,
        };

        if let Some(from) = fetch_from {
            if from < end {
                let fetched = self
                    .upstream
                    .bars_range(symbol, timeframe, from, end)
                    .await?;
                self.store.append(symbol, timeframe, &fetched).map_err(io)?;
            }
            return self.store.read_all(symbol, timeframe).map_err(io);
        }
        Ok(stored)
    }
}

#[async_trait]
impl<U: BarSource> BarSource for LocalBars<U> {
    /// The recent window from the file, after making sure the file reaches back far
    /// enough and forward to the last completed bar.
    async fn bars(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Bar>, MarketError> {
        let days = lookback_days(symbol.asset_class(), timeframe, limit) as i64;
        let need_start = self.today_midnight() - days * MS_PER_DAY;
        let mut all = self.ensure(symbol, timeframe, need_start).await?;
        let skip = all.len().saturating_sub(limit);
        Ok(all.split_off(skip))
    }

    /// `[start, end)` from the file; `end` is capped at midnight today because the
    /// store never holds a bar that is still forming.
    async fn bars_range(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        start: i64,
        end: i64,
    ) -> Result<Vec<Bar>, MarketError> {
        let end = end.min(self.today_midnight());
        if start >= end {
            return Ok(Vec::new());
        }
        let mut all = self.ensure(symbol, timeframe, start).await?;
        all.retain(|b| b.ts >= start && b.ts < end);
        Ok(all)
    }
}

#[cfg(test)]
mod local_bars_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    const DAY: i64 = MS_PER_DAY;
    /// A fixed "now": 2026-10-07 (Wednesday) 10:00 UTC.
    const NOW: i64 = 1_791_453_600_000 + 10 * 3_600_000;
    const TODAY: i64 = NOW - NOW % DAY;

    /// Upstream fake: one bar per calendar day in the asked range, every call logged.
    #[derive(Default)]
    struct Upstream {
        calls: AtomicUsize,
        ranges: StdMutex<Vec<(i64, i64)>>,
    }

    fn bar(symbol: &Symbol, ts: i64) -> Bar {
        Bar {
            symbol: symbol.clone(),
            ts,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
        }
    }

    #[async_trait]
    impl BarSource for Upstream {
        async fn bars(&self, _: &Symbol, _: Timeframe, _: usize) -> Result<Vec<Bar>, MarketError> {
            unreachable!("LocalBars must only use bars_range upstream")
        }
        async fn bars_range(
            &self,
            symbol: &Symbol,
            _: Timeframe,
            start: i64,
            end: i64,
        ) -> Result<Vec<Bar>, MarketError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.ranges.lock().unwrap().push((start, end));
            let first = start - start.rem_euclid(DAY) + if start % DAY == 0 { 0 } else { DAY };
            Ok((0..)
                .map(|i| first + i * DAY)
                .take_while(|t| *t < end)
                .map(|t| bar(symbol, t))
                .collect())
        }
    }

    fn fixture() -> (Arc<BarFileStore>, Arc<Upstream>) {
        let dir = std::env::temp_dir().join(format!(
            "local_bars_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (
            Arc::new(BarFileStore::new(dir)),
            Arc::new(Upstream::default()),
        )
    }

    fn local(store: &Arc<BarFileStore>, up: &Arc<Upstream>) -> LocalBars<Arc<Upstream>> {
        LocalBars::new(store.clone(), up.clone()).with_clock(Arc::new(|| NOW))
    }

    #[tokio::test]
    async fn cold_symbol_fetches_once_then_serves_from_disk() {
        let (store, up) = fixture();
        let lb = local(&store, &up);
        let nvda = Symbol::from("NVDA");

        let got = lb.bars(&nvda, Timeframe::Day1, 20).await.unwrap();
        assert_eq!(got.len(), 20);
        assert_eq!(
            got.last().unwrap().ts,
            TODAY - DAY,
            "last completed bar is yesterday"
        );
        assert_eq!(up.calls.load(Ordering::SeqCst), 1);
        let (s, e) = up.ranges.lock().unwrap()[0];
        assert_eq!(e, TODAY, "upstream range is capped at midnight today");
        assert!(
            s <= TODAY - 20 * DAY,
            "asked far enough back for 20 trading days"
        );

        let again = lb.bars(&nvda, Timeframe::Day1, 20).await.unwrap();
        assert_eq!(again.len(), 20);
        assert_eq!(
            up.calls.load(Ordering::SeqCst),
            1,
            "warm read made no request"
        );
        let _ = std::fs::remove_dir_all(store.root());
    }

    #[tokio::test]
    async fn stale_file_fetches_only_the_gap() {
        let (store, up) = fixture();
        let nvda = Symbol::from("NVDA");
        // A file that ends ten days ago.
        let old: Vec<Bar> = (10..=30)
            .rev()
            .map(|d| bar(&nvda, TODAY - d * DAY))
            .collect();
        store.append(&nvda, Timeframe::Day1, &old).unwrap();

        let lb = local(&store, &up);
        let got = lb.bars(&nvda, Timeframe::Day1, 5).await.unwrap();
        assert_eq!(up.calls.load(Ordering::SeqCst), 1);
        let (s, e) = up.ranges.lock().unwrap()[0];
        assert_eq!(
            s,
            TODAY - 10 * DAY + 1,
            "from just after the last stored bar"
        );
        assert_eq!(e, TODAY);
        assert_eq!(got.last().unwrap().ts, TODAY - DAY);
        let _ = std::fs::remove_dir_all(store.root());
    }

    #[tokio::test]
    async fn fresh_stock_file_over_a_weekend_makes_no_request() {
        let (store, up) = fixture();
        let nvda = Symbol::from("NVDA");
        // Last bar two days ago: a stock skipping a weekend is not stale.
        let recent: Vec<Bar> = (2..=40)
            .rev()
            .map(|d| bar(&nvda, TODAY - d * DAY))
            .collect();
        store.append(&nvda, Timeframe::Day1, &recent).unwrap();
        let lb = local(&store, &up);
        let got = lb.bars(&nvda, Timeframe::Day1, 10).await.unwrap();
        assert_eq!(got.len(), 10);
        assert_eq!(up.calls.load(Ordering::SeqCst), 0);

        // The same file for a crypto pair IS stale: it prints every day.
        let btc = Symbol::from("BTC/USD");
        let recent: Vec<Bar> = (2..=40).rev().map(|d| bar(&btc, TODAY - d * DAY)).collect();
        store.append(&btc, Timeframe::Day1, &recent).unwrap();
        lb.bars(&btc, Timeframe::Day1, 10).await.unwrap();
        assert_eq!(up.calls.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(store.root());
    }

    #[tokio::test]
    async fn short_history_is_rebuilt_from_the_needed_start() {
        let (store, up) = fixture();
        let nvda = Symbol::from("NVDA");
        let short: Vec<Bar> = (1..=5).rev().map(|d| bar(&nvda, TODAY - d * DAY)).collect();
        store.append(&nvda, Timeframe::Day1, &short).unwrap();
        let lb = local(&store, &up);
        let got = lb.bars(&nvda, Timeframe::Day1, 60).await.unwrap();
        assert_eq!(up.calls.load(Ordering::SeqCst), 1);
        let (s, _) = up.ranges.lock().unwrap()[0];
        assert!(
            s < TODAY - 60 * DAY,
            "rebuilt from far enough back, not appended"
        );
        assert!(got.len() >= 60);
        let _ = std::fs::remove_dir_all(store.root());
    }

    #[tokio::test]
    async fn bars_range_caps_end_at_today_and_serves_the_slice() {
        let (store, up) = fixture();
        let lb = local(&store, &up);
        let nvda = Symbol::from("NVDA");
        let (start, end) = (TODAY - 30 * DAY, TODAY + 5 * DAY);
        let got = lb
            .bars_range(&nvda, Timeframe::Day1, start, end)
            .await
            .unwrap();
        assert!(got.iter().all(|b| b.ts >= start && b.ts < TODAY));
        assert_eq!(got.len(), 30);
        let (_, e) = up.ranges.lock().unwrap()[0];
        assert_eq!(e, TODAY);
        let _ = std::fs::remove_dir_all(store.root());
    }
}

// ---- CryptoWs ---------------------------------------------------------------------

/// Alpaca crypto market-data websocket. Free tier, one connection, 24/7. In-band
/// auth: connect, send `auth`, wait for `authenticated`, send `subscribe`.
pub struct CryptoWs {
    key: String,
    secret: String,
    ws_url: String,
}

impl CryptoWs {
    pub fn new(key: String, secret: String, ws_url: String) -> Self {
        Self {
            key,
            secret,
            ws_url,
        }
    }
}

/// One element of a WS frame (frames are JSON arrays of these). Fields are optional
/// because a frame carries several message types (`success`, `error`, `t`rade, ...).
#[derive(Deserialize)]
struct WsMsg {
    #[serde(rename = "T")]
    typ: String,
    #[serde(rename = "S")]
    symbol: Option<String>,
    #[serde(rename = "p")]
    price: Option<f64>,
    #[serde(rename = "s")]
    size: Option<f64>,
    #[serde(rename = "t")]
    ts: Option<String>,
    msg: Option<String>,
}

#[async_trait]
impl MarketStream for CryptoWs {
    async fn subscribe(
        &self,
        symbols: &[Symbol],
    ) -> Result<BoxStream<'static, Result<Tick, MarketError>>, MarketError> {
        let (mut ws, _resp) = connect_async(self.ws_url.as_str())
            .await
            .map_err(|e| MarketError::Connect(e.to_string()))?;

        // 1. auth
        let auth = serde_json::json!({"action":"auth","key":self.key,"secret":self.secret});
        ws.send(Message::text(auth.to_string()))
            .await
            .map_err(|e| MarketError::Subscribe(e.to_string()))?;

        // 2. wait for `authenticated` (skip the initial `connected`; bail on `error`)
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => match classify(t.as_str()) {
                    Handshake::Authenticated => break,
                    Handshake::Error(m) => return Err(MarketError::Subscribe(m)),
                    Handshake::Other => continue,
                },
                Some(Ok(_)) => continue, // ping/binary during handshake
                Some(Err(e)) => return Err(MarketError::Connect(e.to_string())),
                None => return Err(MarketError::Connect("closed during auth".into())),
            }
        }

        // 3. subscribe to trades
        let syms: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
        let sub = serde_json::json!({"action":"subscribe","trades":syms});
        ws.send(Message::text(sub.to_string()))
            .await
            .map_err(|e| MarketError::Subscribe(e.to_string()))?;

        // 4. stream ticks. A single frame can hold many ticks, so buffer them and
        // drain one per poll. Stream *ending* (Close/None/Err) is normal — the caller
        // (Ingester) reconnects; we just terminate here.
        let init: (_, VecDeque<Tick>) = (ws, VecDeque::new());
        let stream = futures::stream::unfold(init, |(mut ws, mut buf)| async move {
            loop {
                if let Some(tick) = buf.pop_front() {
                    return Some((Ok(tick), (ws, buf)));
                }
                match ws.next().await {
                    Some(Ok(Message::Text(t))) => {
                        for tick in parse_ticks(t.as_str()) {
                            buf.push_back(tick);
                        }
                        // if the frame carried no trades (subscription ack, etc.), loop
                    }
                    Some(Ok(Message::Close(_))) | None => return None,
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "crypto ws error; ending stream for reconnect");
                        return None;
                    }
                }
            }
        });
        Ok(stream.boxed())
    }
}

enum Handshake {
    Authenticated,
    Error(String),
    Other,
}

fn classify(frame: &str) -> Handshake {
    let msgs: Vec<WsMsg> = match serde_json::from_str(frame) {
        Ok(m) => m,
        Err(_) => return Handshake::Other,
    };
    for m in &msgs {
        if m.typ == "error" {
            return Handshake::Error(m.msg.clone().unwrap_or_else(|| "unknown ws error".into()));
        }
        if m.typ == "success" && m.msg.as_deref() == Some("authenticated") {
            return Handshake::Authenticated;
        }
    }
    Handshake::Other
}

/// Extract trade ticks (`T == "t"`) from one frame; non-trade messages are dropped.
fn parse_ticks(frame: &str) -> Vec<Tick> {
    let msgs: Vec<WsMsg> = match serde_json::from_str(frame) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "undecodable ws frame");
            return Vec::new();
        }
    };
    msgs.into_iter()
        .filter(|m| m.typ == "t")
        .filter_map(|m| {
            Some(Tick {
                symbol: Symbol::from(m.symbol?),
                ts: parse_rfc3339_millis(m.ts.as_deref()?)?,
                price: m.price?,
                size: m.size.unwrap_or(0.0),
            })
        })
        .collect()
}

#[cfg(test)]
mod ws_tests {
    use super::*;

    #[test]
    fn parses_trade_frame_ignoring_status_and_bad_ts() {
        let frame = r#"[
            {"T":"subscription","trades":["BTC/USD"]},
            {"T":"t","S":"BTC/USD","p":42000.5,"s":0.01,"t":"2024-01-02T03:04:05.678Z"},
            {"T":"t","S":"ETH/USD","p":2500.0,"s":1.0,"t":"not-a-date"}
        ]"#;
        let ticks = parse_ticks(frame);
        assert_eq!(ticks.len(), 1, "only the well-formed trade survives");
        assert_eq!(ticks[0].symbol.as_str(), "BTC/USD");
        assert_eq!(ticks[0].price, 42000.5);
        assert_eq!(ticks[0].ts, 1_704_164_645_678);
    }

    #[test]
    fn classify_handshake_frames() {
        assert!(matches!(
            classify(r#"[{"T":"success","msg":"authenticated"}]"#),
            Handshake::Authenticated
        ));
        assert!(matches!(
            classify(r#"[{"T":"success","msg":"connected"}]"#),
            Handshake::Other
        ));
        assert!(matches!(
            classify(r#"[{"T":"error","code":402,"msg":"auth failed"}]"#),
            Handshake::Error(_)
        ));
    }
}
