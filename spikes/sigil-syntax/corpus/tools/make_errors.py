"""Generate the error corpus (errors/*.ron, errors/*.sigil, errors/expected.json).

Each case applies exactly one textual change to a copy of a base pattern, identically in both
syntaxes. Line and column of the expected diagnostic are computed from the generated file, so
they stay exact when a base pattern changes. Run from any directory:

    python spikes/sigil-syntax/corpus/tools/make_errors.py

The expected causes and fix hints are target messages (what a good compiler should say), not
measured output of any existing parser.
"""

from __future__ import annotations

import json
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent
EXT = {"ron": "ron", "sigil": "sigil"}

TOKEN = re.compile(
    r'"(?:[^"\\\n]|\\.)*"'
    r"|-?\d+(?:\.\d+)?(?:deg/t|deg|u/t2|u/t|u|t|beats)?"
    r"|[A-Za-z_][A-Za-z0-9_]*"
    r"|\S"
)

CASES = [
    {
        "id": "e01", "slug": "key-typo", "base": "01-ring-burst",
        "title_de": "Tippfehler im Schlüssel",
        "ron": {
            "find": "count: 24,", "replace": "cuont: 24,", "phase": "schema",
            "point": ("cuont", "after", 0), "node_path": "emitters.burst.block.cuont",
            "cause": "Unknown field `cuont` in block `Ring`.",
            "fix_hint": "Did you mean `count`? Allowed fields of `Ring`: count, start.",
            "note": "serde with deny_unknown_fields names the field and the allowed ones; "
                    "which position ron attaches (start or end of the identifier) is to be measured.",
        },
        "sigil": {
            "find": "    count = 24\n", "replace": "    cuont = 24\n", "phase": "schema",
            "point": ("cuont", "after", 0), "node_path": "emitters.burst.block.cuont",
            "cause": "Unknown field `cuont` in block `ring`.",
            "fix_hint": "Did you mean `count`? Allowed fields of `ring`: count, start.",
        },
    },
    {
        "id": "e02", "slug": "wrong-type", "base": "02-aimed-stream",
        "title_de": "Falscher Typ (Gleitkomma statt Ganzzahl)",
        "ron": {
            "find": "count: 3,", "replace": "count: 3.5,", "phase": "schema",
            "point": ("3.5", "after", 0), "node_path": "emitters.stream.block.count",
            "cause": "Field `count` expects an integer, found the float `3.5`.",
            "fix_hint": "Use a whole number of bullets, e.g. `count: 3,`.",
            "note": "serde reports `invalid type: floating point, expected u16` without the field "
                    "name; the node path needs a span-tracking layer on top of ron.",
        },
        "sigil": {
            "find": "    count = 3\n", "replace": "    count = 3.5\n", "phase": "schema",
            "point": ("3.5", "after", 0), "node_path": "emitters.stream.block.count",
            "cause": "Field `count` expects an integer, found the float `3.5`.",
            "fix_hint": "Use a whole number of bullets, e.g. `count = 3`.",
        },
    },
    {
        "id": "e03", "slug": "missing-field", "base": "04-mirrored-spiral",
        "title_de": "Fehlendes Pflichtfeld",
        "ron": {
            "find": "            speed: UnitsPerTick(0.09),\n", "replace": "", "phase": "schema",
            "point": ('"bloom"', "before", 0), "node_path": "emitters.bloom.speed",
            "cause": "Emitter `bloom` is missing the required field `speed`.",
            "fix_hint": "Add `speed: UnitsPerTick(<value>),`, e.g. `speed: UnitsPerTick(0.09),`.",
            "note": "serde raises `missing field` only when the struct ends, so ron most likely "
                    "points at the closing parenthesis of the emitter instead of its name (to be measured).",
        },
        "sigil": {
            "find": "  speed = 0.09u/t\n", "replace": "", "phase": "schema",
            "point": ("emitter bloom", "before", 8), "node_path": "emitters.bloom.speed",
            "cause": "Emitter `bloom` is missing the required field `speed`.",
            "fix_hint": "Add a line `speed = <value>u/t`, e.g. `speed = 0.09u/t`.",
        },
    },
    {
        "id": "e04", "slug": "unbalanced-bracket", "base": "03-subemitter-cascade",
        "title_de": "Nicht geschlossene Klammer",
        "ron": {
            "find": '                    emitter: "bloom",\n                ),\n            ],\n        ),\n',
            "replace": '                    emitter: "bloom",\n                ),\n            ],\n',
            "phase": "parse",
            "point": ("Bullet(", "after", 0), "node_path": "bullets.seed",
            "related": ("Bullet(", "before", 6, "opening parenthesis"),
            "cause": "The parenthesis of `Bullet(` opened at line {related_line} is not closed "
                     "before the next `Bullet(`.",
            "fix_hint": "Insert `),` before this line.",
            "note": "ron has no item keywords to resynchronise on: it reads `Bullet` as a field "
                    "name of the seed struct, so the likely raw message is `unknown field Bullet` "
                    "(deny_unknown_fields) or `expected ':'`; the opener line needs extra "
                    "bookkeeping (to be measured).",
        },
        "sigil": {
            "find": "    emitter = bloom\n  }\n}\n", "replace": "    emitter = bloom\n  }\n",
            "phase": "parse",
            "point": ("bullet shard", "after", 0), "node_path": "bullets.seed",
            "related": ("bullet seed {", "before", 12, "opening brace"),
            "cause": "`bullet` cannot start a member inside `bullet seed`; the `{` opened at "
                     "line {related_line} is not closed.",
            "fix_hint": "Insert `}` on its own line before this line.",
        },
    },
    {
        "id": "e05", "slug": "out-of-range", "base": "01-ring-burst",
        "title_de": "Wert außerhalb des Bereichs",
        "ron": {
            "find": "count: 24,", "replace": "count: 0,", "phase": "validate",
            "point": ("count: 0", "after", 7), "node_path": "emitters.burst.block.count",
            "cause": "`count` of block `Ring` must be in 1..=512, found 0; a ring of 0 bullets "
                     "has no defined angle step.",
            "fix_hint": "Use at least 1 bullet, e.g. `count: 24,`.",
        },
        "sigil": {
            "find": "    count = 24\n", "replace": "    count = 0\n", "phase": "validate",
            "point": ("count = 0", "after", 8), "node_path": "emitters.burst.block.count",
            "cause": "`count` of block `ring` must be in 1..=512, found 0; a ring of 0 bullets "
                     "has no defined angle step.",
            "fix_hint": "Use at least 1 bullet, e.g. `count = 24`.",
        },
    },
    {
        "id": "e06", "slug": "unknown-behaviour", "base": "05-wave-line-composite",
        "title_de": "Unbekannte Behaviour-ID",
        "ron": {
            "find": 'behaviour: "orbit_parent",', "replace": 'behaviour: "seek_target_weak2",',
            "phase": "validate",
            "point": ('"seek_target_weak2"', "after", 0), "node_path": "bullets.satellite.behaviour",
            "cause": "Unknown behaviour `seek_target_weak2`; registered behaviours: orbit_parent, "
                     "seek_target_weak.",
            "fix_hint": "Did you mean `seek_target_weak`? New behaviours must be registered in "
                        "Rust first (FR-10).",
        },
        "sigil": {
            "find": "behaviour = orbit_parent", "replace": "behaviour = seek_target_weak2",
            "phase": "validate",
            "point": ("seek_target_weak2", "after", 0), "node_path": "bullets.satellite.behaviour",
            "cause": "Unknown behaviour `seek_target_weak2`; registered behaviours: orbit_parent, "
                     "seek_target_weak.",
            "fix_hint": "Did you mean `seek_target_weak`? New behaviours must be registered in "
                        "Rust first (FR-10).",
        },
    },
    {
        "id": "e07", "slug": "wrong-unit", "base": "02-aimed-stream",
        "title_de": "Falsche Einheit (Ticks statt Grad)",
        "ron": {
            "find": "spread: Deg(12.0),", "replace": "spread: Ticks(12),", "phase": "schema",
            "point": ("Ticks(12)", "after", 0), "node_path": "emitters.stream.block.spread",
            "cause": "Field `spread` expects `Deg(..)`, found `Ticks(..)`.",
            "fix_hint": "Write `spread: Deg(12.0),`.",
            "note": "Only detectable if the ron version in use checks newtype struct names against "
                    "the expected type; otherwise `Ticks(12)` silently deserialises as `Deg(12.0)` "
                    "and no diagnostic appears at all (to be measured).",
        },
        "sigil": {
            "find": "spread = 12deg", "replace": "spread = 12t", "phase": "schema",
            "point": ("12t", "after", 0), "node_path": "emitters.stream.block.spread",
            "cause": "Field `spread` expects an angle in `deg`, found the tick quantity `12t`.",
            "fix_hint": "Write `spread = 12deg`.",
        },
    },
    {
        "id": "e08", "slug": "duplicate-emitter", "base": "05-wave-line-composite",
        "title_de": "Doppelter Emitter-Name",
        "ron": {
            "find": 'name: "rail",', "replace": 'name: "tide",', "phase": "validate",
            "point": ('"tide"', "after", 0), "node_path": "emitters[1].name",
            "related": ('name: "tide"', "first", 6, "first definition"),
            "cause": "Duplicate emitter name `tide`; first defined at line {related_line}.",
            "fix_hint": "Rename one of the emitters; emitter names must be unique within a unit.",
            "note": "Found by the validator because emitters are a list; a RON map keyed by name "
                    "would silently keep only the last entry when deserialised into a BTreeMap.",
        },
        "sigil": {
            "find": "emitter rail {", "replace": "emitter tide {", "phase": "validate",
            "point": ("emitter tide {", "after", 8), "node_path": "emitters[1].name",
            "related": ("emitter tide {", "first", 8, "first definition"),
            "cause": "Duplicate emitter name `tide`; first defined at line {related_line}.",
            "fix_hint": "Rename one of the emitters; emitter names must be unique within a unit.",
        },
    },
    {
        "id": "e09", "slug": "comma-misuse", "base": "04-mirrored-spiral",
        "title_de": "Komma-Fehler (doppeltes Komma in Liste)",
        "ron": {
            "find": "(at: Ticks(20), mul: 0.3),", "replace": "(at: Ticks(20), mul: 0.3),,",
            "phase": "parse",
            "point": ("),,", "after", 2), "node_path": "emitters.bloom.modifiers[2].keys",
            "cause": "Unexpected `,` in list `keys`: expected a value or `]`.",
            "fix_hint": "Remove the extra comma; a single trailing comma is allowed.",
            "note": "Trailing commas are legal in RON; only the doubled comma is an error.",
        },
        "sigil": {
            "find": "(at = 20t, mul = 0.3),", "replace": "(at = 20t, mul = 0.3),,",
            "phase": "parse",
            "point": ("),,", "after", 2), "node_path": "emitters.bloom.modifiers[2].keys",
            "cause": "Unexpected `,` in list `keys`: expected a value or `]`.",
            "fix_hint": "Remove the extra comma; a single trailing comma is allowed.",
        },
    },
    {
        "id": "e10", "slug": "wrong-version", "base": "03-subemitter-cascade",
        "title_de": "Falscher Versions-Header",
        "ron": {
            "find": "    version: 1,", "replace": "    version: 2,", "phase": "validate",
            "point": ("version: 2", "after", 9), "node_path": "version",
            "cause": "Unsupported Sigil version 2; this compiler reads version 1.",
            "fix_hint": "Set `version: 1,` or migrate the file with a newer `sigilc`.",
            "note": "The version is an ordinary field: the file is deserialised with the v1 schema "
                    "before the version can be checked, so a real v2 file would first produce "
                    "schema errors for its new fields unless the compiler pre-scans `version:`.",
        },
        "sigil": {
            "find": "sigil 1\n", "replace": "sigil 2\n", "phase": "parse",
            "point": ("sigil 2", "after", 6), "node_path": "version",
            "cause": "Unsupported Sigil version 2; this compiler reads `sigil 1`.",
            "fix_hint": "Change the header to `sigil 1` or migrate the file with a newer `sigilc`.",
        },
    },
]


