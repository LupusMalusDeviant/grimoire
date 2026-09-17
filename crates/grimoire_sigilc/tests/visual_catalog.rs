//! The visual catalogue (`docs/formats/sigil.md` §10.10): `grimoire_sigilc::catalog` equals the
//! catalogue table of the format document, and every name compiles to its catalogue index in any
//! unit, independent of which other names the unit uses.
//!
//! The format document's table is the single source both sides are held to. This crate has no edge
//! to `grimoire_render` (contract §1), so the bullet pass tables are checked against the same table
//! by the facade's `tests/visual_catalog.rs`; together the two tests keep the compiler's indices and
//! the bullet pass rows from drifting apart.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use grimoire_sigilc::catalog::{ENEMY_PALETTES, SILHOUETTES};
use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};

/// One row of the catalogue table.
#[derive(Debug)]
struct Row {
    table: String,
    index: usize,
    name: String,
    drawn: bool,
}

/// Reads the rows between the `visual-catalog` markers of `docs/formats/sigil.md`.
fn catalogue_rows() -> Vec<Row> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/formats/sigil.md");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {path:?}: {error}"));
    let begin = text
        .find("<!-- visual-catalog:begin -->")
        .expect("begin marker in docs/formats/sigil.md");
    let end = text
        .find("<!-- visual-catalog:end -->")
        .expect("end marker in docs/formats/sigil.md");
    let rows: Vec<Row> = text[begin..end]
        .lines()
        .filter(|line| line.starts_with("| silhouette ") || line.starts_with("| enemy palette "))
        .map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 6, "malformed catalogue row: {line}");
            Row {
                table: cells[1].to_string(),
                index: cells[2]
                    .parse()
                    .unwrap_or_else(|_| panic!("index in {line}")),
                name: cells[3].trim_matches('`').to_string(),
                drawn: cells[4].starts_with("yes"),
            }
        })
        .collect();
    assert!(!rows.is_empty(), "no catalogue rows found");
    rows
}

fn table<'a>(rows: &'a [Row], name: &str) -> Vec<&'a Row> {
    rows.iter().filter(|row| row.table == name).collect()
}

#[test]
fn catalog_constants_equal_the_table_in_the_format_document() {
    let rows = catalogue_rows();
    for (table_name, constants) in [
        ("silhouette", SILHOUETTES),
        ("enemy palette", ENEMY_PALETTES),
    ] {
        let rows = table(&rows, table_name);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, constants, "{table_name} names or order differ");
        for (position, row) in rows.iter().enumerate() {
            assert_eq!(row.index, position, "{table_name} `{}` has a gap", row.name);
        }
        let drawn = rows.iter().take_while(|row| row.drawn).count();
        assert!(
            drawn > 0,
            "{table_name}: the bullet pass draws at least one row"
        );
        assert!(
            rows[drawn..].iter().all(|row| !row.drawn),
            "{table_name}: drawn rows must be a prefix of the catalogue"
        );
    }
}

struct NoImports;

impl SourceLoader for NoImports {
    fn load(&self, _path: &str) -> Result<String, LoadError> {
        Err(LoadError::NotFound)
    }
}

/// A unit with one bullet per `(silhouette, palette)` pair, named so the bullets sort in the given
/// order, returning each bullet type's `(silhouette, palette)` indices in that order.
fn compiled_visuals(pairs: &[(&str, &str)]) -> Vec<(u16, u16)> {
    let mut source = String::from("sigil 1\n");
    for (i, (silhouette, palette)) in pairs.iter().enumerate() {
        source.push_str(&format!(
            "\nbullet b{i:02} {{\n  silhouette = {silhouette}\n  palette = enemy.{palette}\n  glow = 0.5\n  radius = 0.2u\n  damage = 1\n  flags = []\n}}\n"
        ));
    }
    source.push_str(
        "\nemitter burst {\n  bullet = b00\n  repeat = 1\n  interval = 1t\n  speed = 0.1u/t\n  block ring {\n    count = 4\n  }\n}\n",
    );
    let output = compile("probe.sigil", &source, &NoImports, &BTreeMap::new());
    assert!(
        output.diagnostics.is_empty(),
        "{:#?}\n{source}",
        output.diagnostics
    );
    let unit = grimoire_sigil::SigilUnit::from_bytes(&output.bytes.unwrap()).unwrap();
    unit.bullet_types()
        .iter()
        .map(|bullet| (bullet.visual.silhouette, bullet.visual.palette))
        .collect()
}

