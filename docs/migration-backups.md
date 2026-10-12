# Pre-migration database backups

Opening an existing database with an older schema first creates a SQLite
snapshot in `migration-backups/` next to that database. The filename records the
old and new schema versions plus a unique ID. Fresh databases and databases
already on the current schema do not create another snapshot.

The snapshot uses SQLite's backup API, preserving committed WAL data and row
IDs used by memory search. A temporary file is published under its final name
only after the copy completes and is synced. The backup directory is private
on Unix. If copying or syncing fails, startup fails before migration begins;
Aether does not silently proceed without the backup. Existing snapshots are
never overwritten or automatically pruned.

These are database snapshots, not complete workspace archives. Artifact bytes
remain in the database's sibling `artifacts/` directory, and the images sent
with messages in `aether.attachments/`; retain those directories when
preserving a rollback point. A rollback also requires a compatible Aether
binary and an orderly daemon stop. Do not replace an active daemon's database,
and do not open the snapshot with a newer Aether binary merely to inspect it:
that would migrate the snapshot too. A read-only SQLite connection can inspect
its original schema without changing it.
