"""numeric / prorate: billing amounts in integer minor units, split, prorated and refunded without losing a unit.

Per seed: the currency's minor digits (0, 2 or 3), the rounding mode (half-even or
half-up), whether a day range counts its end day, and how allocation ties break.
The starter works on the easy cases; the doc spells out what it gets wrong.
"""
from .common import fill, make_spec, package_name

CURRENCIES = {"JPY": 0, "USD": 2, "KWD": 3}
MODES = {"half_even": "ties go to the even neighbour (2.5 becomes 2, 3.5 becomes 4)",
         "half_up": "ties go away from zero (2.5 becomes 3, -2.5 becomes -3)"}
BOUNDARY = {True: "inclusive: both the start day and the end day count, so 1 Jan to 1 Jan is one day.",
            False: "end-exclusive: the start day counts and the end day does not, so 1 Jan to 1 Jan is zero days."}
TIE_RULES = {"index": "the extra unit goes to the line that comes first in the list",
             "weight": "the extra unit goes to the line with the larger weight, and between equal weights to the earlier line"}

DOC = '''
# Billing amounts (@@CODE@@)

Every amount is an `int` count of minor units (@@DIGITS@@ decimal digits, so 1 major unit is @@UNIT@@ minor units).
No function may use floats: results are exact for integers of any size.

## Rounding (`currency.py`)
- `round_div(n, d)` is n / d rounded to an int; `d` must be positive (`ValueError`), both must be ints (`TypeError`).
  Mode `@@MODE@@`: @@MODE_TEXT@@.
- Rounding is symmetric: `round_div(-n, d) == -round_div(n, d)`, so a refund rounds like its charge.
- `to_minor(text)` parses a plain decimal string (`-?digits`, optionally `.digits`; nothing else, no
  spaces, no exponent, no `+`; `ValueError`; non-strings raise `TypeError`) into minor units. More fractional
  digits than @@DIGITS@@ are rounded with the same mode, applied to the exact decimal value.
- `format_minor(n)` is the inverse: no currency code, no thousands separators, always exactly
  @@DIGITS@@ fractional digits@@NO_POINT@@, and a leading `-` for negatives (including values like -5 minor units).

## Allocation (`alloc.py`)
- `allocate(amount, weights)` splits `amount` across the lines by weight with the largest-remainder method:
  each line gets the floor of its exact share, then the units that are left are handed out one each to
  the lines with the largest fractional remainder.
- When remainders are equal, @@TIE_TEXT@@.
- The shares always add up to `amount` exactly. A line with weight 0 always gets 0.
- A negative amount (a refund or credit) is split as the negative of the split of its absolute value.
- Weights are ints >= 0 (`TypeError` for non-ints including `bool`, `ValueError` for negatives). An empty list or
  all-zero weights raise `ValueError`. `amount` must be an int (`TypeError`).

## Days (`proration.py`)
- Dates are `datetime.date` (`TypeError` otherwise, `datetime` included). Day counts use the real calendar,
  leap years included (2000 and 2024 are leap years, 2100 is not).
- A range of days is @@BOUNDARY_TEXT@@
- `month_period(year, month)` is the billing period of one calendar month as `(start, end)` under that convention.
- `prorate(amount, period, start, end)` is `amount * used_days / period_days` with one single rounding
  by `round_div`. `used_days` are the days of `(start, end)` that fall inside `period`; days outside it never count.
  `start` after `end` raises `ValueError`; a period of fewer than one day raises `ValueError`.
- `split_refund(amount_paid, period, cancel_on)` returns `(charged, refund)`. Service ran from the start of
  the period up to `cancel_on` under the range convention above; `charged` is the prorated amount for that range
  and `refund` is `amount_paid - charged`, so the two always add up to `amount_paid` exactly. A `cancel_on`
  after the period counts the whole period; one before it raises `ValueError`.
- `split_by_usage(amount, period, spans)` allocates `amount` over the `(start, end)` spans with `allocate`, using
  each span's used days inside `period` as its weight. A span with no day in the period gets 0; when no span
  has a day in the period it raises `ValueError`.
'''

