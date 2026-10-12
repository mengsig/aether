# Request correlation and malformed frames

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
it does not introduce an incompatible wire change or bump the protocol version.
The daemon applies it to both negotiated versions in its N/N-1 window. The
versioning contract and negotiation are defined in `aether-protocol`'s
`PROTOCOL_VERSION` and `compat` module.

# Images on a message

Protocol 18 lets `send_message` carry images next to its text:

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
`MAX_MESSAGE_IMAGE_BYTES` in `aether-core`'s `attachment` module. Of a
`name`, only the last path component is kept, without control characters and
cut at 120 characters.

Image bytes are never written to the event log, an inbox item or a
transcript. The daemon stores each image once, under the SHA-256 of its bytes,
in `aether.attachments/` beside `aether.db` (mode 0700), and everything
durable carries a reference in its place:

```json
{"type": "image_ref", "id": "<64 hex digits>", "media_type": "image/png",
 "size_bytes": 48211, "name": "shot.png"}
```

That block follows the text block in a message's `content` wherever a message
appears: `inbox.delivered`, `get_inbox` and `get_transcript`. A message that
is only images has no text block. The file is named `<id>.<ext>`.

The images are part of an idempotency key's fingerprint: a retry with the
same key and the same images is the same send, and the same key with
different images is refused like any other reuse.

A client that negotiated protocol 17 receives each `image_ref` block as a
text block reading `[image: shot.png, 47.1 KB]`, since it has no such variant
to decode. Sending images needs protocol 18.

`subtree: true` delivers the same references to every recipient; the bytes
are still stored once. The `send_message` tool that agents use to write to
each other is text only.
