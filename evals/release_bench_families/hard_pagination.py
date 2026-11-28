"""seededbug / pagination: a colleague's cursor-pagination commit that claims to be done but is not.

The history holds two commits: a plain store, then "Add cursor pagination"
across store.py, cursor.py and api.py plus docs/pagination.md. Each seed
plants one to three defects from a pool; the prompt reports one symptom and
the rest are found only by reading the change against the doc.
"""
from .common import fill, make_spec, package_name

DOC = '''
# Item listing and cursor pagination

`api.list_items(store, sort, descending, status, limit, cursor)` returns one page:
`{"items": [...], "has_more": bool, "next_cursor": str or None}`.

## Order
- `sort` is `"@@NUM@@"` or `"name"`; anything else raises `ValueError`.
- Rows are ordered by `(sort value, id)`. The id breaks ties, so the order is total and never
  depends on insertion order.
- `descending=True` is the exact reverse of that order: larger sort value first, and among
  equal values the larger id first.

## Filter
- `status`, when given, keeps only rows with that status. The filter is applied **before**
  paging: every page except the last holds exactly `limit` matching rows.

## Page size
- `limit` is an `int` from 1 to @@MAX@@ inclusive (default @@DEFAULT@@). Any other value, including
  0, negatives, larger numbers and non-ints, raises `ValueError`.

## Cursor
- `next_cursor` is an opaque string. Pass it back with the same `sort`, `descending` and
  `status` to get the page that follows.
- A cursor marks a *position* in the order, not a row and not a count. Between two pages rows
  may be inserted or deleted, even the row the cursor came from: every row that existed the
  whole time is returned exactly once, none twice, none skipped. Rows inserted after the
  position show up in later pages, rows inserted before it never do.
- A cursor that is not a string made by this module, is malformed, or was issued for a
  different `sort`, `descending` or `status` raises `ValueError`. `cursor=None` is the first page.

## Ends of the listing
- `has_more` is true exactly when at least one matching row follows the page.
- `next_cursor` is `None` exactly when `has_more` is false.
- An empty listing is `{"items": [], "has_more": False, "next_cursor": None}`.
- Items are copies: changing them never changes the store.
'''

CURSOR = '''
"""Opaque cursor tokens: URL-safe base64 of a small JSON object."""
import base64
import binascii
import json


def encode(payload):
    assert isinstance(payload, dict), "a cursor payload is a dict"
    raw = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return base64.urlsafe_b64encode(raw).decode("ascii")


def decode(token):
    """The payload dict of `token`; ValueError when it is not a token made by `encode`."""
    if not isinstance(token, str) or not token:
        raise ValueError("a cursor is a non-empty string")
    try:
        payload = json.loads(base64.urlsafe_b64decode(token.encode("ascii")))
    except (binascii.Error, UnicodeError, ValueError) as error:
        raise ValueError("malformed cursor") from error
    if not isinstance(payload, dict):
        raise ValueError("malformed cursor")
    return payload
'''

STORE_BASE = '''
"""An in-memory table of items."""


class Store:
    def __init__(self):
        self._rows = {}

    def add(self, row):
        if row["id"] in self._rows:
            raise ValueError(f"duplicate id {row['id']}")
        self._rows[row["id"]] = dict(row)

    def remove(self, row_id):
        del self._rows[row_id]

    def __len__(self):
        return len(self._rows)
'''

ORDERED = '''
    def ordered(self, sort, descending=False, status=None):
        """Copies of the rows with `status` (all when None), in listing order."""
        rows = [dict(row) for row in self._rows.values() if status is None or row["status"] == status]
        @@ORDER@@
        return rows
'''
ORDER = {
    "good": 'rows.sort(key=lambda row: (row[sort], row["id"]), reverse=descending)',
    "ties": "rows.sort(key=lambda row: row[sort], reverse=descending)",
}

