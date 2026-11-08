# Upgrading

What an update between two releases asks of you. `aether update` prints each
crossed release's "Before updating" notes from the
[changelog](../CHANGELOG.md) and asks before a one-way store migration; this
page explains them for the releases that change more than a patch.

## From 1.2.11 to 1.3.0

1.3.0 is released with Archon 1.5.0. It serves one web app from the remote
listener and the new desktop app, migrates the store to schema 33 and
speaks protocol 26.

| | 1.2.11 | 1.3.0 |
| --- | --- | --- |
| Daemon protocol | 25, serves 17 to 25 | 26, serves 17 to 26 |
| Store schema (`aether.db`) | 28 | 33, one-way migration with a backup |
| Remote listener | the phone app | the web app from `web/dist/`; the phone app is removed |
| Archon | 1.4.x served its own web app | Archon 1.5.0 is mounted at `/archon` on this listener |
| Desktop app | none | `web/apps/desktop`, packaged per [Desktop app](desktop.md) |
| Noesis | none | present, off by default; `noesis.db` only once turned on |

### Protocol 26 and the clients

The daemon serves protocol 17 to 26, so 1.2.x CLIs, TUIs and clients keep
working against it. Everything protocol 26 adds is behind a feature (see
[Daemon protocol](protocol.md)): a client on version 25 or older receives
every frame as before.

The 1.3.0 Python and TypeScript clients ask for protocol 26 by default, which
a 1.2.11 daemon refuses as `incompatible`. Update the daemon first, or give
the Python client `protocol_version=25` to reach an older daemon.

Archon 1.5.0 asks Aether for protocol 24 and accepts 17 and later, so it runs
its queue on Aether 1.2.11 or 1.3.0. Its remote access needs Aether 1.3.0:
only 1.3.0 mounts archond at `/archon`. Archon's own upgrade notes describe
its side.

The desktop app's Message Archon and question or gate answers also need
Archon 1.5.0: its `archon-web` connections have a device's standing only from
protocol 24. Against an older archond the app greys those controls with a hint
and the bridge refuses `say` and `answer`; everything else works as before.

### The store migrations

The first start of 1.3.0 on a schema 28 store migrates it to 33 in one step:

| Version | What it does |
| --- | --- |
| V29 | `agents.subsystem` (set once at creation, `noesis` for Noesis agents) and the indexes for paging and `usage_breakdown` |
| V30 | Memory tiers and states, support, verification and refs; every existing memory becomes active, unchanged |
| V31 | `memory_stale` and `memory_holdouts` |
| V32 | No table change: only the version stamp moves |
| V33 | `agent_usage`, each agent's usage, and `usage_totals`, the fleet's, each run's and each agent's subtree's over every run and within each, kept by triggers and filled from the existing agents, so `status` and a tree's roll-ups read rows by key instead of every agent; `agent_tree`, each agent's ancestors, so a rooted tree pages without walking its subtree; `agents.archived` and its indexes, and `agents(status, id)` in place of `agents(status)`, so `get_tree` filters before it reads an agent and pins the newest live ones by index |

Before it migrates, the daemon writes one verified backup,
`migration-backups/schema-28-before-33-<id>.sqlite.zst`, beside `aether.db`
(see [Migration backups](migration-backups.md)). Startup fails, and nothing
migrates, if the backup cannot be written. The migration is one way: 1.2.11
refuses to start on a schema 33 store (`store schema version 33 is newer than
supported 28`).

From schema 33, writing an agent into `aether.db` with another program, such
as the `sqlite3` shell, fails: the triggers that keep the usage totals call a
function only Aether registers, so no agent is written uncounted. Reading the
store that way is unaffected.

