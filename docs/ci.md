# CI

`.github/workflows/ci.yml` runs `scripts/ci.sh`, the same script `make ci` runs on
your machine, so a check that fails in CI fails locally with the same command.

## What runs

| Job | Runner | When | Command |
| --- | --- | --- | --- |
| `lint` | `ubuntu-latest` | every pull request, every push to `main` | `scripts/ci.sh lint` |
| `test` | `ubuntu-latest` | every pull request, every push to `main` | `scripts/ci.sh test` |
| `binary` | `ubuntu-latest` | every pull request, every push to `main` | `scripts/ci.sh binary` |
| `macos` | `macos-latest` | `ci:macos` label or manual dispatch, from `.github/workflows/ci-macos.yml` | `scripts/ci.sh` |

The three stages together are the whole script, split so they run side by side:

- **`lint`**: `scripts/lib_test.sh`, `sh -n install.sh`, `shellcheck` on
  `install.sh`, `scripts/release.sh` and `scripts/ci_scope.sh`,
  `THIRD-PARTY-NOTICES` against `Cargo.lock`
  (`python3 scripts/third_party_notices.py --check`), the relative links and
  anchors of every tracked Markdown document (`python3 scripts/check_links.py`),
  `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`.
- **`test`**: `cargo test --workspace`.
- **`binary`**: builds the release binary, then checks everything that needs it
  or Python: the generated clients against the protocol types, the Python script
  suites (the installer tests among them) and client suite, the TypeScript client
  (typecheck and tests), the release timing test, the mock end-to-end, the §9.8
  budgets and process measurements, the release install check, and the tmux
  capture lanes.

## Run the same checks locally

```sh
make ci                   # every stage, in order: lint, test, binary
scripts/ci.sh lint test   # only the stages you name, in the order given
```

`lint` and `test` need the toolchain in `rust-toolchain.toml`, git and Python 3;
`lint` also uses `shellcheck`. `binary` also needs Python 3.11+, Node 22.6+ with
npm, tmux, Pillow and `dash`. Locally, a check whose tool is missing is skipped
with a yellow `skipped (needs ...)` line, and the Pillow-backed tests and the
`dash` installer test skip themselves. The Linux jobs install or verify these
tools in a step of their own, so none of those checks is skipped there.

## Where it runs, and what a fork can trigger

Every job in the workflow runs on a GitHub-hosted runner: a fresh virtual machine
that is discarded after the job. The workflow names no self-hosted runner.

- The only triggers of both workflows are `pull_request`, `push` to `main`
  (`ci.yml` only) and `workflow_dispatch`. There is no `pull_request_target` and no `workflow_run`, so code from a fork
  never runs with more than a fork's rights.
- The token is `contents: read` for every job. No job reads a secret, and the
  checkout does not keep its credentials (`persist-credentials: false`).
- Every action is pinned to a full commit SHA, with its version in a comment.
  To update one, replace the SHA and the comment together.
- `Swatinem/rust-cache` restores build dependencies. Only a run on `main` saves a
  cache; a pull request reads it.

A `pull_request` run uses the workflow file of the pull request, not the one on
`main`. A fork can therefore edit its copy: change a job's `runs-on`, drop the
`ci:macos` condition, or save a cache. What GitHub enforces whatever the file
says is what protects the repository:

- a pull request from a fork gets a read-only token and no secrets;
- a cache it saves is visible to that pull request only, never to `main`;
- it can reach **any runner registered to the repository**, by naming that
  runner's labels in `runs-on`.

**So the repository must have no self-hosted runner registered while it is
public.** Removing the label from this workflow is not enough: a runner that is
still registered (Settings → Actions → Runners) would run a fork's code on the
machine it lives on. With none registered, a fork can start jobs on GitHub-hosted
runners with a read-only token, and nothing else. Do not register one later for
this repository; a maintainer who wants a private runner should attach it to a
private repository instead. Requiring approval before a first-time contributor's
workflows run (Settings → Actions → General) is a second line of defence, not a
replacement.

CI sets two build options a local run does not: `CARGO_INCREMENTAL=0` and
`CARGO_PROFILE_DEV_DEBUG=line-tables-only`. With full debug info the debug test
binaries of the workspace take about 21 GB, more than a hosted runner's disk; with
line tables only they take about 8 GB, and a panic still reports its file and line.
The commands and the assertions are the same.

A newer push to the same branch cancels the run it supersedes, on `main` too: the
newest commit is the tree that needs checking.

