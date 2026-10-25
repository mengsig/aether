//! Fetch, match, authorize. Endpoint paths are assembled only from validated
//! repository names and numeric IDs, never URLs supplied in issue text.
use super::{
    RepositoryState, Runtime, Settings,
    source::{Error, Source},
    standing::{self, Authorship, Standing},
};
use aether_core::{
    github::{GithubRepositoryConfig, GithubWatcherConfig},
    time::Timestamp,
};
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};

pub(super) const MAX_REQUESTS: usize = 60;
const MAX_PAGES: usize = 10;
const MAX_DECISIONS: usize = 100;

/// A GitHub account as the REST API names it on an object.
pub(super) struct Author {
    pub login: String,
    pub bot: bool,
}
pub(super) struct Candidate {
    pub kind: &'static str,
    pub id: u64,
    pub number: u64,
    pub login: String,
    pub bot: bool,
    pub body: String,
    pub url: String,
    pub at: Timestamp,
    pub label: Option<String>,
    /// Who wrote `body`: the requester, except that a label's text is the
    /// issue's. `None` when GitHub names no usable author for that issue.
    pub author: Option<Author>,
    /// GraphQL id of the object holding `body`, for the authorship lookup.
    pub node_id: Option<String>,
}
impl Candidate {
    pub fn key(&self, repo: &str) -> String {
        let digest = Sha256::digest(format!(
            "{repo}\0{}\0{}\0{}",
            self.kind,
            self.id,
            if self.kind == "label" { "" } else { &self.body }
        ));
        format!("github.watcher.decision:{}", hex::encode(digest))
    }
}
/// One phase's GitHub requests, each phase with its own budget.
pub(super) struct Pass<'a, S> {
    source: &'a S,
    repo: &'a str,
    pub requests: usize,
    /// Requests this pass may make so far; at most [`MAX_REQUESTS`].
    pub limit: usize,
    /// Whether `limit` has refused a request.
    pub refused: bool,
}
impl<'a, S: Source> Pass<'a, S> {
    pub fn new(source: &'a S, repo: &'a str) -> Self {
        Self {
            source,
            repo,
            requests: 0,
            limit: MAX_REQUESTS,
            refused: false,
        }
    }
    fn spend(&mut self) -> Result<(), Error> {
        debug_assert!(
            self.limit <= MAX_REQUESTS,
            "a pass never exceeds 60 requests"
        );
        if self.requests >= self.limit {
            self.refused = true;
            return Err(Error::new(format!(
                "GitHub request budget exhausted ({}); cursor retained, reduce repository activity or poll backlog",
                self.limit
            )));
        }
        self.requests += 1;
        Ok(())
    }
    async fn get(&mut self, endpoint: &str) -> Result<Value, Error> {
        self.spend()?;
        self.source
            .get(&format!("repos/{}/{endpoint}", self.repo))
            .await
    }
    async fn graphql(&mut self, request: &Value) -> Result<Value, Error> {
        self.spend()?;
        self.source.graphql(request).await
    }
    pub async fn list(&mut self, endpoint: &str) -> Result<Vec<Value>, Error> {
        let separator = if endpoint.contains('?') { '&' } else { '?' };
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let value = self
                .get(&format!("{endpoint}{separator}per_page=100&page={page}"))
                .await?;
            let page = value
                .as_array()
                .ok_or_else(|| Error::new("GitHub listing was not an array"))?;
            items.extend(page.iter().cloned());
            if page.len() < 100 {
                return Ok(items);
            }
        }
        Err(Error::new(
            "GitHub listing exceeds 1000 items; cursor retained rather than silently dropping results",
        ))
    }
}
pub(super) fn string<'a>(v: &'a Value, field: &str) -> Result<&'a str, Error> {
    v.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new(format!("GitHub item missing string {field}")))
}
pub(super) fn number(v: &Value, field: &str) -> Result<u64, Error> {
    v.get(field)
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::new(format!("GitHub item missing positive {field}")))
}
pub(super) fn timestamp(v: &Value, field: &str) -> Result<Timestamp, Error> {
    string(v, field)?
        .parse()
        .map_err(|e| Error::new(format!("invalid GitHub {field}: {e}")))
}
pub(super) fn thread(v: &Value, field: &str) -> Result<u64, Error> {
    string(v, field)?
        .rsplit('/')
        .next()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::new(format!("invalid GitHub {field} thread number")))
}
pub(super) fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 100
        && login
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-[]".contains(&c))
}
fn actor(user: &Value) -> Result<Author, Error> {
    let login = string(user, "login")?.to_owned();
    if !valid_login(&login) {
        return Err(Error::new("invalid GitHub actor login"));
    }
    Ok(Author {
        bot: string(user, "type")? != "User" || login.ends_with("[bot]"),
        login,
    })
}
/// The object's GraphQL id, when it is one a lookup can safely name.
fn node_id(v: &Value) -> Option<String> {
    v.get("node_id")
        .and_then(Value::as_str)
        .filter(|id| {
            (1..=128).contains(&id.len())
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-=".contains(&c))
        })
        .map(str::to_owned)
}
pub(super) fn candidate(
    v: &Value,
    kind: &'static str,
    n: u64,
    body: String,
    author: &str,
    time: &str,
) -> Result<Candidate, Error> {
    let requester = actor(&v[author])?;
    Ok(Candidate {
        kind,
        id: number(v, "id")?,
        number: n,
        bot: requester.bot,
        login: requester.login.clone(),
        body,
        label: None,
        url: v
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        at: timestamp(v, time)?,
        author: Some(requester),
        node_id: node_id(v),
    })
}
/// A label request: `event`'s actor asks, and the text is `issue`'s.
fn label_candidate(
    event: &Value,
    issue: &Value,
    n: u64,
    body: &str,
    name: &str,
) -> Result<Candidate, Error> {
    let mut c = candidate(event, "label", n, body.to_owned(), "actor", "created_at")?;
    c.url = string(issue, "html_url")?.into();
    c.label = Some(name.into());
    // An unusable author only withholds standing; it never fails the pass.
    c.author = actor(&issue["user"]).ok();
    c.node_id = node_id(issue);
    Ok(c)
}

