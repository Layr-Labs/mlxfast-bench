//! Sealed `score.json` payload — a faithful port of the Swift
//! `ScorePayload` / `ScoreMetrics` (Sources/MLXFastCore/Score.swift).
//!
//! Field names, JSON keys, nesting, and null semantics match the Swift Codable
//! types so a benchd-written score parses/diffs against `benchmark.sh --local-iterate`
//! (the M1 / WS1-10 gate). Diagnostic real-valued fields are coarsened to
//! `PUBLIC_DIAGNOSTIC_SIGNIFICANT_FIGURES` (2) sig figs before writing, exactly like
//! Swift `withCoarsenedPublicDiagnostics`; ranking/floor fields stay precise.
//!
//! benchd is the SOLE writer of the score (no discard/reseal), and writes a
//! `.sha256` sidecar of the exact score bytes.

use bench_core::constants::PUBLIC_DIAGNOSTIC_SIGNIFICANT_FIGURES;
use serde::{Deserialize, Serialize};

/// Port of Swift `ScorePayload`: `{ score: Double?, passed: Bool, metrics: {...} }`.
///
/// `Deserialize` is derived (in addition to the sealed-write `Serialize`) so the A-3 overlay
/// (`overlay-timing`) can READ a sealed `gates-score.json` back into a typed `ScorePayload`,
/// validate it, and overlay the measured timing onto its metrics. Deserialization is additive —
/// it does not change the sealed bytes or the Swift-parity schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScorePayload {
    /// Finite score, or `null` on any failure (Swift encodes nil explicitly).
    pub score: Option<f64>,
    pub passed: bool,
    pub metrics: ScoreMetrics,
}

