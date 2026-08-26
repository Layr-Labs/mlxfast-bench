//! Scoring + golden-schema constants, ported verbatim from the Swift
//! `MLXFastConstants` enum (Sources/MLXFastCore/Constants.swift).
//!
//! This module is the single source of truth for the values that are triplicated
//! across the Swift codebase (MLXFastConstants, benchmark.yml, overlay-paired-timing.sh).
//! Only the scoring + golden-validation subset is ported here (this crate's scope).
//!
//! NOTE: the baseline seconds-per-token values and the acceptance-band tolerances are the
//! Gemma 4 26B A4B box-3 calibration of 2026-08-25 (four cool-gated `--local-iterate` runs
//! on the stock tree; David's 2026-08-25 ruling makes box 3 the ranked box), carried here
//! bit-identical to the reference engine's `Constants.swift` @ `e1fddf39` — see the
//! per-constant doc comments below for provenance and the derivation pointer.

// --- Scoring subset (MLXFastConstants.score*, *BandTolerance, officialBaseline*) ---

/// `MLXFastConstants.scoreDecodeWeight`
pub const SCORE_DECODE_WEIGHT: f64 = 0.75;
/// `MLXFastConstants.scorePrefillWeight`
pub const SCORE_PREFILL_WEIGHT: f64 = 0.25;

/// `MLXFastConstants.scoreDecodeSpeedupFloor`
pub const SCORE_DECODE_SPEEDUP_FLOOR: f64 = 0.95;
/// `MLXFastConstants.scorePrefillSpeedupFloor`
pub const SCORE_PREFILL_SPEEDUP_FLOOR: f64 = 0.95;

/// `MLXFastConstants.prefillBandUpTolerance` (prefill is a symmetric +/-3% health gate).
///
/// Bands re-derived 2026-08-25 from the gemma4 box-3 calibration session's CVs, qwen band
/// shape preserved (symmetric prefill; asymmetric decode, tight on regressions): tight
/// side = ceil-to-half-percent of 10x the per-axis session CV, decode down side = qwen's
/// 2.5x down:up asymmetry ratio. Full derivation: the BAND DERIVATION block in the
/// reference engine (`mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastCore/Constants.swift`
/// `@e1fddf39:305-330`). Lockstep with that file — retune both together.
pub const PREFILL_BAND_UP_TOLERANCE: f64 = 0.03;
/// `MLXFastConstants.prefillBandDownTolerance`
pub const PREFILL_BAND_DOWN_TOLERANCE: f64 = 0.03;
/// `MLXFastConstants.decodeBandUpTolerance` (+1% regression ceiling on the scored axis)
pub const DECODE_BAND_UP_TOLERANCE: f64 = 0.01;
/// `MLXFastConstants.decodeBandDownTolerance` (per-submission decode gain capped at 2.5%)
pub const DECODE_BAND_DOWN_TOLERANCE: f64 = 0.025;

/// `MLXFastConstants.publicDiagnosticSignificantFigures`
pub const PUBLIC_DIAGNOSTIC_SIGNIFICANT_FIGURES: u32 = 2;

// --- qwen-mtp-paired-decode-only scoring (track qwen3.8-27b-mtp-v1) ---
//
// The authoritative paired score for the MTP spec-decode track (benchmark.json `scoring`
// mode `qwen-mtp-paired-decode-only`, mirrored in the track fixture
// qwen3_8_27b_mtp_track.json `scoring_semantics`). This is DECODE-ONLY and serial-anchored
// (serial control = 1.0, no normalization): per prompt the raw ratio is
// `mean(serial depth-0 decode s/tok) / mean(candidate decode s/tok)` over that prompt's
// accepted pairs, and the published score is the EVEN-N median of the per-prompt raw ratios.
// These constants REPLACE the generic 0.95 decode/prefill speedup floors for the paired
// score; the generic `SCORE_*_SPEEDUP_FLOOR` path (ds^0.75·ps^0.25) is untouched.

