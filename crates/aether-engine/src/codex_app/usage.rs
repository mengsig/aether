//! Native cumulative-counter accounting and context-usage conversion.

use aether_core::model::Usage;
use serde_json::Value;

/// Field-wise `now - earlier` over two readings of the same cumulative
/// counter, floored at zero. Tokens only; the price is folded on the outcome.
pub(crate) fn consumed_since(now: Usage, earlier: Usage) -> Usage {
    let reasoning = now
        .reasoning_tokens
        .map(|n| n.saturating_sub(earlier.reasoning_tokens.unwrap_or(0)));
    Usage {
        input_tokens: now.input_tokens.saturating_sub(earlier.input_tokens),
        output_tokens: now.output_tokens.saturating_sub(earlier.output_tokens),
        cache_read_tokens: now
            .cache_read_tokens
            .saturating_sub(earlier.cache_read_tokens),
        cache_write_tokens: now
            .cache_write_tokens
            .saturating_sub(earlier.cache_write_tokens),
        reasoning_tokens: reasoning,
        cost_usd: 0.0,
        cost_complete: true,
        cost_gap: None,
        cost_basis: Default::default(),
        estimated_cost_usd: None,
    }
}

/// The counters Codex reports, before the cached-inside-input split. Kept in
/// the shape the engine sends so one counter restarting cannot make another's
/// derived value jump: `cachedInputTokens` dropping to zero on a compaction
/// would otherwise read as half a megatoken of fresh input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RawTotals {
    pub(crate) input: u64,
    pub(crate) cached: u64,
    pub(crate) output: u64,
    pub(crate) cache_write: u64,
    pub(crate) reasoning: u64,
}

impl RawTotals {
    /// The reading a persisted [`Usage`] came from. Lossless for anything
    /// [`RawTotals::into_usage`] produced, which is every figure persisted.
    pub(crate) fn of(usage: Usage) -> Self {
        Self {
            input: usage.input_tokens.saturating_add(usage.cache_read_tokens),
            cached: usage.cache_read_tokens,
            output: usage.output_tokens,
            cache_write: usage.cache_write_tokens,
            reasoning: usage.reasoning_tokens.unwrap_or(0),
        }
    }

    pub(crate) fn into_usage(self) -> Usage {
        Usage {
            // Cached tokens are counted inside `inputTokens`; split them out
            // so the cheaper rate is not charged at the input rate.
            input_tokens: self.input.saturating_sub(self.cached),
            output_tokens: self.output,
            cache_read_tokens: self.cached,
            cache_write_tokens: self.cache_write,
            reasoning_tokens: (self.reasoning > 0).then_some(self.reasoning),
            // Tokens only: the price is on the outcome, and any estimate for
            // it comes from the registry, in the runtime.
            cost_usd: 0.0,
            cost_complete: true,
            cost_gap: None,
            cost_basis: Default::default(),
            estimated_cost_usd: None,
        }
    }
}

/// How one cumulative counter moved between two readings.
pub(crate) struct Advance {
    /// Fresh spend this reading adds.
    pub(crate) billed: u64,
    /// What the next reading is compared against.
    pub(crate) carried: u64,
    /// How far this reading came back below the previous one. Zero unless the
    /// counter went backwards at all; reported whichever way it was read.
    pub(crate) dipped: u64,
    pub(crate) restarted: bool,
}

/// A counter that came back smaller than the amount it lost has restarted - a
/// new thread, a compaction - so its whole new reading is spend nobody has
/// paid for. A shallower dip is a correction: bill nothing and hold the
/// higher reading, so a climb back to it is not billed a second time. Never
/// a refund either way. Both are the counter contradicting itself, reported
/// either way; only the depth decides which way it is billed, and which of
/// the two notes says so.
pub(crate) fn advanced(now: u64, last: u64) -> Advance {
    let Some(dropped) = last.checked_sub(now).filter(|d| *d > 0) else {
        return Advance {
            billed: now - last,
            carried: now,
            dipped: 0,
            restarted: false,
        };
    };
    if now <= dropped {
        return Advance {
            billed: now,
            carried: now,
            dipped: dropped,
            restarted: true,
        };
    }
    Advance {
        billed: 0,
        carried: last,
        dipped: dropped,
        restarted: false,
    }
}

/// This reading's raw counters, and the fields it could not supply. An absent
/// or unparseable field is NOT zero: read as zero it looks like a restarted
/// counter and re-bills the whole thread, so the previous reading
/// stands for it instead.
pub(super) fn read_totals(total: &Value, previous: RawTotals) -> (RawTotals, Vec<&'static str>) {
    const REASONING: &str = "reasoningOutputTokens";
    let mut unusable = Vec::new();
    let mut read = |key: &'static str, previous: u64| -> u64 {
        let raw = &total[key];
        if raw.is_null() {
            // Reasoning is optional by contract: a build that reports it only
            // when there is some must not read as a counter that went away.
            if previous > 0 && key != REASONING {
                unusable.push(key);
            }
            return previous;
        }
        match raw.as_u64() {
            // Same contract: a reported zero means "none", not "reset to 0".
            Some(0) if key == REASONING => previous,
            Some(n) => n,
            None => {
                unusable.push(key);
                previous
            }
        }
    };
    let totals = RawTotals {
        input: read("inputTokens", previous.input),
        cached: read("cachedInputTokens", previous.cached),
        output: read("outputTokens", previous.output),
        cache_write: read("cacheWriteInputTokens", previous.cache_write),
        reasoning: read(REASONING, previous.reasoning),
    };
    (totals, unusable)
}

/// Codex counts cached tokens inside `inputTokens`; split them out so cost
/// is not charged twice. For `last` (the context size) only - billing reads
/// the counters through [`read_totals`], which carries fields it cannot read.
pub(super) fn usage_of(v: &Value) -> Usage {
    let get = |k: &str| v[k].as_u64().unwrap_or(0);
    let cached = get("cachedInputTokens");
    let reasoning = get("reasoningOutputTokens");
    Usage {
        input_tokens: get("inputTokens").saturating_sub(cached),
        output_tokens: get("outputTokens"),
        cache_read_tokens: cached,
        cache_write_tokens: get("cacheWriteInputTokens"),
        reasoning_tokens: (reasoning > 0).then_some(reasoning),
        // Tokens only; the price is on the outcome and is folded once there.
        cost_usd: 0.0,
        cost_complete: true,
        cost_gap: None,
        cost_basis: Default::default(),
        estimated_cost_usd: None,
    }
}
