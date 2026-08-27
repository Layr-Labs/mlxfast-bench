//! Telemetry provider trait: macmon (M5) | nvml (DGX Spark).
//!
//! Temp gate, GPU/SM clock floor, util quiescence sampling. On Spark, clock-lock via NVML
//! is the reproducibility spine (nvidia-smi -lgc). Values come from target.toml, not
//! hardcoded Apple numbers. Readers are ring-0-owned (a gamed gate is a scoring attack).
//!
//! This crate implements the **macmon** provider (WS2-6, canonical issue #7 in the engine
//! development tracker). Its normative spec is `docs/measure-job-contract.md`
//! §3 "How macmon is sampled". Two things live here:
//!
//! 1. **Parsing** — [`MacmonSample`] turns one `macmon pipe` JSON object into the three
//!    numbers the gates need (`temp.gpu_temp_avg` °C, `gpu_usage[0]` freq MHz,
//!    `gpu_usage[1]` util 0..1), robust to the many other keys macmon emits.
//! 2. **Gate logic** — [`cool_gate_ok`], [`steady_loaded_samples`], [`clock_floor_ok`],
//!    [`util_quiescent`] are pure functions over already-parsed samples so they are
//!    fully unit-testable without a live macmon.
//!
//! The one thing that CANNOT be unit-tested here is that the samples we parse actually
//! match what the macmon CLI emits on the box: macmon is not installed in this
//! workspace. [`MacmonProvider::sample`] is therefore kept deliberately thin — it shells
//! out to `macmon pipe -s1` and hands the line to the tested parser. Confirming
//! "samples match the macmon CLI" is a live check deferred to the ai-server; the parsing
//! and gate logic are what the tests in this crate lock down.

use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure modes of the telemetry path. Std-only (no `thiserror`) so the crate stays
/// buildable offline against the crates already in `Cargo.lock`.
#[derive(Debug)]
pub enum TelemetryError {
    /// Spawning / reading the macmon child process failed.
    Io(std::io::Error),
    /// A macmon JSON line was present but did not parse into a [`MacmonSample`].
    Parse(String),
    /// macmon ran but misbehaved (non-zero exit, empty output, stderr diagnostic).
    Macmon(String),
}

impl std::fmt::Display for TelemetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TelemetryError::Io(e) => write!(f, "telemetry io error: {e}"),
            TelemetryError::Parse(m) => write!(f, "macmon sample parse error: {m}"),
            TelemetryError::Macmon(m) => write!(f, "macmon error: {m}"),
        }
    }
}

impl std::error::Error for TelemetryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TelemetryError::Io(e) => Some(e),
            TelemetryError::Parse(_) | TelemetryError::Macmon(_) => None,
        }
    }
}

