"""algo / diff: a minimal line diff with a forward tie-break rule and unified-style hunks.

The starter wraps difflib, which is neither minimal nor tie-broken as the doc
requires; a textbook LCS backtrack from the end is minimal but breaks ties
differently. Hidden checks compare against the reference on random files
drawn from a tiny alphabet, where ties are the rule, not the exception.
"""
from .common import fill, make_spec, package_name, reference_block

SOLUTION = r'''
"""Line diffs as described in docs/diff.md."""
CONTEXT = @@CONTEXT@@
DELETE_FIRST = @@DELETE_FIRST@@
IGNORE_TRAILING = @@IGNORE_TRAILING@@


def _lines(text):
    if not isinstance(text, str):
        raise TypeError("diff_lines compares text")
    parts = text.split("\n")
    if parts[-1] == "":
        parts.pop()
    return parts


def _same(left, right):
    return left.rstrip(" \t") == right.rstrip(" \t") if IGNORE_TRAILING else left == right


def _costs(old, new):
    """costs[i][j]: fewest deleted plus inserted lines turning old[i:] into new[j:]."""
    rows, cols = len(old), len(new)
    costs = [[0] * (cols + 1) for _ in range(rows + 1)]
    for i in range(rows, -1, -1):
        for j in range(cols, -1, -1):
            if i == rows or j == cols:
                costs[i][j] = (rows - i) + (cols - j)
            elif _same(old[i], new[j]):
                costs[i][j] = costs[i + 1][j + 1]
            else:
                costs[i][j] = 1 + min(costs[i + 1][j], costs[i][j + 1])
    return costs


def _script(old, new):
    """[(op, old index, new index, text)] walking both files from the top, ties broken as documented."""
    costs, i, j, script = _costs(old, new), 0, 0, []
    while i < len(old) or j < len(new):
        if i < len(old) and j < len(new) and _same(old[i], new[j]):
            script.append((" ", i, j, new[j]))
            i, j = i + 1, j + 1
            continue
        can_delete = i < len(old) and costs[i][j] == 1 + costs[i + 1][j]
        can_insert = j < len(new) and costs[i][j] == 1 + costs[i][j + 1]
        delete = can_delete and (DELETE_FIRST or not can_insert)
        script.append(("-", i, j, old[i]) if delete else ("+", i, j, new[j]))
        i, j = (i + 1, j) if delete else (i, j + 1)
    return script


def _groups(script):
    changes = [index for index, entry in enumerate(script) if entry[0] != " "]
    groups = []
    for index in changes:
        if groups and index - groups[-1][1] - 1 <= 2 * CONTEXT:
            groups[-1][1] = index
        else:
            groups.append([index, index])
    return groups


def _hunk(script, first, last):
    low, high = max(0, first - CONTEXT), min(len(script), last + CONTEXT + 1)
    entries = script[low:high]
    old_count = sum(1 for entry in entries if entry[0] != "+")
    new_count = sum(1 for entry in entries if entry[0] != "-")
    old_at, new_at = entries[0][1], entries[0][2]
    header = (f"@@ -{old_at + 1 if old_count else old_at},{old_count} "
              f"+{new_at + 1 if new_count else new_at},{new_count} @@\n")
    return header + "".join(f"{op}{text}\n" for op, _, _, text in entries)


def diff_lines(old, new):
    """The documented diff of two texts; "" when their lines are the same."""
    script = _script(_lines(old), _lines(new))
    return "".join(_hunk(script, first, last) for first, last in _groups(script))
'''

STARTER = '''
"""Line diffs as described in docs/diff.md."""
import difflib

CONTEXT = @@CONTEXT@@


def diff_lines(old, new):
    """The documented diff of two texts; "" when their lines are the same."""
    hunks = difflib.unified_diff(old.splitlines(), new.splitlines(), n=CONTEXT, lineterm="")
    return "".join(line + "\\n" for line in hunks if not line.startswith(("---", "+++")))
'''

