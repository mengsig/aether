"""ordering / jobgraph: a job dependency scheduler with waves, a width limit, cycle reports and failure handling.

Per seed: the job-name prefix, which priority runs first, and which unknown
dependency is reported first. The starter plans and runs the simple cases and
gets tie order, canonical cycles, transitive skips, retries and mutation wrong.
"""
from .common import fill, make_spec, package_name

PREFIXES = ("job-", "task_", "step.", "", "ci:")
UNKNOWN_RULES = {"name": "the smallest `(job, dependency)` pair by plain string comparison",
                 "input": "the first one met when walking the jobs in the order given, and each job's dependencies in the order given"}

DOC = '''
# Job scheduler

A job is a dict: `name` (non-empty str, unique), `deps` (list of names, default `[]`),
`priority` (int, default 0) and `retries` (int >= 0, default 0). Any other value, a duplicate name or a
missing name raises `ValueError`. Functions never modify the jobs they are given (nor their `deps` lists)
and return new lists and dicts.

## Validation (`graph.py`)
Checks run in this order and the first failure raises:
1. unknown dependency: `UnknownDependency` (a `ValueError`, with `.job` and `.dependency`). With several, report
   @@UNKNOWN_TEXT@@.
2. dependency cycle: `CycleError` (a `ValueError`) with `.cycle`, a list of names that starts and ends with the
   same job, and the message `dependency cycle: a -> b -> a`. A job depending on itself is a cycle `[a, a]`.
   Canonical cycle: take the smallest name (plain string comparison, so `"B" < "a"` and `"job10" < "job2"`) of
   any job that lies on a cycle. The reported cycle is the shortest cycle through that job, following each job to
   one of its dependencies; among equally short ones, the smallest dependency name is chosen at every step.

## Plan (`plan.py`)
`plan(jobs, max_parallel=None)` returns a list of waves, each a list of names.
- A job is ready when all its dependencies ran in earlier waves (not the same wave).
- Each wave takes the ready jobs ordered by priority, @@PRIORITY_TEXT@@, then by name.
- `max_parallel` (int >= 1, else `ValueError`; `None` means no limit) caps a wave to its first `max_parallel` ready jobs;
  the others stay ready for the next wave.

## Run (`runner.py`)
`run(jobs, execute, max_parallel=None, dry_run=False)` plans first (so validation errors come first), then walks the
planned waves in order, each wave in order, calling `execute(name)`. A falsy return is a failure; an exception
propagates unchanged. It returns
`{"order": [...], "failed": [...], "skipped": {...}, "attempts": {...}}`:
- `order` lists every job that was executed, in execution order; `failed` those that failed, in that order.
- A job is tried up to `1 + retries` times and stops at the first success; `attempts[name]` is the number of tries.
- A job with a failed job among its transitive dependencies is not executed. It is `skipped[name]`, the sorted list of
  failed jobs it (transitively) depends on; the failed job itself is not skipped. Skipped jobs have no entry in
  `attempts`. Independent jobs still run. The plan is fixed up front: skipping never moves other jobs between waves.
- `dry_run=True` executes nothing (`execute` is not called and may be `None`): `order` is the whole plan flattened,
  `failed` is `[]`, `skipped` is `{}`, `attempts` is `{}`.
'''

