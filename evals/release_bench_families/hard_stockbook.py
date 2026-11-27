"""hard / stockbook: add a feature to a four-module inventory package without breaking its stated invariants.

A seed picks the feature (cancel plus expiry, or partial fulfilment), the SKU
names and one or two latent defects of the existing code. Hidden checks run
scripted cases and random operation sequences, also against a reference.
"""
from .common import fill, make_spec, package_name

MODELS = '''
"""Stock state shared by the service, the event replay and the report (docs/inventory.md)."""


class StockError(ValueError):
    """A rejected call; the inventory is exactly as it was before it."""


STATUSES = @@STATUSES@@


class State:
    def __init__(self):
        self.items = {}
        self.reservations = {}
        self.next_id = 1

    def available(self, sku):
        item = self.items.get(sku)
        return 0 if item is None else item["on_hand"] - item["reserved"]

    def snapshot(self):
        return {"items": {sku: dict(item) for sku, item in self.items.items()},
                "reservations": {rid: dict(res) for rid, res in self.reservations.items()},
                "next_id": self.next_id}
'''
EVENTS = '''
"""Event log entries: applying one to a State, and rebuilding a State from a log."""
from .models import State


def _received(state, event):
    item = state.items.setdefault(event["sku"], {"on_hand": 0, "reserved": 0})
    item["on_hand"] += event["qty"]


def _reserved(state, event):
    state.items[event["sku"]]["reserved"] += event["qty"]
    state.reservations[event["id"]] = {"sku": event["sku"], "qty": event["qty"], "shipped": 0,
                                       "status": "open", "expires_at": event["expires_at"]}
    state.next_id = event["id"] + 1


def _written_off(state, event):
    state.items[event["sku"]]["on_hand"] -= event["qty"]

@@FULFILLED@@
@@RELEASED@@
HANDLERS = {"received": _received, "reserved": _reserved, "written_off": _written_off, "fulfilled": _fulfilled@@MORE_HANDLERS@@}


def apply(state, event):
    handler = HANDLERS.get(event.get("type"))
    if handler is None:
        raise ValueError(f"unknown event type: {event!r}")
    handler(state, event)


def replay(events):
    """The State that applying `events` in order produces; their `n` must run 1, 2, 3, ..."""
    state = State()
    for expected, event in enumerate(events, start=1):
        if event.get("n") != expected:
            raise ValueError(f"event {expected} is out of sequence: {event!r}")
        apply(state, event)
    return state
'''
FULFILLED = {
    "good": '''
def _fulfilled(state, event):
    reservation = state.reservations[event["id"]]
    item = state.items[reservation["sku"]]
    item["on_hand"] -= event["qty"]
    item["reserved"] -= event["qty"]
    reservation["shipped"] += event["qty"]
    if reservation["shipped"] == reservation["qty"]:
        reservation["status"] = "fulfilled"
''',
    "whole": '''
def _fulfilled(state, event):
    reservation = state.reservations[event["id"]]
    item = state.items[reservation["sku"]]
    item["on_hand"] -= reservation["qty"]
    item["reserved"] -= reservation["qty"]
    reservation["shipped"] = reservation["qty"]
    reservation["status"] = "fulfilled"
''',
}
RELEASED = '''
def _released(status):
    def handler(state, event):
        reservation = state.reservations[event["id"]]
        state.items[reservation["sku"]]["reserved"] -= reservation["qty"] - reservation["shipped"]
        reservation["status"] = status
    return handler

'''
MORE_HANDLERS = ', "cancelled": _released("cancelled"), "expired": _released("expired")'

