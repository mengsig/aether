"""Tests for the upgrade lane's offline parts: release choice, checksums, the
schema rewind, the fake ssh and the fake `claude`. The lane itself needs the
network and two builds; .github/workflows/upgrade.yml runs it."""
import contextlib
import io
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import textwrap
import types
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import upgrade_compat as lane  # noqa: E402
import upgrade_compat_fixture as fixture  # noqa: E402
import upgrade_compat_migration as migration  # noqa: E402
import upgrade_compat_release as release  # noqa: E402

ENGINE = Path(__file__).resolve().parent / "upgrade_compat_engine.py"

STUB_BRIDGE = textwrap.dedent("""\
    import json, sys
    fail = sys.argv[1] == "fail"
    for line in sys.stdin:
        frame = json.loads(line)
        if "id" not in frame:
            continue
        method = frame["method"]
        if method == "initialize":
            result = {"serverInfo": {"name": "stub"}}
        elif method == "tools/list":
            result = {"tools": [{"name": "inspect_state"}]}
        else:
            result = {"content": [{"type": "text", "text": "state"}], "isError": fail}
        print(json.dumps({"jsonrpc": "2.0", "id": frame["id"], "result": result}), flush=True)
""")


def published(tag, draft=False, prerelease=False):
    return {"tag_name": tag, "draft": draft, "prerelease": prerelease}


class ReleaseChoice(unittest.TestCase):
    def test_the_newest_published_release_at_or_below_the_workspace(self):
        releases = [published("v1.0.11", draft=True), published("v1.0.9"),
                    published("v1.0.10"), published("v1.1.0-rc.1", prerelease=True),
                    published("nightly"), published("v1.0.12")]
        self.assertEqual(release.select_previous(releases, (1, 0, 11)), "v1.0.10")
        self.assertEqual(release.select_previous(releases, (1, 0, 10)), "v1.0.10")

    def test_a_release_tag_run_never_picks_its_own_release(self):
        releases = [published("v1.0.10"), published("v1.0.11")]
        self.assertEqual(release.select_previous(releases, (1, 0, 11)), "v1.0.11")
        self.assertEqual(release.select_previous(releases, (1, 0, 11), "v1.0.11"), "v1.0.10")

    def test_no_candidate_is_an_error_not_a_guess(self):
        with self.assertRaisesRegex(ValueError, "no published release"):
            release.select_previous([published("v2.0.0")], (1, 0, 0))

    def test_the_workspace_version_is_read_from_its_own_section(self):
        with tempfile.TemporaryDirectory() as directory:
            cargo = Path(directory) / "Cargo.toml"
            cargo.write_text('[package]\nversion = "9.9.9"\n\n'
                             '[workspace.package]\nversion = "1.2.3"\n')
            self.assertEqual(release.workspace_version(cargo), (1, 2, 3))
        self.assertEqual(len(release.workspace_version()), 3)

    def test_a_checksum_must_name_the_archive(self):
        digest = "ab" * 32
        name = "aether-x86_64-unknown-linux-musl.tar.gz"
        self.assertEqual(release.expected_digest(f"{digest}  {name}\n", name), digest)
        with self.assertRaises(ValueError):
            release.expected_digest(f"{digest}  other.tar.gz\n", name)
        with self.assertRaises(ValueError):
            release.expected_digest("not-a-digest\n", name)


