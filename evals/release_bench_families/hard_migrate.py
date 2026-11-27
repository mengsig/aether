"""hard / migrate: a JSON-lines record store moves from schema v1 to v2 across models, migrate and store.

Each seed fixes the owner format, the v1 timestamp format, the money rounding
and the duplicate-extras rule in docs/schema.md, then plants four to six
defects spread over the three modules that only edge rows expose.
"""
import json
from datetime import datetime, timedelta, timezone

from .common import fill, make_spec, package_name

MODELS = '''
"""Records of the schema v1 to v2 migration (rules: docs/schema.md)."""
import json
import re
from datetime import datetime, timedelta, timezone
from decimal import ROUND_DOWN, ROUND_HALF_EVEN, ROUND_HALF_UP, Decimal

V1_FIELDS = ("id", "title", "owner", "price", "updated")
ROUNDING = @@ROUNDING@@
PRICE = re.compile(r"-?\\d+(\\.\\d+)?")
ISO = "%Y-%m-%dT%H:%M:%SZ"
OFFSET_HOURS = @@OFFSET@@


def schema_of(text):
    """1 or 2: a first non-blank line that is exactly `{"schema": 2}` marks v2."""
    for line in text.split("\\n"):
        if not line.strip():
            continue
        try:
            first = json.loads(line)
        except ValueError:
            return 1
        if not (isinstance(first, dict) and set(first) == {"schema"}):
            return 1
@@SCHEMA_CHECK@@
        return 2
    return 1


def parse_v2(text):
    """The records of a v2 file, without the header line."""
    lines = [line for line in text.split("\\n") if line.strip()]
    assert not lines or json.loads(lines[0]) == {"schema": 2}, "a v2 file starts with its header"
    return [json.loads(line) for line in lines[1:]]


def render_records(records):
    """Canonical v2 text: the header, then one compact sorted-key line per record."""
    lines = ['{"schema":2}']
    lines += [@@DUMP@@ for record in records]
    return "\\n".join(lines) + "\\n"

@@PRICE@@

@@OWNER@@

@@STAMP@@

def upgrade_record(old):
    """The v2 record for one v1 record; ValueError when the record is bad."""
    if not isinstance(old, dict) or not isinstance(old.get("id"), str) or not old["id"]:
        raise ValueError(f"a record needs a non-empty string id: {old!r}")
    first, last = split_owner(old.get("owner"))
    record = {"id": old["id"], "name": old.get("title"), "owner_first": first, "owner_last": last,
              "price_minor": price_to_minor(old.get("price")), "updated_at": to_utc_iso(old.get("updated"))}
@@EXTRA@@
    return record
'''
SCHEMA_CHECK = {
    "good": '        if first["schema"] != 2 or type(first["schema"]) is not int:\n'
            '            raise ValueError(f"unsupported schema {first[\'schema\']!r}")',
    "unsupported": '        if first["schema"] != 2:\n            return 1',
}
DUMP = {"good": 'json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=False)',
        "render": "json.dumps(record, sort_keys=True)"}
PRICE_HEAD = '''def price_to_minor(value):
    """Minor units of a v1 price string, rounded as docs/schema.md says; None stays None."""
'''
PRICE_NULL = "    if value is None:\n        return None\n"
PRICE_CHECK = ('    if not isinstance(value, str) or not PRICE.fullmatch(value.strip()):\n'
               '        raise ValueError(f"price must be a decimal string: {value!r}")\n')
