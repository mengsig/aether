"""bundle / bundle: one repository with several open issues, each a hard task of its own, all to be fixed.

Per seed: which hard templates (PARTS of them, all different) and which of their
seeds. Each part keeps its own package, doc, starter, history and hidden checks;
the task passes only when every part's checks pass, as a release would need.
"""
PARTS = 3
PART_SEEDS = 20
SHARED_FILES = ("README.md", "tests/__init__.py")
PROMPT = ("This repository holds several packages, each with its contract in its own doc under `docs/`. "
          "{count} issues are open against it, and all of them must be fixed before the release:\n\n{issues}")

# The hidden module of a bundle: every part's checks run in a module of their own, so a part whose
# package no longer imports fails only its own checks, each still by name.
HIDDEN = '''
import ast
import sys
import types
import unittest

PARTS = {parts!r}


def _declared(source):
    tree = ast.parse(source)
    return {{node.name: [item.name for item in node.body
                        if isinstance(item, ast.FunctionDef) and item.name.startswith("test")]
            for node in tree.body if isinstance(node, ast.ClassDef)}}


def _failing(error):
    def check(self):
        self.fail(f"this part's hidden checks could not load: {{type(error).__name__}}: {{error}}")
    return check


def _load(label, source):
    module = types.ModuleType(f"hidden_{{label}}")
    sys.modules[module.__name__] = module
    try:
        exec(compile(source, f"<hidden {{label}}>", "exec"), module.__dict__)
    except Exception as error:  # The part's package does not import: its checks fail, the others still run.
        return {{name: type(name, (unittest.TestCase,), {{test: _failing(error) for test in tests}})
                for name, tests in _declared(source).items() if tests}}
    return {{name: value for name, value in vars(module).items()
            if isinstance(value, type) and issubclass(value, unittest.TestCase) and value.__module__ == module.__name__}}


for _label, _source in PARTS:
    for _name, _case in _load(_label, _source).items():
        _alias = f"{{_label}}_{{_name}}"
        globals()[_alias] = type(_alias, (_case,), {{"__module__": __name__, "__qualname__": _alias}})
# A loop variable left at module level would be loaded as one more test class.
del _label, _source, _name, _case, _alias
'''


def generate(seed, rng, make, templates):
    """A bundle of PARTS hard tasks; `make(template, seed)` builds one part's spec."""
    assert len(templates) >= PARTS and len(set(templates)) == len(templates), "enough distinct templates"
    chosen = sorted(rng.sample(sorted(templates), PARTS))
    parts = [make(template, rng.randrange(PART_SEEDS)) for template in chosen]
    assert len({part["id"] for part in parts}) == PARTS, "every part is a different task"
    labels = [part["id"].replace("-", "_") for part in parts]
    spec = {"id": f"bundle-{seed}", "family": "bundle", "template": "bundle", "seed": seed,
            "prompt": _prompt(parts), "files": _merged_files(parts), "solution": _merged_solution(parts),
            "meta": {"parts": ",".join(part["id"] for part in parts), "allow_delegation": False},
            "hidden": HIDDEN.format(parts=[(label, part["hidden"]) for label, part in zip(labels, parts)]),
            "checks": sum(part["checks"] for part in parts)}
    spec["history"], spec["tags"] = _merged_history(parts)
    assert all(not part["meta"].get("allow_delegation") for part in parts), "every part is a solo task"
    return spec


def _prompt(parts):
    issues = "\n\n".join(f"{index}. {part['prompt'].strip()}" for index, part in enumerate(parts, 1))
    return PROMPT.format(count=len(parts), issues=issues)


def _merged_files(parts):
    """The first commit: every part's files, with one README that holds each part's README in turn."""
    files, readmes = {}, []
    for part in parts:
        for path, text in part["files"].items():
            if path == "README.md":
                readmes.append(text.strip())
                continue
            if path in files and files[path] != text:
                raise ValueError(f"bundle parts disagree on {path}")
            files[path] = text
    files["README.md"] = "\n\n".join(readmes) + "\n"
    assert len(readmes) == len(parts), "every part has a README"
    return files


def _merged_solution(parts):
    solution = {}
    for part in parts:
        for path, text in part["solution"].items():
            if path in SHARED_FILES or (path in solution and solution[path] != text):
                raise ValueError(f"bundle part {part['id']} solves the shared file {path}")
            solution[path] = text
    return solution


def _merged_history(parts):
    """Every part's later commits in turn; a part's tags shift by the commits of the parts before it."""
    history, tags = [], {}
    for part in parts:
        offset = len(history)
        for name, index in part["tags"].items():
            if name in tags:
                raise ValueError(f"bundle parts share the tag {name}")
            tags[name] = offset + index
        history += part["history"]
    assert all(-1 <= index < len(history) for index in tags.values()), "tags point into the merged history"
    return history, tags

