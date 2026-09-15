You are a second author in a syntax experiment. Work fast; do not explore beyond the files listed.

Read ONLY these files (relative to the working directory):
- spikes/sigil-syntax/corpus/grammar/sigil-1.ebnf        (grammar of the text syntax "sigil 1")
- spikes/sigil-syntax/corpus/grammar/schema-model.txt    (shared data model; RON form via serde, mapping table at the end)
- spikes/sigil-syntax/corpus/ron/04-mirrored-spiral.ron and spikes/sigil-syntax/corpus/sigil/04-mirrored-spiral.sigil (one example pair)
- spikes/sigil-syntax/generability/briefs/g3-twin-spirals.md          (the pattern description, German)

Do NOT open anything else. In particular do not open spikes/sigil-syntax/generability/claude/,
spikes/sigil-syntax/generability/codex/, spikes/sigil-syntax/results.md, spikes/sigil-syntax/proto/,
or anything under docs/. Do not run any parser or checker; this measures a first attempt.

Task: write the pattern described in the brief TWICE: once as RON (serde form per schema-model.txt,
starting with #![enable(implicit_some)]) and once in "sigil 1". Both must mean exactly the same.
Do not write files (read-only sandbox). Reply with exactly two fenced code blocks and nothing else:
first a block with info string "ron", then a block with info string "sigil".
