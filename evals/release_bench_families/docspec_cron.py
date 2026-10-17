"""docspec / cron: a cron-style schedule whose many interacting rules live in docs/schedules.md."""
from .common import fill, make_spec, package_name, reference_block

SOLUTION = '''
"""Cron-style schedules; the rules are in docs/schedules.md."""
import re
from datetime import datetime, time, timedelta

SUNDAY_SEVEN = @@SUNDAY_SEVEN@@
EITHER_DAY = @@EITHER_DAY@@
STAR_PREFIX = @@STAR_PREFIX@@
SINGLE_STEP = @@SINGLE_STEP@@
MACROS = @@MACROS@@
HORIZON = timedelta(days=3653)
MONTHS = {name: number for number, name in enumerate(
    ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"], 1)}
WEEKDAYS = {name: number for number, name in enumerate(["sun", "mon", "tue", "wed", "thu", "fri", "sat"])}
# (lowest, highest written value, highest value of the whole range, names)
FIELDS = [(0, 59, 59, {}), (0, 23, 23, {}), (1, 31, 31, {}), (1, 12, 12, MONTHS),
          (0, 7 if SUNDAY_SEVEN else 6, 6, WEEKDAYS)]
NUMBER = re.compile(r"[0-9]+")


def _value(text, field):
    low, high, _, names = field
    if text.lower() in names:
        return names[text.lower()]
    if not NUMBER.fullmatch(text) or not low <= int(text) <= high:
        raise ValueError(f"bad value {text!r}")
    return int(text)


def _item(text, field):
    base, slash, step_text = text.partition("/")
    step = 1
    if slash:
        if not NUMBER.fullmatch(step_text) or int(step_text) == 0:
            raise ValueError(f"bad step in {text!r}")
        step = int(step_text)
    if base == "*":
        start, end = field[0], field[2]
    elif "-" in base:
        first, _, last = base.partition("-")
        start, end = _value(first, field), _value(last, field)
        if start > end:
            raise ValueError(f"backwards range {text!r}")
    else:
        start = _value(base, field)
        if slash and not SINGLE_STEP:
            raise ValueError(f"a step needs a range or * in {text!r}")
        end = field[2] if slash else start
    return set(range(start, end + 1, step))


def _parse(expression):
    if not isinstance(expression, str):
        raise TypeError("a schedule is text")
    text = expression.strip()
    if text.startswith("@"):
        if text not in MACROS:
            raise ValueError(f"unknown macro {text!r}")
        text = MACROS[text]
    fields = re.split(r"[ \\t]+", text) if text else []
    if len(fields) != 5:
        raise ValueError("a schedule has five fields")
    sets = []
    for written, field in zip(fields, FIELDS):
        items = written.split(",")
        if not all(items):
            raise ValueError(f"empty list item in {written!r}")
        sets.append(set().union(*(_item(item, field) for item in items)))
    sets[4] = {0 if value == 7 else value for value in sets[4]}
    starred = [(written.startswith("*") if STAR_PREFIX else written == "*") for written in fields[2:5:2]]
    return sets, starred


def _day_matches(parsed, day):
    (_, _, days, months, weekdays), (day_star, weekday_star) = parsed
    if day.month not in months:
        return False
    by_day, by_weekday = day.day in days, (day.weekday() + 1) % 7 in weekdays
    if EITHER_DAY and not day_star and not weekday_star:
        return by_day or by_weekday
    return by_day and by_weekday


def matches(expression, moment):
    """Whether the minute of `moment` is one the schedule fires at."""
    parsed = _parse(expression)
    minutes, hours = parsed[0][0], parsed[0][1]
    return moment.minute in minutes and moment.hour in hours and _day_matches(parsed, moment.date())


def next_after(expression, after):
    """The first minute strictly after `after` the schedule fires at."""
    parsed = _parse(expression)
    if not isinstance(after, datetime):
        raise TypeError("after is a datetime")
    start = after.replace(second=0, microsecond=0) + timedelta(minutes=1)
    day = start.date()
    while datetime.combine(day, time()) - after <= HORIZON:
        if _day_matches(parsed, day):
            for hour in sorted(parsed[0][1]):
                for minute in sorted(parsed[0][0]):
                    candidate = datetime.combine(day, time(hour, minute))
                    if candidate >= start:
                        if candidate - after > HORIZON:
                            raise ValueError("the schedule never fires within the horizon")
                        return candidate
        day += timedelta(days=1)
    raise ValueError("the schedule never fires within the horizon")
'''

