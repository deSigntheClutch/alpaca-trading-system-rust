use crate::domain::error::BrokerError;
use crate::domain::types::{Order, OrderRequest, Position};
use async_trait::async_trait;
use rust_decimal::Decimal;

/// Order placement + account state. impl #1 = Alpaca REST (paper). The pipeline is
/// generic over this, so execution is testable with a fake broker (no live orders).
#[async_trait]
pub trait Broker: Send + Sync {
    /// Cash available to open new positions.
    async fn buying_power(&self) -> Result<Decimal, BrokerError>;
    /// Current open positions (source of truth for SELL sizing).
    async fn positions(&self) -> Result<Vec<Position>, BrokerError>;
    /// Submit a market order. Idempotent on `client_order_id`: a duplicate must
    /// resolve to the already-placed order, never a second submission.
    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError>;
}

#[async_trait]
impl<T: Broker + ?Sized> Broker for &T {
    async fn buying_power(&self) -> Result<Decimal, BrokerError> {
        (**self).buying_power().await
    }
    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        (**self).positions().await
    }
    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError> {
        (**self).submit_order(req).await
    }
}