SERVICE = '''
"""The inventory service: validates a call, then records exactly one event per change."""
from .events import apply
from .models import State, StockError


def _name(value, what):
    if not isinstance(value, str) or not value:
        raise StockError(f"{what} must be a non-empty string: {value!r}")


def _positive_int(value, what):
    if type(value) is not int or value <= 0:
        raise StockError(f"{what} must be a positive int: {value!r}")


class Inventory:
    def __init__(self):
        self.state = State()
        self._log = []

    @property
    def events(self):
        return [dict(event) for event in self._log]

    def snapshot(self):
        return self.state.snapshot()

    def _record(self, kind, **fields):
        event = {"n": len(self._log) + 1, "type": kind, **fields}
        apply(self.state, event)
        self._log.append(event)

    def _open_reservation(self, reservation_id):
        reservation = self.state.reservations.get(reservation_id) if type(reservation_id) is int else None
        if reservation is None or reservation["status"] != "open":
            raise StockError(f"no open reservation {reservation_id!r}")
        return reservation

    def receive(self, sku, qty):
        _name(sku, "sku")
        _positive_int(qty, "qty")
        self._record("received", sku=sku, qty=qty)

    def reserve(self, sku, qty, expires_at=None):
        _name(sku, "sku")
        _positive_int(qty, "qty")
        if expires_at is not None and type(expires_at) is not int:
            raise StockError(f"expires_at must be an int or None: {expires_at!r}")
        if qty > self.state.available(sku):
            raise StockError(f"only {self.state.available(sku)} of {sku!r} available")
        reservation_id = self.state.next_id
        self._record("reserved", id=reservation_id, sku=sku, qty=qty, expires_at=expires_at)
        return reservation_id

    def write_off(self, sku, qty):
        _name(sku, "sku")
        _positive_int(qty, "qty")
        if qty > self.state.available(sku):
            raise StockError(f"only {self.state.available(sku)} of {sku!r} available to write off")
        self._record("written_off", sku=sku, qty=qty)
@@FULFIL@@@@FEATURE@@'''
FULFIL = {
    "whole": '''
    def fulfil(self, reservation_id):
        reservation = self._open_reservation(reservation_id)
        self._record("fulfilled", id=reservation_id, qty=reservation["qty"] - reservation["shipped"])
''',
    "partial": '''
    def fulfil(self, reservation_id, qty=None):
        reservation = self._open_reservation(reservation_id)
        remaining = reservation["qty"] - reservation["shipped"]
        qty = remaining if qty is None else qty
        _positive_int(qty, "qty")
        if qty > remaining:
            raise StockError(f"only {remaining} left to fulfil on reservation {reservation_id}")
        self._record("fulfilled", id=reservation_id, qty=qty)
''',
}
FEATURE = {
    "cancel": '''
    def cancel(self, reservation_id):
        self._open_reservation(reservation_id)
        self._record("cancelled", id=reservation_id)

    def expire(self, now):
        if type(now) is not int:
            raise StockError(f"now must be an int: {now!r}")
        due = sorted(rid for rid, res in self.state.reservations.items()
                     if res["status"] == "open" and res["expires_at"] is not None and res["expires_at"] <= now)
        for reservation_id in due:
            self._record("expired", id=reservation_id)
        return due
''',
    "partial": "",
}
REPORT = '''
"""Stock reports (docs/inventory.md)."""
from .models import STATUSES


def summary(inventory):
    """Per-item rows, totals over those rows and reservation counts by status."""
    state = inventory.state
    items = {}
    for sku in sorted(state.items):
        item = state.items[sku]
        opened = sum(1 for res in state.reservations.values() if res["sku"] == sku and res["status"] == "open")
        items[sku] = {"on_hand": item["on_hand"], "reserved": item["reserved"],
                      "available": item["on_hand"] - item["reserved"], "open_reservations": opened}
    totals = {key: sum(row[key] for row in items.values()) for key in ("on_hand", "reserved", "available", "open_reservations")}
    by_status = {status: 0 for status in STATUSES}
    for reservation in state.reservations.values():
        by_status[reservation["status"]] += 1
    return {"items": items, "totals": totals, "reservations": by_status}
'''
STARTER_REPORT = REPORT.replace(
    "from .models import STATUSES\n", "").replace(
    "{status: 0 for status in STATUSES}", '{"open": 0, "fulfilled": 0}').replace(
    'res["sku"] == sku and res["status"] == "open")', '@@OPEN_FILTER@@)')
OPEN_FILTER = {"good": 'res["sku"] == sku and res["status"] == "open"', "open_count": 'res["sku"] == sku'}

STARTER_SERVICE = '''
"""The inventory service: validates a call, mutates the state and logs the change."""
from .models import State, StockError


def _name(value, what):
    if not isinstance(value, str) or not value:
        raise StockError(f"{what} must be a non-empty string: {value!r}")


def _positive_int(value, what):
    if type(value) is not int or value <= 0:
        raise StockError(f"{what} must be a positive int: {value!r}")


class Inventory:
    def __init__(self):
        self.state = State()
        self._log = []

    @property
    def events(self):
        return [dict(event) for event in self._log]

    def snapshot(self):
        return self.state.snapshot()

    def _log_event(self, kind, **fields):
        self._log.append({"n": len(self._log) + 1, "type": kind, **fields})

    def _open_reservation(self, reservation_id):
        reservation = self.state.reservations.get(reservation_id) if type(reservation_id) is int else None
        if reservation is None or reservation["status"] != "open":
            raise StockError(f"no open reservation {reservation_id!r}")
        return reservation

    def receive(self, sku, qty):
        _name(sku, "sku")
        _positive_int(qty, "qty")
        item = self.state.items.setdefault(sku, {"on_hand": 0, "reserved": 0})
        item["on_hand"] += qty
        self._log_event("received", sku=sku, qty=qty)

    def reserve(self, sku, qty, expires_at=None):
        _name(sku, "sku")
        _positive_int(qty, "qty")
        if expires_at is not None and type(expires_at) is not int:
            raise StockError(f"expires_at must be an int or None: {expires_at!r}")
        reservation_id = self.state.next_id
@@RESERVE@@
        self.state.items[sku]["reserved"] += qty
        self.state.reservations[reservation_id] = {"sku": sku, "qty": qty, "shipped": 0, "status": "open",
                                                   "expires_at": expires_at}
        self.state.next_id = reservation_id + 1
        self._log_event("reserved", id=reservation_id, sku=sku, qty=qty, expires_at=expires_at)
        return reservation_id

    def write_off(self, sku, qty):
        _name(sku, "sku")
        _positive_int(qty, "qty")
        item = self.state.items.get(sku)
        if item is None or qty > @@WRITE_OFF_LIMIT@@:
            raise StockError(f"not enough {sku!r} to write off")
        item["on_hand"] -= qty
        self._log_event("written_off", sku=sku, qty=qty)

    def fulfil(self, reservation_id):
        reservation = self._open_reservation(reservation_id)
        item = self.state.items[reservation["sku"]]
        item["on_hand"] -= reservation["qty"]
        item["reserved"] -= reservation["qty"]
        reservation["shipped"] = reservation["qty"]
        reservation["status"] = "fulfilled"
        self._log_event("fulfilled", id=reservation_id, qty=reservation["qty"])
'''
RESERVE = {
    "good": '        if qty > self.state.available(sku):\n            raise StockError(f"only {self.state.available(sku)} of {sku!r} available")',
    "burn": '        self.state.next_id += 1\n        if qty > self.state.available(sku):\n'
            '            raise StockError(f"only {self.state.available(sku)} of {sku!r} available")',
}
WRITE_OFF_LIMIT = {"good": 'item["on_hand"] - item["reserved"]', "writeoff": 'item["on_hand"]'}

