# LearnHarness: visual and behavioural reference

A clean-room record of how upstream LearnHarness (v0.5.0, checked out unmodified under `/var/tmp`)
looks and behaves, for the Aether frontend team. Every description is my own wording, from
watching the running app. Captures live in `.archon/evidence/learnharness/` and are listed in
its `manifest.json` (`{file, screen, state, theme}`).

## Clean-room statement

Nothing was copied from upstream: no source, CSS, prompts, assets or text passages. The
screenshots show the running app; the only upstream wording in this file is short UI labels
(a few words each), quoted so readers can find the control. The lesson content in the
screenshots (gradient descent, quizzes, problems, the knowledge-graph propositions) was
written for this exercise and served by a scripted model in
`.archon/evidence/learnharness/tools/`. I read upstream source only to find selectors
(`aria-label`, `data-testid`) and request shapes. None of it is reproduced here.

## How it was captured

- **App**: `next dev` on loopback, with a throwaway profile (`LH_DATA_DIR`, `LH_AUTH_FILE`,
  `HOME` all under `/var/tmp/lh-ref-run`). The checkout was not modified.
- **Model**: `tools/scripted_model.py`, a loopback OpenAI-compatible server. It streams replies
  in small timed slices, with reasoning and tool calls. A scenario module chooses each turn from
  the request: prober, teacher, extractor, auditor, researcher or practitioner.
  - Aether's `evals/noesis/differential/faux_model.py` could not serve this app. It replays a
    fixed sequence with no pacing or reasoning, and LearnHarness picks its agent by the tools
    it offers. So I wrote a private server. No paid key was used.
- **Provider**: registered through the app's own onboarding (Custom endpoint, Keyless), the
  same way a user would.
- **Driver**: `tools/capture.mjs` and `tools/capture-flows.mjs` run one headless Chromium
  through Aether's own `playwright-core`.
  - Viewport 1440×900. The Next.js dev badge is hidden with injected CSS.
  - `tools/driver.mjs` is the step-by-step helper I used to explore before scripting.
  - To re-run from a numbered capture, set `CAPTURE_RESUME_FROM`, `CAPTURE_SECTIONS` and
    `CAPTURE_TEACHING_URL`.
- **Themes**: every still is taken with `prefers-color-scheme: dark` and again with `light`
  emulated.
  - **LearnHarness is dark-only.** Settings → Appearance shows Mode fixed to Dark and marked
    "Coming soon" (`31-settings-appearance-*`).
  - 47 of 55 pairs are pixel-identical. The other 8 differ only because a stream, spinner or
    fade was still animating between the two shots.
  - The `-light` files are kept to show that the app ignores the OS scheme. They are not a
    second design.
- **Videos** (dark, 960×600): `video-chat-streaming-dark.webm` (17 s: typing a request, then a
  long markdown/maths/code reply streaming in) and `video-graph-interaction-dark.webm` (10 s:
  wheel zoom in, drag pan, zoom out).

## Parity table

The numbered prefix is the capture order; each still exists as `NN-…-dark.png` and `NN-…-light.png`.

