#!/usr/bin/env bash
# Plan 0002 WP4.4, platform identity gate: compares the output of build-sigil-units.sh from every
# operating system byte for byte and writes the outcome to the GitHub job summary
# (docs/formats/sigil.md §15). Any difference fails the step: a differing or unreadable file, a file
# one platform has and another lacks, a platform without units, and fewer platforms than expected
# when the test jobs succeeded (a lost artifact rather than a red test job).
#
# Usage: compare-sigil-units.sh <download-dir> <label>
#   <download-dir>/<platform>/ holds one platform's artifact (the workflow downloads each by name
#   into its own directory); every file below it is compared.
#
# Environment:
#   TESTS_RESULT        result of the test jobs (success, failure, cancelled, skipped).
#   EXPECTED_PLATFORMS  platforms that must deliver units (default 3).
set -euo pipefail
export LC_ALL=C

dir="${1:?download directory missing}"
label="${2:-}"
summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
tests_result="${TESTS_RESULT:-}"
expected="${EXPECTED_PLATFORMS:-3}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failed=0

platforms=()
if [ -d "$dir" ]; then
  while IFS= read -r platform; do
    platforms+=("$platform")
  done < <(find "$dir" -mindepth 1 -maxdepth 1 -type d -exec basename {} \; | sort)
fi

{
  echo "## Sigil-Units ${label}"
  echo
} >> "$summary"

# One sorted file list per platform; a platform without a single unit is an error.
usable=()
for platform in ${platforms[@]+"${platforms[@]}"}; do
  (cd "$dir/$platform" && find . -type f | sed 's|^\./||' | sort) > "$work/$platform.list"
  if grep -q '\.unit$' "$work/$platform.list"; then
    usable+=("$platform")
  else
    echo "::error title=Sigil-Units fehlen::${platform} hat keine einzige .unit-Datei geliefert."
    failed=1
  fi
done
count="${#usable[@]}"

if [ "$count" -lt "$expected" ]; then
  message="Nur ${count} von ${expected} Plattformen haben Sigil-Units geliefert."
  if [ "$tests_result" = "success" ]; then
    message="${message} Alle Testjobs waren gruen, also ging ein Artefakt verloren (Build-Schritt oder Upload pruefen)."
    echo "::error title=Sigil-Units unvollstaendig::${message}"
    failed=1
  else
    message="${message} Testjobs: ${tests_result:-unbekannt}; das fehlende Artefakt folgt vermutlich aus dem roten Testjob."
    echo "::warning title=Sigil-Units unvollstaendig::${message}"
  fi
  echo "**Unvollstaendig.** ${message}" >> "$summary"
  echo >> "$summary"
fi

if [ "$count" -eq 0 ]; then
  if [ "$failed" -ne 0 ]; then
    echo "**Ergebnis: fehlgeschlagen.**" >> "$summary"
  fi
  exit "$failed"
fi

reference="${usable[0]}"
differences=0
for platform in "${usable[@]:1}"; do
  missing="$(comm -23 "$work/$reference.list" "$work/$platform.list")"
  extra="$(comm -13 "$work/$reference.list" "$work/$platform.list")"
  if [ -n "$missing" ]; then
    echo "::error title=Sigil-Units abweichend::${platform} fehlen Dateien, die ${reference} hat: $(echo "$missing" | tr '\n' ' ')"
    differences=$((differences + $(echo "$missing" | wc -l)))
  fi
  if [ -n "$extra" ]; then
    echo "::error title=Sigil-Units abweichend::${platform} hat Dateien, die ${reference} nicht hat: $(echo "$extra" | tr '\n' ' ')"
    differences=$((differences + $(echo "$extra" | wc -l)))
  fi
  while IFS= read -r file; do
    if ! cmp -s "$dir/$reference/$file" "$dir/$platform/$file"; then
      detail="$(cmp "$dir/$reference/$file" "$dir/$platform/$file" 2>&1 | head -n 1 || true)"
      echo "::error title=Sigil-Units abweichend::${file}: ${reference} und ${platform} unterscheiden sich (${detail})."
      differences=$((differences + 1))
    fi
  done < <(comm -12 "$work/$reference.list" "$work/$platform.list")
done

files="$(wc -l < "$work/$reference.list" | tr -d ' ')"
units="$(grep -c '\.unit$' "$work/$reference.list" || true)"
if [ "$differences" -eq 0 ]; then
  message="Alle ${files} Dateien (${units} Units, dazu die JSON-Berichte) sind auf ${count} Plattformen byte-gleich: $(printf '%s ' "${usable[@]}")"
  echo "**Identisch.** ${message}" >> "$summary"
  echo "::notice title=Sigil-Units identisch::${message}"
else
  message="${differences} Abweichung(en) zwischen den Plattformen; der Compiler erzeugt nicht ueberall dieselben Bytes (Plan 0002 WP4.4)."
  echo "**Abweichend.** ${message}" >> "$summary"
  echo "::error title=Sigil-Units abweichend::${message}"
  failed=1
fi

{
  echo
  echo "<details><summary>SHA-256 je Datei (${reference})</summary>"
  echo
  echo "| Datei | Bytes | SHA-256 |"
  echo "|---|---|---|"
  while IFS= read -r file; do
    size="$(wc -c < "$dir/$reference/$file" | tr -d ' ')"
    hash="$(sha256sum "$dir/$reference/$file" | cut -d ' ' -f 1)"
    echo "| \`${file}\` | ${size} | \`${hash}\` |"
  done < "$work/$reference.list"
  echo
  echo "</details>"
} >> "$summary"

if [ "$failed" -ne 0 ]; then
  {
    echo
    echo "**Ergebnis: fehlgeschlagen.** Siehe die Fehler oben."
  } >> "$summary"
fi
exit "$failed"
