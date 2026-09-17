//! `pack_inspect`: prints what a pack v1 file contains and checks every entry (Plan 0002 WP8.3,
//! `docs/formats/pack.md`). Console only, no window.
//!
//! ```text
//! cargo run -p grimoire_assets --example pack_inspect -- <file.grimpack>
//! ```
//!
//! Prints the manifest (compiler, compiler version, application block), then one line per entry in
//! table-of-contents order: asset id, kind, kind version, payload length, SHA-256 prefix, the
//! result of reading the entry (which verifies its SHA-256) and its path, and finally the content
//! hash of the whole pack. Exit code `0` if the pack parses and every entry reads back, `1` for a
//! structural error or a failed entry, `2` for a usage error. A malformed pack is reported as the
//! reader's error, never as a panic.

use std::path::Path;
use std::process::ExitCode;

use grimoire_assets::{AssetKind, AssetSource, PackReader};
use grimoire_platform::StdFileSystem;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        eprintln!("usage: pack_inspect <file.grimpack>");
        return ExitCode::from(2);
    };
    let path = Path::new(&path);

    let reader = match PackReader::open(&StdFileSystem, path) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            return ExitCode::from(1);
        }
    };

    let manifest = reader.manifest();
    println!("pack      {}", path.display());
    println!(
        "compiler  {} {}",
        manifest.compiler(),
        manifest.compiler_version()
    );
    println!(
        "app block {} bytes{}",
        manifest.application().len(),
        match std::str::from_utf8(manifest.application()) {
            Ok(text) if !text.is_empty() => format!(" ({text:?})"),
            _ => String::new(),
        }
    );
    println!("entries   {}", reader.entries().len());
    println!();
    println!(
        "{:<16}  {:<14}  {:>7}  {:>10}  {:<16}  {:<6}  path",
        "asset id", "kind", "version", "bytes", "sha-256", "read"
    );

    let mut failed = false;
    for entry in reader.entries() {
        let sha: String = entry.sha256.0[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let read = match reader.read(entry.id) {
            Ok(_) => "ok".to_owned(),
            Err(error) => {
                failed = true;
                format!("FAILED: {error}")
            }
        };
        let path = manifest.path_of(entry.id).map_or("?", |path| path.as_str());
        println!(
            "{}  {:<14}  {:>7}  {:>10}  {:<16}  {:<6}  {}",
            entry.id,
            kind_name(entry.kind),
            entry.kind_version,
            entry.len,
            sha,
            read,
            path
        );
    }

    let content_hash: String = reader
        .content_hash()
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!();
    println!("content hash {content_hash}");

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// A readable name for an entry kind (contract §12: `1` Sigil, `2..=5` reserved engine kinds a v1
/// reader never hands out, `0x8000..=0xFFFF` application-defined).
fn kind_name(kind: AssetKind) -> String {
    match kind {
        AssetKind::SIGIL => "sigil".to_owned(),
        AssetKind(raw @ 0x8000..=0xFFFF) => format!("app 0x{raw:04x}"),
        AssetKind(raw) => format!("0x{raw:04x}"),
    }
}