STARTER = '''
"""Cron-style schedules; the rules are in docs/schedules.md."""
from datetime import timedelta


def _field(text, low, high):
    values = set()
    for item in text.split(","):
        base, _, step = item.partition("/")
        step = int(step) if step else 1
        if base == "*":
            start, end = low, high
        elif "-" in base:
            start, end = (int(part) for part in base.split("-"))
        else:
            start = end = int(base)
        values.update(range(start, end + 1, step))
    return values


def _parse(expression):
    minute, hour, day, month, weekday = expression.split()
    return (_field(minute, 0, 59), _field(hour, 0, 23), _field(day, 1, 31), _field(month, 1, 12),
            _field(weekday, 0, 6))


def _fires(fields, moment):
    minutes, hours, days, months, weekdays = fields
    return (moment.minute in minutes and moment.hour in hours and moment.day in days
            and moment.month in months and moment.weekday() in weekdays)


def matches(expression, moment):
    """Whether the minute of `moment` is one the schedule fires at."""
    return _fires(_parse(expression), moment)


def next_after(expression, after):
    """The first minute strictly after `after` the schedule fires at."""
    fields = _parse(expression)
    moment = after.replace(second=0, microsecond=0)
    for _ in range(366 * 24 * 60):
        if moment > after and _fires(fields, moment):
            return moment
        moment += timedelta(minutes=1)
    raise ValueError("no match within a year")
'''

DOC = '''
# Schedules

Jobs run on cron-style schedules. `next_after(expression, after)` returns the
first minute strictly after the naive `datetime` `after` at which the schedule
fires (a `datetime` with zero seconds and microseconds), and
`matches(expression, moment)` tells whether the minute of `moment` is one of
them. Both raise `ValueError` for an invalid expression.

## Fields

An expression has exactly five fields separated by one or more spaces or tabs
(leading and trailing whitespace is ignored):

| Field | Values |
| --- | --- |
| minute | 0-59 |
| hour | 0-23 |
| day of month | 1-31 |
| month | 1-12, or `jan` ... `dec` |
| day of week | @@WEEKDAY_VALUES@@ |

Names are three letters and case-insensitive (`MON`, `Jan`) and may be used
anywhere a number may, including in ranges (`mon-fri`).

## Field syntax

A field is a comma-separated list of items; an empty item is an error. Each
item is one of:

- `*`: every value of the field (for the day of week: 0-6);
- a value: `5`;
- a range `a-b` with `a <= b`, both ends included; a backwards range such as
  `5-1` is an error;
- a range or `*` with a step, `a-b/n` or `*/n`: every n-th value starting at
  the first (`*/15` is 0, 15, 30, 45; `10-20/5` is 10, 15, 20); the step must
  be a whole number of at least 1;
- @@SINGLE_STEP_RULE@@

Numbers are plain decimal digits. A value outside its field's range, an
unknown name, or anything else is an error.

## Which days fire

A day fires when its month is in the month field and @@DAY_RULE@@

## Macros

@@MACRO_RULE@@

## Searching

`next_after` looks at most 3653 days past `after`: when the schedule does not
fire within that window (`0 0 30 2 *` never does), it raises `ValueError`. It
answers within a fraction of a second even for schedules that fire rarely or
never.
'''

