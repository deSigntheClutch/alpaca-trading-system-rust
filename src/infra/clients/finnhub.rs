//! Finnhub REST: transport plus the `FundamentalSource` impl. One vendor, one
//! port, so the client implements the trait itself (no wrapper, no Arc).

use std::time::Duration;

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::domain::error::FundamentalError;
use crate::domain::types::{EarningsEvent, EarningsHour, Symbol};
use crate::traits::FundamentalSource;

use super::http::send_json;

const BASE: &str = "https://finnhub.io/api/v1";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCalendar {
    earnings_calendar: Vec<RawEarning>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEarning {
    symbol: String,
    date: NaiveDate,
    /// `"bmo"` | `"amc"` | `"dmh"` | `""`; absent in some rows, hence `default`.
    #[serde(default)]
    hour: String,
    eps_estimate: Option<f64>,
}

/// Wire → domain. Finnhub's session codes stay in this file; the domain only
/// knows before/after open. `"dmh"` (during hours) and `""` are both `None`.
impl From<RawEarning> for EarningsEvent {
    fn from(r: RawEarning) -> Self {
        EarningsEvent {
            symbol: Symbol(r.symbol),
            date: r.date,
            earnings_hour: match r.hour.as_str() {
                "bmo" => Some(EarningsHour::BeforeOpen),
                "amc" => Some(EarningsHour::AfterClose),
                _ => None,
            },
            eps_estimate: r.eps_estimate,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProfile {
    market_capitalization: Option<f64>,
}

impl RawProfile {
    /// Finnhub reports market cap in **millions** of USD; 0 means "no figure".
    /// Convert to Decimal first, then scale, so 1234.567 → 1_234_567_000 exactly.
    fn market_cap_usd(self) -> Option<Decimal> {
        self.market_capitalization
            .filter(|m| *m > 0.0)
            .and_then(Decimal::from_f64)
            .map(|m| m * Decimal::from(1_000_000))
    }
}

pub struct FinnhubClient {
    http: reqwest::Client,
    key: String,
}

impl FinnhubClient {
    /// Fails fast on an empty key so a bad `.env` surfaces at startup, not as a
    /// 401 at 6am.
    pub fn new(key: String) -> Result<Self, FundamentalError> {
        if key.trim().is_empty() {
            return Err(FundamentalError::MissingKey("FINNHUB_KEY"));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        Ok(Self { http, key })
    }

    /// One GET against Finnhub, decoded as `T`. Appends `token=` itself; status
    /// handling and the single 429 retry are the shared `send_json`.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T, FundamentalError> {
        let req = self
            .http
            .get(format!("{BASE}{path}"))
            .query(params)
            .query(&[("token", self.key.as_str())]);
        Ok(send_json(path, req).await?)
    }
}

#[async_trait]
impl FundamentalSource for FinnhubClient {
    async fn earnings_calendar(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<EarningsEvent>, FundamentalError> {
        let params = [("from", from.to_string()), ("to", to.to_string())];
        let parsed: RawCalendar = self.get_json("/calendar/earnings", &params).await?;
        Ok(parsed
            .earnings_calendar
            .into_iter()
            .map(EarningsEvent::from)
            .collect())
    }

    async fn market_cap(&self, symbol: &Symbol) -> Result<Option<Decimal>, FundamentalError> {
        let params = [("symbol", symbol.0.clone())];
        let parsed: RawProfile = self.get_json("/stock/profile2", &params).await?;
        Ok(parsed.market_cap_usd())
    }
}

/// In-memory `FundamentalSource` for tests of whatever consumes the port
/// (screener, commentary agent, report). Holds a fixed calendar and cap table;
/// `earnings_calendar` filters by date range the way the vendor would.
#[cfg(test)]
pub struct MemFundamentals {
    pub events: Vec<EarningsEvent>,
    pub caps: std::collections::HashMap<Symbol, Decimal>,
}

#[cfg(test)]
#[async_trait]
impl FundamentalSource for MemFundamentals {
    async fn earnings_calendar(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<EarningsEvent>, FundamentalError> {
        Ok(self
            .events
            .iter()
            .filter(|e| e.date >= from && e.date <= to)
            .cloned()
            .collect())
    }

    async fn market_cap(&self, symbol: &Symbol) -> Result<Option<Decimal>, FundamentalError> {
        Ok(self.caps.get(symbol).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    /// Shape of a real `/calendar/earnings` response, trimmed to the fields we
    /// read. Row 4 has no `hour` key at all, which exercises `#[serde(default)]`.
    const CALENDAR: &str = r#"{"earningsCalendar":[
        {"symbol":"AAPL","date":"2026-10-30","hour":"amc","epsEstimate":1.52,"quarter":4,"year":2026},
        {"symbol":"MSFT","date":"2026-10-28","hour":"bmo","epsEstimate":null},
        {"symbol":"XYZ","date":"2026-10-29","hour":"dmh","epsEstimate":0.1},
        {"symbol":"NOHOUR","date":"2026-10-29"}
    ]}"#;

    fn events() -> Vec<EarningsEvent> {
        let raw: RawCalendar = serde_json::from_str(CALENDAR).expect("fixture parses");
        raw.earnings_calendar
            .into_iter()
            .map(EarningsEvent::from)
            .collect()
    }

    #[test]
    fn calendar_fixture_yields_every_row() {
        let ev = events();
        assert_eq!(ev.len(), 4);
        assert_eq!(ev[0].symbol, Symbol::from("AAPL"));
        assert_eq!(ev[0].date, NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
        assert_eq!(ev[0].eps_estimate, Some(1.52));
    }

    #[test]
    fn hour_codes_map_to_domain() {
        let ev = events();
        assert_eq!(ev[0].earnings_hour, Some(EarningsHour::AfterClose), "amc");
        assert_eq!(ev[1].earnings_hour, Some(EarningsHour::BeforeOpen), "bmo");
        assert_eq!(ev[2].earnings_hour, None, "dmh is neither");
        assert_eq!(ev[3].earnings_hour, None, "missing hour key");
    }

    #[test]
    fn null_eps_estimate_is_none() {
        assert_eq!(events()[1].eps_estimate, None);
    }

    #[test]
    fn empty_key_is_rejected_at_construction() {
        assert!(matches!(
            FinnhubClient::new("  ".into()),
            Err(FundamentalError::MissingKey("FINNHUB_KEY"))
        ));
        assert!(FinnhubClient::new("abc".into()).is_ok());
    }

    fn profile(json: &str) -> RawProfile {
        serde_json::from_str(json).expect("profile parses")
    }

    #[test]
    fn market_cap_is_millions_to_dollars() {
        assert_eq!(
            profile(r#"{"marketCapitalization": 1234.5}"#).market_cap_usd(),
            Some(dec!(1_234_500_000))
        );
    }

    #[test]
    fn market_cap_scales_in_decimal_not_f64() {
        // 1234.567 * 1e6 in f64 is 1234566999.9999998; Decimal math keeps it exact.
        assert_eq!(
            profile(r#"{"marketCapitalization": 1234.567}"#).market_cap_usd(),
            Some(dec!(1_234_567_000))
        );
    }

    #[test]
    fn missing_or_zero_market_cap_is_none() {
        assert_eq!(
            profile(r#"{"name": "X", "ticker": "X"}"#).market_cap_usd(),
            None
        );
        assert_eq!(
            profile(r#"{"marketCapitalization": 0}"#).market_cap_usd(),
            None
        );
        assert_eq!(
            profile(r#"{"marketCapitalization": null}"#).market_cap_usd(),
            None
        );
    }

    #[tokio::test]
    async fn mem_fake_filters_by_range_and_looks_up_caps() {
        let d = |day| NaiveDate::from_ymd_opt(2026, 10, day).unwrap();
        let ev = |sym: &str, day| EarningsEvent {
            symbol: Symbol::from(sym),
            date: d(day),
            earnings_hour: None,
            eps_estimate: None,
        };
        let fake = MemFundamentals {
            events: vec![ev("A", 1), ev("B", 5), ev("C", 9)],
            caps: [(Symbol::from("A"), dec!(1_000_000_000))]
                .into_iter()
                .collect(),
        };
        let got = fake.earnings_calendar(d(2), d(8)).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].symbol, Symbol::from("B"));
        assert_eq!(
            fake.market_cap(&Symbol::from("A")).await.unwrap(),
            Some(dec!(1_000_000_000))
        );
        assert_eq!(fake.market_cap(&Symbol::from("Z")).await.unwrap(), None);
    }
}
