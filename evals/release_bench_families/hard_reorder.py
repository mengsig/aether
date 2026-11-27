"""hard / reorder: a per-source in-order delivery buffer whose docs fix duplicates, gaps, windows and snapshots.

Each seed picks the first sequence number rule, what an exceeded window does
and a window size, then plants three or four defects the visible tests skip.
"""
from .common import fill, make_spec, package_name, reference_block

CODE = '''
"""Per-source in-order delivery of an event stream (rules: docs/ordering.md)."""
import copy

FIRST = @@FIRST@@
WINDOW = @@WINDOW@@
@@ERROR_CLASS@@

def _checked(event):
    """(source, seq) of a well-formed event; anything else is a ValueError."""
    if not isinstance(event, dict) or {"source", "seq", "payload"} - event.keys():
        raise ValueError(f"an event needs source, seq and payload: {event!r}")
    source, seq = event["source"], event["seq"]
    if not isinstance(source, str) or type(seq) is not int:
        raise ValueError(f"source must be a string and seq an int: {event!r}")
    if FIRST is not None and seq < FIRST:
        raise ValueError(f"seq {seq} is below the first seq {FIRST}")
    return source, seq


class OrderedBuffer:
    def __init__(self):
        self._next = {}
        self._held = {}
        self.missing = []

    def push(self, events):
        """Accept events in the given order; return the events that became deliverable, in delivery order."""
        next_seq, held, missing = @@WORKING_STATE@@
        delivered = []
        for event in events:
            self._accept(event, next_seq, held, missing, delivered)
        self._next, self._held, self.missing = next_seq, held, missing
        return delivered

    def _copy_state(self):
        return dict(self._next), {s: dict(h) for s, h in self._held.items()}, list(self.missing)

    @staticmethod
    def _accept(event, next_seq, held, missing, delivered):
        source, seq = _checked(event)
        if source not in next_seq:
            next_seq[source] = @@START@@
            held[source] = {}
        expected, pending = next_seq[source], held[source]
@@STALE@@
        if seq - expected @@LIMIT@@ WINDOW:
@@OVERFLOW@@
        pending[seq] = event
        while expected in pending:
            delivered.append(pending.pop(expected))
            expected += 1
        next_seq[source] = expected

    def snapshot(self):
        """JSON-compatible state that `restore` turns back into an equivalent buffer."""
        return {"next": @@SNAP_NEXT@@,
                "held": @@SNAP_HELD@@,
                "missing": [list(item) for item in self.missing]}

    @classmethod
    def restore(cls, snapshot):
        buffer = cls()
        buffer._next = @@RESTORE_NEXT@@
        buffer._held = @@RESTORE_HELD@@
        buffer.missing = [list(item) for item in snapshot["missing"]]
        assert set(buffer._next) == set(buffer._held), "a snapshot holds the same sources in next and held"
        return buffer
'''

ERROR_CLASS = '''

class GapError(ValueError):
    """A source's window was exceeded: `source` and the `missing` seqs, ascending."""

    def __init__(self, source, missing):
        super().__init__(f"{source}: seqs {missing} are missing and the window of {WINDOW} is exceeded")
        self.source, self.missing = source, missing

'''
STALE = {
    "good": "        if seq < expected or seq in pending:\n            return",
    "stale": "        if seq in pending or any(e[\"source\"] == source and e[\"seq\"] == seq for e in delivered):\n"
             "            return\n        if seq < expected:\n            delivered.append(event)\n            return",
}
OVERFLOW = {
    "raise": "            raise GapError(source, [n for n in range(expected, seq) if n not in pending])",
    "skip": "            target = seq - WINDOW + 1\n            for number in range(expected, target):\n"
            "                if number in pending:\n                    delivered.append(pending.pop(number))\n"
            "                else:\n                    missing.append([source, number])\n            expected = target",
    "skipdrop": "            target = seq - WINDOW + 1\n            for number in range(expected, target):\n"
                "                if number not in pending:\n                    missing.append([source, number])\n"
                "                pending.pop(number, None)\n            expected = target",
}
SNAPSHOT = {
    "good": ("dict(self._next)", "{s: [copy.deepcopy(h[n]) for n in sorted(h)] for s, h in self._held.items()}",
             'dict(snapshot["next"])',
             '{s: {e["seq"]: copy.deepcopy(e) for e in events} for s, events in snapshot["held"].items()}'),
    "lossy": ("dict(self._next)", "{s: [] for s in self._held}", 'dict(snapshot["next"])',
              '{s: {} for s in snapshot["held"]}'),
    "intkeys": ("dict(self._next)", "{s: dict(h) for s, h in self._held.items()}", 'dict(snapshot["next"])',
                '{s: dict(h) for s, h in snapshot["held"].items()}'),
    "alias": ("self._next", "{s: [copy.deepcopy(h[n]) for n in sorted(h)] for s, h in self._held.items()}",
              'snapshot["next"]',
              '{s: {e["seq"]: copy.deepcopy(e) for e in events} for s, events in snapshot["held"].items()}'),
}

