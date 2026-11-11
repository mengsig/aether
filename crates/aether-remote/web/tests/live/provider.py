#!/usr/bin/env python3
"""Extend the shared streaming mock with one deterministic live question."""

import importlib.util
from pathlib import Path
import sys
from http.server import ThreadingHTTPServer


def load_mock(source):
    path = Path(source).resolve(strict=True)
    assert path.is_file(), "Mock provider must be a file"
    spec = importlib.util.spec_from_file_location("live_mock_openai", path)
    assert spec is not None and spec.loader is not None, "Mock must be importable"
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def question_decider(mock):
    original = mock.decide
    assert callable(original), "Mock must expose its decision function"
    assert callable(mock.last_user_text), "Mock must expose transcript parsing"

    def decide(messages, names):
        assert isinstance(messages, list), "Provider messages must be a list"
        assert isinstance(names, set), "Tool names must be normalized"
        prompt = mock.last_user_text(messages)
        if "live acceptance question" in prompt.lower():
            if mock.last_tool_result(messages) is None and "ask_user" in names:
                return "Choosing the acceptance color.", [
                    ("ask_user", {"question": "Which acceptance color should I use?"})
                ]
        return original(messages, names)

    return decide


def main(arguments):
    if len(arguments) != 2:
        raise ValueError("Usage: provider.py /path/to/scripts/mock_openai.py PORT")
    port = int(arguments[1])
    if not 0 <= port <= 65535:
        raise ValueError("Port must be between 0 and 65535")
    mock = load_mock(arguments[0])
    mock.decide = question_decider(mock)
    with ThreadingHTTPServer(("127.0.0.1", port), mock.Handler) as server:
        assert server.server_address[0] == "127.0.0.1", "Fixture must stay local"
        assert server.server_address[1] > 0, "Server must have a bound port"
        print(f"mock openai server on http://127.0.0.1:{server.server_address[1]}/v1", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main(sys.argv[1:])
