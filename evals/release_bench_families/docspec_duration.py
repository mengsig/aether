"""docspec / duration: parse and format durations by rules written only in docs/durations.md."""
from .common import fill, make_spec, package_name, reference_block

SOLUTION = '''
"""Durations as text; the rules are in docs/durations.md."""
import re
from decimal import ROUND_CEILING, ROUND_HALF_UP, Decimal

UNITS = @@UNITS@@
SPACE_INSIDE = @@SPACE_INSIDE@@
BARE_SECONDS = @@BARE@@
ROUNDING = ROUND_HALF_UP if @@HALF_UP@@ else ROUND_CEILING
LIMIT = @@LIMIT@@
SEPARATOR = @@SEPARATOR@@
PART = re.compile(r"(\\d+(?:\\.\\d+)?)" + (r"[ \\t]*" if SPACE_INSIDE else "") + r"([A-Za-z]+)")
NUMBER = re.compile(r"\\d+(?:\\.\\d+)?")


def _parts(body):
    names = [name for name, _ in UNITS]
    total, position, last = Decimal(0), 0, -1
    while position < len(body):
        match = PART.match(body, position)
        if match is None:
            raise ValueError(f"bad duration text at {body[position:]!r}")
        unit = match.group(2).lower()
        if unit not in names:
            raise ValueError(f"unknown unit {unit!r}")
        index = names.index(unit)
        if index <= last:
            raise ValueError("units must appear once each, largest first")
        last = index
        total += Decimal(match.group(1)) * UNITS[index][1]
        position = match.end()
        while position < len(body) and body[position] in " \\t":
            position += 1
    return total


def parse_duration(text):
    """Seconds in `text`, an int (see docs/durations.md)."""
    body = text.strip()
    sign = 1
    if body[:1] in ("+", "-"):
        sign, body = (-1 if body[0] == "-" else 1), body[1:]
    if not body or body[0] in " \\t":
        raise ValueError("empty duration")
    if NUMBER.fullmatch(body):
        if not BARE_SECONDS:
            raise ValueError("a number needs a unit")
        total = Decimal(body)
    else:
        total = _parts(body)
    seconds = sign * int(total.to_integral_value(rounding=ROUNDING))
    if abs(seconds) > LIMIT:
        raise ValueError("duration out of range")
    return seconds


def format_duration(seconds):
    """Canonical text for a whole number of seconds (see docs/durations.md)."""
    if seconds == 0:
        return "0s"
    if seconds < 0:
        return "-" + format_duration(-seconds)
    parts = []
    for name, size in UNITS:
        count, seconds = divmod(seconds, size)
        if count:
            parts.append(f"{count}{name}")
    return SEPARATOR.join(parts)
'''

STARTER = '''
"""Durations as text; the rules are in docs/durations.md."""

UNITS = {"h": 3600, "m": 60, "s": 1}


def parse_duration(text):
    """Seconds in `text`, an int (see docs/durations.md)."""
    text = text.strip()
    return int(text[:-1]) * UNITS[text[-1]]


def format_duration(seconds):
    """Canonical text for a whole number of seconds (see docs/durations.md)."""
    return f"{seconds}s"
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.durations import format_duration, parse_duration


class DurationTests(unittest.TestCase):
    def test_single_units(self):
        self.assertEqual(parse_duration("90s"), 90)
        self.assertEqual(parse_duration("2h"), 7200)

    def test_format_seconds(self):
        self.assertEqual(format_duration(45), "45s")


if __name__ == "__main__":
    unittest.main()
'''

DOC = '''
# Durations

Every duration a user types (timeouts, retention, schedules) goes through
`parse_duration(text)`, which returns a whole number of seconds as an `int`,
and is printed with `format_duration(seconds)`.

## Units

@@UNIT_TABLE@@

Unit letters are case-insensitive (`1H` is `1h`).

## Text form

- A duration is one or more parts, each a number followed by a unit: `1h30m`.
  Parts may be separated by spaces or tabs: `2d 4h`.
- @@SPACE_RULE@@
- Units must appear largest first and at most once: `30m1h` and `1h1h` are errors.
- A number may have a fractional part (`1.5h`). The total is computed exactly
  and then made whole: @@ROUND_RULE@@
- A leading `-` makes the whole duration negative and a leading `+` is allowed;
  no space may follow the sign. The magnitude is made whole before the sign is applied,
  so `-1.2s` is @@NEG_EXAMPLE@@.
- @@BARE_RULE@@
- Leading and trailing whitespace is ignored. Empty text, a sign alone, or
  anything else that does not follow these rules raises `ValueError`.
- A duration whose magnitude exceeds @@LIMIT@@ seconds is an error (`ValueError`).

## Canonical form

`format_duration(seconds)` writes the largest whole units first and leaves out
units with a zero count@@SEPARATOR_RULE@@: `5400` is `@@EXAMPLE_5400@@`. Zero is
`0s`, and a negative duration is `-` followed by the form of its magnitude.
@@MONTH_RULE@@Formatting then parsing gives back the same number of seconds.
'''