DOC = '''
# Line diffs

`diff_lines(old, new)` (in `@@PKG@@/diff.py`) returns the text the review tool
shows for a change from `old` to `new`.

## Lines

A text is split at `\\n`. A final `\\n` does not start another line: `"a\\nb\\n"`
and `"a\\nb"` are both the lines `a`, `b`; `""` has no lines and `"\\n"` has one
empty line. Two lines match when @@MATCH_RULE@@.

## The edit script

The diff comes from an edit script that keeps, deletes and inserts lines to
turn the old lines into the new ones. It must be **minimal**: as few deleted
plus inserted lines as possible. Several minimal scripts often exist; the
script used is the one found by walking both files from the top and, at each
step:

1. keeping the current old and new line when they match;
2. otherwise @@FIRST_RULE@@;
3. otherwise @@SECOND_RULE@@.

(Keeping matching lines never makes a script longer, so rule 1 is always safe.)

## Output

Identical line lists give `""`. Otherwise the output is a sequence of hunks.
A hunk holds a run of changed lines plus up to @@CONTEXT@@ kept line(s) of
context on each side; two runs of changes separated by at most @@TWICE@@ kept
line(s) share one hunk. Each hunk starts with a header line

    @@ -a,b +c,d @@

where `b` is the number of old lines in the hunk (kept and deleted) and `a` is
the 1-based number of its first old line; `d` and `c` are the same for new lines
(kept and inserted). When `b` is 0, `a` is the number of the old line just
before the hunk (0 at the very start); likewise `c` when `d` is 0. Both counts
are always written, even when they are 1.

The header is followed by the hunk's lines in script order, each prefixed with
a space (kept), `-` (deleted) or `+` (inserted)@@KEPT_TEXT@@. Every output line,
headers included, ends in `\\n`. Inputs that are not text raise `TypeError`.
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.diff import diff_lines


class DiffTests(unittest.TestCase):
    def test_identical(self):
        self.assertEqual(diff_lines("a\\nb\\n", "a\\nb\\n"), "")

    def test_one_changed_line_without_neighbours(self):
        self.assertEqual(diff_lines("a\\n", "b\\n"), "@@ -1,1 +1,1 @@\\n-a\\n+b\\n" if @@DELETE_FIRST@@ else
                         "@@ -1,1 +1,1 @@\\n+b\\n-a\\n")


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = r'''
import random
import unittest

from @@PKG@@.diff import diff_lines

@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)


def text(lines, final="\n"):
    return "\n".join(lines) + (final if lines else "")


class HiddenDiffChecks(unittest.TestCase):
    def assert_like_reference(self, pairs):
        for old, new in pairs:
            self.assertEqual(outcome(diff_lines, old, new), outcome(REFERENCE["diff_lines"], old, new),
                             f"old={old!r} new={new!r}")

    def test_identical_and_line_splitting(self):
        self.assert_like_reference([("", ""), ("a", "a\n"), ("a\nb", "a\nb\n"), ("\n", ""), ("\n", "\n\n"),
                                    ("a\n\n", "a\n"), ("", "x\n"), ("x\n", "")])

    def test_minimal_where_difflib_is_not(self):
        self.assert_like_reference([(text(list("abcabba")), text(list("cbabac"))),
                                    (text(list("xaxbxcx")), text(list("abc"))),
                                    (text(list("aaabbb")), text(list("bbbaaa"))),
                                    (text(["}", "x", "}", "y", "}"]), text(["}", "y", "}", "x", "}"]))])

    def test_ties_follow_the_walk(self):
        self.assert_like_reference([(text(list("ab")), text(list("ba"))), (text(list("abc")), text(list("cab"))),
                                    (text(list("aab")), text(list("abb"))), (text(list("xy")), text(list("yx"))),
                                    (text(list("abab")), text(list("baba")))])

    def test_context_and_merging(self):
        base = [f"line {number}" for number in range(1, 15)]
        for gap in range(0, 2 * @@CONTEXT@@ + 3):
            changed = list(base)
            changed[3] = "changed"
            if 4 + gap < len(changed):
                changed[4 + gap] = "changed too"
            self.assert_like_reference([(text(base), text(changed))])

    def test_headers_at_the_edges(self):
        self.assert_like_reference([("", "a\nb\n"), ("a\nb\n", ""), ("a\nb\n", "x\na\nb\n"),
                                    ("a\nb\n", "a\nb\nx\n"), ("x\na\nb\nc\nd\n", "a\nb\nc\nd\n"),
                                    ("a\nb\nc\nd\nx\n", "a\nb\nc\nd\n")])

    def test_whitespace_at_line_ends(self):
        self.assert_like_reference([("a \nb\n", "a\nb\t\n"), ("a\n", "a  \nb\n"), (" a\n", "a\n"),
                                    ("a\t\nb\nc \n", "a\nB\nc\n")])

    def test_non_text_is_refused(self):
        self.assert_like_reference([(None, "a"), ("a", ["a"])])

    def test_generated_small_alphabet(self):
        rng = random.Random(@@SEED@@)
        pairs = []
        for _ in range(400):
            old = [rng.choice("abc") + rng.choice(["", "", " "]) for _ in range(rng.randint(0, 9))]
            new = [rng.choice("abc") + rng.choice(["", "", " "]) for _ in range(rng.randint(0, 9))]
            pairs.append((text(old, rng.choice(["", "\n"])), text(new, rng.choice(["", "\n"]))))
        self.assert_like_reference(pairs)

    def test_generated_edits_of_longer_files(self):
        rng = random.Random(@@SEED@@ + 1)
        pairs = []
        for _ in range(40):
            old = [rng.choice(["{", "}", "pass", "return x", "x += 1", ""]) for _ in range(rng.randint(20, 60))]
            new = list(old)
            for _ in range(rng.randint(1, 6)):
                at = rng.randrange(len(new) + 1)
                if rng.random() < 0.5 and at < len(new):
                    del new[at]
                else:
                    new.insert(at, rng.choice(["{", "}", "pass", "y = 2"]))
            pairs.append((text(old), text(new)))
        self.assert_like_reference(pairs)

    def test_large_files_finish(self):
        rng = random.Random(@@SEED@@ + 2)
        old = [rng.choice("abcd") for _ in range(400)]
        new = [rng.choice("abcd") for _ in range(400)]
        self.assert_like_reference([(text(old), text(new))])
