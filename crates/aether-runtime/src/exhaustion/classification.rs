use aether_core::model::{ModelErrorClass, provider_exhaustion};

/// Provider classes a configured fallback may recover from. The exhausted
/// link itself is never retried.
pub(crate) fn recoverable_by_fallback(class: ModelErrorClass) -> bool {
    matches!(
        class,
        ModelErrorClass::QuotaExhausted
            | ModelErrorClass::UsageLimit
            | ModelErrorClass::ContextLimit
    )
}

/// Classes a native turn ends on to fall back or suspend rather than fail: a
/// refused sign-in is renewed by the operator, and no retry of the turn helps.
pub(crate) fn native_suspends(class: ModelErrorClass) -> bool {
    recoverable_by_fallback(class) || class == ModelErrorClass::Auth
}

/// Classes a native turn waits out instead of failing. Exhaustion and context
/// limits are never among them: those take the fallback/suspension path.
pub(crate) fn native_transient(class: ModelErrorClass) -> bool {
    matches!(
        class,
        ModelErrorClass::Network
            | ModelErrorClass::Timeout
            | ModelErrorClass::Overloaded
            | ModelErrorClass::Server
    )
}

/// Native engines expose error prose, unlike the API's authoritative class.
/// Only for text a native engine reported as its own failure.
pub(crate) fn engine_error_class(error: &str) -> ModelErrorClass {
    if let Some(class) = provider_exhaustion(error) {
        return class;
    }
    let lower = error.to_lowercase();
    if lower.contains("contextlimit:")
        || lower.contains("context_length_exceeded")
        || lower.contains("prompt is too long")
        || lower.contains("context window exceeded")
    {
        ModelErrorClass::ContextLimit
    } else if AUTH_SIGNALS.iter().any(|signal| lower.contains(signal)) {
        ModelErrorClass::Auth
    } else if lower.contains("codex app-server did not start the turn in time")
        || transient_spawn_failure(&lower)
    {
        ModelErrorClass::Timeout
    } else if [
        "selected model is at capacity",
        "model is overloaded",
        "overloaded_error",
    ]
    .iter()
    .any(|signal| lower.contains(signal))
    {
        ModelErrorClass::Overloaded
    } else if NETWORK_SIGNALS.iter().any(|signal| lower.contains(signal)) {
        ModelErrorClass::Network
    } else if SERVER_SIGNALS.iter().any(|signal| lower.contains(signal)) {
        ModelErrorClass::Server
    } else {
        ModelErrorClass::Unknown
    }
}

/// A launch the OS refused for now: fork limits or memory under load, or the
/// binary being replaced by an update. A missing CLI or a denial is not one.
fn transient_spawn_failure(lower: &str) -> bool {
    lower.contains("could not start ") && transient_os_error(lower)
}

/// Whether lowercased `os_error` is one the OS clears by itself: EAGAIN,
/// ETXTBSY or ENOMEM, as opposed to a missing file or a denial.
pub(crate) fn transient_os_error(os_error: &str) -> bool {
    debug_assert_eq!(os_error, os_error.to_lowercase(), "matched lowercased");
    TRANSIENT_SPAWN_SIGNALS
        .iter()
        .any(|signal| os_error.contains(signal))
}

/// EAGAIN, ETXTBSY and ENOMEM, as the OS words them and as Rust names their kinds.
const TRANSIENT_SPAWN_SIGNALS: [&str; 6] = [
    "resource temporarily unavailable",
    "operation would block",
    "text file busy",
    "executable file busy",
    "cannot allocate memory",
    "out of memory",
];

/// A sign-in the provider refused, as Claude Code (its API error types, its
/// `/login` hint, the 401 its result reports) and Codex word it.
const AUTH_SIGNALS: [&str; 11] = [
    "authentication_error",
    "invalid api key",
    "invalid x-api-key",
    "please run /login",
    "oauth token has expired",
    "api_error_status: 401",
    "api error: 401",
    "unexpected status 401",
    "status code 401",
    "401 unauthorized",
    "access token could not be refreshed",
];

/// Connectivity failures as Claude Code (Node errno and its own prose) and
/// Codex report them while the host is offline.
const NETWORK_SIGNALS: [&str; 15] = [
    "eai_again",
    "enotfound",
    "econnrefused",
    "econnreset",
    "etimedout",
    "enetunreach",
    "ehostunreach",
    "can't reach the api server",
    "connection refused",
    "connection error",
    "fetch failed",
    "socket hang up",
    "network error",
    "stream disconnected",
    "workspace routing discovery failed",
];

