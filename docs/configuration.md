# Configuration

On first start Aether creates commented templates in the platform configuration
directory (`~/.config/aether/` on Linux unless `XDG_CONFIG_HOME` changes it;
`~/Library/Application Support/aether/` on macOS). `AETHER_CONFIG_DIR`
overrides that location. The [shipped templates](../crates/aether-model/src/config/templates.rs)
are the source for supported keys and defaults.

```toml
# config.toml — example choices, not a list of shipped defaults
[model]
default = "claude-code/opus"

[scheduler]
max_concurrent_turns = 8
max_concurrent_engine_turns = 4
run_budget_usd = 25.0

[retention]
completed_run_days = 10
```

- **Models and effort:** choose them per agent; switch with `/model` or
  `aether set`. A running turn finishes on its original model; the new choice
  applies when the agent starts its next turn. Fallback chains are explicit
  and validated against billing and execution constraints.
- **Concurrency and budgets:** control global/provider/subtree scheduling and
  per-agent limits. The run-dollar limit above blocks new delegation; it does
  not retroactively cancel running work, and unknown cost is not a hard cap.
- **Providers:** configure endpoints, credentials by reference, and provider
  concurrency in `providers.toml`.
- **Interface:** theme, motion, mouse, session defaults, pinned workspaces,
  and workspace discovery roots are stored separately in `ui.toml`; see the
  [TUI guide](tui.md) and
  [theme overrides](theme-overrides.md).
- **History and storage:** configure completed-run and artifact retention.
  Inspect eligible runs and protected reasons with `aether retention preview --json`.
  [Retention](log-retention.md) is not a total-disk quota or a backup.
- **Tools and integrations:** configure MCP servers, web search, webhook
  delivery, and optional voice. Native agents keep their native tool setup.

Daemon configuration changes generally require a restart. GitHub watcher
settings are stored in the daemon database and can be changed live.

## Common settings

These settings live in `config.toml`; dotted names identify the TOML
section and key. Omitted settings use the shipped defaults.

| Setting | Default | Purpose |
| --- | --- | --- |
| `model.default` | First model of the first ready provider | Choose the default `provider/model`. |
| `scheduler.max_concurrent_turns` | `32` | Bound simultaneous model turns. |
| `scheduler.max_concurrent_engine_turns` | Unset | Add a limit for native CLI turns. |
| `scheduler.run_budget_usd` | Unset | Refuse new delegation once recorded run spend reaches the limit. |
| `approval.default` | `"yolo"` | `"ask"` gates external effects; local mutations need an explicit agent policy. |
| `retention.completed_run_days` | `7` | Retain eligible completed runs this many days. |

`network.default` is `"allowed"`. Scope and native-engine restrictions still
apply; approval policy alone is not a sandbox. See the
[engine integration guide](engines.md) and [known limitations](known-limitations.md).

## Providers

One signed-in native CLI **or** one configured API provider is enough.
These are all the provider IDs in the [built-in registry](../crates/aether-model/src/builtin.rs).
Native engines keep their CLI's tools and subscription sign-in; hosted APIs
use separate API access. Grok and Gemini consumer subscriptions are not
supported sign-in routes: use an xAI or Google API key.
The native CLI login commands below are only needed if you have not already signed in to that CLI.

| Provider id | Kind | Sign-in or key variable | Checked for 1.0 |
| --- | --- | --- | --- |
| `claude-code` | Native Claude Code CLI | `aether login claude-code` (Claude subscription) | Unit and scripted-CLI tests; live paired runs on 2026-10-10/11 ([measurements](benchmarks.md)) |
| `codex` | Native Codex CLI | `aether login codex` (ChatGPT subscription) | Unit and scripted-CLI tests; live paired runs on 2026-10-10/11 ([measurements](benchmarks.md)) |
| `anthropic` | Hosted Messages API | `ANTHROPIC_API_KEY`; also `ANTHROPIC_AUTH_TOKEN` or imported Anthropic profile | Messages adapter: fake-server tests |
| `openai` | Hosted Chat Completions API | `OPENAI_API_KEY` | Chat adapter: fake-server tests |
| `openai-responses` | Hosted Responses API | `OPENAI_API_KEY` | Responses adapter: fake-server tests |
| `openai-codex` | Operator-configured Codex Responses dialect; disabled by default | No built-in login or key variable; supply endpoint and auth, then enable in `providers.toml` | Codex dialect: fake-server tests; disabled-default unit test |
| `xai` | Hosted xAI (Grok), Chat Completions API | `XAI_API_KEY` | Shared Chat adapter: fake-server tests |
| `google` | Hosted Google (Gemini), OpenAI-compatible API | `GEMINI_API_KEY` or `GOOGLE_API_KEY` | Shared Chat adapter: fake-server tests |
| `groq` | Hosted Groq, Chat Completions API | `GROQ_API_KEY` | Shared Chat adapter: fake-server tests |
| `openrouter` | Hosted OpenRouter, Chat Completions API | `OPENROUTER_API_KEY` or `aether login openrouter` (browser sign-in issues an API key) | Shared Chat adapter: fake-server tests; no live browser check |
| `ollama` | Local Ollama server | None by default | Shared Chat adapter: fake-server tests |
| `lmstudio` | Local LM Studio server | None by default | Shared Chat adapter: fake-server tests |
| `vllm` | Local vLLM server | None by default | Shared Chat adapter: fake-server tests |
| `llamacpp` | Local llama.cpp server | None by default | Shared Chat adapter: fake-server tests |

