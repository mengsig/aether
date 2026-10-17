"""docspec / config: a small key-value config format whose rules live only in docs/format.md."""
from .common import fill, make_spec, package_name

SOLUTION = '''
"""Configuration parser; the format is described in docs/format.md."""
import re

COMMENT_CHARS = @@COMMENTS@@
INLINE_COMMENTS = @@INLINE@@
SECTIONS = @@SECTIONS@@
DUPLICATES = @@DUPLICATES@@
TYPED = @@TYPED@@
EMPTY_IS_NONE = @@EMPTY_NONE@@
CONTINUATION = @@CONTINUATION@@
INTERPOLATION = @@INTERPOLATION@@
LEGACY = @@LEGACY@@
TRUE_WORDS, FALSE_WORDS = {"true", "yes", "on"}, {"false", "no", "off"}
INTEGER = re.compile(r"[+-]?(0|[1-9][0-9]*)")


class ConfigError(ValueError):
    pass


def _logical_lines(text):
    pending, start = None, 0
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip() if pending is None else pending + " " + raw.strip()
        start = number if pending is None else start
        if CONTINUATION and line.endswith("\\\\"):
            pending = line[:-1].rstrip()
            continue
        pending = None
        yield start, line
    if pending is not None:
        yield start, pending


def _strip_inline_comment(value):
    if not INLINE_COMMENTS or value.startswith('"'):
        return value
    match = re.search(r"\\s#", value)
    return value[:match.start()].rstrip() if match else value


def _unquote(value, number):
    end, out, index = None, [], 1
    while index < len(value):
        char = value[index]
        if char == "\\\\" and index + 1 < len(value) and value[index + 1] in '"\\\\':
            out.append(value[index + 1])
            index += 2
            continue
        if char == '"':
            end = index
            break
        out.append(char)
        index += 1
    if end is None:
        raise ConfigError(f"line {number}: unterminated quoted value")
    rest = value[end + 1:].strip()
    if rest and not (INLINE_COMMENTS and rest.startswith("#")):
        raise ConfigError(f"line {number}: text after a quoted value")
    return "".join(out)


def _convert(value):
    if value == "":
        return None if EMPTY_IS_NONE else ""
    if TYPED and value.lower() in TRUE_WORDS | FALSE_WORDS:
        return value.lower() in TRUE_WORDS
    if TYPED and INTEGER.fullmatch(value):
        return int(value)
    return value


def _split_pair(line, number):
    separator = "=" if "=" in line or not LEGACY else ":"
    if separator not in line:
        raise ConfigError(f"line {number}: expected key = value")
    key, value = (part.strip() for part in line.split(separator, 1))
    if LEGACY and key.lower().startswith("export "):
        key = key[len("export "):].strip()
    if not key:
        raise ConfigError(f"line {number}: empty key")
    return key.lower(), value


def _entries(text):
    entries, section = {}, None
    for number, line in _logical_lines(text):
        if not line or line[0] in COMMENT_CHARS:
            continue
        if SECTIONS and line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip().lower()
            if not section:
                raise ConfigError(f"line {number}: empty section name")
            continue
        key, value = _split_pair(line, number)
        key = key if section is None else f"{section}.{key}"
        value = _strip_inline_comment(value)
        if key in entries and DUPLICATES == "error":
            raise ConfigError(f"line {number}: duplicate key {key}")
        if key in entries and DUPLICATES == "first":
            continue
        quoted = value.startswith('"')
        entries[key] = (_unquote(value, number) if quoted else value, quoted)
    return entries


def _expand(key, entries, active):
    raw, quoted = entries[key]
    if quoted or not INTERPOLATION:
        return raw
    if key in active:
        raise ConfigError(f"interpolation cycle through {key}")
    active.add(key)

    def replace(match):
        if match.group(0) == "$$":
            return "$"
        name = match.group(1)
        if name not in entries:
            raise ConfigError(f"unknown key {name} referenced from {key}")
        return _expand(name, entries, active)

    text = re.sub(r"\\$\\$|\\$\\{([^}]*)\\}", replace, raw)
    active.discard(key)
    return text


def parse(text):
    """Parse configuration text into a flat dict (see docs/format.md)."""
    entries = _entries(text)
    result = {}
    for key, (raw, quoted) in entries.items():
        result[key] = raw if quoted else _convert(_expand(key, entries, set()))
    return result
'''

