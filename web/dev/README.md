# web/dev

Development checks that need more than the unit tests: a real daemon, a browser, or both.

## Live acceptance of the served frontend

```sh
npm ci
npx playwright install chromium   # once, for the browser half
node dev/live/run.ts /absolute/path/to/aether
```

Give it a binary built from this checkout: it checks what that binary embeds, and never builds
Rust. It starts a disposable daemon with its config, data and socket under a new `/var/tmp/aw-*`
directory, removed afterwards, and turns its remote listener on at a free loopback port. Then:

- `bundle.ts`: every file of `web/dist/` is served byte for byte with the listener's CSP, `/pair`
  is the app shell, and the retired phone app's paths are `404`;
- `pairing.ts`: the claim endpoint refuses invisible and over-long device names and rate-limits
  wrong codes, and a browser opens the printed pairing URL (`/pair#code=…`) at both loopback
  origins, `127.0.0.1` and `localhost`. The code is filled in and leaves the address bar, the host
  confirms, and the socket's welcome names the new device. A CSP violation fails the run.

Without Chromium the browser half is skipped with a yellow banner, and the rest still runs.
`scripts/ci.sh binary` runs this against the release binary.

The retired app's other suites drove its own screens and its protocol 18 mock daemon, so they did
not move: the new app's unit tests cover its screens with in-memory transports.

## Archon task graph frame bench

```sh
npx playwright install chromium   # once
node dev/archon-dag-bench.ts [out.json]
```

It serves the C2 fixture (`packages/archon/src/dag/fixture`) with Vite, opens 2,000 items over its
faux archond in headless Chromium, and drives real wheel and drag input one step a frame: 90 zoom-in
steps, 180 drag steps and 90 zoom-out steps. It reports each rAF callback's time (the renderer draws
in its rAF) and the intervals between frames, and fails when the p95 callback exceeds 16.7 ms.

## Memory screen screenshots

```sh
npx playwright install chromium   # once
node dev/memory-capture.ts <out-dir> [prefix] [--flows]
```

It serves the memory fixture (`packages/aether/src/memory/fixture`) with Vite: the memory screen in
the real shell over the scripted daemon and the Memory 2.0 demo data, never a real daemon. It writes
full-page screenshots at a desktop and a 390 px phone width. `--flows` adds the retrieval explain, the
import and consolidate dry runs, the dark theme, an observer and a daemon without `memory_v2`.

## Learn screenshots

`node dev/learn-capture.ts <out-dir>` serves the Learn fixture (`packages/noesis/src/learn/fixture`)
with Vite over its faux Noesis daemon and writes full-page screenshots in the light and dark themes:
the sessions home, a graded lesson, an open question, the settings sheet and the Knowledge view.

## Shared Markdown security and rendering

`node dev/markdown-check.ts` builds the UI Markdown fixture and opens it in Chromium under
production's CSP. It checks actual Mermaid flowchart/sequence layout, native MathML,
source fallback, malicious labels and zero external requests in both themes. Screenshots
and the isolated bundle go to ignored `web/e2e/.out/markdown/`. Run through the host CI slot
from the repository root: `nice -n 19 ionice -c3 sh scripts/ci_slot.sh sh -c 'cd web && node dev/markdown-check.ts'`.
