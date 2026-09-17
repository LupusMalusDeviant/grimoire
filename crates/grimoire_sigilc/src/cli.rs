//! The `sigilc` command-line interface (Plan 0002 WP4.3; `docs/formats/sigil.md` §13).
//!
//! `src/bin/sigilc.rs` only forwards `argv` and the standard streams to [`run`]; everything else
//! lives here so the commands can be tested in-process. Commands:
//!
//! - `check [--json] [--root <dir>] [--behaviors <file>] <file>...` compiles without writing.
//! - `build [--json] --root <dir> --out <dir> [--behaviors <file>] <file>...` compiles and writes
//!   one `<out>/<canonical path without .sigil>.unit` per source.
//! - `parse --json [--values] <file>` prints the parser's diagnostics document, with `--values`
//!   the value listing of [`crate::edit::list_values`] as well.
//! - `set [--json] <file> <node-path>=<value>` rewrites one value losslessly
//!   ([`crate::edit::set_value`]).
//! - `fmt [--check] <file>...` applies the canonical layout ([`crate::fmt::format_canonical`]).
//!
//! Exit codes: [`EXIT_OK`] when the command succeeded with no problem in any file,
//! [`EXIT_PROBLEMS`] when it ran but found problems (diagnostics, a refused `set`, a file `fmt
//! --check` would change), [`EXIT_ERROR`] when it could not run as asked (usage error, unreadable
//! input for `parse`/`set`/`fmt`, invalid behaviour manifest, unwritable output). Malformed input
//! of any kind ends in one of these codes, never in a panic (contract §2 rule 9).
//!
//! The CLI compiles on the calling thread only and derives nothing from the wall clock; its
//! output bytes depend only on its inputs (contract §3).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::behaviors::{MAX_BEHAVIOR_MANIFEST_BYTES, parse_behavior_manifest};
use crate::compiler::{LoadError, SourceLoader, compile};
use crate::content_path::{SIGIL_EXTENSION, canonical_content_path, validate_content_path};
use crate::diagnostics::{Diagnostic, DiagnosticsDocument};
use crate::edit::{SetError, list_values, set_value};
use crate::fmt::{FormatError, format_canonical};
use crate::parser::parse;
use crate::span::Position;

/// The command succeeded and found no problem.
pub const EXIT_OK: u8 = 0;
/// The command ran but found problems: diagnostics, a refused `set`, or a file `fmt --check`
/// would change.
pub const EXIT_PROBLEMS: u8 = 1;
/// The command could not run as asked: usage error, unreadable input for `parse`/`set`/`fmt`,
/// invalid behaviour manifest, or output that could not be written.
pub const EXIT_ERROR: u8 = 2;

/// Largest Sigil source file the CLI reads (entry files and imports), in bytes.
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

const BUILD_SCHEMA: &str = "grimoire.sigilc.build";
const VALUES_SCHEMA: &str = "grimoire.sigilc.values";
const SET_SCHEMA: &str = "grimoire.sigilc.set";
const SCHEMA_VERSION: u32 = 1;

/// Runs `sigilc` with `args` (without the program name), writing to `stdout` and `stderr`, and
/// returns the process exit code ([`EXIT_OK`], [`EXIT_PROBLEMS`] or [`EXIT_ERROR`]).
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let result = to_strings(args).and_then(|args| dispatch(&args, stdout));
    let flushed = stdout.flush();
    match (result, flushed) {
        (Ok(code), Ok(())) => code,
        (Ok(_), Err(error)) => {
            let _ = writeln!(stderr, "sigilc: could not write output: {error}");
            EXIT_ERROR
        }
        (Err(error), _) => {
            let _ = writeln!(stderr, "sigilc: {}", error.0);
            if error.1 {
                let _ = writeln!(stderr, "Run `sigilc --help` for usage.");
            }
            EXIT_ERROR
        }
    }
}

/// A failure that ends the command with [`EXIT_ERROR`]: the message, and whether to point at the
/// usage text.
struct CliError(String, bool);

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self(message.into(), true)
    }

    fn failed(message: impl Into<String>) -> Self {
        Self(message.into(), false)
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::failed(format!("could not write output: {error}"))
    }
}

