# Desktop app

The desktop app (`web/apps/desktop/`) is a thin Electron window over the same
frontend the daemon serves to a browser: it loads the committed `web/dist/`,
byte for byte, from `app://aether`, and reaches local daemons only through
the validated `window.desktop` bridge in its main process. It has no screens
of its own. This page covers how to package it, sign it, install it and keep
it up to date.

## Contents

1. [Packages](#packages)
2. [Building the packages](#building-the-packages)
3. [Signing](#signing)
4. [What "unsigned build" means](#what-unsigned-build-means)
5. [Installing](#installing)
6. [Updates](#updates)
7. [CI](#ci)
8. [Chatting with an agent](#chatting-with-an-agent)

## Packages

| System | Files | Signed |
| --- | --- | --- |
| Linux (x86-64) | `Aether-<version>-linux-x86_64-unsigned.AppImage`, `Aether-<version>-linux-amd64-unsigned.deb` | never: electron-builder signs neither format |
| macOS (arm64, x64) | `Aether-<version>-mac-<arch>-<signing>.dmg` and `.zip` | when the maintainer's credentials are present |

`<signing>` is `unsigned`, `signed` or `signed-notarized`. Every file name
carries it, so a copied file still says how it was built. The packages carry
the workspace version from `Cargo.toml`, the version release tags name.

Windows is not packaged.

## Building the packages

From a checkout, with Node.js 22.18 or later:

```sh
(cd web && npm ci)
cd web/apps/desktop
npm ci
npm run package                    # this system's packages
npm run package -- --target linux  # AppImage and deb
npm run package -- --dir           # only the unpacked app, for a quick look
```

Always package through `npm run package` (`build/package.ts`), never by
calling electron-builder directly. The script sets the version, decides the
signing (below), records it in the packaged `package.json` as `aetherBuild`,
and names the files. electron-builder alone stops at the artifact name,
which needs that decision.

The packages land in `target/desktop/` at the repository root, beside an
update file (`latest-linux.yml` or `latest-mac.yml`) that lists each
package's SHA-512 checksum. macOS packages build only on a Mac.

To check a build, start it headless and let it report itself:

```sh
xvfb-run -a node build/smoke.ts --exec ../../../target/desktop/linux-unpacked/aether-desktop \
  --resources ../../../target/desktop/linux-unpacked/resources --signing unsigned
```

It needs a display. CI uses `xvfb-run`. Without Xvfb, a headless Wayland
compositor works too: start `WLR_BACKENDS=headless sway -c /dev/null` with
its own `XDG_RUNTIME_DIR`, and point `WAYLAND_DISPLAY` at its socket by
absolute path (`$runtime/wayland-1`). Chromium's own
`--ozone-platform=headless` crashes Electron, so it is not an option.

The smoke test checks that the package holds exactly the committed
`web/dist/` and protocol schemas, byte for byte. It then starts the app with
`AETHER_DESKTOP_SMOKE=1`, in a scratch home, with discovery pointed at
sockets that do not exist, so it never reaches a running daemon. Once the
window has loaded `app://aether/index.html`, the app prints the page title,
whether `window.desktop` is there, and the build's signing label, then exits.
The test passes when all of these match and the app exits with status 0.

## Signing

The build signs only when electron-builder's standard credentials are in the
environment. They come from CI secrets, never from a file in the repository:

| Variable | What it is |
| --- | --- |
| `CSC_LINK`, `CSC_KEY_PASSWORD` | the Developer ID Application certificate (a base64 `.p12` or a path) and its password |
| `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`, `APPLE_TEAM_ID` | notarization with an Apple ID |
| `APPLE_API_KEY`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER` | or notarization with an App Store Connect key |

The rules (`build/signing.ts`):

- With no certificate, the build is unsigned. It is never signed with
  whatever identity the machine's keychain holds: signing is switched off
  outright (`mac.identity: null`, `CSC_IDENTITY_AUTO_DISCOVERY=false`).
- With a certificate, the build must sign: electron-builder runs with
  `forceCodeSigning`, so a signing failure fails the build instead of
  producing an unsigned package under a signed name.
- A notarization set is used only with a certificate, and only one set at a
  time.
- A set given only in part (a certificate without its password, say) is an
  error, not a quiet fallback to unsigned. An empty variable counts as unset.
- Linux packages are always unsigned.

The script prints only its decision ("unsigned", "signed, not notarized"…),
never a credential.

## What "unsigned build" means

An unsigned build carries no publisher signature. The system cannot tell
who made it, and it cannot tell a modified copy from the original. It is the
same code, built the same way: unsigned changes who vouches for the files,
not what they do.

- **macOS** quarantines an unsigned app downloaded from the web, and
  Gatekeeper refuses to open it. To run it, open it once from Finder with
  right-click → Open, or clear the quarantine flag with
  `xattr -dr com.apple.quarantine /Applications/Aether.app`. An unsigned Mac
  build cannot update itself (Squirrel.Mac checks signatures), so it never
  offers updates: install the new version by hand.
- **Linux** has no system-wide signature check for AppImage or deb files.
  Check the SHA-256 checksum in the release's `SHA256SUMS.desktop` before
  you install.

The app says when it is unsigned:

- the macOS About panel shows "unsigned build" beside the version;
- `window.desktop.diagnostics()` reports it as `versions.build`;
- every update prompt says so before you confirm anything.

## Installing

- **deb** (Debian, Ubuntu): `sudo apt install ./Aether-<version>-linux-amd64-unsigned.deb`.
  The app goes to `/opt/Aether/`, with `aether-desktop` on the `PATH` (not
  `aether`, which stays the daemon's command), and is listed in the
  application menu. Remove it with `sudo apt remove aether-desktop`.
- **AppImage**: `chmod +x Aether-*.AppImage`, then run it. It needs FUSE
  (`libfuse2`). Without it, run it with `--appimage-extract-and-run`.
- **macOS**: open the dmg and drag Aether to Applications.

Install the daemons as [install.md](install.md) describes. The app starts a
daemon only when managed start is on, and it never stops one on its own.

### Setting up this computer

The app's setup runs in three steps, each one when you click it:

1. **Find the programs.** It runs `aether version` and `archon --version`.
   Apps opened from a dock or menu often get a shorter `PATH` than a
   terminal, so the app adds `~/.local/bin`, `~/.cargo/bin`,
   `/usr/local/bin` and, on macOS, `/opt/homebrew/bin` after your own `PATH`
   when they exist. Managed start uses the same `PATH`.
2. **Connect Archon's tools.** The app first asks in a native dialog. If
   Aether is already running, the same dialog says Aether may restart. If
   you cancel, nothing is written and nothing is run. If you agree and
   Archon has no config file yet (`$ARCHON_CONFIG_DIR/config.toml`, else on
   Linux `$XDG_CONFIG_HOME/archon/config.toml` or
   `~/.config/archon/config.toml`, on macOS
   `~/Library/Application Support/archon/config.toml`), the app writes one
   with `[aether] mode = "shared"`, so Archon uses this computer's Aether.
   It never changes a config file that exists. Then it runs
   `archon setup --json`, which registers Archon's tools with Aether. If
   Aether started on its own while you answered and has not loaded the
   tools, setup stops there; trying again asks with the restart named.
3. **Start.** Clicking "Set up" turns managed start on. Aether reads its
   tools only when it starts. If Aether was running without Archon's tools
   and you agreed to the restart in step 2, the app runs `aether stop`,
   waits until Aether has stopped, and starts it again; working agents
   pause and pick up where they left off. Without that agreement nothing is
   stopped. Then it starts Aether and Archon, leaving alone any that
   already run.

## Updates

A packaged app checks for an update once each time it starts. It never
downloads or installs one by itself:

1. It asks the GitHub releases of `mengsig/aether` for a newer published
   release, and only that feed. The address is fixed in the app
   (`src/update/updates.ts`) and in `electron-builder.yml`. Drafts and
   prereleases are ignored, and so is anything older than the running
   version.
2. If there is one, a native dialog names the new version, your current
   version and whether the build is signed. Download it, or cancel.
3. Once it has downloaded and its SHA-512 checksum matches the release's
   update file, a second dialog asks whether to install and restart. If you
   cancel, nothing installs, now or when you quit.

electron-updater runs with `autoDownload` and `autoInstallOnAppQuit` off.
The flow refuses to run if either is on. The version from the feed is checked
before any dialog shows it. A deb update asks for your password (through
`pkexec` or `sudo`) to install. An AppImage replaces itself in place.

A check that fails (offline, or a release without desktop packages) is
logged and changes nothing. A download that fails after you confirmed it is
shown in a dialog.

## CI

`.github/workflows/desktop.yml`:

- **linux** (every pull request into `main` or `next`, and pushes to them,
  that touch `web/`, the protocol schema or `Cargo.toml`; and every release
  tag): runs the packaging tests, builds the AppImage and deb, installs the
  deb with `apt`, and smoke-tests the installed app and the extracted
  AppImage under `xvfb`. The packages are uploaded as the `desktop-linux`
  workflow artifact.
- **macos** (opt-in, like `ci-macos`): runs when a pull request carries the
  `ci:macos` label, when a manual run ticks *macos*, or on a release tag when
  the repository variable `DESKTOP_MACOS` is `true`. It builds the dmg and
  zip, signed when the secrets above are set.
- **publish** (release tags only): waits for the draft release that
  `release.yml` opens for the tag, then attaches the packages, the update
  files and a `SHA256SUMS.desktop`. Apps see the update once the owner
  publishes the draft.

The workflow's token is read-only except in the publish job, which may write
release assets. The signing secrets reach only the macOS build step, and only
on a push to `main`, `next` or a release tag. A pull request, whoever opened
it, and a manual run, which can name any branch, build unsigned.

## Chatting with an agent

**Start an agent** on the Activity screen starts a **Chat** by default: a
task-less assistant, started the way the TUI starts one, that gets your first
message and then waits for the next one after every reply. It stays open until
you cancel it. A first message that is empty or too long to keep (over 200,000
characters) is refused before any chat is created. The chat opens once its first
message is delivered, sent as typed: a `/` at its start is never read as a
command. If that message is refused, or its reply never comes, the sheet stays
open with the chat's settings shown fixed, the message as typed, the reason and,
for a lost reply, a warning that it may have arrived. **Send it again** resends
the same text to the same chat, so a retry never starts a second one. **Open the
chat** goes there, and the chat shows the same notice with **Send it as typed**
until the message is delivered or dismissed. **New chat with different
settings** is the only way to create another, and the form keeps what you typed.
If the reply to the create itself never comes, the chat may exist, so the sheet
says so and points to the recovery review instead of creating it again. A **Task
agent** instead works on an objective and ends once it is done; an ended agent
takes no more messages, and its composer says why and offers to wake it (after a
failure), start a new chat or open its controls.

The composer sits under the conversation. Enter sends, Shift+Enter adds a
line, and **Stop** interrupts the running turn without losing the agent's
context. What you type is kept per daemon, agent and field (the composer, the
Steer form and each Inbox question's answer) while the app is open, and saved
for a reload where the session store allows. A draft is cleared once the daemon
confirms delivery, and only if it still reads exactly what was sent, so text
typed during a send stays. Where the session store is full or disabled, one note
says drafts will not survive a reload, and a started chat's undelivered first
message stays in the sheet instead of following you to the chat.

Typing `/` opens the command menu. Arrow keys move, Tab completes a name and
then offers its choices, Enter runs, Escape closes; the pointer works too.

| Command | Opens or does |
| --- | --- |
| `/settings [page]` | the settings, or one page: connections, devices, remote, providers, notifications |
| `/usage` | providers, sign-in and limits (Settings, Providers) |
| `/notifications` | the Inbox: questions, approvals and failures |
| `/memory` | this daemon's memories |
| `/report`, `/artifacts`, `/schedules`, `/approvals` | the agent's controls (Records, Schedules, Approvals) |
| `/doctor` | Settings, Connections |
| `/activity` | every daemon event as it happens |
| `/help` | the command palette (Ctrl+K): every screen and its keys |
| `/mode <name>` | a transcript view mode; `/<name>` toggles that mode with the standard one |

To Archon's orchestrator the menu also offers Archon's own commands
(`/status`, `/queue`, `/answer`, `/approve`, `/merge` and the rest), which go
to it as typed. Any other `/line` is sent to the agent unchanged, as the TUI
sends it, except while the view mode list is missing (below).

### View modes

A view mode changes what the conversation draws, never what the agent does. The
picker above the transcript offers `standard`, the built-in `focus` and the
modes in the daemon's `<config dir>/modes/*.toml` whose scope fits the agent,
read over `list_view_modes` (see [the TUI's view modes](tui.md#view-modes)); a
mode scoped to another agent is not offered. Arrow keys move the choice, and
`/mode <name>` or `/<name>` (again for standard) choose one from the composer
and run in the app. While the daemon's list is loading or unavailable, `/mode`,
`/focus` and Archon's `/calm` say so rather than reach the agent. Any other
`/<name>` no command knows may name a user mode, so it is held while the list
is loading, its last read failed (the last list stays in use) or a mode file
could not be read, which the bar names; once the list is whole and names no
such mode, it is sent byte for byte. The choice is remembered per daemon and
agent, read once as the chat opens. Where storage refuses to save it, the choice
holds for the open chat and the bar says it will not be remembered after a
reload; where the saved choice cannot be read, the chat opens in its default view
and the bar says so. Without one, Archon's orchestrator opens in its Calm mode
whenever a `calm.toml` that fits it exists, whatever other modes are scoped to
it; any other agent opens in the first user mode scoped to it, else in standard.

The rules are the TUI's, pinned row for row by one shared fixture
(`crates/aether-tui/tests/fixtures/view_mode_parity.json`) that both test
suites read. A named mode hides the categories it does not list, draws a
failed tool call or edit as one line when it omits `tool`, and with `compact` density
draws delegations, messages, completions and status as one-line cards; drawn
whole, a card keeps the call beside it, input and result.
What the operator typed, questions, approvals, errors, failed children and
calls that were denied or cancelled stay whole in every mode. **Details** (Ctrl+T)
reveals everything the mode hides without leaving it. Switching keeps the draft
and the row at the top of the view.

A user turn is drawn as the daemon wrote it. An inbox turn, which may hold
several deliveries and quote headers in their bodies, is never split or parsed:
it is drawn verbatim as the operator's text, whole in every mode. A delegated
agent's first turn is its task only when it is the `Your task:` turn the daemon
writes for a new transcript; any other first turn is drawn as the operator's.

The agent's own delegations, messages to other agents, waits and questions, and
a delegated agent's task, are cards that open the other agent's chat and, for
delegations, the fleet graph. A line beside the picker says what the agent is
working on or waiting for, and its open questions and approvals are pinned above
the transcript until they are answered.