PRICE_RETURN = {
    "good": "    return int((Decimal(value.strip()) * 100).quantize(Decimal(1), rounding=ROUNDING))",
    "mode": "    return int((Decimal(value.strip()) * 100).quantize(Decimal(1), rounding=ROUND_HALF_UP))",
    "float": "    return int(round(float(value) * 100))",
}
OWNER = {
    "first_last": {
        "good": 'def split_owner(owner):\n    """(first, last) of a "First Middle Last" owner; None stays None."""\n'
                '    if owner is None:\n        return None, None\n    if not isinstance(owner, str):\n'
                '        raise ValueError(f"owner must be text: {owner!r}")\n    words = owner.split()\n'
                '    return " ".join(words[:-1]), (words[-1] if words else "")',
        "owner": 'def split_owner(owner):\n    """(first, last) of a "First Middle Last" owner; None stays None."""\n'
                 '    if owner is None:\n        return None, None\n    first, _, last = owner.partition(" ")\n'
                 '    return first, last',
    },
    "last_first": {
        "good": 'def split_owner(owner):\n    """(first, last) of a "Last, First" owner; None stays None."""\n'
                '    if owner is None:\n        return None, None\n    if not isinstance(owner, str):\n'
                '        raise ValueError(f"owner must be text: {owner!r}")\n'
                '    last, comma, first = owner.partition(",")\n    if not comma:\n'
                '        return "", " ".join(owner.split())\n    return " ".join(first.split()), " ".join(last.split())',
        "owner": 'def split_owner(owner):\n    """(first, last) of a "Last, First" owner; None stays None."""\n'
                 '    if owner is None:\n        return None, None\n    last, _, first = owner.partition(",")\n'
                 '    return first, last',
    },
}
STAMP = {
    "epoch": {
        "good": 'def to_utc_iso(value):\n    """ISO UTC text of a v1 epoch-seconds int; None stays None."""\n'
                '    if value is None:\n        return None\n    if type(value) is not int:\n'
                '        raise ValueError(f"updated must be integer seconds: {value!r}")\n'
                '    return datetime.fromtimestamp(value, timezone.utc).strftime(ISO)',
        "stamp": 'def to_utc_iso(value):\n    """ISO UTC text of a v1 epoch-seconds int; None stays None."""\n'
                 '    if value is None:\n        return None\n    if type(value) is not int:\n'
                 '        raise ValueError(f"updated must be integer seconds: {value!r}")\n'
                 '    return datetime.fromtimestamp(value, timezone.utc).isoformat()',
    },
    "local": {
        "good": 'def to_utc_iso(value):\n    """ISO UTC text of a v1 `DD/MM/YYYY HH:MM` local time; None stays None."""\n'
                '    if value is None:\n        return None\n    if not isinstance(value, str):\n'
                '        raise ValueError(f"updated must be text: {value!r}")\n    try:\n'
                '        local = datetime.strptime(value, "%d/%m/%Y %H:%M")\n    except ValueError as error:\n'
                '        raise ValueError(f"bad updated {value!r}") from error\n'
                '    return (local - timedelta(hours=OFFSET_HOURS)).strftime(ISO)',
        "stamp": 'def to_utc_iso(value):\n    """ISO UTC text of a v1 `DD/MM/YYYY HH:MM` local time; None stays None."""\n'
                 '    if value is None:\n        return None\n    if not isinstance(value, str):\n'
                 '        raise ValueError(f"updated must be text: {value!r}")\n    try:\n'
                 '        local = datetime.strptime(value, "%d/%m/%Y %H:%M")\n    except ValueError as error:\n'
                 '        raise ValueError(f"bad updated {value!r}") from error\n'
                 '    return local.strftime(ISO)',
    },
}
EXTRA = {"good": '    extra = {key: value for key, value in old.items() if key not in V1_FIELDS}\n'
                 '    if extra:\n        record["extra"] = extra',
         "extra": "    pass"}

MIGRATE = '''
"""Whole-file migration from schema v1 to v2 (rules: docs/schema.md)."""
import json
import os
from pathlib import Path

from .models import parse_v2, render_records, schema_of, upgrade_record


class MigrationError(ValueError):
    """A bad record; `line` is its 1-based physical line number."""

    def __init__(self, line, reason):
        super().__init__(f"line {line}: {reason}")
        self.line, self.reason = line, reason


def _stamp(record):
    return record["updated_at"] is not None, record["updated_at"] or ""


def _pick(old, new):
    winner, loser = (new, old) if @@NEWER@@ else (old, new)
    winner = dict(winner)
@@EXTRAS@@
    return winner


def _resolve(records):
    chosen = {}
    for record in records:
        old = chosen.get(record["id"])
@@STORE@@
    return list(chosen.values())


def upgrade_text(text):
    """The v2 records of a v1 or v2 file; duplicates resolved, ValueError names the bad line."""
@@V2_BRANCH@@    records = []
    for number, line in enumerate(text.split("\\n"), start=@@LINE_START@@):
        if not line.strip():
            continue
        try:
            records.append(upgrade_record(json.loads(line)))
        except ValueError as error:
            raise MigrationError(number, str(error)) from error
    return _resolve(records)


def migrate_text(text):
    """The v2 text for a v1 file; v2 text is returned unchanged."""
@@IDEMPOTENT@@    return render_records(upgrade_text(text))


def migrate_file(src, dst=None):
    """Write the migrated `src` to `dst`, or over `src`; a bad record writes nothing."""
@@FILE@@
'''
NEWER = {"good": "_stamp(new) >= _stamp(old)", "tie": "_stamp(new) > _stamp(old)", "last": "True"}
EXTRAS = {
    "merge": '    extra = {**loser.get("extra", {}), **winner.get("extra", {})}\n'
             '    if extra:\n        winner["extra"] = extra',
    "winner": '    extra = winner.get("extra", {})\n    if extra:\n        winner["extra"] = extra',
}
STORE = {"good": '        chosen[record["id"]] = record if old is None else _pick(old, record)',
         "order": '        chosen.pop(record["id"], None)\n'
                  '        chosen[record["id"]] = record if old is None else _pick(old, record)'}
