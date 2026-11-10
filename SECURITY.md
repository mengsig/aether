# Security policy

## Supported versions

| Version | Supported |
| --- | --- |
| 1.x | Yes: fixes go into the latest 1.x release |
| 0.1.x | No |

Fixes are not backported to older releases. Upgrade to the latest release
before reporting, and say which version you tested.

## Reporting a vulnerability

Report privately through GitHub: open the repository's **Security** tab and
choose **Report a vulnerability**
([direct link](https://github.com/mengsig/aether/security/advisories/new)).
This opens a private advisory that only the maintainers and you can see.

Do not open a public issue, pull request or discussion for a vulnerability,
and do not include exploit details in one.

### What to include

- The Aether version (`aether --version`) or commit, and your operating system.
- Which engine or provider was in use (Claude Code, Codex, an API provider)
  and its version, when the problem involves one.
- The configuration that matters: approval mode, network scope, execution
  backend (`host` or `container`), `allowed_paths`, MCP servers, GitHub watcher.
- Steps to reproduce, ideally with a minimal prompt or script, and what you
  expected to happen instead.
- The impact you see: what an attacker gains, and what they need to start with.

Remove real credentials, tokens and private paths from logs before attaching
them. `aether diagnose` produces a redacted bundle; check it before sharing.

### What to expect

This is a small project and no response time is promised. In plain terms:

- A maintainer reads the report, replies in the advisory, and asks questions
  there if something is unclear.
- If the report is confirmed, a fix is prepared privately, released in a 1.x
  version, and published together with the advisory. You are credited unless
  you ask not to be.
- If the report describes documented behaviour or something out of scope, the
  maintainer says so and explains why. You are then free to discuss it publicly.

There is no bug bounty.

## Scope: what Aether does and does not protect

Aether is an agent harness. It starts model-driven agents that run shell
commands and edit files **with your own user's permissions**, on your machine,
in the directory you give them. Whoever can instruct an agent can do what your
account can do. That is the design, not a vulnerability.

The boundaries Aether does maintain, where a bypass is a security bug:

- **The daemon's local interface.** The Unix socket and Aether's private
  files (configuration, credentials, database, logs, exports) are for the
  owning user only. Remote TCP access requires a token.
- **Agent scope.** A scope that denies network access, limits writable paths
  through Aether's own file tools, or selects the container backend for
  Aether's shell is enforced as documented.
- **Approvals.** When a policy requires approval for a class of tool call,
  the call does not run before it is granted.
- **Credential handling.** Stored provider credentials are kept in
  owner-only files, redacted from the event log and from diagnostic bundles,
  and hidden from Aether's built-in tools and from MCP servers.
- **Redaction of tool output.** Secret shapes and the secret values the
  daemon knows are masked in tool outcomes and errors, harness outcomes,
  native-engine bridge tool replies, refusals and denial notes, approval and
  rule notes and revocation reasons as they are recorded, the full-output
  artifacts tools store, `read_artifact` replies to any
  connection, and run exports. What the masking cannot see is listed in
  [known limitations](docs/known-limitations.md#security-boundaries).
- **External triggers.** The GitHub watcher and signed webhooks only dispatch
  requests that pass their documented permission, allowlist and signature
  checks, and never grant more authority than documented.
- **Outbound requests.** `web_fetch`, `web_search` and event subscriptions
  refuse the private and link-local destinations they are documented to refuse.

Out of scope, because Aether does not claim to prevent it:

- An agent doing something harmful that your own account is allowed to do,
  including after reading hostile text in a repository, web page, issue or
  tool result (prompt injection). Use a container, a separate account or a
  disposable workspace for work you do not trust.
- The behaviour of the Claude Code and Codex CLIs, model providers and MCP
  servers themselves. Report those to their maintainers.
- Running with approvals off, which is the shipped default, or with
  `[approval] yolo = true`, which also runs the engines unsandboxed.
- Exposing the daemon's TCP port to an untrusted network. It has no built-in
  TLS; use an SSH tunnel.
- Denial of service by a local user who can already run commands as you.

### Known limitations

These are documented gaps, and not new reports:

- The `allowed_paths` Git guard does not reach an engine's own Bash tool, so a
  Claude Code or Codex agent can run a destructive Git command outside its
  assigned paths.
- Commands run by the Codex app-server can inherit provider credentials from
  that process's environment in the cases the documentation lists.

Both are described in [known limitations](docs/known-limitations.md), with
the other boundaries that look stronger than they are. A report that shows one
of them is worse than documented is welcome.
