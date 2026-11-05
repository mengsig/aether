"""The only module that knows how the upstream app is started and driven.

Nothing of upstream is copied here: this file holds guesses about a black box,
and each one is listed below so a maintainer can correct it for their checkout.

ASSUMPTIONS (override by editing ENDPOINTS or setting the environment variables):
  A1. A checkout is a directory holding one of package.json, pyproject.toml or
      requirements.txt. The start command is LEARNHARNESS_START (shell-split)
      or, failing that, `npm start`.
  A2. The app listens on the port in the PORT environment variable and keeps
      its data under HOME, so a temp HOME isolates it.
  A3. A provider can be set over HTTP with a base URL and no key (ENDPOINTS
      ["configure"]), taking {"base_url", "model", "api_key"}.
  A4. A conversation is created with ENDPOINTS["create"] (returns {"id"}),
      messages are posted to ENDPOINTS["message"], and ENDPOINTS["state"]
      returns the structure read by `trace_from_state`.
  A5. The state JSON holds: phase_history (list of phase names), questions
      (each with "valid"), plan_attempts (each with "valid" and "problems",
      problem names from compare vocabulary), and graph with statements and
      premise_sets (conclusion plus premises, as statements).
  A6. The app's processes stay in the launcher's process group. stop() signals
      that group, so a child that starts its own session would escape it.
If a reply lacks a field named above, AdapterError says which assumption broke.
"""
import json
import os
import shlex
import signal
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ENDPOINTS = {
    "configure": "/api/settings/provider",
    "create": "/api/conversations",
    "message": "/api/conversations/{id}/messages",
    "state": "/api/conversations/{id}/state",
}
CHECKOUT_MARKERS = ("package.json", "pyproject.toml", "requirements.txt")
READY_TIMEOUT_SECONDS = 60
REQUEST_TIMEOUT_SECONDS = 120
STOP_GRACE_SECONDS = 15
KILL_WAIT_SECONDS = 5
GROUP_POLL_SECONDS = 0.05


class AdapterError(Exception):
    pass


def looks_like_checkout(path):
    return bool(path) and os.path.isdir(path) and any(os.path.isfile(os.path.join(path, m)) for m in CHECKOUT_MARKERS)


def start_command(env=os.environ):
    raw = env.get("LEARNHARNESS_START", "")
    return shlex.split(raw) if raw else ["npm", "start"]


def free_port():
    import socket

    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http_json(method, url, body=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT_SECONDS) as reply:
            raw = reply.read()
    except urllib.error.URLError as error:
        raise AdapterError(f"{method} {url} failed: {error}") from error
    return json.loads(raw) if raw else {}


class UpstreamApp:
    """Runs the checkout unmodified as a subprocess with a temp HOME and loopback port."""

    def __init__(self, checkout, model_url):
        assert looks_like_checkout(checkout), f"{checkout} is not a checkout"
        assert model_url.startswith("http://127.0.0.1:"), "the model endpoint must be loopback"
        self.checkout, self.model_url = checkout, model_url
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self._home = tempfile.TemporaryDirectory(prefix="noesis-diff-")
        self._proc = None
        self._pgid = None

    def start(self):
        """Launch in a new session, so the launcher and every child share one recorded process group."""
        assert self._proc is None, "an app is started once"
        env = dict(os.environ, HOME=self._home.name, PORT=str(self.port))
        self._proc = subprocess.Popen(start_command(), cwd=self.checkout, env=env, start_new_session=True,
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self._pgid = os.getpgid(self._proc.pid)
        assert self._pgid == self._proc.pid, "the launcher leads its own process group"
        self._wait_ready()

    def _wait_ready(self):
        deadline = time.monotonic() + READY_TIMEOUT_SECONDS
        while time.monotonic() < deadline:
            if self._proc.poll() is not None:
                raise AdapterError(f"the app exited with {self._proc.returncode} before it listened")
            try:
                urllib.request.urlopen(self.base, timeout=2).close()
                return
            except urllib.error.HTTPError:
                return
            except (urllib.error.URLError, OSError):
                time.sleep(0.5)
        raise AdapterError(f"the app did not listen on {self.base} within {READY_TIMEOUT_SECONDS}s (assumption A1/A2)")

    def stop(self):
        """Stop the recorded process group even if the launcher already exited; always drop the temp HOME."""
        try:
            if self._pgid is not None:
                stop_group(self._pgid, self._proc)
        finally:
            self._home.cleanup()

    def configure(self):
        body = {"base_url": self.model_url, "model": "faux", "api_key": ""}
        http_json("POST", self.base + ENDPOINTS["configure"], body)

    def converse(self, topic, messages):
        created = http_json("POST", self.base + ENDPOINTS["create"], {"topic": topic})
        if "id" not in created:
            raise AdapterError("conversation reply lacks 'id' (assumption A4)")
        for text in messages:
            http_json("POST", self.base + ENDPOINTS["message"].format(id=created["id"]), {"text": text})
        return http_json("GET", self.base + ENDPOINTS["state"].format(id=created["id"]))


def _signal_group(pgid, sig):
    """Send `sig` to the group; False once no member is left. Signal 0 only probes."""
    try:
        os.killpg(pgid, sig)
    except ProcessLookupError:
        return False
    return True


def _group_gone(pgid, leader, seconds):
    """Reap the leader and poll until the group has no member, or `seconds` pass."""
    deadline = time.monotonic() + seconds
    while True:
        leader.poll()
        if not _signal_group(pgid, 0):
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(GROUP_POLL_SECONDS)


def stop_group(pgid, leader):
    """SIGTERM the launcher's group, wait a bounded time, then SIGKILL it. Never matches by name."""
    assert pgid > 1, f"refusing to signal process group {pgid}"
    assert leader is not None and leader.pid == pgid, "only the launcher's own group is signalled"
    if _signal_group(pgid, signal.SIGTERM) and not _group_gone(pgid, leader, STOP_GRACE_SECONDS):
        _signal_group(pgid, signal.SIGKILL)
        if not _group_gone(pgid, leader, KILL_WAIT_SECONDS):
            raise AdapterError(f"process group {pgid} still has members {KILL_WAIT_SECONDS}s after SIGKILL")
    leader.poll()


def trace_from_state(state):
    """Reduce the upstream state JSON (assumption A5) to the structural trace."""
    try:
        graph = state["graph"]
        return {
            "phases": _dedupe_runs(state["phase_history"]),
            "questions": [{"valid": q["valid"]} for q in state["questions"]],
            "plan_validations": [{"valid": p["valid"], "problems": p.get("problems", [])} for p in state["plan_attempts"]],
            "graph": {"statements": graph["statements"], "premise_sets": graph["premise_sets"]},
        }
    except (KeyError, TypeError) as error:
        raise AdapterError(f"upstream state does not match assumption A5: {error!r}") from error


def _dedupe_runs(items):
    assert isinstance(items, list), "phase history is a list"
    return [item for i, item in enumerate(items) if i == 0 or items[i - 1] != item]
