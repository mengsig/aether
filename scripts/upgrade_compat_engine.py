#!/usr/bin/env python3
"""A stand-in `claude` for the upgrade lane (scripts/upgrade_compat.py).

  upgrade_compat_engine.py --state DIR --installed PATH [claude arguments...]

It answers `--version`, and runs a `-p` turn the way Claude Code does as far as
Aether can see: stream-json in and out, and the MCP bridge Aether names in
`--mcp-config` started as the engine's MCP server. A turn with a bridge:

1. starts the bridge, lists its tools and calls `inspect_state`;
2. writes `bridge-ready` in DIR and waits for `swapped` (the lane replaces the
   installed binary in between), then calls the tool again on that bridge;
3. starts a second bridge from the same command, as the next turn would, and
   calls the tool there too;
4. starts one more bridge, from the installed binary PATH with the same
   arguments, so the build now installed meets the running daemon.

Every step is written to `session.json` in DIR. The turn ends in success only
when every call succeeded; otherwise its result names the first failure.
"""
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time

VERSION = "2.1.265 (Claude Code)"
TOOL = "inspect_state"
STEP_SECONDS = 30
SWAP_WAIT_SECONDS = 90


class BridgeError(Exception):
    pass


def emit(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def option(args, name):
    """The value after `name`, or None when the flag is absent."""
    if name not in args:
        return None
    index = args.index(name)
    if index + 1 >= len(args):
        raise SystemExit(f"fake claude: {name} has no value")
    return args[index + 1]


class Bridge:
    """One MCP server process, spoken to over newline-delimited JSON-RPC."""

    def __init__(self, command, args):
        assert command, "the MCP config names no bridge command"
        self.process = subprocess.Popen(
            [command, *args], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True)
        self.lines = queue.Queue()
        self.next_id = 1
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def request(self, method, params):
        request_id = self.next_id
        self.next_id += 1
        self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + STEP_SECONDS
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise BridgeError(f"{method}: no answer within {STEP_SECONDS} s")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty:
                continue
            if line is None:
                raise BridgeError(f"{method}: the bridge closed its output ({self._stderr()})")
            frame = json.loads(line)
            if frame.get("id") == request_id:
                return frame

    def notify(self, method):
        self._send({"jsonrpc": "2.0", "method": method})

    def _send(self, frame):
        try:
            self.process.stdin.write(json.dumps(frame) + "\n")
            self.process.stdin.flush()
        except BrokenPipeError as error:
            raise BridgeError(f"the bridge exited ({self._stderr()})") from error

    def _stderr(self):
        if self.process.poll() is None:
            return "still running"
        return f"exit {self.process.returncode}: {self.process.stderr.read()[-400:].strip()}"

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=STEP_SECONDS)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()


def start(command, args):
    """A bridge past `initialize`, with the tools it offers."""
    bridge = Bridge(command, args)
    init = bridge.request("initialize", {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "upgrade-compat-engine", "version": "1"}})
    if "result" not in init:
        raise BridgeError(f"initialize failed: {init}")
    bridge.notify("notifications/initialized")
    listed = bridge.request("tools/list", {})
    names = [tool["name"] for tool in listed.get("result", {}).get("tools", [])]
    return bridge, names


def call(bridge, names):
    """Call the harmless state tool; its text, or why it failed."""
    if TOOL not in names:
        raise BridgeError(f"the bridge offers no {TOOL}: {names}")
    answer = bridge.request("tools/call", {"name": TOOL, "arguments": {}})
    result = answer.get("result")
    if not result or result.get("isError"):
        raise BridgeError(f"{TOOL} failed: {answer}")
    text = "".join(part.get("text", "") for part in result.get("content", []))
    if not text.strip():
        raise BridgeError(f"{TOOL} answered with no text: {answer}")
    return text


def build_of(command):
    """Which build a bridge command names: its bytes' digest and its version."""
    digest = hashlib.sha256(Path(command).read_bytes()).hexdigest()
    out = subprocess.run([command, "--version"], capture_output=True, text=True,
                         timeout=STEP_SECONDS)
    version = out.stdout.strip() or f"exit {out.returncode}: {out.stderr.strip()}"
    return {"sha256": digest, "version": version}


def wait_for(path):
    deadline = time.monotonic() + SWAP_WAIT_SECONDS
    while not path.exists():
        if time.monotonic() > deadline:
            raise BridgeError(f"{path.name} did not appear within {SWAP_WAIT_SECONDS} s")
        time.sleep(0.05)


def publish(state, name, record):
    staged = state / f".{name}.tmp"
    staged.write_text(json.dumps(record, indent=2))
    os.replace(staged, state / name)


def bridge_session(state, installed, command, args, record):
    """The four steps in the module docstring, recorded as they happen."""
    record["first_build"] = build_of(command)
    first, names = start(command, args)
    try:
        record["first_tools"] = names
        record["before_swap"] = call(first, names)[:200]
        publish(state, "bridge-ready", record)
        wait_for(state / "swapped")
        record["after_swap"] = call(first, names)[:200]
    finally:
        first.close()
    fresh_bridge(command, args, record, "second")
    fresh_bridge(installed, args, record, "installed")


def fresh_bridge(command, args, record, name):
    """Start a bridge from `command`, call the tool once, and close it."""
    record[f"{name}_build"] = build_of(command)
    bridge, names = start(command, args)
    try:
        record[f"{name}_tools"] = names
        record[f"{name}_bridge"] = call(bridge, names)[:200]
    finally:
        bridge.close()


def run_turn(state, installed, args):
    session = option(args, "--session-id") or option(args, "--resume") or "fake-session"
    prompt = json.loads(sys.stdin.readline())
    assert prompt.get("type") == "user", f"the first stdin line is not a user message: {prompt}"
    emit({"type": "system", "subtype": "init", "session_id": session, "model": "fake",
          "tools": [f"mcp__aether__{TOOL}"]})
    emit({"type": "user", "isReplay": True, "message": prompt["message"]})
    record = {"args": args}
    error = None
    config = option(args, "--mcp-config")
    if config is not None:
        server = json.loads(config)["mcpServers"]["aether"]
        record["command"] = server["command"]
        try:
            bridge_session(state, installed, server["command"], server.get("args", []), record)
        except BridgeError as failure:
            error = str(failure)
    record["error"] = error
    publish(state, "session.json", record)
    text = error or "every bridge call succeeded"
    emit({"type": "assistant", "message": {"id": "msg_fake", "model": "fake",
                                           "role": "assistant",
                                           "content": [{"type": "text", "text": text}]}})
    emit({"type": "result", "subtype": "success" if error is None else "error_during_execution",
          "is_error": error is not None, "session_id": session, "result": text,
          "usage": {"input_tokens": 1, "output_tokens": 1}})


def main(argv):
    if len(argv) < 4 or argv[0] != "--state" or argv[2] != "--installed":
        raise SystemExit("usage: upgrade_compat_engine.py --state DIR --installed PATH "
                         "[claude arguments...]")
    state, installed = Path(argv[1]), argv[3]
    assert state.is_dir(), f"state directory {state} does not exist"
    args = argv[4:]
    with open(state / "invocations.log", "a") as log:
        log.write(json.dumps(args) + "\n")
    if args[:1] == ["--version"]:
        print(VERSION)
        return 0
    if "-p" not in args:
        print(f"fake claude: unsupported invocation {args}", file=sys.stderr)
        return 2
    run_turn(state, installed, args)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
