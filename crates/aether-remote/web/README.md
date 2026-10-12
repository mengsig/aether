# Aether for the web

A static phone, tablet and desktop client for the daemon's protocol 18 remote carrier. The daemon serves these files directly. There are no runtime packages, transpilation or build steps.

## Development and checks

Node 22 or later is used only for development. From this directory:

```sh
npm ci
PLAYWRIGHT_BROWSERS_PATH="$PWD/node_modules/.browsers" npx playwright install chromium
npm run check
```

`npm run check` verifies the committed precache, strict JSDoc types in browser/Node/service-worker code, Node unit and protocol tests, and phone Chromium integration tests. Stop a running `npm run dev` first: integration tests start a fresh mock on port 4173. Browser profiles, downloads, npm cache, traces and temporary files remain inside this directory. No Cargo invocation is needed. Controls, conversation, graph and delivery-recovery browser regressions run as part of the same command.

```sh
npm run dev
```

Open `http://127.0.0.1:4173/#code=aether-demo`. Use `aether-observer` for observer access. The development server verifies real non-extractable P-256 browser keys, origin-bound signatures and WebSocket request/reply envelopes. It simulates host confirmation after 1.5 seconds. Restart it to reset fixture state. Mock notification delivery is unavailable; the Settings screen says so.

After changing runtime files, run `npm run precache`, inspect and commit `precache.js`, then run `npm run check`. This is a development maintenance command, never a build/install prerequisite for Aether. The checked-in cache version hashes the runtime source content.

## Real-daemon acceptance

```sh
npm run test:live -- /absolute/path/to/aether
```

Use a binary built from this checkout: the test exercises its embedded web assets, not a development proxy. The command starts a disposable daemon and the repository's streaming mock provider, creates a Git workspace, pairs operator and observer browsers through real P-256 authentication, and runs root creation, delegation, streaming, file completion, slash controls, approvals, questions, schedules, memories, artifacts, reports, revocation, restart and offline recovery. It drops a committed send's acknowledgement and checks that receipt lookup confirms exactly one delivery without an operator prompt. It also checks pairing validation, rate limiting, and the actual CSP/WebSocket connection at both loopback origins. The mock provider adds one deterministic `ask_user` prompt and a second model entry for model handoff.

Python 3, Git and the local Chromium installation are required. Configuration, data and the short Unix socket live under a unique `/var/tmp/aw-*` directory, removed on completion; browser temporary files remain in `node_modules/.live-tmp`. Logs and both-theme captures go to `test-results/live/`. This command never builds Rust. Real push delivery and physical iOS installation remain separate device checks: the remote endpoint policy intentionally refuses loopback/private push services.

Repository `scripts/ci.sh` runs the web type checks and units when Node/npm are present. Missing local Chromium skips browser checks explicitly; a browser that is installed but fails is an error. With Chromium, Python and the CI binary, the same lane runs this real-daemon suite after the mock browser suite. `RUN_CONTROLS_BROWSER=0 npm run test:unit` runs only nonbrowser units.

## Runtime layout

- `boot.js` and `app.js`: startup failures, route lifetime, shell integration and service-worker registration.
- `ui/`: DOM primitives, native dialogs and focus restoration, navigation, appearance, notification recovery and safe deep links.
- `core/`: device-key IndexedDB, protocol and host-service boundary, socket correlation, reconnect, event projection, durable mutation receipts and drafts.
- `features/fleet/`: bounded SVG delegation graph with pan/pinch/zoom and agent sheets, compact accessible list, exact decision review, conditional questions, fleet-state ring, ranked usage charts and bounded event history.
- `features/conversation/`: structural Markdown rendering, safe links, code/diff/tool cards, variable-height transcript virtualization, search, streaming and composer.
- `features/controls/`: operator commands, existing-agent and fresh-run schedules, memories, artifacts, reports, approval rules, settings and uncertain-delivery review.
- `features/start/`: pairing phrase, platform install guidance, workspace completion/validation and agent/task creation.
- `sw.js`: versioned offline shell, same-instance push display and safe notification actions. `push-renewal.js` re-authenticates when a browser subscription changes.

The DOM is built structurally; agent text never becomes HTML. External links accept safe schemes and use `noopener`/`noreferrer`. There are no remote fonts or CDN assets. Colors follow system appearance unless overridden. Touch targets are at least 44px, layouts tolerate larger text, and motion honors system or explicit reduced-motion preference.

The transcript uses measured row heights and a Fenwick index, rendering a viewport-sized window. Streaming updates are scheduled with animation frames and keep historical nodes stable. Usage charts report observed per-agent totals with explicit unknown and partial coverage; they do not invent historical usage or costs.

The browser suite also loads 5,000 messages through the authenticated mock socket, samples 120 scrolling frames, searches the full transcript and checks that a concurrent stream preserves the current reading position. Its output is `test-results/transcript-performance.json` and `test-results/screenshots/conversation-5000.png`. Timing is diagnostic for the test host, not a physical-phone performance guarantee.

## Contract dependencies

The source of truth is repository `docs/remote.md` (protocol 18). See `core/CONTRACT.md` for the detailed transport and storage behavior.