When the `binary` job fails it uploads its temporary directory (daemon and
end-to-end logs, retained captures) and `target/pb-*` as the `binary-check-logs`
artifact.

## A change to documentation only

A change that touches nothing but documentation does not build or test the
workspace. Documentation is `**/*.md`, `docs/**`, `LICENSE` and the files under
`.github/` that are not workflows. `docs/engines.md` and `docs/tui.md` are the
exception: `cargo test` reads both, so they count as code.

- **On a pull request** the three jobs still start, and each ends after two
  steps: the checkout, and `scripts/ci_scope.sh`, which reads the paths the pull
  request changes (`git diff --name-only HEAD^1 HEAD` on the merge commit, hence
  `fetch-depth: 2`) and prints `code=false`. Every later step is skipped, the
  `lint` job runs the link check alone, and `lint`, `test` and `binary` report
  within a minute. The workflow has no `paths` filter on `pull_request`
  on purpose: a workflow that a filter skips reports nothing, and a check that
  branch protection requires would then wait forever. A job-level `if` on a
  matrix job has the same flaw, which is why the decision is a step.
- **On a push to `main`** nothing is required of the run, so the trigger itself
  carries the filter (`paths` with the same list, negated) and no run starts.
- **A manual dispatch** always runs everything.

`scripts/test_ci_scope.py` holds the list: it fails when the script and the
push trigger of `ci.yml` disagree, or when `pull_request` gains a path filter.

## macOS: label or manual dispatch only

GitHub bills macOS runners at roughly ten times the Linux rate, so the `macos`
job has a workflow of its own, `.github/workflows/ci-macos.yml`, and runs the
whole script only when:

- that workflow is triggered manually (`workflow_dispatch`), or
- a pull request carries the `ci:macos` label.

Adding the label to an open pull request starts the `macos` job and nothing
else: `ci.yml` does not listen for labels, so the Linux jobs are neither started
again nor cancelled, and their results stay as they were. While the label stays
on, later pushes to that pull request run macOS too. On a pull request without
the label the workflow starts, the job is skipped, and no runner is used. A
manual dispatch of `ci.yml` runs the Linux jobs only. The capture lanes are
skipped where tmux is missing, and the process measurements are Linux-only.

## Why `cargo test`, and not a faster runner

The `test` job compiles for longer than it tests: on a cold cache about nine
minutes of compiling and six of running. `cargo nextest` would shorten only the
second part, and it runs every test in a process of its own. The kernel tests
of `aether-runtime` cap how many of them fork a real `bash` at once with one
semaphore per test binary (`crates/aether-runtime/tests/kernel/fixture.rs`); a
process per test would have a semaphore per test, the cap would bound nothing,
and those tests time out on a loaded machine. The doctests would also need a
second command. So the stage stays `cargo test --workspace`, the same command
`make test` runs.

## What CI does not cover

- **The container backend lane** (`crates/aether-tools/tests/container_backend.rs`).
  It needs Docker and a local image and is not part of `cargo test --workspace`.
  Run it with `AETHER_TEST_CONTAINER_IMAGE` set, as `scripts/ci.sh` prints.
- **Tests marked `#[ignore]`** that need a real sandbox (`bwrap`, `socat`), a
  headless browser, or a live provider and credentials.
- **macOS on every pull request**, for the cost reason above.
- **Timing on dedicated hardware.** The release budgets run on a shared virtual
  machine, like every other hosted job.

## Capture lanes

The four full theme sessions run together, then the four width smoke sessions run
together. Each invocation owns its temporary directory, daemon and socket, private
tmux server and session, and dynamically allocated mock port. Every PID is waited
on even after a failure, and any failed capture fails the stage before its checker
group runs. Failed captures are kept and their directory is printed.

The lanes are written to behave on tmux 3.4, which Ubuntu 24.04 packages, as
they do on a current tmux:

- A session ends when tmux reports how `aether` exited, not when its pane closes.
  Before tmux 3.7 a pane is dead from the moment its terminal closes, and its
  exit status follows only once tmux has reaped the process. Before 3.6 a tmux
  built with utempter, as Ubuntu's is, can lose the `SIGCHLD` of an exit that
  comes within a millisecond of the closed terminal and then never has a status,
  so the script sends the tmux server a `SIGCHLD` while it waits, as tmux does
  itself from 3.6. A quit that fails names the exit status or the signal.
