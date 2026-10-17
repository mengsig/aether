"""edgecase / bucket: a token-bucket limiter whose docstring covers clocks, costs and rounding the tests skip."""
from .common import fill, make_spec, package_name

CONTRACT = '''
"""A token bucket that limits @@WHAT@@ for @@PKG@@.

`TokenBucket(rate, capacity, clock=time.monotonic)`:

- `rate` is tokens added per second (an int or float, at least 0) and
  `capacity` the most tokens the bucket holds (more than 0); other values raise
  `ValueError`. The bucket starts full.
- Tokens are added continuously: on every call the bucket adds
  `(now - last) * rate` tokens, never above `capacity`, where `now` is the
  clock's reading and `last` the reading at the previous call.
- If the clock reads earlier than at the previous call, no tokens are added
  and that earlier reading becomes the new reference.
- `try_acquire(cost=1)` takes `cost` tokens and returns `True` when the bucket
  holds at least that many, and otherwise takes nothing and returns `False`.
- `retry_after(cost=1)` returns the seconds to wait before `try_acquire(cost)`
  could succeed, `0.0` when it would succeed now; it takes nothing. @@ZERO_RATE@@
- `cost` must be a number greater than 0 and at most `capacity`; any other
  cost raises `ValueError` from both methods.
- `available` (a property) is the current number of tokens, after adding.
- Comparisons of token counts allow for floating-point error: a bucket that
  holds `cost - 1e-9` tokens or more counts as holding `cost`.
"""
import time
'''

SOLUTION = CONTRACT + '''
EPSILON = 1e-9


class TokenBucket:
    def __init__(self, rate, capacity, clock=time.monotonic):
        if isinstance(rate, bool) or not isinstance(rate, (int, float)) or rate < 0:
            raise ValueError("rate must be a number of at least 0")
        if isinstance(capacity, bool) or not isinstance(capacity, (int, float)) or capacity <= 0:
            raise ValueError("capacity must be a number above 0")
        self._rate, self._capacity, self._clock = rate, capacity, clock
        self._tokens = float(capacity)
        self._last = clock()

    def _refill(self):
        now = self._clock()
        if now > self._last:
            self._tokens = min(self._capacity, self._tokens + (now - self._last) * self._rate)
        self._last = now

    def _check(self, cost):
        if isinstance(cost, bool) or not isinstance(cost, (int, float)) or not 0 < cost <= self._capacity:
            raise ValueError("cost must be above 0 and at most the capacity")

    @property
    def available(self):
        self._refill()
        return self._tokens

    def try_acquire(self, cost=1):
        self._check(cost)
        self._refill()
        if self._tokens + EPSILON < cost:
            return False
        self._tokens = max(0.0, self._tokens - cost)
        return True

    def retry_after(self, cost=1):
        self._check(cost)
        self._refill()
        missing = cost - self._tokens
        if missing <= EPSILON:
            return 0.0
        return @@RETRY_MISSING@@
'''