V2_BRANCH = "    if schema_of(text) == 2:\n        return parse_v2(text)\n"
IDEMPOTENT = "    if schema_of(text) == 2:\n        return text\n"
FILE = {
    "good": '    target = Path(dst) if dst is not None else Path(src)\n'
            '    migrated = migrate_text(Path(src).read_bytes().decode("utf-8"))\n'
            '    temp = target.with_name(target.name + ".tmp")\n    try:\n'
            '        temp.write_bytes(migrated.encode("utf-8"))\n        os.replace(temp, target)\n'
            '    except OSError:\n        temp.unlink(missing_ok=True)\n        raise',
    "atomic": '    target = Path(dst) if dst is not None else Path(src)\n'
              '    text = Path(src).read_bytes().decode("utf-8")\n'
              '    with open(target, "w", encoding="utf-8", newline="") as out:\n'
              '        out.write(migrate_text(text))',
}
STORE_MODULE = {
    "good": '''
"""Reading and writing record files of either schema."""
from pathlib import Path

from .migrate import upgrade_text
from .models import render_records


def load(path):
    """The v2 records of a v1 or v2 file."""
    return upgrade_text(Path(path).read_bytes().decode("utf-8"))


def save(path, records):
    """Write `records` as a v2 file."""
    Path(path).write_bytes(render_records(records).encode("utf-8"))
''',
    "reader": '''
"""Reading and writing record files of either schema."""
import json
from pathlib import Path

from .models import render_records


def load(path):
    """The v2 records of a v2 file."""
    lines = Path(path).read_bytes().decode("utf-8").split("\\n")
    return [json.loads(line) for line in lines[1:] if line.strip()]


def save(path, records):
    """Write `records` as a v2 file."""
    Path(path).write_bytes(render_records(records).encode("utf-8"))
''',
}

DOC = '''
# Record schemas

A record file is UTF-8 text, one JSON object per line, lines separated by `\\n`.
Blank lines are ignored but still count when a line is numbered.

## Detecting the version

A file whose first non-blank line is exactly `{"schema": 2}` is **v2**. Any other file
is **v1** (it has no header). A first line `{"schema": N}` with any other `N` raises
`ValueError("unsupported schema ...")`. A first line that is not valid JSON is not a
header, so the file is v1 and that line is reported as a bad record.

## v1 records

`id` (string, required, non-empty), `title`, `owner`, `price`, `updated`; null or
missing means "unknown". Anything else in the object is an *unknown field*.

- `owner`: @@OWNER_V1@@ A missing or null owner is unknown.
- `price`: a decimal string matching `-?digits[.digits]` (`"12.5"`, `"-0.125"`); any
  other text, or a non-string, is a bad record.
- `updated`: @@UPDATED_V1@@

## v2 records and the migration (`models.upgrade_record`, `migrate.migrate_text`)

A v2 file is the header line `{"schema":2}` followed by one record per line. Fields:

| v2 | from v1 |
|---|---|
| `id` | `id` |
| `name` | `title`, renamed (None when unknown) |
| `owner_first`, `owner_last` | the split of `owner` (both None when unknown) |
| `price_minor` | `price` as an integer of minor units (hundredths): multiply by 100, then round @@ROUNDING_RULE@@. None when unknown |
| `updated_at` | `updated` as UTC text `YYYY-MM-DDTHH:MM:SSZ`. None when unknown |
| `extra` | a dict of the unknown fields with their values untouched; the key is left out when there are none |

**Duplicate ids.** One record per id survives. The record with the newest `updated_at`
wins; an unknown `updated_at` is older than any known one; on a tie the record on the
later line wins. Survivors stay in the order their ids first appeared in the file.
@@EXTRAS_RULE@@

**Canonical text.** `render_records` writes the header, then each record as JSON with
sorted keys, separators `,` and `:` without spaces, non-ASCII characters unescaped, each
line ending in `\\n`. An empty record list gives the header alone.

**Idempotent.** `migrate_text` on v2 text returns it unchanged, byte for byte; migrating a
v1 file twice gives the same bytes as once. `models.schema_of(text)` returns 1 or 2.

**Bad records.** Invalid JSON, a line that is not an object, a missing or empty `id`, a bad
`price` or a bad `updated` makes the migration raise `migrate.MigrationError` (a
`ValueError`) whose message starts `line N:` with the 1-based physical line number of the
first bad line.

## Files and the store

`migrate.migrate_file(src, dst=None)` writes the migrated text to `dst`, or over `src` when
`dst` is None. It is atomic: when a record is bad, `src` and any existing `dst` are exactly as
they were, no `dst` is created and no temporary file is left behind. A successful run leaves
no temporary file either. `store.load(path)` returns the v2 records of either a v1 or a v2
file (duplicates in v1 resolved as above); `store.save(path, records)` writes canonical v2.
'''
OWNER_V1 = {
    "first_last": 'text like `Ada King Lovelace`: split on whitespace; the last word is `owner_last` and the '
                  'words before it, joined by single spaces, are `owner_first`. One word is a last name with an '
                  'empty first name; a blank string gives two empty strings.',
    "last_first": 'text like `Lovelace, Ada King`: split at the first comma; both parts are stripped and inner '
                  'whitespace runs collapse to one space. Without a comma the whole text, collapsed the same '
                  'way, is `owner_last` and `owner_first` is empty.',
}
UPDATED_V1 = {
    "epoch": "an int of seconds since the Unix epoch (UTC). Any other type, `bool` included, is a bad record.",
    "local": "text `DD/MM/YYYY HH:MM` in the shop's local time, which is a fixed UTC@@SIGNED@@ with no "
             "daylight saving. Text in any other shape, or not a real date, and any non-string is a bad record.",
}
ROUNDING_RULES = {
    "half_up": "halves away from zero (`0.125` is 13, `-0.125` is -13)",
    "half_even": "halves to the nearest even integer (`0.125` is 12, `0.135` is 14)",
    "down": "toward zero, dropping the fraction (`0.129` is 12, `-0.129` is -12)",
}
EXTRAS_RULE = {
    True: "The unknown fields of every duplicate are kept: for each key in `extra`, the value from the "
          "winning record of that key's holders (the same newest-then-later-line order) is used.",
    False: "Only the winner's `extra` is kept; the discarded duplicates' unknown fields are dropped.",
}
ROUNDING_NAMES = {"half_up": "ROUND_HALF_UP", "half_even": "ROUND_HALF_EVEN", "down": "ROUND_DOWN"}

