"""delegation / workstreams: four independent service packages to fix, with delegation allowed and asked for.

Each seed picks four of the services in delegation_services.py. Every package
has its own README rule set and its own defects, and none imports another, so
the work splits cleanly across sub-agents. The task passes only when all four
packages are right. Its spec sets `allow_delegation`, so Aether and Archon arms
run without the work-alone instruction and record how many agents they started.
"""
from .common import fill, make_spec
from .delegation_services import SERVICES

SERVICE_COUNT = 4

HIDDEN_HEAD = '''
import unittest
'''

PROMPTS = (
    "There are @@COUNT@@ independent bugs, one in each package under `services/` (@@NAMES@@). Each package's "
    "README.md states the behaviour its `core.py` must have; the code does not follow it. The packages share no "
    "code, so split the work: give each package to its own sub-agent, and when they are done check the whole "
    "suite with `python -m unittest`. Every package must end up matching its README exactly.",
    "Fix the @@COUNT@@ packages under `services/` (@@NAMES@@) so that each `core.py` does exactly what its "
    "README.md says. They are unrelated to each other: delegate one package per sub-agent and work in "
    "parallel, then run `python -m unittest` over everything before you finish.",
)


def generate(seed, rng):
    names = sorted(rng.sample(sorted(SERVICES), SERVICE_COUNT))
    services = {name: SERVICES[name](rng, name) for name in names}
    files = {"README.md": "# services\n\nIndependent small services; see each package's README.md.\n",
             "services/__init__.py": "", "tests/__init__.py": ""}
    solution, hidden = {}, HIDDEN_HEAD
    for service in services.values():
        assert not set(service["files"]) & set(files), "services never share a file"
        files.update(service["files"])
        solution.update(service["solution"])
        hidden += service["hidden"]
    assert len(solution) == SERVICE_COUNT, "each service has one file to fix"
    prompt = fill(rng.choice(PROMPTS), COUNT=SERVICE_COUNT, NAMES=", ".join(f"`{name}`" for name in names))
    return make_spec(family="delegation", template="workstreams", seed=seed, prompt=prompt, files=files,
                     hidden=hidden, solution=solution,
                     meta={"allow_delegation": True, "services": ",".join(names)})
