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
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import release_bench_families as families
from harness_bench import fetch_price_table, save_json
from harness_bench_assets import native_cli_provenance, pin_binary, sha256_file
from harness_bench_metadata import native_configured_defaults
from release_bench_arms import ARCHON_DEFAULTS, ARCHON_REVIEWS, build_arm, is_release_label, parse_arms
from release_bench_archon import PIPELINE_STAGES
from release_bench_calibrate import CALIBRATION_HARNESSES, SELECTION_DEFAULTS, acceptance, select, task_rates
from release_bench_calibrate import render as render_calibration
from release_bench_generate import verify
from release_bench_pool import Gates, read_results, run_pool, schedule
from release_bench_host import default_jobs, default_load_max, read_invocations, record_invocation
from release_bench_outcomes import digest_document
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
    jobs, load_max = default_jobs(), default_load_max()
    parser.add_argument("--jobs", type=int, default=jobs, help=f"trials at once (default {jobs} on this host)")
    parser.add_argument("--load-max", type=float, default=load_max,
                        help=f"start no trial above this 1-minute load (default {load_max:g} on this host)")
    parser.add_argument("--mem-min-gb", type=float, default=2.0, help="start no trial below this free memory")
    parser.add_argument("--budget", type=float, default=budget, help="stop admitting trials at this spend (USD)")
    parser.add_argument("--timeout", type=int, help=f"seconds per trial (default: the set's own trial timeout, "
                        f"else {TRIAL_TIMEOUT_SECONDS})")
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
    run.add_argument("--release", type=_release, action="append", default=[], metavar="TAG=PATH",
                     help="an extra release binary, such as v1.0.0=<aether 1.0.0>, run by arms like haiku/v1.0.0")
    run.add_argument("--archon-old", type=Path, help="archon of the last Archon release; adds <model>/archon-old "
                     "arms, run over the --old aether")
    run.add_argument("--archon-new", type=Path, help="archon release candidate; adds <model>/archon-new arms, "
                     "run over the --new aether")
    run.add_argument("--archon-bin", type=Path, help="an alias of --archon-new")
    _archon_flags(run)
    run.add_argument("--allow-off-target", action="store_true",
                     help="run a set whose calibration missed its acceptance targets (never a release verdict)")
    run.add_argument("--limit", type=int, help="only the first N tasks of the set (smoke runs)")
    run.add_argument("--early-stop", action="store_true", help="stop at an interim look on a decisive FAIL")
    run.add_argument("--accuracy-margin", type=float, default=GATE_DEFAULTS["accuracy_margin"])
    run.add_argument("--cost-margin", type=float, default=GATE_DEFAULTS["cost_margin"])
    run.add_argument("--confidence", type=float, default=GATE_DEFAULTS["confidence"])
    _pool_options(run, budget=20.0)
    _calibrate_parser(commands)
    report = commands.add_parser("report", help="re-render the report of a finished run")
    report.add_argument("run_dir", type=Path)
    args = parser.parse_args(argv)
    if getattr(args, "jobs", 1) < 1 or getattr(args, "repeats", 1) < 1:
        parser.error("--jobs and --repeats must be at least 1")
    if args.command == "run":
        _merge_archon_alias(parser, args)
    if args.command == "calibrate":
        _check_calibration_arms(parser, args)
    return args


def _archon_flags(parser):
    parser.add_argument("--archon-pipeline", choices=sorted(PIPELINE_STAGES), default=ARCHON_DEFAULTS["pipeline"])
    parser.add_argument("--archon-review", choices=ARCHON_REVIEWS, default=ARCHON_DEFAULTS["review"],
                        help="Archon's review family for the Archon arms (default %(default)s; none is an ablation)")
    parser.add_argument("--archon-budget-factor", type=float, default=ARCHON_DEFAULTS["budget_factor"],
                        help="an Archon trial's allowance as a multiple of its tier's (default %(default)s)")


