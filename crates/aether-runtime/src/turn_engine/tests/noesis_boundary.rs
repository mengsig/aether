//! A native engine's model boundaries apply the Noesis gate and budgets.
use super::*;
use crate::noesis_guard::{NoesisBudget, NoesisCharge, NoesisLane, NoesisRefusal, NoesisTurn};
use std::sync::atomic::{AtomicU64, Ordering};

/// A charge whose budget is spent once the turn reports `limit` micro-USD.
struct Metered {
    limit_micros: u64,
    reported_micros: AtomicU64,
}

impl NoesisCharge for Metered {
    fn is_current(&self) -> bool {
        true
    }

    fn before_invocation(&self, spent: &aether_core::model::Usage) -> Option<String> {
        let micros = (spent.cost_usd * 1e6).round() as u64;
        self.reported_micros.store(micros, Ordering::SeqCst);
        (micros >= self.limit_micros).then(|| "the Noesis budget is spent".to_owned())
    }

    fn refuses_model(&self, _model: &ModelRef) -> Option<String> {
        None
    }

    fn settle(&self, _usage: &aether_core::model::Usage) {}
}

/// Keeps the gate Enabled; these tests never admit through it.
#[derive(Debug)]
struct Never;

impl NoesisBudget for Never {
    fn admit(
        &self,
        _agent: &aether_core::agent::Agent,
        _lane: NoesisLane,
    ) -> std::result::Result<Box<dyn NoesisCharge>, NoesisRefusal> {
        Err(NoesisRefusal::Busy("not used here".into()))
    }
}

/// A running Noesis turn of `lane` whose charge allows `limit_micros`.
async fn noesis_turn(
    runtime: &Arc<Runtime>,
    lane: NoesisLane,
    limit_micros: u64,
) -> (TurnCtx, Arc<Metered>, CancellationToken) {
    let mut ctx = started_agent_ctx(runtime).await;
    let charge = Arc::new(Metered {
        limit_micros,
        reported_micros: AtomicU64::new(0),
    });
    let turn = NoesisTurn {
        lane,
        generation: 1,
        charge: charge.clone(),
    };
    ctx.noesis = Some(turn.clone());
    let interrupt = CancellationToken::new();
    runtime.sched.lock().running.insert(
        ctx.agent.id,
        crate::RunningTurn {
            task: None,
            cancel: CancellationToken::new(),
            cancel_request: None,
            stop_reason: None,
            interrupt: interrupt.clone(),
            addressed: false,
            provider: "claude-code".into(),
            engine: true,
            on_provider: true,
            noesis: Some(turn),
        },
    );
    (ctx, charge, interrupt)
}

#[tokio::test]
async fn an_engine_turn_is_interrupted_at_the_boundary_its_spend_uses_up_the_budget() {
    let (runtime, _dir) = test_runtime(|c| {
        c.noesis.enable(Arc::new(Never));
    })
    .await;
    let (mut ctx, charge, interrupt) = noesis_turn(&runtime, NoesisLane::Interactive, 10_000).await;
    let mut subtree = SubtreeUsage::default();
    ctx.turn_usage.cost_usd = 0.004;
    assert!(
        budget_crossed(&runtime, &ctx, &mut subtree)
            .unwrap()
            .is_none()
    );
    assert!(!interrupt.is_cancelled(), "under the Noesis budget");
    assert_eq!(charge.reported_micros.load(Ordering::SeqCst), 4_000);
    ctx.turn_usage.cost_usd = 0.01;
    budget_crossed(&runtime, &ctx, &mut subtree).unwrap();
    assert!(
        interrupt.is_cancelled(),
        "the spend that used up the budget stops the engine"
    );
    runtime.sched.lock().running.clear();
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_background_engine_turn_is_interrupted_at_its_next_boundary_once_disabled() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    assert!(!runtime.noesis_gate().is_enabled());
    let (ctx, _charge, interrupt) = noesis_turn(&runtime, NoesisLane::Background, u64::MAX).await;
    budget_crossed(&runtime, &ctx, &mut SubtreeUsage::default()).unwrap();
    assert!(
        interrupt.is_cancelled(),
        "a background job stops at the boundary"
    );
    let (other, _charge, kept) = noesis_turn(&runtime, NoesisLane::Interactive, u64::MAX).await;
    budget_crossed(&runtime, &other, &mut SubtreeUsage::default()).unwrap();
    assert!(
        !kept.is_cancelled(),
        "an interactive turn runs on through the grace"
    );
    runtime.sched.lock().running.clear();
    runtime.shutdown().await;
}