VISIBLE_TESTS = '''
import unittest
from datetime import datetime

from @@PKG@@.schedule import next_after


class ScheduleTests(unittest.TestCase):
    def test_every_quarter_hour(self):
        self.assertEqual(next_after("*/15 * * * *", datetime(2026, 3, 2, 10, 7)), datetime(2026, 3, 2, 10, 15))

    def test_daily(self):
        self.assertEqual(next_after("0 9 * * *", datetime(2026, 3, 2, 10, 0)), datetime(2026, 3, 3, 9, 0))


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import random
import unittest
from datetime import datetime, timedelta

from @@PKG@@.schedule import matches, next_after

@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)
START = datetime(2026, 1, 1)


def at(text):
    return datetime.fromisoformat(text)


class HiddenScheduleChecks(unittest.TestCase):
    def assert_like_reference(self, cases):
        for expression, after in cases:
            expected = outcome(REFERENCE["next_after"], expression, at(after))
            self.assertEqual(outcome(next_after, expression, at(after)), expected, (expression, after))

@@CHECKS@@

    def test_never_firing_schedules_fail_fast(self):
        for expression in ("0 0 30 2 *", "0 0 31 4,6,9,11 *"):
            with self.assertRaises(ValueError):
                next_after(expression, at("2026-01-01T00:00"))

    def test_matches_agrees(self):
        rng = random.Random(@@SEED@@)
        for expression in ("*/7 3-5 * * *", "0 0 13 * fri", "30 12 1,15 * mon-wed", "@@MACRO_SAMPLE@@"):
            for _ in range(60):
                moment = START + timedelta(minutes=rng.randrange(0, 2 * 366 * 24 * 60))
                self.assertEqual(matches(expression, moment), REFERENCE["matches"](expression, moment),
                                 (expression, moment))

    def test_generated_schedules(self):
        rng = random.Random(@@SEED@@ + 1)
        mismatches = []
        for _ in range(150):
            expression = random_expression(rng)
            after = START + timedelta(minutes=rng.randrange(0, 3 * 366 * 24 * 60), seconds=rng.choice([0, 0, 30]))
            expected = outcome(REFERENCE["next_after"], expression, after)
            actual = outcome(next_after, expression, after)
            if expected != actual:
                mismatches.append((expression, str(after), expected, actual))
        self.assertEqual(mismatches[:4], [], f"{len(mismatches)} of 150 generated schedules differ")


def random_item(rng, low, high, names):
    def value():
        number = rng.randint(low, high)
        if names and rng.random() < 0.3:
            reverse = {v: k for k, v in names.items()}
            if number in reverse:
                return rng.choice([str.lower, str.upper, str.title])(reverse[number])
        return str(number)
    kind = rng.random()
    if kind < 0.25:
        return "*" if rng.random() < 0.5 else f"*/{rng.randint(1, 20)}"
    if kind < 0.6:
        return value()
    first, last = sorted((rng.randint(low, high), rng.randint(low, high)))
    item = f"{first}-{last}"
    return item + (f"/{rng.randint(1, 5)}" if rng.random() < 0.3 else "")


def random_expression(rng):
    if rng.random() < 0.08:
        return rng.choice(["@hourly", "@daily", "@weekly", "@monthly", "@yearly", "@annually", "@midnight"])
    limits = [(0, 59, {}), (0, 23, {}), (1, 31, {}), (1, 12, REFERENCE["MONTHS"]), (0, 7, REFERENCE["WEEKDAYS"])]
    fields = []
    for low, high, names in limits:
        count = rng.choice([1, 1, 1, 2, 3])
        fields.append(",".join(random_item(rng, low, high, names) for _ in range(count)))
    if rng.random() < 0.05:
        fields[rng.randrange(5)] = rng.choice(["5-1", "*/0", "60", "", "1,,2", "foo"])
    if rng.random() < 0.3:
        fields[0], fields[1] = str(rng.randint(0, 59)), str(rng.randint(0, 23))
    return rng.choice([" ", "  ", "\\\\t"]).join(fields)
'''