GRAPH = '''
"""Validating a job table (docs/scheduler.md)."""
from collections import deque


class UnknownDependency(ValueError):
    def __init__(self, job, dependency):
        super().__init__(f"{job} depends on unknown job {dependency}")
        self.job, self.dependency = job, dependency


class CycleError(ValueError):
    def __init__(self, cycle):
        super().__init__("dependency cycle: " + " -> ".join(cycle))
        self.cycle = list(cycle)


def _int_field(job, field, default, minimum=None):
    value = job.get(field, default)
    if type(value) is not int or (minimum is not None and value < minimum):
        raise ValueError(f"bad {field} for {job.get('name')!r}: {value!r}")
    return value


def normalize(jobs):
    """{name: {"deps": tuple, "priority": int, "retries": int}} in input order, built from copies."""
    table = {}
    for job in jobs:
        name = job.get("name") if isinstance(job, dict) else None
        if not isinstance(name, str) or not name or name in table:
            raise ValueError(f"a job needs a unique non-empty name: {name!r}")
        deps = job.get("deps", [])
        if not isinstance(deps, (list, tuple)) or not all(isinstance(dep, str) for dep in deps):
            raise ValueError(f"deps of {name!r} must be a list of names")
        table[name] = {"deps": tuple(dict.fromkeys(deps)), "priority": _int_field(job, "priority", 0),
                       "retries": _int_field(job, "retries", 0, minimum=0)}
    return table

@@FIND_UNKNOWN@@

@@FIND_CYCLE@@

@@CHECK@@
'''
FIND_UNKNOWN = '''
def find_unknown(table):
    """The (job, dependency) pair to report, or None."""
    pairs = [(name, dep) for name, job in table.items() for dep in job["deps"] if dep not in table]
    return @@PICK@@ if pairs else None
'''
PICKS = {"name": "min(pairs)", "input": "pairs[0]"}
FIND_CYCLE = {
    "good": '''
def _steps_to(table, target):
    """Fewest dependency steps from each job that can reach `target` (target itself is 0)."""
    dependents = {}
    for name, job in table.items():
        for dep in job["deps"]:
            dependents.setdefault(dep, []).append(name)
    steps, queue = {target: 0}, deque([target])
    while queue:
        node = queue.popleft()
        for parent in dependents.get(node, []):
            if parent not in steps:
                steps[parent] = steps[node] + 1
                queue.append(parent)
    return steps


def find_cycle(table):
    """The canonical cycle as a list that starts and ends with the same job, or None."""
    for start in sorted(table):
        steps = _steps_to(table, start)
        back = [steps[dep] for dep in table[start]["deps"] if dep in steps]
        if not back:
            continue
        remaining, node, cycle = min(back) + 1, start, [start]
        while remaining:
            node = min(dep for dep in table[node]["deps"] if steps.get(dep) == remaining - 1)
            cycle.append(node)
            remaining -= 1
        assert cycle[0] == cycle[-1] == node, "a cycle closes on its first job"
        return cycle
    return None
''',
    "dfs": '''
def find_cycle(table):
    """The first cycle a depth-first walk meets, as a list that starts and ends with the same job, or None."""
    state = {}

    def visit(node, path):
        state[node] = "active"
        for dep in table[node]["deps"]:
            if dep not in table:
                continue
            if state.get(dep) == "active":
                return path[path.index(dep):] + [dep]
            if dep not in state:
                found = visit(dep, path + [dep])
                if found:
                    return found
        state[node] = "done"
        return None

    for name in table:
        if name not in state:
            found = visit(name, [name])
            if found:
                return found
    return None
''',
}
CHECK = {
    "good": '''
def check(jobs):
    """The normalized table of valid jobs; raises UnknownDependency, then CycleError."""
    table = normalize(jobs)
    unknown = find_unknown(table)
    if unknown:
        raise UnknownDependency(*unknown)
    cycle = find_cycle(table)
    if cycle:
        raise CycleError(cycle)
    return table
''',
    "cycle_first": '''
def check(jobs):
    """The normalized table of valid jobs; raises CycleError, then UnknownDependency."""
    table = normalize(jobs)
    cycle = find_cycle(table)
    if cycle:
        raise CycleError(cycle)
    unknown = find_unknown(table)
    if unknown:
        raise UnknownDependency(*unknown)
    return table
''',
}

PLAN = {
    "good": '''
"""Execution plans (docs/scheduler.md)."""
from .graph import check

PRIORITY_SIGN = @@SIGN@@


def check_width(max_parallel):
    if max_parallel is not None and (type(max_parallel) is not int or max_parallel < 1):
        raise ValueError(f"max_parallel must be an int >= 1 or None, got {max_parallel!r}")


def waves_for(table, max_parallel):
    """Waves of names for an already validated table."""
    check_width(max_parallel)
    done, waves = set(), []
    while len(done) < len(table):
        ready = sorted((name for name, job in table.items() if name not in done and done.issuperset(job["deps"])),
                       key=lambda name: (PRIORITY_SIGN * table[name]["priority"], name))
        assert ready, "a validated table always has a ready job"
        wave = ready[:max_parallel] if max_parallel else ready
        waves.append(wave)
        done.update(wave)
    return waves


def plan(jobs, max_parallel=None):
    return waves_for(check(jobs), max_parallel)
''',
    "kahn": '''
"""Execution plans (docs/scheduler.md)."""
from .graph import check

PRIORITY_SIGN = @@WRONG_SIGN@@


def waves_for(table, max_parallel):
    """Waves of names for an already validated table."""
    done, waves = set(), []
    while len(done) < len(table):
        ready = sorted((name for name, job in table.items() if name not in done and done.issuperset(job["deps"])),
                       key=lambda name: (PRIORITY_SIGN * table[name]["priority"], name))
        wave = ready[:max_parallel] if max_parallel else ready
        waves.append(wave)
        done.update(wave)
    return waves


def plan(jobs, max_parallel=None):
    table = check(jobs)
    pending = {job["name"]: job for job in jobs}
    for job in pending.values():
        job.setdefault("deps", [])
    waves = waves_for(table, max_parallel)
    for wave in waves:
        for name in wave:
            pending[name]["deps"] = [dep for dep in pending[name]["deps"] if dep not in wave]
    return waves
''',
}

