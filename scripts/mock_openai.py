#!/usr/bin/env python3
"""A tiny OpenAI-compatible server for end-to-end smoke tests.

Speaks both wire shapes: `POST /v1/chat/completions` (protocol
`openai-chat-completions`) and `POST /v1/responses` (protocol
`openai-responses`). It streams SSE like a real server and answers from a
small script keyed on the latest user text, so the whole path (daemon,
adapter, SSE parser, tools, TUI) can be exercised without credentials. Not a
model: it pattern matches.

Usage: python3 scripts/mock_openai.py [port]   (0 picks a free one, printed on start)
Provider config: see scripts/mock_providers.toml
"""

import base64
import hashlib
import json
import re
import sys
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def last_user_text(messages):
    for m in reversed(messages):
        if m.get("role") == "user":
            c = m.get("content")
            if isinstance(c, str):
                return c
            if isinstance(c, list):
                return " ".join(p.get("text", "") for p in c if p.get("type") == "text")
    return ""


def last_user_images(messages):
    """The images on the latest user message, each as (media type, bytes)."""
    for m in reversed(messages):
        if m.get("role") != "user":
            continue
        parts = m.get("content")
        if not isinstance(parts, list):
            return []
        return [data_url(p["image_url"]["url"]) for p in parts if p.get("type") == "image_url"]
    return []


def data_url(url):
    match = re.fullmatch(r"data:([^;,]+);base64,(.*)", url, re.DOTALL)
    if match is None:
        raise ValueError(f"an image that is not a base64 data URL: {url[:60]}")
    return match.group(1), base64.b64decode(match.group(2), validate=True)


def images_reply(images):
    """Names what arrived exactly, so a check can compare it with what was sent."""
    seen = "; ".join(
        f"{media_type}, {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}"
        for media_type, data in images
    )
    return f"I was sent {len(images)} image(s): {seen}"


def last_tool_result(messages):
    for m in reversed(messages):
        if m.get("role") == "tool":
            return m.get("content") or ""
        if m.get("role") == "user":
            return None
    return None


def folded_transcript(messages):
    """The transcript the compaction request carried, as plain text."""
    request = last_user_text(messages)
    marker = "# Transcript to fold"
    if marker not in request:
        return ""
    body = request.split(marker, 1)[1]
    body = body.split("\n# Schema", 1)[0]
    return " ".join(body.split())[:1200]


def compaction_reply(messages):
    """The compaction service asks for a JSON checkpoint; mimic a good one.

    A real compactor summarizes. This one carries the folded turns through
    verbatim, so an end-to-end check can see exactly what crossed over.
    """
    facts = [m for m in messages if m.get("role") == "user"]
    folded = folded_transcript(messages)
    return json.dumps({
        "checkpoint": {
            "objective": "Continue helping the user in this chat.",
            "state": f"Folded {len(facts)} earlier turns. What was said: {folded}"
                     if folded
                     else f"Folded {len(facts)} earlier turns; the user has been exploring the workspace.",
            "decisions": ["Keep answers brief."],
            "constraints": ["Stay inside the workspace."],
            "artifacts": [{"reference": "README.md", "note": "sample file in the workspace"}],
            "open_loops": [],
            "uncertainty": [],
        },
        "memory_candidates": [{
            "scope": "repository", "kind": "fact",
            "text": "The workspace contains a README.md sample file used for smoke tests",
            "importance": 0.6, "confidence": 0.9, "tags": ["smoke"],
        }],
    })


def tool_names(tools):
    """Tool names in either wire shape: chat nests them under `function`, the
    Responses API is flat."""
    out = set()
    for t in tools or []:
        name = t.get("name") or (t.get("function") or {}).get("name")
        if name:
            out.add(name)
    return out


def carried_checkpoint(messages):
    """The checkpoint layer this turn was given, if any. Adapters may merge
    the system layers into one message, so match on the layer's own opening."""
    marker = "Checkpoint v"
    for m in messages:
        if m.get("role") != "system":
            continue
        c = m.get("content")
        if isinstance(c, str) and marker in c:
            return c[c.index(marker):]
    return None


def after_tool(user, tool_result):
    """A short closing line like a real model's; the transcript already shows the output."""
    lowered = tool_result.lower()
    if "delegated to agent" in lowered:
        return "Handed the changelog to a worker." if "changelog" in user else "Handed the listing to a worker."
    if "artifact" in lowered:
        return "The output was long, so I stored it as an artifact."
    if "no such file" in lowered or ("exit code" in lowered and "exit code: 0" not in lowered):
        return "That failed: the file does not exist."
    if "total" in lowered:
        listed = [line.split()[-1] for line in tool_result.splitlines() if line[:1] in ("-", "d", "l") and line.split()]
        entries = sum(1 for name in listed if name not in (".", ".."))
        if entries == 0:
            return "Done. The workspace is empty."
        return f"Done. The workspace has {entries} {'entry' if entries == 1 else 'entries'}."
    if "write" in user:
        return "Done. The file is written."
    if "fetch" in user:
        return "Fetched the page. Nothing in it needs action."
    return "Done."


