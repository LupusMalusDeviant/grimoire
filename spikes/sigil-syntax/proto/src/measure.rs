//! All measurements of spike OF-4.1, rendered as Markdown blocks.
//!
//! `results.md` embeds each block between `<!-- BEGIN GENERATED: name -->` and
//! `<!-- END GENERATED: name -->`. `cargo run -- report --write` refreshes them, and the test
//! `results_md_is_up_to_date` fails when a block no longer matches the code.

use crate::check::{self, Outcome, Syntax};
use crate::diag::{self, Diag};
use crate::model::*;
use crate::ron_front::RonSigil;
use crate::{ron_cst, sigil};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const PATTERNS: [&str; 5] = [
    "01-ring-burst",
    "02-aimed-stream",
    "03-subemitter-cascade",
    "04-mirrored-spiral",
    "05-wave-line-composite",
];
pub const BRIEFS: [&str; 3] = ["g1-pendulum-fan", "g2-mine-field", "g3-twin-spirals"];
pub const AUTHORS: [&str; 2] = ["claude", "codex"];

/// Manual quality ratings by Claude (provisional, PO review pending), rubric in results.md:
/// (id, cause RON, hint RON, cause sigil 1, hint sigil 1).
/// RON e03 and e07 were re-rated after the RON adapter learned to relocate a missing field to
/// its owner and to name the field of a wrong unit (review of 2026-09-15); before: 2/2 and 2/2.
pub const RATINGS: &[(&str, u8, u8, u8, u8)] = &[
    ("e01", 3, 3, 3, 3),
    ("e02", 1, 0, 3, 3),
    ("e03", 3, 2, 3, 2),
    ("e04", 1, 0, 3, 3),
    ("e05", 3, 3, 3, 3),
    ("e06", 3, 3, 3, 3),
    ("e07", 3, 3, 3, 3),
    ("e08", 3, 3, 3, 3),
    ("e09", 1, 0, 3, 3),
    ("e10", 3, 3, 3, 3),
];

/// Independent second rating by Codex (gpt-6-astra, 2026-09-15), same shape as `RATINGS`.
/// The rater did not see `RATINGS`: `second-rating/prompt.md` (built by `make_prompt.py`) holds
/// only the rubric, the injected change and the checker output of every error file. The reply is
/// kept unchanged in `second-rating/codex.out.md`; a test checks that this table matches it.
pub const RATINGS_CODEX: &[(&str, u8, u8, u8, u8)] = &[
    ("e01", 3, 3, 3, 3),
    ("e02", 1, 0, 3, 3),
    ("e03", 3, 3, 3, 3),
    ("e04", 1, 1, 3, 3),
    ("e05", 3, 3, 3, 3),
    ("e06", 3, 3, 3, 3),
    ("e07", 3, 3, 3, 3),
    ("e08", 3, 3, 3, 3),
    ("e09", 1, 0, 3, 3),
    ("e10", 3, 3, 3, 3),
];

/// Reads the rating table of a rater reply (`| case | syntax | cause | fix hint | reason |`)
/// back into the shape of `RATINGS`; rows that are not a case are skipped.
pub fn parse_rating_reply(text: &str) -> Vec<(String, u8, u8, u8, u8)> {
    let mut by_case: BTreeMap<String, [Option<u8>; 4]> = BTreeMap::new();
    for line in text.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 5 || !cells[1].starts_with('e') {
            continue;
        }
        let (Ok(cause), Ok(hint)) = (cells[3].parse::<u8>(), cells[4].parse::<u8>()) else {
            continue;
        };
        let slot = by_case.entry(cells[1].to_string()).or_default();
        match cells[2] {
            "RON" => (slot[0], slot[1]) = (Some(cause), Some(hint)),
            "sigil 1" => (slot[2], slot[3]) = (Some(cause), Some(hint)),
            _ => {}
        }
    }
    by_case
        .into_iter()
        .filter_map(|(id, s)| Some((id, s[0]?, s[1]?, s[2]?, s[3]?)))
        .collect()
}

/// `spikes/sigil-syntax`.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn file(rel: &str) -> PathBuf {
    root().join(rel)
}

pub fn read(rel: &str) -> String {
    std::fs::read_to_string(file(rel))
        .unwrap_or_default()
        .replace("\r\n", "\n")
}

fn ext(s: Syntax) -> &'static str {
    match s {
        Syntax::Ron => "ron",
        Syntax::Sigil => "sigil",
    }
}

fn yes(b: bool) -> &'static str {
    if b { "ja" } else { "nein" }
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn pos_str(p: Option<diag::Pos>) -> String {
    p.map_or("–".to_string(), |p| format!("{}:{}", p.line, p.col))
}

// ------------------------------------------------------------------------------------------
// Corpus
// ------------------------------------------------------------------------------------------

pub fn corpus_block() -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Muster | Diagnosen RON | Diagnosen sigil 1 | Aufgelöste Modelle gleich | CST sigil 1 verlustfrei | RON-Scanner verlustfrei |"
    );
    let _ = writeln!(o, "|---|---:|---:|:-:|:-:|:-:|");
    for p in PATTERNS {
        let r = check::check_file(&file(&format!("corpus/ron/{p}.ron")));
        let s = check::check_file(&file(&format!("corpus/sigil/{p}.sigil")));
        let equal = r.resolved.is_some() && r.resolved == s.resolved;
        let (cst, _) = sigil::parse(&s.src);
        let _ = writeln!(
            o,
            "| `{p}` | {} | {} | {} | {} | {} |",
            r.diags.len(),
            s.diags.len(),
            yes(equal),
            yes(cst.text(&s.src) == s.src),
            yes(ron_cst::lossless(&r.src))
        );
    }
    o
}

// ------------------------------------------------------------------------------------------
// Error corpus
// ------------------------------------------------------------------------------------------

pub struct ErrorResult {
    pub id: String,
    pub syntax: Syntax,
    pub file: String,
    pub outcome: Outcome,
    pub exp_pos: diag::Pos,
    pub exp_phase: String,
    pub exp_path: String,
}

