#!/usr/bin/env bash
# Compares the float probes of all operating systems and writes the outcome to the GitHub job
# summary (engine ADR 0004, PRD-0018 FR-04b).
#
# - core-<os>-<arch>.txt (basic_hash, dmath_hash) must be identical on every platform. A divergence
#   or an unreadable core probe fails the step. Fewer core probes than expected fail it too when the
#   test jobs succeeded, because then a probe was lost rather than never written; after a red test
#   job the gap is only a warning, since that job already fails the run.
# - std-trig-<os>-<arch>.txt is informational (input for OF-2.1); a divergence never fails.
#
# Usage: compare-float-probes.sh <download-dir> <label>
#   <download-dir> holds the probe files. The workflows download with merge-multiple: true, so the
#   files lie directly in it (their names carry <os>-<arch> and cannot collide). One level of
#   per-artifact sub-directories is accepted as well: actions/download-artifact without
#   merge-multiple nests only when more than one artifact matches, so both layouts occur.
#
# Environment:
#   TESTS_RESULT        result of the test jobs (success, failure, cancelled, skipped).
#   EXPECTED_PLATFORMS  platforms that must deliver probes (default 3).
set -euo pipefail

dir="${1:?download directory missing}"
label="${2:-}"
summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
tests_result="${TESTS_RESULT:-}"
expected="${EXPECTED_PLATFORMS:-3}"

shopt -s nullglob

failed=0
declare -A core=()
declare -A trig=()
declare -A targets=()

# Prints the value of the line `<key>=0x...` in a probe file, or nothing.
read_hash() {
  sed -n "s/^$1=\(0x[0-9A-Fa-f]\{1,\}\)[[:space:]]*\$/\1/p" "$2" | head -n 1
}

for file in "$dir"/core-*.txt "$dir"/*/core-*.txt; do
  target="$(basename "$file" .txt)"
  target="${target#core-}"
  if [ -n "${core[$target]+set}" ]; then
    echo "::warning title=Float-Probe doppelt::Mehr als eine core-Probe fuer ${target}; ${file} wird ignoriert."
    continue
  fi
  basic="$(read_hash basic_hash "$file")"
  dmath="$(read_hash dmath_hash "$file")"
  if [ -z "$basic" ] || [ -z "$dmath" ]; then
    echo "::error title=Float-Probe unlesbar::${file} enthaelt nicht beide Zeilen basic_hash=0x... und dmath_hash=0x..."
    failed=1
  fi
  core["$target"]="${basic:-unlesbar} ${dmath:-unlesbar}"
  targets["$target"]=1
done

for file in "$dir"/std-trig-*.txt "$dir"/*/std-trig-*.txt; do
  target="$(basename "$file" .txt)"
  target="${target#std-trig-}"
  if [ -n "${trig[$target]+set}" ]; then
    echo "::warning title=Float-Probe doppelt::Mehr als eine std-trig-Probe fuer ${target}; ${file} wird ignoriert."
    continue
  fi
  value="$(read_hash std_trig_hash "$file")"
  if [ -z "$value" ]; then
    value="unlesbar"
    echo "::warning title=Float-Probe unlesbar::${file} enthaelt keine Zeile std_trig_hash=0x..."
  fi
  trig["$target"]="$value"
  targets["$target"]=1
done

core_count="${#core[@]}"
trig_count="${#trig[@]}"

{
  echo "## Float-Probe ${label}"
  echo
} >> "$summary"

if [ "${#targets[@]}" -gt 0 ]; then
  {
    echo "| Ziel (OS-Arch) | basic_hash | dmath_hash | std_trig_hash |"
    echo "|---|---|---|---|"
    while IFS= read -r target; do
      pair="${core[$target]:-- -}"
      echo "| ${target} | \`${pair% *}\` | \`${pair#* }\` | \`${trig[$target]:--}\` |"
    done < <(printf '%s\n' "${!targets[@]}" | sort)
    echo
  } >> "$summary"
fi

# core: must be identical and complete.
if [ "$core_count" -gt 0 ]; then
  distinct="$(printf '%s\n' "${core[@]}" | sort -u | wc -l | tr -d ' ')"
  if [ "$distinct" -eq 1 ]; then
    message="Alle ${core_count} core-Proben haben dieselben basic_hash- und dmath_hash-Werte."
    echo "**core: identisch.** ${message}" >> "$summary"
    echo "::notice title=Float-Probe core identisch::${message}"
  else
    message="core-Proben weichen ab: ${distinct} verschiedene Wertepaare bei ${core_count} Plattformen. Die deterministische Arithmetik ist nicht plattformuebergreifend identisch (ADR-0004)."
    echo "**core: abweichend.** ${message}" >> "$summary"
    echo "::error title=Float-Probe core abweichend::${message}"
    failed=1
  fi
fi

if [ "$core_count" -lt "$expected" ]; then
  message="Nur ${core_count} von ${expected} Plattformen haben core-Proben geliefert."
  if [ "$tests_result" = "success" ]; then
    message="${message} Alle Testjobs waren gruen, also ging eine Probe verloren (Probe-Pfad oder Upload pruefen)."
    echo "::error title=Float-Probe fehlt::${message}"
    failed=1
  else
    message="${message} Testjobs: ${tests_result:-unbekannt}; die fehlende Probe folgt vermutlich aus dem roten Testjob."
    echo "::warning title=Float-Probe unvollstaendig::${message}"
  fi
  {
    echo
    echo "**core: unvollstaendig.** ${message}"
  } >> "$summary"
fi

# std-trig: informational only.
if [ "$trig_count" -gt 0 ]; then
  distinct="$(printf '%s\n' "${trig[@]}" | sort -u | wc -l | tr -d ' ')"
  echo >> "$summary"
  if [ "$distinct" -eq 1 ]; then
    message="Alle ${trig_count} std-trig-Proben haben denselben std_trig_hash."
    echo "**std-trig: identisch.** ${message}" >> "$summary"
    echo "::notice title=Float-Probe std-trig identisch::${message}"
  else
    message="std_trig_hash weicht ab: ${distinct} verschiedene Werte bei ${trig_count} Plattformen. Informativ, kein Fehler - Eingabe fuer OF-2.1 (f32 vs. Fixed-Point)."
    echo "**std-trig: abweichend.** ${message}" >> "$summary"
    echo "::notice title=Float-Probe-Divergenz::${message}"
  fi
fi

if [ "$trig_count" -lt "$expected" ]; then
  echo "::warning title=Float-Probe unvollstaendig::Nur ${trig_count} von ${expected} Plattformen haben std-trig-Proben geliefert."
fi

# Raw probe files for manual inspection.
files=("$dir"/*.txt "$dir"/*/*.txt)
if [ "${#files[@]}" -gt 0 ]; then
  {
    echo
    echo "<details><summary>Rohdaten aller Proben</summary>"
    echo
    for file in "${files[@]}"; do
      echo "**${file#"$dir"/}**"
      echo
      echo '```text'
      cat "$file"
      echo
      echo '```'
    done
    echo "</details>"
  } >> "$summary"
fi

if [ "$failed" -ne 0 ]; then
  {
    echo
    echo "**Ergebnis: fehlgeschlagen.** Siehe die Fehler oben."
  } >> "$summary"
fi
exit "$failed"
