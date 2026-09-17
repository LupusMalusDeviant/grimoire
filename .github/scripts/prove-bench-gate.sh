#!/usr/bin/env bash
# Selbsttest des scharfen Benchmark-Gates (Plan 0002 WP6.2 „Negativnachweis“, M2 „Selbsttest-Job
# belegt den Bruch“; Engine-ADR-0010). Läuft im Job `regression-proof` von
# .github/workflows/bench-gate.yml auf echten Callgrind-Messungen desselben Laufs:
#
#   basis.jsonl   ohne Einspeisung gemessen, dient als akzeptierte Basis
#   repeat.jsonl  ein zweites Mal ohne Einspeisung gemessen
#   plus5.jsonl   mit GRIMOIRE_BENCH_INJECT_REGRESSION=1, kalibriert +5 % auf ecs_query_10k und sim_step_600
#   plus15.jsonl  dasselbe mit +15 %
#
# und wertet jede Messung mit genau dem Skript des Jobs `gate` im Modus `hard` aus. Der Nachweis
# scheitert, wenn nicht gilt:
#   plus15  bricht den Lauf (Exit 3), rot sind genau die beiden eingespeisten Szenarien
#   plus5   bricht den Lauf nicht (Exit 0), die beiden Szenarien warnen, alle übrigen grün
#   repeat  bricht den Lauf nicht (Exit 0), alle Szenarien grün (kein Fehlalarm)
#
# Die Ausgaben der Auswertungen stehen im Log, Workflow-Befehle darin sind abgeschaltet, damit ihre
# erwarteten Fehler- und Warnmeldungen nicht als Annotationen dieses grünen Laufs erscheinen.
#
# Usage: prove-bench-gate.sh <compare-binary> <verzeichnis mit den vier jsonl-Dateien>
set -uo pipefail

compare_bin="${1:?Pfad zu compare fehlt}"
dir="${2:?Messverzeichnis fehlt}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

for file in basis repeat plus5 plus15; do
  if [[ ! -s "$dir/$file.jsonl" ]]; then
    echo "::error title=Messung fehlt::$dir/$file.jsonl fehlt oder ist leer."
    exit 1
  fi
done

scenarios="$(grep -c . "$dir/basis.jsonl")"
failures=0

{
  echo "## Selbsttest: scharfes Gate bricht bei echter Regression (Engine-ADR-0010)"
  echo
  echo "| Messung | Exit erwartet | Exit | rot | Warnung | grün | erwartet rot / Warnung / grün | OK |"
  echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
} >>"$summary"

# prove <messung> <exit> <rot> <warnung> <grün>
prove() {
  local name="$1" want_exit="$2" want_red="$3" want_warn="$4" want_green="$5"
  local out="$dir/verdict-$name.txt" token got red warn green ok
  token="proof-$(date +%s%N)"
  GITHUB_STEP_SUMMARY="" GITHUB_OUTPUT="" \
    bash "$script_dir/evaluate-bench-gate.sh" "$dir/$name.jsonl" "$dir/basis.jsonl" hard "$compare_bin" \
    >"$out" 2>&1
  got=$?
  echo "== $name: evaluate-bench-gate.sh (hard) exit $got =="
  echo "::stop-commands::$token"
  cat "$out"
  echo "::$token::"
  red="$(grep -c '| RED |$' "$out" || true)"
  warn="$(grep -c '| warn |$' "$out" || true)"
  green="$(grep -c '| green |$' "$out" || true)"
  if [[ "$got" == "$want_exit" && "$red" == "$want_red" && "$warn" == "$want_warn" && "$green" == "$want_green" ]]; then
    ok="ja"
  else
    ok="**NEIN**"
    failures=$((failures + 1))
    echo "::error title=Selbsttest verfehlt::$name: Exit $got (erwartet $want_exit), rot $red / Warnung $warn / grün $green (erwartet $want_red / $want_warn / $want_green)"
  fi
  echo "| $name | $want_exit | $got | $red | $warn | $green | $want_red / $want_warn / $want_green | $ok |" >>"$summary"
}

prove plus15 3 2 0 $((scenarios - 2))
prove plus5 0 0 2 $((scenarios - 2))
prove repeat 0 0 0 "$scenarios"

echo >>"$summary"
if [[ "$failures" -gt 0 ]]; then
  echo "Selbsttest fehlgeschlagen: $failures Messung(en) ohne das erwartete Urteil." >>"$summary"
  exit 1
fi
echo "Eine eingespeiste Regression von +15 % bricht den Lauf im scharfen Modus; +5 % und eine Wiederholungsmessung brechen ihn nicht." >>"$summary"