DOC = '''
# Inventory rules

`Inventory` (`@@PKG@@/service.py`) tracks stock per SKU and reservations against it. All
state lives in `inventory.state` (`@@PKG@@/models.py`); `@@PKG@@/events.py` rebuilds it from the
event log and `@@PKG@@/report.py` summarises it.

## Calls

Every call validates first. A call that raises `models.StockError` (a `ValueError`) changes
nothing: no state, no event, no reservation id is used up. Quantities, ids and `now` are ints;
`bool` is not accepted; quantities are positive. SKUs are non-empty strings.

- `receive(sku, qty)`: adds `qty` to `on_hand` (the SKU is created on first receipt).
- `reserve(sku, qty, expires_at=None)`: needs `qty <= available`; returns the new reservation
  id (1, 2, 3, ...). `expires_at` is None or an int.
- `write_off(sku, qty)`: removes stock for good; it may only take stock that is `available`.
- `fulfil(reservation_id@@FULFIL_ARG@@)`: @@FULFIL_RULE@@
@@FEATURE_DOC@@
## Invariants

1. `available = on_hand - reserved` is never negative, and `on_hand` is never negative.
2. `reserved` of an SKU equals the quantity still to be shipped on its open reservations.
3. Every call that changes state appends exactly one event to the log, `inventory.events`
   (a list of dict copies); a call that changes nothing appends none. Events carry `n` (1, 2,
   3, ... without gaps) and `type`, plus:

   | type | fields |
   |---|---|
   | `received` | `sku`, `qty` |
   | `reserved` | `id`, `sku`, `qty`, `expires_at` |
   | `written_off` | `sku`, `qty` |
   | `fulfilled` | `id`, `qty` (the quantity shipped by this call) |
@@EVENT_ROWS@@
4. `events.replay(events)` returns a `State` with the same `snapshot()` as the inventory that
   wrote the events, for the whole log and for every prefix of it. It raises `ValueError` for an
   unknown type or an `n` that is out of sequence.
5. `inventory.snapshot()` is `{"items": {sku: {"on_hand", "reserved"}}, "reservations": {id:
   {"sku", "qty", "shipped", "status", "expires_at"}}, "next_id"}`; `qty` is the reserved
   quantity and `shipped` how much of it has gone out.
6. `report.summary(inventory)` returns `{"items": ..., "totals": ..., "reservations": ...}`:
   `items` maps each SKU in sorted order to `{"on_hand", "reserved", "available",
   "open_reservations"}` (the number of open reservations on it); `totals` holds the same four
   keys, each the sum over `items`; `reservations` counts reservations by status, with a key
   for every status (@@STATUS_LIST@@), also when its count is 0.
'''
FULFIL_ARGS = {"whole": ("", "ships everything still to ship on an open reservation: `on_hand` and `reserved` both drop by it, and the reservation becomes `fulfilled`."),
               "partial": (", qty=None",
                           "ships `qty` of what is still to ship on an open reservation (None: all of it). "
                           "`on_hand` and `reserved` both drop by `qty` and the reservation's `shipped` grows by it. "
                           "The reservation stays `open` until everything is shipped, then becomes `fulfilled`. "
                           "`qty` above what is left is rejected.")}
