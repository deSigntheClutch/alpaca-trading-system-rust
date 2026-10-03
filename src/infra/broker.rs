//! `Broker` adapters. `AlpacaBroker`: account, positions and orders on the trading
//! host, over a shared `AlpacaClient`.

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::StatusCode;
use rust_decimal::Decimal;

use crate::domain::error::BrokerError;
use crate::domain::types::{Order, OrderRequest, Position, Symbol};
use crate::traits::Broker;

use super::clients::alpaca::{Host, RawAccount, RawOrder, RawPosition};
use super::clients::http::HttpError;
use super::clients::AlpacaClient;

pub struct AlpacaBroker {
    client: Arc<AlpacaClient>,
}

impl AlpacaBroker {
    pub fn new(client: Arc<AlpacaClient>) -> Self {
        Self { client }
    }

    async fn order_by_client_id(&self, client_order_id: &str) -> Result<Order, BrokerError> {
        let raw: RawOrder = self
            .client
            .get_json(
                Host::Trade,
                "/v2/orders:by_client_order_id",
                &[("client_order_id", client_order_id)],
            )
            .await
            .map_err(rejected_on_4xx)?;
        Ok(raw.into())
    }
}

/// A 4xx from the trading host is the broker saying no; everything else is a plain
/// request failure.
fn rejected_on_4xx(e: HttpError) -> BrokerError {
    match e {
        HttpError::Status { code, body } if code.is_client_error() => {
            BrokerError::Rejected(format!("{code}: {body}"))
        }
        other => other.into(),
    }
}

#[async_trait]
impl Broker for AlpacaBroker {
    async fn buying_power(&self) -> Result<Decimal, BrokerError> {
        let acct: RawAccount = self
            .client
            .get_json(Host::Trade, "/v2/account", &[])
            .await?;
        acct.buying_power
            .parse()
            .map_err(|e| BrokerError::Decode(format!("buying_power: {e}")))
    }

    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        let raws: Vec<RawPosition> = self
            .client
            .get_json(Host::Trade, "/v2/positions", &[])
            .await?;
        raws.into_iter()
            .map(|p| {
                Ok(Position {
                    symbol: Symbol::from(p.symbol),
                    qty: p
                        .qty
                        .parse()
                        .map_err(|e| BrokerError::Decode(format!("qty: {e}")))?,
                    market_value: p
                        .market_value
                        .parse()
                        .map_err(|e| BrokerError::Decode(format!("market_value: {e}")))?,
                })
            })
            .collect()
    }

    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError> {
        let body = serde_json::json!({
            "symbol": req.symbol.as_str(),
            "qty": req.qty.to_string(),
            "side": req.side,           // Side serializes lowercase
            "type": "market",
            "time_in_force": "gtc",
            "client_order_id": req.client_order_id,
        });
        match self
            .client
            .post_json::<RawOrder>(Host::Trade, "/v2/orders", &body)
            .await
        {
            Ok(raw) => Ok(raw.into()),
            // 422 = duplicate client_order_id (idempotency): resolve to the existing
            // order rather than double-submitting.
            Err(HttpError::Status { code, .. }) if code == StatusCode::UNPROCESSABLE_ENTITY => {
                self.order_by_client_id(&req.client_order_id).await
            }
            Err(e) => Err(rejected_on_4xx(e)),
        }
    }
}

impl From<RawOrder> for Order {
    fn from(r: RawOrder) -> Self {
        Order {
            id: r.id,
            client_order_id: r.client_order_id,
            symbol: Symbol::from(r.symbol),
            status: r.status,
        }
    }
}