def _calibrate_parser(commands):
    calibrate = commands.add_parser("calibrate", help="run generated candidates on the calibration arms")
    calibrate.add_argument("--count", type=int, default=200, help="candidates to generate")
    calibrate.add_argument("--first-seed", type=int, default=0)
    calibrate.add_argument("--templates", help="comma-separated templates (default: all)")
    calibrate.add_argument("--price-binary", type=Path, required=True,
                           help="aether binary whose price table is used, and which the new and archon-new arms run")
    calibrate.add_argument("--arms", default="haiku/native,luna/native",
                           help="native, new and archon-new arms to calibrate on (default %(default)s)")
    calibrate.add_argument("--archon-new", type=Path, help="archon the archon-new arms run")
    _archon_flags(calibrate)
    calibrate.add_argument("--freeze", type=Path, help="write the chosen set here (e.g. bench/release-set/v3)")
    calibrate.add_argument("--accept-off-target", action="store_true",
                           help="freeze even when the set misses its targets, recorded as not accepted")
    calibrate.add_argument("--carry", type=Path, help="a frozen set whose tasks join the pool with their recorded "
                           "calibration rates; its tasks of --templates are left out")
    _selection_flags(calibrate)
    _pool_options(calibrate, budget=25.0)
    calibrate.set_defaults(repeats=4)


def _selection_flags(parser):
    defaults = SELECTION_DEFAULTS
    for name in ("min_size", "max_size", "min_repeats", "release_jobs", "max_release_minutes"):
        parser.add_argument("--" + name.replace("_", "-"), type=int, default=defaults[name])
    for name in ("haiku_target", "luna_target", "haiku_tolerance", "max_template_share"):
        parser.add_argument("--" + name.replace("_", "-"), type=float, default=defaults[name])
    parser.add_argument("--haiku-band", type=_band, default=defaults["haiku_band"],
                        help="eligible per-task Haiku pass rates on --select-harness, LOW,HIGH (default 0.2,0.8)")
    parser.add_argument("--luna-band", type=_band, default=defaults["luna_band"],
                        help="the accepted mean Luna pass rate, LOW,HIGH (default 0.25,0.35)")
    parser.add_argument("--select-harness", choices=CALIBRATION_HARNESSES, default=defaults["select_harness"],
                        help="the harness whose pass rates select and accept the set (default %(default)s)")
    parser.add_argument("--release-arms", choices=("aether", "shared"), default=defaults["release_arms"],
                        help="the release matrix the time target counts: aether (native, old, new) or shared "
                        "(those and archon-old, archon-new)")


def _check_calibration_arms(parser, args):
    try:
        args.arm_keys = parse_arms(args.arms, ("new",) if args.archon_new else ())
    except ValueError as error:
        parser.error(str(error))
    harnesses = {key.split("/")[1] for key in args.arm_keys}
    if not harnesses <= set(CALIBRATION_HARNESSES):
        parser.error(f"calibration arms run {', '.join(CALIBRATION_HARNESSES)} only, not {sorted(harnesses)}")
    if args.select_harness not in harnesses:
        parser.error(f"--select-harness {args.select_harness} has no arm in --arms")


def _merge_archon_alias(parser, args):
    """--archon-bin is --archon-new under its old name; both at once must agree."""
    if args.archon_bin is not None:
        if args.archon_new is not None and args.archon_new != args.archon_bin:
            parser.error("--archon-bin is an alias of --archon-new; give one of them")
        args.archon_new = args.archon_bin
    if args.archon_budget_factor < 1:
        parser.error("--archon-budget-factor must be at least 1")
    assert args.archon_bin is None or args.archon_new == args.archon_bin, "the alias names the candidate"


def archon_sides(args):
    """The Archon sides ("old", "new") the run was given a binary for."""
    return tuple(side for side in ("old", "new") if getattr(args, f"archon_{side}", None) is not None)


