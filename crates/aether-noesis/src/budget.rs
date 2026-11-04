//! Daily budgets, concurrency caps and the billing rule for Noesis model calls.
//!
//! Every call is admitted *before* it is made: [`Accountant::admit`] checks
//! the lane's cap and reserves the call's estimated spend, so a call that would
//! cross a limit is refused without reaching a model. Subscription engines
//! report no price, so their estimated cost counts in the same USD limits.

use std::sync::Arc;

use chrono::NaiveDate;
use parking_lot::Mutex;

use crate::settings::NoesisSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// A teaching turn the learner is waiting for.
    Interactive,
    /// Extraction and other jobs nobody is waiting for.
    Background,
}

impl Lane {
    fn index(self) -> usize {
        match self {
            Lane::Interactive => 0,
            Lane::Background => 1,
        }
    }
}

/// Spend in USD (billed, or estimated for subscription engines) and tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spend {
    pub usd: f64,
    pub tokens: u64,
}

impl Spend {
    fn is_valid(self) -> bool {
        self.usd.is_finite() && self.usd >= 0.0
    }

    fn plus(self, other: Spend) -> Spend {
        Spend {
            usd: self.usd + other.usd,
            tokens: self.tokens.saturating_add(other.tokens),
        }
    }

    fn minus(self, other: Spend) -> Spend {
        Spend {
            usd: (self.usd - other.usd).max(0.0),
            tokens: self.tokens.saturating_sub(other.tokens),
        }
    }
}

/// Spend already charged today, per lane; the daemon seeds it at start from
/// Aether's usage records so a restart does not reset the day.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LaneSpend {
    pub interactive: Spend,
    pub background: Spend,
}

