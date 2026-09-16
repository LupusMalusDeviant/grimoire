//! Sigil compiler: name resolution, imports/composition with parameter overrides, static
//! validation and lowering to `grimoire_sigil::SigilUnit` bytes (Plan 0002 WP4.2).
//!
//! Scope, relative to the parser (WP4.1): the parser is schema-blind by design (field names,
//! units-per-field, ranges, cascade depth and name resolution are all unchecked there). This
//! module is exactly that missing schema pass, plus the compiler proper. See `docs/formats/sigil.md`
//! for the full diagnostic-code table (this module continues it from `SIG0012`) and the binary
//! format's section layouts.

mod diagnose;
mod lower;
mod model;
mod resolve;
mod validate;

pub use resolve::{LoadError, SourceLoader};

use std::collections::BTreeMap;

use crate::diagnostics::Diagnostic;

/// Result of compiling one entry `.sigil` file: either compiled unit bytes, or every diagnostic
/// collected while trying (parser diagnostics from the entry file and every file it transitively
/// imports, plus this module's own schema/resolution/validation diagnostics).
#[derive(Debug)]
pub struct CompileOutput {
    /// The compiled unit's canonical bytes (contract §11.1), present iff `diagnostics` is empty.
    pub bytes: Option<Vec<u8>>,
    /// Every diagnostic collected, across every file involved, in the order found.
    pub diagnostics: Vec<Diagnostic>,
}

/// Compiles `entry_path` (already read into `entry_source`) into a [`grimoire_sigil::SigilUnit`]'s
/// canonical bytes.
///
/// `loader` resolves the paths named by `import "<path>" as <alias>` items, relative to whatever
/// convention the caller wants (a content root on disk, an in-memory corpus for tests, ...); this
/// function never touches a filesystem itself.
///
/// `behavior_ids` maps a Sigil `behaviour = <name>` reference to the numeric
/// `grimoire_sigil::BehaviorId` a game's Rust code registered it under (contract §11.5: ids are
/// assigned by whoever calls `BehaviorRegistryBuilder::register`, not derived from the name, so
/// this crate cannot compute them on its own). A name absent from this map is reported as an
/// unknown reference, exactly like an unknown bullet or emitter reference; see this module's
/// top-level docs and the pull request description for why a real build pipeline supplying this
/// map is left to Plan 0002 WP4.3.
#[must_use]
pub fn compile(
    entry_path: &str,
    entry_source: &str,
    loader: &dyn SourceLoader,
    behavior_ids: &BTreeMap<String, u32>,
) -> CompileOutput {
    let (workspace, mut diagnostics) =
        match resolve::load_workspace(entry_path, entry_source, loader) {
            Ok(workspace) => workspace,
            Err(diagnostics) => {
                return CompileOutput {
                    bytes: None,
                    diagnostics,
                };
            }
        };

    let resolved = match resolve::resolve_entry(&workspace, entry_path) {
        Ok(resolved) => resolved,
        Err(diagnostic) => {
            diagnostics.push(*diagnostic);
            return CompileOutput {
                bytes: None,
                diagnostics,
            };
        }
    };

    let mut validation_diagnostics = validate::validate(&workspace, &resolved, behavior_ids);
    diagnostics.append(&mut validation_diagnostics);

    if !diagnostics.is_empty() {
        return CompileOutput {
            bytes: None,
            diagnostics,
        };
    }

    match lower::lower(&resolved, behavior_ids) {
        Ok(bytes) => CompileOutput {
            bytes: Some(bytes),
            diagnostics,
        },
        Err(error) => {
            diagnostics.push(diagnose::diagnostic(
                "SIG0014",
                entry_path,
                diagnose::unresolved_position(),
                "",
                "",
                format!("Could not derive a unit id: {error}"),
                "This is astronomically unlikely; try a different canonical content path.",
            ));
            CompileOutput {
                bytes: None,
                diagnostics,
            }
        }
    }
}

#[cfg(test)]
mod tests;
