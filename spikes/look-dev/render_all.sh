#!/usr/bin/env sh
# Renders the look-dev comparison offscreen on the software adapter (WARP on Windows, lavapipe on
# Linux). Every GPU process stays short: one process per look x variant, one per timing round.
# Composites and metrics are then built from the files without a GPU. Output: out/ (or $OUT).
#
# Environment:
#   STEPS           comma-separated subset of render,timing,compose (default: all three)
#   OUT             output directory (default: out)
#   TIMING_ROUNDS   timing rounds per variant, including 3 discarded warm-up rounds (default: 18)
#   LOOK_DEV_GUARD  command run before every GPU process; a non-zero exit stops the script (exit 3)
#   RESUME=1        keep existing results and skip the runs and timing rounds already recorded
set -eu
cd "$(dirname "$0")"
GRIMOIRE_GPU_ADAPTER=software
export GRIMOIRE_GPU_ADAPTER
OUT=${OUT:-out}
STEPS=${STEPS:-render,timing,compose}
TIMING_ROUNDS=${TIMING_ROUNDS:-18}
RESUME=${RESUME:-0}

has_step() {
    case ",$STEPS," in
        *",$1,"*) return 0 ;;
        *) return 1 ;;
    esac
}

gpu_run() {
    if [ -n "${LOOK_DEV_GUARD:-}" ] && ! sh -c "$LOOK_DEV_GUARD"; then
        echo "render_all: guard refused before: $*" >&2
        exit 3
    fi
    cargo run --release --quiet -- "$@"
}

cargo build --release --quiet

if has_step render; then
    for look in toon stylized realistic; do
        for variant in calm busy; do
            if [ "$RESUME" = 1 ] && [ -f "$OUT/data/${look}_${variant}.txt" ]; then
                echo "render_all: ${look} ${variant} already rendered"
                continue
            fi
            gpu_run --look "$look" --variant "$variant" --out "$OUT"
        done
    done
fi

if has_step timing; then
    mkdir -p "$OUT/data"
    for variant in calm busy; do
        file="$OUT/data/timing_${variant}.txt"
        if [ "$RESUME" != 1 ]; then
            rm -f "$file"
        fi
        k=0
        while [ "$k" -lt "$TIMING_ROUNDS" ]; do
            if [ "$RESUME" = 1 ] && [ -f "$file" ] && grep -q "^round $k " "$file"; then
                echo "render_all: timing round $k ($variant) already recorded"
            else
                gpu_run --time-round "$k" --variant "$variant" --out "$OUT"
            fi
            k=$((k + 1))
        done
    done
fi

if has_step compose; then
    cargo run --release --quiet -- --compose --out "$OUT"
fi
