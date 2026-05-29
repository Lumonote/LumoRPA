//! AI cost estimation shared by the `ai.chat` action and the VM hook helpers.
//!
//! Both the explicit `ai.chat` action (X-10) and the AI hooks (P1-4) write
//! `ai_calls` ledger rows with a `cost_usd_micro` estimate, so the rate table
//! lives here rather than being duplicated per call site.

/// Per-million-token rates in micro-USD (1_000_000 = $1). Numbers are the
/// public list prices from the providers; off-list models default to a
/// conservative "premium-flagship" rate so the user is more likely to see
/// over-estimate than under-estimate. Update as the providers change.
fn price_per_million_micro(provider: &str, model: &str) -> (i64, i64) {
    let m = model.to_ascii_lowercase();
    match provider {
        "anthropic" => {
            if m.contains("opus-4") {
                (15_000_000, 75_000_000)
            } else if m.contains("sonnet-4") {
                (3_000_000, 15_000_000)
            } else if m.contains("haiku-4") {
                (800_000, 4_000_000)
            } else if m.contains("haiku") {
                (250_000, 1_250_000)
            } else if m.contains("sonnet") {
                (3_000_000, 15_000_000)
            } else if m.contains("opus") {
                (15_000_000, 75_000_000)
            } else {
                (3_000_000, 15_000_000)
            }
        }
        "openai" => {
            if m.starts_with("gpt-5") {
                (5_000_000, 15_000_000)
            } else if m.contains("4o-mini") {
                (150_000, 600_000)
            } else if m.contains("4o") {
                (2_500_000, 10_000_000)
            } else if m.contains("gpt-4") {
                (10_000_000, 30_000_000)
            } else {
                (1_000_000, 3_000_000)
            }
        }
        _ => (1_000_000, 3_000_000),
    }
}

/// Estimate the cost of one call in micro-USD from its token counts.
pub(crate) fn cost_micro(provider: &str, model: &str, input: u32, output: u32) -> i64 {
    let (rin, rout) = price_per_million_micro(provider, model);
    let in_cost = (input as i64 * rin) / 1_000_000;
    let out_cost = (output as i64 * rout) / 1_000_000;
    in_cost + out_cost
}
