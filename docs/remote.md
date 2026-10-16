# Remote control

Aether's daemon can be driven from two places besides the terminal it runs
in:

- a **phone or browser**, through a web app the daemon serves itself and a
  WebSocket that carries the ordinary daemon protocol;
- **another computer over SSH**, with the normal `aether` CLI and TUI.

This page is the reference for both: the HTTP endpoints, the WebSocket
handshake, pairing, authorization, the commands added for remote clients,
push notifications, configuration and reconnect rules. The daemon protocol
itself is described in [protocol.md](protocol.md); everything here builds on
it and changes none of its shapes. Remote control is protocol version **18**.

Types named in `code` are in `clients/protocol.schema.json` and the
generated Python and TypeScript libraries, the HTTP bodies and the push
payload included.

## Contents

1. [Model](#model)
2. [Configuration](#configuration)
3. [HTTP endpoints](#http-endpoints)
4. [WebSocket carrier](#websocket-carrier)
5. [Device identity and the handshake](#device-identity-and-the-handshake)
6. [Pairing](#pairing)
7. [Authorization](#authorization)
8. [Commands added in protocol 18](#commands-added-in-protocol-18)
9. [Push notifications](#push-notifications)
10. [Reliability: idempotency, reconnect and resync](#reliability-idempotency-reconnect-and-resync)
11. [Error codes](#error-codes)
12. [SSH remotes](#ssh-remotes)
13. [Command line](#command-line)

## Model

```
phone ──https──▶ tailscale serve ──http──▶ 127.0.0.1:8377 (remote listener)
                                             ├─ GET  /…            embedded web app
                                             ├─ GET  /api/health
                                             ├─ POST /api/pair/*   pairing bootstrap
                                             └─ GET  /ws           daemon protocol

laptop ──ssh──▶ aether bridge --stdio ──▶ aetherd.sock             daemon protocol
```

- The remote listener speaks plain HTTP/1.1 and binds to loopback. TLS and
  the public name come from `tailscale serve` or another reverse proxy on the
  same machine. Aether terminates no TLS. [remote-setup.md](remote-setup.md) is the
  step-by-step guide for it.
- The listener is **off** until `aether remote enable`. Nothing is reachable
  from the network on a fresh install.
- There is no REST API. Every operation is a daemon protocol command sent
  over the WebSocket. HTTP exists only for what cannot ride the socket:
  serving the app, a health probe, and the two pairing calls a device makes
  before it has an identity.
- A browser is a **device**: it holds a non-extractable signing key and the
  daemon stores the public half. There is no shared token, cookie or bearer
  secret, and nothing in browser storage that could be copied to another
  machine.
- A device has a **role**: `operator` (everything an operator at the host can
  do, minus a host-only set) or `observer` (read-only).
- An SSH remote is not a device. It reaches the daemon's Unix socket as the
  logged-in user and has exactly the standing a local client has.
- The `[listen]` TCP transport with its shared token is unchanged. A token
  connection is never a device, and cannot administer remote access.

## Configuration

Two layers. Static settings are in `config.toml` and are read when the daemon
starts. Runtime settings live in the daemon's store and change at once
through `aether remote …` (the `configure_remote` command); they are never
edited by hand and need no restart.

### `[remote]` in config.toml

```toml
[remote]
address = "127.0.0.1:8377"   # where the listener binds when enabled
allow_remote = false         # required to bind outside loopback
max_connections = 32         # device sessions open at once, and HTTP connections served at once
idle_timeout_secs = 300      # a silent WebSocket is closed after this long
push_previews = false        # allow a short content preview in push payloads
push_subject = ""            # VAPID `sub`; empty derives it from the first public origin
push_hosts = []              # push services a device may name, beyond the browsers' own
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `address` | string `host:port` | `127.0.0.1:8377` | Bind address. Must parse as a socket address. |
| `allow_remote` | bool | `false` | Bind a non-loopback address. Without it a non-loopback `address` is refused when the configuration loads. Mirrors `[listen] allow_remote`. |
| `max_connections` | integer ≥ 1 | `32` | Two caps of this size, and a third derived from it: see [Connection limits](#connection-limits). Device sessions, which are WebSockets whose hello was accepted; HTTP connections being served; and, at `min(max_connections, 8)`, WebSockets still waiting for their hello to be accepted. |
| `idle_timeout_secs` | integer ≥ 3 | `300` | Reported in `Welcome.idle_timeout_secs`. |
| `push_previews` | bool | `false` | When `false`, a push payload never contains transcript, question or tool content. |
| `push_subject` | string | `""` | A `mailto:` or `https://` URL. Empty means the first public origin; with no public origin either, the token carries no `sub`. |
| `push_hosts` | string[] | `[]` | Extra push service hosts, each an exact host name or `*.suffix`. See [which push services](#which-push-services). A malformed entry is refused when the daemon starts. |

Unknown keys are refused, as in every other section.

### Runtime settings (`RemoteSettings`)

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | The listener is open. |
| `pairing` | bool | `false` | `start_pairing` may open a pairing. |
| `public_origins` | string[] | `[]` | Exact origins browsers use, e.g. `https://box.tail1234.ts.net`. The Host and Origin allowlist. Each is `https://host[:port]` with no path, query or trailing slash; at most 16. |
| `push` | bool | `false` | The daemon sends Web Push notifications. |

Enabling with no public origin is allowed and reachable only on loopback
(`http://127.0.0.1:8377`, `http://localhost:8377`), which browsers treat as a
secure context.

`configure_remote` accepts an origin only as a browser writes it in the
`Origin` header, so the two compare byte for byte: host in lower case, no
default port `443`. An origin that is not `https`, carries a path, query,
fragment or credentials, or is not in that form is `invalid_argument` (the
message names the form to use) and nothing changes. Duplicates are dropped.

The settings are stored under the `remote.settings` key of the daemon's
store with their revision, and are applied again when the daemon starts,
before it answers its first connection. If the stored settings cannot open
the listener then (the address is in use, say), the daemon still starts:
`get_remote` carries `error`, and the next accepted `configure_remote` tries
again. Likewise a VAPID key that cannot be read at start leaves the daemon
running with nothing sent, and `get_push_config` reports why.

## HTTP endpoints

All responses carry the [security headers](#response-headers). All JSON
bodies are UTF-8 `application/json`. Request bodies are at most 16 KiB.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/health` | Liveness and protocol window. |
| `POST` | `/api/pair/claim` | A device presents a pairing code with its public key. |
| `POST` | `/api/pair/status` | The claiming device waits for the host's decision. |
| `GET` | `/ws` | WebSocket upgrade; the daemon protocol. |
| `GET`, `HEAD` | anything else not under `/api/` | The embedded web app. |

### Request checks, in order

1. **Host.** There must be exactly one `Host` header, and it must be the
   `host[:port]` of a configured public origin, or `127.0.0.1`, `localhost`
   or `[::1]` with the bound port. A request target in absolute form must
   name the same authority. Otherwise `403 forbidden_host`. This stops DNS
   rebinding.
2. **Route and method.** Unknown `/api/` path: `404 not_found`. Wrong method:
   `405 method_not_allowed`. A path with an empty, `.` or `..` segment, a
   backslash or a percent-escape: `400 bad_request`.
3. **Origin**, on `/ws` and every `POST`: exactly one `Origin` header must
   be present and equal to a configured public origin or a loopback origin
   of the bound port. If `Sec-Fetch-Site` is present it must be
   `same-origin`. Otherwise `403 forbidden_origin`. This stops cross-site
   WebSocket hijacking.
4. **Body**, on `POST`: `Content-Type: application/json`
   (`415 unsupported_media_type`), at most 16 KiB (`413 payload_too_large`),
   valid JSON of the documented shape with no unknown fields
   (`400 bad_request`).

A request whose head is not complete within 10 seconds, or is larger than
32 KiB, is dropped by the HTTP layer before any of these checks; so is a
body that is not complete within 10 seconds (`400 bad_request`). A
connection whose peer takes none of a response for 10 seconds is dropped
too; a slow link that keeps taking bytes is not.

### Connection limits

Nothing on this listener is authenticated until a WebSocket's hello is
accepted, so everything before that point is kept apart from the device
sessions it could otherwise crowd out. There are three budgets:

| Budget | Size | Held | When it is full |
| --- | --- | --- | --- |
| HTTP connections | `max_connections` | From accept until the connection ends or upgrades. | A new connection takes the place of the one that has been open longest, which is dropped. |
| WebSockets not yet greeted | `min(max_connections, 8)` | From the `101` until the hello is accepted or the socket closes. | A new upgrade takes the place of the socket that has waited longest, which is closed with `4408`. |
| Device sessions | `max_connections` | From an accepted hello until the socket closes. | The hello is answered `unavailable` and the socket closed with `1000`; the device keeps its pairing and reconnects. |

A place in the first two is safe from a newcomer for 500 ms. While every
occupant is younger than that, a new HTTP connection waits in the kernel's
backlog and a new upgrade is answered `503 unavailable`. Half a second is
enough for a request or a hello and too little to hold a place by sitting
on it: connections that send nothing no longer keep others out for the 10
seconds their deadlines allow.

A place is taken from an occupant only for a connection that has been accepted.
With no one waiting, a full set of HTTP connections is left alone however
slow they are: even with `max_connections = 1`, a slow request is cut only
once another peer is actually there to take its place.

No peer without an enrolled key can take or hold a device session, and an
open session is never displaced. What remains is that a peer able to reach
the listener and willing to keep opening connections faster than 16
upgrades or 64 connections a second can delay *new* connections while it
does so. Aether does not rate-limit by address, because behind a reverse
proxy every peer has the same one; that belongs to the proxy.

### Errors

Every non-2xx response has this body (`HttpError`):

```json
{"error": {"code": "invalid_code", "message": "that code is not open for pairing"}}
```

| `code` | Status | When |
| --- | --- | --- |
| `bad_request` | 400 | Malformed path, JSON, field or public key; a `/ws` request that is not a valid upgrade. |
| `forbidden_host` | 403 | `Host` is not this listener. |
| `forbidden_origin` | 403 | `Origin` missing or not allowlisted. |
| `pairing_closed` | 403 | No pairing is open, or pairing is disabled. |
| `invalid_code` | 403 | The code is wrong or already claimed. |
| `not_found` | 404 | Unknown endpoint, asset or pairing id. |
| `method_not_allowed` | 405 | |
| `expired` | 410 | The pairing expired. |
| `payload_too_large` | 413 | |
| `unsupported_media_type` | 415 | |
| `rate_limited` | 429 | Too many codes were tried this second; `Retry-After` is set. |
| `internal` | 500 | |
| `unavailable` | 503 | The daemon is shutting down, or every place for a WebSocket that has not greeted yet was taken within the last 500 ms. |

### `GET /api/health`

No authentication. Response `200` (`Health`):

```json
{"status": "ok", "protocol_version": 18, "min_protocol_version": 17, "pairing_open": false}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `status` | string | Always `"ok"`. |
| `protocol_version` | integer | Newest protocol the daemon speaks. |
| `min_protocol_version` | integer | Oldest it serves. |
| `pairing_open` | bool | A pairing is waiting to be claimed. |

### `POST /api/pair/claim`

Request (`PairClaimRequest`):

```json
{"code": "7K2M-9QXD", "public_key": "BHq…43 more…", "device_name": "Robin's iPhone"}
```

| Field | Type | Rule |
| --- | --- | --- |
| `code` | string | The pairing code. Case-insensitive; dashes and spaces are ignored. |
| `public_key` | string | The device's raw P-256 public key: the 65-byte uncompressed point `0x04 ‖ X ‖ Y`, base64url without padding (87 characters). A point that is not on the curve is `400 bad_request`. |
| `device_name` | string | 1 to 64 characters once normalised; see [Device names](#device-names). |

Response `200` (`PairClaimResponse`):

```json
{"pairing": "01JABCDEFGHJKMNPQRSTVWXYZ0", "fingerprint": "9f2c…64 hex…", "phrase": "amber-otter-canyon", "expires_at": "2026-10-12T09:32:00Z"}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `pairing` | string | Names this attempt in `/api/pair/status`. Known only to the claimant. |
| `fingerprint` | string | Lowercase hex SHA-256 of the 65 raw public-key bytes. |
| `phrase` | string | Three lowercase words joined by `-`, chosen by the daemon. Display it verbatim; the operator compares it with the host's. |
| `expires_at` | RFC 3339 | When the host's confirmation window closes. |

A code is single-use: the first valid claim consumes it, and every later
claim of the same code is `invalid_code`.

#### Device names

The host reads a device's name to decide whether to enrol it, so a name must
show as what it is. The same rule applies when a device is renamed.

- Leading and trailing whitespace is dropped, and every run of whitespace of
  any kind (no-break, ideographic, thin, line and paragraph separators) becomes
  one plain space. The result is what is stored and shown.
- The result is 1 to 64 characters.
- A name is refused (`400 bad_request`; `invalid_argument` on a rename) if it
  holds a control character, including tab and newline; a Unicode format
  character, which covers the bidirectional marks, embeddings, overrides and
  isolates, the zero-width space and joiners, the soft hyphen and the byte
  order mark; any other code point that displays as nothing (variation
  selectors, Hangul fillers, the blank braille pattern, tag characters); or a
  private-use or noncharacter code point.

Emoji written with a joiner or a variation selector are refused with the
rest; a plain emoji is a visible character like any other.

A claim is judged in this order: the shape of the request (`400`), whether a
pairing exists (`pairing_closed`) and has expired (`expired`, whatever the
code), whether the code is eight Crockford symbols at all (`invalid_code`,
answered at once), the rate limit (`429`), and only then the code itself
(`invalid_code`, after the delay). Only a claim that reaches the last step is
a guess: nothing refused earlier counts against the rate limit.

### `POST /api/pair/status`

Request (`PairStatusRequest`): `{"pairing": "01JABC…"}`.

Response `200` (`PairStatusResponse`):

```json
{"state": "enrolled", "device": {"id": "dev_01JABCDEFGHJKMNPQRSTVWXYZ0", "name": "Robin's iPhone", "role": "operator"}}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `state` | `claimed` \| `enrolled` \| `rejected` \| `expired` \| `cancelled` | `claimed` means still waiting for the host. |
| `device` | `DeviceIdentity`, optional | Present once `enrolled`. `device.id` is what the device authenticates as from then on. |

Poll once a second while `claimed`. An unknown `pairing` is `404 not_found`,
as is the id of a pairing nobody has claimed: the id is learned only from a
successful claim.

### Assets

`GET` or `HEAD` of any other path serves the embedded web app:

- an exact file match is served with its `Content-Type`;
- a path whose last segment has no `.` (an app route such as `/pair`) is
  served `/index.html`;
- anything else is `404 not_found`.

Every asset carries a strong `ETag` (the quoted SHA-256 of the file), and
`If-None-Match` is answered with `304`. Caching is keyed by that hash:

- `GET /path?v=<hash>`, where `<hash>` is the file's SHA-256 in lowercase
  hex, is served with `Cache-Control: public, max-age=31536000, immutable`:
  a URL that names its content never changes.
- Any other URL, including one whose `v` is stale, is served with
  `Cache-Control: no-cache`, so the browser revalidates with the `ETag`.
- `/index.html` (and so every app route), `/sw.js`, `/service-worker.js` and
  `/asset-manifest.json` are always `no-cache`, whatever the query. A stale
  service worker would pin the app to an old release.

`GET /asset-manifest.json` is generated by the daemon: one JSON object
mapping every embedded path to its hash, `{"/index.html": "9f2c…", …}`. A
service worker reads it to precache `path?v=hash` without a build step.
Responses that are not assets carry `Cache-Control: no-store`.

The app's source is `crates/aether-remote/web/`. Its runtime files are
compiled into the binary as written: plain static files and ES modules, no
bundler, no Node. `web/index.html` must exist, and is all that must.

What is embedded: at the top of `web/`, `index.html`,
`manifest.webmanifest` and every `*.js` and `*.css`; and everything under
`assets/`, `ui/`, `core/` and `features/`. Never embedded, at any depth:
the directories `dev/`, `tests/`, `scripts/`, `node_modules/`,
`test-results/` and `playwright-report/`; `*.test.js`; `*.md`; hidden files
and directories; and package and tool files (`package.json`, lockfiles,
`tsconfig.json`, `jsconfig.json`, `*.config.*`). The build reruns when a
file under `web/` is added, removed or changed.

### Response headers

On every response, including errors and `304`:

```
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self';
    img-src 'self' data:; font-src 'self'; manifest-src 'self'; worker-src 'self';
    connect-src 'self'; base-uri 'none'; form-action 'none';
    frame-ancestors 'none'
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
Referrer-Policy: no-referrer
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Resource-Policy: same-origin
Permissions-Policy: camera=(), geolocation=(), microphone=(self)
```

The policy allows no inline script or style, no `eval`, and no third-party
origin of any kind. The web app must load everything from its own origin.
The policy is the same on every origin and names no host. The page and
`/ws` share an origin, and `'self'` in `connect-src` covers that origin's
`ws:` and `wss:` forms: `wss:` from an `https:` page, `ws:` from the
loopback `http:` page, whether it was opened as `127.0.0.1`, `localhost` or
`[::1]`. This is how CSP Level 3 defines `'self'`, and Chromium-based
browsers, Firefox 55 and later and Safari 16 and later implement it. An
older browser blocks the socket; no host source is added for it, because an
IPv6 literal such as `ws://[::1]:8377` is not a valid source and makes a
browser report the policy as malformed.

## WebSocket carrier

`GET /ws` with `Upgrade: websocket`, `Sec-WebSocket-Version: 13` and
`Sec-WebSocket-Protocol: aether`. The daemon answers `101` selecting
`aether`. A request without that subprotocol is `400 bad_request`. When no
place is free for a socket that has not greeted yet, the answer is
`503 unavailable`: see [Connection limits](#connection-limits).

The carrier is the daemon protocol with one change of framing:

- **One JSON frame per text message.** A client message is exactly one
  `ClientFrame` (`{"req": 7, "cmd": "get_tree", …}`); a daemon message is
  exactly one `ServerFrame`. Messages are compact JSON and contain no literal
  newline; a trailing newline is tolerated.
- **Text only.** A binary message closes the connection with `1003`, as
  does a text message that is not UTF-8 or has a newline inside it.
- **Size.** A message is at most 64 KiB until the hello is accepted, and
  64 MiB afterwards. Larger closes with `1009`. Before the hello the limit
  is enforced on every frame header before the WebSocket parser can reserve
  its announced payload. Before authentication, each data-frame reservation
  is bounded by 64 KiB, independent of the 64 MiB authenticated cap; the
  parser starts with a 16 KiB read buffer. Fragmented data shares the message
  budget. In both phases, control frames must be final and at most 125 bytes;
  reserved opcodes and RSV bits are refused at the header with `1002`.
- **Keepalive.** The daemon sends a WebSocket ping every 30 seconds; browsers
  answer on their own. A peer from which nothing at all arrives for 90
  seconds, pongs included, is closed with `4408`. Separately, a connection
  that sends no *frame* for `Welcome.idle_timeout_secs` is closed, so a
  client that only listens sends the `ping` command more often than that.
- **Backpressure.** The daemon-to-carrier pipe holds at most 256 KiB.
  Framing one outbound message can additionally hold its payload (up to
  64 MiB) and the WebSocket write buffer. A peer that takes nothing for
  30 seconds while frames wait is closed with `1013`; a slow link that keeps making progress is not.
- **Subscriptions.** Every connection, including Unix socket, TCP and SSH
  clients, may hold at most 16 event subscriptions. Excess `subscribe`
  requests receive `unavailable` before a receiver or task is allocated.
  Unsubscribe waits for cleanup and frees a slot; a task that ends itself
  also frees its slot. The TUI and web app each normally use one.
- Everything else, including request ids, replies, subscriptions, event and
  delta frames, is exactly as on the Unix socket.

### Close codes

| Code | Meaning | Client reaction |
| --- | --- | --- |
| `1000` | Normal close. | Reconnect if the app is still open. |
| `1001` | The daemon is shutting down. | Reconnect with backoff. |
| `1002` | Invalid control frame, reserved opcode or RSV bits. | Fix the client; extensions are not negotiated. |
| `1003` | A binary message, invalid UTF-8, or a newline inside a message. | A client bug. |
| `1009` | A message exceeded the size limit. | A client bug. |
| `1011` | The daemon failed to serve the connection. | Reconnect with backoff. |
| `1013` | The client did not keep up with what it subscribed to. | Reconnect and resync. |
| `4401` | The hello was refused. | Do not retry; the device must pair again. |
| `4403` | The device was revoked, or its role changed, while connected. A pending outbound frame instead causes immediate transport closure. | Reconnect once: a role change succeeds with the new role, a revocation is refused with `4401`. |
| `4408` | No accepted hello within 10 seconds, or before a newer connection needed the place; idle past the timeout; or pings unanswered for 90 seconds. | Reconnect. |
| `4503` | The remote listener was disabled, or the origin the connection arrived by was removed from `public_origins`. | Reconnect with the longest backoff. |

Before a close the daemon sends, where it can, a reply frame with `id` 0
carrying the reason as an `ErrorReply`, as it does on the socket. A client
that closes first has its close frame echoed.

## Device identity and the handshake

### The key

A device generates its key once, with WebCrypto:

```js
const pair = await crypto.subtle.generateKey(
  { name: "ECDSA", namedCurve: "P-256" }, /* extractable */ false, ["sign"]);
const raw = await crypto.subtle.exportKey("raw", pair.publicKey);   // 65 bytes
```

The `CryptoKeyPair` is stored in IndexedDB, where a non-extractable key stays
non-extractable. The page and the service worker share that database, so
both can authenticate. The key never leaves the browser and cannot be read by
script; the daemon only ever sees `raw`.

Encodings, everywhere in this document:

- **base64url** means RFC 4648 §5 without `=` padding.
- A **public key** is the 65-byte uncompressed point, base64url (87 chars).
- A **signature** is the 64-byte fixed-width `r ‖ s` (IEEE P1363, what
  `crypto.subtle.sign` returns), base64url (86 chars). DER is refused.
- A **fingerprint** is the lowercase hex SHA-256 of the 65 public-key bytes.
  Interfaces show its first 16 characters in groups of four.

### The handshake

On this carrier the daemon speaks first.

```
client                                           daemon
  │ ── GET /ws (Upgrade) ─────────────────────────▶ │  Host and Origin checked
  │ ◀──────────────────────────── 101 Switching ── │
  │ ◀── {"frame":"challenge","nonce":…,"instance_id":…}
  │ ── {"req":1,"cmd":"hello",…,"device":{"id":…,"signature":…}} ▶
  │ ◀── {"frame":"reply","id":1,"result":{"Ok":{"reply":"welcome","data":{…}}}}
  │                 … ordinary protocol …
```

**1. Challenge** (`ServerFrame`, `frame: "challenge"`), sent as soon as the
socket opens:

```json
{"frame": "challenge", "nonce": "q83vEjRWeJCrze8SNFZ4kKvN7xI0VniQq83vEjRWeJA", "instance_id": "01JABCDEFGHJKMNPQRSTVWXYZ0"}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `nonce` | string | 32 random bytes, base64url (43 chars). Valid for this connection only, and for one hello. |
| `instance_id` | string | The daemon's store identity; the same value `Welcome.instance_id` carries. |

**2. Hello** (`Command`, `cmd: "hello"`), within 10 seconds:

```json
{"req": 1, "cmd": "hello", "protocol_version": 18, "client_name": "aether-web", "client_version": "1.0.1",
 "features": ["deltas", "artifact_bytes"],
 "device": {"id": "dev_01JABCDEFGHJKMNPQRSTVWXYZ0", "signature": "kB3x…86 chars…"}}
```

`device` (`DeviceProof`) is new in protocol 18:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `DeviceId` (`dev_` + ULID) | From `PairStatusResponse.device.id`. |
| `signature` | string | ECDSA P-256 with SHA-256 over the signing input below, base64url `r ‖ s`. |

`token` is ignored on this carrier. `client_name` is `aether-web` for a page
and `aether-web-sw` for a service worker; it is informational.

**The signing input** is the UTF-8 encoding of five fields joined by a
single line feed (`0x0A`), with no trailing line feed:

```
aether-remote-auth-v1
<instance_id>
<origin>
<device id>
<nonce>
```

- `instance_id` and `nonce` are the strings from the challenge, byte for byte.
- `origin` is the page's origin as the browser serializes it:
  `location.origin` in a page, `self.location.origin` in a service worker
  (for example `https://box.tail1234.ts.net`, no trailing slash). The daemon
  uses the `Origin` header it allowlisted for this connection.
- `device id` is `device.id` from the hello.

```js
const input = ["aether-remote-auth-v1", challenge.instance_id, location.origin,
               deviceId, challenge.nonce].join("\n");
const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" },
                                     pair.privateKey, new TextEncoder().encode(input));
// signature = base64url(new Uint8Array(sig))
```

Binding the origin means a signature made for one origin is useless at
another; binding the nonce means it is useless on another connection.

**3. Welcome.** `Welcome` gains two optional fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `device` | `DeviceIdentity`, optional | `{id, name, role}`: who this connection is. Absent on the Unix socket and the token listener. |
| `submission_horizon` | string, optional | An opaque mark of how many idempotency receipts the host had recorded at this handshake. Present on every protocol 18 connection, whatever the transport; its presence is how a client knows the host answers [`resolve_submission`](#resolve_submission). Kept with each keyed command first sent on this connection: see [resolving a lost acknowledgement](#resolving-a-lost-acknowledgement). |

The client reads its role from here and hides what an observer cannot do.

**Refusal.** An unknown device, a revoked device, a malformed proof and a
wrong signature are indistinguishable: after 250 ms the daemon answers the
hello with `{"code": "unauthorized", "message": "device authentication failed"}`
and closes with `4401`. A hello with no `device` is refused the same way. A
hello arriving while `max_connections` device sessions are open is not a
refusal of the device: it is answered `unavailable` and closed with `1000`,
and the client reconnects with backoff. The
protocol version is negotiated before the proof is checked, so a client
outside the served window gets `incompatible` with both bounds named.

**Service worker.** A service worker acting on a notification opens its own
WebSocket and performs the same handshake with the same key, sends its
command with an idempotency key, waits for the reply and closes.

## Pairing

Pairing enrols one device. It needs the host's terminal and the device in
the same hands.

```
host: aether remote pair                 device
  start_pairing ─▶ code + QR
                                         opens https://<origin>/pair#code=7K2M-9QXD
                                         generates its key
                                         POST /api/pair/claim ─▶ fingerprint + phrase
  get_pairing ─▶ claimed: name, fingerprint, phrase
  operator compares the phrase on both screens
  confirm_pairing(accept) ─▶ enrolled
                                         POST /api/pair/status ─▶ enrolled + device id
                                         connects to /ws and authenticates
```

### The code and the URL

- The **code** is 8 characters of Crockford base32
  (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`), printed as `XXXX-XXXX`: 40 bits.
  Input is case-insensitive and ignores dashes and spaces.
- The **pairing URL** is `<first public origin>/pair#code=XXXX-XXXX`. The code
  is in the fragment, which a browser does not send to any server or proxy,
  so it appears in no access log. `aether remote pair` prints the URL as a QR
  code and the code as text for typing.
- With no public origin configured, the URL uses the loopback origin.

### States

One pairing exists at a time, in memory; a daemon restart closes it.

| State | Entered by | Leaves to |
| --- | --- | --- |
| `open` | `start_pairing` | `claimed` (valid claim), `expired` (120 s), `cancelled` |
| `claimed` | `POST /api/pair/claim` with the right code | `enrolled` or `rejected` (`confirm_pairing`), `expired` (120 s after the claim), `cancelled` |
| `enrolled` | `confirm_pairing` with `accept: true` | terminal |
| `rejected` | `confirm_pairing` with `accept: false` | terminal |
| `expired` | the clock | terminal |
| `cancelled` | `cancel_pairing`, a new `start_pairing`, or `configure_remote` disabling pairing or the listener | terminal |

Rules:

- `start_pairing` while a pairing is `open` or `claimed` cancels it and opens
  a new one. It is refused with `invalid_argument` when
  `RemoteSettings.pairing` is `false` or the listener is not enabled.
- A claim is accepted only in `open`. The first valid claim consumes the code.
- At most ten codes a second are compared with an open pairing's, across
  all peers and right or wrong; the rest are `429 rate_limited` with
  `Retry-After: 1`. Wrong codes are answered after a 250 ms delay.
- No number of wrong codes closes a pairing, so a peer without the code
  cannot cancel one. What bounds guessing is the rate: a code lives 120 s, so
  at most 1200 guesses are ever compared with it, and the chance that any of
  them is right is at most 1200 / 2^40, about one in 900 million per pairing.
  A right guess still enrols nothing until the host confirms the device name
  and phrase it sees.
- A peer that keeps the listener flooded with well-formed guesses for the
  whole window can still have an honest claim answered `429`: the claim is
  made again a second later, or the host starts a new pairing.
- The device is enrolled only by `confirm_pairing` with `accept: true`, with
  the role `start_pairing` named. Nothing about a claimed device is durable
  before that.
- Terminal states are kept for five minutes so the device and the host both
  read the outcome, then forgotten (`not_found`).

### The phrase

The daemon derives three words from the pairing id and the claimed public
key and returns them to the claimant and to the host. The operator confirms
they match. If someone else claimed the code first, the phone in the
operator's hand shows `invalid_code` and no phrase, and the host shows a name
and phrase the operator does not recognize: reject it.

### Records and audit

A device record holds its id, name, role, public key, creation time, last
authentication time and revocation time. Records are durable in the daemon's
store and survive restarts; a revoked record is kept and never authenticates
again.

Administration is recorded in the event log as `custom` events whose actor
is the local operator:

| `name` | `payload` |
| --- | --- |
| `remote.configured` | `{revision, enabled, pairing, public_origins, push}` |
| `remote.pairing_started` | `{pairing, role, expires_at}`; never the code |
| `remote.pairing_cancelled` | `{pairing}` |
| `remote.device_enrolled` | `{device, name, role, fingerprint}` |
| `remote.pairing_rejected` | `{name, fingerprint}` |
| `remote.device_updated` | `{device, name, role}` |
| `remote.device_revoked` | `{device, name, reason}` |

Authentication outcomes are recorded the same way, with the system as actor:

| `name` | `payload` |
| --- | --- |
| `remote.device_authenticated` | `{device, name, role, origin}` |
| `remote.device_refused` | `{device, origin, reason, unrecorded}` |

`remote.device_refused` is written for a hello whose proof was checked and
failed: an unknown or revoked device, a wrong signature. Anyone who reaches
the listener can cause one, so only the first five in any ten minutes are
recorded; the rest are refused all the same and only counted. `unrecorded`
is how many refusals were counted since the previous
`remote.device_refused`, so the events add up to every refusal there was,
except those counted since the latest one. That count is kept in memory and
is lost if the daemon restarts first. A hello with no `device` proof at all,
or one answering a challenge that was already answered, is refused before any
device is looked up and is not recorded. Refused hellos are logged at
`debug` only.

## Authorization

Every connection has an authorization context, fixed when its hello is
accepted:

| Context | How it is established |
| --- | --- |
| **local** | The Unix socket (including `aether bridge --stdio` over SSH). |
| **token** | The `[listen]` TCP transport and its shared token. |
| **device operator** | The remote carrier, a paired device with role `operator`. |
| **device observer** | The remote carrier, a paired device with role `observer`. |

Every command belongs to one class, and the context is checked against the
class **before the command is looked at in any other way**: before
subscriptions are opened, before shutdown, before dispatch. A command that is
not in the table is refused; in the daemon the table is one exhaustive match,
so a command cannot be added without a class.

| Class | local | token | device operator | device observer |
| --- | --- | --- | --- | --- |
| `observe` | yes | yes | yes | yes |
| `operate` | yes | yes | yes | no |
| `host_only` | yes | yes | no | no |
| `remote_admin` | yes | no | no | no |
| `device_self` | no | no | yes | yes |

A refusal is `forbidden` with a message naming the command, except
`device_self` from a connection that is not a device, which is
`invalid_argument` (there is no device to act on).

The token listener keeps exactly what it had before protocol 18, which is
why `host_only` is open to it; it gains none of the remote administration.

### The matrix

| Command (`cmd`) | Class | Notes |
| --- | --- | --- |
| `hello` | observe | |
| `ping` | observe | |
| `status` | observe | |
| `preview_retention` | observe | |
| `shutdown` | host_only | |
| `create_agent` | operate | |
| `get_agent` | observe | |
| `list_agents` | observe | |
| `get_tree` | observe | |
| `update_agent` | operate | `host_only` when `patch.backend` or `patch.tools` is supplied. |
| `reply_to_question` | operate | |
| `send_message` | operate | The same with `attachments` (images). |
| `cancel` | operate | |
| `interrupt` | operate | |
| `restart` | operate | |
| `wake` | operate | |
| `pause` | operate | |
| `resume` | operate | |
| `archive` | operate | |
| `set_approvals` | operate | |
| `compact` | operate | |
| `get_github_watcher` | observe | |
| `configure_github_watcher` | operate | A device may not change `operator_users`: a save whose `operator_users` differs from the stored value is `forbidden`. |
| `create_schedule` | operate | |
| `get_schedule` | observe | |
| `list_schedules` | observe | |
| `update_schedule` | operate | |
| `schedule_history` | observe | |
| `trigger` | operate | |
| `engine_tool` | host_only | Engines reach the daemon on its socket. |
| `engine_tools` | host_only | |
| `create_task` | operate | |
| `get_task` | observe | |
| `list_tasks` | observe | |
| `list_approvals` | observe | |
| `resolve_approval` | operate | `host_only` when `remember.scope` is `everywhere`. |
| `list_approval_rules` | observe | |
| `add_approval_rule` | operate | `host_only` when `scope` is `everywhere`. |
| `revoke_approval_rule` | operate | A device may not revoke a rule whose scope is `everywhere`: `forbidden`. |
| `subscribe` | observe | |
| `unsubscribe` | observe | |
| `get_events` | observe | |
| `get_transcript` | observe | |
| `get_inbox` | observe | |
| `get_snapshot` | observe | |
| `get_invocation` | observe | |
| `list_invocations` | observe | |
| `get_checkpoint` | observe | |
| `latest_checkpoint` | observe | |
| `publish_artifact` | operate | |
| `get_artifact` | observe | |
| `list_artifacts` | observe | |
| `read_artifact` | observe | |
| `memories` | observe | `host_only` when `query.proposed` is true. |
| `create_memory` | operate | `host_only` when `spec.proposed` is true. |
| `accept_memory` | host_only | Accepts a pending proposal, applying any deferred replacement. |
| `reject_memory` | host_only | Curates a pending proposal out with a reason. |
| `forget_memory` | operate | |
| `restore_memory` | operate | |
| `pin_memory` | operate | |
| `sync_repository_knowledge` | operate | |
| `alias_repository` | host_only | Moves memory between repository scopes, irreversibly. |
| `list_repository_aliases` | observe | |
| `memories_in_context` | host_only | Which memories each model call saw; for weighing them. |
| `record_memory_feedback` | host_only | Records evidence on a memory the named task saw. |
| `retract_memory_feedback` | host_only | Withdraws feedback while preserving the original record. |
| `memory_feedback` | host_only | Reads recent feedback, including retractions. |
| `list_runs` | observe | |
| `get_run_report` | observe | |
| `replay` | observe | |
| `export_run` | host_only | Writes a host path. |
| `import_run` | host_only | Reads a host path. |
| `list_models` | observe | |
| `auth_status` | observe | |
| `providers` | observe | |
| `mcp_servers` | observe | `operate` when `refresh` is `true`. |
| `get_attention` | observe | |
| `list_workspaces` | observe | |
| `complete_path` | operate | |
| `validate_workspace` | operate | |
| `get_git_summary` | operate | |
| `get_remote` | remote_admin | |
| `configure_remote` | remote_admin | |
| `start_pairing` | remote_admin | |
| `get_pairing` | remote_admin | |
| `confirm_pairing` | remote_admin | |
| `cancel_pairing` | remote_admin | |
| `list_devices` | operate | `remote_admin` when `include_revoked` is `true`. A device is told less than the host: see [`list_devices`](#list_devices). |
| `revoke_device` | remote_admin | |
| `update_device` | remote_admin | |
| `get_push_config` | observe | |
| `subscribe_push` | device_self | |
| `unsubscribe_push` | device_self | |
| `test_push` | device_self | |
| `resolve_submission` | operate | Read-only. Takes the class of the `command` it names when that is stricter, and every condition in that command's row: asking is permitted exactly when sending it would be. |

What no device can do, whatever its role: stop the daemon; enable, disable
or configure remote access; pair, rename, re-role or revoke devices, or read
their fingerprints and revocation history; change
who the GitHub watcher treats as an operator; add or remove an approval rule
that applies to every agent; export or import a run through a host path; act
as an engine's tool bridge.

### Field-level ceilings and readable data

The following conservative limits apply where the original command matrix
left fields unspecified. Local and token connections retain their existing
behavior.

- A device may choose a root workspace, but an explicit creation scope cannot
  grant `allowed_paths` or supply a `repository` identity. Repository identity
  is derived from the workspace. Children still narrow their parent's scope.
- Remote creation and updates cannot enable networking when the daemon's
  `network_default` is `denied`. Root approval policies cannot be weaker than
  the daemon's configured default; children remain within their parent's
  approval ceiling. This applies to both `approval` and `config.approval`.
- `update_agent` with `backend` or `tools` is `host_only`. A device can select
  a container for a new root, but cannot change an existing agent's backend
  or tool exposure. Models that would silently discard required approvals
  cannot be selected remotely, including fallback models.
- An operator device may create spawn schedules, with the same creation
  ceilings as an immediate root. Agents still cannot create spawn targets.
- Scoped approval rules, including rules remembered from an approval answer,
  remain allowed. `everywhere` rules are always host-only, including through
  `remember`; omitting `remember.scope` means only the answered agent.
- A remote `trigger.source` is 1–128 ASCII letters, digits, dots, underscores
  or hyphens. Its payload never grants operator standing; only a stored actor
  or the daemon's verified GitHub watcher may do that.
- Both device roles can read fleet snapshots, transcripts, artifacts, replay,
  MCP descriptions and events. Remote-administration events (`remote.*`)
  become `remote.restricted` with `{"redacted":true}` payloads on device
  connections, preserving sequence numbers while hiding fingerprints and
  revocation history. Token connections receive the same redaction. This
  applies to historical and live events alike.
  An event filter is applied before the redaction and cannot see through it:
  every custom event has the type and the family `custom` whatever its name,
  so `types` and `families` select all custom events or none, and no filter
  reads a custom event's name or payload.
- Delta streams honor agent (including the direct-child option), run and task
  filters. Event family/type filters select durable events and suppress
  deltas, which have no event type. Revocation also cancels blocked deliveries
  and discards queued device data frames.

These are protocol authorization boundaries. Operator devices still direct
code running under the daemon's existing execution policy. Host-mode agent
execution is not an OS sandbox around the host account or its Unix socket.

### Provenance

An operator device has an operator's standing: its messages are the
operator's, and agents treat them so. What changes is the record of who
acted. Events caused over a device connection carry

```json
{"actor": "user", "origin": {"transport": "device", "device": "dev_01J…", "name": "Robin's iPhone", "connection": "01J…"}}
```

and render as `user:device:Robin's iPhone(dev_01J…)#01J…`, beside the
existing `user:local_socket#…` and `user:tcp:<peer>#…`. `device` and `name`
are the daemon's enrolled record, never anything the client sent on the
connection. Names in rendered markers are UTF-8 percent-encoded except for
ASCII letters, digits, spaces, apostrophes, hyphens and underscores, so names
cannot insert marker delimiters, line breaks or bidi controls. Structured
records retain the original name. Approval inbox deliveries carry optional
`by` provenance, rendered alongside the decision; message and question-reply
inboxes retain their operator actor. Observer connections never get an
operator-standing actor. A client speaking protocol 17 cannot decode this origin and
receives the same actor as `{"actor": "user", "name": "device:<name>"}`.

### Revocation

`revoke_device` marks the record revoked, then immediately closes every live
connection of that device, which ends their subscriptions, and
deletes its push subscription. `update_device` changing the role closes the
device's connections the same way, so the next connection carries the new
role. Renaming does not disconnect. Cancellation interrupts outbound sends
and discards unsent responses. The daemon stops its protocol writer without
queuing a final notice. An idle WebSocket receives a `forbidden` notice and
`4403`; if an outbound frame is pending, the transport is dropped immediately
without flushing that frame or sending a WebSocket close frame. Bytes already handed to the transport before
revocation cannot be recalled.

## Commands added in protocol 18

Replies are shown as the `data` of a reply frame:
`{"frame":"reply","id":N,"result":{"Ok":{"reply":"<tag>","data":…}}}`.
Optional fields are omitted when absent.

### Attention

#### `get_attention`

Everything waiting on the operator, read at one position in the event log.
No fields.

Reply `attention` (`Attention`):

| Field | Type | Meaning |
| --- | --- | --- |
| `last_seq` | integer | The log position this was read at. |
| `approvals` | `ApprovalRequest[]` | Pending approvals, oldest first; the same items `list_approvals` returns. Each names the tool, its `description` and its `argument`, which is what is being approved. |
| `questions` | `PendingQuestion[]` | Questions nobody has answered or superseded, oldest first. |
| `agents` | `AgentAttention[]` | Failed agents, and agents blocked on something only the operator can clear. Archived agents are excluded. Oldest first. |

A question is pending while it is the latest `question.asked` of a live,
unarchived agent and no operator (`user` or `client`) message to that agent
has followed it, the rule `reply_to_question` applies. A failed agent is
listed unless it has been restarted. A blocked agent is listed when its block
is an inspection or exhausted credit; one waiting on a dependency or a
resource clears itself and is not.

Each list holds at most 200 items. When more qualify, `approvals` and
`questions` keep the oldest, which block work, and `agents` keeps the most
recent. `question` is cut at 4000 characters and `reason` at 300, a cut
ending in `…`.

`PendingQuestion`:

| Field | Type | Meaning |
| --- | --- | --- |
| `agent` | `AgentId` | Who asked. |
| `agent_name` | string, optional | |
| `question_seq` | integer | Pass to `reply_to_question`. |
| `question` | string | The question text. |
| `asked_at` | RFC 3339 | |

`AgentAttention`:

| Field | Type | Meaning |
| --- | --- | --- |
| `agent` | `AgentId` | |
| `agent_name` | string, optional | |
| `kind` | `agent_failed` \| `agent_blocked` | |
| `reason` | string | One line: the failure, or what it is blocked on. |
| `since` | RFC 3339 | When it entered that state. |

The reply is read with the event log held still, so `last_seq` is exactly the
position of what it shows. A client shows a badge from this reply and keeps it
current from the event stream: subscribe with `after` at or before `last_seq`.

### Host services

These let a client that is not on the host choose and inspect workspaces.
Every path is on the daemon's machine.

Every path and `prefix` a client sends is limited to 4096 bytes and must not
contain NUL or any other control character; one that does, or a `limit` of 0,
a relative `workspace` or a path that is not UTF-8, is `invalid_argument`.
Filesystem work runs off the async runtime, at most 8 requests at once and 5
seconds each; beyond either, the reply is `unavailable`. A job that outlives its
5 seconds keeps its slot until it returns, so a stuck mount cannot exhaust the
pool, and the requests it blocks are answered `unavailable`. Git runs under the
same kind of bound (see `get_git_summary`).

#### `complete_path`

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | `directory` \| `workspace_file` | What to complete. |
| `prefix` | string | What was typed so far. May be empty. |
| `workspace` | path, optional | Required for `workspace_file`. |
| `limit` | integer, optional | Default 50; values above 200 are treated as 200. |

`directory`: `prefix` is an absolute path or starts with `~/` (the daemon
user's home); an empty prefix or `~` means the home directory. `.` and `..`
in the prefix are folded lexically, as a shell's `cd` does. The reply lists
the directories in the prefix's parent whose name starts with its last
segment. Entries are absolute; `~` is expanded. Hidden directories are listed
only when the last segment starts with `.`. Symlinks to directories are listed
and not resolved. Directories are completed wherever the daemon user can read.

`workspace_file`: `prefix` is relative to `workspace`. The reply lists files
and directories under `workspace` matching the prefix, as paths relative to
it. Entries ignored by git, and `.git`, are omitted; hidden entries follow the
same rule as `directory`. A `prefix` that leaves the workspace (by `..` or an
absolute path), and a symlink whose target is outside it, match nothing: a
symlink to somewhere inside is listed. `workspace` must pass
`validate_workspace` (an existing directory the daemon user can read and
enter); otherwise `invalid_argument`.

In both kinds, the last segment matches case-insensitively unless it contains
an uppercase letter. Names that are not UTF-8 or contain control characters
are never listed.

Both read one directory per request, never recurse, and stop at `limit`. A
directory of more than 50 000 entries, or one that takes over a second to
read, is cut short and the reply is `truncated`.

Reply `path_completions` (`PathCompletions`):

| Field | Type | Meaning |
| --- | --- | --- |
| `entries` | `PathCompletion[]` | Sorted by `path`, ignoring a directory's trailing `/`. |
| `truncated` | bool | More matched than `limit`, or the directory was only partly read. |

`PathCompletion`: `path` (string; a directory ends in `/`), `is_dir` (bool).

A prefix whose parent does not exist or cannot be read is not an error: the
reply is empty.

#### `list_workspaces`

| Field | Type | Meaning |
| --- | --- | --- |
| `limit` | integer, optional | Default 50, at most 200. |

Reply `workspaces` (`Workspace[]`), most recently active first: every
directory that is the workspace of a root agent in the store, live or
finished, plus the daemon's own workspace root.

| Field | Type | Meaning |
| --- | --- | --- |
| `path` | path | Canonical absolute path. |
| `agents` | integer | Live agents in trees rooted here. |
| `running` | integer | Of those, running a turn now. |
| `active_at` | RFC 3339 | Latest update of any root agent here; the daemon's start for its own root when no agent has worked there. |
| `exists` | bool | The directory is still there. A workspace that is gone is still listed, flagged `false`. |
| `git` | `GitSummary`, optional | Absent when the directory no longer exists, or its git state could not be read. |

Paths are canonicalised before they are compared, so two spellings of one
directory are one workspace. `limit` applies after ordering; ties on
`active_at` are ordered by `path`. Git states are read in parallel, each
within 2 seconds.

#### `validate_workspace`

| Field | Type | Meaning |
| --- | --- | --- |
| `path` | string | Absolute, or starting with `~/`. |

Reply `workspace_validation` (`WorkspaceValidation`):

| Field | Type | Meaning |
| --- | --- | --- |
| `valid` | bool | An agent can be started here: the path resolves to an existing directory the daemon user can read and enter. |
| `path` | path, optional | The canonical path, when the input resolved. Use this in `create_agent`'s scope. |
| `reason` | string, optional | Why not, in one sentence. Absent when `valid`. |
| `git` | `GitSummary`, optional | Present when `valid`. |

An invalid path is a successful reply with `valid: false`, not an error: a
file, a missing directory, one the daemon user cannot enter, a relative path
and `~user` all say why in `reason`. `path` is still given when the input
resolved to something that is not usable. Input that is over-long or contains
control characters is `invalid_argument`.

#### `get_git_summary`

| Field | Type | Meaning |
| --- | --- | --- |
| `workspace` | path | A directory on the host. |

Reply `git_summary` (`GitSummary`):

| Field | Type | Meaning |
| --- | --- | --- |
| `repository` | bool | The directory is inside a git work tree. When `false` the other fields are absent or `false`. A bare repository or the inside of `.git` is not a work tree. |
| `root` | path, optional | Top level of the work tree. |
| `branch` | string, optional | Current branch. Absent when detached; in a repository with no commit, the branch `HEAD` points at. |
| `head` | string, optional | Abbreviated commit (7 characters) of `HEAD`. Absent in a repository with no commit. |
| `detached` | bool | |
| `dirty` | bool | Uncommitted changes to tracked files, or untracked files that are not ignored. |
| `ahead` | integer, optional | Commits the branch has that its upstream lacks. Absent without an upstream. Read from local refs; nothing is fetched. |
| `behind` | integer, optional | Commits the upstream has that the branch lacks. |
| `staged` | integer | Paths with changes in the index. |
| `modified` | integer | Tracked paths changed in the work tree and not staged, conflicts included. |
| `untracked` | integer | Untracked paths that are not ignored. |

`workspace` must be an absolute directory the daemon user can read and enter,
otherwise `invalid_argument`. Not being in a repository is a reply with
`repository: false`. If git is not installed, cannot be run, takes over 5
seconds, or cannot read the repository, the error is `unavailable` with the
reason.

The repository's content is untrusted, its git directory and config
included, and no read of it may start a program the repository names. Every
`git` runs with a scrubbed environment (no user or system config, no global
attributes, no prompts, no optional locks), no pager, hooks and the filesystem
monitor disabled, every transport denied, lazy fetches from a promisor remote
off, and stdin closed. Beyond that the read is split in two, so that the claim
does not rest on a list of dangerous config keys:

- **Refs and commits** are read in the repository itself, with its config:
  where the work tree is, what `HEAD` names, the commit, and the distance from
  the upstream. These commands (`rev-parse`, `symbolic-ref`, `for-each-ref`,
  `config`) never open the index or a file of the work tree and never write.
- **The work tree** is compared by `git status` run with a git directory the
  daemon creates for the purpose and removes afterwards, pointed at the
  repository's index, objects and files. Status is what hashes files, and so
  what would run a `filter.*.clean` or `filter.*.process` command; in that
  run git never opens the repository's config, in any scope or through any
  include, so no filter, monitor, hook path, diff driver or other program it
  configures exists. Submodules are not entered.

Only eight settings are carried from the repository into that run, because
they decide whether a file counts as changed and each takes one word:
`core.fileMode`, `core.symlinks`, `core.ignoreCase`, `core.autocrlf`,
`core.eol`, `core.trustctime`, `core.checkStat` and `core.precomposeUnicode`.
A value that is not a single word is `unavailable`. The repository's
`info/exclude` still applies.

What follows for a client:

- A repository that configures filters is summarised like any other. Its
  filters do not run, so a file a clean filter would have normalised (one
  tracked by Git LFS, say) counts as `modified` if it was touched since it was
  checked out.
- A repository owned by another user is `unavailable` (git's "dubious
  ownership"), whatever the daemon user's own `safe.directory` says: that
  setting lives in the user config the daemon does not read, and a
  repository cannot grant it to itself.
- Counts stop at 10 000 entries, after which they are lower bounds.

### Remote administration

All `remote_admin`, the host's Unix socket only, except `list_devices`
without `include_revoked`, which an operator device may also call.

#### `get_remote`

No fields. Reply `remote` (`RemoteStatus`):

| Field | Type | Meaning |
| --- | --- | --- |
| `revision` | integer | Bumped by every accepted `configure_remote`. Starts at 0. |
| `settings` | `RemoteSettings` | See [runtime settings](#runtime-settings-remotesettings). |
| `address` | string | `[remote] address` as configured. |
| `listening` | string, optional | The address actually bound. Absent while closed. |
| `allow_remote` | bool | `[remote] allow_remote` as configured. |
| `devices` | integer | Enrolled, unrevoked devices. |
| `connections` | integer | Live device connections. |
| `push_previews` | bool | `[remote] push_previews` as configured. |
| `error` | string, optional | Why the listener is not open although `enabled`. |

#### `configure_remote`

| Field | Type | Meaning |
| --- | --- | --- |
| `settings` | `RemoteSettings` | The whole new settings. |
| `expected_revision` | integer | The `revision` the caller last read. |

Compare-and-swap, like `configure_github_watcher`: a stale
`expected_revision` is `invalid_argument` and nothing changes. On success the
listener is opened, closed or rebound before the reply, which is `remote`.
If the listener cannot be opened, the settings are still saved and the reply
carries `error`.

Disabling the listener closes every device connection with `4503` and
cancels an open pairing. Removing an origin from `public_origins` closes the
connections that arrived by it. Turning `push` on for the first time creates
the daemon's VAPID key; if the key cannot be created or read, the change is
refused with `internal` and nothing is saved.

#### `start_pairing`

| Field | Type | Meaning |
| --- | --- | --- |
| `role` | `operator` \| `observer`, optional | Default `operator`. |

Reply `pairing` (`Pairing`) in state `open`, with `code` and `url` present.
This is the only reply that ever carries them.

`Pairing`:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | |
| `state` | `PairingState` | `open`, `claimed`, `enrolled`, `rejected`, `expired`, `cancelled`. |
| `role` | `DeviceRole` | What the device will be enrolled as. |
| `expires_at` | RFC 3339 | End of the current window. |
| `code` | string, optional | `XXXX-XXXX`. Only in the reply to `start_pairing`. |
| `url` | string, optional | The pairing URL. Same presence as `code`. |
| `candidate` | `PairingCandidate`, optional | From `claimed` on: `name`, `fingerprint`, `phrase`, `claimed_at`. |
| `device` | `DeviceIdentity`, optional | Once `enrolled`. |

#### `get_pairing`

`{id}`. Reply `pairing`. Unknown or forgotten id: `not_found`.
`aether remote pair` polls this twice a second.

#### `confirm_pairing`

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | |
| `accept` | bool | `true` enrols the candidate; `false` rejects it. |

Valid only in `claimed`; otherwise `invalid_transition`. Reply `pairing` in
`enrolled` (with `device`) or `rejected`. Repeating the same answer to a
pairing already in that terminal state returns the same reply.

#### `cancel_pairing`

`{id}`. Reply `pairing` in `cancelled`. Cancelling a terminal pairing
returns it unchanged.

#### `list_devices`

| Field | Type | Meaning |
| --- | --- | --- |
| `include_revoked` | bool, optional | Default `false`. `true` is `remote_admin`: a device asking for it is `forbidden`. |

Reply `devices` (`Device[]`), oldest first.

`Device`:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `DeviceId` | |
| `name` | string | |
| `role` | `DeviceRole` | |
| `fingerprint` | string, optional | 64 hex characters. **Only in replies to the host.** |
| `created_at` | RFC 3339 | |
| `last_seen_at` | RFC 3339, optional | Last accepted hello. |
| `current` | bool | This is the device the request came from. Always `false` in replies to the host. |
| `revoked_at` | RFC 3339, optional | |
| `connections` | integer | Live connections now. A connection whose session was just ended by a revocation or role change does not count, though its socket may still be closing. |
| `push` | bool | Holds a push subscription. |

An operator device may call this so a phone can show the paired devices in
its settings. It receives the unrevoked devices with no `fingerprint` and no
other key material, and `current: true` on its own record. An observer
device is refused. Pairing, renaming, re-roling and revoking stay on the
host.

#### `revoke_device`

`{id, reason?}`. Reply `device`. Revoking a revoked device returns it
unchanged. `reason` is 1 to 256 characters with no control characters. See
[Revocation](#revocation).

#### `update_device`

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `DeviceId` | |
| `name` | string, optional | A [device name](#device-names). |
| `role` | `DeviceRole`, optional | |

At least one of `name` and `role`. Reply `device`. A revoked device cannot
be updated: `invalid_transition`. An update that changes nothing is answered
with the device as it is and records no event.

### Push

#### `get_push_config`

No fields. Reply `push_config` (`PushConfig`):

| Field | Type | Meaning |
| --- | --- | --- |
| `enabled` | bool | `RemoteSettings.push`. |
| `vapid_public_key` | string, optional | The application server key: a 65-byte uncompressed P-256 point, base64url. Absent until push was first enabled. |
| `previews` | bool | `[remote] push_previews`. |
| `subscribed` | bool | The calling device holds a subscription. `false` for a connection that is not a device. |
| `kinds` | `AttentionKind[]` | What the calling device is notified about; empty means every kind. |

#### `subscribe_push`

| Field | Type | Meaning |
| --- | --- | --- |
| `subscription` | `PushSubscription` | `{endpoint, p256dh, auth}` from the browser. |
| `kinds` | `AttentionKind[]`, optional | Notify about these only. Empty or absent: all. |

`PushSubscription`: `endpoint` is an `https:` URL of at most 2048 bytes with
no credentials and no explicit port, whose host is an
[allowed push service](#which-push-services); `p256dh` is the subscription's
65-byte P-256 key and `auth` its 16-byte secret, both base64url without
padding. Anything else is `invalid_argument`.

A device has at most one subscription; this replaces it. Sending the same
subscription again changes nothing. Reply `push_config`. Refused with
`unavailable` while `RemoteSettings.push` is `false`.

#### `unsubscribe_push`

No fields. Forgets the calling device's subscription; succeeds when there is
none. Reply `push_config`.

#### `test_push`

No fields. Sends the calling device one notification of kind `test`, now and
once: it is not queued and not retried. Reply `ok` once the push service
accepted it; `unavailable` with the service's status otherwise, and also
while `RemoteSettings.push` is `false`; `invalid_argument` when the device
has no subscription. A `404` or `410` from the push service removes the
subscription before the reply, unless the device replaced it while the test
was being sent: then nothing is removed and the reply says so.

### Submissions

#### `resolve_submission`

Asks what became of a keyed command whose reply never arrived. It executes
nothing and writes nothing.
[Resolving a lost acknowledgement](#resolving-a-lost-acknowledgement) says
when to send it and what a client does with each answer.

| Field | Type | Meaning |
| --- | --- | --- |
| `command` | object | The keyed command exactly as it was first sent: its `cmd`, every field, and its `idempotency_key`. |
| `horizon` | string, optional | The `Welcome.submission_horizon` of the connection `command` was first sent on. Without it the answer is never `absent_safe`. |

```json
{"req": 9, "cmd": "resolve_submission",
 "command": {"cmd": "send_message", "to": "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV",
             "content": "ship it", "idempotency_key": "outbox-7"},
 "horizon": "01K7C2Q3ZV8N4T6R9W1XBYD5HM:41"}
```

Reply `submission_resolved` (`SubmissionResolution`), an object tagged by
`outcome`:

| `outcome` | Other fields | Meaning |
| --- | --- | --- |
| `executed` | `reply`, `horizon` | The command ran, once. `reply` is the whole reply, `{"reply": "<tag>", "data": …}`, that a second send under the same key is answered with: the original receipt, unchanged by anything that happened to its subject since. |
| `absent_safe` | `horizon`, `retained_since` | The command has not run, and the host proves it: no receipt is held, and no receipt recorded since the request's `horizon` has ever been dropped. |
| `unknown` | `reason`, `horizon`, `retained_since` | No receipt is held and nothing proves there never was one. |

```json
{"outcome": "executed", "horizon": "01K7C2Q3ZV8N4T6R9W1XBYD5HM:57",
 "reply": {"reply": "message_sent", "data": {"message": "msg_01ARZ3NDEKTSV4RRFFQ69G5FAV"}}}

{"outcome": "absent_safe", "horizon": "01K7C2Q3ZV8N4T6R9W1XBYD5HM:57",
 "retained_since": "2026-10-05T08:00:00Z"}

{"outcome": "unknown", "reason": "forgotten",
 "horizon": "01K7C2Q3ZV8N4T6R9W1XBYD5HM:57",
 "retained_since": "2026-10-11T19:02:44Z"}
```

`horizon` in a reply is the host's horizon now. It is informational: a client
never writes it over the one an outbox entry already has. `retained_since` is
for showing a person, and is not part of the decision, which uses no clock:
every receipt recorded after that instant is still held.

| `reason` | The host cannot prove absence because… |
| --- | --- |
| `no_horizon` | the request carried no `horizon`. |
| `foreign_horizon` | the horizon was issued under another ledger: the store was replaced, restored under another identity, or first kept a ledger after the command was sent. |
| `horizon_ahead` | the horizon counts more receipts than the store has recorded: the store was rolled back to an earlier copy of itself. |
| `forgotten` | a receipt recorded after the horizon has been dropped, by the seven-day expiry or by the size cap. It may have been this one. |

A horizon is opaque. A client stores and returns the string and never parses
or compares it.

Errors, each of which leaves the command unresolved:

- `invalid_argument`: `command` is not a command that takes an idempotency
  key, has no `idempotency_key`, or is itself a `resolve_submission`;
  `horizon` is not a string this host could have issued; or a receipt is held
  under that key for different fields, which is the refusal sending it would
  get.
- `unauthorized`, `forbidden` and every refusal in
  [the matrix](#the-matrix): the caller may not send `command` now. A
  resolution reveals nothing about a command its caller could not send.

The lookup reads the caller's own key namespace: a device's keys are its own,
and local and token clients share theirs (see
[idempotency keys](#idempotency-keys)). A key only another caller used is
absent here, and is truly new work for this caller.

## Push notifications

### Subscribing

```js
const config = await request({ cmd: "get_push_config" });            // enabled, vapid_public_key
const registration = await navigator.serviceWorker.ready;
const sub = await registration.pushManager.subscribe({
  userVisibleOnly: true,
  applicationServerKey: base64urlDecode(config.vapid_public_key),
});
const json = sub.toJSON();
await request({ cmd: "subscribe_push",
                subscription: { endpoint: json.endpoint, p256dh: json.keys.p256dh, auth: json.keys.auth } });
```

On iOS, Web Push works only for an app added to the home screen, and
`Notification.requestPermission()` must be called from a tap.

The client resubscribes when the browser fires `pushsubscriptionchange`, and
on start when `get_push_config` says `subscribed: false` while permission is
granted.

### What is sent

The daemon watches its event stream and notifies every subscribed, unrevoked
device, filtered by that device's `kinds`:

| `kind` | When |
| --- | --- |
| `approval` | An approval request starts waiting. |
| `question` | An agent asks a question. |
| `agent_failed` | An agent fails. |
| `agent_blocked` | An agent blocks on something only the operator can clear: a tool call that needs inspection, or a provider's allowance running out. A failed probe of the same exhaustion is not announced again. |
| `task_completed` | A root agent's task completes. |
| `task_failed` | A root agent's task fails. |
| `test` | `test_push`. |

An approval that a remembered rule answered in the same commit notifies
nobody, and one decided before its notification left the daemon is not sent.
Imported history notifies nobody.

A device with a live WebSocket connection is still notified; the service
worker decides whether to show a notification while a page is visible. The
one exception is the device that caused the event (the event's actor names
it as its origin): it is not told about what it just did.

Nothing is sent while `RemoteSettings.push` is `false`, and what was waiting
is dropped when it is turned off.

Delivery errors and logs include only the push-service host and a reason
category, never the endpoint path or query (the endpoint is a capability).

Requests to the push service:

- go through Aether's SSRF-aware HTTP client: the endpoint must be `https`
  and resolve to public addresses only, and the connection is pinned to the
  vetted address;
- are VAPID-signed (RFC 8292, ES256) with the daemon's key: `aud` is the
  endpoint's origin, `exp` is twelve hours ahead, and `sub` is
  `push_subject`, else the first public origin, else absent;
- are encrypted for the subscription (RFC 8291, `aes128gcm`, one record);
- carry `TTL: 3600`, `Urgency: high` for `approval` and `question` and
  `normal` otherwise, and a `Topic`.

#### Coalescing

A `Topic` names an agent and one of two classes: what waits on the operator
(`approval`, `question`) and status (`agent_failed`, `agent_blocked`,
`task_completed`, `task_failed`). A newer notification replaces an
undelivered older one with the same topic, in the daemon's queue and at the
push service. Provider exhaustion uses one topic per provider instead, so a
fleet that runs out together is one notification. The topic is an opaque
32-character token; it reveals no id to the push service.

The daemon also paces itself. A notification waits 400 ms before it is sent,
so the events of one commit collapse, and after a send the same topic stays
quiet for 5 seconds: a burst becomes one notification at its start and one,
carrying the latest event, at its end.

#### Failures

| The push service answers | The daemon |
| --- | --- |
| `2xx` | Done. |
| `404`, `410` | Deletes the subscription it sent to, and what was waiting for the device. |
| `408`, `429`, `5xx`, a timeout, no connection | Retries. |
| Any other status, or an endpoint the client refuses to contact | Drops the notification; the subscription stays. |

A `404` or `410` is about the subscription the request was sent to, and
only that one is deleted. A device that subscribed again while the request
was with the push service keeps its new subscription and what was waiting
for it, and the refused notification is sent again, to the new one.

A retry waits 5 seconds, doubling per consecutive failure to at most 15
minutes, with jitter over the upper half of each step, and never less than a
`Retry-After` the push service sent. The wait applies to the device, one send
at a time, so a push service that is down is not hammered. A notification is
given up after an hour, the `TTL` it would have had. At most 256 notifications
wait at once, 32 per device; beyond that the oldest waiting one is dropped.
None of this blocks the daemon or its event stream.

#### Which push services

The endpoint comes from a paired device, and the daemon will connect to it
and report the answer to `test_push`. So that a device cannot aim the daemon
at an arbitrary host, an endpoint must name a push service the daemon knows:

`fcm.googleapis.com`, `jmt17.google.com`, `*.push.services.mozilla.com`,
`*.push.apple.com`, `*.notify.windows.com`

or a host the operator added in `[remote] push_hosts` (for a self-hosted
push service). The host must be a name, never an address, on the default
port. The check runs when a device subscribes and again before every send,
on top of the public-address rule above.

#### The key and the subscriptions

The VAPID key pair is created when `push` is first turned on. The private key
is a file beside the store (`aether.push-vapid-key` next to `aether.db`),
readable by the daemon's user only; the daemon refuses to use it with any
other mode. It is never logged, never sent to a client or an agent, and never
leaves the machine. Deleting it makes the daemon create a new one, and every
device must subscribe again. Subscriptions are kept in the daemon's store,
one per device. Revoking a device deletes its subscription.

### Payload

The decrypted message is one JSON object (`PushPayload`):

```json
{"v": 1, "kind": "approval", "instance_id": "01JABC…", "seq": 48213, "at": "2026-10-12T09:31:07Z",
 "title": "Approval needed", "url": "#attention?item=apr_01J…",
 "agent": "agt_01J…", "agent_name": "release-manager", "approval": "apr_01J…"}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `v` | integer | Payload version: `1`. |
| `kind` | `AttentionKind` | |
| `instance_id` | string | The daemon that sent it. |
| `seq` | integer | The event that caused it. `0` for `test`. |
| `at` | RFC 3339 | |
| `title` | string | Fixed text for the kind (below). |
| `url` | string | Where a tap lands: a fragment route of the web app (below), relative to the service worker's scope. Always present. |
| `agent` | `AgentId`, optional | |
| `agent_name` | string, optional | At most 80 characters. |
| `task` | `TaskId`, optional | For the task kinds. |
| `approval` | `ApprovalId`, optional | For `approval`. |
| `question_seq` | integer, optional | For `question`. |
| `preview` | string, optional | At most 200 characters, on one line, of the question, the tool call summary, the failure or the reason for the block. Never for `task_completed` or `test`. **Only when `push_previews` is `true`.** |

Titles: `Approval needed`, `An agent has a question`, `An agent failed`,
`An agent is blocked`, `Task completed`, `Task failed`,
`Aether notifications are working`.

`url` by kind:

| `kind` | `url` |
| --- | --- |
| `approval` | `#attention?item=<approval>` |
| `question` | `#attention?item=question-<question_seq>` |
| `agent_failed`, `agent_blocked`, `task_completed`, `task_failed` | `#agent/<agent>` |
| `test`, or any kind without an agent | `#activity` |

A message is at most 3993 bytes of JSON, so that the encrypted body fits the
4096 bytes every push service accepts.

Without `push_previews` a payload contains identifiers, an agent name and a
fixed title: nothing an agent or a tool wrote. Push services see only
ciphertext either way.

### Acting on a notification

The service worker shows the notification from the payload. A tap opens the
app at the payload's `url`. An action button (approve, deny) is handled in the service
worker: it opens a WebSocket, authenticates with the device key, sends
`resolve_approval` with an idempotency key derived from the notification
(`"push-" + approval + "-" + decision`), and closes. A payload whose
`instance_id` is not the one the app last connected to is shown without
actions.

## Reliability: idempotency, reconnect and resync

A phone loses its connection constantly. The rules below make that safe.

### Idempotency keys

A remote client sends `idempotency_key` on every command that accepts one.
The key is 1 to 200 bytes of printable ASCII with no spaces;
`crypto.randomUUID()` is a good one. The normal retention window is seven
days. The existing store-wide 100,000-record size cap can evict older keys
inside that window, reported by a `submissions.evicted` event. A retry after
expiry or eviction is new work, so a client never resends a command whose
reply it did not see until the host has said what became of it:
[resolving a lost acknowledgement](#resolving-a-lost-acknowledgement).
Immutable receipts can use substantially more storage than the older
identity-only records.

Retention runs in the transaction that records a submission. The age pass runs
on every write: one seek into the `at` index plus whatever it deletes. Counting
the table to find an overflow is dearer, so only some writes run the size pass.
Everything the size pass evicts was inside the age window, which is why it is
reported (`submissions.evicted`) rather than dropped quietly. What either pass
drops moves the ledger's watermark in the same transaction, so no receipt is
gone while the ledger still vouches for it.

- Sending the same key with the same fields returns the first outcome and
  does the work once.
- Sending the same key with different fields is `invalid_argument`.

| Command | Key | Without a key, a resend… |
| --- | --- | --- |
| `create_agent` | `idempotency_key` | creates a second agent |
| `create_task` | `idempotency_key` | creates a second task |
| `send_message` | `idempotency_key` | delivers twice |
| `reply_to_question` | `idempotency_key` | is refused as stale |
| `trigger` | `idempotency_key` | delivers twice |
| `create_schedule` | `idempotency_key` | creates a second schedule |
| `resolve_approval` | `idempotency_key` | fails with "already resolved" |
| `restart` | `idempotency_key` | fails if the agent already has a successor |
| `cancel` | `idempotency_key` | repeats the cancellation request |
| `interrupt` | `idempotency_key` | can stop a newer turn |
| `pause` | `idempotency_key` | can pause work resumed since the first request |
| `resume` | `idempotency_key` | can resume a later pause or wait |
| `archive` | `idempotency_key` | can change work restored since the first request |
| `update_agent` | `idempotency_key` | can overwrite a later update |
| `set_approvals` | `idempotency_key` | can overwrite a later policy |
| `update_schedule` | `idempotency_key` | can overwrite a later transition |
| `create_memory` | `idempotency_key` | can create a duplicate |
| `record_memory_feedback` | `idempotency_key` | records a second verdict |
| `add_approval_rule` | `idempotency_key` | creates a duplicate where remotely allowed |

Keys from device connections are scoped to the enrolled device id, surviving
reconnects and name changes. Local and token clients retain their shared key
namespace. Keys are still checked after authorization: a receipt never grants
permission to an observer or a revoked device.

Successful mutations and their receipts are committed atomically. New records
return the original receipt, including its original agent, task, memory or
schedule snapshot, even if that object later changes. Message, question reply
and trigger receipts retain their original delivery identities. Legacy records
written before immutable receipts continue to resolve their original object
identities. Rejections before a commit do not consume the key.

A cancellation receipt acknowledges durable cancellation intent; the runtime
finishes the subtree cancellation and retries unfinished work after a restart.
An interrupt records the result for the turn it observed and signals only that
turn's cancellation token; retrying that key never interrupts a newer turn.
Keyed archive sweeps use one atomic transaction, which can be larger than an
unkeyed, chunked sweep over a long history.

The remaining convergent commands do not accept keys: `wake`, `compact`,
`pin_memory`, `forget_memory`, `restore_memory`, `subscribe_push` and
`unsubscribe_push`. A resend may answer `invalid_transition`; read the target
state before deciding whether another action is needed.

### Client state

A client keeps, durably:

- `instance_id`: from the last `Welcome`;
- `last_seq`: the highest event `seq` it has applied;
- an **outbox**: every keyed command sent and not yet answered. Each entry
  holds the command with its idempotency key and exact fields, the
  `Welcome.submission_horizon` of the connection it was first sent on, and
  whether it has become **unconfirmed** (and why). An entry is written before
  the command is first sent. Its horizon is written once and never changed.

### Reconnecting

1. Wait with exponential backoff: 1 s doubling to 30 s, with jitter, reset
   after a `Welcome`. Do not reconnect after close `4401`.
2. Open the socket, answer the challenge, read the `Welcome`.
3. **If `Welcome.instance_id` differs from the stored one**, the daemon is a
   different store: discard everything cached, including `last_seq` and the
   outbox (its targets do not exist there), tell the user, and do a full
   resync.
4. Otherwise send `subscribe` with `after: last_seq`. The reply is
   `subscribed {id, last_seq}` and the missed events follow in order, then
   live ones.
5. **If that `subscribe` fails with `not_found`**, the history after
   `last_seq` has expired: do a full resync.
6. Resolve the outbox in order, one entry at a time, as
   [resolving a lost acknowledgement](#resolving-a-lost-acknowledgement)
   describes. Nothing in the outbox is resent before the host has answered
   for it, and an entry the host cannot account for does not hold up the
   connection or the entries after it.
   An outbox file that cannot be read as an operation (undecodable, oversized,
   not a regular file, or named for another key) does not hold it up either:
   it is listed as unreadable, with its path, until a person discards it.
   An entry whose send is still in flight in another process is skipped; the
   sender holds an advisory lock on it until it is answered.
7. Deltas are not replayed. For an agent that was mid-turn, read
   `get_transcript` after the last transcript entry held.

If a subscription's replay fails midway, the daemon sends a reply frame with
`id` 0 and closes; reconnect and do a full resync.

### Resolving a lost acknowledgement

A command was sent and its reply never arrived. It may have run. Sending it
again is safe only while the host still holds its receipt, and a receipt is
dropped after seven days or, sooner, by the size cap; a resend after that
runs the command a second time. `resolve_submission` settles the question
without running anything.

**What the host keeps.** The store numbers every receipt it records, 1, 2,
3, … in commit order, and keeps a ledger of two numbers: `recorded`, the
number of the newest receipt, and `forgotten`, the highest number among the
receipts it has ever dropped. The ledger is written in the same transaction
as the receipt it counts and as the expiry or eviction sweep that drops one,
so it is never behind the table it describes; neither number ever decreases,
and both survive a restart. The ledger names the `instance_id` of its
store and has a random `epoch`. A store that has no ledger (a database from
before receipts were numbered, or a new one) starts one under a new epoch
with `forgotten` equal to `recorded`: it vouches for nothing recorded before
that moment. The same happens if the ledger names a different `instance_id`
from the one in the database's own `meta` table, which no normal operation
produces.

The `instance_id` is stored in the same database as the ledger, so it
travels with a copy or a restore, and a database copied to another machine, or
restored from a backup, keeps its `instance_id` and its `epoch`. The ledger
cannot tell such a store from the original and does not try. What protects
the client is the numbers: a restored older copy has a `recorded` below a
horizon the client holds, which is `horizon_ahead` and so `unknown`. Two copies
that each go on recording are not told apart: a fork is not detected.

**The horizon.** `Welcome.submission_horizon` is the ledger's epoch and
`recorded` at the handshake, as one opaque string. A command first sent on
that connection, if it runs, is given a number above the horizon. So when no
receipt is held for it:

- `forgotten` at or below the horizon, in the same epoch, means every receipt
  numbered above the horizon is still held, and this command's is not among
  them: it never ran. That is `absent_safe`.
- anything else means a receipt that could have been this command's is gone,
  or the ledger cannot be compared with the horizon. That is `unknown`.

The host looks for the receipt first and reads the ledger second, so a
receipt dropped between the two is covered by the ledger it then reads.
The comparison is between numbers and never between times: a wall clock that
steps back would stamp a new receipt earlier than a time a client had already
been told, and a time-based watermark would then prove an absence that is not
one.

`absent_safe` says the command has not run *yet*. The first send may still be
in flight on the connection that failed, which is why the resend uses the
same key and the same fields: whichever of the two commits first is the one
execution, and the other is answered with its receipt.

**The algorithm.** After every `Welcome`, for each outbox entry in the order
the entries were first sent:

1. If the `Welcome` has no `submission_horizon`, the host cannot resolve
   (protocol 17). Mark the entry unconfirmed and send nothing for it.
2. Otherwise send `resolve_submission` with the entry's command and the
   entry's own horizon, omitting `horizon` when the entry has none.
3. `executed`: the command ran. Treat `reply` as the reply to the original
   send, and remove the entry.
4. `absent_safe`, for an entry that was never marked unconfirmed: send the
   command again, unchanged, with the same key. A reply or an error reply is
   the answer to the original send; remove the entry. If the connection fails
   first, keep the entry exactly as it was, with its original horizon, and
   resolve it again on the next connection.
5. `unknown`, an error reply, or `absent_safe` for an entry already marked
   unconfirmed: mark the entry unconfirmed with the reason, keep it, and go
   on to the next entry. Never resend it automatically.
6. If the connection fails while an entry is being resolved, leave that
   entry and the rest as they are and resolve them on the next connection.

An unconfirmed entry is shown to the person as **delivery unconfirmed**, on
every connection until they decide it, with exactly three choices, each of
which the client records:

- **Mark delivered**: it did run. Remove the entry; send nothing.
- **Send again**: send the command unchanged under the same key, knowing it
  runs a second time if the first ran and its receipt is gone.
- **Discard**: it is no longer wanted. Remove the entry; send nothing.

Unconfirmed is permanent until a person decides. A later `absent_safe` for
the same entry is not acted on: a store rolled back to an older copy of
itself answers `horizon_ahead` only until it has recorded as many receipts
again, and a client that once heard the host could not tell must not start
believing it.

New commands are sent while unconfirmed entries wait. An unconfirmed entry
never blocks the connection, the subscription or the rest of the outbox.

Only one process acts on an entry at a time. A command holds an advisory lock
(`flock`) on its entry from before it is first sent until it is answered, and
a connection resolving the outbox, or a person settling an entry, takes the
same lock first and reads the entry again once it holds it. An entry locked by
a live process is left alone, so a second `aether --remote NAME` cannot ask the
host about a send that is still in flight and then write an unconfirmed mark
over an entry the first has since retired. The lock is released when the
process exits, so a crashed sender's entry is resolved by the next
connection.

A client that resolves on a long-lived connection uses the horizon of the
connection each command was first sent on, not a newer one: a later horizon
vouches for receipts dropped in between.

### Full resync

1. `subscribe` with `after: null` (live only). Note `subscribed.last_seq`.
   Buffer the events that arrive.
2. Read the snapshots: `get_tree`, `get_attention`, and whatever view is
   open (`get_transcript`, `list_tasks`, …).
3. Apply the buffered events, then continue live.

### Applying events

- Apply an event only if `event.seq > last_seq`, then set `last_seq`.
  Anything else is a duplicate and is dropped.
- Event application replaces state and never accumulates it, so an event
  already reflected in a snapshot (step 2 above can read state newer than
  the subscription's start) is harmless to apply again.
- `seq` is per daemon instance. Never compare sequence numbers across
  `instance_id`s.

## Error codes

Protocol errors (`ErrorReply.code`):

| Code | Meaning |
| --- | --- |
| `incompatible` | Protocol version outside the served window, or the first frame was not `hello`. |
| `unauthorized` | The hello was refused: no or wrong token, or device authentication failed. |
| `forbidden` | **New in 18.** Authenticated, but this connection's transport or role may not run the command. |
| `not_found` | No such agent, task, device, pairing…; also a `subscribe` or `get_events` whose history has expired. |
| `invalid_transition` | The target is not in a state that allows this. |
| `invalid_argument` | A malformed or contradictory request; a stale `expected_revision`; an idempotency key reused with different fields. |
| `unavailable` | A provider, model or service is not configured or not reachable; push is disabled; the connection already holds 16 event subscriptions. |
| `unimplemented` | **New in 18.** The command is part of the protocol and this build does not serve it. |
| `internal` | A daemon fault. |

A protocol 17 client receives `forbidden` as `unauthorized` and
`unimplemented` as `unavailable`.

HTTP error codes are in [Errors](#errors); WebSocket close codes in
[Close codes](#close-codes).

## SSH remotes

A named remote is another machine's daemon reached through SSH. No listener,
token or pairing is involved: SSH authenticates the user, and the bridge
connects to that user's existing Unix socket. Start the daemon on the host
before adding a profile.

```
aether remote add box --ssh me@box
aether --remote box status
aether --remote box agents
aether --remote box run --workspace /srv/project "fix the tests"
aether --remote box                    # the ordinary TUI on this host
```

Profiles live in `<configuration directory>/remotes.toml`. They contain
connection coordinates, never passwords, tokens or private keys:

```toml
[remotes.box]
ssh = "me@box"
executable = "/home/me/.local/bin/aether" # optional; defaults to aether
socket = "/run/user/1000/aether/aetherd.sock" # optional; host default otherwise
```

- `NAME` is 1 to 32 characters of `a-z`, `0-9`, `-` and `_`. `TARGET`
  accepts SSH destinations and `Host` aliases, with no leading `-`, control
  characters or whitespace. Unknown profile fields and invalid profiles are
  refused. Add verifies connectivity and performs the protocol handshake
  before atomically saving anything. Existing names must be removed before
  replacement. List and remove work without contacting the host.
- `remote add` accepts `--executable PATH` and `--remote-socket PATH` for
  hosts whose noninteractive SSH environment uses different paths. These
  overrides can also be edited in the profile. Every remote command argument
  is quoted for the host shell; the local client spawns SSH directly.
- The carrier runs `ssh -T TARGET 'aether' --local bridge --stdio`, with
  `BatchMode=yes`, `ConnectTimeout=10`, `ServerAliveInterval=15` and
  `ServerAliveCountMax=3`. SSH configuration, ProxyJump, identity selection and
  host-key verification remain SSH's responsibility. Set up key or agent
  authentication and known hosts using SSH before connecting. Authentication
  and bridge failures include the tail of SSH stderr before the TUI enters
  its terminal mode. The client drains stderr continuously, owns its child,
  and kills and reaps it when the last connection owner is dropped.
- `--remote NAME` or `AETHER_REMOTE` selects the remote for daemon commands.
  `--local` ignores `AETHER_REMOTE`; an explicitly typed `--socket` overrides
  an environment-only remote. Typing `--remote` with `--socket` or `--local`
  is exit code 2. A typed remote overrides `AETHER_SOCKET`. Profile management
  (`remote add`, `list`, `remove`) always operates on the client's profiles.
- Protocols 17 through 20 are supported. If the newest hello is refused, the
  client opens a fresh SSH carrier and tries the newest older version the
  daemon's refusal says it serves, down to 17, as it does on the Unix socket
  and the token listener ([versions](protocol.md#versions)).
  Incompatible errors report the client version and the daemon's served
  window. They never suggest automatically restarting the host's daemon.
  Protocol 17 hosts can run existing fleet commands; protocol 18 host
  services return a clear unavailable message on those hosts, and so does a
  message with images on a host older than protocol 19.
- `aether bridge --stdio` is a pure byte pump. It reads no configuration,
  interprets no frame, sends no additional hello and never starts a daemon.
  Missing sockets and I/O failures print diagnostics only on stderr and exit
  1; orderly EOF exits 0. Closing stdin half-closes the Unix socket's write
  side and drains its remaining response. Daemon EOF ends the bridge even
  while stdin is still open.
- The bridge reaches the daemon as a **local** authorized connection,
  with provenance `user:local_socket#…`. Neither a remote listener nor device
  pairing is required.
- The TUI displays `box · connected` and the SSH host identity. Workspace
  choice, directory and `@` completion, and git summaries use the host-service
  protocol. Unsupported services display an explanation instead of examining
  laptop paths. Remote `run` requires an explicit `--workspace`, validated on
  the host; the remote TUI requires choosing a workspace before starting an
  agent and does not create an automatic root.
- Preferences, drafts, queued sends, cursors and the keyed outbox use
  `<configuration directory>/endpoints/<endpoint-and-instance digest>/`.
  Local sessions keep using `<configuration directory>/ui.toml`. Custom themes
  and modes remain shared in `<configuration directory>/themes/` and `modes/`,
  including interactive `/theme NAME` and `/mode reload`.
  Remote mutations with supported idempotency keys are saved before sending,
  with the host's `submission_horizon`, and retired after a reply. Losing an
  acknowledgement says **delivery unconfirmed**. The next connection, by any
  command or the TUI, asks the host what became of each saved send
  ([resolving a lost acknowledgement](#resolving-a-lost-acknowledgement)): a
  receipt the host still holds retires it and is reported as `confirmed
  queued remote <command>`; a send the host proves never ran is sent again
  under the same key; one the host cannot account for stays **delivery
  unconfirmed** and never holds up the connection. A protocol 17 host cannot
  be asked, so every saved send stays unconfirmed there. Nothing is resent
  without proof or a person's say-so. A file in the outbox that is not a
  readable operation never holds up a connection either: `outbox` lists it as
  unreadable with its path, and only `outbox discard` (or `x` in `/outbox`)
  removes it. `aether --remote NAME outbox` lists the
  unconfirmed sends and `outbox delivered KEY`, `outbox resend KEY` and
  `outbox discard KEY` decide one; the TUI shows `N unconfirmed /outbox` on
  its status line and `/outbox` decides them with `d`, `r` and `x`, each
  confirmed with Enter. Every outcome, automatic or chosen, is appended to
  `outbox/settled.jsonl` in the endpoint's directory. An endpoint holds at
  most 1024 unresolved sends; beyond that new keyed commands are refused
  until some are settled. A changed
  `Welcome.instance_id` selects a different namespace, clears the old TUI
  state and visibly reloads. Expired history clears the cursor and reloads
  current snapshots after subscribing live. The TUI reconnects through the
  selected endpoint with cancellable exponential backoff from 1 to 30 seconds
  and jitter. `run` also reconnects to its selected host and refuses to reuse
  its cursor across a daemon identity change.
- Local machine features are refused with a remote selected: `daemon`,
  `serve`, `update`, `login`, `logout`, `diagnose`, `doctor`, `bridge`,
  `mcp-bridge`, voice, and run export/import through local paths. Provider
  login and voice actions are refused in the remote TUI as well. Run these
  features using `aether --local` on the host.

## Command line

```
aether remote setup [--yes] [--json] [--proxy-url URL | --funnel]
                                          guided: Tailscale, enable, pair
aether remote status [--json]             listener, origins, devices, pairing
aether remote enable [--origin URL]… [--no-pairing] [--push | --no-push] [--json]
aether remote disable [--json]
aether remote pair [--role operator|observer]
aether remote devices [--all] [--json]
aether remote revoke DEVICE [--reason TEXT] [--json]
aether remote rename DEVICE NAME [--json]

aether remote add NAME --ssh TARGET [--executable PATH] [--remote-socket PATH] [--json]
aether remote list [--json]
aether remote remove NAME [--json]

aether bridge --stdio
aether --remote NAME <command>
aether --remote NAME outbox [list [--json]]
aether --remote NAME outbox delivered KEY [--json]
aether --remote NAME outbox resend KEY [--json]
aether --remote NAME outbox discard KEY [--json]
aether --local <command>
```

`DEVICE` is a device id or an unambiguous name. Everything under
`aether remote` except `add`, `list` and `remove` talks to the local daemon
and is refused with `--remote`.

`aether outbox` is about the sends to one SSH remote whose delivery the host
could not confirm (see [SSH remotes](#ssh-remotes)). `KEY` is the send's
idempotency key as `outbox list` prints it. `delivered` and `discard` send
nothing and only record the decision; `resend` sends the command unchanged
under the same key and prints the host's reply, and a refusal still settles
it. With `--json` each prints `{"key", "decision", "reply"}`, where `decision`
is `marked_delivered`, `sent_again` or `discarded` and `reply` is the host's
answer to a resend and `null` otherwise. Against the local daemon the outbox
is always empty.

An outbox file that cannot be read back as an operation is listed after the
unconfirmed sends, keyed by its file name without `.json`, with the reason
and its path. It can only be discarded; `outbox delivered` and `resend` refuse
it. In `--json` it appears in the same array as `{"key", "path", "reason"}`
and discarding it is logged in `outbox/settled.jsonl`.

`aether remote setup` is the guided path, written for someone who has never
used Tailscale; see [Use Aether from your phone](remote-setup.md). It checks
that Tailscale is installed, signed in, and has MagicDNS and HTTPS
certificates, printing the exact steps for the platform when not. It enables
the listener with the machine's `https://<name>.<tailnet>.ts.net` origin,
runs `tailscale serve --bg --https=443 http://127.0.0.1:8377`, checks that the
address answers `GET /api/health` and is on the allowlist, and pairs a phone.
Every step detects existing state and is skipped or repaired, so the command
can be run again. It runs no `sudo` and no shell: where one is needed it prints
the command and waits.

| Flag | Effect |
| --- | --- |
| `--yes`, `-y` | Never ask or wait. Stops at what only a person can do and reports it. Pairing is never started: the report ends with `aether remote pair` as the next step, and no device is accepted. |
| `--json` | One report object on stdout (`status`, `steps`, `next`). Changes nothing without `--yes`. The exit code is zero when a report was produced. |
| `--proxy-url URL` | An `https://host[:port]` that already reaches the listener. Skips the Tailscale steps. The address is saved as a browser writes it: host in lower case, `:443` left out. |
| `--funnel` | Publish with `tailscale funnel` to the public internet, after a typed confirmation. Not with `--proxy-url`. |

`aether remote pair` prints the code and a QR of the pairing URL, waits for
a device to claim it, shows the device's name, fingerprint and phrase, and
asks for confirmation. Only `y` or `yes` enrols; any other answer, or none
because the terminal closed, rejects the device. `Ctrl+C` cancels the
pairing. The command fails when nothing was decided: the code expired or
the pairing was cancelled.

`aether remote status --json` prints `{"status": RemoteStatus, "devices":
Device[]}`; `enable` and `disable` print the `RemoteStatus`, `devices` the
`Device[]`, and `revoke` and `rename` the `Device`. `aether remote enable`
keeps the stored origins unless `--origin` is given, keeps the stored push
setting unless `--push` or `--no-push` is given, and fails if the listener
could not be opened.

Memory proposals stay out of recall until the host accepts them. Rejection
uses the same curation as `forget_memory`; `restore_memory` reopens review
without admitting the lesson. See [memory proposals](protocol.md#memory-proposals)
for the protocol and CLI workflow. Paired devices cannot create, list or review
proposals; ordinary memory commands keep their existing access.
