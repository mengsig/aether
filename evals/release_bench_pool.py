"""Parallel, resumable, budget- and load-gated execution of release-bench trials.

Trials of one (task, repeat) run back to back with a rotated arm order, so
with enough workers every arm of a task runs at the same time, under the same
load and provider weather: the pairs the statistics compare stay paired.
"""
import json
import os
import random
import threading
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait

from harness_bench_process import terminate_owned_process_groups

GATE_POLL_SECONDS = 5


def schedule(task_ids, arm_keys, repeats, seed):
    """(task, repeat, arm) triples: tasks shuffled, each task's arms adjacent and rotated per task."""
    assert len(set(task_ids)) == len(task_ids) and task_ids, "schedule each task once"
    assert repeats > 0 and arm_keys, "schedule at least one repeat of one arm"
    rng = random.Random(seed)
    tasks = list(task_ids)
    rng.shuffle(tasks)
    order = []
    for repeat in range(repeats):
        for position, task in enumerate(tasks):
            shift = (position + repeat) % len(arm_keys)
            order += [(task, repeat, arm) for arm in arm_keys[shift:] + arm_keys[:shift]]
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

    def __init__(self, load_max, mem_min_gb, budget_usd, unpriced_usd):
        assert load_max > 0 and mem_min_gb >= 0 and budget_usd > 0, "gate limits are positive"
        assert all(value > 0 for value in unpriced_usd.values()), "an unpriced trial is assumed to cost something"
        self.load_max, self.mem_min_gb, self.budget_usd = load_max, mem_min_gb, budget_usd
        self.unpriced_usd = dict(unpriced_usd)
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

    def reserve(self, model):
        """Hold a trial's worst case against the budget; False when it no longer fits."""
        worst = self.unpriced_usd[model]
        if self.spent_usd + self.reserved_usd + worst > self.budget_usd:
            return False
        self.reserved_usd += worst
        return True

    def settle(self, model, cost_usd):
        self.reserved_usd -= self.unpriced_usd[model]
        self.spent_usd += self.unpriced_usd[model] if cost_usd is None else cost_usd
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


def run_pool(order, run_one, model_of, *, jobs, gates, results_path, log, should_stop=None):
    """Run `order` (skipping trials already in `results_path`) on `jobs` workers; returns every record.

    `run_one(triple) -> record` must never raise for a trial's own failure.
    `should_stop(records)` may end the run early; budget exhaustion also stops admission."""
    assert jobs > 0, "a pool needs at least one worker"
    records = read_results(results_path)
    done = {trial_key(item) for item in records}
    for item in records:
        gates.spent_usd += gates.unpriced_usd[item["model"]] if item["cost_usd"] is None else item["cost_usd"]
    pending = [triple for triple in order if tuple(triple) not in done]
    log(f"{len(done)} trial(s) already recorded, {len(pending)} to run on {jobs} worker(s)")
    lock, stop, stopped_reason = threading.Lock(), threading.Event(), [None]
    with open(results_path, "a") as sink, ThreadPoolExecutor(max_workers=jobs) as pool:
        running = {}
        try:
            for triple in pending:
                while len(running) >= jobs:
                    _collect(wait(running, return_when=FIRST_COMPLETED).done, running, records, sink, lock,
                             gates, log)
                    if should_stop is not None and stopped_reason[0] is None:
                        stopped_reason[0] = should_stop(records)
                if stopped_reason[0]:
                    break
                _await_host(gates, log, stop)
                reserved = gates.reserve(model_of(triple))
                while not reserved and running:
                    _collect(wait(running, return_when=FIRST_COMPLETED).done, running, records, sink, lock,
                             gates, log)
                    reserved = gates.reserve(model_of(triple))
                if not reserved:
                    stopped_reason[0] = f"budget ${gates.budget_usd:.2f} reached (${gates.spent_usd:.2f} spent)"
                    break
                running[pool.submit(run_one, triple)] = triple
            _collect(wait(running).done, running, records, sink, lock, gates, log)
        except BaseException:
            stop.set()
            terminate_owned_process_groups()
            raise
    if stopped_reason[0]:
        log(f"stopped early: {stopped_reason[0]}")
    return records, stopped_reason[0]


def _collect(finished, running, records, sink, lock, gates, log):
    for future in finished:
        triple = running.pop(future)
        item = future.result()
        assert trial_key(item) == tuple(triple), f"record {trial_key(item)} does not match its trial {triple}"
        with lock:
            gates.settle(item["model"], item["cost_usd"])
            sink.write(json.dumps(item, sort_keys=True) + "\n")
            sink.flush()
            records.append(item)
        cost = "unpriced" if item["cost_usd"] is None else f"${item['cost_usd']:.4f}"
        verdict = "INFRA" if item["infra_error"] else ("pass" if item["passed"] else "fail")
        log(f"[{len(records)}] {item['arm']:<12} {item['task']:<16} r{item['repeat']} {verdict:<5} "
            f"{item['checks_passed']}/{item['checks_total']} {cost} {item['seconds'] or 0:.0f}s "
            f"(spent ${gates.spent_usd:.2f})")
