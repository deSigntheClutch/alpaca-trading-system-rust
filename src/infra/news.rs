//! `NewsSource` adapters. `AlpacaNews`: the Alpaca News API (Benzinga feed) over a
//! shared `AlpacaClient`. Headlines only; summaries and bodies are never fetched.

use std::sync::Arc;

use async_trait::async_trait;
use itertools::Itertools as _;

use crate::domain::error::NewsError;
use crate::domain::types::{Headline, Symbol};
use crate::traits::NewsSource;

use super::clients::alpaca::{NewsResponse, RawNews};
use super::clients::{parse_rfc3339_millis, AlpacaClient};

/// Alpaca caps one news page at 50 items.
const MAX_PAGE: usize = 50;
/// Longest headline we keep. Real headlines run 60–150 chars; anything past this is
/// not a headline and is cut before it can reach a prompt.
const MAX_HEADLINE_CHARS: usize = 300;

pub struct AlpacaNews {
    client: Arc<AlpacaClient>,
}

impl AlpacaNews {
    pub fn new(client: Arc<AlpacaClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl NewsSource for AlpacaNews {
    /// One request per page of up to 50, following `next_page_token` until `limit`
    /// headlines are in hand or the vendor runs out. Vendor sort is newest-first and
    /// we re-sort locally anyway, so page order never matters.
    async fn headlines(
        &self,
        symbols: &[Symbol],
        since: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError> {
        if symbols.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let url = self.client.data_url("/v1beta1/news");
        let joined = symbols.iter().join(",");
        let start = chrono::DateTime::from_timestamp_millis(since)
            .ok_or_else(|| NewsError::Request(format!("since out of range: {since}")))?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let page_limit = limit.min(MAX_PAGE).to_string();

        let mut out: Vec<Headline> = Vec::with_capacity(limit);
        let mut page_token: Option<String> = None;
        loop {
            let mut req = self.client.get(&url).query(&[
                ("symbols", joined.as_str()),
                ("start", start.as_str()),
                ("limit", page_limit.as_str()),
                ("sort", "desc"),
                ("include_content", "false"),
            ]);
            if let Some(tok) = &page_token {
                req = req.query(&[("page_token", tok.as_str())]);
            }
            let resp = req.send().await.map_err(|e| {
                if e.is_timeout() {
                    NewsError::Timeout
                } else {
                    NewsError::Request(e.to_string())
                }
            })?;

            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(NewsError::RateLimited);
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(NewsError::Request(format!("news {status}: {body}")));
            }
            let parsed: NewsResponse = resp
                .json()
                .await
                .map_err(|e| NewsError::Decode(e.to_string()))?;

            let page_len = parsed.news.len();
            out.extend(parsed.news.into_iter().filter_map(Headline::from_raw));

            match parsed.next_page_token {
                Some(tok) if !tok.is_empty() && out.len() < limit && page_len > 0 => {
                    page_token = Some(tok)
                }
                _ => break,
            }
        }

        finish(&mut out, since, limit);
        Ok(out)
    }
}

/// Newest first, nothing older than `since`, at most `limit`. Shared by the vendor
/// adapter and the in-memory fake so both answer the port's contract identically.
fn finish(out: &mut Vec<Headline>, since: i64, limit: usize) {
    out.retain(|h| h.ts >= since);
    out.sort_by(|a, b| b.ts.cmp(&a.ts).then(a.id.cmp(&b.id)));
    out.dedup_by_key(|h| h.id);
    out.truncate(limit);
}

impl Headline {
    /// Wire → domain. Drops an item whose timestamp does not parse rather than
    /// inventing one; empty `url` becomes `None`.
    fn from_raw(r: RawNews) -> Option<Headline> {
        Some(Headline {
            id: r.id,
            ts: parse_rfc3339_millis(&r.created_at)?,
            headline: sanitize(&r.headline),
            source: r.source,
            url: r.url.filter(|u| !u.trim().is_empty()),
            symbols: r.symbols.into_iter().map(Symbol::from).collect(),
        })
    }
}

/// Headline hygiene at the boundary: decode the HTML entities Benzinga leaves in
/// (`&amp;`), collapse whitespace and newlines to single spaces, cap the length. This
/// is data cleaning, not prompt-injection defence; that lives where the prompt is
/// built (#66), because no string filter makes text safe to follow as instructions.
fn sanitize(s: &str) -> String {
    let decoded = s
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let collapsed = decoded.split_whitespace().join(" ");
    collapsed.chars().take(MAX_HEADLINE_CHARS).collect()
}

/// In-memory `NewsSource` for tests of whatever consumes the port (commentary
/// agent, report). Holds fixed headlines and answers the contract the way the
/// vendor would: symbol match, `since` cut-off, newest first, `limit`.
#[cfg(test)]
pub struct MemNews {
    pub items: Vec<Headline>,
}

#[cfg(test)]
#[async_trait]
impl NewsSource for MemNews {
    async fn headlines(
        &self,
        symbols: &[Symbol],
        since: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError> {
        let mut out: Vec<Headline> = self
            .items
            .iter()
            .filter(|h| h.symbols.iter().any(|s| symbols.contains(s)))
            .cloned()
            .collect();
        finish(&mut out, since, limit);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `/v1beta1/news?symbols=NVDA,MU` page, trimmed to the fields we read.
    /// Item 2 has an entity in the headline and a newline; item 3 has an empty url.
    const PAGE: &str = r#"{"news":[
      {"id":62151688,"headline":"S&amp;P 500 Index Nears Record High as Wall Street Braces for Strong Earnings Season",
       "author":"Crispus Nyaga","source":"benzinga","created_at":"2026-10-03T17:30:07Z","updated_at":"2026-10-03T17:30:08Z",
       "url":"https://www.benzinga.com/markets/equities/26/10/62151688/sp-500","symbols":["FDS","GS","MU","NVDA"],
       "summary":"The S&amp;P 500 Index is on the verge"},
      {"id":62150001,"headline":"Micron  Technology\n  Rises On Memory Pricing","source":"benzinga",
       "created_at":"2026-10-03T15:02:00Z","url":"https://www.benzinga.com/x","symbols":["MU"]},
      {"id":62149000,"headline":"Nvidia ships","source":"benzinga","created_at":"2026-10-02T09:00:00Z","url":"","symbols":["NVDA"]}
    ],"next_page_token":"MTc5MTAyODgxMDAwMDAwMDAwMHw2MjE1MTI5NA=="}"#;

    fn page() -> NewsResponse {
        serde_json::from_str(PAGE).expect("fixture parses")
    }

    #[test]
    fn page_fixture_maps_every_item() {
        let items: Vec<Headline> = page()
            .news
            .into_iter()
            .filter_map(Headline::from_raw)
            .collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, 62151688);
        assert_eq!(items[0].ts, 1_791_048_607_000);
        assert_eq!(items[0].symbols.len(), 4);
        assert!(items[0].symbols.contains(&Symbol::from("NVDA")));
        assert_eq!(items[0].source, "benzinga");
        assert_eq!(
            page().next_page_token.as_deref().map(|t| t.is_empty()),
            Some(false)
        );
    }

    #[test]
    fn headlines_are_cleaned_at_the_boundary() {
        let items: Vec<Headline> = page()
            .news
            .into_iter()
            .filter_map(Headline::from_raw)
            .collect();
        assert!(
            items[0].headline.starts_with("S&P 500 Index"),
            "{}",
            items[0].headline
        );
        assert_eq!(
            items[1].headline,
            "Micron Technology Rises On Memory Pricing"
        );
        assert_eq!(items[2].url, None, "empty url becomes None");
        assert_eq!(items[1].url.as_deref(), Some("https://www.benzinga.com/x"));
    }

    #[test]
    fn unparsable_timestamp_drops_the_item() {
        let raw: NewsResponse = serde_json::from_str(
            r#"{"news":[{"id":1,"headline":"x","created_at":"yesterday","symbols":["A"]}]}"#,
        )
        .unwrap();
        assert!(raw
            .news
            .into_iter()
            .filter_map(Headline::from_raw)
            .next()
            .is_none());
    }

    #[test]
    fn sanitize_caps_length_and_decodes_entities() {
        let long = "a".repeat(1000);
        assert_eq!(sanitize(&long).chars().count(), MAX_HEADLINE_CHARS);
        assert_eq!(
            sanitize("A &quot;B&quot; &lt;C&gt; &#39;D&#39;"),
            "A \"B\" <C> 'D'"
        );
        assert_eq!(sanitize("  spaced \t out \n lines "), "spaced out lines");
    }

    #[test]
    fn finish_orders_filters_dedups_and_truncates() {
        let h = |id: u64, ts: i64| Headline {
            id,
            ts,
            headline: format!("h{id}"),
            source: "t".into(),
            url: None,
            symbols: vec![Symbol::from("A")],
        };
        let mut v = vec![h(1, 100), h(2, 300), h(2, 300), h(3, 50), h(4, 200)];
        finish(&mut v, 100, 2);
        assert_eq!(v.iter().map(|x| x.id).collect::<Vec<_>>(), vec![2, 4]);
    }

    #[tokio::test]
    async fn mem_fake_answers_the_contract() {
        let h = |id: u64, ts: i64, sym: &str| Headline {
            id,
            ts,
            headline: format!("h{id}"),
            source: "t".into(),
            url: None,
            symbols: vec![Symbol::from(sym)],
        };
        let fake = MemNews {
            items: vec![
                h(1, 100, "NVDA"),
                h(2, 300, "MU"),
                h(3, 200, "AAPL"),
                h(4, 50, "NVDA"),
            ],
        };
        let got = fake
            .headlines(&[Symbol::from("NVDA"), Symbol::from("MU")], 100, 10)
            .await
            .unwrap();
        assert_eq!(
            got.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec![2, 1],
            "newest first, AAPL and the stale one excluded"
        );
    }
}
