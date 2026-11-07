//! The Noesis budgets as the runtime's admission guard sees them, and the
//! day's spend read back from Aether's own usage records.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aether_core::agent::Agent;
use aether_core::event::EventKind;
use aether_core::id::AgentId;
use aether_core::model::{ModelRef, Usage};
use aether_core::noesis::SessionUsage;
use aether_model::Models;
use aether_noesis::budget::{
    Accountant, Admission, INTERACTIVE_BUDGET_SETTING, Lane, LaneSpend, Spend,
};
use aether_noesis::settings::NoesisSettings;
use aether_runtime::noesis_guard::{
    NoesisBudget, NoesisCharge, NoesisLane, NoesisRefusal, lane_of, usage_spent,
};
use aether_store::{AgentFilter, EventFilter, Store};
use chrono::{NaiveDate, Utc};

/// Every Noesis role starts with this; creation refuses it to other agents.
const ROLE_PREFIX: &str = "noesis.";

pub(crate) fn today() -> NaiveDate {
    Utc::now().date_naive()
}

/// What a turn's usage charges: billed dollars plus the estimate for
/// subscription engines, which §6.1 counts in the same limits.
pub(crate) fn spend_of(usage: &Usage) -> Spend {
    let usd = usage.cost_usd + usage.estimated_cost_usd.unwrap_or(0.0);
    Spend {
        usd: if usd.is_finite() { usd.max(0.0) } else { 0.0 },
        tokens: usage.total_tokens(),
    }
}

fn lane(lane: NoesisLane) -> Lane {
    match lane {
        NoesisLane::Interactive => Lane::Interactive,
        NoesisLane::Background => Lane::Background,
    }
}

/// Events read per page when rebuilding the day.
const PAGE: usize = 2_000;

/// Each Noesis agent with its usage today, rebuilt from the
/// `agent.usage_updated` records of `noesis.*` agents since UTC midnight.
/// Each record's absolute total is turned into a charge by the same
/// [`usage_spent`] a turn settles with, so completed and failed invocations
/// and compaction attempts count here exactly as they were charged.
pub(crate) fn usage_today(
    store: &dyn Store,
    day: NaiveDate,
) -> aether_store::Result<Vec<(Agent, Usage)>> {
    let midnight = day.and_hms_opt(0, 0, 0).map(|time| time.and_utc());
    debug_assert!(midnight.is_some(), "midnight exists on every day");
    let mut agents = noesis_agents(store)?;
    let filter = EventFilter {
        types: vec!["agent.usage_updated".into()],
        ..EventFilter::default()
    };
    let mut last_total: HashMap<AgentId, Usage> = HashMap::new();
    let mut today: HashMap<AgentId, Usage> = HashMap::new();
    let mut after = 0u64;
    loop {
        let page = store.events(after, PAGE, &filter)?;
        let Some(last) = page.last() else { break };
        assert!(last.seq > after, "the log pages forwards");
        after = last.seq;
        for event in &page {
            let (Some(agent), EventKind::AgentUsageUpdated { total, .. }) =
                (event.agent_id, &event.kind)
            else {
                continue;
            };
            if !agents.contains_key(&agent) {
                continue;
            }
            let before = last_total.insert(agent, *total).unwrap_or_default();
            if midnight.is_some_and(|midnight| event.at < midnight) {
                continue;
            }
            *today.entry(agent).or_default() += usage_spent(&before, total);
        }
        if page.len() < PAGE {
            break;
        }
    }
    let used: Vec<(Agent, Usage)> = today
        .into_iter()
        .filter_map(|(id, usage)| agents.remove(&id).map(|agent| (agent, usage)))
        .collect();
    debug_assert!(
        used.iter()
            .all(|(agent, _)| agent.role.as_str().starts_with(ROLE_PREFIX))
    );
    Ok(used)
}

