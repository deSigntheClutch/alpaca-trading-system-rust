use async_trait::async_trait;

use crate::domain::error::NewsError;
use crate::domain::types::{Headline, Symbol};

/// Recent headlines by symbol. impl #1 = Alpaca News (Benzinga). The port returns
/// what the vendor tagged, newest first; the consumer decides what to do with the
/// text. Headlines are untrusted input wherever they meet a prompt (#66).
#[async_trait]
pub trait NewsSource: Send + Sync {
    /// Headlines mentioning any of `symbols`, published at or after `since`
    /// (epoch millis), newest first, at most `limit`.
    async fn headlines(
        &self,
        symbols: &[Symbol],
        since: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError>;
}

#[async_trait]
impl<T: NewsSource + ?Sized> NewsSource for &T {
    async fn headlines(
        &self,
        symbols: &[Symbol],
        since: i64,
        limit: usize,
    ) -> Result<Vec<Headline>, NewsError> {
        (**self).headlines(symbols, since, limit).await
    }
}
