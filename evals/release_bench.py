#!/usr/bin/env python3
"""Release bench: a calibrated, cheap, parallel benchmark run before every release.

  run        the frozen set on native, old-release and candidate arms; prints a
             Markdown report and exits 1 unless the release gate passes.
  calibrate  generated candidates on the native arms only; with --freeze, writes
             the chosen set as a new frozen version.
  report     re-renders the report of an earlier run directory.

See docs/release-bench.md. Usually driven by `make bench-release` / `make bench-calibrate`.
"""
import argparse
import json
import os
import shutil
import sys
import tempfile
import time
import uuid
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import release_bench_families as families
from harness_bench import fetch_price_table, save_json
from harness_bench_assets import native_cli_provenance, pin_binary
from harness_bench_metadata import native_configured_defaults
from release_bench_arms import build_arm, parse_arms
from release_bench_calibrate import SELECTION_DEFAULTS, acceptance, select, task_rates
from release_bench_calibrate import render as render_calibration
from release_bench_generate import verify
from release_bench_pool import Gates, read_results, run_pool, schedule
from release_bench_report import render
from release_bench_spend import reservation_usd
from release_bench_stats import (
    GATE_DEFAULTS,
    interim_fail,
    reached_looks,
    sequential_confidence,
)
from release_bench_tasks import load_set, write_set
from release_bench_trial import TRIAL_TIMEOUT_SECONDS, run_release_trial

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRATCH = REPO_ROOT / "target" / "release-bench"
DEFAULT_SET = REPO_ROOT / "bench" / "release-set" / "v2"
MODELS = ("haiku", "luna")


def log(message):
    print(f"{time.strftime('%H:%M:%S')} {message}", flush=True)


def _pool_options(parser, budget):
    parser.add_argument("--jobs", type=int, default=6, help="trials at once (default 6)")
    parser.add_argument("--load-max", type=float, default=16.0, help="start no trial above this 1-minute load")
    parser.add_argument("--mem-min-gb", type=float, default=2.0, help="start no trial below this free memory")
    parser.add_argument("--budget", type=float, default=budget, help="stop admitting trials at this spend (USD)")
    parser.add_argument("--timeout", type=int, default=TRIAL_TIMEOUT_SECONDS, help="seconds per trial")
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--run-id", help="resume or name a run (default: a new id)")
    parser.add_argument("--work-root", type=Path, default=Path(os.environ.get("RELEASE_BENCH_WORK_ROOT")
                                                                or tempfile.gettempdir()) / "aether-release-bench",
                        help="where task workspaces live; never inside this repository")


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run", help="benchmark old and candidate binaries on the frozen set")
    run.add_argument("--set", type=Path, default=DEFAULT_SET)
    run.add_argument("--old", type=Path, required=True, help="aether binary of the last release")
    run.add_argument("--new", type=Path, required=True, help="aether binary of the release candidate")
    run.add_argument("--arms", default="all", help='"all", a model tier, or keys such as haiku/native,luna/new')
    run.add_argument("--archon-bin", type=Path, help="archon executable; adds an Archon arm per model tier")
    run.add_argument("--allow-off-target", action="store_true",
                     help="run a set whose calibration missed its acceptance targets (never a release verdict)")
    run.add_argument("--limit", type=int, help="only the first N tasks of the set (smoke runs)")
    run.add_argument("--early-stop", action="store_true", help="stop at an interim look on a decisive FAIL")
    run.add_argument("--accuracy-margin", type=float, default=GATE_DEFAULTS["accuracy_margin"])
    run.add_argument("--cost-margin", type=float, default=GATE_DEFAULTS["cost_margin"])
    run.add_argument("--confidence", type=float, default=GATE_DEFAULTS["confidence"])
    _pool_options(run, budget=20.0)
    calibrate = commands.add_parser("calibrate", help="run generated candidates on the native arms")
    calibrate.add_argument("--count", type=int, default=200, help="candidates to generate")
    calibrate.add_argument("--first-seed", type=int, default=0)
    calibrate.add_argument("--templates", help="comma-separated templates (default: all)")
    calibrate.add_argument("--price-binary", type=Path, required=True, help="aether binary whose price table is used")
    calibrate.add_argument("--freeze", type=Path, help="write the chosen set here (e.g. bench/release-set/v3)")
    calibrate.add_argument("--min-size", type=int, default=SELECTION_DEFAULTS["min_size"])
    calibrate.add_argument("--max-size", type=int, default=SELECTION_DEFAULTS["max_size"])
    calibrate.add_argument("--accept-off-target", action="store_true",
                           help="freeze even when the set misses its targets, recorded as not accepted")
    calibrate.add_argument("--carry", type=Path, help="a frozen set whose tasks join the pool with their recorded "
                           "calibration rates; its tasks of --templates are left out")
    calibrate.add_argument("--haiku-band", type=_band, default=SELECTION_DEFAULTS["haiku_band"],
                           help="eligible native Haiku pass rates, LOW,HIGH (default 0.2,0.8)")
    _pool_options(calibrate, budget=25.0)
    calibrate.set_defaults(repeats=4)
    report = commands.add_parser("report", help="re-render the report of a finished run")
    report.add_argument("run_dir", type=Path)
    args = parser.parse_args(argv)
    if getattr(args, "jobs", 1) < 1 or getattr(args, "repeats", 1) < 1:
        parser.error("--jobs and --repeats must be at least 1")
    return args


