//! Bench-result JSON schema v1 (engine contract §15.1, Plan-0002 WP6.2).
//!
//! One [`BenchResult`] is one JSON Lines entry: the output of a single bench run and, appended
//! over time, the measurement history (the trend branch, P-12, is not part of this schema — it
//! only decides *where* these lines are stored). The wire format wraps the payload in a fixed
//! `schema`/`schema_version` envelope (contract §15) that is not itself a field of
//! [`BenchResult`]; [`BenchResult::to_json_line`] writes it, [`BenchResult::from_json_line`]
//! checks it.
//!
//! Everything here is strict (contract §15, "Streng"): an unknown `schema`, a higher
//! `schema_version`, a missing or unknown field, or a violated size limit is a [`SchemaError`],
//! never a panic. Only `runner.fingerprint` and `params` are extensible maps; every other field
//! set is closed (`deny_unknown_fields`).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{HexU64Error, u64_as_hex16, u64_from_hex16};

/// Schema name written into every [`BenchResult`] line (contract §15.1).
pub const RESULT_SCHEMA: &str = "grimoire.bench.result";
/// Schema version written into every [`BenchResult`] line (contract §15.1).
pub const RESULT_SCHEMA_VERSION: u32 = 1;

/// Maximum byte length of one JSON Lines line (contract §15, "Größengrenzen und Pfade").
pub const MAX_BENCH_LINE_BYTES: usize = 64 * 1024;
/// Maximum number of entries in [`BenchResult::samples`] (contract §15).
pub const MAX_SAMPLES: usize = 100_000;
/// Maximum number of entries in [`BenchResult::params`] (contract §15).
pub const MAX_PARAMS: usize = 64;
/// Maximum number of entries in [`RunnerInfo::fingerprint`] (contract §15).
pub const MAX_FINGERPRINT_ENTRIES: usize = 64;
/// Maximum byte length of a free-form string field without a narrower pattern (contract §15).
pub const MAX_TEXT_BYTES: usize = 1024;

/// Errors a [`BenchResult`] reader or writer can return (contract §15.1). Never a panic.
///
/// `#[non_exhaustive]`: contract §2 rule 13 (external code cannot construct or exhaustively match
/// this enum, so a new variant is not a breaking change).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SchemaError {
    /// The input was not valid JSON at all.
    #[error("invalid JSON: {0}")]
    Json(String),
    /// The `schema` field did not name this schema.
    #[error("unknown schema {0:?}, expected {RESULT_SCHEMA:?}")]
    UnknownSchema(String),
    /// The `schema_version` field named a version this reader does not understand.
    #[error("unsupported schema_version {0}, expected {RESULT_SCHEMA_VERSION}")]
    UnsupportedVersion(u32),
    /// A required field was absent.
    #[error("missing field {0:?}")]
    MissingField(&'static str),
    /// The input contained a field this schema does not define.
    #[error("unknown field {0:?}")]
    UnknownField(String),
    /// A field was present but its value violated the schema (pattern, closed set, range, …).
    #[error("invalid value for {field:?}: {reason}")]
    InvalidValue {
        /// The field whose value was rejected.
        field: &'static str,
        /// Human-readable reason, not machine-parsed.
        reason: String,
    },
    /// `median` did not match the value [`median`] computes from `samples`.
    #[error("stated median {stated} does not match computed median {computed}")]
    MedianMismatch {
        /// The `median` field as read from the input.
        stated: f64,
        /// What [`median`] computed from `samples`.
        computed: f64,
    },
    /// A field exceeded its maximum byte length.
    #[error("field {field:?} is {len} bytes, over the limit of {max}")]
    TooLarge {
        /// The oversized field.
        field: &'static str,
        /// Its actual length in bytes.
        len: usize,
        /// The limit it exceeded.
        max: usize,
    },
    /// A collection field exceeded its maximum entry count.
    #[error("field {field:?} has {count} entries, over the limit of {max}")]
    TooManyEntries {
        /// The oversized collection field.
        field: &'static str,
        /// Its actual entry count.
        count: usize,
        /// The limit it exceeded.
        max: usize,
    },
}

