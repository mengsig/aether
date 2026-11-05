"""A scripted OpenAI-compatible chat-completions server for the differential harness.

Loopback only, keyless, stdlib only. Each POST pops the next scripted turn and
streams it as server-sent events; extra requests get a plain "done" turn and
are counted as overruns so the harness can report a script that ran short.
"""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

OVERRUN_TURN = {"content": "done"}


def sse_chunks(turn, model="faux"):
    """Return the SSE data payloads (dicts) that stream one scripted turn."""
    assert isinstance(turn, dict), "a turn is an object"
    assert set(turn) <= {"content", "tool_calls"}, f"unknown turn keys {sorted(turn)}"
    base = {"id": "chatcmpl-faux", "object": "chat.completion.chunk", "model": model}
    chunks = []
    if turn.get("content"):
        chunks.append(dict(base, choices=[{"index": 0, "delta": {"role": "assistant", "content": turn["content"]}}]))
    for index, call in enumerate(turn.get("tool_calls", [])):
        function = {"name": call["name"], "arguments": json.dumps(call["arguments"])}
        delta = {"tool_calls": [{"index": index, "id": f"call_{index}", "type": "function", "function": function}]}
        chunks.append(dict(base, choices=[{"index": 0, "delta": delta}]))
    finish = "tool_calls" if turn.get("tool_calls") else "stop"
    chunks.append(dict(base, choices=[{"index": 0, "delta": {}, "finish_reason": finish}]))
    return chunks


class FauxModel:
    def __init__(self, turns):
        assert isinstance(turns, list), "turns must be a list"
        self._turns = list(turns)
        self._lock = threading.Lock()
        self.requests = 0
        self.overruns = 0
        self._server = None
        self._thread = None

    def next_turn(self):
        with self._lock:
            self.requests += 1
            if self._turns:
                return self._turns.pop(0)
            self.overruns += 1
            return OVERRUN_TURN

    def start(self):
        assert self._server is None, "already started"
        model = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self._send_json({"object": "list", "data": [{"id": "faux", "object": "model"}]})

            def do_POST(self):
                self.rfile.read(int(self.headers.get("Content-Length", 0)))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                for chunk in sse_chunks(model.next_turn()):
                    self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
                self.wfile.write(b"data: [DONE]\n\n")

            def _send_json(self, body):
                payload = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self.base_url

    @property
    def base_url(self):
        assert self._server is not None, "not started"
        return f"http://127.0.0.1:{self._server.server_address[1]}/v1"

    def stop(self):
        assert self._server is not None, "not started"
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=5)
        self._server = None
