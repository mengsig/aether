//! Per-request accounting retained across native usage fragments.
use aether_core::model::Usage;

/// Engine ids and locally minted ids occupy separate namespaces within a run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum UsageRequestId {
    /// `message.id`, as the engine reported it.
    Engine(String),
    /// Minted for a message the engine gave no id.
    Minted(u64),
}

/// Successive monotonic totals for one request within an engine run.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestUsage {
    pub id: UsageRequestId,
    pub previous: Usage,
    pub total: Usage,
}

impl RequestUsage {
    /// Newly observed tokens between the adapter's per-request high-water marks.
    pub fn delta(&self) -> Usage {
        let mut coverage = self.previous;
        coverage.merge_cost_coverage(self.total);
        debug_assert_eq!(
            coverage.cost_complete,
            self.previous.cost_complete && self.total.cost_complete
        );
        debug_assert!(coverage.cost_complete || coverage.cost_gap.is_some());
        Usage {
            input_tokens: self
                .total
                .input_tokens
                .saturating_sub(self.previous.input_tokens),
            output_tokens: self
                .total
                .output_tokens
                .saturating_sub(self.previous.output_tokens),
            cache_read_tokens: self
                .total
                .cache_read_tokens
                .saturating_sub(self.previous.cache_read_tokens),
            cache_write_tokens: self
                .total
                .cache_write_tokens
                .saturating_sub(self.previous.cache_write_tokens),
            reasoning_tokens: self
                .total
                .reasoning_tokens
                .map(|total| total.saturating_sub(self.previous.reasoning_tokens.unwrap_or(0))),
            cost_complete: coverage.cost_complete,
            cost_gap: coverage.cost_gap,
            ..Usage::default()
        }
    }
}