STARTER = '''
"""Configuration parser; the format is described in docs/format.md."""


class ConfigError(ValueError):
    pass


def parse(text):
    """Parse configuration text into a flat dict (see docs/format.md)."""
    result = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        key, value = line.split("=", 1)
        result[key.strip()] = value.strip()
    return result
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.parser import parse


class ParseTests(unittest.TestCase):
    def test_pairs(self):
        self.assertEqual(parse("name = demo\\nmode = fast\\n"), {"name": "demo", "mode": "fast"})

    def test_blank_lines_and_comments(self):
        self.assertEqual(parse("\\n# a comment\\nname = demo\\n\\n"), {"name": "demo"})


if __name__ == "__main__":
    unittest.main()
'''

DOC_HEAD = '''
# Configuration format

`parse(text)` reads a configuration file into a flat `dict`. Every rule below
is part of the format; programs that read our files depend on all of them.

## Lines

- Leading and trailing whitespace on a line is ignored, and blank lines are skipped.
- A line whose first non-blank character is @@COMMENT_WORDS@@ is a comment.
- Every other line is `key = value`. Whitespace around the key and the value is
  ignored. Keys are case-insensitive: `Name` and `NAME` are both stored as `name`.
- A line that is not a comment and has no `=`@@COLON_WORDS@@, or has an empty key, is an error:
  raise `ConfigError` (a `ValueError`) whose message contains `line N`, the
  1-based line number of the offending line.
'''

RULE_DOCS = {
    "interpolation": '''
## References

An unquoted value may refer to another key as `${key}` (the full key, with its
section prefix, such as `${server.host}`). The reference is replaced by the
referenced key's value text as written in the file (before any conversion;
for a quoted value, its text without the quotes, never expanded), and
references inside unquoted text are expanded in turn; a key may refer to keys
defined later in the file.
The expanded text is then treated like any unquoted value. `$$` stands for a
literal `$`. Quoted values are never expanded. A reference to a key that does
not exist, or a chain of references that leads back to itself, is an error
(`ConfigError`).
''',
    "legacy_pointer": '''
## Older files

Files written by our previous tooling use a few older forms that are still
accepted; they are listed in `docs/compat.md`.
''',
    "inline": '''
## Inline comments

Outside quotes, a `#` preceded by whitespace starts a comment that runs to the
end of the line: `host = example.org  # primary` gives `"example.org"`. A `#`
with no whitespace before it is part of the value: `url = http://h/#top`.
''',
    "no_inline": '''
## No inline comments

A `#` inside a value is always part of the value: `colour = #ff0000 # red` gives
`"#ff0000 # red"`. Only whole-line comments exist.
''',
    "sections": '''
## Sections

`[name]` starts a section; surrounding whitespace inside the brackets is
ignored and section names are lower-cased. Keys that follow are stored as
`section.key`: `[Server]` then `Port = 1` gives `server.port`. Keys before the
first section have no prefix. `[]` is an error (`ConfigError`, with `line N`).
''',
    "quotes": '''
## Quoted values

A value that starts with `"` is quoted: it ends at the next unescaped `"`, the
quotes are removed and everything inside is kept as written, including
whitespace and `#`. Inside quotes, `\\"` stands for `"` and `\\\\` for `\\`.
A quoted value is always a string. A missing closing quote, or anything but
whitespace@@QUOTE_COMMENT@@ after the closing quote, is an error (`ConfigError`).
''',
    "typed": '''
## Typed values

Unquoted values are converted: `true`, `yes`, `on` become `True` and `false`,
`no`, `off` become `False`, in any letter case; an optional sign followed by
digits becomes an `int`, except that a number with a leading zero (`007`)
stays a string. Every other value is a string.
''',
    "untyped": '''
## Values are strings

Values are never converted: `port = 80` gives the string `"80"` and
`debug = true` the string `"true"`.
''',
    "empty_none": '''
## Empty values

`key =` with nothing after the `=` gives `None`. A quoted empty value `""` gives `""`.
''',
    "empty_string": '''
## Empty values

`key =` with nothing after the `=` gives the empty string `""`.
''',
    "continuation": '''
## Continued lines

A line that ends with `\\` continues on the next line: the backslash is
dropped and the two parts are joined with exactly one space, whatever
whitespace surrounded the break. A line number in an error is that of the
first line of the continued line.
''',
    "dup_last": '''
## Repeated keys

When a key appears more than once, the last value wins.
''',
    "dup_first": '''
## Repeated keys

When a key appears more than once, the first value wins and later ones are ignored.
''',
    "dup_error": '''
## Repeated keys

A key may appear only once (after section prefixes are applied); a repeat is an
error (`ConfigError`, with the `line N` of the repeat).
''',
}