PROMPTS = (
    "Implement `parse_duration` and `format_duration` in `@@PKG@@/durations.py` so they follow "
    "the duration rules in `docs/durations.md`; the current code only handles a single unit.",
    "Users report that our `--timeout` and `--keep` options reject durations they expect to work. "
    "`@@PKG@@/durations.py` only understands a single unit like `90s`. Finish the module.",
)

HIDDEN = '''
import random
import unittest

from @@PKG@@.durations import format_duration, parse_duration

@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)
UNITS, LIMIT = REFERENCE["UNITS"], REFERENCE["LIMIT"]


def random_case(rng, text):
    return "".join(char.upper() if rng.random() < 0.3 else char for char in text)


def random_number(rng):
    return str(rng.randint(0, 150)) if rng.random() < 0.7 else f"{rng.randint(0, 20)}.{rng.randint(0, 99)}"


def random_duration(rng):
    """Text built only from documented features: parts, order, case, spacing, signs, bare numbers, the limit."""
    if rng.random() < 0.08:
        name, size = rng.choice(UNITS)
        return f"{LIMIT // size + rng.randint(0, 1)}{name}"
    if rng.random() < 0.08:
        body = random_number(rng)
    else:
        indexes = sorted(rng.sample(range(len(UNITS)), rng.choice([1, 2, 2, 3])))
        if rng.random() < 0.15:
            rng.shuffle(indexes)
        if rng.random() < 0.1:
            indexes.append(indexes[-1])
        parts = [random_number(rng) + rng.choice(["", "", "", " ", "\\t"]) + random_case(rng, UNITS[i][0])
                 for i in indexes]
        body = parts[0] + "".join(rng.choice(["", "", " ", "\\t ", "  "]) + part for part in parts[1:])
    pad = rng.choice(["", "", " ", "\\t"])
    return pad + rng.choice(["", "", "", "-", "+", "- ", "--", "+-"]) + body + pad


class HiddenDurationChecks(unittest.TestCase):
    def test_generated_text_follows_every_rule(self):
        rng = random.Random(@@SEED@@)
        mismatches = []
        for _ in range(500):
            text = random_duration(rng)
            expected, actual = outcome(REFERENCE["parse_duration"], text), outcome(parse_duration, text)
            if expected != actual:
                mismatches.append((text, expected, actual))
        self.assertEqual(mismatches[:5], [], f"{len(mismatches)} of 500 generated durations differ")

    def test_generated_seconds_format_canonically(self):
        rng = random.Random(@@SEED@@ + 1)
        for _ in range(300):
            seconds = rng.choice([rng.randint(-10 ** 6, 10 ** 6), rng.randint(-LIMIT, LIMIT), rng.randint(-120, 120)])
            self.assertEqual(format_duration(seconds), REFERENCE["format_duration"](seconds), seconds)

    def test_compound_durations(self):
        self.assertEqual(parse_duration("1h30m"), 5400)
        self.assertEqual(parse_duration("2d 4h"), 187200)
        self.assertEqual(parse_duration(" 1w\\t2d10s "), 777610)

    def test_units_are_case_insensitive(self):
        self.assertEqual(parse_duration("1H30M"), 5400)
        self.assertEqual(parse_duration("3D"), 259200)

    def test_order_repeats_and_junk_are_rejected(self):
        for text in ("30m1h", "1h1h", "1x", "h", "1h junk", "1.h", "1h-2m"):
            with self.assertRaises(ValueError, msg=text):
                parse_duration(text)

    def test_fractions_are_made_whole(self):
        self.assertEqual(parse_duration("1.5m"), 90)
        self.assertEqual(parse_duration("1.2s"), @@ONE_POINT_TWO@@)
        self.assertEqual(parse_duration("2.5s"), 3)
        self.assertEqual(parse_duration("0.25h"), 900)

    def test_signs(self):
        self.assertEqual(parse_duration("-1h"), -3600)
        self.assertEqual(parse_duration("+90s"), 90)
        self.assertEqual(parse_duration("-1.2s"), @@NEG_ONE_POINT_TWO@@)
        for text in ("- 1h", "-", "+", "--1h"):
            with self.assertRaises(ValueError, msg=text):
                parse_duration(text)

    def test_numbers_without_units(self):
@@BARE_CHECK@@

    def test_space_between_number_and_unit(self):
@@SPACE_CHECK@@

    def test_empty_text(self):
        for text in ("", "   "):
            with self.assertRaises(ValueError):
                parse_duration(text)

    def test_limit(self):
        self.assertEqual(parse_duration(f"{@@LIMIT@@}s"), @@LIMIT@@)
        self.assertEqual(parse_duration(f"-{@@LIMIT@@}s"), -@@LIMIT@@)
        for text in (f"{@@LIMIT@@ + 1}s", f"-{@@LIMIT@@ + 1}s"):
            with self.assertRaises(ValueError):
                parse_duration(text)

    def test_canonical_form(self):
        self.assertEqual(format_duration(5400), "@@EXAMPLE_5400@@")
        self.assertEqual(format_duration(0), "0s")
        self.assertEqual(format_duration(-90061), "@@EXAMPLE_90061@@")
        self.assertEqual(format_duration(691200), "@@EXAMPLE_691200@@")

    def test_round_trip(self):
        for seconds in (1, 59, 61, 3599, 86399, 90061, 1209600, 5000000, -7322):
            self.assertEqual(parse_duration(format_duration(seconds)), seconds)
@@MONTH_CHECK@@'''

