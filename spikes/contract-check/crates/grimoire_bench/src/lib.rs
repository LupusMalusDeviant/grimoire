//! `grimoire_bench` schema types (contract §15). JSON code (serde) omitted.

use std::collections::BTreeMap;

use grimoire_sim_p1::ContentManifestHash;

pub const MAX_BENCH_LINE_BYTES: usize = 64 * 1024;
pub const MAX_GOLDEN_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SAMPLES: usize = 100_000;
pub const MAX_PARAMS: usize = 64;
pub const MAX_FINGERPRINT_ENTRIES: usize = 64;
pub const MAX_CHECKPOINTS: usize = 1 << 20;
pub const MAX_SUBSYSTEMS_PER_CHECKPOINT: usize = 1024;
pub const MAX_TEXT_BYTES: usize = 1024;

// ---- §15.1 ------------------------------------------------------------------------------------

pub const RESULT_SCHEMA: &str = "grimoire.bench.result";
pub const RESULT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Debug)]
pub struct BenchResult {
    pub scenario: String,
    pub metric: String,
    pub unit: String,
    pub median: f64,
    pub samples: Vec<f64>,
    pub commit: CommitRef,
    pub runner: RunnerInfo,
    pub executor: ExecutorInfo,
    pub params: BTreeMap<String, ParamValue>,
    pub value_origin: ValueOrigin,
    pub injected_regression_percent: u32,
    pub run: Option<RunKey>,
}

impl BenchResult {
    pub fn to_json_line(&self) -> Result<String, SchemaError> {
        unimplemented!()
    }
    pub fn from_json_line(line: &str) -> Result<Self, SchemaError> {
        let _ = line;
        unimplemented!()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitRef {
    pub sha: String,
    pub dirty: bool,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunnerInfo {
    pub os: String,
    pub arch: String,
    pub image: Option<String>,
    pub cpu_model: Option<String>,
    pub logical_cpus: u32,
    pub fingerprint: BTreeMap<String, String>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExecutorInfo {
    pub kind: String,
    pub threads: u32,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParamValue {
    Int(i64),
    Text(String),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValueOrigin {
    Runner,
    Reference,
    Estimate,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunKey {
    pub id: String,
    pub attempt: u32,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SchemaError {
    #[error("json: {0}")]
    Json(String),
    #[error("unknown schema {0}")]
    UnknownSchema(String),
    #[error("unsupported version {0}")]
    UnsupportedVersion(u32),
    #[error("missing field {0}")]
    MissingField(&'static str),
    #[error("unknown field {0}")]
    UnknownField(String),
    #[error("invalid value for {field}: {reason}")]
    InvalidValue { field: &'static str, reason: String },
    #[error("median mismatch")]
    MedianMismatch { stated: f64, computed: f64 },
    #[error("too large")]
    TooLarge { field: &'static str, len: usize, max: usize },
    #[error("too many entries")]
    TooManyEntries { field: &'static str, count: usize, max: usize },
}

pub fn hash_to_json(value: u64) -> String {
    format!("{value:016x}")
}
pub fn hash_from_json(text: &str) -> Result<u64, SchemaError> {
    let _ = text;
    unimplemented!()
}
pub fn median(samples: &[f64]) -> Option<f64> {
    let _ = samples;
    unimplemented!()
}

// ---- §15.2 ------------------------------------------------------------------------------------

pub const GOLDEN_SCHEMA: &str = "grimoire.golden";
pub const GOLDEN_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GoldenMaster {
    pub name: String,
    pub seed: u64,
    pub tick_rate_hz: u32,
    pub hash_every: u64,
    pub ticks: u64,
    pub replay: Option<String>,
    pub content_manifest: ContentManifestHash,
    pub algorithms: AlgorithmVersions,
    pub checkpoints: Vec<Checkpoint>,
    pub recorded_with: RecordedWith,
}

impl GoldenMaster {
    pub fn to_json(&self) -> Result<String, SchemaError> {
        unimplemented!()
    }
    pub fn from_json(text: &str) -> Result<GoldenMaster, SchemaError> {
        let _ = text;
        unimplemented!()
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AlgorithmVersions {
    pub stable_hasher: u32,
    pub sim_rng: u32,
}

impl AlgorithmVersions {
    pub fn current() -> AlgorithmVersions {
        AlgorithmVersions {
            stable_hasher: grimoire_core::StableHasher::ALGORITHM_VERSION,
            sim_rng: grimoire_sim::SimRng::ALGORITHM_VERSION,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecordedWith {
    pub engine_version: String,
    pub engine_build: String,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Checkpoint {
    pub tick: u64,
    pub state_hash: u64,
    pub subsystems: Vec<SubsystemHash>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SubsystemHash {
    pub name: String,
    pub hash: u64,
}
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GoldenRun {
    pub seed: u64,
    pub tick_rate_hz: u32,
    pub hash_every: u64,
    pub content_manifest: ContentManifestHash,
    pub algorithms: AlgorithmVersions,
    pub golden_eligible: bool,
    pub checkpoints: Vec<Checkpoint>,
}

impl GoldenRun {
    pub fn new(
        seed: u64,
        tick_rate_hz: u32,
        hash_every: u64,
        content_manifest: ContentManifestHash,
        golden_eligible: bool,
        checkpoints: Vec<Checkpoint>,
    ) -> GoldenRun {
        GoldenRun {
            seed,
            tick_rate_hz,
            hash_every,
            content_manifest,
            algorithms: AlgorithmVersions::current(),
            golden_eligible,
            checkpoints,
        }
    }
}

#[non_exhaustive]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GoldenVerdict {
    Match,
    NotEligible,
    ContentChanged {
        expected: ContentManifestHash,
        actual: ContentManifestHash,
    },
    ShapeMismatch {
        reason: String,
    },
    Diverged {
        tick: u64,
        expected: u64,
        actual: u64,
        first_subsystem: Option<String>,
    },
}

pub fn compare(master: &GoldenMaster, run: &GoldenRun) -> GoldenVerdict {
    if !run.golden_eligible {
        return GoldenVerdict::NotEligible;
    }
    if master.content_manifest != run.content_manifest {
        return GoldenVerdict::ContentChanged {
            expected: master.content_manifest,
            actual: run.content_manifest,
        };
    }
    if master.algorithms != run.algorithms {
        return GoldenVerdict::ShapeMismatch {
            reason: "algorithms".to_owned(),
        };
    }
    GoldenVerdict::Match
}

pub struct RenewalEntry {
    pub name: String,
    pub previous_final_hash: Option<u64>,
    pub new_final_hash: u64,
    pub first_diverging_tick: Option<u64>,
    pub first_subsystem: Option<String>,
    pub content_manifest_before: Option<ContentManifestHash>,
    pub content_manifest_after: ContentManifestHash,
    pub reason: String,
}

/// Bench measures 1 and N threads through `grimoire_exec` (§1, §15).
pub fn thread_pool(threads: usize) -> Option<grimoire_exec::ThreadPoolExecutor> {
    let _ = grimoire::DEFAULT_TICK_RATE_HZ;
    let _ = grimoire_core::StableHasher::ALGORITHM_VERSION;
    let _ = grimoire_sim::SimRng::ALGORITHM_VERSION;
    let _: Option<&dyn grimoire_p1::GamePlugin> = None;
    grimoire_exec::ThreadPoolExecutor::new(threads).ok()
}
