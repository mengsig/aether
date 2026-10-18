"""Parallel, resumable, budget- and load-gated execution of release-bench trials.

Trials of one (task, repeat) run back to back, so with enough workers every
arm of a task runs at the same time, under the same load and provider
weather: the pairs the statistics compare stay paired. Arm order within a
block is shuffled, and old/new order alternates exactly per model tier.
Every admitted trial is journalled before it starts; a resumed run charges an
attempt that never recorded a result its full reservation.
"""
import json
import os
import random
import threading
import time
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait

from harness_bench_process import terminate_owned_process_groups

GATE_POLL_SECONDS = 5


def schedule(task_ids, arm_keys, repeats, seed):
    """(task, repeat, arm) triples: one block per (repeat, task) in seeded order, its arms adjacent.

    Each block's arms are shuffled; then, per model tier, `old` runs before `new`
    in alternate blocks, so neither binary systematically goes first."""
    assert len(set(task_ids)) == len(task_ids) and task_ids, "schedule each task once"
    assert repeats > 0 and len(set(arm_keys)) == len(arm_keys) and arm_keys, "schedule distinct arms"
    rng = random.Random(seed)
    tasks = list(task_ids)
    rng.shuffle(tasks)
    models = sorted({key.split("/")[0] for key in arm_keys})
    order = []
    for block, (repeat, task) in enumerate((repeat, task) for repeat in range(repeats) for task in tasks):
        arms = list(arm_keys)
        rng.shuffle(arms)
        for index, model in enumerate(models):
            old, new = f"{model}/old", f"{model}/new"
            if old in arms and new in arms:
                i, j = arms.index(old), arms.index(new)
                old_first = (block + index) % 2 == 0
                if (i < j) != old_first:
                    arms[i], arms[j] = arms[j], arms[i]
        order += [(task, repeat, arm) for arm in arms]
    assert len(order) == len(task_ids) * len(arm_keys) * repeats, "every cell is scheduled exactly once"
    return order


def trial_key(item):
    return (item["task"], item["repeat"], item["arm"]) if isinstance(item, dict) else tuple(item)


def read_results(path):
    """Records already written by an earlier, interrupted invocation of the same run."""
    if not os.path.exists(path):
        return []
    records = []
    with open(path) as stream:
        for number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                records.append(json.loads(line))
            except json.JSONDecodeError as error:
                raise ValueError(f"{path}:{number}: torn results line; delete it to resume") from error
    keys = [trial_key(item) for item in records]
    assert len(set(keys)) == len(keys), f"{path} holds a trial twice"
    return records


def available_memory_gb():
    """MemAvailable from /proc/meminfo, or None where the host has none."""
    try:
        with open("/proc/meminfo") as stream:
            for line in stream:
                if line.startswith("MemAvailable:"):
                    return int(line.split()[1]) / 1024 / 1024
    except OSError:
        return None
    return None


class Gates:
    """Admission control: host load, free memory and the run's spend budget."""

    def __init__(self, load_max, mem_min_gb, budget_usd):
        assert load_max > 0 and mem_min_gb >= 0 and budget_usd > 0, "gate limits are positive"
        self.load_max, self.mem_min_gb, self.budget_usd = load_max, mem_min_gb, budget_usd
        self.spent_usd = 0.0
        self.reserved_usd = 0.0

    def host_blocker(self):
        """Why the host cannot take another trial now, or None."""
        load = os.getloadavg()[0]
        if load > self.load_max:
            return f"1-minute load {load:.1f} > {self.load_max}"
        memory = available_memory_gb()
        if memory is not None and memory < self.mem_min_gb:
            return f"available memory {memory:.1f} GB < {self.mem_min_gb} GB"
        return None

    def reserve(self, amount_usd):
        """Hold a trial's worst case against the budget; False when it no longer fits."""
        assert amount_usd > 0, "a trial reserves a positive worst case"
        if self.spent_usd + self.reserved_usd + amount_usd > self.budget_usd:
            return False
        self.reserved_usd += amount_usd
        return True

    def settle(self, amount_usd, cost_usd):
        """Release a reservation for what the trial cost; an unpriced trial is charged all of it."""
        self.reserved_usd -= amount_usd
        self.spent_usd += amount_usd if cost_usd is None else cost_usd
        assert self.reserved_usd > -1e-9 and self.spent_usd >= 0, "spend accounting went negative"