impl ErrorResult {
    pub fn first(&self) -> Option<&Diag> {
        self.outcome.diags.first()
    }
    /// 3 exact, 2 same line, 1 other line, 0 none.
    pub fn pos_score(&self) -> u8 {
        match self.first().and_then(|d| d.pos) {
            Some(p) if p == self.exp_pos => 3,
            Some(p) if p.line == self.exp_pos.line => 2,
            Some(_) => 1,
            None => 0,
        }
    }
    /// 3 exact, 2 prefix (owner of the expected path), 1 other path, 0 none.
    pub fn path_score(&self) -> u8 {
        match self.first().and_then(|d| d.node_path.as_deref()) {
            Some(p) if p == self.exp_path => 3,
            Some(p) if self.exp_path.starts_with(p) || p.starts_with(&self.exp_path) => 2,
            Some(_) => 1,
            None => 0,
        }
    }
}

pub fn error_results() -> Vec<ErrorResult> {
    let cases: serde_json::Value =
        serde_json::from_str(&read("corpus/errors/expected.json")).expect("expected.json");
    let mut out = Vec::new();
    for c in cases.as_array().expect("array") {
        for syntax in [Syntax::Ron, Syntax::Sigil] {
            let exp = &c[ext(syntax)];
            let rel = format!("corpus/{}", exp["file"].as_str().unwrap_or_default());
            let dir = file(&format!("corpus/{}", ext(syntax)));
            let outcome = check::check_file_in(&file(&rel), &dir);
            out.push(ErrorResult {
                id: c["id"].as_str().unwrap_or_default().to_string(),
                syntax,
                file: rel,
                outcome,
                exp_pos: diag::Pos {
                    line: exp["line"].as_u64().unwrap_or(0) as usize,
                    col: exp["column"].as_u64().unwrap_or(0) as usize,
                },
                exp_phase: exp["phase"].as_str().unwrap_or_default().to_string(),
                exp_path: exp["node_path"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
    out
}

pub fn errors_block(results: &[ErrorResult]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Fall | Syntax | Diagnosen | Position Ist (Soll) | Phase Ist (Soll) | Meldung | Fix-Hinweis | Knotenpfad Ist (Soll) |"
    );
    let _ = writeln!(o, "|---|---|---:|---|---|---|---|---|");
    for r in results {
        let d = r.first();
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} ({}) | {} ({}) | {} | {} | {} ({}) |",
            r.id,
            r.syntax.name(),
            r.outcome.diags.len(),
            pos_str(d.and_then(|d| d.pos)),
            pos_str(Some(r.exp_pos)),
            d.map_or("–", |d| d.phase.as_str()),
            r.exp_phase,
            d.map_or("–".to_string(), |d| cell(&d.cause)),
            d.and_then(|d| d.hint.as_deref())
                .map_or("–".to_string(), cell),
            d.and_then(|d| d.node_path.as_deref())
                .map_or("–".to_string(), |p| format!("`{p}`")),
            format_args!("`{}`", r.exp_path),
        );
    }
    o
}

pub fn quality_block(results: &[ErrorResult]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Fall | Position RON | Position sigil 1 | Ursache RON | Ursache sigil 1 | Fix-Hinweis RON | Fix-Hinweis sigil 1 | Knotenpfad RON | Knotenpfad sigil 1 |"
    );
    let _ = writeln!(o, "|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|");
    let mut sums = [0u32; 8];
    for (id, cr, hr, cs, hs) in RATINGS {
        let get = |s: Syntax| results.iter().find(|r| r.id == *id && r.syntax == s);
        let (Some(r), Some(s)) = (get(Syntax::Ron), get(Syntax::Sigil)) else {
            continue;
        };
        let row = [
            r.pos_score(),
            s.pos_score(),
            *cr,
            *cs,
            *hr,
            *hs,
            r.path_score(),
            s.path_score(),
        ];
        for (sum, v) in sums.iter_mut().zip(row) {
            *sum += u32::from(v);
        }
        let _ = writeln!(
            o,
            "| {id} | {} | {} | {} | {} | {} | {} | {} | {} |",
            row[0], row[1], row[2], row[3], row[4], row[5], row[6], row[7]
        );
    }
    let _ = writeln!(
        o,
        "| **Summe (max. 30)** | **{}** | **{}** | **{}** | **{}** | **{}** | **{}** | **{}** | **{}** |",
        sums[0], sums[1], sums[2], sums[3], sums[4], sums[5], sums[6], sums[7]
    );
    o
}

pub fn raw_block(results: &[ErrorResult]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Fall | Rohmeldung `ron` 0.12.2 | Position laut `ron` |"
    );
    let _ = writeln!(o, "|---|---|---|");
    for r in results.iter().filter(|r| r.syntax == Syntax::Ron) {
        // Straight from `ron`, so the adapter's relocation (e03) does not show up here.
        let raw = crate::ron_front::raw_error(&r.outcome.src);
        let _ = writeln!(
            o,
            "| {} | {} | {} |",
            r.id,
            raw.as_ref().map_or(
                "– (Befund des gemeinsamen Validators, nicht von `ron`)".to_string(),
                |(m, _)| format!("`` {} ``", cell(m))
            ),
            raw.map_or("–".into(), |(_, p)| pos_str(Some(p)))
        );
    }
    o
}

pub fn modder_block(results: &[ErrorResult]) -> String {
    let mut o = String::new();
    for id in ["e02", "e03", "e04", "e07", "e09"] {
        for r in results.iter().filter(|r| r.id == id) {
            let _ = writeln!(o, "**{id}, {}**\n", r.syntax.name());
            let _ = writeln!(o, "```text");
            for d in &r.outcome.diags {
                o.push_str(&diag::render(&r.file, &r.outcome.src, d));
            }
            let _ = writeln!(o, "```\n");
        }
    }
    o.trim_end().to_string() + "\n"
}