API = '''
"""The item listing."""
from .cursor import decode, encode

SORTS = ("@@NUM@@", "name")
MAX_LIMIT = @@MAX@@
DEFAULT_LIMIT = @@DEFAULT@@


def _check_arguments(sort, limit):
    if sort not in SORTS:
        raise ValueError(f"cannot sort by {sort!r}")
    if @@LIMIT_TEST@@:
        raise ValueError(f"limit must be an int up to {MAX_LIMIT}, got {limit!r}")


def _open(scope, cursor):
    payload = decode(cursor)
    if any(name not in payload or payload[name] != value for name, value in scope.items()):
        raise ValueError("cursor was issued for a different query")
    return payload

@@START_FN@@
@@TOKEN_FN@@

def list_items(store, sort="@@NUM@@", descending=False, status=None, limit=DEFAULT_LIMIT, cursor=None):
    _check_arguments(sort, limit)
    scope = {"sort": sort, "descending": bool(descending), "status": status}
    rows = @@ROWS@@
    start = _start(rows, scope, cursor)
    page = @@PAGE@@
    more = @@MORE@@
    token = _token(scope, page, start) if @@TOKEN_GUARD@@ else None
    return {"items": page, "has_more": more, "next_cursor": token}
'''
LIMIT_TEST = {
    "good": "type(limit) is not int or not 1 <= limit <= MAX_LIMIT",
    "zero": "type(limit) is not int or not 0 <= limit <= MAX_LIMIT",
    "max": "type(limit) is not int or not 1 <= limit < MAX_LIMIT",
}
START_FN = {
    "good": '''
def _start(rows, scope, cursor):
    """Index in `rows` of the first row after the cursor's position."""
    if cursor is None:
        return 0
    payload = _open(scope, cursor)
    if "key" not in payload or "id" not in payload:
        raise ValueError("cursor holds no position")
    sort, position = scope["sort"], (payload["key"], payload["id"])
    try:
        if scope["descending"]:
            follows = [(row[sort], row["id"]) < position for row in rows]
        else:
            follows = [(row[sort], row["id"]) > position for row in rows]
    except TypeError as error:
        raise ValueError("cursor position does not fit the sort") from error
    return follows.index(True) if True in follows else len(rows)
''',
    "ties": '''
def _start(rows, scope, cursor):
    """Index in `rows` of the first row after the cursor's position."""
    if cursor is None:
        return 0
    payload = _open(scope, cursor)
    sort, key = scope["sort"], payload["key"]
    for index, row in enumerate(rows):
        if (row[sort] < key) if scope["descending"] else (row[sort] > key):
            return index
    return len(rows)
''',
    "offset": '''
def _start(rows, scope, cursor):
    """Index in `rows` of the first row after the cursor's position."""
    if cursor is None:
        return 0
    return _open(scope, cursor)["offset"]
''',
}
TOKEN_FN = {
    "keyset": '''
def _token(scope, page, start):
    last = page[-1]
    return encode({**scope, "key": last[scope["sort"]], "id": last["id"]})
''',
    "offset": '''
def _token(scope, page, start):
    return encode({**scope, "offset": start + len(page)})
''',
}
FILTER = {
    "before": {"ROWS": "store.ordered(sort, descending, status)",
               "PAGE": "rows[start:start + limit]",
               "MORE": "start + limit < len(rows)", "TOKEN_GUARD": "more"},
    "after": {"ROWS": "store.ordered(sort, descending)",
              "PAGE": "[row for row in rows[start:start + limit] if status is None or row[\"status\"] == status]",
              "MORE": "start + limit < len(rows)", "TOKEN_GUARD": "more and page"},
}