MONTH_CHECK = '''
    def test_months(self):
        self.assertEqual(parse_duration("1mo"), 2592000)
        self.assertEqual(parse_duration("1MO2w"), 3801600)
        self.assertEqual(format_duration(2592000 + 61), "@@EXAMPLE_MONTH@@")
        with self.assertRaises(ValueError):
            parse_duration("1w1mo")
'''


def _units(month):
    units = [("w", 604800), ("d", 86400), ("h", 3600), ("m", 60), ("s", 1)]
    return [("mo", 2592000)] + units if month else units


def _unit_table(units):
    rows = ["| Unit | Meaning | Seconds |", "| --- | --- | ---: |"]
    words = {"mo": "month (always 30 days)", "w": "week", "d": "day", "h": "hour", "m": "minute", "s": "second"}
    rows += [f"| `{name}` | {words[name]} | {size} |" for name, size in units]
    return "\n".join(rows)


def _canonical(seconds, units, separator):
    if seconds < 0:
        return "-" + _canonical(-seconds, units, separator)
    parts = []
    for name, size in units:
        count, seconds = divmod(seconds, size)
        if count:
            parts.append(f"{count}{name}")
    return separator.join(parts) or "0s"


def _check_blocks(options):
    bare = ('        self.assertEqual(parse_duration("90"), 90)\n'
            '        self.assertEqual(parse_duration("1.5"), 2)' if options["bare"] else
            '        for text in ("90", "1.5"):\n'
            '            with self.assertRaises(ValueError):\n'
            '                parse_duration(text)')
    space = ('        self.assertEqual(parse_duration("1 h 30 m"), 5400)' if options["space"] else
             '        with self.assertRaises(ValueError):\n'
             '            parse_duration("1 h")')
    return bare, space


def _docs_values(options, units, separator):
    half_up = options["half_up"]
    return {
        "UNIT_TABLE": _unit_table(units),
        "SPACE_RULE": ("Spaces or tabs may also separate a number from its unit: `1 h 30 m`." if options["space"]
                       else "Nothing may separate a number from its unit: `1 h` is an error."),
        "ROUND_RULE": ("rounded to the nearest second, halves away from zero (`2.5s` is 3 seconds, `1.2s` is 1)."
                       if half_up else "rounded up to the next whole second (`1.2s` is 2 seconds, `2.5s` is 3)."),
        "NEG_EXAMPLE": "-1" if half_up else "-2",
        "BARE_RULE": ("A number without any unit counts as seconds (`90` is 90 seconds; `1.5` is made whole too)."
                      if options["bare"] else "A number without a unit is an error: write `90s`, not `90`."),
        "LIMIT": options["limit"],
        "SEPARATOR_RULE": ", separating parts with one space" if separator == " " else ", with nothing between parts",
        "EXAMPLE_5400": _canonical(5400, units, separator),
        "MONTH_RULE": ("Months are used in the canonical form too: 30 days and a minute is "
                       f"`{_canonical(2592060, units, separator)}`. " if options["month"] else ""),
    }


def generate(seed, rng):
    pkg = package_name(rng, "durations")
    options = {"space": rng.random() < 0.5, "bare": rng.random() < 0.5, "half_up": rng.random() < 0.5,
               "month": rng.random() < 0.5, "limit": rng.choice([31536000, 315360000, 2 ** 31 - 1])}
    separator = rng.choice(["", " "])
    units = _units(options["month"])
    docs = _docs_values(options, units, separator)
    bare, space = _check_blocks(options)
    month_check = fill(MONTH_CHECK, dedent=False, EXAMPLE_MONTH=_canonical(2592061, units, separator)) if options["month"] else ""
    solution = fill(SOLUTION, UNITS=repr(units), SPACE_INSIDE=options["space"], BARE=options["bare"],
                    HALF_UP=options["half_up"], LIMIT=options["limit"], SEPARATOR=repr(separator))
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, REFERENCE=reference_block(solution), ONE_POINT_TWO=1 if options["half_up"] else 2,
                  NEG_ONE_POINT_TWO=-1 if options["half_up"] else -2, BARE_CHECK=bare, SPACE_CHECK=space,
                  LIMIT=options["limit"], EXAMPLE_5400=docs["EXAMPLE_5400"], MONTH_CHECK=month_check,
                  EXAMPLE_90061=_canonical(-90061, units, separator),
                  EXAMPLE_691200=_canonical(691200, units, separator))
    files = {"README.md": f"# {pkg}\n\nCommand-line helpers. Duration rules: `docs/durations.md`.\n",
             f"{pkg}/__init__.py": "", f"{pkg}/durations.py": fill(STARTER), "tests/__init__.py": "",
             "tests/test_durations.py": fill(VISIBLE_TESTS, PKG=pkg), "docs/durations.md": fill(DOC, **docs)}
    return make_spec(family="docspec", template="duration", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/durations.py": solution})