// ------------------------------------------------------------------------------------------
// Roundtrip
// ------------------------------------------------------------------------------------------

pub struct Scenario {
    pub label: &'static str,
    pub pattern: &'static str,
    pub path: &'static str,
    pub ron_text: &'static str,
    pub sigil_text: &'static str,
    pub apply: fn(&mut Resolved),
    pub apply_ron: fn(&mut RonSigil),
}

fn set_speed(r: &mut Resolved) {
    r.emitters[0].speed = UnitsPerTick(0.12);
}
fn set_speed_ron(m: &mut RonSigil) {
    if let EmitterItem::Emitter { speed, .. } = &mut m.emitters[0] {
        *speed = UnitsPerTick(0.12);
    }
}
fn set_key(r: &mut Resolved) {
    if let Modifier::SpeedCurve { keys, .. } = &mut r.emitters[0].modifiers[2] {
        keys[1].mul = 0.25;
    }
}
fn set_key_ron(m: &mut RonSigil) {
    if let EmitterItem::Emitter { modifiers, .. } = &mut m.emitters[0] {
        if let Modifier::SpeedCurve { keys, .. } = &mut modifiers[2] {
            keys[1].mul = 0.25;
        }
    }
}
fn set_override(r: &mut Resolved) {
    if let Block::Ring { count, .. } = &mut r.emitters[2].block {
        *count = 16;
    }
}
fn set_override_ron(m: &mut RonSigil) {
    if let EmitterItem::Use { overrides, .. } = &mut m.emitters[2] {
        if let Ok(v) = ron::value::RawValue::from_boxed_ron("16".into()) {
            overrides.insert("block.count".into(), v);
        }
    }
}

pub const SCENARIOS: [Scenario; 3] = [
    Scenario {
        label: "Emitter-Geschwindigkeit",
        pattern: "04-mirrored-spiral",
        path: "emitters.bloom.speed",
        ron_text: "UnitsPerTick(0.12)",
        sigil_text: "0.12u/t",
        apply: set_speed,
        apply_ron: set_speed_ron,
    },
    Scenario {
        label: "Tempokurve, 2. Stützpunkt",
        pattern: "04-mirrored-spiral",
        path: "emitters.bloom.modifiers[2].keys[1].mul",
        ron_text: "0.25",
        sigil_text: "0.25",
        apply: set_key,
        apply_ron: set_key_ron,
    },
    Scenario {
        label: "Override einer importierten Kachel",
        pattern: "05-wave-line-composite",
        path: "emitters.finale.block.count",
        ron_text: "16",
        sigil_text: "16",
        apply: set_override,
        apply_ron: set_override_ron,
    },
];

/// Lines removed and added according to a longest-common-subsequence diff.
pub fn line_diff(a: &str, b: &str) -> (usize, usize) {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let lcs = dp[0][0];
    (a.len() - lcs, b.len() - lcs)
}

pub struct RoundtripRow {
    pub scenario: &'static str,
    pub method: &'static str,
    pub comments: (usize, usize),
    pub diff: (usize, usize),
    pub lines: (usize, usize),
    pub reparse_clean: bool,
    pub model_as_expected: bool,
    pub error: Option<String>,
}

fn comments(syn: Syntax, src: &str) -> usize {
    match syn {
        Syntax::Ron => ron_cst::comment_count(src),
        Syntax::Sigil => sigil::parse(src).0.comment_count(),
    }
}

pub fn roundtrip_rows() -> Vec<RoundtripRow> {
    let mut rows = Vec::new();
    for sc in &SCENARIOS {
        for (method, syn) in [
            ("sigil 1: verlustfreier CST", Syntax::Sigil),
            ("RON: eigener verlustfreier Scanner", Syntax::Ron),
            (
                "RON: `ron`-Crate (Modell ändern, neu serialisieren)",
                Syntax::Ron,
            ),
        ] {
            let rel = format!("corpus/{}/{}.{}", ext(syn), sc.pattern, ext(syn));
            let dir = file(&format!("corpus/{}", ext(syn)));
            let src = read(&rel);
            let before = check::check_src(syn, src.clone(), &dir, 0);
            let new = if method.starts_with("sigil") {
                sigil::set(&src, sc.path, sc.sigil_text)
            } else if method.contains("Scanner") {
                ron_cst::set(&src, sc.path, sc.ron_text)
            } else {
                crate::ron_front::parse(&src)
                    .map_err(|d| d.cause)
                    .and_then(|mut m| {
                        (sc.apply_ron)(&mut m);
                        let cfg = ron::ser::PrettyConfig::new()
                            .struct_names(true)
                            .extensions(ron::extensions::Extensions::IMPLICIT_SOME);
                        ron::ser::to_string_pretty(&m, cfg).map_err(|e| e.to_string())
                    })
            };
            let row = match new {
                Ok(new) => {
                    let after = check::check_src(syn, new.clone(), &dir, 0);
                    let expected = before.resolved.clone().map(|mut r| {
                        (sc.apply)(&mut r);
                        r
                    });
                    RoundtripRow {
                        scenario: sc.label,
                        method,
                        comments: (comments(syn, &src), comments(syn, &new)),
                        diff: line_diff(&src, &new),
                        lines: (src.lines().count(), new.lines().count()),
                        reparse_clean: after.diags.is_empty(),
                        model_as_expected: expected.is_some() && after.resolved == expected,
                        error: None,
                    }
                }
                Err(e) => RoundtripRow {
                    scenario: sc.label,
                    method,
                    comments: (comments(syn, &src), 0),
                    diff: (0, 0),
                    lines: (src.lines().count(), 0),
                    reparse_clean: false,
                    model_as_expected: false,
                    error: Some(e),
                },
            };
            rows.push(row);
        }
    }
    rows
}

