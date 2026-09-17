//! The `grimoire-link` command-line interface (Plan 0002 WP8.5).
//!
//! `src/bin/grimoire-link.rs` only forwards `argv` and the standard streams to [`run`]; everything
//! else lives here so the commands can be tested in process. No window, no GPU, no game.
//!
//! ```text
//! grimoire-link [--addr 127.0.0.1:47474] [--token <64 hex digits>] <command>
//!
//!   stats [--count <n>] [--interval <frames>]
//!       Connects and prints the engine's `Stats` messages (default: 5 messages every 30 frames).
//!
//!   swap --root <dir> [--behaviors <file>] <file.sigil>
//!       Compiles the file once and pushes it; prints compile and round-trip time.
//!
//!   watch --root <dir> [--behaviors <file>] [--poll-ms <ms>] [--swaps <n>] [--stats <frames>]
//!         <file.sigil>
//!       Compiles and pushes the file on every save until Ctrl+C, `--swaps` swaps or the engine
//!       closes the connection.
//! ```
//!
//! Address and token default to `GRIMOIRE_DEBUG_ADDR` and `GRIMOIRE_DEBUG_TOKEN`, the same
//! variables the engine reads (contract §13), so a tool started in the same shell as the engine
//! needs neither flag.
//!
//! Exit codes: [`EXIT_OK`] when everything asked for succeeded, [`EXIT_PROBLEMS`] when the tool
//! ran but the work did not succeed (a source with diagnostics, a swap the engine rejected, an
//! engine that closed the connection), [`EXIT_ERROR`] when it could not run as asked (usage
//! error, no engine to connect to, unreadable behaviour manifest). Malformed input of any kind
//! ends in one of these codes, never in a panic (contract §2 rule 9).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use std::time::Duration;

use grimoire_debug::{DEBUG_ADDR_ENV, DEBUG_TOKEN_ENV, Message};
use grimoire_sigilc::behaviors::{MAX_BEHAVIOR_MANIFEST_BYTES, parse_behavior_manifest};

use crate::client::{LinkClient, LinkClientError};
use crate::tcp::TcpClientTransport;
use crate::watch::{DEFAULT_POLL_INTERVAL, WatchError, WatchOptions, describe, swap_once, watch};

/// Everything asked for succeeded.
pub const EXIT_OK: u8 = 0;
/// The tool ran, but the work did not succeed: diagnostics, a rejected swap, a closed connection.
pub const EXIT_PROBLEMS: u8 = 1;
/// The tool could not run as asked: usage error, no engine, unreadable manifest.
pub const EXIT_ERROR: u8 = 2;

/// `Stats` messages `stats` prints by default.
const DEFAULT_STATS_COUNT: u64 = 5;
/// Frames between two `Stats` the tool asks for by default (contract §13 `Hello`).
const DEFAULT_STATS_INTERVAL: u16 = 30;

const USAGE: &str = "\
usage: grimoire-link [--addr <127.0.0.1:port>] [--token <64 hex digits>] <command>

  stats [--count <n>] [--interval <frames>]
  swap  --root <dir> [--behaviors <file>] <file.sigil>
  watch --root <dir> [--behaviors <file>] [--poll-ms <ms>] [--swaps <n>] [--stats <frames>] <file.sigil>

The address and token default to GRIMOIRE_DEBUG_ADDR and GRIMOIRE_DEBUG_TOKEN.";

/// Runs `grimoire-link` with `args` (without the program name) and returns the process exit code.
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
            let _ = writeln!(stderr, "grimoire-link: could not write output: {error}");
            EXIT_ERROR
        }
        (Err(error), _) => {
            let _ = writeln!(stderr, "grimoire-link: {}", error.message);
            if error.usage {
                let _ = writeln!(stderr, "{USAGE}");
            }
            EXIT_ERROR
        }
    }
}

/// A failure that ends the command with [`EXIT_ERROR`].
#[derive(Debug)]
struct CliError {
    message: String,
    usage: bool,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            usage: true,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            usage: false,
        }
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

