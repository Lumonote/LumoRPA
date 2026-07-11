use crate::improvement::{ImprovementError, ImprovementProposal, ImprovementTarget};
use crate::trace_miner::TraceSummary;
use serde_json::json;
use sha2::{Digest, Sha256};

pub struct ProposalBuilder;

impl ProposalBuilder {
    pub fn deterministic(
        summary: &TraceSummary,
        base_version_hash: &str,
    ) -> Result<Vec<ImprovementProposal>, ImprovementError> {
        let mut proposals = Vec::new();
        for (capability_id, aggregate) in &summary.by_capability {
            let replacement = aggregate
                .replacements
                .iter()
                .filter(|(_, count)| **count >= 2)
                .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)));
            let Some((replacement, _)) = replacement else {
                continue;
            };
            let seed = format!("{capability_id}:{replacement}:{base_version_hash}");
            let id = format!("proposal-{:x}", Sha256::digest(seed.as_bytes()));
            proposals.push(ImprovementProposal::trace_proposal(
                id,
                summary.source_run_ids.clone(),
                ImprovementTarget::RouterExample {
                    capability_id: capability_id.clone(),
                },
                json!({"preferredCapability": replacement}),
                base_version_hash,
            )?);
        }
        Ok(proposals)
    }
}
