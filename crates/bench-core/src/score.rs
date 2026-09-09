//! Score formula, speedups, floors, and acceptance bands.
//!
//! Ported from Sources/MLXFastCore/Score.swift (`BenchmarkScore`, `TimedRunScoreEvaluation`)
//! and Sources/MLXFastCore/AcceptanceBand.swift (`AcceptanceBand`, `AcceptanceBandResult`).
//! The guard / NaN / zero semantics are preserved exactly.

use crate::constants::{
    AcceptanceBands, QWEN_MTP_DECODE_SPEEDUP_CEILING, QWEN_MTP_DECODE_SPEEDUP_FLOOR,
    QWEN_MTP_PER_PAIR_RATIO_BOUND, SCORE_DECODE_SPEEDUP_FLOOR, SCORE_DECODE_WEIGHT,
    SCORE_PREFILL_SPEEDUP_FLOOR, SCORE_PREFILL_WEIGHT,
};

/// `BenchmarkScore.speedup`: baseline/candidate, or 0 if either is non-finite or <= 0.
pub fn speedup(baseline_spt: f64, candidate_spt: f64) -> f64 {
    if !baseline_spt.is_finite()
        || !candidate_spt.is_finite()
        || baseline_spt <= 0.0
        || candidate_spt <= 0.0
    {
        return 0.0;
    }
    baseline_spt / candidate_spt
}

/// `BenchmarkScore.score`: weighted geometric mean of the decode/prefill speedups.
///
/// Returns `f64::NAN` if either speedup is <= 0, or the weights are non-finite /
/// negative / sum to <= 0 (mirrors the Swift `guard ... else { return .nan }`).
pub fn score(
    decode_spt: f64,
    prefill_spt: f64,
    baseline_decode_spt: f64,
    baseline_prefill_spt: f64,
    decode_weight: f64,
    prefill_weight: f64,
) -> f64 {
    let decode_speedup = speedup(baseline_decode_spt, decode_spt);
    let prefill_speedup = speedup(baseline_prefill_spt, prefill_spt);
    let total_weight = decode_weight + prefill_weight;
    // Reject NaN and non-positive inputs. `x.is_nan() || x <= 0.0` is the
    // clippy-clean equivalent of the NaN-catching `!(x > 0.0)` guard (accepts
    // +inf, rejects <= 0 and NaN — identical semantics).
    if decode_speedup.is_nan()
        || decode_speedup <= 0.0
        || prefill_speedup.is_nan()
        || prefill_speedup <= 0.0
        || !decode_weight.is_finite()
        || !prefill_weight.is_finite()
        || decode_weight < 0.0
        || prefill_weight < 0.0
        || total_weight.is_nan()
        || total_weight <= 0.0
    {
        return f64::NAN;
    }
    decode_speedup.powf(decode_weight / total_weight)
        * prefill_speedup.powf(prefill_weight / total_weight)
}

/// Convenience wrapper using the default 0.75 / 0.25 scoring weights.
pub fn score_default_weights(
    decode_spt: f64,
    prefill_spt: f64,
    baseline_decode_spt: f64,
    baseline_prefill_spt: f64,
) -> f64 {
    score(
        decode_spt,
        prefill_spt,
        baseline_decode_spt,
        baseline_prefill_spt,
        SCORE_DECODE_WEIGHT,
        SCORE_PREFILL_WEIGHT,
    )
}

/// THE TWO SPEEDUP FLOORS one scored run enforces and seals (David ruling 2026-09-09: 0.95 decode
/// AND 0.95 prefill, properly enforced, configurable per project).
///
/// PER PROJECT: the `--contract` track fixture declares them (`decode_speedup_floor`,
/// `prefill_speedup_floor`) and the official path REFUSES a fixture that does not — see
/// `benchd::contract::speedup_floors`. One value carries the pair from that fixture to BOTH the
/// gate ([`evaluate_timed_run`]) and the seal (`metrics.decode_speedup_floor` /
/// `metrics.prefill_speedup_floor`), so what a run seals is what it enforced.
///
/// [`SpeedupFloors::DEFAULT`] — the [`SCORE_DECODE_SPEEDUP_FLOOR`] /
/// [`SCORE_PREFILL_SPEEDUP_FLOOR`] constants — is the LOCAL (no `--contract`) default and nothing
/// else. No scored run may reach it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedupFloors {
    pub decode: f64,
    pub prefill: f64,
}