/// Formats `value` as exactly 16 lowercase hex digits (contract §2 rule 11, §15.1).
#[must_use]
pub fn hash_to_json(value: u64) -> String {
    u64_as_hex16(value)
}

/// Parses exactly 16 lowercase hex digits back into a `u64` (contract §2 rule 11, §15.1).
///
/// # Errors
/// Returns [`SchemaError::InvalidValue`] if `text` is not exactly 16 lowercase hex digits.
pub fn hash_from_json(text: &str) -> Result<u64, SchemaError> {
    u64_from_hex16(text).map_err(|err: HexU64Error| SchemaError::InvalidValue {
        field: "hash",
        reason: err.to_string(),
    })
}

/// Sorts `samples` and returns the median (contract §15.1): the middle element for an odd count,
/// the mean of the two middle elements for an even count. `None` for an empty slice.
#[must_use]
pub fn median(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

/// One bench-result JSON Lines entry (contract §15.1): the output of a single bench run, and one
/// row of the measurement history once appended to the trend branch (P-12).
///
/// Construct with plain struct-literal syntax (every field is public), then [`Self::to_json_line`]
/// to write it. [`Self::from_json_line`] parses and fully validates one line.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchResult {
    /// Stable scenario name, `[a-z0-9_]{1,64}` (e.g. `"ecs_query_10k"`, `"sim_step_600"`).
    pub scenario: String,
    /// Metric name, `[a-z0-9_]{1,32}`. v1: `"wall_time"`, `"instructions"`; more per the
    /// OF-17.3 ADR (engine ADR-0010).
    pub metric: String,
    /// Unit of `median`/`samples`: `"ns"`, `"ir"`, `"count"` or `"bytes"`.
    pub unit: String,
    /// The median of `samples`, recomputed and checked by both reader and writer.
    pub median: f64,
    /// Raw samples in measurement order. At least one, all finite, all non-negative. For
    /// `metric == "instructions"` every value is a whole number (Callgrind counts are exact).
    pub samples: Vec<f64>,
    /// The commit this measurement was built from.
    pub commit: CommitRef,
    /// The runner this measurement was taken on.
    pub runner: RunnerInfo,
    /// The executor this measurement ran under.
    pub executor: ExecutorInfo,
    /// Bench parameters (ticks, rounds, entity counts, …). Extensible.
    pub params: BTreeMap<String, ParamValue>,
    /// Where this value came from.
    pub value_origin: ValueOrigin,
    /// Non-zero only in the calibration self-test (Plan-0002 WP6.2): the nominal percentage of
    /// calibrated regression this run injected on top of the real bench body.
    pub injected_regression_percent: u32,
    /// CI run identity, when known.
    pub run: Option<RunKey>,
}

/// The commit a [`BenchResult`] was built from (contract §15.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRef {
    /// 40 lowercase hex characters (a full Git SHA-1).
    pub sha: String,
    /// `true` if the working tree had uncommitted changes. A dirty commit is never judged
    /// (contract §15.1, "Vergleichbarkeit"): comparators must skip it.
    pub dirty: bool,
}

/// The runner a [`BenchResult`] was measured on (contract §15.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerInfo {
    /// `"linux"`, `"windows"`, `"macos"`, …
    pub os: String,
    /// `"x86_64"`, `"aarch64"`, …
    pub arch: String,
    /// CI runner image, when known (e.g. `"ubuntu-24.04"`).
    pub image: Option<String>,
    /// CPU model string, when known.
    pub cpu_model: Option<String>,
    /// Logical CPU count.
    pub logical_cpus: u32,
    /// Extensible tool-version fingerprint (e.g. `{"valgrind": "3.22.0"}`).
    pub fingerprint: BTreeMap<String, String>,
}

/// The executor a [`BenchResult`] ran under (contract §15.1). N-thread benches
/// (`threads > 1`) may only report `metric == "wall_time"` (contract §15.1, "Threads").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorInfo {
    /// `"sequential"` or `"thread_pool"`.
    pub kind: String,
    /// Thread count. `1` when `kind == "sequential"`.
    pub threads: u32,
}