pub fn roundtrip_block() -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Szenario (Knotenpfad) | Methode | Kommentare vorher → nachher | Zeilen −/+ (Diff) | Zeilen vorher → nachher | Neu geparst ohne Diagnose | Modell wie erwartet |"
    );
    let _ = writeln!(o, "|---|---|:-:|:-:|:-:|:-:|:-:|");
    let rows = roundtrip_rows();
    for (i, r) in rows.iter().enumerate() {
        let sc = &SCENARIOS[i / 3];
        let label = format!("{} (`{}`)", r.scenario, sc.path);
        match &r.error {
            Some(e) => {
                let _ = writeln!(
                    o,
                    "| {label} | {} | Fehler: {} | | | | |",
                    r.method,
                    cell(e)
                );
            }
            None => {
                let _ = writeln!(
                    o,
                    "| {label} | {} | {} → {} | −{}/+{} | {} → {} | {} | {} |",
                    r.method,
                    r.comments.0,
                    r.comments.1,
                    r.diff.0,
                    r.diff.1,
                    r.lines.0,
                    r.lines.1,
                    yes(r.reparse_clean),
                    yes(r.model_as_expected)
                );
            }
        }
    }
    o
}

/// The `ron::Value` probe for composition overrides.
pub fn ron_value_block() -> String {
    let mut o = String::new();
    let text = "Ticks(240)";
    let value: Result<ron::Value, _> = ron::from_str(text);
    let _ = writeln!(
        o,
        "- `ron::from_str::<ron::Value>(\"{text}\")` ergibt `{value:?}`."
    );
    if let Ok(v) = value {
        let as_deg: Result<Deg, _> = v.clone().into_rust();
        let _ = writeln!(o, "- Dieser Wert als `Deg` gelesen: `{as_deg:?}`.");
    }
    let raw = match ron::value::RawValue::from_ron(text).map(|r| r.into_rust::<Deg>()) {
        Ok(Ok(d)) => format!("`{d:?}`"),
        Ok(Err(e)) => format!(
            "Fehler `` {} `` an {}:{}",
            e.code, e.span.start.line, e.span.start.col
        ),
        Err(e) => format!("Fehler `` {e} ``"),
    };
    let _ = writeln!(
        o,
        "- Derselbe Text als `RawValue`, gelesen als `Deg`: {raw}."
    );
    o
}

// ------------------------------------------------------------------------------------------
// Several errors in one file
// ------------------------------------------------------------------------------------------

/// Applies the single-hunk edits of several error variants of the same base file.
pub fn merge_variants(base: &str, variants: &[String]) -> String {
    let b: Vec<&str> = base.lines().collect();
    let mut hunks: Vec<(usize, usize, Vec<String>)> = Vec::new();
    for v in variants {
        let n: Vec<&str> = v.lines().collect();
        let pre = b.iter().zip(&n).take_while(|(x, y)| x == y).count();
        let post = b
            .iter()
            .rev()
            .zip(n.iter().rev())
            .take_while(|(x, y)| x == y)
            .count()
            .min(b.len() - pre)
            .min(n.len() - pre);
        hunks.push((
            pre,
            b.len() - post,
            n[pre..n.len() - post]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ));
    }
    hunks.sort_by_key(|h| std::cmp::Reverse(h.0));
    let mut lines: Vec<String> = b.iter().map(|s| s.to_string()).collect();
    for (start, end, new) in hunks {
        lines.splice(start..end, new);
    }
    lines.join("\n") + "\n"
}

pub const MULTI: [(&str, &str, [&str; 2]); 2] = [
    (
        "02-aimed-stream",
        "e02 + e07",
        ["e02-wrong-type", "e07-wrong-unit"],
    ),
    (
        "04-mirrored-spiral",
        "e03 + e09",
        ["e03-missing-field", "e09-comma-misuse"],
    ),
];

pub fn multi_block() -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Kombination | Syntax | Diagnosen | Meldungen (Position) |"
    );
    let _ = writeln!(o, "|---|---|---:|---|");
    for (base, label, ids) in MULTI {
        for syn in [Syntax::Ron, Syntax::Sigil] {
            let e = ext(syn);
            let base_src = read(&format!("corpus/{e}/{base}.{e}"));
            let variants: Vec<String> = ids
                .iter()
                .map(|id| read(&format!("corpus/errors/{id}.{e}")))
                .collect();
            let merged = merge_variants(&base_src, &variants);
            let out = check::check_src(syn, merged, &file(&format!("corpus/{e}")), 0);
            let msgs: Vec<String> = out
                .diags
                .iter()
                .map(|d| format!("{} ({})", cell(&d.cause), pos_str(d.pos)))
                .collect();
            let _ = writeln!(
                o,
                "| {label} | {} | {} | {} |",
                syn.name(),
                out.diags.len(),
                msgs.join("; ")
            );
        }
    }
    o
}

// ------------------------------------------------------------------------------------------
// Generability
// ------------------------------------------------------------------------------------------

pub struct GenResult {
    pub author: &'static str,
    pub brief: &'static str,
    pub syntax: Syntax,
    pub present: bool,
    pub outcome: Option<Outcome>,
}

pub fn generability_results() -> Vec<GenResult> {
    let mut out = Vec::new();
    for author in AUTHORS {
        for brief in BRIEFS {
            for syntax in [Syntax::Ron, Syntax::Sigil] {
                let rel = format!("generability/{author}/{brief}.{}", ext(syntax));
                let path = file(&rel);
                let present = path.exists();
                out.push(GenResult {
                    author,
                    brief,
                    syntax,
                    present,
                    outcome: present.then(|| check::check_file(&path)),
                });
            }
        }
    }
    out
}

