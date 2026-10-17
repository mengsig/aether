"""Release-bench task specs: validation, hashing, workspaces and frozen sets.

A task spec is plain JSON data, so a frozen set holds every byte a trial sees:

    id, family, template, seed  identity; id is "<template>-<seed>"
    prompt                      what every arm is asked, word for word
    files                       {relative path: text} of the first commit
    history                     [{"message", "files": {path: text or None}}] later commits
    tags                        {git tag name: index into history, -1 for the first commit}
    meta                        {name: JSON scalar} facts about the task, such as allow_delegation
    hidden                      unittest module source, never copied into the workspace
    checks                      how many tests the hidden module holds
    solution                    {path: text or None} overlay that passes every check
"""
import hashlib
import json
import os
import subprocess
from pathlib import Path, PurePosixPath

from aether_client import init_git_workspace

SPEC_KEYS = {"id", "family", "template", "seed", "prompt", "files", "history", "tags", "meta",
             "hidden", "checks", "solution"}
GIT_IDENTITY = ["-c", "user.email=eval@aether.test", "-c", "user.name=Aether Eval",
                "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"]
SET_FORMAT = 1


def _check_path(path):
    pure = PurePosixPath(path)
    if pure.is_absolute() or ".." in pure.parts or not pure.parts or path != str(pure):
        raise ValueError(f"task file path must be relative and normalized: {path!r}")
    if pure.parts[0] == ".git":
        raise ValueError(f"task files may not write into .git: {path!r}")


def _check_files(files, label, allow_delete=False):
    if not isinstance(files, dict):
        raise ValueError(f"{label} must map paths to text")
    for path, text in files.items():
        _check_path(path)
        if text is None and allow_delete:
            continue
        if not isinstance(text, str):
            raise ValueError(f"{label}[{path!r}] must be text")


def validate_spec(spec):
    """Raise ValueError unless `spec` is a complete, well-formed task; returns it."""
    if not isinstance(spec, dict) or set(spec) != SPEC_KEYS:
        missing = SPEC_KEYS - set(spec) if isinstance(spec, dict) else SPEC_KEYS
        raise ValueError(f"task spec keys differ from the contract; missing {sorted(missing)}")
    if spec["id"] != f"{spec['template']}-{spec['seed']}":
        raise ValueError(f"task id {spec['id']!r} is not <template>-<seed>")
    if not spec["prompt"].strip() or not spec["hidden"].strip():
        raise ValueError(f"{spec['id']}: prompt and hidden checks must be non-empty")
    if type(spec["checks"]) is not int or spec["checks"] < 2:
        raise ValueError(f"{spec['id']}: a task needs at least two hidden checks")
    _check_files(spec["files"], "files")
    _check_files(spec["solution"], "solution", allow_delete=True)
    if not spec["files"] or not spec["solution"]:
        raise ValueError(f"{spec['id']}: a task needs starter files and a solution")
    for commit in spec["history"]:
        if not commit.get("message") or set(commit) != {"message", "files"}:
            raise ValueError(f"{spec['id']}: a history commit has a message and files")
        _check_files(commit["files"], "history files", allow_delete=True)
    for tag, index in spec["tags"].items():
        if type(index) is not int or not -1 <= index < len(spec["history"]):
            raise ValueError(f"{spec['id']}: tag {tag!r} points outside the history")
    if not isinstance(spec["meta"], dict) or not all(
            value is None or isinstance(value, (str, int, float, bool)) for value in spec["meta"].values()):
        raise ValueError(f"{spec['id']}: meta maps names to JSON scalars")
    return spec


def canonical_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def task_hash(spec):
    return hashlib.sha256(canonical_json(validate_spec(spec)).encode()).hexdigest()


def set_hash(task_hashes):
    """One digest over a set: its sorted (id, hash) pairs, so order never matters."""
    assert task_hashes, "an empty set has no digest"
    listing = "".join(f"{task_id} {digest}\n" for task_id, digest in sorted(task_hashes.items()))
    return hashlib.sha256(listing.encode()).hexdigest()


def _write_files(root, files):
    for path, text in files.items():
        target = Path(root, path)
        if text is None:
            if target.exists():
                target.unlink()
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")


