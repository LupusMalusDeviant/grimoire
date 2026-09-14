#!/usr/bin/env bash
# Compares the std-trig float probes of all operating systems and writes the outcome to the
# GitHub job summary. A divergence is informational (input for OF-2.1), never a failure:
# the golden assertions inside the test suite are what fail a run.
#
# Usage: compare-float-probes.sh <download-dir> <label>
#   <download-dir> contains one sub-directory per artifact (float-probe-<os>/...),
#   as produced by actions/download-artifact with a pattern and without merge-multiple.
set -euo pipefail

dir="${1:?download directory missing}"
label="${2:-}"
summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

shopt -s nullglob

declare -A hashes=()
rows=""

for file in "$dir"/*/std-trig-*.txt; do
  artifact="$(basename "$(dirname "$file")")"
  target="$(basename "$file" .txt)"
  target="${target#std-trig-}"
  value="$(sed -n 's/^std_trig_hash=\(0x[0-9A-Fa-f]\{1,\}\)[[:space:]]*$/\1/p' "$file" | head -n 1)"
  if [ -z "$value" ]; then
    value="unlesbar"
    echo "::warning title=Float-Probe unlesbar::${file} enthaelt keine Zeile std_trig_hash=0x..."
  fi
  hashes["$target"]="$value"
  rows+="| ${target} | \`${value}\` | ${artifact#float-probe-} |"$'\n'
done

count="${#hashes[@]}"

{
  echo "## Float-Probe ${label}"
  echo
} >> "$summary"

if [ "$count" -eq 0 ]; then
  echo "Keine std-trig-Proben gefunden. Entweder hat kein Testlauf Proben geschrieben oder alle Testjobs sind vorher gescheitert." >> "$summary"
  echo "::notice title=Float-Probe::Keine std-trig-Proben gefunden, Vergleich uebersprungen."
  exit 0
fi

{
  echo "| Ziel (OS-Arch) | std_trig_hash | Runner |"
  echo "|---|---|---|"
  printf '%s' "$rows"
  echo
} >> "$summary"

distinct="$(printf '%s\n' "${hashes[@]}" | sort -u | wc -l | tr -d ' ')"

if [ "$distinct" -eq 1 ]; then
  first="$(printf '%s\n' "${hashes[@]}" | head -n 1)"
  message="Alle ${count} gelieferten Proben haben denselben std_trig_hash (${first})."
  echo "**Ergebnis: identisch.** ${message}" >> "$summary"
  echo "::notice title=Float-Probe identisch::${message}"
else
  message="std_trig_hash weicht ab: ${distinct} verschiedene Werte bei ${count} Plattformen. Informativ, kein Fehler - Eingabe fuer OF-2.1 (f32 vs. Fixed-Point)."
  echo "**Ergebnis: abweichend.** ${message}" >> "$summary"
  echo "::notice title=Float-Probe-Divergenz::${message}"
fi

if [ "$count" -lt 3 ]; then
  echo >> "$summary"
  echo "Hinweis: nur ${count} von 3 Plattformen haben Proben geliefert (Testjob gescheitert oder Probe fehlt)." >> "$summary"
  echo "::warning title=Float-Probe unvollstaendig::Nur ${count} von 3 Plattformen haben std-trig-Proben geliefert."
fi

# Raw probe files for manual inspection.
{
  echo
  echo "<details><summary>Rohdaten aller Proben</summary>"
  echo
  for file in "$dir"/*/*.txt; do
    echo "**$(basename "$(dirname "$file")")/$(basename "$file")**"
    echo
    echo '```text'
    cat "$file"
    echo
    echo '```'
  done
  echo "</details>"
} >> "$summary"
