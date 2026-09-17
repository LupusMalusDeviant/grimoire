#!/usr/bin/env bash
# Plan 0002 WP4.4, platform identity gate of compiled Sigil units: compiles every Sigil source that
# must compile with this runner's `sigilc` and writes the units plus the JSON reports into one
# directory, which the workflow uploads per operating system. `compare-sigil-units.sh` then requires
# every file to be byte-identical on Windows, Linux and macOS (docs/formats/sigil.md §15).
#
# Contents of <out-dir> (paths are fixed, so the layout is the same on every runner):
#   units/corpus/*.unit      the valid conformance corpus (crates/grimoire_sigilc/tests/corpus/valid)
#   units/reference/*.unit   the reference patterns (crates/grimoire_sigilc/tests/reference)
#   units/facade/fixtures/*.unit
#                            the facade's unit fixtures (crates/grimoire/tests/fixtures), under the
#                            same canonical paths `grimoire_sigilc/tests/unit_fixtures.rs` uses
#   units/bench/fixtures/*.unit
#                            the full-curtain units of `grimoire_bench` (crates/grimoire_bench/fixtures,
#                            plan 0002 WP6.6), likewise under their canonical paths
#   reports/<group>.json     `sigilc build --json` of each group (unit ids, content hashes, sizes)
#   reports/corpus-invalid.json, reports/corpus-schema-invalid.json
#                            `sigilc check --json` of the invalid corpora: their diagnostics must be
#                            platform-independent too, and they must not compile
#
# Usage: build-sigil-units.sh <out-dir>
#   Run from the repository root. <out-dir> is removed first (rust-cache may restore target/).
#
# Environment:
#   SIGILC          path of a `sigilc` executable to use instead of building one.
#   SIGILC_PROFILE  cargo profile to build `sigilc` with when SIGILC is unset: `debug` (default) or
#                   `release`. Only `grimoire_sigilc`'s binary and its dependencies are built;
#                   building the whole workspace's binaries recompiled the GPU stack for minutes.
#
# Written for bash 3.2 as well (macOS runners): no associative arrays, no mapfile.
set -euo pipefail
export LC_ALL=C

out="${1:?output directory missing}"
profile="${SIGILC_PROFILE:-debug}"

if [ -n "${SIGILC:-}" ]; then
  sigilc="$SIGILC"
else
  case "$profile" in
    debug) cargo build --locked -p grimoire_sigilc --bin sigilc ;;
    release) cargo build --locked -p grimoire_sigilc --bin sigilc --release ;;
    *)
      echo "::error title=Sigil-Units::Unbekanntes SIGILC_PROFILE '${profile}' (debug oder release)."
      exit 2
      ;;
  esac
  target_dir="${CARGO_TARGET_DIR:-target}"
  sigilc="${target_dir}/${profile}/sigilc"
  if [ -f "${sigilc}.exe" ]; then
    sigilc="${sigilc}.exe"
  fi
fi

rm -rf "$out"
mkdir -p "$out/units" "$out/reports"

# Prints the `*.sigil` files of a directory, sorted bytewise, one per line.
sources() {
  find "$1" -maxdepth 1 -type f -name '*.sigil' | sort
}

# build <group> <root> <source-dir> [<behaviour manifest>]
build() {
  local group="$1" root="$2" source_dir="$3" manifest="${4:-}" status=0
  local files=()
  while IFS= read -r file; do
    files+=("$file")
  done < <(sources "$source_dir")
  if [ "${#files[@]}" -eq 0 ]; then
    echo "::error title=Sigil-Units::Keine Quellen in ${source_dir}."
    exit 1
  fi
  local args=(build --json --root "$root" --out "$out/units/$group")
  if [ -n "$manifest" ]; then
    args+=(--behaviors "$manifest")
  fi
  "$sigilc" "${args[@]}" "${files[@]}" > "$out/reports/$group.json" || status=$?
  if [ "$status" -ne 0 ]; then
    echo "::error title=Sigil-Units::sigilc build ${group} endete mit ${status}; der Bericht folgt."
    cat "$out/reports/$group.json"
    exit 1
  fi
  echo "${group}: ${#files[@]} Units"
}

# check_invalid <report> <source-dir> [<behaviour manifest>]: must end with exit code 1, and every
# file of the directory must have at least one diagnostic.
check_invalid() {
  local report="$1" source_dir="$2" manifest="${3:-}" status=0
  local files=()
  while IFS= read -r file; do
    files+=("$file")
  done < <(sources "$source_dir")
  if [ "${#files[@]}" -eq 0 ]; then
    echo "::error title=Sigil-Units::Keine Quellen in ${source_dir}."
    exit 1
  fi
  local args=(check --json)
  if [ -n "$manifest" ]; then
    args+=(--behaviors "$manifest")
  fi
  "$sigilc" "${args[@]}" "${files[@]}" > "$out/reports/$report.json" || status=$?
  if [ "$status" -ne 1 ]; then
    echo "::error title=Sigil-Units::sigilc check ${report} endete mit ${status} statt 1 (dort muss jede Datei Diagnosen haben)."
    cat "$out/reports/$report.json"
    exit 1
  fi
  if grep -q '"diagnostics": \[\]' "$out/reports/$report.json"; then
    echo "::error title=Sigil-Units::In ${source_dir} uebersetzt mindestens eine Datei ohne Diagnose."
    exit 1
  fi
  echo "${report}: ${#files[@]} Dateien mit Diagnosen"
}

corpus=crates/grimoire_sigilc/tests/corpus
reference=crates/grimoire_sigilc/tests/reference

build corpus "$corpus/valid" "$corpus/valid" "$corpus/behaviors.json"
build reference "$reference" "$reference" "$reference/behaviors.json"
build facade crates/grimoire/tests crates/grimoire/tests/fixtures
build bench crates/grimoire_bench crates/grimoire_bench/fixtures
check_invalid corpus-invalid "$corpus/invalid"
check_invalid corpus-schema-invalid "$corpus/schema-invalid" "$corpus/behaviors.json"

"$sigilc" --version
