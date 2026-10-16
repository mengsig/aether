# Pre-migration database backups

Opening an existing database with an older schema first creates a
zstd-compressed SQLite snapshot in `migration-backups/` next to that database,
named `schema-<old>-before-<new>-<id>.sqlite.zst`. Fresh databases and
databases already on the current schema do not create another snapshot.
Backups written by older versions are plain `.sqlite` files and are managed
the same way.

The snapshot uses SQLite's backup API, preserving committed WAL data and row
IDs used by memory search. Aether checks that the SQLite header accounts for
every byte of the copy, compresses it, then decompresses the result and
compares its SHA-256 with the copy. The compressed file is published under its
final name only after that check passes and it is synced; the uncompressed
copy is then deleted. The backup directory is private on Unix. If copying,
checking or syncing fails, startup fails before migration begins; Aether does
not silently proceed without the backup. Existing snapshots are never
overwritten.

Background storage maintenance, which runs at startup and hourly, keeps at
most the newest `[storage] migration_backups_keep` snapshots (two by default),
whatever their age. Below that cap it keeps the newest `backups_keep` and
anything within `backups_days` (fourteen days by default); `backups_days = 0`
turns off only the age rule. `aether storage` lists the backups' bytes and the
decision for each file. See [storage configuration](configuration.md#storage).

## Restoring a backup

`aether stop` returns once the daemon has accepted the request, before it has
finished writing the database. The daemon holds an exclusive lock on
`aether.db.lock` until its process exits, so run the restore under that lock:
`flock` waits for the daemon to be gone and keeps a new one from starting
until the restore is done.

```sh
aether stop
cd ~/.local/share/aether            # the directory holding aether.db
flock aether.db.lock sh -euc '
  cp aether.db aether.db.before-restore
  zstd -d -f migration-backups/schema-21-before-22-<id>.sqlite.zst -o aether.db
  rm -f aether.db-wal aether.db-shm
'
```

Without `flock` (macOS has none by default), wait until
`lsof aether.db aether.db.lock` lists no process before running the three
commands, and start no daemon until they are done.

The file is a single standard zstd frame, so `unzstd` or any zstd library
gives the same bytes. To look at a snapshot without restoring it, decompress
it to another path and open that read-only.

These are database snapshots, not complete workspace archives. Artifact bytes
remain in the database's sibling `artifacts/` directory, and the images sent
with messages in `aether.attachments/`; retain those directories when
preserving a rollback point. A rollback also requires a compatible Aether
binary and an orderly daemon stop. Do not replace an active daemon's database,
and do not open the snapshot with a newer Aether binary merely to inspect it:
that would migrate the snapshot too. A read-only SQLite connection can inspect
its original schema without changing it.
