# Known limitations

This page lists what Aether 1.x does not do, or does not yet do well. It
separates shipped capabilities from unfinished work. An entry marked
"(roadmap)" is a larger piece of work that is not part of 1.0; those are
tracked in the [issue tracker](https://github.com/mengsig/aether/issues).

To report a vulnerability, follow [SECURITY.md](../SECURITY.md) instead of
opening a public issue.

## Security boundaries

Aether runs agents as your own user. An agent can read and change whatever
your account can, unless you narrow its scope or run it in a container.
Nothing below is a sandbox escape by itself: these are places where a setting
looks like a boundary and is not one.

- **Approvals are permissive by default.** The shipped default (`yolo`)
  approves every tool call. `--no-yolo` (`approval.default = "ask"`) gates
  external side effects only: network, web tools and MCP servers. Shell
  commands and file edits inside an agent's scope never prompt under either
  default. Approval prompts are not filesystem isolation.
- **`allowed_paths` does not bound an engine's own shell.** The Git guard that
  refuses `git stash`, path `checkout`/`restore`, `reset --hard` and `clean`
  outside an agent's `allowed_paths` runs only in Aether's built-in shell. A
  Claude Code or Codex agent runs commands through the engine's native Bash
  tool, which never asks the guard, so a worker sharing a checkout can still
  discard a sibling's uncommitted edits. More generally, native engine tools do
  not all pass through Aether's allowed-path guards or resource leases, and can
  reach other paths when the engine's own permissions allow it.
- **Commands run by Codex can read provider credentials.** Aether hides
  configured provider credential variables from its built-in tools, from MCP
  servers and from the Codex loopback exec-server. The Codex app-server keeps
  those variables for its own model calls. On Codex 0.162.0 or newer Aether
  adds filters that keep them out of native commands, but the filter is skipped,
  with a visible notice, when the Codex configuration already sets
  `shell_environment_policy.exclude` or `include_only`, when the configuration
  cannot be inspected, and on older or unknown versions. The `codex exec`
  fallback adds no filter at all. Environment filtering is not credential
  isolation in any case: an unsandboxed turn can read the environment and
  files of other processes of the same user. See
  [Codex credentials and trust boundary](engines.md#codex-credentials-and-trust-boundary).
- **The shell Git guard is a guardrail, not a parser or a sandbox.** Common
  launch prefixes are recognized, but arbitrary wrappers (`eval`,
  `bash -c "git stash"`, a script, an alias) are not, and input typed into a
  `tty` session with `write_stdin` is not inspected.
- **Workspaces are shared unless you isolate them.** The default shared
  workspace does not separate concurrent edits. An isolated Git worktree
  separates ordinary file edits, but is not a security boundary for native
  tools. Inspect a child's result and integrate it deliberately.
- **The container backend covers Aether's shell calls only**, not the whole
  tool surface and not the native CLI engines. Isolated delegation requires
  the host backend because a linked Git worktree's metadata is outside the
  container's workspace mount.
- **Aether terminates no TLS itself.** Phone access goes through Tailscale or
  your own HTTPS proxy in front of a loopback listener, set up by
  `aether remote setup` ([guide](remote-setup.md)); it is private to your
  tailnet unless you opt in to Funnel. The `[listen]` TCP transport and its
  shared token have no TLS: use an SSH tunnel, and do not expose a daemon to an
  untrusted network.
- **A paired phone acts with your authority.** An operator device can approve
  tool calls that run commands and edit files on the host. Revoke a lost phone
  with `aether remote revoke` and remove it from your Tailscale account.
- **Tailscale HTTPS publishes machine names.** Enabling HTTPS certificates for
  a tailnet lists its machine names in public certificate-transparency logs.
- **The GitHub watcher turns repository text into agent work.** Who may
  trigger it is decided by repository permission and the allowlists described
  in the [GitHub watcher guide](github-watcher.md); the text itself is
  untrusted input to a model. Do not point the watcher at a privileged
  workspace.

## Files

- `read_file` and `edit_file` accept files up to 8 MiB; larger files return a
  size-limit error even when a read requests a small line window. `edit_file`
  requires valid UTF-8 and bounds the replacement size as well.
- File writes sync a sibling temporary file, preserve the target mode, and
  replace its inode atomically. Other hard links retain the old contents;
  in-scope symlink aliases resolve to the target before replacement.
  Replacement changes owner/group to those of a newly created file (including
  a setgid directory's group) and drops ACLs and extended attributes. It needs
  directory write permission, so a writable file in a non-writable directory
  or ownership restrictions in a sticky directory can prevent replacement.
  Bind-mounted targets can reject the rename with `EBUSY` or `EXDEV`.
- File reads and writes, and the `list_dir`, `glob` and `grep` walkers, open
  directories component by component without following symlinks, which
  prevents parent symlink swaps. Private report and export opens reject a
  final symlink but still follow parent directories. These controls are not a
  complete hostile-filesystem sandbox. An existing export `.partial` is
  refused; inspect and remove it to retry.
- Engine log retention needs durable creation ownership, a matching native
  session header and a retained owner row or transactional history-expiry record
  proving finality. User and legacy
  sessions, unknown paths, ambiguous ownership and unsupported native formats
  are preserved. A Failed agent can still resume its original native session;
  a durable restart successor or deletion of its history closes that route.
  History expiry now records final-state evidence before deleting the owner.
  Previously expired history without that evidence still protects native logs
  indefinitely; inspect `aether storage` for each path's decision and reason.
- Engine log ownership records do not persist the original inode or a content
  digest. GC rejects a different native session identity and rechecks the inode,
  size, permissions and modification/change times after inspection. A copied
  or externally edited log that still has the recorded session header cannot
  be distinguished from the original. Component-by-component directory handles
  prevent following symlink swaps, but GC is not an atomic conditional unlink
  against a hostile same-user filesystem writer. Keep native session storage
  private and avoid externally rewriting it during a sweep.
- Delegated workspace cleanup preserves Git layouts it cannot safely prove,
  including reftable, config includes, alternate object databases, symlinked
  metadata, cross-worktree symbolic refs and parent-traversal `gitdir` pointers.
  Ordinary linked source worktrees are supported. Broad ancestor scopes and
  `allowed_paths` permission alone do not establish workspace ownership;
  explicit workspace roots, manifest relationships and resource leases do.
- Build output pruning deletes a declared directory outside the commit hold,
  so a long deletion does not stall the daemon. New agents in that workspace
  are held back, but a finished owner resumed during the deletion can see its
  build output disappear and must rebuild. Removal still rechecks everything.

- An agent's private temp directory only covers programs that honour
  `TMPDIR`, `TMP` or `TEMP`. A tool that writes to a hardcoded `/tmp` path
  still writes to the shared `/tmp`. MCP servers that the daemon starts and
  shares between agents, and the daemon's own git helpers, keep the daemon's
  temp directory. Agent temp directories are not cleaned up automatically
  yet: `aether storage` shows their size, and you can remove a finished
  agent's directory by hand.
- The build cache is not evicted yet and has no byte budget: `aether storage`
  shows its size, and you can remove a finished run's partition by hand. Only
  programs that honour the configured variables (by default
  `CARGO_TARGET_DIR`) use it. The agents of one run share a partition and can
  overwrite each other's output; runs are kept apart by directory, not by an
  operating-system boundary, since every agent runs as the same user.
- On macOS the default agent temp root is long enough that a tool's Unix
  socket path under `TMPDIR` can exceed the 104-byte limit, and startup always
  warns about it. Set `[agents] temp_root` to a short, absolute directory that
  only you can write. If the default root cannot be used at all, agents keep
  the daemon's temp directory and only a warning says so.

- Images sent with a message are stored under `aether.attachments/` in the
  data directory and are never removed automatically, including when their
  agent is archived; delete old files by hand once no live agent needs them.
  An agent whose stored image is gone gets the message's text only. A send the
  daemon refuses stores nothing; a delivery whose images were written and whose
  commit then fails leaves those files behind.
- `aether runs export` does not include image files: an imported run shows
  each image as its `[image: name, size]` line.
- An agent on an API model sees the newest 20 images and 15 MiB of them as
  images; older ones in its history become a line naming the stored file,
  which it can open with a tool but no longer sees.
- An image over 3.75 MiB is refused, not shrunk; a full-screen capture of a
  4K display can exceed that as PNG. Crop it or save it as JPEG.
- Only people attach images. Agents cannot send images to each other or in
  replies, the terminal does not draw them, and the picker for other file
  types is not shipped.
- Pasting an image from the clipboard needs `wl-paste` (Wayland), `xclip`
  (X11) or `osascript` (macOS) and a local session; over SSH, attach a file
  by path instead.
- The phone web app cannot attach images yet. It speaks protocol 18, so it
  shows an image sent from elsewhere as its `[image: name, size]` line.
- A host older than protocol 19 (Aether 1.0.2 or before) takes no images: the
  client refuses the message there and nothing is sent.

## Orchestration and visibility

- `wait_for` keeps reported native background jobs attached, but occupies an
  engine slot. It does not preserve jobs across daemon shutdown/restart or
  other turn-ending actions such as `delegate(wait=true)` and `ask_user`.
- Delivery acknowledgment is not answer/compliance verification. Steering a
  running child across restarts and providers is only partly verified
  (roadmap).
- After a direct user message, `wait_for` requires visible assistant text in
  the active turn. If the daemon crashes after consuming that message but
  before a reply, the in-memory requirement is lost on restart; the runtime
  also cannot judge whether the text actually answers the question.
- If the TUI disconnects during a new agent's model patch, its initial
  messages stay held until `/model` is retried after reconnecting. A later
  queued choice can still appear in the picker as the latest choice even
  though no retry is in flight; the disconnect notice gives the retry step.
- The agent scheduling tool cannot create schedules that spawn a fresh agent;
  the CLI can (roadmap).
- `aether workspace diff` attributes commits in a shared workspace by the
  HEADs recorded at each turn's start and end, so another agent's commits in
  the same window are included; the reply flags such diffs `ambiguous`. There
  is no TUI or phone view of it yet (roadmap).
- A background PTY shell has been seen to ignore Ctrl-C on a heavily
  oversubscribed host (roadmap).

## Measurement and compatibility

- Native tools, effort support, model access, and protocol capabilities depend
  on installed CLIs/accounts. Discovery is not a guarantee that a provider
  will accept every request (roadmap).
- The published benchmark reports do not establish universal accuracy, cost,
  or latency parity. Evaluation on real repositories and under repeated
  compaction remains open (roadmap).
- Agent waits avoid model polling, but the daemon/UI still perform periodic
  housekeeping. Process measurements do not establish literal zero idle CPU
  or a no-timers guarantee (roadmap).
- Cost displays are reported/estimated API equivalents, not subscription bills.
  Unknown cost is not zero cost or a reliable dollar-budget guarantee.
- Retention is not a full disk quota. Native sessions, backups, and workspaces
  have separate lifecycles.

## Platforms

Aether supports Linux and macOS. Native Windows is not supported: the daemon
and its clients talk over Unix sockets, and the process runners rely on Unix
process groups. Continuous integration runs on Linux for every change; the
macOS job runs on request (see [CI](ci.md)), so macOS is checked less often.

Native CLI protocols, model capabilities and the stronger forms of native
isolation depend on the installed engine version and the platform; see
[engine behavior](engines.md).

## Voice

- Local voice is opt-in. Its automated tests use fake local processes, never a
  real microphone, speakers or downloaded model, so real audio hardware,
  latency and output-device routing are unverified on any given machine. See
  [limits, privacy and verification](voice.md#limits-privacy-and-verification).
