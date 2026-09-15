# Zweitbewertung der Diagnosen (OF-4.1)

Blinde Zweitbewertung der von Hand bewerteten Spalten Ursache und Fix-Hinweis (`results.md`, Abschnitte
2.3 und 2.8), am 2026-09-15 durch Codex.

| Datei | Inhalt |
|---|---|
| `make_prompt.py` | baut den Prompt aus `corpus/errors/expected.json`, den Fehlerdateien und der Ausgabe von `sigil-syntax-proto check` |
| `prompt.md` | der Prompt, wie er an Codex ging (mit `make_prompt.py` byte-gleich reproduzierbar) |
| `codex.out.md` | die Antwort von Codex, unverändert |

Der Prompt enthält die Skala, die eingebaute Änderung und die vollständige Diagnose je Fehlerdatei, aber
keine Werte von Claude. Codex lief read-only in einem leeren Verzeichnis außerhalb des Repos und hat laut
Protokoll keinen Befehl ausgeführt. Aufruf (aus `spikes/sigil-syntax`, Prototyp gebaut):

```
python second-rating/make_prompt.py > second-rating/prompt.md
codex exec --skip-git-repo-check -m gpt-6-astra -s read-only -c model_reasoning_effort="medium" -C <leeres Verzeichnis> -o codex.out.md - < second-rating/prompt.md
```

Die Werte der Antwort stehen als `RATINGS_CODEX` in `proto/src/measure.rs`; der Test
`second_rating_table_matches_the_stored_codex_reply` prüft, dass beide übereinstimmen.
