# Changelog

What each release of Aether brings, newest first. Aether follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The section of a
release is also the text of its GitHub release.

## Unreleased

### New

- **Storage maintenance has a single configurable policy and visible decisions.**
  `[storage]` defaults to fourteen-day filesystem retention, with per-kind
  overrides, a one-hour grace for safely saved final delegated workspaces,
  and the latest three migration backups protected. `aether storage` and
  `aether storage gc --dry-run` show usage and reasons; the daemon sweeps at
  startup and periodically. Dirty, live, resumable and journal-referenced
  workspaces are protected. Logs gain age rotation and pause optional writes
  below the configured disk floor. New engine sessions have durable ownership
  records; deletion of Claude/Codex session logs is deferred, so these logs
  remain a known source of growth.

### Fixed

- Interrupted native tool calls now carry their cause in transcripts and on resume.
  Stopping or restarting the daemon, interrupting or cancelling an agent, and
  ending a stage no longer leave a synthetic operator denial uncorrected.
  Genuine approval denials retain their denial status.

- **Clipboard images paste into the terminal UI with Ctrl+V.** Ctrl+V prefers
  offered plain text, otherwise attaches a supported image; Alt+V and `/image`
  force an image. Text keeps the usual long-text placeholders and image-path
  detection. A truly empty terminal paste also tries the clipboard image;
  whitespace-only pastes and plain-mode input stay text. Reads happen in the
  background; an empty clipboard, missing helper or SSH session gets a one-line
  explanation without changing the draft. Existing custom key bindings are
  preserved.
- **Codex usage now carries a dollar estimate that says it is one.** A Codex
  (ChatGPT subscription) turn used to be booked as billed dollars at the
  OpenAI list price, so a run with any unreported part, such as an interrupted
  one, left a total with no complete cost and no estimate; tools that enforce
  dollar budgets, like Archon, then stopped the work. Codex turns are now
  estimated from the API model behind the slug, the way a subscription model
  without a price already was, and show as `≈ $X API-equivalent` next to a
  cost of `not reported`. Dollar caps on Codex agents are still refused. Older
  records keep the figures they were stored with.
- **A Claude Code agent that pauses and resumes no longer costs more than its
  receipt.** Claude Code reports a turn's tokens for that turn alone, but its
  dollars and per-model token counts for the whole session. When the turn's
  tokens fell short of what had streamed, Aether priced the difference again
  although the session's counts already covered it, and the extra carried
  over every resume. Aether now prices only streamed tokens those counts do
  not cover, and the note on an interrupted turn says whether the missing
  part was priced. An all-zero crash receipt keeps the last baseline;
  lower nonzero receipts leave a cost gap and become the next turn's
  baseline. Such a turn still counts its tokens at list price as a lower
  bound beside that gap.
- **The daemon no longer churns the disk while agents work.** Reading a long
  agent history made SQLite sort it in throwaway files under `/var/tmp`, tens
  of MB per read. With several agents running, that held host IO pressure
  high enough to stall other programs' syncs. Those sorts now stay in memory,
  and what the store keeps, and when it syncs, is unchanged.
- **Agent lists and a supervisor's history no longer sort everything they
  read.** Listing agents or tasks sorted every stored record on each call:
  the terminal's status poll every 2 seconds, each tree refresh, and
  orchestrators listing agents all paid it. Reading an agent's events together
  with its children's sorted the events' contents. Those sorts held a copy of
  everything they read, in memory or on disk. Full listings, live agents, and
  one run's agents and tasks now read in creation order from an index, and a
  supervisor's history sorts only event numbers. In a test store, listing
  10,000 agents peaked at 2.1 MB instead of 40.8 MB, and reading a supervisor's
  85,715 events with its children's took 0.2 s and 4.1 MB instead of 1.1 s and
  412 MB.
- **One run's agent tree no longer reads every task in the store.** Listing a
  run's tasks walked the whole tasks table. In a test store of 20,000 tasks
  that took 22 ms per tree; it now reads only that run's tasks, in 0.06 ms.
