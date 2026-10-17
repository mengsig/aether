"""traceback / ledger: a nightly import crashed; the log shows one crash, the same file hides more defects.

The header crash in the log has its root cause in a different module than the
frame that raised, and the sample file also exercises silent defects (wrong
cents, swapped dates, wrong line numbers) that only docs/import.md reveals.
"""
from .common import fill, make_spec, package_name, reference_block

READER = r'''
"""Splitting an exported ledger file into numbered lines and fields."""
import csv


def numbered_lines(text):
    """(line number, line) for every line that holds a header or row."""
    @@BOM@@
    @@LINES@@


def split_fields(line, delimiter):
    return next(csv.reader([line], delimiter=delimiter))
'''
BOM = {"good": 'text = text[1:] if text.startswith("\\ufeff") else text', "bom": "text = text"}
LINES = {
    "good": 'rows = []\n'
            '    for number, line in enumerate(text.split("\\n"), 1):\n'
            '        @@SKIP@@\n'
            '        rows.append((number, line))\n'
            '    return rows',
    "line_numbers": 'kept = []\n'
                    '    for line in text.split("\\n"):\n'
                    '        @@SKIP@@\n'
                    '        kept.append(line)\n'
                    '    return list(enumerate(kept, 1))',
}
SKIP = {"good": 'if not line.strip() or line.startswith("#"):\n            continue',
        "comments": "if not line.strip():\n            continue"}

DATES = r'''
"""Ledger dates."""
import re
from datetime import date

ISO = re.compile(r"(\d{4})-(\d{2})-(\d{2})")
SLASHED = re.compile(r"(\d{1,2})/(\d{1,2})/(\d{4})")


def parse_date(text):
    """A date from YYYY-MM-DD or @@SLASHED_FORM@@ text; ValueError otherwise."""
    match = ISO.fullmatch(text)
    if match:
        year, month, day = map(int, match.groups())
        return date(year, month, day)
    match = SLASHED.fullmatch(text)
    if not match:
        raise ValueError(f"not a date: {text!r}")
    first, second, year = map(int, match.groups())
    @@ORDER@@
    return date(year, month, day)
'''

AMOUNTS = r'''
"""Ledger amounts as integer cents."""
import re

SYMBOLS = "$€£"
NUMBER = re.compile(r"@@NUMBER@@(?:\.(\d{1,2}))?")


def parse_amount(text):
    """Integer cents for an amount such as "-$1,234.5"; ValueError otherwise."""
    negative = False
    @@SIGN@@
    if text[:1] and text[0] in SYMBOLS:
        text = text[1:]
    match = NUMBER.fullmatch(text)
    if not match:
        raise ValueError(f"not an amount: {text!r}")
    @@CENTS@@
    return -cents if negative else cents
'''
NUMBER = {"good": r"(\d{1,3}(?:,\d{3})+|\d+)", "thousands": r"(\d+)"}
SIGN = {"good": 'if text.startswith("(") and text.endswith(")"):\n'
                '        negative, text = True, text[1:-1]\n'
                '    elif text.startswith("-"):\n'
                '        negative, text = True, text[1:]',
        "parens": 'if text.startswith("-"):\n'
                  '        negative, text = True, text[1:]'}
CENTS = {"good": 'whole, fraction = match.groups()\n'
                 '    cents = int(whole.replace(",", "")) * 100 + int((fraction or "").ljust(2, "0"))',
         "rounding": 'cents = int(float(match.group(0).replace(",", "")) * 100)'}

