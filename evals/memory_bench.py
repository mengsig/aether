#!/usr/bin/env python3
"""Memory benchmark: Aether's operational memory on a labelled, synthetic set.

  run      seed a private store per arm through the real admission path, rank
           every query as a turn does, and print the report. --check compares
           the deterministic metrics with the set's baseline; --write-baseline
           records them. No model is called unless --task-arm is given.
  report   re-render the report of an earlier run's results.json.
  compare  two results.json files, metric by metric (a baseline and a change).
  profile  time each stage of retrieval on the same stores (driver --profile).

See docs/memory/BASELINE.md. `make bench-memory` runs the baseline check.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import memory_bench_profile as profiling
import memory_bench_task as task_arm
from memory_bench_dataset import build_plan, load_set, memory_index, queries
from memory_bench_metrics import compute, differences
from memory_bench_report import compare, render

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRATCH = REPO_ROOT / "target" / "memory-bench"
DEFAULT_SET = REPO_ROOT / "bench" / "memory-set" / "v1"
DRIVER_TIMEOUT_SECONDS = 900
ARMS = ("none", "local")


def log(message):
    print(f"{time.strftime('%H:%M:%S')} {message}", file=sys.stderr, flush=True)


def save_json(path, obj):
    """Atomic write: an interrupted run leaves the previous file, not a torn one."""
    tmp = f"{path}.tmp"
    with open(tmp, "w") as f:
        json.dump(obj, f, indent=2, sort_keys=False)
        f.write("\n")
    os.replace(tmp, path)


def wait_for_load(load_max):
    """The host runs one heavy job at a time: start only below `load_max`."""
    while os.getloadavg()[0] > load_max:
        log(f"1-minute load {os.getloadavg()[0]:.1f} is above {load_max}; waiting")
        time.sleep(30)


def git_state(repo):
    commit = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"],
                            capture_output=True, text=True, check=True).stdout.strip()
    status = subprocess.run(["git", "-C", str(repo), "status", "--porcelain", "--untracked-files=no"],
                            capture_output=True, text=True, check=True).stdout
    assert len(commit) == 40, commit
    return commit, bool(status.strip())


def build_driver(repo):
    """Build the driver from `repo` and return its path in cargo's target dir."""
    log(f"building the driver in {repo}")
    subprocess.run(["cargo", "build", "--release", "-p", "aether-memory", "--example", "memory_bench"],
                   cwd=repo, check=True)
    metadata = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"],
                              cwd=repo, capture_output=True, text=True, check=True)
    driver = Path(json.loads(metadata.stdout)["target_directory"]) / "release" / "examples" / "memory_bench"
    if not driver.is_file():
        raise FileNotFoundError(f"cargo built no driver at {driver}")
    return driver


def run_driver(driver, plan, run_dir, arm, mode=()):
    """One arm in a fresh sandbox, removed afterwards whatever happens."""
    plan_path = run_dir / f"plan-{arm}.json"
    save_json(plan_path, plan)
    SCRATCH.mkdir(parents=True, exist_ok=True)
    sandbox = Path(tempfile.mkdtemp(prefix=f"sandbox-{arm}-", dir=SCRATCH))
    try:
        with open(run_dir / f"driver-{arm}.log", "w") as stderr:
            completed = subprocess.run([str(driver), *mode, str(plan_path), str(sandbox)], stdout=subprocess.PIPE,
                                       stderr=stderr, text=True, timeout=DRIVER_TIMEOUT_SECONDS)
    finally:
        shutil.rmtree(sandbox, ignore_errors=False)
    if completed.returncode != 0:
        raise RuntimeError(f"driver failed on index={arm} (exit {completed.returncode}); see {run_dir}/driver-{arm}.log")
    report = json.loads(completed.stdout)
    assert report["index"] == arm, report["index"]
    return report


def cmd_run(args):
    dataset = load_set(args.set)
    arms = [a.strip() for a in args.index.split(",") if a.strip()]
    if not arms or any(a not in ARMS for a in arms):
        raise SystemExit(f"--index takes a comma list of {', '.join(ARMS)}")
    wait_for_load(args.load_max)
    driver = Path(args.driver) if args.driver else build_driver(Path(args.repo))
    commit, dirty = git_state(Path(args.repo))
    run_dir = Path(args.out) if args.out else SCRATCH / time.strftime("%Y%m%d-%H%M%S")
    run_dir.mkdir(parents=True, exist_ok=True)
    results = {"set": f"{dataset['manifest']['name']}/{dataset['manifest']['version']}",
               "commit": commit, "dirty": dirty, "arms": {}}
    reports = {}
    for arm in arms:
        log(f"running index={arm}")
        plan = build_plan(dataset, index=arm, reps=args.reps, filler=args.filler)
        reports[arm] = run_driver(driver, plan, run_dir, arm)
        results["arms"][arm] = compute(dataset, reports[arm])
    if args.task_arm:
        results["task_arm"] = run_task_arm(args, dataset, reports[arms[0]], results["arms"][arms[0]])
    save_json(run_dir / "results.json", results)
    save_json(run_dir / "driver.json", reports)
    text = render(results)
    (run_dir / "report.md").write_text(text)
    print(text)
    log(f"results in {run_dir}")
    if args.write_baseline:
        write_baseline(Path(args.set) / "baseline.json", results)
    if args.check:
        return check(Path(args.set) / "baseline.json", results)
    return 0