RUNNER = {
    "good": '''
"""Running a plan (docs/scheduler.md)."""
from .graph import check
from .plan import waves_for


def _causes(table, name, failed, skipped):
    """Sorted failed jobs among the transitive dependencies of `name`."""
    found = set()
    for dep in table[name]["deps"]:
        found.update([dep] if dep in failed else skipped.get(dep, []))
    return sorted(found)


def _try(execute, name, retries):
    """Number of tries made and whether the last one succeeded."""
    for attempt in range(1, retries + 2):
        if execute(name):
            return attempt, True
    return retries + 1, False


def run(jobs, execute, max_parallel=None, dry_run=False):
    table = check(jobs)
    waves = waves_for(table, max_parallel)
    if dry_run:
        return {"order": [name for wave in waves for name in wave], "failed": [], "skipped": {}, "attempts": {}}
    if not callable(execute):
        raise TypeError("execute must be callable")
    result = {"order": [], "failed": [], "skipped": {}, "attempts": {}}
    for name in (name for wave in waves for name in wave):
        causes = _causes(table, name, result["failed"], result["skipped"])
        if causes:
            result["skipped"][name] = causes
            continue
        tries, succeeded = _try(execute, name, table[name]["retries"])
        result["order"].append(name)
        result["attempts"][name] = tries
        if not succeeded:
            result["failed"].append(name)
    return result
''',
    "direct": '''
"""Running a plan (docs/scheduler.md)."""
from .graph import check
from .plan import waves_for


def run(jobs, execute, max_parallel=None, dry_run=False):
    table = check(jobs)
    waves = waves_for(table, max_parallel)
    if dry_run:
        return {"order": [name for wave in waves for name in wave], "failed": [], "skipped": {}, "attempts": {}}
    result = {"order": [], "failed": [], "skipped": {}, "attempts": {}}
    for wave in waves:
        for name in wave:
            blockers = sorted(dep for dep in table[name]["deps"] if dep in result["failed"])
            if blockers:
                result["skipped"][name] = blockers
                continue
            tries, succeeded = 0, False
            for tries in range(1, max(1, table[name]["retries"]) + 1):
                succeeded = bool(execute(name))
                if succeeded:
                    break
            result["order"].append(name)
            result["attempts"][name] = tries
            if not succeeded:
                result["failed"].append(name)
    return result
''',
}

VISIBLE_TESTS = '''
import unittest

from @@PKG@@.graph import CycleError
from @@PKG@@.plan import plan
from @@PKG@@.runner import run


def job(name, *deps, retries=0):
    return {"name": name, "deps": list(deps), "retries": retries}


class SchedulerTests(unittest.TestCase):
    def test_a_chain_runs_in_order(self):
        jobs = [job("c", "b"), job("b", "a"), job("a")]
        self.assertEqual(plan(jobs), [["a"], ["b"], ["c"]])

    def test_ready_jobs_share_a_wave_in_name_order(self):
        jobs = [job("z"), job("m"), job("a"), job("end", "z", "m", "a")]
        self.assertEqual(plan(jobs), [["a", "m", "z"], ["end"]])
        self.assertEqual(plan(jobs, max_parallel=2), [["a", "m"], ["z"], ["end"]])

    def test_two_job_cycle_is_reported(self):
        with self.assertRaises(CycleError) as caught:
            plan([job("a", "b"), job("b", "a")])
        self.assertEqual(caught.exception.cycle, ["a", "b", "a"])

    def test_failure_skips_the_direct_dependent(self):
        jobs = [job("a"), job("b", "a"), job("c")]
        result = run(jobs, lambda name: name != "a")
        self.assertEqual(result["failed"], ["a"])
        self.assertEqual(result["skipped"], {"b": ["a"]})
        self.assertEqual(result["order"], ["a", "c"])

    def test_dry_run_lists_the_plan(self):
        result = run([job("b", "a"), job("a")], None, dry_run=True)
        self.assertEqual(result["order"], ["a", "b"])


if __name__ == "__main__":
    unittest.main()
'''

