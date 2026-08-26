//! Property tests for the score / floor / acceptance-band pure functions.
//!
//! Ports the invariants the Swift example-based tests demonstrate
//! (`Tests/MLXFastTests/ScoreTests.swift`, `Tests/MLXFastTests/AcceptanceBandTests.swift`)
//! into generalized `proptest` cases against the production code in
//! `bench-core::score` (ported from `Sources/MLXFastCore/Score.swift` +
//! `Sources/MLXFastCore/AcceptanceBand.swift`). Closes debt item #45
//! (score monotonicity + acceptance-band symmetry).
//!
//! Constants come from `bench_core::constants` (the same values the production
//! score path uses) rather than being hardcoded, so these tests track the code.

use bench_core::constants::{
    DECODE_BAND_DOWN_TOLERANCE, DECODE_BAND_UP_TOLERANCE, PREFILL_BAND_DOWN_TOLERANCE,
    PREFILL_BAND_UP_TOLERANCE, SCORE_DECODE_SPEEDUP_FLOOR, SCORE_DECODE_WEIGHT,
    SCORE_PREFILL_SPEEDUP_FLOOR, SCORE_PREFILL_WEIGHT,
};
use bench_core::score::{
    check, passes_speedup_floors, robust_reference, score, score_default_weights, speedup,
};
use proptest::prelude::*;

/// Positive, finite, well-conditioned seconds-per-token / speedup magnitudes.
/// Bounded away from 0 and from overflow so `powf` stays numerically clean.
fn pos() -> impl Strategy<Value = f64> {
    1e-4f64..1e4f64
}