/// Issue/comment polling: issues, labels and conversation comments.
pub(super) async fn scan(
    runtime: &Runtime,
    source: &impl Source,
    settings: &Settings,
    repo: &GithubRepositoryConfig,
    state: &RepositoryState,
    at: Timestamp,
) -> Result<(), Error> {
    let mention = Regex::new(&settings.config.mention_regex)
        .map_err(|e| Error::new(format!("mention regex: {e}")))?;
    let label = Regex::new(&settings.config.label_regex)
        .map_err(|e| Error::new(format!("label regex: {e}")))?;
    let since = (state.cursor - chrono::Duration::minutes(5))
        .max(state.baseline)
        .format("%Y-%m-%dT%H:%M:%SZ");
    let mut pass = Pass::new(source, &repo.repository);
    let mut candidates = Vec::new();
    let issues = pass
        .list(&format!(
            "issues?state=all&sort=updated&direction=asc&since={since}"
        ))
        .await?;
    for issue in issues {
        let n = number(&issue, "number")?;
        let body = format!(
            "{}\n{}",
            string(&issue, "title")?,
            issue["body"].as_str().unwrap_or("")
        );
        if mention.is_match(&body) && timestamp(&issue, "created_at")? >= state.baseline {
            candidates.push(candidate(
                &issue,
                "issue",
                n,
                body.clone(),
                "user",
                "updated_at",
            )?);
        }
        let labels: BTreeSet<&str> = issue["labels"]
            .as_array()
            .ok_or_else(|| Error::new("GitHub issue missing labels array"))?
            .iter()
            .filter_map(|l| l["name"].as_str())
            .filter(|s| label.is_match(s))
            .collect();
        if !labels.is_empty() {
            let mut latest = HashMap::new();
            for event in pass.list(&format!("issues/{n}/events")).await? {
                if event["event"] != "labeled" {
                    continue;
                }
                let name = string(&event["label"], "name")?;
                if !labels.contains(name) {
                    continue;
                }
                // The event actor applied this exact currently-present label.
                let c = label_candidate(&event, &issue, n, &body, name)?;
                let entry = latest.entry(name.to_owned()).or_insert(c);
                if (timestamp(&event, "created_at")?, number(&event, "id")?) > (entry.at, entry.id)
                {
                    *entry = label_candidate(&event, &issue, n, &body, name)?;
                }
            }
            candidates.extend(latest.into_values());
        }
    }
    for c in pass
        .list(&format!(
            "issues/comments?sort=updated&direction=asc&since={since}"
        ))
        .await?
    {
        let body = c["body"].as_str().unwrap_or("");
        if mention.is_match(body) {
            candidates.push(candidate(
                &c,
                "comment",
                thread(&c, "issue_url")?,
                body.into(),
                "user",
                "updated_at",
            )?);
        }
    }
    dispatch(runtime, &mut pass, settings, repo, state, candidates, at).await?;
    runtime
        .github_advance_cursor(settings.revision, &repo.repository, at)
        .map_err(|e| Error::new(e.to_string()))
}