def selection_settings(args):
    """The calibration's selection and acceptance settings, from its flags."""
    keys = ("min_size", "max_size", "min_repeats", "release_jobs", "max_release_minutes", "haiku_target",
            "luna_target", "haiku_tolerance", "max_template_share", "haiku_band", "luna_band", "select_harness",
            "release_arms")
    settings = {**SELECTION_DEFAULTS, **{key: getattr(args, key) for key in keys}}
    assert settings["min_size"] <= settings["max_size"], "the set's size bounds are ordered"
    if not 0 < settings["max_template_share"] <= 1:
        raise SystemExit("--max-template-share is a fraction in (0, 1]")
    return settings


def archon_options(args):
    return {"pipeline": args.archon_pipeline, "review": args.archon_review,
            "budget_factor": args.archon_budget_factor}


def _band(text):
    try:
        low, high = (float(part) for part in text.split(","))
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"expected LOW,HIGH, got {text!r}") from error
    if not 0 <= low < high <= 1:
        raise argparse.ArgumentTypeError(f"a band needs 0 <= LOW < HIGH <= 1, got {text!r}")
    return (low, high)


def _release(text):
    label, _, path = text.partition("=")
    if not is_release_label(label) or not path:
        raise argparse.ArgumentTypeError(f"expected TAG=PATH with a tag such as v1.0.0, got {text!r}")
    return label, Path(path)


def _releases(args, arm_keys, run_dir):
    """{tag: (pinned binary, sha256)} of the extra release binaries: one per release arm, and an arm per binary."""
    given = dict(args.release)
    if len(given) != len(args.release):
        raise SystemExit("a release tag was given twice")
    wanted = {key.split("/")[1] for key in arm_keys if is_release_label(key.split("/")[1])}
    if wanted != set(given):
        raise SystemExit(f"release arms {sorted(wanted)} and --release tags {sorted(given)} differ")
    pinned = {label: _pin(path, run_dir, label) for label, path in sorted(given.items())}
    assert all(sha for _, sha in pinned.values()), "every release binary is pinned by its hash"
    return pinned


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


def _config_check(run_dir, config, reselect=()):
    """Pin a run's configuration; resuming with a different one is refused, not silently mixed.

    Keys in `reselect` only choose among finished trials, so a resume may change them, and the
    configuration then records the latest."""
    path = run_dir / "config.json"
    if path.exists():
        previous = json.loads(path.read_text())
        current = json.loads(json.dumps(config, default=str))
        if {key: value for key, value in previous.items() if key not in reselect} != \
                {key: value for key, value in current.items() if key not in reselect}:
            raise SystemExit(f"{run_dir} was started with a different configuration; use a new --run-id")
        if previous == current:
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


def arms_by_task(specs, arm_keys, options=None):
    """{(task, arm key): arm}; each arm installs the agent cap its budget reservation counts."""
    arms = {}
    for spec in specs:
        delegation = bool(spec["meta"].get("allow_delegation"))
        for key in arm_keys:
            model, harness = key.split("/")
            arms[(spec["id"], key)] = build_arm(model, harness, allow_delegation=delegation,
                                                archon_options=options)
    assert len(arms) == len(specs) * len(arm_keys), "every (task, arm) has one arm"
    return arms


def execute(specs, arm_keys, args, run_dir, context, should_stop=None):
    """Schedule, run and collect every (task, repeat, arm) of a run; returns (records, stop reason)."""
    by_id = {spec["id"]: spec for spec in specs}
    assert len(by_id) == len(specs), "task ids are unique"
    table = context["price_manifest"]["price_table"]
    arms = arms_by_task(specs, arm_keys, context.get("archon_options"))

    def run_one(triple):
        task, repeat, key = triple
        return run_release_trial(by_id[task], arms[(task, key)], repeat, context)

    def reservation_of(triple):
        task, _, key = triple
        arm = arms[(task, key)]
        return reservation_usd(table, arm["model"], arm["max_agents"], arm["allowance_usd"])

    order = schedule(sorted(by_id), arm_keys, args.repeats, args.seed)
    gates = Gates(args.load_max, args.mem_min_gb, args.budget)
    return run_pool(order, run_one, reservation_of, jobs=args.jobs, gates=gates,
                    results_path=str(run_dir / "results.jsonl"), attempts_path=str(run_dir / "attempts.jsonl"),
                    log=log, should_stop=should_stop)


