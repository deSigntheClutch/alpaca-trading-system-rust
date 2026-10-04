use async_trait::async_trait;

use crate::domain::error::NewsError;
use crate::domain::types::{Headline, Symbol};

/// Recent headlines by symbol. impl #1 = Alpaca News (Benzinga). The port returns
/// what the vendor tagged, newest first; the consumer decides what to do with the
/// text. Headlines are untrusted input wherever they meet a prompt (#66).
#[async_trait]
pub trait NewsSource: Send + Sync {
    /// Headlines mentioning any of `symbols` published in `[since, until)` (epoch
    /// millis), newest first, at most `limit`. `until` is what makes a replay honest:
    /// a report for a past morning must not see that afternoon's news.
    async fn headlines_between(
        &self,
        symbols: &[Symbol],
        since: i64,
        until: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError>;

    /// `[since, now)`.
    async fn headlines(
        &self,
        symbols: &[Symbol],
        since: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError> {
        self.headlines_between(symbols, since, i64::MAX, limit)
            .await
    }
}

#[async_trait]
impl<T: NewsSource + ?Sized> NewsSource for &T {
    async fn headlines_between(
        &self,
        symbols: &[Symbol],
        since: i64,
        until: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError> {
        (**self)
            .headlines_between(symbols, since, until, limit)
            .await
    }

    async fn headlines(
        &self,
        symbols: &[Symbol],
        since: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError> {
        (**self).headlines(symbols, since, limit).await
    }
}
