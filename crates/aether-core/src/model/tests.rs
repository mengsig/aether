use std::time::Duration;

use super::*;
use serde_json::json;

#[test]
fn cost_basis_tracks_pricing_and_survives_serialization_and_rollups() {
    let rate = ModelCost {
        input_per_mtok: 1.0,
        ..Default::default()
    };
    let mut listed = Usage {
        input_tokens: 1_000_000,
        ..Default::default()
    };
    listed.price_from(Some(rate), None);
    assert_eq!(listed.cost_basis, CostBasis::ListPrice);
    assert_eq!(listed.cost_usd, 1.0);
    assert!(listed.cost_complete);
    let mut provider = Usage::cost_only(Some(2.0));
    assert_eq!(provider.cost_basis, CostBasis::ProviderReported);
    let mut total = Usage::default();
    total += listed;
    assert_eq!(total.cost_basis, CostBasis::ListPrice);
    total += provider;
    assert_eq!(total.cost_basis, CostBasis::Mixed);
    assert_eq!(total.cost_usd, 3.0);
    let wire = serde_json::to_value(total).unwrap();
    assert_eq!(wire["cost_basis"], "mixed");
    assert_eq!(serde_json::from_value::<Usage>(wire).unwrap(), total);
    provider.set_cost(None);
    assert_eq!(provider.cost_basis, CostBasis::Unknown);
    listed.price_from(None, Some(rate));
    assert_eq!(listed.cost_basis, CostBasis::ListPrice);
    assert_eq!(listed.cost_usd, 0.0);
    assert_eq!(listed.estimated_cost_usd, Some(1.0));
    assert!(!listed.cost_complete);
}

#[test]
fn legacy_cost_basis_is_unknown_and_not_guessed_from_a_dollar_amount() {
    let wire = json!({"input_tokens": 1, "output_tokens": 0,
        "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_usd": 1.0});
    let mut usage: Usage = serde_json::from_value(wire).unwrap();
    assert_eq!(usage.cost_basis, CostBasis::Unknown);
    usage += Usage::cost_only(Some(2.0));
    assert_eq!(usage.cost_basis, CostBasis::Unknown);
    assert_eq!(usage.cost_usd, 3.0);
}

#[test]
fn a_zero_cost_receipt_without_tokens_does_not_change_an_empty_total() {
    let mut total = Usage::default();
    total += Usage::cost_only(Some(0.0));
    assert_eq!(total, Usage::default());
}

#[test]
fn exhaustion_requires_explicit_error_evidence() {
    for message in [
        "insufficient_quota",
        "Credit balance is too low",
        "organization_usage_limit_exceeded",
        "You have reached your specified API usage limits. You will regain access on 2026-10-01",
    ] {
        assert_eq!(
            provider_exhaustion(message),
            Some(ModelErrorClass::QuotaExhausted)
        );
    }
    for message in [
        "You've hit your limit · resets Sep 18 at 2pm",
        "You've hit your usage limit.",
        "Weekly limit reached",
        "Daily limit exceeded",
        "5-hour limit reached ∙ resets 2pm",
        "Session limit reached · resets 3pm",
    ] {
        assert_eq!(
            provider_exhaustion(message),
            Some(ModelErrorClass::UsageLimit)
        );
    }
    for message in [
        "quota",
        "429 rate limit",
        "weekly utilization 0.9",
        "authentication failed",
        "timeout",
        "daily limit may apply",
        "session expired",
        "rate limit reached for requests per minute",
        "unknown error",
    ] {
        assert_eq!(provider_exhaustion(message), None, "{message}");
    }
    for class in [ModelErrorClass::QuotaExhausted, ModelErrorClass::UsageLimit] {
        assert!(!ModelError::new(class, "exhausted").is_retryable());
    }
    assert!(ModelError::new(ModelErrorClass::RateLimit, "slow down").is_retryable());
}

#[test]
fn every_error_class_prints_its_wire_name() {
    for class in [
        ModelErrorClass::Auth,
        ModelErrorClass::Permission,
        ModelErrorClass::RateLimit,
        ModelErrorClass::QuotaExhausted,
        ModelErrorClass::UsageLimit,
        ModelErrorClass::Overloaded,
        ModelErrorClass::ContextLimit,
        ModelErrorClass::InvalidRequest,
        ModelErrorClass::NotFound,
        ModelErrorClass::Network,
        ModelErrorClass::Timeout,
        ModelErrorClass::Cancelled,
        ModelErrorClass::Server,
        ModelErrorClass::Unknown,
    ] {
        assert_eq!(
            serde_json::to_value(class).expect("ser"),
            json!(class.as_str()),
            "a projection must print the class the log carries"
        );
    }
}

