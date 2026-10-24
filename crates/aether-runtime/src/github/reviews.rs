//! The review sweep. Review bodies carry no edit timestamp, so a cycle reads
//! every open pull request and every one active since the review cursor, a
//! bounded number per pass, each pass resuming where the last one stopped.
use super::{
    RepositoryState, Runtime, Settings,
    phases::ReviewCycle,
    poll::{self, Candidate, MAX_REQUESTS, Pass, candidate, number, thread, timestamp},
    source::{Error, Source},
};
use aether_core::{github::GithubRepositoryConfig, time::Timestamp};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

/// Fewest requests one pull request costs: its reviews and inline history.
const PULL_REQUESTS: usize = 2;
/// Requests kept for dispatch's permission and authorship lookups.
const DISPATCH_RESERVE: usize = 10;

/// What the swept pull requests hold.
#[derive(Default)]
struct Reviews {
    candidates: Vec<Candidate>,
    /// Submission time of each submitted review read, by review id.
    submitted: HashMap<u64, Timestamp>,
    /// Whole inline histories of pull requests with a newly submitted review.
    inline: Vec<Value>,
}

impl Reviews {
    fn absorb(&mut self, other: Reviews) {
        self.candidates.extend(other.candidates);
        self.submitted.extend(other.submitted);
        self.inline.extend(other.inline);
    }
}

/// One pass of the review sweep: read up to the configured number of the
/// cycle's pending pull requests, oldest first, and dispatch their mentions.
/// Returns the cycle without them; an error keeps the stored position.
pub(super) async fn sweep(
    runtime: &Runtime,
    source: &impl Source,
    settings: &Settings,
    repo: &GithubRepositoryConfig,
    state: &RepositoryState,
    at: Timestamp,
) -> Result<ReviewCycle, Error> {
    let limit = runtime.config.github_review_sweep_prs;
    debug_assert!(limit > 0, "the per-pass limit is validated as positive");
    let mention = Regex::new(&settings.config.mention_regex)
        .map_err(|e| Error::new(format!("mention regex: {e}")))?;
    let since = (state.review_cursor - chrono::Duration::minutes(5))
        .max(state.baseline)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let mut pass = Pass::new(source, &repo.repository);
    pass.limit = MAX_REQUESTS - DISPATCH_RESERVE;
    let feed = pass
        .list(&format!(
            "pulls/comments?sort=updated&direction=asc&since={since}"
        ))
        .await?;
    let mut cycle = match &state.review.cycle {
        Some(cycle) => cycle.clone(),
        None => discover(&mut pass, &since, &feed, at).await?,
    };
    let mut read = Reviews::default();
    let mut swept = 0;
    for &n in cycle.pending.iter().take(limit) {
        if swept > 0 && pass.requests + PULL_REQUESTS > pass.limit {
            break;
        }
        let mut pull = Reviews::default();
        match read_pull(&mut pass, &mention, state, n, &mut pull).await {
            Ok(()) => read.absorb(pull),
            // Out of budget inside this PR: deliver those read, resume here next pass.
            Err(_) if swept > 0 && pass.refused => break,
            Err(error) => return Err(error),
        }
        swept += 1;
    }
    debug_assert!(swept <= limit, "a pass reads at most `limit` pull requests");
    pass.limit = MAX_REQUESTS;
    let chunk: BTreeSet<u64> = cycle.pending[..swept].iter().copied().collect();
    // Comments on pull requests outside this pass wait for their own turn.
    for c in feed {
        if chunk.contains(&thread(&c, "pull_request_url")?) {
            read.inline.push(c);
        }
    }
    let mut candidates = read.candidates;
    for c in &read.inline {
        candidates.extend(inline_candidate(c, &mention, &read.submitted)?);
    }
    poll::dispatch(runtime, &mut pass, settings, repo, state, candidates, at).await?;
    cycle.pending.drain(..swept);
    Ok(cycle)
}

/// A new cycle: every open pull request, and every one updated or given an
/// inline comment since the review cursor, in ascending order.
async fn discover(
    pass: &mut Pass<'_, impl Source>,
    since: &str,
    feed: &[Value],
    at: Timestamp,
) -> Result<ReviewCycle, Error> {
    let mut pulls = BTreeSet::new();
    for issue in pass
        .list(&format!(
            "issues?state=all&sort=updated&direction=asc&since={since}"
        ))
        .await?
    {
        if issue.get("pull_request").is_some() {
            pulls.insert(number(&issue, "number")?);
        }
    }
    for c in feed {
        pulls.insert(thread(c, "pull_request_url")?);
    }
    for p in pass
        .list("pulls?state=open&sort=updated&direction=desc")
        .await?
    {
        pulls.insert(number(&p, "number")?);
    }
    debug_assert!(!pulls.contains(&0), "GitHub numbers are positive");
    Ok(ReviewCycle {
        started_at: at,
        pending: pulls.into_iter().collect(),
    })
}

/// Read pull request `n`'s submitted reviews, and its whole inline history
/// when one of them is new.
async fn read_pull(
    pass: &mut Pass<'_, impl Source>,
    mention: &Regex,
    state: &RepositoryState,
    n: u64,
    read: &mut Reviews,
) -> Result<(), Error> {
    debug_assert!(n > 0, "GitHub numbers are positive");
    let fresh = state.review_cursor - chrono::Duration::minutes(5);
    let mut newly_submitted = false;
    for review in pass.list(&format!("pulls/{n}/reviews")).await? {
        if !matches!(
            review["state"].as_str(),
            Some("APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED")
        ) || review["submitted_at"].is_null()
        {
            continue;
        }
        let date = timestamp(&review, "submitted_at")?;
        newly_submitted |= date >= fresh;
        read.submitted.insert(number(&review, "id")?, date);
        let body = review["body"].as_str().unwrap_or("");
        if mention.is_match(body) {
            read.candidates.push(candidate(
                &review,
                "review",
                n,
                body.into(),
                "user",
                "submitted_at",
            )?);
        }
    }
    // Draft comment updated_at may predate submission and the repository
    // feed window. A newly submitted review requires its PR's full inline
    // history, regardless of the draft comment's creation/edit timestamp.
    if newly_submitted {
        read.inline
            .extend(pass.list(&format!("pulls/{n}/comments")).await?);
    }
    Ok(())
}

/// A mentioning inline comment, dated no earlier than its review's submission.
fn inline_candidate(
    c: &Value,
    mention: &Regex,
    submitted: &HashMap<u64, Timestamp>,
) -> Result<Option<Candidate>, Error> {
    let body = c["body"].as_str().unwrap_or("");
    if !mention.is_match(body) {
        return Ok(None);
    }
    let review = number(c, "pull_request_review_id")?;
    // Repository-wide feeds should omit drafts, but require submission as
    // defence in depth rather than authorizing unpublished review text.
    let Some(submitted_at) = submitted.get(&review) else {
        return Err(Error::new(
            "GitHub inline mention has no submitted parent review; review position retained for retry",
        ));
    };
    let mut candidate = candidate(
        c,
        "review_comment",
        thread(c, "pull_request_url")?,
        body.into(),
        "user",
        "updated_at",
    )?;
    candidate.at = candidate.at.max(*submitted_at);
    Ok(Some(candidate))
}
