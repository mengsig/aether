"""docspec / query: an issue-search language (precedence, negation, quoting, comparisons) from docs/search.md."""
from .common import fill, make_spec, package_name, reference_block

SOLUTION = r'''
"""Issue search; the query language is described in docs/search.md."""
import re
from datetime import date

OR_ANY_CASE = @@OR_ANY_CASE@@
NOT_WORD = @@NOT_WORD@@
WILDCARDS = @@WILDCARDS@@
BARE_LABELS = @@BARE_LABELS@@
MISSING = @@MISSING@@
TEXT_FIELDS = {"title", "status", "owner"}
NUMBER_FIELDS = {"priority": int, "estimate": float}
COMPARISON = re.compile(r"(>=|<=|>|<)?(.+)")


def _tokens(query):
    """Words, quoted text kept whole, and parentheses; a quote that never closes is an error."""
    tokens, index = [], 0
    while index < len(query):
        if query[index] in " \t":
            index += 1
            continue
        if query[index] in "()":
            tokens.append(query[index])
            index += 1
            continue
        start = index
        while index < len(query) and query[index] not in " \t()":
            if query[index] == '"':
                index = _closing_quote(query, index)
            index += 1
        tokens.append(query[start:index])
    return tokens


def _closing_quote(text, opening):
    index = opening + 1
    while index < len(text) and text[index] != '"':
        index += 2 if text[index] == "\\" else 1
    if index >= len(text):
        raise ValueError("unclosed quote")
    return index


def _unquote(text):
    """(value, quoted) for a term's text; escapes count only inside quotes."""
    if not text.startswith('"'):
        if '"' in text:
            raise ValueError(f"stray quote in {text!r}")
        return text, False
    closing = _closing_quote(text, 0)
    if closing != len(text) - 1:
        raise ValueError(f"text after a closing quote in {text!r}")
    value, index = [], 1
    while index < closing:
        if text[index] == "\\":
            index += 1
        value.append(text[index])
        index += 1
    return "".join(value), True


class _Parser:
    def __init__(self, tokens):
        self.tokens, self.position = tokens, 0

    def peek(self):
        return self.tokens[self.position] if self.position < len(self.tokens) else None

    def take(self):
        token = self.peek()
        self.position += 1
        return token

    def is_or(self, token):
        return token is not None and (token.upper() == "OR" if OR_ANY_CASE else token == "OR")

    def expression(self):
        branches = [self.conjunction()]
        while self.is_or(self.peek()):
            self.take()
            branches.append(self.conjunction())
        return ("or", branches)

    def conjunction(self):
        parts = []
        while self.peek() not in (None, ")") and not self.is_or(self.peek()):
            parts.append(self.unary())
        if not parts:
            raise ValueError("an operator or parenthesis is missing a term")
        return ("and", parts)

    def unary(self):
        token = self.take()
        if NOT_WORD and token == "NOT":
            if self.peek() in (None, ")") or self.is_or(self.peek()):
                raise ValueError("NOT needs a term")
            return ("not", self.unary())
        if token == "-":
            if self.peek() != "(":
                raise ValueError("a lone - is an error")
            return ("not", self.unary())
        if token.startswith("-"):
            return ("not", ("term", _term(token[1:])))
        if token == "(":
            inner = self.expression()
            if self.take() != ")":
                raise ValueError("unbalanced parentheses")
            return inner
        if token == ")":
            raise ValueError("unbalanced parentheses")
        return ("term", _term(token))


def _term(text):
    if not text:
        raise ValueError("empty term")
    field, colon, raw = text.partition(":")
    if not colon or field.startswith('"'):
        value, _ = _unquote(text)
        return ("bare", value.lower())
    field = field.lower()
    value, quoted = _unquote(raw)
    if not value:
        raise ValueError(f"no value for {field}")
    if field in NUMBER_FIELDS or field == "created":
        return _comparison(field, value, quoted)
    if field in TEXT_FIELDS or field == "label":
        return (field, value.lower(), WILDCARDS and not quoted and "*" in value)
    raise ValueError(f"unknown field {field!r}")


def _convert(field, text):
    try:
        return date.fromisoformat(text) if field == "created" else NUMBER_FIELDS[field](text)
    except ValueError as error:
        raise ValueError(f"bad {field} value {text!r}") from error


def _comparison(field, value, quoted):
    if quoted:
        raise ValueError(f"{field} takes no quoted value")
    if ".." in value:
        low, _, high = value.partition("..")
        low, high = _convert(field, low), _convert(field, high)
        if low > high:
            raise ValueError("a range runs low..high")
        return ("range", field, low, high)
    operator, text = COMPARISON.fullmatch(value).groups()
    return ("compare", field, operator or "=", _convert(field, text))


def _glob(pattern, text):
    return re.fullmatch(".*".join(re.escape(part) for part in pattern.split("*")), text) is not None


def _text_match(field, value, wild, item):
    if field == "label":
        candidates = [label.lower() for label in item["labels"]]
    else:
        current = item[field]
        if current is None:
            return value == "none" and field == "owner" and not wild
        candidates = [current.lower()]
    if wild:
        return any(_glob(value, candidate) for candidate in candidates)
    if field == "title":
        return any(value in candidate for candidate in candidates)
    return value in candidates


def _compare(operator, current, expected):
    return {"=": current == expected, ">": current > expected, ">=": current >= expected,
            "<": current < expected, "<=": current <= expected}[operator]


def _matches(node, item):
    kind = node[0]
    if kind == "or":
        return any(_matches(branch, item) for branch in node[1])
    if kind == "and":
        return all(_matches(part, item) for part in node[1])
    if kind == "not":
        return not _matches(node[1], item)
    term = node[1]
    if term[0] == "bare":
        words = [item["title"].lower()] + ([label.lower() for label in item["labels"]] if BARE_LABELS else [])
        return any(term[1] in word for word in words)
    if term[0] in ("range", "compare"):
        current = item[term[1]] if term[1] != "created" else date.fromisoformat(item["created"])
        if current is None:
            return MISSING
        if term[0] == "range":
            return term[2] <= current <= term[3]
        return _compare(term[2], current, term[3])
    return _text_match(term[0], term[1], term[2], item)


def search(query, items):
    """The items matching `query`, in their original order."""
    if not isinstance(query, str):
        raise TypeError("a query is text")
    tokens = _tokens(query)
    items = list(items)
    if not tokens:
        return items
    parser = _Parser(tokens)
    tree = parser.expression()
    if parser.peek() is not None:
        raise ValueError("unbalanced parentheses")
    return [item for item in items if _matches(tree, item)]
'''

