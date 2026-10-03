//! Finnhub REST: transport plus the `FundamentalSource` impl. One vendor, one
//! port, so the client implements the trait itself (no wrapper, no Arc).

use std::time::Duration;

use async_trait::async_trait;
use chrono::NaiveDate;
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::domain::error::FundamentalError;
use crate::domain::types::{EarningsEvent, EarningsHour, Symbol};
use crate::traits::FundamentalSource;

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

    /// One GET against Finnhub, decoded as `T`. Appends `token=` itself.
    ///
    /// 429 handling: Finnhub's free tier is 60 req/min. On the first 429 we wait
    /// for `Retry-After` seconds (default 1 if the header is missing or garbled)
    /// and retry exactly once; a second 429 surfaces as `RateLimited` so the
    /// caller can decide, instead of this client spinning at 6am.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T, FundamentalError> {
        let url = format!("{BASE}{path}");
        let mut retried = false;
        loop {
            let resp = self
                .http
                .get(&url)
                .query(params)
                .query(&[("token", self.key.as_str())])
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        FundamentalError::Timeout
                    } else {
                        FundamentalError::Request(e.to_string())
                    }
                })?;

            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                if retried {
                    return Err(FundamentalError::RateLimited);
                }
                let wait = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                retried = true;
                continue;
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(FundamentalError::Request(format!(
                    "{path} {status}: {body}"
                )));
            }
            return resp
                .json::<T>()
                .await
                .map_err(|e| FundamentalError::Decode(e.to_string()));
        }
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
        // self.get_json::<RawProfile>("/stock/profile2", &[("symbol", symbol.0.clone())])
        // marketCapitalization is in millions USD → multiply before Decimal.
        let _ = (
            symbol,
            RawProfile {
                market_capitalization: None,
            },
        );
        todo!("DATA-8: market cap")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