/// Every Noesis agent, by its role.
fn noesis_agents(store: &dyn Store) -> aether_store::Result<HashMap<AgentId, Agent>> {
    let agents = store.agents(&AgentFilter {
        exclude_archived: false,
        ..AgentFilter::default()
    })?;
    Ok(agents
        .into_iter()
        .filter(|agent| agent.role.as_str().starts_with(ROLE_PREFIX))
        .map(|agent| (agent.id, agent))
        .collect())
}

/// Whether `model` runs on an engine CLI (a subscription), not an API.
pub(crate) fn runs_on_engine(models: &Models, model: &ModelRef) -> bool {
    models
        .registry()
        .provider(&model.provider)
        .is_some_and(|provider| provider.protocol.is_engine())
}

/// Agents' usage summed per lane: (interactive, background).
pub(crate) fn by_lane(used: &[(Agent, Usage)]) -> (Usage, Usage) {
    let (mut interactive, mut background) = (Usage::default(), Usage::default());
    for (agent, usage) in used {
        match lane_of(agent.role.as_str()) {
            NoesisLane::Interactive => interactive += *usage,
            NoesisLane::Background => background += *usage,
        }
    }
    (interactive, background)
}

/// Usage on each model summed by billing. Only an API provider bills per
/// request; an engine's use is covered by its plan. A provider no longer
/// configured cannot say which it was, so its use is counted as unknown.
pub(crate) fn by_billing<'a>(
    models: &Models,
    used: impl IntoIterator<Item = (&'a ModelRef, &'a Usage)>,
) -> SessionUsage {
    let mut split = SessionUsage::default();
    let mut total_tokens = 0;
    for (model, usage) in used {
        total_tokens += usage.input_tokens;
        let bucket = match models.registry().provider(&model.provider) {
            Some(provider) if provider.protocol.is_engine() => &mut split.subscription,
            Some(_) => &mut split.api,
            None => &mut split.unknown,
        };
        *bucket += *usage;
    }
    debug_assert_eq!(
        split.subscription.input_tokens + split.api.input_tokens + split.unknown.input_tokens,
        total_tokens,
        "every use lands in exactly one bucket"
    );
    split
}

/// Today's spend per lane as the accountant is seeded with it, so a restart
/// does not reset the day.
pub(crate) fn spent_today(store: &dyn Store, day: NaiveDate) -> aether_store::Result<LaneSpend> {
    let (interactive, background) = by_lane(&usage_today(store, day)?);
    let spent = LaneSpend {
        interactive: spend_of(&interactive),
        background: spend_of(&background),
    };
    debug_assert!(spent.interactive.usd >= 0.0 && spent.background.usd >= 0.0);
    Ok(spent)
}

/// The runtime's view of a running service's [`Accountant`], plus the
/// billing rule: an API-billed link is refused unless `billing.allow_api`.
/// The rule is applied by the turn on the chain it actually executes
/// ([`NoesisCharge::refuses_model`]), never on the row admission read.
/// Closed, it admits nothing, so its outstanding charges can be counted;
/// retired, the charges it gave out start no turn.
#[derive(Debug)]
pub(crate) struct ServiceBudget {
    book: Arc<Book>,
    /// Held across each admission, so a close sees every reservation made.
    open: parking_lot::Mutex<bool>,
}

/// What the budget and every charge it gave out share.
#[derive(Debug)]
struct Book {
    accountant: Accountant,
    allow_api: bool,
    models: Arc<Models>,
    usd_daily: [f64; 2],
    background_tokens_daily: u64,
    /// The one lock over settled and unsettled spend. Every read, charge,
    /// settle and move of spend happens while it is held, so no reader sees
    /// a half-moved state. It is always taken before the accountant's own.
    unsettled: parking_lot::Mutex<[Spend; 2]>,
    retired: AtomicBool,
}

fn index(lane: NoesisLane) -> usize {
    match lane {
        NoesisLane::Interactive => 0,
        NoesisLane::Background => 1,
    }
}

