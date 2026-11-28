"""Host-sized defaults for running release-bench trials side by side, and per-invocation timing.

A trial is one agent CLI plus, on Aether and Archon arms, its own daemons: mostly
waiting on the provider, with bursts of CPU when it runs tests. The defaults leave
two cores and about a gigabyte per trial for the rest of the desktop.
"""
import json
import os
import time

MAX_DEFAULT_JOBS = 10
GB_PER_TRIAL = 1.2
LOAD_PER_CORE = 1.25


def _total_memory_gb():
    """MemTotal from /proc/meminfo, or None where the host has none."""
    try:
        with open("/proc/meminfo") as stream:
            for line in stream:
                if line.startswith("MemTotal:"):
                    return int(line.split()[1]) / 1024 / 1024
    except OSError:
        return None
    return None


def default_jobs(cpus=None, memory_gb=None):
    """Trials at once: all cores but two, at most one per GB_PER_TRIAL of memory, from 2 to MAX_DEFAULT_JOBS."""
    cpus = cpus or os.cpu_count() or 8
    memory_gb = memory_gb if memory_gb is not None else _total_memory_gb()
    assert cpus >= 1, "a host has a core"
    jobs = cpus - 2
    if memory_gb is not None:
        jobs = min(jobs, int(memory_gb // GB_PER_TRIAL))
    jobs = max(2, min(MAX_DEFAULT_JOBS, jobs))
    assert 2 <= jobs <= MAX_DEFAULT_JOBS, "the default stays in its bounds"
    return jobs


def default_load_max(cpus=None):
    """Start no trial above this 1-minute load: LOAD_PER_CORE per core (15 on 12 cores)."""
    cpus = cpus or os.cpu_count() or 8
    assert cpus >= 1, "a host has a core"
    return round(cpus * LOAD_PER_CORE, 1)


def record_invocation(run_dir, started_monotonic, jobs, trials_run):
    """Append this invocation's wall time to the run's timing.json; returns every invocation so far."""
    wall = time.monotonic() - started_monotonic
    assert wall >= 0 and jobs >= 1 and trials_run >= 0, "timing is nonnegative and a run has a worker"
    path = os.path.join(run_dir, "timing.json")
    invocations = read_invocations(run_dir)
    invocations.append({"ended_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "jobs": jobs,
                        "wall_seconds": round(wall, 1), "trials_run": trials_run})
    with open(path, "w") as stream:
        json.dump({"invocations": invocations}, stream, indent=2)
    return invocations


def read_invocations(run_dir):
    """Every invocation timing.json recorded for the run, or [] before the first."""
    path = os.path.join(run_dir, "timing.json")
    if not os.path.exists(path):
        return []
    with open(path) as stream:
        invocations = json.load(stream)["invocations"]
    assert all(item["wall_seconds"] >= 0 for item in invocations), "recorded wall times are nonnegative"
    return invocations
