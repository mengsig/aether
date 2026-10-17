"""multibug / billing: cent errors whose root causes sit in different modules of a small billing package.

Each seed plants three to five defects from a pool, one variant per module
concern, and ISSUE.md reports only one or two of them: the rest are found
only by auditing the code against docs/billing.md.
"""
from .common import fill, make_spec, package_name

MONEY_HEAD = '''
"""Amounts of money as integer cents (docs/billing.md)."""
from decimal import ROUND_HALF_UP, Decimal

SYMBOL = @@SYMBOL@@


def half_up(value):
    """A Decimal rounded to a whole number, halves away from zero."""
    return int(Decimal(value).quantize(Decimal(1), rounding=ROUND_HALF_UP))

'''
TO_CENTS = {
    "good": "return half_up(Decimal(str(amount)) * 100)",
    "float": "return int(float(amount) * 100)",
    "even": "return int(round(Decimal(str(amount)) * 100))",
}
MONEY_BODY = '''

def to_cents(amount):
    """Cents in an amount given as text or a number: "12.5" is 1250."""
    @@TO_CENTS@@


def format_cents(cents):
    """Display text for cents, such as "@@SYMBOL@@1,234.50"."""
    @@FORMAT@@
'''
FORMAT = {
    "good": 'sign = "-" if cents < 0 else ""\n    whole, rest = divmod(abs(cents), 100)\n'
            '    return f"{sign}{SYMBOL}{whole:,}.{rest:02d}"',
    "bad": 'return f"{SYMBOL}{cents / 100:,.2f}"',
}
CART = '''
"""Invoice lines."""
from decimal import Decimal

from .money import half_up, to_cents


def line_total(unit_price, quantity, discount_percent=0):
    """Cents for `quantity` units at `unit_price`, less `discount_percent`."""
    @@BODY@@
'''
LINE = {
    "good": "gross = to_cents(unit_price) * quantity\n"
            "    return half_up(Decimal(gross) * (100 - Decimal(discount_percent)) / 100)",
    "unit": "unit = half_up(Decimal(to_cents(unit_price)) * (100 - Decimal(discount_percent)) / 100)\n"
            "    return unit * quantity",
    "floor": "gross = to_cents(unit_price) * quantity\n"
             "    return int(Decimal(gross) * (100 - Decimal(discount_percent)) / 100 + Decimal(\"0.5\"))",
}
INVOICE = '''
"""Invoices: lines, tax and the rendered text."""
from decimal import Decimal

from .cart import line_total
from .money import format_cents, half_up


class Invoice:
    def __init__(self, number, lines, tax_percent):
        """`lines` holds (unit_price, quantity, discount_percent[, taxable]) tuples; taxable defaults to True."""
        self.number, self.lines, self.tax_percent = number, list(lines), tax_percent

    def subtotal(self):
        return sum(line_total(*line[:3]) for line in self.lines)

    def tax(self):
        rate = Decimal(str(self.tax_percent)) / 100
        lines = @@BASE@@
        @@TAX@@

    def total(self):
        return self.subtotal() + self.tax()

    def render(self):
        return "\\n".join([f"Invoice {self.number}", f"Subtotal: {format_cents(self.subtotal())}",
                          f"Tax: {format_cents(self.tax())}", f"Total: {format_cents(self.total())}"])
'''
BASE = {"good": "[line for line in self.lines if len(line) < 4 or line[3]]", "exempt": "self.lines"}
TAX = {"good": "return half_up(Decimal(sum(line_total(*line[:3]) for line in lines)) * rate)",
       "per_line": "return sum(half_up(Decimal(line_total(*line[:3])) * rate) for line in lines)"}
REPORT = '''
"""Monthly revenue for the finance team."""
from datetime import datetime, timezone

from .money import to_cents


def _cents(amount):
    return @@CONVERT@@


def _month(paid_at):
    return @@MONTH@@


def monthly_revenue(payments):
    """{"YYYY-MM": cents} over payments shaped {"paid_at": "2026-03-04T10:00:00+01:00", "amount": "12.30"}."""
    totals = {}
    for payment in payments:
        month = _month(payment["paid_at"])
        totals[month] = totals.get(month, 0) + _cents(payment["amount"])
    return dict(sorted(totals.items()))
'''
CONVERT = {"good": "to_cents(amount)", "report_float": "int(float(amount) * 100)"}
MONTH = {"good": 'datetime.fromisoformat(paid_at).astimezone(timezone.utc).strftime("%Y-%m")',
         "report_month": "paid_at[:7]"}

DOC = '''
# Billing rules

1. Amounts arrive as decimal text (`"12.30"`, sometimes with more than two
   decimals, such as `"0.125"`) and are converted to integer cents exactly
   once, everywhere by the same rule: multiply by 100 and round halves away
   from zero (`"0.125"` is 13 cents, `"-0.125"` is -13). Floating point is
   never used for money.
2. A line's total is the unit price in cents times the quantity, less the
   line's discount percent, rounded once for the whole line by rule 1's
   rounding. Credit lines have negative prices and round the same way, away
   from zero.
3. A line may be marked tax-exempt (a fourth field, `False`). Tax is charged
   on the sum of the taxable lines' totals and rounded once, not per line.
4. Amounts are displayed as `@@SYMBOL@@1,234.50`: the symbol, thousands
   separated by commas, two decimals. A negative amount puts the minus sign
   before the symbol: `-@@SYMBOL@@12.50`.
5. The monthly revenue report counts a payment in the calendar month of its
   `paid_at` timestamp in UTC (timestamps always carry an offset), and sums
   the same cents the invoices charge.
'''

