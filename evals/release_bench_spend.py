"""Per-trial spend allowance: a live guard that stops a runaway trial, and the budget reservation it implies.

While a trial runs, its spend is read from what its harness writes anyway
(Claude Code's stream-json, Codex's session rollouts, Aether's event store) and
priced on the run's pinned table. When it reaches the arm's allowance, every
process carrying the trial's marker is stopped. The run's budget reserves the
allowance plus the worst overshoot: calls finished but not yet seen, and the
call in flight when the trial is stopped, each at its largest possible price.
"""
import datetime
import json
import os
import signal
import sqlite3
import threading
import time
from contextlib import closing
from pathlib import Path

from harness_bench_pricing import estimate_cost, resolve_price
from harness_bench_usage import UsageError, normalize_usage

ALLOWANCE_USD = {"haiku": 0.50, "luna": 0.15}
PRICED_MODEL = {"haiku": ("claude-code", "claude-haiku-5-5"), "luna": ("codex", "gpt-6-luna")}
MARKER = "RELEASE_BENCH_TRIAL"
POLL_SECONDS = 1.0
# One call finished since the last poll plus the one in flight at the stop, per model-calling agent.
OVERSHOOT_CALLS = 2
# Delegates a delegation task may run at once; each arm installs the limit (release_bench_arms.agent_cap).
DELEGATION_AGENTS = 4
UNOBSERVABLE_SECONDS = 30
STOP_GRACE_SECONDS = 5


class ProbeError(RuntimeError):
    """A spend source exists but could not be read."""


def _engine_row(table, model):
    provider, name = PRICED_MODEL[model]
    rows = [row for row in table["models"] if row["model"] == {"provider": provider, "model": name}]
    assert len(rows) == 1, f"the price table lists {provider}/{name} once"
    return rows[0]


def price_of(table, model):
    """The pinned price entry an arm's usage is priced on."""
    provider, name = PRICED_MODEL[model]
    price = resolve_price(table, provider, name)
    if price is None:
        raise ValueError(f"the pinned price table has no price for {provider}/{name}")
    return price


def max_call_usd(table, model):
    """The most one model call can cost: a full context at the dearest input rate plus a full output."""
    cost = price_of(table, model)["capabilities"]["cost"]
    tiers = [cost] + ([cost["above_threshold"]] if "above_threshold" in cost else [])
    input_rate = max(tier.get(key, 0) for tier in tiers
                     for key in ("input_per_mtok", "cache_write_per_mtok", "cache_write_1h_per_mtok"))
    output_rate = max(tier["output_per_mtok"] for tier in tiers)
    capabilities = _engine_row(table, model)["capabilities"]
    assert capabilities["context_window"] > 0 and capabilities["max_output_tokens"] > 0, "limits are positive"
    return (capabilities["context_window"] * input_rate + capabilities["max_output_tokens"] * output_rate) / 1e6


def reservation_usd(table, model, agents=1):
    """What the run's budget holds for one trial: its allowance plus the bounded overshoot."""
    assert agents >= 1, "a trial runs at least one agent"
    reserve = ALLOWANCE_USD[model] + OVERSHOOT_CALLS * agents * max_call_usd(table, model)
    assert reserve > ALLOWANCE_USD[model], "a reservation covers more than the allowance"
    return reserve


def _priced(usage, price):
    """USD of normalized usage: the pinned estimate, or the harness's own figure when that is higher."""
    estimate = estimate_cost(usage, price) or 0.0
    return max(estimate, usage.get("api_equivalent_cost_usd") or 0.0)


class JsonlTail:
    """Complete JSON lines appended to a file since the last read; a torn last line waits for its end."""

    def __init__(self, path):
        self.path, self.offset, self.partial = Path(path), 0, b""

    def read(self):
        if not self.path.exists():
            return []
        try:
            with self.path.open("rb") as stream:
                stream.seek(self.offset)
                chunk = stream.read()
        except OSError as error:
            raise ProbeError(f"{self.path}: {error}") from error
        self.offset += len(chunk)
        lines = (self.partial + chunk).split(b"\n")
        self.partial = lines.pop()
        events = []
        for line in lines:
            try:
                event = json.loads(line)
            except ValueError:
                continue  # Not every line a CLI prints is an event; its usage lines are.
            if isinstance(event, dict):
                events.append(event)
        return events


class ClaudeStreamProbe:
    """Spend of a `claude -p --output-format stream-json` run, one count per assistant message id."""

    def __init__(self, stdout_path, price):
        self.tail, self.price, self.usage = JsonlTail(stdout_path), price, {}

    def __call__(self):
        if not self.tail.path.exists():
            return None
        for event in self.tail.read():
            message = event.get("message") if event.get("type") == "assistant" else None
            if isinstance(message, dict) and message.get("id") and isinstance(message.get("usage"), dict):
                self.usage[message["id"]] = message["usage"]
        try:
            return sum(_priced(normalize_usage(usage, "claude"), self.price) for usage in self.usage.values())
        except UsageError as error:
            raise ProbeError(f"Claude usage: {error}") from error