CURRENCY = '''
"""Money in integer minor units (docs/rules.md)."""
import re

CODE = "@@CODE@@"
DIGITS = @@DIGITS@@
MODE = "@@MODE@@"
PLAIN_DECIMAL = re.compile(r"-?[0-9]+(\\.[0-9]+)?")


def _check_ints(*values):
    for value in values:
        if type(value) is not int:
            raise TypeError(f"expected an int, got {value!r}")

@@ROUND_DIV@@

def to_minor(text):
    """Minor units of a decimal string such as "12.34"."""
    if not isinstance(text, str):
        raise TypeError(f"expected a string, got {text!r}")
@@TO_MINOR@@


def format_minor(minor):
    """Text for minor units: 1234 is "12.34" when the currency has two digits."""
    _check_ints(minor)
@@FORMAT@@
'''
ROUND_DIV = {
    "good": '''
def round_div(numerator, denominator):
    """numerator / denominator as an int, ties per MODE, symmetric around zero."""
    _check_ints(numerator, denominator)
    if denominator <= 0:
        raise ValueError("the denominator must be positive")
    quotient, remainder = divmod(abs(numerator), denominator)
    twice = 2 * remainder
    if twice > denominator or (twice == denominator and (MODE == "half_up" or quotient % 2 == 1)):
        quotient += 1
    return -quotient if numerator < 0 else quotient
''',
    "half_up": '''
def round_div(numerator, denominator):
    """numerator / denominator as an int, ties per MODE, symmetric around zero."""
    _check_ints(numerator, denominator)
    if denominator <= 0:
        raise ValueError("the denominator must be positive")
    return (2 * numerator + denominator) // (2 * denominator)
''',
    "half_even": '''
def round_div(numerator, denominator):
    """numerator / denominator as an int, ties per MODE, symmetric around zero."""
    _check_ints(numerator, denominator)
    if denominator <= 0:
        raise ValueError("the denominator must be positive")
    return round(numerator / denominator)
''',
}
TO_MINOR = {
    "good": '''    if not PLAIN_DECIMAL.fullmatch(text):
        raise ValueError(f"not a plain decimal: {text!r}")
    whole, _, fraction = text.partition(".")
    scale = 10 ** len(fraction)
    return round_div(int(whole + fraction) * 10 ** DIGITS, scale)''',
    "float": '''    return int(round(float(text) * 10 ** DIGITS))''',
}
FORMAT = {
    "good": '''    whole, fraction = divmod(abs(minor), 10 ** DIGITS)
    sign = "-" if minor < 0 else ""
    if DIGITS == 0:
        return f"{sign}{whole}"
    return f"{sign}{whole}.{fraction:0{DIGITS}d}"''',
    "float": '''    return f"{minor / 10 ** DIGITS:.{DIGITS}f}"''',
}

ALLOC = '''
"""Splitting an amount across weighted lines (docs/rules.md)."""


def _checked_weights(weights):
    weights = list(weights)
    if not weights:
        raise ValueError("no lines to allocate to")
    for weight in weights:
        if type(weight) is not int:
            raise TypeError(f"a weight is an int, got {weight!r}")
        if weight < 0:
            raise ValueError("a weight cannot be negative")
    if not any(weights):
        raise ValueError("every weight is zero")
    return weights

@@ALLOCATE@@
'''
ALLOCATE = {
    "good": '''
def allocate(amount, weights):
    """`amount` split by `weights`: largest remainder, shares add up to `amount`."""
    if type(amount) is not int:
        raise TypeError(f"an amount is an int, got {amount!r}")
    weights = _checked_weights(weights)
    total, magnitude = sum(weights), abs(amount)
    shares = [magnitude * weight // total for weight in weights]
    remainders = [magnitude * weight % total for weight in weights]
    leftover = magnitude - sum(shares)
    order = sorted(range(len(weights)), key=lambda i: (-remainders[i], @@TIE_KEY@@))
    for index in order[:leftover]:
        shares[index] += 1
    assert sum(shares) == magnitude and all(share == 0 for share, weight in zip(shares, weights) if weight == 0)
    return [-share for share in shares] if amount < 0 else shares
''',
    "naive": '''
def allocate(amount, weights):
    """`amount` split by `weights`: largest remainder, shares add up to `amount`."""
    if type(amount) is not int:
        raise TypeError(f"an amount is an int, got {amount!r}")
    weights = _checked_weights(weights)
    total = sum(weights)
    shares = [amount * weight // total for weight in weights]
    for index in range(amount - sum(shares)):
        shares[index] += 1
    return shares
''',
}

