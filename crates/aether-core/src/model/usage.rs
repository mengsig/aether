use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

/// Where dollar figures came from; independent of whether every invocation was priced.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    /// Legacy records and unpriced usage carry no evidence of a cost source.
    #[default]
    Unknown,
    ListPrice,
    ProviderReported,
    Mixed,
}

impl CostBasis {
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Unknown => None,
            Self::ListPrice => Some("list-price estimate"),
            Self::ProviderReported => Some("provider-reported"),
            Self::Mixed => Some("mixed cost basis"),
        }
    }

    pub fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (a, b) if a == b => a,
            _ => Self::Mixed,
        }
    }
}

/// Why the dollar total may omit work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostGap {
    /// The engine did not fully report usage for work it performed.
    Unreported,
    /// Usage was reported without a price or provider cost.
    Unpriced,
    /// Possibly billed work lacks closing accounting, including cancellation after submission.
    Interrupted,
    /// Two or more different missing-cost reasons contributed.
    Mixed,
    /// Legacy records lack a reason; unrecognized future reasons also read as unknown.
    #[serde(other)]
    Unknown,
}

impl CostGap {
    pub fn combine(self, other: Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a,
            _ => Self::Mixed,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "some cost unknown",
            Self::Unpriced => Usage::COST_PARTIAL,
            Self::Unreported => "usage not fully reported",
            Self::Interrupted => "interrupted run, cost unknown",
            Self::Mixed => "multiple cost gaps",
        }
    }
}

/// Token accounting for one invocation, or a running total.
///
/// `cost_usd` sums invocations with a configured price or provider-reported
/// cost; `cost_basis` distinguishes those sources and `cost_complete` says
/// whether any invocations were left out. List prices are estimates, even
/// when every invocation was priced. A budget enforces on this accounting
/// bound - an unpriced invocation
/// cannot make what was already spent smaller - so nothing silently stops
/// firing when a model or engine reports no price.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "UsageWire")]
// UsageWire is a lenient reader for old stores; the wire shape is Usage itself.
#[schemars(!try_from)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// The part of `cache_write_tokens` written to the 1-hour cache tier. Zero
    /// where the engine did not report the split; those writes price as 5-minute ones.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write_1h_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Dollars over the invocations that could be priced. A floor when
    /// `cost_complete` is false.
    pub cost_usd: f64,
    /// Every contribution's cost is accounted for. Adding usage cannot repair a gap;
    /// closing accounting may replace provisional usage for the same run.
    pub cost_complete: bool,
    /// Missing-cost reason; absent for complete totals. Live missing usage is unreported;
    /// legacy or unrecognized reasons read as unknown. Distinct reasons combine as mixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_gap: Option<CostGap>,
    /// Source of the dollar figures, including `estimated_cost_usd` where present.
    /// List-price figures retain their existing budget semantics; this is a label.
    #[serde(default)]
    pub cost_basis: CostBasis,
    /// Dollars nobody billed, at the list price of the API model behind a
    /// subscription one: an **estimate**, so it sits beside `cost_usd`
    /// instead of in it. No budget reads it, and every surface labels it
    /// ([`Self::estimate_said`]). Like `cost_usd` it is a floor - an unpriced
    /// invocation whose model has no known API equivalent adds nothing - and
    /// `None` means nothing here was estimated at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
}

/// An empty accumulator, not a contributor that failed to report: folding
/// nothing in leaves a total complete.
impl Default for Usage {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cache_write_1h_tokens: 0,
            reasoning_tokens: None,
            cost_usd: 0.0,
            cost_complete: true,
            cost_gap: None,
            cost_basis: CostBasis::Unknown,
            estimated_cost_usd: None,
        }
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Reads a store written before `cost_complete` existed, where an absent
/// `cost_usd` was the only way to say nobody priced the invocation.
#[derive(Deserialize)]
struct UsageWire {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    #[serde(default)]
    cache_write_1h_tokens: u64,
    #[serde(default)]
    reasoning_tokens: Option<u64>,
    #[serde(default)]
    cost_usd: Option<f64>,
    #[serde(default)]
    cost_complete: Option<bool>,
    #[serde(default)]
    cost_gap: Option<CostGap>,
    #[serde(default)]
    cost_basis: CostBasis,
    /// Absent in a store written before estimates existed, and absent
    /// whenever nothing was estimated: both read as no estimate.
    #[serde(default)]
    estimated_cost_usd: Option<f64>,
}