pub fn generability_block(results: &[GenResult]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Autor | Muster | Syntax | Datei | Diagnosen | Arten (Phase/Art × Anzahl) | RON ≡ sigil 1 | ≡ anderer Autor |"
    );
    let _ = writeln!(o, "|---|---|---|:-:|---:|---|:-:|:-:|");
    let same_model = |a: Option<&GenResult>, b: Option<&GenResult>| match (
        a.and_then(|x| x.outcome.as_ref()),
        b.and_then(|x| x.outcome.as_ref()),
    ) {
        (Some(a), Some(b)) => yes(a.resolved.is_some() && a.resolved == b.resolved),
        _ => "–",
    };
    for r in results {
        let pair = results
            .iter()
            .find(|x| x.author == r.author && x.brief == r.brief && x.syntax != r.syntax);
        let equal = same_model(Some(r), pair);
        let other_author = results
            .iter()
            .find(|x| x.author != r.author && x.brief == r.brief && x.syntax == r.syntax);
        let cross = same_model(Some(r), other_author);
        match &r.outcome {
            None => {
                let _ = writeln!(
                    o,
                    "| {} | `{}` | {} | fehlt | – | – | – | – |",
                    r.author,
                    r.brief,
                    r.syntax.name()
                );
            }
            Some(out) => {
                let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
                for d in &out.diags {
                    *kinds
                        .entry(format!("{}/{}", d.phase.as_str(), d.kind))
                        .or_default() += 1;
                }
                let kinds = if kinds.is_empty() {
                    "–".to_string()
                } else {
                    kinds
                        .iter()
                        .map(|(k, n)| format!("{k} × {n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let _ = writeln!(
                    o,
                    "| {} | `{}` | {} | ja | {} | {} | {} | {} |",
                    r.author,
                    r.brief,
                    r.syntax.name(),
                    out.diags.len(),
                    kinds,
                    equal,
                    cross
                );
            }
        }
    }
    o
}

// ------------------------------------------------------------------------------------------
// Ergonomics
// ------------------------------------------------------------------------------------------

pub struct Metrics {
    pub lines: usize,
    pub nonblank: usize,
    pub comment_lines: usize,
    pub tokens: usize,
    pub chars: usize,
}

pub fn metrics(syn: Syntax, src: &str) -> Metrics {
    let lines = src.lines().count();
    let nonblank = src.lines().filter(|l| !l.trim().is_empty()).count();
    let comment_lines = src
        .lines()
        .filter(|l| l.trim_start().starts_with("//"))
        .count();
    let (tokens, chars) = match syn {
        Syntax::Sigil => {
            let (toks, _) = sigil::lex(src);
            let sig: Vec<_> = toks
                .iter()
                .filter(|t| {
                    !t.kind.is_trivia() && !matches!(t.kind, sigil::TK::Newline | sigil::TK::Eof)
                })
                .collect();
            (
                sig.len(),
                sig.iter()
                    .map(|t| src[t.start..t.end].chars().count())
                    .sum(),
            )
        }
        Syntax::Ron => {
            let toks = ron_cst::tokenize(src);
            let sig: Vec<_> = toks
                .iter()
                .filter(|t| !matches!(t.kind, ron_cst::RK::Space | ron_cst::RK::Comment))
                .collect();
            (
                sig.len(),
                sig.iter()
                    .map(|t| src[t.start..t.end].chars().count())
                    .sum(),
            )
        }
    };
    Metrics {
        lines,
        nonblank,
        comment_lines,
        tokens,
        chars,
    }
}

pub fn length_block() -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Muster | Zeilen RON / sigil 1 | davon Kommentarzeilen | signifikante Tokens RON / sigil 1 | Zeichen ohne Leerraum und Kommentare RON / sigil 1 |"
    );
    let _ = writeln!(o, "|---|---|---|---|---|");
    let mut tot = [0usize; 6];
    for p in PATTERNS {
        let r = metrics(Syntax::Ron, &read(&format!("corpus/ron/{p}.ron")));
        let s = metrics(Syntax::Sigil, &read(&format!("corpus/sigil/{p}.sigil")));
        for (t, v) in tot
            .iter_mut()
            .zip([r.lines, s.lines, r.tokens, s.tokens, r.chars, s.chars])
        {
            *t += v;
        }
        let _ = writeln!(
            o,
            "| `{p}` | {} / {} | {} / {} | {} / {} | {} / {} |",
            r.lines,
            s.lines,
            r.comment_lines,
            s.comment_lines,
            r.tokens,
            s.tokens,
            r.chars,
            s.chars
        );
    }
    let pct = |a: usize, b: usize| (b as f64 / a as f64 * 100.0).round() as i64;
    let _ = writeln!(
        o,
        "| **Summe** | **{} / {}** ({} %) | | **{} / {}** ({} %) | **{} / {}** ({} %) |",
        tot[0],
        tot[1],
        pct(tot[0], tot[1]),
        tot[2],
        tot[3],
        pct(tot[2], tot[3]),
        tot[4],
        tot[5],
        pct(tot[4], tot[5])
    );
    let _ = writeln!(o, "\nProzentwerte: sigil 1 relativ zu RON.");
    o
}

/// Unit newtypes of the RON form; `Ticks(20)` is four tokens, `20t` in sigil 1 one.
pub const UNIT_NEWTYPES: [&str; 6] = [
    "Ticks",
    "Units",
    "Deg",
    "UnitsPerTick",
    "UnitsPerTick2",
    "DegPerTick",
];

/// Significant tokens of one file split into commas, unit wrappers (name plus both
/// parentheses), header line and the rest.
pub fn token_kinds(syn: Syntax, src: &str) -> [usize; 4] {
    let header_end = match syn {
        Syntax::Ron => src
            .lines()
            .take_while(|l| l.starts_with("#!"))
            .map(|l| l.len() + 1)
            .sum::<usize>(),
        Syntax::Sigil => {
            if src.starts_with("sigil") {
                src.find('\n').unwrap_or(src.len())
            } else {
                0
            }
        }
    };
    let texts: Vec<(usize, &str)> = match syn {
        Syntax::Ron => ron_cst::tokenize(src)
            .into_iter()
            .filter(|t| !matches!(t.kind, ron_cst::RK::Space | ron_cst::RK::Comment))
            .map(|t| (t.start, &src[t.start..t.end]))
            .collect(),
        Syntax::Sigil => sigil::lex(src)
            .0
            .into_iter()
            .filter(|t| {
                !t.kind.is_trivia() && !matches!(t.kind, sigil::TK::Newline | sigil::TK::Eof)
            })
            .map(|t| (t.start, &src[t.start..t.end]))
            .collect(),
    };
    let mut k = [0usize; 4];
    for (start, text) in &texts {
        if *start < header_end {
            k[2] += 1;
        } else if *text == "," {
            k[0] += 1;
        } else if syn == Syntax::Ron && UNIT_NEWTYPES.contains(text) {
            // The wrapper's `(` and `)` are counted here as well, not under the rest.
            k[1] += 3;
        }
    }
    k[3] = texts.len() - k[0] - k[1] - k[2];
    k
}

