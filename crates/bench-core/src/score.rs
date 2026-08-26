//! Score formula, speedups, floors, and acceptance bands.
//!
//! Ported from Sources/MLXFastCore/Score.swift (`BenchmarkScore`, `TimedRunScoreEvaluation`)
//! and Sources/MLXFastCore/AcceptanceBand.swift (`AcceptanceBand`, `AcceptanceBandResult`).
//! The guard / NaN / zero semantics are preserved exactly.

use crate::constants::{
    DECODE_BAND_DOWN_TOLERANCE, DECODE_BAND_UP_TOLERANCE, PREFILL_BAND_DOWN_TOLERANCE,
    PREFILL_BAND_UP_TOLERANCE, SCORE_DECODE_SPEEDUP_FLOOR, SCORE_DECODE_WEIGHT,
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

/// `AcceptanceBand.check`: per-run two-sided band gate against a paired baseline.
pub fn check(
    value: f64,
    reference: f64,
    up_tolerance: f64,
    down_tolerance: f64,
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
    if value < lo {
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
                SCORE_DECODE_SPEEDUP_FLOOR,
                SCORE_PREFILL_SPEEDUP_FLOOR,
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
/// decode band uses the decode up/down tolerances (all from `constants`).
pub fn evaluate_timed_run(
    decode_spt: f64,
    prefill_spt: f64,
    baseline_decode_spt: f64,
    baseline_prefill_spt: f64,
) -> TimedRunScoreEvaluation {
    let s = score_default_weights(
        decode_spt,
        prefill_spt,
        baseline_decode_spt,
        baseline_prefill_spt,
    );
    let decode_speedup = speedup(baseline_decode_spt, decode_spt);
    let prefill_speedup = speedup(baseline_prefill_spt, prefill_spt);
    let prefill_band = check(
        prefill_spt,
        baseline_prefill_spt,
        PREFILL_BAND_UP_TOLERANCE,
        PREFILL_BAND_DOWN_TOLERANCE,
        "prefill",
    );
    let decode_band = check(
        decode_spt,
        baseline_decode_spt,
        DECODE_BAND_UP_TOLERANCE,
        DECODE_BAND_DOWN_TOLERANCE,
        "decode",
    );
    TimedRunScoreEvaluation {
        score: s,
        decode_speedup,
        prefill_speedup,
        passes_floors: passes_speedup_floors(
            decode_speedup,
            prefill_speedup,
            SCORE_DECODE_SPEEDUP_FLOOR,
            SCORE_PREFILL_SPEEDUP_FLOOR,
        ),
        prefill_band,
        decode_band,
    }
}

// ---------------------------------------------------------------------------
// qwen-mtp-paired-decode-only scoring (track qwen3.8-27b-mtp-v1)
// ---------------------------------------------------------------------------
//
// The authoritative paired score for the MTP spec-decode track. DECODE-ONLY, serial-anchored
// (serial control = 1.0, no normalization). This is ADDITIVE — it does NOT touch the generic
// `score()` / `evaluate_timed_run()` ds^0.75·ps^0.25 path above, which the non-paired official
// and local runs still use.

use crate::constants::{
    QWEN_MTP_DECODE_SPEEDUP_CEILING, QWEN_MTP_DECODE_SPEEDUP_FLOOR, QWEN_MTP_PER_PAIR_RATIO_BOUND,
};

/// The serial-denominator calibration band predicate: true iff `measured` lies within
/// `expected * (1 ± band_pct/100)` INCLUSIVE (`band_pct` is a percent, e.g. 2.0 ⇒ ±2%). Returns
/// FALSE on any non-finite input (fail-loud: a NaN/inf measurement or reference is never "in
/// band"). This is the primitive behind the authoritative spec's `serial_denominator_banding`,
/// which catches a box whose serial denominator drifted. Live single-box enforcement needs an
/// on-box serial calibration reference we do not have here, so the caller only ENFORCES this
/// when a reference is actually supplied.
// UNVERIFIED(measure-job): the live ranked enforcement + the calibration provenance are B-4(a).
pub fn within_calibration_band(measured: f64, expected: f64, band_pct: f64) -> bool {
    if !measured.is_finite() || !expected.is_finite() || !band_pct.is_finite() {
        return false;
    }
    let tol = expected.abs() * (band_pct / 100.0);
    measured >= expected - tol && measured <= expected + tol
}

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
        assert!(check(105.0, reference, 0.05, 0.05, "x").passed);
        assert!(check(95.0, reference, 0.05, 0.05, "x").passed);
        assert!(check(100.0, reference, 0.05, 0.05, "x").passed);
    }

    #[test]
    fn band_beyond_edges_fail() {
        let reference = 100.0;
        let above = check(105.0001, reference, 0.05, 0.05, "x");
        assert!(!above.passed);
        assert!(above.reason.contains("slowdown/regression"));
        let below = check(94.9999, reference, 0.05, 0.05, "x");
        assert!(!below.passed);
        assert!(below.reason.contains("improvement too large"));
    }

    #[test]
    fn band_nonfinite_value_fails() {
        let r = check(f64::NAN, 100.0, 0.05, 0.05, "prefill");
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

    #[test]
    fn evaluate_timed_run_all_pass() {
        // decode & prefill at baseline -> speedups 1.0, in-band, floors pass, score 1.0.
        let e = evaluate_timed_run(
            0.1336139485703125,
            0.010605031949609375,
            0.1336139485703125,
            0.010605031949609375,
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
        );
        assert!(!e.passes_floors);
        let reason = e.first_failure_reason().unwrap();
        assert!(reason.starts_with("performance floor failed:"));
    }

    #[test]
    fn evaluate_timed_run_nonfinite_score_first() {
        let e = evaluate_timed_run(0.1, 0.2, 0.0, 0.2);
        assert!(!e.has_finite_score());
        assert_eq!(
            e.first_failure_reason().as_deref(),
            Some("computed score was not finite")
        );
    }

    // --- qwen-mtp-paired-decode-only scoring ---

    #[test]
    fn paired_raw_ratio_is_serial_over_candidate() {
        // Candidate twice as fast on decode ⇒ ratio 2.0; equal ⇒ 1.0.
        assert_eq!(paired_decode_raw_ratio(0.2, 0.1), 2.0);
        assert_eq!(paired_decode_raw_ratio(0.038, 0.038), 1.0);
        // Non-finite / non-positive ⇒ 0 (an implausible/blank pair).
        assert_eq!(paired_decode_raw_ratio(0.038, 0.0), 0.0);
    }

    #[test]
    fn paired_median_odd_is_middle_even_is_mean_of_two_central() {
        // Odd n → middle order statistic.
        assert_eq!(paired_decode_only_median(&[0.9, 1.1, 1.0]), 1.0);
        // Even n → mean of the two central order statistics (NOT lower-median).
        assert_eq!(paired_decode_only_median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        // Single-prompt run → that one ratio.
        assert_eq!(paired_decode_only_median(&[1.234]), 1.234);
        // Unsorted input is ordered first.
        assert_eq!(paired_decode_only_median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
    }

    #[test]
    fn paired_identity_run_scores_one() {
        // candidate == baseline decode spt ⇒ raw ratio 1.0 ⇒ median 1.0 ⇒ passes 0.90/5.0 ⇒ 1.0.
        let r = paired_decode_raw_ratio(0.038, 0.038);
        let s = score_paired_decode_only(&[r], &[r]);
        assert!(s.passed);
        assert_eq!(s.score, Some(1.0));
        assert_eq!(s.raw_median, 1.0);
        assert!(s.failure.is_none());
    }

    #[test]
    fn paired_floor_fail_nulls_score_and_names_floor() {
        // Median 0.85 < 0.90 floor ⇒ null score, error names the 0.9 floor.
        let s = score_paired_decode_only(&[0.85], &[0.85]);
        assert!(!s.passed);
        assert_eq!(s.score, None);
        let msg = s.failure.as_ref().unwrap().message();
        assert!(msg.contains("floor"), "msg={msg}");
        assert!(msg.contains("0.9"), "floor message must name 0.90: {msg}");
    }

    #[test]
    fn paired_ceiling_fail_nulls_score_and_names_ceiling() {
        // Median 6.0 > 5.0 ceiling ⇒ null score, error names the 5.0 ceiling. (Per-pair bound is
        // 8.0, so a single 6.0 pair clears the per-pair check and reaches the median ceiling.)
        let s = score_paired_decode_only(&[6.0], &[6.0]);
        assert!(!s.passed);
        assert_eq!(s.score, None);
        let msg = s.failure.as_ref().unwrap().message();
        assert!(msg.contains("ceiling"), "msg={msg}");
        assert!(msg.contains('5'), "ceiling message must name 5.0: {msg}");
    }

    #[test]
    fn paired_per_pair_bound_rejects_before_aggregation() {
        // A single pair ratio 9.0 > 8.0 bound rejects the run (even though other pairs are sane).
        let s = score_paired_decode_only(&[1.0, 9.0], &[1.0, 9.0]);
        assert!(!s.passed);
        assert_eq!(s.score, None);
        let msg = s.failure.as_ref().unwrap().message();
        assert!(msg.contains("per-pair"), "msg={msg}");
        assert!(
            msg.contains('8'),
            "per-pair message must name the 8.0 bound: {msg}"
        );
        assert!(matches!(
            s.failure,
            Some(PairedDecodeFailure::PerPairBound { .. })
        ));
    }

    #[test]
    fn paired_floor_and_ceiling_edges_inclusive() {
        // Floor is inclusive (>= 0.90 passes) and ceiling inclusive (<= 5.0 passes).
        assert!(score_paired_decode_only(&[0.90], &[0.90]).passed);
        assert!(score_paired_decode_only(&[5.0], &[5.0]).passed);
        assert!(!score_paired_decode_only(&[0.8999], &[0.8999]).passed);
    }

    #[test]
    fn paired_zero_pair_ratio_rejects_as_per_pair_bound_not_floor() {
        // A 0.0 pair ratio (blank/implausible pair) must classify as PerPairBound (docs promise),
        // NOT Floor — even though the median 0.0 is also below the 0.90 floor. The per-pair guard
        // runs first and rejects the run before aggregation.
        let s = score_paired_decode_only(&[0.0], &[0.0]);
        assert!(!s.passed);
        assert_eq!(s.score, None);
        assert!(
            matches!(s.failure, Some(PairedDecodeFailure::PerPairBound { .. })),
            "0.0 pair ratio must reject as PerPairBound, got {:?}",
            s.failure
        );
        // A negative pair ratio is likewise PerPairBound (not Floor).
        assert!(matches!(
            score_paired_decode_only(&[-1.0], &[-1.0]).failure,
            Some(PairedDecodeFailure::PerPairBound { .. })
        ));
    }

    #[test]
    fn calibration_band_inside_edge_outside_and_nonfinite() {
        // The serial-denominator band around the stock-tree expected raw median, ±2%.
        let e = crate::constants::QWEN_MTP_EXPECTED_RAW_MEDIAN;
        let b = crate::constants::QWEN_MTP_CALIBRATION_BAND_PCT;
        let tol = e * (b / 100.0);
        // Inside.
        assert!(within_calibration_band(e, e, b));
        assert!(within_calibration_band(1.00, e, b));
        // Edges are INCLUSIVE.
        assert!(within_calibration_band(e - tol, e, b));
        assert!(within_calibration_band(e + tol, e, b));
        // Just outside either edge.
        assert!(!within_calibration_band(e - tol * 1.01, e, b));
        assert!(!within_calibration_band(e + tol * 1.01, e, b));
        // Non-finite measured / expected / band ⇒ false (fail-loud, never "in band").
        assert!(!within_calibration_band(f64::NAN, e, b));
        assert!(!within_calibration_band(f64::INFINITY, e, b));
        assert!(!within_calibration_band(e, f64::NAN, b));
        assert!(!within_calibration_band(e, e, f64::INFINITY));
    }
}
