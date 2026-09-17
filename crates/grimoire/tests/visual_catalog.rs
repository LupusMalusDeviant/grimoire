//! The bullet pass tables are the drawn rows of the visual catalogue (`docs/formats/sigil.md`
//! §10.10), and `map_visual` lands every catalogue row on the table row of the same name.
//!
//! `sigilc` compiles a silhouette or palette name to its row in the catalogue table of the format
//! document (checked by `grimoire_sigilc`'s `tests/visual_catalog.rs`). This test holds the other
//! side to the same table: `grimoire_render::bullet_silhouette::NAMES` and `bullet_palette::NAMES`
//! must equal its drawn rows, in order. Together the two tests are the no-drift guarantee between
//! compiler and bullet pass, without a crate edge between them (contract §1).

use std::fs;
use std::path::PathBuf;

use grimoire::adapters::sigil_render::map_visual;
use grimoire::render::{bullet_palette, bullet_silhouette, palette_space};
use grimoire::sigil::BulletVisual;

/// One row of the catalogue table.
struct Row {
    table: String,
    index: u16,
    name: String,
    drawn: bool,
}

/// Reads the rows between the `visual-catalog` markers of `docs/formats/sigil.md`.
fn catalogue_rows(table: &str) -> Vec<Row> {
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
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            (cells.len() == 6 && cells[1] == table).then(|| Row {
                table: cells[1].to_string(),
                index: cells[2]
                    .parse()
                    .unwrap_or_else(|_| panic!("index in {line}")),
                name: cells[3].trim_matches('`').to_string(),
                drawn: cells[4].starts_with("yes"),
            })
        })
        .collect();
    assert!(!rows.is_empty(), "no `{table}` rows in the catalogue table");
    rows
}

fn drawn_names(rows: &[Row]) -> Vec<&str> {
    let drawn: Vec<&str> = rows
        .iter()
        .take_while(|row| row.drawn)
        .map(|row| row.name.as_str())
        .collect();
    assert!(
        rows[drawn.len()..].iter().all(|row| !row.drawn),
        "`{}`: drawn rows must be a prefix of the catalogue",
        rows[0].table
    );
    drawn
}

#[test]
fn bullet_pass_tables_are_the_drawn_rows_of_the_catalogue() {
    let silhouettes = catalogue_rows("silhouette");
    assert_eq!(
        drawn_names(&silhouettes),
        bullet_silhouette::NAMES,
        "bullet_silhouette::NAMES must equal the drawn silhouette rows, in order"
    );
    assert_eq!(
        usize::from(bullet_silhouette::COUNT),
        bullet_silhouette::NAMES.len()
    );

    let palettes = catalogue_rows("enemy palette");
    assert_eq!(
        drawn_names(&palettes),
        bullet_palette::NAMES,
        "bullet_palette::NAMES must equal the drawn enemy palette rows, in order"
    );
    assert_eq!(
        usize::from(bullet_palette::COUNT),
        bullet_palette::NAMES.len()
    );

    for rows in [&silhouettes, &palettes] {
        for (position, row) in rows.iter().enumerate() {
            assert_eq!(usize::from(row.index), position, "gap at `{}`", row.name);
        }
    }
}

#[test]
fn map_visual_lands_every_catalogue_row_on_the_table_row_of_the_same_name() {
    let visual = |silhouette: u16, palette: u16| BulletVisual {
        silhouette,
        palette,
        palette_space: palette_space::HOSTILE,
        glow: 128,
    };
    for row in catalogue_rows("silhouette") {
        let mapped = map_visual(visual(row.index, 0));
        if row.drawn {
            let mapped = mapped.unwrap_or_else(|| panic!("drawn `{}` must map", row.name));
            assert_eq!(
                bullet_silhouette::NAMES[usize::from(mapped.silhouette)],
                row.name
            );
        } else {
            assert_eq!(mapped, None, "reserved `{}` must stay unmapped", row.name);
        }
    }
    for row in catalogue_rows("enemy palette") {
        let mapped = map_visual(visual(0, row.index));
        if row.drawn {
            let mapped = mapped.unwrap_or_else(|| panic!("drawn `{}` must map", row.name));
            assert_eq!(bullet_palette::NAMES[usize::from(mapped.palette)], row.name);
        } else {
            assert_eq!(mapped, None, "reserved `{}` must stay unmapped", row.name);
        }
    }
}