IMPORTER = r'''
"""Importing an exported ledger file (docs/import.md)."""
from .amounts import parse_amount
from .dates import parse_date
from .reader import numbered_lines, split_fields

COLUMNS = ("date", "account", "amount", "currency")


def _positions(header, delimiter):
    names = [name.strip().lower() for name in split_fields(header, delimiter)]
    missing = [column for column in COLUMNS if column not in names]
    if missing:
        raise ValueError(f"missing column(s): {missing}")
    return {column: names.index(column) for column in COLUMNS}, len(names)


def _entry(number, values):
    """The entry for one row's stripped values, or the name of its first invalid column."""
    try:
        day = parse_date(values["date"])
    except ValueError:
        return None, "date"
    if not values["account"]:
        return None, "account"
    try:
        cents = parse_amount(values["amount"])
    except ValueError:
        return None, "amount"
    currency = values["currency"].upper()
    if len(currency) != 3 or not currency.isascii() or not currency.isalpha():
        return None, "currency"
    return {"line": number, "date": day.isoformat(), "account": values["account"], "amount_cents": cents,
            "currency": currency}, None


def import_ledger(text):
    """{"entries": [...], "rejected": [(line, reason), ...]} for one exported file."""
    rows = numbered_lines(text)
    if not rows:
        raise ValueError("the file has no header")
    header = rows[0][1]
    delimiter = @@DELIMITER@@
    positions, width = _positions(header, delimiter)
    entries, rejected = [], []
    for number, line in rows[1:]:
        fields = split_fields(line, delimiter)
        @@WIDTH@@
        values = {column: fields[index].strip() for column, index in positions.items()}
        entry, problem = _entry(number, values)
        if problem:
            rejected.append((number, problem))
        else:
            entries.append(entry)
    return {"entries": entries, "rejected": rejected}
'''
DELIMITER = {"good": '";" if ";" in header else ","', "semicolon": '","'}
WIDTH = {"good": 'if len(fields) != width:\n            rejected.append((number, "columns"))\n            continue',
         "short_rows": 'if len(fields) < len(positions):\n            raise ValueError(f"line {number}: too few fields")'}

DOC = r'''
# Ledger import

`import_ledger(text)` reads one exported ledger file (as text) and returns
`{"entries": [...], "rejected": [...]}`.

## File format

- A leading byte-order mark (U+FEFF) is ignored. Lines end in `\n` or `\r\n`.
- Blank lines and lines starting with `#` are skipped, but they still count
  when numbering lines.
- The first line that is not skipped is the header. Its delimiter is `;` if
  the header contains a `;`, otherwise `,`; every row uses the header's
  delimiter. Fields may be double-quoted as in CSV (`""` inside quotes is one
  `"`), which is how an amount like `"1,234.50"` fits in a `,` file.
- Header names ignore letter case and surrounding spaces. The columns `date`,
  `account`, `amount` and `currency` may come in any order; other columns are
  ignored. A missing column, or a file without a header, raises `ValueError`.

## Fields

Values are stripped of surrounding spaces first.

- `date`: `YYYY-MM-DD` or @@SLASHED_DOC@@. A date that does not exist is invalid.
- `account`: any non-empty text.
- `amount`: digits with an optional `.` and one or two decimals, optionally
  with `,` thousands separators (groups of exactly three digits) before the
  point. A currency sign `$`, `€` or `£` may come first. A negative amount has
  a `-` before everything (`-$12.50`) or is wrapped in parentheses
  (`($12.50)`). The result is exact integer cents: `-$1,234.5` is `-123450`.
- `currency`: three ASCII letters, in any case; stored in capitals.

## Result

- `entries`: one dict per valid row, in file order:
  `{"line", "date" (YYYY-MM-DD text), "account", "amount_cents", "currency"}`.
- `rejected`: one `(line, reason)` pair per invalid row, in file order. A row
  whose number of fields differs from the header's has reason `"columns"`;
  otherwise the reason is the first invalid column in the order `date`,
  `account`, `amount`, `currency`.
- `line` is the row's 1-based line number in the file, counting every line.
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.importer import import_ledger


class ImportTests(unittest.TestCase):
    def test_simple_file(self):
        result = import_ledger("date,account,amount,currency\\n2026-03-01,rent,-950.00,eur\\n")
        self.assertEqual(result["entries"], [{"line": 2, "date": "2026-03-01", "account": "rent",
                                              "amount_cents": -95000, "currency": "EUR"}])
        self.assertEqual(result["rejected"], [])

    def test_bad_date_is_rejected(self):
        result = import_ledger("date,account,amount,currency\\n2026-02-30,rent,1,EUR\\n")
        self.assertEqual(result["rejected"], [(2, "date")])


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = r'''
import random
import unittest

from @@PKG@@.importer import import_ledger

@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)
SAMPLE = @@SAMPLE@@
COLUMNS = ["date", "account", "amount", "currency"]


def normalized(result):
    if result[0] != "value":
        return result
    found = result[1]
    return ("value", [dict(entry) for entry in found["entries"]], [tuple(pair) for pair in found["rejected"]])


def ledger(rows, columns=COLUMNS, delimiter=",", prefix="", newline="\n"):
    lines = [delimiter.join(columns)] + [delimiter.join(row) for row in rows]
    return prefix + newline.join(lines) + newline


