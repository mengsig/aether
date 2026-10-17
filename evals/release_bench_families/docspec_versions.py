"""docspec / versions: version ranges (comparators, partials, tilde, caret, hyphens, prereleases) from a doc.

The rules interact (a partial version behind `<=`, a caret on a 0.x version, a
prerelease that only matches beside a comparator on the same release), and
the seed varies the caret and prerelease policies so recall cannot stand in for
the doc.
"""
from .common import fill, make_spec, package_name, reference_block

SOLUTION = r'''
"""Version ranges as described in docs/versions.md."""
import re

CARET_LOCKS_FIRST_NONZERO = @@CARET_ZERO@@
STRICT_PRERELEASES = @@STRICT@@
V_PREFIX = @@V_PREFIX@@

NUMBER = r"(?:0|[1-9][0-9]*)"
IDENT = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
FULL = re.compile(rf"({NUMBER})\.({NUMBER})\.({NUMBER})(?:-({IDENT}(?:\.{IDENT})*))?"
                  r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?")
PARTIAL = re.compile(rf"({NUMBER}|[xX*])(?:\.({NUMBER}|[xX*])(?:\.({NUMBER}|[xX*]))?)?")
COMPARATOR = re.compile(r"(<=|>=|<|>|=|~|\^)?[ \t]*([^\s<>=~^|]+)(?=\s|$)")
HYPHEN = re.compile(r"(\S+)[ \t]+-[ \t]+(\S+)")
LOWEST = (0, ((0, 0),))
NOTHING = [("<", (0, 0, 0, LOWEST))]


def _pre_key(pre):
    if not pre:
        return (1,)
    return (0, tuple((0, int(part)) if part.isdigit() else (1, part) for part in pre.split(".")))


def _strip_prefix(text):
    if text[:1] in ("v", "V") and V_PREFIX:
        return text[1:]
    return text


def parse_version(text):
    """(major, minor, patch, prerelease key) of a full version; ValueError otherwise."""
    match = FULL.fullmatch(_strip_prefix(text.strip()))
    if match is None:
        raise ValueError(f"not a version: {text!r}")
    return (int(match[1]), int(match[2]), int(match[3]), _pre_key(match[4]))


def _partial(text):
    """(parts, prerelease, given): parts has None for a missing or wildcard part."""
    body = _strip_prefix(text)
    full = FULL.fullmatch(body)
    if full:
        return [int(full[1]), int(full[2]), int(full[3])], full[4] or "", 3
    match = PARTIAL.fullmatch(body)
    if match is None:
        raise ValueError(f"not a version or partial version: {text!r}")
    parts = [None if part is None or part in "xX*" else int(part) for part in match.groups()]
    given = next((index for index, part in enumerate(parts) if part is None), 3)
    if any(part is not None for part in parts[given:]):
        raise ValueError(f"a number after a wildcard: {text!r}")
    return parts, "", given


def _floor(parts, pre, given):
    return (*[part or 0 for part in parts[:given]], *[0] * (3 - given), _pre_key(pre))


def _bump(parts, index):
    """The lowest version above every release that keeps parts[:index + 1]: X.Y.Z-0 with part `index` + 1."""
    bumped = [*parts[:index], parts[index] + 1, *[0] * (2 - index)]
    return (*bumped, LOWEST)


def _plain(op, parts, pre, given):
    if given == 3:
        return [(op or "=", _floor(parts, pre, 3))]
    if given == 0:
        return NOTHING if op in ("<", ">") else []
    if op in ("", "="):
        return [(">=", _floor(parts, pre, given)), ("<", _bump(parts, given - 1))]
    if op == ">":
        return [(">=", _bump(parts, given - 1)[:3] + ((1,),))]
    if op == ">=":
        return [(">=", _floor(parts, pre, given))]
    if op == "<":
        return [("<", _floor(parts, pre, given)[:3] + (LOWEST,))]
    return [("<", _bump(parts, given - 1))]


def _tilde(parts, pre, given):
    if given == 0:
        return []
    return [(">=", _floor(parts, pre, given)), ("<", _bump(parts, min(given - 1, 1)))]


def _caret(parts, pre, given):
    if given == 0:
        return []
    lock = 0
    if CARET_LOCKS_FIRST_NONZERO:
        lock = next((index for index in range(given) if parts[index] != 0), given - 1)
    return [(">=", _floor(parts, pre, given)), ("<", _bump(parts, lock))]


def _hyphen(low, high):
    (low_parts, low_pre, low_given), (high_parts, high_pre, high_given) = _partial(low), _partial(high)
    comparators = [(">=", _floor(low_parts, low_pre, low_given))] if low_given else []
    if high_given == 3:
        comparators.append(("<=", _floor(high_parts, high_pre, 3)))
    elif high_given:
        comparators.append(("<", _bump(high_parts, high_given - 1)))
    written = {tuple(parts) for parts, pre in ((low_parts, low_pre), (high_parts, high_pre)) if pre}
    return comparators, written


def _comparator_set(text):
    """(comparators, releases written with a prerelease) of one `||` alternative."""
    text = text.strip()
    hyphen = HYPHEN.fullmatch(text)
    if hyphen:
        return _hyphen(hyphen[1], hyphen[2])
    comparators, written, position = [], set(), 0
    while position < len(text):
        while text[position] in " \t":
            position += 1
        match = COMPARATOR.match(text, position)
        if match is None:
            raise ValueError(f"bad comparator at {text[position:]!r}")
        op, (parts, pre, given) = match[1] or "", _partial(match[2])
        expand = {"~": _tilde, "^": _caret}.get(op)
        comparators += expand(parts, pre, given) if expand else _plain(op, parts, pre, given)
        if pre:
            written.add(tuple(parts))
        position = match.end()
        while position < len(text) and text[position] in " \t":
            position += 1
    return comparators, written


def parse_range(text):
    """The `||` alternatives of a range, each (comparators, prerelease releases)."""
    return [_comparator_set(alternative) for alternative in text.split("||")]


def _holds(key, op, bound):
    return {"<": key < bound, "<=": key <= bound, ">": key > bound, ">=": key >= bound, "=": key == bound}[op]


def _matches(key, alternatives):
    for comparators, written in alternatives:
        if not all(_holds(key, op, bound) for op, bound in comparators):
            continue
        if key[3] == (1,) or not STRICT_PRERELEASES or tuple(key[:3]) in written:
            return True
    return False


def satisfies(version, range_text):
    """Whether `version` lies in `range_text` (see docs/versions.md)."""
    alternatives = parse_range(range_text)
    return _matches(parse_version(version), alternatives)


def max_satisfying(versions, range_text):
    """The highest of `versions` inside `range_text`, as written; None when none is."""
    alternatives = parse_range(range_text)
    best, best_key = None, None
    for version in versions:
        key = parse_version(version)
        if _matches(key, alternatives) and (best_key is None or key > best_key):
            best, best_key = version, key
    return best
'''

