"""algo / batching: replace a greedy batch planner with the cheapest plan under documented tie rules.

The greedy starter passes the visible tests; the cheapest plan needs a dynamic
program over the queue, and the tie rules (fewest batches, then the longest
leading batches) need it to run the right way round.
"""
from .common import fill, make_spec, package_name, reference_block

SOLUTION = r'''
"""Batch planning as described in docs/planning.md."""
MAX_JOBS = @@MAX_JOBS@@
CAPACITY = @@CAPACITY@@
SETUP = @@SETUP@@
PENALTY = @@PENALTY@@
COST_RULE = "@@COST_RULE@@"


def batch_cost(durations):
    if COST_RULE == "longest":
        return SETUP + max(durations) * len(durations)
    return SETUP + sum(durations) + PENALTY * (max(durations) - min(durations))


def _check(durations):
    durations = list(durations)
    for duration in durations:
        if type(duration) is not int:
            raise TypeError(f"a duration is an int, not {duration!r}")
        if duration <= 0 or duration > CAPACITY:
            raise ValueError(f"a duration lies in 1..{CAPACITY}: {duration}")
    return durations


def plan(durations):
    """Batch sizes of the cheapest plan, ties broken as documented."""
    durations = _check(durations)
    count = len(durations)
    # best[i]: (cost, batches, sizes) of the preferred plan for durations[i:].
    best = [None] * count + [(0, 0, [])]
    for start in range(count - 1, -1, -1):
        chosen, total = None, 0
        for size in range(1, min(MAX_JOBS, count - start) + 1):
            total += durations[start + size - 1]
            if total > CAPACITY:
                break
            cost, batches, sizes = best[start + size]
            candidate = (cost + batch_cost(durations[start:start + size]), batches + 1, [size] + sizes)
            if chosen is None or candidate[:2] < chosen[:2] or (candidate[:2] == chosen[:2]
                                                                and candidate[2] > chosen[2]):
                chosen = candidate
        best[start] = chosen
    return best[0][2]
'''

STARTER = '''
"""Batch planning as described in docs/planning.md."""
MAX_JOBS = @@MAX_JOBS@@
CAPACITY = @@CAPACITY@@
SETUP = @@SETUP@@
PENALTY = @@PENALTY@@


def batch_cost(durations):
    @@COST_BODY@@


def plan(durations):
    """Batch sizes, filling each batch as far as the limits allow."""
    sizes, current, total = [], 0, 0
    for duration in durations:
        if current == MAX_JOBS or total + duration > CAPACITY:
            sizes.append(current)
            current, total = 0, 0
        current, total = current + 1, total + duration
    if current:
        sizes.append(current)
    return sizes
'''
COST_BODY = {"longest": "return SETUP + max(durations) * len(durations)",
             "spread": "return SETUP + sum(durations) + PENALTY * (max(durations) - min(durations))"}

DOC = '''
# Batch planning

The nightly runner executes a queue of jobs in batches. `plan(durations)` (in
`@@PKG@@/planner.py`) takes the queue's job durations in minutes and returns the
sizes of the batches, in order: `[3, 1, 2]` runs the first three jobs together,
then the fourth alone, then the last two. `durations` may be any iterable (a
list, a tuple, a generator). Jobs never change order, and every job is in
exactly one batch. An empty queue gives `[]`.

## Limits

A batch holds at most @@MAX_JOBS@@ jobs, and its durations add up to at most
@@CAPACITY@@ minutes.

## Cost

@@COST_TEXT@@ `batch_cost(durations)` computes it for one batch.

## Which plan

`plan` returns the plan with the **lowest total cost** (the sum over its
batches). When several plans cost the same, it returns the one with the fewest
batches; when several of those remain, the one whose first batch has the most
jobs, then (among those) whose second batch has the most jobs, and so on.

Queues may hold a few thousand jobs, and planning one must take well under a
second.

## Errors

A duration must be an `int` (not a `bool`), else `TypeError`. A duration below
1 or above @@CAPACITY@@ raises `ValueError`.
'''
COST_TEXT = {
    "longest": "A batch costs @@SETUP@@ (setup) plus its longest duration times its number of jobs.",
    "spread": "A batch costs @@SETUP@@ (setup) plus the sum of its durations plus @@PENALTY@@ times the "
              "difference between its longest and its shortest duration.",
}

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.planner import plan


