#!/usr/bin/env python3
"""The upgrade-compatibility lane: this tree's build against the previous release.

  python3 scripts/upgrade_compat.py [--new-binary PATH] [--negotiate PATH]
                                    [--previous vX.Y.Z | --previous-binary PATH]
                                    [--release-tag vX.Y.Z] [--keep]

It downloads the previous release (scripts/upgrade_compat_release.py), then:

1. starts the previous release's daemon from an installed path and runs a native
   session against it (a fake `claude`, scripts/upgrade_compat_engine.py) while
   this build replaces the installed binary, as `aether update` does; the session's
   tools must keep working, and the next bridge, now this build, must connect;
2. connects this build's clients to that daemon over the Unix socket, TCP and
   SSH, and checks a command the daemon is too old for is refused cleanly;
3. upgrades the previous release's store, killing the new daemon mid-upgrade
   (scripts/upgrade_compat_migration.py);
4. starts this build's daemon on the upgraded store and replaces the installed
   binary mid-session again: the bridge must stay the daemon's own build.

Linux only. docs/ci.md describes the lane and its workflow.
"""
import argparse
from collections import namedtuple
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

from upgrade_compat_fixture import Daemon, Fixture, LaneFailure, require
from upgrade_compat_migration import check_interrupted_migration
import upgrade_compat_release as release

ROOT = Path(__file__).resolve().parent.parent
NATIVE_MODEL = "claude-code/default"
SESSION_SECONDS = 120
# Commands newer than some release, keyed by the Welcome feature announcing them.
NEWER_COMMANDS = (("workspace_review", ["workspace", "diff"]),)

Binaries = namedtuple("Binaries", "previous new negotiate previous_version")


def step(message):
    print(f"\n==> {message}", flush=True)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def wait_for_bridge(fixture, run):
    """Wait until the fake engine's first bridge has called a tool."""
    ready = fixture.engine_state / "bridge-ready"
    deadline = time.monotonic() + SESSION_SECONDS
    while not ready.exists():
        if run.poll() is not None or time.monotonic() > deadline:
            if run.poll() is None:
                run.kill()
                what = f"the native session's bridge was not ready within {SESSION_SECONDS} s"
            else:
                what = f"`aether run` exited {run.returncode} before its bridge was ready"
            output = run.communicate()[0]
            invocations = fixture.engine_state / "invocations.log"
            calls = invocations.read_text() if invocations.exists() else "(none)"
            raise LaneFailure(f"{what}:\n{output[-3000:]}\nengine invocations:\n{calls[-2000:]}")
        time.sleep(0.05)


def native_session(fixture, binaries, replacement):
    """A native turn whose installed binary is replaced by `replacement` while
    its bridge runs; what the fake engine recorded."""
    fixture.reset_engine_state()
    run = subprocess.Popen(
        [str(binaries.new), "run", "--yes", "--model", NATIVE_MODEL, "upgrade lane session"],
        cwd=fixture.workspace, env=fixture.env(), stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True)
    wait_for_bridge(fixture, run)
    fixture.install(replacement)
    (fixture.engine_state / "swapped").write_text("")
    try:
        output = run.communicate(timeout=SESSION_SECONDS)[0]
    except subprocess.TimeoutExpired as error:
        run.kill()
        raise LaneFailure(f"the native session outlived {SESSION_SECONDS} s") from error
    record = json.loads((fixture.engine_state / "session.json").read_text())
    require(record["error"] is None, f"a bridge call failed: {record['error']}\n{output[-3000:]}")
    require(run.returncode == 0, f"`aether run` exited {run.returncode}:\n{output[-3000:]}")
    require(record["installed_build"]["sha256"] == digest(replacement),
            "the bridge from the installed path was not the build installed mid-session")
    return record


def check_previous_daemon_session(fixture, binaries):
    """Either bridge strategy a release may have is fine: launching the installed
    path (before 1.0.11), or a copy of its own build. Tools must work in both."""
    record = native_session(fixture, binaries, replacement=binaries.new)
    previous = digest(binaries.previous)
    require(record["first_build"]["sha256"] == previous,
            "the session's first bridge was not the previous release")
    if Path(record["command"]) == fixture.installed:
        require(record["second_build"]["sha256"] == digest(binaries.new),
                "the previous daemon launches the installed path, yet its next bridge "
                "was not the build installed there")
        print("  the previous daemon launches the installed path: now this build")
    else:
        require(record["second_build"]["sha256"] == previous,
                f"the previous daemon's own bridge {record['command']} changed build mid-session")
        print("  the previous daemon launches a copy of its own build")
    print("  the running bridge kept its tools; this build's bridge reached the previous daemon")


def check_transports(fixture, binaries):
    """This build's clients over every transport; the daemon's features."""
    features = None
    for transport in ("unix", "tcp"):
        report = fixture.negotiate(binaries.negotiate, transport)
        newest = min(report["daemon_protocol_version"] or report["protocol_version"],
                     report["client_protocol_version"])
        require(report["protocol_version"] == newest,
                f"over {transport} the client settled on protocol {report['protocol_version']}, "
                f"not the newest common one, {newest}")
        if binaries.previous_version is not None:
            require(report["daemon_version"] == binaries.previous_version,
                    f"over {transport} the daemon is {report['daemon_version']}, "
                    f"not {binaries.previous_version}")
        features = report["features"]
        print(f"  {transport}: protocol {report['protocol_version']} "
              f"(client {report['client_protocol_version']}, daemon {report['daemon_version']})")
    fixture.run([binaries.new, "status"])
    print("  unix: `aether status` from this build")
    fixture.run([binaries.new, "remote", "add", "previous", "--ssh", "upgrade-lane-host",
                 "--executable", binaries.previous, "--remote-socket", fixture.socket])
    fixture.run([binaries.new, "--remote", "previous", "status"])
    print("  ssh: `aether --remote previous status` through the previous release's bridge")
    return features