impl From<std::io::Error> for TelemetryError {
    fn from(e: std::io::Error) -> Self {
        TelemetryError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// macmon `pipe` sample parsing
// ---------------------------------------------------------------------------

/// The `temp` sub-object of a macmon `pipe` record. Only `gpu_temp_avg` is read; every
/// other key (`cpu_temp_avg`, …) is ignored by serde's default unknown-field handling.
#[derive(Debug, Deserialize)]
struct MacmonWireTemp {
    gpu_temp_avg: f64,
}

/// The subset of a macmon `pipe` JSON object this crate reads. macmon emits many more
/// top-level keys (`memory`, `ecpu_usage`, `pcpu_usage`, `*_power`, …); serde ignores
/// them, so an added field never breaks parsing.
///
/// `gpu_usage` is macmon's `(frequency, usage)` pair, serialized as a JSON array. We
/// deserialize it as a `Vec<f64>` (rather than a fixed 2-tuple) so a future macmon that
/// appends a third array element still parses; the length is validated in
/// [`TryFrom<MacmonWire> for MacmonSample`].
#[derive(Debug, Deserialize)]
struct MacmonWire {
    temp: MacmonWireTemp,
    gpu_usage: Vec<f64>,
}

/// One parsed macmon sample: exactly the three numbers the thermal / clock gates need.
///
/// Deserializes directly from a macmon `pipe` JSON object via the intermediate
/// [`MacmonWire`] shape (`#[serde(try_from)]`), so `serde_json::from_str::<MacmonSample>`
/// on a raw macmon line yields this flattened struct. Extra top-level keys are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "MacmonWire")]
pub struct MacmonSample {
    /// GPU temperature in °C (macmon `temp.gpu_temp_avg`).
    pub gpu_temp_c: f64,
    /// GPU frequency in MHz (macmon `gpu_usage[0]`).
    pub gpu_freq_mhz: f64,
    /// GPU utilization, 0..1 (macmon `gpu_usage[1]`).
    pub gpu_util: f64,
}

impl TryFrom<MacmonWire> for MacmonSample {
    type Error = String;

    fn try_from(w: MacmonWire) -> Result<Self, Self::Error> {
        let gpu_freq_mhz = w
            .gpu_usage
            .first()
            .copied()
            .ok_or_else(|| "gpu_usage[0] (frequency MHz) missing".to_string())?;
        let gpu_util = w
            .gpu_usage
            .get(1)
            .copied()
            .ok_or_else(|| "gpu_usage[1] (utilization) missing".to_string())?;
        Ok(MacmonSample {
            gpu_temp_c: w.temp.gpu_temp_avg,
            gpu_freq_mhz,
            gpu_util,
        })
    }
}

impl MacmonSample {
    /// Parse a single macmon `pipe` line (one JSON object) into a sample.
    ///
    /// Works for both sampling modes in the spec: the `macmon pipe -s1` point sample and
    /// one line of a `macmon pipe -i <ms>` `.jsonl` stream. Surrounding whitespace /
    /// trailing newline is tolerated.
    pub fn from_pipe_line(line: &str) -> Result<Self, TelemetryError> {
        serde_json::from_str(line.trim()).map_err(|e| TelemetryError::Parse(e.to_string()))
    }

    /// Whether this sample counts as GPU-"loaded" under `cfg` (`util >= gpu_loaded_util`).
    ///
    /// This is the per-sample predicate; it does NOT by itself make a sample count toward
    /// the clock floor — that additionally requires the *preceding* sample to be loaded,
    /// see [`steady_loaded_samples`].
    pub fn is_loaded(&self, cfg: &GateConfig) -> bool {
        self.gpu_util >= cfg.gpu_loaded_util
    }
}

/// Parse a whole macmon `pipe -i <ms>` stream (`.jsonl` text, one JSON object per line)
/// into samples. Blank lines are skipped; the first parse failure aborts with a
/// [`TelemetryError::Parse`] naming the offending line.
pub fn parse_pipe_stream(stream: &str) -> Result<Vec<MacmonSample>, TelemetryError> {
    let mut out = Vec::new();
    for (idx, line) in stream.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let sample = MacmonSample::from_pipe_line(line)
            .map_err(|e| TelemetryError::Parse(format!("line {}: {e}", idx + 1)))?;
        out.push(sample);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Gate configuration
// ---------------------------------------------------------------------------

/// The thermal / clock-floor gate thresholds for one target, sourced from a **signed
/// `target.toml [telemetry]`** — never hardcoded Apple numbers and never runtime-overridable
/// (a gate a submitted workload can weaken is a scoring attack). See
/// `docs/measure-job-contract.md` §3.
///
/// Only the three gate thresholds the pure functions in this crate consume live here.
/// Other operator constants from the spec table (`cool_timeout_s`, `min_loaded_samples`,
/// `sample_interval_ms`, the preflight quiescence ceiling, …) are owned elsewhere in the
/// pipeline, not by the gate math.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct GateConfig {
    /// Cool-gate ceiling in °C: a leg may start only when GPU temp is at/below this.
    pub cool_gate_c: f64,
    /// Clock floor in MHz: every steady-loaded sample must sustain at least this.
    pub clock_floor_mhz: f64,
    /// "Loaded" utilization threshold, 0..1: a sample is loaded when `util >= this`.
    /// (Spec/target.toml field name `loaded_util`; accepted as an alias.)
    #[serde(alias = "loaded_util")]
    pub gpu_loaded_util: f64,
}

impl GateConfig {
    /// Box-3 QMTP (Qwen-MTP) reference thresholds: cool-gate **40 °C**, clock floor
    /// **1100 MHz**, loaded-util **0.70**.
    ///
    /// These are **placeholder defaults for tests and bring-up only**. A real run MUST
    /// take these from the target's signed `target.toml [telemetry]`, which overrides
    /// them. They are *per-silicon* — the 1100 MHz floor and 0.70 util threshold are
    /// **jointly calibrated on box 3** and are only meaningful together
    /// (`measure-job-contract.md` §3); they **do not carry across hardware**. GEN/Laguna
    /// runs at 1600 MHz / 0.90, DFLASH at 1500 MHz / 0.90. Never seed a new track from a
    /// quoted MHz number — re-derive per track/silicon from ≥5 gated sessions.
    pub fn box3_qmtp_defaults() -> Self {
        Self {
            cool_gate_c: 40.0,
            clock_floor_mhz: 1100.0,
            gpu_loaded_util: 0.70,
        }
    }
}

// ---------------------------------------------------------------------------
// Gate logic (pure, over already-parsed samples)
// ---------------------------------------------------------------------------

/// Cool gate: is the GPU cool enough to start a timed leg?
///
/// Passes when `temp_c` is **at or below** `cfg.cool_gate_c` (inclusive at the boundary,
/// per the WS2-6 "at/below" contract). Runs before every timed attempt on both legs.
pub fn cool_gate_ok(temp_c: f64, cfg: &GateConfig) -> bool {
    temp_c <= cfg.cool_gate_c
}

/// The subset of `samples` that counts toward the **clock floor** — the crux of the gate
/// (`docs/measure-job-contract.md` §3, "Steady loaded").
///
/// A sample counts only if it is loaded (`util >= gpu_loaded_util`) **and its immediately
/// preceding sample was also loaded**. The first sample of each loaded stretch is a
/// macmon idle→load **ramp artifact** and is therefore excluded (its predecessor is not
/// loaded, or it is the very first sample). This exclusion is load-bearing: the ramp
/// sample once false-rejected the pinned baseline itself at 51-55 °C.
///
/// Ramp samples are excluded ONLY from the floor — never from raw sample counts, which
/// are tallied separately.
pub fn steady_loaded_samples(samples: &[MacmonSample], cfg: &GateConfig) -> Vec<MacmonSample> {
    samples
        .iter()
        .enumerate()
        .filter(|(i, s)| *i > 0 && s.is_loaded(cfg) && samples[i - 1].is_loaded(cfg))
        .map(|(_, s)| *s)
        .collect()
}

/// Clock-floor gate: did every steady-loaded sample sustain the floor?
///
/// Passes when every sample in [`steady_loaded_samples`] has
/// `gpu_freq_mhz >= cfg.clock_floor_mhz`. A single below-floor **steady-loaded** sample
/// fails the gate (a throttle rejection); a below-floor **ramp** sample is excluded and
/// does not fail it.
///
/// **Empty steady-loaded set → fail (fail-closed).** No steady-loaded samples means there
/// is no evidence the clock was ever held under sustained load, so the floor cannot be
/// considered met. (This is distinct from the *sample-count* rejection `<
/// MIN_LOADED_SAMPLES`, which is a separate gate owning that threshold; this function
/// only answers "did the samples that DO count all clear the floor?" and treats "none
/// counted" as a failure rather than a vacuous pass.)
pub fn clock_floor_ok(samples: &[MacmonSample], cfg: &GateConfig) -> bool {
    let steady = steady_loaded_samples(samples, cfg);
    if steady.is_empty() {
        return false;
    }
    steady.iter().all(|s| s.gpu_freq_mhz >= cfg.clock_floor_mhz)
}

/// Pre-run quiescence check: is the GPU quiet enough to begin?
///
/// Passes when `gpu_util` is at/below `max_gpu_util`. This defends the "run hot to game
/// the floor" attack (`docs/measure-job-contract.md` §3 / ARCH §8 cycle 6): a workload
/// holding the GPU busy fails quiescence and cannot start a timed leg.
///
/// The quiescence ceiling (`PREFLIGHT_MAX_GPU_UTIL`, e.g. 0.10 on box 3) is a **separate,
/// much stricter** number than the [`GateConfig::gpu_loaded_util`] "loaded" threshold, so
/// it is passed explicitly rather than read from [`GateConfig`]. Pairing 0.70 "loaded"
/// with a 0.10 quiescence ceiling on the same box is intentional.
pub fn util_quiescent(gpu_util: f64, max_gpu_util: f64) -> bool {
    gpu_util <= max_gpu_util
}

// ---------------------------------------------------------------------------
// Provider trait + macmon provider
// ---------------------------------------------------------------------------

/// A point telemetry sample: temperature + utilization, the two numbers the cool /
/// quiescence gates read. (Frequency is only meaningful across a loaded stream, so it is
/// carried by [`MacmonSample`], not here.)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TelemetrySample {
    /// GPU temperature in °C.
    pub gpu_temp_c: f64,
    /// GPU utilization, 0..1.
    pub gpu_util: f64,
}

impl From<MacmonSample> for TelemetrySample {
    fn from(s: MacmonSample) -> Self {
        TelemetrySample {
            gpu_temp_c: s.gpu_temp_c,
            gpu_util: s.gpu_util,
        }
    }
}

/// Identifying metadata for a telemetry provider (for report headers / provenance).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderMetadata {
    /// Provider kind, e.g. `"macmon"` (M5) or `"nvml"` (DGX Spark).
    pub name: String,
    /// Where the samples come from, e.g. the resolved macmon binary path.
    pub source: String,
}

