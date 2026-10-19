# GitHub watcher

Aether's opt-in watcher polls selected GitHub.com repositories and delivers
matching requests to an existing Aether agent. The daemon owns the worker:
closing the TUI does not stop polling. No model is used to search or classify
GitHub activity; only accepted requests cause agent work.

## Setup

1. Install `gh` and `git` in the daemon's environment. Run `gh auth login` with
   repository read and collaborator-permission access. Aether does not save a
   second token. `git` uses the same `gh` credential helper for private clones.
2. Add a repository with the CLI. This selects the repository and starts
   polling. Mention
   `@Aether` in a new issue, PR, comment, or submitted review. Matching is
   case insensitive. Aether creates one agent and checkout per issue/PR thread;
   later mentions on that thread reach the same live agent.
3. Inspect delivery counts and errors with the `status` command.

```sh
aether github-watcher add owner/repo
aether github-watcher status
aether github-watcher status --json

# Optional: route to an existing agent or use a custom per-thread template.
aether github-watcher add owner/repo --agent <agt_id>
aether github-watcher add owner/repo --spawn-role worker --spawn-objective "Review the mentioning thread."

# Optional: only these people may dispatch; each still needs write access
# unless it is an operator login.
aether github-watcher add owner/repo --allowed-users octocat,monalisa
aether github-watcher allowed-users owner/repo octocat monalisa
aether github-watcher allowed-users owner/repo --clear

# Optional: these people's own requests carry your authority. Read
# "Operator logins" below before setting it.
aether github-watcher operator-users your-login
aether github-watcher operator-users --clear

# Disable one repository without forgetting its settings, or stop all polling.
aether github-watcher disable owner/repo
aether github-watcher disable
aether github-watcher enable owner/repo
aether github-watcher remove owner/repo
```

Adding a new repository starts global polling. `add` also updates an existing
repository's target and selects it; dispatch flags replace its mode, while
omitting them preserves its mode and the current global switch. Likewise
`--allowed-users` replaces an existing repository's allowlist, and omitting it
keeps the current one. `allowed-users` changes only the allowlist, without
selecting a disabled repository. `operator-users` replaces the watcher-wide
list of operator logins and changes nothing else. Changing a target, an
allowlist, or the operator logins is an operator decision, not something a
GitHub comment or an agent can request.

The TUI has no watcher view; configure it with the CLI above, from any
terminal. `--agent <agt_id>` routes a repository to a fixed agent, and the
`--spawn-*` flags set a custom per-thread template.

## Dispatch modes

Each repository picks exactly one:

- **Automatic** (the default): on the first accepted request for a thread,
  Aether clones that repository into a private, managed checkout beside the
  daemon database and starts a new agent there. Each issue/PR number has its
  own durable agent mapping. A new request resumes a live thread agent; a
  paused or terminal one gets a fresh checkout generation, so old and new
  agents never share a writable tree. Prior generations stay on disk for
  review; uncommitted changes are not copied into replacements. Agents use the
  daemon's default model, approval policy, and network policy. Cloning or agent
  creation errors remain visible and retryable; the request is not marked
  handled.
- **Fixed** (`--agent <agt_id>`): every accepted request goes to one
  existing, operator-chosen agent. Create or select that agent first, with the
  intended repository workspace, model, scope, and instructions; the watcher
  does not clone repositories or widen its scope. If that agent later pauses
  or goes terminal, delivery keeps failing visibly until an operator repoints
  it - fixed dispatch never repoints itself.
- **Thread** (`--spawn-role <role> --spawn-objective <text>`, plus optional
  `--spawn-model`/`--spawn-effort`/`--spawn-token-budget`/`--spawn-cost-budget`/
  `--spawn-max-turns`/`--spawn-backend`/`--spawn-network`): the watcher spawns
  a fresh, bounded agent from this template the first time a given issue/PR
  number is accepted, and resumes that same agent for later mentions on the
  same thread. If that agent later pauses or goes terminal, the next accepted
  mention on that thread spawns a replacement instead of retrying forever
  against a dead target. The template may not set an absolute deadline or be
  long-lived: a thread's spawned agent must end so a later mention can resume
  or replace it. This custom mode uses its configured scope; it does not create
  a managed checkout.

## Matching and configuration

```sh
aether github-watcher configure \
  --mention-regex '(?i)(?:^|[^a-z0-9_@])@aether(?:$|[^a-z0-9_-])' \
  --label-regex '(?i)aether' \
  --poll-interval-secs 120
```