#[test]
fn a_chain_round_trips_and_rejects_a_broken_link() {
    let chain =
        ModelChain::parse("anthropic/opus, openai/gpt-5 ,claude-code/haiku").expect("three links");
    assert_eq!(chain.primary, ModelRef::new("anthropic", "opus"));
    assert_eq!(chain.link_count(), 3);
    assert!(chain.has_fallbacks());
    assert_eq!(
        chain.to_string(),
        "anthropic/opus,openai/gpt-5,claude-code/haiku"
    );
    assert_eq!(ModelChain::parse(&chain.to_string()), Some(chain));

    let one = ModelChain::parse("openrouter/org/model").expect("one link");
    assert!(!one.has_fallbacks());
    assert_eq!(one.primary.model, "org/model");

    // A typo must not silently shorten the chain.
    assert_eq!(ModelChain::parse("anthropic/opus,,openai/gpt-5"), None);
    assert_eq!(ModelChain::parse("anthropic/opus,nope"), None);
    assert_eq!(ModelChain::parse(""), None);
}

#[test]
fn accumulator_assembles_text_and_tool_call() {
    let events = vec![
        StreamEvent::MessageStart {
            provider_message_id: Some("m1".into()),
            model: Some("x".into()),
        },
        StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Text {
                text: String::new(),
            },
        },
        StreamEvent::TextDelta {
            index: 0,
            text: "Hel".into(),
        },
        StreamEvent::TextDelta {
            index: 0,
            text: "lo".into(),
        },
        StreamEvent::BlockStop { index: 0 },
        StreamEvent::BlockStart {
            index: 1,
            block: BlockStart::ToolUse {
                id: "t1".into(),
                name: "echo".into(),
                input: None,
            },
        },
        StreamEvent::ToolInputDelta {
            index: 1,
            partial_json: "{\"msg\":".into(),
        },
        StreamEvent::ToolInputDelta {
            index: 1,
            partial_json: "\"hi\"}".into(),
        },
        StreamEvent::BlockStop { index: 1 },
        StreamEvent::Usage {
            usage: Usage {
                input_tokens: 3,
                output_tokens: 5,
                ..Default::default()
            },
        },
        StreamEvent::Stop {
            reason: StopReason::ToolUse,
        },
        StreamEvent::MessageStop,
    ];
    let mut acc = StreamAccumulator::new();
    for e in &events {
        acc.apply(e).expect("apply");
    }
    let resp = acc.finish().expect("finish");
    assert_eq!(resp.text(), "Hello");
    assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(resp.usage.output_tokens, 5);
    let calls: Vec<_> = resp.tool_uses().collect();
    assert_eq!(calls, vec![("t1", "echo", &json!({"msg": "hi"}))]);
}

#[test]
fn empty_tool_input_becomes_empty_object() {
    let mut acc = StreamAccumulator::new();
    acc.apply(&StreamEvent::BlockStart {
        index: 0,
        block: BlockStart::ToolUse {
            id: "t".into(),
            name: "noop".into(),
            input: None,
        },
    })
    .expect("start");
    let resp = acc.finish().expect("finish");
    assert_eq!(resp.tool_uses().next().map(|c| c.2), Some(&json!({})));
}

#[test]
fn invalid_tool_json_is_an_error() {
    let mut acc = StreamAccumulator::new();
    acc.apply(&StreamEvent::BlockStart {
        index: 0,
        block: BlockStart::ToolUse {
            id: "t".into(),
            name: "bad".into(),
            input: None,
        },
    })
    .expect("start");
    acc.apply(&StreamEvent::ToolInputDelta {
        index: 0,
        partial_json: "{oops".into(),
    })
    .expect("delta");
    assert!(matches!(
        acc.finish(),
        Err(AccumulateError::InvalidToolInput { .. })
    ));
}

#[test]
fn delta_for_unknown_block_is_rejected() {
    let mut acc = StreamAccumulator::new();
    let err = acc
        .apply(&StreamEvent::TextDelta {
            index: 4,
            text: "x".into(),
        })
        .unwrap_err();
    assert_eq!(err, AccumulateError::UnknownBlock(4));
}

