//! Trader agent: turns the analyst's read + the signal into a typed `Decision` (strict
//! JSON). Any failure — the LLM call erroring is bubbled up; bad/unparseable JSON or a
//! missing action collapses to Hold — so a broken reply is never a rogue order. The
//! guards downstream are still the last clamp; this is the first line of defense.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::domain::error::LlmError;
use crate::domain::types::{Action, Decision, Symbol};
use crate::traits::LlmClient;

const SYSTEM: &str = "You are a disciplined crypto trader. Given an analysis and \
signal, decide an action. Respond with ONLY a JSON object: \
{\"action\":\"BUY|SELL|HOLD\",\"qty\":<number>,\"score\":<-1..1>,\"reason\":\"...\"}. \
qty is in units of the base asset. Prefer HOLD when uncertain.";

/// Everything the trader weighs for one decision. A struct (not a long arg list) so it
/// stays readable as inputs grow — few-shot memory from `engine::learning` slots in here.
pub struct TraderInput<'a> {
    pub symbol: &'a Symbol,
    pub analysis: &'a str,
    /// Bull/bear transcript, or empty to skip the debate block entirely.
    pub debate: &'a str,
    pub signal_score: f64,
    pub position_qty: Decimal,
    pub buying_power: Decimal,
}

/// The trader role. Unit for now; gains fields (e.g. injected few-shot memory) later.
#[derive(Default)]
pub struct Trader;

impl Trader {
    /// Decide an action. Returns the parsed `Decision` AND the raw reply — the pipeline
    /// persists the raw text as the `decision` WAL step, so parsing stays observable.
    pub async fn run<L: LlmClient>(
        &self,
        llm: &L,
        input: TraderInput<'_>,
    ) -> Result<(Decision, String), LlmError> {
        let TraderInput {
            symbol,
            analysis,
            debate,
            signal_score,
            position_qty,
            buying_power,
        } = input;
        // Empty debate ⇒ prompt is byte-identical to the pre-debate trader (backtest/tests
        // that skip debate see no change); a transcript is folded in as its own block.
        let debate_block = if debate.is_empty() {
            String::new()
        } else {
            format!("\n\nBull/bear debate:\n{debate}")
        };
        let user = format!(
            "Analysis:\n{analysis}{debate_block}\n\nSignal score: {signal_score:.3}\nPosition qty: {position_qty}\nBuying power: {buying_power}\nRespond with ONLY the JSON object.",
        );
        let raw = llm.message(SYSTEM, &user).await?;
        let decision = parse_decision(symbol, &raw);
        Ok((decision, raw))
    }
}

fn hold(symbol: &Symbol, why: &str) -> Decision {
    Decision {
        symbol: symbol.clone(),
        action: Action::Hold,
        qty: Decimal::ZERO,
        score: 0.0,
        reason: why.to_string(),
    }
}

/// Parse the decision agent's reply. Tolerant of prose around the JSON; ANY failure
/// (bad JSON, missing/unknown action) collapses to Hold — never a rogue order.
fn parse_decision(symbol: &Symbol, raw: &str) -> Decision {
    let v: Value = match serde_json::from_str(extract_json(raw)) {
        Ok(v) => v,
        Err(_) => return hold(symbol, "decision JSON parse failed"),
    };
    let action = match v
        .get("action")
        .and_then(Value::as_str)
        .map(|s| s.to_ascii_uppercase())
        .as_deref()
    {
        Some("BUY") => Action::Buy,
        Some("SELL") => Action::Sell,
        Some("HOLD") => Action::Hold,
        _ => return hold(symbol, "missing/invalid action"),
    };
    Decision {
        symbol: symbol.clone(),
        action,
        qty: parse_decimal(v.get("qty")),
        score: v.get("score").and_then(Value::as_f64).unwrap_or(0.0),
        reason: v
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

/// Slice from the first `{` to the last `}` — strips code fences / prose.
fn extract_json(s: &str) -> &str {
    match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b > a => &s[a..=b],
        _ => s,
    }
}

fn parse_decimal(v: Option<&Value>) -> Decimal {
    match v {
        Some(Value::String(s)) => s.parse().unwrap_or_default(),
        Some(Value::Number(n)) => n
            .as_f64()
            .and_then(Decimal::from_f64_retain)
            .unwrap_or_default(),
        _ => Decimal::ZERO,
    }
}