impl TryFrom<UsageWire> for Usage {
    type Error = String;

    fn try_from(w: UsageWire) -> Result<Self, Self::Error> {
        if w.cache_write_1h_tokens > w.cache_write_tokens {
            return Err(format!(
                "cache_write_1h_tokens ({}) exceeds cache_write_tokens ({})",
                w.cache_write_1h_tokens, w.cache_write_tokens
            ));
        }
        let complete = w.cost_complete.unwrap_or(w.cost_usd.is_some());
        let gap = (!complete).then_some(w.cost_gap.unwrap_or(CostGap::Unknown));
        Ok(Self {
            input_tokens: w.input_tokens,
            output_tokens: w.output_tokens,
            cache_read_tokens: w.cache_read_tokens,
            cache_write_tokens: w.cache_write_tokens,
            cache_write_1h_tokens: w.cache_write_1h_tokens,
            reasoning_tokens: w.reasoning_tokens,
            cost_usd: w.cost_usd.unwrap_or(0.0),
            cost_complete: complete,
            cost_gap: gap,
            cost_basis: w.cost_basis,
            estimated_cost_usd: w.estimated_cost_usd,
        })
    }
}

impl Usage {
    /// How every surface says a cost where nothing at all was priced. One
    /// phrasing, so the report, the CLI and the TUI cannot each invent their
    /// own and leave a reader guessing whether they mean the same thing.
    pub const COST_UNKNOWN: &'static str = "not reported";
    /// What every surface appends to a floor, for the same reason.
    pub const COST_PARTIAL: &'static str = "some unpriced";

    /// The one phrasing, over any number this cost stands behind - the cost
    /// itself, a rate, a forecast - and the caller's rendering of it: a
    /// complete figure is that number, an incomplete one is a floor and says
    /// so, and one with nothing priced at all is not a number at all.
    pub fn said(&self, value: f64, amount: impl FnOnce(f64) -> String) -> String {
        if self.cost_complete {
            return amount(value);
        }
        let gap = self.cost_gap.unwrap_or(CostGap::Unknown);
        if self.cost_usd <= 0.0 {
            return match gap {
                CostGap::Unknown | CostGap::Unpriced => Self::COST_UNKNOWN.to_owned(),
                _ => format!("{} ({})", Self::COST_UNKNOWN, gap.label()),
            };
        }
        format!("≥ {} ({})", amount(value), gap.label())
    }

    /// [`Self::said`] about the cost itself.
    pub fn cost_said(&self, amount: impl FnOnce(f64) -> String) -> String {
        self.said(self.cost_usd, amount)
    }

    /// [`Self::cost_said`] with the four-decimal dollars every surface but
    /// the TUI uses: a cheap run is still a number.
    pub fn cost_line(&self) -> String {
        self.cost_said(|v| format!("${v:.4}"))
    }

    /// What every surface calls an estimate, and the sign it wears - one
    /// phrasing, for the same reason as the qualifiers above. A surface that
    /// can only paint ASCII replaces [`Self::APPROX`] on its way out.
    pub const COST_ESTIMATE: &'static str = "API-equivalent";
    pub const APPROX: &'static str = "≈";

    /// What this usage would have cost at API list prices: the dollars that
    /// were priced, plus the estimate for the ones that were not. `None`
    /// where nothing was estimated, so `cost_usd` already tells the whole
    /// story.
    pub fn api_equivalent_usd(&self) -> Option<f64> {
        let estimated = self.estimated_cost_usd?;
        debug_assert!(estimated >= 0.0, "an estimate is never negative");
        Some(self.cost_usd + estimated)
    }

    /// The estimate as every surface says it - `≈ $1.3600 API-equivalent` -
    /// or nothing to say. Never a bare number: a reader must not be able to
    /// mistake it for dollars somebody billed.
    pub fn estimate_said(&self, amount: impl FnOnce(f64) -> String) -> Option<String> {
        let total = self.api_equivalent_usd()?;
        Some(format!(
            "{} {} {}",
            Self::APPROX,
            amount(total),
            Self::COST_ESTIMATE
        ))
    }

    /// [`Self::estimate_said`] with the four-decimal dollars, beside
    /// [`Self::cost_line`].
    pub fn estimate_line(&self) -> Option<String> {
        self.estimate_said(|v| format!("${v:.4}"))
    }