STARTER = CONTRACT + '''

class TokenBucket:
    def __init__(self, rate, capacity, clock=time.monotonic):
        self._rate, self._capacity, self._clock = rate, capacity, clock
        self._tokens = capacity
        self._last = clock()

    def _refill(self):
        now = self._clock()
        self._tokens = min(self._capacity, self._tokens + (now - self._last) * self._rate)
        self._last = now

    @property
    def available(self):
        self._refill()
        return self._tokens

    def try_acquire(self, cost=1):
        self._refill()
        if self._tokens >= cost:
            self._tokens -= cost
            return True
        return False

    def retry_after(self, cost=1):
        self._refill()
        return max(0.0, (cost - self._tokens) / self._rate)
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.limits import TokenBucket


class Clock:
    def __init__(self):
        self.now = 100.0

    def __call__(self):
        return self.now


class BucketTests(unittest.TestCase):
    def test_starts_full_and_drains(self):
        bucket = TokenBucket(rate=1, capacity=3, clock=Clock())
        self.assertEqual([bucket.try_acquire() for _ in range(4)], [True, True, True, False])

    def test_refills_over_time(self):
        clock = Clock()
        bucket = TokenBucket(rate=2, capacity=2, clock=clock)
        self.assertTrue(bucket.try_acquire(2))
        clock.now += 1
        self.assertTrue(bucket.try_acquire(2))


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import unittest

from @@PKG@@.limits import TokenBucket


class Clock:
    def __init__(self):
        self.now = 1000.0

    def __call__(self):
        return self.now


class HiddenBucketChecks(unittest.TestCase):
    def test_refill_never_exceeds_capacity(self):
        clock = Clock()
        bucket = TokenBucket(rate=5, capacity=3, clock=clock)
        clock.now += 60
        self.assertAlmostEqual(bucket.available, 3)
        self.assertTrue(bucket.try_acquire(3))
        self.assertFalse(bucket.try_acquire())

    def test_failed_acquire_takes_nothing(self):
        clock = Clock()
        bucket = TokenBucket(rate=1, capacity=4, clock=clock)
        self.assertTrue(bucket.try_acquire(3))
        self.assertFalse(bucket.try_acquire(2))
        self.assertAlmostEqual(bucket.available, 1)
        self.assertTrue(bucket.try_acquire(1))

    def test_clock_going_backwards(self):
        clock = Clock()
        bucket = TokenBucket(rate=1, capacity=10, clock=clock)
        self.assertTrue(bucket.try_acquire(10))
        clock.now -= 50
        self.assertAlmostEqual(bucket.available, 0)
        clock.now += 2
        self.assertAlmostEqual(bucket.available, 2)
        self.assertFalse(bucket.try_acquire(3))

    def test_bad_costs(self):
        bucket = TokenBucket(rate=1, capacity=5, clock=Clock())
        for cost in (0, -1, 6, 5.5, True, "1", None):
            with self.assertRaises((ValueError, TypeError), msg=repr(cost)):
                bucket.try_acquire(cost)
            with self.assertRaises((ValueError, TypeError), msg=repr(cost)):
                bucket.retry_after(cost)
        for cost in (0, -1, 6):
            with self.assertRaises(ValueError):
                bucket.try_acquire(cost)
        self.assertAlmostEqual(bucket.available, 5)

    def test_bad_construction(self):
        for rate, capacity in ((-1, 5), (1, 0), (1, -2)):
            with self.assertRaises(ValueError):
                TokenBucket(rate=rate, capacity=capacity, clock=Clock())
        TokenBucket(rate=0, capacity=1, clock=Clock())

    def test_retry_after(self):
        clock = Clock()
        bucket = TokenBucket(rate=4, capacity=8, clock=clock)
        self.assertEqual(bucket.retry_after(8), 0.0)
        self.assertTrue(bucket.try_acquire(8))
        self.assertAlmostEqual(bucket.retry_after(2), 0.5)
        self.assertAlmostEqual(bucket.available, 0)
        clock.now += 0.5
        self.assertTrue(bucket.try_acquire(2))

    def test_zero_rate(self):
        clock = Clock()
        bucket = TokenBucket(rate=0, capacity=2, clock=clock)
        self.assertTrue(bucket.try_acquire(2))
        clock.now += 1e6
        self.assertFalse(bucket.try_acquire(1))
@@ZERO_RATE_CHECK@@

    def test_floating_point_tolerance(self):
        clock = Clock()
        bucket = TokenBucket(rate=0.1, capacity=1, clock=clock)
        self.assertTrue(bucket.try_acquire(1))
        for _ in range(10):
            clock.now += 1
            bucket.available
        self.assertTrue(bucket.try_acquire(1))
        bucket = TokenBucket(rate=0.3, capacity=0.3, clock=clock)
        self.assertTrue(bucket.try_acquire(0.1))
        self.assertTrue(bucket.try_acquire(0.1))
        self.assertTrue(bucket.try_acquire(0.1))
        self.assertFalse(bucket.try_acquire(0.1))
'''

ZERO_RATE = {
    "inf": ("When `rate` is 0 and the tokens are short, it returns `math.inf`.",
            "        self.assertEqual(bucket.retry_after(1), float(\"inf\"))", "float(\"inf\") if self._rate == 0 else missing / self._rate"),
    "error": ("When `rate` is 0 and the tokens are short, it raises `ValueError` (they will never come).",
              "        with self.assertRaises(ValueError):\n            bucket.retry_after(1)",
              "missing / self._rate if self._rate else self._never()"),
}

NEVER = '''
    def _never(self):
        raise ValueError("tokens never refill at rate 0")
'''

PROMPTS = (
    "`@@PKG@@/limits.py` throttles @@WHAT@@ but misbehaves under load: callers see requests let "
    "through or refused when they should not be. Make `TokenBucket` follow its documented contract.",
    "Review and fix `TokenBucket` in `@@PKG@@/limits.py` so it does exactly what its module docstring promises.",
)


def generate(seed, rng):
    pkg = package_name(rng, "throttle")
    what = rng.choice(["outgoing webhook calls", "login attempts", "export jobs", "API requests"])
    zero = rng.choice(sorted(ZERO_RATE))
    words, check, retry = ZERO_RATE[zero]
    contract = {"PKG": pkg, "WHAT": what, "ZERO_RATE": words}
    solution = fill(SOLUTION, **contract, RETRY_MISSING=retry) + (NEVER if zero == "error" else "")
    files = {"README.md": f"# {pkg}\n\nRate limiting helpers.\n", f"{pkg}/__init__.py": "",
             f"{pkg}/limits.py": fill(STARTER, **contract), "tests/__init__.py": "",
             "tests/test_limits.py": fill(VISIBLE_TESTS, PKG=pkg)}
    return make_spec(family="edgecase", template="bucket", seed=seed,
                     prompt=fill(rng.choice(PROMPTS), allow_unused=True, PKG=pkg, WHAT=what), files=files,
                     hidden=fill(HIDDEN, PKG=pkg, ZERO_RATE_CHECK=check), solution={f"{pkg}/limits.py": solution})
