//! Native turns that fail on a transient provider or network error wait it
//! out and resume the same session, instead of failing the agent.
use std::time::Duration;

use tokio::time::Instant;

use super::{
    CancellationToken, Commit, DriveResult, EngineKind, EventKind, MAX_ENGINE_ERROR,
    ModelErrorClass, Result, Runtime, TurnCtx, clamp, json, now,
};

/// Recorded before every wait, so clients can show what the agent waits on.
pub(super) const TRANSIENT_RETRY_EVENT: &str = "engine.transient_retry";

/// Longest single wait between attempts.
const MAX_DELAY: Duration = Duration::from_secs(5 * 60);

/// A connect slower than this counts as unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How the next attempt re-enters the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Resume {
    /// Nothing reached the model: send the same prompt again.
    Replay,
    /// The model already acted on the prompt: resume its session with a note,
    /// never the prompt, so no output or tool call is repeated.
    Continue,
    /// The prompt was submitted but never acknowledged, so the session may or
    /// may not hold it: resume it with the prompt, told to check first.
    Recheck,
}

impl Resume {
    fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Continue => "continue",
            Self::Recheck => "recheck",
        }
    }
}

/// What to do about a failed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Retry {
    /// Not a failure this module waits out.
    No,
    /// Retryable, but the failed process was not seen to exit: another run
    /// on its session could race it.
    Unconfirmed,
    Resume(Resume),
}

/// Whether, and how, a failed run may be retried. `session_known` is whether
/// the engine reported a session the model's partial work lives in.
pub(super) fn retry_for(
    run: &DriveResult,
    class: ModelErrorClass,
    pending_request: bool,
    cancelled: bool,
    session_known: bool,
) -> Retry {
    let outcome = &run.outcome;
    // `session_lost` is no bar: a session that could not be reopened for a
    // transient reason still holds its context, so it is retried as it is.
    if !crate::exhaustion::native_transient(class)
        || !outcome.is_error
        || outcome.interrupted
        || run.park.is_some()
        || run.budget_stop.is_some()
        || cancelled
        || pending_request
    {
        return Retry::No;
    }
    // Silence does not prove the prompt went unsent. Only the adapter's own
    // proof, or the provider refusing it for capacity, lets it be resent as is.
    let resume = if !run.unanswered() {
        Resume::Continue
    } else if outcome.prompt_not_submitted || class == ModelErrorClass::Overloaded {
        Resume::Replay
    } else {
        Resume::Recheck
    };
    if resume != Resume::Replay && !session_known {
        return Retry::No;
    }
    if !run.cleaned_up {
        return Retry::Unconfirmed;
    }
    Retry::Resume(resume)
}

/// What a resumed session is told in place of the prompt it already acted on.
pub(super) fn continuation_note(class: ModelErrorClass) -> String {
    format!(
        "[Aether] Your previous turn was interrupted by a transient {} error between the \
         engine and its provider, and this session has been resumed. Continue where you left \
         off; check the results of tool calls you already started before repeating them.",
        class.as_str()
    )
}

/// What a resumed session is told when it may or may not hold `prompt`.
pub(super) fn recheck_note(class: ModelErrorClass, prompt: &str) -> String {
    format!(
        "[Aether] A transient {} error interrupted delivery of the message below, and this \
         session has been resumed. If this conversation already shows you working on it, \
         continue where you left off without repeating finished work or tool calls; \
         otherwise act on it now.\n\n{prompt}",
        class.as_str()
    )
}

/// The reason a retryable failure was not retried.
pub(super) const UNCONFIRMED_CLEANUP: &str =
    "not retried: the failed engine did not exit, so its session could not be resumed safely";

/// Outages a turn's progress may end before its window stops being reset: an
/// engine that answers a little and then fails every attempt is not recovering.
pub(super) const MAX_RECOVERIES: u32 = 8;

/// Exponential backoff under a hard wall-clock window that starts at an
/// outage's first transient failure. Only real progress ends the outage: a
/// failure that streams Claude Code's synthetic error message is not progress.
pub(super) struct Backoff {
    started: Option<Instant>,
    window: Duration,
    floor: Duration,
    waits: u32,
    recoveries: u32,
}

impl Backoff {
    pub(super) fn new(window: Duration, floor: Duration) -> Self {
        Self {
            started: None,
            window,
            floor,
            waits: 0,
            recoveries: 0,
        }
    }

    /// Starts the window at the first failure; later ones fall inside it.
    pub(super) fn failed(&mut self) {
        self.started.get_or_insert_with(Instant::now);
    }