class PlannerTests(unittest.TestCase):
    def test_empty_queue(self):
        self.assertEqual(plan([]), [])

    def test_every_job_is_planned_once(self):
        sizes = plan([5, 5, 5, 5, 5, 5, 5])
        self.assertEqual(sum(sizes), 7)
        self.assertTrue(all(1 <= size <= @@MAX_JOBS@@ for size in sizes))


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = r'''
import itertools
import random
import time
import unittest

from @@PKG@@.planner import plan

@@REFERENCE@@
REFERENCE = load_reference(REFERENCE_SOURCE)
MAX_JOBS, CAPACITY = REFERENCE["MAX_JOBS"], REFERENCE["CAPACITY"]


def brute_force(durations):
    """Every split of a short queue, ranked by the documented rules."""
    best = None
    for cuts in itertools.product([False, True], repeat=max(0, len(durations) - 1)):
        sizes, size = [], 1
        for cut in cuts:
            if cut:
                sizes.append(size)
                size = 0
            size += 1
        sizes = sizes + [size] if durations else []
        batches, start = [], 0
        for size in sizes:
            batches.append(durations[start:start + size])
            start += size
        if any(len(batch) > MAX_JOBS or sum(batch) > CAPACITY for batch in batches):
            continue
        key = (sum(REFERENCE["batch_cost"](batch) for batch in batches), len(batches), [-size for size in sizes])
        if best is None or key < best[0]:
            best = (key, sizes)
    return best[1]


class HiddenPlannerChecks(unittest.TestCase):
    def assert_like_reference(self, queues):
        for queue in queues:
            self.assertEqual(outcome(plan, list(queue)), outcome(REFERENCE["plan"], list(queue)), repr(queue))

    def test_short_queues_match_an_exhaustive_search(self):
        rng = random.Random(@@SEED@@)
        for _ in range(60):
            queue = [rng.randint(1, CAPACITY // 2) for _ in range(rng.randint(0, 9))]
            self.assertEqual(outcome(plan, list(queue)), ("value", brute_force(queue)), queue)

    def test_cheaper_than_greedy(self):
        self.assert_like_reference(@@CURATED@@)

    def test_ties_prefer_fewer_batches(self):
        rng = random.Random(@@SEED@@ + 1)
        self.assert_like_reference([[rng.choice([1, 2]) for _ in range(rng.randint(1, 12))] for _ in range(60)])

    def test_ties_prefer_long_leading_batches(self):
        self.assert_like_reference([[1] * size for size in range(1, 2 * MAX_JOBS + 3)]
                                   + [[CAPACITY // 2] * size for size in range(1, 7)])

    def test_capacity_and_job_limits(self):
        self.assert_like_reference([[CAPACITY], [CAPACITY, 1, CAPACITY], [CAPACITY - 1, 1, 1],
                                    [1] * (MAX_JOBS + 1), [CAPACITY // 2 + 1] * 4])

    def test_generated_queues(self):
        rng = random.Random(@@SEED@@ + 2)
        queues = []
        for _ in range(300):
            high = rng.choice([3, CAPACITY // 3, CAPACITY])
            queues.append([rng.randint(1, max(1, high)) for _ in range(rng.randint(0, 25))])
        self.assert_like_reference(queues)

    def test_long_queues_are_fast(self):
        rng = random.Random(@@SEED@@ + 3)
        queue = [rng.randint(1, CAPACITY // 2) for _ in range(3000)]
        started = time.monotonic()
        found = plan(list(queue))
        self.assertLess(time.monotonic() - started, 3.0)
        self.assertEqual(found, REFERENCE["plan"](queue))

    def test_invalid_durations(self):
        self.assert_like_reference([[0], [-3], [CAPACITY + 1], [1, 2.5], [True], ["3"], [1, None]])

    def test_iterables(self):
        self.assertEqual(plan(iter([1, 2, 3])), REFERENCE["plan"]([1, 2, 3]))
'''

PROMPTS = (
    "Ops says the nightly runner wastes money: `plan` in `@@PKG@@/planner.py` packs batches greedily. "
    "`docs/planning.md` defines what a batch costs and which plan to pick; make `plan` return exactly that plan.",
    "The batch planner in `@@PKG@@/planner.py` does not pick the plan `docs/planning.md` asks for (cheapest, "
    "with its tie rules), and it must stay fast on long queues. Fix it.",
)


def _options(rng):
    rule = rng.choice(sorted(COST_BODY))
    return {"cost_rule": rule, "max_jobs": rng.randint(3, 6), "capacity": rng.choice([20, 30, 40, 60]),
            "setup": rng.choice([3, 5, 8, 12]), "penalty": rng.choice([1, 2, 3])}


def _curated(options):
    capacity, jobs = options["capacity"], options["max_jobs"]
    long, short = capacity // 2, 1
    return [[short, long, short, short], [long, short, short, long, short], [short] * (jobs + 1) + [long],
            [long // 2, long, long // 2, 1, long], [2, 9, 2, 2, 9, 2, 9][:jobs + 2]]


def generate(seed, rng):
    pkg = package_name(rng, "runner")
    options = _options(rng)
    values = {"MAX_JOBS": options["max_jobs"], "CAPACITY": options["capacity"], "SETUP": options["setup"],
              "PENALTY": options["penalty"]}
    solution = fill(SOLUTION, COST_RULE=options["cost_rule"], **values)
    hidden = fill(HIDDEN, PKG=pkg, SEED=seed, REFERENCE=reference_block(solution),
                  CURATED=repr([[min(value, options["capacity"]) for value in queue] for queue in _curated(options)]))
    cost_text = fill(COST_TEXT[options["cost_rule"]], dedent=False, allow_unused=True,
                     SETUP=options["setup"], PENALTY=options["penalty"])
    files = {"README.md": f"# {pkg}\n\nThe nightly batch runner. Planning rules: `docs/planning.md`.\n",
             f"{pkg}/__init__.py": "", f"{pkg}/planner.py": fill(STARTER, COST_BODY=COST_BODY[options["cost_rule"]],
                                                                 **values),
             "docs/planning.md": fill(DOC, PKG=pkg, MAX_JOBS=options["max_jobs"], CAPACITY=options["capacity"],
                                      COST_TEXT=cost_text),
             "tests/__init__.py": "", "tests/test_planner.py": fill(VISIBLE_TESTS, PKG=pkg, MAX_JOBS=options["max_jobs"])}
    return make_spec(family="algo", template="batching", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/planner.py": solution}, meta=options)
