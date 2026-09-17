#!/usr/bin/env bash
# Plan 0002 WP2.8: meldet je Runner das Ergebnis der deterministischen Render-Testszenen
# (`pbr_materials`, `shadows`, `camera_tilt`, seit WP3.6 `lights_256` und `bullets_on_top`, seit
# WP6.4 auch `overlay` aus crates/grimoire/tests/overlay_scene.rs) in den GitHub-Job-Summary. Seit
# M2 blockieren Windows und Linux: eine Abweichung über der Toleranz oder eine fehlende Referenz
# lässt dort schon den Testschritt scheitern; macOS bleibt bis P3 im Warnmodus. Dieses Skript
# fasst in beiden Fällen zusammen, was die Tests gemeldet haben, und nennt die Host-CPU des Runners
# (lavapipe erzeugt seinen Code passend zur CPU, siehe OF-18.2).
#
# crates/grimoire_render/tests/snapshot_scenes.rs schreibt dafür greppbare Zeilen direkt auf
# stdout (an libtest vorbei, wie schon die WP2.1-Adapterzeile in tests/offscreen.rs):
#   grimoire-snapshot-diff: name=... platform=... mean_abs_diff=... max_abs_diff=... \
#     mean_tolerance=... max_tolerance=... within_tolerance=true|false
#   grimoire-snapshot-no-reference: name=... platform=... candidate=<pfad>
#   grimoire-snapshot-updated: name=... path=<pfad>   (nur bei lokalem GRIMOIRE_SNAPSHOT_UPDATE=1)
#
# Usage: report-snapshot-diff.sh <runner-label> <logdatei>...
# Fehlende Logdateien werden übersprungen (z. B. ein Runner ohne den Software-Adapter-Schritt).
# Beendet sich immer mit Status 0: ob ein Lauf rot wird, entscheiden die Tests selbst (siehe
# snapshot_scenes.rs, Abschnitt "Blocking per platform"); dieses Skript berichtet nur.
set -uo pipefail

label="${1:?Runner-Label fehlt}"
shift

logs=()
for log in "$@"; do
  if [ -f "$log" ]; then
    logs+=("$log")
  fi
done

summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

# Host-CPU des Runners, soweit das Betriebssystem sie ohne Zusatzwerkzeug nennt.
host_cpu() {
  if [ -r /proc/cpuinfo ]; then
    sed -n 's/^model name[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | head -n1
  elif command -v sysctl >/dev/null 2>&1; then
    sysctl -n machdep.cpu.brand_string 2>/dev/null
  else
    echo "${PROCESSOR_IDENTIFIER:-}"
  fi
}

# Blockierende Plattformen, wie `BLOCKING_PLATFORMS` in crates/grimoire_render/tests/support/mod.rs.
blocking_platform() {
  case "$1" in
    windows | linux) return 0 ;;
    *) return 1 ;;
  esac
}

cpu="$(host_cpu | sed 's/[[:space:]]*$//')"
echo "grimoire-snapshot-host-cpu: runner=${label} cpu=[${cpu:-unbekannt}]"
{
  echo "## WP2.8 Render-Testszenen (${label})"
  echo
  echo "Host-CPU: ${cpu:-unbekannt}"
  echo
} >> "$summary"

if [ "${#logs[@]}" -eq 0 ]; then
  {
    echo "Kein Log gefunden."
    echo
  } >> "$summary"
  exit 0
fi

diff_lines="$(grep -h -o 'grimoire-snapshot-diff:.*' -- "${logs[@]}" 2>/dev/null | sort -u || true)"
no_reference_lines="$(grep -h -o 'grimoire-snapshot-no-reference:.*' -- "${logs[@]}" 2>/dev/null | sort -u || true)"
updated_lines="$(grep -h -o 'grimoire-snapshot-updated:.*' -- "${logs[@]}" 2>/dev/null | sort -u || true)"
variance_lines="$(grep -h -o 'grimoire-snapshot-variance:.*' -- "${logs[@]}" 2>/dev/null | sort -u || true)"

if [ -z "$diff_lines" ] && [ -z "$no_reference_lines" ] && [ -z "$updated_lines" ] && [ -z "$variance_lines" ]; then
  {
    echo "Keine WP2.8-Snapshot-Zeilen im Log gefunden (Adapter nicht verfügbar, oder die Szenen \
liefen auf diesem Runner nicht mit \`--ignored\`)."
    echo
  } >> "$summary"
  exit 0
fi

