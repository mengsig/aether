#!/usr/bin/env python3
"""Press Ctrl+V, Alt+V and the terminal's paste as real terminals encode them.

Runs the real runner's EventStream in a PTY against a fixture daemon, with a
fake `wl-paste` first on PATH, and checks what each key did to the draft.
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
printf '%s\\n' "$*" >> "$FAKE_CLIPBOARD/calls"
case "$*" in
    --list-types) cat "$FAKE_CLIPBOARD/types" ;;
    '--type image/png') cat "$FAKE_CLIPBOARD/image.png" ;;
    '--no-newline --type '*) cat "$FAKE_CLIPBOARD/text" ;;
    *) exit 64 ;;
esac
"""
IMAGE_ONLY = "image/png\n"
# What browsers and chat apps offer for a copied image: text beside it.
IMAGE_WITH_TEXT = "text/html\ntext/plain;charset=utf-8\nimage/png\n"
TEXT_ONLY = "text/plain;charset=utf-8\ntext/plain\n"

# (what is pressed, bytes the terminal sends, clipboard types, images after it)
IMAGE_CASES = [
    ("Ctrl+V, legacy control byte", b"\x16", IMAGE_ONLY, 1),
    ("Ctrl+V, kitty keyboard protocol", b"\x1b[118;5u", IMAGE_ONLY, 2),
    ("Ctrl+V, kitty with event type, image beside text", b"\x1b[118;5:1u", IMAGE_WITH_TEXT, 3),
    ("Ctrl+V, legacy, image beside text", b"\x16", IMAGE_WITH_TEXT, 4),
    ("Alt+V, kitty keyboard protocol", b"\x1b[118;3u", IMAGE_ONLY, 5),
    ("Alt+V, legacy escape prefix", b"\x1bv", IMAGE_ONLY, 6),
    ("empty bracketed paste", b"\x1b[200~\x1b[201~", IMAGE_ONLY, 7),
]


def check(executable):
    cache = Path.home() / ".cache"
    cache.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="aether-paste-", dir=cache) as directory:
        root = Path(directory)
        for name in ("config", "data", "bin", "clipboard"):
            (root / name).mkdir()
        clipboard = root / "clipboard"
        helper = root / "bin" / "wl-paste"
        helper.write_text(FAKE_WL_PASTE)
        helper.chmod(0o755)
        (clipboard / "image.png").write_bytes(PNG)
        (clipboard / "calls").write_text("")
        receipt = root / "receipt.jsonl"
        dropped = {"DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SSH_CONNECTION", "SSH_TTY"}
        env = {k: v for k, v in os.environ.items()
               if not k.startswith("AETHER_") and k not in dropped}
        env.update(AETHER_CONFIG_DIR=str(root / "config"), AETHER_DATA_DIR=str(root / "data"),
                   AETHER_SOCKET=str(root / "unused.sock"), AETHER_LOOP_RECEIPT=str(receipt),
                   AETHER_TEST_FAKE_CLIPBOARD="1", FAKE_CLIPBOARD=str(clipboard),
                   PATH=f"{root / 'bin'}{os.pathsep}{env.get('PATH', '')}",
                   WAYLAND_DISPLAY="aether-paste-test", TERM="xterm-256color")
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        output = bytearray()
        child = None

        def controlling_terminal():
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        def pump():
            if select.select([master], [], [], .02)[0]:
                queries = output.count(b"\x1b[6n")
                output.extend(os.read(master, 65536))
                for _ in range(output.count(b"\x1b[6n") - queries):
                    os.write(master, b"\x1b[1;1R")

        def records():
            if not receipt.exists():
                return []
            lines = receipt.read_bytes().split(b"\n")[:-1]
            return [json.loads(line) for line in lines]

        def wait_for(predicate, description):
            deadline = time.monotonic() + 10
            while True:
                found = next((r for r in records() if predicate(r)), None)
                if found is not None:
                    return found
                assert child.poll() is None, (description, child.returncode, bytes(output))
                assert time.monotonic() < deadline, (description, records()[-3:])
                pump()

        def clipboard_calls():
            return (clipboard / "calls").read_text().splitlines()

        def offer(types, text=""):
            (clipboard / "types").write_text(types)
            (clipboard / "text").write_text(text)

        try:
            child = subprocess.Popen(
                [str(Path(executable).resolve()), "--exact",
                 "runner::suspend_tests::fixture_loop_child", "--ignored", "--nocapture"],
                cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave,
                start_new_session=True, preexec_fn=controlling_terminal,
            )
            wait_for(lambda r: r["kind"] == "backfill", "history loaded")
            for name, keys, types, images in IMAGE_CASES:
                offer(types, "text beside the image")
                os.write(master, keys)
                record = wait_for(lambda r, n=images: r["kind"] == "pasted" and r["images"] == n,
                                  name)
                assert record["draft"] == "", (name, record)

            offer(TEXT_ONLY, "hello")
            os.write(master, b"\x1b[118;5u")
            wait_for(lambda r: r["kind"] == "pasted" and r["draft"] == "hello",
                     "Ctrl+V with only text on the clipboard")
            calls = len(clipboard_calls())
            offer(IMAGE_ONLY)
            # Ctrl+Shift+V is the terminal's paste: text, and the clipboard is not read.
            os.write(master, b"\x1b[200~ world\x1b[201~")
            record = wait_for(lambda r: r["kind"] == "input" and r["draft"] == "hello world",
                              "Ctrl+Shift+V text")
            assert record["images"] == 7, record
            time.sleep(.3)
            assert len(clipboard_calls()) == calls, clipboard_calls()[calls:]

            os.killpg(child.pid, signal.SIGTERM)
            assert child.wait(timeout=10) == 0, bytes(output)
            print(f"{len(IMAGE_CASES)} image pastes, Ctrl+V text, Ctrl+Shift+V text: PASS",
                  flush=True)
        finally:
            if child is not None and child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
            os.close(slave)
            os.close(master)


if __name__ == "__main__":
    check(sys.argv[1])