class HiddenLedgerChecks(unittest.TestCase):
    def assert_like_reference(self, texts):
        for text in texts:
            expected = normalized(outcome(REFERENCE["import_ledger"], text))
            self.assertEqual(normalized(outcome(import_ledger, text)), expected, repr(text[:300]))

    def test_the_failed_nightly_file_imports(self):
        self.assert_like_reference([SAMPLE])

    def test_byte_order_mark(self):
        self.assert_like_reference([ledger([["2026-01-05", "cash", "4.00", "EUR"]], prefix="\ufeff"),
                                    ledger([["cash", "4.00", "EUR", "2026-01-05"]], prefix="\ufeff",
                                           columns=["account", "amount", "currency", "date"])])

    def test_semicolon_files(self):
        self.assert_like_reference([ledger([["2026-01-05", "cash", "1,234.50", "EUR"]], delimiter=";"),
                                    ledger([["2026-01-05", '"a;b"', "12", "usd"]], delimiter=";"),
                                    ledger([["2026-01-05", "cash", '"1,234.50"', "EUR"]])])

    def test_skipped_lines_still_count(self):
        rows = "date,account,amount,currency\r\n\r\n# note\r\n2026-01-05,a,1,EUR\r\n#x\r\n2026-13-01,b,1,EUR\r\n"
        self.assert_like_reference([rows, "# exported\n\n" + rows, "\n\n# a\n" + ledger([["x", "a", "1", "EUR"]])])

    def test_amount_forms(self):
        forms = ["12", "12.5", "12.50", "-12.50", "(12.50)", "($12.50)", "-$1,234.5", "€0.05", "£1,000,000",
                 "1,23", "12.345", "1,2345.00", "$-5", "--5", "(5", "", ".5", "1.", "12,345.67"]
        self.assert_like_reference([ledger([["2026-01-05", "a", f'"{form}"', "EUR"] for form in forms])])

    def test_exact_cents(self):
        amounts = [f"{whole}.{cents:02d}" for whole in (0, 1, 4, 19, 1234) for cents in range(0, 100, 7)]
        self.assert_like_reference([ledger([["2026-01-05", "a", amount, "EUR"] for amount in amounts])])

    def test_dates(self):
        dates = ["2026-01-05", "05/01/2026", "5/1/2026", "13/01/2026", "01/13/2026", "31/12/2026", "12/31/2026",
                 "29/02/2028", "02/29/2028", "29/02/2026", "2026-1-5", "2026/01/05", "05-01-2026", "1/1/26"]
        self.assert_like_reference([ledger([[day, "a", "1", "EUR"] for day in dates])])

    def test_rejection_reasons(self):
        rows = [["2026-01-05", "", "1", "EUR"], ["2026-01-05", "a", "x", "EUR"], ["2026-01-05", "a", "1", "EU"],
                ["bad", "", "x", "E"], ["2026-01-05", "a", "1", "eur"], ["2026-01-05", " a ", " 1 ", " EUR "],
                ["2026-01-05", "a", "1", "EÜR"]]
        self.assert_like_reference([ledger(rows)])

    def test_rows_with_the_wrong_number_of_fields(self):
        self.assert_like_reference([ledger([["2026-01-05", "a", "1"], ["2026-01-05", "a", "1", "EUR", "x"],
                                            ["2026-01-05", "a", "1", "EUR"]]),
                                    ledger([["2026-01-05", "a", "1", "EUR", "memo"], ["2026-01-05", "a", "1", "EUR"]],
                                           columns=COLUMNS + ["memo"])])

    def test_headers(self):
        self.assert_like_reference([ledger([], columns=[" Date ", "ACCOUNT", "Amount", "currency"]),
                                    ledger([["2026-01-05", "a", "1"]], columns=["date", "account", "amount"]),
                                    "", "\n# only a comment\n\n"])

    def test_generated_files(self):
        rng = random.Random(@@SEED@@)
        for _ in range(150):
            self.assert_like_reference([random_file(rng)])


def random_value(rng, column):
    if column == "date":
        year, month, day = 2026 + rng.randint(0, 2), rng.randint(1, 12), rng.randint(1, 31)
        return rng.choice([f"{year}-{month:02d}-{day:02d}", f"{day}/{month}/{year}", f"{month:02d}/{day:02d}/{year}"])
    if column == "account":
        return rng.choice(["rent", "cash", "Payroll", "", "  ", "office supplies"])
    if column == "amount":
        whole = rng.choice(["0", "7", "19", "250", "1,234", "12,345,678", "1234", "12,34"])
        text = whole + rng.choice(["", ".5", ".05", ".99", ".29", ".999"])
        text = rng.choice(["", "", "$", "€"]) + text
        return rng.choice([text, text, "-" + text, f"({text})"])
    return rng.choice(["EUR", "usd", "Gbp", "EU", "EURO", "e1r"])