type CliResult = Result<u8, CliError>;

fn to_strings(args: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, CliError> {
    args.into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|arg| CliError::usage(format!("argument {arg:?} is not valid Unicode")))
        })
        .collect()
}

fn dispatch(args: &[String], stdout: &mut dyn Write) -> CliResult {
    let Some((command, rest)) = args.split_first() else {
        return Err(CliError::usage("expected a command"));
    };
    match command.as_str() {
        "--help" | "-h" | "help" => {
            stdout.write_all(USAGE.as_bytes())?;
            Ok(EXIT_OK)
        }
        "--version" | "-V" => {
            writeln!(stdout, "sigilc {}", env!("CARGO_PKG_VERSION"))?;
            Ok(EXIT_OK)
        }
        "check" => run_compile(Mode::Check, rest, stdout),
        "build" => run_compile(Mode::Build, rest, stdout),
        "parse" => run_parse(rest, stdout),
        "set" => run_set(rest, stdout),
        "fmt" => run_fmt(rest, stdout),
        other => Err(CliError::usage(format!("unknown command `{other}`"))),
    }
}

const USAGE: &str = "\
sigilc: offline compiler for Sigil (.sigil) sources (docs/formats/sigil.md section 13)

USAGE:
  sigilc check [--json] [--root <dir>] [--behaviors <file>] <file>...
      Compile without writing. The content root defaults to each file's directory.
  sigilc build [--json] --root <dir> --out <dir> [--behaviors <file>] <file>...
      Compile and write <out>/<canonical path without .sigil>.unit per source.
  sigilc parse --json [--values] <file>
      Print the parse diagnostics as JSON; with --values also every value and its node path.
  sigilc set [--json] <file> <node-path>=<value>
      Replace one existing value in place; nothing else in the file changes.
  sigilc fmt [--check] <file>...
      Rewrite files in the canonical layout; with --check only report files that would change.
  sigilc --version | --help

OPTIONS:
  --root <dir>        Content root: the canonical content path (and the unit id) of a source is
                      its path relative to this directory; imports resolve against it too.
  --out <dir>         Output directory of build.
  --behaviors <file>  Behaviour manifest (schema grimoire.sigilc.behaviors) naming the
                      BehaviorId of every `behaviour = <name>` reference.
  --json              Print one JSON document instead of text.

EXIT CODES:
  0  success, no problems
  1  the command ran and found problems (diagnostics, refused set, unformatted file)
  2  the command could not run (usage, unreadable input, invalid manifest, unwritable output)

Not implemented here: `simulate` (Plan 0002 WP5.6), `migrate` (no second source version exists).
";

/// Parsed command-line arguments of one command.
struct Args {
    flags: Vec<&'static str>,
    options: BTreeMap<&'static str, String>,
    positionals: Vec<String>,
}