ISSUE_LINES = {
    "float": "- {inv1}: 3 x {item1} at 0.29 shows a subtotal of @@S@@0.84; it should be @@S@@0.87.",
    "even": "- {inv1}: 1 x {item1} at 0.125 (samples are priced in tenths of a cent) shows @@S@@0.12; "
            "it should be @@S@@0.13.",
    "unit": "- {inv2}: 7 x {item2} at 1.99 with a 15% discount shows a line total of @@S@@11.83; "
            "it should be @@S@@11.84.",
    "floor": "- Credit note {inv4}: 3 x {item2} returned at -0.99 with a 50% discount shows @@S@@-1.48 "
             "for the line; it should be -@@S@@1.49.",
    "per_line": "- {inv3}: three items at 1.05 each with 10% tax shows tax of @@S@@0.33; it should be @@S@@0.32.",
    "exempt": "- {inv3}: the tax-exempt gift card line was taxed.",
    "format": "- Credit note {inv4} prints `@@S@@-12.50`; our documents print `-@@S@@12.50`.",
    "report_float": "- The revenue report does not match the invoices: three payments of 0.29 add up to 0.84 there.",
    "report_month": "- A payment made at 2026-03-31T23:30:00-02:00 shows up in March's revenue; "
                    "finance books it in April (UTC).",
}

HIDDEN = '''
import unittest

from @@PKG@@.cart import line_total
from @@PKG@@.invoice import Invoice
from @@PKG@@.money import format_cents, to_cents
from @@PKG@@.report import monthly_revenue

S = @@SYMBOL_REPR@@


class HiddenBillingChecks(unittest.TestCase):
    def test_amounts_to_cents(self):
        cases = {"0.29": 29, "1.15": 115, "4.35": 435, "0.125": 13, "2.675": 268, "-0.125": -13,
                 "19.99": 1999, "0.005": 1, "100": 10000, "8.2": 820}
        for text, cents in cases.items():
            self.assertEqual(to_cents(text), cents, text)

    def test_every_two_decimal_amount_converts_exactly(self):
        for cents in range(0, 3000, 7):
            text = f"{cents // 100}.{cents % 100:02d}"
            self.assertEqual(to_cents(text), cents, text)

    def test_discount_rounds_once_per_line(self):
        self.assertEqual(line_total("1.99", 7, 15), 1184)
        self.assertEqual(line_total("0.99", 3, 50), 149)
        self.assertEqual(line_total("2.50", 4), 1000)

    def test_credit_lines_round_away_from_zero(self):
        self.assertEqual(line_total("-0.99", 3, 50), -149)
        self.assertEqual(line_total("-1.99", 7, 15), -1184)
        self.assertEqual(line_total("-0.33", 1, 10), -30)

    def test_tax_once_on_the_taxable_subtotal(self):
        invoice = Invoice("T-1", [("1.05", 1, 0)] * 3, 10)
        self.assertEqual((invoice.subtotal(), invoice.tax(), invoice.total()), (315, 32, 347))
        self.assertEqual(Invoice("T-2", [("0.29", 3, 0), ("1.99", 7, 15)], 7.5).total(), 1366)

    def test_exempt_lines_carry_no_tax(self):
        invoice = Invoice("T-3", [("10.00", 1, 0), ("25.00", 1, 0, False), ("1.05", 1, 0, True)], 20)
        self.assertEqual((invoice.subtotal(), invoice.tax()), (3605, 221))
        self.assertEqual(Invoice("T-4", [("9.99", 2, 0, False)], 20).tax(), 0)

    def test_display_format(self):
        self.assertEqual(format_cents(-1250), f"-{S}12.50")
        self.assertEqual(format_cents(123456789), f"{S}1,234,567.89")
        self.assertEqual(format_cents(5), f"{S}0.05")
        self.assertEqual(format_cents(-5), f"-{S}0.05")

    def test_rendered_credit_note(self):
        text = Invoice("CN-9", [("-12.50", 1, 0)], 0).render()
        self.assertIn(f"Total: -{S}12.50", text)

    def test_report_matches_invoices(self):
        payments = [{"paid_at": "2026-03-10T12:00:00+00:00", "amount": "0.29"}] * 3 + [
            {"paid_at": "2026-02-10T12:00:00+00:00", "amount": "0.125"}]
        self.assertEqual(monthly_revenue(payments), {"2026-02": 13, "2026-03": 87})
        self.assertEqual(list(monthly_revenue(payments)), ["2026-02", "2026-03"])

    def test_report_months_are_utc(self):
        payments = [{"paid_at": "2026-03-31T23:30:00-02:00", "amount": "5.00"},
                    {"paid_at": "2026-05-01T00:30:00+02:00", "amount": "1.00"},
                    {"paid_at": "2026-04-15T08:00:00+00:00", "amount": "2.00"}]
        self.assertEqual(monthly_revenue(payments), {"2026-04": 800})
'''

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.cart import line_total
from @@PKG@@.invoice import Invoice
from @@PKG@@.money import format_cents, to_cents
from @@PKG@@.report import monthly_revenue