STARTER = '''
"""Version ranges as described in docs/versions.md."""


def parse_version(text):
    major, minor, patch = text.strip().split(".")
    return int(major), int(minor), int(patch)


def satisfies(version, range_text):
    """Whether `version` lies in `range_text` (see docs/versions.md)."""
    found = parse_version(version)
    for comparator in range_text.split():
        if comparator.startswith(">="):
            if found < parse_version(comparator[2:]):
                return False
        elif comparator.startswith("<"):
            if found >= parse_version(comparator[1:]):
                return False
        elif found != parse_version(comparator.lstrip("=")):
            return False
    return True


def max_satisfying(versions, range_text):
    """The highest of `versions` inside `range_text`, as written; None when none is."""
    matching = [version for version in versions if satisfies(version, range_text)]
    return max(matching, key=parse_version) if matching else None
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.ranges import max_satisfying, satisfies


class RangeTests(unittest.TestCase):
    def test_bounds(self):
        self.assertTrue(satisfies("1.4.0", ">=1.2.0 <2.0.0"))
        self.assertFalse(satisfies("2.0.0", ">=1.2.0 <2.0.0"))

    def test_highest_match(self):
        self.assertEqual(max_satisfying(["1.2.0", "1.10.0", "2.0.0"], ">=1.0.0 <2.0.0"), "1.10.0")


if __name__ == "__main__":
    unittest.main()
'''

