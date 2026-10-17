"""The eval harness's view of a daemon: the client library plus the helpers
the evals need.

The protocol itself lives in `clients/python`, generated from the Rust types;
this module is a consumer of it. What stays here is eval-specific: waiting for
an agent's reply, and standing a throwaway daemon up and down.
"""

import os
import socket
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "clients", "python"))

from aether import AetherError, default_socket_path, protocol  # noqa: E402
from aether import AetherClient as _AetherClient  # noqa: E402

PROTOCOL_VERSION = protocol.PROTOCOL_VERSION

__all__ = [
    "AetherClient", "AetherError", "PROTOCOL_VERSION", "isolated_env", "init_git_workspace",
    "start_daemon", "stop_daemon",
]


class AetherClient(_AetherClient):
    """The library client, plus what driving an agent turn by turn needs."""

    def __init__(self, socket_path=None, **kwargs):
        # Every event this client has consumed, so a check that runs after the
        # fact can still find one.
        self.events_seen = []
        stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        stream.connect(socket_path or default_socket_path())
        super().__init__(stream, **kwargs)

    def next_frame(self, timeout=60.0):
        frame = super().next_frame(timeout=timeout)
        if frame.get("frame") == "event":
            self.events_seen.append(frame["event"])
        return frame

    def find_event(self, agent_id, event_type):
        """An already-consumed event of this type for the agent, newest first."""
        for event in reversed(self.events_seen):
            if event.get("agent_id") == agent_id and event["kind"]["type"] == event_type:
                return event
        return None

    # ----- helpers -----------------------------------------------------------

    def create_agent(self, role="assistant", objective=None, model=None, long_lived=True, effort=None, parent=None,
                     tools=None, budget=None):
        """`budget` sets any of the config's `max_turns`, `token_budget` and `cost_budget`."""
        config = {"long_lived": long_lived, "approval": {"require_for": []}}
        if effort:
            config["effort"] = effort
        if tools is not None:
            config["tools"] = tools
        if budget:
            unknown = set(budget) - {"max_turns", "token_budget", "cost_budget"}
            assert not unknown, f"unknown budget keys: {sorted(unknown)}"
            config.update(budget)
        model_ref = None
        if model:
            provider, name = model.split("/", 1)
            model_ref = {"provider": provider, "model": name}
        return self.request(
            protocol.create_agent(
                role=role,
                start=True,
                objective=objective or None,
                model=model_ref,
                parent=parent or None,
                config=config,
            )
        )["data"]

    def pause(self, agent_id, subtree=False):
        return self.request(protocol.pause(agent=agent_id, subtree=subtree))["data"]

    def resume(self, agent_id, subtree=False):
        return self.request(protocol.resume(agent=agent_id, subtree=subtree))["data"]

    def send(self, agent_id, text):
        return self.request(protocol.send_message(to=agent_id, content=text))["data"]

    def send_subtree(self, agent_id, text):
        return self.request(protocol.send_message(to=agent_id, content=text, subtree=True))["data"]

    def subscribe(self, agent_id=None, after=None, deltas=False):
        query = {"agent": agent_id} if agent_id else {}
        return super().subscribe(after=after, query=query, deltas=deltas)

    def compact(self, agent_id):
        return self.request(protocol.compact(agent=agent_id))

    def get_agent(self, agent_id):
        return self.request(protocol.get_agent(id=agent_id))["data"]

    def wait_for_reply(self, agent_id, timeout=600.0):
        """Wait until the agent answers (its next assistant message) and is
        waiting again. Returns the assistant text."""
        deadline = time.time() + timeout
        text = None
        while time.time() < deadline:
            frame = self.next_frame(timeout=max(1.0, deadline - time.time()))
            if frame.get("frame") != "event":
                continue
            event = frame["event"]
            if event.get("agent_id") != agent_id:
                continue
            kind = event["kind"]
            if kind["type"] == "agent.message" and kind["role"] == "assistant":
                text = "".join(b.get("text", "") for b in kind["content"] if b.get("type") == "text")
            if kind["type"] == "agent.waiting" and text is not None:
                return text
            if kind["type"] in ("agent.failed", "agent.cancelled"):
                raise RuntimeError(f"agent ended: {kind}")
        raise TimeoutError("no reply from agent")

    def wait_for_event(self, agent_id, event_type, timeout=600.0):
        found = self.find_event(agent_id, event_type)
        if found is not None:
            return found
        deadline = time.time() + timeout
        while time.time() < deadline:
            frame = self.next_frame(timeout=max(1.0, deadline - time.time()))
            if frame.get("frame") != "event":
                continue
            event = frame["event"]
            if event.get("agent_id") == agent_id and event["kind"]["type"] == event_type:
                return event
        raise TimeoutError(f"no {event_type} event")