VISIBLE_TESTS = '''
import json
import tempfile
import unittest
from pathlib import Path

from @@PKG@@ import store
from @@PKG@@.migrate import migrate_text
from @@PKG@@.models import upgrade_record

V1 = "\\n".join([
    json.dumps({"id": "a1", "title": "Lamp", "owner": @@OWNER_A@@, "price": "12.50", "updated": @@UPDATED_A@@}),
    json.dumps({"id": "b2", "title": "Desk", "owner": @@OWNER_B@@, "price": "99", "updated": @@UPDATED_B@@}),
]) + "\\n"


class MigrationTests(unittest.TestCase):
    def test_record_fields(self):
        record = upgrade_record(json.loads(V1.splitlines()[0]))
        self.assertEqual((record["id"], record["name"], record["price_minor"]), ("a1", "Lamp", 1250))
        self.assertEqual((record["owner_first"], record["owner_last"]), ("Ada", "Lovelace"))

    def test_text_gets_a_header_and_a_line_per_record(self):
        lines = migrate_text(V1).splitlines()
        self.assertEqual(json.loads(lines[0]), {"schema": 2})
        self.assertEqual([json.loads(line)["id"] for line in lines[1:]], ["a1", "b2"])

    def test_store_saves_a_v2_file(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder, "items.jsonl")
            store.save(path, [upgrade_record(json.loads(line)) for line in V1.splitlines()])
            lines = path.read_text().splitlines()
            self.assertEqual(json.loads(lines[0]), {"schema": 2})
            self.assertEqual([json.loads(line)["price_minor"] for line in lines[1:]], [1250, 9900])


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import json
import os
import random
import sys
import tempfile
import types
import unittest
from pathlib import Path

from @@PKG@@ import store
from @@PKG@@.migrate import MigrationError, migrate_file, migrate_text
from @@PKG@@.models import render_records, schema_of, upgrade_record

REFERENCE_SOURCES = @@REFERENCE@@
PRICES = @@PRICES@@
OWNERS = @@OWNERS@@
STAMPS = @@STAMPS@@
BAD_STAMPS = @@BAD_STAMPS@@
MERGE = @@MERGE@@


@@V1_STAMP@@


def rec(id, **fields):
    return {"id": id, **fields}


def v1_text(*rows, blanks=0):
    return "\\n" * blanks + "\\n".join(row if isinstance(row, str) else json.dumps(row) for row in rows) + "\\n"


def migrated(*rows):
    return store_records(migrate_text(v1_text(*rows)))


def store_records(text):
    return [json.loads(line) for line in text.split("\\n")[1:] if line.strip()]


def load_reference():
    package = types.ModuleType("release_bench_reference")
    package.__path__ = []
    sys.modules[package.__name__] = package
    for name, source in REFERENCE_SOURCES.items():
        module = types.ModuleType(f"release_bench_reference.{name}")
        module.__package__ = package.__name__
        sys.modules[module.__name__] = module
        exec(compile(source, f"reference_{name}", "exec"), module.__dict__)
    return sys.modules["release_bench_reference.migrate"]


class HiddenMigrationChecks(unittest.TestCase):
    def test_prices_round_as_documented(self):
        for text, minor in PRICES.items():
            self.assertEqual(upgrade_record(rec("p", price=text))["price_minor"], minor, text)
        self.assertIsNone(upgrade_record(rec("p", price=None))["price_minor"])
        self.assertIsNone(upgrade_record(rec("p"))["price_minor"])
        for bad in ("12,50", "abc", "", "1e3", " ", 12.5, 12):
            with self.assertRaises(ValueError, msg=repr(bad)):
                upgrade_record(rec("p", price=bad))

    def test_owner_is_split(self):
        for text, (first, last) in OWNERS.items():
            record = upgrade_record(rec("o", owner=text))
            self.assertEqual((record["owner_first"], record["owner_last"]), (first, last), text)
        record = upgrade_record(rec("o"))
        self.assertEqual((record["owner_first"], record["owner_last"]), (None, None))

    def test_timestamps_are_normalised_to_utc(self):
        for value, expected in STAMPS:
            self.assertEqual(upgrade_record(rec("t", updated=value))["updated_at"], expected, value)
        self.assertIsNone(upgrade_record(rec("t", updated=None))["updated_at"])
        for bad in BAD_STAMPS:
            with self.assertRaises(ValueError, msg=repr(bad)):
                upgrade_record(rec("t", updated=bad))

    def test_unknown_fields_are_kept_under_extra(self):
        old = rec("u", title="Lamp", tags=["a", {"b": None}], name="kept", price_minor=7, schema=9, note="n\\u00e9")
        before = json.dumps(old, sort_keys=True)
        record = upgrade_record(old)
        self.assertEqual(record["extra"], {"tags": ["a", {"b": None}], "name": "kept", "price_minor": 7,
                                           "schema": 9, "note": "n\\u00e9"})
        self.assertEqual(record["name"], "Lamp")
        self.assertEqual(json.dumps(old, sort_keys=True), before)
        self.assertNotIn("extra", upgrade_record(rec("u", title="Lamp")))
        self.assertIsNone(upgrade_record(rec("u"))["name"])

    def test_duplicate_ids_are_resolved(self):
        rows = [rec("d1", title="old", updated=v1_stamp(10)), rec("d2", title="only", updated=v1_stamp(20)),
                rec("d1", title="newest", updated=v1_stamp(3000)), rec("d1", title="stale", updated=v1_stamp(500)),
                rec("d3", title="n1"), rec("d3", title="n2"), rec("d4", title="t1", updated=v1_stamp(5)),
                rec("d4", title="t2", updated=v1_stamp(5)), rec("d5", title="dated", updated=v1_stamp(1)),
                rec("d5", title="undated"), rec("d6", title="undated"), rec("d6", title="dated", updated=v1_stamp(2))]
        records = migrated(*rows)
        self.assertEqual([r["id"] for r in records], ["d1", "d2", "d3", "d4", "d5", "d6"])
        self.assertEqual([r["name"] for r in records], ["newest", "only", "n2", "t2", "dated", "dated"])
        self.duplicate_extras_follow_the_rule()

    def duplicate_extras_follow_the_rule(self):
        rows = [rec("e", title="b", updated=v1_stamp(900), x=1, y=2), rec("e", title="a", updated=v1_stamp(100), y=0, z=3),
                rec("e", title="c", updated=v1_stamp(900), y=5)]
        (record,) = migrated(*rows)
        self.assertEqual(record["name"], "c")
        self.assertEqual(record["extra"], {"x": 1, "y": 5, "z": 3} if MERGE else {"y": 5})
        (plain,) = migrated(rec("e", title="a", updated=v1_stamp(1), y=0), rec("e", title="b", updated=v1_stamp(2)))
        self.assertEqual(plain.get("extra"), {"y": 0} if MERGE else None)

    def test_text_is_canonical_and_migration_is_idempotent(self):
        row = rec("c1", title="Caf\\u00e9", owner=@@OWNER_C@@, price="1.5", updated=@@UPDATED_C@@, tag="\\u00fc")
        expected = '{"schema":2}\\n' + @@EXPECTED_LINE@@ + "\\n"
        once = migrate_text(v1_text(row, blanks=2))
        self.assertEqual(once, expected)
        self.assertEqual(migrate_text(once), once)
        spaced = '{"schema": 2}\\n\\n{"id": "z",  "name": "Z"}\\n'
        self.assertEqual(migrate_text(spaced), spaced)
        self.assertEqual(migrate_text(v1_text()), '{"schema":2}\\n')
        self.assertEqual(schema_of(spaced), 2)
        self.assertEqual(schema_of(v1_text(row)), 1)
        self.assertEqual(render_records([]), '{"schema":2}\\n')

    def test_unsupported_schema_and_files_of_both_versions_load(self):
        for text in ('{"schema": 3}\\n', '{"schema": "2"}\\n{"id": "a"}\\n'):
            with self.assertRaises(ValueError, msg=text):
                migrate_text(text)
        v1 = v1_text(rec("a", title="A", price="1.00"), rec("b", title="B", price="2.00"), rec("a", title="A2", price="3.00"))
        expected = [{"id": "a", "name": "A2", "owner_first": None, "owner_last": None, "price_minor": 300, "updated_at": None},
                    {"id": "b", "name": "B", "owner_first": None, "owner_last": None, "price_minor": 200, "updated_at": None}]
        with tempfile.TemporaryDirectory() as folder:
            first, second = Path(folder, "v1.jsonl"), Path(folder, "v2.jsonl")
            first.write_text(v1)
            self.assertEqual(store.load(first), expected)
            store.save(second, expected)
            self.assertEqual(second.read_text(), render_records(expected))
            self.assertEqual(store.load(second), expected)
            self.assertEqual(second.read_text(), migrate_text(second.read_text()))

    def test_bad_records_name_the_line_and_write_nothing(self):
        good = json.dumps(rec("g", title="G"))
        cases = [(2, [good, "{not json", good]), (3, ["", good, json.dumps([1, 2]), json.dumps(rec("x", price="1,5"))]),
                 (5, [good, good, good, good, json.dumps(rec("", title="no id"))]),
                 (2, [good, json.dumps(rec("x", updated=BAD_STAMPS[0]))])]
        for line, rows in cases:
            text = "\\n".join(rows) + "\\n"
            with self.assertRaises(MigrationError, msg=text) as caught:
                migrate_text(text)
            self.assertTrue(str(caught.exception).startswith(f"line {line}:"), str(caught.exception))
            self.assertIsInstance(caught.exception, ValueError)
            self._file_is_untouched(text)

    def _file_is_untouched(self, text):
        with tempfile.TemporaryDirectory() as folder:
            src, dst, keep = Path(folder, "in.jsonl"), Path(folder, "out.jsonl"), Path(folder, "keep.jsonl")
            src.write_text(text)
            keep.write_text("keep me")
            with self.assertRaises(MigrationError):
                migrate_file(src, dst)
            with self.assertRaises(MigrationError):
                migrate_file(src, keep)
            with self.assertRaises(MigrationError):
                migrate_file(src)
            self.assertEqual(src.read_text(), text)
            self.assertEqual(keep.read_text(), "keep me")
            self.assertEqual(sorted(os.listdir(folder)), ["in.jsonl", "keep.jsonl"])

    def test_migrate_file_writes_canonical_text_without_leftovers(self):
        text = v1_text(rec("f1", title="One", price="2.5"), rec("f2", title="Two", price="0.4"))
        with tempfile.TemporaryDirectory() as folder:
            src, dst = Path(folder, "in.jsonl"), Path(folder, "out.jsonl")
            src.write_text(text)
            migrate_file(src, dst)
            self.assertEqual(dst.read_text(), migrate_text(text))
            self.assertEqual(src.read_text(), text)
            migrate_file(src)
            self.assertEqual(src.read_text(), dst.read_text())
            migrate_file(src)
            migrate_file(dst, dst)
            self.assertEqual(src.read_text(), dst.read_text())
            self.assertEqual(sorted(os.listdir(folder)), ["in.jsonl", "out.jsonl"])

    def test_random_files_match_the_reference(self):
        reference = load_reference()
        rng = random.Random(@@SEED@@)
        for _ in range(80):
            rows = [self._random_row(rng) for _ in range(rng.randint(0, 9))]
            text = v1_text(*rows, blanks=rng.randint(0, 2))
            self.assertEqual(migrate_text(text), reference.migrate_text(text), text)

    @staticmethod
    def _random_row(rng):
        row = {"id": rng.choice("abcd")}
        if rng.random() < 0.8:
            row["title"] = rng.choice(["Lamp", "Desk", None])
        if rng.random() < 0.7:
            row["owner"] = rng.choice(list(OWNERS) + [None])
        if rng.random() < 0.8:
            row["price"] = rng.choice(list(PRICES) + [None])
        if rng.random() < 0.7:
            row["updated"] = rng.choice([v1_stamp(rng.randint(0, 6000)), v1_stamp(rng.randint(0, 3)), None])
        if rng.random() < 0.4:
            row[rng.choice(["tag", "note"])] = rng.choice([1, "x", [1], None])
        return row
'''
PRICE_TABLES = {
    "half_up": {"12.5": 1250, "0.125": 13, "0.135": 14, "2.675": 268, "-0.125": -13, "1.005": 101, "19.99": 1999,
                "0.1": 10, "7": 700, "-1.999": -200, "1234.5678": 123457, "0.004": 0},
    "half_even": {"12.5": 1250, "0.125": 12, "0.135": 14, "2.675": 268, "-0.125": -12, "1.005": 100, "19.99": 1999,
                  "0.1": 10, "7": 700, "-1.999": -200, "1234.5678": 123457, "0.004": 0},
    "down": {"12.5": 1250, "0.125": 12, "0.135": 13, "2.675": 267, "-0.125": -12, "1.005": 100, "19.99": 1999,
             "0.1": 10, "7": 700, "-1.999": -199, "1234.5678": 123456, "0.004": 0},
}
OWNER_TABLES = {
    "first_last": {"Ada Lovelace": ("Ada", "Lovelace"), "Ada King  Lovelace": ("Ada King", "Lovelace"),
                   "  Plato ": ("", "Plato"), "": ("", ""), "Jean Baptiste Emmanuel Zorg": ("Jean Baptiste Emmanuel", "Zorg")},
    "last_first": {"Lovelace, Ada": ("Ada", "Lovelace"), "Lovelace,  Ada   King ": ("Ada King", "Lovelace"),
                   "  Plato ": ("", "Plato"), "": ("", ""), "Van  der Berg, Jan, Jr": ("Jan, Jr", "Van der Berg")},
}
OWNER_SAMPLES = {"first_last": ("Ada Lovelace", "Alan Mathison Turing", "Grace  Hopper"),
                 "last_first": ("Lovelace, Ada", "Turing, Alan Mathison", "Hopper,  Grace")}

