#!/usr/bin/env bash
# Engine ADR-0008 "Crate-Map-Erweiterung P1", Tooling-Empfehlung; Crate-Verträge §1 last bullet:
# a CI edge check compares the normal, build and dev engine edges of every workspace member
# against a checked-in allowlist that matches the §1 table exactly, plus the documented dev-edge
# exceptions for grimoire_exec. Any edge not covered by the allowlist is reported as a GitHub
# Actions `::error::` annotation (same style as check-thread-source.sh) and fails the run. A
# positive control (grimoire_sim -> grimoire_ecs, normal) guards against the query itself
# breaking silently, mirroring check-thread-source.sh's own rayon positive control.
#
# JSON extraction uses `jq` if it is on PATH, else falls back to `python3 -c ...` (both are
# expected to be present on GitHub's runner images and in this repo's dev environment).
#
# Usage:
#   bash .github/scripts/check-crate-map.sh              # run the real check against this repo
#   bash .github/scripts/check-crate-map.sh --self-test   # run only the built-in self-test cases
set -euo pipefail

# --- Allowed engine edges (Crate-Verträge §1, "Erlaubte Engine-Kanten (normal, Build)" column).
# Applied to normal, build AND dev edges alike: the contract gives no separate, wider dev-edge
# list for these crates (unlike grimoire_exec below), so a dev edge from one of them is held to
# the same list as its normal/build edges.
declare -A FIXED_ALLOW=(
  [grimoire_core]=""
  [grimoire_platform]="grimoire_core"
  [grimoire_gpu]="grimoire_platform grimoire_core"
  [grimoire_render]="grimoire_gpu grimoire_platform grimoire_core"
  [grimoire_ecs]="grimoire_core"
  [grimoire_sim]="grimoire_ecs grimoire_core"
  [grimoire_collide]="grimoire_ecs grimoire_core"
  [grimoire_sigil]="grimoire_sim grimoire_ecs grimoire_core"
  [grimoire_assets]="grimoire_platform grimoire_core"
  [grimoire_debug]="grimoire_platform grimoire_core"
  [grimoire]="grimoire_core grimoire_ecs grimoire_sim grimoire_platform grimoire_render grimoire_collide grimoire_sigil grimoire_assets grimoire_debug"
  [grimoire_sigilc]="grimoire_sigil grimoire_sim grimoire_ecs grimoire_core"
  [grimoire_audio]=""
  [grimoire_ui]=""
)

# grimoire_exec (engine ADR-0006, PO decision V-1): normal edges and dev edges are different
# lists, so it is not part of FIXED_ALLOW. Dev edges to grimoire_sigil/grimoire_collide exist so
# `hash_gate.rs` can include those crates' scenarios via #[path] (Crate-Verträge §1, §11.7).
EXEC_NORMAL="grimoire_ecs"
EXEC_DEV="grimoire grimoire_core grimoire_sim grimoire_sigil grimoire_collide"

# Tool crates (engine ADR-0008): free to depend on any runtime crate (and, for grimoire_bench,
# on grimoire_exec) without a new ADR; only specific tool-to-tool edges are forbidden. Everything
# else reaching this crate as the dependent side is allowed and not checked further here.
LINK_FORBIDDEN="grimoire_bench"
BENCH_FORBIDDEN="grimoire_sigilc grimoire_link"

# contains_word <space-separated words> <needle>
contains_word() {
  local haystack=" $1 " needle="$2"
  [[ "$haystack" == *" $needle "* ]]
}

# check_edge <crate> <kind: normal|build|dev> <dependency>
# Prints a GitHub Actions ::error:: annotation and returns 1 if the edge is not allowed by
# Crate-Verträge §1 / engine ADR-0008; returns 0 silently if it is allowed.
check_edge() {
  local crate="$1" kind="$2" dep="$3"

  case "$crate" in
    grimoire_exec)
      if [ "$kind" = "dev" ]; then
        contains_word "$EXEC_NORMAL $EXEC_DEV" "$dep" && return 0
      else
        contains_word "$EXEC_NORMAL" "$dep" && return 0
      fi
      ;;
    grimoire_link)
      contains_word "$LINK_FORBIDDEN" "$dep" || return 0
      ;;
    grimoire_bench)
      contains_word "$BENCH_FORBIDDEN" "$dep" || return 0
      ;;
    *)
      if [ "${FIXED_ALLOW[$crate]+set}" = "set" ]; then
        contains_word "${FIXED_ALLOW[$crate]}" "$dep" && return 0
      else
        echo "::error title=Kanten-Check unvollständig::${crate} ist ein Workspace-Mitglied ohne Eintrag in der Positivliste von check-crate-map.sh (neues Crate? Skript und Crate-Verträge §1 nachziehen)"
        return 1
      fi
      ;;
  esac

  echo "::error title=Unzulässige Kante::${crate} --${kind}--> ${dep} ist nicht in der Positivliste (Crate-Verträge §1, Engine-ADR-0008)"
  return 1
}

