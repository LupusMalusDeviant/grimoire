//! The `sigilc` command-line interface (Plan 0002 WP4.3, `docs/formats/sigil.md` §13), run
//! in-process through `grimoire_sigilc::cli::run` against real files in a scratch directory, plus a
//! few runs of the real binary to prove the exit codes reach the process boundary.
//!
//! Integration tests run with the package root as the working directory, so corpus paths are
//! given relative to it (`tests/corpus/...`), exactly as a user would type them.

mod support;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use grimoire_sigilc::cli::{EXIT_ERROR, EXIT_OK, EXIT_PROBLEMS, run};
use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};
use grimoire_sigilc::derive_unit_id;
use serde_json::Value;

struct Output {
    code: u8,
    stdout: String,
    stderr: String,
}

impl Output {
    /// The top-level keys of the pretty-printed JSON document, in written order (`serde_json::Value`
    /// sorts keys, so the order is read from the text: top-level keys are indented by two spaces).
    fn top_level_keys(&self) -> Vec<String> {
        self.stdout
            .lines()
            .filter_map(|line| line.strip_prefix("  \""))
            .filter(|rest| !rest.starts_with(' '))
            .filter_map(|rest| rest.split_once("\":").map(|(key, _)| key.to_string()))
            .collect()
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("stdout is not JSON ({error}):\n{}", self.stdout))
    }
}

fn sigilc<S: AsRef<str>>(args: &[S]) -> Output {
    sigilc_os(
        args.iter()
            .map(|arg| OsString::from(arg.as_ref()))
            .collect(),
    )
}

fn sigilc_os(args: Vec<OsString>) -> Output {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run(args, &mut stdout, &mut stderr);
    Output {
        code,
        stdout: String::from_utf8(stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(stderr).expect("stderr is UTF-8"),
    }
}

/// A scratch directory, removed again when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("grimoire-sigilc-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, relative: &str, contents: &str) -> String {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn path(&self, relative: &str) -> String {
        self.0.join(relative).to_str().unwrap().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn corpus(relative: &str) -> String {
    format!("tests/corpus/{relative}")
}

fn read_corpus(relative: &str) -> String {
    fs::read_to_string(corpus(relative)).unwrap()
}

const BEHAVIORS: &str = r#"{
  "schema": "grimoire.sigilc.behaviors",
  "schema_version": 1,
  "behaviors": [{ "name": "orbit_parent", "id": 1 }]
}"#;

// ---- general --------------------------------------------------------------------------------

#[test]
fn help_version_and_usage_errors() {
    let help = sigilc(&["--help"]);
    assert_eq!(help.code, EXIT_OK);
    assert!(
        help.stdout
            .contains("sigilc set [--json] <file> <node-path>=<value>")
    );

    let version = sigilc(&["--version"]);
    assert_eq!(version.code, EXIT_OK);
    assert_eq!(
        version.stdout,
        format!("sigilc {}\n", env!("CARGO_PKG_VERSION"))
    );

    for args in [
        vec![],
        vec!["simulate"],
        vec!["check"],
        vec!["check", "--bogus", "a.sigil"],
        vec!["check", "--root"],
        vec!["check", "--root=", "a.sigil"],
        vec!["check", "--json", "--json", "a.sigil"],
        vec!["check", "--json=yes", "a.sigil"],
        vec!["build", "a.sigil"],
        vec!["build", "--root", "x", "a.sigil"],
        vec!["parse", "a.sigil"],
        vec!["parse", "--json"],
        vec!["parse", "--json", "a.sigil", "b.sigil"],
        vec!["set", "a.sigil"],
        vec!["set", "a.sigil", "no-equals-sign"],
        vec!["fmt"],
    ] {
        let output = sigilc(&args);
        assert_eq!(output.code, EXIT_ERROR, "{args:?}: {}", output.stderr);
        assert!(output.stderr.starts_with("sigilc"), "{args:?}");
    }
}

#[cfg(unix)]
fn non_unicode_argument() -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(vec![b'a', 0xff, b'.', b's'])
}

#[cfg(windows)]
fn non_unicode_argument() -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(&[u16::from(b'a'), 0xD800, u16::from(b'.')])
}

