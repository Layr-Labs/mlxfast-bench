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
    /// ADDITIVE, benchd-only — the per-timed-prompt records the challenge BOARD reads for its MTP
    /// column and its per-prompt decode readout (see [`ScorePerPrompt`]). One entry per timed prompt
    /// the run actually measured; EMPTY on every path that measured none.
    ///
    /// OMITTED FROM THE JSON WHEN EMPTY (`skip_serializing_if`), so no existing score.json key moves
    /// and no path that does not populate it changes by a byte. That is also why it carries no
    /// `parity.rs` ROSTER bucket: the roster is the SWIFT-PARITY surface, the reference emits no
    /// such key, and a rostered key that is absent on both sides would hard-fail the differ as
    /// SCHEMA-DRIFT-MISSING. `per_prompt_is_the_only_unrostered_metric` pins that this stays the
    /// ONE additive exception.
    #[serde(rename = "per_prompt", skip_serializing_if = "Vec::is_empty")]
    pub per_prompt: Vec<ScorePerPrompt>,
    /// ADDITIVE, benchd-only — THE SPECULATIVE-DECODE SEAL. The mode the engine ECHOED as
    /// `effective_spec` on the timed `free_decode_begin` (`"serial"` / `"mtp"`), already validated
    /// EQUAL to what benchd requested (spec-never-ignored, `bench_protocol::spec_echo_honors_request`).
    /// It answers "what did the scored leg actually run?" from the engine's own words, not from the
    /// operator's intent.
    ///
    /// ABSENT (never `null`) on a leg that carried no spec — today's serial default — so a serial
    /// run's sealed bytes are unchanged. Like `per_prompt` this is UNROSTERED: the Swift reference
    /// emits no such key.
    #[serde(
        rename = "effective_spec_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub effective_spec_mode: Option<String>,
    /// ADDITIVE — the DEPTH inside that echoed spec: the engine-resolved `mtp.depth`, or `0` for
    /// `serial` (serial has no drafter, so zero is its true depth, not a placeholder). Absent
    /// exactly when [`effective_spec_mode`](Self::effective_spec_mode) is absent.
    #[serde(
        rename = "effective_spec_depth",
        skip_serializing_if = "Option::is_none"
    )]
    pub effective_spec_depth: Option<i64>,
    /// ADDITIVE — R, the number of verify rounds the timed free-run window ran
    /// (`acceptance_lengths.len()`). Externally anchored: the phase-close `completed_work` counter
    /// must equal R+1 or the leg is refused. AUDIT-only, never scored.
    #[serde(rename = "spec_rounds", skip_serializing_if = "Option::is_none")]
    pub spec_rounds: Option<u64>,
    /// ADDITIVE — total draft tokens the engine PROPOSED across the window (self-reported).
    /// AUDIT-only, never scored.
    #[serde(rename = "spec_drafted_total", skip_serializing_if = "Option::is_none")]
    pub spec_drafted_total: Option<u64>,
    /// ADDITIVE — total draft tokens the target ACCEPTED across the window (self-reported).
    /// AUDIT-only, never scored.
    #[serde(
        rename = "spec_accepted_total",
        skip_serializing_if = "Option::is_none"
    )]
    pub spec_accepted_total: Option<u64>,
    /// ADDITIVE — `spec_accepted_total / spec_drafted_total`. ABSENT when nothing was drafted (a
    /// serial leg, or an mtp leg whose drafter proposed nothing): a zero denominator has no rate,
    /// and `0.0` would read as "drafted plenty, accepted none". AUDIT-only, never scored.
    #[serde(
        rename = "spec_acceptance_rate",
        skip_serializing_if = "Option::is_none"
    )]
    pub spec_acceptance_rate: Option<f64>,
    /// ADDITIVE — the engine's `verify_replay_disagreements`: how many verify rounds of the timed
    /// window had the BATCHED verify forward and the ONE-ROW replay of the same position choose a
    /// DIFFERENT argmax. The replay is the token that was committed; the divergence is a property
    /// of a tower that is not batch-invariant, so it is COUNTED, never a refusal.
    ///
    /// ABSENT ⇒ NOT REPORTED, never `0`. An engine that does not put the counter on the wire seals
    /// no key here, and "the engine measured none" stays distinguishable from "the engine cannot
    /// say". Bounded at audit-construction time by the rejected drafts
    /// (`spec_drafted_total - spec_accepted_total`), so an impossible count refuses the leg instead
    /// of reaching these bytes. AUDIT-only, never scored.
    #[serde(
        rename = "spec_verify_replay_disagreements",
        skip_serializing_if = "Option::is_none"
    )]
    pub spec_verify_replay_disagreements: Option<u64>,
    /// ADDITIVE — the engine's self-reported VERIFY PATH for the timed window: `"rectangular"`
    /// (one target forward over the 1+k candidate window, recurrent state captured per position
    /// and rolled back to the accepted one) or `"serial"` (one forward per candidate token, the
    /// fallback oracle). Present only when the engine reported it. AUDIT-only, never scored — it
    /// exists so a sealed decode number states which path produced it.
    #[serde(rename = "spec_verification_mode", skip_serializing_if = "Option::is_none")]
    pub spec_verification_mode: Option<String>,
    /// ADDITIVE — verify rounds that ran the rectangular path (self-reported). AUDIT-only.
    #[serde(
        rename = "spec_rectangular_verification_rounds",
        skip_serializing_if = "Option::is_none"
    )]
    pub spec_rectangular_verification_rounds: Option<u64>,
    /// ADDITIVE — verify rounds that fell back to the serial oracle (self-reported). AUDIT-only.
    #[serde(
        rename = "spec_serial_verification_rounds",
        skip_serializing_if = "Option::is_none"
    )]
    pub spec_serial_verification_rounds: Option<u64>,
    /// ADDITIVE — the VERBATIM per-round `acceptance_lengths[]` histogram of the timed window
    /// (RULED OQ4: the raw array, not just the aggregates). One entry per verify round, so a
    /// 128-token window seals at most 128 entries — no cap is needed. EMPTY (and therefore omitted)
    /// on a leg with no free-run audit. AUDIT-only, never scored.
    #[serde(rename = "acceptance_lengths", skip_serializing_if = "Vec::is_empty")]
    pub acceptance_lengths: Vec<u32>,
    /// ADDITIVE — THE ENGINE IDENTITY SEAL, taken from the TIMED worker's `hello` (the worker whose
    /// leg is scored). `hello.backend` VERBATIM: the engine's own self-description, e.g. a ds4
    /// build string carrying its pin, overlay, nvcc and driver. Absent when no timed worker ran or
    /// the engine sent none. AUDIT-only, never scored.
    #[serde(rename = "engine_backend", skip_serializing_if = "Option::is_none")]
    pub engine_backend: Option<String>,
    /// ADDITIVE — the timed worker's `hello.device` VERBATIM (e.g. `"cuda sm_121"`). AUDIT-only.
    #[serde(rename = "engine_device", skip_serializing_if = "Option::is_none")]
    pub engine_device: Option<String>,
    /// ADDITIVE — the timed worker's `hello.protocol_version`. The session handshake already
    /// refuses a version benchd does not speak; this records WHICH one answered. AUDIT-only.
    #[serde(
        rename = "engine_protocol_version",
        skip_serializing_if = "Option::is_none"
    )]
    pub engine_protocol_version: Option<u32>,
    /// ADDITIVE — the timed worker's loaded-head digest (`hello.head_provenance.sha256`, #106).
    /// The board's custom-head reader looks for it here and on each `per_prompt` entry. Absent for
    /// an engine that echoes no head provenance. AUDIT-only, never scored.
    #[serde(
        rename = "head_provenance_sha256",
        skip_serializing_if = "Option::is_none"
    )]
    pub head_provenance_sha256: Option<String>,
    /// ADDITIVE — the timed worker's runner id (`hello.runner.id`, e.g. `"layr/qwen4exp-125b-a6b"`).
    /// Absent for a worker that echoes no runner identity. IDENTITY, never an input to the score.
    #[serde(rename = "runner_id", skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    /// ADDITIVE — the `config.json` model type the timed worker loaded (`hello.runner.model_type`).
    /// IDENTITY, never an input to the score.
    #[serde(rename = "runner_model_type", skip_serializing_if = "Option::is_none")]
    pub runner_model_type: Option<String>,
    /// ADDITIVE — the digest of the timed worker's CANONICAL runner manifest
    /// (`hello.runner.manifest_sha256`, 64 lowercase hex). IDENTITY, never an input to the score;
    /// the conformance kit, not the scorer, is what compares it against a declared manifest.
    #[serde(
        rename = "runner_manifest_sha256",
        skip_serializing_if = "Option::is_none"
    )]
    pub runner_manifest_sha256: Option<String>,
    /// ADDITIVE — the worker build the runner identity was cut from (`hello.runner.build`).
    /// IDENTITY, never an input to the score.
    #[serde(rename = "runner_build", skip_serializing_if = "Option::is_none")]
    pub runner_build: Option<String>,
    /// ADDITIVE — the process id of the RESIDENT process the timed worker ATTACHED to
    /// (`hello.resident.pid`). Absent for a worker that loaded the weights itself. IDENTITY, never
    /// an input to the score.
    #[serde(rename = "resident_pid", skip_serializing_if = "Option::is_none")]
    pub resident_pid: Option<u32>,
    /// ADDITIVE — the resident process's load stamp (`hello.resident.load_epoch`), taken when its
    /// load ended and constant for the life of that process. Every phase of one window seals the
    /// SAME value; the official path REFUSES a window whose phases report different resident
    /// identities, because that is a reload inside the window (weights-load-once). IDENTITY, never
    /// an input to the score.
    #[serde(
        rename = "resident_load_epoch",
        skip_serializing_if = "Option::is_none"
    )]
    pub resident_load_epoch: Option<u64>,
    /// ADDITIVE — THE PAIRED-BASELINE SEAL (David 2026-09-08). WHERE the denominator came from:
    /// `"serial-control-leg"` says it was MEASURED, on this box, in this job, on the reference
    /// tree, immediately before the candidate leg. Absent on every path that did not measure a
    /// control leg, so an absent key is not a claim about one.
    #[serde(rename = "baseline_source", skip_serializing_if = "Option::is_none")]
    pub baseline_source: Option<String>,
    /// ADDITIVE — the ranked BOX the paired run measured both legs on (the runner name the
    /// calibration file names). IDENTITY, never an input to the score.
    #[serde(rename = "baseline_box", skip_serializing_if = "Option::is_none")]
    pub baseline_box: Option<String>,
    /// ADDITIVE — the digest of the per-box calibration FILE this run checked its control leg
    /// against. The file is a health band, never a denominator; the digest states which band.
    #[serde(
        rename = "baseline_calibration_sha256",
        skip_serializing_if = "Option::is_none"
    )]
    pub baseline_calibration_sha256: Option<String>,
    /// ADDITIVE — the digest of the GOLDEN the serial-control leg verified its decode tokens
    /// against. The control leg is serial, so on a track that carries per-depth oracle tapes it
    /// reads a different golden than the candidate leg (`--control-golden`); this states which one.
    #[serde(
        rename = "baseline_golden_sha256",
        skip_serializing_if = "Option::is_none"
    )]
    pub baseline_golden_sha256: Option<String>,
    /// ADDITIVE — the reference tree's engine commit the calibration was captured at.
    #[serde(
        rename = "baseline_reference_commit",
        skip_serializing_if = "Option::is_none"
    )]
    pub baseline_reference_commit: Option<String>,
    /// ADDITIVE — whether the measured control leg sat inside this box's band. It is `true`
    /// wherever it is present: a leg outside the band seals no score at all, so `false` never
    /// reaches a sealed artifact. It is sealed so a reader can see the gate ran.
    #[serde(
        rename = "baseline_band_passed",
        skip_serializing_if = "Option::is_none"
    )]
    pub baseline_band_passed: Option<bool>,
    /// ADDITIVE — the SERIAL-CONTROL leg's measured prefill seconds-per-token. The same value
    /// [`ScoreMetrics::baseline_prefill_seconds_per_token`] carries, named for what it is.
    #[serde(
        rename = "baseline_leg_prefill_seconds_per_token",
        skip_serializing_if = "Option::is_none"
    )]
    pub baseline_leg_prefill_seconds_per_token: Option<f64>,
    /// ADDITIVE — the SERIAL-CONTROL leg's measured decode seconds-per-token.
    #[serde(
        rename = "baseline_leg_decode_seconds_per_token",
        skip_serializing_if = "Option::is_none"
    )]
    pub baseline_leg_decode_seconds_per_token: Option<f64>,
    /// ADDITIVE — the CANDIDATE leg's measured prefill seconds-per-token, READ BACK from
    /// [`ScoreMetrics::prefill_seconds_per_token`] so the two cannot drift. Absent when the
    /// candidate leg produced no timing.
    #[serde(
        rename = "candidate_leg_prefill_seconds_per_token",
        skip_serializing_if = "Option::is_none"
    )]
    pub candidate_leg_prefill_seconds_per_token: Option<f64>,
    /// ADDITIVE — the CANDIDATE leg's measured decode seconds-per-token, READ BACK from
    /// [`ScoreMetrics::decode_seconds_per_token`].
    #[serde(
        rename = "candidate_leg_decode_seconds_per_token",
        skip_serializing_if = "Option::is_none"
    )]
    pub candidate_leg_decode_seconds_per_token: Option<f64>,
}