PRORATION = '''
"""Prorating by days (docs/rules.md)."""
import calendar
from datetime import date, timedelta

from .alloc import allocate
from .currency import round_div

INCLUSIVE = @@INCLUSIVE@@


def _check_dates(*values):
    for value in values:
        if type(value) is not date:
            raise TypeError(f"expected a datetime.date, got {value!r}")


def count_days(start, end):
    """Days in the range under the convention, however far apart; ValueError when start is after end."""
    _check_dates(start, end)
    if start > end:
        raise ValueError(f"{start} is after {end}")
    return (end - start).days + (1 if INCLUSIVE else 0)

@@MONTH_PERIOD@@

def _checked_period(period):
    start, end = period
    if count_days(start, end) < 1:
        raise ValueError("a period has at least one day")
    return start, end

@@USED_DAYS@@

def prorate(amount, period, start, end):
    """The share of `amount` for the days of (start, end) that fall inside `period`."""
    period_start, period_end = _checked_period(period)
    used = _used_days(period, start, end)
    assert 0 <= used <= count_days(period_start, period_end), "used days stay inside the period"
    return round_div(amount * used, count_days(period_start, period_end))

@@SPLIT_REFUND@@

@@SPLIT_BY_USAGE@@
'''
MONTH_PERIOD = {
    "good": '''
def month_period(year, month):
    """(start, end) of the calendar month under the convention."""
    first = date(year, month, 1)
    last_day = calendar.monthrange(year, month)[1]
    return first, first + timedelta(days=last_day - 1 if INCLUSIVE else last_day)
''',
    "no_leap": '''
def month_period(year, month):
    """(start, end) of the calendar month under the convention."""
    first = date(year, month, 1)
    last_day = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1]
    return first, first + timedelta(days=last_day - 1 if INCLUSIVE else last_day)
''',
}
USED_DAYS = {
    "good": '''
def _used_days(period, start, end):
    """Days of (start, end) inside `period`; ValueError when start is after end."""
    count_days(start, end)
    period_start, period_end = period
    clipped_start, clipped_end = max(start, period_start), min(end, period_end)
    if clipped_start > clipped_end:
        return 0
    return count_days(clipped_start, clipped_end)
''',
    "unclipped": '''
def _used_days(period, start, end):
    """Days of (start, end) inside `period`; ValueError when start is after end."""
    return count_days(start, end)
''',
}
SPLIT_REFUND = {
    "good": '''
def split_refund(amount_paid, period, cancel_on):
    """(charged, refund) for service up to `cancel_on`; the two add up to `amount_paid`."""
    period_start, _ = _checked_period(period)
    if cancel_on < period_start:
        raise ValueError("cancelled before the period began")
    charged = prorate(amount_paid, period, period_start, cancel_on)
    return charged, amount_paid - charged
''',
    "independent": '''
def split_refund(amount_paid, period, cancel_on):
    """(charged, refund) for service up to `cancel_on`; the two add up to `amount_paid`."""
    period_start, period_end = _checked_period(period)
    if cancel_on < period_start:
        raise ValueError("cancelled before the period began")
    total = count_days(period_start, period_end)
    used = _used_days(period, period_start, cancel_on)
    return round_div(amount_paid * used, total), round_div(amount_paid * (total - used), total)
''',
}
SPLIT_BY_USAGE = {
    "good": '''
def split_by_usage(amount, period, spans):
    """`amount` allocated over the spans by their used days inside `period`."""
    _checked_period(period)
    return allocate(amount, [_used_days(period, start, end) for start, end in spans])
''',
    "plain": '''
def split_by_usage(amount, period, spans):
    """`amount` allocated over the spans by their used days inside `period`."""
    _checked_period(period)
    return allocate(amount, [(end - start).days for start, end in spans])
''',
}

