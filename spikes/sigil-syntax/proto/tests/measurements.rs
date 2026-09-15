//! Reproduces every measurement of spike OF-4.1 (`cargo test`).

use sigil_syntax_proto::check::{self, Syntax};
use sigil_syntax_proto::measure::{self, PATTERNS};
use sigil_syntax_proto::{ron_cst, sigil};

#[test]
fn corpus_patterns_parse_and_both_syntaxes_yield_the_same_model() {
    for p in PATTERNS {
        let r = check::check_file(&measure::file(&format!("corpus/ron/{p}.ron")));
        let s = check::check_file(&measure::file(&format!("corpus/sigil/{p}.sigil")));
        assert!(r.diags.is_empty(), "{p}.ron: {:#?}", r.diags);
        assert!(s.diags.is_empty(), "{p}.sigil: {:#?}", s.diags);
        assert!(r.resolved.is_some());
        assert_eq!(r.resolved, s.resolved, "{p}: models differ");
    }
}

fn all_files(ext: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for dir in [
        "corpus/ron",
        "corpus/sigil",
        "corpus/errors",
        "generability/claude",
        "generability/codex",
    ] {
        if let Ok(rd) = std::fs::read_dir(measure::file(dir)) {
            for e in rd.flatten() {
                if e.path().extension().and_then(|x| x.to_str()) == Some(ext) {
                    out.push(e.path());
                }
            }
        }
    }
    out
}

#[test]
fn sigil_syntax_tree_is_lossless_for_every_file() {
    for f in all_files("sigil") {
        let src = std::fs::read_to_string(&f).unwrap();
        let (cst, _) = sigil::parse(&src);
        assert_eq!(cst.text(&src), src, "{}", f.display());
    }
}

#[test]
fn ron_scanner_is_lossless_for_every_file() {
    for f in all_files("ron") {
        let src = std::fs::read_to_string(&f).unwrap();
        assert!(ron_cst::lossless(&src), "{}", f.display());
    }
}

#[test]
fn every_error_file_yields_a_positioned_diagnostic() {
    let results = measure::error_results();
    assert_eq!(results.len(), 20);
    for r in &results {
        let d = r
            .first()
            .unwrap_or_else(|| panic!("{}: no diagnostic", r.file));
        assert!(d.pos.is_some(), "{}: no position", r.file);
    }
}

#[test]
fn sigil_diagnostics_hit_the_target_position_and_path() {
    for r in measure::error_results()
        .iter()
        .filter(|r| r.syntax == Syntax::Sigil)
    {
        assert_eq!(r.pos_score(), 3, "{}: {:?}", r.file, r.first());
        assert_eq!(r.path_score(), 3, "{}: {:?}", r.file, r.first());
        assert_eq!(
            r.outcome.diags.len(),
            1,
            "{}: {:#?}",
            r.file,
            r.outcome.diags
        );
    }
}

#[test]
fn lossless_edits_change_exactly_one_line_and_keep_comments() {
    for row in measure::roundtrip_rows()
        .iter()
        .filter(|r| !r.method.contains("`ron`-Crate"))
    {
        assert_eq!(row.diff, (1, 1), "{} / {}", row.scenario, row.method);
        assert_eq!(
            row.comments.0, row.comments.1,
            "{} / {}",
            row.scenario, row.method
        );
        assert!(
            row.reparse_clean && row.model_as_expected,
            "{} / {}",
            row.scenario,
            row.method
        );
    }
}

#[test]
fn results_md_is_up_to_date() {
    let path = measure::results_path();
    let text = std::fs::read_to_string(&path)
        .expect("results.md")
        .replace("\r\n", "\n");
    let updated = measure::update_results(&text, &measure::blocks());
    assert!(
        updated == text,
        "results.md is stale; run `cargo run -- report --write` in spikes/sigil-syntax/proto"
    );
}
