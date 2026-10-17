"""edgecase / slug: URL slugs whose docstring rules (length, collisions, transliteration) the visible tests skip."""
from .common import fill, make_spec, package_name

CONTRACT = '''
"""URL slugs for @@PKG@@ articles.

`slugify(title, max_length=@@MAX@@)` turns a title into a slug:

1. Unicode is decomposed (NFKD) and combining marks are dropped (`é` -> `e`).
   These letters are transliterated: @@TRANSLIT_WORDS@@. Any other non-ASCII
   character is dropped.
2. Apostrophes (`'` and `’`) are removed without a trace: `Don't` -> `dont`.
3. Letters are lower-cased; every run of other characters that are not ASCII
   letters or digits becomes one `@@SEP@@`, and `@@SEP@@` never starts or ends a slug.
4. A slug longer than `max_length` is cut at the last `@@SEP@@` that keeps it within
   `max_length`, so words are never split; if even the first word is too long,
   that word is cut at `max_length`.
5. A title that leaves nothing is slugged as `@@FALLBACK@@`.

`unique_slug(title, taken, max_length=@@MAX@@)` returns `slugify(title, max_length)`
unless that slug is in `taken` (any iterable of slugs, read once). It then
appends `@@SEP@@2`, `@@SEP@@3`, ... and returns the first one not taken. The
whole result, suffix included, still fits `max_length`: the slug in front of
the suffix is shortened by rule 4 to make room.
"""
import unicodedata
'''

SOLUTION = CONTRACT + '''import re

SEPARATOR = @@SEP_REPR@@
FALLBACK = @@FALLBACK_REPR@@
TRANSLITERATE = @@TRANSLIT@@


def _shorten(slug, max_length):
    if len(slug) <= max_length:
        return slug
    cut = slug.rfind(SEPARATOR, 0, max_length + 1)
    return slug[:cut] if cut > 0 else slug[:max_length]


def slugify(title, max_length=@@MAX@@):
    text = "".join(TRANSLITERATE.get(char, char) for char in title)
    text = unicodedata.normalize("NFKD", text)
    text = "".join(char for char in text if char.isascii() and not unicodedata.combining(char))
    text = re.sub(r"[^a-z0-9]+", SEPARATOR, text.replace("'", "").lower()).strip(SEPARATOR)
    return _shorten(text, max_length) or FALLBACK


def unique_slug(title, taken, max_length=@@MAX@@):
    taken = set(taken)
    slug = slugify(title, max_length)
    number = 2
    candidate = slug
    while candidate in taken:
        suffix = f"{SEPARATOR}{number}"
        candidate = _shorten(slug, max_length - len(suffix)) + suffix
        number += 1
    return candidate
'''