VISIBLE_TESTS = '''
import unittest
from datetime import date

from @@PKG@@.alloc import allocate
from @@PKG@@.currency import format_minor, round_div, to_minor
from @@PKG@@.proration import month_period, prorate, split_by_usage, split_refund


class BillingTests(unittest.TestCase):
    def test_round_div(self):
        self.assertEqual(round_div(7, 2) in (3, 4), True)
        self.assertEqual(round_div(9, 3), 3)
        self.assertEqual(round_div(10, 4) in (2, 3), True)

    def test_minor_units_round_trip(self):
        self.assertEqual(to_minor("@@TEXT@@"), @@MINOR@@)
        self.assertEqual(format_minor(@@MINOR@@), "@@TEXT@@")

    def test_allocate(self):
        self.assertEqual(allocate(100, [3, 2]), [60, 40])
        self.assertEqual(allocate(50, [1, 0, 1]), [25, 0, 25])

    def test_prorate_half_of_a_thirty_day_month(self):
        april = month_period(2023, 4)
        self.assertEqual(prorate(3000, april, date(2023, 4, 1), date(2023, 4, @@HALF_END@@)), 1500)

    def test_refund_in_the_middle_of_a_period(self):
        april = month_period(2023, 4)
        self.assertEqual(split_refund(3000, april, date(2023, 4, @@HALF_END@@)), (1500, 1500))

    def test_equal_spans_split_evenly(self):
        april = month_period(2023, 4)
        spans = [(date(2023, 4, 1), date(2023, 4, 10)), (date(2023, 4, 11), date(2023, 4, 20))]
        self.assertEqual(split_by_usage(100, april, spans), [50, 50])


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import calendar
import random
import unittest
from datetime import date, datetime, timedelta
from decimal import ROUND_HALF_EVEN, ROUND_HALF_UP, Decimal
from fractions import Fraction

from @@PKG@@.alloc import allocate
from @@PKG@@.currency import format_minor, round_div, to_minor
from @@PKG@@.proration import month_period, prorate, split_by_usage, split_refund

DIGITS = @@DIGITS@@
HALF_UP = @@HALF_UP@@
INCLUSIVE = @@INCLUSIVE@@
BY_WEIGHT = @@BY_WEIGHT@@


def oracle_div(numerator, denominator):
    exact = Fraction(abs(numerator), denominator)
    floor = exact.numerator // exact.denominator
    rest = exact - floor
    if rest > Fraction(1, 2) or (rest == Fraction(1, 2) and (HALF_UP or floor % 2 == 1)):
        floor += 1
    return -floor if numerator < 0 else floor


def oracle_allocate(amount, weights):
    total, magnitude = sum(weights), abs(amount)
    exact = [Fraction(magnitude * weight, total) for weight in weights]
    shares = [value.numerator // value.denominator for value in exact]
    def priority(index):
        return (-(exact[index] - shares[index]), -weights[index] if BY_WEIGHT else 0, index)
    for index in sorted(range(len(weights)), key=priority)[:magnitude - sum(shares)]:
        shares[index] += 1
    return [-share for share in shares] if amount < 0 else shares


def span_days(start, end):
    return (end - start).days + (1 if INCLUSIVE else 0)


def used_days(period, start, end):
    start, end = max(start, period[0]), min(end, period[1])
    return span_days(start, end) if start <= end else 0


class HiddenBillingChecks(unittest.TestCase):
    def test_rounding_matches_exact_arithmetic_for_any_sign_and_size(self):
        rng = random.Random(@@SEED@@)
        for numerator in (5, 15, 25, -5, -15, -25, 10 ** 18 + 5, -(10 ** 18) - 5, 2 ** 70 + 1, 0):
            self.assertEqual(round_div(numerator, 10), oracle_div(numerator, 10), numerator)
        for _ in range(400):
            numerator = rng.randint(-10 ** 19, 10 ** 19) * rng.choice([1, 1, 2, 5])
            denominator = rng.choice([1, 2, 3, 4, 7, 10, 100, rng.randint(1, 10 ** 9)])
            self.assertEqual(round_div(numerator, denominator), oracle_div(numerator, denominator))
        for denominator, bad in ((0, ValueError), (-3, ValueError)):
            with self.assertRaises(bad):
                round_div(5, denominator)
        for args in ((1.5, 2), (3, 2.0), (True, 2)):
            with self.assertRaises(TypeError, msg=repr(args)):
                round_div(*args)

    def test_to_minor_rounds_the_exact_decimal(self):
        rng = random.Random(@@SEED@@ + 1)
        mode = ROUND_HALF_UP if HALF_UP else ROUND_HALF_EVEN
        for text in ("0", "1", "-1", "0.5", "1.005", "-1.005", "2.675", "0.0005", "-0.0005", "1.0015", "7.5", "100000000000.125"):
            expected = int(Decimal(text).scaleb(DIGITS).quantize(Decimal(1), rounding=mode))
            self.assertEqual(to_minor(text), expected, text)
        for _ in range(300):
            fraction = rng.randint(0, DIGITS + 3)
            text = ("-" if rng.random() < 0.4 else "") + str(rng.randint(0, 99999))
            text += ("." + "".join(rng.choice("0123456789") for _ in range(fraction))) if fraction else ""
            expected = int(Decimal(text).scaleb(DIGITS).quantize(Decimal(1), rounding=mode))
            self.assertEqual(to_minor(text), expected, text)

    def test_to_minor_rejects_what_is_not_a_plain_decimal(self):
        for bad in ("", " 1", "1 ", "1e3", "+1", ".5", "1.", "abc", "1,5", "--1", "1.2.3"):
            with self.assertRaises(ValueError, msg=repr(bad)):
                to_minor(bad)
        for bad in (5, 1.5, None):
            with self.assertRaises(TypeError, msg=repr(bad)):
                to_minor(bad)

    def test_format_minor_is_the_exact_inverse(self):
        for minor in (0, 5, -5, 7, -7, 99, 100, -100, 1234, -1234, 10 ** 15 + 7):
            self.assertEqual(to_minor(format_minor(minor)), minor)
        self.assertEqual(format_minor(0), "0" if DIGITS == 0 else "0." + "0" * DIGITS)
        self.assertEqual(format_minor(-1), "-1" if DIGITS == 0 else "-0." + "0" * (DIGITS - 1) + "1")
        self.assertEqual(format_minor(10 ** 15 + 7).count("."), 0 if DIGITS == 0 else 1)
        self.assertEqual(format_minor(-123456789).lstrip("-").replace(".", ""), "123456789")
        self.assertEqual(len(format_minor(-123456789).lstrip("-").split(".")[-1]), 9 if DIGITS == 0 else DIGITS)

    def test_allocate_follows_largest_remainder_with_the_documented_tie_break(self):
        rng = random.Random(@@SEED@@ + 2)
        for amount, weights in ((10, [1, 2, 3]), (100, [1, 1, 1]), (7, [1, 1]), (-7, [2, 2, 2]), (5, [1, 3, 1, 3])):
            self.assertEqual(allocate(amount, weights), oracle_allocate(amount, weights), (amount, weights))
        for _ in range(500):
            weights = [rng.choice([0, 1, 1, 2, 3, 5, rng.randint(0, 40)]) for _ in range(rng.randint(1, 7))]
            if not any(weights):
                continue
            amount = rng.randint(-200, 200) if rng.random() < 0.8 else rng.randint(-10 ** 15, 10 ** 15)
            self.assertEqual(allocate(amount, weights), oracle_allocate(amount, weights), (amount, weights))

    def test_allocate_conserves_mirrors_and_validates(self):
        self.assertEqual(allocate(0, [1, 2]), [0, 0])
        for amount, weights in ((1, [1, 1, 1]), (99, [0, 5, 0, 7]), (-1, [3, 3])):
            shares = allocate(amount, weights)
            self.assertEqual(sum(shares), amount)
            self.assertEqual(allocate(-amount, weights), [-share for share in shares])
            self.assertTrue(all(share == 0 for share, weight in zip(shares, weights) if weight == 0))
        for weights in ([], [0], [0, 0], [1, -1], (-1,)):
            with self.assertRaises(ValueError, msg=repr(weights)):
                allocate(10, weights)
        for amount, weights in ((10, [1.0, 2]), (10, [True, 2]), (1.5, [1]), (True, [1])):
            with self.assertRaises(TypeError, msg=repr((amount, weights))):
                allocate(amount, weights)
        self.assertEqual(allocate(10, (w for w in [1, 4])), [2, 8])

    def test_month_periods_follow_the_real_calendar(self):
        for year in (1900, 2000, 2023, 2024, 2100):
            for month in range(1, 13):
                start, end = month_period(year, month)
                self.assertEqual(start, date(year, month, 1))
                self.assertEqual(span_days(start, end), calendar.monthrange(year, month)[1], (year, month))
        with self.assertRaises(TypeError):
            prorate(100, month_period(2024, 2), datetime(2024, 2, 1), date(2024, 2, 5))

    def test_prorate_counts_days_with_the_documented_boundary(self):
        rng = random.Random(@@SEED@@ + 3)
        for year, month in ((2024, 2), (2023, 2), (2024, 12), (2023, 8)):
            period = month_period(year, month)
            total = span_days(*period)
            for _ in range(60):
                start = period[0] - timedelta(days=rng.randint(0, 5)) + timedelta(days=rng.randint(0, total))
                end = start + timedelta(days=rng.randint(0, total + 4))
                amount = rng.choice([1, 2, 3, 100, 999, 12345, -12345, -7, 0])
                expected = oracle_div(amount * used_days(period, start, end), total)
                self.assertEqual(prorate(amount, period, start, end), expected, (period, start, end, amount))
        feb = month_period(2024, 2)
        self.assertEqual(prorate(2900, feb, feb[0], feb[1]), 2900)
        self.assertEqual(prorate(2900, feb, date(2020, 1, 1), date(2020, 2, 1)), 0)
        with self.assertRaises(ValueError):
            prorate(100, feb, date(2024, 2, 9), date(2024, 2, 8))
        with self.assertRaises(ValueError):
            prorate(100, (date(2024, 2, 9), date(2024, 2, 8)), date(2024, 2, 8), date(2024, 2, 9))

    def test_refund_split_always_conserves_the_amount(self):
        rng = random.Random(@@SEED@@ + 4)
        period = month_period(2024, 2)
        total = span_days(*period)
        for _ in range(300):
            amount = rng.randint(-5000, 5000)
            cancel = period[0] + timedelta(days=rng.randint(0, total + 3))
            charged, refund = split_refund(amount, period, cancel)
            self.assertEqual(charged + refund, amount, (amount, cancel))
            self.assertEqual(charged, oracle_div(amount * used_days(period, period[0], cancel), total))
        self.assertEqual(split_refund(100, period, period[0]),
                         (oracle_div(100 * span_days(period[0], period[0]), total),
                          100 - oracle_div(100 * span_days(period[0], period[0]), total)))
        self.assertEqual(split_refund(-100, period, period[1] + timedelta(days=9)), (-100, 0))
        with self.assertRaises(ValueError):
            split_refund(100, period, period[0] - timedelta(days=1))

    def test_usage_split_weights_by_days_inside_the_period(self):
        period = month_period(2024, 2)
        outside = (date(2023, 12, 1), date(2024, 1, 20))
        spans = [(date(2024, 1, 20), date(2024, 2, 10)), (date(2024, 2, 11), date(2024, 2, 20)),
                 (date(2024, 2, 21), date(2024, 3, 15)), outside]
        weights = [used_days(period, start, end) for start, end in spans]
        self.assertEqual(weights[3], 0)
        for amount in (1000, 1001, -1001, 7):
            shares = split_by_usage(amount, period, spans)
            self.assertEqual(shares, oracle_allocate(amount, weights), amount)
            self.assertEqual(sum(shares), amount)
        with self.assertRaises(ValueError):
            split_by_usage(100, period, [outside])
        with self.assertRaises(ValueError):
            split_by_usage(100, period, [])
'''


