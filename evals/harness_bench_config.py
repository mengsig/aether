"""Benchmark treatments and explicit native launch settings."""
import os

TEAM_CHILD_MODEL = "claude-code/sonnet"
TEAM_CHILD_EFFORT = "high"
COORDINATOR_EFFORT = "medium"  # matches the native Codex default effort observed for pairing.
COORDINATOR_TEAM_MODEL = "codex/gpt-6-astra"
COORDINATOR_DEFAULT_MODEL = "codex/default"

# Appended to every condition's shared task objective. Native processes have
# no delegation capability at all; Aether single-agent conditions are told
# the same thing so the comparison is an apples-to-apples "work alone" task.
COMMON_SUFFIX = "\n\nWork only inside this repository/workspace. Do not access the network."
SOLO_SUFFIX = COMMON_SUFFIX + (
    " Do not delegate, spawn subagents, or create additional agents; do this yourself."
)
TEAM_SUFFIX = COMMON_SUFFIX + (
    "\n\nStaff this task with exactly two Aether child agents using model "
    f"{TEAM_CHILD_MODEL} at {TEAM_CHILD_EFFORT} effort to do the implementation work; "
    "review and integrate their results yourself before finishing."
)
DEFAULT_COORDINATOR_SUFFIX = COMMON_SUFFIX + (
    "\n\nDecide for yourself whether delegating this task helps. If you do delegate, "
    f"staff Aether child agents with model {TEAM_CHILD_MODEL} at {TEAM_CHILD_EFFORT} effort."
)

# Version 2 starts both sides of a Claude pair without the machine user's MCP servers; version 1
# kept them from the native side only. Manifests of two versions are never merged.
PROTOCOL_VERSION = 2

# Order matters only for the default --conditions listing; kept in the order
# the pilot protocol enumerates them.
CONDITIONS = {
    "native-claude": {"kind": "native-claude", "suffix": SOLO_SUFFIX},
    "native-codex": {"kind": "native-codex", "suffix": SOLO_SUFFIX},
    "aether-single-claude": {"kind": "aether-single", "model": "claude-code/default", "suffix": SOLO_SUFFIX},
    "aether-single-codex": {"kind": "aether-single", "model": "codex/default", "suffix": SOLO_SUFFIX},
    "aether-coordinator-team": {
        "kind": "aether-coordinator-team", "model": COORDINATOR_TEAM_MODEL,
        "effort": COORDINATOR_EFFORT, "suffix": TEAM_SUFFIX,
    },
    "aether-coordinator-default": {
        "kind": "aether-coordinator-default", "model": COORDINATOR_DEFAULT_MODEL,
        "effort": COORDINATOR_EFFORT, "suffix": DEFAULT_COORDINATOR_SUFFIX,
    },
}
DEFAULT_CONDITIONS = [key for key in CONDITIONS if "coordinator" not in key]
from harness_bench_treatments import diagnostic_conditions, lower_tier_conditions, minimal_luna_condition

CONDITIONS.update(lower_tier_conditions(SOLO_SUFFIX))
CONDITIONS["aether-solo-minimal-luna-low"] = minimal_luna_condition(SOLO_SUFFIX)
# Overhead-diagnosis arms: selectable only with --diagnostic, in no default and no counted set.
CONDITIONS.update(diagnostic_conditions(SOLO_SUFFIX))
ALL_CONDITIONS = list(CONDITIONS)

# clients/python/aether/client.py reads AETHER_ADDRESS/AETHER_TOKEN for a TCP
# bridge to a *live* daemon. This harness process may itself be running as an
# Aether CLI-engine session with those (or a live AETHER_SOCKET) set; every
# subprocess we spawn - native CLI or the engines Aether launches inside an
# isolated trial - must never inherit them, or a "single-agent, no-network"
# trial could silently reach the real daemon instead of its own sandbox.
_AETHER_ISOLATED_ENV_KEYS = {"AETHER_CONFIG_DIR", "AETHER_DATA_DIR", "AETHER_SOCKET"}


def strip_live_routing_env(env, keep=()):
    """Delete every AETHER_* key from `env` except `keep`, in place.

    Returns the sorted names removed (never values, so this is safe to log)
    for provenance: a trial record must show this ran, not assume it."""
    doomed = sorted(k for k in env if k.startswith("AETHER_") and k not in keep)
    for key in doomed:
        del env[key]
    return doomed


def scheduler_config_toml(max_concurrent_turns):
    """An Aether config.toml whose daemon runs at most this many model turns at once, engine turns included."""
    assert isinstance(max_concurrent_turns, int) and max_concurrent_turns >= 1, \
        f"a turn cap is a positive integer, not {max_concurrent_turns!r}"
    return f"[scheduler]\nmax_concurrent_turns = {max_concurrent_turns}\n"


# ----- native CLI command assembly ------------------------------------------


# Every native invocation bypasses interactive approval (there is no
# operator present) and structurally disables MCP so "native default" cannot
# accidentally reach back into a configured Aether bridge server. Both are
# recorded verbatim in the trial record's `permission_overrides`/`command`
# fields rather than applied silently.
CLAUDE_PERMISSION_OVERRIDES = ["--dangerously-skip-permissions", "--strict-mcp-config"]
CODEX_PERMISSION_OVERRIDES = ["--dangerously-bypass-approvals-and-sandbox", "-c", "mcp_servers={}"]


def native_claude_command(task_text, model=None, effort=None):
    """Preserve native model/effort unless the treatment explicitly selects them.
    Never replace the system prompt. --strict-mcp-config with no
    --mcp-config means zero MCP servers, regardless of user/project config."""
    overrides = (["--model", model] if model else []) + (["--effort", effort] if effort else [])
    return ["claude", "-p", "--output-format", "stream-json", "--verbose",
            *CLAUDE_PERMISSION_OVERRIDES, *overrides, task_text]


def native_codex_command(task_text, model=None, effort=None):
    """Preserve native model/effort unless the treatment explicitly selects them.
    `-c mcp_servers={}` overrides any configured
    MCP server (including a live Aether bridge) to none for this process."""
    overrides = (["--model", model] if model else [])
    if effort:
        overrides += ["-c", f'model_reasoning_effort="{effort}"']
    return ["codex", "exec", "--json", *CODEX_PERMISSION_OVERRIDES, *overrides, task_text]


def sanitized_native_env():
    """A copy of this process's environment with every AETHER_* var removed,
    so a native trial can never fall back to a live Aether TCP bridge."""
    env = dict(os.environ)
    stripped = strip_live_routing_env(env)
    return env, stripped
