"""Check the pattern corpus: RON/sigil equivalence, readability rules and feature coverage.

Not a parser. Both variants are reduced to token-level streams that must match:
  - the value stream (numbers with units, strings, identifiers, enum variants),
  - the field-name stream (keys of records/structs, override paths),
  - the comment stream (text of every // comment, in order).
It also checks the readability rules of the schema (pairwise distinct silhouettes per unit,
enemy palette space) and prints the coverage matrix used in the README. Run:

    python spikes/sigil-syntax/corpus/tools/check_corpus.py
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PATTERNS = sorted(p.stem for p in (ROOT / "sigil").glob("*.sigil"))

UNIT_RON = {"Ticks": "t", "Deg": "deg", "Units": "u", "UnitsPerTick": "u/t",
            "UnitsPerTick2": "u/t2", "DegPerTick": "deg/t"}
RON_STRUCTS = {"Sigil", "Meta", "Bullet", "Emitter", "Import", "Use", "Times"}
CONTAINERS = {"version", "imports", "meta", "bullets", "emitters", "transforms", "modifiers",
              "block", "name", "path", "alias", "from", "overrides"}
SIGIL_KEYWORDS = {"sigil", "meta", "import", "as", "bullet", "emitter", "from", "block",
                  "modifier", "transform"}

TOK = re.compile(
    r'(?P<str>"(?:[^"\\\n]|\\.)*")'
    r"|(?P<qty>-?\d+(?:\.\d+)?(?:deg/t|deg|u/t2|u/t|u|t|beats)(?![A-Za-z0-9_]))"
    r"|(?P<num>-?\d+(?:\.\d+)?)"
    r"|(?P<id>[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)"
    r"|(?P<p>[{}()\[\]=,:])"
    r"|(?P<ws>\s+)"
)


def split_comments(text: str) -> tuple[str, list[str]]:
    code, comments = [], []
    for line in text.splitlines():
        in_str, cut = False, None
        for i, ch in enumerate(line):
            if ch == '"':
                in_str = not in_str
            elif not in_str and line.startswith("//", i):
                cut = i
                break
        if cut is None:
            code.append(line)
        else:
            code.append(line[:cut])
            comments.append(line[cut + 2:].strip())
    return "\n".join(code), comments


def tokens(code: str) -> list[tuple[str, str]]:
    out, pos = [], 0
    while pos < len(code):
        m = TOK.match(code, pos)
        if not m:
            raise SystemExit(f"cannot tokenise near {code[pos:pos + 20]!r}")
        pos = m.end()
        if m.lastgroup != "ws":
            out.append((m.lastgroup, m.group(0)))
    return out


def snake(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def norm_str(s: str) -> str:
    s = s[1:-1]
    return re.sub(r"\.(ron|sigil)$", "", s)


def split_qty(q: str) -> tuple[float, str]:
    m = re.match(r"(-?\d+(?:\.\d+)?)(.*)", q)
    return float(m.group(1)), m.group(2)


def ron_streams(text: str):
    text = "\n".join(l for l in text.splitlines() if not l.startswith("#!["))
    code, comments = split_comments(text)
    t = tokens(code)
    values, fields = [], []
    i = 0
    while i < len(t):
        kind, s = t[i]
        nxt = t[i + 1][1] if i + 1 < len(t) else ""
        if kind == "id" and nxt == ":":
            if s not in CONTAINERS:
                fields.append(s)
        elif kind == "str" and nxt == ":":
            fields.append(norm_str(s))
        elif kind == "id" and s in UNIT_RON and nxt == "(":
            values.append(("q", float(t[i + 2][1]), UNIT_RON[s]))
            i += 3
        elif kind == "id" and s in RON_STRUCTS:
            pass
        elif kind == "id":
            values.append(("v", snake(s)))
        elif kind == "str":
            values.append(("v", norm_str(s)))
        elif kind == "num":
            values.append(("n", float(s)))
        i += 1
    return values, fields, comments


def sigil_streams(text: str):
    code, comments = split_comments(text)
    t = tokens(code)
    values, fields = [], []
    for i, (kind, s) in enumerate(t):
        nxt = t[i + 1][1] if i + 1 < len(t) else ""
        if kind == "id" and nxt == "=":
            if s not in CONTAINERS:
                fields.append(s)
        elif kind == "id" and s in SIGIL_KEYWORDS:
            pass
        elif kind == "id":
            values.append(("v", s))
        elif kind == "str":
            values.append(("v", norm_str(s)))
        elif kind == "qty":
            n, u = split_qty(s)
            values.append(("q", n, u))
        elif kind == "num":
            values.append(("n", float(s)))
    return values, fields, comments


def first_diff(a: list, b: list) -> str:
    for i, (x, y) in enumerate(zip(a, b)):
        if x != y:
            return f"index {i}: ron {x!r} vs sigil {y!r}; ron context {a[max(0, i - 3):i + 3]}"
    return f"length ron {len(a)} vs sigil {len(b)}"


def readability(stem: str, text: str) -> list[str]:
    problems = []
    sil = re.findall(r"^\s*silhouette = (\w+)", text, re.M)
    if len(sil) != len(set(sil)):
        problems.append(f"silhouettes not pairwise distinct: {sil}")
    for pal in re.findall(r"^\s*palette = ([\w.]+)", text, re.M):
        if not pal.startswith("enemy."):
            problems.append(f"palette outside enemy space: {pal}")
    return problems


def main() -> int:
    failed = False
    coverage: dict[str, set[str]] = {}
    for stem in PATTERNS:
        ron = (ROOT / "ron" / f"{stem}.ron").read_text(encoding="utf-8")
        sig = (ROOT / "sigil" / f"{stem}.sigil").read_text(encoding="utf-8")
        rv, rf, rc = ron_streams(ron)
        sv, sf, sc = sigil_streams(sig)
        pattern_ok = True
        for label, a, b in (("values", rv, sv), ("fields", rf, sf), ("comments", rc, sc)):
            if a != b:
                pattern_ok = False
                print(f"FAIL {stem} {label}: {first_diff(a, b)}")
        for p in readability(stem, sig):
            pattern_ok = False
            print(f"FAIL {stem} readability: {p}")
        failed = failed or not pattern_ok
        if pattern_ok:
            print(f"ok   {stem}: {len(sv)} values, {len(sf)} fields, {len(sc)} comments")
        feats = set()
        feats |= {f"block:{k}" for k in re.findall(r"^\s*block (\w+) \{", sig, re.M)}
        feats |= {f"modifier:{k}" for k in re.findall(r"^\s*modifier (\w+) \{", sig, re.M)}
        feats |= {f"transform:{k}" for k in re.findall(r"^\s*transform (\w+) \{", sig, re.M)}
        feats |= {f"trigger:{k}" for k in re.findall(r"when = (time|distance|event) ", sig)}
        for fl in re.findall(r"flags = \[([^\]]*)\]", sig):
            feats |= {f"flag:{x.strip()}" for x in fl.split(",") if x.strip()}
        for key in ("despawn_vfx", "behaviour", "patron", "offset", "role"):
            if re.search(rf"^\s*{key} = ", sig, re.M):
                feats.add(f"field:{key}")
        if re.search(r"^import ", sig, re.M):
            feats.add("composition:import")
        if re.search(r"^emitter \w+ from ", sig, re.M):
            feats.add("composition:override")
        if "repeat = forever" in sig:
            feats.add("field:repeat_forever")
        for f in feats:
            coverage.setdefault(f, set()).add(stem[:2])
    required = (
        [f"block:{k}" for k in ("ring", "spiral", "fan", "aimed", "wave", "line", "scatter")]
        + [f"modifier:{k}" for k in ("speed_curve", "rotate", "accelerate", "curve",
                                     "sine_offset", "mirror")]
        + [f"transform:{k}" for k in ("burst", "change_type", "reverse", "become_emitter")]
        + [f"trigger:{k}" for k in ("time", "distance", "event")]
        + [f"flag:{k}" for k in ("smashable", "reflectable", "env_active", "grazeable")]
        + ["field:despawn_vfx", "field:behaviour", "field:patron", "composition:import",
           "composition:override"]
    )
    print("\ncoverage:")
    for f in required + sorted(set(coverage) - set(required)):
        where = ", ".join(sorted(coverage.get(f, set()))) or "MISSING"
        if where == "MISSING":
            failed = True
        print(f"  {f:<28} {where}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
