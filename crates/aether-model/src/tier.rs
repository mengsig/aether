//! Capability bands that let callers pick a model without naming one.

pub use aether_core::delegation::ModelTier;

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