STARTER = '''
"""Issue search; the query language is described in docs/search.md."""


def _matches(term, item):
    field, colon, value = term.partition(":")
    if not colon:
        return term.lower() in item["title"].lower()
    if field == "label":
        return value in item["labels"]
    if field == "priority":
        return item["priority"] == int(value)
    return str(item.get(field)).lower() == value.lower()


def search(query, items):
    """The items matching `query`, in their original order."""
    terms = query.split()
    return [item for item in items if all(_matches(term, item) for term in terms)]
'''

DOC = r'''
# Searching issues

`search(query, items)` returns a list of the issues matching a query, in their
original order. `items` may be any iterable of issues. An issue is a dict:

| Key | Type | Example |
| --- | --- | --- |
| `title` | str | `"Crash when saving"` |
| `status` | str | `"open"` |
| `owner` | str or `None` | `"dana"` |
| `labels` | list of str | `["bug", "ui"]` |
| `priority` | int | `2` |
| `estimate` | float or `None` (not estimated) | `1.5` |
| `created` | str, `YYYY-MM-DD` | `"2026-03-14"` |

An empty query (or only whitespace) matches every issue. A query that breaks
any rule below raises `ValueError`; nothing is silently ignored.

## Terms

- A bare word matches issues whose title contains it@@BARE_LABEL_RULE@@,
  ignoring case: `crash`.
- `"a quoted phrase"` is one bare term including its spaces. Inside quotes,
  `\"` stands for `"` and `\\` for `\`. A quote that never closes is an
  error, and so is a quote that ends before the term does (`"ab"c`).
- `field:value` tests one field; field names ignore case, and values may be
  quoted (`title:"save as"`). An empty value (`status:`) or an unknown field
  is an error:
  - `title:` contains the value, ignoring case.
  - `status:` and `owner:` equal the value, ignoring case. `owner:none`
    matches issues without an owner.
  - `label:` matches when any label equals the value, ignoring case.
  - `priority:`, `estimate:` and `created:` compare: `priority:2`,
    `priority:>2`, `estimate:<=1.5`, `created:>=2026-03-01`, and ranges
    `priority:1..3` (both ends included; `3..1` is an error). These take no
    quotes, and a value that is not a number (a date for `created`) is an
    error. @@MISSING_RULE@@
@@WILDCARD_RULE@@
## Combining terms

- Terms separated by whitespace must all match (AND).
- @@OR_RULE@@ AND binds tighter than OR: `a b OR c` is `(a b) OR (c)`.
- Parentheses group: `(a OR b) c`. They need no spaces around them, and they
  must balance.
- `-` directly before a term or a parenthesis negates it: `-label:bug`,
  `-(a OR b)`. A `-` standing alone is an error.@@NOT_RULE@@
- An OR with nothing on one side (`OR a`, `a OR`, `a OR OR b`) and empty
  parentheses are errors.
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.search import search

ISSUES = [
    {"title": "Crash when saving", "status": "open", "owner": "dana", "labels": ["bug"], "priority": 1,
     "estimate": 2.0, "created": "2026-03-14"},
    {"title": "Dark mode", "status": "closed", "owner": None, "labels": ["ui"], "priority": 3,
     "estimate": None, "created": "2026-01-02"},
]


class SearchTests(unittest.TestCase):
    def test_word(self):
        self.assertEqual(search("crash", ISSUES), [ISSUES[0]])

    def test_field(self):
        self.assertEqual(search("status:closed", ISSUES), [ISSUES[1]])
        self.assertEqual(search("label:bug priority:1", ISSUES), [ISSUES[0]])


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import random
import unittest

from @@PKG@@.search import search

@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)
WORDS = ["crash", "save", "dark", "mode", "login", "slow", "export", "csv", "menu", "Crash"]
LABELS = ["bug", "ui", "perf", "Docs", "security"]


def make_items(rng, count):
    items = []
    for _ in range(count):
        title = " ".join(rng.choice(WORDS) for _ in range(rng.randint(1, 4)))
        items.append({"title": title.capitalize(), "status": rng.choice(["open", "closed", "Blocked"]),
                      "owner": rng.choice([None, "dana", "Lee", "kim"]), "labels": rng.sample(LABELS, rng.randint(0, 3)),
                      "priority": rng.randint(0, 4), "estimate": rng.choice([None, 0.5, 1.0, 2.5, 8.0]),
                      "created": f"2026-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d}"})
    return items


ITEMS = make_items(random.Random(@@SEED@@), 40)


def ids(found):
    return [ITEMS.index(item) for item in found]


class HiddenSearchChecks(unittest.TestCase):
    def assert_like_reference(self, queries):
        for query in queries:
            expected = outcome(REFERENCE["search"], query, ITEMS)
            actual = outcome(search, query, ITEMS)
            if expected[0] == "value" and actual[0] == "value":
                self.assertEqual(ids(actual[1]), ids(expected[1]), query)
            else:
                self.assertEqual(actual[0], expected[0], query)

@@CHECKS@@

    def test_generated_queries(self):
        rng = random.Random(@@SEED@@ + 1)
        mismatches = []
        for _ in range(300):
            query = random_query(rng, 0)
            expected, actual = outcome(REFERENCE["search"], query, ITEMS), outcome(search, query, ITEMS)
            same = (ids(actual[1]) == ids(expected[1]) if expected[0] == actual[0] == "value"
                    else actual[0] == expected[0])
            if not same:
                mismatches.append((query, expected[0], actual[0]))
        self.assertEqual(mismatches[:4], [], f"{len(mismatches)} of 300 generated queries differ")

    def test_input_order_and_iterables(self):
        found = search("priority:>=0", iter(ITEMS))
        self.assertEqual(ids(found), list(range(len(ITEMS))))


def random_comparison(rng):
    field = rng.choice(["priority", "estimate", "created"])
    if field == "created":
        single, low, high = f"2026-{rng.randint(1, 12):02d}-01", "2026-02-01", f"2026-0{rng.randint(1, 9)}-01"
    elif field == "priority":
        single, low, high = str(rng.randint(0, 4)), str(rng.randint(0, 2)), str(rng.randint(1, 4))
    else:
        single, low, high = rng.choice(["0.5", "1", "2.5"]), "0", rng.choice(["1", "2.5", "9"])
    if rng.random() < 0.3:
        return f"{field}:{low}..{high}"
    return f"{field}:{rng.choice(['', '', '>', '>=', '<', '<='])}{single}"


def random_term(rng):
    kind = rng.random()
    if kind < 0.2:
        return rng.choice(WORDS).lower() if rng.random() < 0.7 else rng.choice(WORDS).upper()
    if kind < 0.3:
        return \'"\' + " ".join(rng.choice(WORDS) for _ in range(2)) + \'"\'
    if kind < 0.55:
        field = rng.choice(["status", "owner", "label", "title", "Status", "LABEL"])
        value = {"status": rng.choice(["open", "closed", "blocked", "op*", "*e*"]),
                 "owner": rng.choice(["dana", "LEE", "none", "k*"]), "label": rng.choice(LABELS + ["d*", "*u*"]),
                 "title": rng.choice(WORDS + ["sav*", \'"dark mode"\'])}[field.lower()]
        return f"{field}:{value}"
    return random_comparison(rng)


def random_query(rng, depth):
    parts = []
    for _ in range(rng.randint(1, 3)):
        if depth < 2 and rng.random() < 0.25:
            inner = random_query(rng, depth + 1)
            part = rng.choice(["(", "-(", "NOT ("]) + inner + ")"
        else:
            part = random_term(rng)
            roll = rng.random()
            part = ("-" + part) if roll < 0.15 else (("NOT " + part) if roll < 0.22 else part)
        parts.append(part)
        if rng.random() < 0.3:
            parts.append(rng.choice(["OR", "OR", "or", "Or"]))
    if parts[-1].upper() == "OR" and rng.random() < 0.8:
        parts.pop()
    return " ".join(parts) if rng.random() < 0.9 else "  ".join(parts) + rng.choice(["", " )", " ("])
'''