FEATURE_DOCS = {
    "cancel": '''- `cancel(reservation_id)`: releases an open reservation: its `reserved` stock becomes
  available again and its status becomes `cancelled`. Only open reservations can be cancelled.
- `expire(now)`: every open reservation whose `expires_at` is not None and `<= now` is released
  the same way and gets the status `expired`; reservations without `expires_at` never expire.
  Reservations are expired in ascending id order, one event each, and the ids are returned as
  a list (empty when nothing is due, and then nothing is logged).

`fulfil`, `cancel` and `expire` only act on open reservations; a reservation that is
fulfilled, cancelled or expired is closed for good, and its id is never reused.
''',
    "partial": '''
A reservation that is partly shipped is still open: it counts as an open reservation in the
report and keeps holding the stock that is still to ship. Fulfilled reservations are closed
for good, and ids are never reused.
''',
}
EVENT_ROWS = {"cancel": "   | `cancelled` | `id` |\n   | `expired` | `id` |\n", "partial": ""}
STATUS_LISTS = {"cancel": "`open`, `fulfilled`, `cancelled`, `expired`", "partial": "`open`, `fulfilled`"}
STATUSES = {"cancel": '("open", "fulfilled", "cancelled", "expired")', "partial": '("open", "fulfilled")'}

PROMPTS = {
    "cancel": (
        "Add reservation cancellation and expiry to `@@PKG@@` as specified in `docs/inventory.md`: `cancel(id)` and "
        "`expire(now)`. The rest of the package (event log, replay, report) must keep honouring the invariants there.",
        "Our stock package needs `Inventory.cancel` and `Inventory.expire` (see `docs/inventory.md`). Implement them "
        "so every invariant in the docs still holds, including replay and the report.",
        "Implement reservation cancel and expiry in `@@PKG@@` per `docs/inventory.md`. Check the existing code against "
        "the documented invariants while you are in there.",
    ),
    "partial": (
        "Add partial fulfilment to `@@PKG@@`: `fulfil(id, qty=None)` as specified in `docs/inventory.md`. The event log, "
        "replay and report must keep honouring the invariants in that document.",
        "Warehouse staff ship reservations in several parcels. Implement partial `fulfil` in `@@PKG@@` per "
        "`docs/inventory.md` so every documented invariant still holds, including replay and the report.",
        "Make `Inventory.fulfil` accept a partial quantity as `docs/inventory.md` describes, and check the existing "
        "code against the documented invariants while you are in there.",
    ),
}
VISIBLE_TESTS = '''
import unittest

from @@PKG@@.events import replay
from @@PKG@@.models import StockError
from @@PKG@@.report import summary
from @@PKG@@.service import Inventory


class InventoryTests(unittest.TestCase):
    def test_receive_reserve_fulfil(self):
        inv = Inventory()
        inv.receive("@@SKU_A@@", 10)
        reservation = inv.reserve("@@SKU_A@@", 4)
        self.assertEqual(inv.snapshot()["items"]["@@SKU_A@@"], {"on_hand": 10, "reserved": 4})
        inv.fulfil(reservation)
        self.assertEqual(inv.snapshot()["items"]["@@SKU_A@@"], {"on_hand": 6, "reserved": 0})
        self.assertEqual([event["type"] for event in inv.events], ["received", "reserved", "fulfilled"])

    def test_cannot_reserve_more_than_available(self):
        inv = Inventory()
        inv.receive("@@SKU_A@@", 3)
        with self.assertRaises(StockError):
            inv.reserve("@@SKU_A@@", 4)

    def test_replay_matches(self):
        inv = Inventory()
        inv.receive("@@SKU_A@@", 5)
        inv.reserve("@@SKU_A@@", 2)
        self.assertEqual(replay(inv.events).snapshot(), inv.snapshot())

    def test_summary_totals(self):
        inv = Inventory()
        inv.receive("@@SKU_A@@", 5)
        inv.receive("@@SKU_B@@", 2)
        inv.reserve("@@SKU_A@@", 1)
        self.assertEqual(summary(inv)["totals"]["available"], 6)


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import random
import sys
import types
import unittest

from @@PKG@@.events import replay
from @@PKG@@.models import StockError
from @@PKG@@.report import summary
from @@PKG@@.service import Inventory

REFERENCE_SOURCES = @@REFERENCE@@
A, B = "@@SKU_A@@", "@@SKU_B@@"
SCRIPT = @@SCRIPT@@
EXPECTED_EVENTS = @@EXPECTED_EVENTS@@
STATUS_KEYS = @@STATUS_KEYS@@
OPERATIONS = @@OPERATIONS@@


def make():
    inv = Inventory()
    inv.receive(A, 10)
    inv.receive(B, 5)
    return inv


def run_script():
    inv, snapshots = Inventory(), []
    for method, args in SCRIPT:
        getattr(inv, method)(*args)
        snapshots.append(inv.snapshot())
    return inv, snapshots


def unchanged(test, inv, call, *args):
    before = (inv.snapshot(), inv.events)
    with test.assertRaises(StockError, msg=f"{call.__name__}{args}"):
        call(*args)
    test.assertEqual((inv.snapshot(), inv.events), before, f"{call.__name__}{args}")


def random_call(rng, inv):
    name = rng.choice(OPERATIONS)
    top = inv.snapshot()["next_id"] + 1
    if name == "receive":
        return name, (rng.choice([A, B, "C"]), rng.choice([0, 1, 2, 4, 6, 9]))
    if name == "reserve":
        return name, (rng.choice([A, B, "C"]), rng.choice([0, 1, 2, 3, 5]), rng.choice([None, None, 3, 8, 15]))
    if name == "write_off":
        return name, (rng.choice([A, B, "C"]), rng.choice([0, 1, 2, 3]))
    if name == "fulfil":
        return name, (rng.randint(1, top),) + @@FULFIL_RANDOM@@
    if name == "cancel":
        return name, (rng.randint(1, top),)
    return name, (rng.randint(0, 20),)


def load_reference():
    package = types.ModuleType("release_bench_reference")
    package.__path__ = []
    sys.modules[package.__name__] = package
    for name, source in REFERENCE_SOURCES.items():
        module = types.ModuleType(f"release_bench_reference.{name}")
        module.__package__ = package.__name__
        sys.modules[module.__name__] = module
        exec(compile(source, f"reference_{name}", "exec"), module.__dict__)
    return sys.modules["release_bench_reference.service"].Inventory


class HiddenStockChecks(unittest.TestCase):
    def test_new_feature_flow(self):
@@FLOW@@

    def test_new_feature_edges(self):
@@EDGES@@

    def test_rejected_calls_change_nothing(self):
        inv = make()
        reservation = inv.reserve(A, 2)
        bad = [(inv.receive, (A, 0)), (inv.receive, (A, True)), (inv.receive, ("", 1)), (inv.receive, (3, 1)),
               (inv.reserve, (A, 9)), (inv.reserve, ("Z", 1)), (inv.reserve, (A, 0)), (inv.reserve, (A, 1, "9")),
               (inv.reserve, (A, 1, True)), (inv.write_off, (A, 9)), (inv.write_off, ("Z", 1)),
               (inv.write_off, (A, 0)), (inv.fulfil, (99,)), (inv.fulfil, ("1",)), (inv.fulfil, (True,))]
@@BAD_EXTRA@@
        for call, args in bad:
            unchanged(self, inv, call, *args)
        self.assertEqual(inv.reserve(A, 1), @@NEXT_ID@@)

    def test_stock_rules_and_ids(self):
        inv = make()
        first = inv.reserve(A, 7)
        unchanged(self, inv, inv.reserve, A, 4)
        unchanged(self, inv, inv.write_off, A, 4)
        inv.write_off(A, 3)
        self.assertEqual(inv.events[-1], {"n": 4, "type": "written_off", "sku": A, "qty": 3})
        self.assertEqual(inv.snapshot()["items"][A], {"on_hand": 7, "reserved": 7})
        unchanged(self, inv, inv.reserve, A, 1)
        unchanged(self, inv, inv.write_off, A, 1)
        inv.fulfil(first)
        self.assertEqual(inv.events[-1], {"n": 5, "type": "fulfilled", "id": first, "qty": 7})
        self.assertEqual(inv.snapshot()["items"][A], {"on_hand": 0, "reserved": 0})
        unchanged(self, inv, inv.reserve, B, 6)
        self.assertEqual(inv.reserve(B, 5), 2)
        inv.receive("C", 2)
        self.assertEqual(inv.snapshot()["items"]["C"], {"on_hand": 2, "reserved": 0})
        self.assertEqual(inv.snapshot()["next_id"], 3)

    def test_every_change_appends_exactly_one_event(self):
        inv = Inventory()
        for index, (method, args) in enumerate(SCRIPT):
            getattr(inv, method)(*args)
            self.assertEqual(len(inv.events), index + 1, (method, args))
            unchanged(self, inv, inv.receive, A, 0)
            unchanged(self, inv, inv.reserve, A, 10**6)
            unchanged(self, inv, inv.fulfil, 10**6)
        self.assertEqual(inv.events, EXPECTED_EVENTS)
        inv.events.append({"n": 99})
        self.assertEqual(len(inv.events), len(EXPECTED_EVENTS))

    def test_replay_rebuilds_every_prefix(self):
        inv, snapshots = run_script()
        events = inv.events
        for count, snapshot in enumerate(snapshots, start=1):
            self.assertEqual(replay(events[:count]).snapshot(), snapshot, count)
        self.assertEqual(replay([]).snapshot(), Inventory().snapshot())
        with self.assertRaises(ValueError):
            replay([events[1]])
        with self.assertRaises(ValueError):
            replay(events[:2] + [{"n": 3, "type": "shrunk", "id": 1}])

    def test_report_matches_the_state(self):
        inv, _ = run_script()
        self.assert_report_consistent(inv)
        self.assert_report_consistent(make())
        self.assertEqual(list(summary(inv)["reservations"]), STATUS_KEYS)
        self.assertEqual(sum(summary(inv)["reservations"].values()), len(inv.snapshot()["reservations"]))

    def assert_report_consistent(self, inv):
        report, snap = summary(inv), inv.snapshot()
        self.assertEqual(list(report["items"]), sorted(snap["items"]))
        for sku, row in report["items"].items():
            item = snap["items"][sku]
            opened = [r for r in snap["reservations"].values() if r["sku"] == sku and r["status"] == "open"]
            self.assertEqual(row, {"on_hand": item["on_hand"], "reserved": item["reserved"],
                                   "available": item["on_hand"] - item["reserved"], "open_reservations": len(opened)})
        for key in ("on_hand", "reserved", "available", "open_reservations"):
            self.assertEqual(report["totals"][key], sum(row[key] for row in report["items"].values()), key)
        counts = {status: 0 for status in STATUS_KEYS}
        for reservation in snap["reservations"].values():
            counts[reservation["status"]] += 1
        self.assertEqual(report["reservations"], counts)

    def test_random_sequences_keep_the_invariants(self):
        rng = random.Random(@@SEED@@)
        for _ in range(40):
            inv = Inventory()
            for _ in range(rng.randint(8, 40)):
                name, args = random_call(rng, inv)
                before = len(inv.events)
                try:
                    result = getattr(inv, name)(*args)
                except StockError:
                    self.assertEqual(len(inv.events), before, (name, args))
                    continue
                self.assertEqual(len(inv.events) - before, len(result) if name == "expire" else 1, (name, args))
                self.assert_stock_invariants(inv)
            self.assertEqual([event["n"] for event in inv.events], list(range(1, len(inv.events) + 1)))
            self.assertEqual(replay(inv.events).snapshot(), inv.snapshot())
            self.assert_report_consistent(inv)

    def assert_stock_invariants(self, inv):
        snap = inv.snapshot()
        for sku, item in snap["items"].items():
            self.assertGreaterEqual(item["on_hand"], 0, sku)
            self.assertGreaterEqual(item["on_hand"] - item["reserved"], 0, sku)
            owed = sum(r["qty"] - r["shipped"] for r in snap["reservations"].values()
                       if r["sku"] == sku and r["status"] == "open")
            self.assertEqual(item["reserved"], owed, sku)

    def test_random_sequences_match_the_reference(self):
        rng = random.Random(@@SEED@@ + 1)
        reference = load_reference()
        for _ in range(40):
            mine, theirs = Inventory(), reference()
            for _ in range(rng.randint(8, 40)):
                name, args = random_call(rng, theirs)
                self.assertEqual(self.outcome(mine, name, args), self.outcome(theirs, name, args), (name, args))
                self.assertEqual(mine.snapshot(), theirs.snapshot(), (name, args))
                self.assertEqual(mine.events, theirs.events, (name, args))

    @staticmethod
    def outcome(inv, name, args):
        try:
            return "value", getattr(inv, name)(*args)
        except ValueError:
            return "error", None
'''
SCRIPTS = {
    "cancel": ('[("receive", (A, 5)), ("reserve", (A, 2)), ("write_off", (A, 1)), ("fulfil", (1,)), ("reserve", (A, 1, 2)),\n'
               '          ("cancel", (2,)), ("reserve", (A, 1, 3)), ("expire", (3,)), ("receive", (B, 2)), ("reserve", (B, 2, 9))]'),
    "partial": ('[("receive", (A, 5)), ("reserve", (A, 2)), ("write_off", (A, 1)), ("fulfil", (1,)), ("receive", (A, 4)),\n'
                '          ("reserve", (A, 3, 7)), ("fulfil", (2, 1)), ("fulfil", (2,)), ("receive", (B, 2)),\n'
                '          ("reserve", (B, 2)), ("fulfil", (3, 1))]'),
}
EXPECTED = {
    "cancel": '''[
    {"n": 1, "type": "received", "sku": A, "qty": 5},
    {"n": 2, "type": "reserved", "id": 1, "sku": A, "qty": 2, "expires_at": None},
    {"n": 3, "type": "written_off", "sku": A, "qty": 1},
    {"n": 4, "type": "fulfilled", "id": 1, "qty": 2},
    {"n": 5, "type": "reserved", "id": 2, "sku": A, "qty": 1, "expires_at": 2},
    {"n": 6, "type": "cancelled", "id": 2},
    {"n": 7, "type": "reserved", "id": 3, "sku": A, "qty": 1, "expires_at": 3},
    {"n": 8, "type": "expired", "id": 3},
    {"n": 9, "type": "received", "sku": B, "qty": 2},
    {"n": 10, "type": "reserved", "id": 4, "sku": B, "qty": 2, "expires_at": 9},
]''',
    "partial": '''[
    {"n": 1, "type": "received", "sku": A, "qty": 5},
    {"n": 2, "type": "reserved", "id": 1, "sku": A, "qty": 2, "expires_at": None},
    {"n": 3, "type": "written_off", "sku": A, "qty": 1},
    {"n": 4, "type": "fulfilled", "id": 1, "qty": 2},
    {"n": 5, "type": "received", "sku": A, "qty": 4},
    {"n": 6, "type": "reserved", "id": 2, "sku": A, "qty": 3, "expires_at": 7},
    {"n": 7, "type": "fulfilled", "id": 2, "qty": 1},
    {"n": 8, "type": "fulfilled", "id": 2, "qty": 2},
    {"n": 9, "type": "received", "sku": B, "qty": 2},
    {"n": 10, "type": "reserved", "id": 3, "sku": B, "qty": 2, "expires_at": None},
    {"n": 11, "type": "fulfilled", "id": 3, "qty": 1},
]''',
}
FLOWS = {
    "cancel": '''        inv = make()
        first, second, third = inv.reserve(A, 4, 10), inv.reserve(A, 3), inv.reserve(B, 5, expires_at=20)
        self.assertEqual((first, second, third), (1, 2, 3))
        self.assertIsNone(inv.cancel(second))
        snap = inv.snapshot()
        self.assertEqual(snap["items"][A], {"on_hand": 10, "reserved": 4})
        self.assertEqual(snap["reservations"][second]["status"], "cancelled")
        self.assertEqual(inv.events[-1], {"n": 6, "type": "cancelled", "id": second})
        self.assertEqual(inv.reserve(A, 6), 4)
        inv.fulfil(first)
        self.assertEqual(inv.snapshot()["items"][A], {"on_hand": 6, "reserved": 6})
        self.assertEqual(summary(inv)["reservations"], {"open": 2, "fulfilled": 1, "cancelled": 1, "expired": 0})
        self.assertEqual(summary(inv)["items"][A]["open_reservations"], 1)''',
    "partial": '''        inv = make()
        reservation = inv.reserve(A, 6)
        self.assertIsNone(inv.fulfil(reservation, 2))
        snap = inv.snapshot()
        self.assertEqual(snap["items"][A], {"on_hand": 8, "reserved": 4})
        self.assertEqual((snap["reservations"][reservation]["shipped"], snap["reservations"][reservation]["status"]), (2, "open"))
        self.assertEqual(inv.events[-1], {"n": 4, "type": "fulfilled", "id": reservation, "qty": 2})
        self.assertEqual(summary(inv)["items"][A], {"on_hand": 8, "reserved": 4, "available": 4, "open_reservations": 1})
        self.assertEqual(inv.reserve(A, 4), 2)
        unchanged(self, inv, inv.reserve, A, 1)
        inv.fulfil(reservation)
        self.assertEqual(inv.snapshot()["items"][A], {"on_hand": 4, "reserved": 4})
        self.assertEqual(inv.events[-1], {"n": 6, "type": "fulfilled", "id": reservation, "qty": 4})
        self.assertEqual(summary(inv)["reservations"], {"open": 1, "fulfilled": 1})''',
}
EDGES = {
    "cancel": '''        inv = make()
        due, also, later, never, shipped = inv.reserve(A, 4, 10), inv.reserve(B, 2, 10), inv.reserve(A, 1, 11), inv.reserve(A, 1), inv.reserve(B, 1, 5)
        inv.fulfil(shipped)
        count = len(inv.events)
        self.assertEqual(inv.expire(9), [])
        self.assertEqual(len(inv.events), count)
        self.assertEqual(inv.expire(10), [due, also])
        self.assertEqual(inv.events[count:], [{"n": count + 1, "type": "expired", "id": due},
                                              {"n": count + 2, "type": "expired", "id": also}])
        self.assertEqual(inv.snapshot()["items"][A], {"on_hand": 10, "reserved": 2})
        self.assertEqual(inv.expire(10), [])
        self.assertEqual(inv.expire(10**6), [later])
        statuses = {rid: res["status"] for rid, res in inv.snapshot()["reservations"].items()}
        self.assertEqual(statuses, {due: "expired", also: "expired", later: "expired", never: "open", shipped: "fulfilled"})
        self.assertEqual(summary(inv)["items"][B], {"on_hand": 4, "reserved": 0, "available": 4, "open_reservations": 0})
        self.assertEqual(summary(inv)["items"][A]["available"], 9)
        unchanged(self, inv, inv.fulfil, due)
        unchanged(self, inv, inv.cancel, later)
        self.assertEqual(inv.reserve(A, 9), shipped + 1)''',
    "partial": '''        inv = make()
        reservation = inv.reserve(A, 5)
        for qty in (0, -1, True, 1.5, "2", 6):
            unchanged(self, inv, inv.fulfil, reservation, qty)
        inv.fulfil(reservation, 5)
        self.assertEqual(inv.snapshot()["reservations"][reservation]["status"], "fulfilled")
        self.assertEqual(inv.snapshot()["items"][A], {"on_hand": 5, "reserved": 0})
        unchanged(self, inv, inv.fulfil, reservation)
        unchanged(self, inv, inv.fulfil, reservation, 1)
        parcels = inv.reserve(B, 5)
        for _ in range(5):
            inv.fulfil(parcels, 1)
        self.assertEqual(inv.snapshot()["reservations"][parcels]["shipped"], 5)
        self.assertEqual(inv.snapshot()["items"][B], {"on_hand": 0, "reserved": 0})
        self.assertEqual([event["qty"] for event in inv.events[-5:]], [1] * 5)
        self.assertEqual(summary(inv)["totals"]["open_reservations"], 0)''',
}
BAD_EXTRAS = {
    "cancel": '''        done = inv.reserve(B, 1)
        inv.fulfil(done)
        gone = inv.reserve(B, 1)
        inv.cancel(gone)
        lapsed = inv.reserve(B, 1, 3)
        inv.expire(3)
        bad += [(inv.cancel, (99,)), (inv.cancel, (gone,)), (inv.cancel, (done,)), (inv.cancel, (lapsed,)),
                (inv.cancel, ("1",)), (inv.cancel, (True,)), (inv.fulfil, (gone,)), (inv.fulfil, (lapsed,)),
                (inv.expire, ("5",)), (inv.expire, (True,)), (inv.expire, (None,))]''',
    "partial": '''        bad += [(inv.fulfil, (reservation, 0)), (inv.fulfil, (reservation, 3)), (inv.fulfil, (reservation, 1.0))]''',
}
NEXT_ID = {"cancel": 5, "partial": 2}
OPERATIONS = {"cancel": '["receive", "reserve", "reserve", "write_off", "fulfil", "cancel", "expire"]',
              "partial": '["receive", "reserve", "reserve", "write_off", "fulfil", "fulfil"]'}
