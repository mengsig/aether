#!/usr/bin/env python3
"""Press the documented Alt shortcuts as real terminals encode them.

Runs the real runner's EventStream in a PTY against a fixture daemon and
checks, for every encoding class, that each chord decodes as Alt and does
what its binding promises. `--report` prints the matrix instead of failing.
"""

import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="
)
FAKE_WL_PASTE = """#!/bin/sh
case "$*" in
    --list-types) printf 'image/png\\n' ;;
    '--type image/png') cat "$FAKE_CLIPBOARD/image.png" ;;
    *) exit 64 ;;
esac
"""
# Ctrl+E, Ctrl+U: erase the line; Backspace then joins the line above.
CLEAR = b"\x05\x15"

def flashed(prefix):
    """A new flash starting with `prefix` since the state before the key."""
    def check(after, before, _keys):
        return ((after["flash"] or "").startswith(prefix)
                and after["flash_until"] != before["flash_until"])
    return check


def notices(record):
    return (record["transcript"] or "").count("notification center")


def edited(draft, caret):
    return lambda after, _before, _keys: after["draft"] == draft and after["caret"] == caret


# Per binding: (draft typed first, key, what the binding visibly did). The
# fixture daemon has no notification center, so Alt+N shows that notice.
BINDINGS = {
    "alt+o": (b"", "o", flashed("No local file references")),
    "alt+n": (b"", "n", lambda after, before, _: notices(after) == notices(before) + 1),
    "alt+v": (b"", "v", lambda after, before, _: after["images"] == before["images"] + 1),
    "alt+enter": (b"ab", "\r", edited("ab\n", 3)),
    "alt+backspace": (b"ab cd", "\x7f", edited("ab ", 3)),
    "alt+b": (b"ab cd", "b", edited("ab cd", 3)),
    "alt+f": (b"ab cd\x1b[H", "f", edited("ab cd", 2)),
    "alt+,": (b"", ",", flashed("effort")),
    "alt+.": (b"", ".", flashed("effort")),
    # The fixture agent has no parent: the chord must arrive as Alt+Up.
    "alt+up": (b"", "up", lambda _after, _before, keys: keys == ["KeyModifiers(ALT) Up"]),
}
UP = {"legacy": b"\x1b[1;3A", "kitty": b"\x1b[1;3A", "kitty_locks": b"\x1b[1;131A",
      "modify_other_keys": b"\x1b[1;3A", "split": [b"\x1b", None, b"\x1b[A"],
      "rxvt": b"\x1b\x1b[A"}


def encode(encoding, key):
    """The bytes `encoding` sends for Alt+`key`, `None` marking a split
    between two writes; `None` itself where the encoding has no such form."""
    if key == "up":
        return UP.get(encoding)
    code = ord(key)
    return {
        "legacy": b"\x1b" + key.encode(),
        "kitty": f"\x1b[{code};3u".encode(),
        # Num Lock on: kitty reports lock keys in the modifier field.
        "kitty_locks": f"\x1b[{code};131u".encode(),
        "modify_other_keys": f"\x1b[27;3;{code}~".encode(),
        "eight_bit": chr(code | 0x80).encode(),
        "split": [b"\x1b", None, key.encode()],
    }.get(encoding)


ENCODINGS = ["legacy", "kitty", "kitty_locks", "modify_other_keys", "eight_bit", "split", "rxvt"]


