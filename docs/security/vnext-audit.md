# vNext security audit: Aether (H3)

Task H3 of the vNext plan (mengsig/archon#388, umbrella mengsig/archon#377).
It covers the Aether side of five areas:

- the Electron shell (vNext `ARCHITECTURE.md` §c);
- remote access (§d);
- the `/archon` path mount (AE-M);
- pairing across the shared origin;
- Noesis data boundaries (§g, `CONTRACTS.md` §6.2–6.3).

The Archon side is audited separately, in the Archon repository's own
`docs/security/vnext-audit.md`.

- **Audited tree:** `84d897dd` (the integration PR #414 head this item froze on).
- **Method:** a source read of every path named below, the existing tests named
  as evidence, and targeted new tests where a claim had none. Every finding
  fixed here has a test that fails without its fix.

## Severity

| Level | Meaning |
| --- | --- |
| high | A remote or observer role can read secrets or sealed data, or write, run or escalate beyond its class. Or a renderer can reach native power it should not have. |
| medium | A defence that §c/§d names is missing, but another layer still blocks the attack. |
| low | Hardening, documentation drift, or a residual risk the spec accepts. |

## Findings

| Id | Severity | Area | Status |
| --- | --- | --- | --- |
| H3-A1 | high | Noesis / remote | Fixed in this change: `noesis_import_source` masks the artifact as `read_artifact` does |
| H3-A2 | low | Remote docs | Fixed in this change: the matrix in `docs/remote.md` now lists all 134 commands, enforced by a test |
| H3-L1 | low | Electron | Follow-up |
| H3-L2 | low | Electron | Follow-up |
| H3-L3 | low | Electron packaging | Follow-up |
| H3-L4 | low | Electron update | Follow-up (residual risk, documented in the prompt) |
| H3-L5 | low | Electron confirmation | Follow-up |
| H3-L6 | low | Mount | Follow-up |
| H3-L7 | low | Noesis | Follow-up (residual risk §g accepts) |
| H3-L8 | low | Noesis | Follow-up |
| H3-L9 | low | Noesis | Follow-up |
| H3-L10 | low | Noesis | Follow-up |
| H3-L11 | low | Noesis tests | Follow-up |
| H3-L12 | low | Noesis frontend | Follow-up |
| H3-L13 | low | Noesis settings | Follow-up (spec question) |

No medium finding was found. No high finding is open.

### H3-A1 (high, fixed): an imported source bypassed artifact masking

`read_artifact` masks every reply with the runtime redactor
(`crates/aether-daemon/src/connection/dispatch/reads.rs`, `read_artifact`). A
client's published artifact is stored as sent (`crates/aether-runtime/src/artifacts.rs`
module doc). `noesis_import_source` (operate) read the same bytes with no
mask, in `jobs::read_source` (`crates/aether-daemon/src/noesis/graph/jobs.rs`).
The extraction worker then put the text verbatim into its extractor agent's
task objective (`AgentModel::invoke`, same file). The task row and its
`task.created` event are readable by every observer (`get_task`,
`list_tasks`, `get_events` are `observe`).

**Scenario.** The host publishes notes that contain a GitHub token. An observer
device reading the artifact sees `[REDACTED:github_token]`. Then an operator
device, or the host, imports the artifact as a Noesis source. Every observer
can now read the raw token from the extractor's task objective. The model
provider also receives it.

**Fix.** `read_source` takes the runtime's redactor and returns the masked
text. `StoreCorpus` carries the redactor too, so the source ID derivation, the
worker's prompt and the task objective all see only masked text.

**Test.** `noesis::graph::tests::an_imported_source_reaches_its_extractor_masked_as_read_artifact_shows_it`.
It publishes a token and imports it. It then asserts two things: the
extractor task's objective holds `[REDACTED:github_token]` and not the token,
and no stored event carries the token. Without the fix, the first assertion
fails.

### H3-A2 (low, fixed): the remote matrix omitted 27 commands

`docs/remote.md` says its matrix is exhaustive, but it had no row for:

- `storage_status` and `storage_gc`;
- the six notification commands;
- `refresh_provider_usage`;
- all 18 `noesis_*` commands.

Enforcement was right, because the classes come from code. The document was
not.

**Fix.** The rows are added, with the field rule for `noesis_configure`.

**Test.** `authz::tests::the_documented_matrix_names_every_command_with_its_class`
reads the matrix and requires one row per fixture case of
`authz/command_cases.json`, with the same class.

## ARCHITECTURE §c: the Electron shell

Paths are under `web/apps/desktop/`. "Headless check" means a named check in
`test/security/electron/{app,remote}-checks.ts`. They run in a real Electron
under `test/security/electron.test.ts` ("the desktop app passes every
headless security check"), from `make web`.

| # | Requirement | Status | Evidence |
| --- | --- | --- | --- |
| c1 | `contextIsolation`, `sandbox`, `nodeIntegration: false`, `webviewTag: false`, `webSecurity: true` | ✅ | `src/main/app-window.ts:21-29`; `app.enableSandbox()` at `src/main/index.ts:10`. Test `windows.test.ts` "the app window has exactly the hardened options and the one preload"; headless check "app window preferences". |
| c2 | The preload exposes one frozen `window.desktop` through `contextBridge`, and nothing else | ✅ | `src/preload/index.cjs:92`. Headless check "window.desktop is the only bridge" pins the exact key list, `frozen: true`, and no `require` or `process`. |
| c3 | Navigation pinned to `app://`; `setWindowOpenHandler` denies | ✅ | `src/security/navigation.ts:20-30` (`will-navigate`, `will-frame-navigate`, `will-redirect`, webview, window open); every new `webContents` also refuses webviews and new windows (`src/main/start.ts:109-112`). Headless checks "app window navigation is pinned", "app window opens no windows". |
| c4 | `openExternal` accepts only `https:` | ✅ | `externalUrl` at `src/bridge/bridge.ts:134-140` also refuses userinfo. |
| c5 | Permission handlers deny by default, notifications only | ✅ | `src/security/navigation.ts:32-36`. Test "the permission handlers allow notifications and nothing else"; headless check "remote window gets no permissions". |
| c6 | Renderer CSP `default-src 'self'`, `connect-src 'self'`, set by main on every `app://` response | ✅ | `src/security/csp.ts:9-30`; `serveApp` adds `APP_HEADERS` to every response, 404 and 405 included (`src/main/app-protocol.ts`). Headless checks "app CSP blocks the remote origin", "app window has no seen request at the remote origin". |
| c7 | `app://` serves only the bundle | ✅ | Segment allowlist and containment assertion in `src/main/app-protocol.ts:25-33`. Headless check "app:// refuses paths outside the bundle" tries `..`, `%2e%2e`, a dot file, an encoded slash and another host. |
| c8 | Each IPC call is checked for the sender frame (top-level `app://` document) | ✅ | `checkSender` at `src/security/validate.ts:196-203`, fed by `senderOf` (`src/main/host.ts:21-28`), which requires a registered app window, the main frame and an `app://aether` URL. Headless checks "subframes get no bridge", "no IPC handler answers a remote window". |
| c9 | Payload against the generated JSON Schema; frames ≤ 1 MiB, strings ≤ 4 KiB | ✅ | `Validator.check` (`src/security/validate.ts:87-99`) checks sizes before the sender, then re-parses the JSON form, so the bytes it validates are the bytes it sends. Frames go through `FrameSchema` against `clients/protocol.schema.json` and the vendored Archon schema (`src/security/schema.ts`). Limits at `src/security/limits.ts:4-7`. Test `validate.test.ts`; headless check "bridge refuses …" sends a 1 MiB `send_message`. |
| c10 | Class allowlist: Aether observe/operate and Archon Read/Change (remote allowlist) pass `request`; host_only, remote_admin, OperatorRead and OperatorChange never pass; `shutdown` never sent | ✅ | `passThroughRefusal` (`src/security/classes.ts:129-144`) mirrors the field rules of `Command::access`. Test `classes.test.ts` parses `crates/aether-protocol/src/access.rs` and fails when any schema command lacks a class or a static arm differs. G3's `memory_export`, `memory_import`, `memory_feedback`, `memory_retrieval_explain` and `consolidate_memories` are host_only. `noesis_configure` follows `configure_access`. Archon `pause`/`resume` with `hard` are refused. Headless check "bridge refuses shutdown, host-only and remote-admin commands". |
| c11 | Privileged actions are a closed set, local connections only; main builds the frame and the confirmation; the renderer supplies no frame and no text; confirmation is bound to the exact request | ✅ | The `PrivilegedAction` schema and the connection-kind check (`src/security/validate.ts:136-145`). `planAction` builds each frame from validated fields (`src/bridge/privileged.ts`). Templates are main's own (`src/security/confirm.ts:127-152`) and escape control, format and bidi characters (`:37`). A rendering over 60 lines is refused (`:112`). Remote settings carry `expected_revision`, so a change between confirmation and send is refused (`privileged.ts:60,66`). The confirmed text is sent on main's own control link (`src/bridge/bridge.ts:83-101`). Tests `confirm.test.ts`; headless checks "privileged action sends the confirmed frame", "privileged action refused when cancelled". See H3-L5. |
| c12 | No raw bytes, socket path, shell command or filesystem path from the renderer; a picked path comes back as an opaque token | ✅ | `discover` returns no socket (`publicDaemon`, `src/bridge/bridge.ts:127-131`); `connect` takes a daemon id pattern (`validate.ts:50`). `pickDirectory` returns a single-use grant redeemed only in main (`src/native/folder-tokens.ts`). `engine.login` runs a fixed argv from `LOGIN_COMMANDS` (`privileged.ts:24-27`), checked as plain words in `openTerminal` (`src/main/host.ts:63-78`). |
| c13 | Credentials never cross IPC | ✅ | No bridge method takes or returns a credential (`ARGUMENTS`, `validate.ts:47-65`). Engine logins run in a terminal. Test `credentials.test.ts`. |
| c14 | Socket discovery by the CLIs' rules | ✅ | `src/discovery/socket-paths.ts`; test `lifecycle/discovery.test.ts`. |
| c15 | Managed start off by default, detached, never Electron's child; Electron never sends `shutdown` | ✅ | `ManagedStart.start` refuses unless `desktop.managed_start` is `true` (`src/lifecycle/managed-start.ts:101`). The process is spawned `detached`, stdin closed, output to a log, `unref` (`src/lifecycle/launch.ts:29-49`). `shutdown` is refused on every path (c10). Tests `managed-start.test.ts`, `never-stops.test.ts`, `daemon-survives.test.ts`. See H3-L1. |
| c16 | Remote daemons only in a remote window, never in `app://` | ✅ | `openRemote` accepts only an entry of `remote.origins` (`bridge.ts:105-111`), normalized by `normalizeOrigin` (`src/security/origins.ts:28-41`). Headless checks "openRemote refuses http, paths, userinfo and unlisted origins", "openRemote opens a listed origin". |
| c17 | Remote window: no preload, no bridge, same baseline options | ✅ | `src/remote/remote-window.ts` has no `preload`. Test "a remote window has no preload and its own per-origin partition"; headless checks "remote window preferences", "remote window authenticates with Origin and a SameSite=Strict cookie" (`desktop`, `require` and `process` are undefined). |
| c18 | Its own partition `persist:remote:<sha256(origin)>`; removing the origin clears it | ✅ | `remotePartition` (`origins.ts:53-56`), with the partition identity asserted (`remote-windows.ts:36`). `forget` clears storage (`remote-windows.ts:54-61`), wired from `PreferenceStore.set` on removal. Headless checks "remote window has its own partition", "app window has no request in the remote session", "removing an origin clears its partition". |
| c19 | Remote navigation and redirects pinned to the exact origin; window open denied | ✅ | `pinContents(contents, origin)` (`remote-windows.ts:38`). Headless checks "remote navigation is pinned to its origin", "remote redirects are pinned to its origin", "remote window opens no windows". |
| c20 | Certificate errors never overridden | ✅ | No `certificate-error` handler or `setCertificateVerifyProc` exists under `src/` (`grep -rn "certificate-error\|setCertificateVerifyProc" web/apps/desktop/src` prints nothing). Only the test harness trusts its own fixture certificate (`test/security/electron/harness.ts:18-22`). |
| c21 | Main injects nothing into a remote window and holds no IPC channel to it; local connection ids are bound to their owner | ✅ | `RemoteWindows.open` loads the origin and nothing more. Every IPC sender must be a registered app window (c8). Connections are bound to the opening `webContents` (`src/bridge/connections.ts:60-91`). Headless check "no IPC handler answers a remote window". |
| c22 | Updates via electron-updater against Aether's GitHub releases; the user confirms each step; downgrades refused | ✅ | `lockDown` turns off auto-download, install on quit, downgrade and prerelease, and the update flow asserts it again when it is built (`src/update/updates.ts:41-53,98`). The feed version must match `VERSION` before it is shown (`:38`). Download and install each need a native confirmation. The feed is pinned (`UPDATE_FEED`, `electron-builder.yml` `publish`). Tests `packaging/updates.test.ts`. See H3-L4. |
| c23 | Signature checks | ✅ with residual risk | An unsigned macOS build never self-updates (`updatesSupported`, `updates.ts:134-137`), because Squirrel.Mac verifies the code signature. On Linux, electron-updater checks the download's SHA-512 from the release metadata. The prompt says so (`signingNote`, `updates.ts:55-61`). See H3-L4. |
| c24 | Packaged app runs as Electron only | ✅ | Fuses `runAsNode: false`, `enableNodeOptionsEnvironmentVariable: false`, `enableNodeCliInspectArguments: false`, `onlyLoadAppFromAsar: true` (`electron-builder.yml:45-49`). Test `packaging.test.ts`. See H3-L3. |

## ARCHITECTURE §d: remote access

| # | Requirement | Status | Evidence |
| --- | --- | --- | --- |
| d1 | Private by default: listener off until enabled | ✅ | `RemoteSettings` derives `Default`, so `enabled` starts false (`crates/aether-protocol/src/remote.rs:86-90`). Disabled settings close the listener and cancel live pairings, and no pairing opens while disabled (`crates/aether-remote/src/lib.rs:318-322,452`). |
| d2 | Binding and Tailscale assumptions | ✅ | Default `127.0.0.1:8377`. A non-loopback bind needs `allow_remote = true` and is refused otherwise, because the listener speaks plaintext HTTP (`crates/aether-remote/src/listener.rs:46-55`; test `a_non_loopback_bind_needs_the_operator_to_mean_it`). TLS comes from `tailscale serve` or a local reverse proxy (`docs/remote.md`). |
| d3 | Host anti-rebinding check | ✅ | `check_host` runs before routing, mounts included. It needs exactly one `Host` and also checks an absolute-form authority (`crates/aether-remote/src/http/respond.rs:100,185-196`). Test `a_host_this_listener_is_not_known_by_is_refused_first`. |
| d4 | Origin and `Sec-Fetch-Site` checks | ✅ | `check_origin` (`respond.rs:215-229`) for the listener's POST routes and `/ws`. Test `posts_and_upgrades_need_an_allowlisted_same_origin_request`. |
| d5 | CSP on every response | ✅ | `harden` (`respond.rs:488-500`) applies `default-src 'none'; script-src 'self'; connect-src 'self'; …` (`crates/aether-remote/src/http.rs:246-249`). Test `every_response_carries_the_csp_and_the_security_headers`. |
| d6 | Pairing codes and phrases | ✅ | 40-bit Crockford code from the CSPRNG, 120 s expiry, single use, host confirmation required (`crates/aether-remote/src/pairing.rs`). Guesses are rate-limited to `MAX_GUESSES_PER_SECOND = 10` (so at most 1200 over a code's life), each wrong code waits 250 ms, and the compare is constant-time (`pairing.rs:20-37,380`; `crates/aether-protocol/src/auth.rs:16-21`). The three-word comparison phrase hashes the pairing id and the device's public key (`phrase`, `pairing.rs:87-99`). Tests `pairing/tests.rs`: `a_code_is_forty_bits_of_crockford_base32`, `a_code_is_single_use`, `an_unclaimed_code_expires_and_so_does_an_unconfirmed_claim`, `a_flood_gets_no_more_guesses_than_the_documented_bound`. |
| d7 | Device identity | ✅ | Aether devices hold no bearer token. They hold a non-extractable P-256 key in IndexedDB (`web/packages/core/src/transports/identity.ts:48,73-74`). The daemon stores only the public point and verifies ECDSA over a 32-byte nonce bound to origin, instance and device (`crates/aether-remote/src/auth.rs:64-77,104-117`). The legacy `[listen]` token uses a constant-time compare (`crates/aether-daemon/src/listen.rs`). |
| d8 | Role chosen by the host at pairing | ✅ | `start_pairing {role}` is `remote_admin` (Unix socket only). The role is stored at enrolment and never taken from the claimant (`pairing.rs`). |
| d9 | Access classes of every command, including G3 and Noesis | ✅ | `Command::access` is one exhaustive match with no wildcard (`crates/aether-protocol/src/access.rs:78-150`). The matrix is `permits` in `crates/aether-daemon/src/authz.rs:113-126`. `authz::tests::every_command_has_an_explicit_role_decision` derives all 134 tags from the generated schema and checks each against `authz/command_cases.json` in every context. G3: `memory_export`, `memory_import` (both `dry_run`), `memory_feedback`, `memory_retrieval_explain`, `record_memory_feedback`, `retract_memory_feedback`, `consolidate_memories` and `memories_in_context` are host_only; `memory_migration_status` is observe (test `memory_v2_host_commands_refuse_every_paired_role_and_status_admits_observers`). Noesis: reads are observe and mutations operate. `noesis_configure` is host_only when it sets `enabled`, a budget, `billing.allow_api` or `prompts.overrides` (`configure_access`, `crates/aether-protocol/src/noesis.rs:186`, as `CONTRACTS.md` §6.1 says; tests `host_only_keys_make_the_patch_host_only`, `the_other_keys_and_an_empty_patch_are_operate`). `docs/remote.md` now agrees row by row (H3-A2). |
| d10 | Observer limits | ✅ | An observer reaches only `observe` and `device_self` (`permits`). Remote-admin events reach non-local readers as `remote.restricted` (`crates/aether-daemon/src/authz.rs:49-57`). Proposed memories are excluded from `memories` (`crates/aether-store/src/sqlite/memories.rs`). The memory-retrieval trace is stripped from every snapshot read (`crates/aether-daemon/src/connection/dispatch/reads.rs:83-91`). Artifact reads are masked. The Noesis import path now is too (H3-A1). |
| d11 | Revocation | ✅ | `revoke_device` sets `revoked_at`, cancels the device's sessions and forgets its push subscription (`revoke`, `crates/aether-daemon/src/remote/admin.rs:309-338`). Auth rechecks after registering the cancel (`admit`, `crates/aether-remote/src/auth.rs:213-247`). Each command checks cancellation first (`authorize`, `crates/aether-daemon/src/authz.rs:129-135`). The socket closes with 4403. Tests `a_device_pairs_connects_runs_a_command_and_is_revoked` (`crates/aether-daemon/tests/remote.rs`), `revoking_a_device_closes_its_connection_with_4403`, `revocation_discards_a_stalled_large_response_and_ends_the_carrier`. |
| d12 | A remote browser cannot reach a Unix socket | ✅ | The listener's only routes are `/api/health`, pairing, `/ws`, the assets and mounts (`http.rs` `route`). Mount targets are loopback TCP only (d14). |
| d13 | Mounts configured only by host-only commands or the CLI, at most 4, fixed lowercase prefixes | ✅ | `configure_remote` is `remote_admin`. `validate_mounts` enforces the limit of 4, the lowercase segment and the loopback target (`crates/aether-protocol/src/remote/mounts.rs:29`; tests `loopback_mounts_with_one_lowercase_segment_are_accepted`, `more_than_four_mounts_or_a_repeated_prefix_is_refused`). A prefix over the listener's own paths is refused (`is_served_here`, `crates/aether-remote/src/mount.rs`; test `prefixes_the_listener_serves_itself_cannot_be_mounted`). |
| d14 | Targets must be loopback | ✅ | Checked when saved, and again at routing by `target_address` (`mount.rs`), which builds only `127.0.0.1` or `[::1]`. `forward` asserts it. Test `a_target_is_used_only_when_it_is_exactly_loopback_http`. |
| d15 | Funnel never reaches a mount; status lease of 5 s | ✅ | A mount is added only while a fresh status read shows no Funnel (`crates/aether-daemon/src/remote/admin.rs:199-215`; tests `a_mount_is_added_only_while_a_fresh_status_read_shows_no_funnel`, `under_a_funnel_mounts_may_be_kept_or_removed_but_not_changed`). Setup refuses Funnel while a mount exists (`crates/aether/src/cli/remote/setup/tests/mount_flows.rs`). The lease and its 1008 closes are tested in `mount/lease/tests.rs` and `mount/tests.rs` (`an_out_of_band_funnel_closes_tunnels_with_1008_and_refuses_requests_within_5s`, `a_failing_or_hanging_status_read_closes_the_tunnel_within_5s`, `no_frame_reaches_the_target_once_the_lease_is_over_even_before_its_timer_fires`). |
| d16 | Preserved: revocation close codes, Host check, CSP, Funnel refusal, idempotency receipts, `resolve_submission` | ✅ | d3, d5, d11 and d15. `resolve_submission` takes the class of the command it names (`access.rs`, test `resolving_a_submission_needs_the_standing_of_the_command_it_names`). |

## The mount (AE-M)

| # | Check | Status | Evidence |
| --- | --- | --- | --- |
| m1 | What is forwarded | ✅ | Method, the path with the prefix stripped, the query, end-to-end headers and the body (`outbound`, `crates/aether-remote/src/mount/forward.rs`). `Origin` passes unchanged, so archond makes its own Origin decision. |
| m2 | What is stripped or added | ✅ | Hop-by-hop headers and every header `Connection` names are stripped (`end_to_end`, `mount.rs`). `Host` is set to the target. `Sec-WebSocket-Extensions` is dropped, so frames relay byte for byte. Nothing is added: no device identity, no `X-Forwarded-*`, no credential. Test `a_mounted_request_reaches_the_target_with_only_host_changed_and_an_observer_adds_nothing`. Cookies pass as they came. Aether sets no cookie, so the only cookie a browser sends under `/archon` is archond's own (`Path=/archon`). |
| m3 | Path traversal and prefix confusion | ✅ | `under` matches the prefix or the prefix plus `/`, never a bare letter prefix (`/archonx` is not mounted). The rest must pass `is_clean_path`, which refuses empty, `.` and `..` segments, `%` and `\`. Matching is case-sensitive, so `/ARCHON` is never forwarded. The listener's own endpoints are checked first (`is_endpoint`). Tests `only_the_prefix_and_what_is_below_it_are_mounted`, `paths_under_a_mount_that_are_not_clean_are_refused_and_never_forwarded`, `a_mount_over_the_api_never_takes_health_from_the_listener`. |
| m4 | Limits | ✅ | Body cap before forwarding (test `a_body_over_the_listener_limit_is_refused_and_never_forwarded`), answers capped at 8 MiB, frame size via `ws::MAX_MESSAGE` (`mount/tunnel.rs`), and a semaphore of mounted connections (`mount_slots`, `crates/aether-remote/src/lib.rs:167`). |
| m5 | CSRF and Origin on the shared origin | ✅ | Aether's own state-changing routes check Origin and `Sec-Fetch-Site` (d4). Aether authenticates in-band over `/ws` with a signed challenge and uses no ambient credential, so there is nothing to forge cross-site. Mounted requests carry the browser's `Origin`. Archond accepts only its `trusted_origins` when mounted, and its cookie is `SameSite=Strict` (Archon `crates/archon-remote/src/http/checks.rs`, `http/api.rs` `credential_cookie`). |
| m6 | A paired Aether device gets no extra Archon power | ✅ | The mount adds no credential and no header, and archond does not trust the loopback peer address. Archon's checks use `Host`, `Origin` and its own device cookie, never the peer IP or forwarded headers (Archon `http/checks.rs`). So an Aether pairing alone gets 401 from archond, and an Archon observer stays an observer. |
| m7 | Security headers of mounted answers | ✅ | `harden_missing` fills Aether's CSP and headers where archond sent none (`respond.rs:470-486`). Archond's own CSP is stricter (`default-src 'none'`, no script). See H3-L6. |

## Pairing across the shared origin

| # | Check | Status | Evidence |
| --- | --- | --- | --- |
| p1 | Aether credentials cannot be read by script on the origin | ✅ | The device key is a non-extractable `CryptoKey`, refused on load if extractable (`identity.ts:48`). Script on the origin can at most ask it to sign, never copy it. The origin's script is Aether's own bundle, under `script-src 'self'`. Archond's pages run no script. |
| p2 | Archon credentials cannot be read across apps | ✅ | Archond's cookie is `HttpOnly; SameSite=Strict; Path=/archon`, plus `Secure` on https (Archon `http/api.rs` `credential_cookie`). JavaScript cannot read it, and the browser never sends it to Aether's `/ws` or `/api/*`. |
| p3 | The two pairings cannot be confused | ✅ | Aether pairs at `/api/pair/*` and authenticates by signature on `/ws`. Archon pairs and authenticates under `/archon/api/*` and `/archon/ws` by cookie (`web/packages/core/src/transports/ws-archon.ts:9-38`). The listener refuses a mount over `/api` or `/ws` (d13). An Aether signature covers origin, instance and nonce, and Archon accepts no signature, so neither credential is accepted by the other daemon. |
| p4 | Removing a remote origin in the desktop app clears its credentials | ✅ | c18. |

## Noesis data boundaries

| # | Check | Status | Evidence |
| --- | --- | --- | --- |
| n1 | Sealed fields are replaced where the call is persisted | ✅ | `seal_commit` runs in `Runtime::apply_held_by` before write and publish, and `seal_import` on import (`crates/aether-runtime/src/lib.rs`). `seal_event` and `seal_mutations` name every variant with no wildcard (`crates/aether-runtime/src/noesis_seal.rs`). `answer_key`, `explanation` and `solution` become `"<sealed>"`. Tests in `crates/aether-runtime/src/kernel/noesis_seal.rs` and `noesis_seal/tests.rs`. |
| n2 | No `tool_input` delta for a sealed call; no thinking for Noesis agents; no sequence gap | ✅ | `StreamSeal::admits` decides before a delta is numbered (`noesis_seal.rs`, `bus.rs`). Tests `a_stream_withholds_sealed_input_and_noesis_thinking_only` and `assert_deltas_sealed_and_gapless` (`crates/aether-daemon/src/noesis/teaching_tests.rs`). Noesis teaching turns publish no deltas at all (`crates/aether-daemon/src/noesis/engine.rs`). |
| n3 | No read before reveal | ✅ | `Question` and `Problem` have no key or solution field (`crates/aether-core/src/noesis.rs`). `QuestionResult` is built only after the answer commits. `solution` appears only in `noesis.practice_concluded`, after the conclusion commits. Tests `t11_answer_keys_are_sealed_until_the_answer_is_recorded`, `t11_the_solution_is_sealed_on_every_read_path_until_conclude_and_practice_never_writes_the_graph` (`crates/aether-noesis/src/teach/parity.rs`). |
| n4 | Not in logs or error text | ✅ | No `tracing` call prints a tool input. Argument errors show the shape only (`crates/aether-noesis/src/tools/args.rs`). Unknown argument names are never echoed. `Sealer`'s `Debug` prints only a count. |
| n5 | Evidence bundle ≤ 32 KiB, shape checked, refused before storing | ✅ | `validate_bundle` (`crates/aether-protocol/src/noesis.rs`) and `evidence::validate` (`crates/aether-noesis/src/evidence/mod.rs`); refused unless `engineering_context` is in effect. Tests in `crates/aether-noesis/src/evidence/tests.rs`. |
| n6 | Noesis never fetches; the bundle holds only what the user could read | ✅ | `aether-noesis` has no network or process dependency. Test `teach/evidence_flow.rs` compares the socket table before and after. The frontend builder reads only the user's own connections and store (`web/packages/noesis/src/understand/`), shows the exact bundle and sends it on Start only. |
| n7 | Disabled by default | ✅ | Every non-service group returns `noesis_disabled` before validation (`crates/aether-daemon/src/noesis/mod.rs`); tests in `noesis/tests.rs` and `teaching_tests.rs`. See H3-L11. |
| n8 | Imported sources hold no secret a reader could not already see | ✅ after H3-A1 | H3-A1. |

## Low findings (follow-ups)

- **H3-L1 (Electron).** The renderer can set `desktop.managed_start` and
  `remote.origins` itself through `setPreference`, without a native
  confirmation (`src/main/preferences.ts`). A compromised renderer can
  therefore turn on managed start and start the daemons, or open a remote
  window on any `https:` origin. Each is bounded: the start runs only the two
  fixed launchers, and a remote window has no bridge. *Follow-up:* confirm
  these two keys natively in main.
- **H3-L2 (Electron).** Neither session has a `will-download` handler, so a
  remote page can start a download through Electron's default save flow.
  *Follow-up:* refuse downloads in remote sessions, or route them through
  `openExternal`.
- **H3-L3 (Electron packaging).** These fuses are not set:
  `enableEmbeddedAsarIntegrityValidation`, `enableCookieEncryption`,
  `grantFileProtocolExtraPrivileges: false`. *Follow-up:* set them in
  `electron-builder.yml` and pin them in `packaging.test.ts`.
- **H3-L4 (Electron update).** An unsigned Linux build verifies an update only
  by the SHA-512 in the same GitHub release's metadata. No publisher signature
  is checked. The prompt says so. *Follow-up:* sign Linux artifacts, or verify
  a detached signature against a key pinned in the app.
- **H3-L5 (Electron).** The `device.pair` confirmation names a device, but the
  frame (`start_pairing {role}`) does not carry that name; the paired device
  chooses its own. The consequential role is shown in full. *Follow-up:* drop
  the name from the template, or send it.
- **H3-L6 (mount).** A mounted answer keeps whatever CSP archond sends
  (`harden_missing`). Today that CSP is stricter than Aether's. A future,
  weaker archond CSP would apply to a page on the origin that holds Aether's
  device key. *Follow-up:* send Aether's CSP as an additional header on
  mounted answers, so the browser enforces both.
- **H3-L7 (Noesis, accepted residual risk).** A model's visible text that
  restates an answer is stored verbatim in the transcript. The runtime seal
  covers the named fields only, and the teaching loop's `Sealer`
  (`step_text`, `step`, `event`) masks only the published turn report
  (`crates/aether-noesis/src/teach/seal.rs`). §g accepts this residual risk
  ("the model can still restate an answer in its visible text; the prompts
  forbid it and parity test T5 checks it"). *Follow-up for the seal owner:*
  apply the turn's `Sealer` to `Text` blocks in `AetherEngine::record_reply`
  before commit. The loop already knows the secrets there.
- **H3-L8 (Noesis).** An approval's `argument` is computed from the raw input,
  so a sealed-named MCP tool with a remembered rule would record it unsealed
  (`crates/aether-runtime/src/turn/tools.rs`). This is contrived, because
  Noesis tools are not MCP tools. *Follow-up:* compute it from `sealed_view`.
- **H3-L9 (Noesis).** A `ToolInput` delta with an empty tool name (no
  `BlockStart` seen) is admitted, because `sealed_fields("")` is `None`.
  Teaching turns publish no deltas, so no sealed call streams today.
  *Follow-up:* withhold nameless `ToolInput` deltas for sealing-capable agents.
- **H3-L10 (Noesis, privacy).** The learner's optional `thinking` reaches the
  teacher's transcript as part of the answer tool result, where any observer
  can read it. *Follow-up:* decide in `CONTRACTS.md` §6.2 whether it is
  delivered, and document it on `AnswerInput.thinking`.
- **H3-L11 (Noesis tests).** The disabled-by-default sample lists omit five
  commands: `noesis_learner_evidence`, `noesis_correct_learner`,
  `noesis_delete_proposition`, `noesis_delete_source` and
  `noesis_import_source`. `routes.rs` falls back to `Group::Service` by
  wildcard; this fails safe. *Follow-up:* sample every `is_noesis()` command,
  and match exhaustively.
- **H3-L12 (Noesis frontend).** `checkBundle` (`web/packages/noesis/src/understand/bundle.ts:92`) does not bound `ref.id` (the
  daemon allows ≤ 200 B) or `ref.url` (≤ 500 B; `MAX_REF_ID_BYTES` and `MAX_URL_BYTES`,
  `crates/aether-noesis/src/evidence/mod.rs:23-24`), so such a bundle passes the
  preview and is refused only after Start. *Follow-up:* bound both in
  `bundle.ts`.
- **H3-L13 (Noesis settings, spec question).** `engineering_context` and
  `models.*` are `operate`, as `CONTRACTS.md` §6.1 says. So an operator device
  can choose which configured provider receives session data and bundles.
  *Follow-up:* the spec owner decides whether these keys become host_only.

## For the other repository

Read in a read-only clone of mengsig/archon, on branch
`archon/qryx61z9-vnext-phase-0-baselines-architecture-sha` (`f1dbfc2`). Only
the parts the mount depends on were read:

- `crates/archon-remote/src/http/checks.rs` (`Allowed::now`): when mounted,
  `Host` must be loopback and `Origin` one of `trusted_origins`.
- `crates/archon-remote/src/http/api.rs` (`credential_cookie`): the cookie is
  `HttpOnly; SameSite=Strict`, scoped to the mount path, `Secure` when the
  origin is https.
- `crates/archon-remote/src/assets.rs` (`content_security_policy`): every page
  is a scriptless pointer under `default-src 'none'`.

No forwarded or peer-address header is trusted (`grep -rni forwarded
crates/archon-remote/src` prints nothing). No Archon defect was found in what
the mount relies on. The Archon side's own audit covers the rest.
