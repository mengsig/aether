"""longctx / protocol: audit a 120-message decoder table against a ~100 KB spec and its errata.

`docs/protocol.md` defines every message type in its own section, states the
byte order rule in a framing section halfway through, and ends with errata
that amend some sections. The decoder's SCHEMA disagrees with the spec as
amended in four places; the prompt names none of them, so finding them means
reading the whole document against the whole table. Hidden checks encode
messages from the true schema and decode them with the workspace's decoder.
"""
from .common import fill, make_spec, package_name

TYPE_COUNT = 120
BUG_COUNT = 4
BIG_ENDIAN_FROM = 0x80
INTEGER_KINDS = ["u8", "i8", "u16", "i16", "u32", "i32"]
KIND_TEXT = {"u8": "unsigned 8-bit integer", "i8": "signed 8-bit integer", "u16": "unsigned 16-bit integer",
             "i16": "signed 16-bit integer", "u32": "unsigned 32-bit integer", "i32": "signed 32-bit integer",
             "str": "string (u8 length, then that many UTF-8 bytes)", "bool": "boolean (one byte, nonzero is true)"}
SUBJECTS = ["SENSOR", "VALVE", "PUMP", "DOOR", "METER", "RELAY", "FAN", "LAMP", "TANK", "GATE", "BELT", "DRUM",
            "HEATER", "MIXER", "PRESS", "SCALE"]
EVENTS = ["READING", "STATUS", "ALARM", "CONFIG", "RESET", "ACK", "FAULT", "LIMIT", "SAMPLE", "TICK", "PROFILE",
          "BATCH"]
FIELD_WORDS = ["level", "count", "flags", "offset", "zone", "rate", "delta", "limit", "slot", "phase", "mode",
               "retries", "battery", "signal", "label", "owner", "serial", "note", "enabled", "armed", "tag", "seq"]
RENAMES = {"level": "fill_level", "count": "item_count", "rate": "flow_rate", "zone": "zone_id",
           "offset": "time_offset", "slot": "slot_index", "signal": "signal_dbm", "seq": "sequence"}
ROLES = ["the controller", "a field unit", "the gateway", "a handheld", "the scheduler", "a remote panel"]
MOMENTS = ["on every change", "once per minute", "after a restart", "when polled", "when a threshold is crossed",
           "at the end of a cycle", "when an operator asks for it", "before shutting down"]
REACTIONS = ["log it and carry on", "acknowledge it within two seconds", "update their dashboards",
             "forward it upstream unchanged", "drop it when the unit is unknown", "store it for the daily report"]


def _schema(rng):
    """{code: (name, [(field, kind)])}: the true schema, codes sorted, names unique, some codes above 0x80."""
    low = rng.sample(range(0x01, BIG_ENDIAN_FROM), TYPE_COUNT - 30)
    high = rng.sample(range(BIG_ENDIAN_FROM, 0x90), 3) + rng.sample(range(0x90, 0xF0), 27)
    names = rng.sample([f"{subject}_{event}" for subject in SUBJECTS for event in EVENTS], TYPE_COUNT)
    schema = {}
    for code, name in zip(sorted(low + high), names):
        fields = rng.sample(FIELD_WORDS, rng.randint(2, 5))
        kinds = [rng.choice(INTEGER_KINDS + ["str", "bool"]) for _ in fields]
        schema[code] = (name, list(zip(fields, kinds)))
    assert len(schema) == TYPE_COUNT and any(code >= BIG_ENDIAN_FROM for code in schema), "some big-endian types"
    return schema


def _plant(rng, truth):
    """(true schema, starter schema, documented schema, errata lines, bugs).

    The documented schema is what the spec's sections say before the errata;
    the starter is the decoder's table with the planted disagreements."""
    truth = {code: (name, list(fields)) for code, (name, fields) in truth.items()}
    starter = {code: (name, list(fields)) for code, (name, fields) in truth.items()}
    documented = {code: (name, list(fields)) for code, (name, fields) in truth.items()}
    errata, bugs, used = [], [], set()
    for kind in rng.sample(["errata_kind", "errata_added", "kind", "order", "rename", "endian"], BUG_COUNT):
        if kind == "endian":
            bugs.append({"kind": kind, "code": None})
            continue
        eligible = [code for code in sorted(truth) if code < 0x70 and code not in used
                    and (kind != "rename" or any(field in RENAMES for field, _ in truth[code][1]))]
        code = rng.choice(eligible)
        used.add(code)
        line = _plant_one(rng, kind, code, truth, starter, documented)
        errata += [line] if line else []
        bugs.append({"kind": kind, "code": code})
    assert len(bugs) == BUG_COUNT and len(used) == sum(bug["code"] is not None for bug in bugs), "distinct bugs"
    return truth, starter, documented, errata, bugs


