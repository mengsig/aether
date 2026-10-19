# Log retention

New logs are kept in a directory per stream below the configured log directory:
`<stream>/segments-v1/events.0.log`. Streams include `aetherd.log`, `aether.log`,
`aether-mcp.log`, and `aether-bridge.log`.

Each stream retains at most seven segments of 16 MiB each: 112 MiB per stream.
Segment 0 is newest; segments 1 through 6 are progressively older. Rotation
happens during writes, including oversized writes, so a busy day cannot create
an unbounded file. Large records may span segments. An OS file lock serializes
writes and rotation across processes; each write reopens the current segment so
a process cannot continue writing to an old or deleted file after rotation.
Write failures are returned and reported on stderr, once per consecutive error
streak. A detached daemon's stderr is `/dev/null`, so records a write failure
refused - a full disk, typically - are kept in memory, up to 4 MiB, and written
in order once the log takes writes again. Records past that are dropped and
counted in one line written in their place. `aether doctor` names the file the
daemon writes now. Logs are diagnostic output, not the durable task/event
recovery store.

Legacy files such as `aetherd.log.<date>` and earlier managed daily logs such as
`aetherd.log/events.<date>.log` are not removed by this change. They may still
have active old-version writers. This is not a bound on the entire data directory:
database, artifact, native-session, backup, and workspace retention remain
separate lifecycles described in [known limitations](known-limitations.md).

Managed segments also rotate by age (fourteen days by default), under the same
writer lock. Archived segments beyond that age are removed on startup or a
write. Active age uses birth time when available, otherwise modification time;
continuously written segments on filesystems without birth times remain bounded
by size. `[storage] logs_days = 0` disables age pruning. See
[storage configuration](configuration.md#storage).

## A full disk

The log shares a filesystem with the store, so a disk that fills is recorded
where it survives: a turn whose write the disk refused is recovered once writes
land again and a `store.write_failed` event goes on its agent. New turns wait
while free space under the store is below `[scheduler] min_free_disk_mib` (by
default the smaller of 5 GiB and 2% of the disk; `0` turns this off), and
start again once it has a tenth more than that. A `disk.low` event marks the
start and `disk.recovered` the end; `aether status`, `aether doctor` and the
TUI status bar show it while it lasts. A daemon that restarts with the disk
clear, or with the floor turned off, records the `disk.recovered` that the
earlier daemon never saw. Turns already running carry on.

## Completed-run history

The daemon checks completed-run history hourly. The default recovery window is
seven days; to retain ten days, add this to `config.toml`:

```toml
[retention]
completed_run_days = 10
```

The value must be a positive integer; zero is rejected. Eligible runs lose their
events, transcripts, snapshots, and associated execution records permanently.
The sweep protects live or recently updated runs, in-flight turns, and runs
referenced by durable state such as artifacts or memory. These protections can
keep a run beyond the configured window. Requests for expired event history fail
explicitly rather than returning an apparently complete empty history.

SQLite reuses freed pages; deletion does not necessarily shrink the database
file. This history policy does not delete backups, native Claude/Codex session
files, workspaces, or artifact metadata. Filesystem cleanup has its own
[storage policy](configuration.md#storage); neither policy is a total disk-space cap.

## Read-only retention preview

Inspect the connected daemon's current policy and decisions:

```sh
aether retention preview
aether retention preview --json
# An isolated daemon can be selected explicitly:
aether --socket /path/to/aetherd.sock retention preview --json
```

Preview connects to an existing daemon; it does not start one, open/recover the
source store, delete source rows or files, publish staged payloads, append
events, or retry cleanup. Automatic maintenance continues independently. This is a point-in-time
estimate, not a reservation: history expiry is simulated in sweep order on an
isolated in-memory SQLite snapshot, using the real deletion path. Deleting an
earlier run in that snapshot can remove references protecting a later run.
The source store and payload files are unchanged; runtime guards are checked
as each run is evaluated, and a later sweep rechecks its guards before deletion.
The snapshot requires memory proportional to the database size.

The JSON object contains `at`, `history_before`, `runs`, `eligible_artifacts`,
`history_payload_bytes`, `artifact_payload_bytes`, and `pending_cleanup`. Each
run has a `decision` tagged by `status` (`eligible` or `protected`); protected
runs include the first `reason` in the real retention guard order. `events` and
`rows` count the records an eligible run would lose; protected runs report zero.
Active runs and runs within the age window are included, even though automatic
candidate discovery skips them. Archiving is a view filter, not a retention pin.
There is no separate pinned-run policy.

Reasons come from the deletion path: unfinished work, recent state or events,
memory scope/provenance, approval rules, leases, pending approvals, schedules
(including paused schedules) and occurrences, artifacts, external references,
submission replay records, pending cancellation, in-flight/native engine turns,
and held resources. Only the first blocker is reported, not every reference.
Artifact metadata protects its run even after payload eviction.

`history_payload_bytes` estimates serialized JSON payload bytes removed from
SQLite, excluding indexes, page overhead, and the expiry spans retained for
cursor safety. Freed pages can be reused; the database file may not shrink.
`artifact_payload_bytes` is the logical size selected by the same artifact
policy as eviction, counting shared digests once. It is not a guaranteed disk
reclamation figure (files can be missing, external, or share other live paths).
Neither figure includes pending cleanup.

`pending_cleanup.pending_unlinks` lists durable unlink retry records, which may
still share live payloads. `pending_cleanup.staged_publications` lists staged
files awaiting publication or startup recovery; some may still have active
publication leases. Preview only lists these records/files and leaves recovery
to its existing lifecycle. Scan/query errors fail visibly rather than yielding
an incomplete report.