/// One bench parameter value (contract §15.1). Serializes as a bare JSON number or string, never
/// a tagged object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// An integer parameter (e.g. entity count, tick count).
    Int(i64),
    /// A free-form text parameter.
    Text(String),
}

/// Where a [`BenchResult`] value came from (contract §15.1). Serializes as a lowercase string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueOrigin {
    /// Measured on a CI runner.
    Runner,
    /// Measured in a reference measurement session on reference hardware.
    Reference,
    /// Converted with an estimated factor (P-4a A). Never an acceptance proof.
    Estimate,
}

/// CI run identity (contract §15.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunKey {
    /// Opaque run id (e.g. a GitHub Actions run id).
    pub id: String,
    /// Attempt number within that run.
    pub attempt: u32,
}

/// Wire representation: adds the fixed `schema`/`schema_version` envelope (contract §15) in
/// front of [`BenchResult`]'s own fields, in the exact field order the contract's example shows.
/// Field order matters here: serde's derived `Serialize` for a struct writes fields in
/// declaration order (unlike a generic JSON map, which this deliberately is not), which is how
/// this satisfies contract §15's "feste Schlüsselreihenfolge" without depending on
/// `serde_json`'s non-default `preserve_order` feature.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    schema: String,
    schema_version: u32,
    scenario: String,
    metric: String,
    unit: String,
    median: f64,
    samples: Vec<f64>,
    commit: CommitRef,
    runner: RunnerInfo,
    executor: ExecutorInfo,
    params: BTreeMap<String, ParamValue>,
    value_origin: ValueOrigin,
    injected_regression_percent: u32,
    run: Option<RunKey>,
}

fn check_text(field: &'static str, text: &str) -> Result<(), SchemaError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(SchemaError::TooLarge {
            field,
            len: text.len(),
            max: MAX_TEXT_BYTES,
        });
    }
    Ok(())
}

/// `^[a-z0-9_]{min,max}$`, checked byte-wise (no `regex` dependency for one small character
/// class; contract §2 rule 4 asks for a justification for every new third-party dependency).
fn check_slug(field: &'static str, text: &str, max_len: usize) -> Result<(), SchemaError> {
    let ok_len = !text.is_empty() && text.len() <= max_len;
    let ok_chars = text
        .bytes()
        .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'_');
    if !ok_len || !ok_chars {
        return Err(SchemaError::InvalidValue {
            field,
            reason: format!(
                "expected 1-{max_len} bytes of [a-z0-9_], got {text:?} ({} bytes)",
                text.len()
            ),
        });
    }
    Ok(())
}

fn check_sha(sha: &str) -> Result<(), SchemaError> {
    let ok = sha.len() == 40
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !ok {
        return Err(SchemaError::InvalidValue {
            field: "commit.sha",
            reason: format!("expected exactly 40 lowercase hex characters, got {sha:?}"),
        });
    }
    Ok(())
}