/// Parsed options and positional arguments of one command.
struct Args {
    options: BTreeMap<String, String>,
    positionals: Vec<String>,
}

impl Args {
    /// Splits `args` into the known `options` (each `--name <value>`) and positional arguments.
    fn parse(command: &str, args: &[String], options: &[&str]) -> Result<Self, CliError> {
        let mut parsed = Args {
            options: BTreeMap::new(),
            positionals: Vec::new(),
        };
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if let Some(name) = options.iter().find(|name| *name == arg) {
                let value = rest.next().ok_or_else(|| {
                    CliError::usage(format!("grimoire-link {command}: {name} needs a value"))
                })?;
                if parsed
                    .options
                    .insert((*name).to_owned(), value.clone())
                    .is_some()
                {
                    return Err(CliError::usage(format!(
                        "grimoire-link {command}: {name} was given twice"
                    )));
                }
            } else if arg.starts_with("--") {
                return Err(CliError::usage(format!(
                    "grimoire-link {command}: unknown option {arg}"
                )));
            } else {
                parsed.positionals.push(arg.clone());
            }
        }
        Ok(parsed)
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }

    fn required(&self, command: &str, name: &str) -> Result<&str, CliError> {
        self.option(name)
            .ok_or_else(|| CliError::usage(format!("grimoire-link {command}: {name} is required")))
    }

    fn number<T: std::str::FromStr>(
        &self,
        command: &str,
        name: &str,
    ) -> Result<Option<T>, CliError> {
        match self.option(name) {
            None => Ok(None),
            Some(value) => value.parse().map(Some).map_err(|_| {
                CliError::usage(format!(
                    "grimoire-link {command}: {name} expects a number, got {value:?}"
                ))
            }),
        }
    }

    fn one_file(&self, command: &str) -> Result<PathBuf, CliError> {
        match self.positionals.as_slice() {
            [file] => Ok(PathBuf::from(file)),
            [] => Err(CliError::usage(format!(
                "grimoire-link {command}: expected one .sigil file"
            ))),
            files => Err(CliError::usage(format!(
                "grimoire-link {command}: expected one .sigil file, got {}",
                files.len()
            ))),
        }
    }
}

fn dispatch(args: &[String], stdout: &mut dyn Write) -> CliResult {
    // The connection options come before the command, so the command's own parser never sees them.
    let mut connection = Connection::from_env()?;
    let mut rest = args.iter().enumerate();
    let mut command = None;
    while let Some((index, arg)) = rest.next() {
        match arg.as_str() {
            "--addr" | "--token" => {
                let (_, value) = rest
                    .next()
                    .ok_or_else(|| CliError::usage(format!("{arg} needs a value")))?;
                connection.set(arg, value)?;
            }
            "--help" | "-h" => {
                writeln!(stdout, "{USAGE}").map_err(write_error)?;
                return Ok(EXIT_OK);
            }
            "--version" => {
                writeln!(stdout, "grimoire-link {}", env!("CARGO_PKG_VERSION"))
                    .map_err(write_error)?;
                return Ok(EXIT_OK);
            }
            other => {
                command = Some((other.to_owned(), index + 1));
                break;
            }
        }
    }
    let Some((command, start)) = command else {
        return Err(CliError::usage("expected a command"));
    };
    let rest = &args[start..];
    match command.as_str() {
        "stats" => run_stats(&connection, rest, stdout),
        "swap" => run_swap(&connection, rest, stdout),
        "watch" => run_watch(&connection, rest, stdout),
        other => Err(CliError::usage(format!("unknown command {other:?}"))),
    }
}

/// Where to connect and with which token.
struct Connection {
    addr: Option<SocketAddrV4>,
    token: Option<[u8; 32]>,
}

impl Connection {
    /// Defaults from the engine's own environment variables (contract §13).
    fn from_env() -> Result<Self, CliError> {
        let mut connection = Self {
            addr: None,
            token: None,
        };
        if let Ok(value) = std::env::var(DEBUG_ADDR_ENV) {
            connection.set("--addr", &value)?;
        }
        if let Ok(value) = std::env::var(DEBUG_TOKEN_ENV) {
            connection.set("--token", &value)?;
        }
        Ok(connection)
    }

