# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    GithubWatcherConfig,
    RemoteSettings,
)

def configure_github_watcher(*, config: GithubWatcherConfig, expected_revision: int) -> dict[str, Any]:
    """Replace watcher settings only if no other operator changed them."""
    frame: dict[str, Any] = {"cmd": "configure_github_watcher"}
    frame["config"] = config
    frame["expected_revision"] = expected_revision
    return frame

def configure_remote(*, expected_revision: int, settings: RemoteSettings) -> dict[str, Any]:
    """Replace the remote settings only if nobody else changed them. Opens
    or closes the listener at once. Host only.
    """
    frame: dict[str, Any] = {"cmd": "configure_remote"}
    frame["expected_revision"] = expected_revision
    frame["settings"] = settings
    return frame