DOC = '''
# Ordered delivery

`OrderedBuffer` (in `@@PKG@@/buffer.py`) puts an event stream back in order, one
source at a time. An event is a dict `{"source": str, "seq": int, "payload": any}`;
payloads are JSON values. `push(events)` takes any iterable of events and returns
the list of events that became deliverable, in the order they are delivered.

1. **Order per source.** A source's events are delivered strictly by consecutive
   `seq`, each exactly once. An event that arrives ahead of its turn is held until
   every `seq` before it has been delivered, or given up (rule 5).
2. **Delivery order of a call.** Events are processed one by one in the order given.
   Each processed event delivers its source's run of consecutive held events, so
   the result lists events in the order they became deliverable (not sorted by
   source or `seq`).
3. **Duplicates.** An event whose `seq` was already delivered, or is already held,
   is dropped, in the same call or in any later one. For a held `seq` the
   first payload wins. Sources never interfere: `("a", 3)` and `("b", 3)` are
   different events.
4. **First seq.** @@FIRST_RULE@@
5. **Window.** An event whose `seq` is @@WINDOW@@ or more ahead of its source's next
   expected `seq` (`seq - next >= @@WINDOW@@`) exceeds the window. @@OVERFLOW_RULE@@
6. **Rejected input.** A malformed event (not a dict with `source`, `seq`, `payload`;
   `source` not a `str`; `seq` not an `int`, `bool` included@@BELOW_FIRST@@) raises `ValueError`.
   A call that raises changes nothing: events it processed before the error are
   neither delivered nor kept, and the buffer is exactly as before the call.
7. **Snapshots.** `snapshot()` returns JSON-compatible data (`json.dumps` must accept it)
   holding everything needed to continue@@SNAP_EXTRA@@, and `OrderedBuffer.restore(snapshot)`
   returns a buffer that behaves exactly like the original from then on: nothing buffered
   is lost and nothing delivered is delivered again, also after the snapshot went through
   `json.dumps`/`json.loads`. A snapshot never shares mutable state with a buffer, and
   neither does a restored buffer with the snapshot it came from.
'''
FIRST_RULES = {
    0: "Every source starts at `seq` 0; `seq` 0 must arrive (or the window be exceeded) before 1 is delivered.",
    1: "Every source starts at `seq` 1; `seq` 1 must arrive (or the window be exceeded) before 2 is delivered.",
    None: "A source has no fixed start: its first event seen is delivered at once and later events follow from it. "
          "Older `seq` values from then on are late duplicates and are dropped. Sources start independently.",
}
OVERFLOW_RULES = {
    "raise": ("The call raises `GapError` (a `ValueError`, defined in `buffer.py`) with `.source` and `.missing`, the "
              "ascending `seq` values from the next expected up to one before the event that are not held. "
              "Rule 6 applies: the whole call is rolled back."),
    "skip": ("The buffer gives up on the oldest gaps until the event is inside the window: it advances the next "
             "expected `seq` one by one up to `seq - @@WINDOW@@ + 1`, delivering held events it passes and recording "
             "each `seq` that was not held as `[source, seq]` at the end of `buffer.missing` (a list, empty "
             "at the start, kept in snapshots). The event is then accepted normally. A skipped `seq` that "
             "arrives later is a late duplicate."),
}

