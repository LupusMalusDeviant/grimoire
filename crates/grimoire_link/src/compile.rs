//! One `.sigil` file to unit bytes, under the canonical content path its `UnitId` derives from
//! (contract §11.1, §12; Plan 0002 WP8.5).
//!
//! The same compiler the asset pipeline uses (`grimoire_sigilc::compiler::compile`) with the same
//! path rules (`grimoire_sigilc::content_path`), so a unit pushed over the link is byte-identical
//! to the one `sigilc build` would write for that file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};
use grimoire_sigilc::content_path::{
    ContentPathError, canonical_content_path, validate_content_path,
};
use grimoire_sigilc::diagnostics::Diagnostic;

/// Largest `.sigil` source read, entry file or import (as `sigilc`'s CLI, contract §13 of
/// `docs/formats/sigil.md`).
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

/// A compiled unit, ready to push over the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledUnit {
    /// The canonical content path of the source, relative to the content root: what the unit's
    /// `UnitId` derives from and what `SwapSigilUnit.unit_path` carries.
    pub unit_path: String,
    /// The unit's canonical bytes.
    pub bytes: Vec<u8>,
    /// How long reading and compiling took.
    pub duration: Duration,
}

/// Why a file could not be compiled.
///
/// `#[non_exhaustive]`: new failure modes are additive (contract §2 rule 13).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// The file is not under the content root, or its path is no valid content path.
    #[error("{path}: {source}")]
    Path {
        /// The offending file.
        path: PathBuf,
        /// What the content-path rules said.
        source: ContentPathError,
    },
    /// The file could not be read.
    #[error("{path}: {message}")]
    Read {
        /// The offending file.
        path: PathBuf,
        /// What reading reported.
        message: String,
    },
    /// The compiler reported diagnostics; the unit was not built.
    #[error("{path}: {} diagnostic(s), the first is {}", diagnostics.len(), first_message(diagnostics))]
    Diagnostics {
        /// The offending file.
        path: PathBuf,
        /// Every diagnostic, in the order the compiler found them.
        diagnostics: Vec<Diagnostic>,
    },
}

fn first_message(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .first()
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .unwrap_or_else(|| "none".to_owned())
}

/// Reads and compiles `file` (under `root`) with `behavior_ids` for its `behaviour = <name>`
/// references (contract §11.5: a game assigns those ids).
///
/// # Errors
/// [`CompileError`] for a file outside the root, an unreadable file or any diagnostic.
pub fn compile_unit(
    root: &Path,
    file: &Path,
    behavior_ids: &BTreeMap<String, u32>,
) -> Result<CompiledUnit, CompileError> {
    let started = Instant::now();
    let unit_path = canonical_content_path(root, file).map_err(|source| CompileError::Path {
        path: file.to_path_buf(),
        source,
    })?;
    let source = read_source(file)?;
    let loader = RootLoader {
        root: root.to_path_buf(),
    };
    let output = compile(&unit_path, &source, &loader, behavior_ids);
    match output.bytes {
        Some(bytes) if output.diagnostics.is_empty() => Ok(CompiledUnit {
            unit_path,
            bytes,
            duration: started.elapsed(),
        }),
        _ => Err(CompileError::Diagnostics {
            path: file.to_path_buf(),
            diagnostics: output.diagnostics,
        }),
    }
}

/// Reads a source file, with the size limit and the line-ending normalisation the compiler's
/// callers use (`sigilc`'s CLI does the same: `\r\n` never changes compiled bytes).
fn read_source(file: &Path) -> Result<String, CompileError> {
    let read = |message: String| CompileError::Read {
        path: file.to_path_buf(),
        message,
    };
    let metadata = std::fs::metadata(file).map_err(|error| read(error.to_string()))?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(read(format!(
            "{} bytes exceed the {MAX_SOURCE_BYTES}-byte limit for a Sigil source",
            metadata.len()
        )));
    }
    let text = std::fs::read_to_string(file).map_err(|error| read(error.to_string()))?;
    Ok(text.replace("\r\n", "\n"))
}

/// Resolves `import "<path>"` against the content root, like `sigilc`'s CLI does.
struct RootLoader {
    root: PathBuf,
}

impl SourceLoader for RootLoader {
    fn load(&self, path: &str) -> Result<String, LoadError> {
        validate_content_path(path).map_err(|error| LoadError::Other(error.to_string()))?;
        let full = path
            .split('/')
            .fold(self.root.clone(), |full, segment| full.join(segment));
        read_source(&full).map_err(|error| LoadError::Other(error.to_string()))
    }
}