/// Provider-side 5xx failures reported as prose.
const SERVER_SIGNALS: [&str; 5] = [
    "internal server error",
    "api_error",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_failures_classify_transients_without_masking_exhaustion() {
        for (error, class) in [
            (
                "codex app-server did not start the turn in time",
                ModelErrorClass::Timeout,
            ),
            (
                "Selected model is at capacity. Please try a different model.",
                ModelErrorClass::Overloaded,
            ),
            ("Model is overloaded", ModelErrorClass::Overloaded),
            ("overloaded_error", ModelErrorClass::Overloaded),
            (
                "overloaded_error: weekly limit reached",
                ModelErrorClass::UsageLimit,
            ),
            (
                "at capacity: insufficient_quota",
                ModelErrorClass::QuotaExhausted,
            ),
            (
                "prompt is too long; model is overloaded",
                ModelErrorClass::ContextLimit,
            ),
            ("process failed", ModelErrorClass::Unknown),
        ] {
            assert_eq!(engine_error_class(error), class, "{error}");
        }
    }

    #[test]
    fn a_launch_the_os_refused_for_now_is_transient_and_a_missing_cli_is_not() {
        for error in [
            "could not start claude: Resource temporarily unavailable (os error 11)",
            "could not start claude: Text file busy (os error 26)",
            "could not start codex: Cannot allocate memory (os error 12)",
            "could not start claude: operation would block",
            "could not start claude: executable file busy",
        ] {
            assert_eq!(
                engine_error_class(error),
                ModelErrorClass::Timeout,
                "{error}"
            );
        }
        for error in [
            "could not start claude: Permission denied (os error 13)",
            "claude is not installed or not on PATH",
            "FATAL ERROR: JavaScript heap out of memory",
        ] {
            assert_eq!(
                engine_error_class(error),
                ModelErrorClass::Unknown,
                "{error}"
            );
        }
    }

    #[test]
    fn native_connectivity_and_server_failures_classify_as_transient() {
        for (error, class) in [
            (
                "API Error: Can't reach the API server — check your internet or DNS (EAI_AGAIN)",
                ModelErrorClass::Network,
            ),
            (
                "API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)",
                ModelErrorClass::Network,
            ),
            (
                "getaddrinfo ENOTFOUND api.anthropic.com",
                ModelErrorClass::Network,
            ),
            ("read ECONNRESET", ModelErrorClass::Network),
            (
                "connect ETIMEDOUT 160.79.104.10:443",
                ModelErrorClass::Network,
            ),
            ("connect ENETUNREACH", ModelErrorClass::Network),
            ("connect EHOSTUNREACH", ModelErrorClass::Network),
            ("API Error: Connection error.", ModelErrorClass::Network),
            ("TypeError: fetch failed", ModelErrorClass::Network),
            ("socket hang up", ModelErrorClass::Network),
            ("network error while streaming", ModelErrorClass::Network),
            (
                "stream disconnected before completion: error sending request",
                ModelErrorClass::Network,
            ),
            (
                "workspace routing discovery failed",
                ModelErrorClass::Network,
            ),
            (
                "API Error: 500 Internal server error",
                ModelErrorClass::Server,
            ),
            (
                r#"API Error: {"type":"error","error":{"type":"api_error","message":"Internal"}}"#,
                ModelErrorClass::Server,
            ),
            ("502 Bad Gateway", ModelErrorClass::Server),
            ("503 Service Unavailable", ModelErrorClass::Server),
            ("504 Gateway Timeout", ModelErrorClass::Server),
            // Exhaustion and context limits outrank every transient signal.
            (
                "Weekly limit reached (fetch failed)",
                ModelErrorClass::UsageLimit,
            ),
            (
                "insufficient_quota after ECONNRESET",
                ModelErrorClass::QuotaExhausted,
            ),
            (
                "prompt is too long; Internal server error",
                ModelErrorClass::ContextLimit,
            ),
            (
                "context_length_exceeded: stream disconnected",
                ModelErrorClass::ContextLimit,
            ),
            // Capacity keeps its own class even when the transport also failed.
            (
                "Model is overloaded: socket hang up",
                ModelErrorClass::Overloaded,
            ),
            // A refused sign-in outranks the transport and 5xx prose around it.
            (
                "authentication_error: invalid x-api-key",
                ModelErrorClass::Auth,
            ),
            (
                r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired."}} · Please run /login"#,
                ModelErrorClass::Auth,
            ),
            ("Invalid API key · Please run /login", ModelErrorClass::Auth),
            (
                "Failed to authenticate.\napi_error_status: 401",
                ModelErrorClass::Auth,
            ),
            (
                "unexpected status 401 Unauthorized: stream disconnected",
                ModelErrorClass::Auth,
            ),
            (
                "Your access token could not be refreshed because your refresh token was already used.",
                ModelErrorClass::Auth,
            ),
            // A bare 401 status in each CLI's own wording, with no other signal.
            (
                "unexpected status 401: authentication required",
                ModelErrorClass::Auth,
            ),
            (
                "API Error: 401 (stream disconnected)",
                ModelErrorClass::Auth,
            ),
            ("Request failed with status code 401", ModelErrorClass::Auth),
            ("session expired: please log in", ModelErrorClass::Unknown),
        ] {
            assert_eq!(engine_error_class(error), class, "{error}");
        }
    }

    #[test]
    fn a_refused_sign_in_suspends_a_native_turn_and_a_hiccup_does_not() {
        for class in [
            ModelErrorClass::Auth,
            ModelErrorClass::QuotaExhausted,
            ModelErrorClass::UsageLimit,
            ModelErrorClass::ContextLimit,
        ] {
            assert!(native_suspends(class), "{class:?}");
        }
        for class in [
            ModelErrorClass::Network,
            ModelErrorClass::Server,
            ModelErrorClass::Permission,
            ModelErrorClass::Unknown,
        ] {
            assert!(!native_suspends(class), "{class:?}");
        }
    }

    #[test]
    fn only_provider_side_hiccups_are_waited_out() {
        for class in [
            ModelErrorClass::Network,
            ModelErrorClass::Timeout,
            ModelErrorClass::Overloaded,
            ModelErrorClass::Server,
        ] {
            assert!(native_transient(class), "{class:?}");
        }
        for class in [
            ModelErrorClass::QuotaExhausted,
            ModelErrorClass::UsageLimit,
            ModelErrorClass::ContextLimit,
            ModelErrorClass::Auth,
            ModelErrorClass::Unknown,
        ] {
            assert!(!native_transient(class), "{class:?}");
        }
    }
}