def _plant_one(rng, kind, code, truth, starter, documented):
    """Plant one disagreement in `code`'s entries, in place; returns its errata line, if it has one."""
    name, fields = truth[code]
    index = rng.randrange(len(fields))
    field, true_kind = fields[index]
    if kind in ("errata_kind", "kind"):
        wrong = rng.choice([other for other in INTEGER_KINDS + ["str", "bool"] if other != true_kind])
        starter[code][1][index] = (field, wrong)
        if kind == "kind":
            return None
        documented[code][1][index] = (field, wrong)
        return (f"Section 0x{code:02X} {name}: the type of `{field}` is now: {KIND_TEXT[true_kind]} "
                f"(it was: {KIND_TEXT[wrong]}).")
    if kind == "errata_added":
        field, true_kind = fields[-1]
        starter[code] = documented[code] = (name, fields[:-1])
        return (f"Section 0x{code:02X} {name}: a new last field `{field}` follows every existing field; its "
                f"type: {KIND_TEXT[true_kind]}.")
    if kind == "order":
        index = min(index, len(fields) - 2)
        swapped = list(fields)
        swapped[index], swapped[index + 1] = swapped[index + 1], swapped[index]
        starter[code] = (name, swapped)
        return None
    assert kind == "rename", f"unknown bug kind {kind!r}"
    index = next(i for i, (old, _) in enumerate(fields) if old in RENAMES)
    renamed = list(fields)
    renamed[index] = (RENAMES[fields[index][0]], fields[index][1])
    truth[code] = documented[code] = (name, renamed)
    return None


def _section(rng, code, name, fields):
    lines = [f"### 0x{code:02X} {name}", "",
             f"Sent by {rng.choice(ROLES)} {rng.choice(MOMENTS)}. Receivers {rng.choice(REACTIONS)}; "
             f"a {name.split('_')[0].lower()} that sends it {rng.choice(MOMENTS)} is misconfigured. "
             f"Older firmware also sent it {rng.choice(MOMENTS)}, which receivers should {rng.choice(REACTIONS)}.",
             "", "| # | Field | Type |", "|---|---|---|"]
    lines += [f"| {index} | `{field}` | {KIND_TEXT[kind]} |" for index, (field, kind) in enumerate(fields, 1)]
    return "\n".join(lines) + "\n"


def document(rng, documented, errata):
    """The spec: conventions, the message sections with the framing rule halfway through, then the errata."""
    codes = sorted(documented)
    middle = len(codes) // 2
    parts = [DOC_HEAD]
    for index, code in enumerate(codes):
        if index == middle:
            parts.append(FRAMING)
        parts.append(_section(rng, code, *documented[code]))
    decoys = [f"Section 0x{code:02X} {documented[code][0]}: the description's wording was clarified; the fields "
              "are unchanged." for code in rng.sample(codes, 6)]
    entries = errata + decoys
    rng.shuffle(entries)
    parts.append("## Errata\n\nThese amend the sections above and take precedence over them.\n\n" + "".join(
        f"{number}. {entry}\n" for number, entry in enumerate(entries, 1)))
    return "\n".join(parts)


DOC_HEAD = '''# Plant bus protocol

## Conventions

Every message is one type byte followed by that type's fields, in the order
its section lists them, with nothing between or after them. Integer fields
have the sizes their types name. Decoders return a dict holding `type` (the
message name) and every field by name.

## Messages
'''
FRAMING = '''## Framing and byte order

Messages travel in frames of up to 255 bytes. Multi-byte integers of message
types below 0x80 are little-endian; message types from 0x80 up use big-endian
integers, because those come from the older bus controllers.

## Messages (continued)
'''

DECODER = '''
"""Decoder for the plant bus protocol (docs/protocol.md)."""
import struct

BIG_ENDIAN_FROM = @@BIG_ENDIAN_FROM@@
FORMATS = {"u8": "B", "i8": "b", "u16": "H", "i16": "h", "u32": "I", "i32": "i"}
SCHEMA = {
@@SCHEMA@@}


def decode(data):
    """The message in `data` as a dict: its `type` name and every field."""
    if not data:
        raise ValueError("empty message")
    code = data[0]
    if code not in SCHEMA:
        raise ValueError(f"unknown message type 0x{code:02X}")
    name, fields = SCHEMA[code]
    order = ">" if code >= BIG_ENDIAN_FROM else "<"
    offset, message = 1, {"type": name}
    for field, kind in fields:
        if kind == "str":
            length = data[offset]
            message[field] = data[offset + 1:offset + 1 + length].decode("utf-8")
            offset += 1 + length
        elif kind == "bool":
            message[field] = data[offset] != 0
            offset += 1
        else:
            layout = order + FORMATS[kind]
            message[field] = struct.unpack_from(layout, data, offset)[0]
            offset += struct.calcsize(layout)
    if offset != len(data):
        raise ValueError(f"{len(data) - offset} bytes left after a {name} message")
    return message
'''

