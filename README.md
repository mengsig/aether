# Aether

Aether is a local daemon for running AI agents, coordinating their tasks, and supervising their work from a terminal or another application.

![Illustrated Aether overview: event sources feed the local daemon, surrounded by models, persistent state and tools; blue mascot agents show solo work, mixed providers, delegation trees and event-driven automation](docs/assets/aether_graphic.svg)

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | sh
```

Aether drives an engine CLI for the quick start; install [one of the engines below](#prerequisites).
Update with `aether update`.
For source builds and version pinning, see [other installation methods](docs/install.md).

To build from source, install Git, a C compiler and Rust via rustup; the repository pins its Rust toolchain.

```sh
git clone https://github.com/mengsig/aether
cd aether
make install   # or: cargo install --path crates/aether --locked
```
The binary goes into `~/.cargo/bin` by default; update a source build by pulling and installing again, not with `aether update`.

## What you can do

- Run Claude Code and Codex with their native tools and subscription sign-in,
  or use Aether's own loop with hosted APIs (Anthropic, OpenAI, xAI/Grok,
  Google/Gemini, Groq, OpenRouter), local servers (Ollama, LM Studio, vLLM,
  llama.cpp), or any OpenAI-compatible endpoint. Grok and Gemini need API keys,
  not consumer subscriptions; see [providers and checks](docs/configuration.md#providers).
- Delegate tracked tasks to agents with explicit models and effort levels;
  choose separate Git worktrees for concurrent edits.
- Follow agents in the terminal UI, send messages with screenshots attached,
  pause work, and inspect tool calls, results, token usage, and estimated costs.
- Keep task history, reusable memory, and artifacts across client sessions.
- Wake agents through messages, timers, recurring schedules, signed webhooks,
  or authorized GitHub requests.
- Drive the daemon through Python, TypeScript, Rust, or MCP clients, including
  orchestration applications such as [Archon](https://github.com/mengsig/archon).

## Prerequisites

Use **Linux or macOS**; native Windows is not supported. For the quick start,
install **one** engine CLI and sign in: Claude Code runs Claude models; Codex
runs OpenAI models. API-only use needs a configured provider instead of either
CLI. Model access and billing depend on your provider account.

| Program | Used for | Needed? | Install / setup |
| --- | --- | --- | --- |
| [Claude Code](https://code.claude.com/docs/en/setup) (`claude`) | Native Claude agents | One engine | `curl -fsSL https://claude.ai/install.sh \| bash` |
| [Codex](https://developers.openai.com/codex/cli) (`codex`) | Native OpenAI agents | One engine | `npm install -g @openai/codex` (needs Node/npm) |
| [Git](https://git-scm.com/install/) | Worktrees, repository memory, GitHub checkouts | For repository features | Linux: `sudo apt-get install git`; macOS: `brew install git` |
| [GitHub CLI](https://cli.github.com/) (`gh`) | Opt-in GitHub watcher | Optional; local tasks work without it | `brew install gh`; then `gh auth login` |
| [Node.js/npm](https://nodejs.org/en/download) | npm install of Codex; TypeScript client; configured JS MCP servers | Optional; Claude/native binaries work without it | Follow the OS installer |
| [Bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) and [socat](http://www.dest-unreach.org/socat/) | Linux native network-denied turns | Optional; ordinary network-enabled turns work | Distribution packages; see upstream |
| [Docker](https://docs.docker.com/engine/install/) | Container backend for Aether shell tools | Optional; host execution works | Follow the OS installer; not for native engines |
| [Rust](https://rust-lang.org/tools/install/) | Build Aether from source | Optional; binary install works | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |

Voice, clipboard and other optional helpers are listed in [docs/prerequisites.md](docs/prerequisites.md).

Install everything needed for the Claude quick start below (Debian/Ubuntu Linux,
or macOS with [Homebrew](https://brew.sh/) already installed):

```sh
case "$(uname -s)" in
  Linux) sudo apt-get update && sudo apt-get install -y git curl bash ;;
  Darwin) brew install git ;;
esac
curl -fsSL https://claude.ai/install.sh | bash
```

After installation, `aether doctor` checks configuration, daemon, storage, disk, providers,
engines, configured MCP servers, and remote access; it is not a check of every optional program.

## Quick start

Open a terminal in a project you trust.
Sign-in belongs to the engine CLI: if you already signed in to Claude Code or
Codex, skip `aether login`; otherwise, `aether login claude-code` (or
`aether login codex`) starts that CLI's sign-in.

```sh
aether
```

Aether opens the terminal UI in your current project and starts its daemon on demand; type a request to begin.
Use `/model` to choose a model, `?` with an empty draft for shortcuts, and `/quit` to leave while agents keep running.
On a first start with neither engine signed in and no other provider configured,
you see “no model to run on yet”; use `/login claude-code` or `/login codex`, then send a request.

For scripts, use `aether run "Explain what this project does. Do not edit files." --model claude-code/default --effort medium` (for Codex, use `--model codex/default`).
`default` lets that engine CLI select its model; the run prints the agent's response.
Use `aether doctor` when something does not work, and `aether auth` to see each provider's sign-in status (`ready` or `not set`).

## Use it from your phone

```sh
aether remote setup
```

A guided setup puts Aether on your phone's home screen through a private
Tailscale network, with no ports opened to the internet and no files to edit.
See [Use Aether from your phone](docs/remote-setup.md); it works for someone who
has never heard of Tailscale and can be re-run at any time.

## Core ideas

| Term | Meaning |
| --- | --- |
| Agent | A worker with a model, workspace, conversation, and permissions. |
| Task | A tracked objective with an owner, status, and result. |
| Run | A root agent and its delegated descendants, grouped for reports. |
| Inbox | Messages that can wake an agent or steer its current turn. |
| Schedule | A durable recurring trigger that delivers work or starts a fresh run. |
| Memory | Scoped facts, decisions, and preferences retained for later work. |
| Artifact | A recorded output, with a summary and optional file or stored payload. |
| Budget | A limit on work or usage; unknown cost cannot guarantee a dollar cap. |

## Everyday commands

Replace `AGENT_ID` with an ID from `aether agents`.

| Command | Purpose |
| --- | --- |
| `aether status` | Check the daemon and current work. |
| `aether models` | List models and provider readiness. |
| `aether agents` / `aether tasks` | List workers and tracked objectives. |
| `aether send AGENT_ID "What is blocking you?"` | Talk to an agent. |
| `aether send AGENT_ID --image shot.png "What is wrong here?"` | Send a screenshot with the message. |
| `aether pause AGENT_ID` / `aether resume AGENT_ID` | Pause or resume scheduling. |
| `aether interrupt AGENT_ID` | Stop the current turn, keeping the conversation. |
| `aether cancel AGENT_ID` | Cancel an agent and its descendants. |
| `aether runs list` | Find past runs for inspection. |
| `aether schedule list` | List recurring triggers. |
| `aether github-watcher status` | Check the opt-in GitHub watcher. |
| `aether stop` | Stop the daemon and interrupt attached work. |

Use `aether --help` for all commands. The [user guide](docs/guide.md) covers
approvals, reports, recovery, delegation, and event-driven work.

## Configuration

First start writes templates in `~/.config/aether/` on Linux (respecting
`XDG_CONFIG_HOME`) or `~/Library/Application Support/aether/` on macOS.
`AETHER_CONFIG_DIR` overrides that directory. Common `config.toml` settings:

| Setting | What it changes |
| --- | --- |
| `model.default` | Default provider/model for new agents. |
| `scheduler.max_concurrent_turns` | Simultaneous model turns (default 32). |
| `scheduler.max_concurrent_engine_turns` | Optional additional limit for native CLI turns. |
| `scheduler.run_budget_usd` | Refuse new delegation after the run's recorded spend reaches this limit. |
| `approval.default` | `"yolo"` by default; `"ask"` gates external side effects. |

Claude's 1M context window is never on by default; [context windows](docs/engines.md#claude-context-windows) shows the `providers.toml` entry that adds it.

Endpoints belong in `providers.toml`; terminal preferences live in `ui.toml`.
See the [configuration reference](docs/configuration.md) for defaults, paths,
retention, and restart requirements.

## Safety model

By default, agents may run commands, edit files, and access the network without approval.
`--no-yolo` gates external effects, but local writes need an explicit approval policy.
Aether checks tool and coordination permissions; native shell/file tools have separate enforcement.
Worktrees and approval prompts are not a security sandbox. Read the [security policy](SECURITY.md)
and [known limitations](docs/known-limitations.md) before running untrusted work.

## Documentation and status

- [User guide](docs/guide.md) · [Terminal UI](docs/tui.md) · [Engine behavior](docs/engines.md)
- [GitHub integration](docs/github-watcher.md) · [MCP](docs/mcp-servers.md) · [Protocol](docs/protocol.md) · [Remote control](docs/remote.md) · [Phone setup](docs/remote-setup.md)
- [Python client](clients/python/README.md) · [TypeScript client](clients/typescript/README.md)
- [Benchmarks and their limits](docs/benchmarks.md) · [All documentation](docs/README.md)

**1.0.0 release scope:** the stable core is local agent execution, task
coordination, persisted history, and terminal supervision. It is not a promise
of unchanged third-party CLI protocols, model behavior, or universal sandboxing.
Local voice and advanced isolation are outside that stable core; their guides
describe the testing and platform limits. Use matching daemon and client versions.

Contributing: [contributor guide](CONTRIBUTING.md). Licensed under the [MIT license](LICENSE).