The native CLI measurements cover the named trials, not every model or account.
The [API conformance suite](../crates/aether-model/tests/conformance.rs)
replays recorded response fixtures through a fake HTTP server, shared by
providers using the same protocol. It does not verify each vendor's endpoint,
account access, or an installed local server. The API checks listed above
are automated coverage; live API service and account sign-in checks are not
recorded for 1.0.

`openai-codex` ships without an endpoint or OAuth login. The built-in ChatGPT
subscription route is `codex`, the official CLI. Further native CLIs can be
added by implementing the [engine trait](engines.md#the-trait) and
[registering the engine](engines.md#registering-one); they are not enabled
by adding an HTTP provider.

Keys named above must be available in the daemon's environment. Alternatively,
`aether login xai`, for example, prompts for and stores an API key. A stored
credential takes precedence over environment keys. Anthropic also supports
importing an `ant` profile; this is distinct from Claude Code's CLI login.
Local providers need a running server with a model loaded; configure auth
explicitly if your server requires it.

`providers.toml` can also add a model to a built-in provider: Claude's 1M
context window is never on by default, and
[Use the 1M window](engines.md#use-the-1m-window) shows the entry that adds it.

### Add an OpenAI-compatible endpoint

Add this to `providers.toml` in the configuration directory. It matches the
[shipped example](../crates/aether-model/src/config/templates.rs) and assumes
a local server serving `qwen3-coder` at `http://localhost:8080/v1` without
authentication. Replace the URL, model ID and capability limits to match
your server. Aether appends `/chat/completions` to the base URL.

```toml
[providers.local-llm]
name = "Local llama-server"
protocol = "openai-chat-completions"
base_url = "http://localhost:8080/v1"
auth = { none = true }

[[providers.local-llm.add_models]]
id = "qwen3-coder"
name = "Qwen3 Coder (local)"
# A price is what tells a subscription agent's fallback guard this model is
# free; omit it and it is refused as unknown, metered cost.
capabilities = { context_window = 65536, max_output_tokens = 8192, cost = { input_per_mtok = 0.0, output_per_mtok = 0.0 } }
```

The zero prices describe a free local server; do not copy them for a paid
endpoint. For an endpoint requiring a key, replace the `auth` line with
`auth = { api_key_env = ["MY_PROVIDER_API_KEY"] }` and make that variable
available to the daemon. Restart the daemon after editing; the model is
then selectable as `local-llm/qwen3-coder`. This example's configuration is
checked locally; inference still depends on your server and model.

## State and connections

`AETHER_DATA_DIR` overrides the platform data directory, which holds
`aether.db`, logs, and artifact payloads. `AETHER_SOCKET` selects a daemon
socket; `--socket` overrides it for a CLI invocation. The default is
`$XDG_RUNTIME_DIR/aether/aetherd.sock` where a runtime directory is available,
otherwise `aetherd.sock` in the data directory.

Optional `[listen]` settings expose the protocol over token-authenticated TCP.
There is no built-in TLS; use an SSH tunnel. `[serve]` settings configure the
separate webhook/event bridge. See the [protocol](protocol.md),
[MCP](mcp-servers.md), and [GitHub watcher](github-watcher.md) guides.

### Remote control

`[remote]` holds the fixed limits of [remote control](remote.md). Whether the
listener is enabled, its public origins and push are changed live with
`aether remote`, never here. Nothing listens until `aether remote enable`.

```toml
[remote]
address = "127.0.0.1:8377"
allow_remote = false
max_connections = 32
idle_timeout_secs = 300
push_previews = false
push_subject = ""
push_hosts = []
```

| Key | Default | Meaning |
| --- | --- | --- |
| `address` | `"127.0.0.1:8377"` | `host:port` the listener binds once enabled. |
| `allow_remote` | `false` | Required to bind outside loopback; the listener speaks plaintext HTTP. |
| `max_connections` | `32` | At least 1. HTTP connections served at once, and WebSockets open at once. |
| `idle_timeout_secs` | `300` | At least 3. A WebSocket silent for this long is closed. |
| `push_previews` | `false` | Let a push notification carry a short preview of what an agent wrote. |
| `push_subject` | `""` | VAPID contact, a `mailto:` or `https://` URL; empty derives it from the first public origin. |
| `push_hosts` | `[]` | Extra push service hosts a device may subscribe with: exact names or `*.suffix`. |

Unknown keys and values that cannot mean what they say are refused when the
configuration loads.
