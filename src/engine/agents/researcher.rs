//! Researcher agent: the bull/bear debate that sits between the analyst and the trader.
//! Bull and bear differ only by stance + system prompt, so this is ONE struct with a
//! `Stance`, not a trait — the orchestrator just runs `[bull, bear]` for N rounds and
//! feeds the growing transcript back in, so each turn reacts to the other side. When a
//! real second behavior appears (e.g. a research-manager that synthesizes), promote to a
//! trait then; today that would be premature.

use crate::domain::error::LlmError;
use crate::domain::signals::Signal;
use crate::domain::types::Symbol;
use crate::traits::LlmClient;

const BULL_SYS: &str = "You are a bullish crypto researcher. Argue the case FOR entering \
or holding a long, grounded in the analyst's read and the signal. If the bear has spoken, \
rebut its strongest point. 2-3 concrete sentences. Educational, not investment advice.";

const BEAR_SYS: &str = "You are a bearish crypto researcher. Argue the case AGAINST the \
long — the risks, weak signal, downside — grounded in the analyst's read and the signal. \
If the bull has spoken, rebut its strongest point. 2-3 concrete sentences. Educational, \
not investment advice.";

enum Stance {
    Bull,
    Bear,
}

/// One side of the debate. Build with `Researcher::bull()` / `Researcher::bear()`.
pub struct Researcher {
    stance: Stance,
}

impl Researcher {
    pub fn bull() -> Self {
        Self {
            stance: Stance::Bull,
        }
    }

    pub fn bear() -> Self {
        Self {
            stance: Stance::Bear,
        }
    }

    /// Short label for the transcript + the `debate` WAL step.
    pub fn stance(&self) -> &'static str {
        match self.stance {
            Stance::Bull => "bull",
            Stance::Bear => "bear",
        }
    }

    fn system(&self) -> &'static str {
        match self.stance {
            Stance::Bull => BULL_SYS,
            Stance::Bear => BEAR_SYS,
        }
    }

    /// Produce this side's next argument, reacting to the debate so far.
    pub async fn argue<L: LlmClient>(
        &self,
        llm: &L,
        symbol: &Symbol,
        analysis: &str,
        sig: &Signal,
        transcript: &str,
    ) -> Result<String, LlmError> {
        let so_far = if transcript.is_empty() {
            "(you speak first)"
        } else {
            transcript
        };
        let user = format!(
            "Symbol {symbol}\nAnalyst read: {analysis}\nSignal score: {:.3} (RSI {:.1})\n\nDebate so far:\n{so_far}\n\nGive your {} argument now.",
            sig.score,
            sig.rsi,
            self.stance(),
        );
        llm.message(self.system(), &user).await
    }
}
