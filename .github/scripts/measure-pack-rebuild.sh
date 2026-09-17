#!/usr/bin/env bash
# Measures a full pack rebuild with `grimoire-ac build` (Plan 0002 WP9.2, PRD-0016 NFR: < 60 s) on a
# content tree far larger than the game's own: the reference patterns, plus `--copies` copies of every
# standalone one in its own directory. `12-finale-composite.sigil` imports another pattern, and imports
# resolve against the content root (docs/formats/sigil.md §13.2), so only the originals at the root can
# carry it; the copies are the patterns without imports.
#
# Numbers only come from CI runners, never from a development machine.
#
# Usage: measure-pack-rebuild.sh <grimoire-ac> <sigilc> [copies] [budget seconds]
set -euo pipefail

ac=${1:?usage: measure-pack-rebuild.sh <grimoire-ac> <sigilc> [copies] [budget seconds]}
sigilc=${2:?usage: measure-pack-rebuild.sh <grimoire-ac> <sigilc> [copies] [budget seconds]}
copies=${3:-40}
budget=${4:-60}

root=$(git rev-parse --show-toplevel)
reference="$root/crates/grimoire_sigilc/tests/reference"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
content="$work/content"
mkdir -p "$content"
cp "$reference"/*.sigil "$content/"

for index in $(seq 1 "$copies"); do
  directory=$(printf '%s/stage-%03d' "$content" "$index")
  mkdir -p "$directory"
  for file in "$reference"/*.sigil; do
    grep -q '^import ' "$file" || cp "$file" "$directory/"
  done
done

sources=$(find "$content" -name '*.sigil' | wc -l | tr -d ' ')
echo "Content: $sources source files below a single root."

start=$(date +%s%N)
"$ac" build "$content" \
  --out "$work/packs" \
  --behaviors "$reference/behaviors.json" \
  --sigilc "$sigilc" \
  --timings
elapsed_ms=$(( ($(date +%s%N) - start) / 1000000 ))

pack="$work/packs/content.grimpack"
bytes=$(wc -c <"$pack" | tr -d ' ')
per_unit=$(( elapsed_ms * 1000 / sources ))
echo "Pack: $bytes bytes for $sources units; full rebuild in ${elapsed_ms} ms" \
  "(${per_unit} us per unit, budget $(( budget * 1000 )) ms)."

if [ "$elapsed_ms" -gt $(( budget * 1000 )) ]; then
  echo "A full pack rebuild took ${elapsed_ms} ms, above the ${budget} s goal of PRD-0016." >&2
  exit 1
fi
