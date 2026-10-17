"""port / pricing: finish an exact Python port of a legacy JavaScript module, quirks included.

The legacy code coerces with parseInt, Number or parseFloat, rounds with
Math.round, prints with toFixed and orders keys as Object.keys does. A port
that reads naturally in Python differs on many inputs; the hidden checks
compare against a reference that reproduces JavaScript's semantics exactly.
"""
from .common import fill, make_spec, package_name, reference_block

LEGACY = r'''
'use strict';
// Pricing summaries for the storefront.

const REGION_TAX = @@TAX_JS@@;
const COUPON_PREFIX = '@@COUPON@@';

function quantity(value) {
  const n = @@QTY_JS@@;
  return isNaN(n) || n < 0 ? 0 : n;
}

function unitPrice(value) {
  return @@PRICE_JS@@ || 0;
}

function lineTotal(order) {
  let total = quantity(order.qty) * unitPrice(order.price);
  if (order.coupon && String(order.coupon).startsWith(COUPON_PREFIX)) {
    total = total / 2;
  }
  return @@ROUND_JS@@;
}

function summarize(orders) {
  const bySku = {};
  let gross = 0;
  for (const order of orders) {
    const sku = String(order.sku).trim().toLowerCase();
    const total = lineTotal(order);
    bySku[sku] = (bySku[sku] || 0) + total;
    gross += total;
  }
  const region = (orders.length && orders[0].region) || '@@DEFAULT_REGION@@';
  const rate = REGION_TAX[region] || 0;
  const tax = Math.round(gross * rate * 100) / 100;
  const skus = Object.keys(bySku);
  const ranked = skus.slice().sort((a, b) => bySku[b] - bySku[a]);
  return {
    skus: skus,
    totals: skus.map((sku) => bySku[sku].toFixed(2)),
    gross: gross.toFixed(2),
    tax: tax.toFixed(2),
    top: ranked.length ? ranked[0] : null,
    alphabetical: skus.slice().sort(),
  };
}

module.exports = { summarize };
'''
QTY_JS = {"parseInt": "parseInt(value, 10)", "floor": "Math.floor(Number(value))", "int32": "Number(value) | 0"}
PRICE_JS = {"number": "Number(value)", "parseFloat": "parseFloat(value)"}