impl ServiceBudget {
    pub(crate) fn new(
        accountant: Accountant,
        settings: &NoesisSettings,
        models: Arc<Models>,
    ) -> Self {
        let budget = &settings.budget;
        debug_assert!(
            budget.interactive_usd_daily.is_finite() && budget.background_usd_daily.is_finite()
        );
        Self {
            book: Arc::new(Book {
                accountant,
                allow_api: settings.billing_allow_api,
                models,
                usd_daily: [budget.interactive_usd_daily, budget.background_usd_daily],
                background_tokens_daily: budget.background_tokens_daily,
                unsettled: parking_lot::Mutex::new([Spend::default(); 2]),
                retired: AtomicBool::new(false),
            }),
            open: parking_lot::Mutex::new(true),
        }
    }

    /// Admit nothing more; the turns admitted so far and not yet settled.
    pub(crate) fn close(&self) -> u32 {
        let mut open = self.open.lock();
        *open = false;
        self.outstanding()
    }

    pub(crate) fn reopen(&self) {
        debug_assert!(
            !self.book.retired.load(Ordering::SeqCst),
            "a retired budget stays closed"
        );
        *self.open.lock() = true;
    }

    /// Close for good: a charge it gave out that has not started a turn
    /// never will, so no turn spends against a ledger nobody reads.
    pub(crate) fn retire(&self) {
        self.close();
        self.book.retired.store(true, Ordering::SeqCst);
    }

    /// Why an extraction job's next model call must not start: the budget is
    /// closed or retired, or today's background budget is spent. The turn
    /// the call runs as is then admitted and charged by the runtime's guard.
    pub(crate) fn background_refusal(&self) -> Option<String> {
        if !*self.open.lock() || self.book.retired.load(Ordering::SeqCst) {
            return Some("Noesis is not taking background work now".into());
        }
        self.spent(NoesisLane::Background)
    }

    /// Why `lane` admits nothing more today, naming spent, limit and setting.
    pub(crate) fn spent(&self, lane_kind: NoesisLane) -> Option<String> {
        let unsettled = self.book.unsettled.lock();
        self.book.spent_out(&unsettled, lane_kind)
    }

    /// Admissions not yet settled, started or not.
    pub(crate) fn outstanding(&self) -> u32 {
        u32::from(self.book.accountant.running(Lane::Interactive))
            + u32::from(self.book.accountant.running(Lane::Background))
    }
}

impl Book {
    /// `model`, or the compactor it would use, when it is billed per
    /// request and API billing is not allowed.
    fn billed(&self, model: &ModelRef) -> Option<String> {
        if self.allow_api {
            return None;
        }
        let compactor = self.models.compactor_for(model);
        [model, &compactor]
            .into_iter()
            .find(|model| !runs_on_engine(&self.models, model))
            .map(|model| {
                format!(
                    "`{}/{}` is billed per request; set noesis.billing.allow_api = true to use it",
                    model.provider, model.model
                )
            })
    }

    /// Why `lane` admits nothing more today, counting what running turns
    /// have reported as well as what the accountant has settled. The caller
    /// holds the book's lock.
    fn spent_out(&self, unsettled: &[Spend; 2], lane_kind: NoesisLane) -> Option<String> {
        let i = index(lane_kind);
        let settled = self.accountant.spent_today(lane(lane_kind), today());
        let unsettled = unsettled[i];
        let usd = settled.usd + unsettled.usd;
        debug_assert!(usd.is_finite() && usd >= 0.0, "spend is a real amount");
        let (name, setting) = match lane_kind {
            NoesisLane::Interactive => ("interactive", INTERACTIVE_BUDGET_SETTING),
            NoesisLane::Background => ("background", "noesis.budget.background_usd_daily"),
        };
        if usd >= self.usd_daily[i] {
            return Some(format!(
                "today's Noesis {name} budget is spent: ${usd:.2} of ${:.2}; \
                 raise {setting} to continue today",
                self.usd_daily[i]
            ));
        }
        let tokens = settled.tokens.saturating_add(unsettled.tokens);
        if lane_kind == NoesisLane::Background && tokens >= self.background_tokens_daily {
            return Some(format!(
                "today's Noesis background budget is spent: {tokens} of {} tokens; \
                 raise noesis.budget.background_tokens_daily to continue today",
                self.background_tokens_daily
            ));
        }
        None
    }
}