pub fn token_kinds_block() -> String {
    let mut r = [0usize; 4];
    let mut s = [0usize; 4];
    for p in PATTERNS {
        let kr = token_kinds(Syntax::Ron, &read(&format!("corpus/ron/{p}.ron")));
        let ks = token_kinds(Syntax::Sigil, &read(&format!("corpus/sigil/{p}.sigil")));
        for i in 0..4 {
            r[i] += kr[i];
            s[i] += ks[i];
        }
    }
    let total_r: usize = r.iter().sum();
    let total_s: usize = s.iter().sum();
    let diff = total_r as i64 - total_s as i64;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Tokenart (alle fünf Muster) | RON | sigil 1 | Differenz | Anteil an der Differenz |"
    );
    let _ = writeln!(o, "|---|---:|---:|---:|---:|");
    let names = [
        "Trennkommas `,`",
        "Einheiten-Hüllen (`Ticks` `(` `)` usw., je 3 Tokens)",
        "Kopfzeile (`#![enable(implicit_some)]` bzw. `sigil 1`)",
        "übrige (Namen, Werte, Klammern, `:`/`=`, Schlüsselwörter)",
    ];
    for i in 0..4 {
        let d = r[i] as i64 - s[i] as i64;
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} % |",
            names[i],
            r[i],
            s[i],
            d,
            (d as f64 / diff as f64 * 100.0).round() as i64
        );
    }
    let _ = writeln!(
        o,
        "| **Summe** | **{total_r}** | **{total_s}** | **{diff}** | **100 %** |"
    );
    o
}

// ------------------------------------------------------------------------------------------
// Probes outside the scored error corpus
// ------------------------------------------------------------------------------------------

/// (id, description, syntax, base pattern, needle, replacement). Each probe changes a copy of
/// a corpus pattern in memory; the files on disk stay untouched.
pub const PROBES: &[(&str, &str, Syntax, &str, &str, &str)] = &[
    (
        "p01",
        "RON-Gewohnheit: `:` statt `=`",
        Syntax::Sigil,
        "01-ring-burst",
        "    count = 24\n",
        "    count: 24\n",
    ),
    (
        "p02",
        "zwei Felder auf einer Zeile",
        Syntax::Sigil,
        "01-ring-burst",
        "    count = 24\n    start = 7.5deg",
        "    count = 24 start = 7.5deg",
    ),
    (
        "p03",
        "Leerzeichen vor der Einheit",
        Syntax::Sigil,
        "01-ring-burst",
        "radius = 0.25u",
        "radius = 0.25 u",
    ),
    (
        "p04",
        "Wallclock-Einheit `30s`",
        Syntax::Sigil,
        "01-ring-burst",
        "delay = 30t",
        "delay = 30s",
    ),
    (
        "p05",
        "Einheit fehlt",
        Syntax::Sigil,
        "01-ring-burst",
        "speed = 0.05u/t",
        "speed = 0.05",
    ),
    (
        "p06",
        "Newtype fehlt",
        Syntax::Ron,
        "01-ring-burst",
        "speed: UnitsPerTick(0.05)",
        "speed: 0.05",
    ),
    (
        "p07",
        "Strukturname fehlt",
        Syntax::Ron,
        "01-ring-burst",
        "block: Ring(",
        "block: (",
    ),
    (
        "p08",
        "reservierte Einheit `beats`",
        Syntax::Sigil,
        "01-ring-burst",
        "delay = 30t",
        "delay = 30beats",
    ),
    (
        "p08",
        "reservierte Einheit `beats`",
        Syntax::Ron,
        "01-ring-burst",
        "delay: Ticks(30)",
        "delay: Beats(30)",
    ),
    (
        "p09",
        "Kaskadenzyklus (`mote` platzt in `seed`)",
        Syntax::Sigil,
        "03-subemitter-cascade",
        "    bullet = dust\n",
        "    bullet = seed\n",
    ),
    (
        "p09",
        "Kaskadenzyklus (`mote` platzt in `seed`)",
        Syntax::Ron,
        "03-subemitter-cascade",
        "bullet: \"dust\",",
        "bullet: \"seed\",",
    ),
    (
        "p10",
        "Kaskadentiefe 4 (`dust` platzt in neues `grain`)",
        Syntax::Sigil,
        "03-subemitter-cascade",
        "  despawn_vfx = dust_puff\n",
        "  despawn_vfx = dust_puff\n  transform burst {\n    when = time 20t\n    bullet = grain\n    speed = 0.05u/t\n    block ring {\n      count = 2\n      start = 0deg\n    }\n  }\n}\n\nbullet grain {\n  silhouette = star\n  palette = enemy.violet\n  glow = 0.2\n  radius = 0.06u\n  damage = 1\n  flags = [grazeable]\n",
    ),
    (
        "p10",
        "Kaskadentiefe 4 (`dust` platzt in neues `grain`)",
        Syntax::Ron,
        "03-subemitter-cascade",
        "            despawn_vfx: \"dust_puff\",\n        ),\n",
        "            despawn_vfx: \"dust_puff\",\n            transforms: [\n                Burst(\n                    when: Time(Ticks(20)),\n                    bullet: \"grain\",\n                    speed: UnitsPerTick(0.05),\n                    block: Ring(\n                        count: 2,\n                        start: Deg(0.0),\n                    ),\n                ),\n            ],\n        ),\n        Bullet(\n            name: \"grain\",\n            silhouette: \"star\",\n            palette: \"enemy.violet\",\n            glow: 0.2,\n            radius: Units(0.06),\n            damage: 1,\n            flags: [Grazeable],\n        ),\n",
    ),
];

