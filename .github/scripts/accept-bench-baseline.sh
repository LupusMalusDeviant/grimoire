#!/usr/bin/env bash
# Writes a new accepted basis for the benchmark gate (Plan-0002 WP6.2 scope item 5; engine
# ADR-0010 §4.2: "gegen eine akzeptierte Basis... nie den letzten Push"). Run only by the
# `bench-accept-baseline` workflow (`workflow_dispatch`): the accepted basis changes only through
# this explicit, documented step, never automatically from a push to main.
#
# Copies the `instructions`-metric BenchResult lines for the given commit SHA out of
# `trend/results.jsonl` (already on the bench-trends branch, appended by every main push by
# `.github/scripts/append-bench-trend.sh`) into `accepted-basis.jsonl`, replacing it wholesale.
# `wall_time` lines are never part of the basis (trend only, never gated, engine ADR-0010).
#
# Usage (run with cwd inside a checkout of the bench-trends branch): accept-bench-baseline.sh <sha>
set -euo pipefail

SHA="${1:?commit sha to accept as the new basis}"
TREND_FILE="trend/results.jsonl"
BASIS_FILE="accepted-basis.jsonl"

# Defense in depth: the calling workflow already validates this against the same pattern before
# ever reaching this script, but this script must not trust that unconditionally — it is the one
# place that actually builds accepted-basis.jsonl, and a future caller (or a manual invocation)
# might skip that check.
if ! [[ "$SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error title=Ungueltige SHA::erwartet genau 40 Kleinbuchstaben-Hex-Zeichen, erhalten: ${SHA}"
  exit 1
fi

if [[ ! -f "$TREND_FILE" ]]; then
  echo "::error title=Keine Trenddaten::${TREND_FILE} existiert nicht auf bench-trends; noch kein main-Push mit dem Gate gelaufen?"
  exit 1
fi

# One line per scenario for this SHA, instructions metric only; the last matching line in the
# file wins (a commit should only be measured once, but this stays correct if a run was ever
# re-triggered and appended twice).
python3 - "$SHA" "$TREND_FILE" "$BASIS_FILE" <<'PYEOF'
import json
import sys

sha, trend_file, basis_file = sys.argv[1:4]
by_scenario = {}
with open(trend_file, encoding="utf-8") as handle:
    for line in handle:
        line = line.strip()
        if not line:
            continue
        record = json.loads(line)
        if record.get("commit", {}).get("sha") != sha:
            continue
        if record.get("metric") != "instructions":
            continue
        by_scenario[record["scenario"]] = line

if not by_scenario:
    print(f"no instructions-metric results found for commit {sha} in {trend_file}", file=sys.stderr)
    sys.exit(1)

with open(basis_file, "w", encoding="utf-8", newline="\n") as handle:
    for scenario in sorted(by_scenario):
        handle.write(by_scenario[scenario] + "\n")

print(f"wrote {len(by_scenario)} accepted-basis line(s) for commit {sha} to {basis_file}")
PYEOF

cat "$BASIS_FILE"