    /// The whole truth about one usage's cost, for a surface with room for a
    /// sentence: what was billed, and the estimate beside it where there is
    /// one - `not reported (≈ $1.3600 API-equivalent)`.
    pub fn cost_and_estimate_line(&self) -> String {
        match self.estimate_line() {
            Some(estimate) => format!("{} ({estimate})", self.cost_line()),
            None => self.cost_line(),
        }
    }

    /// Price one invocation: the dollars the model or engine reported, or
    /// `None` when it reported none, which makes this usage a floor.
    pub fn set_cost(&mut self, cost: Option<f64>) {
        debug_assert!(cost.is_none_or(|value| value.is_finite() && value >= 0.0));
        debug_assert!(self.estimated_cost_usd.is_none_or(|value| value >= 0.0));
        self.cost_usd = cost.unwrap_or(0.0);
        self.cost_complete = cost.is_some();
        self.cost_gap = cost.is_none().then_some(CostGap::Unpriced);
        self.cost_basis = if cost.is_some() {
            CostBasis::ProviderReported
        } else if self.estimated_cost_usd.is_some() {
            CostBasis::ListPrice
        } else {
            CostBasis::Unknown
        };
    }

    /// Preserve the known floor while recording another missing contribution.
    pub fn mark_cost_gap(&mut self, gap: CostGap) {
        debug_assert!(self.cost_usd.is_finite() && self.cost_usd >= 0.0);
        debug_assert!(self.estimated_cost_usd.is_none_or(|cost| cost >= 0.0));
        self.cost_gap = Some(if self.cost_complete {
            gap
        } else {
            self.cost_gap.unwrap_or(CostGap::Unknown).combine(gap)
        });
        self.cost_complete = false;
    }

    /// Merge coverage without adding the other contribution's counters or cost.
    pub fn merge_cost_coverage(&mut self, other: Self) {
        debug_assert!(self.cost_usd.is_finite() && self.cost_usd >= 0.0);
        debug_assert!(other.cost_usd.is_finite() && other.cost_usd >= 0.0);
        self.cost_gap = if self.cost_complete {
            None
        } else {
            Some(self.cost_gap.unwrap_or(CostGap::Unknown))
        };
        if !other.cost_complete {
            self.mark_cost_gap(other.cost_gap.unwrap_or(CostGap::Unknown));
        }
    }

    /// Price one invocation from the registry: billed dollars where its model
    /// has a price, else the API-equivalent estimate from the price of the API
    /// model behind a subscription one. Both in one step, so an invocation can
    /// never carry a fresh cost beside a stale estimate.
    pub fn price_from(&mut self, billed: Option<ModelCost>, estimate: Option<ModelCost>) {
        debug_assert!(
            billed.is_none() || estimate.is_none(),
            "a priced invocation needs no estimate; both would say its cost twice"
        );
        let cost = billed.map(|rate| rate.cost_of(self));
        let estimated = estimate.map(|rate| rate.cost_of(self));
        debug_assert!(
            estimated.is_none_or(|e| e >= 0.0),
            "a price table with a negative rate would pay the operator"
        );
        self.estimated_cost_usd = estimated;
        self.set_cost(cost);
        if billed.is_some() || estimate.is_some() {
            self.cost_basis = CostBasis::ListPrice;
        }
    }

    /// A cost-only contribution to fold into a running total: what one
    /// invocation cost, or the fact that it never said.
    pub fn cost_only(cost: Option<f64>) -> Self {
        let mut usage = Self::default();
        usage.set_cost(cost);
        usage
    }

    /// Prompt occupancy, including cached input but excluding generated output.
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// The 1-hour writes in `self` beyond `earlier`, never more than the new writes.
    /// Independent high-water marks can grow the split faster than its total.
    pub fn cache_write_1h_since(&self, earlier: &Self) -> u64 {
        debug_assert!(self.cache_write_1h_tokens <= self.cache_write_tokens);
        debug_assert!(earlier.cache_write_1h_tokens <= earlier.cache_write_tokens);
        let writes = self
            .cache_write_tokens
            .saturating_sub(earlier.cache_write_tokens);
        self.cache_write_1h_tokens
            .saturating_sub(earlier.cache_write_1h_tokens)
            .min(writes)
    }
}

