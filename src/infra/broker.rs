//! `Broker` adapters. `AlpacaBroker`: account, positions and orders on the paper-api
//! host, over a shared `AlpacaClient`.

use std::sync::Arc;

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::domain::error::BrokerError;
use crate::domain::types::{Order, OrderRequest, Position, Symbol};
use crate::traits::Broker;

use super::clients::alpaca::{RawAccount, RawOrder, RawPosition};
use super::clients::AlpacaClient;

pub struct AlpacaBroker {
    client: Arc<AlpacaClient>,
}

impl AlpacaBroker {
    pub fn new(client: Arc<AlpacaClient>) -> Self {
        Self { client }
    }

    async fn order_by_client_id(&self, client_order_id: &str) -> Result<Order, BrokerError> {
        let url = self.client.trade_url("/v2/orders:by_client_order_id");
        let raw: RawOrder = self
            .client
            .get(&url)
            .query(&[("client_order_id", client_order_id)])
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| BrokerError::Rejected(e.to_string()))?
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
        Ok(raw.into())
    }
}

#[async_trait]
impl Broker for AlpacaBroker {
    async fn buying_power(&self) -> Result<Decimal, BrokerError> {
        let url = self.client.trade_url("/v2/account");
        let acct: RawAccount = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
        acct.buying_power
            .parse()
            .map_err(|e| BrokerError::Decode(format!("buying_power: {e}")))
    }

    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        let url = self.client.trade_url("/v2/positions");
        let raws: Vec<RawPosition> = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
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
        let url = self.client.trade_url("/v2/orders");
        let body = serde_json::json!({
            "symbol": req.symbol.as_str(),
            "qty": req.qty.to_string(),
            "side": req.side,           // Side serializes lowercase
            "type": "market",
            "time_in_force": "gtc",
            "client_order_id": req.client_order_id,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?;

        // 422 = duplicate client_order_id (idempotency): resolve to the existing order
        // rather than double-submitting.
        if resp.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            return self.order_by_client_id(&req.client_order_id).await;
        }
        let resp = resp
            .error_for_status()
            .map_err(|e| BrokerError::Rejected(e.to_string()))?;
        let raw: RawOrder = resp
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
        Ok(raw.into())
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
