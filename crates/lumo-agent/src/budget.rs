use std::time::{Duration, Instant};

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct AgentBudget {
    max_steps: u32,
    max_runtime: Duration,
    max_tokens: u64,
    max_cost_usd_micro: u64,
    steps_used: u32,
    tokens_used: u64,
    cost_used_usd_micro: u64,
    started_at: Instant,
}

impl AgentBudget {
    pub fn new(
        max_steps: u32,
        max_runtime_ms: u64,
        max_tokens: u64,
        max_cost_usd_micro: u64,
    ) -> Self {
        Self {
            max_steps,
            max_runtime: Duration::from_millis(max_runtime_ms),
            max_tokens,
            max_cost_usd_micro,
            steps_used: 0,
            tokens_used: 0,
            cost_used_usd_micro: 0,
            started_at: Instant::now(),
        }
    }

    pub fn reserve_step(&mut self) -> Result<(), BudgetExceeded> {
        self.check_runtime()?;
        if self.steps_used >= self.max_steps {
            return Err(BudgetExceeded::Steps {
                limit: self.max_steps,
            });
        }
        self.steps_used += 1;
        Ok(())
    }

    pub fn charge_usage(
        &mut self,
        tokens: u64,
        cost_usd_micro: u64,
    ) -> Result<(), BudgetExceeded> {
        self.check_runtime()?;
        let next_tokens = self.tokens_used.saturating_add(tokens);
        if next_tokens > self.max_tokens {
            return Err(BudgetExceeded::Tokens {
                limit: self.max_tokens,
            });
        }
        let next_cost = self.cost_used_usd_micro.saturating_add(cost_usd_micro);
        if self.max_cost_usd_micro > 0 && next_cost > self.max_cost_usd_micro {
            return Err(BudgetExceeded::Cost {
                limit_usd_micro: self.max_cost_usd_micro,
            });
        }
        self.tokens_used = next_tokens;
        self.cost_used_usd_micro = next_cost;
        Ok(())
    }

    pub fn check_runtime(&self) -> Result<(), BudgetExceeded> {
        if self.started_at.elapsed() >= self.max_runtime {
            return Err(BudgetExceeded::Runtime {
                limit_ms: self.max_runtime.as_millis() as u64,
            });
        }
        Ok(())
    }

    pub fn remaining_runtime(&self) -> Result<Duration, BudgetExceeded> {
        self.max_runtime
            .checked_sub(self.started_at.elapsed())
            .ok_or(BudgetExceeded::Runtime {
                limit_ms: self.max_runtime.as_millis() as u64,
            })
    }

    pub fn steps_used(&self) -> u32 {
        self.steps_used
    }

    pub fn tokens_used(&self) -> u64 {
        self.tokens_used
    }

    pub fn cost_used_usd_micro(&self) -> u64 {
        self.cost_used_usd_micro
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetExceeded {
    #[error("step budget exhausted at {limit}")]
    Steps { limit: u32 },
    #[error("runtime budget exhausted at {limit_ms}ms")]
    Runtime { limit_ms: u64 },
    #[error("token budget exhausted at {limit}")]
    Tokens { limit: u64 },
    #[error("cost budget exhausted at {limit_usd_micro} micro-US dollars")]
    Cost { limit_usd_micro: u64 },
}

impl BudgetExceeded {
    pub fn limit(&self) -> &'static str {
        match self {
            Self::Steps { .. } => "steps",
            Self::Runtime { .. } => "runtime",
            Self::Tokens { .. } => "tokens",
            Self::Cost { .. } => "cost",
        }
    }
}