SOLUTION = r'''
"""Pricing summaries, ported exactly from legacy/pricing.js (docs/porting.md)."""
import math
import re
from decimal import ROUND_HALF_UP, Decimal

REGION_TAX = @@TAX_PY@@
COUPON_PREFIX = "@@COUPON@@"
DEFAULT_REGION = "@@DEFAULT_REGION@@"
QUANTITY = "@@QTY@@"
PRICE = "@@PRICE@@"
ROUND_LINES = @@ROUND_LINES@@
UNDEFINED = object()
NAN = float("nan")
JS_SPACE = " \t\n\v\f\r\xa0                　﻿"
DECIMAL = r"[+-]?(?:Infinity|(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?)"
RADIX = re.compile(r"0(?:[xX][0-9a-fA-F]+|[oO][0-7]+|[bB][01]+)")
INDEX = re.compile(r"0|[1-9][0-9]*")


def js_truthy(value):
    if value is UNDEFINED or value is None:
        return False
    if isinstance(value, float):
        return not (value == 0 or math.isnan(value))
    return bool(value)


def js_number_text(number):
    if math.isnan(number):
        return "NaN"
    if math.isinf(number):
        return "Infinity" if number > 0 else "-Infinity"
    if number == int(number) and abs(number) < 1e21:
        return str(int(number))
    text = repr(number)
    assert "e" not in text, "inputs stay where Python and JavaScript print numbers alike"
    return text


def js_string(value):
    """String(value)."""
    if value is UNDEFINED:
        return "undefined"
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return js_number_text(float(value))
    assert isinstance(value, str), f"not a JSON scalar: {value!r}"
    return value


def to_number(value):
    """Number(value)."""
    if value is UNDEFINED:
        return NAN
    if value is None:
        return 0.0
    if isinstance(value, (bool, int, float)):
        return float(value)
    text = value.strip(JS_SPACE)
    if not text:
        return 0.0
    if RADIX.fullmatch(text):
        return float(int(text, 0))
    if re.fullmatch(DECIMAL, text):
        return float(text.replace("Infinity", "inf"))
    return NAN


def parse_float(value):
    match = re.match(DECIMAL, js_string(value).lstrip(JS_SPACE))
    return float(match.group().replace("Infinity", "inf")) if match else NAN


def parse_int(value):
    match = re.match(r"[+-]?[0-9]+", js_string(value).lstrip(JS_SPACE))
    return float(int(match.group())) if match else NAN


def to_int32(number):
    if math.isnan(number) or math.isinf(number):
        return 0.0
    wrapped = math.trunc(number) % 2 ** 32
    return float(wrapped - 2 ** 32 if wrapped >= 2 ** 31 else wrapped)


def js_round(number):
    """Math.round: the nearest integer, halves toward +Infinity."""
    if math.isnan(number) or math.isinf(number):
        return number
    floor = float(math.floor(number))
    return floor + 1.0 if number - floor >= 0.5 else floor


def to_fixed(number):
    """Number.prototype.toFixed(2): exact binary value, ties to the larger magnitude."""
    if math.isnan(number):
        return "NaN"
    assert abs(number) < 1e21, "toFixed prints large numbers differently"
    text = str(Decimal(abs(number)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP))
    return ("-" if number < 0 else "") + text


def object_keys(mapping):
    """Object.keys order: array-index keys ascending, then the others in insertion order."""
    def is_index(key):
        return INDEX.fullmatch(key) is not None and int(key) < 2 ** 32 - 1
    return sorted((key for key in mapping if is_index(key)), key=int) + [key for key in mapping if not is_index(key)]


def quantity(value):
    if QUANTITY == "parseInt":
        number = parse_int(value)
    elif QUANTITY == "floor":
        number = to_number(value)
        number = number if math.isnan(number) else float(math.floor(number))
    else:
        number = to_int32(to_number(value))
    return 0.0 if math.isnan(number) or number < 0 else number


def unit_price(value):
    number = to_number(value) if PRICE == "number" else parse_float(value)
    return number if js_truthy(number) else 0.0


def line_total(order):
    total = quantity(order.get("qty", UNDEFINED)) * unit_price(order.get("price", UNDEFINED))
    coupon = order.get("coupon", UNDEFINED)
    if js_truthy(coupon) and js_string(coupon).startswith(COUPON_PREFIX):
        total = total / 2
    return js_round(total * 100) / 100 if ROUND_LINES else total


def summarize(orders):
    by_sku, gross = {}, 0.0
    for order in orders:
        sku = js_string(order.get("sku", UNDEFINED)).strip(JS_SPACE).lower()
        total = line_total(order)
        previous = by_sku.get(sku, UNDEFINED)
        by_sku[sku] = (previous if js_truthy(previous) else 0.0) + total
        gross += total
    region = orders[0].get("region", UNDEFINED) if orders else UNDEFINED
    region = js_string(region) if js_truthy(region) else DEFAULT_REGION
    rate = REGION_TAX.get(region, 0.0)
    tax = js_round(gross * rate * 100) / 100
    skus = object_keys(by_sku)
    ranked = sorted(skus, key=lambda sku: -by_sku[sku])
    return {"skus": skus, "totals": [to_fixed(by_sku[sku]) for sku in skus], "gross": to_fixed(gross),
            "tax": to_fixed(tax), "top": ranked[0] if ranked else None,
            "alphabetical": sorted(skus, key=lambda sku: sku.encode("utf-16-be", "surrogatepass"))}
'''

STARTER = '''
"""Pricing summaries, ported from legacy/pricing.js (docs/porting.md). Work in progress."""
import math

REGION_TAX = @@TAX_PY@@
COUPON_PREFIX = "@@COUPON@@"


def _quantity(value):
    try:
        number = @@QTY_NAIVE@@
    except (TypeError, ValueError):
        return 0
    return max(number, 0)


def _unit_price(value):
    try:
        return float(value or 0)
    except (TypeError, ValueError):
        return 0.0


def _line_total(order):
    total = _quantity(order.get("qty")) * _unit_price(order.get("price"))
    if str(order.get("coupon") or "").startswith(COUPON_PREFIX):
        total = total / 2
    return @@ROUND_NAIVE@@


def summarize(orders):
    by_sku = {}
    gross = 0.0
    for order in orders:
        sku = str(order.get("sku")).strip().lower()
        total = _line_total(order)
        by_sku[sku] = by_sku.get(sku, 0) + total
        gross += total
    region = (orders[0].get("region") if orders else None) or "@@DEFAULT_REGION@@"
    rate = REGION_TAX.get(region, 0)
    tax = round(gross * rate * 100) / 100
    skus = list(by_sku)
    ranked = sorted(skus, key=lambda sku: -by_sku[sku])
    return {"skus": skus, "totals": [f"{by_sku[sku]:.2f}" for sku in skus], "gross": f"{gross:.2f}",
            "tax": f"{tax:.2f}", "top": ranked[0] if ranked else None, "alphabetical": sorted(skus)}
'''
QTY_NAIVE = {"parseInt": "int(value)", "floor": "math.floor(float(value))", "int32": "int(float(value))"}