/// Port of Swift `ScoreMetrics`. JSON keys match the Swift `CodingKeys`.
///
/// The output is emitted sorted-key + pretty (see [`ScorePayload::to_sealed_json`]),
/// so struct declaration order does not affect the bytes; it is kept in Swift order
/// for auditability. The five `first_failing_*` / token fields and the top-level
/// `score` are the only nullable fields and are emitted as JSON `null` when absent
/// (no `skip_serializing_if`), matching Swift `encodeNil`.
///
/// `Default` lets the parity verdict tool enumerate the serde field names (a
/// `serde_json::to_value(ScoreMetrics::default())` object) so its bucket roster is checked
/// against the ACTUAL schema at `cargo test` time (§T1 exhaustiveness).
///
/// `Deserialize` + container `#[serde(default)]` let the A-3 overlay read a sealed
/// `gates-score.json` back into this type. `default` is fail-CLOSED for the overlay's validation:
/// a gates score that omits `passed_correctness` / `partial_result` deserializes them as `false`,
/// which the overlay's gate check REJECTS (it never fabricates a passing gate from an absent field).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ScoreMetrics {
    #[serde(rename = "peak_ram_gb")]
    pub peak_ram_gb: f64,
    #[serde(rename = "bandwidth_gb_per_token")]
    pub bandwidth_gb_per_token: f64,
    #[serde(rename = "decode_seconds_per_token")]
    pub decode_seconds_per_token: f64,
    #[serde(rename = "prefill_seconds_per_token")]
    pub prefill_seconds_per_token: f64,
    #[serde(rename = "baseline_decode_seconds_per_token")]
    pub baseline_decode_seconds_per_token: f64,
    #[serde(rename = "baseline_prefill_seconds_per_token")]
    pub baseline_prefill_seconds_per_token: f64,
    #[serde(rename = "decode_speedup")]
    pub decode_speedup: f64,
    #[serde(rename = "prefill_speedup")]
    pub prefill_speedup: f64,
    #[serde(rename = "decode_speedup_floor")]
    pub decode_speedup_floor: f64,
    #[serde(rename = "prefill_speedup_floor")]
    pub prefill_speedup_floor: f64,
    #[serde(rename = "passed_decode_speedup_floor")]
    pub passed_decode_speedup_floor: bool,
    #[serde(rename = "passed_prefill_speedup_floor")]
    pub passed_prefill_speedup_floor: bool,
    #[serde(rename = "benchmark_wall_seconds")]
    pub benchmark_wall_seconds: f64,
    #[serde(rename = "preflight_seconds")]
    pub preflight_seconds: f64,
    #[serde(rename = "correctness_seconds")]
    pub correctness_seconds: f64,
    #[serde(rename = "timed_benchmark_seconds")]
    pub timed_benchmark_seconds: f64,
    #[serde(rename = "gpqa_ttft_passed")]
    pub gpqa_ttft_passed: bool,
    #[serde(rename = "gpqa_ttft_pass_count")]
    pub gpqa_ttft_pass_count: i64,
    #[serde(rename = "gpqa_ttft_case_count")]
    pub gpqa_ttft_case_count: i64,
    #[serde(rename = "gpqa_ttft_seconds")]
    pub gpqa_ttft_seconds: f64,
    #[serde(rename = "gpqa_ttft_p50_seconds")]
    pub gpqa_ttft_p50_seconds: f64,
    #[serde(rename = "gpqa_ttft_max_seconds")]
    pub gpqa_ttft_max_seconds: f64,
    #[serde(rename = "gpqa_ttft_source")]
    pub gpqa_ttft_source: String,
    #[serde(rename = "semantic_gpqa_passed")]
    pub semantic_gpqa_passed: bool,
    #[serde(rename = "semantic_gpqa_pass_count")]
    pub semantic_gpqa_pass_count: i64,
    #[serde(rename = "semantic_gpqa_case_count")]
    pub semantic_gpqa_case_count: i64,
    #[serde(rename = "semantic_gpqa_model")]
    pub semantic_gpqa_model: String,
    #[serde(rename = "process_resident_memory_gb")]
    pub process_resident_memory_gb: f64,
    #[serde(rename = "passed_correctness")]
    pub passed_correctness: bool,
    #[serde(rename = "num_layers")]
    pub num_layers: i64,
    #[serde(rename = "checked_steps")]
    pub checked_steps: i64,
    #[serde(rename = "case_count")]
    pub case_count: i64,
    #[serde(rename = "expert_cache_hits")]
    pub expert_cache_hits: u64,
    #[serde(rename = "expert_cache_misses")]
    pub expert_cache_misses: u64,
    #[serde(rename = "expert_cache_evictions")]
    pub expert_cache_evictions: u64,
    #[serde(rename = "expert_bytes_read")]
    pub expert_bytes_read: u64,
    #[serde(rename = "expert_read_seconds")]
    pub expert_read_seconds: f64,
    #[serde(rename = "expert_peak_cached_tensors")]
    pub expert_peak_cached_tensors: u64,
    #[serde(rename = "expert_hit_rate")]
    pub expert_hit_rate: f64,
    #[serde(rename = "first_failing_layer")]
    pub first_failing_layer: Option<i64>,
    #[serde(rename = "first_failing_case")]
    pub first_failing_case: Option<String>,
    #[serde(rename = "first_failing_step")]
    pub first_failing_step: Option<i64>,
    #[serde(rename = "expected_token")]
    pub expected_token: Option<i64>,
    #[serde(rename = "actual_token")]
    pub actual_token: Option<i64>,
    #[serde(rename = "max_abs_diff")]
    pub max_abs_diff: f64,
    #[serde(rename = "golden_hash")]
    pub golden_hash: String,
    #[serde(rename = "bandwidth_source")]
    pub bandwidth_source: String,
    pub error: String,
    pub commit: String,
    pub timestamp: String,
    #[serde(rename = "harness_hash")]
    pub harness_hash: String,
    #[serde(rename = "weights_hash")]
    pub weights_hash: String,
    #[serde(rename = "weights_byte_count")]
    pub weights_byte_count: i64,
    #[serde(rename = "weights_file_count")]
    pub weights_file_count: i64,
    pub runtime: String,
    #[serde(rename = "partial_result")]
    pub partial_result: bool,
}