- **Stopping a background job no longer gives up when one of its processes
  is exiting.** On Linux before 7.2, a process that is exiting briefly hides
  its environment, and the search for a job's escaped processes stopped there
  with an error. Escaped processes later in the search stayed alive, and a
  restarted daemon kept retrying the cleanup instead of finishing it. A
  process exiting mid-search is now skipped like one that has gone.
- **An SSH remote no longer mistakes a finished send for one in flight.** If
  the client started another program just as a send ended, the send's outbox
  entry stayed locked until that program had started. Resolving the outbox on
  connect could skip the entry, and `/outbox` could refuse it as being sent
  by another process. Each entry is now unlocked as soon as its send ends.

### Before updating

- **Codex spend is an estimate, not billed dollars.** A Codex turn now
  records `cost_usd` of 0 and the list-price figure as `estimated_cost_usd`.
  Limits that read only billed dollars, a parent's `cost_budget` over Codex
  children and `[scheduler] run_budget_usd`, no longer count Codex turns; a
  `cost_budget` warns once that its cost is a lower bound, `run_budget_usd`
  stays silent. Use token budgets for Codex work. An
  Archon with `count_estimated_cost` off sees $0 for Codex.
- **Store schema 23.** Adds indexes that list agents and tasks in creation
  order, across the store and within one run. The per-run agents index
  replaces the old run index. The migration is one way: Aether saves the old
  database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)),
  and a binary that predates schema 23 refuses to open a migrated database.
  To go back, stop the daemon and restore that backup with the older binary;
  changes made since are lost.

## 1.0.8

Saying whether a memory helped or hurt a task now changes how it ranks in
recall, and you can see which memories each model call was given. The
terminal no longer shows older state after it reconnects to the daemon.

### New

- **You can record whether a memory helped or hurt a task.**
  `aether memory feedback record` keeps a verdict, where the evidence came
  from and a reason, and only for a memory the task's agents were given.
  Feedback can be retracted and listed, keyed retries never record it twice,
  and every change leaves an event. Agents and paired devices cannot use it;
  clients detect support with `memory_feedback`. Once a memory has helped or
  hurt verdicts from two tasks, they move its rank in recall: its score is
  scaled by 0.75 to 1.25, counting each task's latest operator or review
  verdict over the latest 20 tasks. Pinned memories are untouched, and
  feedback never hides a memory or changes how many are recalled.
  `aether memory feedback list` shows the current factor.
- **You can see which memories each model call was given.** The
  context record of every call now lists the memories it rendered, in the
  order the prompt shows them, for Claude Code and Codex turns too. The prompt
  itself is unchanged. Ask for a task or a single call with
  `aether memory context --task <id>` or `--invocation <id>`; clients can
  detect support with `memory_context`. This is the first step toward judging
  memories by how the work they informed turned out.

### Fixed

- **The terminal no longer goes back in time after a reconnect.** When the
  TUI or `--plain` reconnected to the same daemon, a late answer to a request
  sent before the outage could overwrite newer state: an agent that had
  failed showed as waiting again, or daemon-wide usage dropped to 0. Answers
  from the replaced connection are now dropped when the new connection reads
  the same thing again. Results of what you did still arrive, each settling
  only its own request, and after a switch to another daemon and back
  nothing from before the switch is applied.

### Before updating

- **Store schema 22.** Adds the table that keeps memory feedback. The
  migration is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)), and a binary that predates
  schema 22 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.

## 1.0.7

Lessons can wait for your approval before agents recall them. A Claude Code
or Codex answer cut off partway now looks the same live and after a reload,
however it was stopped. Costs Aether works out itself now match Anthropic's
bill for cached prompts.

### New

- **Memories can wait for review before agents recall them.** Propose an
  operator lesson with `aether memory add --proposed`, then accept it when
  the evidence holds or reject it with a reason. Proposed replacements leave
  the old lesson in use until acceptance, which refuses text already admitted
  in the same scope. Restoring a rejected proposal reopens review without
  putting it into recall, and retrying acceptance or rejection does not write
  duplicate events. Clients can detect support with `memory_proposals`;
  `memory list --proposed` shows the review queue.

