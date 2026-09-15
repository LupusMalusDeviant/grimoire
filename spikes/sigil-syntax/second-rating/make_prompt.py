"""Builds the blind second-rating prompt for spike OF-4.1 (run from spikes/sigil-syntax).

For every error case it shows the rubric, the injected change (diff against the correct pattern)
and the checker output of the prototype; the manual ratings (RATINGS in proto/src/measure.rs) are
not part of the prompt. Needs a built prototype (cargo build in proto/).

    python second-rating/make_prompt.py > second-rating/prompt.md
"""
import difflib
import json
import os
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8")
cases = json.load(open("corpus/errors/expected.json", encoding="utf-8"))
exe = os.path.abspath("proto/target/debug/sigil-syntax-proto" + (".exe" if os.name == "nt" else ""))
print("""You are an independent second rater of compiler diagnostics in a syntax experiment. Do not open,
search or run anything: everything you need is in this prompt. Do not look for other ratings.

Two text syntaxes for the same bullet-pattern data model are compared: "RON" (serde RON with unit
newtypes like `Ticks(20)`, `Deg(12.0)`) and "sigil 1" (own line-oriented grammar, `name = value`,
units as suffix like `20t`, `12deg`, `0.09u/t`). Ten error cases were injected into correct pattern
files, each changing exactly one place. For each case you get the injected change and the complete
diagnostic output of the prototype checker for each syntax (the first `error[...]` line is the
diagnostic's Cause; the `= help:` line is its Fix hint; `= note:` and `= path:` are extra context;
a missing `= help:` line means there is no fix hint).

Rate the FIRST diagnostic of each file on two columns, each 0-3, using exactly this rubric:

- Cause: 0 = none or wrong; 1 = misleading or generic; 2 = names the problem without context;
  3 = names the problem with context (field, kind, counterpart, concrete replacement).
- Fix hint: 0 = none or wrong; 1 = misleading or generic; 2 = names the problem without context;
  3 = names the problem with context (field, kind, counterpart, concrete replacement).

Judge what a modder who made exactly this mistake would learn from the message. Position and node
path are scored automatically elsewhere; do not score them, but a wrong error class counts against
the Cause.

Reply with exactly one Markdown table and nothing else, 20 rows in the order given, columns:
| case | syntax | cause | fix hint | reason (one short sentence) |
with `syntax` written as `RON` or `sigil 1`.
""")
for c in cases:
    print(f"\n## {c['id']}: {c['title_de']} (base pattern {c['base_pattern']})\n")
    for syn, label in (("ron", "RON"), ("sigil", "sigil 1")):
        f = "corpus/" + c[syn]["file"]
        base = f"corpus/{syn}/{c['base_pattern']}.{syn}"
        a = open(base, encoding="utf-8").read().splitlines()
        b = open(f, encoding="utf-8").read().splitlines()
        d = [l for l in difflib.unified_diff(a, b, lineterm="", n=0) if not l.startswith(("---", "+++"))]
        out = subprocess.run([exe, "check", "--dir", f"corpus/{syn}", f],
                             capture_output=True, text=True, encoding="utf-8").stdout
        out = "\n".join(l for l in out.splitlines() if not l.lstrip().startswith("= raw:"))
        print(f"### {c['id']} {label}\n\nInjected change (unified diff against the correct file):\n\n```diff\n"
              + "\n".join(d) + "\n```\n\nChecker output:\n\n```text\n" + out.rstrip() + "\n```\n")
