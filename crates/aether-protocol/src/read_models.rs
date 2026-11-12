//! Protocol 26 read models: agent and transcript pages and the usage
//! breakdown (docs/protocol.md). Each command checks its own bounds here, so
//! the daemon and a client agree on what a request may ask for.

use aether_core::id::{AgentId, RunId};
use aether_core::model::Usage;
use aether_core::time::Timestamp;
use serde::{Deserialize, Serialize};

/// `list_agents_page`: rows per page when `limit` is absent, and the most a page holds.
pub const AGENTS_PAGE_DEFAULT: u32 = 200;
pub const AGENTS_PAGE_MAX: u32 = 500;
/// `get_transcript_page`: entries per page when `limit` is absent, and the most a page holds.
pub const TRANSCRIPT_PAGE_DEFAULT: u32 = 200;
pub const TRANSCRIPT_PAGE_MAX: u32 = 1000;
/// `usage_breakdown`: the most rows a reply holds, and the count when `limit` is absent.
pub const USAGE_ROWS_MAX: u32 = 1000;

/// The page size a request asked for, or why it is out of `1..=max`.
pub fn page_limit(asked: Option<u32>, default: u32, max: u32) -> Result<u32, String> {
    debug_assert!(
        (1..=max).contains(&default),
        "a default inside its own bounds"
    );
    let limit = asked.unwrap_or(default);
    if !(1..=max).contains(&limit) {
        return Err(format!("limit must be between 1 and {max}, got {limit}"));
    }
    Ok(limit)
}

/// Which invocations `usage_breakdown` sums. Every field given narrows the
/// set; none sums the whole fleet.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    /// This agent's own invocations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    /// This agent's and every descendant's invocations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtree: Option<AgentId>,
    /// Agents whose role starts with this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_prefix: Option<String>,
}

/// What each `usage_breakdown` row is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageGroup {
    /// `provider/model` as the invocation recorded it.
    Model,
    Provider,
    /// The invoking agent's role.
    Role,
    /// The UTC date the invocation completed, `YYYY-MM-DD`.
    Day,
    Run,
}

/// One group's spend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageRow {
    pub key: String,
    pub usage: Usage,
    pub invocations: u64,
}

/// Reply to `usage_breakdown`: rows ordered by `key`, and the total over
/// every invocation in scope, those of rows cut by `limit` included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageBreakdown {
    pub rows: Vec<UsageRow>,
    pub total: Usage,
    /// More groups matched than `limit` let through.
    pub truncated: bool,
}

/// Check a `usage_breakdown` request: its bounds, and a window that runs forwards.
pub fn usage_request(
    scope: &UsageScope,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    limit: Option<u32>,
) -> Result<u32, String> {
    if scope.role_prefix.as_deref().is_some_and(str::is_empty) {
        return Err("role_prefix must not be empty".into());
    }
    if let (Some(since), Some(until)) = (since, until)
        && since > until
    {
        return Err(format!("since ({since}) is after until ({until})"));
    }
    page_limit(limit, USAGE_ROWS_MAX, USAGE_ROWS_MAX)
}

/// Check a `get_transcript_page` cursor: `after` and `before` never together.
pub fn transcript_request(
    after: Option<u64>,
    before: Option<u64>,
    limit: Option<u32>,
) -> Result<u32, String> {
    if after.is_some() && before.is_some() {
        return Err("after and before are mutually exclusive".into());
    }
    page_limit(limit, TRANSCRIPT_PAGE_DEFAULT, TRANSCRIPT_PAGE_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_limit_defaults_and_refuses_what_is_out_of_bounds() {
        assert_eq!(page_limit(None, 200, 500), Ok(200));
        assert_eq!(page_limit(Some(1), 200, 500), Ok(1));
        assert_eq!(page_limit(Some(500), 200, 500), Ok(500));
        for refused in [0, 501, u32::MAX] {
            let error = page_limit(Some(refused), 200, 500).unwrap_err();
            assert!(error.contains("between 1 and 500"), "{error}");
        }
    }

    #[test]
    fn a_transcript_cursor_is_after_or_before_never_both() {
        assert_eq!(transcript_request(Some(3), None, None), Ok(200));
        assert_eq!(transcript_request(None, Some(3), Some(1000)), Ok(1000));
        assert!(transcript_request(Some(1), Some(9), None).is_err());
        assert!(transcript_request(None, None, Some(1001)).is_err());
    }

    #[test]
    fn a_usage_window_runs_forwards_and_a_prefix_says_something() {
        let early = "2026-01-01T00:00:00Z".parse().unwrap();
        let late = "2026-02-01T00:00:00Z".parse().unwrap();
        let scope = UsageScope::default();
        assert_eq!(
            usage_request(&scope, Some(early), Some(late), None),
            Ok(1000)
        );
        assert!(usage_request(&scope, Some(late), Some(early), None).is_err());
        let empty = UsageScope {
            role_prefix: Some(String::new()),
            ..UsageScope::default()
        };
        assert!(usage_request(&empty, None, None, None).is_err());
        assert!(usage_request(&scope, None, None, Some(0)).is_err());
    }
}