/// Replace one charge's reported spend `before` with `after` in `slot`.
fn move_spend(slot: &mut Spend, before: Spend, after: Spend) {
    slot.usd = (slot.usd - before.usd + after.usd).max(0.0);
    slot.tokens = slot
        .tokens
        .saturating_sub(before.tokens)
        .saturating_add(after.tokens);
    debug_assert!(slot.usd.is_finite(), "unsettled spend stays a real amount");
}

impl NoesisBudget for ServiceBudget {
    fn admit(
        &self,
        _agent: &Agent,
        lane_kind: NoesisLane,
    ) -> Result<Box<dyn NoesisCharge>, NoesisRefusal> {
        let open = self.open.lock();
        if !*open {
            return Err(NoesisRefusal::Busy(
                "Noesis is switching its settings".into(),
            ));
        }
        let admitted = {
            let unsettled = self.book.unsettled.lock();
            if let Some(reason) = self.book.spent_out(&unsettled, lane_kind) {
                return Err(NoesisRefusal::Exhausted(reason));
            }
            self.book
                .accountant
                .admit(lane(lane_kind), Spend::default(), today())
        };
        drop(open);
        match admitted {
            Ok(admission) => Ok(Box::new(Charge {
                book: self.book.clone(),
                lane: lane_kind,
                state: parking_lot::Mutex::new(ChargeState {
                    admission: Some(admission),
                    reported: Spend::default(),
                }),
            })),
            Err(refusal @ aether_noesis::budget::Refusal::ConcurrencyFull { .. }) => {
                Err(NoesisRefusal::Busy(refusal.to_string()))
            }
            Err(refusal) => Err(NoesisRefusal::Exhausted(refusal.to_string())),
        }
    }
}

struct Charge {
    book: Arc<Book>,
    lane: NoesisLane,
    state: parking_lot::Mutex<ChargeState>,
}

/// `admission` is `None` once settled; `reported` is the spend the running
/// turn has reported so far, counted in the book's `unsettled`.
struct ChargeState {
    admission: Option<Admission>,
    reported: Spend,
}

impl Charge {
    fn finish(&self, spent: Spend) {
        let mut state = self.state.lock();
        let Some(admission) = state.admission.take() else {
            return;
        };
        debug_assert_eq!(
            admission.lane(),
            lane(self.lane),
            "settled on the lane it was admitted on"
        );
        let reported = std::mem::take(&mut state.reported);
        // Charge and withdraw the report in one hold of the book's lock.
        let mut unsettled = self.book.unsettled.lock();
        admission.settle(spent);
        move_spend(&mut unsettled[index(self.lane)], reported, Spend::default());
    }
}

impl NoesisCharge for Charge {
    fn is_current(&self) -> bool {
        !self.book.retired.load(Ordering::SeqCst)
    }

    fn before_invocation(&self, spent: &Usage) -> Option<String> {
        if !self.is_current() {
            return Some("the Noesis service that admitted this turn has been replaced".into());
        }
        let now = spend_of(spent);
        let mut state = self.state.lock();
        // Settled: the turn has nothing more to report.
        state.admission.as_ref()?;
        let before = std::mem::replace(&mut state.reported, now);
        let mut unsettled = self.book.unsettled.lock();
        move_spend(&mut unsettled[index(self.lane)], before, now);
        self.book.spent_out(&unsettled, self.lane)
    }

    fn refuses_model(&self, model: &ModelRef) -> Option<String> {
        self.book.billed(model)
    }

    fn settle(&self, usage: &Usage) {
        self.finish(spend_of(usage));
    }
}

impl Drop for Charge {
    /// Never settled (a panic in the turn's task): charge what it reported.
    fn drop(&mut self) {
        let reported = self.state.lock().reported;
        self.finish(reported);
    }
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
