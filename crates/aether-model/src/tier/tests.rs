use super::*;

const REFS: [(f64, ModelTier); 4] = [
    (0.5, ModelTier::Fast),
    (10.0, ModelTier::Balanced),
    (20.0, ModelTier::Strong),
    (50.0, ModelTier::Frontier),
];

#[test]
fn an_unranked_price_takes_the_nearest_reference_tier() {
    assert_eq!(tier_by_price(0.1, &REFS), Some(ModelTier::Fast));
    assert_eq!(tier_by_price(8.0, &REFS), Some(ModelTier::Balanced));
    assert_eq!(tier_by_price(18.0, &REFS), Some(ModelTier::Strong));
    assert_eq!(tier_by_price(45.0, &REFS), Some(ModelTier::Frontier));
}

#[test]
fn a_tie_takes_the_lower_tier() {
    // 5 is a factor of two from 10 and from 2.5.
    let refs = [(10.0, ModelTier::Strong), (2.5, ModelTier::Balanced)];
    assert_eq!(tier_by_price(5.0, &refs), Some(ModelTier::Balanced));
}

#[test]
fn unusable_prices_stay_unranked() {
    assert_eq!(tier_by_price(0.0, &REFS), None);
    assert_eq!(tier_by_price(f64::NAN, &REFS), None);
    assert_eq!(tier_by_price(5.0, &[]), None);
    assert_eq!(tier_by_price(5.0, &[(0.0, ModelTier::Fast)]), None);
    assert_eq!(
        tier_by_price(5.0, &[(f64::INFINITY, ModelTier::Fast)]),
        None
    );
}

#[test]
fn tiers_serialize_by_name_and_accept_the_legacy_strongest() {
    assert_eq!(
        serde_json::to_string(&ModelTier::Strong).unwrap(),
        "\"strong\""
    );
    assert_eq!(
        serde_json::to_string(&ModelTier::Frontier).unwrap(),
        "\"frontier\""
    );
    let legacy: ModelTier = serde_json::from_str("\"strongest\"").unwrap();
    assert_eq!(legacy, ModelTier::Strong);
    assert!(serde_json::from_str::<ModelTier>("\"cheap\"").is_err());
    assert!(ModelTier::Fast < ModelTier::Balanced && ModelTier::Strong < ModelTier::Frontier);
}