class Tui:
    """The fixture runner in a PTY and the receipt it writes per event."""

    def __init__(self, executable, root):
        self.root = root
        self.receipt = root / "receipt.jsonl"
        self.output = bytearray()
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.child = subprocess.Popen(
            [str(Path(executable).resolve()), "--exact",
             "runner::suspend_tests::fixture_loop_child", "--ignored", "--nocapture"],
            cwd=root, env=fixture_env(root), stdin=slave, stdout=slave, stderr=slave,
            start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
        )
        os.close(slave)

    def pump(self):
        if select.select([self.master], [], [], .02)[0]:
            queries = self.output.count(b"\x1b[6n")
            self.output.extend(os.read(self.master, 65536))
            for _ in range(self.output.count(b"\x1b[6n") - queries):
                os.write(self.master, b"\x1b[1;1R")

    def records(self):
        if not self.receipt.exists():
            return []
        return [json.loads(line) for line in self.receipt.read_bytes().split(b"\n")[:-1]]

    def wait_for(self, predicate, description, start=0, timeout=10):
        deadline = time.monotonic() + timeout
        while True:
            found = next((r for r in self.records()[start:] if predicate(r)), None)
            if found is not None:
                return found
            assert self.child.poll() is None, (description, self.child.returncode,
                                               bytes(self.output[-2000:]))
            if time.monotonic() >= deadline:
                return None
            self.pump()

    def send(self, keys):
        """Write `keys`; a `None` part splits them into two reads."""
        parts = keys if isinstance(keys, list) else [keys]
        for part in parts:
            if part is None:
                time.sleep(.005)
            else:
                os.write(self.master, part)

    def settle(self, marker):
        """Press F12 and wait until the loop handled it: earlier keys are done."""
        start = len(self.records())
        os.write(self.master, b"\x1b[24~")
        found = self.wait_for(lambda r: r.get("key") == "KeyModifiers(0x0) F(12)",
                              f"settle {marker}", start)
        assert found is not None, ("loop stalled", marker, self.records()[-3:])
        return found

    def stop(self):
        if self.child.poll() is None:
            os.killpg(self.child.pid, signal.SIGTERM)
            try:
                self.child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.child.pid, signal.SIGKILL)
                self.child.wait(timeout=5)
        os.close(self.master)


def fixture_env(root):
    dropped = {"DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SSH_CONNECTION", "SSH_TTY"}
    env = {k: v for k, v in os.environ.items() if not k.startswith("AETHER_") and k not in dropped}
    env.update(AETHER_CONFIG_DIR=str(root / "config"), AETHER_DATA_DIR=str(root / "data"),
               AETHER_SOCKET=str(root / "unused.sock"), AETHER_LOOP_RECEIPT=str(root / "receipt.jsonl"),
               AETHER_TEST_FAKE_CLIPBOARD="1", FAKE_CLIPBOARD=str(root / "clipboard"),
               PATH=f"{root / 'bin'}{os.pathsep}{env.get('PATH', '')}",
               WAYLAND_DISPLAY="aether-alt-test", TERM="xterm-256color")
    return env


def escape(tui):
    """A lone Esc, given time to stand alone rather than prefix the next key."""
    tui.send(b"\x1b")
    time.sleep(.2)


def reset(tui):
    """Close any view, empty the draft and the flash, from a known state."""
    before = tui.settle("reset")
    if before["view"] is not None:
        escape(tui)
    state = tui.settle("reset")
    for _ in range(4):
        if state["view"] is None and state["draft"] == "":
            return state
        tui.send(CLEAR + b"\x7f")
        state = tui.settle("reset")
    raise AssertionError(("could not reset", state))


def run_case(tui, binding, encoding):
    """Whether Alt+`binding` sent as `encoding` fired: (fired, detail)."""
    draft, key, fired = BINDINGS[binding]
    keys = encode(encoding, key)
    if keys is None:
        return None, "no such encoding"
    reset(tui)
    if draft:
        tui.send(draft)
    before = tui.settle(binding)
    start = len(tui.records())
    tui.send(keys)
    after = tui.settle(binding)
    if binding == "alt+v":
        found = tui.wait_for(lambda r: r["images"] > before["images"], binding, start, 3)
        after = found or after
    keys = [r["key"] for r in tui.records()[start:]
            if "key" in r and r["key"] != "KeyModifiers(0x0) F(12)"]
    return fired(after, before, keys), keys


def run_matrix(tui):
    matrix = {}
    for binding in BINDINGS:
        for encoding in ENCODINGS:
            matrix[(binding, encoding)] = run_case(tui, binding, encoding)
    return matrix