def _header(config, records, stop_reason, started, invocations=()):
    spent = sum(item["cost_usd"] or 0 for item in records)
    accepted = "" if config["set"].get("accepted") else " **(off target: not a release verdict)**"
    lines = [f"- Task set: `{config['set']['path']}` ({config['set']['tasks']} tasks, "
             f"sha256 `{config['set']['sha256'][:16]}`){accepted}",
             f"- Old: `{config['binaries']['old']['sha256'][:16]}`, candidate: "
             f"`{config['binaries']['new']['sha256'][:16]}`" + (" (same binary: an A/A run)"
                                                                 if config["binaries"]["old"]["sha256"]
                                                                 == config["binaries"]["new"]["sha256"] else ""),
             *[f"- Release {label}: `{info['sha256'][:16]}`" for label, info in config.get("releases", {}).items()],
             *_archon_header(config),
             f"- Native CLIs: claude {config['native']['claude'].get('version')}, "
             f"codex {config['native']['codex'].get('version')}",
             f"- Arms: {', '.join(config['arms'])}; repeats {config['repeats']}; jobs {config['jobs']}; "
             f"timeout {config['timeout']}s",
             f"- Trials: {len(records)}; priced spend ${spent:.2f} API-equivalent"]
    if config.get("versions"):
        lines.append("- Versions: " + ", ".join(f"{role} `{text}`" for role, text in config["versions"].items()))
    lines += _speed_lines(records, invocations)
    if started is not None:
        lines.append(f"- Wall time this invocation: {(time.monotonic() - started) / 60:.1f} min")
    if stop_reason:
        lines.append(f"- Stopped early: {stop_reason}")
    return lines


def _archon_header(config):
    archon = config.get("archon")
    if not isinstance(archon, dict) or not archon:
        return []
    options = config.get("archon_options") or {}
    sides = ", ".join(f"{side} `{info['sha256'][:16]}`" for side, info in archon.items())
    return [f"- Archon: {sides}; pipeline `{options.get('pipeline')}`, review `{options.get('review')}`, "
            f"allowance x{options.get('budget_factor')} of its tier's"]


def _speed_lines(records, invocations):
    """Parallel speed-up: the trials' summed solve time over the wall time of the invocations that ran them."""
    wall = sum(item["wall_seconds"] for item in invocations)
    ran = sum(item["trials_run"] for item in invocations)
    if not invocations or wall <= 0 or ran != len(records):
        return []
    serial = sum(item["seconds"] or 0 for item in records)
    jobs = sorted({item["jobs"] for item in invocations})
    return [f"- Parallelism: {serial / 60:.1f} trial-minutes of solving in {wall / 60:.1f} wall minutes, "
            f"{serial / wall:.2f}x a serial run's solve time, on {'/'.join(map(str, jobs))} job(s)"]


def load_control(config):
    """How the run kept wall times comparable across arms, for the report."""
    jobs = config["jobs"]
    together = (f"so with {jobs} workers they run side by side under the same load" if jobs > 1
                else "and with 1 worker they run one at a time")
    return (f"Load control: the arms of one (task, repeat) run as one adjacent block, {together}; old/new and "
            f"archon-old/archon-new alternate which starts first per block; no trial starts above a 1-minute "
            f"load of {config.get('load_max', '?')} or below the free-memory floor.")


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


