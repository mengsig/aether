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