### Navigation and app shell

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| Left sidebar | About 288 px wide, full height. From the top: logo mark and collapse button; two "new" entries (Teaching, Practice) in small uppercase, letter-spaced monospace; a SESSIONS label beside a search field; a three-way segmented filter (Teaching / Practice / All); the session list; then Background agents and Settings pinned at the bottom above a hairline | `08-home-ready-empty-graph`, `50-navigation-sidebar-all-sessions` |
| Session rows | Title in the sans face, truncated with an ellipsis; below it a small mono status line ("Waiting for input · just now", "Working…", "Problem waiting"). The active row gets a slightly lighter rounded fill; a running session shows a small spinner; practice sessions carry a link-style glyph at the right | `17-chat-streaming-mid-reply`, `48-practice-concluded-solution-revealed`, `50-…` |
| Session search | Typing filters the list live, in place | `51-navigation-sidebar-session-search` |
| Collapsed sidebar | Collapses fully; the main pane takes the whole width, and a small button brings the sidebar back | `53-navigation-sidebar-collapsed` |
| Keyboard cheat sheet | "?" opens a compact centred panel over a dimmed screen: a title with an Esc hint, then shortcuts grouped under Global and Chat, each a mono label with right-aligned key caps in small bordered boxes. It covers dictation, settings, model selector, thinking level, sidebar, graph, new session, find, and keyboard answering of quiz rows | `52-navigation-shortcut-cheat-sheet` |
| Session header | A large serif title (the learner's goal, truncated) spans the top of the chat; on the right sit a PLAN chip and a phase indicator (ring spinner plus PROBING / PLANNING / TEACHING; PRACTICE in practice sessions) | `13-chat-plan-diagram`, `48-…` |
| Background agents | The bottom entry; with no running jobs, clicking it showed nothing visible, so it is not captured | — |

### Onboarding and model setup

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| First-run home | Empty home: the knowledge-graph canvas (empty), a centred illustration with a short two-line hint, and a small pill above the composer prompting for a provider key. The composer still renders | `01-home-empty-no-provider` |
| Provider dialog | Modal with a serif title. A provider dropdown, plus a subscriptions section where sign-in providers each have a Connect button | `02-onboarding-provider-dialog` |
| Provider list | Long scrollable list of hosted providers, ending with "Custom endpoint" | `03-onboarding-provider-list-open` |
| Custom endpoint | Form with base URL, optional model IDs, and an authentication choice (Keyless / API key), then Connect / Cancel and a one-line capability note | `04-onboarding-custom-endpoint-form` |
| Model tiers | Second step: three tiers (Strong, Cheap, Vision). Each tier has a one-line role hint, a "needed" badge until filled, a wide model picker and a narrow reasoning-effort select; Save models at the bottom | `05-onboarding-model-tiers-empty`, `07-onboarding-model-tiers-chosen` |
| Model menu | Searchable popover; each model shows its price state ("Unpriced") and context size ("128k") | `06-onboarding-model-menu-open` |
| Saving | The button turns to "Saving…" and the dialog closes when the save lands. On a cold dev server this took several seconds; navigating away early loses the choice | `08-home-ready-empty-graph` |

### Composer

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| Composer | About 700 px wide, bottom-centre, rounded, on a slightly raised surface. The input is a contenteditable, not a textarea. Attach icon and a square send button on the right; the send button turns into a stop square while a reply streams | `09-composer-goal-typed`, `17-…` |
| Under-composer row | Mode toggle (Teach / Practice) on the home screen; in a session, a context meter ("1.5k / 128k · —"); a wide Dictate bar; a model chip; and a thinking-level chip ("off") | `09-…`, `18-…` |
| Model picker | The model chip opens a searchable list of the connected models | `54-composer-model-picker-open` |
| Thinking level | The "off" chip opens a short menu of reasoning efforts | `55-composer-thinking-level-menu` |
| Session stats | Clicking the context meter opens a popover with context used, total cost (dash when unpriced), unpriced-turn count, and a per-turn list (model · provider · steps) | `27-chat-session-stats-popover` |

### Chat and lesson rendering

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| User message | No bubble: a full-width block with a thick left bar, a tiny "YOU" label, and the text in the sans face | `26-chat-research-brief-expanded` |
| Assistant prose | Unboxed prose at full column width in a humanist sans (Source Sans 3 by default, 16 px), light grey on near-black | `18-chat-explanation-code-math-check-collapsed` |
| Headings | Heavy bold sans (default weight 900), noticeably larger than body text | `17-…`, `18-…` |
| Maths | KaTeX: inline maths flows with the text; display maths sits on its own line, left-aligned rather than centred | `18-…`, `21-chat-table-and-display-math` |
| Code blocks | Darker inset panel with a thin border and rounded corners; line numbers in a muted gutter; syntax colours (pink keywords, purple function names, blue numbers, orange keyword arguments, grey comments); a copy button | `18-…` |
| Tables | Bordered grid with maths inside cells and a copy control | `21-…` |
| Blockquote | Italic, muted, with a thin left bar | `18-…` |
| Mermaid in prose | Rendered inline as a light-on-dark flowchart (white outlined boxes, edge labels on grey chips), with copy-source and expand buttons | `22-chat-mermaid-diagram-and-quiz` |
| Agent reasoning | A collapsed "AGENT'S THINKING" disclosure (small caps label plus chevron) above the reply; expanding it shows the reasoning in muted text | `17-…`, `19-chat-agent-thinking-expanded` |
| Streaming | Text arrives in chunks; the newest words fade in from transparent rather than appearing all at once; the sidebar row says "Working…" with a spinner; send becomes stop | `17-chat-streaming-mid-reply`, `video-chat-streaming-dark.webm` |
| Phase dividers | Full-width dotted rules with a centred pill label (PLANNING, TEACHING) mark phase changes | `17-…`, `25-chat-plan-diagram-full` |
| Lesson plan | The teacher's plan renders as a vertical flowchart of outlined boxes joined by arrows, centred in the column | `13-chat-plan-diagram`, `25-chat-plan-diagram-full` |
| Plan panel | The PLAN chip opens a full-pane overlay with the live plan: done nodes bright, future nodes dim, the current node filled amber with a soft glow; zoom −, fit and + buttons; a "Drag to pan · scroll to zoom" hint | `24-chat-plan-panel-progress` |
| Scroll-to-bottom | A round arrow button floats above the composer when the view is not at the bottom | `25-…`, `26-…` |
| Jump to top | The conversation opens with the learner's goal as the first user block | `28-chat-conversation-top` |

### Probes, quizzes and tool activity

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| Probe dock | Questions appear in a floating card docked above the composer, not inline. Inside: a small type label (PROBE / PREDICTION / QUESTION); the question in bold sans; full-width option rows with radio circles (order shuffled); a dashed "I don't know" row; a CONFIDENCE slider (amber, default 50 %); a YOUR THINKING field with a submit hint and a Submit answer button | `10-chat-probe-question-dock`, `11-chat-probe-answer-selected-with-thinking`, `22-…` |
| Free-form question | The same dock without options: a prompt, a muted reassurance line, and an answer field | `12-chat-free-form-question-dock` |
| Collapsed quiz | Mid-lesson quizzes can arrive collapsed as a one-line bar above the composer: a type label, a truncated question preview, and an Answer / Open action. A "THINKING WHILE READING" note field may sit above it | `15-chat-predict-quiz-collapsed-bar`, `18-…` |
| Opened quiz | Opening the bar expands it into the full dock | `16-chat-predict-quiz-dock`, `20-chat-check-quiz-opened` |
| Answered card | After submitting, the card moves inline into the transcript: a green CORRECT or red INCORRECT badge; the chosen row tinted dark green (with a check mark) or dark red (with a cross); the correct row ticked; the confidence bar; a collapsible YOUR THINKING | `17-…`, `23-chat-quiz-answered-incorrect` |
| Probe conclusion | A bordered card ("probe concluded — your assessment") listing each strand as a mono heading with a one-line judgement, then a summary line | `14-chat-probe-assessment-card` |
| Tool / job rows | Background work shows as compact mono lines with a check mark ("RESEARCH AGENT finished", "RESEARCH complete · view brief") rather than cards | `26-…` |
| Research brief | "view brief" expands an inline bordered card labelled RESEARCH BRIEF with the markdown brief; the toggle then reads "hide brief" | `26-chat-research-brief-expanded` |

### Knowledge graph

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| Home = graph | The home screen *is* the graph canvas, with the composer floating over its bottom edge | `35-graph-default-fit` |
| Canvas | Deep navy, softly radial background (lighter in the middle, near-black at the edges); small solid white circular nodes; thin grey edges with small arrowheads | `35-…`, `36-graph-zoomed-out` |
| Joint premises | A chain with several premises draws them meeting at an unmarked junction point, from which one arrow continues to the conclusion | `35-…`, `36-…` |
| Cluster label | The source session's title is written in lavender-blue monospace at the centre of its cluster | `35-…`, `36-…` |
| Labels by zoom | Node names are hidden when zoomed out and appear when zoomed in, as large letter-spaced muted grey monospace under each node | `36-…`, `37-graph-zoomed-in-labels` |
| Default camera | The first view after loading is fairly close in; part of the graph lies off-screen. The camera is remembered in local storage between visits | `35-…` |
| Zoom / pan | Wheel zooms about the cursor (very sensitive); drag pans | `video-graph-interaction-dark.webm` |
| Hover | The hovered node gets a soft glow and its label switches from muted grey to bright, bolder white; there is no tooltip | `38-graph-node-hover` |
| Selection and detail card | Clicking a node gives it a soft white halo, and a dark rounded card appears right below it: bold name, statement in small mono, and a Sessions section linking to the source session | `39-graph-node-selected-detail-card` |
| Search | The "Search nodes…" field filters live, matching statements as well as names; non-matching nodes and their edges disappear | `40-graph-search-step` |
| Top-right controls | Rounded chips: node search, Chains, Extract files…, and a settings cog | `35-…` |
| Chains view | A table overlay on the blurred graph: count header ("11 chains · 15 nodes"), Name / Props / Created columns, one row per source with a checkbox and a chevron | `41-graph-chains-view` |
| Chains expanded | Expanding a source lists its chains as rows of bordered premise chips joined by ∧, then a long ⟹ arrow, then the conclusion chip. Expanding a chain lists its statements beneath in muted mono | `42-graph-chains-view-expanded` |
| Animation | Zoom and pan are smooth and continuous; labels appear as you zoom in. Layout settles quickly, with no visible spring motion once loaded | `video-graph-interaction-dark.webm` |

### Practice

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| Empty state | A plain dark pane: a small bold mono heading ("New practice session") and one muted mono line on where problems can come from, centred; a "Pick nodes" chip sits above the composer's left edge | `43-practice-new-empty-state` |
| Node picker | Reuses the chains table with checkboxes per source; a floating pill at the bottom summarises the selection ("15 nodes → 11 chains") with Clear and Done | `44-practice-node-picker-selected` |
| Selection chip | After Done, a small chip above the composer shows the count ("15 nodes selected") with a Clear link | `45-practice-composer-with-graph-context` |
| Problem card | The problem appears as a bordered card (PROBLEM label, markdown with maths); the dock asks the learner to work aloud, with a "Submit thinking" button | `46-practice-problem-dock` |
| Learner turns | Each submission shows as a muted italic YOUR THINKING block with a left bar, collapsible | `47-practice-hint-reply`, `48-…` |
| Conclusion | A CONCLUDED card with an outcome badge (amber "SOLVED WITH HINTS"), a one-line verdict, then the MODEL SOLUTION in full markdown and maths, and a METADATA disclosure | `48-practice-concluded-solution-revealed`, `49-practice-concluded-bottom-next-control` |

### Settings

| Feature / behaviour | What LearnHarness does | Capture |
|---|---|---|
| Settings dialog | Large modal over a dimmed app with a serif title; a left list of sections and a right pane of label/hint/control rows | `29-settings-providers` … `34-settings-general` |
| Sections | Providers, Agent, Appearance, Graph, Keyboard (stored on this device), General | `29`–`34` |
| Appearance | Mode (Dark only, disabled), background colour, reading font and size, serif session names toggle, heading weight, bold and italic colour overrides | `31-settings-appearance` |
| Graph | Extraction on/off, graph rendering on/off, and the extraction chunk budget (how much teacher prose collects before propositions are extracted) | `32-settings-graph` |

## Colour and typography system (observed)

- **Surfaces**: app background about `#141414` (it is the Appearance default); cards and docks
  slightly lighter (about `#1e1e1e`) with a 1 px grey border (about `#333`) and roughly 12 px
  radius; code panels darker than the page.
- **Graph**: its own navy palette (about `#1a1f2e` at the centre), white nodes, grey edges,
  lavender cluster labels. It is the only place with a hue in the background.
- **Accents** are scarce and carry meaning:
  - amber: confidence slider, the current plan node, the "solved with hints" badge
  - green: correct
  - red: incorrect
  - everything else is greyscale
- **Type**: three families with clear roles.
  - Monospace for all UI chrome: labels, chips, sidebar, status lines, settings hints, graph
    labels. Usually small, uppercase and letter-spaced.
  - Humanist sans (Source Sans 3) for reading.
  - High-contrast serif for dialog titles and the session title.
- **Density**: generous line height in prose; chrome is small and quiet so the lesson text
  dominates.

## Motion

- **Streaming text**: the newest words fade in from transparent rather than appearing all at
  once (video).
- **Spinners**: a ring spinner sits in the phase indicator and in session rows while an agent
  works.
- **Quizzes**: they slide into the dock, then settle inline once answered.
- **Overlays**: the plan panel, chains view and settings appear over a dimmed or blurred
  backdrop.
- **Graph**: zoom and pan are continuous, and labels appear as you zoom in (video).

## Empty states

- **Home before setup**: an empty graph with a centred hint, plus a pill prompting for a
  provider (`01`).
- **Home after connecting a provider**: the pill changes to a model-setup prompt until the
  tiers are saved (seen during exploration; the tier dialog itself is `05`).
- **Sidebar**: shows "No sessions yet." with no sessions (`01`).
- **Practice**: a heading and hint, and the Pick nodes chip (`43`).
- **Search**: a graph search with no hits shows a short "no matching nodes" message. This was
  read from behaviour during exploration and is not captured.

## What could not be captured, and why

- **A real light theme**: the app has none (Mode is fixed to Dark, marked "coming soon"). The
  light files are emulation only.
- **Background agents panel**: with no running jobs, its entry opened nothing visible. The
  research job finished within seconds, too fast to catch a live job list.
- **Image, visual and demo tools** (`show_image`, `visual`, `demo`): not scripted. They need
  image or sandbox assets that I would have had to make up, and the brief did not ask for them.
- **Hosted providers and subscription sign-in**: these need real accounts and keys, which were
  not used by design.
- **Electron shell**: only the web build was run. The desktop wrapper adds a native window
  frame and nothing that appeared to change the UI.
- **Search with no matches**: seen during exploration but not kept as a capture.
- **The answered-card thinking text**: it reads "Why" rather than the typed note in `17` and
  `23`. In the script, the thinking text went into the shared composer rather than the
  quiz's own field. The layout is still accurate.

## Assumptions

- Dev mode (`next dev`) renders the same UI as a production build. The only dev-only element,
  the Next.js badge, is hidden.
- One run at 1440×900 is representative. Responsive and narrow layouts were not explored.
- The scripted model's content and pacing (50 ms per 12-character slice) stand in for a real
  model. Real streams will be burstier.