#[test]
fn usage_adds() {
    let mut a = Usage {
        input_tokens: 1,
        output_tokens: 2,
        cost_usd: 0.25,
        ..Default::default()
    };
    a += Usage {
        input_tokens: 10,
        reasoning_tokens: Some(3),
        cost_usd: 0.5,
        ..Default::default()
    };
    assert_eq!(a.input_tokens, 11);
    assert_eq!(a.reasoning_tokens, Some(3));
    assert_eq!(a.cost_usd, 0.75);
    assert!(a.cost_complete);
}

#[test]
fn an_empty_total_takes_the_first_cost_it_is_given() {
    let mut total = Usage::default();
    total += priced(0.5);
    assert_eq!(total.cost_usd, 0.5, "nothing yet is not a contributor");
    assert!(total.cost_complete);
    total += Usage::default();
    assert_eq!(total.cost_usd, 0.5, "and adding nothing changes nothing");
    assert!(total.cost_complete);
}

#[test]
fn one_unpriced_invocation_leaves_the_priced_part_standing() {
    // The old dishonesty was summing the priced half and showing it as
    // the whole. The old over-correction was calling the whole unknown,
    // which read as zero wherever a number was needed. Both are wrong:
    // what is known is a floor, and it says so.
    let mut total = Usage::default();
    total += priced(1.0);
    total += unpriced();
    assert_eq!(total.cost_usd, 1.0, "the priced part is still known");
    assert!(!total.cost_complete, "and it is only a floor");
    assert_eq!(total.total_tokens(), 200, "the tokens are still known");
    assert_eq!(total.cost_line(), "≥ $1.0000 (some unpriced)");

    // And in the other order, so it is not an artefact of who arrives first.
    let mut other = Usage::default();
    other += unpriced();
    other += priced(1.0);
    assert_eq!(other.cost_usd, 1.0);
    assert!(!other.cost_complete);
}

#[test]
fn a_total_of_only_unpriced_work_is_unknown_not_free() {
    let mut total = Usage::default();
    total += unpriced();
    total += unpriced();
    assert_eq!(total.cost_usd, 0.0);
    assert!(!total.cost_complete);
    assert_eq!(total.cost_line(), Usage::COST_UNKNOWN);
}

/// A rate or a forecast stands behind the same cost, so it is said the
/// same way - never a confident `$0.00/h` for a run nobody priced.
#[test]
fn a_derived_number_is_said_the_way_its_cost_is() {
    let hour = |v: f64| format!("${v:.2}/h");
    let mut complete = priced(2.0);
    assert_eq!(complete.said(4.0, hour), "$4.00/h");
    complete.cost_complete = false;
    assert_eq!(complete.said(4.0, hour), "≥ $4.00/h (some cost unknown)");
    assert_eq!(unpriced().said(0.0, hour), Usage::COST_UNKNOWN);
}

#[test]
fn a_complete_zero_is_a_number() {
    assert_eq!(Usage::default().cost_line(), "$0.0000");
    assert_eq!(priced(0.0).cost_line(), "$0.0000");
}

/// A store written before `cost_complete` existed said "nobody priced
/// this" by leaving `cost_usd` out. Reading that back as a confident zero
/// is the bug this whole shape exists to prevent.
#[test]
fn a_record_written_before_the_flag_keeps_its_meaning() {
    let unpriced: Usage = serde_json::from_str(
        r#"{"input_tokens":1,"output_tokens":2,"cache_read_tokens":0,"cache_write_tokens":0}"#,
    )
    .expect("legacy usage");
    assert!(!unpriced.cost_complete);
    assert_eq!(unpriced.cost_line(), Usage::COST_UNKNOWN);

    let priced: Usage = serde_json::from_str(
        r#"{"input_tokens":1,"output_tokens":2,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.5}"#,
    )
    .expect("legacy usage");
    assert!(priced.cost_complete);
    assert_eq!(priced.cost_usd, 0.5);
}

#[test]
fn a_token_total_saturates_rather_than_wrapping() {
    // The numbers come out of an import, which chooses them.
    let mut total = Usage {
        input_tokens: u64::MAX,
        ..Default::default()
    };
    total += Usage {
        input_tokens: 10,
        ..Default::default()
    };
    assert_eq!(total.input_tokens, u64::MAX);
    assert_eq!(total.total_tokens(), u64::MAX);
}

/// $10/Mtok in, $50/Mtok out, a tenth of input for a cache read: the
/// shape of a real list price, so the arithmetic below is checkable.
fn list_price() -> ModelCost {
    ModelCost {
        input_per_mtok: 10.0,
        output_per_mtok: 50.0,
        cache_read_per_mtok: 1.0,
        cache_write_per_mtok: 10.0,
        above_threshold: None,
    }
}

