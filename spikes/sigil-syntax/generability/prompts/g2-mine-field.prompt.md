You are a second author in a syntax experiment. Work fast; do not explore beyond the files listed.

Read ONLY these files (relative to the working directory):
- corpus/grammar/sigil-1.ebnf        (grammar of the text syntax "sigil 1")
- corpus/grammar/schema-model.txt    (shared data model; RON form via serde, mapping table at the end)
- corpus/ron/04-mirrored-spiral.ron and corpus/sigil/04-mirrored-spiral.sigil (one example pair)
- generability/briefs/g2-mine-field.md          (the pattern description, German)

Task: write the pattern described in the brief TWICE: once as RON (serde form per schema-model.txt,
starting with #![enable(implicit_some)]) and once in "sigil 1". Both must mean exactly the same.
Do not write files (read-only sandbox). Reply with exactly two fenced code blocks and nothing else:
first a block with info string "ron", then a block with info string "sigil".