    /// The engine made progress: the next failure opens a fresh window at the
    /// floor delay, at most [`MAX_RECOVERIES`] times a turn. False once spent.
    /// Progress before any failure ends no outage and spends nothing.
    pub(super) fn recovered(&mut self) -> bool {
        if self.started.is_none() {
            debug_assert_eq!(self.waits, 0, "waits follow a failure");
            return true;
        }
        if self.recoveries >= MAX_RECOVERIES {
            return false;
        }
        self.recoveries += 1;
        self.started = None;
        self.waits = 0;
        debug_assert!(!self.spent() || self.window.is_zero());
        true
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.started
            .map_or(Duration::ZERO, |started| started.elapsed())
    }

    pub(super) fn started(&self) -> bool {
        self.started.is_some()
    }

    pub(super) fn waits(&self) -> u32 {
        self.waits
    }

    pub(super) fn window(&self) -> Duration {
        self.window
    }

    pub(super) fn spent(&self) -> bool {
        self.elapsed() >= self.window
    }

    /// The next wait, never past the window; `None` once it is spent.
    pub(super) fn next_delay(&mut self) -> Option<Duration> {
        let remaining = self.window.checked_sub(self.elapsed())?;
        if remaining.is_zero() {
            return None;
        }
        let doubled = self
            .floor
            .saturating_mul(2u32.saturating_pow(self.waits.min(16)));
        self.waits += 1;
        Some(doubled.min(MAX_DELAY).min(remaining))
    }

    /// The terminal error's account of the retries, once the window ran out.
    pub(super) fn gave_up(&self, class: ModelErrorClass) -> String {
        format!(
            "gave up on a transient {} failure after retrying for {} ({} waits)",
            class.as_str(),
            human(self.elapsed()),
            self.waits
        )
    }
}

fn human(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

pub(super) enum Waited {
    /// Back off done, the window still open and, for a network failure, the
    /// provider host answers.
    Ready,
    Cancelled,
    /// The retry window ran out before another attempt could start.
    WindowSpent,
}

/// Hosts an engine needs; any one answering is enough to relaunch it.
fn default_targets(kind: EngineKind) -> Vec<String> {
    let hosts: &[&str] = if kind == EngineKind::CODEX {
        &["chatgpt.com:443", "api.openai.com:443"]
    } else {
        &["api.anthropic.com:443"]
    };
    hosts.iter().map(|host| (*host).to_owned()).collect()
}

/// Empty skips the probe. Behind a proxy a direct connect proves nothing, so
/// the backoff alone paces relaunches there.
fn probe_targets(configured: Option<&[String]>, kind: EngineKind) -> Vec<String> {
    if let Some(targets) = configured {
        return targets.to_vec();
    }
    let proxied = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if proxied {
        return Vec::new();
    }
    default_targets(kind)
}

/// DNS resolution plus a TCP connect: cheap evidence the network is back
/// before an engine launch is spent on it.
async fn reachable(targets: &[String]) -> bool {
    if targets.is_empty() {
        return true;
    }
    for target in targets {
        let connect = tokio::net::TcpStream::connect(target.as_str());
        if matches!(
            tokio::time::timeout(PROBE_TIMEOUT, connect).await,
            Ok(Ok(_))
        ) {
            return true;
        }
    }
    false
}

/// What one transient failure is waited out for.
pub(super) struct Failure<'a> {
    pub(super) engine: EngineKind,
    pub(super) class: ModelErrorClass,
    pub(super) error: &'a str,
    pub(super) resume: Resume,
}

/// Back off, announcing each wait; after a network failure keep backing off
/// until the provider host answers, so no launch is spent while offline.
pub(super) async fn wait(
    runtime: &Runtime,
    ctx: &TurnCtx,
    failure: &Failure<'_>,
    backoff: &mut Backoff,
    cancel: &CancellationToken,
) -> Result<Waited> {
    let targets = match failure.class {
        ModelErrorClass::Network => probe_targets(
            runtime.config.engine_probe_targets.as_deref(),
            failure.engine,
        ),
        _ => Vec::new(),
    };
    loop {
        let Some(delay) = backoff.next_delay() else {
            return Ok(Waited::WindowSpent);
        };
        announce(runtime, ctx, failure, backoff, delay)?;
        tracing::warn!(agent = %ctx.agent.id, attempt = backoff.waits, ?delay,
            class = failure.class.as_str(), error = failure.error,
            "waiting out a transient native engine failure");
        tokio::select! {
            () = cancel.cancelled() => return Ok(Waited::Cancelled),
            () = tokio::time::sleep(delay) => {}
        }
        let up = tokio::select! {
            () = cancel.cancelled() => return Ok(Waited::Cancelled),
            up = reachable(&targets) => up,
        };
        // The last wait ends at the window: no launch starts past it.
        if backoff.spent() {
            return Ok(Waited::WindowSpent);
        }
        if up {
            return Ok(Waited::Ready);
        }
        tracing::info!(agent = %ctx.agent.id, ?targets, "provider still unreachable; not relaunching the engine");
    }
}

