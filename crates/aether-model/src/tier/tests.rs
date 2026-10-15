use super::*;

#[test]
fn price_share_ranks_into_bands() {
    assert_eq!(tier_by_price(50.0, 50.0), Some(ModelTier::Strongest));
    assert_eq!(tier_by_price(20.0, 50.0), Some(ModelTier::Strongest));
    assert_eq!(tier_by_price(10.0, 50.0), Some(ModelTier::Balanced));
    assert_eq!(tier_by_price(0.5, 50.0), Some(ModelTier::Fast));
}

#[test]
fn unusable_prices_stay_unranked() {
    assert_eq!(tier_by_price(0.0, 50.0), None);
    assert_eq!(tier_by_price(5.0, 0.0), None);
    assert_eq!(tier_by_price(f64::NAN, 50.0), None);
    assert_eq!(tier_by_price(5.0, f64::INFINITY), None);
}

#[test]
fn tiers_serialize_as_lowercase_names() {
    let json = serde_json::to_string(&ModelTier::Strongest).unwrap();
    assert_eq!(json, "\"strongest\"");
    assert_eq!(ModelTier::Balanced.as_str(), "balanced");
}
