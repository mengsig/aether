"""The upgrade lane's interrupted-migration case.

The store the previous release wrote is opened by the new daemon, which backs
it up and migrates it. The daemon is killed (SIGKILL) at points through that
upgrade, then started again: the second start must either complete the
migration with every agent kept, or refuse with a message naming the store.
"""
from pathlib import Path
import re
import shutil
import sqlite3
import time

from upgrade_compat_fixture import Daemon, LaneFailure, require

ROOT = Path(__file__).resolve().parent.parent
SCHEMA_RS = ROOT / "crates" / "aether-store" / "src" / "schema.rs"
STORE_FILE = "aether.db"
BACKUP_DIR = "migration-backups"
TRIGGER_SECONDS = 10
POLL_SECONDS = 0.002
# What undoes the newest migration block, for when the previous release is
# already on this build's schema. Add the next block's undo when it lands.
REWINDS = {
    24: ("DROP TABLE engine_session_ownership",),
}


def schema_version(schema_rs=SCHEMA_RS):
    """The `SCHEMA_VERSION` this tree's store migrates to."""
    match = re.search(r"pub const SCHEMA_VERSION: u32 = (\d+);", schema_rs.read_text())
    assert match, f"{schema_rs} declares no SCHEMA_VERSION"
    return int(match.group(1))


def stamp(store):
    """The schema version stamped in a store, read without changing it."""
    assert store.is_file(), f"no store at {store}"
    with sqlite3.connect(f"file:{store}?mode=ro", uri=True) as conn:
        row = conn.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()
    assert row is not None, f"{store} has no schema_version stamp"
    return int(row[0])


def agent_ids(store):
    with sqlite3.connect(f"file:{store}?mode=ro", uri=True) as conn:
        return {row[0] for row in conn.execute("SELECT id FROM agents")}


def rewind(store, target):
    """Leave `store` one schema version below `target` when it is not already
    below it, so the new daemon has a migration to run; the version it is at."""
    found = stamp(store)
    require(found <= target, f"the previous release's store is at schema {found}, "
                             f"newer than this tree's {target}")
    if found < target:
        return found
    undo = REWINDS.get(found)
    require(undo is not None, f"the previous release is on schema {found} too, and REWINDS in "
                              f"{Path(__file__).name} has no undo for it: add one")
    with sqlite3.connect(store) as conn:
        for statement in undo:
            conn.execute(statement)
        conn.execute("UPDATE meta SET value = ? WHERE key = 'schema_version'", (str(found - 1),))
    require(stamp(store) == found - 1, f"rewinding {store} did not take")
    return found - 1


def backup_started(data):
    directory = data / BACKUP_DIR
    return directory.is_dir() and any(directory.iterdir())


def backup_written(data):
    return any((data / BACKUP_DIR).glob("*.sqlite.zst"))


def after(seconds):
    started = time.monotonic()
    return lambda _data: time.monotonic() - started >= seconds


# Each point names when the kill lands; the delays reach past a small store's upgrade.
KILL_POINTS = (
    ("at start", lambda: after(0)),
    ("once the backup has begun", lambda: backup_started),
    ("once the backup is written", lambda: backup_written),
    ("after 50 ms", lambda: after(0.05)),
    ("after 150 ms", lambda: after(0.15)),
    ("after 400 ms", lambda: after(0.4)),
)


def interrupt(fixture, new_binary, name, trigger):
    """Start the new daemon and SIGKILL it once `trigger` holds. Every point
    must come: the backup files stay, so a backup point that never comes means
    the upgrade made no backup."""
    daemon = Daemon(fixture, new_binary, f"migration-{name.replace(' ', '-')}")
    deadline = time.monotonic() + TRIGGER_SECONDS
    fired = False
    while time.monotonic() < deadline and daemon.process.poll() is None:
        if trigger(fixture.data):
            fired = True
            break
        time.sleep(POLL_SECONDS)
    exited = daemon.process.poll()
    daemon.kill()
    require(exited is None, f"the new daemon exited {exited} by itself before the kill "
                            f"{name}:\n{daemon.log_tail()}")
    require(fired, f"the kill point `{name}` never came within {TRIGGER_SECONDS} s:\n"
                   f"{daemon.log_tail()}")


def restart(fixture, binaries, name, store):
    """Start again after the kill: completed (True) or refused clearly (False)."""
    daemon = Daemon(fixture, binaries.new, f"restart-{name.replace(' ', '-')}")
    welcome = daemon.wait_ready(binaries.negotiate)
    if welcome is None:
        log = daemon.log_tail()
        require("panicked" not in log and str(store) in log,
                f"after a kill {name} the next start exited {daemon.process.returncode} "
                f"without a message naming {store}:\n{log}")
        return False
    daemon.stop(binaries.new)
    require(welcome["schema_version"] == schema_version(),
            f"after a kill {name} the restarted daemon reports schema "
            f"{welcome['schema_version']}, not {schema_version()}")
    return True


def check_interrupted_migration(fixture, binaries, pristine):
    """Run every kill point on a fresh copy of `pristine`, a data directory the
    previous release wrote; the data directory of the last completed upgrade.
    A copy that passed is deleted once a later one completes, bounding the disk."""
    target = schema_version()
    rewound = fixture.root / "store-rewound"
    shutil.copytree(pristine, rewound)
    origin = rewind(rewound / STORE_FILE, target)
    agents = agent_ids(rewound / STORE_FILE)
    require(agents, "the previous release's store holds no agent to carry across")
    print(f"  store at schema {origin}, {len(agents)} agents; the new build migrates to {target}")
    completed = None
    for index, (name, make_trigger) in enumerate(KILL_POINTS):
        work = fixture.root / f"store-killed-{index}"
        shutil.copytree(rewound, work)
        fixture.data = work
        interrupt(fixture, binaries.new, name, make_trigger())
        left = stamp(work / STORE_FILE)
        require(left in (origin, target),
                f"a kill {name} left the store at schema {left}, neither {origin} nor {target}")
        if not restart(fixture, binaries, name, work / STORE_FILE):
            print(f"  killed {name}: refused clearly on restart")
            shutil.rmtree(work)
            continue
        require(stamp(work / STORE_FILE) == target, f"after a kill {name} the store stayed "
                                                    f"at schema {stamp(work / STORE_FILE)}")
        missing = agents - agent_ids(work / STORE_FILE)
        require(not missing, f"after a kill {name} the upgraded store lost agents {missing}")
        print(f"  killed {name}: left at schema {left}, completed on restart")
        if completed is not None:
            shutil.rmtree(completed)
        completed = work
    if completed is None:
        raise LaneFailure("no interrupted upgrade completed on restart")
    return completed