impl SpeedupFloors {
    /// The floors a LOCAL run (no `--contract`) uses: the ruled 0.95 / 0.95 constants.
    pub const DEFAULT: SpeedupFloors = SpeedupFloors {
        decode: SCORE_DECODE_SPEEDUP_FLOOR,
        prefill: SCORE_PREFILL_SPEEDUP_FLOOR,
    };
}

/// `BenchmarkScore.passesSpeedupFloors`: false if anything is non-finite, else both
/// speedups must clear their floor.
pub fn passes_speedup_floors(
    decode_speedup: f64,
    prefill_speedup: f64,
    decode_floor: f64,
    prefill_floor: f64,
) -> bool {
    if !decode_speedup.is_finite()
        || !prefill_speedup.is_finite()
        || !decode_floor.is_finite()
        || !prefill_floor.is_finite()
    {
        return false;
    }
    decode_speedup >= decode_floor && prefill_speedup >= prefill_floor
}

/// `BenchmarkScore.speedupFloorFailureMessage`: exact POSIX/en_US format, 6 decimals.
pub fn speedup_floor_failure_message(
    decode_speedup: f64,
    prefill_speedup: f64,
    decode_floor: f64,
    prefill_floor: f64,
) -> String {
    // Swift: String(format: "%.6f", locale: en_US_POSIX, value). Rust's {:.6} matches
    // for finite values (the only case this message is produced for in practice).
    format!(
        "performance floor failed: decode_speedup={:.6} floor={:.6} prefill_speedup={:.6} floor={:.6}",
        decode_speedup, decode_floor, prefill_speedup, prefill_floor
    )
}

/// Ported from `AcceptanceBand` (AcceptanceBand.swift).
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptanceBandResult {
    pub passed: bool,
    /// Empty when `passed`; otherwise a human-readable failure reason.
    pub reason: String,
}

impl AcceptanceBandResult {
    fn passed() -> Self {
        AcceptanceBandResult {
            passed: true,
            reason: String::new(),
        }
    }
    fn failed(reason: String) -> Self {
        AcceptanceBandResult {
            passed: false,
            reason,
        }
    }
}

/// `AcceptanceBand.robustReference`: need >= 3 finite/positive samples, drop the single
/// slowest (max) sample, average the rest.
pub fn robust_reference(samples: &[f64]) -> Option<f64> {
    if samples.len() < 3 || !samples.iter().all(|s| s.is_finite() && *s > 0.0) {
        return None;
    }
    // Drop the single slowest (max) sample. Swift's `indices.max(by: <)` returns the
    // last index among equal maxima; the average is identical regardless of which
    // equal-valued max is removed, so index choice is immaterial.
    let mut rest: Vec<f64> = samples.to_vec();
    let mut slowest_idx = 0usize;
    for i in 1..rest.len() {
        if rest[slowest_idx] < rest[i] {
            slowest_idx = i;
        }
    }
    rest.remove(slowest_idx);
    let sum: f64 = rest.iter().sum();
    Some(sum / rest.len() as f64)
}

/// `AcceptanceBand.check`: per-run band gate against a paired baseline. Two-sided when
/// `enforce_lower_bound` is `true`; when `false` the LOWER ("improvement too large") test is SKIPPED
/// and only the upper bound is enforced. The lower bound is disabled for the MTP timed leg's decode
/// axis (see [`crate::constants::AcceptanceBands::decode_down_enabled`]): MTP decode legitimately
/// runs much faster than the serial baseline, so the -tolerance% lower guard would wrongly fail a
/// healthy run — the 0.95 decode speedup floor is the only lower guard the decode axis needs.
pub fn check(
    value: f64,
    reference: f64,
    up_tolerance: f64,
    down_tolerance: f64,
    enforce_lower_bound: bool,
    label: &str,
) -> AcceptanceBandResult {
    if !value.is_finite() || value <= 0.0 || !reference.is_finite() || reference <= 0.0 {
        return AcceptanceBandResult::failed(format!(
            "{label} ({value}) and reference ({reference}) must be finite and positive"
        ));
    }
    let hi = reference * (1.0 + up_tolerance);
    let lo = reference * (1.0 - down_tolerance);
    if value > hi {
        return AcceptanceBandResult::failed(format!(
            "{label} {value} exceeds +{}% of reference {reference} (> {hi}): \
slowdown/regression beyond tolerance",
            up_tolerance * 100.0
        ));
    }
    if enforce_lower_bound && value < lo {
        return AcceptanceBandResult::failed(format!(
            "{label} {value} below -{}% of reference {reference} (< {lo}): \
improvement too large for one submission (chunk it) or a suspiciously lucky reading",
            down_tolerance * 100.0
        ));
    }
    AcceptanceBandResult::passed()
}