def binary_version(path):
    """The first line `<binary> --version` prints, or a note why there is none."""
    try:
        done = subprocess.run([str(path), "--version"], capture_output=True, text=True, timeout=20, check=False,
                              stdin=subprocess.DEVNULL)
    except (OSError, subprocess.TimeoutExpired) as error:
        return f"unknown ({type(error).__name__})"
    lines = (done.stdout or done.stderr).strip().splitlines()
    return lines[0].strip() if done.returncode == 0 and lines else f"unknown (exit {done.returncode})"


def _pin_archon(binary, run_dir, side):
    """Copy an archon binary into the run, so a reinstall mid-run cannot change trials: (path, sha256)."""
    binary = binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"--archon-{side} binary not found: {binary}")
    target = run_dir / f"archon-{side}" / "bin" / "archon"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, target)
    target.chmod(0o755)
    sha = sha256_file(str(target))
    assert sha and target.is_file(), "the pinned archon exists and is hashed"
    return target, sha


def _archon_bins(args, run_dir):
    """{side: (pinned archon, sha256)} of the Archon sides the run was given."""
    return {side: _pin_archon(getattr(args, f"archon_{side}"), run_dir, side) for side in archon_sides(args)}


def _versions(binaries, archons):
    """What each pinned binary says it is, recorded in the run's configuration and its report."""
    versions = {f"aether-{role}": binary_version(path) for role, path in binaries.items()}
    versions.update({f"archon-{side}": binary_version(path) for side, path in archons.items()})
    return versions


def command_run(args):
    manifest, specs = load_set(args.set)
    accepted = _accepted(manifest, args)
    args.timeout = trial_timeout(args.timeout, manifest)
    specs = sorted(specs, key=lambda spec: spec["id"])[:args.limit] if args.limit else specs
    try:
        arm_keys = parse_arms(args.arms, archon_sides(args))
    except ValueError as error:
        raise SystemExit(str(error)) from error
    run_dir = _run_dir("runs", args.run_id)
    old, old_sha = _pin(args.old, run_dir, "old")
    new, new_sha = _pin(args.new, run_dir, "new")
    releases = _releases(args, arm_keys, run_dir)
    archons = _archon_bins(args, run_dir)
    native = {name: native_cli_provenance(name) for name in ("claude", "codex")}
    binaries = {"old": old, "new": new, **{label: path for label, (path, _) in releases.items()}}
    config = {"set": {"path": str(args.set), "sha256": manifest["set_sha256"], "tasks": len(specs),
                      "accepted": accepted, "task_ids": sorted(spec["id"] for spec in specs)},
              "binaries": {"old": {"sha256": old_sha}, "new": {"sha256": new_sha}},
              "native": {name: {"version": info["version"]} for name, info in native.items()},
              "arms": arm_keys, "repeats": args.repeats, "jobs": args.jobs, "timeout": args.timeout,
              "load_max": args.load_max, "seed": args.seed, "gate": _gate_settings(args),
              "archon": {side: {"sha256": sha} for side, (_, sha) in archons.items()},
              "archon_options": archon_options(args) if archons else None,
              "versions": _versions({"old": old, "new": new}, {side: path for side, (path, _) in archons.items()})}
    if releases:
        config["releases"] = {label: {"sha256": sha} for label, (_, sha) in releases.items()}
    _config_check(run_dir, config)
    log(f"run directory {run_dir}")
    context = {"run_dir": str(run_dir), "work_dir": _work_dir(args.work_root, run_dir), "binaries": binaries,
               "timeout": args.timeout, "real_claude": shutil.which("claude"),
               "archon_bins": {side: str(path) for side, (path, _) in archons.items()},
               "archon_options": archon_options(args), "price_manifest": _price_manifest(new, new_sha, run_dir)}
    stop = _early_stop(args, len(specs)) if args.early_stop else None
    started = time.monotonic()
    before = len(read_results(str(run_dir / "results.jsonl")))
    records, stop_reason = execute(specs, arm_keys, args, run_dir, context, stop)
    record_invocation(str(run_dir), started, args.jobs, len(records) - before)
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
    header = _header(config, records, stop_reason, started, read_invocations(str(run_dir)))
    report = render(records, header, MODELS, config["gate"], expected, load_control=load_control(config))
    text, overall = report["text"], report["overall"]
    if overall == "PASS" and not config["set"]["accepted"]:
        overall = "INCONCLUSIVE"
        text = text.replace("# Release bench: PASS",
                            "# Release bench: INCONCLUSIVE (gate passed on an off-target set)", 1)
    (run_dir / "report.md").write_text(text)
    (run_dir / "failure-digest.md").write_text(digest_document(report["digest"], _digest_provenance(config)))
    save_json(str(run_dir / "verdict.json"), _verdict_json(report, overall, stop_reason))
    print(text)
    log(f"report written to {run_dir / 'report.md'}")
    return 0 if overall == "PASS" else 1