def _band(text):
    try:
        low, high = (float(part) for part in text.split(","))
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"expected LOW,HIGH, got {text!r}") from error
    if not 0 <= low < high <= 1:
        raise argparse.ArgumentTypeError(f"a band needs 0 <= LOW < HIGH <= 1, got {text!r}")
    return (low, high)


def _run_dir(kind, run_id):
    run_id = run_id or time.strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:6]
    path = SCRATCH / kind / run_id
    path.mkdir(parents=True, exist_ok=True)
    return path


def _work_dir(work_root, run_dir):
    """The run's workspace root, outside the repository so no solver can find the task generators."""
    path = (work_root / run_dir.parent.name / run_dir.name).resolve()
    if path == REPO_ROOT or REPO_ROOT in path.parents:
        raise SystemExit(f"--work-root {work_root} is inside the repository, where the reference solutions live")
    path.mkdir(parents=True, exist_ok=True)
    assert path.is_dir() and not any(parent == REPO_ROOT for parent in path.parents), "workspaces stay outside"
    return str(path)


def _config_check(run_dir, config):
    """Pin a run's configuration; resuming with a different one is refused, not silently mixed."""
    path = run_dir / "config.json"
    if path.exists():
        previous = json.loads(path.read_text())
        if previous != json.loads(json.dumps(config, default=str)):
            raise SystemExit(f"{run_dir} was started with a different configuration; use a new --run-id")
        return
    save_json(str(path), config)


def _price_manifest(binary, sha, run_dir):
    table_path = run_dir / "prices.json"
    if table_path.exists():
        table = json.loads(table_path.read_text())
    else:
        table = fetch_price_table(binary, sha, str(run_dir / "price-scratch"))
        save_json(str(table_path), table)
    assert table["binary_sha256"] == sha, "the price table belongs to the pinned binary"
    return {"price_table": table, "native": {"codex": {"configured_defaults": native_configured_defaults("codex")}}}


def _pin(binary, run_dir, role):
    binary = binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"--{role} binary not found: {binary}")
    return pin_binary(str(binary), str(run_dir / role))


def arms_by_task(specs, arm_keys):
    """{(task, arm key): arm}; each arm installs the agent cap its budget reservation counts."""
    arms = {}
    for spec in specs:
        delegation = bool(spec["meta"].get("allow_delegation"))
        for key in arm_keys:
            model, harness = key.split("/")
            arms[(spec["id"], key)] = build_arm(model, harness, allow_delegation=delegation)
    assert len(arms) == len(specs) * len(arm_keys), "every (task, arm) has one arm"
    return arms


def execute(specs, arm_keys, args, run_dir, context, should_stop=None):
    """Schedule, run and collect every (task, repeat, arm) of a run; returns (records, stop reason)."""
    by_id = {spec["id"]: spec for spec in specs}
    assert len(by_id) == len(specs), "task ids are unique"
    table = context["price_manifest"]["price_table"]
    arms = arms_by_task(specs, arm_keys)

    def run_one(triple):
        task, repeat, key = triple
        return run_release_trial(by_id[task], arms[(task, key)], repeat, context)

    def reservation_of(triple):
        task, _, key = triple
        arm = arms[(task, key)]
        return reservation_usd(table, arm["model"], arm["max_agents"])

    order = schedule(sorted(by_id), arm_keys, args.repeats, args.seed)
    gates = Gates(args.load_max, args.mem_min_gb, args.budget)
    return run_pool(order, run_one, reservation_of, jobs=args.jobs, gates=gates,
                    results_path=str(run_dir / "results.jsonl"), attempts_path=str(run_dir / "attempts.jsonl"),
                    log=log, should_stop=should_stop)