pub struct ProbeResult {
    pub id: &'static str,
    pub title: &'static str,
    pub outcome: Outcome,
}

pub fn probe_results() -> Vec<ProbeResult> {
    PROBES
        .iter()
        .map(|(id, title, syn, base, needle, repl)| {
            let src = read(&format!("corpus/{}/{base}.{}", ext(*syn), ext(*syn)));
            assert_eq!(
                src.matches(needle).count(),
                1,
                "probe {id} ({}): needle must occur exactly once",
                syn.name()
            );
            let mutated = src.replacen(needle, repl, 1);
            let dir = file(&format!("corpus/{}", ext(*syn)));
            ProbeResult {
                id,
                title,
                outcome: check::check_src(*syn, mutated, &dir, 0),
            }
        })
        .collect()
}

pub fn probes_block(results: &[ProbeResult]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Sonde | Fehlerbild | Syntax | Diagnosen | Meldungen (Position) | Fix-Hinweis der ersten Meldung |"
    );
    let _ = writeln!(o, "|---|---|---|---:|---|---|");
    for r in results {
        let msgs = if r.outcome.diags.is_empty() {
            "– (keine Diagnose)".to_string()
        } else {
            r.outcome
                .diags
                .iter()
                .map(|d| format!("{} ({})", cell(&d.cause), pos_str(d.pos)))
                .collect::<Vec<_>>()
                .join("; ")
        };
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} | {} |",
            r.id,
            r.title,
            r.outcome.syntax.name(),
            r.outcome.diags.len(),
            msgs,
            r.outcome
                .diags
                .first()
                .and_then(|d| d.hint.as_deref())
                .map_or("–".to_string(), cell)
        );
    }
    o
}

/// Lines from the line containing `marker` to the closing line at the same indentation.
pub fn excerpt(src: &str, marker: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let Some(start) = lines.iter().position(|l| l.contains(marker)) else {
        return String::new();
    };
    let indent = lines[start].len() - lines[start].trim_start().len();
    let mut out = Vec::new();
    for l in &lines[start..] {
        out.push(&l[indent.min(l.len() - l.trim_start().len())..]);
        let li = l.len() - l.trim_start().len();
        if out.len() > 1
            && li == indent
            && (l.trim_start().starts_with(')') || l.trim_start().starts_with('}'))
        {
            break;
        }
    }
    out.join("\n")
}

pub fn spiral_block() -> String {
    let r = excerpt(&read("corpus/ron/04-mirrored-spiral.ron"), "SpeedCurve(");
    let s = excerpt(
        &read("corpus/sigil/04-mirrored-spiral.sigil"),
        "modifier speed_curve",
    );
    let (mr, ms) = (metrics(Syntax::Ron, &r), metrics(Syntax::Sigil, &s));
    let mut o = String::new();
    let _ = writeln!(
        o,
        "RON ({} Zeilen, {} signifikante Tokens, {} Zeichen):\n\n```ron\n{r}\n```\n",
        mr.lines, mr.tokens, mr.chars
    );
    let _ = writeln!(
        o,
        "sigil 1 ({} Zeilen, {} signifikante Tokens, {} Zeichen):\n\n```text\n{s}\n```",
        ms.lines, ms.tokens, ms.chars
    );
    o
}

pub fn effort_block() -> String {
    let count = |name: &str| -> usize {
        std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join(name),
        )
        .unwrap_or_default()
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("//")
        })
        .count()
    };
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Modul | Zweck | Codezeilen (ohne Leer- und Kommentarzeilen) |"
    );
    let _ = writeln!(o, "|---|---|---:|");
    for (m, what) in [
        (
            "sigil.rs",
            "sigil 1: Lexer, verlustfreier CST, Parser mit Wiederaufsetzen, Absenkung, `set`",
        ),
        (
            "tree.rs",
            "sigil 1: Wertebaum, serde-Deserializer mit Spans, Meldungstexte",
        ),
        (
            "ron_front.rs",
            "RON: `ron`-Aufruf, Fehlercodes → Diagnose und Hinweis",
        ),
        (
            "ron_cst.rs",
            "RON: verlustfreier Scanner für Positionen, Knotenpfade und `set`",
        ),
        ("model.rs", "gemeinsam: Datenmodell"),
        ("check.rs", "gemeinsam: Pipeline, Komposition, Validierung"),
        ("diag.rs", "gemeinsam: Diagnose, Darstellung"),
    ] {
        let _ = writeln!(o, "| `{m}` | {what} | {} |", count(m));
    }
    o
}

// ------------------------------------------------------------------------------------------
// Second rating
// ------------------------------------------------------------------------------------------

/// Cohen's kappa over paired 0–3 scores; `weighted` uses quadratic weights.
pub fn kappa(pairs: &[(u8, u8)], weighted: bool) -> f64 {
    let n = pairs.len() as f64;
    let w = |i: usize, j: usize| {
        if weighted {
            1.0 - (i as f64 - j as f64).powi(2) / 9.0
        } else if i == j {
            1.0
        } else {
            0.0
        }
    };
    let (mut pa, mut pb, mut po) = ([0f64; 4], [0f64; 4], 0.0);
    for &(a, b) in pairs {
        pa[usize::from(a)] += 1.0 / n;
        pb[usize::from(b)] += 1.0 / n;
        po += w(usize::from(a), usize::from(b)) / n;
    }
    let mut pe = 0.0;
    for (i, pai) in pa.iter().enumerate() {
        for (j, pbj) in pb.iter().enumerate() {
            pe += w(i, j) * pai * pbj;
        }
    }
    (po - pe) / (1.0 - pe)
}