    fn set(&mut self, name: &str, value: &str) -> Result<(), CliError> {
        match name {
            "--addr" => {
                self.addr = Some(value.parse().map_err(|_| {
                    CliError::usage(format!(
                        "--addr expects an address like 127.0.0.1:47474, got {value:?}"
                    ))
                })?);
            }
            _ => {
                self.token = Some(parse_token(value)?);
            }
        }
        Ok(())
    }

    fn addr(&self) -> Result<SocketAddrV4, CliError> {
        self.addr.ok_or_else(|| {
            CliError::usage(format!(
                "no engine address: pass --addr or set {DEBUG_ADDR_ENV}"
            ))
        })
    }

    /// Connects and completes the handshake, reporting the engine it reached.
    fn open(
        &self,
        stats_interval_frames: u16,
        stdout: &mut dyn Write,
    ) -> Result<LinkClient, CliError> {
        let addr = self.addr()?;
        let token = self.token.unwrap_or([0; 32]);
        let transport = TcpClientTransport::connect(addr)
            .map_err(|error| CliError::failed(format!("could not connect to {addr}: {error}")))?;
        let (client, engine) = LinkClient::connect(Box::new(transport), token, stats_interval_frames)
            .map_err(|error| match error {
                LinkClientError::Handshake(error) => CliError::failed(format!(
                    "the engine at {addr} refused the connection: {error} (same engine version and token?)"
                )),
                other => CliError::failed(format!("handshake with {addr} failed: {other}")),
            })?;
        writeln!(
            stdout,
            "connected to {addr}: engine {} build {}",
            engine.engine_version, engine.build_hash
        )
        .map_err(write_error)?;
        Ok(client)
    }
}

fn parse_token(value: &str) -> Result<[u8; 32], CliError> {
    if value.len() != 64 || !value.is_ascii() {
        return Err(CliError::usage(
            "--token expects exactly 64 hex digits".to_owned(),
        ));
    }
    let mut token = [0u8; 32];
    for (index, slot) in token.iter_mut().enumerate() {
        let pair = &value[index * 2..index * 2 + 2];
        *slot = u8::from_str_radix(pair, 16)
            .map_err(|_| CliError::usage("--token expects exactly 64 hex digits".to_owned()))?;
    }
    Ok(token)
}

fn run_stats(connection: &Connection, args: &[String], stdout: &mut dyn Write) -> CliResult {
    let args = Args::parse("stats", args, &["--count", "--interval"])?;
    if !args.positionals.is_empty() {
        return Err(CliError::usage(
            "grimoire-link stats: takes no positional argument",
        ));
    }
    let count: u64 = args
        .number("stats", "--count")?
        .unwrap_or(DEFAULT_STATS_COUNT);
    let interval: u16 = args
        .number("stats", "--interval")?
        .unwrap_or(DEFAULT_STATS_INTERVAL);
    if interval == 0 {
        return Err(CliError::usage(
            "grimoire-link stats: --interval 0 would ask for no stats at all",
        ));
    }
    let mut client = connection.open(interval, stdout)?;

    let mut seen = 0;
    while seen < count {
        match client.next_message(Duration::from_secs(10)) {
            Ok(message) => {
                writeln!(stdout, "{}", describe(&message)).map_err(write_error)?;
                if matches!(message, Message::Stats(_)) {
                    seen += 1;
                }
            }
            Err(LinkClientError::Timeout(timeout)) => {
                writeln!(
                    stdout,
                    "no stats within {timeout:?}: is the engine running frames?"
                )
                .map_err(write_error)?;
                return Ok(EXIT_PROBLEMS);
            }
            Err(error) => {
                writeln!(stdout, "the connection ended: {error}").map_err(write_error)?;
                return Ok(EXIT_PROBLEMS);
            }
        }
    }
    Ok(EXIT_OK)
}

