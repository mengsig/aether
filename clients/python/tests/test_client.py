"""A malformed frame from the daemon must fail the connection cleanly: the
reader thread must not die silently, and a caller already waiting on a reply
must not hang forever. Run: `python3 -m unittest discover -s tests -t .`
from clients/python/.
"""

from __future__ import annotations

import json
import os
import runpy
import socket
import sys
import threading
import time
import unittest
from unittest.mock import Mock, call, patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from aether.client import (
    AetherClient, AetherError, SubscriptionLaggedError, MAX_QUEUED_SUBSCRIPTION_FRAMES,
)  # noqa: E402
from aether import protocol  # noqa: E402


class StubDaemon:
    """Answers the hello with a welcome, then lets the test write whatever
    raw line it wants onto the same connection."""

    def __init__(self, hello_result=None, on_request=None):
        self._on_request = on_request
        self._sending = threading.Lock()
        self._listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._listener.bind(("127.0.0.1", 0))
        self._listener.listen(1)
        self.port = self._listener.getsockname()[1]
        self._conn = None
        self._hello_result = hello_result
        self._accepted = threading.Event()
        self.disconnected = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self):
        conn, _ = self._listener.accept()
        self._conn = conn
        self._accepted.set()
        reader = conn.makefile("rb")
        while True:
            line = reader.readline()
            if not line:
                self.disconnected.set()
                return
            request = json.loads(line)
            if request.get("cmd") == "hello":
                self.send(
                    {
                        "frame": "reply",
                        "id": request["req"],
                        "result": self._hello_result or {
                            "Ok": {
                                "reply": "welcome",
                                "data": {
                                    "protocol_version": 1,
                                    "daemon_version": "stub",
                                    "instance_id": "stub",
                                    "schema_version": 1,
                                    "pid": 1,
                                    "idle_timeout_secs": None,
                                },
                            }
                        },
                    }
                )
            elif self._on_request is not None:
                self._on_request(self, request)
            # Any other line (e.g. the client's own ping) is left
            # unanswered: these tests only care about surviving a malformed
            # frame from the daemon's side.

    def send_raw(self, line: str):
        self._accepted.wait(timeout=5)
        with self._sending:
            self._conn.sendall((line + "\n").encode())

    def send(self, frame: dict):
        self.send_raw(json.dumps(frame))

    def send_many(self, frames):
        """One `sendall` for many frames, so a test pushing thousands of
        them is not thousands of syscalls."""
        self._accepted.wait(timeout=5)
        blob = "".join(json.dumps(frame) + "\n" for frame in frames)
        with self._sending:
            self._conn.sendall(blob.encode())

    def close(self):
        try:
            if self._conn is not None:
                # `shutdown` forces the FIN onto the wire even while the
                # serving thread's own `makefile()` reader still holds the
                # fd open and blocked in a read; a bare `close()` here can
                # leave the client waiting on EOF that never arrives.
                try:
                    self._conn.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                self._conn.close()
        finally:
            self._listener.close()


def close_cleanly(daemon: StubDaemon, client: AetherClient):
    """Close both ends, including the client's background reader."""
    client.close()
    client._reader.join(timeout=5)
    assert not client._reader.is_alive(), "the reader thread must have seen the close"
    daemon.close()



