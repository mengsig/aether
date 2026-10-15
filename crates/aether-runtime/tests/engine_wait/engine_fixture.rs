use super::*;

/// What the runtime did to one engine turn. The fake engine records it; the
/// tests assert on it, so deleting a mechanism fails a test instead of
/// changing nothing.
#[derive(Default, Debug)]
pub(super) struct Observed {
    /// The runtime cancelled the turn's token, i.e. stopped the process.
    pub(super) cancelled: AtomicBool,
    /// The runtime closed the turn's stdin.
    pub(super) stdin_closed: AtomicBool,
    /// The runtime asked for the engine's own stop verb.
    pub(super) stopped: AtomicBool,
    /// The grace the stop verb carried, in milliseconds; `u64::MAX` until
    /// the stop arrives.
    pub(super) stop_grace_ms: AtomicU64,
    /// Everything the runtime answered on the decisions channel, in order.
    pub(super) decided: Mutex<Vec<PermissionDecision>>,
    pub(super) steered: Mutex<Vec<String>>,
}

impl Observed {
    pub(super) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    pub(super) fn stdin_closed(&self) -> bool {
        self.stdin_closed.load(Ordering::SeqCst)
    }
    pub(super) fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    pub(super) fn stop_grace(&self) -> Option<Duration> {
        match self.stop_grace_ms.load(Ordering::SeqCst) {
            u64::MAX => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }
    pub(super) fn decided(&self) -> Vec<PermissionDecision> {
        self.decided.lock().expect("decisions").clone()
    }
    pub(super) fn record_stop(&self, grace: Duration) {
        self.stopped.store(true, Ordering::SeqCst);
        self.stop_grace_ms
            .store(grace.as_millis() as u64, Ordering::SeqCst);
    }
}

/// `u64::MAX` marks "no stop seen yet"; `Default` would read as zero.
pub(super) fn observed() -> Arc<Observed> {
    Arc::new(Observed {
        stop_grace_ms: AtomicU64::new(u64::MAX),
        ..Default::default()
    })
}

/// What the fake engine does when the runtime stops it: report the way a CLI
/// does, or die without a word.
#[derive(Debug, Clone)]
pub(super) enum OnKill {
    /// Report these in order as its last words, the way a killed CLI reports
    /// what it had measured and then a synthetic "cancelled".
    Report(Vec<EngineOutcome>),
    /// Nothing: the stream just closes.
    Silent,
}

/// How the fake engine behaves in one test, beyond keeping its turn open.
#[derive(Debug, Clone, Default)]
pub(super) struct Script {
    pub(super) input_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub(super) run_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub(super) prefill_input: bool,
    /// A tool it reports as backgrounded before the turn parks.
    pub(super) background: Option<String>,
    /// Report this backgrounded tool as finished once the runtime has asked
    /// the turn to stop: the last result a parked turn was waiting for.
    pub(super) finish_background: Option<String>,
    /// What the engine reports on a turn that resumes a session - the first
    /// turn has none: a result it still owes background work for, then spend
    /// that crosses the agent's grant while that result is held.
    pub(super) crossing_on_resume: Option<(EngineOutcome, Usage)>,
    /// Raise a permission prompt through the engine's own protocol once the
    /// runtime asks it to stop. This is Codex's route, not the MCP bridge's.
    pub(super) prompt_on_stop: Option<String>,
    /// Report spend that crosses the agent's grant once the runtime has
    /// already asked the turn to stop - the accounting path's own crossing,
    /// found while a park is already open rather than starting one.
    pub(super) crossing_after_stop: Option<Usage>,
    /// Ask the person a question before the turn parks; the runtime answers
    /// it through the decisions channel once a message arrives.
    pub(super) question: bool,
    /// Report the last word as an adapter that killed the process when its
    /// own grace expired does, rather than as an engine that stopped itself.
    pub(super) end_forced: bool,
    /// Final native usage delivered only when the test lets the turn finish.
    pub(super) end_usage: Option<Usage>,
    /// Hold the event stream open at most this long after the runtime stops
    /// the process, so a test can act inside the reap window; ending the turn
    /// closes it sooner.
    pub(super) linger_after_stop: Option<Duration>,
}

pub(super) fn cancelled_with(usage: Usage) -> EngineOutcome {
    EngineOutcome {
        usage,
        is_error: true,
        error: Some("cancelled".into()),
        ..Default::default()
    }
}

/// One engine turn, as the test drives it: the bridge credentials the real
/// `aether mcp-bridge` would read from its argv, what the runtime asked the
/// engine to resume, and the handle that lets the test end the turn.
pub(super) struct TurnStarted {
    pub(super) agent: AgentId,
    pub(super) token: String,
    /// The user prompt the runtime handed the engine for this turn.
    pub(super) prompt: String,
    /// The engine session the runtime asked for; `None` starts a fresh one.
    pub(super) resumed: Option<String>,
    pub(super) observed: Arc<Observed>,
    pub(super) end: oneshot::Sender<()>,
    pub(super) events: mpsc::WeakSender<EngineEvent>,
}

fn credentials_of(turn: &EngineTurn) -> (AgentId, String) {
    let args = &turn.mcp.as_ref().expect("the turn carries a bridge").args;
    let after = |flag: &str| -> String {
        let at = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing from {args:?}"));
        args[at + 1].clone()
    };
    (
        after("--agent").parse().expect("agent id"),
        after("--token"),
    )
}

/// An engine that hands out its per-turn bridge credentials and then keeps
/// the turn open - like a CLI still running its model loop - until the test
/// lets it finish or the runtime stops it.
pub(super) struct ParkEngine {
    pub(super) kind: EngineKind,
    pub(super) started: mpsc::Sender<TurnStarted>,
    pub(super) on_kill: OnKill,
    pub(super) script: Script,
}

/// The channels one wind-down watches: what the runtime does to the turn,
/// and what the fake engine says back.
struct Winding {
    input: Option<mpsc::Receiver<aether_engine::EngineInput>>,
    stop: mpsc::Receiver<Duration>,
    decisions: mpsc::Receiver<PermissionDecision>,
    events: mpsc::Sender<EngineEvent>,
    observed: Arc<Observed>,
    script: Script,
}

/// Wait for the test to end the turn, recording what the runtime does to it
/// meanwhile. `None` means the runtime stopped the process instead.
async fn wind_down(
    ended: oneshot::Receiver<()>,
    cancel: CancellationToken,
    mut w: Winding,
) -> Option<()> {
    tokio::pin!(ended);
    let mut asked = false;
    loop {
        // Once the runtime has asked it to stop, an engine that ignores the
        // ask can still raise a prompt through its own protocol.
        if asked && let Some(id) = w.script.finish_background.take() {
            let _ = w
                .events
                .send(EngineEvent::BackgroundTask { id, running: false })
                .await;
        }
        if asked && let Some(tool) = w.script.prompt_on_stop.take() {
            let _ = w
                .events
                .send(EngineEvent::Permission {
                    request_id: "after-the-park".into(),
                    tool,
                    input: json!({"command": "rm -rf /tmp/whatever"}),
                    side_effect: None,
                })
                .await;
        }
        if asked && let Some(usage) = w.script.crossing_after_stop.take() {
            let _ = w.events.send(EngineEvent::UsageDelta { usage }).await;
        }
        tokio::select! {
            ending = &mut ended => return ending.ok(),
            () = cancel.cancelled() => {
                // A terminal park stops the process in the same tick as it
                // asks; sweep both, or what is observed depends on which
                // select arm won.
                if let Ok(grace) = w.stop.try_recv() {
                    w.observed.record_stop(grace);
                }
                if w.input.as_mut().is_some_and(|rx| {
                    matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Disconnected))
                }) {
                    w.observed.stdin_closed.store(true, Ordering::SeqCst);
                }
                w.observed.cancelled.store(true, Ordering::SeqCst);
                if let Some(linger) = w.script.linger_after_stop {
                    // A ceiling, not a fixed wait: a test that is done acting
                    // inside the reap window ends the turn and the last word
                    // follows at once.
                    tokio::select! {
                        () = tokio::time::sleep(linger) => {}
                        _ = &mut ended => {}
                    }
                }
                return None;
            }
            Some(grace) = w.stop.recv() => {
                w.observed.record_stop(grace);
                asked = true;
            }
            Some(decision) = w.decisions.recv() => {
                w.observed.decided.lock().expect("decisions").push(decision);
            }
            msg = async {
                let permit = match w.script.input_gate.as_ref() {
                    Some(gate) => Some(gate.acquire().await.expect("input gate")),
                    None => None,
                };
                let message = w.input.as_mut().expect("guarded by the arm").recv().await;
                if let Some(permit) = permit {
                    permit.forget();
                }
                message
            },
                if w.input.is_some() =>
            {
                match msg {
                    Some(message)
                        if w.script.input_gate.is_some() && message.text != "fixture-prefill" =>
                    {
                        w.observed.steered.lock().expect("steered").push(message.text);
                        let _ = w.events.send(EngineEvent::InputAccepted).await;
                    }
                    Some(_) => {}
                    None => {
                        w.observed.stdin_closed.store(true, Ordering::SeqCst);
                        w.input = None;
                        asked = true;
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Engine for ParkEngine {
    fn kind(&self) -> EngineKind {
        self.kind
    }

    fn adapter(&self) -> String {
        "park/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (agent, token) = credentials_of(&turn);
        let prompt = turn.prompt.clone();
        let resumed = turn.session.clone();
        let resumed_turn = resumed.is_some();
        let session = resumed
            .clone()
            .unwrap_or_else(|| turn.new_session_id.clone());
        let started = self.started.clone();
        let on_kill = self.on_kill.clone();
        let script = self.script.clone();
        let seen = observed();
        let (end, ended) = oneshot::channel();
        let (events, rx) = mpsc::channel(64);
        let (input_tx, input_rx) = mpsc::channel(if script.prefill_input { 1 } else { 8 });
        if script.prefill_input {
            input_tx
                .try_send("fixture-prefill".into())
                .expect("prefill input");
        }
        let (stop_tx, stop_rx) = mpsc::channel(1);
        let (decision_tx, decision_rx) = mpsc::channel(8);
        let observed = seen.clone();
        tokio::spawn(async move {
            let _ = events
                .send(EngineEvent::Session {
                    id: session,
                    model: None,
                    tools: vec![],
                })
                .await;
            if let Some(id) = script.background.clone() {
                let _ = events
                    .send(EngineEvent::BackgroundTask { id, running: true })
                    .await;
            }
            if script.question {
                let _ = events
                    .send(EngineEvent::Question {
                        request_id: "before-the-park".into(),
                        prompt: QUESTION.into(),
                    })
                    .await;
            }
            let _ = started
                .send(TurnStarted {
                    agent,
                    token,
                    prompt,
                    resumed,
                    observed: observed.clone(),
                    end,
                    events: events.downgrade(),
                })
                .await;
            if resumed_turn && let Some((done, spend)) = script.crossing_on_resume.clone() {
                let _ = events.send(EngineEvent::Done(done)).await;
                let _ = events.send(EngineEvent::UsageDelta { usage: spend }).await;
            }
            let end_forced = script.end_forced;
            let end_usage = script.end_usage.unwrap_or_default();
            let winding = Winding {
                input: Some(input_rx),
                stop: stop_rx,
                decisions: decision_rx,
                events: events.clone(),
                observed: observed.clone(),
                script,
            };
            let outcomes = match wind_down(ended, cancel, winding).await {
                Some(()) => vec![EngineOutcome {
                    closing_accounting: true,
                    text: Some("ok".into()),
                    forced: end_forced,
                    usage: end_usage,
                    ..Default::default()
                }],
                None => match on_kill {
                    OnKill::Silent => return,
                    OnKill::Report(outcomes) => outcomes,
                },
            };
            for outcome in outcomes {
                let _ = events.send(EngineEvent::Done(outcome)).await;
            }
        });
        if let Some(gate) = self.script.run_gate.as_ref() {
            gate.acquire().await.expect("run gate").forget();
        }
        Ok(EngineRun {
            events: rx,
            input: Some(input_tx),
            decisions: Some(decision_tx),
            stop: Some(stop_tx),
        })
    }
}
