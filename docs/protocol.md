# Daemon protocol

## Request correlation and malformed frames

Aether uses newline-delimited JSON. A client frame carries a nonzero unsigned
64-bit correlation key in `req`; a server reply echoes it in `id`. A command's
own `id` (for example, the memory to forget) is unrelated to correlation.

The daemon reads `req` before decoding the command. Typed decoding failures,
including unknown fields in strict specifications, invalid enum values, and
missing required fields, receive `invalid_argument` at that request's ID, with
the decoding diagnostic. After a successful hello, these errors leave the
connection usable. A malformed hello receives its correlated diagnostic and
closes the connection.

If the daemon cannot read an unambiguous, nonzero `req` (including invalid JSON,
a missing, duplicate, zero, negative, fractional, or out-of-range ID), it sends
an `invalid_argument` reply with `id: 0`, then closes the connection. ID zero is
reserved for connection diagnostics. Clients retain that diagnostic and fail
all pending requests when the connection ends, rather than waiting for request
timeouts. Transport framing failures, such as an overlong line or invalid UTF-8,
may close without a JSON diagnostic; clients still fail pending requests on
closure.

This policy uses the existing reply shape and reserved ID-zero/closure contract;
it does not introduce an incompatible wire change. It applies to every served
protocol version.

## Versions

The protocol is version 19 and the daemon serves versions 17, 18 and 19: its
own and two back, because 18 and 19 were released days apart. The window is
`MIN_PROTOCOL_VERSION` to `PROTOCOL_VERSION` in `aether-protocol`; the
`compat` module translates replies for the older versions.
A hello outside the window is refused with `incompatible`, naming both bounds.
The `Welcome` echoes the version in effect for the connection and reports the
window as `daemon_protocol_version` and `min_protocol_version`.

Only the daemon's answers are rewritten. Every field a newer version added to
a command is optional, so an older client's requests decode unchanged; the
test `no_command_requires_a_field_an_older_client_would_not_send` holds that
against the live schema.

