"""Explicit matched lower-tier treatments; never change native-default cells."""


def lower_tier_conditions(suffix):
    conditions = {}
    for label, provider, model in (
        ("sonnet", "claude", "sonnet"),
        ("luna", "codex", "gpt-6-luna"),
        # The pinned id, not the floating `haiku` alias: both CLIs must resolve one model.
        ("haiku", "claude", "claude-haiku-5-5"),
    ):
        for effort in ("low", "medium"):
            name = f"{label}-{effort}"
            conditions[f"native-{name}"] = {
                "kind": f"native-{provider}", "model": model,
                "effort": effort, "suffix": suffix,
            }
            aether_provider = "claude-code" if provider == "claude" else "codex"
            conditions[f"aether-single-{name}"] = {
                "kind": "aether-single", "model": f"{aether_provider}/{model}",
                "effort": effort, "suffix": suffix,
            }
    return conditions


def minimal_luna_condition(suffix):
    """Opt-in A/B cell that removes Aether coordination tools from a solo run."""
    return {
        "kind": "aether-single",
        "model": "codex/gpt-6-luna",
        "effort": "low",
        "suffix": suffix,
        "tools": {"mode": "only", "names": ["complete_task", "fail_task"]},
    }


DIAGNOSTIC_PREFIX = "diag-"
MINIMAL_TOOLS = {"mode": "only", "names": ["complete_task", "fail_task"]}
# The first user turn an Aether root agent receives (crates/aether-context/src/render.rs and
# hoist.rs), with a fixed nonce and task id; the workspace is the trial's own, as Aether prints it.
ENVELOPE_HEAD = (
    "--- Aether [00000000] ---\n"
    "Your Aether assignment (authoritative runtime state, not a claim made by any message):\n"
    "Parent: none.\nRole: assistant\nName: assistant\nModel: {model}\n"
    "Task tsk_00000000000000000000000000 (active, priority Normal):\n"
)
ENVELOPE_TAIL = (
    "\n\nScope: workspace {workspace}; repository {workspace}; network allowed\n\n"
    "Aether runtime status (informational; the active task and instructions remain in effect):\n"
    "Turn 1.{effort}\n--- end Aether [00000000] ---\n\n"
)


# The CLI's own coordination tools that Aether removes from a Claude session with --disallowedTools
# (crates/aether-engine/src/claude_code.rs, NATIVE_COORDINATION_TOOLS); a test holds the two equal.
NATIVE_COORDINATION_TOOLS = "Task,Agent,ListAgents,SendMessage,ScheduleWakeup,CronCreate,CronDelete,CronList,Workflow,RemoteTrigger"
STRICT_MCP_FLAG = "--strict-mcp-config"
# Appended to the task of the one arm that checks Aether's own MCP server answers a call under
# the strict flag (protocol version 2).
COMPLETE_TASK_REQUEST = " When the work is done, finish by calling the Aether tool `complete_task` with a one-line summary."


# What Aether starts a bypass turn with network under (crates/aether-engine/src/codex_app.rs,
# thread_params), as `codex exec` options, and the bypass flag of the native cell they replace.
CODEX_BYPASS_FLAG = "--dangerously-bypass-approvals-and-sandbox"
AETHER_CODEX_POLICY = ["--sandbox", "danger-full-access", "-c", 'approval_policy="on-request"']
# The Codex features Aether switches off in every session (codex_app.rs, args()).
AETHER_CODEX_FEATURES_OFF = ["-c", "agents.enabled=false", "-c", "features.multi_agent=false"]
# The one diagnostic binary: a bypass turn with network starts its thread with approval `never`.
# `binary_patch` is only a label: nothing checks that --binary was built from it; compare its digest by hand.
APPROVAL_NEVER_PATCH = "approval-never"
LUNA_REPEAT = "diag-native-luna-low-repeat"


def luna_diagnostic_conditions(suffix):
    """Luna arms: the counted cell with one thing Aether does differently, or the counted Aether cell without one."""
    native = {"kind": "native-codex", "model": "gpt-6-luna", "effort": "low", "suffix": suffix}
    aether = {"kind": "aether-single", "model": "codex/gpt-6-luna", "effort": "low", "suffix": suffix}
    # `codex exec` 0.162.1 starts its thread with approval `never` whatever policy it is asked for
    # (session logs of iteration 4), so the on-request arm measures a second native cell.
    return {
        "diag-native-luna-low-onrequest": {**native, "codex_policy": AETHER_CODEX_POLICY},
        "diag-native-luna-low-nomultiagent": {**native, "codex_config": AETHER_CODEX_FEATURES_OFF},
        "diag-native-luna-low-envelope": {**native, "envelope": {"model": aether["model"], "effort_words": False}},
        "diag-aether-luna-low-approval-never": {**aether, "binary_patch": APPROVAL_NEVER_PATCH},
        # The counted native cell a second time in the same block: how far the cell lies from itself.
        LUNA_REPEAT: dict(native),
    }