PACKAGE_TESTS = '''
import unittest

from @@PKG@@.store import Store


class StoreTests(unittest.TestCase):
    def test_add_and_remove(self):
        store = Store()
        store.add({"id": 1, "name": "a", "@@NUM@@": 3, "status": "open"})
        self.assertEqual(len(store), 1)
        store.remove(1)
        self.assertEqual(len(store), 0)

    def test_duplicate_ids_are_rejected(self):
        store = Store()
        store.add({"id": 1, "name": "a", "@@NUM@@": 3, "status": "open"})
        with self.assertRaises(ValueError):
            store.add({"id": 1, "name": "b", "@@NUM@@": 4, "status": "open"})


if __name__ == "__main__":
    unittest.main()
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.api import list_items
from @@PKG@@.store import Store


def small_store():
    store = Store()
    for number in range(1, 6):
        store.add({"id": number, "name": f"item{number}", "@@NUM@@": number * 10,
                   "status": "open" if number % 2 else "done"})
    return store


class ListingTests(unittest.TestCase):
    def test_first_page_and_cursor_walk(self):
        store = small_store()
        page = list_items(store, limit=2)
        self.assertEqual([row["id"] for row in page["items"]], [1, 2])
        self.assertTrue(page["has_more"])
        seen = [row["id"] for row in page["items"]]
        while page["next_cursor"]:
            page = list_items(store, limit=2, cursor=page["next_cursor"])
            seen += [row["id"] for row in page["items"]]
        self.assertEqual(seen, [1, 2, 3, 4, 5])

    def test_descending_and_filter(self):
        store = small_store()
        page = list_items(store, descending=True, status="open")
        self.assertEqual([row["id"] for row in page["items"]], [5, 3, 1])
        self.assertFalse(page["has_more"])

    def test_bad_arguments(self):
        with self.assertRaises(ValueError):
            list_items(small_store(), sort="color")
        with self.assertRaises(ValueError):
            list_items(small_store(), limit=-1)


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import base64
import json
import random
import unittest

from @@PKG@@.api import list_items
from @@PKG@@.store import Store

NUM = "@@NUM@@"
MAX_LIMIT = @@MAX@@
DEFAULT_LIMIT = @@DEFAULT@@
STATUSES = ("@@S1@@", "@@S2@@")


def make_row(row_id, value, status="@@S1@@"):
    return {"id": row_id, "name": f"n{value % 5}-{row_id % 3}", NUM: value, "status": status}


def build(rows):
    store = Store()
    for row in rows:
        store.add(row)
    return store


def tied_rows(count, seed, statuses=("@@S1@@",)):
    rng = random.Random(seed)
    ids = rng.sample(range(1, 4 * count), count)
    return [make_row(row_id, rng.randint(1, 4), rng.choice(statuses)) for row_id in ids]


def expected(rows, sort=NUM, descending=False, status=None):
    kept = [row for row in rows if status is None or row["status"] == status]
    return sorted(kept, key=lambda row: (row[sort], row["id"]), reverse=descending)


def walk(store, limit, **query):
    pages, cursor = [], None
    while len(pages) < 500:
        page = list_items(store, limit=limit, cursor=cursor, **query)
        pages.append(page)
        cursor = page["next_cursor"]
        if cursor is None:
            return pages
    raise AssertionError("pagination never ended")


def ids(rows):
    return [row["id"] for row in rows]


class HiddenPaginationChecks(unittest.TestCase):
    def test_ties_are_ordered_by_id_in_both_directions(self):
        rows = tied_rows(20, 1)
        store = build(rows)
        for sort in (NUM, "name"):
            for descending in (False, True):
                got = list_items(store, sort=sort, descending=descending, limit=MAX_LIMIT)["items"]
                self.assertEqual(ids(got), ids(expected(rows, sort, descending)), (sort, descending))

    def test_walking_pages_over_ties_returns_every_row_once(self):
        rows = tied_rows(37, 2)
        store = build(rows)
        for sort in (NUM, "name"):
            for descending in (False, True):
                for limit in (1, 3, 5, 10):
                    pages = walk(store, limit, sort=sort, descending=descending)
                    got = [row for page in pages for row in page["items"]]
                    self.assertEqual(ids(got), ids(expected(rows, sort, descending)), (sort, descending, limit))

    def test_inserts_between_pages_neither_skip_nor_repeat(self):
        for descending in (False, True):
            rows = [make_row(row_id, 10 * row_id) for row_id in range(1, 13)]
            store = build(rows)
            first = list_items(store, descending=descending, limit=4)
            last = first["items"][-1]
            extra = [make_row(100, last[NUM]), make_row(0, last[NUM]), make_row(101, 5), make_row(102, 500)]
            for row in extra:
                store.add(row)
            rows += extra
            seen = list(first["items"])
            cursor = first["next_cursor"]
            while cursor:
                page = list_items(store, descending=descending, limit=4, cursor=cursor)
                seen += page["items"]
                cursor = page["next_cursor"]
            position = (last[NUM], last["id"])
            after = [row for row in expected(rows, NUM, descending)
                     if ((row[NUM], row["id"]) < position if descending else (row[NUM], row["id"]) > position)]
            self.assertEqual(ids(seen), ids(first["items"]) + ids(after), descending)

    def test_deletes_between_pages_keep_the_position(self):
        rows = tied_rows(24, 3)
        store = build(rows)
        first = list_items(store, limit=6)
        gone = {first["items"][-1]["id"], first["items"][0]["id"], expected(rows)[10]["id"]}
        for row_id in gone:
            store.remove(row_id)
        seen = list(first["items"])
        cursor = first["next_cursor"]
        while cursor:
            page = list_items(store, limit=6, cursor=cursor)
            seen += page["items"]
            cursor = page["next_cursor"]
        remaining = [row for row in expected(rows) if row["id"] not in gone]
        self.assertEqual(ids(seen), ids(first["items"]) + ids(remaining[len(first["items"]) - 2:]))
        self.assertEqual(len(ids(seen)), len(set(ids(seen))))

    def test_has_more_and_next_cursor_are_exact(self):
        for count, limit in ((0, 3), (3, 3), (6, 3), (7, 3), (1, 1), (4, 1), (MAX_LIMIT, MAX_LIMIT)):
            rows = [make_row(number, number) for number in range(1, count + 1)]
            pages = walk(build(rows), limit)
            self.assertEqual(len(pages), max(1, -(-count // limit)), (count, limit))
            for page in pages[:-1]:
                self.assertTrue(page["has_more"] and isinstance(page["next_cursor"], str), (count, limit))
                self.assertEqual(len(page["items"]), limit)
            self.assertFalse(pages[-1]["has_more"], (count, limit))
            self.assertIsNone(pages[-1]["next_cursor"], (count, limit))

    def test_filter_is_applied_before_paging(self):
        rows = tied_rows(40, 4, STATUSES)
        store = build(rows)
        for status in STATUSES:
            matching = expected(rows, status=status)
            pages = walk(store, 4, status=status, descending=True)
            for page in pages[:-1]:
                self.assertEqual(len(page["items"]), 4, status)
            got = [row for page in pages for row in page["items"]]
            self.assertEqual(ids(got), ids(reversed(matching)), status)
        self.assertEqual(list_items(build(rows[:1]), status="nothing"),
                         {"items": [], "has_more": False, "next_cursor": None})

    def test_limit_bounds_and_default(self):
        store = build([make_row(number, number) for number in range(1, MAX_LIMIT + 6)])
        for bad in (0, -1, MAX_LIMIT + 1, "5", 2.0, None):
            with self.assertRaises(ValueError, msg=repr(bad)):
                list_items(store, limit=bad)
        self.assertEqual(len(list_items(store, limit=MAX_LIMIT)["items"]), MAX_LIMIT)
        self.assertEqual(len(list_items(store, limit=1)["items"]), 1)
        self.assertEqual(len(list_items(store)["items"]), DEFAULT_LIMIT)

    def test_cursors_are_validated(self):
        store = build(tied_rows(12, 5, STATUSES))
        token = list_items(store, limit=3)["next_cursor"]
        junk = ["", "not a cursor!", base64.urlsafe_b64encode(b"[1]").decode(),
                base64.urlsafe_b64encode(b"{}").decode(), 12]
        for bad in junk:
            with self.assertRaises(ValueError, msg=repr(bad)):
                list_items(store, limit=3, cursor=bad)
        for other in ({"sort": "name"}, {"descending": True}, {"status": "@@S1@@"}):
            with self.assertRaises(ValueError, msg=str(other)):
                list_items(store, limit=3, cursor=token, **other)

    def test_items_are_copies(self):
        store = build([make_row(1, 5)])
        list_items(store)["items"][0][NUM] = 999
        self.assertEqual(list_items(store)["items"][0][NUM], 5)

    def test_random_edits_between_pages(self):
        rng = random.Random(@@SEED@@)
        for round_number in range(40):
            rows = tied_rows(rng.randint(0, 30), round_number, STATUSES)
            store, live = build(rows), {row["id"]: row for row in rows}
            query = {"descending": rng.random() < 0.5, "status": rng.choice((None,) + STATUSES)}
            limit = rng.randint(1, 6)
            stay = {row["id"] for row in expected(rows, status=query["status"])}
            seen, cursor, next_id = [], None, 1000
            for _ in range(200):
                page = list_items(store, limit=limit, cursor=cursor, **query)
                seen += ids(page["items"])
                cursor = page["next_cursor"]
                self.assertEqual(page["has_more"], cursor is not None)
                if cursor is None:
                    break
                if rng.random() < 0.5 and live:
                    victim = rng.choice(sorted(live))
                    store.remove(victim)
                    stay.discard(victim)
                    del live[victim]
                else:
                    row = make_row(next_id, rng.randint(1, 4), rng.choice(STATUSES))
                    store.add(row)
                    live[next_id] = row
                    next_id += 1
            self.assertEqual(len(seen), len(set(seen)), round_number)
            self.assertTrue(stay <= set(seen), round_number)
'''

