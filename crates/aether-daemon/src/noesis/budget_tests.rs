//! How past use is split by what paid for it.

use aether_core::model::{Billing, Usage};

use super::by_billing;

fn used(tokens: u64, cost: f64) -> Usage {
    Usage {
        input_tokens: tokens,
        cost_usd: cost,
        ..Usage::default()
    }
}

#[test]
fn use_whose_record_kept_no_billing_is_unknown_never_an_api_charge() {
    let (on_engine, on_api, unclassed) = (used(10, 0.01), used(20, 0.02), used(40, 0.04));
    let split = by_billing([
        (Some(Billing::Subscription), &on_engine),
        (Some(Billing::Api), &on_api),
        (None, &unclassed),
        (Some(Billing::Api), &on_api),
    ]);

    assert_eq!(split.subscription, on_engine);
    assert_eq!(split.api.input_tokens, 40, "both API records count");
    assert_eq!(
        split.unknown, unclassed,
        "a record without its class is not billed to the API"
    );
}