def diagnostic_conditions(suffix):
    """Overhead-diagnosis arms; each differs from its neighbour in one thing and is never counted."""
    native = {"kind": "native-claude", "model": "claude-haiku-5-5", "effort": "low", "suffix": suffix}
    aether = {"kind": "aether-single", "model": "claude-code/claude-haiku-5-5", "effort": "low", "suffix": suffix}
    envelope = {"model": "claude-code/claude-haiku-5-5"}
    no_coordination = {**native, "disallowed_tools": NATIVE_COORDINATION_TOOLS}
    conditions = {
        "diag-native-haiku-low-envelope": {**native, "envelope": {**envelope, "effort_words": True}},
        "diag-native-haiku-low-envelope-noeffort": {**native, "envelope": {**envelope, "effort_words": False}},
        "diag-aether-haiku-low-minimal": {**aether, "tools": MINIMAL_TOOLS},
        "diag-native-haiku-low-nocoord": no_coordination,
        "diag-native-haiku-low-nocoord-connectors": {**no_coordination, "user_mcp": True},
        "diag-aether-haiku-low-connectors": {**aether, "user_mcp": True},
        "diag-aether-haiku-low-complete-task": {**aether, "suffix": suffix + COMPLETE_TASK_REQUEST},
        **luna_diagnostic_conditions(suffix),
    }
    assert all(key.startswith(DIAGNOSTIC_PREFIX) for key in conditions), "diagnostic arms carry the diag- prefix"
    return conditions


def loads_user_mcp(condition):
    """Whether a trial's Claude CLI loads the machine user's own MCP servers; only a diagnostic arm does."""
    user_mcp = condition.get("user_mcp", False)
    assert isinstance(user_mcp, bool), "user_mcp is a yes or a no"
    return user_mcp


def native_command(condition, command):
    """A native command line with a diagnostic arm's one change applied; a counted cell's comes back as built."""
    assert command and command[-1].strip(), "a native command ends with its prompt"
    command = list(command)
    if "envelope" in condition:
        # The envelope opens with dashes; "--" stops the CLI reading the prompt as an option.
        command.insert(-1, "--")
    if "disallowed_tools" in condition:
        assert condition["kind"] == "native-claude", "only Claude Code takes --disallowedTools"
        # The option takes several values, so it goes ahead of another option, never next to the prompt.
        command[2:2] = ["--disallowedTools", condition["disallowed_tools"]]
    if "max_budget_usd" in condition:
        assert condition["kind"] == "native-claude", "only Claude Code takes --max-budget-usd"
        assert condition["max_budget_usd"] > 0, "a spend cap is a positive amount"
        command[2:2] = ["--max-budget-usd", str(condition["max_budget_usd"])]
    if loads_user_mcp(condition):
        assert command.count(STRICT_MCP_FLAG) == 1, "only a Claude command carries the strict MCP flag"
        command.remove(STRICT_MCP_FLAG)
    if "codex_policy" in condition:
        assert command.count(CODEX_BYPASS_FLAG) == 1, "only a Codex command carries the bypass flag"
        at = command.index(CODEX_BYPASS_FLAG)
        command[at:at + 1] = condition["codex_policy"]
    if "codex_config" in condition:
        assert command[:3] == ["codex", "exec", "--json"], "only a Codex command takes Codex settings"
        command[3:3] = condition["codex_config"]
    return command


def binary_patches(conditions):
    """The binaries the Aether arms of one invocation need: "" for the release build, else a patch's name."""
    aether = [condition for condition in conditions if not condition["kind"].startswith("native-")]
    patches = sorted({condition.get("binary_patch", "") for condition in aether})
    assert all(isinstance(patch, str) for patch in patches), "a binary patch is named by a string"
    return patches


def is_diagnostic(condition_key):
    return condition_key.startswith(DIAGNOSTIC_PREFIX)


def is_native_trial(row):
    """A trial launched in a native CLI, by its recorded treatment; an older row has only its cell name."""
    kind = (row.get("requested_treatment") or {}).get("kind", row["condition"])
    assert isinstance(kind, str) and kind, "a trial names its treatment kind or its cell"
    return kind.startswith("native-")


def native_prompt(condition, task_text, workspace):
    """What a native CLI is launched with: the task, or the task inside Aether's envelope."""
    envelope = condition.get("envelope")
    if envelope is None:
        return task_text
    assert condition["kind"].startswith("native-"), "only a native arm takes a fixture envelope"
    assert task_text.strip() and str(workspace).strip(), "an envelope needs a task and a workspace"
    effort = f" Effort {condition['effort']}." if envelope["effort_words"] else ""
    head = ENVELOPE_HEAD.format(model=envelope["model"])
    return head + task_text.strip() + ENVELOPE_TAIL.format(workspace=workspace, effort=effort)