FULFIL_RANDOM = {"cancel": "()", "partial": "(rng.choice([None, None, 0, 1, 2, 3]),)"}
SKU_PAIRS = (("bolt", "washer"), ("widget", "gadget"), ("filter", "gasket"), ("beacon", "relay"), ("cable", "plug"))
DEFECT_POOL = ("writeoff", "burn", "open_count")


def _reference(pkg, feature):
    cancel = feature == "cancel"
    events = fill(EVENTS, FULFILLED=FULFILLED["good"], RELEASED=RELEASED if cancel else "",
                  MORE_HANDLERS=MORE_HANDLERS if cancel else "")
    service = fill(SERVICE, FULFIL=FULFIL["whole" if cancel else "partial"], FEATURE=FEATURE[feature])
    return {f"{pkg}/models.py": fill(MODELS, STATUSES=STATUSES[feature]), f"{pkg}/events.py": events,
            f"{pkg}/service.py": service, f"{pkg}/report.py": REPORT}


def _starter(pkg, defects):
    events = fill(EVENTS, FULFILLED=FULFILLED["whole"], RELEASED="", MORE_HANDLERS="")
    service = fill(STARTER_SERVICE, RESERVE=RESERVE["burn" if "burn" in defects else "good"],
                   WRITE_OFF_LIMIT=WRITE_OFF_LIMIT["writeoff" if "writeoff" in defects else "good"])
    report = fill(STARTER_REPORT, OPEN_FILTER=OPEN_FILTER["open_count" if "open_count" in defects else "good"])
    return {f"{pkg}/models.py": fill(MODELS, STATUSES=STATUSES["partial"]), f"{pkg}/events.py": events,
            f"{pkg}/service.py": service, f"{pkg}/report.py": report}