DOC = '''
# Version ranges

A plugin declares the host versions it supports as a range. `satisfies(version,
range)` in `@@PKG@@/ranges.py` says whether a version lies in a range, and
`max_satisfying(versions, range)` picks the highest version of a list (any
iterable of strings) that does, returned exactly as written, or `None` when no
version does. When two matching versions have equal precedence, the first one
listed wins. Invalid input anywhere (a version, an entry of the list, or any
part of the range, even an alternative that is never needed) raises
`ValueError`.

## Versions

A version is `MAJOR.MINOR.PATCH`: three numbers without leading zeros (`0` is
fine, `01` is not). It may continue with `-` and a prerelease: dot-separated
identifiers of ASCII letters, digits and `-`, where an all-digit identifier has
no leading zero (`1.0.0-rc.1`, `2.1.0-alpha-2.x`). It may end with `+` and
build metadata (dot-separated identifiers of ASCII letters, digits and `-`),
which precedence ignores entirely. Whitespace around a version is ignored.
@@V_RULE@@

Precedence compares major, then minor, then patch numerically. A version with a
prerelease is lower than the same version without one (`1.0.0-rc.1 < 1.0.0`).
Two prereleases compare identifier by identifier: two all-digit identifiers
numerically, two others in ASCII order, and an all-digit identifier is lower
than any other; when every identifier of one prerelease equals the start of the
other, the shorter one is lower (`1.0.0-alpha < 1.0.0-alpha.1 <
1.0.0-alpha.beta < 1.0.0-beta < 1.0.0-beta.2 < 1.0.0-beta.11 < 1.0.0-rc.1`).

## Ranges

A range is one or more **alternatives** separated by `||` (with or without
whitespace around it); a version is in the range when it is in any alternative.
An alternative is either a **hyphen range** or a list of **comparators**
separated by whitespace (spaces or tabs); a version is in the alternative when
it satisfies every comparator in it. An empty alternative (also an empty or
all-whitespace range) contains every version, subject to the prerelease rule
below.

A comparator is an optional operator, `<`, `<=`, `>`, `>=`, `=`, `~` or `^`,
then optional spaces or tabs, then a version or a **partial version**. A partial
version has one to three parts, each a number (no leading zeros) or a wildcard
`x`, `X` or `*`; once a part is a wildcard every later part must be one as well
(`1.x.3` is invalid), and missing parts count as wildcards (`1.2` is `1.2.x`,
and `*` alone is all wildcards). Only a version with three numbers may carry a
prerelease or build metadata.@@V_PARTIAL@@ Comparators must be separated by
whitespace: `>=1.2.3<2.0.0` is invalid.

Below, `X.Y.Z-0` stands for the lowest version with that major, minor and
patch: `-0` is lower than every other prerelease of `X.Y.Z`, so `< 2.0.0-0`
excludes 2.0.0 and all its prereleases.

| Comparator | Means |
| --- | --- |
| `1.2.3`, `=1.2.3` | equal precedence to 1.2.3 |
| `<1.2.3`, `<=1.2.3`, `>1.2.3`, `>=1.2.3` | the comparison on precedence |
| `1.2`, `=1.2`, `1.2.x` | `>=1.2.0 <1.3.0-0` |
| `1`, `1.x`, `1.x.x` | `>=1.0.0 <2.0.0-0` |
| `*`, `x`, `>=*`, `<=*` | every version (no constraint) |
| `<*`, `>*` | no version at all |
| `>1.2` | `>=1.3.0` (above every 1.2 version); `>1` is `>=2.0.0` |
| `>=1.2` | `>=1.2.0` |
| `<1.2` | `<1.2.0-0` |
| `<=1.2` | `<1.3.0-0` (every 1.2 version included); `<=1` is `<2.0.0-0` |
| `~1.2.3`, `~1.2.3-beta.2` | `>=1.2.3 <1.3.0-0` (the lower bound keeps its prerelease) |
| `~1.2`, `~1.2.x` | `>=1.2.0 <1.3.0-0` |
| `~1`, `~1.x` | `>=1.0.0 <2.0.0-0` |
| `~*`, `^*` | every version |

**Caret.** `^` allows changes that do not modify @@CARET_TEXT@@ The lower bound
is the partial version with missing parts as 0, keeping a prerelease.

@@CARET_TABLE@@

**Hyphen range.** `A - B` (whitespace on both sides of the `-`, and nothing
else in the alternative) is the inclusive range from `A` to `B`. `A` and `B` are
versions or partial versions without an operator. A partial `A` has its missing
parts as 0 (`1.2 - 2.3.4` is `>=1.2.0 <=2.3.4`); a partial `B` includes
everything it starts (`1.2.3 - 2.3` is `>=1.2.3 <2.4.0-0`, `1.2.3 - 2` is
`>=1.2.3 <3.0.0-0`). A wildcard end has no bound (`* - 2.0.0` is `<=2.0.0`).

## Prereleases

@@PRERELEASE_TEXT@@
'''

