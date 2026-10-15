#!/usr/bin/env python3
"""Drive the real runner's EventStream through shell job control with a DSR responder."""

import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def check(executable):
    cache = Path.home() / ".cache"
    cache.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="aether-suspend-", dir=cache) as directory:
        root = Path(directory)
        for name in ("config", "data"):
            (root / name).mkdir()
        receipt = root / "receipt.jsonl"
        display_vars = {"DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"}
        env = {k: v for k, v in os.environ.items()
               if not k.startswith("AETHER_") and k not in display_vars}
        env.update(AETHER_CONFIG_DIR=str(root / "config"), AETHER_DATA_DIR=str(root / "data"),
                   AETHER_SOCKET=str(root / "unused.sock"), AETHER_SUSPEND_RECEIPT=str(receipt),
                   TERM="xterm-256color", PS1="SUSPEND_SHELL> ", PROMPT_COMMAND="")
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        output = bytearray()
        responses = 0
        shell = None
        runner_group = None

        def controlling_terminal():
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        def pump():
            nonlocal responses
            if select.select([master], [], [], .02)[0]:
                queries = output.count(b"\x1b[6n")
                output.extend(os.read(master, 65536))
                for _ in range(output.count(b"\x1b[6n") - queries):
                    os.write(master, b"\x1b[1;1R")
                    responses += 1

        def wait_for(predicate, description):
            deadline = time.monotonic() + 10
            while not predicate():
                assert shell.poll() is None, (description, shell.returncode, bytes(output))
                assert time.monotonic() < deadline, (description, bytes(output))
                pump()

        def records():
            if not receipt.exists():
                return []
            lines = receipt.read_bytes().split(b"\n")[:-1]
            return [json.loads(line) for line in lines]

        def conversation_drawn(start=0):
            # Ratatui can skip spaces with cursor moves between rendered words.
            return all(word in output[start:] for word in (b"conversation", b"before", b"suspension"))

        def input_record(draft, paste=None):
            start = len(records())
            text = draft if paste is None else paste
            paste_bytes = b"\x1b[200~" + text.encode() + b"\x1b[201~" if text else b""
            os.write(master, paste_bytes + b"\x1b[I")
            wait_for(lambda: any(r.get("draft") == draft for r in records()[start:]),
                     "input reached the reducer")
            return next(r for r in reversed(records()[start:]) if r.get("draft") == draft)

        try:
            shell = subprocess.Popen(
                ["bash", "--noprofile", "--norc", "--noediting", "-i"],
                cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave,
                start_new_session=True, preexec_fn=controlling_terminal,
            )
            wait_for(lambda: b"SUSPEND_SHELL> " in output, "shell ready")
            cooked = termios.tcgetattr(slave)
            command = shlex.join([str(Path(executable).resolve()), "--exact",
                                  "runner::suspend_tests::suspend_loop_child",
                                  "--ignored", "--nocapture"])
            os.write(master, command.encode() + b"\n")
            wait_for(lambda: os.tcgetpgrp(master) != shell.pid, "runner owns the foreground")
            runner_group = os.tcgetpgrp(master)
            assert runner_group != shell.pid, "runner must own the foreground terminal"
            wait_for(lambda: any(r["kind"] == "backfill" for r in records()), "history loaded")
            wait_for(conversation_drawn, "initial draw")
            expected = input_record("")
            assert "conversation before suspension" in expected["transcript"], expected

            for cycle, draft in enumerate(("", "unsent café 👩🏽‍💻 draft", "another draft")):
                if cycle:
                    # Replace the previous draft through real editor input.
                    os.write(master, b"\x15")  # Ctrl+U
                    wait_for(lambda: records()[-1].get("draft") == "", "clear draft")
                    expected = input_record(draft)
                start = len(output)
                os.write(master, b"\x1a")  # Ctrl+Z, handled by the running EventStream.
                wait_for(lambda: b"SUSPEND_SHELL> " in output[start:], "job stopped at shell")
                state = Path(f"/proc/{runner_group}/stat").read_text().rsplit(")", 1)[1].split()[0]
                assert state == "T", ("runner did not stop", state)
                assert termios.tcgetattr(slave) == cooked, "suspend did not restore shell modes"
                stopped = bytes(output[start:])
                assert b"\x1b[?1049l" in stopped and b"\x1b[<1u" in stopped, stopped
                # Resize while stopped; resume must reconcile size even before fresh input.
                width, height = 100 + cycle, 35 + cycle
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
                start = len(output)
                previous = len(records())
                os.write(master, b"fg\n")
                wait_for(lambda: any(r["kind"] == "resumed" for r in records()[previous:])
                         or b"SUSPEND_SHELL> " in output[start:],
                         "fg returned to the running input loop")
                if b"SUSPEND_SHELL> " in output[start:]:
                    assert termios.tcgetattr(slave) == cooked, "failed resume leaked terminal modes"
                    raise AssertionError(("fg exited the TUI; shell modes were restored",
                                          f"DSR replies sent: {responses}", bytes(output[start:])))
                resumed = next(r for r in records()[previous:] if r["kind"] == "resumed")
                for field in ("draft", "selected", "transcript", "cursor"):
                    assert resumed[field] == expected[field], (field, expected, resumed)
                assert resumed["size"] == [width, height], resumed
                assert termios.tcgetattr(slave) != cooked, "resume did not enter raw mode"
                wait_for(lambda: conversation_drawn(start), "resume redraw")
                wait_for(lambda: all(word.encode() in output[start:] for word in draft.split()),
                         "composer redrawn")
                assert b"\x1b[?1049h" in output[start:] and b"\x1b[>1u" in output[start:]
                assert b"\x1b[6n" not in output[start:], "resume queried the cursor with active input"
                expected = input_record(draft + "!", "!")
                assert expected["selected"] == resumed["selected"]

            start = len(output)
            os.killpg(runner_group, signal.SIGTERM)
            wait_for(lambda: b"SUSPEND_SHELL> " in output[start:], "runner exited cleanly")
            runner_group = None
            assert b"test result: ok" in output[start:], bytes(output[start:])
            assert termios.tcgetattr(slave) == cooked, "exit did not restore shell modes"
            assert output.count(b"\x1b[>1u") == output.count(b"\x1b[<1u") == 4
            assert output.count(b"\x1b[?1049h") == output.count(b"\x1b[?1049l") == 4
            assert b"cursor position could not be read" not in output, bytes(output)
            os.write(master, b"exit\n")
            assert shell.wait(timeout=5) == 0
            print("running input loop: empty/Unicode drafts, three suspend/fg cycles, resize, "
                  "state preservation, resumed input and terminal cleanup: PASS", flush=True)
        finally:
            if runner_group is not None:
                try:
                    if os.getsid(runner_group) == shell.pid:
                        os.killpg(runner_group, signal.SIGKILL)
                except ProcessLookupError:
                    pass  # This owned job already exited.
            if shell is not None and shell.poll() is None:
                shell.kill()
                shell.wait(timeout=5)
            os.close(slave)
            os.close(master)


if __name__ == "__main__":
    check(sys.argv[1])