PROMPTS = (
    "We are moving our record store from schema v1 to v2. `@@PKG@@/migrate.py` runs on the sample file but loses or "
    "mangles data on real files. Make the migration, the models and the store follow `docs/schema.md`.",
    "Finish the v1 to v2 migration in `@@PKG@@` as specified in `docs/schema.md`: every rule there, including "
    "duplicates, unknown fields, idempotence and failure behaviour.",
    "`@@PKG@@` migrates record files to schema v2, but customers report lost fields, wrong prices and half-written "
    "files. Audit the package against `docs/schema.md` and fix every deviation.",
)
CONCERNS = {"money": None, "owner": ("owner",), "stamp": ("stamp",), "extra": ("extra",), "nullprice": ("nullprice",),
            "dup": ("tie", "last"), "order": ("order",), "merge": ("merge",), "atomic": ("atomic",),
            "lineno": ("lineno",), "idem": ("idem",), "render": ("render",), "unsupported": ("unsupported",),
            "reader": ("reader",)}


def _pick_defects(rng, rounding, merge):
    options = {name: list(variants or ()) for name, variants in CONCERNS.items()}
    options["money"] = ["float"] + (["mode"] if rounding != "half_up" else [])
    if not merge:
        del options["merge"]
    chosen = rng.sample(sorted(options), rng.randint(4, 6))
    return {rng.choice(options[name]) for name in chosen}


