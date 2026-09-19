//! Analyst agent: turns a technical signal + recent prices into a short prose read of
//! the setup. This is the entry point that grows as we feed the decision more data —
//! new inputs get folded into the user message here, not into the pipeline.

use crate::domain::error::LlmError;
use crate::domain::signals::Signal;
use crate::domain::types::Symbol;
use crate::traits::LlmClient;

const SYSTEM: &str = "You are a crypto market analyst. Given a technical signal \
and recent prices, write 2-3 sentences on the setup. Be concise and non-committal about \
risk. This is educational, not investment advice.";

/// The analyst role. Unit for now; gains fields (e.g. injected few-shot memory) later.
#[derive(Default)]
pub struct Analyst;

impl Analyst {
    /// Produce the analyst's prose take on the current setup.
    pub async fn run<L: LlmClient>(
        &self,
        llm: &L,
        symbol: &Symbol,
        last_close: f64,
        sig: &Signal,
        recent_closes: &[f64],
    ) -> Result<String, LlmError> {
        let user = format!(
            "Symbol {symbol}\nLast close: {last_close}\nSignal score: {:.3} (RSI {:.1})\nRecent closes: {recent_closes:?}",
            sig.score, sig.rsi,
        );
        llm.message(SYSTEM, &user).await
    }
}