def _await_host(gates, log, stop):
    """Block until the host gate opens; logs once per change of reason."""
    last = None
    while not stop.is_set():
        reason = gates.host_blocker()
        if reason is None:
            return
        if reason != last:
            log(f"waiting: {reason}")
            last = reason
        stop.wait(GATE_POLL_SECONDS)


def read_attempts(path):
    """Trial starts journalled by earlier invocations of the same run.

    Every start is written and synced as one line before its trial is submitted,
    so only an unterminated last line can be torn, and that trial never ran."""
    if not os.path.exists(path):
        return []
    with open(path) as stream:
        lines = stream.read().split("\n")
    lines.pop()  # The unterminated tail: empty after a clean last write.
    attempts = []
    for number, line in enumerate(lines, 1):
        try:
            attempts.append(json.loads(line))
        except json.JSONDecodeError as error:
            raise ValueError(f"{path}:{number}: a complete journal line does not parse") from error
    assert all(len(trial_key(item)) == 3 and item["reserve_usd"] > 0 for item in attempts), "attempts are trials"
    return attempts


def cut_torn_tail(path):
    """Truncate an unterminated last line of a journal, so the next start begins a line of its own.

    Returns how many bytes were cut."""
    if not os.path.exists(path):
        return 0
    with open(path, "rb+") as stream:
        data = stream.read()
        keep = data.rfind(b"\n") + 1
        if keep < len(data):
            stream.truncate(keep)
            stream.flush()
            os.fsync(stream.fileno())
    assert 0 <= keep <= len(data), "the cut stays inside the file"
    return len(data) - keep


def charge_history(gates, records, attempts, reservation_of):
    """Charge the budget for recorded trials and for every abandoned attempt; returns the abandoned count."""
    for item in records:
        cost = item["cost_usd"]
        gates.spent_usd += reservation_of(trial_key(item)) if cost is None else cost
    recorded = {trial_key(item) for item in records}
    starts = {}
    for item in attempts:
        starts.setdefault(trial_key(item), []).append(item["reserve_usd"])
    abandoned = 0
    for key, reserves in starts.items():
        # A recorded trial's last start is the one that recorded it; every other start was abandoned.
        lost = reserves[:-1] if key in recorded else reserves
        gates.spent_usd += sum(lost)
        abandoned += len(lost)
    assert gates.spent_usd >= 0 and abandoned >= 0, "history charges are nonnegative"
    return abandoned


def run_pool(order, run_one, reservation_of, *, jobs, gates, results_path, attempts_path, log, should_stop=None):
    """Run `order` (skipping trials already in `results_path`) on `jobs` workers; returns every record.

    `run_one(triple) -> record` must never raise for a trial's own failure.
    `should_stop(records)` may end the run early; budget exhaustion also stops admission."""
    assert jobs > 0, "a pool needs at least one worker"
    records = read_results(results_path)
    abandoned = charge_history(gates, records, read_attempts(attempts_path), reservation_of)
    torn = cut_torn_tail(attempts_path)
    if torn:
        log(f"cut a torn {torn}-byte start from the end of {attempts_path}: that trial never ran")
    done = {trial_key(item) for item in records}
    pending = [triple for triple in order if tuple(triple) not in done]
    log(f"{len(done)} trial(s) already recorded, {abandoned} abandoned attempt(s) charged "
        f"(${gates.spent_usd:.2f} spent), {len(pending)} to run on {jobs} worker(s)")
    state = {"records": records, "running": {}, "lock": threading.Lock(), "stop": threading.Event(),
             "gates": gates, "log": log, "reservation_of": reservation_of,
             "held_seconds": {"host": 0.0, "budget": 0.0}}
    with open(results_path, "a") as sink, open(attempts_path, "a") as journal, \
            ThreadPoolExecutor(max_workers=jobs) as pool:
        state["sink"] = sink
        try:
            reason = _admit_all(pending, run_one, pool, journal, jobs, should_stop, state)
            _collect(wait(state["running"]).done, state)
        except BaseException:
            state["stop"].set()
            terminate_owned_process_groups()
            raise
    held = state["held_seconds"]
    log(f"a free worker waited {held['host'] / 60:.1f} min on the host gate and "
        f"{held['budget'] / 60:.1f} min for budget headroom")
    if reason:
        log(f"stopped early: {reason}")
    return records, reason


