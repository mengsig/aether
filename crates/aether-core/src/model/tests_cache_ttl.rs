//! The 1-hour cache-write split: wire compatibility, combination and pricing.
use super::*;
use serde_json::json;

fn writes(total: u64, one_hour: u64) -> Usage {
    Usage {
        cache_write_tokens: total,
        cache_write_1h_tokens: one_hour,
        ..Default::default()
    }
}

/// Haiku 5.5's list price: $0.10 input, $0.125 5-minute write, $0.20 1-hour write.
fn haiku() -> ModelCost {
    ModelCost {
        input_per_mtok: 0.10,
        output_per_mtok: 0.50,
        cache_read_per_mtok: 0.01,
        cache_write_per_mtok: 0.125,
        cache_write_1h_per_mtok: Some(0.20),
        above_threshold: None,
    }
}

#[test]
fn usage_without_a_split_round_trips_unchanged() {
    let old = json!({"input_tokens": 1, "output_tokens": 2, "cache_read_tokens": 3,
        "cache_write_tokens": 4, "cost_usd": 0.5, "cost_complete": true});
    let usage: Usage = serde_json::from_value(old).unwrap();
    assert_eq!(usage.cache_write_1h_tokens, 0);
    let wire = serde_json::to_value(usage).unwrap();
    assert!(wire.get("cache_write_1h_tokens").is_none(), "{wire}");

    let split = writes(4, 3);
    let wire = serde_json::to_value(split).unwrap();
    assert_eq!(wire["cache_write_1h_tokens"], 3);
    assert_eq!(serde_json::from_value::<Usage>(wire).unwrap(), split);
}

#[test]
fn usage_rejects_a_split_larger_than_its_total() {
    let wire = json!({"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0,
        "cache_write_tokens": 4, "cache_write_1h_tokens": 5});
    let error = serde_json::from_value::<Usage>(wire)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cache_write_1h_tokens (5)"), "{error}");
}

#[test]
fn adding_usage_sums_the_split_and_a_missing_split_adds_nothing() {
    let mut total = writes(100, 60);
    total += writes(50, 0);
    total += writes(10, 10);
    assert_eq!(total.cache_write_tokens, 160);
    assert_eq!(total.cache_write_1h_tokens, 70);
}

#[test]
fn split_growth_is_bounded_by_new_writes() {
    assert_eq!(writes(150, 100).cache_write_1h_since(&writes(100, 40)), 50);
    assert_eq!(writes(150, 100).cache_write_1h_since(&writes(100, 80)), 20);
    // A later report moved existing writes into the 1-hour tier without adding any.
    assert_eq!(writes(100, 100).cache_write_1h_since(&writes(100, 0)), 0);
    assert_eq!(writes(0, 0).cache_write_1h_since(&writes(10, 10)), 0);
}

#[test]
fn one_hour_writes_price_at_their_own_rate() {
    let rate = haiku();
    let cost = rate.cost_of(&writes(1_000_000, 400_000));
    assert!((cost - (0.6 * 0.125 + 0.4 * 0.20)).abs() < 1e-12, "{cost}");
    let unsplit = rate.cost_of(&writes(1_000_000, 0));
    assert!((unsplit - 0.125).abs() < 1e-12, "{unsplit}");
}

#[test]
fn a_price_without_a_one_hour_rate_prices_every_write_as_today() {
    let rate = ModelCost {
        cache_write_1h_per_mtok: None,
        ..haiku()
    };
    let cost = rate.cost_of(&writes(1_000_000, 1_000_000));
    assert!((cost - 0.125).abs() < 1e-12, "{cost}");
    let wire = serde_json::to_value(rate).unwrap();
    assert!(wire.get("cache_write_1h_per_mtok").is_none(), "{wire}");
}

#[test]
fn a_long_prompt_tier_prices_one_hour_writes_at_its_own_rate() {
    let rate = ModelCost {
        above_threshold: Some(ModelCostTier {
            prompt_tokens: 100_000,
            input_per_mtok: 0.50,
            output_per_mtok: 2.50,
            cache_read_per_mtok: 0.05,
            cache_write_per_mtok: 0.625,
            cache_write_1h_per_mtok: Some(1.0),
        }),
        ..haiku()
    };
    let cost = rate.cost_of(&writes(200_000, 200_000));
    assert!((cost - 0.2).abs() < 1e-12, "{cost}");
    assert_eq!(
        rate.rates().collect::<Vec<_>>(),
        vec![0.10, 0.50, 0.01, 0.125, 0.20, 0.50, 2.50, 0.05, 0.625, 1.0]
    );
}

/// Archon's 16 Haiku trials: 1,265,084 cache-write tokens, all on the 1-hour tier.
#[test]
fn archon_aggregate_writes_price_at_twice_input() {
    let usage = writes(1_265_084, 1_265_084);
    let before = ModelCost {
        cache_write_1h_per_mtok: None,
        ..haiku()
    }
    .cost_of(&usage);
    let after = haiku().cost_of(&usage);
    assert!((before - 0.1581355).abs() < 1e-9, "{before}");
    assert!((after - 0.2530168).abs() < 1e-9, "{after}");
    assert!((after - before - 0.0948813).abs() < 1e-9);
}