class CodexRolloutProbe:
    """Spend of a `codex exec --json` run and every subagent thread it spawned, from their session rollouts.

    A subagent's rollout opens with a session_meta naming the root's session (and its parent
    thread); each thread's last cumulative token count is its usage, priced separately."""

    def __init__(self, stdout_path, price, codex_home=None):
        self.tail, self.price = JsonlTail(stdout_path), price
        self.home = Path(codex_home or os.environ.get("CODEX_HOME") or Path.home() / ".codex")
        self.thread_id = self.root_name = None
        self.rollouts, self.totals, self.foreign = {}, {}, set()

    def _days(self):
        days = {datetime.date.today(), datetime.date.today() - datetime.timedelta(days=1),
                datetime.datetime.now(datetime.timezone.utc).date()}
        folders = (self.home / "sessions" / f"{day:%Y}" / f"{day:%m}" / f"{day:%d}" for day in sorted(days))
        return [folder for folder in folders if folder.is_dir()]

    def _find_root(self):
        for folder in self._days():
            found = sorted(folder.glob(f"rollout-*{self.thread_id}.jsonl"))
            if found:
                self.root_name = found[0].name
                self.rollouts[self.thread_id] = JsonlTail(found[0])
                return

    def _find_descendants(self):
        """Adopt every rollout started since the root whose header places it in the root's session."""
        assert self.root_name is not None and self.thread_id in self.rollouts, "the root's rollout is found first"
        known = {tail.path for tail in self.rollouts.values()}
        for folder in self._days():
            # Rollout names begin with their start time, so a descendant's sorts after the root's.
            for path in sorted(folder.glob("rollout-*.jsonl")):
                if path.name <= self.root_name or path in known or path in self.foreign:
                    continue
                meta = _rollout_header(path)
                if meta is None:
                    continue  # Its header is not written yet; the next poll reads it.
                member = self.thread_id in (meta.get("session_id"), meta.get("parent_thread_id"))
                if member or meta.get("parent_thread_id") in self.rollouts:
                    assert isinstance(meta.get("id"), str), f"{path}: a session_meta names its thread"
                    self.rollouts[meta["id"]] = JsonlTail(path)
                else:
                    self.foreign.add(path)

    def __call__(self):
        for event in self.tail.read():
            if event.get("type") == "thread.started" and isinstance(event.get("thread_id"), str):
                self.thread_id = event["thread_id"]
        if self.thread_id is None:
            return None
        if self.root_name is None:
            self._find_root()
        if self.root_name is None:
            return None
        self._find_descendants()
        for thread, rollout in self.rollouts.items():
            total = _last_token_total(rollout.read())
            if total is not None:
                self.totals[thread] = total
        # A thread without a token count yet has not finished its first model call.
        try:
            return sum(_priced(normalize_usage(total, "codex"), self.price) for total in self.totals.values())
        except UsageError as error:
            raise ProbeError(f"Codex usage: {error}") from error

    def descendants(self):
        """{thread id: (USD, tokens)} of every subagent thread seen so far, the root excluded."""
        assert self.thread_id is not None, "descendants are known only once the root thread is"
        try:
            usages = {thread: normalize_usage(total, "codex") for thread, total in self.totals.items()
                      if thread != self.thread_id}
        except UsageError as error:
            raise ProbeError(f"Codex usage: {error}") from error
        return {thread: (_priced(usage, self.price), usage["total_tokens"] or 0) for thread, usage in usages.items()}


def _rollout_header(path):
    """The session_meta payload a rollout opens with, or None while its first line is incomplete."""
    try:
        with path.open("rb") as stream:
            line = stream.readline()
    except OSError as error:
        raise ProbeError(f"{path}: {error}") from error
    if not line.endswith(b"\n"):
        return None
    try:
        event = json.loads(line)
    except ValueError:
        return {}  # Not a rollout Codex wrote, so no thread of this run.
    payload = event.get("payload") if isinstance(event, dict) and event.get("type") == "session_meta" else None
    return payload if isinstance(payload, dict) else {}


def _last_token_total(events):
    """The last cumulative token usage among a rollout's events, or None when they carry none."""
    total = None
    for event in events:
        payload = event.get("payload") if event.get("type") == "event_msg" else None
        info = payload.get("info") if isinstance(payload, dict) and payload.get("type") == "token_count" else None
        if isinstance(info, dict) and isinstance(info.get("total_token_usage"), dict):
            total = info["total_token_usage"]
    return total