- The script sets the pane's `COLORTERM`. tmux sets it only from 3.6 on, and
  without it `aether` paints 256-colour indexes instead of the theme's colours.
- After an Escape the script waits for the panel to close before it sends the
  next key. Read together, an Escape and a key are Alt+key.

## Installer checks

The `lint` stage runs `sh -n install.sh` and, when `shellcheck` is installed,
`shellcheck` on `install.sh`, `scripts/release.sh` and `scripts/ci_scope.sh`.
The `binary` stage runs `scripts/test_install.py`. That test feeds `install.sh`
on stdin to `dash` and to `sh`, against a fake release on a local server, and never reaches
github.com. Where `dash` is not installed it covers `sh` only and shows it:
`test_dash_is_one_of_the_shells` is skipped, with the shells that did run as the
reason, so the run ends with `OK (skipped=1)`. The hosted Linux jobs check that
`shellcheck` and `dash` are present before the script starts, so neither check
is skipped in CI. The hosted job installs Ubuntu's packaged `shellcheck` (0.9.0
on Ubuntu 24.04), which reports findings that newer versions do not, such as
SC2015 on `A && B || C`; a finding of any severity fails the stage.

The `binary` stage also runs the release workflow's install check
(`scripts/release.sh install-check`, described below) on the binary it built,
packed under the archive name `install.sh` asks for on that machine, and
`scripts/test_release_install_check.py`, which gives the check archives of a
fake `aether` that it must refuse.

## Release workflow

`.github/workflows/release.yml` builds the binaries that `install.sh` and
`aether update` download ([Install, update, uninstall](install.md)). It uses
GitHub-hosted runners only, one of each target's own kind, so every binary is
run once (`aether --version`) before it is archived:

| Target | Runner |
| --- | --- |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` |
| `aarch64-apple-darwin` | `macos-15` |
| `x86_64-apple-darwin` | `macos-15-intel` |

The Linux binaries are static (musl, with Ubuntu's `musl-tools` as the C
compiler), so no glibc version applies. Each job installs the toolchain
`rust-toolchain.toml` pins and calls `scripts/release.sh`, which builds with
`--release --locked`; the release profile strips the binary. Each archive holds
`aether`, `LICENSE` and `THIRD-PARTY-NOTICES`.

Each build job then installs its own archive the way a user does
(`scripts/release.sh install-check <target> dist`), and a job that fails here
uploads nothing, so nothing can be published:

- the archive, its `.sha256` and `VERSION` are served from the runner itself in
  the layout of a release, `download/<tag>/` and `latest/download/`;
- the real `install.sh` runs twice from an empty directory, once for the latest
  release and once with `AETHER_VERSION=<tag>`, with `AETHER_INSTALL_BASE_URL`,
  `AETHER_INSTALL_DIR` and a temporary `HOME`. The install directory must then
  hold `aether` and the installer's `.aether-receipt`, and nothing else; the
  empty directory and the home must still be empty;
- the installed `aether --version` must print the workspace version, and
  `aether update --check` against the same server must exit 0 and print that
  this version is up to date.

The phone web app is part of that binary: `crates/aether-remote/build.rs`
embeds the tracked runtime files under `crates/aether-remote/web/` with
`include_bytes!`, so a release build needs no Node or bundler and no web files
beside the binary. The `binary` stage of `scripts/ci.sh` runs the app's type
checks and unit tests, and its phone-viewport browser tests (including a run
against the built daemon) wherever Chromium is installed under
`web/node_modules/.browsers`, as the hosted workflow does.

- **A pushed tag `vMAJOR.MINOR.PATCH`** must equal the workspace version in
  `Cargo.toml`, and `CHANGELOG.md` must have a `## MAJOR.MINOR.PATCH` section
  with text, or the run fails before anything is built. It then creates a
  **draft** release with these assets: four `aether-<target>.tar.gz`, their
  `.sha256` files, `SHA256SUMS`, and `VERSION` (the tag, which `aether update`
  reads from the latest release). The draft's text is that changelog section
  followed by the install command (`scripts/release.sh notes <tag>` prints
  it). Nothing is downloadable as "latest" until the draft is published by
  hand.
- **A manual run** (`workflow_dispatch`) is a dry run unless its box is
  unticked: the same build, with the assets uploaded as the `release-assets`
  workflow artifact and no release. A manual run that is not a dry run must be
  started on a tag.

Only the `publish` job can write: `contents: write` for the release, plus
`id-token: write` and `attestations: write` for the build provenance
attestation, which is made only while the repository is public. Every action is
pinned to a commit, and the only credential is the run's own `GITHUB_TOKEN`.

