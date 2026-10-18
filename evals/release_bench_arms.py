"""The arms of the release bench: which harness drives which model, under what caps.

An arm is one harness on one model tier. Every model tier has a native arm
(the plain CLI, the drift reference) and two Aether arms that differ only in
the binary: `old` (the last release) and `new` (the candidate). The optional
Archon arm queues the task on a private Archon over the candidate binary.
Every arm runs under its tier's spend allowance (release_bench_spend), which
a live guard enforces; harness-native caps are set to the same figure where
the harness has one. Every arm also installs a limit on the model-calling
agents a trial runs at once, the figure its budget reservation assumes.
"""
from harness_bench_config import COMMON_SUFFIX, SOLO_SUFFIX
from harness_bench_treatments import lower_tier_conditions
from release_bench_spend import ALLOWANCE_USD, DELEGATION_AGENTS

MODELS = ("haiku", "luna")
HARNESSES = ("native", "old", "new")
# Codex reports no dollar cost to Aether, so Aether's own cap on Luna arms is in tokens; the guard prices them.
LUNA_TOKEN_CAP = 4_000_000
CONDITION_KEYS = {"haiku": ("native-haiku-low", "aether-single-haiku-low"),
                  "luna": ("native-luna-low", "aether-single-luna-low")}


def arm_key(model, harness):
    assert model in MODELS, f"unknown model tier {model!r}"
    assert harness in HARNESSES + ("archon",), f"unknown harness {harness!r}"
    return f"{model}/{harness}"


def agent_cap(harness, allow_delegation):
    """Model-calling agents one trial of the arm may run at once: the limit the arm installs and the budget reserves.

    Aether's turn scheduler counts every agent, Archon's orchestrator included. The native
    CLIs limit only subagents, and to no fewer than one, so a native trial may run two."""
    assert harness in HARNESSES + ("archon",), f"unknown harness {harness!r}"
    delegates = DELEGATION_AGENTS if allow_delegation else 0
    if harness == "native":
        return 1 + max(1, delegates)
    return 1 + delegates + (1 if harness == "archon" else 0)


def _condition(model, harness, allow_delegation):
    native_key, aether_key = CONDITION_KEYS[model]
    conditions = lower_tier_conditions(COMMON_SUFFIX if allow_delegation else SOLO_SUFFIX)
    condition = dict(conditions[native_key if harness == "native" else aether_key])
    cap = agent_cap(harness, allow_delegation)
    if harness == "native":
        if model == "haiku":
            condition["max_budget_usd"] = ALLOWANCE_USD[model]
            condition["native_env"] = {"CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS": str(cap - 1)}
        else:
            # Codex reads the subagent limit from a different table under each multi-agent version.
            condition["codex_config"] = ["-c", f"agents.max_concurrent_threads_per_session={cap - 1}",
                                         "-c", f"features.multi_agent_v2.max_concurrent_threads_per_session={cap - 1}"]
        return condition
    condition["budget"] = {"cost_budget": ALLOWANCE_USD[model]} if model == "haiku" else {"token_budget": LUNA_TOKEN_CAP}
    condition["max_concurrent_turns"] = cap
    return condition


def build_arm(model, harness, allow_delegation=False):
    """One arm's treatment; `allow_delegation` drops the work-alone instruction for the delegation slice."""
    key = arm_key(model, harness)
    if harness == "archon":
        _, aether_key = CONDITION_KEYS[model]
        model_ref = lower_tier_conditions(SOLO_SUFFIX)[aether_key]["model"]
        cap = agent_cap(harness, allow_delegation)
        return {"key": key, "model": model, "harness": harness, "binary_role": None, "max_agents": cap,
                "condition": {"kind": "archon", "model": model_ref, "effort": "low",
                              "budget_usd": ALLOWANCE_USD[model], "max_concurrent_turns": cap,
                              "suffix": COMMON_SUFFIX if allow_delegation else SOLO_SUFFIX}}
    condition = _condition(model, harness, allow_delegation)
    assert condition["effort"] == "low", "every arm of the release bench runs at low effort"
    binary_role = None if harness == "native" else harness
    assert (binary_role is None) == condition["kind"].startswith("native-"), "only Aether arms name a binary"
    return {"key": key, "model": model, "harness": harness, "binary_role": binary_role,
            "max_agents": agent_cap(harness, allow_delegation), "condition": condition}


def parse_arms(text, archon=False):
    """Arm keys from "haiku/native,luna/new" or the shorthands "all" and a bare model tier."""
    keys = []
    for item in (part.strip() for part in text.split(",") if part.strip()):
        if item == "all":
            keys += [arm_key(model, harness) for model in MODELS for harness in HARNESSES]
        elif item in MODELS:
            keys += [arm_key(item, harness) for harness in HARNESSES]
        else:
            model, _, harness = item.partition("/")
            keys.append(arm_key(model, harness))
    if archon:
        models = sorted({key.split("/")[0] for key in keys}) or list(MODELS)
        keys += [arm_key(model, "archon") for model in models]
    unique = list(dict.fromkeys(keys))
    if not unique:
        raise ValueError(f"no arms selected by {text!r}")
    return unique