### Fixed

- **What a Claude Code or Codex turn streamed before it failed stays on the
  record.** The terminal showed the partial answer and reasoning live, but a
  reload showed nothing: the failure was stored under a new id with no
  content. The failure record now keeps the uncommitted reasoning and text
  under the id they streamed under, so both views show them once. Nothing is
  billed for it, and it is never sent back to the model.
- **The same holds when such a turn is cancelled or interrupted.** What had
  streamed is kept as a cancelled failure under the id it streamed under, so
  a reload matches the live view. It costs nothing, and a resumed turn does
  not see it as something the agent said.
- **It also holds when the turn's own budget check, the engine adapter, or
  an earlier decision ends it.** A turn that crossed its budget mid-answer,
  an engine its adapter stopped (for example Codex reaching for its own
  delegation tool), and text an engine kept streaming after the agent had
  already chosen to wait or finish now keep what streamed the same way. An
  adapter's stop still shows its reason and reads as an interruption, not a
  cancel or a provider error. Text sent after a wait or a finish is shown
  after it and never changes the wait or the task's result.
- **An API reply cut short by a budget or deadline no longer shows twice.**
  The stop drew the half-written reply, then the copy kept for the next turn
  drew it again. Words still arriving when it stopped are shown once, after
  it. A Claude Code or Codex message that completes the cut reply is still
  shown.
- **1-hour prompt-cache writes are priced at their own rate.** Claude Code
  writes 1-hour cache entries, which Anthropic bills at twice the input
  price, but Aether priced every cache write at the 5-minute rate (1.25x).
  Costs and API-equivalent estimates that Aether works out itself were too
  low: across one Haiku benchmark, pricing the tokens that way came to 16%
  less than Claude Code reported.
  Usage now carries `cache_write_1h_tokens`, the 1-hour part of
  `cache_write_tokens`, and model prices carry `cache_write_1h_per_mtok`.
  Claude Code's own reported cost was already right and is unchanged. Older
  records, and engines that do not report the split, are priced as before.
- **Claude Sonnet 5.5 cache reads cost half what Aether charged.** Anthropic
  bills them at $0.10 per million tokens (0.05x input), as for Opus 5.5, but
  Aether used $0.20. A turn reading 200,000 cached tokens is now priced at
  $0.02 for those reads instead of $0.04.

### Before updating

- **Store schema 21.** Adds the column that keeps proposed memories out of
  recall. The migration is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)), and a binary that predates
  schema 21 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.

## 1.0.6

Operators can teach a role, list what they taught, and give one repository
a single memory however its agents name it. Incomplete costs now say why,
and several totals that wrongly read complete are now marked as floors.

### New

- **Operators can file memories by role.** The `role` scope reaches agents
  with that exact role across repositories. The `repository_role` scope
  reaches one role family in a repository, including dotted subroles such
  as `reviewer.security` and Archon roles such as
  `archon-stage:default/review`.
  Both repository scopes accept `repository` to target an existing agent's
  explicit repository name, or `root` for the workspace-derived id. Daemons
  advertise `operator_memory_roles` so clients can check support for the new
  scopes and the `repository` field.
- **Memories can be listed by who recorded them.** The `memories` query
  takes `recorded_by: "operator"` to list only what an operator of this
  daemon filed, locally or remotely, leaving out what agents remembered,
  repository notes and memories read out of an import.
  `aether memory list --recorded-by operator` does the same. Daemons
  advertise `memory_recorded_by`; an older daemon would ignore the field and
  list everything, so clients check for it first and the CLI refuses to ask
  one.
- **A client can name the checkout an agent's memory belongs to.**
  `create_agent` takes `repository_root`, an absolute path the daemon derives
  the repository id from, exactly as for an agent working there, so agents in
  worktrees share memory with sessions in the checkout itself. It cannot be
  combined with `scope.repository`, and children inherit the result.
  `aether run --repository-root` does the same. Daemons advertise
  `repository_root`; an older daemon would ignore the field, so clients check
  for it first and the Rust client refuses to send it to one.
