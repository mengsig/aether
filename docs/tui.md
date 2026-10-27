# The terminal UI

`aether` (or `aether tui`) attaches the terminal UI to the daemon, starting the
daemon if it is not running. The UI is a conversation with one agent at a time;
the rest of the fleet lives on a board you open when you need it.

```sh
aether                        # attach; views or starts a root agent here
aether tui --agent agt_…      # attach viewing that agent
aether --theme light --ascii  # one-run display overrides
aether --plain                # lines instead of a screen
```

## Layout

```
transcript of the viewed agent                     │ board (Ctrl+G)
…                                                  │ Agents  3 · 1 running · $0.51
                                                   │ ● aether  orchestrator
◆ tester asks: …  ·  ctrl+g to open                │ └ ◆ tester  verifier
↪ run the tests too  queued for after this turn    │
• Working (1m 12s • esc to interrupt)              │ inspector of the agent
› Steer aether                                     │ under the cursor
opus-5.5 high · ~/aether · aether                  │
? shortcuts · ctrl+g agents   $0.51 · 82% until compaction
```

From the top:

- **Transcript.** What the *viewed* agent said and did: your messages (`›`),
  its replies and tool calls (`•`), and their children (`└`). Triggers and
  schedule deliveries show where they arrived (`• Trigger from ci · {…}`, one
  row; Ctrl+T shows the sanitized preview, capped at 4 KiB with an explicit
  `[truncated]` marker). A schedule delivery names its schedule
  (`• Trigger from schedule nightly · …`) once the TUI has listed it; Ctrl+T
  adds the id, and an unlisted schedule shows its id. Missing names are looked
  up once per schedule per session; `/schedules` → `r` retries explicitly.
  A command an engine ran through `zsh -lc '…'` shows as the command; Ctrl+T
  shows it as run. It follows the latest output until you scroll up.
- **Needs-you strip.** One row per agent that is waiting on you (a question,
  or an approval you deferred or opened), at most two, then `+N more`.
- **Queued and steering previews.** Messages waiting to reach the agent.
- **Agents strip.** One line while the viewed agent has live descendants.
- **Status row.** Only while the viewed agent is busy: what it is doing, how
  long for (counted like the `Worked for` rule that closes the turn, less any
  wait on your approval), and that Esc interrupts it.
- **Composer**, or whatever view has replaced it (an approval, a picker, a
  list). Your draft survives every view.
- **Status line.** Model and effort, the viewed agent's workspace (or the
  active one when none is viewed), and the viewed agent
  as a breadcrumb (`child ‹ parent`). Items drop right to left as the
  terminal narrows.
