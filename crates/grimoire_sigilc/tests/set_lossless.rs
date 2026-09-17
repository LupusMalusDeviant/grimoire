//! Gate of Plan 0002 WP4.3: `sigilc set` is lossless — comments, order and formatting outside
//! the changed value stay byte-identical.
//!
//! Checked over every clean source (the `valid/` corpus and the facade's unit fixtures), both
//! exhaustively (every listed value of every file, set to itself and to a different value of its
//! own kind) and as property tests (a random value set to a random, arbitrarily nested Sigil value;
//! arbitrary path and value strings). For every successful rewrite the test demands:
//!
//! 1. the new source is exactly `old[..start] + value + old[end..]` for the old value's span;
//! 2. it parses with no diagnostics;
//! 3. the value reads back at the same path, exactly once;
//! 4. every comment is unchanged, in order;
//! 5. every other value outside the replaced span keeps its path and text, in order;
//! 6. setting the old text back restores the original bytes (for single-line old values).
//!
//! A refused rewrite must leave nothing changed, which `set_value` guarantees by returning only an
//! error; the CLI tests (`tests/cli.rs`) check that the file on disk is untouched too.

mod support;

use grimoire_sigilc::edit::{SetError, ValueEntry, ValueKind, list_values, set_value};
use grimoire_sigilc::{SyntaxKind, parse};
use proptest::prelude::*;
use support::{clean_sigil_sources, corpus_sigil_files};

fn comments(source: &str) -> Vec<String> {
    parse("t.sigil", source)
        .tree
        .tokens()
        .filter(|token| token.kind == SyntaxKind::Comment)
        .map(|token| token.text.clone())
        .collect()
}

fn values(source: &str) -> Vec<ValueEntry> {
    list_values(&parse("t.sigil", source).tree)
}

/// Asserts invariants 1-6 for setting `entry` of `source` to `value`, which must succeed.
fn assert_lossless_rewrite(name: &str, source: &str, entry: &ValueEntry, value: &str) {
    let context = format!("{name}: {} = {value:?}", entry.node_path);
    let outcome = set_value(name, source, &entry.node_path, value)
        .unwrap_or_else(|error| panic!("{context}: refused: {error}"));
    let start = entry.span.start as usize;
    let end = entry.span.end as usize;

    // 1. Only the value's bytes change.
    let expected = format!("{}{value}{}", &source[..start], &source[end..]);
    assert_eq!(
        outcome.source, expected,
        "{context}: bytes outside the value changed"
    );
    assert_eq!(outcome.old_value, entry.text, "{context}");
    assert_eq!(outcome.changed, entry.text != value, "{context}");

    // 2. Still parses cleanly.
    let reparsed = parse(name, &outcome.source);
    assert!(
        reparsed.diagnostics.is_empty(),
        "{context}: {:#?}",
        reparsed.diagnostics
    );

    // 3. Reads back exactly once, at the same place.
    let after = list_values(&reparsed.tree);
    let hits: Vec<&ValueEntry> = after
        .iter()
        .filter(|candidate| candidate.node_path == entry.node_path)
        .collect();
    assert_eq!(hits.len(), 1, "{context}: path does not read back once");
    assert_eq!(hits[0].text, value, "{context}");
    assert_eq!(hits[0].span.start as usize, start, "{context}");

    // 4. Comments unchanged.
    assert_eq!(comments(source), comments(&outcome.source), "{context}");

    // 5. Every value outside the replaced span keeps path and text, in order.
    let new_end = start + value.len();
    let outside = |list: &[ValueEntry], end: usize| -> Vec<(String, String)> {
        list.iter()
            .filter(|v| v.span.end as usize <= start || v.span.start as usize >= end)
            .map(|v| (v.node_path.clone(), v.text.clone()))
            .collect()
    };
    assert_eq!(
        outside(&values(source), end),
        outside(&after, new_end),
        "{context}: a value outside the replaced span changed"
    );

    // 6. Setting the old text back restores the original bytes.
    if !entry.text.contains('\n') {
        let back = set_value(name, &outcome.source, &entry.node_path, &entry.text)
            .unwrap_or_else(|error| panic!("{context}: setting back refused: {error}"));
        assert_eq!(
            back.source, source,
            "{context}: setting back did not restore"
        );
    }
}

/// A value of the same kind as `entry`, different from it.
fn different_value_of_same_kind(entry: &ValueEntry) -> String {
    let candidate = match entry.kind {
        ValueKind::Int => "7".to_string(),
        ValueKind::Float => "0.25".to_string(),
        ValueKind::Quantity => format!("3{}", entry.unit.as_deref().unwrap_or("t")),
        ValueKind::String => "\"changed \\\"text\\\"\"".to_string(),
        ValueKind::Ref => "other.ref".to_string(),
        ValueKind::Trigger => "time 5t".to_string(),
        ValueKind::List => "[a, [1, 2u], (x = 1)]".to_string(),
        ValueKind::Record => "(x = 1u, y = -2.5u)".to_string(),
        _ => "1".to_string(),
    };
    if candidate == entry.text {
        "8".to_string()
    } else {
        candidate
    }
}

