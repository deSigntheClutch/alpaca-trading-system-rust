//! The one place a vendor request is sent and decoded. A client's only vendor-specific
//! knowledge is how to build an authenticated `RequestBuilder`; `send_json` does the
//! rest for every vendor the same way: transport and timeout errors, one retry on 429
//! honouring `Retry-After`, non-2xx with the body kept for the log, JSON decode.
//!
//! Port adapters never see `reqwest`: the `From` impls at the bottom turn `HttpError`
//! into each port's own error, so a `?` is all an adapter writes. Anything with
//! business meaning (a 422 on an order means "duplicate", not "failed") is matched
//! explicitly in the adapter before the `?`.

use std::time::Duration;

use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;

use crate::domain::error::{BrokerError, FundamentalError, MarketError, NewsError};

#[derive(Debug)]
pub(crate) enum HttpError {
    /// Connection, DNS, TLS: the request never got an answer.
    Transport(String),
    Timeout,
    /// 429 twice in a row; the first one was retried after `Retry-After`.
    RateLimited,
    /// Any other non-2xx, body kept for the log.
    Status {
        code: StatusCode,
        body: String,
    },
    Decode(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Transport(m) => write!(f, "transport: {m}"),
            HttpError::Timeout => f.write_str("timed out"),
            HttpError::RateLimited => f.write_str("rate limited"),
            HttpError::Status { code, body } => write!(f, "{code}: {body}"),
            HttpError::Decode(m) => write!(f, "decode: {m}"),
        }
    }
}

/// Wait when a 429 carries no usable `Retry-After`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(1);

/// Send `req`, decode the 2xx body as `T`. `what` names the call (the path) so every
/// error says which endpoint it came from. On 429 the request is cloned and re-sent
/// once after `Retry-After`; a second 429 is `RateLimited` and the caller decides.
pub(crate) async fn send_json<T: DeserializeOwned>(
    what: &str,
    req: RequestBuilder,
) -> Result<T, HttpError> {
    let retry = req.try_clone();
    let mut resp = send_once(what, req).await?;
    if resp.status() == StatusCode::TOO_MANY_REQUESTS {
        let Some(retry) = retry else {
            return Err(HttpError::RateLimited);
        };
        tokio::time::sleep(retry_after(&resp)).await;
        resp = send_once(what, retry).await?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(HttpError::RateLimited);
        }
    }
    let code = resp.status();
    if !code.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(HttpError::Status {
            code,
            body: format!("{what}: {body}"),
        });
    }
    resp.json::<T>()
        .await
        .map_err(|e| HttpError::Decode(format!("{what}: {e}")))
}

async fn send_once(what: &str, req: RequestBuilder) -> Result<reqwest::Response, HttpError> {
    req.send().await.map_err(|e| {
        if e.is_timeout() {
            HttpError::Timeout
        } else {
            HttpError::Transport(format!("{what}: {e}"))
        }
    })
}

fn retry_after(resp: &reqwest::Response) -> Duration {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RETRY_AFTER)
}

// ---- HttpError → each port's error --------------------------------------------------
// Ports keep their own error enums (a caller of `Broker` should not know HTTP exists);
// these impls are the only coupling, and they live here, next to the type they consume.

impl From<HttpError> for MarketError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Timeout => MarketError::Timeout,
            HttpError::Decode(m) => MarketError::Decode(m),
            other => MarketError::Request(other.to_string()),
        }
    }
}

impl From<HttpError> for BrokerError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Decode(m) => BrokerError::Decode(m),
            other => BrokerError::Request(other.to_string()),
        }
    }
}

impl From<HttpError> for NewsError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Timeout => NewsError::Timeout,
            HttpError::RateLimited => NewsError::RateLimited,
            HttpError::Decode(m) => NewsError::Decode(m),
            other => NewsError::Request(other.to_string()),
        }
    }
}

impl From<HttpError> for FundamentalError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Timeout => FundamentalError::Timeout,
            HttpError::RateLimited => FundamentalError::RateLimited,
            HttpError::Decode(m) => FundamentalError::Decode(m),
            other => FundamentalError::Request(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16) -> HttpError {
        HttpError::Status {
            code: StatusCode::from_u16(code).unwrap(),
            body: "/x: nope".into(),
        }
    }

    #[test]
    fn timeouts_and_rate_limits_keep_their_identity_where_the_port_has_one() {
        assert!(matches!(
            MarketError::from(HttpError::Timeout),
            MarketError::Timeout
        ));
        assert!(matches!(
            NewsError::from(HttpError::RateLimited),
            NewsError::RateLimited
        ));
        assert!(matches!(
            FundamentalError::from(HttpError::RateLimited),
            FundamentalError::RateLimited
        ));
        // Broker has no Timeout variant: it degrades to Request with the reason kept.
        assert!(
            matches!(BrokerError::from(HttpError::Timeout), BrokerError::Request(m) if m == "timed out")
        );
    }

    #[test]
    fn status_errors_carry_endpoint_and_body() {
        let MarketError::Request(m) = MarketError::from(status(403)) else {
            panic!("expected Request")
        };
        assert_eq!(m, "403 Forbidden: /x: nope");
    }

    #[test]
    fn decode_stays_decode_everywhere() {
        let d = || HttpError::Decode("/x: missing field".into());
        assert!(matches!(MarketError::from(d()), MarketError::Decode(_)));
        assert!(matches!(BrokerError::from(d()), BrokerError::Decode(_)));
        assert!(matches!(NewsError::from(d()), NewsError::Decode(_)));
        assert!(matches!(
            FundamentalError::from(d()),
            FundamentalError::Decode(_)
        ));
    }
}