fn announce(
    runtime: &Runtime,
    ctx: &TurnCtx,
    failure: &Failure<'_>,
    backoff: &Backoff,
    delay: Duration,
) -> Result<()> {
    let next_at =
        now() + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::zero());
    let waiting_for = match failure.class {
        ModelErrorClass::Network => "network",
        _ => "provider",
    };
    runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
        name: TRANSIENT_RETRY_EVENT.into(),
        payload: json!({
            "engine": failure.engine.as_str(),
            "class": failure.class.as_str(),
            "waiting_for": waiting_for,
            "attempt": backoff.waits,
            "delay_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "next_at": next_at,
            "resume": failure.resume.as_str(),
            "retrying_for_secs": backoff.elapsed().as_secs(),
            "window_secs": backoff.window.as_secs(),
            "error": clamp(failure.error, MAX_ENGINE_ERROR),
        }),
    })))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_the_cap_and_never_passes_the_window() {
        let mut backoff = Backoff::new(Duration::from_secs(3600), Duration::from_secs(15));
        backoff.failed();
        let delays: Vec<_> = (0..7).map(|_| backoff.next_delay().unwrap()).collect();
        assert_eq!(
            delays,
            [15, 30, 60, 120, 240, 300, 300].map(Duration::from_secs)
        );
        backoff.failed();
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(300)));
        let mut short = Backoff::new(Duration::from_secs(20), Duration::from_secs(15));
        short.failed();
        assert_eq!(short.next_delay(), Some(Duration::from_secs(15)));
        assert!(short.next_delay().unwrap() <= Duration::from_secs(20));
        let mut none = Backoff::new(Duration::ZERO, Duration::from_secs(1));
        none.failed();
        assert!(none.spent());
        assert_eq!(none.next_delay(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_window_starts_at_the_first_failure_and_only_progress_resets_it() {
        let mut backoff = Backoff::new(Duration::from_secs(60), Duration::from_secs(15));
        tokio::time::advance(Duration::from_secs(3 * 3600)).await;
        backoff.failed();
        assert!(
            !backoff.spent(),
            "a long turn's first failure gets the window"
        );
        tokio::time::advance(Duration::from_secs(45)).await;
        backoff.failed();
        assert!(!backoff.spent());
        assert_eq!(
            backoff.next_delay(),
            Some(Duration::from_secs(15)),
            "a wait is cut to what is left of the window"
        );
        tokio::time::advance(Duration::from_secs(16)).await;
        backoff.failed();
        assert!(
            backoff.spent(),
            "later failures fall inside the first window"
        );
        assert!(backoff.recovered());
        tokio::time::advance(Duration::from_secs(3600)).await;
        backoff.failed();
        assert!(!backoff.spent(), "progress ended the first outage");
        assert_eq!(
            backoff.next_delay(),
            Some(Duration::from_secs(15)),
            "a new outage starts again at the floor"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn progress_ends_only_so_many_outages_of_one_turn() {
        let mut backoff = Backoff::new(Duration::from_secs(60), Duration::from_secs(15));
        assert!(backoff.recovered(), "progress before any failure is free");
        for _ in 1..MAX_RECOVERIES {
            backoff.failed();
            assert!(backoff.recovered());
        }
        backoff.failed();
        assert!(backoff.recovered(), "the last outage progress may end");
        backoff.failed();
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(!backoff.recovered(), "the reset budget is spent");
        assert!(backoff.spent(), "so the window runs out as it would have");
        assert_eq!(backoff.next_delay(), None);
    }

    #[test]
    fn configured_probe_targets_replace_the_defaults() {
        assert!(probe_targets(Some(&[]), EngineKind::CLAUDE_CODE).is_empty());
        assert_eq!(
            probe_targets(Some(&["127.0.0.1:9".into()]), EngineKind::CODEX),
            ["127.0.0.1:9"]
        );
        assert_eq!(
            default_targets(EngineKind::CLAUDE_CODE),
            ["api.anthropic.com:443"]
        );
    }

    #[tokio::test]
    async fn the_probe_tells_a_listening_host_from_a_closed_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap().to_string();
        assert!(reachable(std::slice::from_ref(&open)).await);
        // Reserve the refused port without listening; releasing it lets other
        // parallel fixtures bind it before the negative probe arrives.
        let closed = tokio::net::TcpSocket::new_v4().unwrap();
        closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let closed_addr = closed.local_addr().unwrap().to_string();
        assert!(!reachable(&[closed_addr]).await);
        assert!(reachable(&[]).await);
    }
}