V_RULE = {True: "A single leading `v` or `V` is allowed and ignored (`v1.2.3` is 1.2.3).",
          False: "Nothing may precede the major number: `v1.2.3` is invalid."}
V_PARTIAL = {True: " A version or partial version in a range may also start with `v` or `V`.", False: ""}

CARET_TEXT = {
    True: "the left-most non-zero part of the version (major, minor or patch), counting only the parts "
          "written as numbers; when every such part is 0, the last of them is the one that may not change.",
    False: "the major number, whatever it is (also when it is 0).",
}
CARET_TABLE = {
    True: """
| Comparator | Means |
| --- | --- |
| `^1.2.3`, `^1.2`, `^1.x`, `^1` | `>=1.2.3 <2.0.0-0`, `>=1.2.0 <2.0.0-0`, `>=1.0.0 <2.0.0-0`, `>=1.0.0 <2.0.0-0` |
| `^0.2.3`, `^0.2`, `^0.2.x` | `>=0.2.3 <0.3.0-0`, `>=0.2.0 <0.3.0-0`, `>=0.2.0 <0.3.0-0` |
| `^0.0.3`, `^0.0.3-rc.1` | `>=0.0.3 <0.0.4-0`, `>=0.0.3-rc.1 <0.0.4-0` |
| `^0.0`, `^0.0.x` | `>=0.0.0 <0.1.0-0` |
| `^0`, `^0.x` | `>=0.0.0 <1.0.0-0` |
| `^0.0.0` | `>=0.0.0 <0.0.1-0` |""",
    False: """
| Comparator | Means |
| --- | --- |
| `^1.2.3`, `^1.2`, `^1.x`, `^1` | `>=1.2.3 <2.0.0-0`, `>=1.2.0 <2.0.0-0`, `>=1.0.0 <2.0.0-0`, `>=1.0.0 <2.0.0-0` |
| `^0.2.3`, `^0.0.3-rc.1` | `>=0.2.3 <1.0.0-0`, `>=0.0.3-rc.1 <1.0.0-0` |
| `^0.0`, `^0.x`, `^0` | `>=0.0.0 <1.0.0-0` |""",
}
PRERELEASE_TEXT = {
    True: """A version with a prerelease is in an alternative only when, besides
satisfying every comparator, the alternative has a comparator (or hyphen-range
end) **written** with a prerelease on the same major, minor and patch. So
`1.2.3-beta.4` is in `>=1.2.3-alpha.1 <2.0.0` but `1.2.4-beta` is not, and no
prerelease is in `*`, `1.x` or `>=1.0.0`. Bounds such as `<2.0.0-0` produced
from partial versions, tildes and carets do not count as written.""",
    False: """A version with a prerelease is compared like any other version: it is
in an alternative when it satisfies every comparator. So `1.5.0-beta` is in
`>=1.0.0 <2.0.0` and in `1.x`. `2.0.0-beta` is in `>=1.0.0 <2.0.0` too (it
is lower than `2.0.0`), but not in `1.x` or `>=1.0.0 <2.0.0-0`: the bound
`<2.0.0-0` excludes every prerelease of 2.0.0.""",
}
# (version, range, in it?) for every example PRERELEASE_TEXT states; the reference must agree with each.
PRERELEASE_EXAMPLES = {
    True: [("1.2.3-beta.4", ">=1.2.3-alpha.1 <2.0.0", True), ("1.2.4-beta", ">=1.2.3-alpha.1 <2.0.0", False),
           ("1.2.3-beta.4", "*", False), ("1.2.3-beta.4", "1.x", False), ("1.2.3-beta.4", ">=1.0.0", False)],
    False: [("1.5.0-beta", ">=1.0.0 <2.0.0", True), ("1.5.0-beta", "1.x", True),
            ("2.0.0-beta", ">=1.0.0 <2.0.0", True), ("2.0.0-beta", "1.x", False),
            ("2.0.0-beta", ">=1.0.0 <2.0.0-0", False)],
}