/// Ported from Swift `TimedRunScoreEvaluation`.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedRunScoreEvaluation {
    pub score: f64,
    pub decode_speedup: f64,
    pub prefill_speedup: f64,
    pub passes_floors: bool,
    /// The floors `passes_floors` was decided against, carried so the failure message names the
    /// floors the run actually enforced (never a constant it did not).
    pub floors: SpeedupFloors,
    pub prefill_band: AcceptanceBandResult,
    pub decode_band: AcceptanceBandResult,
}

impl TimedRunScoreEvaluation {
    /// `hasFiniteScore`: score finite && >= 0.
    pub fn has_finite_score(&self) -> bool {
        self.score.is_finite() && self.score >= 0.0
    }

    /// `passesAcceptanceBands`: both bands passed.
    pub fn passes_acceptance_bands(&self) -> bool {
        self.prefill_band.passed && self.decode_band.passed
    }

    /// `firstFailureReason`: same priority order (non-finite score -> floors -> bands).
    pub fn first_failure_reason(&self) -> Option<String> {
        if !self.has_finite_score() {
            return Some("computed score was not finite".to_string());
        }
        if !self.passes_floors {
            return Some(speedup_floor_failure_message(
                self.decode_speedup,
                self.prefill_speedup,
                self.floors.decode,
                self.floors.prefill,
            ));
        }
        if !self.passes_acceptance_bands() {
            let reason = if self.prefill_band.passed {
                &self.decode_band.reason
            } else {
                &self.prefill_band.reason
            };
            return Some(format!("acceptance band failed: {reason}"));
        }
        None
    }
}

/// `BenchmarkScore.evaluateTimedRun`. Prefill band uses the prefill up/down tolerances;
/// decode band uses the decode up/down tolerances (all from `constants`). `floors` is the run's
/// resolved [`SpeedupFloors`] — the track fixture's pair on a scored run — and the evaluation
/// carries it back, so the caller seals the floors this gate enforced.
pub fn evaluate_timed_run(
    decode_spt: f64,
    prefill_spt: f64,
    baseline_decode_spt: f64,
    baseline_prefill_spt: f64,
    bands: AcceptanceBands,
    floors: SpeedupFloors,
) -> TimedRunScoreEvaluation {
    let s = score_default_weights(
        decode_spt,
        prefill_spt,
        baseline_decode_spt,
        baseline_prefill_spt,
    );
    let decode_speedup = speedup(baseline_decode_spt, decode_spt);
    let prefill_speedup = speedup(baseline_prefill_spt, prefill_spt);
    // Prefill is ALWAYS two-sided (±tolerance symmetric health gate). The decode LOWER bound is
    // conditional: the MTP timed leg disables it (`decode_down_enabled == false`), leaving only the
    // decode UP bound and the 0.95 decode speedup floor as guards.
    let prefill_band = check(
        prefill_spt,
        baseline_prefill_spt,
        bands.prefill_up_tolerance,
        bands.prefill_down_tolerance,
        true,
        "prefill",
    );
    let decode_band = check(
        decode_spt,
        baseline_decode_spt,
        bands.decode_up_tolerance,
        bands.decode_down_tolerance,
        bands.decode_down_enabled,
        "decode",
    );
    TimedRunScoreEvaluation {
        score: s,
        decode_speedup,
        prefill_speedup,
        passes_floors: passes_speedup_floors(
            decode_speedup,
            prefill_speedup,
            floors.decode,
            floors.prefill,
        ),
        floors,
        prefill_band,
        decode_band,
    }
}

// ---------------------------------------------------------------------------
// Timed-window liveness (RunTimeout budget)
// ---------------------------------------------------------------------------
//
// A wall-clock deadline for the timed decode round-trips. This is a LIVENESS bound only — it never
// enters the score. (The retired qwen-mtp-paired-decode-only scoring that used to live here went
// with flow B; the single-leg official path scores through `evaluate_timed_run` above.)