HIDDEN_HEAD = '''
import unittest

from @@PKG@@.parser import ConfigError, parse


class HiddenConfigChecks(unittest.TestCase):
    def test_keys_are_trimmed_and_case_insensitive(self):
        self.assertEqual(parse("  Name =  demo app  \\n\\tMODE=fast\\n"), {"name": "demo app", "mode": "fast"})

    def test_line_without_equals_is_an_error_naming_its_line(self):
        self.assertTrue(issubclass(ConfigError, ValueError))
        with self.assertRaises(ConfigError) as caught:
            parse("a = x\\n\\nbroken line\\n")
        self.assertIn("line 3", str(caught.exception))
        with self.assertRaises(ConfigError):
            parse(" = x\\n")
'''

CHECKS = {
    "interpolation": '''
    def test_references_expand(self):
        text = "url = http://${host}:${port}/x\\nhost = example.org\\nport = 8080\\nprice = $$5\\nq = \\"${host}\\"\\n"
        result = parse(text)
        self.assertEqual(result["url"], "http://example.org:8080/x")
        self.assertEqual(result["price"], "$5")
        self.assertEqual(result["q"], "${host}")

    def test_bad_references_are_errors(self):
        for text in ("a = ${missing}\\n", "a = ${b}\\nb = ${c}\\nc = ${a}\\n"):
            with self.assertRaises(ConfigError):
                parse(text)
''',
    "legacy": '''
    def test_older_forms(self):
        self.assertEqual(parse("Name: demo\\nexport MODE = fast\\nurl: http://h:1/\\n"),
                         {"name": "demo", "mode": "fast", "url": "http://h:1/"})
''',
    "comments_hash": '''
    def test_comment_lines(self):
        self.assertEqual(parse("   # indented\\nx = v\\n#x = w\\n"), {"x": "v"})
        with self.assertRaises(ConfigError):
            parse("; not a comment in this format\\n")
''',
    "comments_both": '''
    def test_comment_lines(self):
        self.assertEqual(parse("   # indented\\n; semicolon\\n  ;x = w\\nx = v\\n"), {"x": "v"})
''',
    "inline": '''
    def test_inline_comments(self):
        self.assertEqual(parse("host = example.org  # primary\\nurl = http://h/#top\\nt = a\\t# tab\\n"),
                         {"host": "example.org", "url": "http://h/#top", "t": "a"})
        self.assertEqual(parse('q = "x # y"   # c\\n'), {"q": "x # y"})
''',
    "no_inline": '''
    def test_hash_inside_values_is_kept(self):
        self.assertEqual(parse("colour = #ff0000 # red\\n"), {"colour": "#ff0000 # red"})
''',
    "sections": '''
    def test_sections_prefix_keys(self):
        self.assertEqual(parse("top = a\\n[ Server ]\\nHost = h\\n[db]\\nhost = d\\n"),
                         {"top": "a", "server.host": "h", "db.host": "d"})
        with self.assertRaises(ConfigError) as caught:
            parse("a = b\\n[ ]\\n")
        self.assertIn("line 2", str(caught.exception))
''',
    "quotes": '''
    def test_quoted_values(self):
        self.assertEqual(parse('motto = "  keep # this  "\\n'), {"motto": "  keep # this  "})
        self.assertEqual(parse('q = "say \\\\"hi\\\\" \\\\\\\\ bye"\\n'), {"q": 'say "hi" \\\\ bye'})
        self.assertEqual(parse('flag = "yes"\\nnum = "42"\\n'), {"flag": "yes", "num": "42"})

    def test_bad_quotes_are_errors(self):
        for text in ('a = "open\\n', 'a = "x" y\\n'):
            with self.assertRaises(ConfigError):
                parse(text)
''',
    "typed": '''
    def test_typed_values(self):
        self.assertEqual(parse("a = TRUE\\nb = off\\nc = -12\\nd = 007\\ne = 1.5\\nf = +3\\ng = 0\\nh = Yes\\n"),
                         {"a": True, "b": False, "c": -12, "d": "007", "e": "1.5", "f": 3, "g": 0, "h": True})
''',
    "untyped": '''
    def test_values_stay_strings(self):
        self.assertEqual(parse("a = true\\nc = -12\\n"), {"a": "true", "c": "-12"})
''',
    "empty_none": '''
    def test_empty_values(self):
        self.assertEqual(parse('a =\\nb = ""\\n'), {"a": None, "b": ""})
''',
    "empty_string": '''
    def test_empty_values(self):
        self.assertEqual(parse('a =\\nb = ""\\n'), {"a": "", "b": ""})
''',
    "continuation": '''
    def test_continued_lines(self):
        self.assertEqual(parse("path = /a \\\\\\n     /b\\\\\\n/c\\nx = y\\n"), {"path": "/a /b /c", "x": "y"})
        with self.assertRaises(ConfigError) as caught:
            parse("ok = v\\nbad \\\\\\n  line\\n")
        self.assertIn("line 2", str(caught.exception))
''',
    "dup_last": '''
    def test_repeated_keys(self):
        self.assertEqual(parse("a = one\\nA = two\\n"), {"a": "two"})
''',
    "dup_first": '''
    def test_repeated_keys(self):
        self.assertEqual(parse("a = one\\nA = two\\n"), {"a": "one"})
''',
    "dup_error": '''
    def test_repeated_keys(self):
        with self.assertRaises(ConfigError) as caught:
            parse("a = one\\nb = x\\nA = two\\n")
        self.assertIn("line 3", str(caught.exception))
''',
}

