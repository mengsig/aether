# Contributing to Aether

Thanks for wanting to improve Aether. This page says how to build and test,
which rules the code follows, and what a pull request needs before it merges.

Everyone taking part is expected to follow the
[code of conduct](CODE_OF_CONDUCT.md). To report a vulnerability, use the
private route in [SECURITY.md](SECURITY.md), not an issue or a pull request.

## Build and test

You need Linux or macOS, `git`, and [rustup](https://rustup.rs). The toolchain
is pinned in `rust-toolchain.toml` (Rust 1.98.1 with `clippy` and `rustfmt`);
rustup installs it the first time you run `cargo` in the checkout. The
declared minimum (`rust-version` in `Cargo.toml`) is 1.90; CI tests the pinned
version only.

```sh
git clone https://github.com/mengsig/aether.git
cd aether
make ci      # everything CI runs: scripts/ci.sh
```

`make ci` is the check a pull request has to pass. While you work, the
narrower targets are faster:

```sh
make fmt     # cargo fmt --all
make lint    # cargo clippy --workspace --all-targets -- -D warnings
make test    # cargo test --workspace
make build   # cargo build --release -p aether
cargo test -p aether-runtime some_test_name   # one crate, one test
```

Things worth knowing:

- **Host CI slots.** `make ci` first takes one of `CI_HOST_SLOTS` (default 1)
  host-wide slots that Archon's `make ci` shares, and says once that it waits
  when all are taken. `CI_HOST_SLOTS=0 make ci` or `make ci-checks` runs
  without one; see [docs/ci.md](docs/ci.md#host-ci-slots).
- **cargo-nextest.** The test stage of `make ci` runs the tests under
  [cargo-nextest](https://nexte.st/) with one retry, and lists a test that
  passed only on its retry as `FLAKY`. Install it with
  `cargo install cargo-nextest --locked`; without it the stage runs
  `cargo test` and says so.
- **Memory.** A full build links many test binaries. On a machine with little
  free memory, limit the parallelism: `CARGO_BUILD_JOBS=4 make ci`, and
  `NEXTEST_TEST_THREADS=4` for the tests (`RUST_TEST_THREADS=4` with
  `make test`). The hosted CI sets neither: each of its jobs has a runner to
  itself.
- **Optional tools.** `make ci` also runs the Python and TypeScript client
  suites, a mock end-to-end run and terminal captures. Those steps need
  `python3` (3.11 or newer), Node.js 22.6 or newer with `npm`, and `tmux`. A
  step whose tool is missing is skipped with a yellow notice, and CI on the
  pull request still runs it.
- **Generated clients.** `clients/python` and `clients/typescript` contain
  types generated from the Rust protocol. After changing a protocol type,
  build the binary and run
  `python3 scripts/generate_clients.py --binary target/release/aether`, then
  commit the result; CI fails on drift.
- **Third-party notices.** `THIRD-PARTY-NOTICES` lists the crates in the
  released binary with their licence texts, and goes into every release
  archive. After a change to `Cargo.lock`, run
  `python3 scripts/third_party_notices.py` and commit the result; CI fails
  when the file is out of date.
- **No account needed.** The test suite uses a mock provider. It never calls
  a model and needs no API key. The benchmarks in `evals/` do spend tokens
  and are not part of CI.

[docs/development.md](docs/development.md) has the build, test and CI entry
points in one place. [ARCHITECTURE.md](ARCHITECTURE.md) explains which crate
owns what, and [docs/ci.md](docs/ci.md) describes each CI lane.

## Code rules

These are the rules reviews apply. Most are judgement, some are enforced by
the lints in `Cargo.toml`.

- **Write for the next reader.** Clear names, explicit data flow, flat control
  flow with early returns. No cleverness that needs a paragraph to defend.
- **Small functions.** Keep a function under about 60 lines and with one
  responsibility. Split by meaning, not by line count, and avoid files that
  grow without bound.
- **Real assertions.** Check the assumptions a function depends on: input
  shape and range, state invariants, results of a transformation. Each check
  should be able to catch a plausible bug. Validate expected bad input at the
  boundary and return a useful error; keep assertions for programmer mistakes.
- **No swallowed errors.** Do not ignore a `Result`, catch an error only to
  log it, or continue with made-up data after a failure. Add context and
  return it. `unwrap`, `expect` and `panic!` are lint errors outside tests,
  and `unsafe` is forbidden in the workspace.
- **Tests with the change.** A fix comes with a test that fails without it. A
  feature comes with tests for its main path and its edge cases. Test through
  public interfaces rather than by matching source text.
- **Simple before general.** No new abstraction, configuration layer or
  dependency without a need you can show. A new dependency must have a
  permissive licence.
- **Efficiency by design.** Pick a sensible algorithm and data structure up
  front; do not micro-optimise cold code.
- **Comments are rare and short.** Explain what the code cannot say itself.
- **Formatting and lints are not optional.** `cargo fmt` and
  `cargo clippy -- -D warnings` must be clean.

Do not commit secrets, private paths, account names or machine names, in
code, tests, fixtures or documentation. Use obvious placeholders such as
`octocat`, `example-org/example-repo`, `/home/user` and `example.com`.

## Proposing a change

- **Small fixes** (a bug with a clear cause, a documentation correction):
  open a pull request directly.
- **Anything larger** (a new feature, a new dependency, a protocol or
  configuration change, a refactor across crates): open an issue first and
  wait for a maintainer's answer before you write the code. This saves you
  from building something that cannot be merged.
- Check [known limitations](docs/known-limitations.md) and the
  [open issues](https://github.com/mengsig/aether/issues) first: the gap you
  found may already be described there.

## Pull requests

Fork the repository, work on a branch, and open the pull request against
`main`. A pull request needs:

1. **A description** of what changes and why, and how you tested it. The
   template asks for these.
2. **Tests** that cover the change, and updated documentation when behaviour
   that users see changes.
3. **Green CI.** On a pull request from a fork, a maintainer may have to
   approve the workflow run before it starts.
4. **One approving review from a maintainer.** Nobody merges their own pull
   request without it, and opening a pull request does not give write access.

Keep a pull request to one topic. Unrelated refactoring, reformatting and
dependency updates belong in their own pull requests. Use
[conventional commit](https://www.conventionalcommits.org) subjects
(`fix(tui): …`, `feat(runtime): …`, `docs: …`), as the history does.

## Licence

Aether is released under the [MIT licence](LICENSE). By contributing, you
agree that your contribution is licensed under the same terms (inbound =
outbound). There is no contributor licence agreement to sign. Only submit
work that you have the right to submit under that licence.
