#!/usr/bin/env bash
# CI-Render-Ehrlichkeit (WP2.1, Vorstufe OF-18.2): meldet je Runner, welchen Adapter die
# Offscreen-/GPU-Tests tatsächlich benutzt haben, und wie viele davon mangels Adapter übersprungen
# wurden, in den GitHub-Job-Summary.
#
# crates/grimoire_render/tests/offscreen.rs schreibt dafür zwei greppbare Zeilen direkt auf stdout
# (an libtest vorbei, siehe Kommentar dort):
#   grimoire-gpu-adapter: name=... backend=... device_type=... driver=...   (einmal je Testbinary)
#   grimoire-gpu-tests-skipped: <n>                                        (laufende Summe je Skip)
#
# Usage: report-gpu-adapter.sh <runner-label> <logdatei>...
# Fehlende Logdateien werden übersprungen (der macOS-Lauf hat z. B. keinen Software-Adapter-Schritt).
set -euo pipefail

label="${1:?Runner-Label fehlt}"
shift

logs=()
for log in "$@"; do
  if [ -f "$log" ]; then
    logs+=("$log")
  fi
done

summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

adapters=""
skipped=0
if [ "${#logs[@]}" -gt 0 ]; then
  adapters="$(grep -h -o 'grimoire-gpu-adapter:.*' -- "${logs[@]}" | sort -u || true)"
  max="$(grep -h -o 'grimoire-gpu-tests-skipped: [0-9]\+' -- "${logs[@]}" | grep -o '[0-9]\+' | sort -n | tail -n1 || true)"
  if [ -n "${max:-}" ]; then
    skipped="$max"
  fi
fi

{
  echo "## GPU-Adapter (${label})"
  echo
  if [ -z "$adapters" ]; then
    echo "Kein Adapter gefunden (erwartet auf macOS-Runnern ohne Metal-Software-Adapter)."
  else
    echo '```'
    printf '%s\n' "$adapters"
    echo '```'
  fi
  echo
  echo "Übersprungene GPU-Tests mangels Adapter: ${skipped}"
  echo
} >> "$summary"
