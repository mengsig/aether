"""The independent service packages of the delegation slice: a README rule set, a buggy starter, a fix.

Each service is a function of `rng` returning its files and hidden check class;
no service imports another, so the packages can be fixed in parallel.
"""
from .common import fill

RATELIMIT_README = '''
# @@NAME@@

`Bucket(rate, burst)` starts full, holding `burst` tokens. Tokens refill
continuously at `rate` per second and never exceed `burst`.

`allow(now, cost=1)` first refills for the seconds since the latest `now` it
has seen (a `now` earlier than that refills nothing and does not move the
clock back), then: if at least `cost` tokens are available it takes them and
returns True; otherwise it takes nothing and returns False. A `cost` that is
not positive raises ValueError.
'''
RATELIMIT_STARTER = '''
"""Token bucket (see README.md)."""


class Bucket:
    def __init__(self, rate, burst):
        self.rate, self.burst, self.tokens, self.last = rate, burst, burst, None

    def allow(self, now, cost=1):
        if self.last is not None:
            self.tokens += int(now - self.last) * self.rate
        self.last = now
        if self.tokens > cost:
            self.tokens -= cost
            return True
        return False
'''
RATELIMIT_SOLUTION = '''
"""Token bucket (see README.md)."""


class Bucket:
    def __init__(self, rate, burst):
        self.rate, self.burst, self.tokens, self.last = rate, burst, float(burst), None

    def allow(self, now, cost=1):
        if cost <= 0:
            raise ValueError("cost must be positive")
        if self.last is not None and now > self.last:
            self.tokens = min(self.burst, self.tokens + (now - self.last) * self.rate)
        self.last = now if self.last is None else max(self.last, now)
        if self.tokens >= cost:
            self.tokens -= cost
            return True
        return False
'''
RATELIMIT_HIDDEN = '''
class Hidden@@CLASS@@Checks(unittest.TestCase):
    def test_@@NAME@@_continuous_refill_and_cap(self):
        from services.@@NAME@@.core import Bucket

        bucket = Bucket(@@RATE@@, @@BURST@@)
        self.assertTrue(all(bucket.allow(0) for _ in range(@@BURST@@)))
        self.assertFalse(bucket.allow(0))
        self.assertTrue(bucket.allow(@@ONE_TOKEN@@))
        self.assertFalse(bucket.allow(@@ONE_TOKEN@@))
        later = Bucket(@@RATE@@, @@BURST@@)
        later.allow(0, cost=@@BURST@@)
        self.assertFalse(later.allow(1000, cost=@@BURST@@ + 1))
        self.assertTrue(later.allow(1000, cost=@@BURST@@))

    def test_@@NAME@@_exact_cost_clock_and_validation(self):
        from services.@@NAME@@.core import Bucket

        bucket = Bucket(@@RATE@@, @@BURST@@)
        self.assertTrue(bucket.allow(10, cost=@@BURST@@))
        self.assertFalse(bucket.allow(5))
        self.assertTrue(bucket.allow(10 + @@ONE_TOKEN@@))
        for cost in (0, -1):
            with self.assertRaises(ValueError):
                bucket.allow(20, cost=cost)
'''


def ratelimit(rng, name):
    rate = rng.choice([2, 4, 0.5])
    burst = rng.choice([3, 5, 8])
    values = {"NAME": name, "CLASS": name.title(), "RATE": rate, "BURST": burst, "ONE_TOKEN": repr(1 / rate)}
    visible = f"        self.assertTrue(Bucket({rate}, {burst}).allow(0))\n"
    return _service(name, "Bucket", RATELIMIT_README, RATELIMIT_STARTER, RATELIMIT_SOLUTION, RATELIMIT_HIDDEN,
                    values, visible)