To go back to 1.2.11: stop the daemon, restore that backup as
[Migration backups](migration-backups.md#restoring-a-backup) describes, and
start 1.2.11. Changes made since the update are lost.

### Memory 2.0

The migration gives each memory a standing: tier, state (`candidate`,
`active`, `superseded` or `retracted`), support, verification and refs.
Existing memories become `active` and keep their text, scope, pins and
stats. Admission stays immediate, so new memories are active at once as
before. `aether memory migration-status` reports the migrations, the counts
and the backup's file name.

Retrieval changes what reaches a turn. Under `[memory.retrieval]`:

| Key | Default | Effect |
| --- | --- | --- |
| `max_items` | 6 | At most this many memories per turn, as before |
| `min_score` | 0 | A memory below this relevance is not injected; pinned ones are. 0, the default, turns the cutoff off: an absolute cutoff also drops relevant memories that share no word with the task |
| `holdout_rate` | 0 | A seeded share of the memories that would fit is withheld and logged, to measure lift. Set a positive rate to opt in |
| `holdout_seed` | 0 | The seed of that sample |

`aether memory consolidate --dry-run` shows which near-duplicates would merge;
without `--dry-run` it merges them, keeping the others as `superseded`.
`aether memory export` and `aether memory import` move memories between
stores. All of these are host only. See
[Memory 2.0](memory/MEMORY-V2.md).

### Noesis, off by default

Noesis, the learning service, does nothing until it is turned on: no
`noesis.db`, no model call, no spend, and every `noesis_*` command but
`noesis_status` and `noesis_configure` answers `unavailable`. To turn it on,
either:

- set `[noesis] enabled = true` in `config.toml` and restart the daemon; or
- in the desktop app, use "Turn on Noesis" in the Learn screen's settings,
  which asks you to confirm in a native dialog. A paired browser cannot.

Turning it on is host only, as are its budgets, billing and prompt
overrides; a paired device cannot. Check the daily budgets under
`[noesis.budget]` first (see [Configuration](configuration.md#noesis)).
`billing.allow_api = false` keeps it off API-billed models.

### The web app on the listener

The listener now serves the committed bundle `web/dist/`, compiled into the
binary. The phone app's paths (`/app.js`, `/ui/…`, `/asset-manifest.json`)
answer 404, and the new service worker clears the phone app's caches.

- A browser or Home Screen app that paired with 1.2.11 keeps its device key
  and pairing: open the same address again.
- `/pair#code=…` opens pairing with the code filled in.
- An observer device sees every control disabled, with the reason.
- The app cannot attach images yet (see
  [Known limitations](known-limitations.md)).

### The desktop app

`web/apps/desktop` builds an Electron app that shows the same `web/dist/`:
an AppImage and a deb on Linux, a dmg and a zip on macOS. It finds the local
daemons by the CLIs' rules and never stops one; it starts one only with
managed start turned on. A build without the maintainer's signing
credentials says "unsigned build" and never updates itself on macOS. Updates
come only from this repository's GitHub releases, with each download and
install confirmed. See [Desktop app](desktop.md).

### Remote setup, pairing and mounts

- `aether remote setup` and pairing work as in 1.2.11.
- `aether remote mount add /archon --target http://127.0.0.1:<port>`
  forwards `/archon` to a loopback archond; `archon remote setup` (Archon
  1.5.0) runs it for you. Mounts are host only, at most four.
- Adding a mount is refused while Tailscale Funnel is on or its state cannot
  be read, and `aether remote setup --funnel` is refused while a mount
  exists. Mounted traffic flows only while a status read at most 5 s old
  shows no Funnel; otherwise mounted connections close (`1008`) and requests
  answer `503`.
- A paired Aether device gets no standing on a mount: archond pairs and
  authenticates its own devices. See [Mounts](remote.md#mounts).

### What a paired device and the desktop app may do

The [vNext security audit](security/vnext-audit.md) changed these:

- The desktop page's own archond connections must greet as `archon-web`. It
  can no longer claim the Archon CLI's standing, so it cannot lift a hard
  pause.
- The desktop bridge refuses to change the GitHub watcher's operators and to
  revoke an approval rule for every agent, as aetherd refuses a paired
  device. It passes only observe and operate commands; host-only and
  remote-admin commands go through a privileged action the main process
  confirms.
- A paired device cannot run `memory_retrieval_explain`,
  `consolidate_memories`, `memory_export` or `memory_import`, or turn Noesis
  on, change its budgets, billing or prompts. Observers can read
  `memory_migration_status`.
- `noesis_import_source` masks an artifact as `read_artifact` does.

[Remote control](remote.md#authorization) lists every command's class.

### What to do, in order

1. **Back up, if you want a copy of your own.** `aether storage backup`
   writes a verified snapshot; the update takes one too.
2. **Update.** `aether update`. It shows these notes and asks before the
   one-way migration; answer `y`.
3. **Restart the daemon.** `aether stop`, then `aether`. The store migrates
   on this start. Check with `aether doctor` and `aether memory
   migration-status`.
4. **Update your clients.** The Python and TypeScript clients at 1.3.0; your
   own clients need nothing, as protocol 25 is still served.
5. **Reopen the web app** on each paired browser at the same address.
6. **Archon, if you use it.** Update to Archon 1.5.0 and follow its upgrade
   notes; `archon remote setup` then mounts it at `/archon`.
7. **Optionally** install the desktop app, and turn on Noesis.
