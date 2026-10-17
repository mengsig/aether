"""The arms of the release bench: which harness drives which model, under what caps.

An arm is one harness on one model tier. Every model tier has a native arm
(the plain CLI, the drift reference) and two Aether arms that differ only in
the binary: `old` (the last release) and `new` (the candidate). The optional
Archon arm queues the task on a private Archon over the candidate binary.
Every arm runs under its tier's spend allowance (release_bench_spend), which
a live guard enforces; harness-native caps are set to the same figure where
the harness has one.
"""
from harness_bench_config import COMMON_SUFFIX, SOLO_SUFFIX
from harness_bench_treatments import lower_tier_conditions
from release_bench_spend import ALLOWANCE_USD

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


def _condition(model, harness, allow_delegation):
    native_key, aether_key = CONDITION_KEYS[model]
    conditions = lower_tier_conditions(COMMON_SUFFIX if allow_delegation else SOLO_SUFFIX)
    condition = dict(conditions[native_key if harness == "native" else aether_key])
    if harness == "native":
        if model == "haiku":
            condition["max_budget_usd"] = ALLOWANCE_USD[model]
        return condition
    condition["budget"] = {"cost_budget": ALLOWANCE_USD[model]} if model == "haiku" else {"token_budget": LUNA_TOKEN_CAP}
    return condition


def build_arm(model, harness, allow_delegation=False):
    """One arm's treatment; `allow_delegation` drops the work-alone instruction for the delegation slice."""
    key = arm_key(model, harness)
    if harness == "archon":
        _, aether_key = CONDITION_KEYS[model]
        model_ref = lower_tier_conditions(SOLO_SUFFIX)[aether_key]["model"]
        return {"key": key, "model": model, "harness": harness, "binary_role": None,
                "condition": {"kind": "archon", "model": model_ref, "effort": "low",
                              "budget_usd": ALLOWANCE_USD[model],
                              "suffix": COMMON_SUFFIX if allow_delegation else SOLO_SUFFIX}}
    condition = _condition(model, harness, allow_delegation)
    assert condition["effort"] == "low", "every arm of the release bench runs at low effort"
    binary_role = None if harness == "native" else harness
    assert (binary_role is None) == condition["kind"].startswith("native-"), "only Aether arms name a binary"
    return {"key": key, "model": model, "harness": harness, "binary_role": binary_role, "condition": condition}


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
