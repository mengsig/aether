"""The arms of the release bench: which harness drives which model, under what caps.

An arm is one harness on one model tier. Every model tier has a native arm
(the plain CLI, the drift reference) and two Aether arms that differ only in
the binary: `old` (the last release) and `new` (the candidate). A history run
adds Aether arms named by a release tag, such as `haiku/v1.0.0`, each on that
release's binary. The optional Archon arms queue the task on a private Archon:
`archon-old` runs the last Archon release over the old Aether binary,
`archon-new` the Archon candidate over the candidate Aether binary.
Every arm runs under its tier's spend allowance (release_bench_spend), which
a live guard enforces; harness-native caps are set to the same figure where
the harness has one. Every arm also installs a limit on the model-calling
agents a trial runs at once, the figure its budget reservation assumes.
"""
import re

from harness_bench_config import COMMON_SUFFIX, SOLO_SUFFIX
from harness_bench_treatments import lower_tier_conditions
from release_bench_spend import ALLOWANCE_USD, DELEGATION_AGENTS

MODELS = ("haiku", "luna")
HARNESSES = ("native", "old", "new")
ARCHON_HARNESSES = ("archon-old", "archon-new")
# Runs recorded before Archon had two sides name the candidate side `archon`.
LEGACY_ARCHON = "archon"
# Codex reports no dollar cost to Aether, so Aether's own cap on Luna arms is in tokens; the guard prices them.
LUNA_TOKEN_CAP = 4_000_000
CONDITION_KEYS = {"haiku": ("native-haiku-low", "aether-single-haiku-low"),
                  "luna": ("native-luna-low", "aether-single-luna-low")}
RELEASE_LABEL = re.compile(r"v\d+\.\d+\.\d+")
ARCHON_REVIEWS = ("none", "standard", "strict")
# Archon runs its own reviewers on the arm's model; budget_factor scales the arm's spend allowance to pay for them.
ARCHON_DEFAULTS = {"pipeline": "direct", "review": "standard", "budget_factor": 2.0}
# A reviewed Archon stage must delegate its reviewer, so the work-alone rule names that one exception.
REVIEWED_SOLO_SUFFIX = COMMON_SUFFIX + (
    " Do the work yourself, without sub-agents; the only agent you may start is the reviewer your stage requires.")


def is_release_label(harness):
    """Whether `harness` names an extra Aether release binary of a history run, such as `v1.0.0`."""
    return RELEASE_LABEL.fullmatch(harness) is not None


def is_archon(harness):
    """Whether `harness` runs the task through Archon (either side, or a legacy record's `archon`)."""
    return harness in ARCHON_HARNESSES or harness == LEGACY_ARCHON


def archon_side(harness):
    """"old" or "new": the side whose Archon and Aether binaries an Archon arm runs."""
    assert is_archon(harness), f"{harness!r} is not an Archon arm"
    return "new" if harness == LEGACY_ARCHON else harness.split("-", 1)[1]


def known_harness(harness):
    return harness in HARNESSES or is_archon(harness) or is_release_label(harness)


def arm_key(model, harness):
    assert model in MODELS, f"unknown model tier {model!r}"
    assert known_harness(harness), f"unknown harness {harness!r}"
    return f"{model}/{harness}"


def agent_cap(harness, allow_delegation, reviewed=False):
    """Model-calling agents one trial of the arm may run at once: the limit the arm installs and the budget reserves.

    Aether's turn scheduler counts every agent, Archon's orchestrator included, and an Archon
    stage under review (`reviewed`) its reviewer too. The native CLIs limit only subagents,
    and to no fewer than one, so a native trial may run two."""
    assert known_harness(harness), f"unknown harness {harness!r}"
    assert not reviewed or is_archon(harness), "only Archon arms run reviewers"
    delegates = DELEGATION_AGENTS if allow_delegation else 0
    if harness == "native":
        return 1 + max(1, delegates)
    return 1 + delegates + (1 if is_archon(harness) else 0) + (1 if reviewed else 0)


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