/// H3 (cycle-3) — the RunTimeout wall-clock budget for the timed decode round-trips
/// (PROTOCOL-v1.1 §2.2/§4): `N × band_ceiling_spt × margin`. `n` is the token count, `band_ceiling_spt`
/// the upper acceptance/latency band bound (seconds-per-token) for the series, `margin` a fixed
/// slack factor ([`crate::constants::RUN_TIMEOUT_MARGIN`]). The budget is a LIVENESS bound only; it
/// never enters the score.
///
/// #108 (M2) — FAIL-CLOSED on every degenerate input (`n == 0`, non-finite / non-positive ceiling or
/// margin, non-finite / non-positive product): an `Err`, never a `None` that DISARMS the deadline.
/// This function previously returned `None` there and the caller armed no deadline at all, on the
/// reasoning that "a missing budget falls back to the blocking read — safe, not a fake timeout".
/// That is only true when the degenerate input is benchd's own absent configuration. It is NOT true
/// when the input is ATTACKER-CHOSEN: the ceiling is `calibration.serial_mean × band_high`, both
/// read from the `BASELINE_CALIBRATION` file, so a `band_high` of `0.0` made the product
/// non-positive and turned the §2.2 wall-clock bound off through a config file. A hung or looping
/// engine then wedged benchd inside the timed window with nothing to abort it. The caller turns this
/// `Err` into a leg failure with its own reject class, so the condition is loud and the run dies
/// rather than running unbounded.
pub fn run_timeout_budget(
    n: usize,
    band_ceiling_spt: f64,
    margin: f64,
) -> Result<std::time::Duration, String> {
    if n == 0 {
        return Err(
            "RunTimeout budget: token count N is 0, so N × ceiling × margin is not a \
                    positive wall-clock bound (§2.2)"
                .to_string(),
        );
    }
    if !band_ceiling_spt.is_finite() || band_ceiling_spt <= 0.0 {
        return Err(format!(
            "RunTimeout budget: band ceiling ({band_ceiling_spt} s/tok) is not finite and positive \
             — the §2.2 deadline (N × ceiling × margin) cannot be armed from it, and benchd REFUSES \
             to run the timed window unbounded instead (the ceiling is calibration-derived: \
             serial_mean × serial_band_high)"
        ));
    }
    if !margin.is_finite() || margin <= 0.0 {
        return Err(format!(
            "RunTimeout budget: margin ({margin}) is not finite and positive — the §2.2 deadline \
             cannot be armed from it"
        ));
    }
    let secs = n as f64 * band_ceiling_spt * margin;
    if !secs.is_finite() || secs <= 0.0 {
        return Err(format!(
            "RunTimeout budget: N ({n}) × ceiling ({band_ceiling_spt}) × margin ({margin}) = \
             {secs}, which is not a finite positive number of seconds — refusing to run the timed \
             window with no wall-clock bound (§2.2)"
        ));
    }
    Ok(std::time::Duration::from_secs_f64(secs))
}

/// The RAW serial-relative decode ratio for ONE pair: `serial_decode_spt / candidate_decode_spt`
/// (serial is the numerator / normaliser; a faster candidate ⇒ ratio > 1). Reuses [`speedup`],
/// so it is 0 when either seconds-per-token value is non-finite or ≤ 0 (an implausible/blank
/// pair the caller rejects). One "pair" today = one serial leg vs one candidate leg over the
/// same window.
pub fn paired_decode_raw_ratio(serial_decode_spt: f64, candidate_decode_spt: f64) -> f64 {
    speedup(serial_decode_spt, candidate_decode_spt)
}

/// The EVEN-N median of the per-prompt raw ratios (track fixture
/// `scoring_semantics.median_rule = even_n_mean_of_two_central_order_statistics`): for an even
/// count the mean of the two central order statistics, for an odd count the middle element.
/// (This is NOT the lower-median rule the per-pair diagnostic / CLI p50 use.) A single-prompt
/// run yields that one ratio. Returns `NaN` for an empty slice (the caller guards non-empty).
pub fn paired_decode_only_median(per_prompt_raw_ratios: &[f64]) -> f64 {
    let n = per_prompt_raw_ratios.len();
    if n == 0 {
        return f64::NAN;
    }
    let mut sorted = per_prompt_raw_ratios.to_vec();
    // Total order over f64 for the order statistics; NaN sorts last (and is caught by the
    // finite check in the gate). `partial_cmp` is safe here as we sort a materialised copy.
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Greater));
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        // Mean of the two central order statistics.
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Which bound a paired decode-only run failed (the score is null and `error` names it).
#[derive(Debug, Clone, PartialEq)]
pub enum PairedDecodeFailure {
    /// A single pair ratio exceeded the per-pair plausibility bound (8.0) — rejected before
    /// aggregation. `ratio` is the offending pair value.
    PerPairBound { ratio: f64, bound: f64 },
    /// The raw median was non-finite (a blank/implausible pair leaked through).
    NonFiniteMedian { median: f64 },
    /// The raw median fell below the submission floor (0.90) — a regression worse than -10%.
    Floor { median: f64, floor: f64 },
    /// The raw median exceeded the ceiling (5.0) — a measurement fault or an escape.
    Ceiling { median: f64, ceiling: f64 },
}