fn run_swap(connection: &Connection, args: &[String], stdout: &mut dyn Write) -> CliResult {
    let args = Args::parse("swap", args, &["--root", "--behaviors"])?;
    let options = watch_options("swap", &args)?;
    let mut client = connection.open(0, stdout)?;
    match swap_once(&mut client, &options, options.swap_timeout) {
        Ok(outcome) => {
            writeln!(stdout, "{}", outcome.summary()).map_err(write_error)?;
            Ok(if outcome.applied() {
                EXIT_OK
            } else {
                EXIT_PROBLEMS
            })
        }
        Err(error) => {
            report_swap_error(&error, stdout)?;
            Ok(EXIT_PROBLEMS)
        }
    }
}

fn run_watch(connection: &Connection, args: &[String], stdout: &mut dyn Write) -> CliResult {
    let args = Args::parse(
        "watch",
        args,
        &["--root", "--behaviors", "--poll-ms", "--swaps", "--stats"],
    )?;
    let mut options = watch_options("watch", &args)?;
    if let Some(poll) = args.number::<u64>("watch", "--poll-ms")? {
        if poll == 0 {
            return Err(CliError::usage(
                "grimoire-link watch: --poll-ms 0 would spin without waiting",
            ));
        }
        options.poll_interval = Duration::from_millis(poll);
    }
    let swaps: Option<u64> = args.number("watch", "--swaps")?;
    let stats: u16 = args.number("watch", "--stats")?.unwrap_or(0);
    let mut client = connection.open(stats, stdout)?;

    // Without a signal handler (no new dependency, contract §2 rule 4) Ctrl+C ends the process;
    // the loop itself stops on `--swaps`, on a closed connection or on a write failure.
    let never = || false;
    match watch(&mut client, &options, swaps, stdout, &never) {
        Ok(outcomes) => {
            let applied = outcomes.iter().filter(|outcome| outcome.applied()).count() as u64;
            writeln!(stdout, "{applied} swap(s) applied").map_err(write_error)?;
            Ok(match swaps {
                Some(wanted) if applied < wanted => EXIT_PROBLEMS,
                _ => EXIT_OK,
            })
        }
        Err(error) => {
            report_swap_error(&error, stdout)?;
            Ok(EXIT_PROBLEMS)
        }
    }
}

/// The `--root`, `--behaviors` and file arguments shared by `swap` and `watch`.
fn watch_options(command: &str, args: &Args) -> Result<WatchOptions, CliError> {
    let root = PathBuf::from(args.required(command, "--root")?);
    let file = args.one_file(command)?;
    let mut options = WatchOptions::new(&root, &file);
    options.poll_interval = DEFAULT_POLL_INTERVAL;
    if let Some(path) = args.option("--behaviors") {
        options.behavior_ids = load_behaviors(Path::new(path))?;
    }
    Ok(options)
}

