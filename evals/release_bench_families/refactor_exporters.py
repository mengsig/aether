"""refactor / exporters: consolidate copy-pasted row formatters without changing a single output byte.

Three or four exporters each carry their own copy of the row formatting, and
the copies have drifted: negative amounts, name spacing, currency case, date
layout, note escaping, zero amounts, headers or digit grouping differ per
exporter. The refactor must route every exporter through one shared
`format_row` while every exporter's output stays byte-identical to the
starter's on any input. Hidden checks compare each exporter against the
starter itself on edge-case and generated rows, and check the structure.
"""
import datetime

from .common import fill, make_spec, package_name, reference_block

KINDS = ["orders", "refunds", "invoices", "payouts", "credits"]
FIELDS = ["name", "amount", "currency", "date", "note"]
QUIRKS = ["parens", "strip_names", "asis_currency", "date_dmy", "date_compact", "backslash", "zero_dash",
          "header", "thousands"]
CANONICAL = {"negative": "minus", "collapse_names": True, "raw_currency": False, "date_format": "%Y-%m-%d",
             "escape": "quote", "thousands": False, "zero_dash": False}
QUIRK_OPTIONS = {"parens": {"negative": "parens"}, "strip_names": {"collapse_names": False},
                 "asis_currency": {"raw_currency": True}, "date_dmy": {"date_format": "%d/%m/%Y"},
                 "date_compact": {"date_format": "%Y%m%d"}, "backslash": {"escape": "backslash"},
                 "zero_dash": {"zero_dash": True}, "thousands": {"thousands": True}}
BANNED = ("divmod", "strftime", "split", "replace", "upper")

STARTER_HEAD = '''
"""Partner exports: one line per row, fields joined by DELIMITER.

Every partner parses its file byte for byte, so an exporter's output must
never change. Each row is a dict with "name" (str), "amount_cents" (int,
may be negative), "currency" (str), "date" (datetime.date) and "note" (str).
"""
DELIMITER = @@DELIMITER@@
'''

STARTER_EXPORTER = '''

def export_@@KIND@@(rows):
    """The @@KIND@@ file."""
    lines = @@HEADER@@
    for row in rows:
        name = @@NAME@@
        cents = row["amount_cents"]
        whole, rest = divmod(abs(cents), 100)
        amount = @@AMOUNT@@
        if cents < 0:
            amount = @@NEGATIVE@@
@@ZERO@@        currency = @@CURRENCY@@
        day = row["date"].strftime(@@DATE@@)
@@NOTE@@        lines.append(DELIMITER.join([@@COLUMNS@@]))
    return "\\n".join(lines) + "\\n"
'''

QUOTE_NOTE = '''        note = row["note"]
        if DELIMITER in note or '"' in note or "\\n" in note:
            note = '"' + note.replace('"', '""') + '"'
'''
BACKSLASH_NOTE = '''        note = row["note"].replace("\\\\", "\\\\\\\\").replace(DELIMITER, "\\\\" + DELIMITER).replace("\\n", "\\\\n")
'''

ROWS = '''
"""The one row formatter every exporter shares (see exporters.py for the row shape)."""
DELIMITER = @@DELIMITER@@


def _amount(cents, negative, thousands, zero_dash):
    if zero_dash and cents == 0:
        return "-"
    whole, rest = divmod(abs(cents), 100)
    amount = f"{whole:,}.{rest:02d}" if thousands else f"{whole}.{rest:02d}"
    if cents < 0:
        return f"({amount})" if negative == "parens" else "-" + amount
    return amount


def _note(note, escape):
    if escape == "backslash":
        return note.replace("\\\\", "\\\\\\\\").replace(DELIMITER, "\\\\" + DELIMITER).replace("\\n", "\\\\n")
    if DELIMITER in note or '"' in note or "\\n" in note:
        return '"' + note.replace('"', '""') + '"'
    return note


def format_row(row, *, columns, negative="minus", collapse_names=True, raw_currency=False,
               date_format="%Y-%m-%d", escape="quote", thousands=False, zero_dash=False):
    """One row's line: `columns` in order, each field formatted as the options say."""
    fields = {
        "name": " ".join(row["name"].split()) if collapse_names else row["name"].strip(),
        "amount": _amount(row["amount_cents"], negative, thousands, zero_dash),
        "currency": row["currency"] if raw_currency else row["currency"].upper(),
        "date": row["date"].strftime(date_format),
        "note": _note(row["note"], escape),
    }
    return DELIMITER.join(fields[column] for column in columns)
'''

SOLUTION_EXPORTER = '''

def export_@@KIND@@(rows):
    """The @@KIND@@ file."""
    lines = @@HEADER@@
    lines += [format_row(row, columns=@@COLUMNS@@@@OPTIONS@@) for row in rows]
    return "\\n".join(lines) + "\\n"
'''