impl BenchResult {
    /// Validates every invariant this schema owns (contract §15, §15.1): slug patterns, the
    /// closed `unit`/`executor.kind` sets, the commit SHA shape, all size limits, every sample
    /// and the median being finite, and the stated median matching [`median`]'s recomputation.
    /// Called by both [`Self::to_json_line`] and [`Self::from_json_line`] — a writer must never
    /// produce a line a reader would then reject.
    ///
    /// # Errors
    /// Returns the first [`SchemaError`] found.
    pub fn validate(&self) -> Result<(), SchemaError> {
        check_slug("scenario", &self.scenario, 64)?;
        check_slug("metric", &self.metric, 32)?;
        if !matches!(self.unit.as_str(), "ns" | "ir" | "count" | "bytes") {
            return Err(SchemaError::InvalidValue {
                field: "unit",
                reason: format!("expected one of ns|ir|count|bytes, got {:?}", self.unit),
            });
        }

        if self.samples.is_empty() {
            return Err(SchemaError::MissingField("samples"));
        }
        if self.samples.len() > MAX_SAMPLES {
            return Err(SchemaError::TooManyEntries {
                field: "samples",
                count: self.samples.len(),
                max: MAX_SAMPLES,
            });
        }
        for sample in &self.samples {
            if !sample.is_finite() || *sample < 0.0 {
                return Err(SchemaError::InvalidValue {
                    field: "samples",
                    reason: format!("expected a finite value >= 0, got {sample}"),
                });
            }
        }
        if !self.median.is_finite() {
            return Err(SchemaError::InvalidValue {
                field: "median",
                reason: format!("expected a finite value, got {}", self.median),
            });
        }
        // median() never returns None: samples was already checked non-empty above.
        let computed = median(&self.samples).unwrap_or(self.median);
        if self.median.to_bits() != computed.to_bits() {
            return Err(SchemaError::MedianMismatch {
                stated: self.median,
                computed,
            });
        }

        check_sha(&self.commit.sha)?;

        check_text("runner.os", &self.runner.os)?;
        check_text("runner.arch", &self.runner.arch)?;
        if let Some(image) = &self.runner.image {
            check_text("runner.image", image)?;
        }
        if let Some(cpu_model) = &self.runner.cpu_model {
            check_text("runner.cpu_model", cpu_model)?;
        }
        if self.runner.fingerprint.len() > MAX_FINGERPRINT_ENTRIES {
            return Err(SchemaError::TooManyEntries {
                field: "runner.fingerprint",
                count: self.runner.fingerprint.len(),
                max: MAX_FINGERPRINT_ENTRIES,
            });
        }
        for (key, value) in &self.runner.fingerprint {
            check_text("runner.fingerprint.key", key)?;
            check_text("runner.fingerprint.value", value)?;
        }

        if !matches!(self.executor.kind.as_str(), "sequential" | "thread_pool") {
            return Err(SchemaError::InvalidValue {
                field: "executor.kind",
                reason: format!(
                    "expected sequential|thread_pool, got {:?}",
                    self.executor.kind
                ),
            });
        }
        if self.executor.threads == 0 {
            return Err(SchemaError::InvalidValue {
                field: "executor.threads",
                reason: "expected at least 1 thread".to_string(),
            });
        }
        if self.executor.kind == "sequential" && self.executor.threads != 1 {
            return Err(SchemaError::InvalidValue {
                field: "executor.threads",
                reason: "sequential executor must report exactly 1 thread".to_string(),
            });
        }
        // Contract §15.1 "Threads": N-thread benches (threads > 1) may only report wall_time.
        if self.executor.threads > 1 && self.metric != "wall_time" {
            return Err(SchemaError::InvalidValue {
                field: "metric",
                reason: format!(
                    "executor.threads > 1 may only report wall_time, got metric {:?}",
                    self.metric
                ),
            });
        }

        if self.params.len() > MAX_PARAMS {
            return Err(SchemaError::TooManyEntries {
                field: "params",
                count: self.params.len(),
                max: MAX_PARAMS,
            });
        }
        for (key, value) in &self.params {
            check_text("params.key", key)?;
            if let ParamValue::Text(text) = value {
                check_text("params.value", text)?;
            }
        }

        if let Some(run) = &self.run {
            check_text("run.id", &run.id)?;
        }

        Ok(())
    }

    /// Validates `self`, then writes it as one JSON Lines line (no trailing newline), with the
    /// fixed `schema`/`schema_version` envelope (contract §15).
    ///
    /// # Errors
    /// Returns [`SchemaError`] if [`Self::validate`] fails, if serialization fails (only possible
    /// for a non-finite float, already rejected by `validate`, or a map key that cannot occur
    /// here), or if the resulting line would exceed [`MAX_BENCH_LINE_BYTES`].
    pub fn to_json_line(&self) -> Result<String, SchemaError> {
        self.validate()?;
        let wire = Wire {
            schema: RESULT_SCHEMA.to_string(),
            schema_version: RESULT_SCHEMA_VERSION,
            scenario: self.scenario.clone(),
            metric: self.metric.clone(),
            unit: self.unit.clone(),
            median: self.median,
            samples: self.samples.clone(),
            commit: self.commit.clone(),
            runner: self.runner.clone(),
            executor: self.executor.clone(),
            params: self.params.clone(),
            value_origin: self.value_origin,
            injected_regression_percent: self.injected_regression_percent,
            run: self.run.clone(),
        };
        let line =
            serde_json::to_string(&wire).map_err(|err| SchemaError::Json(err.to_string()))?;
        if line.len() > MAX_BENCH_LINE_BYTES {
            return Err(SchemaError::TooLarge {
                field: "line",
                len: line.len(),
                max: MAX_BENCH_LINE_BYTES,
            });
        }
        Ok(line)
    }

