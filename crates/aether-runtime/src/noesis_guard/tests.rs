use super::admission::usage_spent;
use super::*;

#[derive(Debug)]
struct Unlimited;

struct Free;

impl NoesisCharge for Free {
    fn is_current(&self) -> bool {
        true
    }

    fn record(&self, _spent: &Usage) -> Option<String> {
        None
    }

    fn refuses_model(&self, _model: &ModelRef) -> Option<String> {
        None
    }

    fn settle(&self, _usage: &Usage) {}
}

impl NoesisBudget for Unlimited {
    fn admit(
        &self,
        _agent: &Agent,
        _lane: NoesisLane,
    ) -> std::result::Result<Box<dyn NoesisCharge>, NoesisRefusal> {
        Ok(Box::new(Free))
    }
}

#[test]
fn only_the_extractor_and_auditor_run_in_the_background() {
    assert_eq!(lane_of("noesis.extractor"), NoesisLane::Background);
    assert_eq!(lane_of("noesis.auditor"), NoesisLane::Background);
    assert_eq!(lane_of("noesis.tutor"), NoesisLane::Interactive);
    assert_eq!(lane_of("worker"), NoesisLane::Interactive);
    assert_eq!(lane_of("noesis.extractor.v2"), NoesisLane::Interactive);
}

#[test]
fn a_gate_starts_disabled_and_its_clones_share_one_switch() {
    let gate = NoesisGate::new();
    let daemon_side = gate.clone();
    assert!(!gate.is_enabled());
    assert!(gate.budget().is_none());
    daemon_side.enable(Arc::new(Unlimited));
    assert!(gate.is_enabled());
    assert!(gate.budget().is_some());
    daemon_side.drain();
    assert!(!gate.is_enabled());
    assert!(gate.budget().is_none(), "a draining gate admits nothing");
    daemon_side.disable();
    daemon_side.drain();
    assert!(
        matches!(*gate.mode.read(), Mode::Disabled),
        "draining a disabled gate leaves it disabled"
    );
}

#[test]
fn a_turns_spend_is_the_difference_and_never_negative() {
    let before = Usage {
        input_tokens: 100,
        output_tokens: 10,
        cost_usd: 0.5,
        ..Usage::default()
    };
    let after = Usage {
        input_tokens: 160,
        output_tokens: 25,
        cost_usd: 0.75,
        ..Usage::default()
    };
    let spent = usage_spent(&before, &after);
    assert_eq!(spent.input_tokens, 60);
    assert_eq!(spent.output_tokens, 15);
    assert!((spent.cost_usd - 0.25).abs() < 1e-9);
    let backwards = usage_spent(&after, &before);
    assert_eq!(backwards.input_tokens, 0);
    assert_eq!(backwards.cost_usd, 0.0);
}