proptest! {
    // ---- speedup (Score.swift:4-16) ----

    /// For finite positive inputs, speedup is exactly baseline/candidate and positive.
    #[test]
    fn speedup_is_ratio_and_positive(baseline in pos(), candidate in pos()) {
        let s = speedup(baseline, candidate);
        prop_assert!(s.is_finite() && s > 0.0);
        prop_assert_eq!(s, baseline / candidate);
    }

    /// Monotone in the candidate: a faster candidate (smaller seconds-per-token)
    /// never yields a smaller speedup. (Score.swift:4-16.)
    #[test]
    fn speedup_monotonic_in_candidate(baseline in pos(), c1 in pos(), c2 in pos()) {
        let (fast, slow) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };
        prop_assert!(speedup(baseline, fast) >= speedup(baseline, slow));
    }

    /// Guard: any non-finite or non-positive operand collapses to 0.
    /// (Score.swift:9-14 `guard ... else { return 0 }`.)
    #[test]
    fn speedup_guards_return_zero(x in pos()) {
        for bad in [0.0f64, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            prop_assert_eq!(speedup(bad, x), 0.0);
            prop_assert_eq!(speedup(x, bad), 0.0);
        }
    }

    // ---- score monotonicity / shape (Score.swift:18-48; issue #45) ----

    /// #45 monotonicity: holding prefill and both baselines fixed, a faster decode
    /// (smaller decode seconds-per-token => larger decode speedup) never lowers the
    /// score. (Score.swift:44-47 — score is increasing in each component speedup.)
    #[test]
    fn score_monotonic_in_decode(
        baseline_decode in pos(),
        baseline_prefill in pos(),
        prefill_spt in pos(),
        d1 in pos(),
        d2 in pos(),
    ) {
        let (fast, slow) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
        let s_fast = score_default_weights(fast, prefill_spt, baseline_decode, baseline_prefill);
        let s_slow = score_default_weights(slow, prefill_spt, baseline_decode, baseline_prefill);
        // Relative slack absorbs benign float noise; the direction is what matters.
        prop_assert!(s_fast >= s_slow * (1.0 - 1e-12));
    }

    /// #45 monotonicity, prefill axis: symmetric statement for the prefill component.
    #[test]
    fn score_monotonic_in_prefill(
        baseline_decode in pos(),
        baseline_prefill in pos(),
        decode_spt in pos(),
        p1 in pos(),
        p2 in pos(),
    ) {
        let (fast, slow) = if p1 <= p2 { (p1, p2) } else { (p2, p1) };
        let s_fast = score_default_weights(decode_spt, fast, baseline_decode, baseline_prefill);
        let s_slow = score_default_weights(decode_spt, slow, baseline_decode, baseline_prefill);
        prop_assert!(s_fast >= s_slow * (1.0 - 1e-12));
    }

    /// Weighted-geometric-mean shape: the score always lies between the two component
    /// speedups. (Score.swift:44-47.)
    #[test]
    fn score_between_component_speedups(
        decode_spt in pos(),
        prefill_spt in pos(),
        baseline_decode in pos(),
        baseline_prefill in pos(),
    ) {
        let ds = speedup(baseline_decode, decode_spt);
        let ps = speedup(baseline_prefill, prefill_spt);
        let s = score_default_weights(decode_spt, prefill_spt, baseline_decode, baseline_prefill);
        let lo = ds.min(ps);
        let hi = ds.max(ps);
        prop_assert!(s >= lo * (1.0 - 1e-9) && s <= hi * (1.0 + 1e-9));
    }

    /// Equal component speedups collapse the weighted geomean to that speedup.
    /// (Mirrors ScoreTests.swift:5-27; Score.swift:44-47.)
    #[test]
    fn score_equal_speedups_equals_that_speedup(
        decode_spt in pos(),
        prefill_spt in pos(),
        k in 0.1f64..10.0f64,
    ) {
        // baseline = k * candidate on both axes => both speedups == k.
        let s = score_default_weights(decode_spt, prefill_spt, k * decode_spt, k * prefill_spt);
        prop_assert!((s - k).abs() <= k * 1e-9);
    }

    /// Exponent-weighting shape: when prefill is neutral (speedup 1), the score is the
    /// decode speedup raised to the decode weight share. With the default 0.75/0.25
    /// weights this is `decode_speedup^0.75`. (Mirrors ScoreTests.swift:5-27 `2^0.75`;
    /// Score.swift:44-47.)
    #[test]
    fn score_prefill_neutral_is_decode_speedup_pow_weight(
        decode_spt in pos(),
        baseline_decode in pos(),
        prefill_spt in pos(),
    ) {
        // baseline_prefill == prefill_spt => prefill speedup exactly 1.
        let s = score_default_weights(decode_spt, prefill_spt, baseline_decode, prefill_spt);
        let ds = speedup(baseline_decode, decode_spt);
        let total = SCORE_DECODE_WEIGHT + SCORE_PREFILL_WEIGHT;
        let expected = ds.powf(SCORE_DECODE_WEIGHT / total);
        prop_assert!((s - expected).abs() <= expected * 1e-9);
    }

    /// Guard: a non-positive or non-finite timing / baseline yields NaN.
    /// (Score.swift:36-42 `guard ... else { return .nan }`; ScoreTests.swift:63-70.)
    #[test]
    fn score_rejects_nonpositive_and_nonfinite_timings(
        decode_spt in pos(),
        prefill_spt in pos(),
        baseline_decode in pos(),
        baseline_prefill in pos(),
    ) {
        for bad in [0.0f64, -1.0, f64::NAN, f64::INFINITY] {
            prop_assert!(
                score_default_weights(bad, prefill_spt, baseline_decode, baseline_prefill).is_nan()
            );
            prop_assert!(
                score_default_weights(decode_spt, bad, baseline_decode, baseline_prefill).is_nan()
            );
            // A non-positive baseline zeroes the speedup, which the score rejects as NaN.
            prop_assert!(
                score_default_weights(decode_spt, prefill_spt, bad, baseline_prefill).is_nan()
            );
        }
    }

    /// Guard: a negative or non-finite weight yields NaN. (Score.swift:36-42.)
    #[test]
    fn score_rejects_bad_weights(
        decode_spt in pos(),
        prefill_spt in pos(),
        baseline_decode in pos(),
        baseline_prefill in pos(),
    ) {
        for bad in [-0.1f64, f64::NAN, f64::INFINITY] {
            prop_assert!(
                score(decode_spt, prefill_spt, baseline_decode, baseline_prefill, bad, 0.25).is_nan()
            );
        }
        // Weights summing to zero is also rejected.
        prop_assert!(
            score(decode_spt, prefill_spt, baseline_decode, baseline_prefill, 0.0, 0.0).is_nan()
        );
    }

    // ---- speedup floors (Score.swift:50-64; ScoreTests.swift:29-34) ----

    /// Floors pass iff BOTH speedups clear their (finite) floor. (Score.swift:63.)
    #[test]
    fn floors_pass_iff_both_meet(ds in pos(), ps in pos()) {
        let expected = ds >= SCORE_DECODE_SPEEDUP_FLOOR && ps >= SCORE_PREFILL_SPEEDUP_FLOOR;
        prop_assert_eq!(
            passes_speedup_floors(ds, ps, SCORE_DECODE_SPEEDUP_FLOOR, SCORE_PREFILL_SPEEDUP_FLOOR),
            expected
        );
    }

    /// Boundary: exactly at the floor passes (>=, inclusive). (Score.swift:63.)
    #[test]
    fn floors_boundary_inclusive(headroom in 0.0f64..1.0f64) {
        let d = SCORE_DECODE_SPEEDUP_FLOOR + headroom;
        let p = SCORE_PREFILL_SPEEDUP_FLOOR + headroom;
        prop_assert!(passes_speedup_floors(
            d, p, SCORE_DECODE_SPEEDUP_FLOOR, SCORE_PREFILL_SPEEDUP_FLOOR
        ));
        prop_assert!(passes_speedup_floors(
            SCORE_DECODE_SPEEDUP_FLOOR, SCORE_PREFILL_SPEEDUP_FLOOR,
            SCORE_DECODE_SPEEDUP_FLOOR, SCORE_PREFILL_SPEEDUP_FLOOR
        ));
    }

    /// A non-finite speedup can never pass the floors. (Score.swift:55-60.)
    #[test]
    fn floors_nonfinite_never_pass(ok in pos()) {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            prop_assert!(!passes_speedup_floors(
                bad, ok, SCORE_DECODE_SPEEDUP_FLOOR, SCORE_PREFILL_SPEEDUP_FLOOR
            ));
            prop_assert!(!passes_speedup_floors(
                ok, bad, SCORE_DECODE_SPEEDUP_FLOOR, SCORE_PREFILL_SPEEDUP_FLOOR
            ));
        }
    }

    // ---- acceptance band (AcceptanceBand.swift:35-67) ----

    /// A measurement passes iff it lands inside `[B*(1-down), B*(1+up)]`, inclusive.
    /// Recomputes the bounds with the same float ops the production code uses, so the
    /// equivalence is exact. (AcceptanceBand.swift:49-66.)
    #[test]
    fn band_passes_iff_within_inclusive(
        reference in pos(),
        value in pos(),
        up in 0.0f64..0.5f64,
        down in 0.0f64..0.9f64,
    ) {
        let hi = reference * (1.0 + up);
        let lo = reference * (1.0 - down);
        let expected = value <= hi && value >= lo;
        prop_assert_eq!(check(value, reference, up, down, "x").passed, expected);
    }

    /// #45 band symmetry: with equal up/down tolerances (the prefill ±5% health gate),
    /// a symmetric offset accepts or rejects identically on both sides of B. Here,
    /// strictly inside the band, both the slow and the fast side pass.
    /// (AcceptanceBand.swift:49-65; AcceptanceBandTests.swift:42-65.)
    #[test]
    fn band_symmetric_within(reference in pos(), tol in 1e-4f64..0.5f64, frac in 0.0f64..0.95f64) {
        let delta = frac * tol; // strictly inside the tolerance
        let slow = check(reference * (1.0 + delta), reference, tol, tol, "x");
        let fast = check(reference * (1.0 - delta), reference, tol, tol, "x");
        prop_assert!(slow.passed);
        prop_assert!(fast.passed);
        prop_assert_eq!(slow.passed, fast.passed);
    }

    /// #45 band symmetry, other side: strictly outside an equal-tolerance band, both
    /// the slow and the fast side fail — the slow side as a slowdown, the fast side as
    /// too-large a gain. (AcceptanceBand.swift:51-65; AcceptanceBandTests.swift:50-65.)
    #[test]
    fn band_symmetric_outside(reference in pos(), tol in 1e-4f64..0.5f64, frac in 1.05f64..3.0f64) {
        let delta = frac * tol; // strictly outside the tolerance
        let slow = check(reference * (1.0 + delta), reference, tol, tol, "x");
        let fast = check(reference * (1.0 - delta.min(0.99)), reference, tol, tol, "x");
        prop_assert!(!slow.passed);
        prop_assert!(slow.reason.contains("slowdown"));
        // The fast side fails only while it stays a valid positive value (delta < 1).
        if delta < 1.0 {
            prop_assert!(!fast.passed);
            prop_assert!(fast.reason.contains("chunk"));
        }
    }

    /// The production prefill band really is symmetric (up == down); the decode band
    /// really is asymmetric (up != down). Anchored on the shared constants so the test
    /// tracks any retune. (Constants.swift:103-106.)
    #[test]
    fn prefill_band_symmetric_decode_band_asymmetric(_ in 0..1u8) {
        prop_assert_eq!(PREFILL_BAND_UP_TOLERANCE, PREFILL_BAND_DOWN_TOLERANCE);
        prop_assert_ne!(DECODE_BAND_UP_TOLERANCE, DECODE_BAND_DOWN_TOLERANCE);
    }

    /// Non-finite / non-positive value or reference is always rejected.
    /// (AcceptanceBand.swift:43-48; AcceptanceBandTests.swift:100-105.)
    #[test]
    fn band_rejects_nonfinite_or_nonpositive(ok in pos(), up in 0.0f64..0.5f64, down in 0.0f64..0.5f64) {
        for bad in [0.0f64, -1.0, f64::NAN, f64::INFINITY] {
            prop_assert!(!check(bad, ok, up, down, "x").passed);
            prop_assert!(!check(ok, bad, up, down, "x").passed);
        }
    }

    // ---- robust reference (AcceptanceBand.swift:20-33; AcceptanceBandTests.swift:17-30) ----

    /// With >= 3 finite positive samples the reference is Some, lands within
    /// [min, max], and equals the mean after dropping one (max) sample.
    /// (AcceptanceBand.swift:23-33.)
    #[test]
    fn robust_reference_drops_max_and_bounded(samples in prop::collection::vec(pos(), 3..12)) {
        let r = robust_reference(&samples).expect("finite positive samples yield Some");
        let max = samples.iter().cloned().fold(f64::MIN, f64::max);
        let min = samples.iter().cloned().fold(f64::MAX, f64::min);
        prop_assert!(r >= min * (1.0 - 1e-9) && r <= max * (1.0 + 1e-9));
        let sum: f64 = samples.iter().sum();
        let expected = (sum - max) / (samples.len() as f64 - 1.0);
        prop_assert!((r - expected).abs() <= expected.abs() * 1e-9 + 1e-12);
    }

    /// Fewer than 3 samples => None. (AcceptanceBand.swift:25.)
    #[test]
    fn robust_reference_too_few_is_none(samples in prop::collection::vec(pos(), 0..3)) {
        prop_assert_eq!(robust_reference(&samples), None);
    }

    /// Any non-finite or non-positive sample poisons the whole set => None.
    /// (AcceptanceBand.swift:25; AcceptanceBandTests.swift:26-30.)
    #[test]
    fn robust_reference_rejects_invalid_sample(
        mut samples in prop::collection::vec(pos(), 3..8),
        idx in 0usize..8,
        bad in prop::sample::select(vec![0.0f64, -1.0, f64::NAN, f64::INFINITY]),
    ) {
        let i = idx % samples.len();
        samples[i] = bad;
        prop_assert_eq!(robust_reference(&samples), None);
    }
}