CHECKS = {
    "field_forms": [("5,10-12,*/20 * * * *", "2026-03-02T10:07"), ("5,10-12,*/20 * * * *", "2026-03-02T10:12"),
                    ("10-20/5 */6 * * *", "2026-03-02T19:21"), ("0 0-23/7 * * *", "2026-03-02T21:00")],
    "names": [("0 9 * * MON-fri", "2026-10-16T09:00"), ("0 0 1 Jan,JUL *", "2026-02-01T00:00"),
              ("0 0 * dec sun", "2026-11-01T00:00"), ("15 10 * * Wed", "2026-10-14T10:15")],
    "weekday_numbers": [("0 12 * * 0", "2026-10-14T00:00"), ("0 12 * * 1", "2026-10-14T00:00"),
                        ("0 12 * * 6", "2026-10-14T00:00"), ("0 12 * * 5-6", "2026-10-17T13:00")],
    "sunday_seven": [("0 8 * * 7", "2026-10-14T00:00"), ("0 8 * * 5-7", "2026-10-18T09:00"),
                     ("0 8 * * 6,7", "2026-10-13T00:00")],
    "day_rule": [("0 0 13 * 5", "2026-01-01T00:00"), ("0 0 1,15 * mon", "2026-02-02T01:00"),
                 ("0 0 29 2 1", "2026-03-01T00:00"), ("0 0 */2 * 1", "2026-06-01T01:00"),
                 ("0 0 1 * */3", "2026-06-02T00:00"), ("30 6 31 * sat", "2026-04-01T00:00")],
    "macros": [("@hourly", "2026-05-05T05:05"), ("@daily", "2026-05-05T05:05"), ("@weekly", "2026-05-05T05:05"),
               ("@monthly", "2026-05-05T05:05"), ("@yearly", "2026-05-05T05:05"), ("@annually", "2026-12-31T23:59"),
               ("@midnight", "2026-05-05T00:00"), ("@reboot", "2026-05-05T00:00"), ("@Daily", "2026-05-05T00:00")],
    "single_step": [("5/20 * * * *", "2026-03-02T10:07"), ("0 3/8 * * *", "2026-03-02T20:00"),
                    ("0 0 * * mon/2", "2026-10-14T00:00")],
    "invalid": [("* * * *", "2026-01-01T00:00"), ("* * * * * *", "2026-01-01T00:00"), ("60 * * * *", "2026-01-01"),
                ("* 24 * * *", "2026-01-01"), ("* * 0 * *", "2026-01-01"), ("* * 32 * *", "2026-01-01"),
                ("* * * 13 *", "2026-01-01"), ("* * * * 8", "2026-01-01"), ("5-1 * * * *", "2026-01-01"),
                ("*/0 * * * *", "2026-01-01"), ("1,,2 * * * *", "2026-01-01"), ("* * * foo *", "2026-01-01"),
                ("* * * * monday", "2026-01-01"), ("-1 * * * *", "2026-01-01"), ("", "2026-01-01"),
                ("*/x * * * *", "2026-01-01"), ("1-* * * * *", "2026-01-01")],
    "strictly_after": [("* * * * *", "2026-03-02T10:07:30"), ("7 10 * * *", "2026-03-02T10:07"),
                       ("7 10 * * *", "2026-03-02T10:06:59.999"), ("0 0 * * *", "2026-12-31T23:59:59")],
    "rollover": [("0 0 31 * *", "2026-01-31T00:00"), ("59 23 28-29 2 *", "2027-02-28T23:59"),
                 ("0 0 29 2 *", "2026-03-01T00:00"), ("0 0 1 1 *", "2026-01-01T00:00"),
                 ("0 0 * 2 *", "2026-02-28T00:00"), ("0 0 1 * *", "2026-12-15T12:00")],
}
CHECK_METHOD = '''
    def test_@@NAME@@(self):
        self.assert_like_reference(@@CASES@@)
'''