impl Args {
    fn parse(
        command: &str,
        args: &[String],
        flag_names: &[&'static str],
        option_names: &[&'static str],
    ) -> Result<Self, CliError> {
        let mut parsed = Args {
            flags: Vec::new(),
            options: BTreeMap::new(),
            positionals: Vec::new(),
        };
        let mut index = 0;
        let mut only_positionals = false;
        while index < args.len() {
            let arg = &args[index];
            index += 1;
            if only_positionals || !arg.starts_with('-') || arg == "-" {
                parsed.positionals.push(arg.clone());
                continue;
            }
            if arg == "--" {
                only_positionals = true;
                continue;
            }
            let (name, inline_value) = match arg.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (arg.as_str(), None),
            };
            if let Some(flag) = flag_names.iter().find(|flag| **flag == name) {
                if inline_value.is_some() {
                    return Err(CliError::usage(format!(
                        "sigilc {command}: `{name}` takes no value"
                    )));
                }
                if parsed.flags.contains(flag) {
                    return Err(CliError::usage(format!(
                        "sigilc {command}: `{name}` given twice"
                    )));
                }
                parsed.flags.push(flag);
            } else if let Some(option) = option_names.iter().find(|option| **option == name) {
                let value = match inline_value {
                    Some(value) => value,
                    None => {
                        let Some(value) = args.get(index) else {
                            return Err(CliError::usage(format!(
                                "sigilc {command}: `{name}` needs a value"
                            )));
                        };
                        index += 1;
                        value.clone()
                    }
                };
                if value.is_empty() {
                    return Err(CliError::usage(format!(
                        "sigilc {command}: `{name}` needs a non-empty value"
                    )));
                }
                if parsed.options.insert(option, value).is_some() {
                    return Err(CliError::usage(format!(
                        "sigilc {command}: `{name}` given twice"
                    )));
                }
            } else {
                return Err(CliError::usage(format!(
                    "sigilc {command}: unknown option `{name}`"
                )));
            }
        }
        Ok(parsed)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }
}

/// Reads a UTF-8 text file of at most `max` bytes. The error is a human-readable reason.
fn read_text(path: &Path, max: u64) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > max {
        return Err(format!("the file is larger than {max} bytes"));
    }
    String::from_utf8(bytes).map_err(|_| "the file is not valid UTF-8".to_string())
}

// ---- check / build ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Check,
    Build,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Check => "check",
            Mode::Build => "build",
        }
    }
}

/// The `check`/`build` JSON document (`docs/formats/sigil.md` §13.4).
#[derive(Serialize)]
struct BuildDocument<'a> {
    schema: &'static str,
    schema_version: u32,
    command: &'static str,
    ok: bool,
    units: &'a [UnitReport],
}

/// One source's entry in [`BuildDocument`].
#[derive(Serialize)]
struct UnitReport {
    source: String,
    unit_path: Option<String>,
    output: Option<String>,
    unit_id: Option<String>,
    content_hash: Option<String>,
    size: Option<u64>,
    diagnostics: Vec<Diagnostic>,
}

/// Loads imports relative to the content root; an import path that is not a canonical content
/// path is refused before anything is read, so no import can reach outside the root.
struct RootLoader<'a> {
    root: &'a Path,
}

impl SourceLoader for RootLoader<'_> {
    fn load(&self, path: &str) -> Result<String, LoadError> {
        validate_content_path(path).map_err(|error| LoadError::Other(error.to_string()))?;
        let full = path
            .split('/')
            .fold(self.root.to_path_buf(), |full, segment| full.join(segment));
        read_text(&full, MAX_SOURCE_BYTES).map_err(LoadError::Other)
    }
}

fn run_compile(mode: Mode, args: &[String], stdout: &mut dyn Write) -> CliResult {
    let command = mode.name();
    let option_names: &[&'static str] = match mode {
        Mode::Check => &["--root", "--behaviors"],
        Mode::Build => &["--root", "--out", "--behaviors"],
    };
    let args = Args::parse(command, args, &["--json"], option_names)?;
    if args.positionals.is_empty() {
        return Err(CliError::usage(format!(
            "sigilc {command}: expected at least one source file"
        )));
    }
    let root = args.option("--root");
    let out = args.option("--out");
    if mode == Mode::Build && (root.is_none() || out.is_none()) {
        return Err(CliError::usage(
            "sigilc build: `--root <dir>` and `--out <dir>` are required (the unit id derives from the path relative to the root)",
        ));
    }
    let behaviors = match args.option("--behaviors") {
        Some(path) => {
            let text = read_text(Path::new(path), MAX_BEHAVIOR_MANIFEST_BYTES as u64).map_err(
                |reason| {
                    CliError::failed(format!(
                        "sigilc {command}: cannot read behaviour manifest `{path}`: {reason}"
                    ))
                },
            )?;
            parse_behavior_manifest(&text)
                .map_err(|error| CliError::failed(format!("sigilc {command}: `{path}`: {error}")))?
        }
        None => BTreeMap::new(),
    };

    let mut reports = Vec::with_capacity(args.positionals.len());
    for file in &args.positionals {
        reports.push(compile_one(file, root, out, &behaviors)?);
    }
    let ok = reports.iter().all(|report| report.diagnostics.is_empty());

    if args.flag("--json") {
        let document = BuildDocument {
            schema: BUILD_SCHEMA,
            schema_version: SCHEMA_VERSION,
            command,
            ok,
            units: &reports,
        };
        write_json(stdout, &document)?;
    } else {
        for report in &reports {
            for diagnostic in &report.diagnostics {
                writeln!(stdout, "{}", diagnostic.render_text())?;
            }
            if let (Some(output), Some(unit_id), Some(content_hash), Some(size)) = (
                &report.output,
                &report.unit_id,
                &report.content_hash,
                report.size,
            ) {
                writeln!(
                    stdout,
                    "built {} -> {output} (unit {unit_id}, content hash {content_hash}, {size} bytes)",
                    report.source
                )?;
            }
        }
    }
    Ok(if ok { EXIT_OK } else { EXIT_PROBLEMS })
}

