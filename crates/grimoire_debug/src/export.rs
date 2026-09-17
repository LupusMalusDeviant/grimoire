//! CSV and JSON export of profiler frames (plan 0002 WP6.3, format
//! `docs/formats/profiler-export.md`).
//!
//! A [`ProfileLog`] keeps [`Stats`] values, the same per-frame snapshot the debug protocol sends
//! (contract §13), so an export and a live tool see identical, identically truncated data.

use std::io;

use thiserror::Error;

use crate::generated::debug_protocol::Stats;

/// Schema name of the JSON export (contract §2 rule 11).
pub const PROFILE_EXPORT_SCHEMA: &str = "grimoire.profiler.export";

/// Format version of the JSON export and the CSV header.
pub const PROFILE_EXPORT_SCHEMA_VERSION: u32 = 1;

/// Largest integer a JSON number may carry (contract §2 rule 11: 2^53 − 1).
const MAX_SAFE_JSON_INTEGER: u64 = (1 << 53) - 1;

/// Column header of the CSV export, one row per scope or counter.
const CSV_HEADER: &str = "frame,sim_tick,ticks_this_frame,alpha,frame_time_ns,fps,dropped_time_ns,content_swaps,content_manifest,kind,scope,name,total_ns,calls,budget_ns,estimate,value\n";

/// Failure while exporting a [`ProfileLog`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ExportError {
    /// Writing to the output failed.
    #[error("writing the profiler export failed ({kind:?}): {message}")]
    Io {
        /// Kind of the I/O error.
        kind: io::ErrorKind,
        /// The error's message.
        message: String,
    },
    /// A JSON integer exceeds 2^53 − 1 (contract §2 rule 11), for example a saturated
    /// `u64::MAX` duration.
    #[error(
        "field `{field}` of frame {frame} holds {value}, above the JSON integer limit 2^53 - 1"
    )]
    IntegerTooLarge {
        /// Frame index of the offending value.
        frame: u64,
        /// Field name as written in the export.
        field: &'static str,
        /// The value.
        value: u64,
    },
    /// A JSON number is NaN or infinite (contract §2 rule 11).
    #[error("field `{field}` of frame {frame} is not a finite number")]
    NonFinite {
        /// Frame index of the offending value.
        frame: u64,
        /// Field name as written in the export.
        field: &'static str,
    },
}