impl PairedDecodeFailure {
    /// A human-readable message that NAMES the failing bound (goes into `metrics.error`).
    pub fn message(&self) -> String {
        match self {
            PairedDecodeFailure::PerPairBound { ratio, bound } => format!(
                "paired decode-only per-pair plausibility bound exceeded: pair ratio={ratio} > bound={bound}"
            ),
            PairedDecodeFailure::NonFiniteMedian { median } => {
                format!("paired decode-only median is not finite: raw_median={median}")
            }
            PairedDecodeFailure::Floor { median, floor } => format!(
                "paired decode-only floor failed: raw_median={median} < floor={floor}"
            ),
            PairedDecodeFailure::Ceiling { median, ceiling } => format!(
                "paired decode-only ceiling failed: raw_median={median} > ceiling={ceiling}"
            ),
        }
    }
}

/// The outcome of the qwen-mtp-paired-decode-only score gate.
#[derive(Debug, Clone, PartialEq)]
pub struct PairedDecodeOnlyScore {
    /// The even-n median of the per-prompt raw ratios (ALWAYS reported, full precision — it is
    /// the ranking figure even when a bound fails, for the results.json `decode_speedup`).
    pub raw_median: f64,
    /// `Some(raw_median)` when every bound passed; `None` on any per-pair / floor / ceiling /
    /// non-finite failure.
    pub score: Option<f64>,
    /// True iff `score.is_some()`.
    pub passed: bool,
    /// The failing bound (and its message) when `!passed`.
    pub failure: Option<PairedDecodeFailure>,
}

/// Apply the paired decode-only gate to a run's per-pair ratios (for the per-pair plausibility
/// bound) and per-prompt raw ratios (for the median floor/ceiling). The two slices coincide when
/// there is one pair per prompt (the ranked k=1 default), but are kept separate so the per-pair
/// bound is checked on EACH pair, not on the aggregated per-prompt mean.
///
/// Priority: per-pair plausibility bound (8.0) → non-finite median → floor (0.90) → ceiling (5.0).
pub fn score_paired_decode_only(
    per_pair_ratios: &[f64],
    per_prompt_raw_ratios: &[f64],
) -> PairedDecodeOnlyScore {
    let raw_median = paired_decode_only_median(per_prompt_raw_ratios);
    let fail = |f: PairedDecodeFailure| PairedDecodeOnlyScore {
        raw_median,
        score: None,
        passed: false,
        failure: Some(f),
    };
    // Per-pair plausibility: any single pair above the bound (or non-finite/≤0) rejects the run
    // before aggregation (box wrapper MAX_PLAUSIBLE_PUBLISHED_SPEEDUP).
    for &r in per_pair_ratios {
        // A 0/negative ratio is an implausible/blank pair (docs classify it PerPairBound, NOT a
        // Floor fail) — reject it here before the median aggregation.
        if !r.is_finite() || r <= 0.0 || r > QWEN_MTP_PER_PAIR_RATIO_BOUND {
            return fail(PairedDecodeFailure::PerPairBound {
                ratio: r,
                bound: QWEN_MTP_PER_PAIR_RATIO_BOUND,
            });
        }
    }
    if !raw_median.is_finite() {
        return fail(PairedDecodeFailure::NonFiniteMedian { median: raw_median });
    }
    if raw_median < QWEN_MTP_DECODE_SPEEDUP_FLOOR {
        return fail(PairedDecodeFailure::Floor {
            median: raw_median,
            floor: QWEN_MTP_DECODE_SPEEDUP_FLOOR,
        });
    }
    if raw_median > QWEN_MTP_DECODE_SPEEDUP_CEILING {
        return fail(PairedDecodeFailure::Ceiling {
            median: raw_median,
            ceiling: QWEN_MTP_DECODE_SPEEDUP_CEILING,
        });
    }
    PairedDecodeOnlyScore {
        raw_median,
        score: Some(raw_median),
        passed: true,
        failure: None,
    }
}

// ---------------------------------------------------------------------------
// The composite over a track's DECLARED scored regime
// ---------------------------------------------------------------------------

/// One axis's factor in the composite: `gain ^ exponent`, with the two exponents that have an
/// exact answer folded rather than routed through `powf`.
///
/// * `exponent == 0.0` ⇒ `1.0`. The axis carries no weight, so it DROPS OUT — the gain is never
///   read at all, including when it is `NaN` or infinite, which is the state a track that does not
///   measure that axis is in.
/// * `exponent == 1.0` ⇒ the gain ITSELF, bit-for-bit, with no libm round trip.
///
/// Both folds are the identities `powf` already promises, spelled out here so the decode-only
/// regime's composite is provably the decode gain rather than provably-close to it.
fn gain_factor(gain: f64, exponent: f64) -> f64 {
    if exponent == 0.0 {
        1.0
    } else if exponent == 1.0 {
        gain
    } else {
        gain.powf(exponent)
    }
}

