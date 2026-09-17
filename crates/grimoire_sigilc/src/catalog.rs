//! The visual catalogue: stable names for bullet silhouettes and enemy palettes
//! (`docs/formats/sigil.md` §10.10; PO decision 2026-09-17, plan 0002 WP3.5/WP5.3).
//!
//! A Sigil source names a silhouette (`silhouette = orb`) and a palette (`palette =
//! enemy.hex_magenta`); the compiler writes the name's **catalogue index** into
//! `BulletType::visual` (`docs/formats/sigil.md` §10.3). The index of a name is the same in every
//! unit, so the facade's identity mapping (contract §9.9) lands on the bullet-pass table row of the
//! same name, whatever else a unit contains. Before this catalogue the compiler numbered the names a
//! unit happened to use alphabetically, and an index meant something different in every unit.
//!
//! The catalogue's first rows are exactly the bullet pass's tables, `grimoire_render::
//! bullet_silhouette::NAMES` and `bullet_palette::NAMES`, in their order; the rows after them are
//! reserved names the bullet pass does not draw yet, which compile, but which the facade counts as
//! unmapped. There is no dependency edge from this crate to `grimoire_render` (contract §1), so the
//! single source both sides are held to is the catalogue table in the format document:
//! `tests/visual_catalog.rs` checks these constants against it, and the facade's
//! `tests/visual_catalog.rs` checks the bullet pass tables against the same table.
//!
//! Rows are only ever appended. Renaming, reordering or removing a row changes what compiled units
//! mean and is an incompatible contract change (§2b).

/// Silhouette names, indexed by catalogue index. Rows `0..3` are the bullet pass's
/// `bullet_silhouette` table (`ORB`, `RICE`, `DIAMOND`); the rest are reserved.
pub const SILHOUETTES: &[&str] = &[
    "orb", "rice", "diamond", "blade", "crescent", "petal", "ring", "shard", "star",
];

/// Palette names of the `enemy` palette space (the only space Sigil bullets may use, PRD-0003
/// rule 4), indexed by catalogue index. Rows `0..2` are the bullet pass's `bullet_palette` table
/// (`HEX_MAGENTA`, `POISON_LIME`); the rest are reserved.
pub const ENEMY_PALETTES: &[&str] = &[
    "hex_magenta",
    "poison_lime",
    "amber",
    "crimson",
    "rose",
    "teal",
    "violet",
];

/// The catalogue index of a silhouette name, if the catalogue has it.
#[must_use]
pub fn silhouette_index(name: &str) -> Option<u16> {
    index_in(SILHOUETTES, name)
}

/// The catalogue index of an `enemy` palette name (the part after `enemy.`), if the catalogue has
/// it.
#[must_use]
pub fn enemy_palette_index(name: &str) -> Option<u16> {
    index_in(ENEMY_PALETTES, name)
}

fn index_in(table: &[&str], name: &str) -> Option<u16> {
    table
        .iter()
        .position(|entry| *entry == name)
        .and_then(|index| u16::try_from(index).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_follow_the_table_order() {
        assert_eq!(silhouette_index("orb"), Some(0));
        assert_eq!(silhouette_index("diamond"), Some(2));
        assert_eq!(silhouette_index("star"), Some(8));
        assert_eq!(silhouette_index("wisp"), None);
        assert_eq!(enemy_palette_index("hex_magenta"), Some(0));
        assert_eq!(enemy_palette_index("violet"), Some(6));
        assert_eq!(enemy_palette_index("Violet"), None);
    }

    #[test]
    fn names_are_unique_lowercase_identifiers() {
        for table in [SILHOUETTES, ENEMY_PALETTES] {
            assert!(table.len() < usize::from(u16::MAX));
            for (index, name) in table.iter().enumerate() {
                assert!(
                    name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                    "{name}"
                );
                assert!(!table[..index].contains(name), "{name} is listed twice");
            }
        }
    }
}
