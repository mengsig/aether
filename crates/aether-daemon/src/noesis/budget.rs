//! The Noesis budgets as the runtime's admission guard sees them, and the
//! day's spend read back from Aether's own usage records.

use std::sync::Arc;

use aether_core::agent::Agent;
use aether_core::model::Usage;
use aether_model::Models;
use aether_noesis::budget::{Accountant, Admission, Lane, LaneSpend, Spend};
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
#[derive(Debug)]
pub(crate) struct ServiceBudget {
    pub(crate) accountant: Accountant,
    pub(crate) allow_api: bool,
    pub(crate) models: Arc<Models>,
}

impl ServiceBudget {
    /// The first link a turn could run on that is billed per request, when
    /// API billing is not allowed: a fallback must not reach it silently.
    fn forbidden_link(&self, agent: &Agent) -> Option<String> {
        if self.allow_api {
            return None;
        }
        let registry = self.models.registry();
        let chain = agent.chain();
        let found = chain.links().find(|link| {
            !registry
                .provider(&link.provider)
                .is_some_and(|provider| provider.protocol.is_engine())
        });
        found.map(|link| format!("{}/{}", link.provider, link.model))
    }
}

impl NoesisBudget for ServiceBudget {
    fn admit(
        &self,
        agent: &Agent,
        lane_kind: NoesisLane,
    ) -> Result<Box<dyn NoesisCharge>, NoesisRefusal> {
        if let Some(model) = self.forbidden_link(agent) {
            return Err(NoesisRefusal::Exhausted(format!(
                "`{model}` is billed per request; set noesis.billing.allow_api = true to use it"
            )));
        }
        let admitted = self
            .accountant
            .admit(lane(lane_kind), Spend::default(), today());
        match admitted {
            Ok(admission) => Ok(Box::new(Charge { admission })),
            Err(refusal @ aether_noesis::budget::Refusal::ConcurrencyFull { .. }) => {
                Err(NoesisRefusal::Busy(refusal.to_string()))
            }
            Err(refusal) => Err(NoesisRefusal::Exhausted(refusal.to_string())),
        }
    }
}

struct Charge {
    admission: Admission,
}

impl NoesisCharge for Charge {
    fn settle(self: Box<Self>, usage: &Usage) {
        self.admission.settle(spend_of(usage));
    }
}