def isolated_env(root):
    """Environment variables for a throwaway Aether instance under `root`."""
    env = dict(os.environ)
    env.update(
        AETHER_CONFIG_DIR=os.path.join(root, "config"),
        AETHER_DATA_DIR=os.path.join(root, "data"),
        AETHER_SOCKET=os.path.join(root, "aetherd.sock"),
    )
    os.makedirs(env["AETHER_CONFIG_DIR"], exist_ok=True)
    os.makedirs(env["AETHER_DATA_DIR"], exist_ok=True)
    return env


def init_git_workspace(workspace):
    """Make a fixture workspace a real Git repo with one deterministic commit.

    Several eval prompts ask an agent to inspect the workspace with real
    `git status`/`git log`/`git branch` calls; a bare directory fails every
    one of those with "not a git repository" (P2-7) - a fixture bug shared by
    every harness that builds one of these throwaway workspaces.
    """
    identity = [
        "-c", "user.email=eval@aether.test",
        "-c", "user.name=Aether Eval",
        "-c", "commit.gpgsign=false",
    ]
    # Pinned so the commit hash is stable across runs, not wall-clock.
    pinned_date = dict(os.environ, GIT_AUTHOR_DATE="2026-01-01T00:00:00Z",
                        GIT_COMMITTER_DATE="2026-01-01T00:00:00Z")

    def git(*args, env=None):
        try:
            subprocess.run(["git", *args], cwd=workspace, check=True, capture_output=True, text=True, env=env)
        except subprocess.CalledProcessError as error:
            raise RuntimeError(f"git {' '.join(args)} failed: {error.stderr}") from error

    git("init", "-q", "-b", "main")
    git(*identity, "add", "-A")
    # --allow-empty: some fixtures (e.g. native_web.py) never write a file here.
    git(*identity, "commit", "-q", "--allow-empty", "-m", "Initial fixture commit", env=pinned_date)


def daemon_log_tail(env, lines=40):
    """The end of the isolated daemon's own log. The detached process gets a
    null stdio, so this is the only place a startup failure is written."""
    log_dir = os.path.join(env["AETHER_DATA_DIR"], "logs")
    try:
        names = sorted(n for n in os.listdir(log_dir) if n.startswith("aetherd.log"))
    except OSError as e:
        return f"no daemon log under {log_dir} ({e})"
    if not names:
        return f"no daemon log under {log_dir}"
    path = os.path.join(log_dir, names[-1])
    with open(path, errors="replace") as f:
        tail = f.read().splitlines()[-lines:]
    return f"{path}:\n" + "\n".join(tail)


def start_daemon(binary, env, workspace):
    launch = subprocess.run([binary, "daemon"], env=env, cwd=workspace, capture_output=True, text=True)
    if launch.returncode != 0:
        raise RuntimeError(
            f"`aether daemon` exited {launch.returncode}\n"
            f"stdout: {launch.stdout.strip()}\nstderr: {launch.stderr.strip()}\n"
            f"{daemon_log_tail(env)}"
        )
    # Startup probes the engine CLIs (`claude auth status`, `codex login
    # status`), which can take tens of seconds when they are slow; 10s was
    # short enough to fail mid-eval on a machine under load.
    for _ in range(600):
        if os.path.exists(env["AETHER_SOCKET"]):
            return
        time.sleep(0.1)
    # Never a bare "did not start": the reason is in the daemon's own log.
    raise RuntimeError(
        f"no socket at {env['AETHER_SOCKET']} within 60s\n"
        f"launcher stdout: {launch.stdout.strip()}\nlauncher stderr: {launch.stderr.strip()}\n"
        f"{daemon_log_tail(env)}"
    )


def stop_daemon(binary, env, timeout=30.0):
    """Ask the daemon to stop and wait until its socket is gone. `aether stop`
    returns as soon as shutdown is sent, so without the wait the next run
    starts while the previous daemon is still tearing down."""
    if not os.path.exists(env["AETHER_SOCKET"]):
        return
    proc = subprocess.run([binary, "stop"], env=env, capture_output=True, text=True)
    if proc.returncode != 0:
        # Not fatal - the caller is already tearing down - but silence here is
        # what made a half-stopped daemon look like a fresh one that failed.
        print(f"   `aether stop` exited {proc.returncode}: {proc.stderr.strip()}", flush=True)
        return
    deadline = time.time() + timeout
    while time.time() < deadline:
        if not os.path.exists(env["AETHER_SOCKET"]):
            return
        time.sleep(0.1)
    print(f"   daemon still listening on {env['AETHER_SOCKET']} {timeout:.0f}s after stop", flush=True)
