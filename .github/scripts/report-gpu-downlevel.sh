#!/usr/bin/env bash
# Downlevel-Pruefung (WP3.1, Vorbedingung fuer WP3.4): meldet je Runner die gemessenen Features,
# Downlevel-Flags und Limits des gewaehlten Adapters, sowie wie viele WP3.1-Tests mangels Adapter
# oder Faehigkeit uebersprungen wurden, in den GitHub-Job-Summary.
#
# crates/grimoire_gpu/tests/downlevel.rs schreibt dafuer greppbare Zeilen direkt auf stdout (an
# libtest vorbei, wie schon WP2.1s Adapterzeile in crates/grimoire_render/tests/offscreen.rs):
#   grimoire-gpu-features: <wgpu::Features Debug>                              (einmal je Testbinary)
#   grimoire-gpu-downlevel: shader_model=... fragment_storage=... \
#     fragment_writable_storage=... compute_shaders=... vertex_storage=...     (einmal je Testbinary)
#   grimoire-gpu-limits: max_storage_buffers_per_shader_stage=... ...          (einmal je Testbinary)
#   grimoire-gpu-downlevel-tests-skipped: <n>                                  (laufende Summe je Skip)
#
# Der Skip-Zaehler traegt bewusst ein eigenes Praefix (nicht WP2.1s grimoire-gpu-tests-skipped):
# zwei Testbinaries mit demselben, je Binary bei 1 neu startenden Zaehler wuerden sich sonst nicht
# addieren, sondern das Maximum der beiden Reihen ueberschreiben (siehe tests/downlevel.rs).
#
# Usage: report-gpu-downlevel.sh <runner-label> <logdatei>...
# Fehlende Logdateien werden uebersprungen (der macOS-Lauf hat z. B. keinen Software-Adapter-Schritt).
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

features=""
downlevel=""
limits=""
skipped=0
if [ "${#logs[@]}" -gt 0 ]; then
  features="$(grep -h -o 'grimoire-gpu-features:.*' -- "${logs[@]}" | sort -u || true)"
  downlevel="$(grep -h -o 'grimoire-gpu-downlevel:.*' -- "${logs[@]}" | sort -u || true)"
  limits="$(grep -h -o 'grimoire-gpu-limits:.*' -- "${logs[@]}" | sort -u || true)"
  max="$(grep -h -o 'grimoire-gpu-downlevel-tests-skipped: [0-9]\+' -- "${logs[@]}" | grep -o '[0-9]\+' | sort -n | tail -n1 || true)"
  if [ -n "${max:-}" ]; then
    skipped="$max"
  fi
fi

{
  echo "## WP3.1 Downlevel-Faehigkeiten (${label})"
  echo
  if [ -z "$downlevel" ] && [ -z "$limits" ]; then
    echo "Kein Faehigkeitsbericht gefunden (erwartet auf macOS-Runnern ohne Metal-Software-Adapter, \
falls zusaetzlich der Hardware-Fallback fehlschlaegt)."
  else
    echo '```'
    printf '%s\n' "$downlevel"
    printf '%s\n' "$limits"
    printf '%s\n' "$features"
    echo '```'
  fi
  echo
  echo "Uebersprungene WP3.1-Tests mangels Adapter/Faehigkeit: ${skipped}"
  echo
} >> "$summary"