/// Paired decode-only submission floor on the RAW median (ranked workflow
/// `MLXFAST_QWEN_MTP_DECODE_SPEEDUP_FLOOR`). Operator decision 2026-08-14: 0.90 — "do not
/// regress serial by more than 10%". A candidate that cannot beat serial should stop drafting
/// and take 1.0. Below this the run floor-fails (score null).
///
/// #117 — this floor governs the `free_run_v1_1` series TOO, by David's ruling on #109 (comment
/// 5353123259, 2026-08-20): "floor stays 0.90, no sub-floor bootstrap governance built — the stock
/// free-run median landing below 0.90 'shouldn't happen; ignore the case.'" The ruling is the
/// AUTHORITY there; the ~0.935 calibration that justified 0.90 for the teacher-forced series is
/// NOT inherited into the free-run series (#109 comment 5350423826, §5). Sealed on the free-run
/// measure-job path as `measure_job::FREE_RUN_DECODE_SPEEDUP_FLOOR`, which aliases this constant so
/// the seal and the ranked overlay floor cannot diverge.
pub const QWEN_MTP_DECODE_SPEEDUP_FLOOR: f64 = 0.90;
/// Paired decode-only ceiling on the RAW median (ranked workflow
/// `MLXFAST_QWEN_MTP_DECODE_SPEEDUP_CEILING`). Raised 3.0→5.0 by operator decision 2026-08-17.
/// Above this the median is a measurement fault or an escape and the run ceiling-fails.
pub const QWEN_MTP_DECODE_SPEEDUP_CEILING: f64 = 5.0;
/// Per-PAIR plausibility bound (box wrapper `MAX_PLAUSIBLE_PUBLISHED_SPEEDUP` /
/// `QMTP_MAX_PLAUSIBLE`): any single pair ratio above this is rejected before aggregation.
/// Raised 5.0→8.0 by operator decision 2026-08-17 so it stays strictly looser than the 5.0
/// median ceiling.
pub const QWEN_MTP_PER_PAIR_RATIO_BOUND: f64 = 8.0;
/// Calibration: what an UNMODIFIED (stock depth-2) tree scores under the raw serial-anchored
/// semantics, measured on Qwen 3.8 over six gated sessions (track fixture
/// `calibration.expected_raw_median`). The serial-band analogue for the paired MEDIAN.
// UNVERIFIED(measure-job): expected value + band are track-fixture parity data, not
// re-derived against a live ranked box here.
pub const QWEN_MTP_EXPECTED_RAW_MEDIAN: f64 = 0.9940390645;
/// Calibration band (percent) around [`QWEN_MTP_EXPECTED_RAW_MEDIAN`] (track fixture
/// `calibration.band_pct`): ±2.0% ⇒ [0.9742, 1.0139].
// UNVERIFIED(measure-job): band retained from the ratified sizing, not re-derived here.
pub const QWEN_MTP_CALIBRATION_BAND_PCT: f64 = 2.0;