def decide(messages, names):
    """Return (text, tool_calls) for this turn."""
    system = " ".join(m.get("content", "") if isinstance(m.get("content"), str) else "" for m in messages if m.get("role") == "system")
    if "compaction service" in system:
        return (compaction_reply(messages), [])
    images = last_user_images(messages)
    if images:
        return (images_reply(images), [])
    # A model that takes no images is told where the daemon keeps the file.
    stored = re.search(r"\[image: [^\]\n]*, stored at ([^\]\n]+)\]", last_user_text(messages))
    if stored:
        return (f"I was sent no image, only the file {stored.group(1)}", [])
    user = last_user_text(messages).lower()
    # Answers only from context carried across a handoff, so a check can tell
    # what survived the switch from what the model was told this turn.
    if "recall" in user:
        carried = carried_checkpoint(messages)
        if carried is None:
            return ("I was given no carried context.", [])
        return (f"From my carried context: {' '.join(carried.split())[:900]}", [])
    tool_result = last_tool_result(messages)
    if tool_result is not None:
        if "start a background job" in user:
            process_id = next(
                (
                    match.group(0)
                    for message in reversed(messages)
                    if message.get("role") == "tool"
                    for match in [re.search(r"proc_[0-9A-Z]+", message.get("content") or "")]
                    if match
                ),
                None,
            )
            if "in the background" in tool_result:
                if "shell_processes" in names:
                    return ("Checking what is still running.", [("shell_processes", {})])
            if re.search(r"(?:^|\n)tick [0-9]+(?:\n|$)", tool_result) and "shell_stop" in names:
                if process_id:
                    return ("Stopping the background job.", [("shell_stop", {"process_id": process_id})])
            if re.search(r"proc_[0-9A-Z]+ running ·", tool_result):
                if process_id and "shell_output" in names:
                    return ("Reading its output.", [("shell_output", {"process_id": process_id})])
            if "stopped proc_" in tool_result and "shell_processes" in names:
                return ("Verifying it stopped.", [("shell_processes", {})])
            if "no background processes" in tool_result:
                return ("The background job was read and stopped.", [])
        # Started in the background: list it from the same agent, while it is
        # still running. A fresh `aether run` is a fresh agent, and processes
        # are per-agent, so listing from there could only ever say "none".
        if "in the background" in tool_result:
            if "shell_processes" in names:
                return ("Checking what is still running.", [("shell_processes", {})])
            return (f"Background: {tool_result[:300]}", [])
        if "background processes" in tool_result or " running · " in tool_result:
            return (f"Background listing: {tool_result[:300]}", [])
        if "exit code" in tool_result.lower() or "total" in tool_result.lower():
            if "complete_task" in names and "task" in json.dumps(messages).lower():
                return ("", [("complete_task", {"summary": "Listed the workspace and reported the result."})])
        return (after_tool(user, tool_result), [])
    # The only class that asks for approval is External, and web_fetch is the
    # only such tool: without this branch nothing can drive an approval card.
    if "fetch" in user and "web_fetch" in names:
        return ("Fetching the page.", [("web_fetch", {"url": "https://example.com/changelog"})])
    if "big output" in user and "bash" in names:
        return ("Producing a large output.", [("bash", {"command": "for i in $(seq 1 4000); do echo 'aether artifact line'; done"})])
    if "background" in user and "bash" in names:
        return ("Starting it in the background.", [("bash", {"command": "for i in $(seq 1 60); do echo tick $i; sleep 1; done", "background": True})])
    if "unavailable model" in user and "delegate" in names:
        return ("Checking the delegation refusal.", [("delegate", {"role": "worker", "objective": "Read the README", "model": "missing/model", "effort": "low", "wait": False})])
    # Two levels deep, the grandchild on a slow command, so a capture can catch the tree mid-run.
    if "delegate deeply" in user and "delegate" in names:
        return ("Handing this to a worker.", [("delegate", {"role": "worker", "objective": "Delegate a slow listing to a child", "model": "mock/mock-1", "effort": "low", "wait": False})])
    if "slow listing" in user and "delegate" in names:
        return ("Passing it down.", [("delegate", {"role": "worker", "objective": "Run the slow command", "model": "mock/mock-1", "effort": "low", "wait": True})])
    if "delegate" in user and "changelog" in user and "delegate" in names:
        return ("Handing the fetch to a child.", [("delegate", {"role": "worker", "objective": "Fetch the changelog page", "model": "mock/mock-1", "effort": "low", "wait": False})])
    if "delegate" in user and "delegate" in names:
        return ("Splitting this into a child task.", [("delegate", {"role": "worker", "objective": "List the files in the workspace", "model": "mock/mock-1", "effort": "low", "wait": True})])
    # Long enough for a capture to catch the call while it runs.
    if "slow command" in user and "bash" in names:
        return ("Running a slow command.", [("bash", {"command": "sleep 6 && ls -la | head -6"})])
    # Three read-only calls in one turn: a single explored batch.
    if "explore" in user and "read_file" in names:
        return ("Looking around first.", [("read_file", {"path": "README.md"}), ("grep", {"pattern": "sample"}), ("list_dir", {"path": "."})])
    if "failing command" in user and "bash" in names:
        return ("Running a command that fails.", [("bash", {"command": "cat missing-notes.txt"})])
    if "list" in user and "bash" in names:
        return ("Let me look at the workspace.", [("bash", {"command": "ls -la | head -20"})])
    if "[inbox" in user and "completed" in user and "complete_task" in names:
        return ("", [("complete_task", {"summary": "Child finished; collected its result."})])
    if "readme.md" in user and "write_file" in names:
        return ("Writing the README.", [("write_file", {"path": "README.md", "content": README_SAMPLE})])
    if "write" in user and "write_file" in names:
        return ("Writing the file now.", [("write_file", {"path": "hello.txt", "content": "hello from aether\n"})])
    if "markdown" in user:
        return (MARKDOWN_SAMPLE, [])
    if "slowly" in user:
        return ("Counting slowly: " + " ".join(f"{i}," for i in range(1, 200)) + " done.", [])
    if "hello" in user or "hi" in user:
        return ("Hello. I am a mock model running inside Aether. Ask me to list files, write a file, or delegate.", [])
    return ("I am a scripted mock model. Try: 'list files', 'write a file', 'delegate', or 'hello'.", [])