fn compile_one(
    file: &str,
    root: Option<&str>,
    out: Option<&str>,
    behaviors: &BTreeMap<String, u32>,
) -> Result<UnitReport, CliError> {
    let file_path = Path::new(file);
    let (root_path, root_display) = match root {
        Some(root) => (PathBuf::from(root), trim_separators(root).to_string()),
        None => match file_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            Some(parent) => (
                parent.to_path_buf(),
                trim_separators(&parent.to_string_lossy()).to_string(),
            ),
            None => (PathBuf::from("."), String::new()),
        },
    };
    let mut report = UnitReport {
        source: file.to_string(),
        unit_path: None,
        output: None,
        unit_id: None,
        content_hash: None,
        size: None,
        diagnostics: Vec::new(),
    };

    let source = match read_text(file_path, MAX_SOURCE_BYTES) {
        Ok(source) => source,
        Err(reason) => {
            report.diagnostics.push(Diagnostic::new(
                "SIG0026",
                file,
                Position::start_of_file(),
                "",
                "",
                format!("Cannot read the source file: {reason}."),
                "Check that the file exists and is UTF-8 text of at most 1 MiB.",
            ));
            return Ok(report);
        }
    };

    let entry_label = match canonical_content_path(&root_path, file_path) {
        Ok(path) => {
            report.unit_path = Some(path.clone());
            path
        }
        Err(error) => {
            report.diagnostics.push(Diagnostic::new(
                "SIG0025",
                file,
                Position::start_of_file(),
                "",
                "",
                format!("{error}."),
                "Place the file under the content root and name it with ASCII [a-z0-9_.-] and '/' only, ending in `.sigil`; sigilc never renames a path.",
            ));
            file_path.file_name().map_or_else(
                || file.to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        }
    };

    let loader = RootLoader { root: &root_path };
    let output = compile(&entry_label, &source, &loader, behaviors);
    for mut diagnostic in output.diagnostics {
        if diagnostic.file == entry_label {
            diagnostic.file = file.to_string();
        } else if !root_display.is_empty() {
            diagnostic.file = format!("{root_display}/{}", diagnostic.file);
        }
        report.diagnostics.push(diagnostic);
    }
    if !report.diagnostics.is_empty() {
        return Ok(report);
    }
    let (Some(bytes), Some(unit_path)) = (output.bytes, report.unit_path.clone()) else {
        return Ok(report);
    };
    let unit = grimoire_sigil::SigilUnit::from_bytes(&bytes).map_err(|error| {
        CliError::failed(format!(
            "internal error: the unit compiled from `{file}` does not decode: {error}"
        ))
    })?;
    report.unit_id = Some(format!("{:016x}", unit.id().0));
    report.content_hash = Some(format!("{:016x}", unit.content_hash()));
    report.size = Some(bytes.len() as u64);

    if let Some(out) = out {
        let relative = unit_output_path(&unit_path);
        let target = relative
            .split('/')
            .fold(PathBuf::from(out), |target, segment| target.join(segment));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CliError::failed(format!(
                    "cannot create output directory `{}`: {error}",
                    parent.display()
                ))
            })?;
        }
        std::fs::write(&target, &bytes).map_err(|error| {
            CliError::failed(format!("cannot write `{}`: {error}", target.display()))
        })?;
        report.output = Some(format!("{}/{relative}", trim_separators(out)));
    }
    Ok(report)
}