/// The composite a track's DECLARED regime ([`crate::constants::ScoredRegime`]) computes over the
/// two gain axes: `prefill_gain ^ a * decode_gain ^ b`.
///
/// DECLARED, NOT YET COMPUTED — this function has NO PRODUCTION CALL SITE. Nothing in a run
/// computes a composite today, and no `prefill_gain` / `decode_gain` is derived from the sealed
/// windows. The published figure is still [`score_paired_decode_only`]'s even-n median of the
/// per-prompt raw decode ratios. This is the one form in which a track's declared exponents WOULD
/// be applied, defined here so the declaration and the arithmetic cannot drift apart before then;
/// wiring it up is a later, separately-reviewed change, gated on the work-placement invariant in
/// `docs/scored-regime-and-prefill-window.md` §3.
///
/// It is ADDITIVE either way: it does not touch the generic `ds^0.75 · ps^0.25` [`score`] the
/// official and local runs use, nor the paired decode-only gate.
///
/// FIXTURE-INERT for a decode-only regime (`a = 0.0`, `b = 1.0`): the prefill factor is `1.0` and
/// the decode factor is the decode gain, so the composite is `1.0 * decode_gain` — `decode_gain`
/// bit-for-bit for every finite value, every infinity, and a QUIET NaN. Not for a SIGNALLING NaN,
/// which the multiply quiets (the payload's quiet bit flips); no path here produces one, since a
/// gain is a division of two measured durations. See
/// `constants::tests::the_declared_regime_reproduces_todays_score`.
pub fn composite_score(
    regime: &crate::constants::ScoredRegime,
    prefill_gain: f64,
    decode_gain: f64,
) -> f64 {
    gain_factor(prefill_gain, regime.prefill_gain_exponent)
        * gain_factor(decode_gain, regime.decode_gain_exponent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_timeout_budget_is_n_times_band_ceiling_times_margin() {
        // H3 (cycle-3) — the RunTimeout budget = N × band-ceiling × margin (§2.2/§4).
        let d = run_timeout_budget(128, 0.04, 4.0).unwrap();
        assert!((d.as_secs_f64() - (128.0 * 0.04 * 4.0)).abs() < 1e-9);
        // #108 (M2) — every degenerate input is an ERROR, never a `None` that DISARMS the §2.2
        // deadline. The ceiling is calibration-derived (serial_mean × serial_band_high), so a
        // silently-disarmed deadline was reachable from a config file.
        for (n, ceiling, margin, what) in [
            (0usize, 0.04, 4.0, "N==0"),
            (128, 0.0, 4.0, "non-positive ceiling"),
            (128, -1.0, 4.0, "negative ceiling"),
            (128, 0.04, 0.0, "non-positive margin"),
            (128, f64::NAN, 4.0, "non-finite ceiling"),
            (128, 0.04, f64::INFINITY, "non-finite margin"),
        ] {
            let err = run_timeout_budget(n, ceiling, margin)
                .expect_err(&format!("{what} must not silently disarm the deadline"));
            assert!(err.contains("RunTimeout budget"), "{what}: {err}");
        }
    }

    #[test]
    fn speedup_equal_is_one() {
        assert_eq!(speedup(0.1, 0.1), 1.0);
    }

    #[test]
    fn speedup_twice_as_fast_is_two() {
        // candidate half the seconds-per-token -> 2x speedup.
        assert_eq!(speedup(0.2, 0.1), 2.0);
    }

    #[test]
    fn speedup_guards_return_zero() {
        assert_eq!(speedup(0.0, 0.1), 0.0);
        assert_eq!(speedup(0.1, 0.0), 0.0);
        assert_eq!(speedup(-1.0, 0.1), 0.0);
        assert_eq!(speedup(f64::NAN, 0.1), 0.0);
        assert_eq!(speedup(f64::INFINITY, 0.1), 0.0);
    }

    #[test]
    fn score_equal_speedups_is_one() {
        // baseline == candidate on both axes -> both speedups 1.0 -> score 1.0.
        let s = score_default_weights(0.1, 0.2, 0.1, 0.2);
        assert!((s - 1.0).abs() < 1e-12);
    }

    #[test]
    fn score_decode_two_prefill_one_is_two_pow_075() {
        // decode_speedup = 2.0, prefill_speedup = 1.0 -> 2^0.75 * 1^0.25 = 2^0.75.
        let s = score_default_weights(0.05, 0.2, 0.1, 0.2);
        assert!((s - 2f64.powf(0.75)).abs() < 1e-12);
    }

    #[test]
    fn score_zero_baseline_is_nan() {
        let s = score_default_weights(0.1, 0.2, 0.0, 0.2);
        assert!(s.is_nan());
    }

    #[test]
    fn score_negative_weight_is_nan() {
        let s = score(0.05, 0.2, 0.1, 0.2, -0.1, 0.25);
        assert!(s.is_nan());
    }

    #[test]
    fn floors_at_exactly_095_pass() {
        assert!(passes_speedup_floors(0.95, 0.95, 0.95, 0.95));
    }

    #[test]
    fn floors_below_fail() {
        assert!(!passes_speedup_floors(0.9499, 1.0, 0.95, 0.95));
        assert!(!passes_speedup_floors(1.0, 0.9499, 0.95, 0.95));
    }

    #[test]
    fn floors_nonfinite_fail() {
        assert!(!passes_speedup_floors(f64::NAN, 1.0, 0.95, 0.95));
    }

    #[test]
    fn floor_message_format() {
        let m = speedup_floor_failure_message(0.9, 0.8, 0.95, 0.95);
        assert_eq!(
            m,
            "performance floor failed: decode_speedup=0.900000 floor=0.950000 \
prefill_speedup=0.800000 floor=0.950000"
        );
    }

    #[test]
    fn band_edges_inclusive_pass() {
        let reference = 100.0;
        // hi = 105 (up 5%), lo = 95 (down 5%). Exactly on the edges passes.
        assert!(check(105.0, reference, 0.05, 0.05, true, "x").passed);
        assert!(check(95.0, reference, 0.05, 0.05, true, "x").passed);
        assert!(check(100.0, reference, 0.05, 0.05, true, "x").passed);
    }

    #[test]
    fn band_beyond_edges_fail() {
        let reference = 100.0;
        let above = check(105.0001, reference, 0.05, 0.05, true, "x");
        assert!(!above.passed);
        assert!(above.reason.contains("slowdown/regression"));
        let below = check(94.9999, reference, 0.05, 0.05, true, "x");
        assert!(!below.passed);
        assert!(below.reason.contains("improvement too large"));
    }

    /// Change 3 — the MTP timed leg disables the decode DOWN band: a value far BELOW the lower edge
    /// (an "improvement too large") PASSES when `enforce_lower_bound = false`, while the UP bound
    /// still fails a genuine slowdown. The two-sided call rejects the same low value.
    #[test]
    fn band_lower_bound_disabled_skips_improvement_too_large_but_keeps_upper() {
        let reference = 100.0;
        // Far below the -5% edge, lower bound OFF → passes (the 0.95 floor is the real guard).
        let fast = check(50.0, reference, 0.02, 0.05, false, "decode");
        assert!(
            fast.passed,
            "lower bound disabled must accept a large improvement"
        );
        // The SAME value with the lower bound ON is refused as "improvement too large".
        let fast_two_sided = check(50.0, reference, 0.02, 0.05, true, "decode");
        assert!(!fast_two_sided.passed);
        assert!(fast_two_sided.reason.contains("improvement too large"));
        // A genuine slowdown still fails the UP bound even with the lower bound disabled.
        let slow = check(102.1, reference, 0.02, 0.05, false, "decode");
        assert!(!slow.passed);
        assert!(slow.reason.contains("slowdown/regression"));
    }

    #[test]
    fn band_nonfinite_value_fails() {
        let r = check(f64::NAN, 100.0, 0.05, 0.05, true, "prefill");
        assert!(!r.passed);
        assert!(r.reason.contains("must be finite and positive"));
    }

    #[test]
    fn robust_reference_drops_slowest() {
        // slowest (10.0) dropped, average of {1,2,3} = 2.0.
        assert_eq!(robust_reference(&[1.0, 2.0, 3.0, 10.0]), Some(2.0));
    }

    #[test]
    fn robust_reference_too_few_samples() {
        assert_eq!(robust_reference(&[1.0, 2.0]), None);
    }

    #[test]
    fn robust_reference_rejects_nonpositive() {
        assert_eq!(robust_reference(&[1.0, 2.0, 0.0]), None);
        assert_eq!(robust_reference(&[1.0, 2.0, f64::NAN]), None);
    }

    /// Test-only bands: a symmetric prefill health gate and a two-sided decode band. Values are
    /// arbitrary; the captured bands live in `constants::OFFICIAL_BASELINE`. `decode_down_enabled`
    /// is `true` here so these generic evaluate_timed_run tests exercise the full two-sided gate.
    const TEST_BANDS: AcceptanceBands = AcceptanceBands {
        prefill_up_tolerance: 0.03,
        prefill_down_tolerance: 0.03,
        decode_up_tolerance: 0.01,
        decode_down_tolerance: 0.025,
        decode_down_enabled: true,
    };

    #[test]
    fn evaluate_timed_run_all_pass() {
        // decode & prefill at baseline -> speedups 1.0, in-band, floors pass, score 1.0.
        let e = evaluate_timed_run(
            0.1336139485703125,
            0.010605031949609375,
            0.1336139485703125,
            0.010605031949609375,
            TEST_BANDS,
            SpeedupFloors::DEFAULT,
        );
        assert!((e.score - 1.0).abs() < 1e-12);
        assert!(e.passes_floors);
        assert!(e.passes_acceptance_bands());
        assert!(e.has_finite_score());
        assert_eq!(e.first_failure_reason(), None);
    }

    #[test]
    fn evaluate_timed_run_floor_failure_reported() {
        // Candidate far slower on decode: speedup below floor, and above band.
        let e = evaluate_timed_run(
            1.0,
            0.010605031949609375,
            0.1336139485703125,
            0.010605031949609375,
            TEST_BANDS,
            SpeedupFloors::DEFAULT,
        );
        assert!(!e.passes_floors);
        let reason = e.first_failure_reason().unwrap();
        assert!(reason.starts_with("performance floor failed:"));
    }

    #[test]
    fn evaluate_timed_run_nonfinite_score_first() {
        let e = evaluate_timed_run(0.1, 0.2, 0.0, 0.2, TEST_BANDS, SpeedupFloors::DEFAULT);
        assert!(!e.has_finite_score());
        assert_eq!(
            e.first_failure_reason().as_deref(),
            Some("computed score was not finite")
        );
    }

    /// THE FLOORS ARE THE RUN'S OWN (David 2026-09-09, per-project fixture floors): the gate is
    /// decided against the floors the caller passed, the evaluation CARRIES them, and the failure
    /// message names them — no constant is consulted anywhere in between.
    ///
    /// REVERT-PROOF: put `SCORE_*_SPEEDUP_FLOOR` back into `passes_speedup_floors` or into
    /// `first_failure_reason` and the 0.90 arms below go red.
    #[test]
    fn evaluate_timed_run_enforces_the_floors_it_is_given() {
        // A decode speedup of exactly 0.949 against the ruled 0.95 floor: refused, and the
        // message names 0.950000.
        let below = evaluate_timed_run(1.0, 1.0, 0.949, 1.0, TEST_BANDS, SpeedupFloors::DEFAULT);
        assert!(!below.passes_floors);
        assert!(below
            .first_failure_reason()
            .unwrap()
            .contains("decode_speedup=0.949000 floor=0.950000"));
        // Exactly AT the floor passes (>=, not >).
        let at = evaluate_timed_run(1.0, 1.0, 0.95, 1.0, TEST_BANDS, SpeedupFloors::DEFAULT);
        assert!(at.passes_floors);
        // The SAME 0.949 decode passes a project whose fixture declares 0.90 — the floors are the
        // fixture's, not the constants'.
        let looser = SpeedupFloors {
            decode: 0.90,
            prefill: 0.90,
        };
        let e = evaluate_timed_run(1.0, 1.0, 0.949, 1.0, TEST_BANDS, looser);
        assert!(e.passes_floors);
        assert_eq!(e.floors, looser);
        // Prefill is gated on its own axis, against its own floor.
        let prefill_below =
            evaluate_timed_run(1.0, 1.0, 1.0, 0.949, TEST_BANDS, SpeedupFloors::DEFAULT);
        assert!(!prefill_below.passes_floors);
        assert!(prefill_below
            .first_failure_reason()
            .unwrap()
            .contains("prefill_speedup=0.949000 floor=0.950000"));
        assert!(evaluate_timed_run(1.0, 1.0, 1.0, 0.949, TEST_BANDS, looser).passes_floors);
    }

    /// The no-contract default is the ruled pair, and it is the ONLY place the constants enter.
    #[test]
    fn default_floors_are_the_ruled_pair() {
        assert_eq!(SpeedupFloors::DEFAULT.decode, 0.95);
        assert_eq!(SpeedupFloors::DEFAULT.prefill, 0.95);
    }
}