SYMPTOMS = {
    "offset": "rows are repeated or missing between pages when items are added or removed while someone pages through",
    "ties": "rows with equal sort values go missing or come back in a different order at page boundaries",
    "has_more": "the last page sometimes says there is more, and the next page comes back empty",
    "filter": "listings filtered by status come back with short or empty pages while later pages still have rows",
    "limit": "the page size limits are not enforced the way the doc says",
}
PROMPTS = (
    "A colleague's last commit adds cursor pagination to the item listing and says it is ready, but QA "
    "reports: @@SYMPTOM@@. Review and fix the change so that it meets `docs/pagination.md`.",
    "Please finish off the cursor pagination in `@@PKG@@/` from the last commit. QA says: @@SYMPTOM@@. "
    "The contract is in `docs/pagination.md`; the code should follow it everywhere, not just where QA looked.",
    "The pagination commit in this repo is meant to be done, yet QA reports: @@SYMPTOM@@. Check `@@PKG@@/` "
    "against `docs/pagination.md` and fix whatever departs from it.",
)
VOCABULARIES = (("open", "done"), ("draft", "live"), ("queued", "sent"))


def _modules(pkg, params, defects):
    """(store.py, api.py) text for the planted `defects`, `params` for the shared placeholders."""
    order = ORDER["ties" if "ties" in defects else "good"]
    store = STORE_BASE + fill(ORDERED, ORDER=order, dedent=False)
    if "offset" in defects:
        start, token = START_FN["offset"], TOKEN_FN["offset"]
    else:
        start = START_FN["ties" if "ties" in defects else "good"]
        token = TOKEN_FN["keyset"]
    limit_kind = next((kind for kind in ("zero", "max") if kind in defects), "good")
    api = fill(API, LIMIT_TEST=LIMIT_TEST[limit_kind], START_FN=start, TOKEN_FN=token,
               **FILTER["after" if "filter" in defects else "before"], **params)
    if "has_more" in defects:
        api = api.replace("more = start + limit < len(rows)", "more = len(page) == limit")
    assert "ties" in defects or order is ORDER["good"], "the order defect and its flag agree"
    return fill(store, dedent=True), api