CHECKS = {
    "precedence": ["crash save OR dark", "crash OR save dark", "(crash OR save) dark", "crash (save OR dark) slow",
                   "label:bug OR label:ui priority:>2"],
    "negation": ["-label:bug", "-(crash OR save)", "crash -status:closed", "-(-label:ui)", "- crash",
                 "NOT label:bug", "NOT (crash OR dark)", "NOT", "crash NOT"],
    "or_keyword": ["crash or dark", "crash Or dark", "crash OR dark", "OR crash", "crash OR", "crash OR OR dark"],
    "quotes": ['"dark mode"', 'title:"crash crash"', '"unclosed', '"ab"c', 'title:"a\\"b"', 'status:"open"',
               '"save\\\\"'],
    "fields_and_case": ["Status:OPEN", "OWNER:dana", "owner:none", "owner:None", "label:docs", "LABEL:BUG",
                        "title:CRASH", "colour:red", "status:", "title:save"],
    "comparisons": ["priority:2", "priority:>2", "priority:>=2", "priority:<1", "priority:<=0", "priority:1..3",
                    "priority:3..1", "priority:high", 'priority:"2"', "created:>=2026-06-01",
                    "created:2026-02-01..2026-04-30", "created:soon", "estimate:2.5", "estimate:<1"],
    "missing_values": ["estimate:>1", "-estimate:>1", "estimate:<100", "-estimate:0..100", "owner:none -owner:dana"],
    "wildcards": ["status:op*", "label:*u*", "owner:k*", "title:sav*", 'title:"sav*"', "status:*"],
    "parentheses": ["(crash)", "((crash OR dark))", "(crash", "crash)", "()", "(crash OR)", "(crash)(dark)",
                    "label:bug(crash)"],
    "bare_words": ["bug", "ui", "CRASH", '"crash save"', "docs", "perf crash"],
}
CHECK_METHOD = '''
    def test_@@NAME@@(self):
        self.assert_like_reference(@@QUERIES@@)
'''