class MalformedFramesAreSurvived(unittest.TestCase):
    """A frame that is valid JSON but not an object, an
    unknown frame kind, or a reply with no result used to kill the reader
    thread silently (or, in one case, get queued as if it were an event) -
    every later `request()` then hung forever on `timeout=None`."""

    def _connect(self, daemon: StubDaemon) -> AetherClient:
        sock = socket.create_connection(("127.0.0.1", daemon.port))
        return AetherClient(sock)

    def test_refused_handshake_closes_the_owned_socket(self):
        daemon = StubDaemon({"Err": {"code": "unauthorized", "message": "bad token"}})
        self.addCleanup(daemon.close)
        sock = socket.create_connection(("127.0.0.1", daemon.port))
        with self.assertRaises(AetherError):
            AetherClient(sock)
        self.assertTrue(daemon.disconnected.wait(timeout=2), "failed handshake kept the socket open")

    def test_the_hello_asks_for_the_requested_protocol_version(self):
        daemon = StubDaemon()
        self.addCleanup(daemon.close)
        sock = socket.create_connection(("127.0.0.1", daemon.port))
        with patch.object(protocol, "hello", wraps=protocol.hello) as hello:
            client = AetherClient(sock, protocol_version=protocol.PROTOCOL_VERSION - 1)
            asked = hello.call_args.kwargs["protocol_version"]
        self.addCleanup(close_cleanly, daemon, client)
        self.assertEqual(asked, protocol.PROTOCOL_VERSION - 1)

    def test_an_unserved_protocol_version_is_refused_before_connecting(self):
        for version in (protocol.MIN_PROTOCOL_VERSION - 1, protocol.PROTOCOL_VERSION + 1):
            with self.subTest(version=version):
                sock = Mock()
                with self.assertRaises(ValueError):
                    AetherClient(sock, protocol_version=version)
                sock.makefile.assert_not_called()

    def test_connection_diagnostic_and_eof_wake_every_pending_request(self):
        requests = []

        def on_request(daemon, request):
            requests.append(request)
            if len(requests) == 2:
                daemon.send({"frame": "reply", "id": 0, "result": {"Err": {
                    "code": "invalid_argument", "message": "bad frame: invalid `req`"
                }}})
                daemon.close()

        daemon = StubDaemon(on_request=on_request)
        client = self._connect(daemon)
        failures = []

        def ask():
            try:
                client.request(protocol.ping(), timeout=None)
            except Exception as error:
                failures.append(error)

        threads = [threading.Thread(target=ask, daemon=True) for _ in range(2)]
        try:
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(timeout=2)
                self.assertFalse(thread.is_alive(), "closure did not wake a pending request")
            self.assertEqual(len(failures), 2)
            for error in failures:
                self.assertIsInstance(error, AetherError)
                self.assertEqual(error.code, "invalid_argument")
                self.assertIn("invalid `req`", str(error))
            with self.assertRaisesRegex(AetherError, "invalid `req`"):
                client.request(protocol.ping(), timeout=None)
        finally:
            close_cleanly(daemon, client)

    def test_unserializable_command_does_not_leave_a_pending_request(self):
        daemon = StubDaemon()
        client = self._connect(daemon)
        try:
            with self.assertRaises(TypeError):
                client.request({"cmd": "ping", "invalid": object()})
            self.assertEqual(client._pending, {})
        finally:
            close_cleanly(daemon, client)

    def test_close_unblocks_an_idle_reader_without_server_disconnect(self):
        daemon = StubDaemon()
        client = AetherClient.connect_tcp("127.0.0.1", daemon.port, "test-token")
        closed = threading.Event()
        failures = []

        def close():
            try:
                client.close()
            except Exception as error:
                failures.append(error)
            finally:
                closed.set()

        closer = threading.Thread(target=close, daemon=True)
        closer.start()
        try:
            self.assertTrue(closed.wait(timeout=2), "close blocked on the socket reader")
            self.assertEqual(failures, [])
            client._reader.join(timeout=2)
            self.assertFalse(client._reader.is_alive())
            client.close()  # Closing twice is harmless.
        finally:
            daemon.close()
            closer.join(timeout=2)

    def test_a_list_instead_of_an_object_fails_the_connection(self):
        daemon = StubDaemon()
        self.addCleanup(daemon.close)
        client = self._connect(daemon)
        self.addCleanup(client.close)
        daemon.send_raw("[1, 2, 3]")
        with self.assertRaises(ConnectionError):
            client.request(protocol.ping(), timeout=5)
        self.assertTrue(client._closed)

    def test_a_reply_with_no_result_fails_the_connection(self):
        daemon = StubDaemon()
        self.addCleanup(daemon.close)
        client = self._connect(daemon)
        self.addCleanup(client.close)
        daemon.send_raw('{"frame": "reply", "id": 1}')
        with self.assertRaises(ConnectionError):
            client.request(protocol.ping(), timeout=5)
        self.assertTrue(client._closed)

    def test_an_unknown_frame_kind_fails_the_connection(self):
        daemon = StubDaemon()
        self.addCleanup(daemon.close)
        client = self._connect(daemon)
        self.addCleanup(client.close)
        daemon.send({"frame": "something_new", "sub": 1})
        with self.assertRaises(ConnectionError):
            client.request(protocol.ping(), timeout=5)
        self.assertTrue(client._closed)

    def test_malformed_subscription_reply_settles_its_waiter(self):
        def respond(daemon, request):
            daemon.send({
                "frame": "reply", "id": request["req"],
                "result": {"Ok": {"reply": "subscribed", "data": {"id": 1}}},
            })

        daemon = StubDaemon(on_request=respond)
        client = self._connect(daemon)
        self.addCleanup(close_cleanly, daemon, client)
        with self.assertRaises(ConnectionError):
            client.subscribe(after=0)

    def test_a_request_already_in_flight_is_settled_not_hung(self):
        """The exact failure the review reproduced: a caller blocked in
        `request()` when the malformed frame arrives must be woken with an
        error, not left waiting - this pins that with no explicit timeout
        (the client's own default bounds it either way)."""
        daemon = StubDaemon()
        self.addCleanup(daemon.close)
        client = self._connect(daemon)
        self.addCleanup(client.close)

        result = {}

        def waiter():
            try:
                client.request(protocol.ping(), timeout=10)
            except Exception as exc:  # noqa: BLE001 - captured for the assertion below
                result["error"] = exc

        thread = threading.Thread(target=waiter)
        thread.start()
        time.sleep(0.1)  # let the request land before the malformed reply
        daemon.send_raw("[1, 2, 3]")
        thread.join(timeout=5)
        self.assertFalse(thread.is_alive(), "the waiting request must not hang")
        self.assertIsInstance(result.get("error"), ConnectionError)

    def test_default_request_timeout_bounds_a_silent_daemon(self):
        daemon = StubDaemon()
        client = self._connect(daemon)
        started = time.monotonic()
        with self.assertRaises(TimeoutError):
            client.request(protocol.ping(), timeout=0.2)
        self.assertLess(time.monotonic() - started, 2.0)
        close_cleanly(daemon, client)