1. `core/identity.js`, `contract.js`, `permissions.js` and `host.js`: P-256 raw public key and P1363 signature encodings, pairing claim/status bodies and phrase, daemon instance pinning, origin-bound challenge input, remote command names and authorization matrix.
2. `core/transport.js`: `/ws`, `aether` subprotocol, protocol version and welcome identity, `req`/`id` Rust Result envelopes, heartbeat and close codes, commands accepting idempotency keys.
3. `core/store.js`, `normalize.js`, `events.js`, `receipts.js`: snapshot shapes, `get_attention.last_seq`, subscription cursor/replay/full resync, committed-event versus transient-delta semantics, `submissions.evicted` warnings and redacted `remote.restricted` events. `core/recovery.js` and `submissions.js` depend on the optional Welcome horizon and `resolve_submission` / `submission_resolved` outcomes.
4. `features/start/models.js` and `workspace.js`: `create_agent`/`create_task`, `ModelRef`, effort enum, `ApprovalPolicy.require_for` side-effect classes and canonical `validate_workspace.path`.
5. `features/controls/protocol.js`, `agent.js`, `schedules.js`, `resources.js`, `memories.js`, `rules.js`: existing daemon command/record shapes for each operator surface.
6. `features/fleet/model.js`: `resolve_approval` exact request ID/decision and `reply_to_question` exact question sequence.
7. `features/conversation/cards.js`, `state.js`, `inbox.js` and `core/actors.js`: transcript roles/content blocks, tool inputs/results and invocation association. Resolved inbox deliveries render the optional deciding actor. File mentions call the host-service adapter, preserving truncated results.
8. `ui/push.js`, `push-renewal.js`, and `features/controls/notifications.js`: version-1 instance-bound push payload and required `url` fragment, `get_push_config`, VAPID key and flattened subscription keys. Actions open a review screen; they do not approve work from a notification.

## Scope and remaining platform validation

Provider login/logout, remote-access administration, global approval-rule changes, host paths/configuration and daemon shutdown remain host-only where the protocol requires it. Settings shows keyless paired devices to operators and provides host commands for administration. Observers can manage their own notifications but cannot change agent work. Platform keyboard dictation replaces the TUI's local voice-engine controls. Terminal-specific custom view modes and keybindings are not mirrored; the browser uses its touch layout and expandable transcript details.

Eighteen operations accept idempotency keys; the app sends keys on all eighteen. The remaining protocol mutations do not accept the field. Every page mutation is recorded before transmission. The first send retains its exact fields and opaque submission horizon. After reconnecting to the same daemon, receipt lookup confirms executed requests without resending; only an explicit `absent_safe` result permits automatic resend with the original key. Unsupported lookup, a forgotten receipt or a lookup error leaves delivery unconfirmed, with explicit mark-delivered, send-again and discard choices. Once unconfirmed, a later `absent_safe` never triggers an automatic resend; a returned execution receipt can still settle it. Lookup replies update the horizon only for later requests; the client never compares clocks. Manual retries preserve the original fields and key but can repeat work after receipt eviction, so they require inspection and confirmation.

Background service-worker push renewal uses an authenticated subscription upsert and checks its acknowledgement. It cannot access the page's localStorage ledger; startup reconciles notification state with `get_push_config`. The protocol does not provide idempotency keys or submission lookup for push subscription commands.

The phone browser tests use Chromium. Real iOS Home Screen installation, VoiceOver/TalkBack interaction and delivery through Apple/Google push services still require device acceptance testing. Native PWA manifest/icons and portrait startup images are included. Artifact preview/download reports the protocol's bounded/truncated response rather than pretending a partial download is complete. Charts cover current observed totals; history is not synthesized.

## Test output and embedding

Primary light/dark screenshots are written to `test-results/screenshots/{fleet,attention,activity,settings,new,agent,controls,pair}-{light,dark}.png`; viewport-only versions use `-viewport.png`. Controls-tab screenshots use `controls-{schedules,memories,artifacts,runs,diagnostics,rules}-{light,dark}.png`. Responsive captures are `fleet-768.png`, `fleet-1280.png` and `fleet-large-text.png`. These outputs and failure traces are ignored by Git. `playwright-report/index.html` contains the test report.

The real-daemon suite writes `test-results/live/screenshots/{fleet,attention,activity,new,settings,agent,controls}-{light,dark}.png` and `controls-{schedules,memories,artifacts,runs,diagnostics,rules}-{light,dark}.png`. Real fleets of 1, 10 and 60 agents are captured as `fleet-{1,10,60}-{light,dark}.png`. Local redesign comparisons use `test-results/design-comparison/{before,after-mock,after-live}/{fleet,agent,attention,new,activity}-{light,dark}.png`; the original baseline remains in `node_modules/design-before/screenshots/`. Layout regressions verify primary actions remain reachable above navigation at 390×844, 360×640 and 320×844 with 200% text.

Resolved approval provenance is captured in `test-results/screenshots/agent-inbox-{light,dark}.png`.

Embed only runtime files: the top-level HTML/JS/CSS/manifest, `assets/`, `ui/`, `core/` and `features/`, excluding `*.test.js` and documentation. Never embed `node_modules/`, `dev/`, `tests/`, `scripts/`, test output, reports, package manifests or type-check configuration. `precache.js` enumerates the runtime module/assets set; `sw.js` and `precache.js` are served in addition to that set.