/// Whether `login` holds admin, maintain or write access (or GitHub's explicit
/// push bit). A 404 means "not a collaborator"; other failures stay retryable.
async fn can_push(pass: &mut Pass<'_, impl Source>, login: &str) -> Result<bool, Error> {
    debug_assert!(!login.is_empty(), "candidates carry a validated login");
    let permission = match pass.get(&format!("collaborators/{login}/permission")).await {
        Ok(value) => value,
        Err(error) if error.status == Some(404) => return Ok(false),
        Err(error) => return Err(error),
    };
    if permission.is_null() {
        return Ok(false);
    }
    let role = string(&permission, "permission")?;
    if !matches!(
        role,
        "admin" | "maintain" | "write" | "triage" | "read" | "none"
    ) {
        return Err(Error::new(
            "unknown GitHub collaborator permission; request retained",
        ));
    }
    let push = &permission["user"]["permissions"]["push"];
    if !push.is_null() && !push.is_boolean() {
        return Err(Error::new("invalid GitHub collaborator push permission"));
    }
    Ok(matches!(role, "admin" | "maintain" | "write") || push == true)
}

/// Outside this pass's window, or one of Aether's own marked replies.
fn out_of_scope(candidate: &Candidate, state: &RepositoryState, at: Timestamp) -> bool {
    candidate.at < state.baseline
        || candidate.at > at
        || (candidate.kind != "label" && candidate.body.contains("<!-- aether:github-reply -->"))
}

fn decided(runtime: &Runtime, repo: &str, candidate: &Candidate) -> Result<bool, Error> {
    runtime
        .store
        .meta(&candidate.key(repo))
        .map(|decision| decision.is_some())
        .map_err(|e| Error::new(e.to_string()))
}

/// Whether `candidate` is an operator request whose authorship is not yet known.
fn lacks_authorship(
    settings: &Settings,
    repo: &GithubRepositoryConfig,
    candidate: &Candidate,
    known: &HashMap<String, Authorship>,
) -> bool {
    standing::wants_lookup(&settings.config, repo, candidate)
        && candidate
            .node_id
            .as_ref()
            .is_some_and(|id| !known.contains_key(id))
}

/// Who last edited the next undecided texts an operator login asked about,
/// in `pending`'s order: one GraphQL request for up to 100 of them.
async fn look_up_authorship(
    runtime: &Runtime,
    pass: &mut Pass<'_, impl Source>,
    settings: &Settings,
    repo: &GithubRepositoryConfig,
    pending: &[&Candidate],
    known: &mut HashMap<String, Authorship>,
) -> Result<(), Error> {
    let mut ids = BTreeSet::new();
    for candidate in pending {
        if ids.len() == standing::LOOKUP_BATCH {
            break;
        }
        if lacks_authorship(settings, repo, candidate, known)
            && !decided(runtime, &repo.repository, candidate)?
            && let Some(id) = &candidate.node_id
        {
            ids.insert(id.clone());
        }
    }
    debug_assert!(
        !ids.is_empty(),
        "called for a candidate that lacks authorship"
    );
    let ids: Vec<String> = ids.into_iter().collect();
    let response = pass.graphql(&standing::lookup_request(&ids)).await?;
    let facts = standing::parse_lookup(&response, &ids)?;
    debug_assert_eq!(facts.len(), ids.len(), "one answer per text asked about");
    known.extend(facts);
    Ok(())
}

/// Why a requester may not dispatch on a repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Bot,
    NotAllowlisted,
    NoWriteAccess,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Bot => "the account is a bot",
            Self::NotAllowlisted => "the login is not in the repository's allowed_users",
            Self::NoWriteAccess => "the login lacks repository write access",
        }
    }

    /// Stable name for the once-per-thread notice marker.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Bot => "bot",
            Self::NotAllowlisted => "not-allowlisted",
            Self::NoWriteAccess => "no-write-access",
        }
    }
}

/// What the watcher does with one undecided request.
pub(super) enum Verdict {
    Deliver(Standing),
    /// `notify` leaves a one-line notice instead of a silent count.
    Ignore {
        refusal: Refusal,
        notify: bool,
    },
}