VISIBLE_TESTS = '''
import json
import unittest

from @@PKG@@.buffer import OrderedBuffer

B = @@BASE@@


def ev(source, seq):
    return {"source": source, "seq": seq, "payload": f"{source}{seq}"}


def seqs(events):
    return [e["seq"] for e in events]


class BufferTests(unittest.TestCase):
    def test_in_order_events_pass_through(self):
        self.assertEqual(seqs(OrderedBuffer().push([ev("a", B), ev("a", B + 1)])), [B, B + 1])

    def test_early_event_waits_for_the_gap(self):
        buffer = OrderedBuffer()
        self.assertEqual(seqs(buffer.push([ev("a", B)])), [B])
        self.assertEqual(seqs(buffer.push([ev("a", B + 2)])), [])
        self.assertEqual(seqs(buffer.push([ev("a", B + 1)])), [B + 1, B + 2])

    def test_duplicate_in_one_call(self):
        self.assertEqual(seqs(OrderedBuffer().push([ev("a", B), ev("a", B)])), [B])

    def test_snapshot_after_delivery_restores(self):
        buffer = OrderedBuffer()
        buffer.push([ev("a", B), ev("a", B + 1)])
        again = OrderedBuffer.restore(json.loads(json.dumps(buffer.snapshot())))
        self.assertEqual(seqs(again.push([ev("a", B + 2)])), [B + 2])


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import json
import random
import unittest

from @@PKG@@.buffer import @@IMPORTS@@

@@REFERENCE@@
B = @@BASE@@
F = @@FIRST@@
W = @@WINDOW@@


def ev(source, seq, payload=None):
    return {"source": source, "seq": seq, "payload": payload if payload is not None else f"{source}{seq}"}


def keys(events):
    return [(e["source"], e["seq"]) for e in events]


def primed(*sources):
    buffer = OrderedBuffer()
    buffer.push([ev(source, B) for source in sources])
    return buffer


def state(buffer):
    return json.dumps(buffer.snapshot(), sort_keys=True)


class HiddenOrderChecks(unittest.TestCase):
    def test_events_are_held_until_the_gap_fills(self):
        buffer = primed("a")
        self.assertEqual(keys(buffer.push([ev("a", B + 2), ev("a", B + 3)])), [])
        self.assertEqual(keys(buffer.push([ev("a", B + 1)])), [("a", B + n) for n in (1, 2, 3)])
        self.assertEqual(keys(buffer.push([ev("a", B + 5), ev("a", B + 4)])), [("a", B + 4), ("a", B + 5)])

    def test_duplicates_are_dropped_across_calls(self):
        buffer = primed("a")
        self.assertEqual(keys(buffer.push([ev("a", B)])), [])
        self.assertEqual(buffer.push([ev("a", B + 2, "first"), ev("a", B + 2, "second")]), [])
        self.assertEqual(buffer.push([ev("a", B + 2, "third")]), [])
        out = buffer.push([ev("a", B + 1), ev("a", B + 1)])
        self.assertEqual([e["payload"] for e in out], [f"a{B + 1}", "first"])
        self.assertEqual(buffer.push([ev("a", B + 1), ev("a", B + 2)]), [])

    def test_first_seq_rule(self):
@@FIRST_CHECK@@

    def test_window_limit_and_overflow(self):
@@OVERFLOW_CHECK@@

    def test_rejected_calls_change_nothing(self):
@@REJECT_CHECK@@

    def malformed_calls_change_nothing(self):
        buffer = primed("a")
        before = state(buffer)
        bad = [{"source": "a", "seq": "7", "payload": 1}, {"source": "a", "seq": True, "payload": 1},
               {"source": 3, "seq": B + 1, "payload": 1}, {"source": "a", "seq": B + 1}, "a"]
        for event in bad:
            with self.assertRaises(ValueError, msg=repr(event)):
                buffer.push([ev("a", B + 1), ev("a", B + 3), event])
            self.assertEqual(state(buffer), before)

    def test_snapshot_survives_json_midstream(self):
        buffer = primed("a", "b")
        buffer.push([ev("a", B + 2), ev("b", B + 3), ev("a", B + 4)])
        twin = OrderedBuffer.restore(json.loads(json.dumps(buffer.snapshot())))
        self.assertEqual(state(twin), state(buffer))
        rest = [ev("a", B + 1), ev("b", B + 1), ev("b", B + 2), ev("a", B + 3), ev("a", B + 2)]
        expected = [("a", B + 1), ("a", B + 2), ("b", B + 1), ("b", B + 2), ("b", B + 3), ("a", B + 3), ("a", B + 4)]
        self.assertEqual(keys(buffer.push(rest)), expected)
        self.assertEqual(keys(twin.push(rest)), expected)
        self.assertEqual(twin.push([ev("a", B), ev("b", B + 3)]), [])

    def test_snapshots_and_restored_buffers_share_nothing(self):
        buffer = primed("a")
        buffer.push([ev("a", B + 2)])
        snapshot = buffer.snapshot()
        frozen = json.dumps(snapshot, sort_keys=True)
        buffer.push([ev("a", B + 1), ev("a", B + 3)])
        self.assertEqual(json.dumps(snapshot, sort_keys=True), frozen)
        twin = OrderedBuffer.restore(snapshot)
        self.assertEqual(keys(twin.push([ev("a", B + 1)])), [("a", B + 1), ("a", B + 2)])
        self.assertEqual(json.dumps(snapshot, sort_keys=True), frozen)
        self.assertEqual(keys(OrderedBuffer.restore(snapshot).push([ev("a", B + 1)])), [("a", B + 1), ("a", B + 2)])

    def test_sources_are_independent(self):
        buffer = primed("a", "b")
        buffer.push([ev("a", B + 3)])
        self.assertEqual(keys(buffer.push([ev("b", B + 1), ev("b", B + 2)])), [("b", B + 1), ("b", B + 2)])
        fresh = OrderedBuffer()
        self.assertEqual(keys(fresh.push([ev("c", B), ev("d", B), ev("c", B + 1)])),
                         [("c", B), ("d", B), ("c", B + 1)])

    def test_random_streams_match_the_reference(self):
        rng = random.Random(@@SEED@@)
        reference = load_reference(REFERENCE_SOURCE)["OrderedBuffer"]
        for _ in range(60):
            mine, theirs = OrderedBuffer(), reference()
            for _ in range(rng.randint(3, 12)):
                batch = [ev(rng.choice("abc"), B + rng.randint(0, 11), rng.randint(0, 9)) for _ in range(rng.randint(1, 4))]
                self.assertEqual(outcome(mine.push, batch), outcome(theirs.push, batch), batch)
                if rng.random() < 0.3:
                    mine = OrderedBuffer.restore(json.loads(json.dumps(mine.snapshot())))
                    theirs = reference.restore(json.loads(json.dumps(theirs.snapshot())))
            self.assertEqual(mine.missing, theirs.missing)
'''

