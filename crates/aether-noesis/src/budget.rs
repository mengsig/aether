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
    micros: [u64; 2],
    background_tokens: u64,
    caps: [u8; 2],
}

/// Spend is booked in whole micro-USD, so decimal amounts such as $0.10
/// add up exactly and a limit is reached when the bills say it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Amount {
    micros: u64,
    tokens: u64,
}

const MICROS_PER_USD: f64 = 1_000_000.0;

impl Amount {
    fn of(spend: Spend) -> Self {
        debug_assert!(spend.is_valid(), "only valid spend is booked: {spend:?}");
        Self {
            micros: usd_to_micros(spend.usd),
            tokens: spend.tokens,
        }
    }

    fn plus(self, other: Amount) -> Amount {
        Amount {
            micros: self.micros.saturating_add(other.micros),
            tokens: self.tokens.saturating_add(other.tokens),
        }
    }

    fn minus(self, other: Amount) -> Amount {
        debug_assert!(self.micros >= other.micros && self.tokens >= other.tokens);
        Amount {
            micros: self.micros.saturating_sub(other.micros),
            tokens: self.tokens.saturating_sub(other.tokens),
        }
    }

    fn spend(self) -> Spend {
        Spend {
            usd: micros_to_usd(self.micros),
            tokens: self.tokens,
        }
    }
}

/// Rounds to the nearest micro-USD; `as` saturates for huge limits.
fn usd_to_micros(usd: f64) -> u64 {
    (usd * MICROS_PER_USD).round() as u64
}

fn micros_to_usd(micros: u64) -> f64 {
    micros as f64 / MICROS_PER_USD
}