/// Why `actor` may not dispatch on `repo`, or `None` when it may. Bots never
/// do; the allowlist comes first, so a login it rejects costs no request.
/// An operator login needs no write access; anyone else's is looked up once
/// per pass.
async fn refusal(
    pass: &mut Pass<'_, impl Source>,
    config: &GithubWatcherConfig,
    repo: &GithubRepositoryConfig,
    permissions: &mut HashMap<String, bool>,
    actor: &Author,
) -> Result<Option<Refusal>, Error> {
    debug_assert!(
        !actor.login.is_empty(),
        "candidates carry a validated login"
    );
    if actor.bot {
        return Ok(Some(Refusal::Bot));
    }
    if !repo.allows_requester(&actor.login) {
        return Ok(Some(Refusal::NotAllowlisted));
    }
    if config.is_operator(&actor.login) {
        return Ok(None);
    }
    let allowed = match permissions.get(&actor.login) {
        Some(allowed) => *allowed,
        None => {
            let allowed = can_push(pass, &actor.login).await?;
            permissions.insert(actor.login.clone(), allowed);
            allowed
        }
    };
    Ok((!allowed).then_some(Refusal::NoWriteAccess))
}

/// Whether an ignored request is reported rather than only counted: always
/// for an operator login, and for any human refused for lack of write access.
fn notifies(config: &GithubWatcherConfig, candidate: &Candidate, refusal: Refusal) -> bool {
    debug_assert!(!candidate.login.is_empty(), "candidates carry a login");
    debug_assert!(
        refusal != Refusal::NoWriteAccess || !candidate.bot,
        "a bot is refused as a bot before any write-access lookup"
    );
    config.is_operator(&candidate.login) || refusal == Refusal::NoWriteAccess
}

/// Standing of a request that may dispatch. The text's author must be an
/// operator login that may dispatch too; that costs no permission lookup.
async fn standing_of(
    pass: &mut Pass<'_, impl Source>,
    config: &GithubWatcherConfig,
    repo: &GithubRepositoryConfig,
    permissions: &mut HashMap<String, bool>,
    authorship: &HashMap<String, Authorship>,
    candidate: &Candidate,
) -> Result<Standing, Error> {
    debug_assert!(!candidate.bot, "a bot never dispatches");
    if !config.is_operator(&candidate.login) {
        return Ok(Standing::External);
    }
    let author_allowed = match &candidate.author {
        Some(author) if config.is_operator(&author.login) => {
            refusal(pass, config, repo, permissions, author)
                .await?
                .is_none()
        }
        _ => false,
    };
    let facts = candidate.node_id.as_ref().and_then(|id| authorship.get(id));
    Ok(standing::decide(
        config,
        candidate,
        true,
        author_allowed,
        facts,
    ))
}

pub(super) async fn dispatch(
    runtime: &Runtime,
    pass: &mut Pass<'_, impl Source>,
    settings: &Settings,
    repo: &GithubRepositoryConfig,
    state: &RepositoryState,
    mut candidates: Vec<Candidate>,
    at: Timestamp,
) -> Result<(), Error> {
    candidates.sort_by_key(|c| c.at);
    let pending: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| !out_of_scope(candidate, state, at))
        .collect();
    let mut authorship = HashMap::new();
    let mut permissions = HashMap::new();
    let mut decisions = 0;
    let mut deferred = None;
    for (next, candidate) in pending.iter().copied().enumerate() {
        if decided(runtime, &repo.repository, candidate)? {
            continue;
        }
        if decisions >= MAX_DECISIONS {
            return Err(Error::new(
                "GitHub decision budget exhausted (100); delivered items retained, remaining work retried",
            ));
        }
        // Looked up a batch at a time as the pass reaches them, so a long
        // backlog cannot spend the requests before a decision is saved.
        if lacks_authorship(settings, repo, candidate, &authorship) {
            look_up_authorship(
                runtime,
                pass,
                settings,
                repo,
                &pending[next..],
                &mut authorship,
            )
            .await?;
        }
        let requester = Author {
            login: candidate.login.clone(),
            bot: candidate.bot,
        };
        let config = &settings.config;
        let verdict = match refusal(pass, config, repo, &mut permissions, &requester).await? {
            None => Verdict::Deliver(
                standing_of(pass, config, repo, &mut permissions, &authorship, candidate).await?,
            ),
            Some(refusal) => Verdict::Ignore {
                refusal,
                notify: notifies(config, candidate, refusal),
            },
        };
        match runtime
            .github_decide(settings.revision, repo, candidate, verdict)
            .await
        {
            Ok(true) => decisions += 1,
            Ok(false) => {}
            Err(error) => {
                deferred.get_or_insert_with(|| Error::new(error.to_string()));
            }
        }
    }
    match deferred {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