HIDDEN = r'''
import random
import unittest

from @@PKG@@.ranges import max_satisfying, satisfies

@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)
V_PREFIX = REFERENCE["V_PREFIX"]
PRERELEASES = ["", "", "", "-0", "-1", "-alpha", "-alpha.1", "-alpha.beta", "-beta.2", "-beta.11", "-rc.1", "-x-y.3"]


def random_version(rng):
    numbers = [rng.choice([0, 0, 1, 1, 2, 3, 10]) for _ in range(3)]
    text = ".".join(map(str, numbers)) + rng.choice(PRERELEASES)
    if rng.random() < 0.1:
        text += rng.choice(["+build.5", "+001", "+exp-sha.5114f85"])
    if V_PREFIX and rng.random() < 0.15:
        text = rng.choice("vV") + text
    return text


def random_partial(rng):
    if rng.random() < 0.45:
        return random_version(rng)
    count = rng.choice([1, 2, 2, 3])
    numbers = [str(rng.choice([0, 0, 1, 2, 3])) for _ in range(count)]
    wildcard_from = rng.choice([count, count, rng.randint(0, count)])
    parts = numbers[:wildcard_from] + [rng.choice("xX*") for _ in range(count - wildcard_from)]
    return ".".join(parts)


def random_comparator(rng):
    op = rng.choice(["", "", "=", "<", "<=", ">", ">=", "~", "^", "^"])
    return op + rng.choice(["", "", "", " "]) + random_partial(rng)


def random_alternative(rng):
    if rng.random() < 0.18:
        return random_partial(rng) + rng.choice([" - ", "  -  ", "\t- "]) + random_partial(rng)
    return rng.choice([" ", "  ", "\t"]).join(random_comparator(rng) for _ in range(rng.choice([0, 1, 1, 2, 2, 3])))


def random_range(rng):
    alternatives = [random_alternative(rng) for _ in range(rng.choice([1, 1, 1, 2, 3]))]
    return rng.choice(["||", " || ", "  ||\t"]).join(alternatives)


def invalid_range(rng):
    broken = rng.choice(["1.x.3", "01.2.3", "1.2.3-01", "1.2-rc.1", ">=1.2.3<2.0.0", "~>1.2", "1.2.3 -",
                         "- 1.2.3", ">1 - 2", "1.2.3 - 2 - 3", "|", "1 | 2", "a.b.c", "1.2.3.4", ">=", "1.2.3+",
                         "v1.2.3" if not V_PREFIX else "vv1.2.3", "1.2.3-", "=>1.2.3", "^ ^1"])
    alternatives = [random_alternative(rng) for _ in range(rng.choice([0, 1, 2]))]
    alternatives.insert(rng.randint(0, len(alternatives)), broken)
    return " || ".join(alternatives)


class HiddenRangeChecks(unittest.TestCase):
    def assert_like_reference(self, pairs):
        mismatches = [(version, text) for version, text in pairs
                      if outcome(satisfies, version, text) != outcome(REFERENCE["satisfies"], version, text)]
        self.assertEqual(mismatches[:5], [], f"{len(mismatches)} of {len(pairs)} differ")

    def cases(self, ranges, rng, per_range=12):
        return [(random_version(rng), text) for text in ranges for _ in range(per_range)]

    def test_generated_ranges(self):
        rng = random.Random(@@SEED@@)
        self.assert_like_reference([(random_version(rng), random_range(rng)) for _ in range(1500)])

    def test_precedence_of_prereleases(self):
        order = ["1.0.0-0", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2",
                 "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1-0", "1.10.0"]
        pairs = [(version, "<" + bound) for version in order for bound in order]
        pairs += [(version, ">=" + bound) for version in order for bound in order]
        self.assert_like_reference(pairs)

    def test_build_metadata_is_ignored(self):
        self.assert_like_reference([("1.2.3+build.1", "=1.2.3"), ("1.2.3", "1.2.3+other"),
                                    ("1.2.3+a", "<=1.2.3+b"), ("1.2.3+a", ">1.2.3+b")])

    def test_partials_behind_operators(self):
        rng = random.Random(@@SEED@@ + 1)
        ranges = [op + partial for op in ("", "=", "<", "<=", ">", ">=")
                  for partial in ("1", "1.2", "1.x", "1.2.x", "*", "0", "0.0", "X", "2.X.*")]
        self.assert_like_reference(self.cases(ranges, rng))

    def test_tilde_and_caret(self):
        rng = random.Random(@@SEED@@ + 2)
        ranges = [op + version for op in ("~", "^", "~ ", "^ ") for version in
                  ("1.2.3", "1.2", "1", "0.2.3", "0.2", "0.0.3", "0.0", "0", "0.0.0", "0.0.x", "0.x", "1.x",
                   "0.0.3-rc.1", "1.2.3-beta.2", "*")]
        self.assert_like_reference(self.cases(ranges, rng))

    def test_hyphen_ranges(self):
        rng = random.Random(@@SEED@@ + 3)
        ranges = ["1.2.3 - 2.3.4", "1.2 - 2.3.4", "1.2.3 - 2.3", "1.2.3 - 2", "* - 2.0.0", "1.0.0 - *",
                  "0.0.1-alpha - 0.0.1", "1.2.3\t-\t1.2.3"]
        self.assert_like_reference(self.cases(ranges, rng))

    def test_prerelease_rule(self):
        rng = random.Random(@@SEED@@ + 4)
        ranges = [">=1.2.3-alpha.1 <2.0.0", "*", "1.x", ">=1.0.0", "<2.0.0", "~1.2.3-beta.2", "^1.2.3-alpha",
                  "1.2.3-rc.1 - 1.2.3", ">0.0.0-0", "", "1.2.3-beta.4 || 1.x"]
        versions = ["1.2.3-beta.4", "1.2.4-beta", "2.0.0-beta", "1.5.0-beta", "1.2.3-alpha.0", "0.0.0-0"]
        self.assert_like_reference([(version, text) for text in ranges for version in versions]
                                   + self.cases(ranges, rng))
        for version, text, inside in @@DOC_EXAMPLES@@:
            self.assertIs(satisfies(version, text), inside, (version, text))

    def test_alternatives_and_whitespace(self):
        rng = random.Random(@@SEED@@ + 5)
        ranges = ["1.x || >=2.5.0", "1.2.3||2", "<1.0.0 || ", " || 3.x", "  >=1.0.0\t<1.5.0  ", ">= 1.2.3 < 2",
                  "   "]
        self.assert_like_reference(self.cases(ranges, rng))

    def test_invalid_input(self):
        rng = random.Random(@@SEED@@ + 6)
        pairs = [(random_version(rng), invalid_range(rng)) for _ in range(150)]
        pairs += [(version, "*") for version in ("1.2", "01.2.3", "1.2.3-01", "1.2.3-", "1.2.3+", "1.2.3.4",
                                                  "", "1.2.3-a..b", "v1.2.3", "V1.2.3", "x.1.2")]
        self.assert_like_reference(pairs)

    def test_max_satisfying(self):
        rng = random.Random(@@SEED@@ + 7)
        for _ in range(200):
            versions = [random_version(rng) for _ in range(rng.randint(0, 8))]
            text = random_range(rng)
            self.assertEqual(outcome(max_satisfying, list(versions), text),
                             outcome(REFERENCE["max_satisfying"], list(versions), text), (versions, text))

    def test_max_satisfying_ties_and_iterables(self):
        versions = ["1.2.3+b", "1.2.3", "1.2.3+a", "1.0.0"]
        self.assertEqual(max_satisfying(iter(versions), "1.x"), "1.2.3+b")
        self.assertIsNone(max_satisfying(iter([]), "*"))
        with self.assertRaises(ValueError):
            max_satisfying(["1.0.0", "bad"], "*")
'''