/// `sigil/imp_volley.sigil` -> `sigil/imp_volley.unit`.
fn unit_output_path(unit_path: &str) -> String {
    let stem = unit_path.strip_suffix(SIGIL_EXTENSION).unwrap_or(unit_path);
    format!("{stem}.unit")
}

/// A directory argument without trailing separators, so joining with `/` gives one separator.
fn trim_separators(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() { path } else { trimmed }
}

// ---- parse --------------------------------------------------------------------------------------

/// The `parse --json --values` document (`docs/formats/sigil.md` §13.5).
#[derive(Serialize)]
struct ValuesDocument<'a> {
    schema: &'static str,
    schema_version: u32,
    file: &'a str,
    diagnostics: &'a [Diagnostic],
    values: Vec<ValueJson>,
}

#[derive(Serialize)]
struct ValueJson {
    node_path: String,
    kind: &'static str,
    text: String,
    number: Option<String>,
    unit: Option<String>,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
}

fn run_parse(args: &[String], stdout: &mut dyn Write) -> CliResult {
    let args = Args::parse("parse", args, &["--json", "--values"], &[])?;
    let [path] = args.positionals.as_slice() else {
        return Err(CliError::usage("sigilc parse: expected exactly one file"));
    };
    if !args.flag("--json") {
        return Err(CliError::usage(
            "sigilc parse: only `--json` output exists; use `sigilc check` for text diagnostics",
        ));
    }
    let source = read_text(Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|reason| CliError::failed(format!("cannot read `{path}`: {reason}")))?;
    let output = parse(path.clone(), &source);
    let code = if output.diagnostics.is_empty() {
        EXIT_OK
    } else {
        EXIT_PROBLEMS
    };
    if args.flag("--values") {
        let values = list_values(&output.tree)
            .into_iter()
            .map(|entry| ValueJson {
                kind: entry.kind.as_str(),
                line: entry.start.line,
                column: entry.start.column,
                end_line: entry.end.line,
                end_column: entry.end.column,
                node_path: entry.node_path,
                text: entry.text,
                number: entry.number,
                unit: entry.unit,
            })
            .collect();
        let document = ValuesDocument {
            schema: VALUES_SCHEMA,
            schema_version: SCHEMA_VERSION,
            file: path,
            diagnostics: &output.diagnostics,
            values,
        };
        write_json(stdout, &document)?;
    } else {
        let document = DiagnosticsDocument::new(path.clone(), output.diagnostics);
        write_json(stdout, &document)?;
    }
    Ok(code)
}

// ---- set ----------------------------------------------------------------------------------------

/// The `set --json` document (`docs/formats/sigil.md` §13.6).
#[derive(Serialize)]
struct SetDocument<'a> {
    schema: &'static str,
    schema_version: u32,
    file: &'a str,
    node_path: &'a str,
    value: &'a str,
    ok: bool,
    changed: bool,
    old_value: Option<&'a str>,
    error: Option<SetErrorJson>,
    diagnostics: &'a [Diagnostic],
}

#[derive(Serialize)]
struct SetErrorJson {
    code: &'static str,
    message: String,
}