def store_at(directory, version, tables=()):
    path = Path(directory) / "aether.db"
    with sqlite3.connect(path) as conn:
        conn.execute("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        conn.execute("CREATE TABLE agents (id TEXT PRIMARY KEY)")
        conn.execute("INSERT INTO meta VALUES ('schema_version', ?)", (str(version),))
        for table in tables:
            conn.execute(f"CREATE TABLE {table} (x)")
    return path


def tables(path):
    with sqlite3.connect(path) as conn:
        return {row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}


class SchemaRewind(unittest.TestCase):
    def test_every_schema_version_can_be_rewound(self):
        current = migration.schema_version()
        self.assertIn(current, migration.REWINDS,
                      f"schema {current} has no undo in REWINDS: once a release ships it, the "
                      f"upgrade lane cannot make the next build migrate its store")

    def test_a_store_on_the_same_schema_goes_back_one_version(self):
        with tempfile.TemporaryDirectory() as directory:
            store = store_at(directory, 24, tables=["engine_session_ownership"])
            self.assertEqual(migration.rewind(store, 24), 23)
            self.assertEqual(migration.stamp(store), 23)
            self.assertNotIn("engine_session_ownership", tables(store))

    def test_an_older_store_is_left_alone(self):
        with tempfile.TemporaryDirectory() as directory:
            store = store_at(directory, 22)
            self.assertEqual(migration.rewind(store, 24), 22)
            self.assertEqual(migration.stamp(store), 22)

    def test_a_newer_or_unknown_store_fails_the_lane(self):
        with tempfile.TemporaryDirectory() as directory:
            store = store_at(directory, 25)
            with self.assertRaisesRegex(fixture.LaneFailure, "newer than"):
                migration.rewind(store, 24)
            with self.assertRaisesRegex(fixture.LaneFailure, "no undo"):
                migration.rewind(store, 25)


class FakeSsh(unittest.TestCase):
    def test_it_runs_the_remote_command_locally(self):
        with tempfile.TemporaryDirectory() as directory:
            ssh = Path(directory) / "ssh"
            fixture.write_executable(ssh, fixture.FAKE_SSH)
            done = subprocess.run([ssh, "-T", "-oBatchMode=yes", "--", "host", "echo 'it ran'"],
                                  capture_output=True, text=True, timeout=10)
            self.assertEqual((done.returncode, done.stdout), (0, "it ran\n"))
            refused = subprocess.run([ssh, "host"], capture_output=True, text=True, timeout=10)
            self.assertEqual(refused.returncode, 255)


class FakeEngine(unittest.TestCase):
    def turn(self, mode):
        """One `-p` turn of the fake engine against a stub bridge; its frames and record."""
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            bridge = state / "bridge.py"
            bridge.write_text(STUB_BRIDGE)
            (state / "swapped").write_text("")
            config = {"mcpServers": {"aether": {"command": sys.executable,
                                                "args": [str(bridge), mode]}}}
            prompt = {"type": "user", "message": {"role": "user", "content": "hi"}}
            done = subprocess.run(
                [sys.executable, ENGINE, "--state", state, "--installed", sys.executable,
                 "-p", "--session-id", "s1",
                 "--mcp-config", json.dumps(config)],
                input=json.dumps(prompt) + "\n", capture_output=True, text=True, timeout=60)
            self.assertEqual(done.returncode, 0, done.stderr)
            frames = [json.loads(line) for line in done.stdout.splitlines()]
            return frames, json.loads((state / "session.json").read_text())

    def test_a_turn_calls_the_tool_on_both_bridges_and_succeeds(self):
        frames, record = self.turn("ok")
        self.assertEqual([frame["type"] for frame in frames],
                         ["system", "user", "assistant", "result"])
        self.assertEqual(frames[-1]["subtype"], "success")
        self.assertEqual(frames[-1]["session_id"], "s1")
        self.assertIsNone(record["error"])
        for step in ("before_swap", "after_swap", "second_bridge", "installed_bridge"):
            self.assertEqual(record[step], "state")
        self.assertEqual(record["first_build"]["sha256"], record["second_build"]["sha256"])

    def test_a_failed_tool_call_fails_the_turn_and_says_why(self):
        frames, record = self.turn("fail")
        self.assertTrue(frames[-1]["is_error"])
        self.assertIn("inspect_state failed", record["error"])
        self.assertIn("inspect_state failed", frames[-1]["result"])

    def test_it_reports_a_claude_version(self):
        with tempfile.TemporaryDirectory() as directory:
            done = subprocess.run([sys.executable, ENGINE, "--state", directory,
                                   "--installed", sys.executable, "--version"],
                                  capture_output=True, text=True, timeout=10)
            self.assertEqual(done.stdout.strip(), "2.1.265 (Claude Code)")


class PreviousDaemonBridge(unittest.TestCase):
    """Both bridge strategies a previous release may have pass; a drift fails."""
    sandbox = types.SimpleNamespace(installed=Path("/sandbox/install/aether"))
    binaries = lane.Binaries(Path("previous"), Path("new"), Path("negotiate"), "1.0.10")

    def check(self, command, second):
        record = {"command": command, "first_build": {"sha256": "previous"},
                  "second_build": {"sha256": second}}
        with mock.patch.object(lane, "native_session", return_value=record), \
                mock.patch.object(lane, "digest", side_effect=lambda path: str(path)), \
                contextlib.redirect_stdout(io.StringIO()):
            lane.check_previous_daemon_session(self.sandbox, self.binaries)

    def test_a_daemon_launching_the_installed_path_gets_the_new_build(self):
        self.check(str(self.sandbox.installed), "new")
        with self.assertRaises(fixture.LaneFailure):
            self.check(str(self.sandbox.installed), "previous")

    def test_a_daemon_launching_its_own_copy_keeps_its_build(self):
        self.check("/sandbox/data/bin/aether-1.0.11-x", "previous")
        with self.assertRaisesRegex(fixture.LaneFailure, "changed build"):
            self.check("/sandbox/data/bin/aether-1.0.11-x", "new")


if __name__ == "__main__":
    unittest.main()