def _archon_arm(key, model, harness, allow_delegation, archon_options):
    """An Archon arm: its side's Archon over its side's Aether, the arm's model at low effort for every agent."""
    options = {**ARCHON_DEFAULTS, **(archon_options or {})}
    assert options["review"] in ARCHON_REVIEWS, f"unknown Archon review family {options['review']!r}"
    assert options["pipeline"] and options["budget_factor"] >= 1, "an Archon arm has a pipeline and full budget"
    _, aether_key = CONDITION_KEYS[model]
    model_ref = lower_tier_conditions(SOLO_SUFFIX)[aether_key]["model"]
    reviewed = options["review"] != "none"
    cap = agent_cap(harness, allow_delegation, reviewed)
    allowance = round(ALLOWANCE_USD[model] * options["budget_factor"], 6)
    side = archon_side(harness)
    return {"key": key, "model": model, "harness": harness, "binary_role": side, "archon_role": side,
            "max_agents": cap, "allowance_usd": allowance,
            "condition": {"kind": "archon", "model": model_ref, "effort": "low", "budget_usd": allowance,
                          "max_concurrent_turns": cap, "pipeline": options["pipeline"],
                          "review": options["review"], "suffix": _archon_suffix(allow_delegation, reviewed)}}


def _archon_suffix(allow_delegation, reviewed):
    """The work-alone instruction, which under review still lets the stage delegate its own reviewer."""
    if allow_delegation:
        return COMMON_SUFFIX
    return REVIEWED_SOLO_SUFFIX if reviewed else SOLO_SUFFIX


def build_arm(model, harness, allow_delegation=False, archon_options=None):
    """One arm's treatment; `allow_delegation` drops the work-alone instruction for the delegation slice.

    `archon_options` ({"pipeline", "review", "budget_factor"}) shape the Archon arms only."""
    key = arm_key(model, harness)
    if is_archon(harness):
        return _archon_arm(key, model, harness, allow_delegation, archon_options)
    condition = _condition(model, harness, allow_delegation)
    assert condition["effort"] == "low", "every arm of the release bench runs at low effort"
    binary_role = None if harness == "native" else harness
    assert (binary_role is None) == condition["kind"].startswith("native-"), "only Aether arms name a binary"
    return {"key": key, "model": model, "harness": harness, "binary_role": binary_role,
            "max_agents": agent_cap(harness, allow_delegation), "allowance_usd": ALLOWANCE_USD[model],
            "condition": condition}


def parse_arms(text, archon_sides=()):
    """Arm keys from "haiku/native,luna/new" or the shorthands "all" and a bare model tier.

    `archon_sides` ("old", "new") are the sides whose Archon binary the run was given: the
    shorthands add their arms, and an explicit Archon arm of a side not given is refused.
    `<model>/archon` is the candidate side, `<model>/archon-new`."""
    sides = tuple(archon_sides)
    assert set(sides) <= {"old", "new"}, f"unknown Archon sides {sides}"
    archon_arms = [f"archon-{side}" for side in ("old", "new") if side in sides]
    keys = []
    for item in (part.strip() for part in text.split(",") if part.strip()):
        if item == "all":
            keys += [arm_key(model, harness) for model in MODELS for harness in (*HARNESSES, *archon_arms)]
        elif item in MODELS:
            keys += [arm_key(item, harness) for harness in (*HARNESSES, *archon_arms)]
        else:
            model, _, harness = item.partition("/")
            harness = "archon-new" if harness == LEGACY_ARCHON else harness
            if is_archon(harness) and archon_side(harness) not in sides:
                raise ValueError(f"arm {item!r} needs --archon-{archon_side(harness)}")
            keys.append(arm_key(model, harness))
    unique = list(dict.fromkeys(keys))
    if not unique:
        raise ValueError(f"no arms selected by {text!r}")
    return unique
