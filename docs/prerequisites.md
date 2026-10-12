# Prerequisite details

The [README](../README.md#prerequisites) lists the everyday programs; the table
below lists every program used by Aether.
Only install the optional tools for features you intend to use. The daemon,
CLI, and TUI are Rust binaries; ordinary use does not need Python, Node.js,
a separate database server, or a Rust compiler. SQLite is bundled.

## All programs

| Program | Used for | Needed? | Install / setup |
| --- | --- | --- | --- |
| [Claude Code](https://code.claude.com/docs/en/setup) (`claude`) | Native Claude agents | One engine | `curl -fsSL https://claude.ai/install.sh \| bash` |
| [Codex](https://developers.openai.com/codex/cli) (`codex`) | Native OpenAI agents | One engine | `npm install -g @openai/codex` (needs Node/npm) |
| [Git](https://git-scm.com/install/) | Worktrees, repository memory, GitHub checkouts | For repository features | Linux: `sudo apt-get install git`; macOS: `brew install git` |
| [GitHub CLI](https://cli.github.com/) (`gh`) | Opt-in GitHub watcher | Optional; local tasks work without it | `brew install gh`; then `gh auth login` |
| [Node.js/npm](https://nodejs.org/en/download) | npm install of Codex; TypeScript client; configured JS MCP servers | Optional; Claude/native binaries work without it | Follow the OS installer |
| [Bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) | Linux native network-denied turns | Optional; ordinary network-enabled turns work | Distribution package; see upstream |
| [socat](http://www.dest-unreach.org/socat/) | Linux native network-denied turns | Optional; same as Bubblewrap | Distribution package; see upstream |
| [Docker](https://docs.docker.com/engine/install/) | Container backend for Aether shell tools | Optional; host execution works | Follow the OS installer; not for native engines |
| [Rust](https://rust-lang.org/tools/install/) | Build Aether from source | Optional; binary install works | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| [Bash](http://www.gnu.org/software/bash/) | Built-in shell tool | For shell tasks | Included on macOS and most Linux systems |
| [POSIX shell](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/sh.html) (`sh`) | Installer and sandbox helper | Required | Included with the OS |
| [curl](https://curl.se/download.html) | Download installers | For installation | Linux: `sudo apt install curl`; included on macOS |
| [FFmpeg](https://ffmpeg.org/download.html) | Voice recording and playback | Optional; text UI works | Follow the OS packages link |
| [Python](https://www.python.org/downloads/) (`python3`) | Parakeet/Piper setup and voice adapters; Python client | Optional; normal CLI/TUI works | Python 3.11+ with venv/pip for guided voice setup |
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (`whisper-cli`) | Local speech recognition | Optional; text input works | Aether's `/voice` setup; [manual steps](voice.md) |
| [Piper](https://github.com/OHF-Voice/piper1-gpl) (`piper`) | Spoken agent questions | Optional; text replies work | Aether's `/voice` setup; [manual steps](voice.md) |
| [CMake](https://cmake.org/download/) | Build whisper.cpp during setup | Optional; other voice paths work | Follow the OS installer |
| [C++ compiler](https://clang.llvm.org/get_started.html) | Build whisper.cpp | Optional; other voice paths work | Install your OS C/C++ toolchain |
| [Make](http://www.gnu.org/software/make/) (`make`) | Build whisper.cpp | Optional; Ninja can replace it | Install with your OS build tools |
| [Ninja](https://ninja-build.org/) (`ninja`) | Alternative whisper.cpp builder | Optional; Make can replace it | Follow upstream installation |
| [wl-clipboard](https://github.com/bugaevc/wl-clipboard) (`wl-copy`) | Wayland clipboard | Optional; terminal copy remains | Distribution package; see upstream |
| [xclip](https://github.com/astrand/xclip) | X11 clipboard | Optional; terminal copy remains | Distribution package; see upstream |
| [xsel](https://vergenet.net/~conrad/software/xsel/) | X11 clipboard fallback | Optional; xclip or terminal copy remains | Distribution package; see upstream |
| [pbcopy](https://support.apple.com/guide/terminal/welcome/mac) | macOS clipboard | Optional; terminal copy remains | Included with macOS |
| `ant` (official installer unverified) | Refresh imported Anthropic profiles | Optional; normal provider login works | See [configured extensions](#configured-extensions-and-source-builds) |
| [Desktop URL opener](https://www.freedesktop.org/wiki/Software/xdg-utils/) | Open API-provider OAuth login URLs | Optional; open the printed URL yourself | OS desktop integration; `open` on macOS |
| [cat](http://www.gnu.org/software/coreutils/manual/html_node/cat-invocation.html) | Sandbox helper stdin | For Linux Codex network denial | Included with the OS |
| [kill](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/kill.html) | Sandbox helper cleanup | For Linux Codex network denial | Shell built-in |

## Engines and login

One authenticated native engine is enough. Claude Code and Codex retain their
own model loops, workspace tools, and login mechanisms. Aether discovers them
on `PATH`; updating a CLI can change available models and capabilities.
If you already signed in to Claude Code or Codex, skip `aether login` for that engine.

- [Claude Code installation](https://code.claude.com/docs/en/setup) provides
  the native installer used in the README. Its native installation does not
  require Node.js.
- [Codex installation](https://developers.openai.com/codex/cli) provides npm
  and Homebrew options. A downloaded native Codex binary does not require
  Node.js either; Node/npm is needed for the README's npm installation route.
- [Git installation](https://git-scm.com/install/) covers Linux and macOS.
  Without Git, non-repository conversations still work, but isolated delegation,
  managed GitHub checkouts, and Git-based repository identity need it.
- [GitHub CLI installation](https://cli.github.com/) links OS-specific setup.
  The GitHub watcher starts `gh api` and needs a logged-in account with access
  to the selected repositories. It also uses Git for managed checkouts.
  The watcher is disabled until configured; local work needs no GitHub login.

API and local endpoint providers do not need either native engine. Configure
credentials and endpoints in [providers.toml](configuration.md). Aether speaks
HTTP to local model services; it does not install or start Ollama, vLLM,
LM Studio, or llama.cpp for you.

`aether doctor` checks configuration, daemon/protocol connectivity, SQLite,
free disk space, logs, credential directory permissions, provider credentials,
engine detection, and configured
MCP server health. A missing unused engine can produce a warning even when
your chosen engine works. Exit codes are 0 (healthy), 1 (warning), and 2 (broken).
It does not certify provider account access or test every optional program.

## Shell and isolation

Aether's built-in shell tool starts `bash`. Native engines supply their own
shell/file tools. Their workspace dependencies are yours to install.

On Linux, network-denied native turns need `bwrap` and `socat` on `PATH`;
ordinary network-enabled turns do not. Claude Code needs version 2.1.219 or
later for its network-denied path; Codex needs 0.159.0 or later for its loopback
sandbox. These are feature-specific version floors, not universal minimums
for every Aether turn. See [engine behavior](engines.md).

The Codex loopback helper additionally uses `sh`, `cat`, and the shell's `kill`.
These are normal OS facilities, not separate model runtimes. The optional
container backend starts Docker and requires a running Docker service; it
covers Aether shell tools, not native Claude Code/Codex engines. Host execution
continues to work without Docker. See [known limitations](known-limitations.md).

## Voice and clipboard

Text input and text replies work without all voice dependencies. Voice is off
by default. The TUI's `/voice` setup checks prerequisites before installing:

| Feature | Programs and packages |
| --- | --- |
| Audio capture/playback | FFmpeg with the required OS audio devices. |
| Whisper recognition | Git, CMake, a C++ compiler (`c++`, `g++`, or `clang++`), Make or Ninja; setup builds `whisper-cli` and downloads a model. |
| Parakeet recognition | Python 3.11+ with venv/pip; setup installs pinned sherpa-onnx and NumPy wheels plus model files. |
| Piper speech | Python with venv/pip; setup installs pinned Piper wheels and a voice, then runs `piper`. |

Use the [voice guide](voice.md) for pinned versions, installation, platform
restrictions, audio-device requirements, model sizes, and licensing. Generic
upstream package installation alone does not configure Aether's model paths.

Clipboard helpers are optional: `pbcopy` on macOS, `wl-copy` on Wayland, and
`xclip` with `xsel` as a fallback on X11. The TUI also sends OSC 52 clipboard
sequences; whether those work depends on the terminal. Over SSH it skips local
clipboard helpers. Terminal selection remains available without these programs.

## Configured extensions and source builds

A configured stdio MCP server or credential helper starts the command you
specify; that command may need Python, Node, or another runtime. HTTP MCP
servers do not need a local server process. See [MCP configuration](mcp-servers.md).
There is no fixed install command that covers arbitrary configured extensions.

API-provider OAuth login can ask the OS to open a browser, and also prints the
URL. Desktop opener programs depend on the platform; a local browser is optional
when you can open that URL elsewhere. Native CLI logins handle their own browser
setup.

An optional Anthropic profile import can call `ant --version` and
`ant auth print-credentials --access-token`. No current official installer was
verified for that executable. It is not a prerequisite for normal Claude Code
or API-key login. Without it, a stored profile can be read but not refreshed.

The Python and TypeScript clients require their language runtimes only in the
applications using them. Building Aether from source needs the toolchain pinned
in [rust-toolchain.toml](../rust-toolchain.toml); see [installation](install.md).

## Installation source limits

The table above reproduces verified upstream one-line commands for Claude Code,
Codex, Git, GitHub CLI, curl, and Rust. Many optional projects publish OS-specific
packages or multi-step builds instead of a universal install command; use their
linked upstream instructions. No system packages or provider logins are
installed automatically by Aether's normal startup.
