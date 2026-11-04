//! The Noesis budgets as the runtime's admission guard sees them, and the
//! day's spend read back from Aether's own usage records.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aether_core::agent::Agent;
use aether_core::model::{ModelRef, Usage};
use aether_model::Models;
use aether_noesis::budget::{Accountant, Admission, Lane, LaneSpend, Spend};
use aether_noesis::settings::NoesisSettings;
use aether_protocol::read_models::{UsageGroup, UsageScope};
use aether_runtime::noesis_guard::{
    NoesisBudget, NoesisCharge, NoesisLane, NoesisRefusal, lane_of,
};
use aether_store::Store;
use chrono::{NaiveDate, Utc};

use crate::usage_breakdown::{UsageRequest, usage_breakdown};

/// Every Noesis role starts with this; creation refuses it to other agents.
const ROLE_PREFIX: &str = "noesis.";
/// More Noesis roles than this would mean the role scheme changed.
const MAX_ROLES: usize = 64;

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

/// Today's Noesis usage per lane (interactive, background), from the
/// `invocation.completed` records of `noesis.*` agents since UTC midnight.
pub(crate) fn usage_today(
    store: &dyn Store,
    day: NaiveDate,
) -> aether_store::Result<(Usage, Usage)> {
    let midnight = day.and_hms_opt(0, 0, 0).map(|time| time.and_utc());
    debug_assert!(midnight.is_some(), "midnight exists on every day");
    let request = UsageRequest {
        scope: UsageScope {
            role_prefix: Some(ROLE_PREFIX.into()),
            ..UsageScope::default()
        },
        by: UsageGroup::Role,
        since: midnight,
        until: None,
        limit: MAX_ROLES,
    };
    let breakdown = usage_breakdown(store, &request)?;
    let (mut interactive, mut background) = (Usage::default(), Usage::default());
    for row in &breakdown.rows {
        debug_assert!(row.key.starts_with(ROLE_PREFIX), "{}", row.key);
        match lane_of(&row.key) {
            NoesisLane::Interactive => interactive += row.usage,
            NoesisLane::Background => background += row.usage,
        }
    }
    if breakdown.truncated {
        // Rows cut by the limit are still in the total: charge them to both lanes.
        interactive = breakdown.total;
        background = breakdown.total;
    }
    Ok((interactive, background))
}

/// Today's spend per lane as the accountant is seeded with it, so a restart
/// does not reset the day.
pub(crate) fn spent_today(store: &dyn Store, day: NaiveDate) -> aether_store::Result<LaneSpend> {
    let (interactive, background) = usage_today(store, day)?;
    let spent = LaneSpend {
        interactive: spend_of(&interactive),
        background: spend_of(&background),
    };
    debug_assert!(spent.interactive.usd >= 0.0 && spent.background.usd >= 0.0);
    Ok(spent)
}

/// The runtime's view of a running service's [`Accountant`], plus the
/// billing rule: an API-billed link is refused unless `billing.allow_api`.
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
    /// Spend turns still running have reported, not yet in the accountant.
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
        let registry = self.models.registry();
        let billed = |model: &ModelRef| {
            !registry
                .provider(&model.provider)
                .is_some_and(|provider| provider.protocol.is_engine())
        };
        let compactor = self.models.compactor_for(model);
        [model, &compactor]
            .into_iter()
            .find(|model| billed(model))
            .map(|model| {
                format!(
                    "`{}/{}` is billed per request; set noesis.billing.allow_api = true to use it",
                    model.provider, model.model
                )
            })
    }

    /// The first link of the agent's chain the billing rule refuses.
    fn forbidden_chain(&self, agent: &Agent) -> Option<String> {
        agent.chain().links().find_map(|link| self.billed(link))
    }

    /// Why `lane` admits nothing more today, counting what running turns
    /// have reported as well as what the accountant has settled.
    fn spent_out(&self, lane_kind: NoesisLane) -> Option<String> {
        let i = index(lane_kind);
        let settled = self.accountant.spent_today(lane(lane_kind), today());
        let unsettled = self.unsettled.lock()[i];
        let usd = settled.usd + unsettled.usd;
        debug_assert!(usd.is_finite() && usd >= 0.0, "spend is a real amount");
        if usd >= self.usd_daily[i] {
            return Some(format!(
                "today's Noesis {lane_kind:?} budget of ${:.2} is spent",
                self.usd_daily[i]
            ));
        }
        let tokens = settled.tokens.saturating_add(unsettled.tokens);
        if lane_kind == NoesisLane::Background && tokens >= self.background_tokens_daily {
            return Some(format!(
                "today's Noesis background budget of {} tokens is spent",
                self.background_tokens_daily
            ));
        }
        None
    }

    /// Replace one charge's reported spend `before` with `after`.
    fn move_unsettled(&self, lane_kind: NoesisLane, before: Spend, after: Spend) {
        let mut unsettled = self.unsettled.lock();
        let slot = &mut unsettled[index(lane_kind)];
        slot.usd = (slot.usd - before.usd + after.usd).max(0.0);
        slot.tokens = slot
            .tokens
            .saturating_sub(before.tokens)
            .saturating_add(after.tokens);
        debug_assert!(slot.usd.is_finite(), "unsettled spend stays a real amount");
    }
}

impl NoesisBudget for ServiceBudget {
    fn admit(
        &self,
        agent: &Agent,
        lane_kind: NoesisLane,
    ) -> Result<Box<dyn NoesisCharge>, NoesisRefusal> {
        let open = self.open.lock();
        if !*open {
            return Err(NoesisRefusal::Busy(
                "Noesis is switching its settings".into(),
            ));
        }
        if let Some(reason) = self.book.forbidden_chain(agent) {
            return Err(NoesisRefusal::Exhausted(reason));
        }
        if let Some(reason) = self.book.spent_out(lane_kind) {
            return Err(NoesisRefusal::Exhausted(reason));
        }
        let admitted = self
            .book
            .accountant
            .admit(lane(lane_kind), Spend::default(), today());
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
        // Charged before the report is withdrawn: briefly counted twice, never zero.
        admission.settle(spent);
        let reported = std::mem::take(&mut state.reported);
        self.book
            .move_unsettled(self.lane, reported, Spend::default());
    }
}

impl NoesisCharge for Charge {
    fn is_current(&self) -> bool {
        !self.book.retired.load(Ordering::SeqCst)
    }

    fn record(&self, spent: &Usage) -> Option<String> {
        let now = spend_of(spent);
        {
            let mut state = self.state.lock();
            // Settled: the turn has nothing more to report.
            state.admission.as_ref()?;
            let before = std::mem::replace(&mut state.reported, now);
            self.book.move_unsettled(self.lane, before, now);
        }
        self.book.spent_out(self.lane)
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
