"""The upgrade lane's sandbox: directories, environment and daemons.

Everything lives under one temporary root: config, data, socket, a home, the
installed binary, and a PATH entry holding the fake `claude` and `ssh`. Nothing
reaches the caller's own Aether or home directory.
"""
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import time

SCRIPTS = Path(__file__).resolve().parent
COMMAND_SECONDS = 120
READY_SECONDS = 60
STOP_SECONDS = 30
# Settings of the caller's own Aether that must not reach the one under test.
AETHER_VARIABLES = ("AETHER_SOCKET", "AETHER_CONFIG_DIR", "AETHER_DATA_DIR", "AETHER_TOKEN",
                    "AETHER_REMOTE", "AETHER_ADDRESS", "AETHER_LOG")

FAKE_SSH = """#!/bin/sh
# Stands in for ssh: runs the remote command here, as the host's shell would.
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
[ "$#" -eq 3 ] || { echo "fake ssh: expected -- TARGET COMMAND, got $*" >&2; exit 255; }
exec sh -c "$3"
"""


class LaneFailure(Exception):
    """A check of the lane failed; the message says which and why."""


def require(condition, message):
    if not condition:
        raise LaneFailure(message)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def write_executable(path, text):
    path.write_text(text)
    path.chmod(0o755)


class Fixture:
    """One sandbox. `data` can be pointed at another store between daemons."""

    def __init__(self, root):
        assert root.parent.is_dir(), f"{root.parent} must exist"
        root.mkdir()
        self.root = root
        self.config = root / "config"
        self.data = root / "data"
        self.home = root / "home"
        self.workspace = root / "workspace"
        self.install_dir = root / "install"
        self.fake_bin = root / "fake-bin"
        self.engine_state = root / "engine"
        self.socket = root / "aetherd.sock"
        self.address = f"127.0.0.1:{free_port()}"
        self.token = secrets.token_hex(32)
        self.daemons = []
        for directory in (self.config, self.data, self.home, self.workspace, self.install_dir,
                          self.fake_bin, self.engine_state):
            directory.mkdir()
        (self.workspace / "README.md").write_text("# upgrade lane workspace\n")
        (self.config / "config.toml").write_text(
            f'[listen]\naddress = "{self.address}"\ntoken_env = "AETHER_TOKEN"\n')
        self._write_fakes()

    def _write_fakes(self):
        engine = shlex.quote(str(SCRIPTS / "upgrade_compat_engine.py"))
        state = shlex.quote(str(self.engine_state))
        python = shlex.quote(sys.executable)
        installed = shlex.quote(str(self.installed))
        write_executable(self.fake_bin / "claude", f'#!/bin/sh\nexec {python} {engine} '
                                                   f'--state {state} --installed {installed} "$@"\n')
        write_executable(self.fake_bin / "ssh", FAKE_SSH)

    @property
    def installed(self):
        return self.install_dir / "aether"

    def install(self, binary):
        """Put `binary` at the installed path the way `aether update` does:
        staged beside it, then renamed over it, so a running image keeps its inode."""
        staged = self.install_dir / ".aether.new"
        shutil.copyfile(binary, staged)
        staged.chmod(0o755)
        os.replace(staged, self.installed)

    def reset_engine_state(self):
        shutil.rmtree(self.engine_state)
        self.engine_state.mkdir()

    def kill_daemons(self):
        """End every daemon still running, so a failed run leaves none behind."""
        for daemon in self.daemons:
            daemon.kill()

    def env(self):
        env = {name: value for name, value in os.environ.items() if name not in AETHER_VARIABLES}
        env.update({
            "HOME": str(self.home),
            "PATH": f"{self.fake_bin}{os.pathsep}{env.get('PATH', '/usr/bin:/bin')}",
            "AETHER_CONFIG_DIR": str(self.config),
            "AETHER_DATA_DIR": str(self.data),
            "AETHER_SOCKET": str(self.socket),
            "AETHER_TOKEN": self.token,
            "NO_COLOR": "1",
        })
        return env

    def run(self, args, check=True, timeout=COMMAND_SECONDS):
        """Run a client command in the workspace; its completed process."""
        try:
            done = subprocess.run([str(arg) for arg in args], cwd=self.workspace, env=self.env(),
                                  capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise LaneFailure(f"{shlex.join(map(str, args))} did not finish in {timeout} s") from error
        if check:
            require(done.returncode == 0,
                    f"{shlex.join(map(str, args))} exited {done.returncode}:\n"
                    f"{done.stdout[-2000:]}{done.stderr[-2000:]}")
        return done

    def negotiate(self, negotiate_binary, transport):
        target = self.socket if transport == "unix" else self.address
        done = self.run([negotiate_binary, transport, target])
        return json.loads(done.stdout.strip().splitlines()[-1])


class Daemon:
    """A foreground daemon started from `binary` over the fixture's current store."""

    def __init__(self, fixture, binary, log_name):
        self.fixture = fixture
        self.log = fixture.root / f"{log_name}.log"
        with open(self.log, "ab") as log:
            self.process = subprocess.Popen(
                [str(binary), "daemon", "--foreground"], cwd=fixture.workspace,
                env=fixture.env(), stdout=log, stderr=subprocess.STDOUT,
                start_new_session=True)
        fixture.daemons.append(self)

    def log_tail(self, lines=30):
        return "\n".join(self.log.read_text(errors="replace").splitlines()[-lines:])

    def wait_ready(self, negotiate_binary):
        """Wait until a client gets a welcome; the welcome. A daemon that exits
        first is returned as None, for the caller to judge its refusal."""
        deadline = time.monotonic() + READY_SECONDS
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                return None
            probe = subprocess.run(
                [str(negotiate_binary), "unix", str(self.fixture.socket)],
                env=self.fixture.env(), capture_output=True, text=True, timeout=COMMAND_SECONDS)
            if probe.returncode == 0:
                return json.loads(probe.stdout.strip().splitlines()[-1])
            time.sleep(0.1)
        self.kill()
        raise LaneFailure(f"the daemon was not ready within {READY_SECONDS} s:\n{self.log_tail()}")

    def stop(self, cli):
        """Stop it through the CLI and wait for the process to be gone."""
        self.fixture.run([cli, "stop"])
        try:
            code = self.process.wait(timeout=STOP_SECONDS)
        except subprocess.TimeoutExpired as error:
            self.kill()
            raise LaneFailure(f"the daemon outlived `aether stop` by {STOP_SECONDS} s:\n"
                              f"{self.log_tail()}") from error
        require(code == 0, f"the daemon exited {code} after `aether stop`:\n{self.log_tail()}")

    def kill(self):
        """SIGKILL its whole session, as a crash or a power cut would end it."""
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGKILL)
        self.process.wait(timeout=STOP_SECONDS)