impl AddAssign for Usage {
    /// Saturating throughout: these numbers are read out of an import, and a
    /// wrapped total is a wrong total.
    fn add_assign(&mut self, rhs: Self) {
        // Empty/unpriced contributors cannot change the source of priced dollars.
        // A legacy positive amount with unknown provenance must stay unknown.
        let has_figure = |u: &Self| {
            u.cost_usd != 0.0
                || u.estimated_cost_usd.is_some()
                || (u.total_tokens() > 0 && u.cost_basis != CostBasis::Unknown)
        };
        self.cost_basis = match (has_figure(self), has_figure(&rhs)) {
            (false, true) => rhs.cost_basis,
            (true, true) => self.cost_basis.combine(rhs.cost_basis),
            _ => self.cost_basis,
        };
        self.input_tokens = self.input_tokens.saturating_add(rhs.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(rhs.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(rhs.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(rhs.cache_write_tokens);
        self.cache_write_1h_tokens = self
            .cache_write_1h_tokens
            .saturating_add(rhs.cache_write_1h_tokens);
        debug_assert!(
            self.cache_write_1h_tokens <= self.cache_write_tokens,
            "1-hour writes are a subset of cache writes"
        );
        self.reasoning_tokens = match (self.reasoning_tokens, rhs.reasoning_tokens) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        };
        self.cost_usd += rhs.cost_usd;
        self.merge_cost_coverage(rhs);
        // Like `reasoning_tokens`: a total nobody estimated stays unestimated
        // rather than becoming a confident zero.
        self.estimated_cost_usd = match (self.estimated_cost_usd, rhs.estimated_cost_usd) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
    }
}

/// Price per million tokens in USD.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelCost {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_read_per_mtok: f64,
    #[serde(default)]
    pub cache_write_per_mtok: f64,
    /// Rate for 1-hour cache writes; absent prices them at `cache_write_per_mtok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above_threshold: Option<ModelCostTier>,
}

/// Rates for an entire request whose prompt exceeds `prompt_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelCostTier {
    pub prompt_tokens: u64,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_read_per_mtok: f64,
    #[serde(default)]
    pub cache_write_per_mtok: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_per_mtok: Option<f64>,
}

impl ModelCost {
    /// All configured rates, including the optional long-prompt tier.
    pub fn rates(&self) -> impl Iterator<Item = f64> {
        [
            self.input_per_mtok,
            self.output_per_mtok,
            self.cache_read_per_mtok,
            self.cache_write_per_mtok,
        ]
        .into_iter()
        .chain(self.cache_write_1h_per_mtok)
        .chain(self.above_threshold.into_iter().flat_map(|tier| {
            [
                tier.input_per_mtok,
                tier.output_per_mtok,
                tier.cache_read_per_mtok,
                tier.cache_write_per_mtok,
            ]
            .into_iter()
            .chain(tier.cache_write_1h_per_mtok)
        }))
    }

    /// Price one request before its usage is folded into an accumulator.
    pub fn cost_of(&self, usage: &Usage) -> f64 {
        debug_assert!(
            usage.cache_write_1h_tokens <= usage.cache_write_tokens,
            "1-hour writes are a subset of cache writes"
        );
        let prompt_tokens = usage.context_tokens();
        let (input, output, read, write, write_1h) = match self.above_threshold {
            Some(tier) if prompt_tokens > tier.prompt_tokens => (
                tier.input_per_mtok,
                tier.output_per_mtok,
                tier.cache_read_per_mtok,
                tier.cache_write_per_mtok,
                tier.cache_write_1h_per_mtok,
            ),
            _ => (
                self.input_per_mtok,
                self.output_per_mtok,
                self.cache_read_per_mtok,
                self.cache_write_per_mtok,
                self.cache_write_1h_per_mtok,
            ),
        };
        let five_minute = usage
            .cache_write_tokens
            .saturating_sub(usage.cache_write_1h_tokens);
        let per = |tokens: u64, price: f64| tokens as f64 / 1_000_000.0 * price;
        let cost = per(usage.input_tokens, input)
            + per(usage.output_tokens, output)
            + per(usage.cache_read_tokens, read)
            + per(five_minute, write)
            + per(usage.cache_write_1h_tokens, write_1h.unwrap_or(write));
        debug_assert!(!cost.is_nan(), "a finite rate table prices to a number");
        cost
    }
}
