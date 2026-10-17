"""Parametrized task templates of the release bench, grouped into families.

Each template module exposes `generate(seed, rng) -> spec` (see
release_bench_tasks.py for the spec contract). A (template, seed) pair always
yields the same bytes, so a variant is cheap to make and a frozen set can be
re-derived and checked, yet the names, rules and bugs differ per seed.
"""
import random

from . import (
    algo_batching,
    algo_diff,
    delegation_workstreams,
    docspec_config,
    docspec_cron,
    docspec_duration,
    docspec_query,
    docspec_versions,
    edgecase_bucket,
    edgecase_ranges,
    edgecase_slug,
    longctx_protocol,
    multibug_billing,
    port_pricing,
    refactor_exporters,
    traceback_ledger,
)

TEMPLATES = {
    "batching": algo_batching.generate,
    "diff": algo_diff.generate,
    "config": docspec_config.generate,
    "cron": docspec_cron.generate,
    "duration": docspec_duration.generate,
    "query": docspec_query.generate,
    "versions": docspec_versions.generate,
    "ranges": edgecase_ranges.generate,
    "bucket": edgecase_bucket.generate,
    "slug": edgecase_slug.generate,
    "billing": multibug_billing.generate,
    "ledger": traceback_ledger.generate,
    "pricing": port_pricing.generate,
    "exporters": refactor_exporters.generate,
    "workstreams": delegation_workstreams.generate,
    "protocol": longctx_protocol.generate,
}


def generate(template, seed):
    """The task for one template and seed; the same pair always gives the same spec."""
    if template not in TEMPLATES:
        raise ValueError(f"unknown template {template!r}; choose from {sorted(TEMPLATES)}")
    assert type(seed) is int and seed >= 0, "a seed is a non-negative integer"
    spec = TEMPLATES[template](seed, random.Random(f"{template}:{seed}"))
    assert spec["template"] == template and spec["seed"] == seed, "a template must stamp its own identity"
    return spec


def candidates(count, first_seed=0, templates=None):
    """`count` specs, round-robin over templates so every family is represented evenly."""
    names = sorted(TEMPLATES) if templates is None else list(templates)
    assert count > 0 and names, "ask for at least one candidate from at least one template"
    return [generate(names[index % len(names)], first_seed + index // len(names)) for index in range(count)]
