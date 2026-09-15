| case | syntax | cause | fix hint | reason (one short sentence) |
|---|---|---|---|---|
| e01 | RON | 3 | 3 | Identifies the misspelled field and suggests `count`. |
| e01 | sigil 1 | 3 | 3 | Identifies the unknown field in `ring` and suggests `count`. |
| e02 | RON | 1 | 0 | Misleadingly reports a missing comma instead of an integer type mismatch and offers no fix. |
| e02 | sigil 1 | 3 | 3 | Explains that `count` requires an integer and gives a valid replacement. |
| e03 | RON | 3 | 3 | Names the missing `speed` field in emitter `bloom` and shows how to add it. |
| e03 | sigil 1 | 3 | 3 | Names the missing `speed` field in emitter `bloom` and supplies the required syntax. |
| e04 | RON | 1 | 1 | Misclassifies the missing delimiter as an unknown field and gives a misleading field-list hint. |
| e04 | sigil 1 | 3 | 3 | Identifies the unclosed bullet block and explicitly instructs inserting `}`. |
| e05 | RON | 3 | 3 | Explains the invalid `Ring` count, its valid range, and a concrete correction. |
| e05 | sigil 1 | 3 | 3 | Explains the invalid `ring` count, its valid range, and a concrete correction. |
| e06 | RON | 3 | 3 | Names the unknown behaviour, lists registered alternatives, and suggests a valid identifier. |
| e06 | sigil 1 | 3 | 3 | Names the unknown behaviour, lists registered alternatives, and suggests a valid identifier. |
| e07 | RON | 3 | 3 | States the expected and supplied units for `spread` and gives the corrected expression. |
| e07 | sigil 1 | 3 | 3 | States the expected and supplied units for `spread` and gives the corrected assignment. |
| e08 | RON | 3 | 3 | Identifies the duplicate emitter name and explains renaming within the unit’s uniqueness constraint. |
| e08 | sigil 1 | 3 | 3 | Identifies the duplicate emitter name and explains renaming within the unit’s uniqueness constraint. |
| e09 | RON | 1 | 0 | Misleadingly reports a struct type error instead of an extra comma and offers no fix. |
| e09 | sigil 1 | 3 | 3 | Identifies the unexpected comma in `keys` and explicitly says to remove it. |
| e10 | RON | 3 | 3 | States the unsupported and supported versions and supplies the corrected version field. |
| e10 | sigil 1 | 3 | 3 | States the unsupported and supported versions and supplies the corrected header. |