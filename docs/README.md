# Documentation

Start with the [README](../README.md) for prerequisites, installation, and a first task.

## Current guides

- [Install, update, uninstall](install.md): the installer, `aether update`,
  checksums, where files live, and building from source.
- [User guide](guide.md): models, delegation, supervision, automation, and recovery.
- [Prerequisite details](prerequisites.md): optional programs and feature-specific setup.
- [Configuration](configuration.md): common settings, defaults, and storage paths.
- [Harness measurements](benchmarks.md): the detailed comparisons formerly in the README.
- [Release bench](release-bench.md): `make bench-release`, the regression
  check run before each release on the last release, the candidate and the
  native CLIs.
- [Development](development.md): build, test, and CI entry points.
- [Architecture](../ARCHITECTURE.md): ownership, execution paths, persistence,
  and where code lives.
- [Engine integration](engines.md): native Claude Code/Codex behavior and the
  adapter contract.
- [Harness tools](harness-tools.md): Aether coordination and background-work
  semantics.
- [MCP servers](mcp-servers.md): configured servers and the caller identity
  each call carries in `_meta`.
- [Daemon protocol](protocol.md): request correlation, malformed-frame errors,
  the served version window with what protocols 18 and 19 changed, and images
  on a message.
- [Phone setup](remote-setup.md): the guided `aether remote setup` for a
  non-technical reader, with troubleshooting and what protects you.
- [Remote control](remote.md): the phone web app's contract (endpoints, device
  handshake, pairing, authorization matrix, push, reconnect) and SSH remotes.
- [GitHub watcher](github-watcher.md): fixed/per-thread routing, permissions,
  matching, and live configuration.
- [Terminal UI](tui.md): layout, the agents board, approvals, keys, and
  commands.
- [Local voice](voice.md): dictation, speech, setup, and opt-in controls.
- [Theme overrides](theme-overrides.md): the accent colour override.
- [Retention](log-retention.md): logs, completed-run history, and exclusions.
- [Diagnostic bundles](diagnose.md): read-only collection, run cursors and redaction.
- [Migration backups](migration-backups.md): database rollback snapshots.
- [Status timestamps](status-time.md): accurate state-duration semantics.
- [CI](ci.md): local/hosted checks and optional lanes.
- [Known limitations](known-limitations.md): security boundaries and remaining gaps.
- [Benchmarks against the native harnesses](#benchmark-reports): dated
  reports with their method, raw tables and limits.
- [Configuration templates](../crates/aether-model/src/config/templates.rs):
  supported settings and shipped defaults.
- [Python client](../clients/python/README.md) and
  [TypeScript client](../clients/typescript/README.md).
- [Evaluation tooling](../evals/README.md): reproducible experiments, not a
  blanket claim of model parity.

## Benchmark reports

Each report states the binary, the cases, the seeds and what it does not
show. Reports are never pooled across protocol versions. To run a comparison
yourself, see the [evaluation tooling](../evals/README.md) and its
[protocol](../evals/harness_bench_protocol.md).

- [Release bench 1.2.3](release-bench/1.2.3.md): Aether 1.2.3 against 1.2.2
  on set v2, 440 trials and a 220-trial Luna rerun; Haiku +0.9 points, Luna
  -9.1 in the first run, not confirmed by the rerun (+3.6) and -2.7 pooled:
  no decrease. The verdict is INCONCLUSIVE because v2 is off target.
- [Release bench 1.2.2](release-bench/1.2.2.md): Aether 1.2.2 against 1.2.1
  on set v2, 440 trials; the gate passes on both tiers, Haiku +0.0 and Luna
  +4.5 points: no decrease. The verdict is INCONCLUSIVE because v2 is off
  target.
- [Release bench 1.2.1](release-bench/1.2.1.md): Aether 1.2.1 against 1.2.0
  on set v2 in three runs; the gate passes on both tiers each time, and
  Haiku, 6.4 points lower in the first run, is -1.1 points pooled over 361
  pairs: no decrease, within noise. Each run's verdict is INCONCLUSIVE
  because v2 is off target.
- [Release bench 1.2.0](release-bench/1.2.0.md): Aether 1.2.0 against 1.1.1
  on set v2, 660 trials; the gate passes on both tiers, and the verdict is
  INCONCLUSIVE because v2 is off target.
- [Release bench A/A check of set v2](release-bench-2026-10-18-aa-v2.md): Aether
  1.1.0 against itself on 110 tasks and six arms, 660 trials; the gate passes on
  both tiers, and the verdict is INCONCLUSIVE because v2 is off target.
- [Release bench A/A check of set v1](release-bench-2026-10-18-aa.md): Aether
  1.1.0 against itself on 92 tasks and six arms, 552 trials; the gate passes.
- [Native versus Aether, six cases, second sitting](harness-bench-2026-10-11.md):
  Haiku 5.5 and GPT-6 Luna at medium effort, 72 trials on six held-out-graded
  tasks under protocol version 2, with paired statistics.
- [Native versus Aether, six cases, first sitting](harness-bench-2026-10-10.md):
  the same 72 trials under protocol version 1.
- [Lower-tier comparison](harness-lower-tier-results-2026-10-10.md): five
  counted runs of matched Haiku 5.5 and GPT-6 Luna low-effort cells on two
  cases, native against Aether.
- [Lower-tier comparison, protocol version 2](harness-lower-tier-results-2026-10-10-protocol-2.md):
  five planned runs after Aether stopped repeating the effort in words and
  both sides of a Claude pair started with the same MCP set.
- [Lower-tier comparison, counted again](harness-lower-tier-results-2026-10-11.md):
  five more planned runs under protocol version 2, with paired statistics
  over those runs and over all ten.
- [Harness overhead diagnosis](harness-overhead-diagnosis-2026-10-10.md):
  diagnostic arms, never counted, that look for why those two models behave
  differently through Aether than natively.

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for how to build, test and propose a
change, [SECURITY.md](../SECURITY.md) for reporting a vulnerability, and the
[changelog](../CHANGELOG.md) for what changed between releases.
