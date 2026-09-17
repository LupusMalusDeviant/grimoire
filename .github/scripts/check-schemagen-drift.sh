#!/usr/bin/env bash
# Project ADR-0011 "Schema-Codegen aus einer Quelle" (accepted, option 2e); Plan-0002 WP8.1.
#
# Regenerates the Rust payload/manifest types and codecs, the docs/formats/*.md field tables and
# the C# codecs of the tooling suite (tools/src/Grimoire.Formats/Generated, Plan-0002 WP9.1) that
# `grimoire_schemagen` derives from schema/*.gschema, then fails if that changes anything against
# the checked-in files. `git status --porcelain` also reports a generated file that is not checked
# in yet. This is the "generated code is current" gate ADR-0011 requires: a
# schema edit that lands without regenerated output turns this red instead of drifting silently
# until someone notices the contract and the generated code disagree.
#
# Deliberately a plain `git status`/`git diff`, not a hash comparison — identical on every OS
# given `.gitattributes`' `* text=auto eol=lf` (no CRLF surprises on the Windows runner) and the
# pinned toolchain's rustfmt (rust-toolchain.toml), the same two safeguards this repo's other
# `check-*.sh` scripts already rely on. `cargo fmt --all` runs after regeneration so a formatting
# difference alone never fails this check; the emitter's own docs explain why it does not try to
# emit already-perfectly-formatted code itself.
set -euo pipefail

GENERATED_PATHS=(
  crates/grimoire_debug/src/generated
  crates/grimoire_assets/src/generated
  docs/formats
  tools/src/Grimoire.Formats/Generated
)

cargo run -p grimoire_schemagen --locked --quiet -- generate
cargo fmt --all

changed="$(git status --porcelain -- "${GENERATED_PATHS[@]}")"

if [ -n "$changed" ]; then
  echo "::error title=Generierter Code veraltet::'cargo run -p grimoire_schemagen -- generate' hat Dateien geändert, die nicht (oder nicht mehr aktuell) eingecheckt sind (Projekt-ADR-0011, Plan-0002 WP8.1). Bitte 'cargo run -p grimoire_schemagen -- generate' plus 'cargo fmt --all' lokal ausführen und das Ergebnis committen."
  echo "$changed"
  git --no-pager diff -- "${GENERATED_PATHS[@]}"
  exit 1
fi

echo "generated code is up to date"