FIRST_CHECKS = {
    "fixed": '''        buffer = OrderedBuffer()
        self.assertEqual(keys(buffer.push([ev("a", F + 1), ev("a", F + 2)])), [])
        self.assertEqual(keys(buffer.push([ev("a", F), ev("b", F)])), [("a", F), ("a", F + 1), ("a", F + 2), ("b", F)])
        with self.assertRaises(ValueError):
            OrderedBuffer().push([ev("a", F - 1)])''',
    "seen": '''        buffer = OrderedBuffer()
        self.assertEqual(keys(buffer.push([ev("a", 40), ev("a", 42), ev("a", 41)])), [("a", n) for n in (40, 41, 42)])
        self.assertEqual(buffer.push([ev("a", 39), ev("a", 41)]), [])
        self.assertEqual(keys(buffer.push([ev("b", -3), ev("b", -2)])), [("b", -3), ("b", -2)])''',
}
OVERFLOW_CHECKS = {
    "raise": '''        buffer = primed("a")
        self.assertEqual(buffer.push([ev("a", B + W)]), [])
        with self.assertRaises(GapError) as caught:
            buffer.push([ev("a", B + W + 1)])
        self.assertEqual(caught.exception.source, "a")
        self.assertEqual(caught.exception.missing, list(range(B + 1, B + W)))
        self.assertIsInstance(caught.exception, ValueError)
        self.assertEqual(keys(buffer.push([ev("a", n) for n in range(B + 1, B + W)])),
                         [("a", n) for n in range(B + 1, B + W + 1)])''',
    "skip": '''        buffer = primed("a")
        self.assertEqual(buffer.push([ev("a", B + W)]), [])
        self.assertEqual(buffer.missing, [])
        self.assertEqual(buffer.push([ev("a", B + W + 1)]), [])
        self.assertEqual(buffer.missing, [["a", B + 1]])
        self.assertEqual(keys(buffer.push([ev("a", n) for n in range(B + 2, B + W)])),
                         [("a", n) for n in range(B + 2, B + W + 2)])
        self.assertEqual(buffer.push([ev("a", B + 1)]), [])
        other = primed("a")
        other.push([ev("a", B + 2)])
        self.assertEqual(keys(other.push([ev("a", B + W + 2)])), [("a", B + 2)])
        self.assertEqual(other.missing, [["a", B + 1]])
        self.assertEqual(keys(other.push([ev("a", B + 1)])), [])''',
}
REJECT_CHECKS = {
    "raise": '''        buffer = primed("a", "b")
        buffer.push([ev("a", B + 2)])
        before = state(buffer)
        with self.assertRaises(GapError):
            buffer.push([ev("a", B + 1), ev("b", B + 1), ev("a", B + W + 3)])
        self.assertEqual(state(buffer), before)
        self.assertEqual(keys(buffer.push([ev("a", B + 1)])), [("a", B + 1), ("a", B + 2)])
        self.malformed_calls_change_nothing()''',
    "skip": "        self.malformed_calls_change_nothing()",
}