#[cfg(any(unix, windows))]
#[test]
fn a_non_unicode_argument_is_an_error_not_a_panic() {
    let output = sigilc_os(vec![OsString::from("check"), non_unicode_argument()]);
    assert_eq!(output.code, EXIT_ERROR);
    assert!(output.stderr.contains("not valid Unicode"));
}

#[test]
fn the_binary_returns_the_documented_exit_codes() {
    let binary = env!("CARGO_BIN_EXE_sigilc");
    let status = |args: &[&str]| {
        Command::new(binary)
            .args(args)
            .output()
            .unwrap()
            .status
            .code()
    };
    assert_eq!(status(&["--version"]), Some(i32::from(EXIT_OK)));
    assert_eq!(
        status(&["check", &corpus("valid/01-ring-burst.sigil")]),
        Some(i32::from(EXIT_OK))
    );
    assert_eq!(
        status(&["check", &corpus("invalid/e-unclosed-body.sigil")]),
        Some(i32::from(EXIT_PROBLEMS))
    );
    assert_eq!(status(&["no-such-command"]), Some(i32::from(EXIT_ERROR)));
}

// ---- check ------------------------------------------------------------------------------------

#[test]
fn check_compiles_the_valid_corpus_quietly() {
    let output = sigilc(&[
        "check",
        &corpus("valid/01-ring-burst.sigil"),
        &corpus("valid/02-aimed-stream.sigil"),
        &corpus("valid/03-subemitter-cascade.sigil"),
        &corpus("valid/04-mirrored-spiral.sigil"),
    ]);
    assert_eq!(output.code, EXIT_OK, "{}", output.stdout);
    assert_eq!(output.stdout, "");
}

#[test]
fn check_resolves_behaviours_only_through_a_manifest() {
    let scratch = Scratch::new("behaviours");
    let file = corpus("valid/05-wave-line-composite.sigil");

    let without = sigilc(&["check", &file]);
    assert_eq!(without.code, EXIT_PROBLEMS);
    assert!(
        without.stdout.contains("error[SIG0014]"),
        "{}",
        without.stdout
    );
    assert!(without.stdout.starts_with(&file), "{}", without.stdout);

    let manifest = scratch.write("behaviors.json", BEHAVIORS);
    let with = sigilc(&["check", "--behaviors", &manifest, &file]);
    assert_eq!(with.code, EXIT_OK, "{}", with.stdout);

    let broken = scratch.write("broken.json", "{\"schema\": 1}");
    let invalid = sigilc(&["check", "--behaviors", &broken, &file]);
    assert_eq!(invalid.code, EXIT_ERROR);
    assert!(invalid.stderr.contains("malformed"), "{}", invalid.stderr);

    let missing = sigilc(&["check", "--behaviors", &scratch.path("nope.json"), &file]);
    assert_eq!(missing.code, EXIT_ERROR);
}

#[test]
fn check_json_reports_every_file_with_the_compiler_diagnostics() {
    let valid = corpus("valid/01-ring-burst.sigil");
    let invalid = corpus("schema-invalid/e-glow-out-of-range.sigil");
    let output = sigilc(&["check", "--json", &valid, &invalid]);
    assert_eq!(output.code, EXIT_PROBLEMS);
    let json = output.json();
    assert_eq!(json["schema"], "grimoire.sigilc.build");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "check");
    assert_eq!(json["ok"], false);
    assert_eq!(
        output.top_level_keys(),
        ["schema", "schema_version", "command", "ok", "units"]
    );

    let units = json["units"].as_array().unwrap();
    assert_eq!(units.len(), 2);

    let ok = &units[0];
    assert_eq!(ok["source"], valid.as_str());
    assert_eq!(ok["unit_path"], "01-ring-burst.sigil");
    assert_eq!(ok["output"], Value::Null);
    let unit_id = derive_unit_id("01-ring-burst.sigil").unwrap().0;
    assert_eq!(ok["unit_id"], format!("{unit_id:016x}"));
    let hash = ok["content_hash"].as_str().unwrap();
    assert_eq!(hash.len(), 16);
    assert!(
        hash.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert!(ok["size"].as_u64().unwrap() > 40);
    assert_eq!(ok["diagnostics"], Value::Array(Vec::new()));

    // The diagnostics are the compiler's, with the entry file labelled as given.
    let failed = &units[1];
    assert_eq!(failed["unit_id"], Value::Null);
    let expected: Value = serde_json::from_str(&read_corpus(
        "schema-invalid/e-glow-out-of-range.expected.json",
    ))
    .unwrap();
    let mut expected_diagnostics = expected["diagnostics"].clone();
    for diagnostic in expected_diagnostics.as_array_mut().unwrap() {
        diagnostic["file"] = Value::from(invalid.as_str());
    }
    assert_eq!(failed["diagnostics"], expected_diagnostics);
}

