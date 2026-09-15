# Prompts für die Generierbarkeitsmessung

Die drei Prompts, mit denen Codex am 2026-09-15 die Muster G1 bis G3 geschrieben hat. Die erste Fassung
aus dem unbeaufsichtigten Lauf brach ab, weil Codex nicht verfügbar war (siehe Git-Historie dieser Dateien). Für die Nachmessung sind
nur die Pfade auf die Wurzel des Engine-Repos umgestellt und ein ausdrückliches Verbot ergänzt, Claudes
Dateien, `results.md`, `proto/` oder `docs/` zu öffnen oder einen Prüfer laufen zu lassen.

| Prompt | Aufgabentext |
|---|---|
| `g1-pendulum-fan.prompt.md` | `../briefs/g1-pendulum-fan.md` |
| `g2-mine-field.prompt.md` | `../briefs/g2-mine-field.md` |
| `g3-twin-spirals.prompt.md` | `../briefs/g3-twin-spirals.md` |

Aufruf je Muster, so am 2026-09-15 gelaufen (Beispiel G1, aus dem Wurzelverzeichnis des Engine-Repos,
read-only, je Muster genau ein Versuch):

```
codex exec -m gpt-6-astra -s read-only -c model_reasoning_effort="medium" -C . -o g1.out.md - < spikes/sigil-syntax/generability/prompts/g1-pendulum-fan.prompt.md
```

Die Antwort enthält zwei Codeblöcke (`ron`, dann `sigil`). Sie wird unverändert als
`generability/codex/raw/<muster>.out.md` abgelegt, der Inhalt der beiden Codeblöcke ohne Änderung als
`generability/codex/<muster>.ron` und `generability/codex/<muster>.sigil`. Danach in `proto/`
`cargo run -- report --write` ausführen.
