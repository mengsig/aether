# vNext security audit: Aether (H3)

Task H3 of the vNext plan (mengsig/archon#388, umbrella mengsig/archon#377).
It covers the Aether side of four areas:

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

- **The web UI.** 1.3.0 ships the phone app of 1.2.11 (`crates/aether-remote/web/`)
  unchanged, under the same CSP, cookies-free pairing and Host and Origin
  checks. The desktop app and the second web UI were removed, so their
  findings and their section of this audit went with them. No daemon-side
  check was loosened for that.

## Severity

| Level | Meaning |
| --- | --- |
| high | A remote or observer role can read secrets or sealed data, or write, run or escalate beyond its class.  |
| medium | A defence that §d names is missing, but another layer still blocks the attack. |
| low | Hardening, documentation drift, or a residual risk the spec accepts. |

## Findings

| Id | Severity | Area | Status |
| --- | --- | --- | --- |
| H3-A1 | high | Noesis / remote | Fixed in this change: `noesis_import_source` masks the artifact as `read_artifact` does |
| H3-A2 | low | Remote docs | Fixed in this change: the matrix in `docs/remote.md` now lists all 134 commands, enforced by a test |
| H3-L6 | low | Mount | Follow-up |
| H3-L7 | low | Noesis | Follow-up (residual risk §g accepts) |
| H3-L8 | low | Noesis | Follow-up |
| H3-L9 | low | Noesis | Follow-up |
| H3-L10 | low | Noesis | Follow-up |
| H3-L11 | low | Noesis tests | Follow-up |
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
worker's prompt and the task objective all see only masked text. Two sources
that differ only in a masked secret now derive the same text, and so the same
source ID; that is intended.

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

## ARCHITECTURE §d: remote access

| # | Requirement | Status | Evidence |
| --- | --- | --- | --- |
| d1 | Private by default: listener off until enabled | ✅ | `RemoteSettings` derives `Default`, so `enabled` starts false (`crates/aether-protocol/src/remote.rs:86-90`). Disabled settings close the listener and cancel live pairings, and no pairing opens while disabled (`crates/aether-remote/src/lib.rs:318-322,452`). |
| d2 | Binding and Tailscale assumptions | ✅ | Default `127.0.0.1:8377`. A non-loopback bind needs `allow_remote = true` and is refused otherwise, because the listener speaks plaintext HTTP (`crates/aether-remote/src/listener.rs:46-55`; test `a_non_loopback_bind_needs_the_operator_to_mean_it`). TLS comes from `tailscale serve` or a local reverse proxy (`docs/remote.md`). |
| d3 | Host anti-rebinding check | ✅ | `check_host` runs before routing, mounts included. It needs exactly one `Host` and also checks an absolute-form authority (`crates/aether-remote/src/http/respond.rs:100,185-196`). Test `a_host_this_listener_is_not_known_by_is_refused_first`. |
| d4 | Origin and `Sec-Fetch-Site` checks | ✅ | `check_origin` (`respond.rs:215-229`) for the listener's POST routes and `/ws`. Test `posts_and_upgrades_need_an_allowlisted_same_origin_request`. |
| d5 | CSP on every response | ✅ | `harden` (`respond.rs:488-500`) applies `default-src 'none'; script-src 'self'; connect-src 'self'; …` (`crates/aether-remote/src/http.rs:246-249`). Test `every_response_carries_the_csp_and_the_security_headers`. |
| d6 | Pairing codes and phrases | ✅ | 40-bit Crockford code from the CSPRNG, 120 s expiry, single use, host confirmation required (`crates/aether-remote/src/pairing.rs`). Guesses are rate-limited to `MAX_GUESSES_PER_SECOND = 10` (so at most 1200 over a code's life), each wrong code waits 250 ms, and the compare is constant-time (`pairing.rs:20-37,380`; `crates/aether-protocol/src/auth.rs:16-21`). The three-word comparison phrase hashes the pairing id and the device's public key (`phrase`, `pairing.rs:87-99`). Tests `pairing/tests.rs`: `a_code_is_forty_bits_of_crockford_base32`, `a_code_is_single_use`, `an_unclaimed_code_expires_and_so_does_an_unconfirmed_claim`, `a_flood_gets_no_more_guesses_than_the_documented_bound`. |
| d7 | Device identity | ✅ | Aether devices hold no bearer token. They hold a non-extractable P-256 key in IndexedDB (`crates/aether-remote/web/core/identity.js:30-36`). The daemon stores only the public point and verifies ECDSA over a 32-byte nonce bound to origin, instance and device (`crates/aether-remote/src/auth.rs:64-77,104-117`). The legacy `[listen]` token uses a constant-time compare (`crates/aether-daemon/src/listen.rs`). |
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
| p1 | Aether credentials cannot be read by script on the origin | ✅ | The device key is a non-extractable `CryptoKey`, refused on load if extractable (`identity.js:30`). Script on the origin can at most ask it to sign, never copy it. The origin's script is Aether's own bundle, under `script-src 'self'`. Archond's pages run no script. |
| p2 | Archon credentials cannot be read across apps | ✅ | Archond's cookie is `HttpOnly; SameSite=Strict; Path=/archon`, plus `Secure` on https (Archon `http/api.rs` `credential_cookie`). JavaScript cannot read it, and the browser never sends it to Aether's `/ws` or `/api/*`. |
| p3 | The two pairings cannot be confused | ✅ | Aether pairs at `/api/pair/*` and authenticates by signature on `/ws`. Archon pairs and authenticates under `/archon/api/*` and `/archon/ws` by cookie (`crates/aether-remote/src/mount/forward.rs`). The listener refuses a mount over `/api` or `/ws` (d13). An Aether signature covers origin, instance and nonce, and Archon accepts no signature, so neither credential is accepted by the other daemon. |

## Noesis data boundaries

| # | Check | Status | Evidence |
| --- | --- | --- | --- |
| n1 | Sealed fields are replaced where the call is persisted | ✅ | `seal_commit` runs in `Runtime::apply_held_by` before write and publish, and `seal_import` on import (`crates/aether-runtime/src/lib.rs`). `seal_event` and `seal_mutations` name every variant with no wildcard (`crates/aether-runtime/src/noesis_seal.rs`). `answer_key`, `explanation` and `solution` become `"<sealed>"`. Tests in `crates/aether-runtime/tests/kernel/noesis_seal.rs` and `noesis_seal/tests.rs`. |
| n2 | No `tool_input` delta for a sealed call; no thinking for Noesis agents; no sequence gap | ✅ | `StreamSeal::admits` decides before a delta is numbered (`noesis_seal.rs`, `bus.rs`). Tests `a_stream_withholds_sealed_input_and_noesis_thinking_only` and `assert_deltas_sealed_and_gapless` (`crates/aether-daemon/src/noesis/teaching_tests.rs`). Noesis teaching turns publish no deltas at all (`crates/aether-daemon/src/noesis/engine.rs`). |
| n3 | No read before reveal | ✅ | `Question` and `Problem` have no key or solution field (`crates/aether-core/src/noesis.rs`). `QuestionResult` is built only after the answer commits. `solution` appears only in `noesis.practice_concluded`, after the conclusion commits. Tests `t11_answer_keys_are_sealed_until_the_answer_is_recorded`, `t11_the_solution_is_sealed_on_every_read_path_until_conclude_and_practice_never_writes_the_graph` (`crates/aether-noesis/src/teach/parity.rs`). |
| n4 | Not in logs or error text | ✅ | No `tracing` call prints a tool input. Argument errors show the shape only (`crates/aether-noesis/src/tools/args.rs`). Unknown argument names are never echoed. `Sealer`'s `Debug` prints only a count. |
| n5 | Evidence bundle ≤ 32 KiB, shape checked, refused before storing | ✅ | `validate_bundle` (`crates/aether-protocol/src/noesis.rs`) and `evidence::validate` (`crates/aether-noesis/src/evidence/mod.rs`); refused unless `engineering_context` is in effect. Tests in `crates/aether-noesis/src/evidence/tests.rs`. |
| n6 | Noesis never fetches; the bundle holds only what the user could read | ✅ | `aether-noesis` has no network or process dependency. Test `teach/evidence_flow.rs` compares the socket table before and after. |
| n7 | Disabled by default | ✅ | Every non-service group returns `noesis_disabled` before validation (`crates/aether-daemon/src/noesis/mod.rs`); tests in `noesis/tests.rs` and `teaching_tests.rs`. See H3-L11. |
| n8 | Imported sources hold no secret a reader could not already see | ✅ after H3-A1 | H3-A1. |

## Low findings (follow-ups)

- **H3-L6 (mount).** A mounted answer keeps whatever CSP archond sends
  (`harden_missing`). Today that CSP is stricter than Aether's. A future,
  weaker archond CSP would apply to a page on the origin that holds Aether's
  device key. *Follow-up:* send Aether's CSP as an additional header on
  mounted answers, so the browser enforces both.
- **H3-L7 (Noesis, resolved).** A model's visible text that restated an answer
  was stored verbatim in the transcript. Each reply is now checked whole by
  `reply_leaks` before `AetherEngine::record_reply` commits anything of it
  (`crates/aether-noesis/src/teach/seal.rs`). A leaking reply is discarded and
  the step runs again once with a reminder, then falls back to a neutral step.
  Tests `a_reply_giving_its_own_key_away_is_discarded_and_made_again` and
  `a_second_leaking_reply_becomes_the_neutral_step` (`teach/recovery.rs` and
  `crates/aether-daemon/src/noesis/teaching_tests/engine_path/live_seal.rs`).
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