def _header(config, records, stop_reason, started):
    spent = sum(item["cost_usd"] or 0 for item in records)
    accepted = "" if config["set"].get("accepted") else " **(off target: not a release verdict)**"
    lines = [f"- Task set: `{config['set']['path']}` ({config['set']['tasks']} tasks, "
             f"sha256 `{config['set']['sha256'][:16]}`){accepted}",
             f"- Old: `{config['binaries']['old']['sha256'][:16]}`, candidate: "
             f"`{config['binaries']['new']['sha256'][:16]}`" + (" (same binary: an A/A run)"
                                                                 if config["binaries"]["old"]["sha256"]
                                                                 == config["binaries"]["new"]["sha256"] else ""),
             f"- Native CLIs: claude {config['native']['claude'].get('version')}, "
             f"codex {config['native']['codex'].get('version')}",
             f"- Arms: {', '.join(config['arms'])}; repeats {config['repeats']}; jobs {config['jobs']}; "
             f"timeout {config['timeout']}s",
             f"- Trials: {len(records)}; priced spend ${spent:.2f} API-equivalent"]
    if started is not None:
        lines.append(f"- Wall time this invocation: {(time.monotonic() - started) / 60:.1f} min")
    if stop_reason:
        lines.append(f"- Stopped early: {stop_reason}")
    return lines


def _gate_settings(args):
    """Gate settings; with early stopping every look, the final one included, gets a share of the error rate."""
    confidence = sequential_confidence(args.confidence) if args.early_stop else args.confidence
    return {"accuracy_margin": args.accuracy_margin, "cost_margin": args.cost_margin, "confidence": confidence,
            "sequential": bool(args.early_stop)}


def _accepted(manifest, args):
    """Whether the set met its calibration targets; an off-target set runs only when asked to."""
    acceptance = manifest.get("acceptance") or {}
    if acceptance.get("accepted") is True:
        return True
    reasons = "; ".join(acceptance.get("reasons") or ["its manifest records no acceptance"])
    if not args.allow_off_target:
        raise SystemExit(f"{args.set} missed its calibration targets ({reasons}); "
                         "pass --allow-off-target (ALLOW_OFF_TARGET=1) to run it anyway")
    log(f"running an off-target set: {reasons}")
    return False


def command_run(args):
    manifest, specs = load_set(args.set)
    accepted = _accepted(manifest, args)
    specs = sorted(specs, key=lambda spec: spec["id"])[:args.limit] if args.limit else specs
    arm_keys = parse_arms(args.arms, archon=args.archon_bin is not None)
    run_dir = _run_dir("runs", args.run_id)
    old, old_sha = _pin(args.old, run_dir, "old")
    new, new_sha = _pin(args.new, run_dir, "new")
    native = {name: native_cli_provenance(name) for name in ("claude", "codex")}
    config = {"set": {"path": str(args.set), "sha256": manifest["set_sha256"], "tasks": len(specs),
                      "accepted": accepted, "task_ids": sorted(spec["id"] for spec in specs)},
              "binaries": {"old": {"sha256": old_sha}, "new": {"sha256": new_sha}},
              "native": {name: {"version": info["version"]} for name, info in native.items()},
              "arms": arm_keys, "repeats": args.repeats, "jobs": args.jobs, "timeout": args.timeout,
              "seed": args.seed, "gate": _gate_settings(args), "archon": bool(args.archon_bin)}
    _config_check(run_dir, config)
    log(f"run directory {run_dir}")
    context = {"run_dir": str(run_dir), "work_dir": _work_dir(args.work_root, run_dir),
               "binaries": {"old": old, "new": new}, "timeout": args.timeout,
               "real_claude": shutil.which("claude"), "archon_bin": str(args.archon_bin or ""),
               "price_manifest": _price_manifest(new, new_sha, run_dir)}
    stop = _early_stop(args, len(specs)) if args.early_stop else None
    started = time.monotonic()
    records, stop_reason = execute(specs, arm_keys, args, run_dir, context, stop)
    return _write_report(run_dir, config, records, stop_reason, started)