def run_task_arm(args, dataset, report, arm):
    log(f"task arm through `{args.task_command}`, at most {args.task_max_calls} calls")
    rows = {row["id"]: row for row in report["queries"]}
    return task_arm.run(args.task_command, queries(dataset), rows, memory_index(dataset),
                        arm["queries"], args.task_max_calls)


def write_baseline(path, results):
    baseline = {
        "set": results["set"],
        "commit": results["commit"],
        "arms": {arm: {"deterministic": r["deterministic"], "measured": r["measured"]}
                 for arm, r in results["arms"].items()},
    }
    save_json(path, baseline)
    log(f"baseline written to {path}")


def check(path, results):
    """Exit status 1 when a deterministic metric differs from the baseline."""
    baseline = json.loads(Path(path).read_text())
    problems = []
    for arm, result in results["arms"].items():
        expected = baseline["arms"].get(arm)
        if expected is None:
            problems.append(f"index={arm}: the baseline has no such arm")
            continue
        problems += [f"index={arm}: {d}" for d in differences(expected["deterministic"], result["deterministic"])]
    if problems:
        print(f"BASELINE MISMATCH against {path} (recorded at {baseline['commit']}):", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print(f"BASELINE OK: deterministic metrics match {path} (recorded at {baseline['commit']}).")
    return 0


def cmd_profile(args):
    """The stage split of retrieval per arm, raw samples and a summary."""
    dataset = load_set(args.set)
    arms = [a.strip() for a in args.index.split(",") if a.strip()]
    if not arms or any(a not in ARMS for a in arms):
        raise SystemExit(f"--index takes a comma list of {', '.join(ARMS)}")
    wait_for_load(args.load_max)
    driver = Path(args.driver) if args.driver else build_driver(Path(args.repo))
    commit, dirty = git_state(Path(args.repo))
    run_dir = Path(args.out) if args.out else SCRATCH / time.strftime("profile-%Y%m%d-%H%M%S")
    run_dir.mkdir(parents=True, exist_ok=True)
    raw, summaries = {}, {"commit": commit, "dirty": dirty, "arms": {}}
    for arm in arms:
        log(f"profiling index={arm}")
        plan = build_plan(dataset, index=arm, reps=args.reps, filler=args.filler)
        raw[arm] = run_driver(driver, plan, run_dir, arm, mode=("--profile",))
        assert raw[arm]["index"] == arm, raw[arm]["index"]
        summaries["arms"][arm] = profiling.summarise(raw[arm])
    save_json(run_dir / "profile-raw.json", raw)
    save_json(run_dir / "profile.json", summaries)
    text = "\n".join(profiling.render(s) for s in summaries["arms"].values())
    (run_dir / "profile.md").write_text(text)
    print(text)
    log(f"profile in {run_dir}")
    return 0


def cmd_report(args):
    print(render(json.loads(Path(args.results).read_text())))
    return 0


def cmd_compare(args):
    before = json.loads(Path(args.before).read_text())
    after = json.loads(Path(args.after).read_text())
    if before["set"] != after["set"]:
        raise SystemExit(f"different sets: {before['set']} and {after['set']}; results are not comparable")
    print(compare(before, after))
    return 0


def parser():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    run = sub.add_parser("run", help="run the benchmark")
    run.add_argument("--set", default=str(DEFAULT_SET), help="dataset directory")
    run.add_argument("--repo", default=str(REPO_ROOT), help="checkout whose driver is built and measured")
    run.add_argument("--driver", help="a prebuilt driver binary instead of building one")
    run.add_argument("--index", default=",".join(ARMS), help="arms: none (the default config), local")
    run.add_argument("--reps", type=int, default=30, help="timed retrievals per query")
    run.add_argument("--filler", type=int, default=5000, help="synthetic memories in the scaled store")
    run.add_argument("--load-max", type=float, default=10.0, help="wait while the 1-minute load is above this")
    run.add_argument("--out", help="results directory (default target/memory-bench/<time>)")
    run.add_argument("--check", action="store_true", help="exit 1 unless the baseline reproduces")
    run.add_argument("--write-baseline", action="store_true", help="record this run as the set's baseline")
    run.add_argument("--task-arm", action="store_true", help="also run the model-graded task arm (spends tokens)")
    run.add_argument("--task-command", default='claude -p --model haiku --tools ""',
                     help="model command: prompt on stdin, answer on stdout")
    run.add_argument("--task-max-calls", type=int, default=24, help="refuse a task arm needing more calls")
    run.set_defaults(func=cmd_run)
    prof = sub.add_parser("profile", help="time each stage of retrieval")
    prof.add_argument("--set", default=str(DEFAULT_SET), help="dataset directory")
    prof.add_argument("--repo", default=str(REPO_ROOT), help="checkout whose driver is built and measured")
    prof.add_argument("--driver", help="a prebuilt driver binary instead of building one")
    prof.add_argument("--index", default=",".join(ARMS), help="arms: none (the default config), local")
    prof.add_argument("--reps", type=int, default=30, help="timed passes per query")
    prof.add_argument("--filler", type=int, default=5000, help="synthetic memories in the scaled store")
    prof.add_argument("--load-max", type=float, default=10.0, help="wait while the 1-minute load is above this")
    prof.add_argument("--out", help="results directory (default target/memory-bench/profile-<time>)")
    prof.set_defaults(func=cmd_profile)
    report = sub.add_parser("report", help="re-render a results.json")
    report.add_argument("results")
    report.set_defaults(func=cmd_report)
    comp = sub.add_parser("compare", help="compare two results.json files")
    comp.add_argument("before")
    comp.add_argument("after")
    comp.set_defaults(func=cmd_compare)
    return p


def main(argv=None):
    args = parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