/// A source of host-owned GPU telemetry. Point samples feed the cool / quiescence gates;
/// the streaming path (parsed via [`parse_pipe_stream`]) feeds the clock-floor gate.
pub trait TelemetryProvider {
    /// Take a single point sample (temperature + utilization) right now.
    fn sample(&self) -> Result<TelemetrySample, TelemetryError>;

    /// Identifying metadata for this provider.
    fn metadata(&self) -> ProviderMetadata;
}

/// The macmon (Apple-silicon) telemetry provider.
///
/// Shells out to a configurable macmon binary — default
/// [`MacmonProvider::DEFAULT_MACMON_BIN`], overridable from config. The path is **never**
/// `$0`-derived (that is the serial wrapper's bug); every child must agree on the same
/// exported binary.
#[derive(Debug, Clone)]
pub struct MacmonProvider {
    /// Absolute path to the macmon binary.
    pub macmon_bin: PathBuf,
    /// Gate thresholds for this target (from `target.toml`).
    pub config: GateConfig,
}

impl MacmonProvider {
    /// Default macmon binary path (spec: `MACMON`, `docs/measure-job-contract.md` §3).
    /// Exported so every child agrees; overridable via [`MacmonProvider::new`]. Never
    /// `$0`-derive the binary.
    pub const DEFAULT_MACMON_BIN: &str = "/opt/bench-runner/bin/macmon";