# The `readme` eval task, which a pattern matcher can genuinely finish: it
# gives the API path one real tool round trip to measure.
README_SAMPLE = """# pyproj

`utils.py` holds small text and list helpers; `stats.py` computes summary
statistics over a sequence of numbers.

Run the tests with `python3 -m pytest -q`.
"""


MARKDOWN_SAMPLE = """## Plan

Two steps, then a check:

1. Read `Cargo.toml` and the **workspace** members
2. Run the tests
   - unit tests first
   - then the *kernel* suite

```rust
fn main() {
    println!("hello from aether");
}
```

| crate | tests |
|---|---|
| aether-core | 41 |
| aether-tui | 35 |

> Note: nothing is written until you approve.

See [the architecture notes](https://example.com/arch) for details.
"""


def sse(handler, obj):
    handler.wfile.write(f"data: {json.dumps(obj)}\n\n".encode())
    handler.wfile.flush()


def sse_named(handler, name, payload):
    """A Responses-API event: the name goes on both the SSE line and `type`."""
    body = json.dumps({"type": name, **payload})
    handler.wfile.write(f"event: {name}\ndata: {body}\n\n".encode())
    handler.wfile.flush()


def responses_messages(req):
    """The Responses `input` array as the chat-shaped messages `decide` reads.

    Two shape differences matter: the system prompt arrives out of band in
    `instructions`, and a turn's tool outputs are emitted before its user
    text, while `last_tool_result` scans back expecting them after it.
    """
    messages = []
    if req.get("instructions"):
        messages.append({"role": "system", "content": req["instructions"]})
    pending_output = []
    for item in req.get("input") or []:
        kind = item.get("type")
        if kind == "function_call_output":
            pending_output.append({"role": "tool", "content": item.get("output") or ""})
            continue
        if kind == "function_call":
            messages.append({"role": "assistant", "content": ""})
            continue
        role = item.get("role")
        if role is None:
            continue
        messages.append({"role": role, "content": chat_content(item.get("content"))})
        if role == "user":
            messages.extend(pending_output)
            pending_output = []
    messages.extend(pending_output)
    return messages


def chat_content(content):
    """A Responses item's content in the chat shape: a string, or parts once it has an image."""
    if isinstance(content, str):
        return content
    parts = content or []
    text = " ".join(p.get("text", "") for p in parts if p.get("type") in ("input_text", "output_text"))
    images = [p["image_url"] for p in parts if p.get("type") == "input_image"]
    if not images:
        return text
    return [{"type": "text", "text": text}] + [{"type": "image_url", "image_url": {"url": url}} for url in images]