impl From<io::Error> for ExportError {
    fn from(error: io::Error) -> Self {
        Self::Io {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

/// Recorded profiler frames with CSV and JSON export.
///
/// The log keeps every pushed frame; the caller decides how many to keep and calls
/// [`ProfileLog::clear`] when needed.
///
/// ```
/// use std::time::Duration;
/// use grimoire_debug::{FrameProfile, ProfileLog, ScopeId, StatsFrame};
///
/// let mut profile = FrameProfile::default();
/// let mut log = ProfileLog::new();
/// for frame in 0..2 {
///     profile.begin(frame);
///     profile.record(ScopeId(0), "sim", Duration::from_micros(900));
///     let mut values = StatsFrame::default();
///     values.frame = frame;
///     log.push(profile.to_stats(&values));
/// }
/// let mut json = Vec::new();
/// log.write_json(&mut json)?;
/// assert!(json.starts_with(br#"{"schema":"grimoire.profiler.export","schema_version":1,"#));
/// # Ok::<(), grimoire_debug::ExportError>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileLog {
    frames: Vec<Stats>,
}

impl ProfileLog {
    /// Creates an empty log. Identical to [`ProfileLog::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one frame.
    pub fn push(&mut self, frame: Stats) {
        self.frames.push(frame);
    }

    /// The recorded frames, in push order.
    #[must_use]
    pub fn frames(&self) -> &[Stats] {
        &self.frames
    }

    /// Number of recorded frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether no frame is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Removes every recorded frame.
    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// Writes the log as CSV (`docs/formats/profiler-export.md`): a header, then one row per scope
    /// and one per counter of every frame, a single `frame` row for a frame without either.
    ///
    /// CSV has no integer or float limits, so only I/O can fail.
    ///
    /// # Errors
    /// [`ExportError::Io`] if writing to `out` fails.
    pub fn write_csv(&self, out: &mut dyn io::Write) -> Result<(), ExportError> {
        let mut text = String::from(CSV_HEADER);
        for frame in &self.frames {
            let prefix = format!(
                "{},{},{},{},{},{},{},{},{:016x}",
                frame.frame,
                frame.sim_tick,
                frame.ticks_this_frame,
                frame.alpha,
                frame.frame_time_ns,
                frame.fps,
                frame.dropped_time_ns,
                frame.content_swaps,
                frame.content_manifest,
            );
            if frame.scopes.is_empty() && frame.counters.is_empty() {
                text.push_str(&prefix);
                text.push_str(",frame,,,,,,,\n");
            }
            for scope in &frame.scopes {
                text.push_str(&format!(
                    "{prefix},scope,{},{},{},{},{},{},\n",
                    scope.scope,
                    csv_field(&scope.name),
                    scope.total_ns,
                    scope.calls,
                    scope.budget_ns,
                    scope.estimate,
                ));
            }
            for counter in &frame.counters {
                text.push_str(&format!(
                    "{prefix},counter,,{},,,,,{}\n",
                    csv_field(&counter.name),
                    counter.value,
                ));
            }
        }
        out.write_all(text.as_bytes())?;
        Ok(())
    }

    /// Writes the log as one JSON document (`docs/formats/profiler-export.md`, contract §2
    /// rule 11): fixed key order, `content_manifest` as 16 lowercase hex digits, UTF-8 without
    /// BOM. Nothing is written if a value breaks the JSON rules.
    ///
    /// # Errors
    /// - [`ExportError::IntegerTooLarge`] for an integer above 2^53 − 1 (for example a saturated
    ///   duration).
    /// - [`ExportError::NonFinite`] for a NaN or infinite `alpha` or `fps`.
    /// - [`ExportError::Io`] if writing to `out` fails.
    pub fn write_json(&self, out: &mut dyn io::Write) -> Result<(), ExportError> {
        let mut text = format!(
            "{{\"schema\":\"{PROFILE_EXPORT_SCHEMA}\",\"schema_version\":{PROFILE_EXPORT_SCHEMA_VERSION},\"frames\":["
        );
        for (index, frame) in self.frames.iter().enumerate() {
            if index > 0 {
                text.push(',');
            }
            let id = frame.frame;
            text.push_str(&format!(
                "{{\"frame\":{},\"sim_tick\":{},\"ticks_this_frame\":{},\"alpha\":{},\"frame_time_ns\":{},\"fps\":{},\"dropped_time_ns\":{},\"content_swaps\":{},\"content_manifest\":\"{:016x}\",\"scopes\":[",
                json_integer(id, "frame", frame.frame)?,
                json_integer(id, "sim_tick", frame.sim_tick)?,
                frame.ticks_this_frame,
                json_float(id, "alpha", frame.alpha)?,
                json_integer(id, "frame_time_ns", frame.frame_time_ns)?,
                json_float(id, "fps", frame.fps)?,
                json_integer(id, "dropped_time_ns", frame.dropped_time_ns)?,
                frame.content_swaps,
                frame.content_manifest,
            ));
            for (position, scope) in frame.scopes.iter().enumerate() {
                if position > 0 {
                    text.push(',');
                }
                text.push_str(&format!(
                    "{{\"scope\":{},\"name\":{},\"total_ns\":{},\"calls\":{},\"budget_ns\":{},\"estimate\":{}}}",
                    scope.scope,
                    json_string(&scope.name),
                    json_integer(id, "total_ns", scope.total_ns)?,
                    scope.calls,
                    json_integer(id, "budget_ns", scope.budget_ns)?,
                    scope.estimate,
                ));
            }
            text.push_str("],\"counters\":[");
            for (position, counter) in frame.counters.iter().enumerate() {
                if position > 0 {
                    text.push(',');
                }
                text.push_str(&format!(
                    "{{\"name\":{},\"value\":{}}}",
                    json_string(&counter.name),
                    json_integer(id, "value", counter.value)?,
                ));
            }
            text.push_str("]}");
        }
        text.push_str("]}\n");
        out.write_all(text.as_bytes())?;
        Ok(())
    }
}

/// `value` if it fits a JSON number (contract §2 rule 11).
fn json_integer(frame: u64, field: &'static str, value: u64) -> Result<u64, ExportError> {
    if value > MAX_SAFE_JSON_INTEGER {
        return Err(ExportError::IntegerTooLarge {
            frame,
            field,
            value,
        });
    }
    Ok(value)
}

/// `value` if it is finite (contract §2 rule 11). Rust prints finite floats without an exponent,
/// which is always a valid JSON number, and exactly enough digits to read the same `f32` back.
fn json_float(frame: u64, field: &'static str, value: f32) -> Result<f32, ExportError> {
    if !value.is_finite() {
        return Err(ExportError::NonFinite { frame, field });
    }
    Ok(value)
}

/// `value` as a JSON string literal.
fn json_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                quoted.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// `value` as a CSV field (RFC 4180): quoted, with doubled quotes, when it contains a comma, a
/// quote or a line break.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{FrameProfile, ScopeId, StatsFrame};

    fn sample_log() -> ProfileLog {
        let mut profile = FrameProfile::default();
        profile.set_budget(ScopeId(0), Some(Duration::from_millis(4)));
        let mut log = ProfileLog::new();
        profile.begin(0);
        profile.record(ScopeId(0), "sim", Duration::from_nanos(1_250_000));
        profile.record_estimate(ScopeId(1), "gpu, \"estimated\"", Duration::from_nanos(500));
        profile.add_counter("bullets_drawn", 10_000);
        let mut values = StatsFrame {
            sim_tick: 1,
            ticks_this_frame: 1,
            alpha: 0.5,
            frame_time: Duration::from_nanos(16_666_667),
            fps: 59.94,
            content_manifest: 0x00ab_cdef,
            ..StatsFrame::default()
        };
        log.push(profile.to_stats(&values));
        profile.begin(1);
        values.frame = 1;
        log.push(profile.to_stats(&values));
        log
    }

    #[test]
    fn csv_has_a_row_per_scope_and_counter_and_quotes_names() {
        let mut out = Vec::new();
        sample_log().write_csv(&mut out).expect("writing to a Vec");
        let text = String::from_utf8(out).expect("UTF-8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], CSV_HEADER.trim_end());
        assert_eq!(
            lines[1],
            "0,1,1,0.5,16666667,59.94,0,0,0000000000abcdef,scope,0,sim,1250000,1,4000000,false,"
        );
        assert_eq!(
            lines[2],
            "0,1,1,0.5,16666667,59.94,0,0,0000000000abcdef,scope,1,\"gpu, \"\"estimated\"\"\",500,1,0,true,"
        );
        assert_eq!(
            lines[3],
            "0,1,1,0.5,16666667,59.94,0,0,0000000000abcdef,counter,,bullets_drawn,,,,,10000"
        );
        assert_eq!(
            lines[4],
            "1,1,1,0.5,16666667,59.94,0,0,0000000000abcdef,frame,,,,,,,"
        );
        assert_eq!(lines.len(), 5);
        let columns = CSV_HEADER.split(',').count();
        assert!(
            lines[1..]
                .iter()
                .filter(|line| !line.contains('"'))
                .all(|line| line.split(',').count() == columns)
        );
    }

    #[test]
    fn json_has_the_documented_shape_and_key_order() {
        let mut out = Vec::new();
        sample_log().write_json(&mut out).expect("writing to a Vec");
        let text = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            text,
            concat!(
                r#"{"schema":"grimoire.profiler.export","schema_version":1,"frames":["#,
                r#"{"frame":0,"sim_tick":1,"ticks_this_frame":1,"alpha":0.5,"frame_time_ns":16666667,"fps":59.94,"dropped_time_ns":0,"content_swaps":0,"content_manifest":"0000000000abcdef","scopes":["#,
                r#"{"scope":0,"name":"sim","total_ns":1250000,"calls":1,"budget_ns":4000000,"estimate":false},"#,
                r#"{"scope":1,"name":"gpu, \"estimated\"","total_ns":500,"calls":1,"budget_ns":0,"estimate":true}"#,
                r#"],"counters":[{"name":"bullets_drawn","value":10000}]},"#,
                r#"{"frame":1,"sim_tick":1,"ticks_this_frame":1,"alpha":0.5,"frame_time_ns":16666667,"fps":59.94,"dropped_time_ns":0,"content_swaps":0,"content_manifest":"0000000000abcdef","scopes":[],"counters":[]}"#,
                "]}\n"
            )
        );
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(
            parsed["frames"][0]["fps"].as_f64().map(|fps| fps as f32),
            Some(59.94)
        );
    }

    #[test]
    fn json_escapes_control_characters_and_stays_parseable() {
        let mut profile = FrameProfile::default();
        profile.begin(0);
        profile.record(ScopeId(0), "a\\b\n\u{1}é", Duration::ZERO);
        let mut log = ProfileLog::new();
        log.push(profile.to_stats(&StatsFrame::default()));
        let mut out = Vec::new();
        log.write_json(&mut out).expect("writing to a Vec");
        let text = String::from_utf8(out).expect("UTF-8");
        // Built from char codes so no tool turns the escapes into the characters themselves.
        let bs = char::from(92u8);
        let expected = format!("\"name\":\"a{bs}{bs}b{bs}n{bs}u0001\u{e9}\"");
        assert!(text.contains(&expected), "{text}");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(parsed["frames"][0]["scopes"][0]["name"], "a\\b\n\u{1}é");
    }

    #[test]
    fn json_refuses_unsafe_integers_and_non_finite_floats_without_writing() {
        let mut profile = FrameProfile::default();
        profile.begin(3);
        profile.record(ScopeId(0), "frame", Duration::MAX);
        let mut values = StatsFrame {
            frame: 3,
            ..StatsFrame::default()
        };
        let mut log = ProfileLog::new();
        log.push(profile.to_stats(&values));
        let mut out = Vec::new();
        assert_eq!(
            log.write_json(&mut out),
            Err(ExportError::IntegerTooLarge {
                frame: 3,
                field: "total_ns",
                value: u64::MAX
            })
        );
        assert!(out.is_empty());

        profile.begin(4);
        values.frame = 4;
        values.fps = f32::NAN;
        log.clear();
        log.push(profile.to_stats(&values));
        assert_eq!(
            log.write_json(&mut out),
            Err(ExportError::NonFinite {
                frame: 4,
                field: "fps"
            })
        );
        assert!(out.is_empty());
        // CSV has no such limits.
        log.write_csv(&mut out).expect("CSV accepts every value");
        assert!(!out.is_empty());
    }

    #[test]
    fn an_empty_log_exports_a_header_and_an_empty_frame_list() {
        let log = ProfileLog::new();
        assert!(log.is_empty());
        let mut csv = Vec::new();
        log.write_csv(&mut csv).expect("writing to a Vec");
        assert_eq!(csv, CSV_HEADER.as_bytes());
        let mut json = Vec::new();
        log.write_json(&mut json).expect("writing to a Vec");
        assert_eq!(
            json,
            b"{\"schema\":\"grimoire.profiler.export\",\"schema_version\":1,\"frames\":[]}\n"
        );
    }

    #[test]
    fn io_failures_become_export_errors() {
        struct Broken;
        impl io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = sample_log()
            .write_csv(&mut Broken)
            .expect_err("broken writer");
        assert!(matches!(
            error,
            ExportError::Io {
                kind: io::ErrorKind::BrokenPipe,
                ..
            }
        ));
    }
}