/// The pilot's shape: mostly cache reads, a little fresh input, some output.
fn subscription_turn() -> Usage {
    Usage {
        input_tokens: 40_000,
        output_tokens: 61_000,
        cache_read_tokens: 3_210_000,
        cost_complete: false,
        ..Default::default()
    }
}

#[test]
fn an_engine_that_priced_the_turn_is_never_estimated_over() {
    let mut usage = subscription_turn();
    usage.price_from(Some(list_price()), None);
    assert!(usage.cost_complete, "the registry priced it");
    assert!(usage.cost_usd > 0.0);
    assert_eq!(
        usage.estimated_cost_usd, None,
        "a priced invocation needs no estimate"
    );
    assert_eq!(usage.api_equivalent_usd(), None);
    assert_eq!(usage.estimate_line(), None);
    assert_eq!(usage.cost_and_estimate_line(), usage.cost_line());
}

#[test]
fn an_unpriced_turn_with_a_known_api_twin_is_estimated_beside_its_cost() {
    let mut usage = subscription_turn();
    usage.price_from(None, Some(list_price()));
    assert_eq!(usage.cost_usd, 0.0, "nobody billed these dollars");
    assert!(!usage.cost_complete, "and the total says so");
    // 0.04 Mtok in at $10 + 0.061 Mtok out at $50 + 3.21 Mtok cached at $1.
    let expected = 0.4 + 3.05 + 3.21;
    let estimated = usage.estimated_cost_usd.expect("estimate");
    assert!(
        (estimated - expected).abs() < 1e-9,
        "{estimated} vs {expected}"
    );
    assert_eq!(usage.api_equivalent_usd(), Some(estimated));
    assert_eq!(
        usage.cost_and_estimate_line(),
        "not reported (≈ $6.6600 API-equivalent)",
        "an estimate never renders as a bare number"
    );
}

#[test]
fn an_unpriced_turn_with_no_known_twin_gets_no_estimate() {
    let mut usage = subscription_turn();
    usage.price_from(None, None);
    assert_eq!(usage.estimated_cost_usd, None);
    assert_eq!(usage.api_equivalent_usd(), None, "a floor, like the cost");
    assert_eq!(usage.cost_and_estimate_line(), Usage::COST_UNKNOWN);
}

/// An estimate folds like a cost but stays out of it: `cost_usd` is the
/// only figure a budget reads, and it must not grow by an estimate.
#[test]
fn estimates_fold_without_touching_the_billed_total() {
    let mut estimated = subscription_turn();
    estimated.price_from(None, Some(list_price()));
    let mut total = priced(1.0);
    total += estimated;
    total += estimated;
    assert_eq!(
        total.cost_usd, 1.0,
        "still only the dollars somebody billed"
    );
    assert!(!total.cost_complete);
    let each = estimated.estimated_cost_usd.expect("estimate");
    assert_eq!(total.estimated_cost_usd, Some(each * 2.0));
    assert_eq!(total.api_equivalent_usd(), Some(1.0 + each * 2.0));

    // A total nobody estimated stays unestimated rather than a zero.
    let mut none = priced(1.0);
    none += priced(2.0);
    assert_eq!(none.estimated_cost_usd, None);
    assert_eq!(none.api_equivalent_usd(), None);
}

#[test]
fn a_record_written_before_estimates_has_none_and_adds_no_field() {
    let old: Usage = serde_json::from_str(
        r#"{"input_tokens":1,"output_tokens":2,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.5}"#,
    )
    .expect("usage without an estimate");
    assert_eq!(old.estimated_cost_usd, None);
    assert_eq!(
        serde_json::to_value(old).expect("ser")["estimated_cost_usd"],
        serde_json::Value::Null,
        "nothing estimated writes no field"
    );

    let mut estimated = subscription_turn();
    estimated.price_from(None, Some(list_price()));
    let back: Usage =
        serde_json::from_value(serde_json::to_value(estimated).expect("ser")).expect("de");
    assert_eq!(back.estimated_cost_usd, estimated.estimated_cost_usd);
    assert!(!back.cost_complete, "and the round trip keeps the floor");
}

fn priced(cost: f64) -> Usage {
    Usage {
        input_tokens: 100,
        cost_usd: cost,
        cost_complete: true,
        ..Default::default()
    }
}