def _defects(rng):
    pool = ["offset", "ties", "has_more", "filter", rng.choice(["zero", "max"])]
    return sorted(rng.sample(pool, rng.choice([1, 2, 3])))


def _symptom(rng, defects):
    kind = rng.choice(defects)
    return SYMPTOMS["limit" if kind in ("zero", "max") else kind]


def generate(seed, rng):
    pkg = package_name(rng, "listing")
    num = rng.choice(["score", "price", "rank"])
    s1, s2 = rng.choice(VOCABULARIES)
    params = {"NUM": num, "MAX": rng.choice([25, 50, 100]), "DEFAULT": rng.choice([5, 10, 20])}
    defects = _defects(rng)
    store, api = _modules(pkg, params, defects)
    good_store, good_api = _modules(pkg, params, [])
    base_store = fill(STORE_BASE)
    first = {"README.md": f"# {pkg}\n\nAn in-memory item table with a paged listing.\n", f"{pkg}/__init__.py": "",
             f"{pkg}/store.py": base_store, "tests/__init__.py": "",
             "tests/test_store.py": fill(PACKAGE_TESTS, PKG=pkg, NUM=num)}
    change = {f"{pkg}/store.py": store, f"{pkg}/cursor.py": fill(CURSOR), f"{pkg}/api.py": api,
              "docs/pagination.md": fill(DOC, **params),
              "tests/test_listing.py": fill(VISIBLE_TESTS, PKG=pkg, NUM=num)}
    history = [{"message": "Add cursor pagination to the item listing", "files": change}]
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, S1=s1, S2=s2, NUM=num, MAX=params["MAX"], DEFAULT=params["DEFAULT"])
    prompt = fill(rng.choice(PROMPTS), allow_unused=True, PKG=pkg, SYMPTOM=_symptom(rng, defects))
    return make_spec(family="seededbug", template="pagination", seed=seed, prompt=prompt, files=first,
                     history=history, tags={"before-pagination": -1}, hidden=hidden,
                     solution={f"{pkg}/store.py": good_store, f"{pkg}/api.py": good_api},
                     meta={"defects": len(defects)})