PAGINATE_README = '''
# @@NAME@@

`paginate(items, per_page, page)` returns a dict with `items` (that page's
items), `page`, `pages` (the number of pages) and `has_next`.

Pages are numbered from 1, and the last page may be shorter than the others.
An empty `items` has exactly one page, which is empty. `per_page` must be from
1 to @@MAX@@ (otherwise ValueError); a `page` below 1 or above `pages` raises
IndexError.
'''
PAGINATE_STARTER = '''
"""Pagination (see README.md)."""


def paginate(items, per_page, page):
    pages = len(items) // per_page
    start = page * per_page
    return {"items": items[start:start + per_page], "page": page, "pages": pages, "has_next": page < pages}
'''
PAGINATE_SOLUTION = '''
"""Pagination (see README.md)."""
MAX_PER_PAGE = @@MAX@@


def paginate(items, per_page, page):
    if not 1 <= per_page <= MAX_PER_PAGE:
        raise ValueError(f"per_page must be from 1 to {MAX_PER_PAGE}")
    pages = max(1, -(-len(items) // per_page))
    if not 1 <= page <= pages:
        raise IndexError(f"page {page} is not in 1..{pages}")
    start = (page - 1) * per_page
    return {"items": items[start:start + per_page], "page": page, "pages": pages, "has_next": page < pages}
'''
PAGINATE_HIDDEN = '''
class Hidden@@CLASS@@Checks(unittest.TestCase):
    def test_@@NAME@@_pages_and_numbering(self):
        from services.@@NAME@@.core import paginate

        items = list(range(@@COUNT@@))
        last = paginate(items, @@PER@@, @@PAGES@@)
        self.assertEqual(last["items"], items[(@@PAGES@@ - 1) * @@PER@@:])
        self.assertEqual((last["pages"], last["has_next"]), (@@PAGES@@, False))
        first = paginate(items, @@PER@@, 1)
        self.assertEqual((first["items"], first["has_next"]), (items[:@@PER@@], True))
        self.assertEqual(paginate([], 5, 1), {"items": [], "page": 1, "pages": 1, "has_next": False})

    def test_@@NAME@@_bounds(self):
        from services.@@NAME@@.core import paginate

        for per_page in (0, @@MAX@@ + 1):
            with self.assertRaises(ValueError):
                paginate([1], per_page, 1)
        self.assertEqual(paginate([1, 2], @@MAX@@, 1)["items"], [1, 2])
        for page in (0, @@PAGES@@ + 1):
            with self.assertRaises(IndexError):
                paginate(list(range(@@COUNT@@)), @@PER@@, page)
'''


