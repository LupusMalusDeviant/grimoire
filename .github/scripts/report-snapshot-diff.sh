#!/usr/bin/env bash
# Plan 0002 WP2.8: meldet je Runner das Ergebnis der drei deterministischen Render-Testszenen
# (`pbr_materials`, `shadows`, `camera_tilt`) in den GitHub-Job-Summary — im Warnmodus: eine
# Abweichung über der Toleranz oder eine fehlende Referenz lässt den Job nicht rot werden, steht
# hier aber unübersehbar.
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
# Beendet sich immer mit Status 0: dieses Skript selbst darf den Lauf nie röten, das ist gerade der
# Punkt des Warnmodus (siehe snapshot_scenes.rs, Abschnitt "Warning mode", und wie er verbindlich
# wird).
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

{
  echo "## WP2.8 Render-Testszenen (${label})"
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

if [ -z "$diff_lines" ] && [ -z "$no_reference_lines" ] && [ -z "$updated_lines" ]; then
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
    if [ "$within" = "true" ]; then
      status="OK"
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
hochgeladen. Warnmodus: sieh \`tests/snapshot_scenes.rs\` (\"Warning mode\") für die Umstellung \
auf einen roten Lauf."
      echo
    } >> "$summary"
  fi
fi

if [ -n "$no_reference_lines" ]; then
  {
    echo "Noch keine Referenz für diesen Runner (erwartet auf Plattformen, auf denen dieser \
Änderungssatz keine Referenz mitliefert — siehe \`tests/snapshot_scenes.rs\`, Abschnitt \
\"References per platform\"):"
    echo '```'
    printf '%s\n' "$no_reference_lines"
    echo '```'
    echo "Kandidatenbilder liegen im Artefakt \`snapshot-candidates-${label}\`, falls hochgeladen; \
zur Übernahme als Referenz nach \`crates/grimoire_render/tests/snapshots/<plattform>/\` kopieren \
und committen."
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