class BillingTests(unittest.TestCase):
    def test_round_amounts(self):
        self.assertEqual(to_cents("12.50"), 1250)
        self.assertEqual(line_total("2.00", 3), 600)

    def test_invoice(self):
        invoice = Invoice("T-0", [("10.00", 2, 0)], 20)
        self.assertEqual(invoice.total(), 2400)
        self.assertEqual(format_cents(1050), "@@SYMBOL@@10.50")

    def test_report(self):
        payments = [{"paid_at": "2026-03-10T12:00:00+00:00", "amount": "10.00"}]
        self.assertEqual(monthly_revenue(payments), {"2026-03": 1000})


if __name__ == "__main__":
    unittest.main()
'''

PROMPTS = (
    "Finance reported the invoicing errors in `ISSUE.md`. Fix their root causes in `@@PKG@@`. An audit "
    "against the billing rules in `docs/billing.md` suggests these are not the only deviations: make the "
    "package follow every rule there.",
    "Our billing package `@@PKG@@` must follow `docs/billing.md` exactly. `ISSUE.md` lists the errors "
    "customers noticed so far; fix those and every other place where the code breaks the documented rules.",
)
# One variant per concern: a seed plants at most one defect in each.
CONCERNS = {"cents": ("float", "even"), "line": ("unit", "floor"), "tax_rounding": ("per_line",),
            "tax_base": ("exempt",), "format": ("format",), "report_cents": ("report_float",),
            "report_month": ("report_month",)}


def _bugs(rng):
    concerns = rng.sample(sorted(CONCERNS), rng.randint(3, 5))
    return {rng.choice(CONCERNS[concern]) for concern in concerns}


def _pick(table, bugs):
    chosen = [key for key in table if key != "good" and key in bugs]
    assert len(chosen) <= 1, f"two defects for one concern: {chosen}"
    return table[chosen[0] if chosen else "good"]


def _modules(pkg, symbol, bugs):
    """Module sources with `bugs` planted; an empty set gives the reference."""
    money = fill(MONEY_HEAD, SYMBOL=repr(symbol)) + fill(MONEY_BODY, dedent=False, SYMBOL=symbol,
                                                         TO_CENTS=_pick(TO_CENTS, bugs),
                                                         FORMAT=_pick(FORMAT, {"bad"} if "format" in bugs else set()))
    invoice = fill(INVOICE, BASE=_pick(BASE, bugs), TAX=_pick(TAX, bugs))
    report = fill(REPORT, CONVERT=_pick(CONVERT, bugs), MONTH=_pick(MONTH, bugs))
    return {f"{pkg}/money.py": money, f"{pkg}/cart.py": fill(CART, BODY=_pick(LINE, bugs)),
            f"{pkg}/invoice.py": invoice, f"{pkg}/report.py": report}


def _issue(rng, reported, symbol):
    names = {"inv1": f"INV-{rng.randint(1000, 9999)}", "inv2": f"INV-{rng.randint(1000, 9999)}",
             "inv3": f"INV-{rng.randint(1000, 9999)}", "inv4": f"CN-{rng.randint(100, 999)}",
             "item1": rng.choice(["Sticker", "Sample pack", "Pencil"]), "item2": rng.choice(["Notebook", "Mug", "Cable"])}
    lines = [fill(ISSUE_LINES[bug], allow_unused=True, S=symbol).format(**names) for bug in sorted(reported)]
    return "# Invoicing errors reported this month\n\n" + "\n".join(lines) + "\n"


def generate(seed, rng):
    pkg = package_name(rng, "billing")
    symbol = rng.choice(["$", "€", "£"])
    bugs = _bugs(rng)
    reported = set(rng.sample(sorted(bugs), rng.choice([1, 2])))
    assert reported < bugs, "at least one defect is found only by the audit"
    files = {"README.md": f"# {pkg}\n\nInvoices and revenue reports. Rules: `docs/billing.md`.\n",
             f"{pkg}/__init__.py": "", **_modules(pkg, symbol, bugs), "tests/__init__.py": "",
             "tests/test_billing.py": fill(VISIBLE_TESTS, PKG=pkg, SYMBOL=symbol),
             "docs/billing.md": fill(DOC, SYMBOL=symbol), "ISSUE.md": _issue(rng, reported, symbol)}
    return make_spec(family="multibug", template="billing", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=fill(HIDDEN, PKG=pkg, SYMBOL_REPR=repr(symbol)),
                     solution=_modules(pkg, symbol, set()), meta={"defects": len(bugs), "reported": len(reported)})