fn unpriced() -> Usage {
    Usage {
        input_tokens: 100,
        cost_complete: false,
        cost_gap: Some(CostGap::Unpriced),
        ..Default::default()
    }
}

#[test]
fn model_ref_parses() {
    assert_eq!(
        ModelRef::parse("anthropic/claude-opus-5"),
        Some(ModelRef::new("anthropic", "claude-opus-5"))
    );
    assert_eq!(
        ModelRef::parse("openrouter/org/model"),
        Some(ModelRef::new("openrouter", "org/model"))
    );
    assert_eq!(ModelRef::parse("nope"), None);
}

#[test]
fn model_error_serde_roundtrip() {
    let err = ModelError::new(ModelErrorClass::RateLimit, "slow down")
        .with_status(429)
        .with_retry_after(Duration::from_millis(1500));
    let json = serde_json::to_value(&err).expect("ser");
    assert_eq!(json["retry_after"], 1500);
    let back: ModelError = serde_json::from_value(json).expect("de");
    assert_eq!(back, err);
    assert!(back.is_retryable());
}

fn tiered_price() -> ModelCost {
    ModelCost {
        input_per_mtok: 0.10,
        output_per_mtok: 0.50,
        cache_read_per_mtok: 0.01,
        cache_write_per_mtok: 0.125,
        above_threshold: Some(ModelCostTier {
            prompt_tokens: 100_000,
            input_per_mtok: 0.50,
            output_per_mtok: 2.50,
            cache_read_per_mtok: 0.05,
            cache_write_per_mtok: 0.625,
        }),
    }
}

#[test]
fn tiered_pricing_is_per_request_and_prices_billed_and_estimated_usage() {
    let rate = tiered_price();
    let mut total = Usage::default();
    for (tokens, expected) in [(99_999, 0.0099999), (100_000, 0.01), (100_001, 0.0500005)] {
        let mut usage = Usage {
            input_tokens: tokens,
            ..Default::default()
        };
        usage.price_from(Some(rate), None);
        assert!((usage.cost_usd - expected).abs() < 1e-12);
        assert!(usage.cost_complete);
        assert_eq!(usage.cost_basis, CostBasis::ListPrice);
        total += usage;
        usage.price_from(None, Some(rate));
        assert_eq!(usage.cost_usd, 0.0);
        assert!(!usage.cost_complete);
        assert!((usage.estimated_cost_usd.unwrap() - expected).abs() < 1e-12);
    }
    assert_eq!(total.input_tokens, 300_000);
    assert!((total.cost_usd - 0.0700004).abs() < 1e-12);
}

#[test]
fn tier_selection_counts_cache_tokens_and_saturates_prompt_totals() {
    let mut rate = tiered_price();
    rate.above_threshold.as_mut().unwrap().prompt_tokens = u64::MAX - 1;
    let usage = Usage {
        input_tokens: u64::MAX - 1,
        cache_read_tokens: 1,
        cache_write_tokens: 1,
        output_tokens: 100,
        ..Default::default()
    };
    let expected = (u64::MAX - 1) as f64 / 1_000_000.0 * 0.50
        + 0.05 / 1_000_000.0
        + 0.625 / 1_000_000.0
        + 100.0 / 1_000_000.0 * 2.50;
    assert_eq!(rate.cost_of(&usage), expected);
}

#[test]
fn legacy_flat_prices_and_optional_tiers_round_trip() {
    let wire = json!({"input_per_mtok": 0.10, "output_per_mtok": 0.50});
    let flat: ModelCost = serde_json::from_value(wire).unwrap();
    assert_eq!(flat.above_threshold, None);
    let usage = Usage {
        input_tokens: 200_000,
        output_tokens: 10_000,
        ..Default::default()
    };
    assert!((flat.cost_of(&usage) - 0.025).abs() < 1e-12);
    assert!(
        serde_json::to_value(flat)
            .unwrap()
            .get("above_threshold")
            .is_none()
    );
    let tiered = tiered_price();
    assert_eq!(
        serde_json::from_value::<ModelCost>(serde_json::to_value(tiered).unwrap()).unwrap(),
        tiered
    );
}

#[test]
fn all_configured_rates_include_the_long_prompt_tier() {
    let rate = tiered_price();
    assert_eq!(
        rate.rates().collect::<Vec<_>>(),
        vec![0.10, 0.50, 0.01, 0.125, 0.50, 2.50, 0.05, 0.625]
    );
    assert_eq!(
        ModelCost::default().rates().collect::<Vec<_>>(),
        vec![0.0; 4]
    );
}