class FramesQueueIsWellBehaved(unittest.TestCase):
    """`_frames` was unbounded, and held more than
    `next_frame`'s own docstring promises - a reply to a request that had
    already timed out was queued as if it were an event."""

    def _connect(self, daemon: StubDaemon) -> AetherClient:
        sock = socket.create_connection(("127.0.0.1", daemon.port))
        return AetherClient(sock)

    def test_a_late_reply_after_timeout_is_dropped_not_queued(self):
        daemon = StubDaemon()
        client = self._connect(daemon)
        with self.assertRaises(TimeoutError):
            client.request(protocol.ping(), timeout=0.1)
        # The daemon answers late, after the caller already gave up. The
        # hello was request 1, so the ping above was request 2.
        daemon.send({"frame": "reply", "id": 2, "result": {"Ok": {"reply": "ok"}}})
        with self.assertRaises(TimeoutError):
            client.next_frame(timeout=0.3)
        close_cleanly(daemon, client)


class SubscriptionQueues(unittest.TestCase):
    def setUp(self):
        self.next_sub = 0
        self.history = []
        self.released = threading.Event()
        self.daemon = StubDaemon(on_request=self.respond)
        self.client = AetherClient.connect_tcp("127.0.0.1", self.daemon.port, "token")
        self.addCleanup(close_cleanly, self.daemon, self.client)

    def respond(self, daemon, request):
        cmd = request["cmd"]
        if cmd == "subscribe":
            self.next_sub += 1
            cursor = request.get("after", 0)
            result = {"reply": "subscribed", "data": {"id": self.next_sub, "last_seq": cursor}}
            frames = [{"frame": "reply", "id": request["req"], "result": {"Ok": result}}]
            frames.extend(self.event(self.next_sub, seq) for seq in self.history if seq > cursor)
            daemon.send_many(frames)
        else:
            daemon.send({"frame": "reply", "id": request["req"], "result": {"Ok": {"reply": "ok"}}})
            if cmd == "unsubscribe":
                self.released.set()

    @staticmethod
    def event(sub, seq):
        return {"frame": "event", "sub": sub, "event": {"seq": seq}}

    def overflow(self, sub):
        self.daemon.send_many(self.event(sub.id, seq) for seq in range(1, 1027))
        self.assertTrue(self.released.wait(5), "overflow must cancel daemon delivery")
        self.client.ping()  # Reader barrier after the sent frames.

    def test_overflow_ends_only_lagging_subscription_and_requests_continue(self):
        slow = self.client.subscribe(after=0)
        fast = self.client.subscribe(after=0)
        self.assertEqual(slow["id"], slow.id)
        self.assertEqual(slow["last_seq"], 0)
        self.overflow(slow)
        self.daemon.send(self.event(fast.id, 77))
        self.assertEqual(fast.next_frame(timeout=2)["event"]["seq"], 77)
        self.assertEqual(self.client.ping(), {"reply": "ok"})
        self.assertEqual(len(slow._frames), MAX_QUEUED_SUBSCRIPTION_FRAMES)
        seqs = [slow.next_frame(timeout=2)["event"]["seq"] for _ in range(1024)]
        self.assertEqual(seqs, list(range(1, 1025)))
        with self.assertRaises(SubscriptionLaggedError) as raised:
            slow.next_frame(timeout=2)
        self.assertEqual(raised.exception.subscription_id, slow.id)
        self.assertEqual(raised.exception.last_seq, 1024)
        self.assertEqual(raised.exception.capacity, 1024)
        with self.assertRaises(SubscriptionLaggedError):
            slow.next_frame(timeout=0)
        self.daemon.send(self.event(fast.id, 78))
        self.assertEqual(fast.next_frame(timeout=2)["event"]["seq"], 78)

    def test_resuming_from_reported_sequence_has_no_gaps(self):
        sub = self.client.subscribe(after=0)
        self.overflow(sub)
        consumed = [sub.next_frame(timeout=2)["event"]["seq"] for _ in range(1024)]
        with self.assertRaises(SubscriptionLaggedError) as raised:
            sub.next_frame(timeout=2)
        self.history = list(range(1, 1031))
        resumed = self.client.subscribe(after=raised.exception.last_seq)
        consumed.extend(resumed.next_frame(timeout=2)["event"]["seq"] for _ in range(6))
        self.assertEqual(consumed, self.history)

    def test_merged_events_raise_instead_of_silently_dropping(self):
        sub = self.client.subscribe(after=0)
        other = self.client.subscribe(after=0)
        self.overflow(sub)
        events = self.client.events(timeout=2)
        self.assertEqual([next(events)["seq"] for _ in range(1024)], list(range(1, 1025)))
        with self.assertRaises(SubscriptionLaggedError) as raised:
            next(events)
        self.assertEqual(raised.exception.last_seq, 1024)
        self.daemon.send(self.event(other.id, 2000))
        self.assertEqual(self.client.next_frame(timeout=2)["event"]["seq"], 2000)
        with self.assertWarns(DeprecationWarning):
            self.assertEqual(self.client.dropped_frames, 0)

    def test_handle_and_merged_reader_share_cursor_without_duplicate_frames(self):
        sub = self.client.subscribe(after=0)
        other = self.client.subscribe(after=0)
        self.daemon.send_many([self.event(sub.id, 1), self.event(other.id, 2), self.event(sub.id, 3)])
        self.client.ping()
        self.assertEqual(sub.next_frame(timeout=2)["event"]["seq"], 1)
        self.assertEqual(self.client.next_frame(timeout=2)["event"]["seq"], 2)
        self.assertEqual(self.client.next_frame(timeout=2)["event"]["seq"], 3)
        with self.assertRaises(TimeoutError):
            sub.next_frame(timeout=0)

    def test_merged_notice_keeps_arrival_order(self):
        first = self.client.subscribe(after=0)
        second = self.client.subscribe(after=0)
        notice = {"frame": "reply", "id": 0, "result": {"Ok": {"reply": "ok"}}}
        frames = [self.event(first.id, 1), self.event(second.id, 2), notice, self.event(first.id, 3)]
        self.daemon.send_many(frames)
        self.client.ping()
        self.assertEqual([self.client.next_frame(timeout=2) for _ in frames], frames)

    def test_delta_only_overflow_reports_initial_cursor(self):
        sub = self.client.subscribe(after=19, deltas=True)
        self.daemon.send_many(
            {"frame": "delta", "sub": sub.id, "delta": {"type": "text", "text": "x"}}
            for _ in range(1025)
        )
        self.assertTrue(self.released.wait(5))
        for _ in range(1024):
            self.assertEqual(sub.next_frame(timeout=2)["frame"], "delta")
        with self.assertRaises(SubscriptionLaggedError) as raised:
            sub.next_frame(timeout=2)
        self.assertEqual(raised.exception.last_seq, 19)

    def test_reply_and_replay_in_one_packet_are_registered_before_delivery(self):
        self.history = [1, 2, 3]
        sub = self.client.subscribe(after=0)
        self.assertEqual([sub.next_frame(timeout=2)["event"]["seq"] for _ in range(3)], self.history)

    def test_close_wakes_subscription_reader(self):
        sub = self.client.subscribe(after=0)
        waiting = threading.Event()
        failures = []
        original_wait = self.client._state.wait

        def wait(timeout=None):
            waiting.set()
            return original_wait(timeout)

        def read():
            try:
                sub.next_frame(timeout=None)
            except Exception as error:
                failures.append(error)

        with patch.object(self.client._state, "wait", side_effect=wait):
            reader = threading.Thread(target=read, daemon=True)
            reader.start()
            self.assertTrue(waiting.wait(2), "subscription reader did not block")
            self.client.close()
            reader.join(timeout=2)
            self.assertFalse(reader.is_alive(), "close did not wake subscription reader")
        self.assertEqual(len(failures), 1)
        self.assertIsInstance(failures[0], ConnectionError)

    def test_unsubscribe_ends_handle_and_leaves_other_subscription_usable(self):
        sub = self.client.subscribe(after=0)
        other = self.client.subscribe(after=0)
        sub.unsubscribe()
        with self.assertRaisesRegex(ConnectionError, "subscription ended"):
            sub.next_frame(timeout=None)
        self.daemon.send(self.event(other.id, 1))
        self.assertEqual(other.next_frame(timeout=2)["event"]["seq"], 1)