#[test]
fn every_catalogue_name_compiles_to_its_row_index() {
    // Silhouettes must differ within a unit (SIG0020), palettes need not.
    let pairs: Vec<(&str, &str)> = SILHOUETTES
        .iter()
        .enumerate()
        .map(|(i, silhouette)| (*silhouette, ENEMY_PALETTES[i % ENEMY_PALETTES.len()]))
        .collect();
    let expected: Vec<(u16, u16)> = (0..SILHOUETTES.len())
        .map(|i| (i as u16, (i % ENEMY_PALETTES.len()) as u16))
        .collect();
    assert_eq!(compiled_visuals(&pairs), expected);

    // Every palette again, against a different silhouette, in reverse catalogue order.
    let reversed: Vec<(&str, &str)> = ENEMY_PALETTES
        .iter()
        .rev()
        .zip(SILHOUETTES)
        .map(|(palette, silhouette)| (*silhouette, *palette))
        .collect();
    let expected: Vec<(u16, u16)> = (0..ENEMY_PALETTES.len())
        .map(|i| (i as u16, (ENEMY_PALETTES.len() - 1 - i) as u16))
        .collect();
    assert_eq!(compiled_visuals(&reversed), expected);
}

#[test]
fn an_index_does_not_depend_on_the_other_names_of_the_unit() {
    // Under the former per-unit alphabetical numbering, both of these units would have written
    // silhouette 0 and palette 0 for every bullet.
    assert_eq!(compiled_visuals(&[("star", "violet")]), vec![(8, 6)]);
    assert_eq!(
        compiled_visuals(&[("star", "violet"), ("orb", "hex_magenta")]),
        vec![(8, 6), (0, 0)]
    );
    assert_eq!(
        compiled_visuals(&[("rice", "poison_lime"), ("diamond", "hex_magenta")]),
        vec![(1, 1), (2, 0)]
    );
}

#[test]
fn names_outside_the_catalogue_do_not_compile() {
    let codes = |silhouette: &str, palette: &str| -> Vec<&'static str> {
        let source = format!(
            "sigil 1\nbullet b {{\n  silhouette = {silhouette}\n  palette = {palette}\n  glow = 0.5\n  radius = 0.2u\n  damage = 1\n  flags = []\n}}\n\nemitter e {{\n  bullet = b\n  repeat = 1\n  interval = 1t\n  speed = 0.1u/t\n  block ring {{\n    count = 4\n  }}\n}}\n"
        );
        let output = compile("probe.sigil", &source, &NoImports, &BTreeMap::new());
        assert!(output.bytes.is_none() || output.diagnostics.is_empty());
        output.diagnostics.iter().map(|d| d.code).collect()
    };
    assert_eq!(codes("orb", "enemy.hex_magenta"), Vec::<&str>::new());
    assert_eq!(codes("wisp", "enemy.hex_magenta"), vec!["SIG0027"]);
    assert_eq!(codes("orb", "enemy.azure"), vec!["SIG0027"]);
    assert_eq!(codes("Orb", "enemy.Violet"), vec!["SIG0027", "SIG0027"]);
    // A foreign palette space is SIG0021 alone: the catalogue only covers `enemy`.
    assert_eq!(codes("orb", "player.azure"), vec!["SIG0021"]);
}