if [ -n "$diff_lines" ]; then
  {
    echo "| Szene | Status | mean_abs_diff | max_abs_diff | Toleranz (mean / max) |"
    echo "| --- | --- | --- | --- | --- |"
  } >> "$summary"
  mismatch_count=0
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    name="$(sed -n 's/.*name=\([^ ]*\).*/\1/p' <<<"$line")"
    mean="$(sed -n 's/.*mean_abs_diff=\([^ ]*\).*/\1/p' <<<"$line")"
    max="$(sed -n 's/.*max_abs_diff=\([^ ]*\).*/\1/p' <<<"$line")"
    mean_tol="$(sed -n 's/.*mean_tolerance=\([^ ]*\).*/\1/p' <<<"$line")"
    max_tol="$(sed -n 's/.*max_tolerance=\([^ ]*\).*/\1/p' <<<"$line")"
    within="$(sed -n 's/.*within_tolerance=\([^ ]*\).*/\1/p' <<<"$line")"
    platform="$(sed -n 's/.*platform=\([^ ]*\).*/\1/p' <<<"$line")"
    if [ "$within" = "true" ]; then
      status="OK"
    elif blocking_platform "$platform"; then
      status="**MISMATCH** (blockierend, Lauf rot)"
      mismatch_count=$((mismatch_count + 1))
    else
      status="**MISMATCH** (Warnmodus, Lauf bleibt grün)"
      mismatch_count=$((mismatch_count + 1))
    fi
    echo "| ${name} | ${status} | ${mean} | ${max} | ${mean_tol} / ${max_tol} |" >> "$summary"
  done <<<"$diff_lines"
  echo >> "$summary"
  if [ "$mismatch_count" -gt 0 ]; then
    {
      echo "${mismatch_count} Szene(n) über der Toleranz — Kandidatenbild und \
Referenz-neben-Kandidat-Streifen liegen im Artefakt \`snapshot-candidates-${label}\`, falls \
hochgeladen. Windows und Linux blockieren, macOS warnt nur: sieh \`tests/snapshot_scenes.rs\` \
(\"Blocking per platform\")."
      echo
    } >> "$summary"
  fi
fi

if [ -n "$no_reference_lines" ]; then
  {
    echo "Noch keine Referenz für diesen Runner. Unter Windows und Linux lässt das den Testschritt \
scheitern, unter macOS nur warnen (siehe \`tests/snapshot_scenes.rs\`, Abschnitte \
\"Blocking per platform\" und \"References per platform\"):"
    echo '```'
    printf '%s\n' "$no_reference_lines"
    echo '```'
    echo "Kandidatenbilder liegen im Artefakt \`snapshot-candidates-${label}\`, falls hochgeladen; \
zur Übernahme als Referenz nach \`crates/grimoire_render/tests/snapshots/<plattform>/\` kopieren \
und committen."
    echo
  } >> "$summary"
fi

# Plan 0002 WP3.6 (OF-18.2): run-to-run variance per scene on this runner's adapter and, off
# Windows, the difference to the Windows (WARP) reference (`scene_variance` in snapshot_scenes.rs).
if [ -n "$variance_lines" ]; then
  {
    echo "### OF-18.2: Streuung je Adapter"
    echo
    echo "| Szene | Läufe | Lauf-zu-Lauf mean / max | gegen Windows-Referenz |"
    echo "| --- | --- | --- | --- |"
  } >> "$summary"
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    name="$(sed -n 's/^grimoire-snapshot-variance: name=\([^ ]*\).*/\1/p' <<<"$line")"
    if grep -q 'skipped=' <<<"$line"; then
      echo "| ${name} | – | kein Adapter | – |" >> "$summary"
      continue
    fi
    runs="$(sed -n 's/.*runs=\([^ ]*\).*/\1/p' <<<"$line")"
    run_mean="$(sed -n 's/.*run_mean_abs_diff=\([^ ]*\).*/\1/p' <<<"$line")"
    run_max="$(sed -n 's/.*run_max_abs_diff=\([^ ]*\).*/\1/p' <<<"$line")"
    if grep -q 'vs_windows_mean_abs_diff=' <<<"$line"; then
      vs_mean="$(sed -n 's/.*vs_windows_mean_abs_diff=\([^ ]*\).*/\1/p' <<<"$line")"
      vs_max="$(sed -n 's/.*vs_windows_max_abs_diff=\([^ ]*\).*/\1/p' <<<"$line")"
      versus="${vs_mean} / ${vs_max}"
    else
      versus="$(sed -n 's/.*vs_windows=\([^ ]*\).*/\1/p' <<<"$line")"
    fi
    echo "| ${name} | ${runs} | ${run_mean} / ${run_max} | ${versus} |" >> "$summary"
  done <<<"$variance_lines"
  adapter="$(sed -n 's/.*adapter=\[\(.*\)\]$/\1/p' <<<"$(head -n1 <<<"$variance_lines")")"
  {
    echo
    echo "Adapter: ${adapter:-unbekannt}"
    echo
  } >> "$summary"
fi

if [ -n "$updated_lines" ]; then
  {
    echo "Hinweis: \`GRIMOIRE_SNAPSHOT_UPDATE=1\` war gesetzt (sollte in CI nie vorkommen):"
    echo '```'
    printf '%s\n' "$updated_lines"
    echo '```'
    echo
  } >> "$summary"
fi

exit 0
