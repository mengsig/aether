"""edgecase / ranges: integer range helpers whose docstrings promise edge cases the visible tests skip."""
from .common import fill, make_spec, package_name

CONTRACT = '''
"""Integer ranges for the @@PKG@@ scheduler.

A range is a pair `(start, end)` of ints, @@CONVENTION@@
Every function below accepts any iterable of ranges (a list, a tuple, a
generator), never modifies its argument, and checks every range:

- `start` and `end` must be ints; anything else, including `bool`, raises `TypeError`;
- `start > end` raises `ValueError`;
- @@EMPTY_RULE@@

Results are lists of tuples, sorted by start.
"""
'''

SOLUTION = CONTRACT + '''
INCLUSIVE = @@INCLUSIVE@@


def _validated(ranges):
    result = []
    for start, end in ranges:
        if type(start) is not int or type(end) is not int:
            raise TypeError("range bounds must be ints")
        if start > end:
            raise ValueError(f"range starts after it ends: {(start, end)}")
        if INCLUSIVE or start < end:
            result.append((start, end))
    return result


def merge(ranges):
    """The same points as `ranges`, as the fewest ranges: ranges that overlap or touch become one."""
    merged, step = [], 1 if INCLUSIVE else 0
    for start, end in sorted(_validated(ranges)):
        if merged and start <= merged[-1][1] + step:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged


def covered(ranges):
    """How many integers the ranges cover together, each counted once."""
    return sum(end - start + (1 if INCLUSIVE else 0) for start, end in merge(ranges))


def gaps(ranges, low, high):
    """The parts of the window `(low, high)` (same convention, same checks) that no range covers."""
    if not _validated([(low, high)]):
        return []
    step = 0 if INCLUSIVE else 1
    last, cursor, found = high - step, low, []
    for start, end in merge(ranges):
        end -= step
        if end < cursor:
            continue
        if start > last:
            break
        if start > cursor:
            found.append((cursor, start - 1))
        cursor = max(cursor, end + 1)
    if cursor <= last:
        found.append((cursor, last))
    return [(first, final + step) for first, final in found]
'''

STARTER = CONTRACT + '''

def merge(ranges):
    """The same points as `ranges`, as the fewest ranges: ranges that overlap or touch become one."""
    ranges = sorted(ranges)
    merged = [ranges[0]]
    for start, end in ranges[1:]:
        if start < merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged


def covered(ranges):
    """How many integers the ranges cover together, each counted once."""
    return sum(end - start@@PLUS_ONE@@ for start, end in merge(ranges))


def gaps(ranges, low, high):
    """The parts of the window `(low, high)` (same convention, same checks) that no range covers."""
    result, cursor = [], low
    for start, end in merge(ranges):
        if start > cursor:
            result.append((cursor, start@@MINUS_ONE@@))
        cursor = max(cursor, end@@PLUS_ONE@@)
    if cursor < high:
        result.append((cursor, high))
    return result
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.ranges import covered, gaps, merge


class RangeTests(unittest.TestCase):
    def test_overlapping_ranges_merge(self):
        self.assertEqual(merge([(1, 5), (2, 8), (20, 30)]), [(1, 8), (20, 30)])

    def test_covered_counts_points(self):
        self.assertEqual(covered([(0, 10), (5, 15)]), @@COVERED_0_15@@)

    def test_gaps_between_ranges(self):
        self.assertEqual(gaps([(10, 20), (40, 50)], 0, 100), @@GAPS_EXAMPLE@@)


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import random
import unittest

from @@PKG@@.ranges import covered, gaps, merge

INCLUSIVE = @@INCLUSIVE@@


def points(ranges):
    found = set()
    for start, end in ranges:
        found.update(range(start, end + 1 if INCLUSIVE else end))
    return found


def random_ranges(rng):
    result = []
    for _ in range(rng.randint(0, 6)):
        start = rng.randint(-20, 20)
        result.append((start, start + rng.randint(0, 6)))
    return result


class HiddenRangeChecks(unittest.TestCase):
    def test_touching_ranges_merge(self):
        self.assertEqual(merge(@@TOUCHING@@), @@TOUCHING_MERGED@@)
        self.assertEqual(merge(@@APART@@), @@APART@@)

    def test_unsorted_nested_and_generator_input(self):
        self.assertEqual(merge([(30, 40), (1, 10), (2, 3)]), [(1, 10), (30, 40)])
        self.assertEqual(merge(r for r in [(5, 9), (0, 2)]), [(0, 2), (5, 9)])
        self.assertEqual(covered(r for r in [(0, 4), (2, 6)]), @@COVERED_0_6@@)

    def test_empty_input(self):
        self.assertEqual(merge([]), [])
        self.assertEqual(covered([]), 0)
        self.assertEqual(gaps([], 3, 9), [(3, 9)])

    def test_invalid_ranges(self):
        with self.assertRaises(ValueError):
            merge([(1, 2), (9, 3)])
        for bad in ([(1.0, 2)], [(True, 4)], [("1", 2)], [(0, None)]):
            with self.assertRaises(TypeError, msg=repr(bad)):
                merge(bad)
        with self.assertRaises(ValueError):
            gaps([], 9, 3)

    def test_degenerate_ranges(self):
@@DEGENERATE@@

    def test_argument_is_not_modified(self):
        ranges = [[8, 9], [1, 3]]
        result = merge(ranges)
        self.assertEqual(ranges, [[8, 9], [1, 3]])
        self.assertTrue(all(type(item) is tuple for item in result))

    def test_gaps_at_the_window_edges(self):
@@GAP_EDGES@@

    def test_merge_matches_points_on_random_input(self):
        rng = random.Random(@@SEED@@)
        step = 1 if INCLUSIVE else 0
        for _ in range(300):
            ranges = random_ranges(rng)
            merged = merge(ranges)
            self.assertEqual(points(merged), points(ranges), ranges)
            self.assertEqual(covered(ranges), len(points(ranges)), ranges)
            for (_, end), (start, _) in zip(merged, merged[1:]):
                self.assertGreater(start, end + step, merged)

    def test_gaps_match_points_on_random_input(self):
        rng = random.Random(@@SEED@@ + 1)
        for _ in range(300):
            ranges = random_ranges(rng)
            low = rng.randint(-25, 10)
            high = low + rng.randint(1, 30)
            window = points([(low, high)])
            found = gaps(ranges, low, high)
            self.assertEqual(points(found), window - points(ranges), (ranges, low, high))
            self.assertEqual(found, merge(found), (ranges, low, high))
'''

