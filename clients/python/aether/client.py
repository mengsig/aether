"""A client for the Aether daemon protocol: NDJSON over a Unix socket or TCP.

Hand-written, and deliberately small. The types and command builders beside
it in `protocol.py` are generated from the Rust types; this file is the part
that owns a connection - the handshake, request/reply multiplexing over one
socket, subscriptions, and the keepalive a tokened TCP transport needs.

One thread reads the socket and nothing else does. Replies are routed to the
request that is waiting for them by id. Each subscription owns a bounded
queue, drained either through its handle or the merged `next_frame` reader.
The client is usable from more than one thread at a time, including by its
own keepalive, which is an ordinary request like any other.
"""

from __future__ import annotations

from collections import deque
import errno
import json
import os
import socket
import threading
import time
import warnings

from . import protocol

CLIENT_NAME = "aether-python"
CLIENT_VERSION = "1.2.5"

# Longest accepted frame, matching the daemon's own cap.
MAX_LINE = 64 * 1024 * 1024

MAX_QUEUED_SUBSCRIPTION_FRAMES = 1024
# Deprecated alias; the limit now applies to each subscription.
MAX_QUEUED_FRAMES = MAX_QUEUED_SUBSCRIPTION_FRAMES

# No caller of `request` waits forever on a daemon that has stopped
# answering; see the class docstring on `request`.
DEFAULT_REQUEST_TIMEOUT = 60.0


class AetherError(RuntimeError):
    """The daemon refused a request. `code` is the protocol's error code."""

    def __init__(self, code, message):
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


class SubscriptionLaggedError(RuntimeError):
    """Terminal overflow; resume events after `last_seq`. Deltas cannot replay."""

    capacity = MAX_QUEUED_SUBSCRIPTION_FRAMES

    def __init__(self, subscription_id, last_seq):
        super().__init__(
            f"subscription {subscription_id} exceeded {self.capacity} queued frames; "
            f"resubscribe after event sequence {last_seq}"
        )
        self.subscription_id = subscription_id
        self.last_seq = last_seq


class Subscription(dict):
    """A subscription handle, retaining the legacy `id`/`last_seq` mapping."""

    def __init__(self, client, data):
        super().__init__(data)
        self.id = data["id"]
        self.last_seq = data["last_seq"]
        self._client = client
        self._frames = deque()
        self._lagged = False
        self._ended = False
        self._last_delivered_seq = self.last_seq

    def next_frame(self, timeout=60.0):
        """Drain frames, then raise the terminal lag or connection error."""
        return self._client._take_frame(self, timeout)

    def events(self, timeout=60.0):
        while True:
            frame = self.next_frame(timeout)
            if frame["frame"] == "event":
                yield frame["event"]

    def unsubscribe(self):
        with self._client._state:
            self._ended = True
            self._frames.clear()
            self._client._subscriptions.pop(self.id, None)
            self._client._state.notify_all()
        return self._client.request(protocol.unsubscribe(id=self.id))


def default_socket_path():
    """Where the daemon listens, resolved the way the daemon resolves it."""
    if os.environ.get("AETHER_SOCKET"):
        return os.environ["AETHER_SOCKET"]
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        return os.path.join(runtime, "aether", "aetherd.sock")
    data = os.environ.get("AETHER_DATA_DIR") or os.path.expanduser("~/.local/share/aether")
    return os.path.join(data, "aetherd.sock")


def _split_host_port(address):
    """Split `host:port`, accepting a bracketed IPv6 host (`[::1]:1234`) the
    way a plain `rpartition(":")` cannot - and a missing separator is a
    named error here rather than whatever `int()` raises on the whole
    string."""
    if address.startswith("["):
        end = address.find("]")
        if end == -1 or address[end + 1 : end + 2] != ":" or not address[end + 2 :]:
            raise ValueError(f"not a valid host:port address: {address!r}")
        return address[1:end], address[end + 2 :]
    host, sep, port = address.rpartition(":")
    if not sep or not host:
        raise ValueError(f"not a valid host:port address: {address!r}")
    return host, port