- **An old repository name can be aliased to the id its checkout derives.**
  Memories filed under a bare name such as `archon` (what Archon up to 1.2.1
  sends) can now join the id direct sessions and `repository_root` use, such
  as `github.com/mengsig/archon`. `aether memory alias archon --root PATH`
  (or `--repository ID`) moves every memory filed under the old name, and
  agents still naming it read and write the new scope from then on;
  `--dry-run` reports what would move first, and `aether memory aliases`
  lists them. The move is one transaction and cannot be undone. Daemons
  advertise `repository_aliases`, and paired devices cannot register one.

### Fixed

- **Incomplete costs now say why.** A usage total that is a lower bound
  (`cost_complete: false`) now carries `cost_gap`: `unpriced` (tokens with
  no known price), `unreported` (the engine did not report all its usage),
  `interrupted` (work stopped after the prompt was sent, before its closing
  accounting) or `mixed`. Records stored earlier read as `unknown`. Complete
  totals omit the field, and older clients ignore it. Cancelling a submitted
  engine turn now leaves its cost a lower bound even when the engine sends a
  final receipt. Harness stops of in-flight requests also stay interrupted;
  a receipt received before the stop stays complete if no new request
  followed it. Terminal parks still allow final accounting before their
  deadline. An API stream that fails midway keeps what it saw as a
  floor instead of reading complete. Failures before launch still add no
  spend. Cost text names the reason, e.g. "≥ $0.42 (interrupted run, cost
  unknown)".
- **A parent's terminal history is no longer crowded out by its children's
  tool activity.** Opening an agent fetched its newest 5,000 events together
  with its children's, though a child's tool calls draw nothing in the parent.
  A parent with 300 turns whose child then made 2,600 tool calls opened on
  none of its turns and had to page up to reach them. History pages now take
  only the child events the parent's transcript shows, and the same parent
  opens on all 300. An older daemon ignores the narrower request and pages as
  before.
- **A failed answer's reasoning survives a reload.** When an API response
  failed partway, the terminal showed its partial reasoning live but lost it
  after a reload, and lost it in both views when the answer had begun too.
  The stored reasoning now shows beside the partial answer in both.

## 1.0.5

Claude Code agents can opt into a leaner start that skips the operator's
skill listing and extra MCP servers, and engine costs now include requests
that a closing receipt left out.

### New

- **Lean host context for Claude Code agents.** An agent created with
  `host_context: "lean"` starts Claude Code without the operator's skill
  listing or MCP servers other than Aether's own. CLAUDE.md, hooks, settings,
  plugins and built-in tools stay. Delegated children inherit it unless
  `delegate` sets `host_context`. It is off by default; measured with the
  operator's setup on Haiku, the first prompt was 38% smaller.

### Fixed

- **Engine costs include requests missing from the closing receipt.** Streamed
  tokens omitted when Claude Code stops after a wait are now priced beside the
  reported cost. When no billed rate is available, the cost is marked incomplete
  and an API-equivalent estimate is shown where available.

## 1.0.4

Keep live replies visible while children work and when navigating approvals.
Recalled memories show their origin and precedence, changed requirements
are kept, and detailed fleet status avoids repeated ownership scans.

### Fixed

- **Detailed fleet status avoids repeated ownership scans.** The
  detailed status matched every child to its task by scanning all tasks, and
  every task to its owner by scanning all children. Ownership is now looked
  up once per turn and the rendered text is unchanged. In an isolated
  benchmark of pure context preparation, without database, engine or model
  time, the observed median for 1,000 children with 1,000 tasks was 0.48 ms
  instead of 1.20 ms. For fleets of ten children or fewer, observed median
  increases were at most 0.7 µs; this is not a worst-case bound.
