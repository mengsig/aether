"""Helpers every task template shares: spec assembly, placeholders and name pools."""
import ast
import re
import textwrap

PLACEHOLDER = re.compile(r"@@([A-Z][A-Z0-9_]*)@@")
PACKAGE_WORDS = ["acme", "orbit", "harbor", "cobalt", "lumen", "quarry", "tandem", "willow",
                 "ember", "falcon", "garnet", "juniper", "kestrel", "meadow", "nimbus", "otter"]


def fill(text, dedent=True, allow_unused=False, **values):
    """Replace every @@KEY@@ in `text` (dedented unless it is an indented snippet).

    A key without a value is a template bug, and so is an unused value unless
    the caller fills one of several alternative texts (`allow_unused`)."""
    text = textwrap.dedent(text).lstrip("\n") if dedent else text
    used = set(PLACEHOLDER.findall(text))
    missing, unused = used - set(values), set(values) - used
    assert not missing, f"template placeholders without values: {sorted(missing)}"
    assert allow_unused or not unused, f"values for placeholders the template lacks: {sorted(unused)}"
    return PLACEHOLDER.sub(lambda match: str(values[match.group(1)]), text)


def count_checks(hidden_source):
    """The number of test methods a hidden module declares."""
    tree = ast.parse(hidden_source)
    stray = [node.name for node in tree.body if isinstance(node, ast.FunctionDef) and node.name.startswith("test")]
    assert not stray, f"hidden checks outside a TestCase are never run: {stray}"
    return sum(1 for node in tree.body if isinstance(node, ast.ClassDef)
               for item in node.body if isinstance(item, ast.FunctionDef) and item.name.startswith("test"))


def package_name(rng, suffix):
    assert suffix.isidentifier(), "a package suffix is an identifier"
    return f"{rng.choice(PACKAGE_WORDS)}_{suffix}"


def make_spec(*, family, template, seed, prompt, files, hidden, solution, history=(), tags=None, meta=None):
    checks = count_checks(hidden)
    assert checks >= 2, f"{template}-{seed}: a task needs at least two hidden checks"
    assert prompt.strip(), "a task needs a prompt"
    return {"id": f"{template}-{seed}", "family": family, "template": template, "seed": seed,
            "prompt": textwrap.dedent(prompt).strip(), "files": dict(files), "history": list(history),
            "tags": dict(tags or {}), "meta": dict(meta or {}), "hidden": hidden, "checks": checks, "solution": dict(solution)}


# Hidden-module helpers for differential checks: the reference solution runs
# beside the workspace's code on generated inputs that stay inside documented rules.
DIFFERENTIAL_HELPERS = '''
def load_reference(source):
    """The reference solution's globals, executed apart from the workspace's package."""
    namespace = {"__name__": "release_bench_reference"}
    exec(compile(source, "release_bench_reference", "exec"), namespace)
    return namespace


def outcome(function, *args):
    """A call's documented result: its value, or the documented error class it raised."""
    try:
        return ("value", function(*args))
    except ValueError:
        return ("ValueError", None)
    except TypeError:
        return ("TypeError", None)
'''


def reference_block(solution_source):
    """Hidden-module text defining REFERENCE_SOURCE plus the differential helpers."""
    assert solution_source.strip(), "a differential check needs the reference source"
    compile(solution_source, "release_bench_reference", "exec")
    return f"REFERENCE_SOURCE = {solution_source!r}\n" + DIFFERENTIAL_HELPERS