class StreamingExample(unittest.TestCase):
    def test_lag_resumes_from_reported_cursor(self):
        example = runpy.run_path(os.path.join(os.path.dirname(__file__), "..", "examples", "list_and_stream.py"))
        client = Mock()
        client.welcome = {"daemon_version": "stub", "protocol_version": 1, "instance_id": "stub"}
        client.request.return_value = {"data": []}
        first = Mock()
        first.events.side_effect = SubscriptionLaggedError(1, 1024)
        resumed = Mock()
        resumed.events.side_effect = KeyboardInterrupt
        client.subscribe.side_effect = [first, resumed]
        with patch.object(AetherClient, "connect", return_value=client), patch("builtins.print"):
            example["main"]()
        self.assertEqual(client.subscribe.call_args_list, [call(after=0), call(after=1024)])
        client.close.assert_called_once_with()


class SplitHostPort(unittest.TestCase):
    """MINOR: `connect()` could not parse an IPv6 `AETHER_ADDRESS`."""

    def test_ipv6_with_brackets(self):
        from aether.client import _split_host_port

        self.assertEqual(_split_host_port("[::1]:4433"), ("::1", "4433"))

    def test_ipv4_still_works(self):
        from aether.client import _split_host_port

        self.assertEqual(_split_host_port("127.0.0.1:4433"), ("127.0.0.1", "4433"))

    def test_no_colon_raises_a_named_error(self):
        from aether.client import _split_host_port

        with self.assertRaises(ValueError):
            _split_host_port("no-port-here")

    def test_malformed_brackets_raise_a_named_error(self):
        from aether.client import _split_host_port

        with self.assertRaises(ValueError):
            _split_host_port("[::1")


if __name__ == "__main__":
    unittest.main()