def _early_stop(args, planned_tasks):
    """A should_stop callback that evaluates each interim look once, as the error split assumes."""
    settings, seen = _gate_settings(args), {model: 0 for model in MODELS}

    def should_stop(records):
        for model in MODELS:
            old_arm, new_arm = f"{model}/old", f"{model}/new"
            reached = reached_looks(records, old_arm, new_arm, planned_tasks)
            if reached <= seen[model]:
                continue
            seen[model] = reached
            verdict = interim_fail(records, old_arm, new_arm, settings)
            if verdict is not None:
                return f"{model}: decisive FAIL at interim look {reached} ({'; '.join(verdict['reasons'])})"
        return None
    return should_stop


def _write_report(run_dir, config, records, stop_reason, started=None):
    if not records:
        raise SystemExit("no trial finished; nothing to report")
    expected = {"arms": config["arms"], "tasks": config["set"]["task_ids"], "repeats": config["repeats"]}
    text, overall, verdicts, gaps = render(records, _header(config, records, stop_reason, started), MODELS,
                                           config["gate"], expected)
    if overall == "PASS" and not config["set"]["accepted"]:
        overall = "INCONCLUSIVE"
        text = text.replace("# Release bench: PASS",
                            "# Release bench: INCONCLUSIVE (gate passed on an off-target set)", 1)
    (run_dir / "report.md").write_text(text)
    save_json(str(run_dir / "verdict.json"), {"overall": overall, "stop_reason": stop_reason, "incomplete": gaps,
                                               "models": {model: {key: verdict[key] for key in
                                                                  ("verdict", "reasons", "warnings")}
                                                          for model, verdict in verdicts.items()}})
    print(text)
    log(f"report written to {run_dir / 'report.md'}")
    return 0 if overall == "PASS" else 1


def command_report(args):
    run_dir = args.run_dir.resolve()
    config = json.loads((run_dir / "config.json").read_text())
    if "binaries" not in config:
        templates = config["candidates"]["templates"]
        carry_rates = carried(Path(config["carry"]), set(templates.split(",")) if templates else None)[1] \
            if config.get("carry") else None
        band = tuple(config.get("haiku_band", SELECTION_DEFAULTS["haiku_band"]))
        records = read_results(str(run_dir / "results.jsonl"))
        return _write_calibration(run_dir, config, records, {**SELECTION_DEFAULTS, "haiku_band": band}, None, False,
                                  carry_rates)
    stop = json.loads((run_dir / "verdict.json").read_text()).get("stop_reason") if (
        run_dir / "verdict.json").exists() else None
    return _write_report(run_dir, config, read_results(str(run_dir / "results.jsonl")), stop)


def _verified_candidates(args):
    templates = args.templates.split(",") if args.templates else None
    specs = families.candidates(args.count, args.first_seed, templates)
    rejected = [(spec["id"], result["problems"]) for spec in specs for result in [verify(spec)] if not result["ok"]]
    if rejected:
        raise SystemExit(f"candidates failed verification: {rejected[:5]}")
    assert len({spec["id"] for spec in specs}) == len(specs), "candidate ids are unique"
    return specs


def carried(path, recalibrated):
    """(specs, rates) of a frozen set's tasks, rates as recorded at its calibration, minus `recalibrated` templates."""
    manifest, specs = load_set(path)
    kept = [spec for spec in specs if not recalibrated or spec["template"] not in recalibrated]
    missing = [spec["id"] for spec in kept if spec["id"] not in manifest.get("calibration", {})]
    if missing:
        raise SystemExit(f"{path} records no calibration rates for {missing[:5]}")
    rates = {spec["id"]: {"template": spec["template"], "family": spec["family"], "infra": 0, "cost": 0.0,
                          "carried": True, **manifest["calibration"][spec["id"]]} for spec in kept}
    assert all(0 <= row["haiku"] <= 1 and 0 <= row["luna"] <= 1 for row in rates.values()), "rates are fractions"
    return kept, rates


