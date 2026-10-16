"""Which release the upgrade lane runs against, and fetching its binary.

The previous release is the newest published (not draft, not prerelease)
release whose version is at or below the workspace version, other than the
release tag under test: between releases it is the one users run, and on a
release tag, even one already published, it is the one before.
"""
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import sys
import tarfile
import urllib.request

REPOSITORY = "mengsig/aether"
ROOT = Path(__file__).resolve().parent.parent
TAG = re.compile(r"^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
DOWNLOAD_SECONDS = 120
TARGETS = {"x86_64": "x86_64-unknown-linux-musl", "aarch64": "aarch64-unknown-linux-musl"}


def parse_tag(tag):
    """(major, minor, patch) of a `vX.Y.Z` tag, or None for any other name."""
    match = TAG.match(tag)
    return tuple(int(part) for part in match.groups()) if match else None


def workspace_version(cargo_toml=ROOT / "Cargo.toml"):
    """The `version` under `[workspace.package]`, as (major, minor, patch)."""
    section = None
    for line in cargo_toml.read_text().splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            section = stripped
        elif section == "[workspace.package]" and stripped.startswith("version"):
            version = parse_tag("v" + stripped.split("=", 1)[1].strip().strip('"'))
            assert version is not None, f"workspace version is not X.Y.Z: {stripped}"
            return version
    raise ValueError(f"{cargo_toml} has no version under [workspace.package]")


def select_previous(releases, workspace, testing=None):
    """The tag of the newest published release at or below `workspace` that is
    not `testing`, the release tag this run checks."""
    assert len(workspace) == 3, f"a version is (major, minor, patch), got {workspace}"
    assert testing is None or parse_tag(testing), f"not a release tag: {testing}"
    candidates = []
    for release in releases:
        version = parse_tag(release.get("tag_name", ""))
        if release.get("draft") or release.get("prerelease") or version is None:
            continue
        if release["tag_name"] == testing:
            continue
        if version <= workspace:
            candidates.append((version, release["tag_name"]))
    if not candidates:
        raise ValueError(f"no published release at or below {'.'.join(map(str, workspace))}")
    return max(candidates)[1]


def fetch(url, accept="application/octet-stream"):
    headers = {"Accept": accept, "User-Agent": "aether-upgrade-lane"}
    token = os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers),
                                timeout=DOWNLOAD_SECONDS) as response:
        return response.read()


def previous_tag(testing=None):
    url = f"https://api.github.com/repos/{REPOSITORY}/releases?per_page=30"
    releases = json.loads(fetch(url, accept="application/vnd.github+json"))
    assert isinstance(releases, list), f"the releases API answered {releases!r:.200}"
    return select_previous(releases, workspace_version(), testing)


def release_target():
    machine = platform.machine()
    if sys.platform != "linux" or machine not in TARGETS:
        raise SystemExit(f"the upgrade lane runs on Linux x86_64 or aarch64, not "
                         f"{sys.platform} {machine}")
    return TARGETS[machine]


def expected_digest(checksum_text, archive_name):
    """The SHA-256 a `.sha256` file gives for `archive_name`."""
    fields = checksum_text.split()
    digest = fields[0].lower() if fields else ""
    named = fields[1].lstrip("*") if len(fields) > 1 else archive_name
    if not re.fullmatch(r"[0-9a-f]{64}", digest) or named != archive_name:
        raise ValueError(f"not a checksum of {archive_name}: {checksum_text!r:.200}")
    return digest


def download_binary(tag, directory):
    """Download, verify and unpack the release's `aether`; its path."""
    assert parse_tag(tag), f"not a release tag: {tag}"
    archive_name = f"aether-{release_target()}.tar.gz"
    base = f"https://github.com/{REPOSITORY}/releases/download/{tag}/{archive_name}"
    archive = fetch(base)
    digest = expected_digest(fetch(base + ".sha256").decode(), archive_name)
    actual = hashlib.sha256(archive).hexdigest()
    if actual != digest:
        raise ValueError(f"{archive_name} of {tag} has SHA-256 {actual}, its .sha256 says {digest}")
    path = directory / archive_name
    path.write_bytes(archive)
    with tarfile.open(path, "r:gz") as tar:
        member = tar.getmember("aether")
        assert member.isfile(), f"`aether` in {archive_name} is not a regular file"
        tar.extract(member, directory / tag, filter="data")
    binary = directory / tag / "aether"
    binary.chmod(0o755)
    return binary
