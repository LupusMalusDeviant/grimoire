//! # grimoire_bench
//!
//! Benchmark scenarios and JSON-schema types for bench results and golden masters
//! (contract §15), so that engine tooling and application harnesses share the same readers and
//! writers.
//!
//! Outside the determinism set (engine ADR-0008, contract §1, §3): this is the only engine crate
//! allowed to read the wall clock, and it measures with 1 and N threads via `grimoire_exec`. Wall
//! clock samples never enter simulation state or subsystem hashes. It deliberately carries no
//! `clippy.toml`.
//!
//! **Status:** skeleton (Plan-0002 WP1.3). Bench scenarios under `benches/`, the full
//! `BenchResult` (contract §15.1) and `GoldenMaster` (§15.2) schema types, and wall-clock
//! measurement with 1 and N threads via `grimoire_exec` all land in WP6.
//!
//! Contract §2 rule 11 requires every `u64` hash, id or seed that crosses a JSON interface to be
//! encoded as exactly 16 lowercase hex digits, never as a JSON number (a JSON number can only
//! round-trip integers up to 2^53 - 1 through a typical parser). [`u64_as_hex16`] and
//! [`u64_from_hex16`] are that one rule as a small, reusable primitive; the full schema types that
//! use it land in WP6.

use std::fmt;

/// Error returned by [`u64_from_hex16`] when the input is not exactly 16 lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HexU64Error {
    /// The input's length in bytes was not exactly 16.
    WrongLength {
        /// The number of bytes actually found.
        found: usize,
    },
    /// The input contained a byte that is not a lowercase hex digit (`0`-`9`, `a`-`f`).
    NotLowercaseHex,
}

impl fmt::Display for HexU64Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HexU64Error::WrongLength { found } => {
                write!(f, "expected exactly 16 hex digits, found {found}")
            }
            HexU64Error::NotLowercaseHex => {
                write!(f, "expected only lowercase hex digits (0-9, a-f)")
            }
        }
    }
}

impl std::error::Error for HexU64Error {}

/// Formats `value` as exactly 16 lowercase hex digits (contract §2 rule 11).
///
/// ```
/// assert_eq!(grimoire_bench::u64_as_hex16(0), "0000000000000000");
/// assert_eq!(grimoire_bench::u64_as_hex16(0xdead_beef), "00000000deadbeef");
/// ```
#[must_use]
pub fn u64_as_hex16(value: u64) -> String {
    format!("{value:016x}")
}

/// Parses exactly 16 lowercase hex digits back into a `u64` (contract §2 rule 11).
///
/// Rejects any input whose byte length is not exactly 16, and any input containing a byte
/// outside `0-9`/`a-f` (uppercase hex included) — this format never produces those, so a reader
/// must not silently accept them.
///
/// ```
/// assert_eq!(grimoire_bench::u64_from_hex16("00000000deadbeef"), Ok(0xdead_beef));
/// assert!(grimoire_bench::u64_from_hex16("DEADBEEF00000000").is_err());
/// assert!(grimoire_bench::u64_from_hex16("abc").is_err());
/// ```
pub fn u64_from_hex16(text: &str) -> Result<u64, HexU64Error> {
    if text.len() != 16 {
        return Err(HexU64Error::WrongLength { found: text.len() });
    }
    if !text
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(HexU64Error::NotLowercaseHex);
    }
    u64::from_str_radix(text, 16).map_err(|_| HexU64Error::NotLowercaseHex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_zero() {
        assert_eq!(u64_from_hex16(&u64_as_hex16(0)).unwrap(), 0);
    }

    #[test]
    fn round_trips_max() {
        assert_eq!(u64_from_hex16(&u64_as_hex16(u64::MAX)).unwrap(), u64::MAX);
    }

    #[test]
    fn round_trips_arbitrary_values() {
        for value in [1u64, 42, 0xdead_beef, 0x1234_5678_9abc_def0, u64::MAX / 3] {
            let text = u64_as_hex16(value);
            assert_eq!(text.len(), 16);
            assert_eq!(u64_from_hex16(&text).unwrap(), value);
        }
    }

    #[test]
    fn rejects_wrong_length() {
        assert_eq!(
            u64_from_hex16("abc"),
            Err(HexU64Error::WrongLength { found: 3 })
        );
        assert_eq!(
            u64_from_hex16("00000000deadbeef0"),
            Err(HexU64Error::WrongLength { found: 17 })
        );
        assert_eq!(
            u64_from_hex16(""),
            Err(HexU64Error::WrongLength { found: 0 })
        );
    }

    #[test]
    fn rejects_uppercase_hex() {
        assert_eq!(
            u64_from_hex16("00000000DEADBEEF"),
            Err(HexU64Error::NotLowercaseHex)
        );
    }

    #[test]
    fn rejects_non_hex_characters() {
        assert_eq!(
            u64_from_hex16("000000000000000g"),
            Err(HexU64Error::NotLowercaseHex)
        );
    }

    #[test]
    fn output_is_always_lowercase() {
        let text = u64_as_hex16(0xABCDEF);
        assert_eq!(text, text.to_lowercase());
    }
}