HIDDEN = '''
import ast
import datetime
import random
import unittest
from pathlib import Path

from @@PKG@@ import exporters

@@REFERENCE@@
STARTER = load_reference(REFERENCE_SOURCE)
DELIMITER = @@DELIMITER@@
KINDS = @@KINDS@@
BANNED = @@BANNED@@


def row(name, cents, currency, day, note):
    return {"name": name, "amount_cents": cents, "currency": currency, "date": day, "note": note}


EDGE_ROWS = [
    row("Ada Lovelace", 1250, "EUR", datetime.date(2026, 1, 5), "first"),
    row("  Grace   Hopper ", -1250, "usd", datetime.date(2025, 12, 31), "refund, partial"),
    row("Alan\\tTuring", 0, "Gbp", datetime.date(2024, 2, 29), 'said "hi"'),
    row("Edsger  W.  Dijkstra", 123456789, "chf", datetime.date(2026, 7, 4), "line one\\nline two"),
    row("Barbara Liskov", -1, "JPY", datetime.date(2023, 3, 9), "back\\\\slash" + DELIMITER + "delim"),
    row(" ", 99, "eur", datetime.date(2026, 10, 1), ""),
    row("Ken Thompson", -100000, "nok", datetime.date(2022, 11, 30), DELIMITER),
]


def generated_rows(seed, count):
    rng = random.Random(seed)
    words = ["ada", "Grace", "ALAN", "Edsger", "ken", "Liskov", "von", "de la"]
    spaces = [" ", "  ", "\\t", " \\t "]
    notes = ["", "ok", "a" + DELIMITER + "b", 'x "y"', "multi\\nline", "back\\\\", "plain text", DELIMITER * 2]
    rows = []
    for _ in range(count):
        name = rng.choice(["", " "]) + rng.choice(spaces).join(rng.sample(words, rng.randint(1, 3))) + rng.choice(
            ["", " ", "  "])
        cents = rng.choice([0, 1, -1, 99, -99, 100, -100, rng.randint(-10 ** 9, 10 ** 9), rng.randint(-5000, 5000)])
        currency = rng.choice(["EUR", "eur", "Usd", "gbp", "CHF"])
        day = datetime.date(2020, 1, 1) + datetime.timedelta(days=rng.randint(0, 3000))
        rows.append(row(name, cents, currency, day, rng.choice(notes) + rng.choice(["", " ", "z"])))
    return rows


class HiddenExporterChecks(unittest.TestCase):
    def assert_unchanged(self, kind):
        name = f"export_{kind}"
        for rows in [EDGE_ROWS, [], *(generated_rows(seed, 80) for seed in range(4))]:
            self.assertEqual(getattr(exporters, name)(list(rows)), STARTER[name](list(rows)), name)
@@KIND_CHECKS@@
    def test_every_exporter_goes_through_format_row(self):
        from @@PKG@@.rows import format_row

        self.assertTrue(callable(format_row))
        tree = ast.parse(Path(exporters.__file__).read_text(encoding="utf-8"))
        functions = {node.name: node for node in tree.body if isinstance(node, ast.FunctionDef)}
        for kind in KINDS:
            calls = [node for node in ast.walk(functions[f"export_{kind}"]) if isinstance(node, ast.Call)]
            names = {getattr(call.func, "id", None) or getattr(call.func, "attr", None) for call in calls}
            self.assertIn("format_row", names, kind)

    def test_exporters_format_no_field_themselves(self):
        source = Path(exporters.__file__).read_text(encoding="utf-8")
        for word in BANNED:
            self.assertNotIn(word, source, word)
'''

KIND_CHECK = '''
    def test_@@KIND@@_output_unchanged(self):
        self.assert_unchanged("@@KIND@@")
'''

VISIBLE_TESTS = '''
import datetime
import unittest

from @@PKG@@ import exporters

ROW = {"name": "Ada Lovelace", "amount_cents": 1250, "currency": "EUR", "date": datetime.date(2026, 1, 5),
       "note": "first"}


class ExporterTests(unittest.TestCase):
@@CASES@@

if __name__ == "__main__":
    unittest.main()
'''

PROMPTS = (
    "`@@PKG@@/exporters.py` has @@COUNT@@ exporters with near-identical copies of the row-formatting code, "
    "and every fix has to be made @@COUNT@@ times. Consolidate it: put one shared `format_row` function in a "
    "new module `@@PKG@@/rows.py` and make every exporter build its lines through it. Afterwards "
    "`@@PKG@@/exporters.py` must not format any field itself: none of the words @@BANNED@@ may remain in "
    "that file. Partners parse these files byte for byte, so each exporter's output must stay exactly what "
    "it is today for every possible input, including wherever the exporters currently differ from each other.",
    "Refactor `@@PKG@@/exporters.py`: its @@COUNT@@ exporters each carry their own copy of the row formatting. "
    "Move the formatting into one `format_row` in `@@PKG@@/rows.py` that every exporter calls, so that "
    "`@@PKG@@/exporters.py` no longer contains any of @@BANNED@@. This is a pure refactor: the bytes every "
    "exporter returns must not change for any input, quirks included; partners depend on them.",
)