/// H3 (cycle-3) — RunTimeout liveness safeguard (PROTOCOL-v1.1 §2.2/§4). benchd arms a wall-clock
/// timeout on the timed decode round-trips equal to `N × band-ceiling × margin`; this is the fixed
/// `margin` slack factor. It is a LIVENESS bound, never an input to the score — a passing run
/// finishes well inside the budget; the margin only exists so normal jitter never trips it. On
/// timeout benchd raises `RunTimeout`, discards the session (fail-closed), and the pair fails.
pub const RUN_TIMEOUT_MARGIN: f64 = 4.0;
/// H3 (cycle-3) — fallback per-token latency `band-ceiling` (seconds-per-token) for the RunTimeout
/// budget when no `BASELINE_CALIBRATION` is available (e.g. the free-run path or `BASELINE_BAND_ENFORCE=0`).
/// With calibration present, the band-ceiling is `calibration.serial_mean × calibration.band_high`
/// (the upper acceptance/latency band bound); absent it, this deliberately-generous constant
/// bounds a hung engine without ever tripping a healthy run.
///
/// #127 — this used to ALIAS [`OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN`], which was carrying
/// the RETIRED `mlxfast-challenge-dev` fork's Gemma-era value. Correcting that constant to the
/// reference's Qwen value (below) would have tightened this liveness ceiling ~9.6×, to BELOW the
/// decode seconds-per-token a healthy candidate actually measures (the §8 window measured
/// ~0.0347 s/token against a 0.01386 s/token reference baseline — a candidate is SLOWER than the
/// reference-runner baseline, which is the whole point of the speedup denominator). A liveness
/// bound that a passing run trips is not a liveness bound, so the ceiling keeps its own literal:
/// numerically unchanged, no longer coupled to a scoring denominator it never meant to track.
pub const RUN_TIMEOUT_DEFAULT_BAND_CEILING_SECONDS_PER_TOKEN: f64 = 0.1336139485703125;

/// `MLXFastConstants.officialBaselinePrefillSecondsPerToken`
/// (`mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastCore/Constants.swift:359` @ `e1fddf39`).
///
/// #127 (RULED David 2026-08-20) — the LOCAL-ITERATE scoring denominator: the ruling makes
/// local-iterate score against these constants, so a stale value here is a live scoring defect,
/// not dead documentation. (Historical: before that ruling landed, this slot carried the RETIRED
/// `mlxfast-challenge-dev` fork's pair — `0.010605031949609375` / `0.1336139485703125`, that
/// fork's `Constants.swift:124-125` — 28.9x / 9.64x off the then-reference's; the fixture keeps
/// that pair as `retired_fork` assert_ne targets.)
///
/// Current values: the Gemma 4 26B A4B box-3 calibration of 2026-08-25 — the mean of four
/// consecutive cool-gated (40C, macmon) `./benchmark.sh --local-iterate` runs on the stock tree
/// at engine `dec515a5` / benchd `c2327d15`, fresh worker per phase, zero warmup (prefill CV
/// 0.2712%, decode CV 0.0937%). The reference engine's own comment block records the full
/// provenance; this constant is its bit-identical mirror.
pub const OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN: f64 = 0.0003276219582519531;
/// `MLXFastConstants.officialBaselineDecodeSecondsPerToken`
/// (`mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastCore/Constants.swift:360` @ `e1fddf39`).
pub const OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN: f64 = 0.012374741210937498;

// --- Golden-schema validation subset (MLXFastConstants.*) ---

/// The frozen internal architecture id of the pinned Gemma 4 26B A4B text tower (Swift
/// `MLXFastConstants.requiredGoldenModelType`,
/// `mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastCore/Constants.swift@ade3b99a:117`).
/// benchd's golden loader requires it exactly, matching the Swift benchmark/correctness
/// path — a golden without it, or with a different value, is rejected byte-for-byte as Swift
/// does. This is a bench-core-level identity fact (not a CLI detail), single-sourced here so
/// the loader and any consumer share one definition and it falls under the loader-parity
/// corpus. (Per-target once `target.toml [model]` lands; this track is frozen to the
/// `gemma4_text` tower. Was `"qwen3_5_text"` — the retired Qwen-era identity; a real gemma
/// golden, whose engine writes `model_type: "gemma4_text"`, would have been rejected on
/// identity by the old value.)
pub const REQUIRED_GOLDEN_MODEL_TYPE: &str = "gemma4_text";