CONVENTIONS = {
    True: ("inclusive at both ends: `(3, 5)` holds 3, 4 and 5, and `(4, 4)` is the single point 4.",
           "`(4, 4)` is a valid range of one point."),
    False: ("half-open: `(3, 5)` holds 3 and 4 but not 5, so `(4, 4)` holds nothing.",
            "an empty range such as `(4, 4)` is valid and covers nothing; it never appears in a result."),
}

PROMPTS = (
    "`@@PKG@@/ranges.py` is used by the scheduler to merge booked slots and find free ones. Its visible "
    "tests pass, but production keeps hitting cases it gets wrong. Make `merge`, `covered` and `gaps` "
    "honour the contract in the module docstring and in each function's docstring.",
    "Fix `@@PKG@@/ranges.py` so `merge`, `covered` and `gaps` behave as the module documents for every "
    "input, not just the ones in `tests/`.",
)


def _cases(inclusive):
    if inclusive:
        return {"TOUCHING": "[(4, 6), (1, 3)]", "TOUCHING_MERGED": "[(1, 6)]", "APART": "[(1, 3), (5, 6)]",
                "COVERED_0_6": 7, "COVERED_0_15": 16, "GAPS_EXAMPLE": "[(0, 9), (21, 39), (51, 100)]",
                "DEGENERATE": ("        self.assertEqual(merge([(4, 4), (5, 5)]), [(4, 5)])\n"
                               "        self.assertEqual(covered([(4, 4)]), 1)"),
                "GAP_EDGES": ("        self.assertEqual(gaps([(0, 3), (8, 12)], 2, 9), [(4, 7)])\n"
                              "        self.assertEqual(gaps([(5, 5)], 5, 5), [])\n"
                              "        self.assertEqual(gaps([(1, 2)], 5, 6), [(5, 6)])")}
    return {"TOUCHING": "[(3, 6), (1, 3)]", "TOUCHING_MERGED": "[(1, 6)]", "APART": "[(1, 3), (4, 6)]",
            "COVERED_0_6": 6, "COVERED_0_15": 15, "GAPS_EXAMPLE": "[(0, 10), (20, 40), (50, 100)]",
            "DEGENERATE": ("        self.assertEqual(merge([(4, 4), (1, 2), (7, 7)]), [(1, 2)])\n"
                           "        self.assertEqual(covered([(4, 4)]), 0)\n"
                           "        self.assertEqual(gaps([(1, 2)], 5, 5), [])"),
            "GAP_EDGES": ("        self.assertEqual(gaps([(0, 3), (8, 12)], 2, 9), [(3, 8)])\n"
                          "        self.assertEqual(gaps([(5, 6)], 5, 6), [])\n"
                          "        self.assertEqual(gaps([(1, 2)], 5, 7), [(5, 7)])")}


def generate(seed, rng):
    pkg = package_name(rng, "slots")
    inclusive = rng.random() < 0.5
    convention, empty_rule = CONVENTIONS[inclusive]
    cases = _cases(inclusive)
    contract = {"PKG": pkg, "CONVENTION": convention, "EMPTY_RULE": empty_rule}
    starter = fill(STARTER, **contract, PLUS_ONE=" + 1" if inclusive else "", MINUS_ONE=" - 1" if inclusive else "")
    solution = fill(SOLUTION, **contract, INCLUSIVE=inclusive)
    hidden = fill(HIDDEN, PKG=pkg, INCLUSIVE=inclusive, SEED=seed, **{k: v for k, v in cases.items()
                                                                     if k not in ("COVERED_0_15", "GAPS_EXAMPLE")})
    visible = fill(VISIBLE_TESTS, PKG=pkg, COVERED_0_15=cases["COVERED_0_15"], GAPS_EXAMPLE=cases["GAPS_EXAMPLE"])
    files = {"README.md": f"# {pkg}\n\nSlot bookkeeping for the scheduler.\n", f"{pkg}/__init__.py": "",
             f"{pkg}/ranges.py": starter, "tests/__init__.py": "", "tests/test_ranges.py": visible}
    return make_spec(family="edgecase", template="ranges", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/ranges.py": solution})
