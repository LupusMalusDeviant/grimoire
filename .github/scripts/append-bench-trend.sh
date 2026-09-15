#!/usr/bin/env bash
# Appends this run's BenchResult lines to the trend branch (P-12, engine ADR-0010 Nachtrag):
# `bench-trends`, an orphan branch dedicated to measurement history and the accepted basis, kept
# separate from `main` so its growing history never touches the engine's own commit graph. Creates
# the branch (as an orphan) on first use. Never force-pushes: a push race with a concurrent run
# (unlikely — this job only runs on push-to-main, but a re-run or two pushes landing close
# together are possible) is resolved by fetch + rebase(-equivalent) + retry, not by overwriting.
#
# Usage: append-bench-trend.sh <results-file.jsonl> <sha>
# Must run with cwd inside a checkout of the engine repo (any branch/ref; this script manages its
# own worktree for bench-trends and never touches the caller's checked-out branch or files).
set -euo pipefail

RESULTS="${1:?path to the BenchResult JSONL file of the current run}"
SHA="${2:?commit sha this run measured}"
BRANCH=bench-trends
TREND_FILE="trend/results.jsonl"
export GIT_AUTHOR_NAME="grimoire-bench-gate"
export GIT_AUTHOR_EMAIL="bench-gate@users.noreply.github.com"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME"
export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"

if [[ ! -s "$RESULTS" ]]; then
  echo "::error title=Keine Bench-Ergebnisse::$RESULTS ist leer oder fehlt; nichts zum Anhängen an $BRANCH."
  exit 1
fi
RESULTS="$(cd "$(dirname "$RESULTS")" && pwd)/$(basename "$RESULTS")"

git fetch origin "$BRANCH" 2>/dev/null || true

MAX_ATTEMPTS=5
for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
  echo "== append-bench-trend: attempt $attempt/$MAX_ATTEMPTS =="
  WORKDIR="$(mktemp -d)"

  if git show-ref --verify --quiet "refs/remotes/origin/$BRANCH"; then
    git worktree add --detach "$WORKDIR" "origin/$BRANCH" >/dev/null
    (cd "$WORKDIR" && git checkout -B "$BRANCH" >/dev/null)
  else
    echo "== $BRANCH does not exist on origin yet: creating it as an orphan branch =="
    git worktree add --detach "$WORKDIR" HEAD >/dev/null
    (
      cd "$WORKDIR"
      git checkout --orphan "$BRANCH" >/dev/null
      git rm -rf . >/dev/null 2>&1 || true
      cat >README.md <<'EOF2'
# bench-trends

Measurement history and the accepted basis for the engine benchmark gate (engine ADR-0010, P-12).

- `trend/results.jsonl`: every push-to-main measurement (contract §15.1 `BenchResult` lines,
  `wall_time` and `instructions` alike), appended by the `bench-gate` CI job
  (`.github/workflows/bench-gate.yml`). Never edited by hand.
- `accepted-basis.jsonl`: the basis the gate compares candidates against (engine ADR-0010 §4.2:
  never the last push). Changed **only** by the `bench-accept-baseline` workflow
  (`workflow_dispatch`, `.github/workflows/bench-accept-baseline.yml`), never automatically.

This branch is orphaned from `main` on purpose: it holds measurement data, not engine source
history, and grows independently of it.
EOF2
      git add README.md
      git commit -m "chore(bench-trends): initialize trend branch (P-12)" >/dev/null
    )
  fi

  mkdir -p "$WORKDIR/trend"
  cat "$RESULTS" >>"$WORKDIR/$TREND_FILE"

  set +e
  (
    cd "$WORKDIR"
    git add "$TREND_FILE"
    if git diff --cached --quiet; then
      echo "nothing new to append (identical content already present on $BRANCH)"
      exit 0
    fi
    git commit -m "chore(bench-trends): append results for ${SHA:0:12}" >/dev/null
    git push origin "HEAD:refs/heads/$BRANCH"
  )
  push_status=$?
  set -e

  git worktree remove --force "$WORKDIR" >/dev/null 2>&1 || rm -rf "$WORKDIR"

  if [[ "$push_status" -eq 0 ]]; then
    echo "== append-bench-trend: done on attempt $attempt =="
    exit 0
  fi

  echo "== append-bench-trend: push rejected (concurrent writer?), retrying after fetch ==" >&2
  git fetch origin "$BRANCH"
  sleep "$((attempt * 2))"
done

echo "::error title=bench-trends nicht aktualisiert::append-bench-trend.sh: giving up after ${MAX_ATTEMPTS} attempts"
exit 1
