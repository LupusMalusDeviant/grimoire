# Prompts für die Generierbarkeitsmessung

Die drei Prompts, mit denen Codex im unbeaufsichtigten Lauf vom 2026-09-15 die Muster G1 bis G3 schreiben sollte.
Die Läufe brachen ab, weil Codex nicht verfügbar war (siehe `results.md`, Abschnitt 4); die Dateien sind unverändert
aus dem unbeaufsichtigten Lauf übernommen, damit die Messung wiederholbar ist.

| Prompt | Aufgabentext |
|---|---|
| `g1-pendulum-fan.prompt.md` | `../briefs/g1-pendulum-fan.md` |
| `g2-mine-field.prompt.md` | `../briefs/g2-mine-field.md` |
| `g3-twin-spirals.prompt.md` | `../briefs/g3-twin-spirals.md` |

Die Pfade in den Prompts sind relativ zu `spikes/sigil-syntax`. Aufruf je Muster (Beispiel G1, aus
dem Wurzelverzeichnis des Engine-Repos, read-only):

```
codex exec -s read-only -C spikes/sigil-syntax -o g1.out.md - < spikes/sigil-syntax/generability/prompts/g1-pendulum-fan.prompt.md
```

Die Antwort enthält zwei Codeblöcke (`ron`, dann `sigil`). Sie werden als
`generability/codex/g1-pendulum-fan.ron` und `generability/codex/g1-pendulum-fan.sigil` abgelegt;
danach in `proto/` `cargo run -- report --write` ausführen.