DOC = '''
# Porting legacy/pricing.js

The storefront's pricing summaries come from `legacy/pricing.js`, which has run
in production for years. It is being replaced by the Python module
`@@PKG@@/pricing.py`. Downstream services compare the two outputs byte for
byte, so the port must be exact: for every input in the domain below,
`summarize(orders)` in Python returns the same value as the legacy
`summarize(orders)`, with JavaScript's semantics and every quirk they imply.
Do not "fix" legacy behavior, even where it looks wrong.

## Input domain

`orders` is a list (possibly empty) of dicts decoded from JSON. An order may
have any of the keys `sku`, `qty`, `price`, `coupon` and `region`; a missing key
is JavaScript `undefined`, and `None` is `null`. Every value is one of:

- `None`, `True` or `False`;
- a number (int or float) whose absolute value is 0 or between 0.001 and
  1,000,000,000;
- a string of printable ASCII characters, spaces, tabs and line feeds that
  never contains `Infinity`.

No `sku` names a member of JavaScript's `Object.prototype` (such as
`constructor`).

## Output

A dict with the same keys as the legacy object; JavaScript strings are Python
`str`, arrays are lists and `null` is `None`.

The legacy module is deleted once the port ships: the port must not run Node.
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.pricing import summarize


class PortTests(unittest.TestCase):
    def test_two_orders(self):
        result = summarize([{"sku": "mug", "qty": 2, "price": 4.5, "region": "@@REGION@@"},
                            {"sku": "pen", "qty": 1, "price": 2}])
        self.assertEqual(result["skus"], ["mug", "pen"])
        self.assertEqual(result["totals"], ["9.00", "2.00"])
        self.assertEqual(result["gross"], "11.00")
        self.assertEqual(result["top"], "mug")

    def test_no_orders(self):
        self.assertEqual(summarize([])["skus"], [])


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = r'''
import os
import random
import unittest

from @@PKG@@.pricing import summarize

os.environ["PATH"] = "/nonexistent"
@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)
MISSING = object()
POOLS = {
    "sku": ["Widget", " gadget ", "WIDGET", "42", "7", "007", "-1", "1.5", "10", "x1", "", "  ", "4294967295",
            "4294967294", 3, 12.0, 1.5, 0, -2, None, True, MISSING],
    "qty": ["3", " 4", "5 boxes", "2.9", "-1", "abc", "", "0x10", "+7", "1e2", "  12abc", "0b11", ".5", "-0",
            2, 2.9, -3, 0, 1.5, 1e9, None, True, False, MISSING],
    "price": ["12.5", " 7 ", "", "abc", "1e2", "0x1A", ".5", "5.", "-3", "12abc", "1,5", "0o17", "\t2.25\n",
              9.99, 0.125, 1.005, 2.675, 0.3, 19.995, 0.045, 100, 0, -1.5, None, True, False, MISSING],
    "coupon": [MISSING, MISSING, None, "", "@@COUPON@@", "@@COUPON@@2026", "@@COUPON_LOWER@@",
               "X@@COUPON@@", 0, 50, True],
    "region": @@REGIONS@@,
}


def order(rng):
    made = {}
    for key, pool in POOLS.items():
        value = rng.choice(pool)
        if value is not MISSING:
            made[key] = value
    return made


class HiddenPortChecks(unittest.TestCase):
    def assert_like_legacy(self, cases):
        for orders in cases:
            expected = outcome(REFERENCE["summarize"], orders)
            self.assertEqual(outcome(summarize, orders), expected, repr(orders))

    def single(self, key, values, base=None):
        base = base or {"sku": "a", "qty": 3, "price": 2.5}
        return [[{**base, key: value}] for value in values]

    def test_plain_orders(self):
        self.assert_like_legacy([[{"sku": "mug", "qty": 2, "price": 4.5}, {"sku": "pen", "qty": 3, "price": 1.25}],
                                 [], [{"sku": "mug", "qty": 1, "price": 3}, {"sku": "MUG ", "qty": 1, "price": 2}]])

    def test_quantity_coercion(self):
        self.assert_like_legacy(self.single("qty", POOLS["qty"][:-1]) + [[{"sku": "a", "price": 2}]])

    def test_price_coercion(self):
        self.assert_like_legacy(self.single("price", POOLS["price"][:-1]) + [[{"sku": "a", "qty": 2}]])

    def test_rounding_halves(self):
        prices = [0.125, 0.375, 1.005, 2.675, 0.045, 1.115, 10.075, 0.005, 1.25, 2.5]
        self.assert_like_legacy([[{"sku": "a", "qty": qty, "price": price}] for price in prices for qty in (1, 3)])

    def test_sku_text(self):
        self.assert_like_legacy(self.single("sku", POOLS["sku"][:-1]) + [[{"qty": 1, "price": 1}]])

    def test_key_order(self):
        skus = ["b", "10", "a", "2", "007", "-1", "1.5", "4294967295", "4294967294", "0"]
        self.assert_like_legacy([[{"sku": sku, "qty": 1, "price": 1} for sku in skus],
                                 [{"sku": sku, "qty": 1, "price": 1} for sku in reversed(skus)]])

    def test_region_and_tax(self):
        self.assert_like_legacy([[{"sku": "a", "qty": 1, "price": 99.99, "region": region}]
                                 for region in POOLS["region"] if region is not MISSING]
                                + [[{"sku": "a", "qty": 1, "price": 99.99}]])

    def test_coupons(self):
        self.assert_like_legacy(self.single("coupon", [value for value in POOLS["coupon"] if value is not MISSING]))

    def test_ranking_ties(self):
        self.assert_like_legacy([[{"sku": "b", "qty": 1, "price": 2}, {"sku": "a", "qty": 2, "price": 1},
                                  {"sku": "5", "qty": 1, "price": 2}],
                                 [{"sku": "z", "qty": 1, "price": 0}, {"sku": "y", "qty": "x", "price": 3}]])

    def test_generated_orders(self):
        rng = random.Random(@@SEED@@)
        self.assert_like_legacy([[order(rng) for _ in range(rng.randint(0, 6))] for _ in range(400)])