    /// Build a provider with an explicit macmon binary path and gate config.
    pub fn new(macmon_bin: impl Into<PathBuf>, config: GateConfig) -> Self {
        Self {
            macmon_bin: macmon_bin.into(),
            config,
        }
    }

    /// Build a provider using [`MacmonProvider::DEFAULT_MACMON_BIN`].
    pub fn with_default_bin(config: GateConfig) -> Self {
        Self::new(Self::DEFAULT_MACMON_BIN, config)
    }
}

impl TelemetryProvider for MacmonProvider {
    /// Point sample via `macmon pipe -s1`.
    ///
    /// NOTE: this run-path is intentionally thin and is NOT unit-tested — macmon is not
    /// installed in this workspace. It shells out, checks the exit status, and hands the
    /// first non-empty stdout line to the tested [`MacmonSample::from_pipe_line`] parser.
    fn sample(&self) -> Result<TelemetrySample, TelemetryError> {
        let output = Command::new(&self.macmon_bin)
            .args(["pipe", "-s1"])
            .output()
            .map_err(TelemetryError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TelemetryError::Macmon(format!(
                "`{} pipe -s1` exited {}: {}",
                self.macmon_bin.display(),
                output.status,
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .ok_or_else(|| TelemetryError::Macmon("macmon pipe -s1 produced no output".into()))?;

        Ok(MacmonSample::from_pipe_line(line)?.into())
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            name: "macmon".to_string(),
            source: self.macmon_bin.display().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (hardware-free: fixture macmon JSON lines, no live macmon)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// Float compare with a small tolerance — parsed values are exact, but this keeps the
    /// assertions off `==` on floats.
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// A realistically-shaped macmon `pipe` line: the fields we read plus the many we
    /// ignore (`cpu_temp_avg`, `memory`, `ecpu_usage`, `pcpu_usage`, `*_power`).
    fn real_shaped_line() -> &'static str {
        r#"{
            "temp": {"cpu_temp_avg": 48.5, "gpu_temp_avg": 55.0},
            "memory": {"ram_total": 137438953472, "ram_usage": 42000000000},
            "ecpu_usage": [1400, 0.12],
            "pcpu_usage": [3200, 0.34],
            "gpu_usage": [1380.0, 0.82],
            "cpu_power": 5.5, "gpu_power": 22.1, "ane_power": 0.0,
            "all_power": 27.6, "sys_power": 31.0
        }"#
    }

    /// Build a sample directly (for gate tests) without going through JSON.
    fn s(temp_c: f64, freq_mhz: f64, util: f64) -> MacmonSample {
        MacmonSample {
            gpu_temp_c: temp_c,
            gpu_freq_mhz: freq_mhz,
            gpu_util: util,
        }
    }

    #[test]
    fn parses_real_shaped_pipe_line() {
        let sample = MacmonSample::from_pipe_line(real_shaped_line()).expect("should parse");
        assert!(
            approx(sample.gpu_temp_c, 55.0),
            "temp = {}",
            sample.gpu_temp_c
        );
        assert!(
            approx(sample.gpu_freq_mhz, 1380.0),
            "freq = {}",
            sample.gpu_freq_mhz
        );
        assert!(approx(sample.gpu_util, 0.82), "util = {}", sample.gpu_util);
    }

    #[test]
    fn parse_is_robust_to_unknown_top_level_keys() {
        // Minimal known keys + several keys this crate has never heard of.
        let line = r#"{
            "temp": {"gpu_temp_avg": 41.25},
            "gpu_usage": [1105, 0.71],
            "brand_new_macmon_field": {"nested": [1, 2, 3]},
            "another_unknown": "ignored",
            "throttle": false
        }"#;
        let sample =
            MacmonSample::from_pipe_line(line).expect("unknown keys must not break parsing");
        assert!(approx(sample.gpu_temp_c, 41.25));
        assert!(approx(sample.gpu_freq_mhz, 1105.0));
        assert!(approx(sample.gpu_util, 0.71));
    }

    #[test]
    fn parse_tolerates_extra_gpu_usage_elements() {
        // A future macmon that appends a third element must still parse.
        let line = r#"{"temp": {"gpu_temp_avg": 40.0}, "gpu_usage": [1200, 0.9, 999]}"#;
        let sample =
            MacmonSample::from_pipe_line(line).expect("extra array element must not break parsing");
        assert!(approx(sample.gpu_freq_mhz, 1200.0));
        assert!(approx(sample.gpu_util, 0.9));
    }

    #[test]
    fn parse_errors_on_short_gpu_usage() {
        let line = r#"{"temp": {"gpu_temp_avg": 40.0}, "gpu_usage": [1200]}"#;
        let err = MacmonSample::from_pipe_line(line).expect_err("missing util must error");
        assert!(matches!(err, TelemetryError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn parse_errors_on_garbage() {
        let err = MacmonSample::from_pipe_line("not json").expect_err("garbage must error");
        assert!(matches!(err, TelemetryError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn parse_pipe_stream_reads_jsonl() {
        let stream = concat!(
            r#"{"temp":{"gpu_temp_avg":40.0},"gpu_usage":[1100,0.71]}"#,
            "\n",
            "\n", // blank line skipped
            r#"{"temp":{"gpu_temp_avg":42.0},"gpu_usage":[1200,0.80]}"#,
            "\n",
        );
        let samples = parse_pipe_stream(stream).expect("stream should parse");
        assert_eq!(samples.len(), 2);
        assert!(approx(samples[1].gpu_freq_mhz, 1200.0));
    }

    #[test]
    fn cool_gate_boundary_is_inclusive() {
        let cfg = GateConfig::box3_qmtp_defaults(); // cool_gate_c = 40.0
        assert!(cool_gate_ok(39.9, &cfg), "below threshold passes");
        assert!(
            cool_gate_ok(40.0, &cfg),
            "exactly at threshold passes (at/below)"
        );
        assert!(!cool_gate_ok(40.1, &cfg), "above threshold fails");
    }

    /// The key test: `[idle, load(ramp), load, load, idle, load(ramp), load]` — the two
    /// ramp-first-of-stretch samples are excluded from the floor, the 3 steady ones kept.
    #[test]
    fn steady_loaded_excludes_ramp_first_of_each_stretch() {
        let cfg = GateConfig::box3_qmtp_defaults(); // loaded_util = 0.70
        let samples = [
            s(35.0, 500.0, 0.02),  // 0 idle
            s(45.0, 1300.0, 0.90), // 1 load  -> RAMP (prev idle) EXCLUDED
            s(46.0, 1300.0, 0.91), // 2 load  -> steady KEEP
            s(46.0, 1300.0, 0.92), // 3 load  -> steady KEEP
            s(37.0, 500.0, 0.03),  // 4 idle
            s(45.0, 1300.0, 0.90), // 5 load  -> RAMP (prev idle) EXCLUDED
            s(46.0, 1300.0, 0.93), // 6 load  -> steady KEEP
        ];
        let steady = steady_loaded_samples(&samples, &cfg);
        assert_eq!(
            steady.len(),
            3,
            "exactly the 3 non-ramp loaded samples count"
        );
        // The kept ones are indices 2, 3, 6 (utils 0.91, 0.92, 0.93).
        let utils: Vec<f64> = steady.iter().map(|s| s.gpu_util).collect();
        assert!(approx(utils[0], 0.91));
        assert!(approx(utils[1], 0.92));
        assert!(approx(utils[2], 0.93));
        // Raw sample count is untouched by ramp exclusion.
        assert_eq!(samples.len(), 7);
    }

    #[test]
    fn steady_loaded_empty_when_never_two_in_a_row() {
        let cfg = GateConfig::box3_qmtp_defaults();
        // Alternating load/idle: every loaded sample's predecessor is idle -> all ramps.
        let samples = [
            s(40.0, 1300.0, 0.90),
            s(40.0, 500.0, 0.05),
            s(40.0, 1300.0, 0.90),
            s(40.0, 500.0, 0.05),
        ];
        assert!(steady_loaded_samples(&samples, &cfg).is_empty());
    }

    #[test]
    fn clock_floor_fails_on_below_floor_steady_sample() {
        let cfg = GateConfig::box3_qmtp_defaults(); // floor 1100 MHz
        let samples = [
            s(40.0, 1300.0, 0.90), // 0 ramp (first) excluded
            s(40.0, 1000.0, 0.91), // 1 STEADY, below 1100 -> fails
            s(40.0, 1300.0, 0.92), // 2 steady, ok
        ];
        assert!(!clock_floor_ok(&samples, &cfg));
    }

    #[test]
    fn clock_floor_ignores_below_floor_ramp_sample() {
        let cfg = GateConfig::box3_qmtp_defaults(); // floor 1100 MHz
        let samples = [
            s(40.0, 500.0, 0.02),  // 0 idle
            s(40.0, 1000.0, 0.90), // 1 RAMP, below 1100 -> excluded, must NOT fail
            s(40.0, 1300.0, 0.91), // 2 steady, ok
            s(40.0, 1250.0, 0.92), // 3 steady, ok
        ];
        assert!(
            clock_floor_ok(&samples, &cfg),
            "a below-floor ramp sample is excluded and does not fail the gate"
        );
    }

    #[test]
    fn clock_floor_fails_closed_on_empty_steady_set() {
        let cfg = GateConfig::box3_qmtp_defaults();
        // No two loaded samples in a row -> empty steady set -> fail-closed.
        let samples = [s(40.0, 1300.0, 0.90), s(40.0, 500.0, 0.05)];
        assert!(steady_loaded_samples(&samples, &cfg).is_empty());
        assert!(
            !clock_floor_ok(&samples, &cfg),
            "empty steady-loaded set fails closed (no evidence the floor was held)"
        );
    }

    #[test]
    fn util_quiescent_boundary() {
        // Preflight ceiling is a distinct, stricter number than loaded_util (0.70).
        assert!(util_quiescent(0.05, 0.10));
        assert!(util_quiescent(0.10, 0.10), "at/below ceiling is quiescent");
        assert!(!util_quiescent(0.11, 0.10));
        assert!(!util_quiescent(0.70, 0.10), "a loaded GPU is not quiescent");
    }

    #[test]
    fn telemetry_sample_from_macmon_sample_drops_freq() {
        let sample = MacmonSample::from_pipe_line(real_shaped_line()).unwrap();
        let point: TelemetrySample = sample.into();
        assert!(approx(point.gpu_temp_c, 55.0));
        assert!(approx(point.gpu_util, 0.82));
    }

    #[test]
    fn gate_config_deserializes_from_toml_style_json_with_alias() {
        // Simulates target.toml [telemetry] parsed into JSON: `loaded_util` alias works.
        let cfg: GateConfig = serde_json::from_str(
            r#"{"cool_gate_c": 42.0, "clock_floor_mhz": 1500.0, "loaded_util": 0.9}"#,
        )
        .expect("should deserialize with loaded_util alias");
        assert!(approx(cfg.cool_gate_c, 42.0));
        assert!(approx(cfg.clock_floor_mhz, 1500.0));
        assert!(approx(cfg.gpu_loaded_util, 0.9));
    }

    #[test]
    fn macmon_provider_metadata_reports_configured_bin() {
        let p = MacmonProvider::new("/custom/path/macmon", GateConfig::box3_qmtp_defaults());
        let md = p.metadata();
        assert_eq!(md.name, "macmon");
        assert_eq!(md.source, "/custom/path/macmon");
        // Default-bin constructor uses the spec path, never $0.
        let d = MacmonProvider::with_default_bin(GateConfig::box3_qmtp_defaults());
        assert_eq!(d.macmon_bin.to_str(), Some("/opt/bench-runner/bin/macmon"));
    }
}