def check_newer_command_refused(fixture, binaries, features):
    agents = json.loads(fixture.run([binaries.new, "agents", "--all", "--json"]).stdout)
    require(agents, "the previous daemon lists no agent to address")
    for feature, args in NEWER_COMMANDS:
        if feature in features:
            continue
        done = fixture.run([binaries.new, *args, agents[0]["id"]], check=False)
        said = done.stdout + done.stderr
        require(done.returncode != 0 and "Nothing was sent" in said and feature in said,
                f"`aether {' '.join(args)}` against a daemon without `{feature}` exited "
                f"{done.returncode} without the client's refusal:\n{said[-2000:]}")
        print(f"  `aether {' '.join(args)}` refused before sending: no `{feature}`")
        return
    print("  skipped: the previous release announces every feature NEWER_COMMANDS knows")


def previous_daemon_cases(fixture, binaries):
    """Cases 1 and 2 of the module docstring; a copy of the store it wrote."""
    step("the previous release's daemon, a session across `aether update`")
    fixture.install(binaries.previous)
    daemon = Daemon(fixture, fixture.installed, "previous-daemon")
    require(daemon.wait_ready(binaries.negotiate) is not None,
            f"the previous release's daemon did not start:\n{daemon.log_tail()}")
    check_previous_daemon_session(fixture, binaries)
    step("this build's clients against the previous release's daemon")
    features = check_transports(fixture, binaries)
    check_newer_command_refused(fixture, binaries, features)
    daemon.stop(binaries.new)
    pristine = fixture.root / "store-previous"
    shutil.copytree(fixture.data, pristine)
    return pristine


def new_daemon_case(fixture, binaries, store):
    """Case 4: this build's daemon keeps its own bridge across a replaced binary."""
    step("this build's daemon on the upgraded store, a session across a replaced binary")
    fixture.data = store
    fixture.install(binaries.new)
    daemon = Daemon(fixture, fixture.installed, "new-daemon")
    require(daemon.wait_ready(binaries.negotiate) is not None,
            f"this build's daemon did not start on the upgraded store:\n{daemon.log_tail()}")
    record = native_session(fixture, binaries, replacement=binaries.previous)
    command = Path(record["command"])
    require(command != fixture.installed and command.parent == store.resolve() / "bin",
            f"the daemon launched {command} as its bridge, not its own copy under {store}/bin")
    new = digest(binaries.new)
    require(record["first_build"]["sha256"] == new and record["second_build"]["sha256"] == new,
            "a bridge of this build's daemon was not this build")
    daemon.stop(binaries.new)
    print(f"  both bridges ran {command.name}, the daemon's own build")
    print("  the previous release's bridge, now installed, reached this build's daemon")


def resolve_binaries(args, scratch):
    new = Path(args.new_binary).resolve()
    negotiate = Path(args.negotiate).resolve()
    for path in (new, negotiate):
        require(path.is_file() and os.access(path, os.X_OK), f"{path} is not an executable")
    if args.previous_binary:
        previous, version = Path(args.previous_binary).resolve(), None
    else:
        require(args.release_tag is None or release.parse_tag(args.release_tag) is not None,
                f"`{args.release_tag}` is not a vX.Y.Z release tag")
        tag = args.previous or release.previous_tag(args.release_tag)
        require(release.parse_tag(tag) is not None, f"`{tag}` is not a vX.Y.Z release tag")
        print(f"previous release: {tag}")
        previous, version = release.download_binary(tag, scratch), tag.removeprefix("v")
    require(digest(previous) != digest(new), "the previous release and this build are one binary")
    return Binaries(previous, new, negotiate, version)


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--new-binary", default=str(ROOT / "target/debug/aether"))
    parser.add_argument("--negotiate", default=str(ROOT / "target/debug/examples/negotiate"))
    which = parser.add_mutually_exclusive_group()
    which.add_argument("--previous", help="release tag to test against (default: the newest "
                                          "published one at or below the workspace version)")
    which.add_argument("--previous-binary", help="an already downloaded previous `aether`")
    parser.add_argument("--release-tag", help="the release tag this run checks, never chosen "
                                              "as its previous release")
    parser.add_argument("--keep", action="store_true", help="keep the sandbox after a pass")
    return parser.parse_args(argv)


def main(argv):
    args = parse_args(argv)
    release.release_target()
    root = Path(tempfile.mkdtemp(prefix="aether-upgrade-", dir=os.environ.get("TMPDIR")))
    started = time.monotonic()
    fixture = None
    try:
        binaries = resolve_binaries(args, root)
        fixture = Fixture(root / "sandbox")
        pristine = previous_daemon_cases(fixture, binaries)
        step("an interrupted upgrade of the previous release's store")
        upgraded = check_interrupted_migration(fixture, binaries, pristine)
        new_daemon_case(fixture, binaries, upgraded)
    except LaneFailure as failure:
        print(f"\nupgrade lane FAILED: {failure}\nsandbox kept in {root}", file=sys.stderr)
        return 1
    finally:
        if fixture is not None:
            fixture.kill_daemons()
    print(f"\nupgrade lane passed in {time.monotonic() - started:.0f} s")
    if args.keep:
        print(f"sandbox kept in {root}")
    else:
        shutil.rmtree(root)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