/// Every listed value of every clean source, set to itself and to a different value.
#[test]
fn every_value_of_every_clean_source_can_be_set_losslessly() {
    let mut checked = 0usize;
    for (name, source) in clean_sigil_sources() {
        let listed = values(&source);
        assert!(!listed.is_empty(), "{name} lists no values");
        for entry in &listed {
            if !entry.text.contains('\n') {
                let same = set_value(&name, &source, &entry.node_path, &entry.text)
                    .unwrap_or_else(|error| panic!("{name}: {}: {error}", entry.node_path));
                assert_eq!(same.source, source, "{name}: {}", entry.node_path);
                assert!(!same.changed);
            }
            assert_lossless_rewrite(&name, &source, entry, &different_value_of_same_kind(entry));
            checked += 1;
        }
    }
    // The corpus and fixtures have a few hundred values; a collapse here means the walk broke.
    assert!(checked > 300, "only {checked} values were checked");
}

/// Files with parse diagnostics are refused as a whole, whatever the path.
#[test]
fn every_invalid_corpus_file_is_refused() {
    for (name, source) in corpus_sigil_files() {
        if !name.starts_with("invalid/") {
            continue;
        }
        let error = set_value(&name, &source, "meta.density", "1").unwrap_err();
        assert!(
            matches!(error, SetError::SourceHasErrors { ref diagnostics } if !diagnostics.is_empty()),
            "{name}: {error:?}"
        );
    }
}

fn ident() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,8}"
}

fn number() -> impl Strategy<Value = String> {
    "-?[0-9]{1,4}(\\.[0-9]{1,3})?"
}

fn quantity() -> impl Strategy<Value = String> {
    (
        number(),
        prop::sample::select(vec!["deg/t", "deg", "u/t2", "u/t", "u", "t", "beats"]),
    )
        .prop_map(|(number, unit)| format!("{number}{unit}"))
}

fn string_literal() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9 _.,;:!?()\\[\\]{}=/-]",
            Just("\\\"".to_string()),
            Just("\\\\".to_string()),
            Just("\\n".to_string()),
            Just("\\t".to_string()),
        ],
        0..10,
    )
    .prop_map(|parts| format!("\"{}\"", parts.concat()))
}

fn scalar_value() -> BoxedStrategy<String> {
    prop_oneof![
        number(),
        quantity(),
        string_literal(),
        (ident(), prop::collection::vec(ident(), 0..3)).prop_map(|(head, tail)| std::iter::once(
            head
        )
        .chain(tail)
        .collect::<Vec<_>>()
        .join(".")),
        (
            prop::sample::select(vec!["time", "distance", "event"]),
            prop_oneof![quantity(), ident()]
        )
            .prop_map(|(keyword, argument)| format!("{keyword} {argument}")),
    ]
    .boxed()
}

/// Any single-line Sigil value, nested up to three levels of lists and records, with canonical or
/// compact separators.
fn sigil_value() -> impl Strategy<Value = String> {
    scalar_value().prop_recursive(3, 24, 4, |inner| {
        let separator = prop::sample::select(vec![", ", ","]);
        prop_oneof![
            (
                prop::collection::vec(inner.clone(), 0..4),
                separator.clone(),
                any::<bool>()
            )
                .prop_map(|(items, separator, trailing)| {
                    let comma = if trailing && !items.is_empty() {
                        ","
                    } else {
                        ""
                    };
                    format!("[{}{comma}]", items.join(separator))
                }),
            (prop::collection::vec((ident(), inner), 0..3), separator).prop_map(
                |(fields, separator)| {
                    let fields: Vec<String> = fields
                        .into_iter()
                        .map(|(name, value)| format!("{name} = {value}"))
                        .collect();
                    format!("({})", fields.join(separator))
                }
            ),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// A random value of a random clean source, set to a random valid Sigil value.
    #[test]
    fn random_values_set_losslessly(file_pick in any::<prop::sample::Index>(), value_pick in any::<prop::sample::Index>(), value in sigil_value()) {
        let sources = clean_sigil_sources();
        let (name, source) = file_pick.get(&sources);
        let listed = values(source);
        let entry = value_pick.get(&listed);
        assert_lossless_rewrite(name, source, entry, &value);
    }

    /// Arbitrary path and value strings never panic; whatever is accepted is lossless.
    #[test]
    fn arbitrary_paths_and_values_never_panic(file_pick in any::<prop::sample::Index>(), path in "\\PC{0,40}", value in "\\PC{0,40}") {
        let sources = clean_sigil_sources();
        let (name, source) = file_pick.get(&sources);
        if let Ok(outcome) = set_value(name, source, &path, &value) {
            let entry = values(source).into_iter().find(|entry| entry.node_path == path).unwrap();
            prop_assert_eq!(outcome.source, format!("{}{value}{}", &source[..entry.span.start as usize], &source[entry.span.end as usize..]));
        }
    }

    /// An existing path with an arbitrary value string never panics; an accepted value is lossless.
    #[test]
    fn existing_paths_with_arbitrary_values_never_panic(file_pick in any::<prop::sample::Index>(), value_pick in any::<prop::sample::Index>(), value in "\\PC{0,24}") {
        let sources = clean_sigil_sources();
        let (name, source) = file_pick.get(&sources);
        let listed = values(source);
        let entry = value_pick.get(&listed);
        if set_value(name, source, &entry.node_path, &value).is_ok() {
            assert_lossless_rewrite(name, source, entry, &value);
        }
    }
}
