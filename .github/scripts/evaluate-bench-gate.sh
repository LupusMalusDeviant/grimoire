#!/usr/bin/env bash
# Benchmark-Gate-Urteil (Plan 0002 WP6.2, M2; Engine-ADR-0010 §4.3): ruft `compare` auf und
# übersetzt dessen Exitcode in das Ergebnis des CI-Schritts. Der Job `gate` in
# .github/workflows/bench-gate.yml benutzt dieses Skript ebenso wie der Job `regression-proof`,
# der damit belegt, dass eine echte Regression genau diesen Schritt bricht.
#
# Exitcodes von `compare`: 0 grün oder Warnung, 2 Messfehler, 3 mindestens ein Szenario rot.
#
# Usage: evaluate-bench-gate.sh <kandidat.jsonl> <basis.jsonl> <warn|hard> <compare-binary>
# Exitcodes dieses Skripts:
#   0  der Schritt besteht (grün, Warnung, oder rot im Modus `warn`)
#   1  Messfehler (compare 2) oder ein unerwarteter Exitcode, in jedem Modus
#   3  rotes Urteil im Modus `hard`
# Schreibt die Tabelle von `compare` in $GITHUB_STEP_SUMMARY (falls gesetzt) und `code=<n>` nach
# $GITHUB_OUTPUT (falls gesetzt).
set -uo pipefail

candidate="${1:?Kandidatendatei fehlt}"
basis="${2:?Basisdatei fehlt}"
mode="${3:?Modus fehlt (warn oder hard)}"
compare_bin="${4:?Pfad zu compare fehlt}"

case "$mode" in
  warn | hard) ;;
  *)
    echo "::error title=Ungültiger Gate-Modus::BENCH_GATE_MODE ist '${mode}', erlaubt sind warn und hard."
    exit 1
    ;;
esac

summary="${GITHUB_STEP_SUMMARY:-/dev/null}"

BENCH_GATE_MODE="$mode" "$compare_bin" --candidate "$candidate" --basis "$basis" | tee -a "$summary"
code="${PIPESTATUS[0]}"
echo "compare exit code: ${code} (mode: ${mode})"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  echo "code=${code}" >>"$GITHUB_OUTPUT"
fi

case "$code" in
  0)
    exit 0
    ;;
  2)
    echo "::error title=Messfehler::compare meldete einen Messfehler (Exitcode 2); das gilt unabhängig vom Modus immer als Fehlschlag."
    exit 1
    ;;
  3)
    if [[ "$mode" == "hard" ]]; then
      echo "::error title=Regressions-Gate (HARD MODE)::compare meldete Rot (Exitcode 3); HARD MODE lässt den Job fehlschlagen."
      exit 3
    fi
    echo "::warning title=Regressions-Gate (WARN MODE)::compare meldete Rot (Exitcode 3); WARN MODE lässt den Job trotzdem grün."
    exit 0
    ;;
  *)
    echo "::error title=Unerwarteter Exitcode::compare endete mit ${code}; das Gate kann kein Urteil bilden."
    exit 1
    ;;
esac