/// `MLXFastConstants.vocabSize`
/// (`mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastCore/Constants.swift@ade3b99a:161`, read
/// off the pinned checkpoint's own `config.json` `text_config`). Was `248_320` — the Qwen-era
/// vocab; the tape/golden token-range validation would have refused valid gemma token ids in
/// `248_320..262_144`.
///
/// LOCKSTEP HAZARD: this bound exists in TWO copies that must move together — the engine's
/// `MLXFastConstants.vocabSize`, which the reference-tape recorder applies as its emit-time
/// pre-check (`Sources/MLXFastTrustedHarness/QwenRuntimeReferenceTape.swift@e074ec67:184`),
/// and THIS constant, which the benchd tape/golden loaders apply at load time. If they
/// diverge, the recorder's fail-early guarantee is defeated: a token the recorder happily
/// emits (e.g. `250_000`, a legitimate gemma id) is refused only later, at benchd load.
/// benchd was the stale copy when the two last diverged.
pub const VOCAB_SIZE: usize = 262_144;

/// `MLXFastConstants.correctnessSteps`
pub const CORRECTNESS_STEPS: usize = 64;
/// `MLXFastConstants.correctnessPromptTokens`
///
/// 1024 (was 512): David's 2026-08-24 seed-length ruling for the Gemma 4 track — "Seed becomes
/// 1024". The decode window is unchanged ([`BENCHMARK_DECODE_STEPS`] stays 128); golden shape
/// becomes 1024 `prompt_tokens` + 129 `expected_tokens` (seed next-token + 128 checked steps).
/// The 1024-token versions of the hidden pool prompts must be uploaded and referenced from the
/// Gemma benchmark branch, and every golden regenerated at the new seed, before scoring arms.
pub const CORRECTNESS_PROMPT_TOKENS: usize = 1_024;
/// `MLXFastConstants.correctnessTopLogits`
pub const CORRECTNESS_TOP_LOGITS: usize = 8;
/// `MLXFastConstants.correctnessLogitTieTolerance` — the default top-logit delta the
/// anchor rank/delta path uses when a case sets `max_expected_rank` but no explicit
/// `max_top_logit_delta` (Swift `anchor.maxTopLogitDelta ?? correctnessLogitTieTolerance`).
pub const CORRECTNESS_LOGIT_TIE_TOLERANCE: f64 = 1e-6;

/// `MLXFastConstants.correctnessMaxAnchorContextTokens`
pub const CORRECTNESS_MAX_ANCHOR_CONTEXT_TOKENS: usize = 1_024;
/// `MLXFastConstants.correctnessMaxFreeRunSteps`
pub const CORRECTNESS_MAX_FREE_RUN_STEPS: usize = 256;
/// `MLXFastConstants.correctnessMaxBehaviorPromptTokens`
pub const CORRECTNESS_MAX_BEHAVIOR_PROMPT_TOKENS: usize = 2_048;
/// `MLXFastConstants.correctnessMaxBehaviorSteps`
pub const CORRECTNESS_MAX_BEHAVIOR_STEPS: usize = 64;