# Keys that may open a terminal reply, and late replies themselves: what is
# typed after them must still reach the draft, and no reply byte may.
FOLLOW_UPS = [
    ("legacy Alt+]", b"\x1b]"), ("kitty Alt+]", b"\x1b[93;3u"),
    ("legacy Alt+Shift+P", b"\x1bP"), ("kitty Alt+Shift+P", b"\x1b[112;4u"),
    ("legacy Alt+_", b"\x1b_"), ("kitty Alt+_", b"\x1b[95;3u"),
    ("OSC 11 reply", b"\x1b]11;rgb:1c1c/1c1c/1c1c\x1b\\"),
    ("OSC 10 reply, BEL", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    ("DECRQSS reply", b"\x1bP1$r0;48;2;1;2;3m\x1b\\"),
    ("XTGETTCAP reply", b"\x1bP1+r524742=31\x1b\\"),
    ("kitty graphics reply", b"\x1b_Gi=44671;OK\x1b\\"),
]


def follow_up_case(tui, keys):
    """The draft after `keys` and then a typed 'hi'."""
    escape(tui)  # Ends a reply an unfixed filter may still think is open.
    reset(tui)
    tui.send(keys)
    tui.send(b"hi")
    return tui.settle("follow-up")["draft"]


# (f) Alt with Shift: both encodings must decode to the same chord, which a
# keys config can bind (`alt+shift+n`). Legacy sends the capital letter.
SHIFTED = [
    ("legacy Alt+Shift+N", b"\x1bN", "KeyModifiers(SHIFT | ALT) Char('N')"),
    ("kitty Alt+Shift+N", b"\x1b[110;4u", "KeyModifiers(SHIFT | ALT) Char('n')"),
    ("kitty Alt+Shift+Enter", b"\x1b[13;4u", "KeyModifiers(SHIFT | ALT) Enter"),
    ("split Alt+Shift+N", [b"\x1b", None, b"N"], "KeyModifiers(SHIFT | ALT) Char('N')"),
]


def shifted_case(tui, keys):
    """The keys the loop decoded from `keys`."""
    escape(tui)
    reset(tui)
    start = len(tui.records())
    tui.send(keys)
    tui.settle("shifted")
    return [r["key"] for r in tui.records()[start:]
            if "key" in r and r["key"] != "KeyModifiers(0x0) F(12)"]


def check(executable, report):
    cache = Path.home() / ".cache"
    cache.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="aether-alt-", dir=cache) as directory:
        root = Path(directory)
        for name in ("config", "data", "bin", "clipboard"):
            (root / name).mkdir()
        (root / "clipboard" / "image.png").write_bytes(PNG)
        helper = root / "bin" / "wl-paste"
        helper.write_text(FAKE_WL_PASTE)
        helper.chmod(0o755)
        tui = Tui(executable, root)
        try:
            assert tui.wait_for(lambda r: r["kind"] == "backfill", "history loaded"), "no backfill"
            matrix = run_matrix(tui)
            follow_ups = {name: follow_up_case(tui, keys) for name, keys in FOLLOW_UPS}
            shifted = {name: (shifted_case(tui, keys), want) for name, keys, want in SHIFTED}
        finally:
            tui.stop()
    return report_or_assert(matrix, follow_ups, shifted, report)


# Not decodable as Alt: 8-bit meta is a typed character (never remapped); crossterm
# 0.29 drops modifyOtherKeys `CSI 27;…~` and reads rxvt's `ESC ESC [A` as Esc, `[`, `A`.
UNSUPPORTED = {"eight_bit", "modify_other_keys", "rxvt"}


def report_or_assert(matrix, follow_ups, shifted, report):
    failures = []
    lines = ["| binding | " + " | ".join(ENCODINGS) + " |", "|" + "---|" * (len(ENCODINGS) + 1)]
    for binding in BINDINGS:
        cells = []
        for encoding in ENCODINGS:
            fired, detail = matrix[(binding, encoding)]
            cells.append("n/a" if fired is None else ("fires" if fired else "NO"))
            if fired is False and encoding not in UNSUPPORTED:
                failures.append((binding, encoding, detail))
        lines.append(f"| {binding} | " + " | ".join(cells) + " |")
    for name, draft in follow_ups.items():
        lines.append(f"{name}, then 'hi': draft {draft!r}")
        if draft != "hi":
            failures.append((name, "follow-up", draft))
    for name, (keys, want) in shifted.items():
        lines.append(f"{name}: decoded {keys}")
        if keys != [want]:
            failures.append((name, "decoded", keys))
    print("\n".join(lines), flush=True)
    if report:
        return True
    assert not failures, failures
    print(f"{len(BINDINGS)} Alt bindings x {len(ENCODINGS)} encodings, {len(FOLLOW_UPS)} "
          f"follow-ups, {len(SHIFTED)} shifted chords: PASS", flush=True)
    return True


if __name__ == "__main__":
    check(sys.argv[1], "--report" in sys.argv[2:])