def _digest_provenance(config):
    """The digest's header: versions and arms only, never the set's path or tasks."""
    lines = [f"- Arms: {', '.join(config['arms'])}; repeats {config['repeats']}"]
    if config.get("versions"):
        lines.append("- Versions: " + ", ".join(f"{role} `{text}`" for role, text in config["versions"].items()))
    return lines


def _verdict_json(report, overall, stop_reason):
    def tiers(verdicts):
        return {model: {key: verdict[key] for key in ("verdict", "reasons", "warnings")}
                for model, verdict in verdicts.items()}
    return {"overall": overall, "products": report["products"], "stop_reason": stop_reason,
            "incomplete": report["gaps"], "models": tiers(report["verdicts"]),
            "archon_models": tiers(report["archon_verdicts"]),
            "parity": {model: {key: result[key] for key in ("status", "new", "archon_new", "pairs", "behind")}
                       for model, result in report["parity"].items()}}


def command_report(args):
    run_dir = args.run_dir.resolve()
    config = json.loads((run_dir / "config.json").read_text())
    if "binaries" not in config:
        templates = config["candidates"]["templates"]
        carry_rates = carried(Path(config["carry"]), set(templates.split(",")) if templates else None)[1] \
            if config.get("carry") else None
        records = read_results(str(run_dir / "results.jsonl"))
        return _write_calibration(run_dir, config, records, recorded_selection(config), None, False, carry_rates)
    stop = json.loads((run_dir / "verdict.json").read_text()).get("stop_reason") if (
        run_dir / "verdict.json").exists() else None
    return _write_report(run_dir, config, read_results(str(run_dir / "results.jsonl")), stop)


def recorded_selection(config):
    """The selection settings a calibration ran with; older runs recorded only their Haiku band."""
    recorded = config.get("selection") or {"haiku_band": config.get("haiku_band", SELECTION_DEFAULTS["haiku_band"])}
    settings = {**SELECTION_DEFAULTS, **recorded}
    for key in ("haiku_band", "luna_band"):
        settings[key] = tuple(settings[key])
    assert settings["select_harness"] in CALIBRATION_HARNESSES, "a recorded selection names a calibration harness"
    return settings