fn run_set(args: &[String], stdout: &mut dyn Write) -> CliResult {
    let args = Args::parse("set", args, &["--json"], &[])?;
    let [file, assignment] = args.positionals.as_slice() else {
        return Err(CliError::usage(
            "sigilc set: expected `<file> <node-path>=<value>`",
        ));
    };
    let Some((node_path, value)) = assignment.split_once('=') else {
        return Err(CliError::usage(format!(
            "sigilc set: `{assignment}` is not `<node-path>=<value>`"
        )));
    };
    let source = read_text(Path::new(file), MAX_SOURCE_BYTES)
        .map_err(|reason| CliError::failed(format!("cannot read `{file}`: {reason}")))?;

    let result = set_value(file, &source, node_path, value);
    if let Ok(outcome) = &result
        && outcome.changed
    {
        std::fs::write(file, &outcome.source)
            .map_err(|error| CliError::failed(format!("cannot write `{file}`: {error}")))?;
    }

    let json = args.flag("--json");
    match &result {
        Ok(outcome) => {
            if json {
                write_json(
                    stdout,
                    &SetDocument {
                        schema: SET_SCHEMA,
                        schema_version: SCHEMA_VERSION,
                        file,
                        node_path,
                        value,
                        ok: true,
                        changed: outcome.changed,
                        old_value: Some(&outcome.old_value),
                        error: None,
                        diagnostics: &[],
                    },
                )?;
            } else if outcome.changed {
                writeln!(
                    stdout,
                    "{file}: set {node_path} = {value} (was {})",
                    outcome.old_value
                )?;
            } else {
                writeln!(stdout, "{file}: {node_path} is already {value}")?;
            }
            Ok(EXIT_OK)
        }
        Err(error) => {
            if json {
                write_json(
                    stdout,
                    &SetDocument {
                        schema: SET_SCHEMA,
                        schema_version: SCHEMA_VERSION,
                        file,
                        node_path,
                        value,
                        ok: false,
                        changed: false,
                        old_value: None,
                        error: Some(SetErrorJson {
                            code: error.code(),
                            message: error.to_string(),
                        }),
                        diagnostics: error.diagnostics(),
                    },
                )?;
            } else {
                write_set_error(stdout, file, error)?;
            }
            Ok(EXIT_PROBLEMS)
        }
    }
}

fn write_set_error(stdout: &mut dyn Write, file: &str, error: &SetError) -> std::io::Result<()> {
    writeln!(stdout, "{file}: set refused ({}): {error}", error.code())?;
    for diagnostic in error.diagnostics() {
        writeln!(stdout, "{}", diagnostic.render_text())?;
    }
    Ok(())
}

// ---- fmt ----------------------------------------------------------------------------------------

fn run_fmt(args: &[String], stdout: &mut dyn Write) -> CliResult {
    let args = Args::parse("fmt", args, &["--check"], &[])?;
    if args.positionals.is_empty() {
        return Err(CliError::usage("sigilc fmt: expected at least one file"));
    }
    let check = args.flag("--check");
    let mut problems = false;
    let mut failure: Option<CliError> = None;
    for file in &args.positionals {
        let source = match read_text(Path::new(file), MAX_SOURCE_BYTES) {
            Ok(source) => source,
            Err(reason) => {
                failure = Some(CliError::failed(format!("cannot read `{file}`: {reason}")));
                continue;
            }
        };
        match format_canonical(file, &source) {
            Ok(formatted) if formatted == source => {}
            Ok(formatted) => {
                if check {
                    writeln!(stdout, "{file}: not in canonical layout")?;
                    problems = true;
                } else if let Err(error) = std::fs::write(file, formatted) {
                    failure = Some(CliError::failed(format!("cannot write `{file}`: {error}")));
                } else {
                    writeln!(stdout, "formatted {file}")?;
                }
            }
            Err(FormatError::SourceHasErrors { diagnostics }) => {
                for diagnostic in &diagnostics {
                    writeln!(stdout, "{}", diagnostic.render_text())?;
                }
                problems = true;
            }
            Err(error) => {
                failure = Some(CliError::failed(format!("`{file}`: {error}")));
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None if problems => Ok(EXIT_PROBLEMS),
        None => Ok(EXIT_OK),
    }
}

fn write_json(stdout: &mut dyn Write, document: &impl Serialize) -> Result<(), CliError> {
    let json = serde_json::to_string_pretty(document)
        .map_err(|error| CliError::failed(format!("could not encode JSON: {error}")))?;
    writeln!(stdout, "{json}")?;
    Ok(())
}