# --- Self-test: a handful of synthetic edges with known verdicts, exercised directly against
# check_edge (no cargo metadata / real repo state involved). Keeps the self-test independent of
# repo state so it still catches a broken allowlist even if the real check above passes.
run_self_test() {
  local failures=0 crate kind dep expect got
  # <crate> <kind> <dependency> <expected: ok|violation>
  local cases=(
    "grimoire_sim|normal|grimoire_ecs|ok"
    "grimoire_core|normal|grimoire_ecs|violation"
    "grimoire_ecs|normal|grimoire_sigilc|violation"
    "grimoire_link|normal|grimoire_bench|violation"
    "grimoire_link|normal|grimoire_sigilc|ok"
    "grimoire_link|normal|grimoire_render|ok"
    "grimoire_bench|dev|grimoire_link|violation"
    "grimoire_bench|dev|grimoire_exec|ok"
    "grimoire_exec|dev|grimoire_sigil|ok"
    "grimoire_exec|normal|grimoire_sim|violation"
    "grimoire_audio|normal|grimoire_core|violation"
    "grimoire_sigilc|normal|grimoire_debug|violation"
    "grimoire_sigilc|normal|grimoire_sigil|ok"
  )
  for case in "${cases[@]}"; do
    IFS='|' read -r crate kind dep expect <<<"$case"
    if check_edge "$crate" "$kind" "$dep" >/dev/null 2>&1; then
      got=ok
    else
      got=violation
    fi
    if [ "$got" = "$expect" ]; then
      echo "ok   ${crate} --${kind}--> ${dep} (expected ${expect})"
    else
      echo "FAIL ${crate} --${kind}--> ${dep}: expected ${expect}, got ${got}"
      failures=$((failures + 1))
    fi
  done
  if [ "$failures" -ne 0 ]; then
    echo "${failures} self-test case(s) failed"
    return 1
  fi
  echo "all self-test cases passed"
  return 0
}

if [ "${1:-}" = "--self-test" ]; then
  run_self_test
  exit $?
fi

# --- Real check against this repo.
if command -v jq >/dev/null 2>&1; then
  JSON_TOOL=jq
elif command -v python3 >/dev/null 2>&1; then
  JSON_TOOL=python3
else
  echo "::error title=Kein JSON-Werkzeug::weder jq noch python3 ist auf PATH; eines von beiden wird für die cargo-metadata-Auswertung gebraucht."
  exit 1
fi

metadata="$(cargo metadata --format-version 1 --locked)"

# Both jq and python3 can emit CRLF line endings on Windows (observed with jq under Git
# Bash/Cygwin); a trailing \r would otherwise survive into $dep and break every string
# comparison in check_edge silently (no jq/cargo error, just an allowlist that never matches).
if [ "$JSON_TOOL" = jq ]; then
  edges="$(jq -r '
    .workspace_members as $wm
    | .packages[]
    | select(.id as $id | $wm | any(. == $id))
    | .name as $pname
    | (.dependencies // [])[]
    | select(.name | startswith("grimoire"))
    | [$pname, (.kind // "normal"), .name] | @tsv
  ' <<<"$metadata" | tr -d '\r')"
else
  edges="$(python3 -c '
import json, sys

data = json.load(sys.stdin)
workspace_members = set(data["workspace_members"])
for pkg in data["packages"]:
    if pkg["id"] not in workspace_members:
        continue
    pname = pkg["name"]
    for dep in pkg.get("dependencies") or []:
        name = dep["name"]
        if not name.startswith("grimoire"):
            continue
        kind = dep.get("kind") or "normal"
        print(f"{pname}\t{kind}\t{name}")
' <<<"$metadata" | tr -d '\r')"
fi

status=0
found_positive_control=0
while IFS=$'\t' read -r crate kind dep; do
  [ -z "$crate" ] && continue
  if [ "$crate" = "grimoire_sim" ] && [ "$kind" = "normal" ] && [ "$dep" = "grimoire_ecs" ]; then
    found_positive_control=1
  fi
  check_edge "$crate" "$kind" "$dep" || status=1
done <<<"$edges"

if [ "$found_positive_control" -ne 1 ]; then
  echo "::error title=Kanten-Abfrage defekt::grimoire_sim -> grimoire_ecs (normal) wurde nicht gefunden; die cargo-metadata-Auswertung liefert offenbar keine Kanten mehr, statt eines stillen Bestehens."
  exit 1
fi

exit "$status"