def locate(text: str, start: int, spec: tuple) -> int:
    needle, direction, offset = spec[0], spec[1], spec[2]
    if direction == "after":
        pos = text.find(needle, start)
    elif direction == "before":
        pos = text.rfind(needle, 0, start)
    elif direction == "first":
        pos = text.find(needle)
    else:
        raise ValueError(direction)
    if pos < 0:
        raise SystemExit(f"anchor {needle!r} ({direction}) not found")
    return pos + offset


def line_col(text: str, pos: int) -> tuple[int, int]:
    line = text.count("\n", 0, pos) + 1
    col = pos - (text.rfind("\n", 0, pos) + 1) + 1
    return line, col


def token_at(text: str, pos: int) -> str:
    m = TOKEN.match(text, pos)
    return m.group(0) if m else ""


def main() -> None:
    out_dir = ROOT / "errors"
    out_dir.mkdir(exist_ok=True)
    expected = []
    for case in CASES:
        entry = {
            "id": case["id"], "slug": case["slug"], "title_de": case["title_de"],
            "base_pattern": case["base"],
        }
        for syntax in ("ron", "sigil"):
            spec = case[syntax]
            base = (ROOT / syntax / f"{case['base']}.{EXT[syntax]}").read_text(encoding="utf-8")
            hits = base.count(spec["find"])
            if hits != 1:
                raise SystemExit(f"{case['id']} {syntax}: find occurs {hits} times")
            start = base.index(spec["find"])
            text = base[:start] + spec["replace"] + base[start + len(spec["find"]):]
            if text == base:
                raise SystemExit(f"{case['id']} {syntax}: patch changes nothing")
            name = f"{case['id']}-{case['slug']}.{EXT[syntax]}"
            (out_dir / name).write_text(text, encoding="utf-8", newline="\n")

            pos = locate(text, start, spec["point"])
            line, col = line_col(text, pos)
            detail = {
                "file": f"errors/{name}", "phase": spec["phase"], "line": line, "column": col,
                "token": token_at(text, pos), "node_path": spec["node_path"],
            }
            cause = spec["cause"]
            if "related" in spec:
                rpos = locate(text, start, spec["related"])
                rline, rcol = line_col(text, rpos)
                detail["related"] = {"label": spec["related"][3], "line": rline, "column": rcol,
                                     "token": token_at(text, rpos)}
                # Plain replace, not str.format: messages contain literal braces.
                cause = cause.replace("{related_line}", str(rline))
            detail["cause"] = cause
            detail["fix_hint"] = spec["fix_hint"]
            if "note" in spec:
                detail["note"] = spec["note"]
            entry[syntax] = detail
        expected.append(entry)
    (out_dir / "expected.json").write_text(
        json.dumps(expected, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n"
    )
    for e in expected:
        r, s = e["ron"], e["sigil"]
        print(f"{e['id']} {e['slug']:<20} ron {r['line']}:{r['column']} {r['token']!r:<22} "
              f"sigil {s['line']}:{s['column']} {s['token']!r}")


if __name__ == "__main__":
    main()