- **Approval navigation returns to the requester's latest output.** Opening
  an approval's requester from another agent, or denying an approval, now
  restores its live transcript when an older history window was displayed.
  New replies stay visible, and a pending older page cannot switch it back.
- **A changed requirement is no longer mistaken for a duplicate memory.**
  Admission treated notes that shared most of their words as one note, and
  skipped negations, short words, case and punctuation, so "do not run cargo
  fmt" matched "run cargo fmt", and "under 40 lines" matched "under 60 lines".
  The new note was rejected as redundant, or silently replaced the old one.
  Only a note with the same text, apart from surrounding whitespace, is now a
  duplicate; any other note is admitted beside the original, which stays
  current, and naming the original in `supersedes` still replaces it. A note
  that differs only in case or punctuation is now kept as a second memory.
- **Recalled memory shows its origin and date.** Ordinary memory lines name
  the recorder category and creation date, and the prompt says current
  instructions take precedence over recalled context. Imported and unknown
  origins stay distinct; repository notes keep their untrusted file labels.
- **An agent's replies stay in the terminal UI while its children are busy.**
  Every event of a child counted toward its parent's 5,000-event window, so a
  busy child trimmed the parent's own replies out of the transcript and left
  only "earlier history trimmed". A child's events now count only when they add
  a cell to the parent's transcript, such as its spawn or its result.
- **Sending a message returns to the latest output.** In an older history
  window, new replies, including the answer to the message just sent, were
  held off screen until Ctrl+End. Sending a message to the viewed agent now
  leaves the older window and follows the conversation, as Ctrl+End does.

## 1.0.3

Images on a message: attach screenshots and other images in the terminal UI
or with `aether send --image`. Also a fix for retried image sends and a
steadier browser test.

### New

- Attach screenshots and other images to a message. In the terminal UI,
  paste or drop an image file's path, type `/image PATH`, or press Alt+V for
  the image on the clipboard; each shows as a chip above the draft until the
  message is sent. From a script, `aether send --image FILE` (repeatable).
- Claude Code and Codex agents receive the image itself, as do API models
  that take images. The message text always names the stored file, so an
  agent on a model without vision can still open it.
- PNG, JPEG, GIF and WebP; up to 10 images of 3.75 MiB each and 10 MiB per
  message, checked by the daemon with a clear error.
- Protocol 19: `send_message` takes `attachments`. A text-only message is
  unchanged on the wire. The daemon serves 17, 18 and 19; older clients,
  the phone web app among them, read each image as a line of text.
- The phone web app does not attach images yet.
- Messages that wait in an agent's inbox together are handed over one
  message's worth of images at a time (10 images, 10 MiB), so several
  screenshots sent in a row cannot outgrow one engine input line. The rest
  follow as soon as the engine takes input, or open the next turn.
- An API model's request carries the newest 20 images and 15 MiB of them,
  counted across the whole history; older images become a line naming the
  stored file. A long conversation with many screenshots therefore stays
  inside the provider's request size.
- The generated Python and TypeScript types of an `image_ref` content block
  carry its `id`, `media_type`, `size_bytes` and `name`.

### Fixed

- **A retried image send is answered from its receipt first.** A send whose
  idempotency key was already delivered, to one agent or a subtree, and a
  `resolve_submission` for it are answered before any image is decoded or
  stored. A retry no longer needs the attachment directory, a resolution never
  writes to it, and a send the daemon refuses leaves no image file behind.
- **The phone web app's layout test no longer times out on CI.** The mock
  daemon's shutdown waited on a connection the browser had opened and never
  used. It now closes every connection.

### Before updating

- **Store schema 19.** No table changes: the step marks a store whose records
  may hold image references, which an older binary cannot read. The migration
  is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/main/docs/migration-backups.md)), and a binary that predates
  schema 19 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.

## 1.0.2

A fix for sign-in on machines where `~/.config/aether` was created writable by
its group, as it is under the `umask 002` that Ubuntu gives a user.

### Fixed

