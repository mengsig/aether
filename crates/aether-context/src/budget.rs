//! Token budgets derived from a model's capability profile.

use aether_core::model::CapabilityProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub window: u32,
    /// Tokens reserved for the model's reply.
    pub reserve_output: u32,
    /// Cap on the retrieved-memory layer.
    pub memory_max: u32,
    /// Fraction of the window at which compaction is requested.
    pub compaction_threshold_pct: u32,
    /// Absolute size at which compaction is requested, when the model has one;
    /// it replaces the fraction.
    pub compaction_tokens: Option<u32>,
    /// The provider compacts on its server; Aether folds only on overflow.
    pub server_compaction: bool,
}

impl ContextBudget {
    pub fn for_profile(profile: &CapabilityProfile, max_output: Option<u32>) -> Self {
        let reserve_output = max_output
            .unwrap_or(profile.max_output_tokens.min(16_000))
            .min(profile.max_output_tokens)
            .max(256);
        let window = profile.context_window.max(reserve_output + 1_024);
        Self {
            window,
            reserve_output,
            memory_max: (window / 40).clamp(400, 2_000),
            compaction_threshold_pct: 75,
            compaction_tokens: profile.compaction_tokens.map(|tokens| tokens.min(window)),
            server_compaction: profile.compacts_on_server(),
        }
    }

    /// Tokens available to the recent tail after the fixed layers.
    pub fn tail_budget(&self, fixed_tokens: u32) -> u32 {
        self.window
            .saturating_sub(self.reserve_output)
            .saturating_sub(fixed_tokens)
    }

    pub fn over_threshold(&self, used: u32) -> bool {
        if self.server_compaction {
            return false;
        }
        match self.compaction_tokens {
            Some(tokens) => used >= tokens,
            None => used.saturating_mul(100) / self.window >= self.compaction_threshold_pct,
        }
    }

    /// The size at which compaction is requested, for recording in the event.
    pub fn threshold_tokens(&self) -> u32 {
        self.compaction_tokens
            .unwrap_or(self.window / 100 * self.compaction_threshold_pct)
    }

    /// Thinking budget for models that take a fixed count: a share of the
    /// reserved output, never below the provider minimum.
    pub fn thinking_budget(&self) -> u32 {
        (self.reserve_output / 2).clamp(1_024, 32_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_are_sane() {
        let profile = CapabilityProfile {
            context_window: 200_000,
            max_output_tokens: 32_000,
            ..Default::default()
        };
        let b = ContextBudget::for_profile(&profile, None);
        assert_eq!(b.reserve_output, 16_000);
        assert_eq!(b.tail_budget(4_000), 180_000);
        assert!(!b.over_threshold(100_000));
        assert!(b.over_threshold(150_000));
        let b = ContextBudget::for_profile(&profile, Some(64_000));
        assert_eq!(b.reserve_output, 32_000, "capped by the profile");
        assert_eq!(b.thinking_budget(), 16_000);
    }

    #[test]
    fn an_absolute_threshold_replaces_the_fraction_and_stays_inside_the_window() {
        let profile = CapabilityProfile {
            context_window: 1_000_000,
            compaction_tokens: Some(256_000),
            ..Default::default()
        };
        let b = ContextBudget::for_profile(&profile, None);
        assert!(!b.over_threshold(255_999));
        assert!(b.over_threshold(256_000));
        assert_eq!(b.threshold_tokens(), 256_000);
        let small = CapabilityProfile {
            context_window: 100_000,
            compaction_tokens: Some(256_000),
            ..Default::default()
        };
        assert_eq!(
            ContextBudget::for_profile(&small, None).threshold_tokens(),
            100_000
        );
    }
}