def _hidden(pkg, feature, seed, skus, reference):
    modules = {name: reference[f"{pkg}/{name}.py"] for name in ("models", "events", "service", "report")}
    return fill(HIDDEN, PKG=pkg, REFERENCE=repr(modules), SKU_A=skus[0], SKU_B=skus[1], SCRIPT=SCRIPTS[feature],
                EXPECTED_EVENTS=EXPECTED[feature], STATUS_KEYS=repr(list(eval(STATUSES[feature]))),
                OPERATIONS=OPERATIONS[feature], FULFIL_RANDOM=FULFIL_RANDOM[feature], FLOW=FLOWS[feature],
                EDGES=EDGES[feature], BAD_EXTRA=BAD_EXTRAS[feature], NEXT_ID=NEXT_ID[feature], SEED=seed)


def _doc(pkg, feature):
    fulfil_arg, fulfil_rule = FULFIL_ARGS["whole" if feature == "cancel" else "partial"]
    return fill(DOC, PKG=pkg, FULFIL_ARG=fulfil_arg, FULFIL_RULE=fulfil_rule, FEATURE_DOC=FEATURE_DOCS[feature],
                EVENT_ROWS=EVENT_ROWS[feature], STATUS_LIST=STATUS_LISTS[feature])


def generate(seed, rng):
    pkg = package_name(rng, "stock")
    feature = rng.choice(["cancel", "partial"])
    skus = rng.choice(SKU_PAIRS)
    defects = set(rng.sample(DEFECT_POOL, rng.randint(1, 2)))
    reference = _reference(pkg, feature)
    files = {"README.md": f"# {pkg}\n\nStock and reservations. Rules: `docs/inventory.md`.\n", f"{pkg}/__init__.py": "",
             **_starter(pkg, defects), "tests/__init__.py": "",
             "tests/test_stock.py": fill(VISIBLE_TESTS, PKG=pkg, SKU_A=skus[0], SKU_B=skus[1]),
             "docs/inventory.md": _doc(pkg, feature)}
    return make_spec(family="invariant", template="stockbook", seed=seed,
                     prompt=fill(rng.choice(PROMPTS[feature]), allow_unused=True, PKG=pkg), files=files,
                     hidden=_hidden(pkg, feature, seed, skus, reference), solution=reference,
                     meta={"feature": feature, "defects": len(defects)})