PROMPTS = (
    "Plugin installs pick the wrong host versions: `@@PKG@@/ranges.py` only understands plain `>=`/`<` bounds. "
    "Implement version ranges exactly as `docs/versions.md` defines them.",
    "Finish `satisfies` and `max_satisfying` in `@@PKG@@/ranges.py` so they follow every rule in "
    "`docs/versions.md`; the current code handles only the simplest comparators.",
)


def generate(seed, rng):
    pkg = package_name(rng, "plugins")
    options = {"caret_zero": rng.random() < 0.6, "strict": rng.random() < 0.5, "v_prefix": rng.random() < 0.5}
    solution = fill(SOLUTION, CARET_ZERO=options["caret_zero"], STRICT=options["strict"], V_PREFIX=options["v_prefix"])
    doc = fill(DOC, PKG=pkg, V_RULE=V_RULE[options["v_prefix"]], V_PARTIAL=V_PARTIAL[options["v_prefix"]],
               CARET_TEXT=CARET_TEXT[options["caret_zero"]], CARET_TABLE=CARET_TABLE[options["caret_zero"]].strip(),
               PRERELEASE_TEXT=PRERELEASE_TEXT[options["strict"]])
    files = {"README.md": f"# {pkg}\n\nPlugin host compatibility. Range rules: `docs/versions.md`.\n",
             f"{pkg}/__init__.py": "", f"{pkg}/ranges.py": fill(STARTER), "tests/__init__.py": "",
             "tests/test_ranges.py": fill(VISIBLE_TESTS, PKG=pkg), "docs/versions.md": doc}
    examples = PRERELEASE_EXAMPLES[options["strict"]]
    reference = {"__name__": "release_bench_reference"}
    exec(compile(solution, "release_bench_reference", "exec"), reference)
    wrong = [example for example in examples if reference["satisfies"](example[0], example[1]) is not example[2]]
    assert not wrong, f"versions-{seed}: the doc's prerelease examples disagree with the reference: {wrong}"
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, REFERENCE=reference_block(solution), DOC_EXAMPLES=repr(examples))
    return make_spec(family="docspec", template="versions", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/ranges.py": solution}, meta=options)