HIDDEN = '''
import copy
import random
import unittest

from @@PKG@@.graph import CycleError, UnknownDependency
from @@PKG@@.plan import plan
from @@PKG@@.runner import run

PREFIX = "@@PREFIX@@"
HIGH_FIRST = @@HIGH_FIRST@@
FIRST_BY_NAME = @@FIRST_BY_NAME@@


def job(name, deps="", priority=0, retries=0):
    return {"name": PREFIX + name, "deps": [PREFIX + dep for dep in deps.split()], "priority": priority, "retries": retries}


def names(text):
    return [PREFIX + part for part in text.split()]


def priority_order(high_first_names):
    """The listed names (given highest priority first) in the seed's priority direction."""
    listed = names(high_first_names)
    return listed if HIGH_FIRST else listed[::-1]


def cycle_of(jobs):
    with unittest.TestCase().assertRaises(CycleError) as caught:
        plan(jobs)
    return caught.exception


def oracle_plan(table, width):
    sign = -1 if HIGH_FIRST else 1
    waves, done = [], set()
    while len(done) < len(table):
        ready = sorted((n for n in table if n not in done and table[n][0] <= done), key=lambda n: (sign * table[n][1], n))
        wave = ready[:width] if width else ready
        waves.append(wave)
        done |= set(wave)
    return waves


def oracle_run(table, retries, fails, width):
    result = {"order": [], "failed": [], "skipped": {}, "attempts": {}}
    for wave in oracle_plan(table, width):
        for name in wave:
            blockers = set()
            for dep in table[name][0]:
                blockers |= {dep} if dep in result["failed"] else set(result["skipped"].get(dep, []))
            if blockers:
                result["skipped"][name] = sorted(blockers)
                continue
            tries = min(fails.get(name, 0), retries[name]) + 1
            result["order"].append(name)
            result["attempts"][name] = tries
            if fails.get(name, 0) > retries[name]:
                result["failed"].append(name)
    return result


def random_jobs(rng):
    letters = rng.sample("abcdefghijklmnopqrstuvwxyzABCDEF", rng.randint(2, 14))
    jobs = []
    for index, letter in enumerate(letters):
        deps = " ".join(rng.sample(letters[:index], min(index, rng.randint(0, 3))))
        jobs.append(job(letter, deps, rng.randint(-2, 2), rng.randint(0, 2)))
    rng.shuffle(jobs)
    return jobs


def short(fails):
    return {PREFIX + name: count for name, count in fails.items()}


def scripted(fails):
    calls = []

    def execute(name):
        calls.append(name)
        return calls.count(name) > fails.get(name, 0)
    execute.calls = calls
    return execute


class HiddenSchedulerChecks(unittest.TestCase):
    def test_ties_break_by_priority_then_plain_name_order(self):
        jobs = [job("hi", priority=5), job("B"), job("a"), job("job10"), job("job2"), job("lo", priority=-3), job("mid", priority=1)]
        expected = (names("hi mid") + names("B a job10 job2") + names("lo")) if HIGH_FIRST else (
            names("lo") + names("B a job10 job2") + names("mid hi"))
        self.assertEqual(plan(jobs), [expected])
        self.assertEqual(plan(jobs, max_parallel=3), [expected[:3], expected[3:6], expected[6:]])
        three = [job("a", priority=1), job("b", priority=2), job("c", priority=3)]
        order = priority_order("c b a")
        self.assertEqual(plan(three, max_parallel=2), [order[:2], order[2:]])

    def test_a_job_waits_for_the_wave_after_its_dependencies(self):
        jobs = [job("a"), job("b", "a"), job("c", "a"), job("d", "b c"), job("e")]
        self.assertEqual(plan(jobs), [names("a e"), names("b c"), names("d")])
        self.assertEqual(plan(jobs, max_parallel=1), [names("a"), names("b"), names("c"), names("d"), names("e")])
        self.assertEqual(plan(jobs, max_parallel=2), [names("a e"), names("b c"), names("d")])

    def test_plan_matches_a_plain_layering_on_random_graphs(self):
        rng = random.Random(@@SEED@@)
        for _ in range(150):
            jobs = random_jobs(rng)
            table = {j["name"]: (set(j["deps"]), j["priority"]) for j in jobs}
            width = rng.choice([None, 1, 2, 3, 5])
            self.assertEqual(plan(jobs, width), oracle_plan(table, width), (jobs, width))

    def test_cycles_are_reported_in_canonical_form(self):
        cases = [
            ([job("c", "a"), job("a", "b"), job("b", "c")], "a b c a"),
            ([job("a", "a")], "a a"),
            ([job("a", "b"), job("b", "c"), job("c", "b")], "b c b"),
            ([job("a", "b c"), job("b", "d"), job("d", "a"), job("c", "a")], "a c a"),
            ([job("a", "c b"), job("b", "a"), job("c", "a")], "a b a"),
            ([job("x"), job("z", "y"), job("y", "z"), job("w", "z")], "y z y"),
            ([job("a", "B"), job("B", "a")], "B a B"),
            ([job("a", "b"), job("b", "c d"), job("c", "a"), job("d", "a")], "a b c a"),
        ]
        for jobs, expected in cases:
            error = cycle_of(jobs)
            self.assertEqual(error.cycle, names(expected), expected)
            self.assertIn(" -> ".join(names(expected)), str(error))
            self.assertIsInstance(error, ValueError)

    def test_unknown_dependencies_malformed_jobs_and_width(self):
        for bad in (0, -1, 2.0, "2", True):
            with self.assertRaises(ValueError, msg=repr(bad)):
                plan([job("a")], bad)
        self.assertEqual(plan([], None), [])
        jobs = [job("q", "ghost"), job("b", "phantom"), job("a", "b")]
        with self.assertRaises(UnknownDependency) as caught:
            plan(jobs)
        expected = ("b", "phantom") if FIRST_BY_NAME else ("q", "ghost")
        self.assertEqual((caught.exception.job, caught.exception.dependency), tuple(PREFIX + part for part in expected))
        both = [job("a", "b"), job("b", "a"), job("c", "ghost")]
        with self.assertRaises(UnknownDependency):
            plan(both)
        for bad in ([job("a"), job("a")], [{"deps": []}], [{"name": ""}], [{"name": "a", "deps": "b"}],
                    [{"name": "a", "priority": 1.5}], [{"name": "a", "retries": -1}], [{"name": "a", "retries": True}]):
            with self.assertRaises(ValueError, msg=repr(bad)):
                plan(bad)

    def test_failures_skip_every_transitive_dependent_and_name_the_causes(self):
        jobs = [job("a"), job("b", "a"), job("c", "b"), job("d", "c"), job("e"), job("f", "e a"), job("g", "e"),
                job("h", "g"), job("x1", "d")]
        result = run(jobs, scripted(short({"a": 1})))
        self.assertEqual(result["failed"], names("a"))
        self.assertEqual(result["skipped"], {PREFIX + "b": names("a"), PREFIX + "c": names("a"), PREFIX + "d": names("a"),
                                             PREFIX + "x1": names("a"), PREFIX + "f": names("a")})
        self.assertEqual(sorted(result["order"]), sorted(names("a e g h")))
        self.assertEqual(sorted(result["attempts"]), sorted(result["order"]))
        two = run([job("a"), job("b"), job("c", "a b"), job("d", "c")], scripted(short({"a": 1, "b": 1})))
        self.assertEqual(two["skipped"], {PREFIX + "c": names("a b"), PREFIX + "d": names("a b")})

    def test_retries_count_attempts_and_the_plan_stays_fixed(self):
        jobs = [job("a", retries=2), job("b", "a"), job("c", retries=1), job("d", "c"), job("e", retries=0)]
        execute = scripted(short({"a": 2, "c": 2, "e": 1}))
        result = run(jobs, execute, max_parallel=2)
        self.assertEqual(result["attempts"], {PREFIX + "a": 3, PREFIX + "b": 1, PREFIX + "c": 2, PREFIX + "e": 1})
        self.assertEqual(result["failed"], names("c e"))
        self.assertEqual(result["order"], names("a c b e"))
        self.assertEqual(result["skipped"], {PREFIX + "d": names("c")})
        self.assertEqual(execute.calls, names("a a a c c b e"))

    def test_dry_run_executes_nothing_and_still_validates(self):
        jobs = [job("b", "a"), job("a"), job("c", "a", retries=3)]
        for execute in (None, scripted({})):
            result = run(jobs, execute, max_parallel=1, dry_run=True)
            self.assertEqual(result, {"order": names("a b c"), "failed": [], "skipped": {}, "attempts": {}})
        with self.assertRaises(CycleError):
            run([job("a", "a")], None, dry_run=True)
        with self.assertRaises(UnknownDependency):
            run([job("a", "z")], None, dry_run=True)

    def test_inputs_are_never_modified(self):
        jobs = [job("c", "a b", 2), {"name": PREFIX + "a"}, job("b", "a", retries=1), job("d", "c")]
        before = copy.deepcopy(jobs)
        first = plan(jobs, 2)
        self.assertEqual(jobs, before)
        run(jobs, scripted(short({"a": 5})), max_parallel=2)
        run(jobs, None, dry_run=True)
        self.assertEqual(jobs, before)
        self.assertEqual(plan(jobs, 2), first)
        first[0].append("junk")
        self.assertEqual(plan(jobs, 2)[0], first[0][:-1])
        for broken in ([job("a", "b"), job("b", "a")], [job("a", "ghost")]):
            snapshot = copy.deepcopy(broken)
            with self.assertRaises(ValueError):
                run(broken, scripted({}))
            self.assertEqual(broken, snapshot)

    def test_run_matches_a_plain_model_on_random_graphs(self):
        rng = random.Random(@@SEED@@ + 1)
        for _ in range(150):
            jobs = random_jobs(rng)
            table = {j["name"]: (set(j["deps"]), j["priority"]) for j in jobs}
            retries = {j["name"]: j["retries"] for j in jobs}
            fails = {j["name"]: rng.randint(1, 4) for j in jobs if rng.random() < 0.3}
            width = rng.choice([None, 1, 2, 4])
            self.assertEqual(run(copy.deepcopy(jobs), scripted(fails), width), oracle_run(table, retries, fails, width), (jobs, fails, width))
'''