// ---- build ------------------------------------------------------------------------------------

struct NoImports;

impl SourceLoader for NoImports {
    fn load(&self, _path: &str) -> Result<String, LoadError> {
        Err(LoadError::NotFound)
    }
}

#[test]
fn build_writes_the_same_bytes_as_the_library_under_the_canonical_path() {
    let scratch = Scratch::new("build");
    let source = read_corpus("valid/01-ring-burst.sigil");
    let file = scratch.write("content/sigil/ring_burst.sigil", &source);
    let root = scratch.path("content");
    let out = scratch.path("out");

    let output = sigilc(&["build", "--json", "--root", &root, "--out", &out, &file]);
    assert_eq!(output.code, EXIT_OK, "{}", output.stdout);
    let json = output.json();
    assert_eq!(json["command"], "build");
    assert_eq!(json["ok"], true);
    let unit = &json["units"][0];
    assert_eq!(unit["unit_path"], "sigil/ring_burst.sigil");
    assert_eq!(unit["output"], format!("{out}/sigil/ring_burst.unit"));

    let written = fs::read(Path::new(&out).join("sigil").join("ring_burst.unit")).unwrap();
    let expected = compile(
        "sigil/ring_burst.sigil",
        &source,
        &NoImports,
        &BTreeMap::new(),
    )
    .bytes
    .unwrap();
    assert_eq!(written, expected);
    let decoded = grimoire_sigil::SigilUnit::from_bytes(&written).unwrap();
    assert_eq!(unit["unit_id"], format!("{:016x}", decoded.id().0));
    assert_eq!(
        decoded.id(),
        derive_unit_id("sigil/ring_burst.sigil").unwrap()
    );
    assert_eq!(
        unit["content_hash"],
        format!("{:016x}", decoded.content_hash())
    );
    assert_eq!(unit["size"], written.len());

    // Text mode says what it built; a second build rewrites identical bytes.
    let text = sigilc(&["build", "--root", &format!("{root}/"), "--out", &out, &file]);
    assert_eq!(text.code, EXIT_OK);
    assert!(
        text.stdout.starts_with(&format!(
            "built {file} -> {out}/sigil/ring_burst.unit (unit {:016x}, content hash {:016x}, {} bytes)",
            decoded.id().0,
            decoded.content_hash(),
            written.len()
        )),
        "{}",
        text.stdout
    );
    assert_eq!(
        fs::read(Path::new(&out).join("sigil").join("ring_burst.unit")).unwrap(),
        written
    );
}

#[test]
fn build_resolves_imports_against_the_root_and_labels_their_diagnostics() {
    let scratch = Scratch::new("imports");
    let root = scratch.path("content");
    scratch.write(
        "content/lib/ring.sigil",
        &read_corpus("valid/01-ring-burst.sigil"),
    );
    let entry = scratch.write(
        "content/boss/finale.sigil",
        "sigil 1\nimport \"lib/ring.sigil\" as ring\n\nemitter finale from ring.burst {\n  repeat = 1\n}\n",
    );
    let out = scratch.path("out");
    let output = sigilc(&["build", "--root", &root, "--out", &out, &entry]);
    assert_eq!(output.code, EXIT_OK, "{}", output.stdout);
    assert!(Path::new(&out).join("boss").join("finale.unit").is_file());

    // A parse error in the imported file is labelled with the root-joined path.
    scratch.write("content/lib/ring.sigil", "sigil 1\nbullet orb {\n");
    let broken = sigilc(&["check", "--json", "--root", &root, &entry]);
    assert_eq!(broken.code, EXIT_PROBLEMS);
    let diagnostics = broken.json()["units"][0]["diagnostics"].clone();
    let files: Vec<&str> = diagnostics
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect();
    assert!(
        files.contains(&format!("{root}/lib/ring.sigil").as_str()),
        "{files:?}"
    );

    // An import cannot reach outside the root.
    let escaping = scratch.write(
        "content/boss/escape.sigil",
        "sigil 1\nimport \"../outside.sigil\" as out\n\nemitter e from out.burst {\n  repeat = 1\n}\n",
    );
    scratch.write("outside.sigil", &read_corpus("valid/01-ring-burst.sigil"));
    let escaped = sigilc(&["check", "--json", "--root", &root, &escaping]);
    assert_eq!(escaped.code, EXIT_PROBLEMS);
    let codes: Vec<String> = escaped.json()["units"][0]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect();
    assert!(codes.contains(&"SIG0012".to_string()), "{codes:?}");
}