'''

PROMPTS = (
    "Reviewers complain that the diffs in our review tool are longer than they need to be and do not look "
    "like `docs/diff.md` says. Make `diff_lines` in `@@PKG@@/diff.py` produce exactly the documented diff.",
    "`@@PKG@@/diff.py` still wraps difflib, but the review tool now promises the diff described in "
    "`docs/diff.md` (minimal, with a fixed tie-break and hunk format). Implement that exactly.",
)


def _options(rng):
    return {"context": rng.randint(0, 3), "delete_first": rng.random() < 0.5, "ignore_trailing": rng.random() < 0.5}


def _doc(pkg, options):
    delete, insert = "deleting the current old line, if some minimal script deletes it here", \
        "inserting the current new line, if some minimal script inserts it here"
    first, second = (delete, "inserting the current new line") if options["delete_first"] else \
        (insert, "deleting the current old line")
    return fill(DOC, PKG=pkg, CONTEXT=options["context"], TWICE=2 * options["context"], FIRST_RULE=first,
                SECOND_RULE=second,
                MATCH_RULE=("they are equal once trailing spaces and tabs are removed from each (leading "
                             "whitespace still counts)"
                            if options["ignore_trailing"] else "they are exactly equal"),
                KEPT_TEXT=("; a kept line is shown as it is in the new text" if options["ignore_trailing"] else ""))


def generate(seed, rng):
    pkg = package_name(rng, "review")
    options = _options(rng)
    solution = fill(SOLUTION, CONTEXT=options["context"], DELETE_FIRST=options["delete_first"],
                    IGNORE_TRAILING=options["ignore_trailing"])
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, CONTEXT=options["context"], REFERENCE=reference_block(solution))
    files = {"README.md": f"# {pkg}\n\nCode review helpers. Diff format: `docs/diff.md`.\n",
             f"{pkg}/__init__.py": "", f"{pkg}/diff.py": fill(STARTER, CONTEXT=options["context"]),
             "docs/diff.md": _doc(pkg, options), "tests/__init__.py": "",
             "tests/test_diff.py": fill(VISIBLE_TESTS, PKG=pkg, DELETE_FIRST=options["delete_first"])}
    return make_spec(family="algo", template="diff", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/diff.py": solution}, meta=options)