- **Signing in works when the credentials directory is unsafe.** Aether refuses
  to read stored credentials from a directory that its group or anyone else can
  write to. That refusal used to mark every provider as not connected, so the
  TUI kept asking for `/login` although `claude auth status` said you were
  signed in. Providers that sign in through their own CLI (Claude Code, Codex)
  or need no credentials (Ollama) now work regardless; a stored or environment
  key still comes first when one can be read.
- **Aether creates its directories private.** The config directory and the ones
  beneath it are made with mode `0700` whatever the umask. An existing unsafe
  directory is still refused for stored credentials, and the error now names
  the `chmod 700` that repairs it.
- **The reason a provider is not connected is shown.** `aether auth`, the login
  view and the TUI's header report what the daemon found, and a CLI sign-in
  check reads as the program and its outcome. `aether doctor` warns about an
  unsafe credentials directory.

## 1.0.1

Remote control: drive Aether from a phone, and from another machine over SSH.
See [Remote control](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote.md) and [phone setup](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote-setup.md).
Also a fix for failed tool calls in view modes that hide tools.

### New

- **Phone web app.** The daemon serves an installable web app from a loopback
  listener that is off until `aether remote enable`. `tailscale serve` (or your
  own HTTPS proxy) puts it on your tailnet; the app is compiled into the binary.
- **Device pairing and roles.** Each phone is a device with its own key,
  enrolled by a one-time code you confirm at the host. A device is an
  `operator` or an `observer`; `aether remote pair`, `devices`, `rename` and
  `revoke` manage them, and events record which device acted.
- **Push notifications.** Opt-in Web Push tells you that an agent needs you,
  without the agent's words unless `push_previews` is set.
- **Host services.** Path completion, workspace listing and validation, and a
  git summary for the phone's pickers.
- **Guided `aether remote setup`.** Checks Tailscale, enables the listener,
  publishes it and pairs a phone; safe to re-run. `--yes` never waits and
  leaves pairing for `aether remote pair`; `--json` reports without changing
  anything; `--proxy-url` and `--funnel` are the alternatives to a private tailnet.
- **SSH named remotes.** `aether remote add NAME --ssh TARGET`, then
  `aether --remote NAME <command>` runs against that machine's daemon; no
  listener, token or pairing is involved. `aether --local` forces this host.
