use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::domain::error::FundamentalError;
use crate::domain::types::{EarningsEvent, Symbol};

/// Company-level data no bar feed carries: who reports when, how big a name is.
/// impl #1 = Finnhub. Universe filtering is the caller's job, not the port's.
#[async_trait]
pub trait FundamentalSource: Send + Sync {
    /// Every earnings event dated in `[from, to]`, any symbol the vendor covers.
    async fn earnings_calendar(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<EarningsEvent>, FundamentalError>;

    /// Market cap in full USD. `None` when the vendor has no figure for the name.
    async fn market_cap(&self, symbol: &Symbol) -> Result<Option<Decimal>, FundamentalError>;
}

#[async_trait]
impl<T: FundamentalSource + ?Sized> FundamentalSource for &T {
    async fn earnings_calendar(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<EarningsEvent>, FundamentalError> {
        (**self).earnings_calendar(from, to).await
    }

    async fn market_cap(&self, symbol: &Symbol) -> Result<Option<Decimal>, FundamentalError> {
        (**self).market_cap(symbol).await
    }
}