def _styles(rng):
    """Per exporter: its columns and the options where its formatting drifted from the others."""
    kinds = sorted(rng.sample(KINDS, rng.choice([3, 4])))
    delimiter = rng.choice([",", ";", "|", "\t"])
    pool = [quirk for quirk in QUIRKS if not (quirk == "thousands" and delimiter == ",")]
    quirks = rng.sample(pool, rng.choice([3, 4]))
    styles = {kind: {"columns": [], "options": {}, "header": False} for kind in kinds}
    for quirk in quirks:
        style = styles[rng.choice(kinds)]
        clash = quirk.startswith("date_") and "date_format" in style["options"]
        if quirk == "header":
            style["header"] = True
        elif not clash:
            style["options"].update(QUIRK_OPTIONS[quirk])
    for style in styles.values():
        style["columns"] = rng.sample(FIELDS, rng.choice([4, 5]))
    assert len(styles) in (3, 4) and all(len(style["columns"]) >= 4 for style in styles.values())
    return delimiter, styles


def _starter_exporter(kind, style):
    options = {**CANONICAL, **style["options"]}
    header = f'[DELIMITER.join({style["columns"]!r})]' if style["header"] else "[]"
    name = 'row["name"].strip()' if not options["collapse_names"] else '" ".join(row["name"].split())'
    amount = 'f"{whole:,}.{rest:02d}"' if options["thousands"] else 'f"{whole}.{rest:02d}"'
    negative = 'f"({amount})"' if options["negative"] == "parens" else '"-" + amount'
    zero = '        if cents == 0:\n            amount = "-"\n' if options["zero_dash"] else ""
    currency = 'row["currency"]' if options["raw_currency"] else 'row["currency"].upper()'
    note = BACKSLASH_NOTE if options["escape"] == "backslash" else QUOTE_NOTE
    columns = ", ".join({"date": "day"}.get(column, column) for column in style["columns"])
    return fill(STARTER_EXPORTER, dedent=False, KIND=kind, HEADER=header, NAME=name, AMOUNT=amount,
                NEGATIVE=negative, ZERO=zero, CURRENCY=currency, DATE=repr(options["date_format"]), NOTE=note,
                COLUMNS=columns)


def _solution_exporter(kind, style):
    header = f'[DELIMITER.join({style["columns"]!r})]' if style["header"] else "[]"
    options = "".join(f", {key}={value!r}" for key, value in sorted(style["options"].items()))
    return fill(SOLUTION_EXPORTER, dedent=False, KIND=kind, HEADER=header, COLUMNS=repr(tuple(style["columns"])),
                OPTIONS=options)


def sources(delimiter, styles):
    """(starter exporters.py, solution exporters.py, solution rows.py) for one seed's styles."""
    head = fill(STARTER_HEAD, DELIMITER=repr(delimiter))
    starter = head + "".join(_starter_exporter(kind, style) for kind, style in styles.items())
    solution_head = head.replace(f"DELIMITER = {delimiter!r}\n", "from .rows import DELIMITER, format_row\n")
    solution = solution_head + "".join(_solution_exporter(kind, style) for kind, style in styles.items())
    assert solution != starter and "format_row" not in starter, "the starter has no shared formatter yet"
    assert not any(word in solution for word in BANNED), "the reference exporters format no field themselves"
    return starter, solution, fill(ROWS, DELIMITER=repr(delimiter))


def _visible_cases(starter, kinds):
    namespace = {"__name__": "release_bench_starter"}
    exec(compile(starter, "exporters", "exec"), namespace)
    row = {"name": "Ada Lovelace", "amount_cents": 1250, "currency": "EUR", "date": datetime.date(2026, 1, 5),
           "note": "first"}
    cases = [f"    def test_{kind}_simple_row(self):\n"
             f"        self.assertEqual(exporters.export_{kind}([ROW]), {namespace[f'export_{kind}']([row])!r})\n"
             for kind in kinds]
    return "\n".join(cases)


def generate(seed, rng):
    pkg = package_name(rng, "exports")
    delimiter, styles = _styles(rng)
    starter, solution, rows = sources(delimiter, styles)
    kinds = list(styles)
    banned = ", ".join(f"`{word}`" for word in BANNED)
    hidden = fill(HIDDEN, PKG=pkg, REFERENCE=reference_block(starter), DELIMITER=repr(delimiter), KINDS=repr(kinds),
                  BANNED=repr(BANNED), KIND_CHECKS="".join(fill(KIND_CHECK, dedent=False, KIND=kind) for kind in kinds))
    files = {"README.md": f"# {pkg}\n\nPartner export files.\n", f"{pkg}/__init__.py": "",
             f"{pkg}/exporters.py": starter, "tests/__init__.py": "",
             "tests/test_exporters.py": fill(VISIBLE_TESTS, PKG=pkg, CASES=_visible_cases(starter, kinds))}
    prompt = fill(rng.choice(PROMPTS), PKG=pkg, COUNT=len(kinds), BANNED=banned)
    meta = {"exporters": len(kinds), "quirks": sum(len(s["options"]) + s["header"] for s in styles.values())}
    return make_spec(family="refactor", template="exporters", seed=seed, prompt=prompt, files=files, hidden=hidden,
                     solution={f"{pkg}/exporters.py": solution, f"{pkg}/rows.py": rows}, meta=meta)
