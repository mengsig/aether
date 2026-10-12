"""Deterministic guards for measurement parsing, replay validation and cleanup."""

import contextlib
import os
from pathlib import Path
import shutil
import signal
import sys
import tempfile
import time
import tomllib
import unittest
from unittest.mock import MagicMock, patch

from tui_process_budgets import (
    VOLATILE, ControllerCancelled, ControllerDeadline, Fixture, controller_limits, deadline_expired,
    assert_quiescent_root, await_waiting_root, parse_cpu_ticks, records, validate_replay,
)


@unittest.skipUnless(sys.platform == "linux", "Linux process fixture")
class ProcessBudgetTests(unittest.TestCase):
    def test_fixture_configures_a_model_without_credentials_or_installed_engines(self):
        with contextlib.closing(Fixture(sys.executable)) as fixture:
            config = tomllib.loads((fixture.state / "config/config.toml").read_text())
            providers = tomllib.loads((fixture.state / "config/providers.toml").read_text())
            self.assertEqual(config["model"]["default"], "idle-fixture/idle")
            provider = providers["providers"]["idle-fixture"]
            self.assertEqual(provider["protocol"], "openai-chat-completions")
            self.assertEqual(provider["auth"], {"none": True})
            self.assertEqual(provider["models"][0]["id"], "idle")
            self.assertEqual(provider["base_url"], "http://127.0.0.1:9/v1")

    def test_fixture_keeps_receipts_relative_under_a_symlinked_target(self):
        with tempfile.TemporaryDirectory(prefix="ci-target-", dir="target") as directory:
            root = Path(directory).resolve()
            (root / "cache").mkdir()
            (root / "checkout").mkdir()
            (root / "checkout" / "target").symlink_to(root / "cache", target_is_directory=True)
            with contextlib.chdir(root / "checkout"), contextlib.closing(Fixture(sys.executable)) as fixture:
                self.assertFalse(fixture.root.is_absolute())
                self.assertEqual(fixture.root.parts[0], "target")
                self.assertTrue(fixture.root.resolve().is_relative_to(root / "cache"))

    def test_daemon_state_is_memory_backed_while_running_and_kept_with_the_receipts(self):
        with contextlib.closing(Fixture(sys.executable)) as fixture:
            self.assertEqual(fixture.state.parent, VOLATILE)
            self.assertLess(len(os.fsencode(fixture.socket)), 104)
            for name, path in (("AETHER_SOCKET", "s"), ("AETHER_CONFIG_DIR", "config"), ("AETHER_DATA_DIR", "data")):
                self.assertEqual(fixture.env[name], str(fixture.state / path))
            self.assertEqual(list(fixture.root.iterdir()), [])
            (fixture.state / "data/aether.db").write_text("store")
        self.assertEqual((fixture.root / "data/aether.db").read_text(), "store")
        self.assertTrue((fixture.root / "config/config.toml").is_file())
        self.assertFalse(fixture.state.exists())

    def test_daemon_state_that_cannot_be_kept_fails_cleanup_and_stays_in_place(self):
        with self.assertRaisesRegex(RuntimeError, "fixture cleanup failed.*fixture state left in"):
            with contextlib.closing(Fixture(sys.executable)) as fixture:
                self.addCleanup(shutil.rmtree, fixture.state)
                (fixture.root / "data").write_text("occupied")
        self.assertTrue((fixture.state / "data").is_dir())

    def test_idle_fixture_accepts_only_the_quiescent_waiting_root(self):
        idle = {"agents_live": 1, "agents_waiting": 1, "agents_running": 0,
                "agents_blocked": 0, "tasks_active": 0, "engine_turns_running": 0}
        assert_quiescent_root({"data": idle})
        for field in idle:
            busy = dict(idle, **{field: idle[field] + 1})
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_quiescent_root({"data": busy})

    def test_idle_sampling_waits_for_the_waiting_root_not_for_a_fixed_delay(self):
        fixture = MagicMock()
        client = fixture.connect.return_value.__enter__.return_value
        client.request.side_effect = [{"data": {"agents_waiting": 0}}, {"data": {"agents_waiting": 1}}]
        status = await_waiting_root(fixture, child=object(), seconds=5)
        self.assertEqual(status["data"]["agents_waiting"], 1)
        self.assertEqual(client.request.call_count, 2)
        self.assertEqual(fixture.alive.call_count, 2)

    def test_a_cli_that_never_creates_its_root_fails_with_the_last_status(self):
        fixture = MagicMock()
        fixture.connect.return_value.__enter__.return_value.request.return_value = {"data": {"agents_waiting": 0}}
        with self.assertRaisesRegex(AssertionError, "no waiting root in 0.1s.*agents_waiting"):
            await_waiting_root(fixture, child=object(), seconds=.1)

    def test_cpu_counters_include_user_and_system_not_children(self):
        # The comm field can contain spaces and parentheses; child CPU is not ours.
        stat = "123 (aether (tui)) S " + "0 " * 10 + "17 23 999 888"
        self.assertEqual(parse_cpu_ticks(stat), 40)

    def test_invalid_cpu_counters_do_not_turn_into_zero(self):
        for stat in ("garbage", "1 (tui) S", "1 (tui) S " + "0 " * 10 + "-1 2"):
            with self.subTest(stat=stat), self.assertRaises((IndexError, ValueError)):
                parse_cpu_ticks(stat)

    def test_replay_requires_exact_order_content_and_reducer_application(self):
        expected = [{"seq": 2, "content": "before"}, {"seq": 3, "content": "missed"}]
        def observed(events, cursor=None):
            rows = [{"kind": "event", "event": event} for event in events]
            rows[-1]["cursor"] = events[-1]["seq"] if cursor is None else cursor
            return rows
        validate_replay(expected, observed(expected))
        for wrong in (expected[:1], expected + expected[1:], list(reversed(expected)),
                      [expected[0], {"seq": 3, "content": "corrupt"}]):
            with self.subTest(wrong=wrong), self.assertRaisesRegex(AssertionError, "mismatch"):
                validate_replay(expected, observed(wrong))
        with self.assertRaisesRegex(AssertionError, "cursor"):
            validate_replay(expected, observed(expected, cursor=2))

    def test_partial_receipt_is_not_mistaken_for_corrupt_or_complete_event(self):
        with contextlib.closing(Fixture(sys.executable)) as fixture:
            receipt = fixture.root / "partial.jsonl"
            receipt.write_text('{"kind":"subscribed"}\n{"kind":')
            self.assertEqual(records(receipt), [{"kind": "subscribed"}])
            receipt.write_text('{not json}\n')
            with self.assertRaises(ValueError):
                records(receipt)

    def test_failure_reaps_stopped_child_and_preserves_original_cause(self):
        child = None
        with self.assertRaisesRegex(RuntimeError, "original fixture failure"):
            with contextlib.closing(Fixture(sys.executable)) as fixture:
                child = fixture.spawn([sys.executable, "-c", "import time; time.sleep(30)"], "child.log")
                child.send_signal(signal.SIGSTOP)
                raise RuntimeError("original fixture failure")
        self.assertIsNotNone(child.returncode)
        self.assertFalse(Path(f"/proc/{child.pid}").exists())

    def test_controller_signals_unwind_and_reap_stopped_child(self):
        for delivered, error in ((signal.SIGTERM, ControllerCancelled), (signal.SIGALRM, ControllerDeadline)):
            with self.subTest(signal=delivered):
                previous = signal.getsignal(delivered)
                with self.assertRaises(error), controller_limits():
                    with contextlib.closing(Fixture(sys.executable)) as fixture:
                        child = fixture.spawn([sys.executable, "-c", "import time; time.sleep(30)"], "child.log")
                        fixture.pause(child)
                        os.kill(os.getpid(), delivered)
                self.assertEqual(signal.getsignal(delivered), previous)
                self.assertIsNotNone(child.returncode)
                self.assertFalse(Path(f"/proc/{child.pid}").exists())

    def test_controller_deadline_is_never_a_retryable_readiness_error(self):
        with contextlib.closing(Fixture(sys.executable)) as fixture:
            child = fixture.spawn([sys.executable, "-c", "import time; time.sleep(30)"], "child.log")
            ready = MagicMock()
            ready.__enter__.return_value.request.return_value = {"data": {}}
            calls = 0
            def connect():
                nonlocal calls
                calls += 1
                if calls == 1:
                    deadline_expired(signal.SIGALRM, None)
                return ready
            with patch.object(fixture, "spawn", return_value=child), patch.object(fixture, "connect", connect):
                with self.assertRaisesRegex(ControllerDeadline, "process budget deadline"):
                    fixture.start_daemon()
            self.assertEqual(calls, 1, "global deadline was retried")

    def test_repeated_stop_never_resignals_a_retired_group(self):
        with patch("tui_process_budgets.os.killpg", wraps=os.killpg) as kill_group:
            with contextlib.closing(Fixture(sys.executable)) as fixture:
                child = fixture.spawn([sys.executable, "-c", "import time; time.sleep(30)"], "child.log")
                fixture.stop(child)
                fixture.stop(child)
            self.assertEqual(kill_group.call_count, 1)
            self.assertIsNotNone(child.returncode)

    def test_signal_during_close_finishes_all_groups_and_preserves_cause(self):
        for delivered, error in ((signal.SIGTERM, ControllerCancelled), (signal.SIGALRM, ControllerDeadline)):
            for original in (False, True):
                with self.subTest(signal=delivered, original=original), controller_limits():
                    expected = ValueError if original else error
                    with self.assertRaises(expected) as raised:
                        with contextlib.closing(Fixture(sys.executable)) as fixture:
                            children = [fixture.spawn([sys.executable, "-c", "import time; time.sleep(30)"],
                                                      f"child-{i}.log") for i in range(2)]
                            stop = fixture.stop
                            calls = 0
                            def interrupted_stop(child):
                                nonlocal calls
                                calls += 1
                                if calls == 1:
                                    os.kill(os.getpid(), delivered)
                                stop(child)
                            fixture.stop = interrupted_stop
                            if original:
                                raise ValueError("original fixture failure")
                    self.assertEqual(calls, 2, "signal interrupted cleanup before all groups stopped")
                    for child in children:
                        self.assertIsNotNone(child.returncode)
                        self.assertFalse(Path(f"/proc/{child.pid}").exists())
                    if original:
                        self.assertEqual(str(raised.exception), "original fixture failure")
                        self.assertIn("process budget", " ".join(raised.exception.__notes__))

    def test_cleanup_kills_owned_group_after_its_leader_exits(self):
        leader = None
        try:
            with contextlib.closing(Fixture(sys.executable)) as fixture:
                script = ("import subprocess, sys; "
                          "p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)']); "
                          "print(p.pid, flush=True)")
                leader = fixture.spawn([sys.executable, "-c", script], "group.log")
                leader.wait(timeout=5)
                descendant = int((fixture.root / "group.log").read_text())
                self.assertEqual(os.getpgid(descendant), leader.pid)
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                try:
                    state = Path(f"/proc/{descendant}/stat").read_text().rsplit(")", 1)[1].split()[0]
                except (FileNotFoundError, ProcessLookupError):
                    break  # Reaped already; a read that races the reaper gets ESRCH.
                if state == "Z":
                    break  # Only the adopting parent can reap this killed descendant.
                time.sleep(.01)
            else:
                self.fail("owned descendant survived group cleanup")
            self.assertIsNotNone(leader.returncode)
        finally:
            if leader is not None:
                try:
                    os.killpg(leader.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                leader.wait(timeout=5)

    def test_early_child_failure_is_visible_with_artifact_path(self):
        with contextlib.closing(Fixture(sys.executable)) as fixture:
            child = fixture.spawn([sys.executable, "-c", "raise SystemExit(7)"], "child.log")
            child.wait(timeout=5)
            with self.assertRaisesRegex(AssertionError, "exited 7; logs in target/"):
                fixture.alive(child)


if __name__ == "__main__":
    unittest.main()