    /// Parses and fully validates one JSON Lines line (contract §15).
    ///
    /// Checks the input's byte length before parsing (contract §15, "Größengrenzen und Pfade":
    /// "Jeder Leser prüft vor dem Parsen die Eingabelänge"), then the `schema`/`schema_version`
    /// envelope, then every field via [`Self::validate`].
    ///
    /// # Errors
    /// Returns [`SchemaError::TooLarge`] if `line` exceeds [`MAX_BENCH_LINE_BYTES`],
    /// [`SchemaError::Json`] if it is not valid JSON or has an unknown/missing field,
    /// [`SchemaError::UnknownSchema`]/[`SchemaError::UnsupportedVersion`] for a wrong envelope,
    /// or any other [`SchemaError`] from [`Self::validate`].
    pub fn from_json_line(line: &str) -> Result<Self, SchemaError> {
        if line.len() > MAX_BENCH_LINE_BYTES {
            return Err(SchemaError::TooLarge {
                field: "line",
                len: line.len(),
                max: MAX_BENCH_LINE_BYTES,
            });
        }
        let wire: Wire =
            serde_json::from_str(line).map_err(|err| SchemaError::Json(err.to_string()))?;
        if wire.schema != RESULT_SCHEMA {
            return Err(SchemaError::UnknownSchema(wire.schema));
        }
        if wire.schema_version != RESULT_SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion(wire.schema_version));
        }
        let result = BenchResult {
            scenario: wire.scenario,
            metric: wire.metric,
            unit: wire.unit,
            median: wire.median,
            samples: wire.samples,
            commit: wire.commit,
            runner: wire.runner,
            executor: wire.executor,
            params: wire.params,
            value_origin: wire.value_origin,
            injected_regression_percent: wire.injected_regression_percent,
            run: wire.run,
        };
        result.validate()?;
        Ok(result)
    }

    /// Two results are comparable (contract §15.1, "Vergleichbarkeit") only if `scenario`,
    /// `metric`, `unit`, `executor` and `params` are all equal, and neither commit is dirty.
    /// A comparator must check this before judging a gate; it is not implied by
    /// [`Self::validate`], which only checks one result on its own.
    #[must_use]
    pub fn comparable_to(&self, other: &BenchResult) -> bool {
        !self.commit.dirty
            && !other.commit.dirty
            && self.scenario == other.scenario
            && self.metric == other.metric
            && self.unit == other.unit
            && self.executor == other.executor
            && self.params == other.params
    }
}