/// Why a call was refused. Nothing was sent to a model.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Refusal {
    #[error("the {lane:?} budget is 0, so Noesis makes no {lane:?} calls")]
    BudgetZero { lane: Lane },
    #[error("the {lane:?} daily budget of {limit} is used up ({used} spent or reserved)")]
    BudgetExhausted { lane: Lane, limit: f64, used: f64 },
    #[error("the {lane:?} call estimated at {estimate} would pass the daily budget of {limit}")]
    WouldExceed {
        lane: Lane,
        limit: f64,
        estimate: f64,
    },
    #[error("the background daily token budget of {limit} is used up or would be passed")]
    TokensExhausted { limit: u64 },
    #[error("background jobs are switched off (noesis.concurrency.background = 0)")]
    BackgroundDisabled,
    #[error("all {cap} {lane:?} slots are busy")]
    ConcurrencyFull { lane: Lane, cap: u8 },
    #[error("an estimate must be a finite USD amount ≥ 0, got {usd}")]
    InvalidEstimate { usd: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Limits {
    usd: [f64; 2],
    background_tokens: u64,
    caps: [u8; 2],
}

#[derive(Debug)]
struct Ledger {
    day: NaiveDate,
    spent: [Spend; 2],
    reserved: [Spend; 2],
    running: [u8; 2],
}

/// The budget and concurrency book for one running Noesis service.
#[derive(Debug, Clone)]
pub struct Accountant {
    limits: Limits,
    ledger: Arc<Mutex<Ledger>>,
}

impl Accountant {
    pub fn new(settings: &NoesisSettings, day: NaiveDate, spent: LaneSpend) -> Self {
        debug_assert!(
            settings.validate().is_ok(),
            "settings are validated at start"
        );
        debug_assert!(spent.interactive.is_valid() && spent.background.is_valid());
        let budget = &settings.budget;
        let limits = Limits {
            usd: [budget.interactive_usd_daily, budget.background_usd_daily],
            background_tokens: budget.background_tokens_daily,
            caps: [
                settings.concurrency_interactive,
                settings.concurrency_background,
            ],
        };
        let ledger = Ledger {
            day,
            spent: [
                fail_closed(spent.interactive),
                fail_closed(spent.background),
            ],
            reserved: [Spend::default(); 2],
            running: [0; 2],
        };
        Self {
            limits,
            ledger: Arc::new(Mutex::new(ledger)),
        }
    }

    /// Admit one model call on `lane`, reserving `estimate` and a slot until
    /// the returned [`Admission`] is settled or dropped. Refuses before the call.
    pub fn admit(&self, lane: Lane, estimate: Spend, day: NaiveDate) -> Result<Admission, Refusal> {
        if !estimate.is_valid() {
            return Err(Refusal::InvalidEstimate { usd: estimate.usd });
        }
        let i = lane.index();
        let mut ledger = self.ledger.lock();
        ledger.roll_over(day);
        self.check_budget(&ledger, lane, estimate)?;
        let cap = self.limits.caps[i];
        if lane == Lane::Background && cap == 0 {
            return Err(Refusal::BackgroundDisabled);
        }
        if ledger.running[i] >= cap {
            return Err(Refusal::ConcurrencyFull { lane, cap });
        }
        ledger.running[i] += 1;
        ledger.reserved[i] = ledger.reserved[i].plus(estimate);
        Ok(Admission {
            ledger: Arc::clone(&self.ledger),
            lane,
            estimate,
            settled: false,
        })
    }

    fn check_budget(&self, ledger: &Ledger, lane: Lane, estimate: Spend) -> Result<(), Refusal> {
        let i = lane.index();
        let limit = self.limits.usd[i];
        if limit <= 0.0 {
            return Err(Refusal::BudgetZero { lane });
        }
        let used = ledger.spent[i].plus(ledger.reserved[i]);
        if used.usd >= limit {
            return Err(Refusal::BudgetExhausted {
                lane,
                limit,
                used: used.usd,
            });
        }
        if used.usd + estimate.usd > limit {
            return Err(Refusal::WouldExceed {
                lane,
                limit,
                estimate: estimate.usd,
            });
        }
        if lane == Lane::Background {
            let limit = self.limits.background_tokens;
            let after = used.tokens.saturating_add(estimate.tokens);
            if limit == 0 || used.tokens >= limit || after > limit {
                return Err(Refusal::TokensExhausted { limit });
            }
        }
        Ok(())
    }

    /// Spend charged today on `lane`, not counting calls still in flight.
    pub fn spent_today(&self, lane: Lane, day: NaiveDate) -> Spend {
        let mut ledger = self.ledger.lock();
        ledger.roll_over(day);
        ledger.spent[lane.index()]
    }

    /// Calls admitted on `lane` and not yet settled.
    pub fn running(&self, lane: Lane) -> u8 {
        self.ledger.lock().running[lane.index()]
    }
}

/// A seed that is not a real amount counts as unlimited spend, so a corrupt
/// usage record refuses calls instead of admitting them all.
fn fail_closed(spend: Spend) -> Spend {
    if spend.is_valid() {
        return spend;
    }
    Spend {
        usd: f64::INFINITY,
        tokens: u64::MAX,
    }
}

impl Ledger {
    /// A new day starts with nothing spent. Calls in flight keep their slots
    /// and reservations. A clock that moves back never refunds a day.
    fn roll_over(&mut self, day: NaiveDate) {
        if day > self.day {
            self.day = day;
            self.spent = [Spend::default(); 2];
        }
    }

    fn release(&mut self, lane: Lane, estimate: Spend, charge: Spend) {
        let i = lane.index();
        assert!(self.running[i] > 0, "a release without an admission");
        self.running[i] -= 1;
        self.reserved[i] = self.reserved[i].minus(estimate);
        self.spent[i] = self.spent[i].plus(charge);
    }
}

/// One admitted call. Settle it with what the call cost. Dropped unsettled
/// (an error or a cancel after the call may have started), it charges its
/// estimate, so an unknown outcome is never free.
#[derive(Debug)]
#[must_use = "an admission holds a slot and a reservation until settled"]
pub struct Admission {
    ledger: Arc<Mutex<Ledger>>,
    lane: Lane,
    estimate: Spend,
    settled: bool,
}

impl Admission {
    pub fn lane(&self) -> Lane {
        self.lane
    }

    /// Charge the call's actual (or estimated) spend and free its slot.
    pub fn settle(mut self, actual: Spend) {
        let charge = if actual.is_valid() {
            actual
        } else {
            debug_assert!(false, "settled with an invalid spend {actual:?}");
            self.estimate
        };
        self.ledger.lock().release(self.lane, self.estimate, charge);
        self.settled = true;
    }

    /// The call was never sent: free the slot and charge nothing.
    pub fn abandon_unsent(mut self) {
        self.ledger
            .lock()
            .release(self.lane, self.estimate, Spend::default());
        self.settled = true;
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if !self.settled {
            self.ledger
                .lock()
                .release(self.lane, self.estimate, self.estimate);
        }
    }
}

/// How a model is paid for, as reported by Aether's model list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Billing {
    /// An engine CLI on the user's own subscription (Claude Code, Codex).
    Subscription,
    /// A provider billed per request through an API key.
    Api,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelOffer<'a> {
    pub id: &'a str,
    pub billing: Billing,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelRefusal {
    #[error("`{model}` is billed per request; set noesis.billing.allow_api = true to use it")]
    ApiNotAllowed { model: String },
    #[error("no ready model for Noesis{}", alternatives_hint(.alternatives))]
    NoModel {
        wanted: Option<String>,
        alternatives: Vec<String>,
    },
}

fn alternatives_hint(alternatives: &[String]) -> String {
    if alternatives.is_empty() {
        return String::new();
    }
    format!("; ready alternatives: {}", alternatives.join(", "))
}

/// Pick the model for a slot. A configured model is used only if it is ready
/// and allowed, never swapped for another; unset takes the first ready
/// subscription model. An API model is used only when `allow_api` is set.
pub fn choose_model(
    configured: Option<&str>,
    offers: &[ModelOffer<'_>],
    allow_api: bool,
) -> Result<String, ModelRefusal> {
    let allowed = |offer: &&ModelOffer<'_>| allow_api || offer.billing == Billing::Subscription;
    let no_model = |wanted: Option<&str>| ModelRefusal::NoModel {
        wanted: wanted.map(str::to_string),
        alternatives: offers
            .iter()
            .filter(|offer| offer.ready)
            .filter(allowed)
            .map(|offer| offer.id.to_string())
            .collect(),
    };
    if let Some(wanted) = configured {
        let Some(offer) = offers.iter().find(|offer| offer.id == wanted) else {
            return Err(no_model(Some(wanted)));
        };
        if !allowed(&offer) {
            return Err(ModelRefusal::ApiNotAllowed {
                model: wanted.to_string(),
            });
        }
        if !offer.ready {
            return Err(no_model(Some(wanted)));
        }
        return Ok(wanted.to_string());
    }
    let ready = |billing: Billing| {
        offers
            .iter()
            .find(|offer| offer.ready && offer.billing == billing)
    };
    let chosen = ready(Billing::Subscription).or_else(|| ready(Billing::Api).filter(allowed));
    chosen
        .map(|offer| offer.id.to_string())
        .ok_or_else(|| no_model(None))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn day(n: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, n).unwrap()
    }

    fn usd(usd: f64) -> Spend {
        Spend { usd, tokens: 0 }
    }

    fn accountant(edit: impl FnOnce(&mut NoesisSettings)) -> Accountant {
        let mut settings = NoesisSettings::default();
        edit(&mut settings);
        Accountant::new(&settings, day(1), LaneSpend::default())
    }

    /// A stand-in for a model call: the call happens only if admitted.
    fn call(
        accountant: &Accountant,
        lane: Lane,
        estimate: Spend,
        made: &Cell<u32>,
    ) -> Result<(), Refusal> {
        let admission = accountant.admit(lane, estimate, day(1))?;
        made.set(made.get() + 1);
        admission.settle(estimate);
        Ok(())
    }

    #[test]
    fn a_zero_budget_refuses_every_call_before_it_is_made() {
        let made = Cell::new(0);
        let interactive = accountant(|s| s.budget.interactive_usd_daily = 0.0);
        let refusal = call(&interactive, Lane::Interactive, usd(0.0), &made).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::BudgetZero {
                lane: Lane::Interactive
            }
        );
        let background = accountant(|s| s.budget.background_usd_daily = 0.0);
        let refusal = call(&background, Lane::Background, usd(0.0), &made).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::BudgetZero {
                lane: Lane::Background
            }
        );
        let tokens = accountant(|s| s.budget.background_tokens_daily = 0);
        let refusal = call(&tokens, Lane::Background, usd(0.0), &made).unwrap_err();
        assert_eq!(refusal, Refusal::TokensExhausted { limit: 0 });
        assert_eq!(made.get(), 0, "a refused call reached the model");
    }

    #[test]
    fn at_the_limit_the_next_call_is_refused_before_it_is_made() {
        let made = Cell::new(0);
        let book = accountant(|s| s.budget.interactive_usd_daily = 1.0);
        call(&book, Lane::Interactive, usd(0.6), &made).unwrap();
        call(&book, Lane::Interactive, usd(0.4), &made).unwrap();
        assert_eq!(book.spent_today(Lane::Interactive, day(1)), usd(1.0));
        let refusal = call(&book, Lane::Interactive, usd(0.0), &made).unwrap_err();
        assert!(matches!(refusal, Refusal::BudgetExhausted { limit, .. } if limit == 1.0));
        assert_eq!(made.get(), 2, "the call at the limit reached the model");
        assert_eq!(book.running(Lane::Interactive), 0);
    }

    #[test]
    fn a_call_that_would_pass_the_limit_is_refused_and_reserves_nothing() {
        let book = accountant(|s| s.budget.interactive_usd_daily = 1.0);
        book.admit(Lane::Interactive, usd(0.7), day(1))
            .unwrap()
            .settle(usd(0.7));
        let refusal = book.admit(Lane::Interactive, usd(0.5), day(1)).unwrap_err();
        assert!(matches!(refusal, Refusal::WouldExceed { .. }), "{refusal}");
        book.admit(Lane::Interactive, usd(0.3), day(1))
            .unwrap()
            .settle(usd(0.3));
        assert_eq!(book.running(Lane::Interactive), 0);
    }

    #[test]
    fn calls_in_flight_count_against_the_limit() {
        let book = accountant(|s| {
            s.budget.interactive_usd_daily = 1.0;
            s.concurrency_interactive = 4;
        });
        let first = book.admit(Lane::Interactive, usd(0.6), day(1)).unwrap();
        let refusal = book.admit(Lane::Interactive, usd(0.6), day(1)).unwrap_err();
        assert!(matches!(refusal, Refusal::WouldExceed { .. }), "{refusal}");
        first.abandon_unsent();
        assert_eq!(book.spent_today(Lane::Interactive, day(1)), usd(0.0));
        book.admit(Lane::Interactive, usd(0.6), day(1))
            .unwrap()
            .settle(usd(0.2));
        assert_eq!(book.spent_today(Lane::Interactive, day(1)), usd(0.2));
    }

    #[test]
    fn background_tokens_have_their_own_daily_limit() {
        let book = accountant(|s| s.budget.background_tokens_daily = 100);
        let spend = Spend {
            usd: 0.0,
            tokens: 100,
        };
        book.admit(Lane::Background, spend, day(1))
            .unwrap()
            .settle(spend);
        let refusal = book
            .admit(Lane::Background, Spend::default(), day(1))
            .unwrap_err();
        assert_eq!(refusal, Refusal::TokensExhausted { limit: 100 });
        let over = Spend {
            usd: 0.0,
            tokens: 1,
        };
        let fresh = accountant(|s| s.budget.background_tokens_daily = 100);
        fresh
            .admit(
                Lane::Background,
                Spend {
                    usd: 0.0,
                    tokens: 99,
                },
                day(1),
            )
            .unwrap()
            .settle(Spend {
                usd: 0.0,
                tokens: 99,
            });
        fresh
            .admit(Lane::Background, over, day(1))
            .unwrap()
            .settle(over);
        assert!(
            fresh
                .admit(
                    Lane::Interactive,
                    Spend {
                        usd: 0.0,
                        tokens: 10_000
                    },
                    day(1)
                )
                .is_ok()
        );
    }

    #[test]
    fn caps_limit_calls_per_lane_and_free_on_settle_or_drop() {
        let book = accountant(|s| s.concurrency_interactive = 1);
        let first = book.admit(Lane::Interactive, usd(0.1), day(1)).unwrap();
        let refusal = book.admit(Lane::Interactive, usd(0.1), day(1)).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::ConcurrencyFull {
                lane: Lane::Interactive,
                cap: 1
            }
        );
        let background = book.admit(Lane::Background, usd(0.1), day(1)).unwrap();
        assert_eq!(
            (
                book.running(Lane::Interactive),
                book.running(Lane::Background)
            ),
            (1, 1)
        );
        drop(first);
        drop(background);
        assert_eq!(book.running(Lane::Interactive), 0);
        assert!(book.admit(Lane::Interactive, usd(0.1), day(1)).is_ok());
    }

    #[test]
    fn an_unsettled_drop_charges_its_estimate() {
        let book = accountant(|_| {});
        drop(book.admit(Lane::Interactive, usd(0.25), day(1)).unwrap());
        assert_eq!(book.spent_today(Lane::Interactive, day(1)), usd(0.25));
    }

    #[test]
    fn background_concurrency_zero_means_no_background_jobs() {
        let book = accountant(|s| s.concurrency_background = 0);
        let refusal = book.admit(Lane::Background, usd(0.0), day(1)).unwrap_err();
        assert_eq!(refusal, Refusal::BackgroundDisabled);
        assert!(book.admit(Lane::Interactive, usd(0.0), day(1)).is_ok());
    }

    #[test]
    fn a_new_day_resets_spend_and_a_clock_moving_back_does_not() {
        let mut settings = NoesisSettings::default();
        settings.budget.interactive_usd_daily = 1.0;
        let seeded = LaneSpend {
            interactive: usd(1.0),
            background: usd(0.0),
        };
        let book = Accountant::new(&settings, day(2), seeded);
        assert!(
            book.admit(Lane::Interactive, usd(0.1), day(2)).is_err(),
            "the seed counts"
        );
        assert!(
            book.admit(Lane::Interactive, usd(0.1), day(1)).is_err(),
            "no refund backwards"
        );
        book.admit(Lane::Interactive, usd(0.1), day(3))
            .unwrap()
            .settle(usd(0.1));
        assert_eq!(book.spent_today(Lane::Interactive, day(3)), usd(0.1));
    }

    #[test]
    fn bad_estimates_and_seeds_fail_closed() {
        let book = accountant(|_| {});
        let refusal = book
            .admit(Lane::Interactive, usd(f64::NAN), day(1))
            .unwrap_err();
        assert!(matches!(refusal, Refusal::InvalidEstimate { .. }));
        assert!(book.admit(Lane::Interactive, usd(-1.0), day(1)).is_err());
        assert_eq!(fail_closed(usd(f64::NAN)).usd, f64::INFINITY);
        assert_eq!(fail_closed(usd(0.5)), usd(0.5));
    }

    fn offers() -> Vec<ModelOffer<'static>> {
        vec![
            ModelOffer {
                id: "openai/gpt",
                billing: Billing::Api,
                ready: true,
            },
            ModelOffer {
                id: "codex/gpt",
                billing: Billing::Subscription,
                ready: false,
            },
            ModelOffer {
                id: "claude-code/opus",
                billing: Billing::Subscription,
                ready: true,
            },
        ]
    }

    #[test]
    fn unset_slots_take_the_first_ready_subscription_model() {
        assert_eq!(
            choose_model(None, &offers(), false).unwrap(),
            "claude-code/opus"
        );
        assert_eq!(
            choose_model(None, &offers(), true).unwrap(),
            "claude-code/opus"
        );
    }

    #[test]
    fn api_models_are_never_a_silent_fallback() {
        let api_only = [ModelOffer {
            id: "openai/gpt",
            billing: Billing::Api,
            ready: true,
        }];
        let refusal = choose_model(None, &api_only, false).unwrap_err();
        assert_eq!(
            refusal,
            ModelRefusal::NoModel {
                wanted: None,
                alternatives: vec![]
            }
        );
        assert_eq!(choose_model(None, &api_only, true).unwrap(), "openai/gpt");
        let refusal = choose_model(Some("openai/gpt"), &offers(), false).unwrap_err();
        assert_eq!(
            refusal,
            ModelRefusal::ApiNotAllowed {
                model: "openai/gpt".into()
            }
        );
        assert!(refusal.to_string().contains("noesis.billing.allow_api"));
        assert_eq!(
            choose_model(Some("openai/gpt"), &offers(), true).unwrap(),
            "openai/gpt"
        );
    }

    #[test]
    fn a_configured_model_that_is_not_ready_is_reported_not_replaced() {
        let refusal = choose_model(Some("codex/gpt"), &offers(), false).unwrap_err();
        let ModelRefusal::NoModel {
            wanted,
            alternatives,
        } = &refusal
        else {
            panic!("expected no_model, got {refusal:?}");
        };
        assert_eq!(wanted.as_deref(), Some("codex/gpt"));
        assert_eq!(alternatives, &["claude-code/opus".to_string()]);
        assert!(
            refusal
                .to_string()
                .contains("ready alternatives: claude-code/opus")
        );
        let missing = choose_model(Some("nobody/none"), &offers(), true).unwrap_err();
        assert!(
            matches!(missing, ModelRefusal::NoModel { alternatives, .. } if alternatives.len() == 2)
        );
    }

    #[test]
    fn the_book_is_shared_across_threads() {
        let book = accountant(|s| {
            s.budget.interactive_usd_daily = 1.0;
            s.concurrency_interactive = 4;
        });
        let admitted: u32 = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    let book = book.clone();
                    scope.spawn(move || {
                        let mut admitted = 0;
                        for _ in 0..50 {
                            if let Ok(admission) = book.admit(Lane::Interactive, usd(0.01), day(1))
                            {
                                admitted += 1;
                                admission.settle(usd(0.01));
                            }
                        }
                        admitted
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .sum()
        });
        let spent = book.spent_today(Lane::Interactive, day(1)).usd;
        assert!(spent <= 1.0 + 1e-9, "spent {spent} over the limit");
        assert!((spent - f64::from(admitted) * 0.01).abs() < 1e-9);
        assert_eq!(book.running(Lane::Interactive), 0);
    }
}