PROMPTS = (
    "The event buffer in `@@PKG@@/buffer.py` passes its tests but misorders, re-delivers or loses events in "
    "production. Make it follow `docs/ordering.md` exactly.",
    "`@@PKG@@/buffer.py` should deliver each source's events in order as `docs/ordering.md` specifies, including "
    "duplicates, the window, the first seq and snapshots. Fix the buffer.",
    "Consumers of `OrderedBuffer` see gaps, repeats and lost events after a restart. Audit `@@PKG@@/buffer.py` "
    "against `docs/ordering.md` and fix every deviation.",
)


def _defects(rng, first, overflow):
    pool = ["stale", "snapshot", "limit", "atomic"]
    pool += ["overflow"] if overflow == "skip" else []
    pool += ["start"] if first is not None else []
    return set(rng.sample(pool, rng.randint(3, min(4, len(pool)))))


def _source(values, defects, snapshot_kind, overflow):
    stale = STALE["stale" if "stale" in defects else "good"]
    if overflow == "skip":
        action = OVERFLOW["skipdrop" if "overflow" in defects else "skip"]
    else:
        action = OVERFLOW["raise"]
    snap_next, snap_held, rest_next, rest_held = SNAPSHOT[snapshot_kind if "snapshot" in defects else "good"]
    working = "self._next, self._held, self.missing" if "atomic" in defects else "self._copy_state()"
    start = "seq" if "start" in defects else "seq if FIRST is None else FIRST"
    return fill(CODE, **values, WORKING_STATE=working, START=start, STALE=stale, LIMIT=">" if "limit" in defects else ">=", OVERFLOW=action,
                SNAP_NEXT=snap_next, SNAP_HELD=snap_held, RESTORE_NEXT=rest_next, RESTORE_HELD=rest_held)


def generate(seed, rng):
    pkg = package_name(rng, "stream")
    first = rng.choice([0, 1, None])
    overflow = rng.choice(["raise", "skip"])
    window = rng.choice([4, 5, 6])
    base = 40 if first is None else first
    defects = _defects(rng, first, overflow)
    kind = rng.choice(["lossy", "intkeys", "alias"])
    values = {"FIRST": repr(first), "WINDOW": window,
              "ERROR_CLASS": ERROR_CLASS if overflow == "raise" else ""}
    solution = _source(values, set(), kind, overflow)
    starter = _source(values, defects, kind, overflow)
    doc = fill(DOC, PKG=pkg, WINDOW=window, FIRST_RULE=FIRST_RULES[first],
               OVERFLOW_RULE=fill(OVERFLOW_RULES[overflow], dedent=False, allow_unused=True, WINDOW=window),
               BELOW_FIRST="" if first is None else f"; `seq` below {first}",
               SNAP_EXTRA=", including the `missing` list" if overflow == "skip" else "")
    hidden = fill(HIDDEN, PKG=pkg, IMPORTS="GapError, OrderedBuffer" if overflow == "raise" else "OrderedBuffer",
                  REFERENCE=reference_block(solution), BASE=base, FIRST=repr(first), WINDOW=window, SEED=seed,
                  FIRST_CHECK=FIRST_CHECKS["seen" if first is None else "fixed"],
                  OVERFLOW_CHECK=OVERFLOW_CHECKS[overflow], REJECT_CHECK=REJECT_CHECKS[overflow])
    files = {"README.md": f"# {pkg}\n\nOrdered delivery of event streams. Rules: `docs/ordering.md`.\n",
             f"{pkg}/__init__.py": "", f"{pkg}/buffer.py": starter, "tests/__init__.py": "",
             "tests/test_buffer.py": fill(VISIBLE_TESTS, PKG=pkg, BASE=base), "docs/ordering.md": doc}
    return make_spec(family="ordering", template="reorder", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/buffer.py": solution},
                     meta={"first": "seen" if first is None else first, "overflow": overflow, "defects": len(defects)})
