use lumo_core::CancelToken;

use crate::{AgentBudget, AgentPlan, LoopEngine, LoopReport};

#[derive(Clone)]
pub struct AgentHarness {
    engine: LoopEngine,
}

impl AgentHarness {
    pub fn new(engine: LoopEngine) -> Self {
        Self { engine }
    }

    pub async fn execute(
        &self,
        plan: AgentPlan,
        budget: AgentBudget,
        cancel: CancelToken,
    ) -> LoopReport {
        self.engine.execute(plan, budget, cancel).await
    }
}
