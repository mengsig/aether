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

The protocol is version 19 and the daemon serves versions 17, 18 and 19: its
own and two back, because 18 and 19 were released days apart. The window is
`MIN_PROTOCOL_VERSION` to `PROTOCOL_VERSION` in `aether-protocol`; the
`compat` module translates replies for the older versions.
A hello outside the window is refused with `incompatible`, naming both bounds.
The `Welcome` echoes the version in effect for the connection and reports the
window as `daemon_protocol_version` and `min_protocol_version`.

Only the daemon's answers are rewritten. Every field a newer version added to
a command is optional, so an older client's requests decode unchanged; the
test `no_command_requires_a_field_an_older_client_would_not_send` holds that
against the live schema.

Version 19 adds [images on a message](#images-on-a-message):
`send_message.attachments` and the `image_ref` content block. A version 17 or
18 client cannot decode that block and receives a line of text in its place.
A version 18 daemon ignores `attachments` and would deliver the text alone,
so it refuses a version 19 hello as outside its window; a client that falls
back to an older version for such a daemon must not send images to it (the
Rust client refuses with `unimplemented`).

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

## Images on a message

Protocol 19 lets `send_message` carry images next to its text:

```json
{"req": 7, "cmd": "send_message", "to": "agt_...", "content": "What is this?",
 "attachments": [{"media_type": "image/png", "data": "iVBORw0KGgo...", "name": "shot.png"}]}
```

`attachments` defaults to empty and is left off the wire when empty, so a
text-only message is the frame it always was. Each entry is one image:
`media_type` (`image/png`, `image/jpeg`, `image/gif` or `image/webp`), `data`
(the file's bytes in standard base64) and an optional `name` shown to people.
`content` may be empty when there is at least one image.

The daemon checks every image before it stores or delivers anything and
refuses the whole message with `invalid_argument` and a message naming the
image and the rule:

| Rule | Limit |
| --- | --- |
| Images per message | 10 |
| One image, decoded | 3,932,160 bytes (3.75 MiB) |
| All images of one message, decoded | 10,485,760 bytes (10 MiB) |
| Type | PNG, JPEG, GIF or WebP by its leading bytes, which must agree with `media_type` |
| `data` | valid standard base64, not empty |

The limits are `MAX_IMAGES_PER_MESSAGE`, `MAX_IMAGE_BYTES` and
`MAX_MESSAGE_IMAGE_BYTES` in `aether-core`'s `attachment` module. Two more,
`MAX_REQUEST_IMAGES` (20) and `MAX_REQUEST_IMAGE_BYTES` (15 MiB), are not
checked on a command: they bound the images one request to an API model
carries across its whole history, see [engines](engines.md#images-on-a-message). Of a
`name`, only the last path component is kept, without control characters and
cut at 120 characters.

The limits also bound what an agent is handed at once. Messages that wait in
its inbox together are folded into one user turn only while their images stay
within one message's limits; the rest stay unconsumed and follow as the next
input (a steer into the running engine turn, or the next turn).

Image bytes are never written to the event log, an inbox item or a
transcript. The daemon stores each image once, under the SHA-256 of its bytes,
in `aether.attachments/` beside `aether.db` (mode 0700), and everything
durable carries a reference in its place:

```json
{"type": "image_ref", "id": "<64 hex digits>", "media_type": "image/png",
 "size_bytes": 48211, "name": "shot.png"}
```

A reference is a stored format too: store schema 19 marks a database whose
records may hold one, so a binary that cannot read it refuses the store.

That block follows the text block in a message's `content` wherever a message
appears: `inbox.delivered`, `get_inbox` and `get_transcript`. A message that
is only images has no text block. The file is named `<id>.<ext>`.

The images are part of an idempotency key's fingerprint: a retry with the
same key and the same images is the same send, and the same key with
different images is refused like any other reuse.

A client that negotiated protocol 17 or 18 receives each `image_ref` block
as a text block reading `[image: shot.png, 47.1 KB]`, since it has no such
variant to decode. Sending images needs a protocol 19 daemon.

Sending a message with images needs the same standing as sending one
without: `operate` in the [authorization](remote.md#authorization) matrix. An
observer device is refused with `forbidden` before any image is looked at.
[`resolve_submission`](remote.md#resolving-a-lost-acknowledgement) takes the
whole command, images included, and stores nothing.

`subtree: true` delivers the same references to every recipient; the bytes
are still stored once. The `send_message` tool that agents use to write to
each other is text only.