Version 19 adds [images on a message](#images-on-a-message):
`send_message.attachments` and the `image_ref` content block. A version 17 or
18 client cannot decode that block and receives a line of text in its place.
A version 18 daemon ignores `attachments` and would deliver the text alone,
so it refuses a version 19 hello as outside its window; a client that falls
back to an older version for such a daemon must not send images to it (the
Rust client refuses with `unimplemented`).

Version 18 adds remote control, whose contract is [Remote control](remote.md);
the wire changes are:

- **Challenge frame.** `ServerFrame::Challenge` (`{"frame": "challenge",
  "nonce", "instance_id"}`) is sent first, and only, by the remote WebSocket
  carrier. The Unix socket and the token listener never send it, and a client
  never sends one. The carrier's hello answers it.
- **`Hello.device` and `Welcome.device`.** A device proves its enrolled key by
  signing the challenge in `Hello.device`. The `Welcome` names the
  authenticated device (`id`, `name`, `role`); it is absent on the Unix socket
  and the token listener. See [the handshake](remote.md#the-handshake).
- **Error codes.** `forbidden` (authenticated, but the transport or role may
  not run the command) and `unimplemented` (a protocol command this build does
  not serve). A version 17 client cannot decode them, so it receives
  `unauthorized` and `unavailable` with the same message.
- **Device provenance.** An actor caused over a device connection carries an
  origin naming the enrolled device. A version 17 client receives that actor
  as a user named `device:<name>`. See [provenance](remote.md#provenance).
- **Idempotency keys.** Eighteen commands take an optional `idempotency_key`:
  `create_agent`, `create_task`, `send_message`, `reply_to_question`,
  `trigger`, `create_schedule`, `resolve_approval`, `restart`, `cancel`,
  `interrupt`, `pause`, `resume`, `archive`, `update_agent`, `set_approvals`,
  `update_schedule`, `create_memory` and `add_approval_rule`. The same key with
  the same fields returns the first outcome; with different fields it is
  `invalid_argument`. See [idempotency keys](remote.md#idempotency-keys).
- **`resolve_submission` and `Welcome.submission_horizon`.** A read-only
  command that says what became of a keyed command whose reply was lost:
  `executed` with the original receipt, `absent_safe` when the host proves it
  never ran, or `unknown`. The proof compares the host's durable receipt
  ledger with the opaque horizon every version 18 `Welcome` carries, which a
  client keeps with each keyed command it sends. See
  [resolving a lost acknowledgement](remote.md#resolving-a-lost-acknowledgement).
- **New commands and replies.** Attention, host services (path completion,
  workspaces, git summary), remote administration, device management and push,
  each classified by the access class it needs
  ([authorization](remote.md#authorization)).

A version 17 daemon cannot decode these commands and ignores the new keys, so
it refuses a version 18 hello as outside its window rather than dropping them.

The versioning contract and negotiation are defined in `aether-protocol`'s
`PROTOCOL_VERSION` and `compat` module.

## Optional features

The `Welcome` lists optional features the daemon supports; a client ignores
names it does not know. `host_context` means `create_agent` accepts
`config.host_context` (`"full"` or `"lean"`, see
[lean host context](engines.md#lean-host-context)). It is left off the wire
when full, so an older daemon or client that never sees it behaves as before.
A client must check that the `Welcome` lists `host_context` before sending
`config.host_context`: an older daemon silently ignores the field and creates
a full agent.

`operator_memory_roles` means `create_memory` accepts the `role` and
`repository_role` operator scopes and the `repository` field on both repository
scopes. Check this feature before sending any of them. An older daemon rejects
unknown scopes or fields with a decode error; it cannot file the memory under
a different scope. The protocol version is unchanged.

`memory_recorded_by` means the `memories` query applies `recorded_by` (see
[memories by recorder](#memories-by-recorder)). A client must check it before
sending the field: an older daemon ignores it and returns every memory the
rest of the query matches, which would read as all of them having been filed
by the operator. The Rust client refuses such a query itself.

`repository_root` means `create_agent` accepts `repository_root` (see
[naming a checkout](#naming-a-checkout)). A client must check it before
sending the field: an older daemon ignores it and files the agent's memory
under its workspace's repository instead. The Rust client refuses such a
create itself.

`memory_proposals` means operator memories can await review before recall; see
[memory proposals](#memory-proposals). Check it before proposing, filtering or
reviewing proposals. The Rust client refuses these requests to older daemons.

`memory_feedback` means the host can record, retract and read evidence about
memories a task saw; see [memory feedback](#memory-feedback). Check it before
sending any of the three commands. The Rust client refuses them to older daemons.

`repository_aliases` means the daemon serves `alias_repository` and
`list_repository_aliases` (see [repository aliases](#repository-aliases)). An
older daemon refuses both commands as unknown; the Rust client refuses to send
them to one.

`memory_context` means the daemon serves `memories_in_context` (see
[memories in context](#memories-in-context)). An older daemon refuses the
command as unknown; the Rust client refuses to send it to one.

## Naming a checkout

An agent's memory is scoped by `scope.repository`. Left unset, the daemon
derives it from the agent's workspace: the normalized `origin` remote, else
the canonical path. A client whose agents work in worktrees or mirrors can
name the checkout they belong to instead, and get the id an agent working in
that checkout would get:

```json
{"cmd": "create_agent", "role": "worker", "objective": "...",
  "scope": {"workspace_root": "/src/worktrees/widget/item-7"},
  "repository_root": "/src/widget"}
```

`repository_root` sits beside `scope`, not inside it. It is input only: the
derived id is stored as `scope.repository` and the path itself is not kept.
It must be an absolute path to an existing directory, and cannot be sent with
`scope.repository`. The id then goes through the usual narrowing check, so a
child naming a different repository than its parent is refused, and a child
created without one inherits its parent's. The field is part of the
idempotency fingerprint: a key reused with a different `repository_root` is
refused. A paired device cannot send it, as it cannot send `scope.repository`,
and agents delegating with the `delegate` tool cannot set it. The id is
derived from the path given, so a subdirectory of a checkout with no `origin`
names a different scope than the checkout itself. Check the
`repository_root` feature first.

## Repository aliases

An agent created with a bare `scope.repository`, such as `archon`, files its
memory under that name, apart from agents whose id was derived from the
checkout, such as `github.com/mengsig/archon`. An operator joins the two by
aliasing the old name to the derived one:

```json
{"cmd": "alias_repository", "alias": "archon", "root": "/src/archon"}
{"cmd": "alias_repository", "alias": "archon",
  "repository": "github.com/mengsig/archon", "dry_run": true}
```

Name exactly one of `root` and `repository`. A `root` is an absolute path to
a checkout, and the id is derived from it exactly as for `repository_root`. A
`repository` is taken as given. Both ids are normalized like
`scope.repository`, and must differ.

What moves: every memory scoped `repository` or `repository_role` under the
alias, in any state, including forgotten and superseded ones. Each keeps its
id, text, role, provenance, supersession links, pin and curation; only its
scope changes. A memory whose text is already current in the target scope is
moved beside it, not merged, and counted under `duplicates`. Global, role,
agent and subtree memories are not touched, and neither are agent rows: an
agent created with the alias keeps it, and its reads and writes resolve to
the target from then on. A new agent, or a child, naming the alias is created
with the target id, so a child of an agent still carrying the alias passes
the narrowing check, and so do later checks against that agent: tightening,
restarting or scheduling to the child. A `memories` query, `create_memory` and
`aether memory list --scope` given the alias read and write the target. The
row, the move and one `custom` event named `repository.aliased` are committed
together, or not at all.

The reply, `RepositoryAliased`, carries the alias, the target, `existing`
(the alias was already registered) and `counts`: memories by scope kind
(`repository`, `repository_role`), `duplicates`, and the `agents` still
carrying the alias. With `dry_run` the counts are what a real run would move
now, and nothing is written.

Registering the same pair again is accepted, and moves anything filed under
the alias since, such as an imported run's memories. Aliasing a name to a
different target is refused, and so is a chain: an alias cannot point at
another alias, and a target cannot itself become an alias. There is no command
to remove an alias. Moved memories do not move back, and a removed alias would
put an old agent's new memories under the old name again, so an alias is
permanent. `list_repository_aliases` answers `RepositoryAliases`, every alias
ordered by name. `alias_repository` is host only, so a paired device cannot
send it; it can list them. Check the `repository_aliases` feature first.

## Operator memory scopes

`create_memory` is operator-only. Its `spec.scope` describes which agents
recall the memory; the runtime records the authenticated user as its author.
The scopes are `{"scope": "global"}`, `{"scope": "agent", "id": "agt_..."}`,
`{"scope": "repository", "root": "/workspace"}`,
`{"scope": "role", "role": "reviewer"}` and
`{"scope": "repository_role", "repository": "aether", "role": "reviewer"}`.

A `role` memory reaches agents with that exact role in any repository.
For `repository_role`, a memory filed for `reviewer` is recalled by agents with
role `reviewer` or `reviewer.security`, but not `reviewers`, in that repository.
Surrounding spaces are trimmed. Roles must be nonempty and at most 256
characters after trimming; any control character (including a trailing newline)
is refused. A repository role family requires one to eight dot-separated,
nonempty segments. Roles such as `archon-stage:default/review` and
`archon-run:default` are accepted; the stricter directory-name grammar for
`.aether/roles` does not apply to operator scopes.

Both repository scopes accept either `root` or `repository`, never both.
`root` targets the id Aether derives for that workspace from its origin remote
or canonical path. An explicit root must be the daemon's workspace, a live
agent's workspace, or a directory within one. `repository` targets agents
created with an explicit `scope.repository` (as Archon does). The name is
normalized using the same rules as agent creation and must already be carried
by at least one agent, including terminal or archived agents, or be the
target of a [repository alias](#repository-aliases); an alias names its
target. For example,
`{"scope": "repository", "repository": "aether"}` targets that explicit scope
rather than the workspace's derived id. When neither field is set, the daemon's
workspace root is used. Unknown scope fields and invalid roles, roots or
repository names are refused before a memory is written.

## Memories by recorder

The `memories` query takes `recorded_by` to list only what one kind of
recorder authored, for example what the operator taught for a scope:

```json
{"cmd": "memories", "query": {"recorded_by": "operator",
  "scope": {"scope": "repository", "id": "aether"}, "limit": 50}}
```

The only recorder is `operator`: a memory whose runtime-recorded
`provenance.actor` is an operator of this daemon (`{"actor": "user", ...}`),
which is what `create_memory` records. That includes an operator who
authenticated remotely, over TCP or from a paired device. Memories an agent remembered, notes
synced from a repository's files (even when an operator ran the sync) and
system-recorded memories do not match. Neither does an operator memory read
out of an import (`{"actor": "imported", "was": {"actor": "user"}}`): that
person acted on another daemon, and the prompt labels such a memory
`imported`, not `operator`. The filter combines with every other field, so
`scope`, `kinds`, `include_superseded` and `include_forgotten` narrow it as
usual. An unknown recorder is refused with a decode error. Check the
`memory_recorded_by` feature first.

## Narrowing a child's events

`get_events` and `subscribe` take an event query. With `agent`, it matches the
agent's own events and its children's: those whose `parent_agent_id` is the
agent. `child_types` lists the only event types taken from the children:

```json
{"req": 9, "cmd": "get_events", "after": 0, "limit": 5000,
 "query": {"agent": "agt_...", "before": 18446744073709551615,
           "child_types": ["agent.created", "task.completed", "message.sent"]}}
```

The agent's own events are unaffected, and `families` and `types` still
apply to both. Empty, the default, takes every type, and is left off the wire.
A history page then spends its `limit` on what the client shows rather than
on a busy child's tool calls: the terminal asks for the types that draw in a
parent's transcript (`PARENT_VISIBLE_CHILD_TYPES` in `aether-tui`). Expired
history is reported only when an expired event matches the narrowed query. A
subscription with `child_types` leaves out the children's streaming deltas,
which have no type.

An older daemon ignores the field and answers with every child event, as it
always did: the event query never refused unknown fields. No `Welcome`
feature is needed, and a client talking to such a daemon gets the pages it
got before.

## Images on a message

Protocol 19 lets `send_message` carry images next to its text:

```json
{"req": 7, "cmd": "send_message", "to": "agt_...", "content": "What is this?",
 "attachments": [{"media_type": "image/png", "data": "iVBORw0KGgo...", "name": "shot.png"}]}
```

`attachments` defaults to empty and is left off the wire when empty, so a
text-only message is the frame it always was. Each entry is one image:
`media_type` (`image/png`, `image/jpeg`, `image/gif` or `image/webp`), `data`
(the file's bytes in standard base64) and an optional `name` shown to people.
`content` may be empty when there is at least one image.

The daemon checks every image before it stores or delivers anything and
refuses the whole message with `invalid_argument` and a message naming the
image and the rule:

| Rule | Limit |
| --- | --- |
| Images per message | 10 |
| One image, decoded | 3,932,160 bytes (3.75 MiB) |
| All images of one message, decoded | 10,485,760 bytes (10 MiB) |
| Type | PNG, JPEG, GIF or WebP by its leading bytes, which must agree with `media_type` |
| `data` | valid standard base64, not empty |

The limits are `MAX_IMAGES_PER_MESSAGE`, `MAX_IMAGE_BYTES` and
`MAX_MESSAGE_IMAGE_BYTES` in `aether-core`'s `attachment` module. Two more,
`MAX_REQUEST_IMAGES` (20) and `MAX_REQUEST_IMAGE_BYTES` (15 MiB), are not
checked on a command: they bound the images one request to an API model
carries across its whole history, see [engines](engines.md#images-on-a-message). Of a
`name`, only the last path component is kept, without control characters and
cut at 120 characters.

The limits also bound what an agent is handed at once. Messages that wait in
its inbox together are folded into one user turn only while their images stay
within one message's limits; the rest stay unconsumed and follow as the next
input (a steer into the running engine turn, or the next turn).

Image bytes are never written to the event log, an inbox item or a
transcript. The daemon stores each image once, under the SHA-256 of its bytes,
in `aether.attachments/` beside `aether.db` (mode 0700), and everything
durable carries a reference in its place:

```json
{"type": "image_ref", "id": "<64 hex digits>", "media_type": "image/png",
 "size_bytes": 48211, "name": "shot.png"}
```

A reference is a stored format too: store schema 19 marks a database whose
records may hold one, so a binary that cannot read it refuses the store.

That block follows the text block in a message's `content` wherever a message
appears: `inbox.delivered`, `get_inbox` and `get_transcript`. A message that
is only images has no text block. The file is named `<id>.<ext>`.

The images are part of an idempotency key's fingerprint: a retry with the
same key and the same images is the same send, and the same key with
different images is refused like any other reuse. Either is answered before
an image is decoded or stored, so a retry of a delivered send does not need
the attachment directory.

A client that negotiated protocol 17 or 18 receives each `image_ref` block
as a text block reading `[image: shot.png, 47.1 KB]`, since it has no such
variant to decode. Sending images needs a protocol 19 daemon.

Sending a message with images needs the same standing as sending one
without: `operate` in the [authorization](remote.md#authorization) matrix. An
observer device is refused with `forbidden` before any image is looked at.
[`resolve_submission`](remote.md#resolving-a-lost-acknowledgement) takes the
whole command, images included, and stores nothing.

`subtree: true` delivers the same references to every recipient; the bytes
are still stored once. The `send_message` tool that agents use to write to
each other is text only.

## Usage and cost coverage

`Usage.cost_usd` is the sum of known costs; `cost_complete: false` makes it a
lower bound. `cost_basis` identifies list prices, provider receipts, or a mix.
The optional `cost_gap` explains incomplete coverage: `unpriced` means reported
usage had neither a price nor a provider cost; `unreported` means the engine
did not fully report usage; `interrupted` means possibly billed work lacks
closing accounting, including caller cancellation after submission. Closing
accounting means the engine's own final usage message, not a synthesized
adapter outcome or an interrupted turn completion. Any two different reasons
combine as `mixed`. `unknown` is reserved for legacy
incomplete records and unrecognized future reasons in Rust readers.

Complete totals omit `cost_gap`. Missing reasons on older incomplete records
read as `unknown`. Adding complete usage cannot repair a gap or lower known
costs or token counters. Engine failures with closing accounting keep their
reported coverage; a missing dollar figure alone is not an interruption.
Caller cancellation after submission contributes an `interrupted` gap even if
the engine reports a final cost: an aborted stream can omit output. Proven pre-launch
failures, including unsupported engine configuration, remain complete zero.
An absent start acknowledgement or usage receipt is not proof of zero spend.

`cache_write_1h_tokens`, when present, is the part of `cache_write_tokens`
written to Anthropic's 1-hour prompt cache. It is never larger than
`cache_write_tokens`, and is omitted when zero. A model price's optional
`cache_write_1h_per_mtok` prices those tokens (2x input for Claude models;
5-minute writes are 1.25x). Writes from an engine that does not report the
split, and prices without that rate, use `cache_write_per_mtok` as before.

An API request becomes possibly billed once its stream starts. A stream
failure without closing accounting keeps observed tokens and priced dollars
as a floor with an `interrupted` gap. An error response before stream start adds no spend.

After a harness stop (park, kill-switch, or exhaustion), costs are complete
only if closing accounting arrived before the stop was requested and no possible
new request followed it. A closing receipt after interrupting an in-flight
request cannot repair the `interrupted` gap. Terminal parks allow final accounting
until their grace expires; they do not request a native interrupt.
Engine message invocation records carry no usage when the engine accounts
per request. Their zero-dollar `unreported` gap describes missing per-message
usage, not missing turn accounting: request usage and its coverage belong to
the turn and agent totals.

Only possible new model work invalidates an engine's closing receipt: assistant
content, tool activity, or background results and steered messages handed to
the engine. Rejected input and bookkeeping events add no spend. A background
result delivered after a receipt is possibly billed; without another answer
within the grace period, coverage includes an `interrupted` gap.

Known limits: if streamed usage crosses a budget before the runtime reads the
closing receipt, the run remains an incomplete floor. A receipt alone cannot
prove that a cancelled request reported every output token. Usage and prices
remain dependent on what engines and providers report; list prices are estimates.

No Welcome feature flag is needed: this is optional response metadata. Older
daemons omit it, and clients must keep an absent reason unknown. Generated
Python and TypeScript clients list the known reason strings; Rust readers map
unrecognized strings to `unknown`.

## Memory proposals

Check `Welcome.features` for `memory_proposals` before sending these fields or
commands. `create_memory` with `spec.proposed: true` stores an operator lesson
for review. Admission policy, duplicate detection and scope validation still
apply. It records `proposed_at` and the authenticated proposer in
`provenance.actor`. A proposal is never recalled, rendered in context, indexed,
or counted as an existing active memory by admission or repository seeding.
Agent `remember` behavior is unchanged.

```json
{"cmd":"create_memory","spec":{"text":"Run the release verification script before publishing","kind":"lesson","scope":{"scope":"global"},"proposed":true,"supersedes":"mem_..."}}
{"cmd":"memories","query":{"proposed":true,"limit":50}}
{"cmd":"accept_memory","id":"mem_...","reason":"Confirmed by three successful releases"}
{"cmd":"reject_memory","id":"mem_...","reason":"Not enough supporting evidence"}
```

`supersedes` is allowed when proposing. The old memory stays active until
acceptance. Acceptance sets `accepted_at`, preserves `proposed_at`, and applies
the replacement atomically. `reason` is optional on acceptance and is also
recorded on `memory.superseded`. If the target was forgotten or superseded
meanwhile, acceptance fails with `invalid_argument` and changes nothing;
reject the stale proposal and submit a replacement against the current record.
Acceptance also refuses an active memory with the same text in the canonical
scope, except the predecessor being replaced. The error names that memory id
and leaves records and events unchanged.
Accepting an already accepted, still-current proposal returns the same record
without another write or event. Missing ids return `not_found`; ordinary
memories, rejected/forgotten proposals and subsequently superseded accepted
proposals cannot be accepted.

Rejection requires a nonblank reason. It sets `forgotten_at`, keeps the record
and emits `memory.proposal_rejected` and `memory.forgotten`. Repeated rejection
of a forgotten pending proposal returns it unchanged without more events.
Accepted or ordinary memories cannot be rejected. `forget_memory` also curates
a pending proposal out. `restore_memory` clears that curation and emits the
usual `memory.restored`; the proposal still awaits acceptance and remains out
of recall. Pinning never bypasses review.

Creation emits `memory.candidate` and a `custom` event named `memory.proposed`.
Its payload is `{"memory":"mem_...","scope":{"scope":"global"},"tags":["build"]}`,
with the canonical scope serialized as on the memory record and an optional
`"supersedes":"mem_..."` omitted when no predecessor exists. The authenticated
actor is on the event envelope.
Acceptance emits `memory.accepted` with an optional reason, plus
`memory.superseded` when replacing a record. `memory.proposal_rejected` is
also a `custom` event name. These existing event shapes keep older subscribers
able to decode the stream.
Curation reasons and authenticated actors remain in the durable event log.
`memories` excludes pending proposals by default, even with
`include_forgotten` or `include_superseded`. Set `proposed: true` to select
pending proposals instead; add `include_forgotten: true` for rejected ones.
Accepted proposals appear with ordinary active memories.

Proposed creation, proposal listing, acceptance and rejection are host-only
commands, served on the operator socket or authenticated shared-token
transport. Paired devices cannot issue them. Agents have no proposal review
MCP tools, and runtime review methods reject agent actors. Older clients keep
their existing behavior. Schema version 21 adds an indexed proposal flag;
existing v20 memories retain their state and an older binary refuses the new
store.

The CLI uses the same workflow:

```sh
aether memory add 'Run the release verification script before publishing' --kind lesson --proposed --supersedes mem_...
aether memory list --proposed
aether memory accept mem_... --reason 'Confirmed by three successful releases'
aether memory reject mem_... --reason 'Not enough evidence'
aether memory list --proposed --all
```

`add --scope` accepts `global` (the default), `agent:<id>`, `role:<role>`,
`repo-role:<role>@<repository>`, or a repository id. `--root` instead derives
the repository from a checkout. `memory show <id>` explicitly finds proposals
as well as admitted records and marks them `proposed` or `rejected`.
`memory list --all` alone never includes pending proposals.

## Memories in context

Every model call is assembled from a context snapshot, and `snapshot.created`
announces it. The `retrieved_memory` layer of that event now carries
`memories`: the ids of the stored memories actually rendered into the prompt,
in the order the prompt shows them, after the memory budget trimmed what did
not fit. Operator and agent memories come first, then repository notes. It
is the snapshot's `memory_ids`, one list from one place. The field is left out
when the layer holds no memories, so older events decode and re-serialize
unchanged, and recording it does not change the prompt. Claude Code and Codex
turns list the ids too, including a turn whose session already holds the
memory set and so is not sent it again (`placement: "retained"`).

`memories` is what that call's layer delivered. A resumed Claude Code or Codex
session also keeps what earlier calls in it were sent, until the engine
compacts its own history, so a memory forgotten or displaced since then can
still be in that session without being listed on the later call. Read a
task's answer as a whole for everything its agents were shown.

```json
{"type":"snapshot.created","snapshot":"snp_...","turn":1,"model":{"provider":"anthropic","model":"..."},"token_estimate":1840,"layers":[{"kind":"retrieved_memory","text":"Relevant memory ...","tokens":96,"items":2,"placement":"system","memories":["mem_...","mem_..."]}]}
```

The envelope names the agent and task. An invocation names its snapshot on
`invocation.completed` and `invocation.failed`.

`memories_in_context` asks, after the fact, which memories each model call was
given. Name exactly one of `task` or `invocation`; both or neither is
`invalid_argument`, and an unknown id is `not_found`.

```json
{"cmd":"memories_in_context","task":"tsk_..."}
{"cmd":"memories_in_context","invocation":"inv_..."}
```

The reply is `MemoriesInContext`: one entry per snapshot, oldest first.

```json
{"snapshots":[{"snapshot":"snp_...","invocations":["inv_..."],"agent":"agt_...","task":"tsk_...","turn":1,"at":"2026-10-15T09:00:00Z","memories":["mem_...","mem_..."]}],"truncated":false}
```

`invocations` lists the calls answered from the snapshot. A Claude Code or
Codex turn can answer several messages from one snapshot, and it is left out
when none finished. `memories` is always present, empty when the call saw
none. By task, the answer covers every model call of the task's agents, read
through the task's indexed events, and lists at most 256 snapshots. A longer
task returns its oldest 256 with `truncated: true`. By invocation, it is that
one call's snapshot, found from its invocation record or, for a failed call
with none (daemon recovery and imported history keep none), from its
`invocation.failed` event. A chat agent with no task is reachable by
invocation, and so is a call daemon recovery closed: its event names no task,
so a task's answer lists that call's snapshot without it.

The command is host-only: the operator socket and the shared-token transport
are served, paired devices are `forbidden`, and the runtime refuses agent
actors. The CLI is `aether memory context --task <id>` or `--invocation <id>`,
with `--json` for the reply as sent.


### Memory feedback

The host can record evidence about a memory a task saw. Check the Welcome
feature `memory_feedback` first. All three commands are host-only: the local
operator socket and shared-token transport are served; paired operators and
observers are `forbidden`. The runtime also refuses agent actors, including
reads. Feedback does not affect recall, scores, prompts or token counts.

```json
{"cmd":"record_memory_feedback","memory":"mem_...","verdict":"helped","evidence":"review","task":"tsk_...","invocation":"inv_...","reason":"The release check caught the regression","idempotency_key":"release-review-17"}
```

`task` is required. `verdict` is `helped`, `hurt` or `neutral`; `evidence` is
`operator`, `review` or `outcome`. `invocation` is optional. `reason` must be
nonblank and at most 4096 Unicode characters; the stored text is preserved.
Unknown memory, task or invocation ids are `not_found`. A named invocation
must resolve to a snapshot of that task, through its invocation row or its
`invocation.failed` event when no row exists. A different task is
`invalid_argument`.

Without an invocation, the memory must occur in the retrieved-memory layer of
an announced, stored snapshot of the task (legacy events use the snapshot's
stored `memory_ids`). With an invocation, the memory must have been given to
that call or an earlier call of the same task, because a resumed Claude Code or
Codex session may retain earlier context. The named call's own stored memory
list qualifies directly; earlier calls are those whose snapshot was created at
or before the named call's snapshot, by wall-clock time. A memory seen only by
a later call or another task does not qualify. A call refused before sending,
for example over the context limit, still counts as given. This checks all
calls directly in storage, including calls beyond the 256-snapshot
`memories_in_context` limit. A never-shown proposal is refused with
`invalid_argument`. Memories forgotten or superseded after being shown remain
eligible. Expired history cannot prove new eligibility.

The reply is `MemoryFeedback`:

```json
{"id":"mfb_...","memory":"mem_...","verdict":"helped","evidence":"review","task":"tsk_...","invocation":"inv_...","reason":"The release check caught the regression","actor":{"actor":"user"},"created_at":"2026-10-15T09:00:00Z"}
```

Each unkeyed submission appends a distinct record, including repeated verdicts
on the same task. An `idempotency_key` replays the original reply and writes
neither a second record nor a second event, even after a later retraction.
Read `memory_feedback` for the current state; a keyed retry returns the original
receipt. Reusing the key with different fields is refused. Receipt retention
follows the existing keyed-submission rules. Recording emits a custom
`memory.feedback` event whose payload has `feedback` (the new id), `memory`,
`verdict`, `evidence`, `task`, optional `invocation`, and `reason`; the
envelope also carries the task.

```json
{"cmd":"retract_memory_feedback","feedback":"mfb_...","reason":"The review was based on an obsolete build"}
```

Retraction preserves every original field and sets `retracted_at` and
`retraction_reason`. It returns the record, and emits the custom event
`memory.feedback_retracted` with `{feedback, memory, reason}`. An unknown id
is `not_found`. The reason has the same bound as recording. Retrying a
retraction returns the first retracted record and emits no second event;
a different retry reason does not overwrite the first reason. If the task's
history has expired, the retraction event carries neither a task nor a run in
its envelope.

```json
{"cmd":"memory_feedback","memory":"mem_...","limit":100}
{"records":[{"id":"mfb_...","memory":"mem_...","verdict":"helped","evidence":"review","task":"tsk_...","reason":"Release check passed","actor":{"actor":"user"},"created_at":"2026-10-15T09:00:00Z","retracted_at":"2026-10-15T10:00:00Z","retraction_reason":"Obsolete build"}],"truncated":false}
```

This read-only command returns `MemoryFeedbackList`: newest first, including
retracted records. The default and maximum limit are 100; values outside
1–100 are `invalid_argument`. `truncated` says more records exist. Feedback
survives history retention. Memory and feedback records are not part of run
export/import. Their audit events can travel with run history; typed ids in
those events are remapped, but importing does not recreate memory or feedback
records.

The CLI mirrors these commands:

```sh
aether memory feedback record <memory> --task <id> --verdict helped --evidence review --reason 'Release check passed'
aether memory feedback retract <feedback> --reason 'Obsolete build'
aether memory feedback list <memory> --json
```

Recording also accepts `--invocation <id>`. All three accept `--json`.