def usage_of(messages, text, calls):
    prompt = sum(len(json.dumps(m)) for m in messages) // 4
    completion = max(1, len(text) // 4 + sum(len(json.dumps(a)) for _, a in calls) // 4)
    return prompt, completion


def stream_responses(handler, req):
    """One turn in the Responses wire format.

    Every output item gets its own `output_index`: the adapter keys blocks on
    `(output_index, content_index)` for text and `(output_index, 0)` for a
    function call, so sharing an index would collide. No `data: [DONE]`
    either - the Responses parser reads every data line as JSON.
    """
    messages = responses_messages(req)
    text, calls = decide(messages, tool_names(req.get("tools")))
    rid = "resp_" + uuid.uuid4().hex[:12]
    handler.send_response(200)
    handler.send_header("Content-Type", "text/event-stream")
    handler.send_header("Cache-Control", "no-cache")
    handler.end_headers()

    sse_named(handler, "response.created",
              {"response": {"id": rid, "model": req.get("model", "mock-1")}})
    pace = 0.25 if "slowly" in last_user_text(messages).lower() else 0.03
    if text:
        sse_named(handler, "response.output_item.added",
                  {"output_index": 0, "item": {"id": "msg_" + rid, "type": "message", "role": "assistant"}})
        sse_named(handler, "response.content_part.added",
                  {"output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": ""}})
        for i in range(0, len(text), 6):
            sse_named(handler, "response.output_text.delta",
                      {"output_index": 0, "content_index": 0, "delta": text[i : i + 6]})
            time.sleep(pace)
        # Only the content part is closed: `output_item.done` would close the
        # same block a second time.
        sse_named(handler, "response.content_part.done", {"output_index": 0, "content_index": 0})
    for i, (name, args) in enumerate(calls):
        index = i + 1
        call_id = "call_" + uuid.uuid4().hex[:8]
        sse_named(handler, "response.output_item.added", {
            "output_index": index,
            "item": {"id": "fc_" + call_id, "type": "function_call", "call_id": call_id, "name": name},
        })
        payload = json.dumps(args)
        # Arguments only ever arrive as deltas; the adapter never reads
        # `item.arguments`, and a whole-object item yields an empty input.
        for j in range(0, len(payload), 24):
            sse_named(handler, "response.function_call_arguments.delta",
                      {"output_index": index, "delta": payload[j : j + 24]})
            time.sleep(0.01)
        sse_named(handler, "response.output_item.done", {"output_index": index})
    prompt_tokens, completion_tokens = usage_of(messages, text, calls)
    sse_named(handler, "response.completed", {"response": {
        "id": rid,
        "usage": {"input_tokens": prompt_tokens, "output_tokens": completion_tokens},
    }})


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        sys.stderr.write("mock: " + fmt % args + "\n")

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            body = json.dumps({"object": "list", "data": [{"id": "mock-1", "object": "model"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def read_request(self):
        length = int(self.headers.get("Content-Length", "0"))
        return json.loads(self.rfile.read(length) or b"{}")

    def do_POST(self):
        req = self.read_request()
        if self.path.rstrip("/").endswith("/responses"):
            stream_responses(self, req)
            return
        messages = req.get("messages", [])
        text, calls = decide(messages, tool_names(req.get("tools")))
        cid = "chatcmpl-" + uuid.uuid4().hex[:12]
        model = req.get("model", "mock-1")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()

        def chunk(delta, finish=None, usage=None):
            obj = {
                "id": cid,
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }
            if usage is not None:
                obj["usage"] = usage
            return obj

        sse(self, chunk({"role": "assistant", "content": ""}))
        # "slowly" streams at a pace a person can interrupt.
        pace = 0.25 if "slowly" in last_user_text(messages).lower() else 0.03
        # Fixed-size chunks keep indentation and newlines exactly as written.
        for i in range(0, len(text), 6):
            sse(self, chunk({"content": text[i : i + 6]}))
            time.sleep(pace)
        for i, (name, args) in enumerate(calls):
            sse(self, chunk({"tool_calls": [{"index": i, "id": "call_" + uuid.uuid4().hex[:8], "type": "function", "function": {"name": name, "arguments": ""}}]}))
            payload = json.dumps(args)
            for j in range(0, len(payload), 24):
                sse(self, chunk({"tool_calls": [{"index": i, "function": {"arguments": payload[j : j + 24]}}]}))
                time.sleep(0.01)
        prompt_tokens, completion_tokens = usage_of(messages, text, calls)
        sse(self, chunk({}, "tool_calls" if calls else "stop", {"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens, "total_tokens": prompt_tokens + completion_tokens}))
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8811
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    # The bound port, so a caller passing 0 learns which one it got.
    port = server.server_address[1]
    print(f"mock openai server on http://127.0.0.1:{port}/v1", flush=True)
    server.serve_forever()
