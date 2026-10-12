# Daemon protocol

## Request correlation and malformed frames

Aether uses newline-delimited JSON. A client frame carries a nonzero unsigned
64-bit correlation key in `req`; a server reply echoes it in `id`. A command's
own `id` (for example, the memory to forget) is unrelated to correlation.

The daemon reads `req` before decoding the command. Typed decoding failures,
including unknown fields in strict specifications, invalid enum values, and
missing required fields, receive `invalid_argument` at that request's ID, with
the decoding diagnostic. After a successful hello, these errors leave the
connection usable. A malformed hello receives its correlated diagnostic and
closes the connection.

If the daemon cannot read an unambiguous, nonzero `req` (including invalid JSON,
a missing, duplicate, zero, negative, fractional, or out-of-range ID), it sends
an `invalid_argument` reply with `id: 0`, then closes the connection. ID zero is
reserved for connection diagnostics. Clients retain that diagnostic and fail
all pending requests when the connection ends, rather than waiting for request
timeouts. Transport framing failures, such as an overlong line or invalid UTF-8,
may close without a JSON diagnostic; clients still fail pending requests on
closure.

This policy uses the existing reply shape and reserved ID-zero/closure contract;
it does not introduce an incompatible wire change. It applies to every served
protocol version.

## Versions

The protocol is version 18 and the daemon serves versions 17 and 18: its own
and one back. The window is `MIN_PROTOCOL_VERSION` to `PROTOCOL_VERSION` in
`aether-protocol`; the `compat` module translates replies for the older end.
A hello outside the window is refused with `incompatible`, naming both bounds.
The `Welcome` echoes the version in effect for the connection and reports the
window as `daemon_protocol_version` and `min_protocol_version`.

Only the daemon's answers are rewritten. Every field a newer version added to
a command is optional, so a version 17 client's requests decode unchanged; the
test `no_command_requires_a_field_an_older_client_would_not_send` holds that
against the live schema.
Version 18 adds remote control, whose contract is [Remote control](remote.md);
the wire changes are:

- **Challenge frame.** `ServerFrame::Challenge` (`{"frame": "challenge",
  "nonce", "instance_id"}`) is sent first, and only, by the remote WebSocket
  carrier. The Unix socket and the token listener never send it, and a client
  never sends one. The carrier's hello answers it.
- **`Hello.device` and `Welcome.device`.** A device proves its enrolled key by
  signing the challenge in `Hello.device`. The `Welcome` names the
  authenticated device (`id`, `name`, `role`); it is absent on the Unix socket
  and the token listener. See [the handshake](remote.md#the-handshake).
- **Error codes.** `forbidden` (authenticated, but the transport or role may
  not run the command) and `unimplemented` (a protocol command this build does
  not serve). A version 17 client cannot decode them, so it receives
  `unauthorized` and `unavailable` with the same message.
- **Device provenance.** An actor caused over a device connection carries an
  origin naming the enrolled device. A version 17 client receives that actor
  as a user named `device:<name>`. See [provenance](remote.md#provenance).
- **Idempotency keys.** Eighteen commands take an optional `idempotency_key`:
  `create_agent`, `create_task`, `send_message`, `reply_to_question`,
  `trigger`, `create_schedule`, `resolve_approval`, `restart`, `cancel`,
  `interrupt`, `pause`, `resume`, `archive`, `update_agent`, `set_approvals`,
  `update_schedule`, `create_memory` and `add_approval_rule`. The same key with
  the same fields returns the first outcome; with different fields it is
  `invalid_argument`. See [idempotency keys](remote.md#idempotency-keys).
- **`resolve_submission` and `Welcome.submission_horizon`.** A read-only
  command that says what became of a keyed command whose reply was lost:
  `executed` with the original receipt, `absent_safe` when the host proves it
  never ran, or `unknown`. The proof compares the host's durable receipt
  ledger with the opaque horizon every version 18 `Welcome` carries, which a
  client keeps with each keyed command it sends. See
  [resolving a lost acknowledgement](remote.md#resolving-a-lost-acknowledgement).
- **New commands and replies.** Attention, host services (path completion,
  workspaces, git summary), remote administration, device management and push,
  each classified by the access class it needs
  ([authorization](remote.md#authorization)).

A version 17 daemon cannot decode these commands and ignores the new keys, so
it refuses a version 18 hello as outside its window rather than dropping them.

The versioning contract and negotiation are defined in `aether-protocol`'s
`PROTOCOL_VERSION` and `compat` module.