PROMPTS = (
    "`@@PKG@@/` plans and runs jobs with dependencies, and `docs/scheduler.md` says what it must do. The visible "
    "tests pass but production plans come out in the wrong order and failures are handled badly. Make the code "
    "follow the doc.",
    "Our job scheduler in `@@PKG@@/` misbehaves on cycles, retries and failed jobs, and its plans differ from "
    "what `docs/scheduler.md` describes. Fix `graph.py`, `plan.py` and `runner.py` so they meet the doc.",
    "Audit `@@PKG@@/` against `docs/scheduler.md` and fix every departure: ordering, error reporting, failure "
    "propagation, retries and the dry run.",
)


def _modules(params, bad):
    """(graph, plan, runner) module text; `bad` selects the starter's defective variants."""
    find_unknown = fill(FIND_UNKNOWN, PICK=PICKS[params["UNKNOWN"]])
    graph = fill(GRAPH, FIND_UNKNOWN=find_unknown, FIND_CYCLE=FIND_CYCLE["dfs" if bad else "good"],
                 CHECK=CHECK["cycle_first" if bad else "good"])
    sign = -1 if params["HIGH_FIRST"] else 1
    plan_text = fill(PLAN["kahn" if bad else "good"], allow_unused=True, SIGN=sign, WRONG_SIGN=-sign)
    return graph, plan_text, RUNNER["direct" if bad else "good"]