/// One timed prompt's board-facing record, sealed in [`ScoreMetrics::per_prompt`].
///
/// THE READER IS THE BOARD, and it reads every field INDEPENDENTLY and OPTIONALLY
/// (yukon `apps/challenges-ui/shared/lib/throughput-metrics.ts`, PR #622; `mlxfast/lib/format.ts`
/// on master): the MTP column shows the MEAN of `effective_mean_draft_len` over the entries and a
/// dash when the array is absent or empty; the decode readout uses `mtp_seconds_per_token_mean`,
/// skipping any entry whose value is missing or <= 0.
///
/// NOT [`crate::measure_job::PerPrompt`]. That type is the PAIRED flow's results.json record: its
/// `parity_ok`, `accepted_pair_count`, `serial_seconds_per_token_mean` and `raw_ratio_of_means` are
/// all mandatory and all describe a SERIAL-vs-CANDIDATE pair, which a single-leg run does not have.
/// Reusing it here would mean inventing values for the pair half. This type carries only the fields
/// the board reads, under the identical JSON key names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ScorePerPrompt {
    /// The timed prompt's identity, BOUND BY BYTES: the sha256 of the golden this run timed
    /// ([`bench_core::golden::GoldenFixture::sha256`]) — the same `golden_hash` this score already
    /// seals, and the same rule the paired flow applies for its own records
    /// (`measure_job.rs`, "the prompt IDENTITY is the sha256 of THIS golden's bytes").
    pub prompt_sha256: String,
    /// The mean number of committed tokens per verify round, exactly as the free-run audit computes
    /// it ([`bench_core::free_run::FreeRunAudit::effective_mean_draft_len`]). `0` is a REAL measured
    /// value, never a placeholder. AUDIT-ONLY — never a scoring input.
    pub effective_mean_draft_len: f64,
    /// This prompt's ENFORCED whole-window decode seconds-per-token — the SAME number
    /// [`ScoreMetrics::decode_seconds_per_token`] carries. On a single timed prompt the two are
    /// equal by construction. It is deliberately NOT a decode-only figure: see the RED-TEAM REVERT
    /// notes in `bench-runner/src/timing.rs`, which exist because an earlier revision redefined this
    /// quantity to exclude the seed forward.
    pub mtp_seconds_per_token_mean: f64,
    /// ADDITIVE — this prompt's verify-round count R. Absent on a leg with no free-run audit, so an
    /// entry that has none seals the historical three keys exactly.
    #[serde(rename = "spec_rounds", skip_serializing_if = "Option::is_none")]
    pub spec_rounds: Option<u64>,
    /// ADDITIVE — this prompt's total PROPOSED draft tokens (self-reported). AUDIT-only.
    #[serde(rename = "spec_drafted_total", skip_serializing_if = "Option::is_none")]
    pub spec_drafted_total: Option<u64>,
    /// ADDITIVE — this prompt's total ACCEPTED draft tokens (self-reported). AUDIT-only.
    #[serde(
        rename = "spec_accepted_total",
        skip_serializing_if = "Option::is_none"
    )]
    pub spec_accepted_total: Option<u64>,
    /// ADDITIVE — the timed worker's loaded-head digest, MIRRORED here because the board's
    /// custom-head reader reads it off the per-prompt entry. Same value as
    /// [`ScoreMetrics::head_provenance_sha256`]. AUDIT-only.
    #[serde(
        rename = "head_provenance_sha256",
        skip_serializing_if = "Option::is_none"
    )]
    pub head_provenance_sha256: Option<String>,
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
            // Carried VERBATIM: `mtp_seconds_per_token_mean` must stay byte-equal to
            // `decode_seconds_per_token`, which is a ranking field and is not coarsened either.
            per_prompt: self.per_prompt.clone(),
            // The SPEC and ENGINE-IDENTITY seals are carried VERBATIM. They are counts, an echoed
            // mode/depth, a ratio and identity strings — facts about what ran, not diagnostic
            // real-valued measurements, so coarsening them would only lose information.
            effective_spec_mode: self.effective_spec_mode.clone(),
            effective_spec_depth: self.effective_spec_depth,
            spec_rounds: self.spec_rounds,
            spec_drafted_total: self.spec_drafted_total,
            spec_accepted_total: self.spec_accepted_total,
            spec_acceptance_rate: self.spec_acceptance_rate,
            spec_verify_replay_disagreements: self.spec_verify_replay_disagreements,
            spec_verification_mode: self.spec_verification_mode.clone(),
            spec_rectangular_verification_rounds: self.spec_rectangular_verification_rounds,
            spec_serial_verification_rounds: self.spec_serial_verification_rounds,
            acceptance_lengths: self.acceptance_lengths.clone(),
            engine_backend: self.engine_backend.clone(),
            engine_device: self.engine_device.clone(),
            engine_protocol_version: self.engine_protocol_version,
            head_provenance_sha256: self.head_provenance_sha256.clone(),
            runner_id: self.runner_id.clone(),
            runner_model_type: self.runner_model_type.clone(),
            runner_manifest_sha256: self.runner_manifest_sha256.clone(),
            runner_build: self.runner_build.clone(),
            resident_pid: self.resident_pid,
            resident_load_epoch: self.resident_load_epoch,
            // The PAIRED-BASELINE seal is carried VERBATIM. Its two leg pairs mirror the ranking
            // fields `baseline_*_seconds_per_token` / `*_seconds_per_token`, which are not
            // coarsened either, and the rest is identity.
            baseline_source: self.baseline_source.clone(),
            baseline_box: self.baseline_box.clone(),
            baseline_calibration_sha256: self.baseline_calibration_sha256.clone(),
            baseline_golden_sha256: self.baseline_golden_sha256.clone(),
            baseline_reference_commit: self.baseline_reference_commit.clone(),
            baseline_band_passed: self.baseline_band_passed,
            baseline_leg_prefill_seconds_per_token: self.baseline_leg_prefill_seconds_per_token,
            baseline_leg_decode_seconds_per_token: self.baseline_leg_decode_seconds_per_token,
            candidate_leg_prefill_seconds_per_token: self.candidate_leg_prefill_seconds_per_token,
            candidate_leg_decode_seconds_per_token: self.candidate_leg_decode_seconds_per_token,
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
/// #58: re-exported from [`bench_core::hash`] rather than reimplemented — benchd and the
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
            per_prompt: Vec::new(),
            effective_spec_mode: None,
            effective_spec_depth: None,
            spec_rounds: None,
            spec_drafted_total: None,
            spec_accepted_total: None,
            spec_acceptance_rate: None,
            spec_verify_replay_disagreements: None,
            spec_verification_mode: None,
            spec_rectangular_verification_rounds: None,
            spec_serial_verification_rounds: None,
            acceptance_lengths: Vec::new(),
            engine_backend: None,
            engine_device: None,
            engine_protocol_version: None,
            head_provenance_sha256: None,
            runner_id: None,
            runner_model_type: None,
            runner_manifest_sha256: None,
            runner_build: None,
            resident_pid: None,
            resident_load_epoch: None,
            baseline_source: None,
            baseline_box: None,
            baseline_calibration_sha256: None,
            baseline_golden_sha256: None,
            baseline_reference_commit: None,
            baseline_band_passed: None,
            baseline_leg_prefill_seconds_per_token: None,
            baseline_leg_decode_seconds_per_token: None,
            candidate_leg_prefill_seconds_per_token: None,
            candidate_leg_decode_seconds_per_token: None,
        }
    }

    // ---------------------------------------------------------------------------------------
    // `metrics.per_prompt` — the ADDITIVE board array (see [`ScorePerPrompt`]).
    // ---------------------------------------------------------------------------------------

    /// The 56 `metrics.*` keys the sealed score carried BEFORE `per_prompt` was added. Pinned
    /// VERBATIM (not derived from the struct) so this is a real before/after snapshot: any future
    /// edit that renames, drops or reorders an existing key fails here.
    const METRICS_KEYS_BEFORE_PER_PROMPT: &[&str] = &[
        "actual_token",
        "bandwidth_gb_per_token",
        "bandwidth_source",
        "baseline_decode_seconds_per_token",
        "baseline_prefill_seconds_per_token",
        "benchmark_wall_seconds",
        "case_count",
        "checked_steps",
        "commit",
        "correctness_seconds",
        "decode_seconds_per_token",
        "decode_speedup",
        "decode_speedup_floor",
        "error",
        "expected_token",
        "expert_bytes_read",
        "expert_cache_evictions",
        "expert_cache_hits",
        "expert_cache_misses",
        "expert_hit_rate",
        "expert_peak_cached_tensors",
        "expert_read_seconds",
        "first_failing_case",
        "first_failing_layer",
        "first_failing_step",
        "golden_hash",
        "gpqa_ttft_case_count",
        "gpqa_ttft_max_seconds",
        "gpqa_ttft_p50_seconds",
        "gpqa_ttft_pass_count",
        "gpqa_ttft_passed",
        "gpqa_ttft_seconds",
        "gpqa_ttft_source",
        "harness_hash",
        "max_abs_diff",
        "num_layers",
        "partial_result",
        "passed_correctness",
        "passed_decode_speedup_floor",
        "passed_prefill_speedup_floor",
        "peak_ram_gb",
        "preflight_seconds",
        "prefill_seconds_per_token",
        "prefill_speedup",
        "prefill_speedup_floor",
        "process_resident_memory_gb",
        "semantic_gpqa_case_count",
        "semantic_gpqa_model",
        "semantic_gpqa_pass_count",
        "semantic_gpqa_passed",
        "timed_benchmark_seconds",
        "timestamp",
        "weights_byte_count",
        "weights_file_count",
        "weights_hash",
        "runtime",
    ];

    fn sealed_metrics_keys(metrics: ScoreMetrics) -> std::collections::BTreeSet<String> {
        let json = ScorePayload {
            score: Some(1.0),
            passed: true,
            metrics,
        }
        .to_sealed_json()
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["metrics"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    }

    #[test]
    fn per_prompt_is_purely_additive_and_moves_no_existing_key() {
        let before: std::collections::BTreeSet<String> = METRICS_KEYS_BEFORE_PER_PROMPT
            .iter()
            .map(|k| (*k).to_string())
            .collect();

        // A score that measured no timed prompt is byte-for-byte the OLD key set: an empty
        // `per_prompt` is omitted from the JSON entirely.
        assert_eq!(
            sealed_metrics_keys(zero_metrics()),
            before,
            "an unpopulated per_prompt must not add a key"
        );

        // A score that DID measure one adds exactly `per_prompt` — nothing else moves.
        let mut populated = zero_metrics();
        populated.per_prompt = vec![ScorePerPrompt {
            prompt_sha256: "ab".repeat(32),
            effective_mean_draft_len: 4.0,
            mtp_seconds_per_token_mean: 0.125,
            ..Default::default()
        }];
        let after = sealed_metrics_keys(populated);
        let added: Vec<_> = after.difference(&before).collect();
        let removed: Vec<_> = before.difference(&after).collect();
        assert_eq!(added, vec!["per_prompt"], "one added key, and only one");
        assert!(removed.is_empty(), "existing keys removed: {removed:?}");
    }

    /// The ONE sanctioned ADDITIVE key set — `per_prompt` plus the speculative-decode seal and the
    /// engine identity (backend/device/protocol version, the loaded-head digest, the runner
    /// identity, and the resident-process identity). Every entry is omitted-when-unset, so a run that does not produce it seals
    /// byte-identically to before it existed.
    const ADDITIVE_METRICS_KEYS: &[&str] = &[
        "acceptance_lengths",
        "effective_spec_depth",
        "effective_spec_mode",
        "engine_backend",
        "engine_device",
        "engine_protocol_version",
        "head_provenance_sha256",
        "per_prompt",
        "resident_load_epoch",
        "resident_pid",
        "runner_build",
        "runner_id",
        "runner_manifest_sha256",
        "runner_model_type",
        "spec_acceptance_rate",
        "spec_accepted_total",
        "spec_drafted_total",
        "spec_rectangular_verification_rounds",
        "spec_rounds",
        "spec_serial_verification_rounds",
        "spec_verification_mode",
        "spec_verify_replay_disagreements",
    ];

    /// KEY-SET SNAPSHOT. The 56 pre-existing keys are UNCHANGED — none renamed, dropped or
    /// reordered — and a FULLY populated metrics block adds exactly the sanctioned additive set and
    /// nothing else.
    #[test]
    fn the_spec_and_identity_seals_are_purely_additive() {
        let before: std::collections::BTreeSet<String> = METRICS_KEYS_BEFORE_PER_PROMPT
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        assert_eq!(
            before.len(),
            56,
            "the pinned pre-existing key set is 56 keys"
        );

        // Nothing populated: byte-for-byte the historical key set.
        assert_eq!(sealed_metrics_keys(zero_metrics()), before);

        let populated = ScoreMetrics {
            per_prompt: vec![ScorePerPrompt {
                prompt_sha256: "ab".repeat(32),
                effective_mean_draft_len: 2.0,
                mtp_seconds_per_token_mean: 0.125,
                spec_rounds: Some(64),
                spec_drafted_total: Some(64),
                spec_accepted_total: Some(32),
                head_provenance_sha256: Some("cd".repeat(32)),
            }],
            effective_spec_mode: Some("mtp".to_string()),
            effective_spec_depth: Some(1),
            spec_rounds: Some(64),
            spec_drafted_total: Some(64),
            spec_accepted_total: Some(32),
            spec_acceptance_rate: Some(0.5),
            spec_verify_replay_disagreements: Some(7),
            spec_verification_mode: Some("rectangular".to_string()),
            spec_rectangular_verification_rounds: Some(7),
            spec_serial_verification_rounds: Some(0),
            acceptance_lengths: vec![2; 64],
            engine_backend: Some("ds4-dfm-rs@abc".to_string()),
            engine_device: Some("cuda sm_121".to_string()),
            engine_protocol_version: Some(1),
            head_provenance_sha256: Some("cd".repeat(32)),
            runner_id: Some("layr/qwen4exp-125b-a6b".to_string()),
            runner_model_type: Some("qwen4_exp".to_string()),
            runner_manifest_sha256: Some("ef".repeat(32)),
            runner_build: Some("c4089870".to_string()),
            resident_pid: Some(4242),
            resident_load_epoch: Some(1_756_944_000),
            ..zero_metrics()
        };
        let after = sealed_metrics_keys(populated);
        let added: Vec<String> = after.difference(&before).cloned().collect();
        let removed: Vec<String> = before.difference(&after).cloned().collect();
        assert_eq!(added, ADDITIVE_METRICS_KEYS, "the additive set, exactly");
        assert!(removed.is_empty(), "existing keys removed: {removed:?}");
    }

    /// The per-prompt entry keeps its historical THREE keys when the run drafted nothing and the
    /// engine announced no head, and grows only the four additive ones when it did.
    #[test]
    fn per_prompt_additive_keys_are_omitted_when_unset() {
        let entry_keys = |pp: ScorePerPrompt| -> Vec<String> {
            let json = ScorePayload {
                score: Some(1.0),
                passed: true,
                metrics: ScoreMetrics {
                    per_prompt: vec![pp],
                    ..zero_metrics()
                },
            }
            .to_sealed_json()
            .unwrap();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            v["metrics"]["per_prompt"][0]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };

        assert_eq!(
            entry_keys(ScorePerPrompt {
                prompt_sha256: "ab".repeat(32),
                effective_mean_draft_len: 1.0,
                mtp_seconds_per_token_mean: 0.125,
                ..Default::default()
            }),
            vec![
                "effective_mean_draft_len",
                "mtp_seconds_per_token_mean",
                "prompt_sha256"
            ]
        );
        assert_eq!(
            entry_keys(ScorePerPrompt {
                prompt_sha256: "ab".repeat(32),
                effective_mean_draft_len: 2.0,
                mtp_seconds_per_token_mean: 0.125,
                spec_rounds: Some(64),
                spec_drafted_total: Some(64),
                spec_accepted_total: Some(32),
                head_provenance_sha256: Some("cd".repeat(32)),
            }),
            vec![
                "effective_mean_draft_len",
                "head_provenance_sha256",
                "mtp_seconds_per_token_mean",
                "prompt_sha256",
                "spec_accepted_total",
                "spec_drafted_total",
                "spec_rounds",
            ]
        );
    }

    #[test]
    fn per_prompt_json_keys_round_trip_under_the_names_the_board_reads() {
        let metrics = ScoreMetrics {
            decode_seconds_per_token: 0.125,
            per_prompt: vec![ScorePerPrompt {
                prompt_sha256: "cd".repeat(32),
                effective_mean_draft_len: 0.0,
                mtp_seconds_per_token_mean: 0.125,
                ..Default::default()
            }],
            ..zero_metrics()
        };
        let json = ScorePayload {
            score: Some(1.0),
            passed: true,
            metrics: metrics.clone(),
        }
        .to_sealed_json()
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        // The reader's exact key names (yukon throughput-metrics.ts): the array is under
        // `metrics.per_prompt`, and each entry carries these three, no more.
        let entry = &v["metrics"]["per_prompt"][0];
        let keys: Vec<&str> = entry
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "effective_mean_draft_len",
                "mtp_seconds_per_token_mean",
                "prompt_sha256",
            ]
        );
        assert_eq!(entry["prompt_sha256"], serde_json::json!("cd".repeat(32)));
        assert_eq!(entry["effective_mean_draft_len"], serde_json::json!(0.0));
        assert_eq!(
            entry["mtp_seconds_per_token_mean"],
            serde_json::json!(0.125)
        );

        // Round-trips back into the typed payload unchanged (the overlay deserializes sealed
        // scores), and the coarsening pass leaves the array verbatim.
        let back: ScorePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.metrics.per_prompt, metrics.per_prompt);
        assert_eq!(
            metrics.with_coarsened_public_diagnostics(2).per_prompt,
            metrics.per_prompt,
            "per_prompt mirrors the ranking fields: never coarsened"
        );
    }
}