def _stamp_cases(kind, offset):
    base = datetime(2024, 3, 5, 14, 30)
    if kind == "epoch":
        values = [0, 1709649000, 1735689599]
        iso = lambda n: datetime.fromtimestamp(n, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        return [(v, iso(v)) for v in values], ["1709649000", True, 1.5, 1709649000.0]
    locals_ = [base, datetime(2025, 1, 1, 0, 15), datetime(2024, 2, 29, 23, 45)]
    pairs = [(t.strftime("%d/%m/%Y %H:%M"), (t - timedelta(hours=offset)).strftime("%Y-%m-%dT%H:%M:%SZ")) for t in locals_]
    return pairs, ["2024-03-05 14:30", "31/02/2024 10:00", 5, "05/03/2024", "5/3/24 4:30"]


V1_STAMP = {
    "epoch": "def v1_stamp(minutes):\n    return 1709000000 + minutes * 60",
    "local": ("from datetime import datetime, timedelta\n\n\ndef v1_stamp(minutes):\n"
              "    return (datetime(2024, 2, 27, 22, 0) + timedelta(minutes=minutes)).strftime(\"%d/%m/%Y %H:%M\")"),
}


def _models(cfg, defects):
    price = PRICE_HEAD + ("" if "nullprice" in defects else PRICE_NULL) + PRICE_CHECK
    price += PRICE_RETURN["float" if "float" in defects else "mode" if "mode" in defects else "good"]
    owner = OWNER[cfg["owner"]]["owner" if "owner" in defects else "good"]
    stamp = STAMP[cfg["stamp"]]["stamp" if "stamp" in defects else "good"]
    return fill(MODELS, ROUNDING=ROUNDING_NAMES[cfg["rounding"]], OFFSET=cfg["offset"],
                SCHEMA_CHECK=SCHEMA_CHECK["unsupported" if "unsupported" in defects else "good"],
                DUMP=DUMP["render" if "render" in defects else "good"], PRICE=price, OWNER=owner, STAMP=stamp,
                EXTRA=EXTRA["extra" if "extra" in defects else "good"])


def _migrate(cfg, defects):
    newer = "last" if "last" in defects else "tie" if "tie" in defects else "good"
    idem = "idem" in defects
    return fill(MIGRATE, NEWER=NEWER[newer], EXTRAS=EXTRAS["merge" if cfg["merge"] and "merge" not in defects else "winner"],
                STORE=STORE["order" if "order" in defects else "good"],
                V2_BRANCH="" if idem else V2_BRANCH, IDEMPOTENT="" if idem else IDEMPOTENT,
                LINE_START=0 if "lineno" in defects else 1, FILE=FILE["atomic" if "atomic" in defects else "good"])


def _package(pkg, cfg, defects):
    return {f"{pkg}/models.py": _models(cfg, defects), f"{pkg}/migrate.py": _migrate(cfg, defects),
            f"{pkg}/store.py": STORE_MODULE["reader" if "reader" in defects else "good"]}


def _canonical_line(cfg, row_owner, updated_iso):
    first, last = OWNER_TABLES[cfg["owner"]][row_owner] if row_owner in OWNER_TABLES[cfg["owner"]] else ("Ada", "Lovelace")
    record = {"id": "c1", "name": "Caf\u00e9", "owner_first": first, "owner_last": last, "price_minor": 150,
              "updated_at": updated_iso, "extra": {"tag": "\u00fc"}}
    return json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _hidden(pkg, cfg, seed, reference):
    stamps, bad = _stamp_cases(cfg["stamp"], cfg["offset"])
    owners = OWNER_TABLES[cfg["owner"]]
    owner_c = "Ada Lovelace" if cfg["owner"] == "first_last" else "Lovelace, Ada"
    updated_c, iso_c = stamps[0]
    return fill(HIDDEN, PKG=pkg, REFERENCE=repr(reference), PRICES=repr(PRICE_TABLES[cfg["rounding"]]),
                OWNERS=repr(owners), STAMPS=repr(stamps), BAD_STAMPS=repr(bad), MERGE=cfg["merge"],
                V1_STAMP=V1_STAMP[cfg["stamp"]], OWNER_C=repr(owner_c), UPDATED_C=repr(updated_c),
                EXPECTED_LINE=repr(_canonical_line(cfg, owner_c, iso_c)), SEED=seed)


def _docs(cfg):
    rounding = ROUNDING_RULES[cfg["rounding"]]
    updated = fill(UPDATED_V1[cfg["stamp"]], dedent=False, allow_unused=True, SIGNED=f"{cfg['offset']:+d}")
    return fill(DOC, OWNER_V1=OWNER_V1[cfg["owner"]], UPDATED_V1=updated, ROUNDING_RULE=rounding,
                EXTRAS_RULE=EXTRAS_RULE[cfg["merge"]])


def _visible(pkg, cfg):
    stamps, _ = _stamp_cases(cfg["stamp"], cfg["offset"])
    owner_a, owner_b = OWNER_SAMPLES[cfg["owner"]][:2]
    owner_a = "Ada Lovelace" if cfg["owner"] == "first_last" else "Lovelace, Ada"
    return fill(VISIBLE_TESTS, PKG=pkg, OWNER_A=repr(owner_a), OWNER_B=repr(owner_b),
                UPDATED_A=repr(stamps[0][0]), UPDATED_B=repr(stamps[1][0]))


def generate(seed, rng):
    pkg = package_name(rng, "records")
    cfg = {"owner": rng.choice(sorted(OWNER)), "stamp": rng.choice(sorted(STAMP)),
           "rounding": rng.choice(sorted(ROUNDING_NAMES)), "merge": rng.random() < 0.5}
    cfg["offset"] = rng.choice([-7, -3, 2, 5, 9]) if cfg["stamp"] == "local" else 0
    defects = _pick_defects(rng, cfg["rounding"], cfg["merge"])
    solution = _package(pkg, cfg, set())
    reference = {name: solution[f"{pkg}/{name}.py"] for name in ("models", "migrate")}
    files = {"README.md": f"# {pkg}\n\nRecord store. Schemas: `docs/schema.md`.\n", f"{pkg}/__init__.py": "",
             **_package(pkg, cfg, defects), "tests/__init__.py": "", "tests/test_migrate.py": _visible(pkg, cfg),
             "docs/schema.md": _docs(cfg)}
    return make_spec(family="migration", template="migrate", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=_hidden(pkg, cfg, seed, reference), solution=solution,
                     meta={"owner": cfg["owner"], "stamp": cfg["stamp"], "rounding": cfg["rounding"],
                           "merge": cfg["merge"], "defects": len(defects)})