The same steps run locally:

```sh
scripts/release.sh check-tag v1.0.0
scripts/release.sh notes v1.0.0
scripts/release.sh package x86_64-unknown-linux-musl dist   # needs musl-gcc
scripts/release.sh install-check x86_64-unknown-linux-musl dist
```

### Cutting a release

Before the first tag, and after any change to the workflow, start a dry run and
look at its artifact: `gh workflow run release.yml --ref main -f dry_run=true`.
Until one has passed, only the x86_64 Linux build is known to work.

1. Set `version` under `[workspace.package]` in `Cargo.toml`, refresh
   `Cargo.lock` (`cargo check`), write the release's `## MAJOR.MINOR.PATCH`
   section in `CHANGELOG.md`, and merge that to `main`.
2. Tag that commit and push the tag: `git tag v1.0.0 && git push origin v1.0.0`.
3. When the run is green, read the draft (`gh release view v1.0.0`) and publish
   it: `gh release edit v1.0.0 --draft=false`.
4. Check the published release from a clean directory:

```sh
dir=$(mktemp -d)
curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | AETHER_INSTALL_DIR="$dir" sh
"$dir/aether" --version
"$dir/aether" update --check; echo "exit code: $?"
curl -fsSL https://github.com/mengsig/aether/releases/latest/download/SHA256SUMS
gh release download --repo mengsig/aether --pattern 'aether-x86_64-unknown-linux-musl.tar.gz' --dir "$dir"
gh attestation verify "$dir/aether-x86_64-unknown-linux-musl.tar.gz" --repo mengsig/aether
```

## §9.8 process measurements (partial)

`scripts/ci.sh` retains the render, key-response and memory gates and also runs:

```sh
cargo build --release -p aether
python3 -m unittest discover -s scripts -p test_tui_process_budgets.py
cargo test --release -p aether-tui --lib runner::process_tests::measured_process_budgets \
  -- --exact --nocapture --test-threads=1
```

The process test is ignored in debug and explicitly skips non-Linux platforms.
It requires Python 3.11+, `/proc` and PTYs; it does not require tmux, a provider,
credentials, the operator's daemon, or network access. Each fixture owns its
foreground daemon, config, persistent data and Unix socket (memory-backed while it
runs: see the next section) and a receipts directory `target/pb-*`.
Each child starts its own session/process group. Cleanup kills
that owned group (also if its leader already exited) and reaps the direct child,
including stopped children and SIGTERM cancellation. Successfully stopped groups
are retired by child handle, so later cleanup never signals their recycled numeric
IDs. SIGTERM/SIGALRM during cleanup are deferred until all owned groups are handled;
then cancellation is raised, or noted on an already-active original failure. Descendants in the group are
killed; an adopting parent, not the controller, reaps orphaned descendants. Controller cancellation/deadline
exceptions are not retryable connection errors. The controller has a 90-second work deadline plus bounded cleanup; connection,
readiness, observation and cleanup waits are individually bounded. JSON receipts,
expected events, daemon logs and drained terminal output remain in that directory;
measurements and artifact paths also appear in the CI job log.

### The fixture's daemon state is memory-backed

A daemon creates its socket, lock and pid files at start-up, and its store syncs
every commit. With those under `target/`, a fixture daemon's start-up waited on
the build disk, and on a runner whose disk stalled for tens of seconds the
15-second readiness wait expired before the daemon had finished creating its
database ("daemon readiness timeout"). Nothing in §9.8 budgets disk latency, so
each fixture keeps its socket, config and data in a private directory under
`/dev/shm` while it runs. Receipts, daemon logs and terminal output are still
written to `target/pb-*`. Cleanup copies `config/` and `data/` there as well and
removes the memory-backed directory; a failed copy fails cleanup and names the
directory that was left.

This removes the build disk from start-up, not memory pressure: `/dev/shm` is
backed by swap, and the binaries are still paged in from disk. On a host that is
swapping heavily a daemon start can still take longer than the readiness wait.

A controller that is killed outright (`SIGKILL`, a cancelled job) cannot clean up,
and its `/dev/shm/aether-pb-*` directory then holds memory until a reboot. With no
process measurement running, remove leftovers with `rm -rf /dev/shm/aether-pb-*`.