#[derive(Debug)]
struct Ledger {
    day: NaiveDate,
    spent: [Amount; 2],
    reserved: [Amount; 2],
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
            micros: [
                usd_to_micros(budget.interactive_usd_daily),
                usd_to_micros(budget.background_usd_daily),
            ],
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
            reserved: [Amount::default(); 2],
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
        let estimate = Amount::of(estimate);
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
            day: ledger.day,
            estimate,
            settled: false,
        })
    }

    fn check_budget(&self, ledger: &Ledger, lane: Lane, estimate: Amount) -> Result<(), Refusal> {
        let i = lane.index();
        let limit = self.limits.micros[i];
        if limit == 0 {
            return Err(Refusal::BudgetZero { lane });
        }
        let used = ledger.spent[i].plus(ledger.reserved[i]);
        if used.micros >= limit {
            return Err(Refusal::BudgetExhausted {
                lane,
                limit: micros_to_usd(limit),
                used: micros_to_usd(used.micros),
            });
        }
        if used.micros.saturating_add(estimate.micros) > limit {
            return Err(Refusal::WouldExceed {
                lane,
                limit: micros_to_usd(limit),
                estimate: micros_to_usd(estimate.micros),
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

    /// Spend charged to `day` on `lane`, not counting calls still in flight.
    pub fn spent_today(&self, lane: Lane, day: NaiveDate) -> Spend {
        let mut ledger = self.ledger.lock();
        ledger.roll_over(day);
        ledger.spent[lane.index()].spend()
    }

    /// Calls admitted on `lane` and not yet settled.
    pub fn running(&self, lane: Lane) -> u8 {
        self.ledger.lock().running[lane.index()]
    }
}

/// A seed that is not a real amount counts as unlimited spend, so a corrupt
/// usage record refuses calls instead of admitting them all.
fn fail_closed(spend: Spend) -> Amount {
    if spend.is_valid() {
        return Amount::of(spend);
    }
    Amount {
        micros: u64::MAX,
        tokens: u64::MAX,
    }
}

impl Ledger {
    /// A new day starts with nothing spent. Calls in flight keep their slots
    /// and reservations. A clock that moves back never refunds a day.
    fn roll_over(&mut self, day: NaiveDate) {
        if day > self.day {
            self.day = day;
            self.spent = [Amount::default(); 2];
        }
    }

    /// A call is charged to the day that admitted it; once that day has
    /// rolled over, its charge no longer counts against the current one.
    fn release(&mut self, lane: Lane, day: NaiveDate, estimate: Amount, charge: Amount) {
        let i = lane.index();
        debug_assert!(self.running[i] > 0, "a release without an admission");
        debug_assert!(day <= self.day, "an admission from a day not yet reached");
        self.running[i] = self.running[i].saturating_sub(1);
        self.reserved[i] = self.reserved[i].minus(estimate);
        if day == self.day {
            self.spent[i] = self.spent[i].plus(charge);
        }
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
    day: NaiveDate,
    estimate: Amount,
    settled: bool,
}

impl Admission {
    pub fn lane(&self) -> Lane {
        self.lane
    }

    /// The day this call's spend is charged to.
    pub fn day(&self) -> NaiveDate {
        self.day
    }

    /// Charge the call's actual (or estimated) spend and free its slot.
    pub fn settle(mut self, actual: Spend) {
        let charge = if actual.is_valid() {
            Amount::of(actual)
        } else {
            debug_assert!(false, "settled with an invalid spend {actual:?}");
            self.estimate
        };
        self.finish(charge);
    }

    /// The call was never sent: free the slot and charge nothing.
    pub fn abandon_unsent(mut self) {
        self.finish(Amount::default());
    }

    fn finish(&mut self, charge: Amount) {
        debug_assert!(!self.settled, "an admission is finished once");
        let mut ledger = self.ledger.lock();
        ledger.release(self.lane, self.day, self.estimate, charge);
        self.settled = true;
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if !self.settled {
            self.finish(self.estimate);
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
    #[error("{}", no_model_text(.wanted.as_deref(), .alternatives))]
    NoModel {
        wanted: Option<String>,
        alternatives: Vec<String>,
    },
}

/// Names the model refused, if one was configured, and what could run instead.
fn no_model_text(wanted: Option<&str>, alternatives: &[String]) -> String {
    let head = match wanted {
        Some(wanted) => format!("no ready model for Noesis: `{wanted}` is not ready"),
        None => "no ready model for Noesis: no subscription model is ready".to_string(),
    };
    if alternatives.is_empty() {
        return format!(
            "{head}; sign in to an engine CLI (claude-code, codex) or set \
             noesis.billing.allow_api = true with a provider key"
        );
    }
    format!("{head}; ready alternatives: {}", alternatives.join(", "))
}

/// Pick the model for a slot. A configured model is used only if it is ready
/// and allowed, never swapped for another. Unset takes the first ready
/// subscription model, never an API one: API use is configured and allowed.
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
    let chosen = offers
        .iter()
        .find(|offer| offer.ready && offer.billing == Billing::Subscription);
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
        assert_eq!(fail_closed(usd(f64::NAN)).micros, u64::MAX);
        assert_eq!(fail_closed(usd(0.5)).spend(), usd(0.5));
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
        let refusal = choose_model(None, &api_only, true).unwrap_err();
        let alternatives = vec!["openai/gpt".to_string()];
        assert_eq!(
            refusal,
            ModelRefusal::NoModel {
                wanted: None,
                alternatives
            }
        );
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
        assert!(refusal.to_string().contains("`codex/gpt` is not ready"));
        let none = choose_model(None, &[], false).unwrap_err().to_string();
        assert!(none.contains("sign in to an engine CLI"), "{none}");
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

    #[test]
    fn decimal_amounts_meet_the_limit_exactly() {
        let made = Cell::new(0);
        let book = accountant(|s| s.budget.interactive_usd_daily = 0.3);
        for _ in 0..3 {
            call(&book, Lane::Interactive, usd(0.1), &made).unwrap();
        }
        let refusal = call(&book, Lane::Interactive, usd(0.0), &made).unwrap_err();
        assert!(
            matches!(refusal, Refusal::BudgetExhausted { .. }),
            "{refusal}"
        );
        let dollar = accountant(|s| s.budget.interactive_usd_daily = 1.0);
        for _ in 0..10 {
            call(&dollar, Lane::Interactive, usd(0.1), &made).unwrap();
        }
        let refusal = call(&dollar, Lane::Interactive, usd(0.0), &made).unwrap_err();
        assert!(
            matches!(refusal, Refusal::BudgetExhausted { .. }),
            "{refusal}"
        );
        assert_eq!(dollar.spent_today(Lane::Interactive, day(1)), usd(1.0));
        assert_eq!(
            made.get(),
            13,
            "a call refused below the limit or admitted past it"
        );
    }

    #[test]
    fn a_call_is_charged_to_the_day_that_admitted_it() {
        for lane in [Lane::Interactive, Lane::Background] {
            for observe_first in [false, true] {
                let book = accountant(|_| {});
                let admission = book.admit(lane, usd(0.5), day(1)).unwrap();
                assert_eq!(admission.day(), day(1));
                if observe_first {
                    assert_eq!(book.spent_today(lane, day(2)), usd(0.0));
                }
                admission.settle(usd(0.5));
                assert_eq!(
                    book.spent_today(lane, day(2)),
                    usd(0.0),
                    "{lane:?} {observe_first}"
                );
                assert_eq!(book.running(lane), 0);
            }
            let book = accountant(|_| {});
            let admission = book.admit(lane, usd(0.5), day(1)).unwrap();
            assert_eq!(book.spent_today(lane, day(2)), usd(0.0));
            drop(admission);
            assert_eq!(book.spent_today(lane, day(2)), usd(0.0), "{lane:?} drop");
        }
    }

    #[test]
    fn a_call_admitted_while_the_clock_is_behind_counts_on_the_ledger_day() {
        let book = Accountant::new(&NoesisSettings::default(), day(2), LaneSpend::default());
        let admission = book.admit(Lane::Interactive, usd(0.5), day(1)).unwrap();
        assert_eq!(admission.day(), day(2));
        admission.settle(usd(0.5));
        assert_eq!(book.spent_today(Lane::Interactive, day(2)), usd(0.5));
    }

    #[test]
    fn in_flight_reservations_across_midnight_still_hold_their_slot() {
        let book = accountant(|s| {
            s.budget.interactive_usd_daily = 1.0;
            s.concurrency_interactive = 1;
        });
        let late = book.admit(Lane::Interactive, usd(0.9), day(1)).unwrap();
        let refusal = book.admit(Lane::Interactive, usd(0.2), day(2)).unwrap_err();
        assert!(matches!(refusal, Refusal::WouldExceed { .. }), "{refusal}");
        late.settle(usd(0.9));
        assert!(book.admit(Lane::Interactive, usd(0.2), day(2)).is_ok());
    }
}