def paginate(rng, name):
    per = rng.choice([3, 4, 7])
    count = per * rng.randint(2, 5) + rng.randint(1, per - 1)
    values = {"NAME": name, "CLASS": name.title(), "MAX": rng.choice([50, 100, 200]), "PER": per, "COUNT": count,
              "PAGES": -(-count // per)}
    visible = "        self.assertEqual(paginate([1, 2, 3, 4], 2, 1)[\"items\"], [1, 2])\n"
    return _service(name, "paginate", PAGINATE_README, PAGINATE_STARTER, PAGINATE_SOLUTION, PAGINATE_HIDDEN,
                    values, visible)


BACKOFF_README = '''
# @@NAME@@

`delay(attempt)` is the wait before retry number `attempt` (1, 2, 3, ...):
`@@BASE@@ * @@FACTOR@@ ** (attempt - 1)` seconds, but never more than @@CAP@@,
rounded to 3 decimals. An `attempt` below 1 raises ValueError.

`schedule(n)` lists the delays of attempts 1 to `n`, except that it stops at
the first capped delay: the cap appears once, as the last entry.
'''
BACKOFF_STARTER = '''
"""Retry backoff (see README.md)."""


def delay(attempt):
    return round(@@BASE@@ * @@FACTOR@@ ** attempt, 3)


def schedule(n):
    return [delay(attempt) for attempt in range(1, n + 1)]
'''
BACKOFF_SOLUTION = '''
"""Retry backoff (see README.md)."""
CAP = @@CAP@@


def delay(attempt):
    if attempt < 1:
        raise ValueError("attempts start at 1")
    return round(min(CAP, @@BASE@@ * @@FACTOR@@ ** (attempt - 1)), 3)


def schedule(n):
    delays = []
    for attempt in range(1, n + 1):
        delays.append(delay(attempt))
        if delays[-1] >= CAP:
            break
    return delays
'''
BACKOFF_HIDDEN = '''
class Hidden@@CLASS@@Checks(unittest.TestCase):
    def test_@@NAME@@_delays(self):
        from services.@@NAME@@.core import delay

        self.assertEqual(delay(1), @@FIRST@@)
        self.assertEqual(delay(2), @@SECOND@@)
        self.assertEqual(delay(60), @@CAP@@)
        for attempt in (0, -2):
            with self.assertRaises(ValueError):
                delay(attempt)

    def test_@@NAME@@_schedule_stops_at_the_cap(self):
        from services.@@NAME@@.core import schedule

        self.assertEqual(schedule(50), @@SCHEDULE@@)
        self.assertEqual(schedule(1), [@@FIRST@@])
        self.assertEqual(schedule(0), [])
'''


def backoff(rng, name):
    base, factor, cap = rng.choice([0.1, 0.25, 0.5]), rng.choice([2, 3]), rng.choice([5, 10, 30])
    full = []
    for attempt in range(1, 51):
        full.append(round(min(cap, base * factor ** (attempt - 1)), 3))
        if full[-1] >= cap:
            break
    values = {"NAME": name, "CLASS": name.title(), "BASE": base, "FACTOR": factor, "CAP": cap,
              "FIRST": full[0], "SECOND": full[1], "SCHEDULE": repr(full)}
    visible = "        self.assertEqual(len(schedule(1)), 1)\n"
    return _service(name, "schedule", BACKOFF_README, BACKOFF_STARTER, BACKOFF_SOLUTION, BACKOFF_HIDDEN, values,
                    visible)


SPLIT_README = '''
# @@NAME@@

`split(total_cents, weights)` divides an integer number of cents among
parties in proportion to their positive integer `weights`. Each party first
gets `floor(total * weight / sum(weights))`; the cents left over then go one
each to the parties with the largest remainders, ties going to the earlier
party. The shares always add up to `total_cents`. A negative total is split
as its absolute value and every share negated. Empty `weights`, or any weight
below 1, raises ValueError.
'''
SPLIT_STARTER = '''
"""Proportional splits (see README.md)."""


def split(total_cents, weights):
    whole = sum(weights)
    return [round(total_cents * weight / whole) for weight in weights]
'''
SPLIT_SOLUTION = '''
"""Proportional splits (see README.md)."""


def split(total_cents, weights):
    if not weights or any(weight < 1 for weight in weights):
        raise ValueError("weights must be positive")
    if total_cents < 0:
        return [-share for share in split(-total_cents, weights)]
    whole = sum(weights)
    shares = [total_cents * weight // whole for weight in weights]
    remainders = [total_cents * weight % whole for weight in weights]
    order = sorted(range(len(weights)), key=lambda index: (-remainders[index], index))
    for index in order[:total_cents - sum(shares)]:
        shares[index] += 1
    return shares
'''
SPLIT_HIDDEN = '''
class Hidden@@CLASS@@Checks(unittest.TestCase):
    def test_@@NAME@@_remainders_and_ties(self):
        from services.@@NAME@@.core import split

        self.assertEqual(split(100, [1, 1, 1]), [34, 33, 33])
        self.assertEqual(split(@@TOTAL@@, @@WEIGHTS@@), @@SHARES@@)
        self.assertEqual(split(-100, [1, 1, 1]), [-34, -33, -33])
        self.assertEqual(split(5, [2, 1, 1, 2]), [2, 1, 1, 1])

    def test_@@NAME@@_sums_and_validation(self):
        from services.@@NAME@@.core import split

        for total in (0, 1, 7, 999, 10 ** 6 + 3):
            self.assertEqual(sum(split(total, [3, 5, 7])), total)
        for weights in ([], [1, 0], [2, -1]):
            with self.assertRaises(ValueError):
                split(10, weights)
'''


def _reference_split(total, weights):
    shares = [total * weight // sum(weights) for weight in weights]
    order = sorted(range(len(weights)), key=lambda index: (-(total * weights[index] % sum(weights)), index))
    for index in order[:total - sum(shares)]:
        shares[index] += 1
    return shares


def split(rng, name):
    weights = [rng.randint(1, 9) for _ in range(rng.randint(3, 5))]
    total = rng.randint(101, 9999)
    values = {"NAME": name, "CLASS": name.title(), "TOTAL": total, "WEIGHTS": repr(weights),
              "SHARES": repr(_reference_split(total, weights))}
    visible = "        self.assertEqual(split(10, [1, 1]), [5, 5])\n"
    return _service(name, "split", SPLIT_README, SPLIT_STARTER, SPLIT_SOLUTION, SPLIT_HIDDEN, values, visible)


EMAILS_README = '''
# @@NAME@@

`unique_emails(addresses)` keeps the first occurrence of each address, in
order. Two addresses are the same when they are equal after: stripping
surrounding whitespace, lower-casing the domain (the part after the last `@`),
and replacing an aliased domain by its canonical one (@@ALIASES_TEXT@@).
The local part stays case-sensitive. Blank entries are skipped. Each kept
address is returned in that normalized form.
'''
EMAILS_STARTER = '''
"""Address de-duplication (see README.md)."""


def unique_emails(addresses):
    seen, kept = set(), []
    for address in addresses:
        key = address.lower()
        if key not in seen:
            seen.add(key)
            kept.append(address)
    return kept
'''
EMAILS_SOLUTION = '''
"""Address de-duplication (see README.md)."""
ALIASES = @@ALIASES@@


def _normal(address):
    local, _, domain = address.strip().rpartition("@")
    domain = domain.lower()
    return f"{local}@{ALIASES.get(domain, domain)}"


def unique_emails(addresses):
    seen, kept = set(), []
    for address in addresses:
        if not address.strip():
            continue
        normal = _normal(address)
        if normal not in seen:
            seen.add(normal)
            kept.append(normal)
    return kept
'''
EMAILS_HIDDEN = '''
class Hidden@@CLASS@@Checks(unittest.TestCase):
    def test_@@NAME@@_domain_case_and_aliases(self):
        from services.@@NAME@@.core import unique_emails

        self.assertEqual(unique_emails(["Ann@Example.COM", "ann@example.com", "ann@EXAMPLE.com"]),
                         ["Ann@example.com", "ann@example.com"])
        self.assertEqual(unique_emails(["bo@@@ALIAS_FROM_UPPER@@", "bo@@@ALIAS_TO@@"]), ["bo@@@ALIAS_TO@@"])

    def test_@@NAME@@_blanks_and_whitespace(self):
        from services.@@NAME@@.core import unique_emails

        self.assertEqual(unique_emails(["", "  ", " cy@x.org ", "cy@X.ORG", "\\t"]), ["cy@x.org"])
        self.assertEqual(unique_emails(["a@b@Host.io", "a@b@host.io"]), ["a@b@host.io"])
'''


def emails(rng, name):
    pairs = [("googlemail.com", "gmail.com"), ("me.com", "icloud.com"), ("ymail.com", "yahoo.com")]
    aliases = dict(rng.sample(pairs, 2))
    alias_from = sorted(aliases)[0]
    text = ", ".join(f"`{key}` is `{value}`" for key, value in sorted(aliases.items()))
    values = {"NAME": name, "CLASS": name.title(), "ALIASES": repr(aliases), "ALIASES_TEXT": text,
              "ALIAS_FROM_UPPER": alias_from.upper(), "ALIAS_TO": aliases[alias_from]}
    visible = "        self.assertEqual(unique_emails([\"a@x.org\", \"a@x.org\"]), [\"a@x.org\"])\n"
    return _service(name, "unique_emails", EMAILS_README, EMAILS_STARTER, EMAILS_SOLUTION, EMAILS_HIDDEN, values,
                    visible)


MEAN_README = '''
# @@NAME@@

`trailing_mean(values, window)` returns one result per input value: the mean
of that value and up to `window - 1` values before it (fewer at the start).
`None` values are skipped: a result is the mean of the non-None values in its
window, or None when there are none. Means are rounded half up (as in
school, not banker's rounding) to @@DIGITS@@ decimals and returned as floats.
A `window` below 1 raises ValueError.
'''
MEAN_STARTER = '''
"""Trailing means (see README.md)."""


def trailing_mean(values, window):
    return [round(sum(values[index - window + 1:index + 1]) / window, @@DIGITS@@)
            for index in range(window - 1, len(values))]
'''
MEAN_SOLUTION = '''
"""Trailing means (see README.md)."""
from decimal import ROUND_HALF_UP, Decimal

DIGITS = @@DIGITS@@


def trailing_mean(values, window):
    if window < 1:
        raise ValueError("window must be at least 1")
    results = []
    for index in range(len(values)):
        present = [value for value in values[max(0, index - window + 1):index + 1] if value is not None]
        if not present:
            results.append(None)
            continue
        mean = sum(Decimal(str(value)) for value in present) / len(present)
        results.append(float(mean.quantize(Decimal(1).scaleb(-DIGITS), rounding=ROUND_HALF_UP)))
    return results
'''
MEAN_HIDDEN = '''
class Hidden@@CLASS@@Checks(unittest.TestCase):
    def test_@@NAME@@_start_and_rounding(self):
        from services.@@NAME@@.core import trailing_mean

        self.assertEqual(trailing_mean([1, 2, 3, 4], 3), [1.0, 1.5, 2.0, 3.0])
        self.assertEqual(trailing_mean(@@HALF_VALUES@@, 2), @@HALF_EXPECTED@@)

    def test_@@NAME@@_gaps_and_validation(self):
        from services.@@NAME@@.core import trailing_mean

        self.assertEqual(trailing_mean([None, 4, None, None, None, 6], 3), [None, 4.0, 4.0, 4.0, None, 6.0])
        self.assertEqual(trailing_mean([], 2), [])
        with self.assertRaises(ValueError):
            trailing_mean([1], 0)
'''


def mean(rng, name):
    digits = rng.choice([1, 2])
    step = 10 ** -digits
    # Two values whose mean ends in an exact 5 one place past `digits`: half up rounds it away from zero.
    low = round(rng.randint(1, 50) * step, digits)
    values = [low, round(low + step, digits)]
    expected = [float(values[0]), round(low + step, digits)]
    values_text, expected_text = repr(values), repr(expected)
    values = {"NAME": name, "CLASS": name.title(), "DIGITS": digits, "HALF_VALUES": values_text,
              "HALF_EXPECTED": expected_text}
    visible = "        self.assertEqual(trailing_mean([2, 2], 1), [2.0, 2.0])\n"
    return _service(name, "trailing_mean", MEAN_README, MEAN_STARTER, MEAN_SOLUTION, MEAN_HIDDEN, values, visible)


SERVICES = {"ratelimit": ratelimit, "paginate": paginate, "backoff": backoff, "split": split, "emails": emails,
            "mean": mean}

VISIBLE = '''
import unittest

from services.@@NAME@@.core import @@SYMBOL@@


class @@CLASS@@Tests(unittest.TestCase):
    def test_basic(self):
@@CASE@@

if __name__ == "__main__":
    unittest.main()
'''


def _service(name, symbol, readme, starter, solution, hidden, values, visible_case):
    """{"files", "solution", "hidden"} of one service package named `name`."""
    assert name.isidentifier() and symbol.isidentifier(), "a service and its entry point are identifiers"
    base = f"services/{name}"
    files = {f"{base}/README.md": fill(readme, allow_unused=True, **values), f"{base}/__init__.py": "",
             f"{base}/core.py": fill(starter, allow_unused=True, **values),
             f"tests/test_{name}.py": fill(VISIBLE, NAME=name, SYMBOL=symbol, CLASS=values["CLASS"],
                                           CASE=visible_case)}
    service = {"files": files, "solution": {f"{base}/core.py": fill(solution, allow_unused=True, **values)},
               "hidden": fill(hidden, allow_unused=True, **values)}
    assert service["solution"][f"{base}/core.py"] != files[f"{base}/core.py"], "the starter needs a fix"
    return service