/// Port of Swift `roundedToSignificantFigures`: monotone sig-fig rounding via a
/// formatted round-trip so the result is the clean nearest double to the N-sig-fig
/// decimal. Non-finite / zero / non-positive `figures` pass through unchanged.
pub fn rounded_to_significant_figures(value: f64, figures: u32) -> f64 {
    if !value.is_finite() || value == 0.0 || figures == 0 {
        return value;
    }
    // printf `%.*g` keeps `figures` significant digits. `{:.*e}` with `figures-1`
    // fractional mantissa digits is the same significant-figure grid, and parsing
    // the scientific string yields the clean nearest double (drops float noise).
    let formatted = format!("{:.*e}", (figures - 1) as usize, value);
    formatted.parse::<f64>().unwrap_or(value)
}

impl ScoreMetrics {
    /// Port of Swift `withCoarsenedPublicDiagnostics`: round the diagnostic
    /// (non-ranking) real-valued fields to `figures` sig figs; leave the ranking /
    /// floor / int / bool / string fields untouched. Re-clamps the ordering pairs
    /// (wall >= timed, ttft_max >= p50) after rounding, as Swift does.
    pub fn with_coarsened_public_diagnostics(&self, figures: u32) -> ScoreMetrics {
        let r = |v: f64| rounded_to_significant_figures(v, figures);

        let rounded_timed = r(self.timed_benchmark_seconds);
        let rounded_wall = r(self.benchmark_wall_seconds).max(rounded_timed);
        let rounded_p50 = r(self.gpqa_ttft_p50_seconds);
        let rounded_ttft_max = r(self.gpqa_ttft_max_seconds).max(rounded_p50);

        ScoreMetrics {
            peak_ram_gb: r(self.peak_ram_gb),
            bandwidth_gb_per_token: r(self.bandwidth_gb_per_token),
            decode_seconds_per_token: self.decode_seconds_per_token,
            prefill_seconds_per_token: self.prefill_seconds_per_token,
            baseline_decode_seconds_per_token: self.baseline_decode_seconds_per_token,
            baseline_prefill_seconds_per_token: self.baseline_prefill_seconds_per_token,
            decode_speedup: self.decode_speedup,
            prefill_speedup: self.prefill_speedup,
            decode_speedup_floor: self.decode_speedup_floor,
            prefill_speedup_floor: self.prefill_speedup_floor,
            passed_decode_speedup_floor: self.passed_decode_speedup_floor,
            passed_prefill_speedup_floor: self.passed_prefill_speedup_floor,
            benchmark_wall_seconds: rounded_wall,
            preflight_seconds: r(self.preflight_seconds),
            correctness_seconds: r(self.correctness_seconds),
            timed_benchmark_seconds: rounded_timed,
            gpqa_ttft_passed: self.gpqa_ttft_passed,
            gpqa_ttft_pass_count: self.gpqa_ttft_pass_count,
            gpqa_ttft_case_count: self.gpqa_ttft_case_count,
            gpqa_ttft_seconds: r(self.gpqa_ttft_seconds),
            gpqa_ttft_p50_seconds: rounded_p50,
            gpqa_ttft_max_seconds: rounded_ttft_max,
            gpqa_ttft_source: self.gpqa_ttft_source.clone(),
            semantic_gpqa_passed: self.semantic_gpqa_passed,
            semantic_gpqa_pass_count: self.semantic_gpqa_pass_count,
            semantic_gpqa_case_count: self.semantic_gpqa_case_count,
            semantic_gpqa_model: self.semantic_gpqa_model.clone(),
            process_resident_memory_gb: r(self.process_resident_memory_gb),
            passed_correctness: self.passed_correctness,
            num_layers: self.num_layers,
            checked_steps: self.checked_steps,
            case_count: self.case_count,
            expert_cache_hits: self.expert_cache_hits,
            expert_cache_misses: self.expert_cache_misses,
            expert_cache_evictions: self.expert_cache_evictions,
            expert_bytes_read: self.expert_bytes_read,
            expert_read_seconds: r(self.expert_read_seconds),
            expert_peak_cached_tensors: self.expert_peak_cached_tensors,
            expert_hit_rate: r(self.expert_hit_rate),
            first_failing_layer: self.first_failing_layer,
            first_failing_case: self.first_failing_case.clone(),
            first_failing_step: self.first_failing_step,
            expected_token: self.expected_token,
            actual_token: self.actual_token,
            max_abs_diff: r(self.max_abs_diff),
            golden_hash: self.golden_hash.clone(),
            bandwidth_source: self.bandwidth_source.clone(),
            error: self.error.clone(),
            commit: self.commit.clone(),
            timestamp: self.timestamp.clone(),
            harness_hash: self.harness_hash.clone(),
            weights_hash: self.weights_hash.clone(),
            weights_byte_count: self.weights_byte_count,
            weights_file_count: self.weights_file_count,
            runtime: self.runtime.clone(),
            partial_result: self.partial_result,
        }
    }
}