def generate(seed, rng):
    pkg = package_name(rng, "jobs")
    params = {"PREFIX": rng.choice(PREFIXES), "HIGH_FIRST": rng.random() < 0.5, "UNKNOWN": rng.choice(sorted(UNKNOWN_RULES))}
    priority = "higher numbers first" if params["HIGH_FIRST"] else "lower numbers first"
    doc = fill(DOC, UNKNOWN_TEXT=UNKNOWN_RULES[params["UNKNOWN"]], PRIORITY_TEXT=priority)
    hidden = fill(HIDDEN, PKG=pkg, PREFIX=params["PREFIX"], HIGH_FIRST=params["HIGH_FIRST"],
                  FIRST_BY_NAME=params["UNKNOWN"] == "name", SEED=seed)
    starter, solution = _modules(params, bad=True), _modules(params, bad=False)
    names = ("graph", "plan", "runner")
    files = {"README.md": f"# {pkg}\n\nA small job scheduler. Rules: `docs/scheduler.md`.\n", "docs/scheduler.md": doc,
             f"{pkg}/__init__.py": "", "tests/__init__.py": "", "tests/test_scheduler.py": fill(VISIBLE_TESTS, PKG=pkg),
             **{f"{pkg}/{name}.py": text for name, text in zip(names, starter)}}
    return make_spec(family="ordering", template="jobgraph", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/{name}.py": text for name, text in zip(names, solution)},
                     meta={"high_first": params["HIGH_FIRST"], "unknown": params["UNKNOWN"]})