PROMPTS = (
    "`@@PKG@@/schedule.py` only handles the simplest cron expressions and gets several of them wrong. "
    "Make `next_after` and `matches` implement the schedule rules in `docs/schedules.md` completely.",
    "Our job runner fires jobs at the wrong times: the schedule module `@@PKG@@/schedule.py` does not follow "
    "`docs/schedules.md`. Fix it so both functions follow every rule there.",
)

MACRO_LINES = {"@yearly": "`0 0 1 1 *`", "@annually": "`0 0 1 1 *`", "@monthly": "`0 0 1 * *`",
               "@weekly": "`0 0 * * 0`", "@daily": "`0 0 * * *`", "@midnight": "`0 0 * * *`",
               "@hourly": "`0 * * * *`"}


def _options(rng):
    return {"sunday_seven": rng.random() < 0.5, "either_day": rng.random() < 0.6, "star_prefix": rng.random() < 0.4,
            "single_step": rng.random() < 0.5,
            "macros": sorted(rng.sample(sorted(MACRO_LINES), rng.randint(3, 6)))}


def _doc(options):
    weekdays = "0-6 (0 is Sunday), or `sun` ... `sat`"
    if options["sunday_seven"]:
        weekdays = "0-7 (0 and 7 are both Sunday), or `sun` ... `sat`"
    star = ("starts with `*` (so `*/2` counts as unrestricted too)" if options["star_prefix"]
            else "is exactly `*` (`*/2` is a restriction)")
    if options["either_day"]:
        day_rule = (f"either: when both the day-of-month and the day-of-week fields are restricted, its day "
                    f"matches one of them or both (**or**); when either field is unrestricted, its day matches "
                    f"both fields. A field is unrestricted when it {star}.")
    else:
        day_rule = "its day matches both the day-of-month field and the day-of-week field."
    single = ("a value with a step, `a/n`: every n-th value from `a` to the field's highest value "
              "(`5/20` in the minute field is 5, 25, 45)." if options["single_step"]
              else "nothing else: a step after a single value, such as `5/20`, is an error.")
    macros = "\n".join(f"- `{name}` is {MACRO_LINES[name]}" for name in options["macros"])
    macro_rule = (f"An expression may instead be one of these macros, written exactly so (lower case):\n\n{macros}\n\n"
                  "Any other expression that starts with `@` is an error.")
    return fill(DOC, WEEKDAY_VALUES=weekdays, DAY_RULE=day_rule, SINGLE_STEP_RULE=single, MACRO_RULE=macro_rule)


def generate(seed, rng):
    pkg = package_name(rng, "jobs")
    options = _options(rng)
    solution = fill(SOLUTION, SUNDAY_SEVEN=options["sunday_seven"], EITHER_DAY=options["either_day"],
                    STAR_PREFIX=options["star_prefix"], SINGLE_STEP=options["single_step"],
                    MACROS=repr({name: MACRO_LINES[name].strip("`") for name in options["macros"]}))
    checks = "".join(fill(CHECK_METHOD, dedent=False, NAME=name, CASES=repr(cases)) for name, cases in CHECKS.items())
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, REFERENCE=reference_block(solution), CHECKS=checks.rstrip("\n"),
                  MACRO_SAMPLE=options["macros"][0])
    files = {"README.md": f"# {pkg}\n\nA small job runner. Schedule rules: `docs/schedules.md`.\n",
             f"{pkg}/__init__.py": "", f"{pkg}/schedule.py": fill(STARTER), "tests/__init__.py": "",
             "tests/test_schedule.py": fill(VISIBLE_TESTS, PKG=pkg), "docs/schedules.md": _doc(options)}
    return make_spec(family="docspec", template="cron", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/schedule.py": solution},
                     meta={key: value for key, value in options.items() if key != "macros"})