/// Reads the behaviour manifest `sigilc` also takes (`{"<name>": <id>}`, contract §11.5).
fn load_behaviors(path: &Path) -> Result<BTreeMap<String, u32>, CliError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        CliError::failed(format!(
            "could not read the behaviour manifest {}: {error}",
            path.display()
        ))
    })?;
    if metadata.len() > MAX_BEHAVIOR_MANIFEST_BYTES as u64 {
        return Err(CliError::failed(format!(
            "the behaviour manifest {} is larger than {MAX_BEHAVIOR_MANIFEST_BYTES} bytes",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path).map_err(|error| {
        CliError::failed(format!(
            "could not read the behaviour manifest {}: {error}",
            path.display()
        ))
    })?;
    parse_behavior_manifest(&text).map_err(|error| {
        CliError::failed(format!(
            "the behaviour manifest {} is invalid: {error}",
            path.display()
        ))
    })
}

fn report_swap_error(error: &WatchError, stdout: &mut dyn Write) -> Result<(), CliError> {
    writeln!(stdout, "not swapped: {error}").map_err(write_error)?;
    if let WatchError::Compile(crate::compile::CompileError::Diagnostics { diagnostics, .. }) =
        error
    {
        for diagnostic in diagnostics {
            writeln!(
                stdout,
                "  {}:{}:{} {} {}",
                diagnostic.file,
                diagnostic.line,
                diagnostic.column,
                diagnostic.code,
                diagnostic.message
            )
            .map_err(write_error)?;
        }
    }
    Ok(())
}

fn write_error(error: std::io::Error) -> CliError {
    CliError::failed(format!("could not write output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the CLI with `args` and returns `(exit code, stdout, stderr)`.
    fn run_cli(args: &[&str]) -> (u8, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run(args.iter().map(OsString::from), &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8(stdout).expect("utf-8"),
            String::from_utf8(stderr).expect("utf-8"),
        )
    }

    #[test]
    fn help_and_version_succeed() {
        let (code, out, _) = run_cli(&["--help"]);
        assert_eq!(code, EXIT_OK);
        assert!(out.contains("usage: grimoire-link"), "{out}");
        let (code, out, _) = run_cli(&["--version"]);
        assert_eq!(code, EXIT_OK);
        assert!(out.starts_with("grimoire-link "), "{out}");
    }

    #[test]
    fn usage_errors_name_what_is_wrong_and_print_the_usage() {
        for (args, expected) in [
            (vec![], "expected a command"),
            (vec!["dance"], "unknown command"),
            (vec!["--addr"], "--addr needs a value"),
            (
                vec!["--addr", "localhost:1", "stats"],
                "--addr expects an address",
            ),
            (
                vec!["--token", "abc", "stats"],
                "--token expects exactly 64 hex digits",
            ),
            (vec!["stats", "--count"], "--count needs a value"),
            (vec!["stats", "--count", "many"], "--count expects a number"),
            (vec!["stats", "--interval", "0"], "would ask for no stats"),
            (
                vec!["stats", "pattern.sigil"],
                "takes no positional argument",
            ),
            (vec!["swap", "a.sigil"], "--root is required"),
            (vec!["swap", "--root", "."], "expected one .sigil file"),
            (
                vec!["swap", "--root", ".", "a.sigil", "b.sigil"],
                "expected one .sigil file, got 2",
            ),
            (
                vec!["watch", "--root", ".", "--poll-ms", "0", "a.sigil"],
                "would spin without waiting",
            ),
            (
                vec!["swap", "--root", ".", "--jazz", "a.sigil"],
                "unknown option --jazz",
            ),
            (
                vec!["swap", "--root", ".", "--root", "..", "a.sigil"],
                "--root was given twice",
            ),
        ] {
            let (code, _, err) = run_cli(&args);
            assert_eq!(code, EXIT_ERROR, "{args:?} -> {err}");
            assert!(err.contains(expected), "{args:?} -> {err}");
            assert!(err.contains("usage: grimoire-link"), "{args:?} -> {err}");
        }
    }

    #[test]
    fn a_missing_address_is_a_usage_error() {
        // No `--addr`, and the ambient environment usually has none either; with one set, the
        // connection attempt fails instead, which is the other error path.
        let (code, _, err) = run_cli(&["stats", "--count", "1"]);
        assert_eq!(code, EXIT_ERROR);
        if std::env::var(DEBUG_ADDR_ENV).is_err() {
            assert!(err.contains("no engine address"), "{err}");
        }
    }

    #[test]
    fn an_unreadable_behaviour_manifest_is_reported_without_connecting() {
        let (code, _, err) = run_cli(&[
            "--addr",
            "127.0.0.1:1",
            "swap",
            "--root",
            ".",
            "--behaviors",
            "no/such/manifest.json",
            "a.sigil",
        ]);
        assert_eq!(code, EXIT_ERROR);
        assert!(err.contains("behaviour manifest"), "{err}");
        assert!(!err.contains("usage: grimoire-link"), "{err}");
    }

    #[test]
    fn a_token_of_64_hex_digits_parses_into_32_bytes() {
        let token = parse_token(&"0f".repeat(32)).expect("64 hex digits");
        assert_eq!(token, [0x0f; 32]);
        assert!(parse_token(&"zz".repeat(32)).is_err());
        assert!(parse_token("0f").is_err());
    }
}