def _admit_all(pending, run_one, pool, journal, jobs, should_stop, state):
    """Submit trials while the budget, host and early-stop rule allow; returns why it stopped, or None."""
    running, gates = state["running"], state["gates"]
    reason = None
    for triple in pending:
        while len(running) >= jobs:
            _collect(wait(running, return_when=FIRST_COMPLETED).done, state)
            if should_stop is not None and reason is None:
                reason = should_stop(state["records"])
        if reason:
            return reason
        amount = state["reservation_of"](tuple(triple))
        if not _admit(amount, state):
            return f"budget ${gates.budget_usd:.2f} reached (${gates.spent_usd:.2f} spent)"
        task, repeat, arm = triple
        journal.write(json.dumps({"task": task, "repeat": repeat, "arm": arm, "reserve_usd": amount,
                                  "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}) + "\n")
        journal.flush()
        os.fsync(journal.fileno())
        running[pool.submit(run_one, triple)] = triple
    return reason


def _admit(amount, state):
    """Reserve `amount` once the host gate is open; False when the budget cannot hold it with nothing running.

    The host is checked again after every wait for budget headroom, since load and
    memory change while running trials finish: admission follows an open gate at once."""
    running, gates = state["running"], state["gates"]
    assert amount > 0, "a trial reserves a positive worst case"
    while True:
        held = time.monotonic()
        _await_host(gates, state["log"], state["stop"])
        state["held_seconds"]["host"] += time.monotonic() - held
        if gates.reserve(amount):
            assert gates.reserved_usd >= amount - 1e-9, "the reservation is held while the trial runs"
            return True
        if not running:
            return False
        # A free worker waits for budget headroom: the price of reserving every trial's worst case.
        held = time.monotonic()
        _collect(wait(running, return_when=FIRST_COMPLETED).done, state)
        state["held_seconds"]["budget"] += time.monotonic() - held


def _collect(finished, state):
    gates = state["gates"]
    for future in finished:
        triple = state["running"].pop(future)
        item = future.result()
        assert trial_key(item) == tuple(triple), f"record {trial_key(item)} does not match its trial {triple}"
        with state["lock"]:
            gates.settle(state["reservation_of"](tuple(triple)), item["cost_usd"])
            state["sink"].write(json.dumps(item, sort_keys=True) + "\n")
            state["sink"].flush()
            state["records"].append(item)
        cost = "unpriced" if item["cost_usd"] is None else f"${item['cost_usd']:.4f}"
        verdict = ("INFRA" if item["infra_error"] else "HARN" if item.get("harness_failure")
                   else "pass" if item["passed"] else "fail")
        state["log"](f"[{len(state['records'])}] {item['arm']:<12} {item['task']:<16} r{item['repeat']} "
                     f"{verdict:<5} {item['checks_passed']}/{item['checks_total']} {cost} "
                     f"{item['seconds'] or 0:.0f}s (spent ${gates.spent_usd:.2f})")