fn de_decimal(x: f64) -> String {
    format!("{x:.2}").replace('.', ",")
}

pub fn second_rating_block(results: &[ErrorResult]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| Fall | Syntax | Ursache Claude | Ursache Codex | Fix-Hinweis Claude | Fix-Hinweis Codex | größter Abstand |"
    );
    let _ = writeln!(o, "|---|---|:-:|:-:|:-:|:-:|:-:|");
    let mut pairs: Vec<(u8, u8)> = Vec::new();
    let mut big: Vec<String> = Vec::new();
    // Index: 0 cause RON, 1 hint RON, 2 cause sigil 1, 3 hint sigil 1; then [Claude, Codex].
    let mut sums = [[0u32; 2]; 4];
    for (a, b) in RATINGS.iter().zip(RATINGS_CODEX) {
        assert_eq!(
            a.0, b.0,
            "RATINGS and RATINGS_CODEX must list the same cases"
        );
        let va = [a.1, a.2, a.3, a.4];
        let vb = [b.1, b.2, b.3, b.4];
        for i in 0..4 {
            sums[i][0] += u32::from(va[i]);
            sums[i][1] += u32::from(vb[i]);
            pairs.push((va[i], vb[i]));
        }
        for (syn, c, h) in [("RON", 0, 1), ("sigil 1", 2, 3)] {
            let gap = va[c].abs_diff(vb[c]).max(va[h].abs_diff(vb[h]));
            if gap >= 2 {
                big.push(format!("{} {syn}", a.0));
            }
            let _ = writeln!(
                o,
                "| {} | {syn} | {} | {} | {} | {} | {gap} |",
                a.0, va[c], vb[c], va[h], vb[h]
            );
        }
    }
    let _ = writeln!(
        o,
        "| **Summe RON (max. 30)** | | **{}** | **{}** | **{}** | **{}** | |",
        sums[0][0], sums[0][1], sums[1][0], sums[1][1]
    );
    let _ = writeln!(
        o,
        "| **Summe sigil 1 (max. 30)** | | **{}** | **{}** | **{}** | **{}** | |",
        sums[2][0], sums[2][1], sums[3][0], sums[3][1]
    );
    let exact = pairs.iter().filter(|(a, b)| a == b).count();
    let within_one = pairs.iter().filter(|(a, b)| a.abs_diff(*b) <= 1).count();
    let auto = |s: Syntax| -> u32 {
        results
            .iter()
            .filter(|r| r.syntax == s)
            .map(|r| u32::from(r.pos_score()) + u32::from(r.path_score()))
            .sum()
    };
    let (auto_r, auto_s) = (auto(Syntax::Ron), auto(Syntax::Sigil));
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "- Übereinstimmung: {exact} von {} Werten gleich ({} %), {within_one} von {} höchstens 1 Punkt auseinander.",
        pairs.len(),
        (exact as f64 / pairs.len() as f64 * 100.0).round() as i64,
        pairs.len()
    );
    let _ = writeln!(
        o,
        "- Cohens Kappa über alle {} Wertepaare: ungewichtet {}, quadratisch gewichtet {}.",
        pairs.len(),
        de_decimal(kappa(&pairs, false)),
        de_decimal(kappa(&pairs, true))
    );
    let _ = writeln!(
        o,
        "- Fälle mit einem Abstand von 2 oder mehr: {}.",
        if big.is_empty() {
            "keine".to_string()
        } else {
            big.join(", ")
        }
    );
    let _ = writeln!(
        o,
        "- Gesamtpunkte mit Position und Knotenpfad (max. 120): RON {} (Claude) bzw. {} (Codex), sigil 1 {} (Claude) bzw. {} (Codex).",
        auto_r + sums[0][0] + sums[1][0],
        auto_r + sums[0][1] + sums[1][1],
        auto_s + sums[2][0] + sums[3][0],
        auto_s + sums[2][1] + sums[3][1]
    );
    o
}

// ------------------------------------------------------------------------------------------
// results.md
// ------------------------------------------------------------------------------------------

pub fn blocks() -> Vec<(&'static str, String)> {
    let errors = error_results();
    let generability = generability_results();
    vec![
        ("korpus", corpus_block()),
        ("fehler", errors_block(&errors)),
        ("fehler-roh", raw_block(&errors)),
        ("qualitaet", quality_block(&errors)),
        ("zweitbewertung", second_rating_block(&errors)),
        ("mehrfach", multi_block()),
        ("modder", modder_block(&errors)),
        ("roundtrip", roundtrip_block()),
        ("ron-value", ron_value_block()),
        ("generierbarkeit", generability_block(&generability)),
        ("sonden", probes_block(&probe_results())),
        ("laenge", length_block()),
        ("token-arten", token_kinds_block()),
        ("spirale", spiral_block()),
        ("aufwand", effort_block()),
    ]
}

/// Replaces every generated block in `text`; blocks without markers are left out.
pub fn update_results(text: &str, blocks: &[(&str, String)]) -> String {
    let mut text = text.replace("\r\n", "\n");
    for (name, content) in blocks {
        let begin = format!("<!-- BEGIN GENERATED: {name} -->");
        let end = format!("<!-- END GENERATED: {name} -->");
        let (Some(b), Some(e)) = (text.find(&begin), text.find(&end)) else {
            continue;
        };
        if e < b {
            continue;
        }
        let content = content.trim_end();
        text = format!("{}{begin}\n{content}\n{}", &text[..b], &text[e..]);
    }
    text
}

pub fn results_path() -> PathBuf {
    root().join("results.md")
}

pub fn full_report() -> String {
    let mut o = String::new();
    for (name, content) in blocks() {
        let _ = writeln!(o, "## {name}\n\n{content}");
    }
    o
}

pub fn exists(p: &Path) -> bool {
    p.exists()
}