The cost: the disk sync of the few events seeded during a restart is no longer
part of ready-to-replay. Reading, replay, reconnect and reduction still are, and
the under-1 s gate and the 15-second readiness wait are unchanged. The gate has
no slack multiplier, so it still fails on a host too loaded to run the daemon and
the TUI within a second.

### Idle CPU: measured, not yet enforced

The **unmodified release CLI/TUI binary** runs in a 160×200 PTY with an empty
daemon initially. The real CLI automatically creates one waiting root. The fixture
waits for it (15 seconds at most: a loaded host can need longer than the settling
time), lets start-up settle for four seconds, then verifies exactly that one waiting
agent, zero running/blocked agents, no active tasks or engine turns. It supplies no input
and verifies the same quiescent state and unchanged durable event cursor after all
samples, so no agent animation/work is hidden between snapshots. Three fixed
five-second windows sample the
TUI PID's `/proc/<pid>/stat` user+system CPU counters. The controller drains terminal
output in a different process; daemon CPU, child CPU and host load are not added.
Each record reports raw tick delta, `SC_CLK_TCK`, actual monotonic wall duration,
one-core CPU percentage and the accounting quantum as a percentage of that window.
At 100 Hz the quantum is 10 ms. User/system counters are rounded independently:
**zero sampled ticks is a finite-resolution observation, not literal zero CPU**
(the summed rounding uncertainty can approach two ticks).

The old frame-period assertion is now honestly named as a frame-rate selection
unit test. A 250 ms housekeeping timer and periodic Status requests still run;
there is no claim that the no-timers requirement is met. The baseline measured
0, 0 and 1 ticks in three 5 s windows (the nonzero window was about 0.20% CPU).
That baseline checked the empty daemon before launching the CLI; the refined
fixture additionally verifies the resulting waiting root throughout sampling.
Consequently this lane **does not enforce the literal idle-zero budget**, invent
a replacement CPU ceiling, or close the gap. It prints `OPEN GAP` even when all
three counters happen to round to zero. The controller's optional
`--enforce-idle-cpu` fails on any nonzero sample, but a pass still cannot establish
no timers. An event-driven housekeeping/status design and its no-wakeup regression
tests remain follow-up work; multiplying zero by the other suites' slack cannot
resolve this gap.

### Restart/replay: measured and enforced

The runner child is the release TUI unit-test executable in a real PTY, using the
CLI's Tokio runtime configuration and the **same `run_inner` loop, request effects,
subscription, reconnect worker and reducer** as production. A `cfg(test)` observer
records subscription frames and the cursor/timeline **after reduction**; it neither
injects events nor drives reconnection. CPU sampling never uses this observer.

Each of three independent cases starts live delivery, stops the actual daemon long
enough to leave a Status RPC pending, kills/reaps it, observes the failed reply and
disconnect, then restarts the daemon over the same database after a fixed 0.15,
3.25 or 5.25 s outage delay. The TUI is briefly stopped while missed task events are
committed after restart, ensuring the fixture really requires replay. That seeding
and resume cost is included in the measured reconnect interval. It asserts exact
ordered sequence **and full event content**, both at the subscription and reducer
timeline, against a fresh durable event query. Two subsequent live events and a
1.1 s observation tail catch late delivery, duplicate subscriptions or reconnects.
The fixture covers durable task/startup events, not provider streaming deltas or an
arbitrarily large historical transcript.

Two separate timestamps keep daemon downtime distinct from client responsiveness:

- **Crash-to-ready** starts immediately before killing the stopped daemon and ends
  at the controller's first successful handshake + Status reply from the new daemon.
- **Ready-to-replay** ends when the controller observes the runner's post-reduction
  receipt for the last required durable event. This is gated at **under 1 s**, without
  a slack multiplier. Ready probes are 10 ms apart; receipt polls are 5 ms apart, so
  these are observed process intervals, not exact kernel readiness timestamps.
- **Crash-to-replay** is also printed, but cannot be held under 1 s while the fixture
  deliberately keeps the daemon unavailable for several seconds.

The baseline 2 s retry cap produced **1.651 s ready-to-replay** after the 3.25 s
outage (3.471 s crash-to-ready, 5.122 s total), despite correct replay. The narrow
production fix caps exponential retry sleeps at 500 ms instead: 100, 200, 400,
then 500 ms. It preserves the existing single-worker/cancellation path and gives
handshake/replay time inside the budget. The tradeoff is at most about **two offline
connection attempts per second** instead of one every two seconds, not a busy loop.
The timing gate checks the process result, not these constants.