/// `MLXFastConstants.benchmarkPrefillPromptTokens`
///
/// 1024 (was 512): moves with [`CORRECTNESS_PROMPT_TOKENS`] under the 2026-08-24 seed-length
/// ruling — the timed prefill leg is now 8 x 1024 tokens per cohort. Any baseline/calibration
/// value derived at the 512-token prefill window is invalidated and must be re-derived.
pub const BENCHMARK_PREFILL_PROMPT_TOKENS: usize = 1_024;
/// `MLXFastConstants.benchmarkDecodeSeedTokens`
///
/// 1024 (was 512): moves with [`CORRECTNESS_PROMPT_TOKENS`] under the 2026-08-24 seed-length
/// ruling.
pub const BENCHMARK_DECODE_SEED_TOKENS: usize = 1_024;
/// `MLXFastConstants.benchmarkDecodeSteps`
pub const BENCHMARK_DECODE_STEPS: usize = 128;
/// `MLXFastConstants.localIterateBenchmarkDecodeSteps` — the checked decode window the
/// participant edit loop (`--local-iterate`) uses.
///
/// This is `benchmarkDecodeSteps` on the reference tree, NOT a shorter window: the reference
/// states the reason inline — "Local iterate charges the same seed prefill as the
/// official decode window" ([`BENCHMARK_DECODE_SEED_TOKENS`], 1024 since the 2026-08-24
/// seed-length ruling; 512 at the time of the quoted reference), "so it must use the same
/// denominator to produce a comparable decode seconds-per-token estimate"
/// (`mlxfast-qwen-38-27b-mtp-engine/Sources/MLXFastCore/Constants.swift@6279c7a:197-201`).
///
/// It was ported as `16` from the retired Laguna/DFlash fork
/// (`mlxfast-challenge-dev/Sources/MLXFastCore/Constants.swift:71`), which is the tree the
/// original local-iterate port read; the Qwen 3.8 engine — this challenge's reference —
/// carries `= benchmarkDecodeSteps`. Same class of stale-reference drift as the golden
/// `model_provenance` row (#112/#114): benchd was LOOSER than the reference because it held
/// the old fork's value.
pub const LOCAL_ITERATE_BENCHMARK_DECODE_STEPS: usize = BENCHMARK_DECODE_STEPS;
/// `MLXFastConstants.localSubmitBenchmarkDecodeSteps` — the long continuous checked
/// decode window the submit path (`--local-submit`) times (Swift `QwenRuntime.localIterate`
/// invoked with `decodeSteps = 1023`, main.swift:264-291). It reuses the local-iterate
/// checked-timing machinery over a 1023-step decode of `cases[0]`.
pub const LOCAL_SUBMIT_BENCHMARK_DECODE_STEPS: usize = 1023;

/// `MLXFastConstants.defaultMaxTransformedWeightsBytes` (Constants.swift:133) — the
/// default transformed-weights size cap (25 GiB) enforced by weights preflight, overridable
/// by `MLXFAST_MAX_WEIGHTS_BYTES` (`0`/`none`/`unlimited` disable it).
pub const DEFAULT_MAX_TRANSFORMED_WEIGHTS_BYTES: u64 = 25 * 1024 * 1024 * 1024;

/// (b) admission — the PER-STREAM token-tolerance threshold, in tokens-per-thousand (David's
/// blanket-10% ruling, 2026-08-25). Each cohort stream may differ from the trusted reference argmax
/// on at most this many of every 1000 of its OWN committed tokens; expressed per-thousand so the gate
/// is pure INTEGER arithmetic (`mismatches * 1000 <= COHORT_TOKEN_TOLERANCE_PER_THOUSAND *
/// committed_len`), with no float ratio and no rounding at the 10% boundary — exactly 10% passes.
///
/// PER-STREAM, never a cohort average: ANY single stream over the threshold rejects the WHOLE run
/// ([`crate::cohort_tolerance::evaluate_cohort_token_tolerance`]). The reference argmax comes from the
/// organizer's TRUSTED oracle replaying the candidate's own committed tokens over the pinned reference
/// weights, so the candidate can only choose WHICH tokens it commits, not steer the reference.
///
/// Anti-gaming caveats (stated for the verdict): David accepted that a UNIFORMLY degraded model wrong
/// on ≤10% of tokens per stream passes and can win on speed — (b) is a similar-output speedup bar, not
/// a lossless-correctness one. CONCENTRATION gaming (pushing all divergence into one stream) is closed
/// by the per-stream rule: one stream over 10% fails the run regardless of the others. The value lives
/// ONLY here (never in the JSON fixture — config-carries-no-prose).
pub const COHORT_TOKEN_TOLERANCE_PER_THOUSAND: u32 = 100;

/// `MLXFastConstants.benchmarkPrefillWarmupRuns` — zero: the timed benchmark runs
/// cold (the correctness gate must not warm the measured path), and the official
/// baseline was calibrated the same way. See Constants.swift.
pub const BENCHMARK_PREFILL_WARMUP_RUNS: usize = 0;
/// `MLXFastConstants.benchmarkPrefillTimedRuns` — one measured prefill run.
pub const BENCHMARK_PREFILL_TIMED_RUNS: usize = 1;