class _Waiter:
    """One request's slot: the reader fills it and wakes the caller."""

    __slots__ = ("done", "result", "error")

    def __init__(self):
        self.done = threading.Event()
        self.result = None
        self.error = None


class AetherClient:
    """One connection, usable from several threads. Requests block until
    their reply arrives; each subscription buffers event and delta frames
    for its handle or the merged `next_frame` reader."""

    def __init__(self, sock, token=None, client_name=CLIENT_NAME,
                 protocol_version=protocol.PROTOCOL_VERSION):
        """`protocol_version` is the version the hello asks for, so a caller
        can speak to an older daemon within `MIN_PROTOCOL_VERSION`."""
        if not protocol.MIN_PROTOCOL_VERSION <= protocol_version <= protocol.PROTOCOL_VERSION:
            raise ValueError(
                f"protocol {protocol_version} is outside this client's "
                f"{protocol.MIN_PROTOCOL_VERSION} to {protocol.PROTOCOL_VERSION}"
            )
        self.sock = sock
        self.file = sock.makefile("rwb")
        # Held while writing a frame, so two threads never interleave bytes.
        self._writing = threading.Lock()
        # Guards request state and subscription buffers; wakes frame readers.
        self._state = threading.Condition()
        self._next_id = 1
        self._pending = {}
        self._subscriptions = {}
        self._frame_order = 0
        self._notice_frame = None
        self._failure = None
        self._closed = False
        # Set on close, so the keepalive stops at once rather than after one
        # more interval of sleeping.
        self._stopping = threading.Event()
        # The daemon's last word on request id 0; see `_route`.
        self.notice = None
        self._keepalive = None
        self._reader = threading.Thread(
            target=self._read_until_closed, name="aether-reader", daemon=True
        )
        self._reader.start()
        try:
            self.welcome = self.request(
                protocol.hello(
                    protocol_version=protocol_version,
                    client_name=client_name,
                    client_version=CLIENT_VERSION,
                    features=["deltas", "artifact_bytes"],
                    token=token,
                )
            )["data"]
            self._start_keepalive()
        except Exception as error:
            try:
                self.close()
            except Exception as cleanup_error:
                raise error from cleanup_error
            raise

    # -- construction ------------------------------------------------------

    @classmethod
    def connect_unix(cls, path=None, **kwargs):
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.connect(path or default_socket_path())
            return cls(sock, **kwargs)
        except Exception:
            sock.close()
            raise

    @classmethod
    def connect_tcp(cls, host, port, token, **kwargs):
        """The TCP transport authorizes on the token alone; it is required."""
        if not token:
            raise ValueError("the TCP transport needs a token")
        sock = socket.create_connection((host, int(port)))
        try:
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            return cls(sock, token=token, **kwargs)
        except Exception:
            sock.close()
            raise

    @classmethod
    def connect(cls, address=None, token=None, **kwargs):
        """`host:port` when given (or in AETHER_ADDRESS), else the socket."""
        address = address or os.environ.get("AETHER_ADDRESS")
        if not address:
            return cls.connect_unix(**kwargs)
        host, port = _split_host_port(address)
        return cls.connect_tcp(host, port, token or os.environ.get("AETHER_TOKEN"), **kwargs)

    # -- the reader --------------------------------------------------------

    def _read_until_closed(self):
        """The only thread that reads the socket. Ends by settling everyone
        waiting on it, so a dropped connection - or a frame that does not
        fit this protocol's shape - wakes every caller instead of leaving
        them blocked or killing this thread silently."""
        try:
            while True:
                line = self.file.readline(MAX_LINE)
                if not line:
                    self._fail(
                        self.notice or ConnectionError("the daemon closed the connection")
                    )
                    return
                try:
                    frame = json.loads(line)
                except ValueError as bad:
                    self._fail(ConnectionError(f"the daemon sent a frame that is not JSON: {bad}"))
                    return
                if not isinstance(frame, dict):
                    self._fail(
                        ConnectionError(f"the daemon sent a frame that is not an object: {frame!r}")
                    )
                    return
                self._route(frame)
                if self._closed:
                    return
        except (OSError, ValueError) as failed:
            self._fail(ConnectionError(f"the connection to the daemon failed: {failed}"))
        except Exception as failed:  # the reader must never die without settling every waiter
            self._fail(ConnectionError(f"the daemon sent a frame this client could not handle: {failed}"))

    def _route(self, frame):
        kind = frame.get("frame")
        if kind == "reply":
            if "result" not in frame:
                self._fail(ConnectionError(f"the daemon sent a reply with no result: {frame!r}"))
                return
            req_id = frame.get("id")
            with self._state:
                waiter = self._pending.get(req_id)
                result = frame["result"]
                reply = result.get("Ok", {})
                if waiter is not None:
                    if reply.get("reply") == "subscribed":
                        # Register before waking the caller: replay can immediately follow.
                        subscription = Subscription(self, reply["data"])
                        self._subscriptions[subscription.id] = subscription
                        reply["data"] = subscription
                    self._pending.pop(req_id)
                    waiter.result = result
                    waiter.done.set()
                    return
            if reply.get("reply") == "subscribed":
                self._release_subscription(reply["data"]["id"])
                return
            # Request ids start at 1, so a reply on id 0 is the daemon
            # speaking on its own account rather than answering anything -
            # the reason a connection is about to close. Kept, so the close
            # that follows says why, and queued, so a caller reading frames
            # sees it too.
            if req_id == 0:
                error = (frame.get("result") or {}).get("Err")
                if error:
                    self.notice = AetherError(error["code"], error["message"])
                with self._state:
                    self._frame_order += 1
                    self._notice_frame = (self._frame_order, frame)
                    self._state.notify_all()
                return
            # Any other id with no waiter is a reply to a request that
            # already gave up (timed out): not a frame `next_frame`/`events`
            # promise to hand back.
            return
        if kind not in ("event", "delta"):
            self._fail(ConnectionError(f"the daemon sent an unknown frame kind: {kind!r}"))
            return
        self._enqueue_frame(frame)

    def _enqueue_frame(self, frame):
        with self._state:
            subscription = self._subscriptions.get(frame["sub"])
            if subscription is None or subscription._lagged or subscription._ended:
                return
            if len(subscription._frames) < MAX_QUEUED_SUBSCRIPTION_FRAMES:
                self._frame_order += 1
                subscription._frames.append((self._frame_order, frame))
                self._state.notify_all()
                return
            subscription._lagged = True
            self._state.notify_all()
        self._release_subscription(subscription.id)

    def _release_subscription(self, subscription_id):
        # Never wait for an unsubscribe reply on the socket reader thread.
        def release():
            try:
                self.request(protocol.unsubscribe(id=subscription_id))
            except (OSError, ValueError, TimeoutError, AetherError) as error:
                if not self._closed:
                    warnings.warn(f"could not release subscription {subscription_id}: {error}")

        threading.Thread(target=release, name="aether-unsubscribe", daemon=True).start()

    def _fail(self, error):
        """Close the connection's logical state exactly once and wake every
        caller with the reason."""
        with self._state:
            if self._closed:
                return
            if self._failure is None:
                self._failure = error
            self._closed = True
            waiting = list(self._pending.values())
            self._pending.clear()
        for waiter in waiting:
            waiter.error = error
            waiter.done.set()
        with self._state:
            self._state.notify_all()

    # -- frames ------------------------------------------------------------

    def _send(self, frame):
        line = json.dumps(frame) + "\n"
        with self._writing:
            self.file.write(line.encode())
            self.file.flush()

    def request(self, command, timeout=DEFAULT_REQUEST_TIMEOUT):
        """Send one command and return its `Ok` payload, raising on refusal.

        Blocks until the reply arrives, the connection drops, or `timeout`
        seconds pass - `None` waits forever. Safe to call from several
        threads at once."""
        waiter = _Waiter()
        with self._state:
            if self._closed:
                raise self._failure or ConnectionError("the connection is closed")
            req = self._next_id
            self._next_id += 1
            self._pending[req] = waiter
        try:
            self._send({"req": req, **command})
        except Exception:
            with self._state:
                self._pending.pop(req, None)
            raise
        if not waiter.done.wait(timeout):
            with self._state:
                self._pending.pop(req, None)
            raise TimeoutError(f"no reply to request {req} within {timeout}s")
        if waiter.error is not None:
            raise waiter.error
        result = waiter.result or {}
        if "Err" in result:
            raise AetherError(result["Err"]["code"], result["Err"]["message"])
        return result["Ok"]

    @property
    def dropped_frames(self):
        """Deprecated: always zero. Overflow now raises SubscriptionLaggedError."""
        warnings.warn(
            "dropped_frames is deprecated; handle SubscriptionLaggedError instead",
            DeprecationWarning,
            stacklevel=2,
        )
        return 0

    def next_frame(self, timeout=60.0):
        """Drain active subscriptions in arrival order, or a daemon notice.

        Overflow raises SubscriptionLaggedError after that subscription's
        retained prefix. Other subscriptions and requests remain usable.
        """
        return self._take_frame(None, timeout)

    def _take_frame(self, subscription, timeout):
        deadline = None if timeout is None else time.monotonic() + timeout
        with self._state:
            while True:
                if subscription is None:
                    candidates = list(self._subscriptions.values())
                    ready = [sub for sub in candidates if sub._frames or sub._lagged]
                    selected = min(
                        ready,
                        key=lambda sub: sub._frames[0][0] if sub._frames else -1,
                        default=None,
                    )
                    if self._notice_frame is not None and (
                        selected is None or (
                            selected._frames and self._notice_frame[0] < selected._frames[0][0]
                        )
                    ):
                        _, frame = self._notice_frame
                        self._notice_frame = None
                        return frame
                else:
                    selected = subscription
                if selected is not None:
                    if selected._frames:
                        _, frame = selected._frames.popleft()
                        if frame["frame"] == "event":
                            selected._last_delivered_seq = frame["event"]["seq"]
                        return frame
                    if selected._lagged:
                        self._subscriptions.pop(selected.id, None)
                        raise SubscriptionLaggedError(selected.id, selected._last_delivered_seq)
                    if selected._ended:
                        raise ConnectionError("the subscription ended")
                if self._closed:
                    raise self._failure or ConnectionError("the connection is closed")
                remaining = None if deadline is None else deadline - time.monotonic()
                if remaining is not None and remaining <= 0:
                    raise TimeoutError(f"no frame within {timeout}s")
                self._state.wait(remaining)

    # -- subscriptions -----------------------------------------------------

    def subscribe(self, after=None, query=None, deltas=False):
        """Start a subscription. `after=0` replays from the beginning of the
        log; `None` starts from now. Returns a Subscription handle with the
        legacy `id` and initial `last_seq` mapping keys."""
        return self.request(
            protocol.subscribe(after=after, query=query or {}, deltas=deltas)
        )["data"]

    def events(self, timeout=60.0):
        """Event payloads from every active subscription, as they arrive."""
        while True:
            frame = self.next_frame(timeout=timeout)
            if frame.get("frame") == "event":
                yield frame["event"]

    # -- lifecycle ---------------------------------------------------------

    def ping(self):
        """Hold a quiet connection open. The keepalive sends this for you;
        call it directly if you turned the keepalive off."""
        return self.request(protocol.ping())

    def _start_keepalive(self):
        """A TCP connection that says nothing is closed by the daemon. Ping
        well inside that window."""
        idle = (self.welcome or {}).get("idle_timeout_secs")
        if not idle or idle < 3:
            return
        self._keepalive = threading.Thread(
            target=self._ping_until_closed,
            args=(idle / 3.0,),
            name="aether-keepalive",
            daemon=True,
        )
        self._keepalive.start()

    def _ping_until_closed(self, every):
        while not self._closed:
            if self._stopping.wait(every):
                return
            try:
                self.ping()
            except (OSError, ValueError, TimeoutError, AetherError, ConnectionError):
                return

    def close(self):
        self._fail(ConnectionError("the client closed the connection"))
        self._stopping.set()
        try:
            # Wake the reader before closing its buffered file, whose lock
            # is held while a socket read is blocked.
            try:
                self.sock.shutdown(socket.SHUT_RDWR)
            except OSError as error:
                if error.errno not in (errno.ENOTCONN, errno.EBADF):
                    raise
        finally:
            try:
                self.file.close()
            finally:
                self.sock.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