- **Protocol 18.** The daemon serves 17 and 18; version 17 clients are
  translated. Adds the carrier's challenge frame, `Hello.device` and
  `Welcome.device`, the `forbidden` and `unimplemented` error codes, device
  provenance, and idempotency keys on eighteen commands. See the
  [protocol](https://github.com/mengsig/aether/blob/v1.0.1/docs/protocol.md).
- **Lost acknowledgements are resolved, not guessed.** `resolve_submission`
  tells a client whether a keyed command whose reply was lost ran
  (`executed`, with its original receipt), provably did not (`absent_safe`,
  so it is sent again under the same key) or cannot be accounted for
  (`unknown`). An SSH remote no longer refuses to connect while a send is
  unresolved: it resolves each on connecting, and `aether --remote NAME
  outbox` and the TUI's `/outbox` let you mark delivered, send again or
  discard what the host cannot confirm. See
  [resolving a lost acknowledgement](https://github.com/mengsig/aether/blob/v1.0.1/docs/remote.md#resolving-a-lost-acknowledgement).
- **Configuration.** A `[remote]` section in `config.toml` with `address`,
  `allow_remote`, `max_connections`, `idle_timeout_secs`, `push_previews`,
  `push_subject` and `push_hosts`; defaults are loopback only and no previews.
  See [configuration](https://github.com/mengsig/aether/blob/v1.0.1/docs/configuration.md#remote-control).
- `aether doctor` also checks remote access.

### Fixed

- **A failed tool call no longer floods a view that hides tools.** In a view
  mode whose `show` omits `tool`, such as the built-in `focus`, a failed call
  printed its whole command and output. It is now one row: the tool's name
  (`Command` for a shell call), `failed` and the first line of the error.
  Failed calls with nothing shown between them share the row of the first.
  Modes that list `tool`, the standard mode and Ctrl+T draw failed calls whole,
  as before. See [view modes](https://github.com/mengsig/aether/blob/v1.0.1/docs/tui.md#view-modes).

### Before updating

- **Store schema 18.** Adds the paired devices and push subscriptions, and
  numbers idempotency receipts for the ledger that resolution reads. The
  migration is one way: Aether saves the old database first (see
  [migration backups](https://github.com/mengsig/aether/blob/v1.0.1/docs/migration-backups.md)), and a binary that predates
  schema 18 refuses to open a migrated database. To go back, stop the daemon
  and restore that backup with the older binary; changes made since are lost.
- **Protocol 17 clients keep working.** The daemon serves protocol 17 and 18,
  so a client built against 1.0.0 still connects.
- **Nothing is exposed until you ask.** The remote listener stays off until
  `aether remote enable` or `aether remote setup`.

## 1.0.0

First public release.

### Terminal UI

- `aether` opens a conversation with one agent beside a board of the whole
  agent tree. Send, steer a running turn, queue a message, interrupt, pause
  and resume from the keyboard.
- Inspect tool calls, results, reasoning, token usage and estimated cost, and
  answer approval prompts where you type.
- Themes (`auto`, `dark`, `light`, `mono` and more), reduced motion, ASCII
  glyphs, and a plain line mode for pipes and screen readers.

### Daemon and durable agents

- A local daemon starts on demand and keeps agents running after the terminal
  closes. Agents, tasks, history and artifacts are stored on disk, and agents
  resume when the daemon starts again.
- `aether run` for one task from a script, and commands to list, message,
  pause, resume, interrupt, cancel, replay and report on agents and runs.
- Python, TypeScript, Rust and MCP clients drive the same daemon protocol.
- `aether doctor` checks an installation; `aether diagnose` writes a redacted
  diagnostic directory.

### Engines and model providers

- Claude Code and Codex run as native engines, with their own tools and
  sign-in, coordinated by Aether.
- Aether's own loop runs hosted APIs (Anthropic, OpenAI, xAI/Grok,
  Google/Gemini, Groq, OpenRouter), local servers (Ollama, LM Studio, vLLM,
  llama.cpp) and any OpenAI-compatible endpoint.

### Delegation and worktrees

- Agents delegate tracked tasks to other agents with an explicit model and
  effort, and wait for results without polling a model.
- A delegated agent can work in its own Git worktree; its parent integrates
  the changes after a check for conflicting edits.

### Schedules and watchers

- Recurring schedules, by interval or cron expression, deliver to an agent or
  start a fresh run. One-shot durable wakes, external triggers and signed
  webhooks wake agents too.
- An opt-in GitHub watcher routes authorized mentions, labels and reviews in
  selected repositories to agents.

### Budgets and limits

- Token and turn limits per agent and per scheduled run, and dollar limits
  where the engine reports a cost that can be enforced; a spend limit per run
  that stops new delegation; limits on concurrent turns.
- Approval rules for tool calls. Approvals are off by default.

### Memory

- Scoped, durable memory of facts, decisions and preferences, with
  `aether memory` to browse, pin, forget and restore it, and import of notes
  committed to a repository.

### MCP

- Configured MCP servers reach every agent through the same approvals as
  built-in tools, and `aether mcp serve` exposes Aether itself over MCP.

### Voice

- Optional local dictation and spoken questions in the terminal UI, off until
  turned on. Voice is outside the stable core of 1.0.

### Install and update

- `install.sh` installs a prebuilt, checksum-verified binary without `sudo`,
  for Linux (x86_64 and aarch64, statically linked) and macOS (Apple silicon
  and Intel).
- `aether update` installs a newer release, `--check` only reports one, and
  `--version` installs exactly the release named.

### Known limitations

Approvals and worktrees are not a security sandbox, native Windows is not
supported, and macOS is tested less often than Linux. The full list is in
[known limitations](https://github.com/mengsig/aether/blob/v1.0.0/docs/known-limitations.md).