impl fmt::Display for BenchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{} = {} {} (commit {}{})",
            self.scenario,
            self.metric,
            self.median,
            self.unit,
            &self.commit.sha[..self.commit.sha.len().min(8)],
            if self.commit.dirty { ", dirty" } else { "" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result() -> BenchResult {
        BenchResult {
            scenario: "ecs_query_10k".to_string(),
            metric: "instructions".to_string(),
            unit: "ir".to_string(),
            median: 48_211_377.0,
            samples: vec![48_211_377.0],
            commit: CommitRef {
                sha: "f".repeat(40),
                dirty: false,
            },
            runner: RunnerInfo {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                image: Some("ubuntu-24.04".to_string()),
                cpu_model: None,
                logical_cpus: 4,
                fingerprint: BTreeMap::from([("valgrind".to_string(), "3.22.0".to_string())]),
            },
            executor: ExecutorInfo {
                kind: "sequential".to_string(),
                threads: 1,
            },
            params: BTreeMap::from([
                ("entities".to_string(), ParamValue::Int(10_000)),
                ("rounds".to_string(), ParamValue::Int(1)),
            ]),
            value_origin: ValueOrigin::Runner,
            injected_regression_percent: 0,
            run: None,
        }
    }

    #[test]
    fn round_trips() {
        let original = sample_result();
        let line = original.to_json_line().unwrap();
        let parsed = BenchResult::from_json_line(&line).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn wire_has_fixed_key_order() {
        let line = sample_result().to_json_line().unwrap();
        let schema_at = line.find("\"schema\"").unwrap();
        let scenario_at = line.find("\"scenario\"").unwrap();
        let run_at = line.find("\"run\"").unwrap();
        assert!(schema_at < scenario_at);
        assert!(scenario_at < run_at);
    }

    #[test]
    fn rejects_unknown_schema() {
        let mut result = sample_result();
        result.scenario = "x".to_string();
        let line = result
            .to_json_line()
            .unwrap()
            .replacen(RESULT_SCHEMA, "grimoire.other", 1);
        assert_eq!(
            BenchResult::from_json_line(&line),
            Err(SchemaError::UnknownSchema("grimoire.other".to_string()))
        );
    }

    #[test]
    fn rejects_unsupported_version() {
        let line = sample_result().to_json_line().unwrap().replacen(
            "\"schema_version\":1",
            "\"schema_version\":2",
            1,
        );
        assert_eq!(
            BenchResult::from_json_line(&line),
            Err(SchemaError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn rejects_unknown_field() {
        let line = sample_result().to_json_line().unwrap().replacen(
            "\"run\":null}",
            "\"run\":null,\"extra\":1}",
            1,
        );
        assert!(matches!(
            BenchResult::from_json_line(&line),
            Err(SchemaError::Json(_))
        ));
    }

    #[test]
    fn rejects_missing_field() {
        let line = sample_result()
            .to_json_line()
            .unwrap()
            .replacen("\"run\":null}", "}", 1);
        assert!(matches!(
            BenchResult::from_json_line(&line),
            Err(SchemaError::Json(_))
        ));
    }

    #[test]
    fn writer_rejects_nan_sample() {
        let mut result = sample_result();
        result.samples = vec![f64::NAN];
        result.median = f64::NAN;
        assert!(matches!(
            result.to_json_line(),
            Err(SchemaError::InvalidValue {
                field: "samples",
                ..
            })
        ));
    }

    #[test]
    fn writer_rejects_infinite_median() {
        let mut result = sample_result();
        result.median = f64::INFINITY;
        assert!(matches!(
            result.to_json_line(),
            Err(SchemaError::InvalidValue {
                field: "median",
                ..
            })
        ));
    }

    #[test]
    fn median_mismatch_is_rejected() {
        let mut result = sample_result();
        result.median = 1.0;
        assert!(matches!(
            result.to_json_line(),
            Err(SchemaError::MedianMismatch { .. })
        ));
    }

    #[test]
    fn median_even_count_averages_middle_two() {
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
    }

    #[test]
    fn median_odd_count_is_middle_element() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    }

    #[test]
    fn median_single_sample() {
        assert_eq!(median(&[7.0]), Some(7.0));
    }

    #[test]
    fn median_empty_is_none() {
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn rejects_bad_sha() {
        let mut result = sample_result();
        result.commit.sha = "not-a-sha".to_string();
        assert!(matches!(
            result.validate(),
            Err(SchemaError::InvalidValue {
                field: "commit.sha",
                ..
            })
        ));
    }

    #[test]
    fn rejects_uppercase_sha() {
        let mut result = sample_result();
        result.commit.sha = "F".repeat(40);
        assert!(result.validate().is_err());
    }

    #[test]
    fn rejects_bad_scenario_slug() {
        let mut result = sample_result();
        result.scenario = "Ecs Query".to_string();
        assert!(matches!(
            result.validate(),
            Err(SchemaError::InvalidValue {
                field: "scenario",
                ..
            })
        ));
    }

    #[test]
    fn rejects_bad_unit() {
        let mut result = sample_result();
        result.unit = "seconds".to_string();
        assert!(matches!(
            result.validate(),
            Err(SchemaError::InvalidValue { field: "unit", .. })
        ));
    }

    #[test]
    fn rejects_empty_samples() {
        let mut result = sample_result();
        result.samples = vec![];
        assert_eq!(result.validate(), Err(SchemaError::MissingField("samples")));
    }

    #[test]
    fn rejects_too_many_samples() {
        let mut result = sample_result();
        result.samples = vec![1.0; MAX_SAMPLES + 1];
        result.median = median(&result.samples).unwrap();
        assert!(matches!(
            result.validate(),
            Err(SchemaError::TooManyEntries {
                field: "samples",
                ..
            })
        ));
    }

    #[test]
    fn rejects_too_many_params() {
        let mut result = sample_result();
        result.params = (0..=MAX_PARAMS)
            .map(|i| (format!("p{i}"), ParamValue::Int(i as i64)))
            .collect();
        assert!(matches!(
            result.validate(),
            Err(SchemaError::TooManyEntries {
                field: "params",
                ..
            })
        ));
    }

    #[test]
    fn rejects_too_many_fingerprint_entries() {
        let mut result = sample_result();
        result.runner.fingerprint = (0..=MAX_FINGERPRINT_ENTRIES)
            .map(|i| (format!("k{i}"), "v".to_string()))
            .collect();
        assert!(matches!(
            result.validate(),
            Err(SchemaError::TooManyEntries {
                field: "runner.fingerprint",
                ..
            })
        ));
    }

    #[test]
    fn rejects_oversized_text_field() {
        let mut result = sample_result();
        result.runner.os = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(matches!(
            result.validate(),
            Err(SchemaError::TooLarge {
                field: "runner.os",
                ..
            })
        ));
    }

    #[test]
    fn nthread_bench_may_only_report_wall_time() {
        let mut result = sample_result();
        result.executor = ExecutorInfo {
            kind: "thread_pool".to_string(),
            threads: 4,
        };
        assert!(matches!(
            result.validate(),
            Err(SchemaError::InvalidValue {
                field: "metric",
                ..
            })
        ));
    }

    #[test]
    fn sequential_executor_must_report_one_thread() {
        let mut result = sample_result();
        result.executor.threads = 2;
        assert!(matches!(
            result.validate(),
            Err(SchemaError::InvalidValue {
                field: "executor.threads",
                ..
            })
        ));
    }

    #[test]
    fn dirty_commit_is_never_comparable() {
        let mut dirty = sample_result();
        dirty.commit.dirty = true;
        assert!(!dirty.comparable_to(&sample_result()));
        assert!(!sample_result().comparable_to(&dirty));
    }

    #[test]
    fn different_params_are_not_comparable() {
        let mut other = sample_result();
        other.params.insert("extra".to_string(), ParamValue::Int(1));
        assert!(!sample_result().comparable_to(&other));
    }

    #[test]
    fn identical_results_are_comparable() {
        assert!(sample_result().comparable_to(&sample_result()));
    }

    #[test]
    fn param_value_round_trips_untagged() {
        let result = sample_result();
        let line = result.to_json_line().unwrap();
        assert!(line.contains("\"entities\":10000"));
        assert!(!line.contains("\"Int\""));
    }

    #[test]
    fn value_origin_round_trips_as_snake_case_string() {
        let result = sample_result();
        let line = result.to_json_line().unwrap();
        assert!(line.contains("\"value_origin\":\"runner\""));
    }

    #[test]
    fn hash_helpers_round_trip() {
        assert_eq!(
            hash_from_json(&hash_to_json(0xdead_beef)).unwrap(),
            0xdead_beef
        );
        assert!(hash_from_json("not-hex").is_err());
    }

    // Proptest with arbitrary, truncated and individually altered lines: never a panic (contract
    // §15.1 "Vertragstests"). Keeps the case count modest (no separate CI budget for it).
    proptest::proptest! {
        #[test]
        fn from_json_line_never_panics(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256)) {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let _ = BenchResult::from_json_line(text);
            }
        }

        #[test]
        fn truncated_valid_line_never_panics(len in 0usize..200) {
            let line = sample_result().to_json_line().unwrap();
            let cut = line.as_bytes().get(..len.min(line.len())).unwrap_or(&[]);
            if let Ok(text) = std::str::from_utf8(cut) {
                let _ = BenchResult::from_json_line(text);
            }
        }
    }
}