def random_file(rng):
    columns = rng.sample(COLUMNS, 4) + rng.choice([[], ["memo"]])
    delimiter, newline = rng.choice([",", ";"]), rng.choice(["\n", "\r\n"])
    lines = [delimiter.join(name.upper() if rng.random() < 0.2 else name for name in columns)]
    for _ in range(rng.randint(1, 8)):
        roll = rng.random()
        if roll < 0.1:
            lines.append(rng.choice(["", "# comment", "   "]))
            continue
        values = [random_value(rng, column) if column in COLUMNS else "memo" for column in columns]
        values = [f'"{value}"' if delimiter in value else value for value in values]
        lines.append(delimiter.join(values[:-1] if roll < 0.15 else values))
    prefix = rng.choice(["", "", "\ufeff", "# exported" + newline])
    return prefix + newline.join(lines) + newline
'''

PROMPTS = (
    "Last night's ledger import crashed: the log is `logs/nightly.log` and the file it failed on is "
    "`samples/nightly.csv`. Fix the `@@PKG@@` importer so that files like this one import exactly as "
    "`docs/import.md` specifies.",
    "The nightly import job is failing (see `logs/nightly.log`; the input is saved as `samples/nightly.csv`). "
    "Please find and fix the cause in `@@PKG@@`, and make sure the import result for that file is right "
    "according to `docs/import.md`, not just that it stops crashing.",
)
SILENT = ("comments", "line_numbers", "thousands", "parens", "rounding", "date_order")


def _bugs(rng):
    header = rng.choice([{"bom"}, {"semicolon"}, {"bom", "semicolon"}])
    crash = {"short_rows"} if rng.random() < 0.5 else set()
    return header | crash | set(rng.sample(SILENT, rng.randint(2, 3)))


def _pick(table, bugs):
    chosen = [name for name in table if name in bugs]
    assert len(chosen) <= 1, f"two defects for one concern: {chosen}"
    return table[chosen[0] if chosen else "good"]


def _modules(pkg, bugs, day_first):
    good_order = "day, month = first, second" if day_first else "month, day = first, second"
    bad_order = "month, day = first, second" if day_first else "day, month = first, second"
    form = "DD/MM/YYYY" if day_first else "MM/DD/YYYY"
    lines = fill(_pick(LINES, bugs), dedent=False, SKIP=_pick(SKIP, bugs))
    return {f"{pkg}/__init__.py": "",
            f"{pkg}/reader.py": fill(READER, BOM=_pick(BOM, bugs), LINES=lines),
            f"{pkg}/dates.py": fill(DATES, SLASHED_FORM=form,
                                    ORDER=bad_order if "date_order" in bugs else good_order),
            f"{pkg}/amounts.py": fill(AMOUNTS, NUMBER=_pick(NUMBER, bugs), SIGN=_pick(SIGN, bugs),
                                      CENTS=_pick(CENTS, bugs)),
            f"{pkg}/importer.py": fill(IMPORTER, DELIMITER=_pick(DELIMITER, bugs), WIDTH=_pick(WIDTH, bugs))}


def _reference_source(modules):
    """The reference package as one module: intra-package imports dropped, modules in dependency order."""
    order = ["reader.py", "dates.py", "amounts.py", "importer.py"]
    by_name = {path.rsplit("/", 1)[1]: text for path, text in modules.items()}
    return "\n\n".join("\n".join(line for line in by_name[name].splitlines() if not line.startswith("from ."))
                       for name in order)


def _sample_rows(rng, day_first):
    """Rows of the failed file: valid entries plus one row per silent defect and per rejection reason."""
    def slashed(day, month):
        return f"{day:02d}/{month:02d}/2026" if day_first else f"{month:02d}/{day:02d}/2026"
    rows = [{"date": "2026-10-01", "account": "rent", "amount": "-1,950.00", "currency": "EUR"},
            {"date": slashed(rng.randint(13, 28), rng.randint(1, 9)), "account": "consulting",
             "amount": f"{rng.randint(1, 9)},{rng.randint(100, 999)}.{rng.choice([29, 57, 99])}", "currency": "eur"},
            {"date": slashed(rng.randint(2, 9), rng.randint(10, 12)), "account": "refund",
             "amount": f"(${rng.randint(10, 99)}.{rng.choice([15, 35, 45])})", "currency": "USD"},
            {"date": "2026-10-03", "account": "coffee", "amount": f"{rng.randint(2, 9)}.{rng.choice([29, 58, 99])}",
             "currency": "EUR"},
            {"date": "2026-02-30", "account": "typo", "amount": "10", "currency": "EUR"},
            {"date": "2026-10-04", "account": "fees", "amount": "0.99", "currency": "EURO"},
            {"date": "2026-10-05", "account": "", "amount": "5", "currency": "EUR"},
            {"date": "2026-10-06", "account": "payroll", "amount": "-12,000", "currency": "EUR"}]
    rng.shuffle(rows)
    return rows


def _sample(rng, bugs, day_first):
    delimiter = ";" if "semicolon" in bugs or rng.random() < 0.3 else ","
    columns = rng.sample(["date", "account", "amount", "currency"], 4) + ["memo"]
    lines = ["# exported by ledgerd 4.2", delimiter.join(columns)]
    for index, row in enumerate(_sample_rows(rng, day_first)):
        values = [row.get(column, f"ref {index + 1}") for column in columns]
        values = [f'"{value}"' if delimiter in value else value for value in values]
        lines.append(delimiter.join(values[:-2] if index == 4 else values))
        if index in (2, 5):
            lines.append(rng.choice(["", "# reconciled above"]))
    bom = "﻿" if "bom" in bugs or rng.random() < 0.3 else ""
    newline = rng.choice(["\n", "\r\n"])
    return bom + newline.join(lines) + newline


def _line_of(source, needle):
    numbers = [number for number, line in enumerate(source.splitlines(), 1) if line.strip() == needle]
    assert len(numbers) == 1, f"{needle!r} occurs {len(numbers)} times"
    return numbers[0], needle


def _log(pkg, modules, sample):
    """The nightly job's log: the starter's real crash on the sample, as a traceback."""
    namespace = {}
    exec(compile(_reference_source(modules), "starter", "exec"), namespace)
    try:
        namespace["import_ledger"](sample)
    except ValueError as error:
        message = str(error)
    else:
        raise AssertionError("the starter must crash on the sample file")
    assert message.startswith("missing column(s)"), f"the logged crash is the header one, not {message!r}"
    importer = modules[f"{pkg}/importer.py"]
    call, raised = _line_of(importer, "positions, width = _positions(header, delimiter)"), _line_of(
        importer, 'raise ValueError(f"missing column(s): {missing}")')
    return "\n".join([
        "2026-10-12T02:00:01Z INFO  nightly-import: reading incoming/ledger-2026-10-12.csv",
        "2026-10-12T02:00:02Z INFO  nightly-import: imported 184 entries, 2 rejected",
        "2026-10-13T02:00:01Z INFO  nightly-import: reading incoming/ledger-2026-10-13.csv",
        "2026-10-13T02:00:01Z ERROR nightly-import: import failed",
        "Traceback (most recent call last):",
        '  File "/opt/ledger/jobs/nightly.py", line 41, in run',
        "    result = import_ledger(text)",
        f'  File "/opt/ledger/{pkg}/importer.py", line {call[0]}, in import_ledger',
        f"    {call[1]}",
        f'  File "/opt/ledger/{pkg}/importer.py", line {raised[0]}, in _positions',
        f"    {raised[1]}",
        f"ValueError: {message}",
        "2026-10-13T02:00:01Z INFO  nightly-import: input kept as samples/nightly.csv",
        ""])


def generate(seed, rng):
    pkg = package_name(rng, "ledger")
    bugs, day_first = _bugs(rng), rng.random() < 0.5
    starter, solution = _modules(pkg, bugs, day_first), _modules(pkg, set(), day_first)
    sample = _sample(rng, bugs, day_first)
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, SAMPLE=repr(sample),
                  REFERENCE=reference_block(_reference_source(solution)))
    files = dict(starter)
    files.update({"README.md": f"# {pkg}\n\nImports the bank's exported ledger files. Format: `docs/import.md`.\n",
                  "docs/import.md": fill(DOC, SLASHED_DOC=("`DD/MM/YYYY` (day first" if day_first else
                                                               "`MM/DD/YYYY` (month first")
                                         + "; day and month may have one or two digits)"),
                  "samples/nightly.csv": sample, "logs/nightly.log": _log(pkg, starter, sample),
                  "tests/__init__.py": "", "tests/test_import.py": fill(VISIBLE_TESTS, PKG=pkg)})
    changed = {path: text for path, text in solution.items() if files[path] != text}
    assert changed, "the starter must differ from the reference"
    return make_spec(family="traceback", template="ledger", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution=changed,
                     meta={"defects": ",".join(sorted(bugs)), "day_first": day_first})
