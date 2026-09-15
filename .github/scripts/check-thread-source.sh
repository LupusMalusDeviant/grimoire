#!/usr/bin/env bash
# Engine ADR-0006 block 6: simulation threads come only from grimoire_exec.
#
# Every crate with the determinism clippy.toml must not depend on rayon, rayon-core or
# grimoire_exec, neither as a normal, build nor dev dependency, for any target and with every feature
# enabled (an optional dependency behind a non-default feature counts). The six clippy.toml files
# must be identical. A positive control on grimoire_exec proves the query itself works.
# Locally without network, `--target all` fails for the facade; the full check runs in CI.
set -euo pipefail
status=0
lints=(crates/*/clippy.toml)
if [ "$(sha256sum "${lints[@]}" | awk '{print $1}' | sort -u | wc -l)" -ne 1 ]; then
  echo "::error title=clippy.toml abweichend::Die clippy.toml der Determinismus-Crates sind nicht identisch."
  status=1
fi
tree() { cargo tree --locked -p "$1" -e "$2" --target all --all-features --prefix none --format '{p}'; }
for lint in "${lints[@]}"; do
  crate=$(basename "$(dirname "$lint")")
  deps=$(tree "$crate" normal,build,dev)   # errexit: a failing cargo tree fails the step, never a silent pass
  if grep -qE '^(rayon|rayon-core|grimoire_exec) ' <<<"$deps"; then
    echo "::error title=Determinismus-Crate hängt von rayon ab::${crate} (Engine-ADR-0006, Baustein 6)"
    status=1
  fi
done
deps=$(tree grimoire_exec normal)
grep -qE '^rayon ' <<<"$deps" || { echo "::error title=rayon-Prüfung defekt::grimoire_exec zeigt kein rayon"; exit 1; }
exit "$status"
