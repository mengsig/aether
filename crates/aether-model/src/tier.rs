//! Capability bands that let callers pick a model without naming one.

use serde::{Deserialize, Serialize};

/// A capability band within one provider, weakest first. The vocabulary is
/// shared with Archon's catalog tiers: keep the names stable. `strongest` is
/// the legacy name of `strong` and is still accepted when reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Fast,
    Balanced,
    #[serde(alias = "strongest")]
    Strong,
    Frontier,
}

impl ModelTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Strong => "strong",
            Self::Frontier => "frontier",
        }
    }
}

/// Rank an unranked model by output price: the tier of the nearest priced,
/// tiered model of the same provider (`references`), nearest in price ratio.
/// A tie takes the lower tier. Gaps alone cannot separate tiers, so only the
/// tiered models' own prices anchor the bands. `None` when the price or every
/// reference is not a positive finite number: an unpriced model stays unranked.
pub fn tier_by_price(output_per_mtok: f64, references: &[(f64, ModelTier)]) -> Option<ModelTier> {
    let usable = |price: f64| price.is_finite() && price > 0.0;
    if !usable(output_per_mtok) {
        return None;
    }
    let distance = |price: f64| (output_per_mtok / price).ln().abs();
    references
        .iter()
        .filter(|(price, _)| usable(*price))
        .min_by(|(pa, ta), (pb, tb)| {
            distance(*pa)
                .total_cmp(&distance(*pb))
                .then_with(|| ta.cmp(tb))
        })
        .map(|&(_, tier)| tier)
}

#[cfg(test)]
mod tests;