- **Hint row.** Key hints on the left (hidden while you type); run cost and
  the context left before compaction on the right (measured against where the
  agent compacts, 256K for Claude, 272K for Codex, else three quarters of the
  window; `[models.compaction_tokens]` overrides it). Short confirmations such
  as `effort high` replace the hints for a moment. When the row is tight, `82% until compaction`
  shortens to `82%`, then cost and context drop whole; a figure is never cut.
  On a focused board the Enter, Esc and lifecycle keys come first, so at the
  100-column dock only `82%` fits (nothing with `--ascii`); the inspector's
  Context row always shows it. With a quota alert, the action keys stay visible
  and the alert shortens to `quota` when its full text does not fit. The
  [notification](#notification-center) count sits at the far right.

## Workspaces

A workspace is the directory a root agent and its tree work in. The *active*
workspace is the viewed agent's root's, else the one last picked, else the
directory you launched from; `/new` and the root started for your first
message use it. An agent's own workspace never changes.

On launch, Aether views the most recently updated live root already working
in the launch directory. If there is none it starts one there, leaving live
roots in other projects on the board. `--agent` wins and starts nothing; it
chooses only that first view.

**Ctrl+O** or `/workspace` opens the picker: pinned workspaces, workspaces
with live agents (most recent first, with branch and agent counts), recent
workspaces, the launch directory, then git projects one or two levels under
your home, or under the roots set below. Type to filter; type a path (`/`,
`~`, `./`, `../`) to list that directory and its matching sub-directories, and
**Tab** to complete one. **Enter** views the latest live root there, or starts
a new root there. `/workspace <path>` (also `/cd`) does the same without the
picker. Recent workspaces are kept in `ui.toml`.

**Ctrl+P** in the picker pins the highlighted workspace, or unpins it. Pinned
rows come first, in the order you pinned them, marked `pinned`, and are saved
at once as `pinned_workspaces` in `ui.toml`. A pinned directory that no longer
exists is not shown, but stays in the file until you remove it.

Where the picker looks for git projects is set by hand in `ui.toml`:

```toml
workspace_roots = ["~/work", "~/src"]  # absent: your home; [] looks nowhere
workspace_depth = 3                    # levels below each root, 1-4; absent: 2
```

A root that is itself a git project is listed as one; your home never is,
however it is reached, but the search goes on below it. A root you list is
searched even if it is hidden or a symlink, as the directory it names; below
the roots, the search skips hidden directories and symlinks. It never looks
inside a project it found, and is bounded: it reads shallow levels of every
root first, stops after a fixed number of filesystem calls, and lists at most
40 projects. Opening the picker says which configured roots are not there.

A message typed while a workspace's root is starting waits for that root, and
picking the same workspace again does not start a second one. Moving elsewhere
before it arrives leaves you where you went; the root still gets its own
messages. If starting fails you stay in that workspace, and your next message
tries again there. Directory names that are not valid UTF-8 are refused, since
the daemon and `ui.toml` cannot hold them exactly.

## The board

Ctrl+G goes to the agents: it focuses the board, opening it first if it is
hidden, and pressed again in the board returns focus to the composer. `/agent`
does the same as the first press. Ctrl+B only shows or hides the board; showing
a docked board leaves focus in the composer, and hiding the board from inside
it hands focus back. On a docked board, Esc returns focus to the composer and
leaves the board open. The overlay (below) covers the composer, so Ctrl+B shows
it focused and Ctrl+G, Ctrl+B or Esc closes it.

- At 100 columns or wider the board docks on the right, about 30 % of the
  width (40–56 columns). Below 100 columns it covers the screen instead, and
  Esc or Enter closes it.
- On a terminal at least 140 columns wide the board starts open.
- The top is the agent tree, two rows per agent: status glyph, name,
  role, elapsed time and cost, then model and effort. A root's second row
  adds its workspace (`~`, a folder name, or `wt/aether` when two names
  clash); a child's adds one only when it works elsewhere than its parent:
  `isolated` for a separate worktree, else the sub-folder. `◆ needs you` marks an
  agent waiting on you. Finished subtrees fold to one row, whose cost is the
  subtree spend marked `total`.
- Below it, the inspector shows the agent under the cursor: its task, context
  use, model, tokens (`in · cached · out`, with cache reads excluded from
  `in`), cost (own and with children), workspace and latest
  activity. A value too long for its row continues on the next, under the
  value column, rather than being cut. A failed agent shows its error and the `/wake` and `/restart`
  hints.
- Arrows move the cursor and fold; **Enter** makes that agent the viewed one.
  The transcript, the composer's target, the status line and the context
  figure all switch to it. **Alt+↑** views the parent from anywhere, and
  `/agent <name>` jumps straight to an agent.
- `.` shows or hides archived agents (work `/clear` put away).
- **Shift+X** ends the cursor's agent and every live descendant, even beneath
  finished workers. Confirmation defaults to **No** and counts live workers.
- **Shift+D** removes only that row by archiving it; history remains. Finished
  agents are removed immediately. Live agents ask **stop and remove?**, then wait
  for termination before archiving. Descendants end but their rows are not archived.
  Removal failures are reported separately from stopping. After a disconnect or
  removal failure, press Shift+D again to retry. `.` reveals archived rows;
  **Shift+D** on one restores it. Removing a root or the viewed agent asks first,
  even if finished. Removing the viewed agent moves the chat to another available
  agent.
- **Shift+P** toggles pause/resume for the subtree (running turns finish first).
  **Shift+I** interrupts only the current turn; the agent stays for another message.
  Lowercase `x/d/p/i` never perform lifecycle actions; mouse clicks never do either.

Esc never moves between agents: it interrupts.

## Talking to agents

Everything you send goes to the viewed agent.

- **Enter** sends. While the agent is running, Enter *steers* it: the message
  is delivered at the next tool call, and its preview reads
  `steering · lands at the next tool call` until the agent picks it up.
- **Tab** queues the message for after the current turn instead
  (`queued for after this turn`). The queue is kept per agent and sends one
  message each time a turn ends. When the agent is idle, Tab simply sends.
- **Shift+←** pulls the last queued message back into the composer to edit.
- **Shift+Enter**, **Alt+Enter** or **Ctrl+J** insert a newline.
- **↑ / ↓** recall earlier messages; **Ctrl+R** searches them (type to
  filter, Ctrl+R or ↑ for older, Enter keeps the match, Esc restores your
  draft).
- `/` at the start of an empty draft opens the command popup, in a fixed
  order; type to filter, Tab completes, Enter runs.
- `@` opens mentions with two tabs (←/→ switch): **files** in the viewed
  agent's workspace insert a path; **agents** insert `@name`, which is
  expanded to the agent's id when sent (if the name is unique) so the model
  can message it.
  Mentioning never redirects the message.
- A paste longer than 1,000 characters shows as `[Pasted N chars]` and is
  expanded when sent.
- **Alt+,** and **Alt+.** step the viewed agent's reasoning effort; `/model`
  opens the model and effort picker. Each choice shows whether it supports
  token caps only or token and dollar caps. Native Codex supports token caps;
  its API-equivalent estimates cannot enforce dollar caps.

Each agent keeps its own draft: switching to another agent parks your text,
pasted blocks and image chips, and selecting the first agent again brings them back.

### Images

A message can carry images (a screenshot, a diagram) next to its text. There
are three ways to attach one, and each adds a chip above the draft,
`[image: shot.png, 1.2 MB]`:

- **Paste or drop the file.** A paste that is exactly the path of one existing
  PNG, JPEG, GIF or WebP file becomes an image instead of text. Dragging a
  file onto the terminal pastes its path, so that works too; quoted paths,
  backslash-escaped spaces, `~/` and `file://` URLs are all read. Anything
  else, such as a path inside a sentence or a file that does not exist, stays
  text.
- **`/image <path>`** attaches a file by name; `~` and paths relative to the
  workspace resolve as they do elsewhere.
- **Ctrl+V** pastes from the clipboard. An image on the clipboard is
  attached, even when the app that copied it (a browser, a chat app) offers
  text beside it. A file copied in a file manager (`text/uri-list`) attaches
  when it is one PNG, JPEG, GIF or WebP file. Otherwise the clipboard's text is
  pasted, with the same long-text placeholders and image-path detection as the
  terminal's paste. While the board, a view or find holds the keys, Ctrl+V
  says so and leaves the clipboard alone.
  **Alt+V** and `/image` with no path attach only the clipboard image.
  Clipboard reads run in the background through `wl-paste` on Wayland,
  `xclip` on X11, or `osascript` and `pbpaste` on macOS. If the helper is
  missing or the clipboard is empty, the status row says so in one line and
  the draft is untouched. Over SSH the clipboard is on the other machine: copy
  the file across and use `/image <path>`.

**Ctrl+Shift+V**, the terminal's own paste, pastes text: use it when the
clipboard holds both and you want the text. If the terminal sends a truly
empty bracketed paste while the composer has focus, Aether tries to attach the
clipboard image; some terminals send nothing for an image, so use **Ctrl+V**
there. Ctrl+V is read the same in terminals using the kitty keyboard protocol
(Ghostty, kitty, foot) and in legacy ones. Existing custom key
bindings take precedence over the Ctrl+V default; `/keys` shows the active
binding.
Whitespace-only pastes keep their whitespace; plain-mode input stays text.

Chips stay through slash commands and leave with the next message. Backspace
at the very start of the draft removes the last chip, and `/image --clear`
removes them all. An image needs no words: Enter sends it on its own. Enter
while a paste or image is still being read keeps the draft and says so. The
queue (Tab) holds text only, so a draft with images is not queued; send it with
Enter.

A sent image is one line of the transcript under the message's text,
`[image: shot.png, 1.2 MB]`, in every view mode. The picture itself is not
drawn in the terminal.

A message takes at most 10 images, each at most 3.75 MiB and 10 MiB together,
and a file is checked by its bytes, not its name. A screenshot over the limit
is refused with the size in the message; it is not shrunk for you. What each
engine does with an image is in [the engine guide](engines.md#images-on-a-message).

## Approvals

When an agent asks permission, the approval replaces the composer (your draft
is kept underneath). Requests from every agent queue oldest first, and one that
arrives within a second of a keystroke waits until you pause typing.

| Key | Answer |
| --- | --- |
| `y` | allow once |
| `a` | save an allow rule for the asking agent and its subagents (survives reattach/restart) |
| `p` | always allow this, everywhere (saved as an approval rule) |
| `n` / `Esc` | deny; the composer then targets the asking agent so you can say what to do instead |
| `o` | view the asking agent; the approval waits in the needs-you strip |
| `↑ / ↓ / 1-4`, `Enter` | pick and confirm a row |
| `Ctrl+C` | defer without deciding; it moves to the needs-you strip and the board |

`/approvals` lists remembered rules, revokes them, and holds the
auto-approve toggle.

## Find and details

- **Ctrl+F** finds text in the viewed transcript, ignoring case. The bar
  replaces the hint row: Enter or ↓ for the next match, ↑ for the previous,
  Esc to close. The counter counts individual occurrences; only the current
  occurrence is bold, underlined and shown in reverse video.
- **Ctrl+T** toggles details: full tool outputs and reasoning instead of the
  summaries.
- **PgUp / PgDn** scroll by a page and **Shift+↑ / Shift+↓** by a line.
  **Ctrl+Home** goes to the top; **Ctrl+End** returns to the latest output and
  follows it again. Sending a message to the viewed agent does the same, also
  from an older history window, so its reply is on screen.

## Notification center

Agents and services such as Archon post what they need you to see to the
daemon's notification center: questions, blockers, decisions, outcomes and
plain news. The right end of the hint row counts the unread ones. Questions
and blockers stand out in the attention colour, with how many wait on you:

```
  connected                                          $0.51 · ◆ 2 waiting · 2 unread
```

On a narrow screen it shortens to `◆ 2` (or just the count), and it is the
last thing on the row to go. A new question or blocker also rings the bell
and raises a desktop notification while the terminal is not focused.

`/notifications` or **Alt+N** opens the panel: unread, then read, then
resolved, newest first within each, with the poster and its age.

```
  Notifications  2 unread · 2 waiting on you

  Unread
› ◆ Need a GitHub token    blocker · assistant · 29s
  ◆ Merge #231 into next?  question · archon · 1m
  Resolved
    Shipped #229           outcome · archon · 59m

  ↑↓ choose · enter open · r read · x resolve · o go to chat · esc back
```

Enter opens one and marks it read. The detail shows its Markdown body, its
link, and the answers it offers:

```
  Question  from archon · 1m · unread

  Merge #231 into next?

  CI is green on next.

› 1. Merge
  2. Hold
  3. write an answer  a

  ↑↓ choose · enter answer · x resolve · o go to chat · esc list
```

- **Enter** or a **digit** answers with that action; **a** (or the last row)
  lets you type your own answer when the poster accepts one, sent with Enter.
  An answer resolves the notification. An agent that posted it receives the
  answer in its inbox, which wakes it; a service reads it from the daemon.
- **r** marks it read and **x** resolves it without an answer, in the list or
  the detail.
- **o** closes the panel and views the agent that posted it, or the agent its
  link names. A service's notification has no chat to open.

Notifications are kept by the daemon, so they survive restarts and every TUI
attached to it sees the same center, updated live. A daemon from before the
center shows no indicator, and Alt+N says the daemon has none. See
[the protocol](protocol.md#notification-center) for posting from a service.

## Views

Commands that need more than a line open a view in the bottom pane:
`/model`, `/memory`, `/approvals`, `/usage`, `/settings`, `/mode`, `/artifacts`,
`/mcp`, `/schedules`, `/notifications`, `/report`, `/doctor`, `/voice setup` and `/login`.
Schedules show the local date alongside the next fire time when it is not today.
Lists take ↑/↓ or a digit to choose, Enter to use, and Esc to go back. The run
report and an opened artifact are read in a full-screen pager.

## Themes

The UI paints text with the terminal's own colours and keeps colour for
meaning: what is running, what needs you, what finished or failed, and which
kind of tool a call was. Running agents and calls pulse gently in the running
hue; everything else holds still.

| Theme | |
| --- | --- |
| `auto` | default: Aether's own colours, light or dark from the terminal background |
| `dark`, `light` | Aether's colours, forced to one background |
| `nord`, `catppuccin`, `gruvbox`, `tokyo-night` | built-in palettes; add `-dark` or `-light` to pin one (`catppuccin-mocha`, `catppuccin-latte` and `tokyo-night-day` also work) |
| `mono` | weight, dim and reverse only; also chosen when `NO_COLOR` is set. `/theme` still previews the other themes in colour unless `NO_COLOR` chose `mono` |
| any other name | a theme file, below |

`/theme` lists every theme with a swatch of its colours on your terminal
(colourless when `NO_COLOR` chose `mono`);
Enter uses one and saves it to `ui.toml` as `theme = "<name>"`. `/theme <name>`
does the same directly, and `/settings` steps through the built-ins. For one
run use `--theme` (also `AETHER_THEME`). The flag wins over `NO_COLOR`, which
wins over the saved choice. `dusk`, `dawn` and `monochrome` are still accepted
as `dark`, `light` and `mono`. When the terminal does not report its colours,
`auto` falls back to `COLORFGBG`, then dark.

When the terminal reports its background, each hue is made readable against it
(WCAG AA, 4.5:1); the pulse dims below that between its peaks, and quiet text
and rules are blended from the terminal's own colours. Colours are reduced to
the terminal's depth: 256-colour terminals get the nearest palette entry, and
16-colour terminals use their own basic colours, so there every palette looks
the same.

### Theme files

A theme file is `~/.config/aether/themes/<name>.toml` (under
`AETHER_CONFIG_DIR` when that is set); `<name>` is lower-case letters, digits,
`-` and `_`. It starts from a built-in and sets any of the hues below to
`"#RRGGBB"`, for both backgrounds at the top level or for one under `[dark]`
or `[light]`:

```toml
# ~/.config/aether/themes/ember.toml
base = "gruvbox"          # optional; aether when left out
running = "#FF9E3B"
agent = "#E46876"

[light]
running = "#B45309"
```

| Key | Colours |
| --- | --- |
| `accent` | the model chip, selection marks and find hits |
| `running`, `waiting`, `blocked` | agents and calls in that state; the pulse runs in `running` |
| `attention` | needs you: approvals and questions |
| `success`, `failure` | finished and failed |
| `shell`, `read`, `edit`, `web`, `call`, `agent` | tool verbs: `Ran`, `Explored`, `Edited`, `Fetched`, `Called`, `Spawned` |
| `user` | the `›` before what you typed |
| `code`, `link`, `heading` | markdown in replies |
| `diff_add`, `diff_remove` | added and removed lines |

A file is read each time it is chosen, so edits show on the next `/theme`. A
file with a mistake still paints its `base` and says which file and key are
wrong; a name with no file changes nothing and says so. See
[theme overrides](theme-overrides.md) to set a hue over every theme.

## Motion and ASCII

- Only the status row, running agents on the board and streaming text move.
  `--reduced-motion`, `AETHER_REDUCED_MOTION` or `/settings → Motion` keep
  them still.
- `--ascii` (also `AETHER_ASCII`, or `ascii = true` in `ui.toml`) swaps every
  glyph for an ASCII one (`›` → `>`, `◆` → `!`, `✓` → `+`, …), including the
  separators the TUI authors in notices. Message text and trigger payloads
  keep their own `·` and `→`. It is set at launch; `/settings` shows it but
  cannot change it.

## Mouse and copying

Mouse capture is **on** by default. The wheel scrolls the pane under the
pointer: the viewed transcript, the agents tree or an open scrollable view.
The composer ignores the wheel. Click an agent row to view it; click the
transcript or composer to return to typing.

Drag to select & copy text within one pane. Selection stays inside that pane,
trims trailing spaces and remains highlighted until Esc or a new click.
Transcript selection follows its content across scrolling; dragging beyond
its top or bottom auto-scrolls. Decorations are copied as shown, and transcript
soft wraps retain newlines because the render model does not mark continuations.
Composer soft wraps copy as their original logical lines.
Shift+drag uses the terminal's own selection (which can span panes).

`/mouse off` or `/settings → Mouse → off` disables capture and restores native
terminal selection. Saved `mouse = "off"` remains honoured. `scroll` enables
scrolling, clicks and selection; the old `full` preference is an equivalent alias.

Copying always sends OSC 52, including over SSH. Locally it also uses `wl-copy`
on Wayland, `xclip` / `xsel` on X11 or `pbcopy` on macOS. Terminal clipboard
permission is required; OSC 52 has no acknowledgement. Known supporting
terminals are assumed to allow it; unknown terminals get a “sent” notice rather
than confirmed success. Helper failure is reported if that terminal route cannot
be relied on. Payloads are limited to 1 MiB and helper execution to two seconds.
In tmux, enable `set-clipboard on`.

`aether tui --mouse` starts a run with `scroll`, and `--no-mouse` with `off`,
without saving.

## Plain mode

`--plain` prints conversation cells and session status as lines: no colour,
glyphs, motion or cursor movement, for screen readers, logs and pipes. It is used automatically
when standard output is not a terminal. Each line you type is sent as if
typed into the composer, slash commands included; end input (Ctrl+D) or type
`/quit` to leave immediately, even while input remains open. Assistant text
streams incrementally; continuation lines are indented and the completed
reply is not printed again. While an approval waits, only bare `y/a/p/n` answer
its printed prompt; slash commands still work. `/cancel` prints a separate
confirmation: type `yes` to end the named agent, or `no` to decline. Other text
abandons that confirmation and is processed normally. Slash commands also
abandon it. `/usage` and `/help` print text; interactive lists and pagers require
the TUI. Use `aether agents --all` to browse archived agents in plain workflows.
`/image <path>` prints `attached shot.png, 1.2 MB` and the image leaves with the
next message line; a line that is only a path is text here, never an image.

Plain logs now use `> message` and `* reply` cell text plus session summaries.
The earlier timestamped records and per-reply duration/cost/thinking headers
are no longer emitted. Update parsers accordingly; `aether run` is unchanged.

## Moved out of the TUI

These removals are intentional. The daemon capabilities remain available:

| Former command or view | CLI equivalent |
| --- | --- |
| `/budget`, `/priority`, `/concurrency` | `aether set <agent> --token-budget/--cost-budget/--max-turns/--priority/--concurrency` |
| `/subtree` broadcast | `aether send <agent> --subtree <message>` |
| `/trigger` | `aether trigger <agent> <source> --payload <json>` |
| Replay and inspector Turns/Tools/Checkpoint tabs | `aether replay <agent>`; transcript Ctrl+T still shows output and reasoning |
| Event timeline and timeline search | `aether events` (including follow/filter options) |
| `/approve`, `/deny`, including batch answers | `aether approvals` lists pending IDs; `aether approve <approval>` / `aether deny <approval>`, or `--all-pending --run <run>` for a scoped batch |
| `/always`, `/never`, tool-wide rules and remembered refusals | `aether rules list\|allow\|deny\|revoke`; TUI `y/a/p/n` answer one prompt |
| GitHub-watcher view | `aether github-watcher` |
| Notice history | Client failures persist as error cells in the current transcript; daemon events are available through `aether events` |
| Memory curation Activity tab | `aether events`; `/memory` Enter opens full record details |
| `/show` in plain mode | `aether agents --all`; `.` remains available in the interactive board |

Vim cursor/selection, divider dragging/sidebar resizing, fullscreen panel keys,
and the old Ctrl+P/Ctrl+X×2/Ctrl+↑↓/Ctrl+O bindings were also removed
(Ctrl+O now opens the workspace picker, where Ctrl+P pins a workspace).
Use terminal selection or `/copy`, the automatic board layout, the slash popup,
`/cancel`, board navigation, and Ctrl+T respectively. Ctrl+G now goes to the
agents board. High-contrast
`contrast`/`hc` themes and the saved `high_contrast` setting were removed; choose
a theme with `/theme` and set contrast in your terminal.
Existing dusk/dawn aliases still work.

### Event timeline from the CLI

`aether events` shows the latest 50 matching events when stdout is a terminal.
Piped output retains the full history; use `--tail N` to bound either mode
(`--tail 0 -f` watches only new events). Results are in sequence order, with
UTC dates and times in human output and one complete event per line with
`--json`. `--after SEQ` is an exclusive lower bound, including with a tail.

Filter with `--agent ID`, `--run ID`, `--task ID`, `--type tool.failed`, or
`--family tool`. Types and families accept repeated flags or comma-separated
values. For example:

```sh
aether events --run RUN_ID --type tool.failed --since 1h --tail 20 -f
```

`--since` accepts positive integer durations such as `30s`, `5m`, `2h`, or
`1d`, measured from command startup. `--grep TEXT` matches a case-sensitive
literal substring of the full event JSON. These two filters run locally
because the event protocol has no timestamp or text filter; sparse matches
can require reading older pages. A tail counts matching events, then `-f`
continues from the history snapshot without replaying it.

## Leaving

Quitting only detaches this client. Agents keep running in the daemon, and the
UI says so on the way out:

```
Agents keep running in the daemon (3 running · 1 needs you). Reattach with: aether
```

**Esc**, first match wins: answer a due approval with deny; close the popup,
`?` panel, find bar or view; leave the board; interrupt the viewed agent if it
is running.

**Ctrl+C**, first match wins: defer a due approval; close the view, popup,
`?` panel or find bar; clear the draft (↑ brings it back); interrupt the
viewed agent if it is running; otherwise quit. Ctrl+D on an empty composer
and `/quit` also quit. `q` is only ever a letter.

On Unix, Ctrl+Z suspends the TUI after restoring the terminal. Use your shell's
`fg` command to resume it; the screen redraws and your draft stays intact.

`/compact` and `aether compact AGENT_ID` fold older context without requesting
an assistant reply. Pending messages still receive replies once the agent's
wait lets them through; a waiting agent returns to its wait. Empty context or a
failed compactor produces a notice without a reply call. A Claude Code agent
runs Claude Code's own `/compact` in its session; other agents use the
configured compactor. The transcript shows the fold while it runs, then who
folded it, the token counts and how long it took. An agent's `request_checkpoint` tool keeps
its normal continuation, and changing models preserves handoff compaction.
A request during a running turn waits for its next context assembly.

To stop the daemon and everything it runs, use `aether stop`. The daemon syncs
its final store writes before allowing up to two seconds for leftover Tokio
blocking work to exit.

## Keys

`?` on an empty draft shows the current bindings above the composer. The
shortcut list below shows defaults; `/settings` → **Keys** changes them.
List, popup, board navigation and approval keys stay fixed.

<!-- BEGIN GENERATED: keys -->
**Anywhere**

| Keys | Action |
| --- | --- |
| `Esc` | close what is open, leave the board, else interrupt the turn |
| `Ctrl+C` | close, clear the draft, interrupt, else quit (agents keep running) |
| `Ctrl+D` | quit on an empty draft (agents keep running) |
| `Ctrl+G` | go to the agents board, opening it if hidden |
| `Ctrl+B` | show / hide the agents board |
| `Ctrl+O` | switch workspace: view its agent, or start one there |
| `Alt+O` | files in the focused or latest message |
| `Alt+E` | open the viewed file externally |
| `Alt+W` | close the file viewer |
| `Alt+Z` | toggle full screen file viewer |
| `Alt+-` | narrow the file viewer |
| `Alt+=` | widen the file viewer |
| `Alt+T` | switch a PDF to text or an HTML file to a screenshot |
| `Alt+N` | the notification center: answer, mark read, resolve, go to the chat |
| `Alt+↑` | view the parent agent |
| `Ctrl+T` | details: full outputs and reasoning |
| `Ctrl+R` | search message history |
| `Ctrl+F` | find in the transcript |
| `Alt+, / Alt+.` | less / more reasoning effort |
| `PgUp / PgDn` | scroll the transcript |
| `Shift+↑ / Shift+↓` | scroll the transcript a line |
| `Ctrl+Home / Ctrl+End` | top of the transcript / latest, following again |
| `F4` | start/stop local dictation; review the draft before sending |
| `F5` | insert recovered dictation at the cursor without replacing the draft |

**In the composer**

| Keys | Action |
| --- | --- |
| `Enter` | send; while the agent runs, steer it |
| `Tab` | queue for after this turn; when idle, send |
| `Shift+Enter / Alt+Enter / Ctrl+J` | newline without sending |
| `↑ / ↓` | previous / next message from history |
| `Shift+←` | pull the last queued message back to edit |
| `?` | shortcuts, on an empty draft |
| `/` | commands, at the start of a draft |
| `@` | mention a file or an agent |
| `Ctrl+A / Ctrl+E` | start / end of the line |
| `Ctrl+W / Ctrl+U / Ctrl+K` | delete word back / to line start / to line end |
| `Ctrl+Y` | paste what was last deleted |
| `Ctrl+V / Alt+V` | paste the clipboard image, else its text (Ctrl+Shift+V: text) / only an image |
| `Alt+B / Alt+F / Ctrl+← / Ctrl+→` | move a word back / forward |

**In the board (Ctrl+G)**

| Keys | Action |
| --- | --- |
| `↑ / ↓` | move between agents |
| `← / →` | fold / unfold, or go to the parent / first child |
| `Home / End` | first / last agent |
| `Enter` | view the agent |
| `Shift+X` | confirm ending the agent and all live descendants |
| `Shift+D` | archive this agent; confirm stop first if live; restore an archived row |
| `Shift+P` | pause or resume the agent subtree |
| `Shift+I` | interrupt only the current turn; keep the agent |
| `.` | show or hide archived agents |
| `Esc / Ctrl+G` | back to the composer |

**In an approval or a list**

| Keys | Action |
| --- | --- |
| `y / p / n` | allow once / always, or deny |
| `a` | save an allow rule for this agent and its subagents |
| `o` | open the agent that asks |
| `↑ / ↓ / 1-9` | choose a row |
| `Enter` | use the highlighted row |
<!-- END GENERATED: keys -->

### Changing key bindings

Open `/settings`, choose **Keys**, and press Enter. Each action lists its
current chord; `*` marks a custom binding. Enter starts capture, the next key
sets the binding, and Esc cancels capture. Backspace or `r` restores the
highlighted action's defaults (including its aliases); **Reset all** restores
every action. A replacement chord replaces all aliases for that action.
Changes take effect immediately in dispatch, the shortcuts panel and hints,
and are saved in `ui.toml`. Modified and function-key global shortcuts take
priority over view shortcuts and approval answers; plain navigation stays
fixed. During capture, the key editor keeps every key:

```toml
[keys]
agents = "f2"
details = "ctrl+l"
newline = "alt+enter"
```

Only overrides are saved; omit an action to keep its defaults. Action names
include `agents`, `board`, `workspace`, `details`, `history`, `find`, `parent`,
`less_effort`, `more_effort`, `page_up`, `page_down`, `line_up`, `line_down`,
`top`, `latest`, `dictate`, `insert_dictation`, `send`, `queue`, `newline`,
`unqueue`, `shortcuts`, `line_start`, `line_end`, `erase_word`, `erase_start`,
`erase_end`, `yank`, `word_left` and `word_right`.

Quit/interrupt chords (Ctrl+C, Ctrl+D and Esc), terminal flow-control chords,
the terminal-response prefix Alt+], and ordinary typing/list-navigation keys
cannot be reassigned. A chord used
by another action is refused with its name; resetting can also be refused
if another action now uses that default. Reset all resolves that situation.
Ctrl+I is normalised to Tab, Ctrl+M to Enter and Ctrl+[ to Esc; ambiguous Ctrl
combinations are refused. Function keys F1–F24, navigation names such as
`up` and `pageup`, and Ctrl/Alt/Shift combinations are supported. Some
terminals intercept chords before Aether receives them; capture only binds
keys that arrive at the TUI.

An unknown action, invalid chord or conflicting override in the file gives a
startup notice naming the entry and uses that action's defaults. Invalid
entries remain in the file when other preferences are saved, so they can be
corrected; resetting that action or all keys removes them. A failed save is
reported and the binding still applies for the current run.

## Commands

`/` lists these in this order. Older names (`/models`, `/effort`, `/theme`,
`/mouse`, `/motion`, `/rules`, `/yolo`, `/exit`, …) still work but are not
listed. A few take an argument directly: `/theme nord`, `/mouse scroll`,
`/motion reduced`, `/effort high`.

<!-- BEGIN GENERATED: commands -->
| Command | What it does | Key |
| --- | --- | --- |
| `/model [--default] [provider/model]` | choose the model and effort; --default sets new agents |  |
| `/new [objective]` | start a new root agent |  |
| `/workspace [path]` | switch workspace: view its agent, or start one there | `Ctrl+O` |
| `/agent [name]` | open the agents board, or view the named agent | `Ctrl+G` |
| `/notifications` | what agents and services need you to see; answer, read, resolve | `Alt+N` |
| `/compact` | fold older context into a checkpoint |  |
| `/copy` | copy the last reply to the clipboard |  |
| `/image [path\|--clear]` | attach an image file, or the clipboard image; --clear drops them | `Alt+V` |
| `/memory` | browse durable memories; pin or forget one |  |
| `/approvals` | remembered approval rules and the auto-approve toggle |  |
| `/usage` | providers, sign-in, health and account limits |  |
| `/settings` | theme, motion, mouse and ascii |  |
| `/theme [name]` | choose the colour theme; a name sets it directly |  |
| `/remember [--agent\|--global\|--repository] <durable fact>` | record an operator fact; default: the viewed agent only |  |
| `/stop` | interrupt the current turn; the agent keeps its context | `Esc` |
| `/cancel` | end the viewed agent for good (asks first) |  |
| `/pause [subtree]` | pause the viewed agent, or everything under it, after its turn |  |
| `/resume [subtree]` | resume a paused agent, or everything paused under it |  |
| `/wake` | wake a failed agent in place, keeping its history |  |
| `/restart` | recreate a failed agent from its last checkpoint |  |
| `/rename <new name>` | rename the viewed agent |  |
| `/instructions <text>` | replace the viewed agent's standing instructions |  |
| `/artifacts` | the viewed agent's artifacts; Enter opens one |  |
| `/mcp` | MCP servers, their health and their tools; r reconnects |  |
| `/schedules` | recurring timers and their history; pause, resume or cancel |  |
| `/report` | what this agent's run did: agents, tools, cost, open loops |  |
| `/doctor` | what is and is not working: connection, sign-in, paths |  |
| `/voice [status\|setup\|check\|on\|off\|mute\|replies on\|replies off\|start\|stop\|cancel\|insert]` | local dictation and spoken replies | `F4` |
| `/login [provider]` | sign in: Claude or ChatGPT subscription, or an API key |  |
| `/logout <provider>` | remove stored credentials for a provider |  |
| `/clear` | clear finished work out of the view; it stays in the store |  |
| `/help` | keys and commands | `?` |
| `/quit` | leave the client; agents keep running in the daemon | `Ctrl+C` |
| `/mode [name\|reload]` | choose a transcript view mode |  |
| `/outbox` | remote operations the host cannot confirm; deliver, resend or discard |  |
<!-- END GENERATED: commands -->

These tables are generated from `crates/aether-tui/src/commands/`; regenerate
them with `AETHER_BLESS_DOCS=1 cargo test -p aether-tui --test docs_tables`.

A `/name` missing from this table goes to the viewed agent as an ordinary
message, verbatim, so another program's commands (Archon's `/answer 3 yes`)
reach the agent that reads them. The first one in a session says so. With no
live agent in view it stays an unknown command and nothing is sent.

### Host commands

A program that runs Aether for its own agents can list its commands in the
menu. `aether tui --host-commands <PATH>` reads a JSON file:

```json
{"version": 1, "host": "archon", "commands": [
  {"name": "answer", "args": "<n> <text>", "about": "answer question n"}
]}
```

Each command appears in the `/` popup after the built-ins, with its
argument hint and marked with the host's name (`· archon`; `· host` when `host` is left out), and in `/help`.
Choosing or typing one sends the line, as typed, to the viewed agent; the
TUI never runs it. A name is lowercase `[a-z][a-z0-9-]*` without the slash.
An entry that is malformed, repeated or named like a built-in command, one
of its aliases or a view mode is left out, and a missing or unreadable file loads no
commands; either way the TUI starts and says why once.

## View modes

A view mode changes presentation of the viewed agent's retained transcript.
The default `standard` keeps the stock view. The built-in `focus` shows prose,
delegations, messages, completions and status, with compact coordination rows.
Tools, thinking, memory, compaction and accounting remain available with Ctrl+T.
Questions, approvals, failures and actionable health notices always stay visible.
A failed tool call is tool detail: a mode without `tool` keeps one line for it.
Modes do not alter agent execution, event ingestion, approval controls or the board.

Embedders can supply `<config dir>/modes/brief.toml` alongside `ui.toml`:

```toml
version = 1
label = "Brief"
description = "Coordination without tool detail"
density = "compact"
show = ["user_text", "assistant_text", "delegation", "agent_message",
        "completion", "status", "question", "approval", "error"]

# Optional; exact owner match, AND when both fields appear.
[scope]
role = "supervisor"
# name = "coordinator"
```

`version` and `show` are required. Density defaults to `comfortable` (stock cell
rendering); `compact` makes routine coordination one physical line and reduces
gaps. Prose and decisions keep their full bodies; the Details shortcut (Ctrl+T
by default, editable in `/settings` → Keys) reveals truncated rows.
The remaining categories are `tool`, `reasoning`, `memory`, `compaction`, and
`usage`. Failed children and compactions are visible even when their category
is omitted. There is no subtree inheritance or combined fleet feed.
A scope mismatch uses standard presentation for that agent.

`error` covers the agent and the session: a failed turn, a model or provider
error, a daemon, store or connection fault. These are drawn whole in every mode,
listed or not. A tool call that failed belongs to `tool`. A mode that lists
`tool` draws it whole. A mode that omits `tool` draws one row for it, clipped to
the width: the tool's name (`Command` for a shell call), `failed`, and the first
line of its error. The command, the arguments and the rest of the output stay
out. Failed calls with nothing shown between them share the row of the first.
Ctrl+T and the standard mode still draw every call: nothing leaves the
transcript. A call that waits for approval, was denied or was cancelled is drawn
whole in every mode.

External trigger notices show `Trigger from <source> · <summary>` when the JSON
payload is an object with a nonblank top-level string `summary`. This applies to
standard and named modes. Summaries are sanitized, flattened to one line, and
bounded to a 4,096-byte preview before row-width truncation. Ctrl+T reveals the
JSON payload as well, with the existing sanitized 4,096-byte preview limit and a
`[truncated]` marker for longer payloads. Missing, blank, or non-string summaries
keep the usual payload preview; string and null payloads also keep their usual
rendering. This field affects only presentation: agents receive the full payload.

The filename stem is the mode name: `[a-z][a-z0-9_-]{0,47}`. Built-ins, `reload`,
and all slash commands and aliases are reserved. Unknown keys (including nested
scope keys), versions, categories, density values, duplicate categories and
blank scope values reject the entire file. Labels may contain up to 64 characters,
descriptions 160, and scope values 128; all must be nonblank single-line text
without terminal controls. Definitions are never rewritten by preference saves.
Discovery accepts at most 128 regular TOML files of at most 64 KiB each; symlinks
and nonregular entries are rejected. Bad files produce a startup notice and
persistent diagnostics in `/mode`; valid files remain available. Exceeding the
file count rejects the entire external catalog.

Use `/brief` or `/focus` to toggle that mode back to standard; `/mode brief` sets
it idempotently. `/mode` opens the picker; `/settings` includes a View mode row
after Keys. Select it with `6` or the arrow keys, use Left/Right to cycle modes,
or Enter to open the picker. The Keys row still opens the key-binding editor.
Choices save `view_mode` in `ui.toml`; failed saves are reported while the choice
still applies for this session. `/mode reload` reloads definitions explicitly.
A removed or invalid active mode falls back visibly to standard without
rewriting the saved selection. There are no watchers or additional timers.

Launch selection is `--view-mode` > nonblank `AETHER_VIEW_MODE` > `ui.toml`
`view_mode` > `standard`. The global flag works for both bare `aether` and
`aether tui`. Flag/env overrides are session-only. Invalid explicit selections
fail before daemon startup or terminal attach; stale saved selections fall back
with a persistent notice.

```sh
aether tui --view-mode focus
AETHER_VIEW_MODE=brief aether
```

Named modes show inter-agent deliveries immediately, promote the same cell when
consumed, retain full outbound message bodies, and show deduplicated own-task
results. Standard keeps consumption-time display and its normal outgoing preview.
Find searches full retained text of visible cells and enables details when a hit
needs them. Compact rows prefer shown occurrences and reindex when the pane
width changes; hidden categories do not create phantom matches. Mouse copy uses the
shown rows; `/copy` retains its last-assistant-reply behavior. ASCII decoration,
mono/NO_COLOR and reduced motion continue to use the existing accessibility
preferences; mode files cannot override them.

Plain output shares category and scope decisions. `/mode` prints the catalog and
diagnostics. A runtime switch prints a boundary and applies to subsequent updates;
it cannot erase or replay earlier output. Suppressed cells and streamed prefixes
advance output cursors. Questions and approvals remain answerable, and output is
sanitized as usual. Rendering caches the visible index and updates changed cells
from a bounded journal, so unchanged frames do not rescan hidden history.

## Local file viewer

**Alt+O** lists local file references in the last clicked message, or the
latest message when none is focused. Bare paths, Markdown links and local
`file://` links are supported. Click a highlighted reference without dragging
to open it; dragging continues to select text. The viewed agent's own
workspace resolves relative paths, including a child's separate workspace.
OSC 8 links are parsed when present in the original message; terminal control
sequences are stripped before painting.
Reference discovery examines the first 256 KiB of each message, with a shared
256 KiB lookahead budget and at most 256 references, so malformed links cannot
trigger repeated unbounded scans while output streams.

The viewer starts at half width. **Alt+- / Alt+=** resize it, **Alt+Z** toggles
full screen, and **Esc** (with the viewer focused) or **Alt+W** closes it. Narrow terminals show it full width.
**Tab** switches focus and leaves full screen when returning to chat; arrows and PageUp/PageDown scroll while focused, and
**Alt+E** opens the selected file through `xdg-open` (Linux) or `open` (macOS).
Approvals retain priority. Closing the viewer does not stop the agent.
Full screen blocks composer edits and pastes; **Tab** returns to chat.
Completed dictation is retained for later insertion. **Ctrl+C** closes a
full-screen viewer before it can clear the draft.

PNG, JPEG, GIF (first frame) and WebP decode off the UI thread, as do text and
Markdown layout. A startup terminal query, bounded to 200 ms, chooses Kitty,
Sixel or iTerm2; no terminal-name guessing is used. Missing pixel cell metrics
or an unsupported terminal falls back to coloured Unicode half blocks.
Through tmux and SSH, unsupported or unanswered probes also fall back; no
passthrough configuration is changed. An SSH-connected remote agent shows a
notice rather than reading a same-named local path. There are no URL fetches.

Image inputs are limited to 20 MiB, 8 million pixels and a 64 MiB decoder
allocation budget; rendering is capped separately. UTF-8 text inputs are
limited to 1 MiB, with a preview of up to 64 KiB / 4,000 lines. Terminal
controls are stripped. Only one decode runs at a time; rapid resize requests
coalesce. A canonical path outside the workspace requires a second Enter
before reading. Native protocol encodings are cached until the preview
changes. Markdown uses existing styles; language-specific syntax highlighting
is deferred. SVG offers **Alt+E** to open externally only.
A missing file shows `File not found: <name>` with the name in the pane title.

### PDF

A PDF renders one page at a time through the same graphics path as images.
The worker uses `pdftoppm` (poppler-utils), or `mutool` (MuPDF) when poppler is
missing, and counts pages with `pdfinfo` or `mutool info`. The header shows
`page n/total`. **PgUp/PgDn** turn pages (arrows scroll), and **Alt+T**
switches between the page image and its `pdftotext` text. Without a renderer the
pane shows the page text. With `pdftotext` but neither `pdfinfo` nor `mutool`
it shows the whole document's text, unpaged. Without PDF tools it names the
packages to install.
**Alt+E** still opens the file externally.

Pages render only when shown, off the UI thread, one request at a time. Each
tool runs as a child process with an argv, never a shell, in its own process
group. It has a deadline: 10 s for page counts and text, 20 s for a page
image. The whole group is killed at the deadline. The tools read a private
copy of the checked file in a fresh temporary directory, never the path
itself. PDFs are limited to 50 MiB and 100,000 pages. A page image is scaled
so its longer side is at most 2,048 pixels, and its PNG is limited to 20 MiB
before the usual image limits apply. The six most recent rendered pages
(48 MiB at most) are cached, keyed by file size, modification time, page and
size.

### HTML

HTML files up to 5 MiB show as readable text, rendered by the
[`html2text`](https://crates.io/crates/html2text) crate on top of
`html5ever` (about 100 KiB and 90 KiB of source; no default features).
Headings, paragraphs, lists, tables, emphasis and code blocks keep their
shape. `<script>`, `<style>` and `<head>` content is dropped, and terminal
controls are stripped. Links are styled and numbered, and listed with their
targets at the end. They are never fetched or followed. Text stops after
4,000 rows. Mis-nested markup can make an HTML parser build far more than
the file holds, so parsing stops after 3 s or 200,000 nodes, and a tree
deeper than 512 elements is refused; **Alt+E** still opens such a file.

**Alt+T** takes a screenshot with headless Chromium (`chromium`,
`chromium-browser`, `google-chrome` or `google-chrome-stable` on `PATH`) and
shows it through the image path. The page renders isolated:

- Chromium never sees the file. The checked bytes are embedded in a fresh
  temporary page, as the `srcdoc` of an `<iframe sandbox>` with no
  permissions. Scripts, meta refresh and top-level navigation are off, and the
  frame's origin is opaque.
- A `Content-Security-Policy` of `default-src 'none'` refuses every fetch,
  frame and font or image that is not a `data:` URL, including local files.
- The browser is offline. A dead proxy serves every host, loopback included,
  and host resolution is disabled.
- A disposable profile and `HOME` live in the temporary directory, which is
  deleted afterwards. Chromium's own sandbox stays on.
- The process group is killed after 15 s.

The screenshot shows the top of the page at the pane's shape (800 to 1,600
pixels wide). **Alt+T** returns to text, and without Chromium it shows the text
with a notice. **Alt+E** opens the file in your browser through `xdg-open` or
`open`, outside these restrictions.

Tools are found on `PATH` once per TUI start; restart the TUI after
installing one.

## Scenario tests

There is no `--script` mode: the TUI needs a terminal, and mouse input and
multi-MIME clipboard contents (an image plus text) cannot be driven through
one headlessly. Scenario tests instead drive the `App` state machine directly,
inside `aether-tui`'s unit tests:

- `crate::testing` builds apps (`populated_app`, `loaded_app`,
  `app_with_transcript`) and `render_to(&app, w, h)` returns the screen as text.
- `app.reduce(AppEvent::...)` feeds daemon replies, key presses, pastes and
  previews; clicks arrive as the same events a mouse produces.
- Assert on app state (`app.input`, `app.selected`, `app.viewer`) and on the
  rendered text. Examples: `viewer/tests.rs` (open a reference, load it,
  render the pane) and `app/tests/images.rs` (chips, drafts per agent).
- Key chords are covered by the table in `tests/registry/key_cases.rs`.