class AetherStoreProbe:
    """Spend of every agent in an Aether data directory, from its latest cumulative usage event."""

    def __init__(self, data_dir, price):
        self.database, self.price = Path(data_dir) / "aether.db", price
        self.seq, self.totals = 0, {}

    def __call__(self):
        if not self.database.exists():
            return None
        try:
            uri = self.database.resolve().as_uri() + "?mode=ro"
            with closing(sqlite3.connect(uri, uri=True, timeout=2)) as db:
                rows = db.execute("SELECT seq, body FROM events WHERE seq > ? ORDER BY seq", (self.seq,)).fetchall()
        except sqlite3.Error as error:
            raise ProbeError(f"{self.database}: {error}") from error
        for seq, body in rows:
            self.seq = max(self.seq, seq)
            try:
                event = json.loads(body)
            except ValueError as error:
                raise ProbeError(f"{self.database}: event {seq} is not JSON") from error
            kind = event.get("kind") or {}
            if kind.get("type") == "agent.usage_updated" and isinstance(kind.get("total"), dict):
                self.totals[event.get("agent_id")] = kind["total"]
        try:
            return sum(_priced(normalize_usage(total, "aether"), self.price) for total in self.totals.values())
        except UsageError as error:
            raise ProbeError(f"Aether usage: {error}") from error

    def total_tokens(self):
        return sum(normalize_usage(total, "aether")["total_tokens"] or 0 for total in self.totals.values())

    def output_tokens(self):
        return sum(normalize_usage(total, "aether")["output_tokens"] or 0 for total in self.totals.values())


def marked_pids(value):
    """Processes of this user whose environment carries the trial marker."""
    needle = f"{MARKER}={value}".encode()
    found = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit() or int(entry) == os.getpid():
            continue
        try:
            with open(f"/proc/{entry}/environ", "rb") as stream:
                environ = stream.read().split(b"\0")
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue  # The process ended, or belongs to someone else.
        if needle in environ:
            found.append(int(entry))
    return found


def _signal_all(pids, signal_number):
    for pid in pids:
        try:
            os.kill(pid, signal_number)
        except ProcessLookupError:
            continue  # It exited between the scan and the signal.


def stop_marked(value):
    """SIGTERM every marked process, then SIGKILL whatever carries the marker after a grace period."""
    assert value and "\0" not in value, "a marker value is non-empty text"
    first = marked_pids(value)
    _signal_all(first, signal.SIGTERM)
    deadline = time.monotonic() + STOP_GRACE_SECONDS
    while time.monotonic() < deadline and marked_pids(value):
        time.sleep(0.2)
    # A second scan also catches children started after the first one.
    survivors = marked_pids(value)
    _signal_all(survivors, signal.SIGKILL)
    return sorted(set(first) | set(survivors))


class SpendGuard:
    """A thread that stops a trial's processes once `probe()` reaches `allowance_usd`.

    `probe()` returns the spend so far, or None while no usage source can be read;
    unknown spend, like a ProbeError, is tolerated for `unobservable_seconds`, then
    the trial is stopped and `unobserved` set. `tripped` holds why it stopped them,
    or None; `peak_usd` the highest spend it saw."""

    def __init__(self, probe, allowance_usd, marker, poll_seconds=POLL_SECONDS, stop=stop_marked,
                 unobservable_seconds=UNOBSERVABLE_SECONDS):
        assert allowance_usd > 0 and poll_seconds > 0, "a guard needs a positive allowance and poll interval"
        assert unobservable_seconds > 0, "unknown spend is tolerated for a bounded, positive time"
        self.probe, self.allowance_usd, self.marker = probe, allowance_usd, marker
        self.poll_seconds, self.stop, self.unobservable_seconds = poll_seconds, stop, unobservable_seconds
        self.tripped, self.unobserved, self.peak_usd = None, False, 0.0
        self._done = threading.Event()
        self._thread = threading.Thread(target=self._watch, name=f"spend-guard {marker}", daemon=True)

    def __enter__(self):
        self._thread.start()
        return self

    def __exit__(self, *exc):
        self._done.set()
        self._thread.join()
        return False

    def _watch(self):
        unknown_since, why = None, "no usage source has appeared"
        while not self._done.wait(self.poll_seconds):
            try:
                spent = self.probe()
            except ProbeError as caught:
                spent, why = None, str(caught)
            except Exception as caught:  # noqa: BLE001 - an unguarded trial must not keep running
                self._trip(f"spend guard failed: {type(caught).__name__}: {caught}")
                return
            if spent is None:
                unknown_since = unknown_since or time.monotonic()
                if time.monotonic() - unknown_since >= self.unobservable_seconds:
                    self.unobserved = True
                    self._trip(f"spend could not be observed for {self.unobservable_seconds}s: {why}")
                    return
                continue
            assert spent >= 0, f"a probe reported negative spend {spent}"
            unknown_since, why = None, "no usage source has appeared"
            self.peak_usd = max(self.peak_usd, spent)
            if self.peak_usd >= self.allowance_usd:
                self._trip(f"spend ${self.peak_usd:.3f} reached the trial allowance ${self.allowance_usd:.2f}")
                return

    def _trip(self, reason):
        self.tripped = reason
        self.stop(self.marker)