def command_calibrate(args):
    specs = _verified_candidates(args)
    log(f"{len(specs)} candidates verified")
    templates = set(args.templates.split(",")) if args.templates else None
    carry_specs, carry_rates = carried(args.carry, templates) if args.carry else ([], {})
    if {spec["id"] for spec in specs} & set(carry_rates):
        raise SystemExit("a carried task is also a candidate; recalibrate its template with --templates")
    run_dir = _run_dir("calibration", args.run_id)
    binary, sha = _pin(args.price_binary, run_dir, "pricing")
    arm_keys = ["haiku/native", "luna/native"]
    config = {"candidates": {"count": args.count, "first_seed": args.first_seed, "templates": args.templates},
              "arms": arm_keys, "repeats": args.repeats, "jobs": args.jobs, "timeout": args.timeout,
              "seed": args.seed, "price_binary_sha256": sha, "haiku_band": list(args.haiku_band),
              "carry": str(args.carry) if args.carry else None}
    _config_check(run_dir, config)
    log(f"calibration directory {run_dir}")
    context = {"run_dir": str(run_dir), "work_dir": _work_dir(args.work_root, run_dir), "binaries": {},
               "timeout": args.timeout, "real_claude": None,
               "archon_bin": "", "price_manifest": _price_manifest(binary, sha, run_dir)}
    records, stop_reason = execute(specs, arm_keys, args, run_dir, context)
    if stop_reason:
        log(f"calibration incomplete: {stop_reason}")
    settings = {**SELECTION_DEFAULTS, "min_size": args.min_size, "max_size": args.max_size,
                "haiku_band": args.haiku_band}
    return _write_calibration(run_dir, config, records, settings,
                              args.freeze and (args.freeze, specs + carry_specs), args.accept_off_target, carry_rates)


def _write_calibration(run_dir, config, records, settings, freeze, accept_off_target, carry_rates=None):
    """calibration.md from the records (re-selecting is free on a resumed run), then the frozen set if asked.

    `carry_rates` are tasks measured by an earlier calibration. A set that misses its
    targets is frozen only with `accept_off_target`, and its manifest says so."""
    assert records, "a calibration needs at least one trial"
    assert settings["min_size"] <= settings["max_size"], "the set's size bounds are ordered"
    measured = task_rates(records)
    rates = {**(carry_rates or {}), **measured}
    assert len(rates) == len(measured) + len(carry_rates or {}), "a carried task is never measured again"
    chosen = select(rates, settings)
    spent = sum(item["cost_usd"] or 0 for item in records)
    header = [f"- Candidates: {len(measured)}; trials: {len(records)}; arms: {', '.join(config['arms'])}; "
              f"repeats: {config['repeats']}", f"- Priced spend: ${spent:.2f} API-equivalent"]
    if carry_rates:
        header.append(f"- Carried from `{config['carry']}` with the rates its calibration recorded: "
                      f"{len(carry_rates)} tasks")
    text = render_calibration(rates, chosen, settings, header)
    (run_dir / "calibration.md").write_text(text)
    print(text)
    if not freeze:
        return 0
    directory, specs = freeze
    verdict = acceptance(rates, chosen, settings)
    if not verdict["accepted"] and not (accept_off_target and chosen):
        raise SystemExit(f"the set misses its targets ({'; '.join(verdict['reasons'])}); not freezing "
                         "(--accept-off-target freezes it marked as not accepted)")
    chosen_specs = [spec for spec in specs if spec["id"] in set(chosen)]
    keys = ("haiku", "luna", "haiku_n", "luna_n", "haiku_seconds", "luna_seconds")
    stats = {task: {key: rates[task].get(key) for key in keys} for task in chosen}
    write_set(directory, chosen_specs, {"version": directory.name, "calibration": stats, "acceptance": verdict,
                                        "calibration_config": config, "selection": settings})
    shutil.copy(run_dir / "calibration.md", directory / "calibration.md")
    log(f"froze {len(chosen_specs)} tasks into {directory}")
    return 0


def main(argv=None):
    if sys.version_info < (3, 11):
        raise SystemExit(f"the release bench needs Python 3.11 or newer, not {sys.version.split()[0]}")
    args = parse_args(sys.argv[1:] if argv is None else argv)
    handlers = {"run": command_run, "calibrate": command_calibrate, "report": command_report}
    return handlers[args.command](args)


if __name__ == "__main__":
    raise SystemExit(main())