COMPAT_DOC = '''
# Older forms

The previous tooling wrote files that are still accepted:

- `key: value`: on a line with no `=`, the first `:` separates key and value
  (`url: http://h:1/` gives the key `url` and the value `http://h:1/`).
- `export KEY = value`: a leading `export ` before the key is ignored, so this
  gives the key `key`.
'''

README = '''
# @@PKG@@

Reads the configuration files of our services. The file format is specified in
`docs/format.md`. Run the tests with `python3 -m unittest discover -s tests -t .`.
'''

PROMPTS = (
    "The configuration parser in `@@PKG@@/parser.py` is a stub that only handles the simplest "
    "files, yet the tests in `tests/` pass. Implement the full file format described in `docs/format.md`.",
    "Our services fail to load some configuration files, or load wrong values, through "
    "`@@PKG@@.parser.parse`: it only handles the simplest files, although `tests/` pass. "
    "Make `parse` support the real format.",
)


def _options(rng):
    return {"interpolation": rng.random() < 0.7, "legacy": rng.random() < 0.6,"comments": rng.choice(["#", "#;"]), "inline": rng.random() < 0.6,
            "sections": rng.random() < 0.6, "duplicates": rng.choice(["last", "first", "error"]),
            "typed": rng.random() < 0.6, "empty_none": rng.random() < 0.5,
            "continuation": rng.random() < 0.5}


def _rules(options):
    rules = ["comments_hash" if options["comments"] == "#" else "comments_both",
             "inline" if options["inline"] else "no_inline", "quotes",
             "typed" if options["typed"] else "untyped",
             "empty_none" if options["empty_none"] else "empty_string", f"dup_{options['duplicates']}"]
    if options["sections"]:
        rules.append("sections")
    if options["continuation"]:
        rules.append("continuation")
    if options["interpolation"]:
        rules.append("interpolation")
    if options["legacy"]:
        rules.append("legacy")
    return rules


def _docs(options, rules):
    words = "`#`" if options["comments"] == "#" else "`#` or `;`"
    colon = " (nor a `:` in an older form, see below)" if options["legacy"] else ""
    parts = [fill(DOC_HEAD, COMMENT_WORDS=words, COLON_WORDS=colon)]
    for rule in rules:
        key = rule.removeprefix("comments_hash").removeprefix("comments_both")
        key = "legacy_pointer" if key == "legacy" else key
        if not key:
            continue
        text = RULE_DOCS[key]
        values = {"QUOTE_COMMENT": " or an inline comment" if options["inline"] else ""} if key == "quotes" else {}
        parts.append(fill(text, **values))
    return "\n".join(parts)


def generate(seed, rng):
    pkg = package_name(rng, "config")
    options = _options(rng)
    rules = _rules(options)
    solution = fill(SOLUTION, COMMENTS=repr(options["comments"]), INLINE=options["inline"],
                    SECTIONS=options["sections"], DUPLICATES=repr(options["duplicates"]),
                    TYPED=options["typed"], EMPTY_NONE=options["empty_none"], CONTINUATION=options["continuation"],
                    INTERPOLATION=options["interpolation"], LEGACY=options["legacy"])
    hidden = fill(HIDDEN_HEAD, PKG=pkg) + "".join(CHECKS[rule] for rule in rules)
    files = {"README.md": fill(README, PKG=pkg), f"{pkg}/__init__.py": "",
             f"{pkg}/parser.py": fill(STARTER), "tests/__init__.py": "",
             "tests/test_parser.py": fill(VISIBLE_TESTS, PKG=pkg), "docs/format.md": _docs(options, rules)}
    if options["legacy"]:
        files["docs/compat.md"] = fill(COMPAT_DOC)
    return make_spec(family="docspec", template="config", seed=seed,
                     prompt=fill(rng.choice(PROMPTS), PKG=pkg), files=files, hidden=hidden,
                     solution={f"{pkg}/parser.py": solution})