impl ScorePayload {
    /// Serialize to the sealed JSON bytes: coarsen diagnostics, then encode
    /// pretty + sorted-key (serde_json's default `Map` is a `BTreeMap`, so routing
    /// through `to_value` sorts keys, matching Swift's `.sortedKeys`). No trailing
    /// newline (Swift `data.write` writes the encoder output verbatim).
    pub fn to_sealed_json(&self) -> Result<String, serde_json::Error> {
        let published = ScorePayload {
            score: self.score,
            passed: self.passed,
            metrics: self
                .metrics
                .with_coarsened_public_diagnostics(PUBLIC_DIAGNOSTIC_SIGNIFICANT_FIGURES),
        };
        let value = serde_json::to_value(&published)?;
        serde_json::to_string_pretty(&value)
    }
}

/// Lowercase-hex sha256 of `bytes` (for the `.sha256` sidecar).
///
/// #58: re-exported from [`bench_core::hash`] rather than reimplemented — benchctl and the
/// golden loader must agree byte-for-byte on what a digest of the same bytes is, so there is
/// exactly one implementation. Kept exposed here because `crate::score::sha256_hex` is the
/// name the sidecar/score writers already call.
pub use bench_core::hash::sha256_hex;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_sig_two_figures_matches_printf_g() {
        assert_eq!(rounded_to_significant_figures(18.0, 2), 18.0);
        assert_eq!(rounded_to_significant_figures(20.25, 2), 20.0);
        assert_eq!(rounded_to_significant_figures(0.384, 2), 0.38);
        // 0.0106 -> "1.1e-2" -> 0.011
        assert_eq!(rounded_to_significant_figures(0.0106, 2), 0.011);
    }

    #[test]
    fn round_sig_passthrough_edge_cases() {
        assert_eq!(rounded_to_significant_figures(0.0, 2), 0.0);
        assert!(rounded_to_significant_figures(f64::NAN, 2).is_nan());
        assert_eq!(rounded_to_significant_figures(5.0, 0), 5.0);
    }

    #[test]
    fn ranking_fields_are_not_coarsened() {
        let mut m = zero_metrics();
        m.decode_seconds_per_token = 0.1336139485703125;
        m.prefill_seconds_per_token = 0.010605031949609375;
        m.decode_speedup = 1.234567;
        m.peak_ram_gb = 20.25;
        let c = m.with_coarsened_public_diagnostics(2);
        // ranking fields untouched, diagnostics coarsened
        assert_eq!(c.decode_seconds_per_token, 0.1336139485703125);
        assert_eq!(c.prefill_seconds_per_token, 0.010605031949609375);
        assert_eq!(c.decode_speedup, 1.234567);
        assert_eq!(c.peak_ram_gb, 20.0);
    }

    #[test]
    fn coarsen_reclamps_wall_at_least_timed() {
        let mut m = zero_metrics();
        m.timed_benchmark_seconds = 0.049; // -> 0.049
        m.benchmark_wall_seconds = 0.051; // r -> 0.051, but must be >= r(timed)
        let c = m.with_coarsened_public_diagnostics(2);
        assert!(c.benchmark_wall_seconds >= c.timed_benchmark_seconds);
    }

    #[test]
    fn sealed_json_is_sorted_and_nested() {
        let payload = ScorePayload {
            score: Some(1.5),
            passed: true,
            metrics: zero_metrics(),
        };
        let json = payload.to_sealed_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("metrics").unwrap().is_object());
        assert_eq!(v.get("passed").unwrap(), &serde_json::json!(true));
        // sorted keys: top-level order is metrics, passed, score
        let top_keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        assert_eq!(top_keys, vec!["metrics", "passed", "score"]);
        // null nullable fields present, not omitted
        assert!(json.contains("\"first_failing_layer\": null"));
    }

    #[test]
    fn null_score_is_emitted() {
        let payload = ScorePayload {
            score: None,
            passed: false,
            metrics: zero_metrics(),
        };
        let json = payload.to_sealed_json().unwrap();
        assert!(json.contains("\"score\": null"));
    }

    /// A zeroed metrics block used across tests.
    pub(crate) fn zero_metrics() -> ScoreMetrics {
        ScoreMetrics {
            peak_ram_gb: 0.0,
            bandwidth_gb_per_token: 0.0,
            decode_seconds_per_token: 0.0,
            prefill_seconds_per_token: 0.0,
            baseline_decode_seconds_per_token: 0.0,
            baseline_prefill_seconds_per_token: 0.0,
            decode_speedup: 0.0,
            prefill_speedup: 0.0,
            decode_speedup_floor: 0.0,
            prefill_speedup_floor: 0.0,
            passed_decode_speedup_floor: false,
            passed_prefill_speedup_floor: false,
            benchmark_wall_seconds: 0.0,
            preflight_seconds: 0.0,
            correctness_seconds: 0.0,
            timed_benchmark_seconds: 0.0,
            gpqa_ttft_passed: false,
            gpqa_ttft_pass_count: 0,
            gpqa_ttft_case_count: 0,
            gpqa_ttft_seconds: 0.0,
            gpqa_ttft_p50_seconds: 0.0,
            gpqa_ttft_max_seconds: 0.0,
            gpqa_ttft_source: String::new(),
            semantic_gpqa_passed: false,
            semantic_gpqa_pass_count: 0,
            semantic_gpqa_case_count: 0,
            semantic_gpqa_model: String::new(),
            process_resident_memory_gb: 0.0,
            passed_correctness: false,
            num_layers: 0,
            checked_steps: 0,
            case_count: 0,
            expert_cache_hits: 0,
            expert_cache_misses: 0,
            expert_cache_evictions: 0,
            expert_bytes_read: 0,
            expert_read_seconds: 0.0,
            expert_peak_cached_tensors: 0,
            expert_hit_rate: 0.0,
            first_failing_layer: None,
            first_failing_case: None,
            first_failing_step: None,
            expected_token: None,
            actual_token: None,
            max_abs_diff: 0.0,
            golden_hash: String::new(),
            bandwidth_source: String::new(),
            error: String::new(),
            commit: String::new(),
            timestamp: String::new(),
            harness_hash: String::new(),
            weights_hash: String::new(),
            weights_byte_count: 0,
            weights_file_count: 0,
            runtime: String::new(),
            partial_result: false,
        }
    }
}