HIDDEN = '''
import random
import struct
import unittest

from @@PKG@@.decode import decode

TRUTH = @@TRUTH@@
BIG_ENDIAN_FROM = 0x80
FORMATS = {"u8": "B", "i8": "b", "u16": "H", "i16": "h", "u32": "I", "i32": "i"}
RANGES = {"u8": (0, 255), "i8": (-128, 127), "u16": (0, 65535), "i16": (-32768, 32767), "u32": (0, 2 ** 32 - 1),
          "i32": (-2 ** 31, 2 ** 31 - 1)}


def sample(code, rng):
    name, fields = TRUTH[code]
    order = ">" if code >= BIG_ENDIAN_FROM else "<"
    data, message = bytes([code]), {"type": name}
    for field, kind in fields:
        if kind == "str":
            value = "".join(rng.choice("abcxyz é0") for _ in range(rng.randint(0, 9)))
            raw = value.encode("utf-8")
            data += bytes([len(raw)]) + raw
        elif kind == "bool":
            value = rng.random() < 0.5
            data += bytes([rng.randint(1, 255) if value else 0])
        else:
            value = rng.randint(*RANGES[kind])
            data += struct.pack(order + FORMATS[kind], value)
        message[field] = value
    return data, message


class HiddenProtocolChecks(unittest.TestCase):
    def assert_decodes(self, codes, rounds=25):
        rng = random.Random(7)
        for code in codes:
            for _ in range(rounds):
                data, message = sample(code, rng)
                self.assertEqual(decode(data), message, f"0x{code:02X}")
@@BUG_CHECKS@@
    def test_every_message_type_decodes(self):
        self.assert_decodes(sorted(TRUTH), rounds=6)
'''
BUG_CHECK = '''
    def test_@@LABEL@@(self):
        self.assert_decodes(@@CODES@@)
'''

PROMPTS = (
    "Partners report wrong values from `@@PKG@@/decode.py` for some message types. The decoder's SCHEMA must "
    "match `docs/protocol.md` exactly, as amended by the errata at its end, but nobody knows which entries are "
    "wrong. Audit the decoder against the whole spec and fix every disagreement; message types that are "
    "already right must keep decoding as they do.",
    "`@@PKG@@/decode.py` decodes the plant bus messages described in `docs/protocol.md`. Some of its table "
    "entries, or its rules, no longer agree with the spec (read all of it, errata included). Find and fix every "
    "disagreement without breaking the message types that already decode correctly.",
)


def _schema_source(schema):
    return "".join(f"    0x{code:02X}: ({name!r}, {fields!r}),\n" for code, (name, fields) in sorted(schema.items()))


def generate(seed, rng):
    pkg = package_name(rng, "bus")
    truth, starter, documented, errata, bugs = _plant(rng, _schema(rng))
    big_from = 0x90 if any(bug["kind"] == "endian" for bug in bugs) else BIG_ENDIAN_FROM
    checks = []
    for bug in bugs:
        codes = ([code for code in sorted(truth) if BIG_ENDIAN_FROM <= code < 0x90][:6] if bug["code"] is None
                 else [bug["code"]])
        label = "big_endian_types" if bug["code"] is None else f"type_{bug['code']:02x}_{bug['kind']}"
        checks.append(fill(BUG_CHECK, dedent=False, LABEL=label, CODES=repr(codes)))
    assert all("CODES" not in check for check in checks), "every bug check names its message types"
    files = {"README.md": f"# {pkg}\n\nPlant bus tools. The protocol is in `docs/protocol.md`.\n",
             "docs/protocol.md": document(rng, documented, errata), f"{pkg}/__init__.py": "",
             f"{pkg}/decode.py": fill(DECODER, BIG_ENDIAN_FROM=f"0x{big_from:02X}", SCHEMA=_schema_source(starter)),
             "tests/__init__.py": "", "tests/test_decode.py": fill(VISIBLE_TESTS, PKG=pkg)}
    solution = fill(DECODER, BIG_ENDIAN_FROM=f"0x{BIG_ENDIAN_FROM:02X}", SCHEMA=_schema_source(truth))
    hidden = fill(HIDDEN, PKG=pkg, TRUTH=repr(truth), BUG_CHECKS="".join(checks))
    meta = {"bugs": ",".join(bug["kind"] for bug in bugs), "doc_bytes": len(files["docs/protocol.md"])}
    return make_spec(family="longctx", template="protocol", seed=seed, prompt=fill(rng.choice(PROMPTS), PKG=pkg),
                     files=files, hidden=hidden, solution={f"{pkg}/decode.py": solution}, meta=meta)


VISIBLE_TESTS = '''
import unittest

from @@PKG@@.decode import decode


class DecodeTests(unittest.TestCase):
    def test_empty_message(self):
        with self.assertRaises(ValueError):
            decode(b"")

    def test_unknown_type(self):
        with self.assertRaises(ValueError):
            decode(bytes([0xFF]))


if __name__ == "__main__":
    unittest.main()
'''