def _git(workspace, *args):
    env = dict(os.environ, GIT_AUTHOR_DATE="2026-01-01T00:00:00Z", GIT_COMMITTER_DATE="2026-01-01T00:00:00Z")
    try:
        subprocess.run(["git", *GIT_IDENTITY, *args], cwd=workspace, check=True,
                       capture_output=True, text=True, env=env)
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"git {' '.join(args)} failed in {workspace}: {error.stderr}") from error


def materialize(spec, workspace):
    """Write the task's starter repository (with its history and tags) into an empty directory."""
    validate_spec(spec)
    workspace = Path(workspace)
    workspace.mkdir(parents=True, exist_ok=True)
    if any(workspace.iterdir()):
        raise ValueError(f"workspace must start empty: {workspace}")
    _write_files(workspace, spec["files"])
    init_git_workspace(str(workspace))
    tags_at = {}
    for tag, index in spec["tags"].items():
        tags_at.setdefault(index, []).append(tag)
    for tag in sorted(tags_at.get(-1, [])):
        _git(workspace, "tag", tag)
    for index, commit in enumerate(spec["history"]):
        _write_files(workspace, commit["files"])
        _git(workspace, "add", "-A")
        _git(workspace, "commit", "-q", "--allow-empty", "-m", commit["message"])
        for tag in sorted(tags_at.get(index, [])):
            _git(workspace, "tag", tag)
    assert (workspace / ".git").is_dir(), "a materialized task is a git repository"
    return workspace


def apply_solution(spec, workspace):
    """Overlay the reference solution onto a materialized workspace."""
    assert Path(workspace, ".git").is_dir(), "apply the solution to a materialized task"
    _write_files(workspace, spec["solution"])


def write_set(directory, specs, manifest):
    """Freeze a set: tasks.jsonl holds every spec, manifest.json their hashes and the set digest."""
    directory = Path(directory)
    if directory.exists() and any(directory.iterdir()):
        raise FileExistsError(f"a frozen set is never overwritten: {directory}")
    ids = [spec["id"] for spec in specs]
    if len(set(ids)) != len(ids):
        raise ValueError("a frozen set holds each task once")
    directory.mkdir(parents=True, exist_ok=True)
    hashes = {spec["id"]: task_hash(spec) for spec in specs}
    with open(directory / "tasks.jsonl", "w", encoding="utf-8") as stream:
        for spec in sorted(specs, key=lambda s: s["id"]):
            stream.write(canonical_json(spec) + "\n")
    full = {**manifest, "format": SET_FORMAT, "task_count": len(specs),
            "task_hashes": dict(sorted(hashes.items())), "set_sha256": set_hash(hashes)}
    (directory / "manifest.json").write_text(json.dumps(full, indent=2, sort_keys=True) + "\n")
    return full


def _jsonl_lines(text):
    """JSON Lines records split on "\n" only: str.splitlines also breaks on U+2028, U+0085 and form feeds,
    which canonical JSON leaves unescaped inside strings."""
    assert "\r" not in text, "a frozen set is written with plain newlines"
    return text.split("\n")


def load_set(directory):
    """(manifest, specs) of a frozen set; raises ValueError when any byte changed since freezing."""
    directory = Path(directory)
    manifest = json.loads((directory / "manifest.json").read_text())
    if manifest.get("format") != SET_FORMAT:
        raise ValueError(f"{directory}: unsupported set format {manifest.get('format')!r}")
    specs = [validate_spec(json.loads(line)) for line in
             _jsonl_lines((directory / "tasks.jsonl").read_text(encoding="utf-8")) if line.strip()]
    hashes = {spec["id"]: task_hash(spec) for spec in specs}
    if hashes != manifest["task_hashes"]:
        changed = sorted(set(hashes.items()) ^ set(manifest["task_hashes"].items()))
        raise ValueError(f"{directory}: tasks differ from the manifest: {changed[:4]}")
    if set_hash(hashes) != manifest["set_sha256"] or len(specs) != manifest["task_count"]:
        raise ValueError(f"{directory}: set digest or size differs from the manifest")
    return manifest, specs
