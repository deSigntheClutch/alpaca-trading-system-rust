//! Config: secrets from env (`.env`), strategies from a TOML file. The live barrier
//! is fail-closed — a non-paper base aborts the load unless `ALLOW_LIVE=1`.

use serde::Deserialize;

use super::error::ConfigError;

const DEFAULT_PAPER_BASE: &str = "https://paper-api.alpaca.markets";
const DEFAULT_CRYPTO_WS: &str = "wss://stream.data.alpaca.markets/v1beta3/crypto/us";

/// How often a strategy runs. Crypto = `Stream` (resident); stock = `Cron` (later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tempo {
    #[default]
    Stream,
    Cron,
}

/// One strategy from `strategies.toml`. Symbols stay `String` here; callers wrap
/// them in `Symbol` at use.
#[derive(Debug, Clone, Deserialize)]
pub struct Strategy {
    pub id: String,
    pub symbols: Vec<String>,
    #[serde(default)]
    pub tempo: Tempo,
}

#[derive(Debug, Deserialize)]
struct StrategiesFile {
    strategies: Vec<Strategy>,
}

/// Resolved runtime config. Secrets from env; strategies from TOML.
#[derive(Debug, Clone)]
pub struct Config {
    pub alpaca_key: String,
    pub alpaca_secret: String,
    pub anthropic_key: String,
    /// Trading REST base. Paper by default.
    pub alpaca_trade_base: String,
    /// Crypto market-data websocket endpoint.
    pub alpaca_data_ws: String,
    /// True only when a non-paper base was explicitly allowed.
    pub allow_live: bool,
    pub strategies: Vec<Strategy>,
}

impl Config {
    /// Load strategies from `strategies_path` and secrets from the environment.
    /// Missing keys are tolerated (skeleton/dry runs); the live barrier is not.
    pub fn load(strategies_path: &str) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(strategies_path)
            .map_err(|e| ConfigError::File(strategies_path.to_string(), e.to_string()))?;
        let parsed: StrategiesFile =
            toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string()))?;

        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let base = {
            let v = env("ALPACA_BASE");
            if v.is_empty() {
                DEFAULT_PAPER_BASE.to_string()
            } else {
                v
            }
        };
        let allow_live = env("ALLOW_LIVE") == "1";

        // Fail-closed: any base that isn't clearly the paper endpoint requires opt-in.
        if !base.contains("paper") && !allow_live {
            return Err(ConfigError::LiveBlocked(base));
        }

        let data_ws = {
            let v = env("ALPACA_DATA_WS");
            if v.is_empty() {
                DEFAULT_CRYPTO_WS.to_string()
            } else {
                v
            }
        };

        Ok(Self {
            alpaca_key: env("ALPACA_KEY"),
            alpaca_secret: env("ALPACA_SECRET"),
            anthropic_key: env("ANTHROPIC_API_KEY"),
            alpaca_trade_base: base,
            alpaca_data_ws: data_ws,
            allow_live,
            strategies: parsed.strategies,
        })
    }

    pub fn strategy(&self, id: &str) -> Option<&Strategy> {
        self.strategies.iter().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, body: &str) -> String {
        let path = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn parses_two_strategies_with_default_tempo() {
        let path = write_tmp(
            "alpaca_test_strategies.toml",
            r#"
                [[strategies]]
                id = "crypto-momentum"
                symbols = ["BTC/USD", "ETH/USD"]
                tempo = "stream"

                [[strategies]]
                id = "stock-daily"
                symbols = ["AAPL"]
            "#,
        );
        // paper base implied; no env needed
        std::env::remove_var("ALPACA_BASE");
        std::env::remove_var("ALLOW_LIVE");
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.strategies.len(), 2);
        assert_eq!(cfg.strategy("crypto-momentum").unwrap().tempo, Tempo::Stream);
        // omitted tempo defaults to Stream
        assert_eq!(cfg.strategy("stock-daily").unwrap().tempo, Tempo::Stream);
        assert!(cfg.alpaca_trade_base.contains("paper"));
    }
}