def trial_timeout(requested, manifest=None):
    """Seconds per trial: the one asked for, else the frozen set's own, else the bench default."""
    timeout = requested or (manifest or {}).get("trial_timeout_seconds") or TRIAL_TIMEOUT_SECONDS
    assert type(timeout) is int, "a trial timeout is whole seconds"
    if timeout < 60:
        raise SystemExit(f"a trial timeout of {timeout}s is too short for any harness to start")
    return timeout


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
    args.timeout = trial_timeout(args.timeout)
    specs = _verified_candidates(args)
    log(f"{len(specs)} candidates verified")
    templates = set(args.templates.split(",")) if args.templates else None
    carry_specs, carry_rates = carried(args.carry, templates) if args.carry else ([], {})
    if {spec["id"] for spec in specs} & set(carry_rates):
        raise SystemExit("a carried task is also a candidate; recalibrate its template with --templates")
    run_dir = _run_dir("calibration", args.run_id)
    binary, sha = _pin(args.price_binary, run_dir, "pricing")
    archons = {"new": _pin_archon(args.archon_new, run_dir, "new")} if args.archon_new else {}
    settings = selection_settings(args)
    config = {"candidates": {"count": args.count, "first_seed": args.first_seed, "templates": args.templates},
              "arms": args.arm_keys, "repeats": args.repeats, "jobs": args.jobs, "timeout": args.timeout,
              "seed": args.seed, "price_binary_sha256": sha, "haiku_band": list(args.haiku_band),
              "carry": str(args.carry) if args.carry else None,
              "selection": {key: list(value) if isinstance(value, tuple) else value for key, value in settings.items()},
              "archon": {side: {"sha256": archon_sha} for side, (_, archon_sha) in archons.items()},
              "archon_options": archon_options(args) if archons else None,
              "versions": _versions({"new": binary}, {side: path for side, (path, _) in archons.items()})}
    _config_check(run_dir, config, reselect=("selection", "haiku_band"))
    log(f"calibration directory {run_dir}")
    context = {"run_dir": str(run_dir), "work_dir": _work_dir(args.work_root, run_dir), "binaries": {"new": binary},
               "timeout": args.timeout, "real_claude": shutil.which("claude"),
               "archon_bins": {side: str(path) for side, (path, _) in archons.items()},
               "archon_options": archon_options(args), "price_manifest": _price_manifest(binary, sha, run_dir)}
    records, stop_reason = execute(specs, args.arm_keys, args, run_dir, context)
    if stop_reason:
        log(f"calibration incomplete: {stop_reason}")
    return _write_calibration(run_dir, config, records, settings,
                              args.freeze and (args.freeze, specs + carry_specs), args.accept_off_target, carry_rates)


def _write_calibration(run_dir, config, records, settings, freeze, accept_off_target, carry_rates=None):
    """calibration.md from the records (re-selecting is free on a resumed run), then the frozen set if asked.

    `carry_rates` are tasks measured by an earlier calibration. A set that misses its
    targets is frozen only with `accept_off_target`, and its manifest says so."""
    assert records, "a calibration needs at least one trial"
    assert settings["min_size"] <= settings["max_size"], "the set's size bounds are ordered"
    measured = task_rates(records, settings["select_harness"])
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
    stats = {task: calibration_stats(rates[task]) for task in chosen}
    write_set(directory, chosen_specs, {"version": directory.name, "calibration": stats, "acceptance": verdict,
                                        "calibration_config": config, "selection": settings,
                                        "trial_timeout_seconds": config["timeout"]})
    shutil.copy(run_dir / "calibration.md", directory / "calibration.md")
    log(f"froze {len(chosen_specs)} tasks into {directory}")
    return 0


def calibration_stats(row):
    """What a frozen set's manifest keeps of a task's calibration: pass rates, repeats and seconds per harness."""
    keys = [key for key in row if key in ("haiku", "luna", "haiku_n", "luna_n") or key.endswith("_seconds")]
    assert {"haiku", "luna"} <= set(keys), "a calibrated task has a pass rate on both tiers"
    return {key: row[key] for key in keys}


def main(argv=None):
    if sys.version_info < (3, 11):
        raise SystemExit(f"the release bench needs Python 3.11 or newer, not {sys.version.split()[0]}")
    args = parse_args(sys.argv[1:] if argv is None else argv)
    handlers = {"run": command_run, "calibrate": command_calibrate, "report": command_report}
    return handlers[args.command](args)


if __name__ == "__main__":
    raise SystemExit(main())