#[test]
fn build_refuses_paths_it_would_have_to_normalise_and_unreadable_files() {
    let scratch = Scratch::new("paths");
    let source = read_corpus("valid/01-ring-burst.sigil");
    let root = scratch.path("content");
    let out = scratch.path("out");
    let upper = scratch.write("content/Ring.sigil", &source);
    let outside = scratch.write("elsewhere/ring.sigil", &source);
    let wrong_extension = scratch.write("content/ring.txt", &source);
    let missing = scratch.path("content/missing.sigil");
    let not_utf8 = scratch.path("content/binary.sigil");
    fs::write(&not_utf8, [0xff, 0xfe, 0x00]).unwrap();

    let output = sigilc(&[
        "build",
        "--json",
        "--root",
        &root,
        "--out",
        &out,
        &upper,
        &outside,
        &wrong_extension,
        &missing,
        &not_utf8,
    ]);
    assert_eq!(output.code, EXIT_PROBLEMS, "{}", output.stdout);
    let json = output.json();
    let codes: Vec<Vec<String>> = json["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| {
            assert_eq!(unit["output"], Value::Null);
            unit["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d["code"].as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    assert_eq!(
        codes,
        vec![
            vec!["SIG0025".to_string()],
            vec!["SIG0025".to_string()],
            vec!["SIG0025".to_string()],
            vec!["SIG0026".to_string()],
            vec!["SIG0026".to_string()],
        ]
    );
    assert!(!Path::new(&out).exists(), "nothing may be written");
}

// ---- parse ------------------------------------------------------------------------------------

#[test]
fn parse_json_still_prints_exactly_the_diagnostics_document() {
    for name in [
        "invalid/e-unclosed-list.sigil",
        "valid/02-aimed-stream.sigil",
    ] {
        let path = corpus(name);
        let output = sigilc(&["parse", "--json", &path]);
        let expected_code = if name.starts_with("valid/") {
            EXIT_OK
        } else {
            EXIT_PROBLEMS
        };
        assert_eq!(output.code, expected_code, "{name}");
        let mut expected: Value =
            serde_json::from_str(&read_corpus(&name.replace(".sigil", ".expected.json"))).unwrap();
        expected["file"] = Value::from(path.as_str());
        for diagnostic in expected["diagnostics"].as_array_mut().unwrap() {
            diagnostic["file"] = Value::from(path.as_str());
        }
        assert_eq!(output.json(), expected, "{name}");
    }
    let missing = sigilc(&["parse", "--json", "tests/corpus/valid/missing.sigil"]);
    assert_eq!(missing.code, EXIT_ERROR);
}

// ---- set --------------------------------------------------------------------------------------

#[test]
fn set_rewrites_one_value_in_place() {
    let scratch = Scratch::new("set");
    let source = read_corpus("valid/02-aimed-stream.sigil");
    let file = scratch.write("aimed.sigil", &source);

    let output = sigilc(&["set", &file, "emitters.jitter.offset.y=-0.75u"]);
    assert_eq!(output.code, EXIT_OK, "{}", output.stdout);
    assert_eq!(
        output.stdout,
        format!("{file}: set emitters.jitter.offset.y = -0.75u (was -0.5u)\n")
    );
    let expected = source.replace("y = -0.5u)", "y = -0.75u)");
    assert_ne!(expected, source);
    assert_eq!(fs::read_to_string(&file).unwrap(), expected);

    let json = sigilc(&[
        "set",
        "--json",
        &file,
        "emitters.stream.modifiers[0].amplitude=0.5u",
    ]);
    assert_eq!(json.code, EXIT_OK);
    let document = json.json();
    assert_eq!(
        json.top_level_keys(),
        [
            "schema",
            "schema_version",
            "file",
            "node_path",
            "value",
            "ok",
            "changed",
            "old_value",
            "error",
            "diagnostics"
        ]
    );
    assert_eq!(document["schema"], "grimoire.sigilc.set");
    assert_eq!(document["ok"], true);
    assert_eq!(document["changed"], true);
    assert_eq!(document["old_value"], "0.4u");
    assert_eq!(document["error"], Value::Null);
    let expected = expected.replace("amplitude = 0.4u //", "amplitude = 0.5u //");
    assert_eq!(fs::read_to_string(&file).unwrap(), expected);

    // Setting the same value again changes nothing and still succeeds.
    let again = sigilc(&["set", &file, "emitters.stream.modifiers[0].amplitude=0.5u"]);
    assert_eq!(again.code, EXIT_OK);
    assert!(again.stdout.contains("is already 0.5u"));
}

#[test]
fn a_refused_set_leaves_the_file_untouched() {
    let scratch = Scratch::new("set-refused");
    let source = read_corpus("valid/02-aimed-stream.sigil");
    let file = scratch.write("aimed.sigil", &source);
    for (assignment, code) in [
        ("meta.nope=1", "not_found"),
        ("meta..density=1", "invalid_node_path"),
        ("meta.density=1 // c", "invalid_value"),
        ("meta.density=", "invalid_value"),
    ] {
        let output = sigilc(&["set", "--json", &file, assignment]);
        assert_eq!(output.code, EXIT_PROBLEMS, "{assignment}");
        let document = output.json();
        assert_eq!(document["ok"], false);
        assert_eq!(document["error"]["code"], code, "{assignment}");
        assert_eq!(document["old_value"], Value::Null);
        assert_eq!(fs::read_to_string(&file).unwrap(), source, "{assignment}");
    }

    let broken_source = read_corpus("invalid/e-unclosed-body.sigil");
    let broken = scratch.write("broken.sigil", &broken_source);
    let output = sigilc(&["set", "--json", &broken, "meta.density=1"]);
    assert_eq!(output.code, EXIT_PROBLEMS);
    let document = output.json();
    assert_eq!(document["error"]["code"], "source_has_errors");
    assert!(!document["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&broken).unwrap(), broken_source);

    let text = sigilc(&["set", &file, "meta.nope=1"]);
    assert_eq!(text.code, EXIT_PROBLEMS);
    assert!(text.stdout.contains("set refused (not_found)"));

    let missing = sigilc(&["set", &scratch.path("missing.sigil"), "meta.density=1"]);
    assert_eq!(missing.code, EXIT_ERROR);
}

// ---- fmt --------------------------------------------------------------------------------------

#[test]
fn fmt_checks_and_rewrites_to_the_canonical_layout() {
    let scratch = Scratch::new("fmt");
    let canonical = read_corpus("valid/01-ring-burst.sigil");
    let messy_source = canonical
        .replace("\n", "\r\n")
        .replace("  count = 24", "\tcount=24   ");
    let messy = scratch.write("messy.sigil", &messy_source);
    let clean = scratch.write("clean.sigil", &canonical);

    let check = sigilc(&["fmt", "--check", &messy, &clean]);
    assert_eq!(check.code, EXIT_PROBLEMS);
    assert_eq!(check.stdout, format!("{messy}: not in canonical layout\n"));
    assert_eq!(fs::read_to_string(&messy).unwrap(), messy_source);

    let write = sigilc(&["fmt", &messy, &clean]);
    assert_eq!(write.code, EXIT_OK);
    assert_eq!(write.stdout, format!("formatted {messy}\n"));
    assert_eq!(fs::read_to_string(&messy).unwrap(), canonical);
    assert_eq!(fs::read_to_string(&clean).unwrap(), canonical);

    assert_eq!(sigilc(&["fmt", "--check", &messy]).code, EXIT_OK);

    let broken_source = read_corpus("invalid/e-double-comma.sigil");
    let broken = scratch.write("broken.sigil", &broken_source);
    let refused = sigilc(&["fmt", &broken]);
    assert_eq!(refused.code, EXIT_PROBLEMS);
    assert!(
        refused.stdout.contains("error[SIG0009]"),
        "{}",
        refused.stdout
    );
    assert_eq!(fs::read_to_string(&broken).unwrap(), broken_source);

    let missing = sigilc(&["fmt", &scratch.path("missing.sigil"), &clean]);
    assert_eq!(missing.code, EXIT_ERROR);
}