def _no_point(digits):
    return " (no decimal point when the currency has none)" if digits == 0 else ""


def _unit_example(digits):
    """(text, minor units) of one example amount in a currency with `digits` fractional digits."""
    text = "12" + ("." + "3" * digits if digits else "")
    return text, int("12" + "3" * digits)


def _tie_key(rule):
    return "i" if rule == "index" else "-weights[i], i"


def _modules(pkg, params, bad):
    """(currency, alloc, proration) text; `bad` selects the starter's defective variants."""
    mode_key = params["MODE"]
    rounding = ROUND_DIV[mode_key] if bad else ROUND_DIV["good"]
    currency = fill(CURRENCY, ROUND_DIV=rounding, TO_MINOR=TO_MINOR["float" if bad else "good"],
                    FORMAT=FORMAT["float" if bad else "good"], CODE=params["CODE"], DIGITS=params["DIGITS"], MODE=mode_key)
    allocate = fill(ALLOCATE["naive" if bad else "good"], TIE_KEY=_tie_key(params["TIE"]), allow_unused=True)
    alloc = fill(ALLOC, ALLOCATE=allocate)
    which = "bad" if bad else "good"
    proration = fill(PRORATION, INCLUSIVE=params["INCLUSIVE"],
                     MONTH_PERIOD=MONTH_PERIOD["no_leap" if bad else "good"],
                     USED_DAYS=USED_DAYS["unclipped" if bad else "good"],
                     SPLIT_REFUND=SPLIT_REFUND["independent" if bad else "good"],
                     SPLIT_BY_USAGE=SPLIT_BY_USAGE["plain" if bad else "good"])
    assert which in ("bad", "good"), "the variant switch is a plain flag"
    return currency, alloc, proration