The default mention pattern is case insensitive and does not address
`@aether-bot` or email-like `me@aether`. The label pattern is a case-insensitive
substring search: `aether`, `aether-review`, and other labels containing that
word match. To restrict it, use `(?i)^aether(-review)?$` instead. Expressions
use Rust's `regex` syntax, not shell patterns; quote them in a shell.

The interval accepts 60–86400 seconds, with a default of 120. Up to 32
repositories can be configured. Invalid expressions and repository names are
rejected before settings change. Whole-configuration saves compare a revision
number, so a stale client cannot silently overwrite another operator's edits.
Refresh after a conflict before retrying.

Settings and polling state belong to the daemon's persistent store, not
`config.toml` or the TUI's preferences. The CLI uses the same daemon API as
every other client.

## Trust boundary

Only explicitly selected repositories and operator-chosen dispatch modes participate.
A mention's author is checked; a label request is attributed to the actor who
applied the label, not the issue's author. Matching text alone is not sufficient
authorization. Accepted requests remain `ExternalTrigger` inbox entries; they
are never recast as user messages. They are external content unless the
watcher verified them as an operator's own request (see
[Operator logins](#operator-logins)), and no login is an operator unless you
name it.

By default any human collaborator with write access may dispatch (see
[Coverage, restart safety, and limits](#coverage-restart-safety-and-limits)),
and so may any human account named in the watcher's `operator_users`, with or
without write access (see [Operator logins](#operator-logins)). A repository's
optional `allowed_users` narrows both to named GitHub logins:

```json
{ "repository": "owner/repo", "enabled": true,
  "dispatch": { "mode": "automatic" },
  "allowed_users": ["octocat", "monalisa"] }
```

The allowlist narrows the permission check and never replaces it. A listed
login that is not an operator login must still hold `admin`, `maintain`, or
`write` access, so dropping someone's access on GitHub revokes them even while
they stay listed. Anyone not listed, operator logins included, is ignored
exactly like an unauthorized requester: no delivery, the repository's
`ignored` counter increases, and no collaborator-permission request is spent
on them. For a label, the check applies to the actor who
applied it. Logins match case-insensitively; the daemon stores them lowercase
without duplicates. Each must be a GitHub login (1-39 letters, digits, or
single hyphens, not starting or ending with a hyphen), and at most 100
distinct logins fit one repository; repeats, in any case, count once. An empty list is rejected rather than read as "allow everyone";
omit the field (or run `allowed-users owner/repo --clear`) for the default.
Allowlists are set only through the operator-only watcher configuration, like
every other watcher setting.

### Operator logins

By default every GitHub request reaches an agent as external content: the
agent is told it is not an operator instruction, whoever wrote it. The
watcher's optional `operator_users` names GitHub logins whose own requests
instead carry the local operator's authority:

```sh
aether github-watcher operator-users your-login octocat   # replaces the list
aether github-watcher operator-users --clear             # nobody again
aether github-watcher status                             # shows "Operator logins"
```

```json
{ "enabled": true, "operator_users": ["your-login", "octocat"],
  "repositories": [{ "repository": "owner/repo", "enabled": true,
                     "dispatch": { "mode": "automatic" } }] }
```

**A listed account is as powerful as you are.** Anyone who controls it, or a
token that can post as it, can instruct your agents with your authority on
every watched repository its allowlist admits, whether or not the account has
write access there. Losing the account is losing operator authority.
The list is empty unless you set it; list only accounts you would hand your
terminal to, and clear the list the moment one is in doubt. The list holds
login names, not account ids: remove a login before its account is renamed or
deleted, because someone else can then take the name.

A request carries operator authority only when all of this holds in the same
poll. Anything short of it is delivered as before, as external content:

- The requester's login is in `operator_users` (matched case-insensitively).
- The requester may dispatch on that repository: a human account, listed in
  the repository's `allowed_users` when one is set. An operator login needs no
  write access to the repository, so no collaborator-permission lookup is spent
  on it; a bot never dispatches, whatever its login. An operator login the
  allowlist excludes, or a bot, is not delivered at all, exactly like anyone
  else, and leaves a [notice](#ignored-requests).
- The delivered text is that login's own and nobody else's. GitHub must name
  that account as the author and report that the text was never edited, or
  was last edited by its author. The same holds for the title of an issue or
  pull request, which is part of the delivered text. Collaborators with write
  access can edit other people's comments and titles; such text is external.
- For a label request the text is the issue's, not the labeller's. The
  issue's author must be an operator login too and pass the same dispatch
  and authorship checks. Otherwise the request is external, and the agent is
  told that operator `<login>` added the label but the text is by `<author>`.
  The label's name is never the operator's words: anyone with write access
  can create or rename a label, the authorship check does not cover it, and
  the trust sentence of such a request says so.

Authorship is read from GitHub's GraphQL API (`author`, `editor`,
`lastEditedAt`, and the last title rename) with the same `gh` credential, for
exactly the texts that could gain authority: one request per stream for up to
100 texts, so normally at most two per repository pass, and none when no
operator login asked or the list is unset. The text GitHub
reports there must equal the text being delivered, so an edit between the two
reads cannot slip through. When GitHub does not say who last edited the text,
it is external and the agent is told why. The same holds when GitHub names an
editor or title renamer whose login the watcher cannot verify, such as an
enterprise-managed login with an underscore. When the lookup itself fails, the
stream's undecided requests are kept for the next poll and the error shows in
`status`; a request is never downgraded or upgraded on a failed lookup. A
credential that cannot use the GraphQL API therefore holds back a stream
whenever an operator login asks, until it can or the list is cleared.

An agent sees the difference in three daemon-set places, none of which a
GitHub user can write: the trigger's `operator` field (`true` or `false`), its
`trust` sentence, and the heading of the inbox line, which reads
`External trigger from github [authenticated operator instruction]:` only for
a verified operator request. Words inside an issue, comment, or review never
change any of them, whatever they claim.

Operator authority is authority over what the agent is asked to do. It does
not change how the agent runs:

- It answers no pending approval prompt and no `ask_user` question. Those
  still need the local operator.
- It changes no approval policy, network policy, sandbox, scope, or budget.
- It is no way to change watcher settings: no GitHub request and no agent
  tool can set `operator_users`. Only the local operator can, through the CLI
  or the authenticated local API.
- It does not reach an agent the request would not have reached anyway:
  dispatch targets are unchanged.

An agent processes requests within its existing scope and approval policy.
Repository collaborators can still submit hostile code or text. Approval mode
alone is not a filesystem sandbox: use an appropriately isolated workspace or
container for untrusted repositories. Do not point the watcher at a privileged
agent merely because a repository is private.

### Ignored requests

An ignored request is counted in the repository's `ignored` total in
`status`. Most are silent, but two kinds leave a one-line notice naming the
repository, thread, login and reason: any request by an operator login (for
example one the allowlist excludes), and any request by a human refused for
lack of write access. The notice never carries the request's text and never
starts a turn:

- When the thread has a live agent (the fixed target, or the thread's own
  agent under thread or automatic dispatch), it is a note on that agent's
  transcript. The TUI and the phone's activity feed show it, and the agent's
  next turn reads it in its note digest.
- Otherwise it is a `github.watcher.ignored` event in the daemon's event log,
  with `repository`, `number`, `requester`, `reason`, `url` and `notice`:
  `aether events --type custom` lists it. No agent or checkout is created for
  an ignored request.

Each thread gets at most one notice per reason, however many requests are
ignored there for it. Bots and logins the allowlist excludes, other than
operator logins, stay silent apart from the count.

## API

Rust clients call `github_watcher()` and
`configure_github_watcher(config, expected_revision)`. Wire commands are
`get_github_watcher` and `configure_github_watcher`; both return a
`github_watcher` reply containing configuration, revision, and per-repository
polling health. Watcher configuration is operator-only. Protocol v17 adds the
optional watcher-level `operator_users` field and serves v16 clients. A v16
client cannot represent the list, and saving without it would silently take
operator authority away from every listed login. So while the list is set,
v16 watcher reads return a clear upgrade error and v16 watcher saves are
refused; a v16 client cannot set the list either. Without it, v16 clients read
and save watcher settings unchanged, allowlists included, and their other
commands always remain available. v15 clients are outside the served window.

Every GitHub trigger's payload now carries `operator` (a boolean) next to
`trust`. The inbox item itself has a typed `operator` flag that is written
only when set; an older client ignores it and reads the item as an ordinary
external trigger.

## Downgrading to an older daemon

A v16 or older daemon cannot decode a persisted `operator_users` field: it
refuses the stored watcher settings instead of running without the list.
Before downgrading, run `aether github-watcher operator-users --clear` and
check that `aether github-watcher status --json` shows no `operator_users`.
Triggers already delivered keep their `operator` flag in the store; an older
daemon ignores it and shows them to agents as external content.

A v14 or older daemon cannot decode a persisted `allowed_users` field. Before
downgrading that far, clear every allowlist with the current CLI
(`aether github-watcher allowed-users owner/repo --clear`) and check that
`aether github-watcher status --json` shows no `allowed_users` entries.

Before replacing a v14 daemon with v13, also use the CLI to remove every
automatic repository (`aether github-watcher remove owner/repo`) or retarget
each one to an existing agent
(`aether github-watcher add owner/repo --agent <agt_id>`). Check
`aether github-watcher status --json` for any remaining
`"mode": "automatic"` entries. A v13 daemon cannot decode that persisted
dispatch mode; globally disabling the watcher leaves the entries in place.
Removing or retargeting a route preserves the managed checkouts and agent
history for later inspection.

## Coverage, restart safety, and limits

On activation (including re-enabling), a repository starts from **now**, not
historical backfill. The ordinary stream polls repository issues (GitHub's
endpoint includes PRs), their current matching label events, and issue/PR
conversation comments. Review bodies are swept on open PRs and recently updated
PRs; repository-wide inline-comment updates discover closed PRs too. Only
submitted reviews authorize review text or inline comments. Newly submitted
reviews also read the PR's inline history, so draft comments with older edit
timestamps are not lost when published.

Issue/PR body mentions are accepted only for threads created since activation.
An older issue's `updated_at` can change when an outsider merely comments; it
cannot prove that a maintainer issued a new instruction. For an older thread,
post a new mentioning comment or apply a matching label. Review bodies likewise
must have been submitted since activation; GitHub supplies no review-body edit
timestamp. Body-only edits on long-closed, otherwise inactive PRs are not
guaranteed discovery. These restrictions avoid pretending that GitHub supplied
provenance it does not expose.

Patterns run on raw Markdown (including code spans and fences), not a rendered
Markdown tree. Mentions are content-deduplicated: repeating the exact same text
on one object does not create new work; a changed mentioning body can. Labels
use the latest application event for each exact currently present matching
label, deduplicated by event ID, independent of subsequent issue edits. The
payload includes the label name and the actor who applied it.

Authorization requires a human GitHub account with `admin`, `maintain`, or
`write` permission (or GitHub's explicit push permission), or one named in
`operator_users`. Bots never dispatch, and read/triage access or
non-collaborators cannot unless listed in `operator_users`. A repository's
`allowed_users`, when set, is checked first and only narrows this further, for
operator logins too. Permission lookup failures are
visible retryable errors, not authorization. Watcher replies are instructed to
include `<!-- aether:github-reply -->`; mentioning bodies with that marker are
ignored. Explicit human-applied labels still work on marked threads. A durable
limit of **20 deliveries per thread per hour** additionally bounds feedback
loops even when an agent uses a human's GitHub credentials. Excess requests
remain undecided and retry later, with a visible status error.

Inbox delivery, counters, and the dedup decision commit in one SQLite
transaction. For automatic dispatch, creation of the new agent and its first
trigger are in that same transaction: a crash cannot leave a delivered request
without its marker, or start a new agent before its request arrives.
Markers persist without the short retention of generic client idempotency
keys; storage grows with decided objects/content revisions. Re-enabling or
retargeting does not erase them. Under fixed dispatch, a paused, terminal, or
missing target receives no new delivery; failures keep the request available
for retry, and the watcher does not complete tasks itself or automatically
unpause agents. Under thread dispatch, a paused or terminal per-thread agent
is replaced by a freshly spawned one on the next accepted mention instead of
blocking delivery.

Each repository pass allows at most 60 HTTP requests, 10 pages / 1,000 items per
listing, 30 PRs in a review sweep, and 100 decisions per stream. The
authorship lookups for operator logins count against the same 60. These are
intentional bounded-work limits, not claims of unlimited repository coverage.
Oversized listings/sweeps fail visibly without advancing that stream's cursor.
The ordinary and review cursors are independent: an oversized review sweep does
not prevent issue/comment delivery. A repeatedly oversized ordinary listing
needs reduced activity or a future resumable-pagination implementation; this
version never silently skips its unprocessed tail.

API requests time out after 30 seconds and have bounded captured output (16 MiB
stdout, 64 KiB stderr). Poll errors exponentially back off up to one hour,
never polling faster than a longer configured interval. Backoff currently
applies to the whole repository pass, so persistent review errors can also
delay otherwise working issue/comment polling.
GitHub's `Retry-After` and rate-limit reset headers can extend the wait (up to
one day); low remaining quota also pauses ingestion. That cooldown applies to
all repositories and survives daemon restarts. Status records last poll,
last successful whole pass, next poll, counters, and the latest error; logs
carry failures as well. Managed clones time out after 90 seconds, use
command-scoped credentials, and keep authentication failures visible. Polling
never posts to GitHub on its own.

A cancelled clone removes its incomplete `.clone-*` directory after Git exits.
An abrupt daemon or host crash can interrupt that cleanup. If disk usage grows,
stop the daemon and remove only `.clone-*` directories beneath the managed
checkout root beside `aether.db`; leave numbered `g0`, `g1`, and later
generation directories in place because agents may have work there.