STARTER = CONTRACT + '''
import re


def slugify(title, max_length=@@MAX@@):
    text = unicodedata.normalize("NFKD", title).encode("ascii", "ignore").decode()
    slug = re.sub(r"[^a-z0-9]+", @@SEP_REPR@@, text.lower()).strip(@@SEP_REPR@@)
    return slug[:max_length]


def unique_slug(title, taken, max_length=@@MAX@@):
    slug = slugify(title, max_length)
    number = 2
    candidate = slug
    while candidate in taken:
        candidate = f"{slug}@@SEP@@{number}"
        number += 1
    return candidate
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.slugs import slugify, unique_slug


class SlugTests(unittest.TestCase):
    def test_simple_title(self):
        self.assertEqual(slugify("Hello World"), "hello@@SEP@@world")

    def test_accents(self):
        self.assertEqual(slugify("Café au lait"), "cafe@@SEP@@au@@SEP@@lait")

    def test_unique(self):
        self.assertEqual(unique_slug("Hello World", {"hello@@SEP@@world"}), "hello@@SEP@@world@@SEP@@2")


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import re
import unittest

from @@PKG@@.slugs import slugify, unique_slug

SEP = @@SEP_REPR@@


class HiddenSlugChecks(unittest.TestCase):
    def test_apostrophes_vanish(self):
        self.assertEqual(slugify("Don't Stop"), f"dont{SEP}stop")
        self.assertEqual(slugify("Rock \\u2019n\\u2019 Roll"), f"rock{SEP}n{SEP}roll")

    def test_transliteration_and_other_scripts(self):
@@TRANSLIT_CHECK@@
        self.assertEqual(slugify("Ünïcödé — naïve"), f"unicode{SEP}naive")
        self.assertEqual(slugify("Tokyo 東京 2026"), f"tokyo{SEP}2026")

    def test_runs_and_ends(self):
        self.assertEqual(slugify("  --Hello,   World!!--  "), f"hello{SEP}world")
        self.assertEqual(slugify("a_b-c.d"), SEP.join("abcd"))

    def test_fallback(self):
        for title in ("", "!!!", "東京", "'"):
            self.assertEqual(slugify(title), @@FALLBACK_REPR@@)

    def test_cut_at_word_boundary(self):
        title = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron"
        slug = slugify(title, max_length=20)
        self.assertEqual(slug, SEP.join(["alpha", "beta", "gamma"]))
        self.assertEqual(slugify("one two three", max_length=7), f"one{SEP}two")
        self.assertEqual(slugify("one two three", max_length=8), f"one{SEP}two")

    def test_first_word_too_long(self):
        self.assertEqual(slugify("Supercalifragilistic day", max_length=10), "supercalif")

    def test_unique_counts_up_and_reads_taken_once(self):
        taken = iter([f"hello{SEP}world", f"hello{SEP}world{SEP}2", f"hello{SEP}world{SEP}3"])
        self.assertEqual(unique_slug("Hello World", taken), f"hello{SEP}world{SEP}4")
        self.assertEqual(unique_slug("Fresh", [f"hello{SEP}world"]), "fresh")

    def test_unique_suffix_fits_the_limit(self):
        taken = {f"alpha{SEP}beta{SEP}gamma"}
        result = unique_slug("Alpha Beta Gamma", taken, max_length=16)
        self.assertEqual(result, f"alpha{SEP}beta{SEP}2")
        long_taken = {"abcdefghij"}
        self.assertEqual(unique_slug("abcdefghijklmnop", long_taken, max_length=10), f"abcdefgh{SEP}2")

    def test_every_slug_is_well_formed(self):
        shape = re.compile(r"[a-z0-9]+(" + re.escape(SEP) + r"[a-z0-9]+)*")
        titles = ["A  B", "Ça va? Ça va!", "100% organic -- 50% off", "x" * 90, "end-", "-start", "Ωmega 3"]
        for title in titles:
            for limit in (5, 12, 40):
                slug = slugify(title, max_length=limit)
                self.assertTrue(shape.fullmatch(slug), (title, limit, slug))
                self.assertLessEqual(len(slug), limit, (title, limit, slug))
'''

TRANSLITERATIONS = [{"ß": "ss", "æ": "ae", "ø": "o", "ł": "l"}, {"ß": "ss", "œ": "oe", "đ": "d"},
                    {"æ": "ae", "ø": "oe", "å": "aa"}]
TRANSLIT_CHECKS = {
    0: '        self.assertEqual(slugify("Straße Ærø Łódź"), f"strasse{SEP}aero{SEP}lodz")',
    1: '        self.assertEqual(slugify("Straße Œuvre Đorđe"), f"strasse{SEP}oeuvre{SEP}dorde")',
    2: '        self.assertEqual(slugify("Ærø Ålborg"), f"aeroe{SEP}aalborg")',
}

PROMPTS = (
    "Editors keep getting broken or clashing article URLs from `@@PKG@@/slugs.py`. The visible tests "
    "pass, but the functions do not follow the rules written in the module docstring. Fix `slugify` "
    "and `unique_slug`.",
    "Bring `slugify` and `unique_slug` in `@@PKG@@/slugs.py` in line with the documented slug rules.",
)


def generate(seed, rng):
    pkg = package_name(rng, "articles")
    separator = rng.choice(["-", "_"])
    fallback = rng.choice(["untitled", "n-a" if separator == "-" else "n_a", "post"])
    choice = rng.randrange(len(TRANSLITERATIONS))
    table = TRANSLITERATIONS[choice]
    upper = {key.upper(): value for key, value in table.items() if key.upper() != key and len(key.upper()) == 1}
    table = {**table, **upper}
    words = ", ".join(f"`{key}` -> `{value}`" for key, value in TRANSLITERATIONS[choice].items())
    contract = {"PKG": pkg, "MAX": 60, "TRANSLIT_WORDS": words + " (and their capitals)",
                "SEP": separator, "FALLBACK": fallback}
    starter = fill(STARTER, **contract, SEP_REPR=repr(separator))
    solution = fill(SOLUTION, **contract, SEP_REPR=repr(separator), FALLBACK_REPR=repr(fallback), TRANSLIT=repr(table))
    hidden = fill(HIDDEN, PKG=pkg, SEP_REPR=repr(separator), FALLBACK_REPR=repr(fallback),
                  TRANSLIT_CHECK=TRANSLIT_CHECKS[choice])
    files = {"README.md": f"# {pkg}\n\nPublishing helpers.\n", f"{pkg}/__init__.py": "",
             f"{pkg}/slugs.py": starter, "tests/__init__.py": "",
             "tests/test_slugs.py": fill(VISIBLE_TESTS, PKG=pkg, SEP=separator)}
    return make_spec(family="edgecase", template="slug", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/slugs.py": solution})