PROMPTS = (
    "Billing amounts in `@@PKG@@/` are wrong on some invoices: totals drift by a unit, refunds do not add up, "
    "and month ends look off. `docs/rules.md` is the contract. Fix the package so it follows it.",
    "Finance reports that splits, prorations and refunds from `@@PKG@@` lose or invent minor units. "
    "Make `currency.py`, `alloc.py` and `proration.py` match `docs/rules.md`.",
    "Please audit `@@PKG@@/` against `docs/rules.md`. The visible tests pass, but allocation, rounding and "
    "day counting break on edge cases the doc describes. Fix them.",
)


def generate(seed, rng):
    pkg = package_name(rng, "billing")
    code = rng.choice(sorted(CURRENCIES))
    params = {"CODE": code, "DIGITS": CURRENCIES[code], "MODE": rng.choice(sorted(MODES)),
              "INCLUSIVE": rng.random() < 0.5, "TIE": rng.choice(sorted(TIE_RULES))}
    text, minor = _unit_example(params["DIGITS"])
    doc = fill(DOC, CODE=code, DIGITS=params["DIGITS"], UNIT=10 ** params["DIGITS"], MODE=params["MODE"],
               MODE_TEXT=MODES[params["MODE"]], NO_POINT=_no_point(params["DIGITS"]),
               TIE_TEXT=TIE_RULES[params["TIE"]], BOUNDARY_TEXT=BOUNDARY[params["INCLUSIVE"]])
    starter = _modules(pkg, params, bad=True)
    solution = _modules(pkg, params, bad=False)
    visible = fill(VISIBLE_TESTS, PKG=pkg, TEXT=text, MINOR=minor, HALF_END=15 if params["INCLUSIVE"] else 16)
    hidden = fill(HIDDEN, PKG=pkg, DIGITS=params["DIGITS"], HALF_UP=params["MODE"] == "half_up",
                  INCLUSIVE=params["INCLUSIVE"], BY_WEIGHT=params["TIE"] == "weight", SEED=seed)
    names = ("currency", "alloc", "proration")
    files = {"README.md": f"# {pkg}\n\nBilling arithmetic in integer minor units. Rules: `docs/rules.md`.\n",
             "docs/rules.md": doc, f"{pkg}/__init__.py": "", "tests/__init__.py": "", "tests/test_billing.py": visible,
             **{f"{pkg}/{name}.py": text for name, text in zip(names, starter)}}
    return make_spec(family="numeric", template="prorate", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/{name}.py": text for name, text in zip(names, solution)},
                     meta={"currency": code, "mode": params["MODE"], "inclusive": params["INCLUSIVE"]})