PROMPTS = (
    "The issue search in `@@PKG@@/search.py` understands only a fraction of the query language users are told "
    "about in `docs/search.md`, and gets some of that wrong. Implement the whole language as documented.",
    "Users keep reporting that queries from our search help page (`docs/search.md`) return the wrong issues or "
    "crash. Make `search` in `@@PKG@@/search.py` follow every documented rule.",
)


def _options(rng):
    return {"or_any_case": rng.random() < 0.5, "not_word": rng.random() < 0.5, "wildcards": rng.random() < 0.5,
            "bare_labels": rng.random() < 0.5, "missing": rng.random() < 0.3}


def _doc(options):
    wildcard = ("- In values of `title:`, `status:`, `owner:` and `label:`, an unquoted `*` matches any run of\n"
                "  characters, and the pattern must match the whole value (for `title:` too): `status:op*`.\n"
                "  A quoted `*` is a plain character.\n" if options["wildcards"] else
                "- `*` has no special meaning: it is matched as a plain character.\n")
    return fill(DOC,
                BARE_LABEL_RULE=" or one of its labels contains it" if options["bare_labels"] else "",
                MISSING_RULE=("An issue without an estimate matches every `estimate:` comparison."
                              if options["missing"] else
                              "An issue without an estimate matches no `estimate:` comparison (so it does match "
                              "`-estimate:>1`)."),
                WILDCARD_RULE=wildcard,
                OR_RULE=("`OR` (in any letter case: `or`, `Or`) between terms matches when either side does."
                         if options["or_any_case"] else
                         "`OR`, in capitals, between terms matches when either side does; `or` is an ordinary word."),
                NOT_RULE=("\n- `NOT` (in capitals) followed by a term or parenthesis negates it too: `NOT label:bug`. "
                          "`NOT` with nothing after it is an error." if options["not_word"] else
                          " `NOT` is an ordinary word."))


def generate(seed, rng):
    pkg = package_name(rng, "issues")
    options = _options(rng)
    solution = fill(SOLUTION, OR_ANY_CASE=options["or_any_case"], NOT_WORD=options["not_word"],
                    WILDCARDS=options["wildcards"], BARE_LABELS=options["bare_labels"], MISSING=options["missing"])
    checks = "".join(fill(CHECK_METHOD, dedent=False, NAME=name, QUERIES=repr(queries))
                     for name, queries in CHECKS.items())
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, REFERENCE=reference_block(solution), CHECKS=checks.rstrip("\n"))
    files = {"README.md": f"# {pkg}\n\nIssue tracker helpers. Query language: `docs/search.md`.\n",
             f"{pkg}/__init__.py": "", f"{pkg}/search.py": fill(STARTER), "tests/__init__.py": "",
             "tests/test_search.py": fill(VISIBLE_TESTS, PKG=pkg), "docs/search.md": _doc(options)}
    return make_spec(family="docspec", template="query", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/search.py": solution}, meta=options)