'''

PROMPTS = (
    "We are retiring the Node service. `legacy/pricing.js` is what runs in production and "
    "`@@PKG@@/pricing.py` is an unfinished Python port of it. Finish the port so it is exact, as "
    "`docs/porting.md` requires.",
    "Downstream services diff the old and new pricing outputs byte for byte, and the Python port in "
    "`@@PKG@@/pricing.py` does not match `legacy/pricing.js` yet. Make it match exactly over the input "
    "domain in `docs/porting.md`.",
)
REGION_NAMES = ["eu", "uk", "us", "ca", "de", "au"]
RATES = [0.2, 0.07, 0.13, 0.0825, 0.19, 0.05, 0.1]


def _options(rng):
    table = {region: rng.choice(RATES) for region in rng.sample(REGION_NAMES, 4)}
    return {"qty": rng.choice(sorted(QTY_JS)), "price": rng.choice(sorted(PRICE_JS)),
            "round_lines": rng.random() < 0.6, "coupon": rng.choice(["HALF", "PROMO", "SAVE"]),
            "default_region": rng.choice(sorted(table)), "table": table}


def sources(options):
    """(legacy JavaScript, reference Python, starter Python) for one option set."""
    tax_js = "{ " + ", ".join(f"{name}: {rate!r}" for name, rate in options["table"].items()) + " }"
    tax_py = repr(options["table"])
    legacy = fill(LEGACY, TAX_JS=tax_js, COUPON=options["coupon"], QTY_JS=QTY_JS[options["qty"]],
                  PRICE_JS=PRICE_JS[options["price"]], DEFAULT_REGION=options["default_region"],
                  ROUND_JS="Math.round(total * 100) / 100" if options["round_lines"] else "total")
    reference = fill(SOLUTION, TAX_PY=tax_py, COUPON=options["coupon"], DEFAULT_REGION=options["default_region"],
                     QTY=options["qty"], PRICE=options["price"], ROUND_LINES=options["round_lines"])
    starter = fill(STARTER, TAX_PY=tax_py, COUPON=options["coupon"], QTY_NAIVE=QTY_NAIVE[options["qty"]],
                   DEFAULT_REGION=options["default_region"],
                   ROUND_NAIVE="round(total * 100) / 100" if options["round_lines"] else "total")
    return legacy, reference, starter


def generate(seed, rng):
    pkg = package_name(rng, "pricing")
    options = _options(rng)
    legacy, reference, starter = sources(options)
    regions = sorted(options["table"]) + [name.upper() for name in sorted(options["table"])][:2] + [
        "", "mx", 0, None, "MISSING"]
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, REFERENCE=reference_block(reference),
                  COUPON=options["coupon"], COUPON_LOWER=options["coupon"].lower(),
                  REGIONS=repr(regions).replace("'MISSING'", "MISSING"))
    files = {"README.md": f"# {pkg}\n\nStorefront pricing. Porting notes: `docs/porting.md`.\n",
             "legacy/pricing.js": legacy, f"{pkg}/__init__.py": "", f"{pkg}/pricing.py": starter,
             "docs/porting.md": fill(DOC, PKG=pkg), "tests/__init__.py": "",
             "tests/test_pricing.py": fill(VISIBLE_TESTS, PKG=pkg, REGION=options["default_region"])}
    meta = {key: value for key, value in options.items() if key != "table"}
    return make_spec(family="port", template="pricing", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/pricing.py": reference}, meta=meta)