#[test]
fn long_prompt_tiers_reject_unknown_rate_keys() {
    let mut wire = serde_json::to_value(tiered_price()).unwrap();
    let tier = wire["above_threshold"].as_object_mut().unwrap();
    let rate = tier.remove("cache_read_per_mtok").unwrap();
    tier.insert("cache_raed_per_mtok".into(), rate);
    let error = serde_json::from_value::<ModelCost>(wire).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown field `cache_raed_per_mtok`")
    );
}

#[test]
fn context_tokens_include_caches_exclude_output_and_saturate() {
    let usage = Usage {
        input_tokens: 10,
        cache_read_tokens: 20,
        cache_write_tokens: 30,
        output_tokens: u64::MAX,
        ..Default::default()
    };
    assert_eq!(usage.context_tokens(), 60);
    for usage in [
        Usage {
            input_tokens: u64::MAX,
            cache_read_tokens: 1,
            ..Default::default()
        },
        Usage {
            cache_read_tokens: u64::MAX,
            cache_write_tokens: 1,
            ..Default::default()
        },
    ] {
        assert_eq!(usage.context_tokens(), u64::MAX);
    }
}

#[test]
fn cost_gaps_combine_all_distinct_reasons_as_mixed() {
    use CostGap::{Interrupted, Mixed, Unknown, Unpriced, Unreported};
    for left in [Unknown, Unpriced, Interrupted, Unreported, Mixed] {
        for right in [Unknown, Unpriced, Interrupted, Unreported, Mixed] {
            assert_eq!(
                left.combine(right),
                if left == right { left } else { Mixed }
            );
            assert_eq!(left.combine(right), right.combine(left));
        }
    }
}

#[test]
fn legacy_cost_gaps_read_as_unknown_and_complete_records_drop_the_gap() {
    let base = serde_json::json!({"input_tokens": 1, "output_tokens": 2,
        "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_usd": 0.5,
        "cost_complete": false});
    let old: Usage = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(old.cost_gap, Some(CostGap::Unknown));
    assert_eq!(old.cost_usd, 0.5);
    for gap in [
        CostGap::Unknown,
        CostGap::Unpriced,
        CostGap::Unreported,
        CostGap::Interrupted,
        CostGap::Mixed,
    ] {
        let mut wire = base.clone();
        wire["cost_gap"] = serde_json::to_value(gap).unwrap();
        let usage: Usage = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(usage.cost_gap, Some(gap));
        assert_eq!(
            serde_json::from_value::<Usage>(serde_json::to_value(usage).unwrap()).unwrap(),
            usage
        );
        wire["cost_complete"] = true.into();
        let complete: Usage = serde_json::from_value(wire).unwrap();
        assert_eq!(complete.cost_gap, None);
        assert!(
            serde_json::to_value(complete)
                .unwrap()
                .get("cost_gap")
                .is_none()
        );
    }
}

#[test]
fn cost_gap_rollups_preserve_all_counters_and_known_dollars() {
    let mut total = priced(2.0);
    total.input_tokens = 10;
    let mut gap = Usage {
        output_tokens: 5,
        ..Default::default()
    };
    gap.mark_cost_gap(CostGap::Interrupted);
    total += gap;
    assert_eq!(total.cost_gap, Some(CostGap::Interrupted));
    assert_eq!(total.cost_usd, 2.0);
    assert_eq!(total.total_tokens(), 15);
    assert_eq!(
        total.cost_line(),
        "≥ $2.0000 (interrupted run, cost unknown)"
    );
    total += unpriced();
    assert_eq!(total.cost_gap, Some(CostGap::Mixed));
    assert!(total.cost_usd >= 2.0);
    assert!(total.total_tokens() >= 15);
    total += Usage {
        cost_complete: false,
        ..Default::default()
    };
    assert_eq!(total.cost_gap, Some(CostGap::Mixed));
    assert_eq!(total.cost_usd, 2.0);
}

#[test]
fn future_cost_gap_values_read_as_unknown() {
    let wire = serde_json::json!({"input_tokens": 1, "output_tokens": 2,
        "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_usd": 0.5,
        "cost_complete": false, "cost_gap": "future_reason"});
    let usage: Usage = serde_json::from_value(wire).unwrap();
    assert_eq!(usage.cost_gap, Some(CostGap::Unknown));
    assert_eq!(usage.cost_usd, 0.5);
}
