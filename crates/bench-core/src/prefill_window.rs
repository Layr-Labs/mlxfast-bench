//! Prefill-window CERTIFICATION for the free-run timed window.
//!
//! benchd already owns the only clock that scores anything: it opens a window immediately before
//! it sends `free_decode_begin` and closes it when `free_decode_run(N)` returns. Inside that one
//! window the engine does two different things — the seed prefill, then the free-run decode — and
//! the boundary between them is a message boundary benchd can see. So benchd SPLITS ITS OWN CLOCK
//! at that boundary. No new message, no new wire field, no engine-supplied duration.
//!
//! A track that scores prefill needs that split to be trustworthy. A track that does NOT score
//! prefill needs nothing from it at all — the number is a diagnostic. That is the whole rule this
//! module implements: certification is ARMED by the track's declared regime
//! ([`crate::constants::ScoredRegime::prefill_is_scored`]) and by nothing else.
//!
//! * ARMED (`prefill_gain_exponent != 0.0`) — the window MUST be observed and measurable, its two
//!   halves MUST account for the whole window, and any independently reported prefill duration
//!   MUST agree with benchd's own within [`PREFILL_WINDOW_TOLERANCE`]. Each breach refuses BY
//!   NAME, quoting an exact-match sentinel.
//! * NOT ARMED (`prefill_gain_exponent == 0.0`) — the window is REPORT-ONLY. It is carried into the
//!   record when it exists and nothing is enforced on it. No refusal in this module can fire.
//!
//! The second half is why `qwen3.8-27b-mtp-v1` is untouched: it scores decode-only, so its
//! declared prefill exponent is `0.0`, so certification is never armed on it.

use crate::constants::ScoredRegime;

/// The pinned RELATIVE tolerance the certification holds two measurements of the same duration to.
///
/// It covers the arithmetic a split introduces (two `f64` sub-intervals summed against one whole
/// interval measured separately) and ordinary clock jitter between two readings — it is NOT a
/// performance band. A disagreement wider than this is an accounting fault: the halves describe a
/// different window than the whole, or the reported duration describes a different window than the
/// one benchd timed.
pub const PREFILL_WINDOW_TOLERANCE: f64 = 0.05;

/// EXACT-MATCH name of the refusal "an armed track produced no usable prefill window".
pub const PREFILL_WINDOW_NOT_OBSERVED: &str = "PREFILL-WINDOW-NOT-OBSERVED";

/// EXACT-MATCH name of the refusal "two measurements of the same prefill window disagree beyond
/// [`PREFILL_WINDOW_TOLERANCE`]".
pub const PREFILL_WINDOW_DISAGREES: &str = "PREFILL-WINDOW-DISAGREES";

/// Sealed cross-check state: NO second measurement of the prefill window was observed, so benchd's
/// own split was the only source. This is the state EVERY run is in today — no message on this
/// protocol carries a duration — and it is sealed as a NAME rather than left to be inferred from a
/// `false`, because "we checked and found nothing to compare" and "there was nothing to check" are
/// different claims and only one of them is true.
pub const PREFILL_WINDOW_CROSS_CHECK_NOT_OBSERVED: &str = "not-observed";

/// Sealed cross-check state: a second measurement WAS observed and agreed with benchd's own split
/// inside [`PREFILL_WINDOW_TOLERANCE`]. (A disagreement never seals — it refuses under
/// [`PREFILL_WINDOW_DISAGREES`] — so there is no third sealed state.)
pub const PREFILL_WINDOW_CROSS_CHECK_AGREED: &str = "agreed";

/// The split of ONE free-run timed window, all four numbers from benchd's own clock and its own
/// configured token counts. Nothing here is engine-reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseWindow {
    /// The SEED-PREFILL half of the free-run window. Opens immediately before `free_decode_begin`
    /// is sent; closes immediately after the response `seed_token` is validated against the golden.
    /// The seed oracle check is charged here.
    ///
    /// NAMED `seed_` deliberately. `bench_runner::FreeRunTimingResult` carries its own
    /// `prefill_elapsed_seconds`, which is a DIFFERENT quantity — the v1 prefill PHASE, a separate
    /// timed round trip on its own prompt, outside this window entirely. Two fields spelled the
    /// same on one result type would be one substitution away from a wrong gain.
    pub seed_prefill_elapsed_seconds: f64,
    /// Opens the instant the prefill window closes; closes when `free_decode_run(N)` returns.
    pub decode_elapsed_seconds: f64,
    /// The whole window, measured ONCE end to end — not the sum of the two halves. This is the
    /// number `seconds_per_token` divides, and it is unchanged by the split.
    pub whole_window_elapsed_seconds: f64,
    /// Tokens the prefill window covered (the seed length).
    pub prefill_token_total: usize,
    /// Tokens the decode window covered (N).
    pub decode_token_total: usize,
}

/// What certification concluded about one leg's window.
///
/// TWO facts, never one. "The checks ran and passed" and "a second measurement confirmed benchd's
/// clock" are independent, and today the second is ALWAYS absent. Collapsing them into one boolean
/// would let a self-consistency pass read as a corroborated one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrefillWindowCertification {
    /// The regime gives prefill zero weight. The window (when there is one) is carried for the
    /// record and NOTHING is enforced on it.
    ReportOnly(Option<PhaseWindow>),
    /// The regime scores prefill and the window passed every check that could be run.
    Certified {
        window: PhaseWindow,
        /// [`PREFILL_WINDOW_CROSS_CHECK_NOT_OBSERVED`] or [`PREFILL_WINDOW_CROSS_CHECK_AGREED`].
        /// The former means the certification rests on benchd's own split ALONE — self-consistent
        /// (the halves account for the whole window), not corroborated.
        cross_check: &'static str,
    },
}

impl PrefillWindowCertification {
    /// The window this certification carries, if any. `None` only for a report-only leg that
    /// produced no window (a teacher-forced leg, which opens no free-run verbs).
    pub fn window(&self) -> Option<PhaseWindow> {
        match self {
            PrefillWindowCertification::ReportOnly(w) => *w,
            PrefillWindowCertification::Certified { window, .. } => Some(*window),
        }
    }

    /// True when the window was actually held to the certification checks.
    pub fn is_certified(&self) -> bool {
        matches!(self, PrefillWindowCertification::Certified { .. })
    }

    /// The sealed cross-check state. A REPORT-ONLY leg is
    /// [`PREFILL_WINDOW_CROSS_CHECK_NOT_OBSERVED`] too: nothing was compared, because nothing was
    /// checked at all.
    pub fn cross_check(&self) -> &'static str {
        match self {
            PrefillWindowCertification::ReportOnly(_) => PREFILL_WINDOW_CROSS_CHECK_NOT_OBSERVED,
            PrefillWindowCertification::Certified { cross_check, .. } => cross_check,
        }
    }
}

/// The relative gap between two measurements of the same duration, `|a - b| / max(|a|, |b|)`.
/// Returns `f64::INFINITY` when either side is non-finite or both are zero — a gap no tolerance
/// admits, so a degenerate input never reads as agreement.
fn relative_gap(a: f64, b: f64) -> f64 {
    if !a.is_finite() || !b.is_finite() {
        return f64::INFINITY;
    }
    let scale = a.abs().max(b.abs());
    if scale == 0.0 {
        return f64::INFINITY;
    }
    (a - b).abs() / scale
}

/// Certify one leg's prefill window against the track's DECLARED regime.
///
/// `observed` is benchd's own split of its own clock (`None` on a leg that opened no free-run
/// window). `reported_prefill_seconds` is an INDEPENDENTLY reported duration for the same prefill
/// window, when a source for one exists; it is cross-checked against benchd's measurement and is
/// never a substitute for it. Today's protocol carries no engine-reported duration on any message,
/// so every live call site passes `None` — the parameter is the seam a reported duration would
/// arrive through, and it is checked rather than trusted if it ever does.
///
/// NOT ARMED ⇒ always `Ok(ReportOnly(observed))`: this function cannot refuse a track whose
/// prefill exponent is `0.0`, whatever it is handed.
pub fn certify_prefill_window(
    track_id: &str,
    regime: &ScoredRegime,
    observed: Option<PhaseWindow>,
    reported_prefill_seconds: Option<f64>,
) -> Result<PrefillWindowCertification, String> {
    if !regime.prefill_is_scored() {
        return Ok(PrefillWindowCertification::ReportOnly(observed));
    }

    let window = observed.ok_or_else(|| {
        format!(
            "{PREFILL_WINDOW_NOT_OBSERVED}: track_id {track_id:?} scores prefill \
             (prefill_gain_exponent={}), but this leg produced no prefill window — benchd splits \
             its own clock at the free_decode_begin/free_decode_run boundary, and a leg that never \
             opened those verbs has no prefill duration to score; refusing",
            regime.prefill_gain_exponent
        )
    })?;

    let measurable = |v: f64| v.is_finite() && v > 0.0;
    if !measurable(window.seed_prefill_elapsed_seconds)
        || !measurable(window.decode_elapsed_seconds)
        || !measurable(window.whole_window_elapsed_seconds)
        || window.prefill_token_total == 0
        || window.decode_token_total == 0
    {
        return Err(format!(
            "{PREFILL_WINDOW_NOT_OBSERVED}: track_id {track_id:?} scores prefill \
             (prefill_gain_exponent={}), but the window is not measurable \
             (prefill={} s over {} tokens, decode={} s over {} tokens, whole={} s); every part must \
             be finite and positive; refusing",
            regime.prefill_gain_exponent,
            window.seed_prefill_elapsed_seconds,
            window.prefill_token_total,
            window.decode_elapsed_seconds,
            window.decode_token_total,
            window.whole_window_elapsed_seconds
        ));
    }

    // The two halves must account for the whole window benchd measured end to end. They are three
    // separate readings of one interval, so they agree to the tolerance, not exactly — but a split
    // that leaves an untimed gap (or overlaps) fails here rather than silently shrinking the half
    // the score divides by.
    let split_total = window.seed_prefill_elapsed_seconds + window.decode_elapsed_seconds;
    let split_gap = relative_gap(split_total, window.whole_window_elapsed_seconds);
    if split_gap > PREFILL_WINDOW_TOLERANCE {
        return Err(format!(
            "{PREFILL_WINDOW_DISAGREES}: track_id {track_id:?} — the prefill window ({} s) and the \
             decode window ({} s) sum to {split_total} s, which differs from the whole timed window \
             benchd measured ({} s) by {:.6} relative, beyond the pinned tolerance {:.6}; the two \
             halves do not describe the window that was timed; refusing",
            window.seed_prefill_elapsed_seconds,
            window.decode_elapsed_seconds,
            window.whole_window_elapsed_seconds,
            split_gap,
            PREFILL_WINDOW_TOLERANCE
        ));
    }

    let mut cross_check = PREFILL_WINDOW_CROSS_CHECK_NOT_OBSERVED;
    if let Some(reported) = reported_prefill_seconds {
        let gap = relative_gap(reported, window.seed_prefill_elapsed_seconds);
        if gap > PREFILL_WINDOW_TOLERANCE {
            return Err(format!(
                "{PREFILL_WINDOW_DISAGREES}: track_id {track_id:?} — the reported prefill duration \
                 ({reported} s) differs from the window benchd measured on its own clock ({} s) by \
                 {:.6} relative, beyond the pinned tolerance {:.6}; benchd's clock is the scored \
                 source and it will not seal a certified window a second measurement contradicts; \
                 refusing",
                window.seed_prefill_elapsed_seconds,
                gap,
                PREFILL_WINDOW_TOLERANCE
            ));
        }
        cross_check = PREFILL_WINDOW_CROSS_CHECK_AGREED;
    }

    Ok(PrefillWindowCertification::Certified {
        window,
        cross_check,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{scored_regime, TRACK_ID};

    /// A regime that SCORES prefill, so certification is armed. Not a declared track's regime —
    /// no track on this branch scores prefill — so it is built here for the armed cases.
    fn armed() -> ScoredRegime {
        ScoredRegime {
            scored_batch_size: 1,
            prefill_gain_exponent: 0.25,
            decode_gain_exponent: 0.75,
        }
    }

    fn good_window() -> PhaseWindow {
        PhaseWindow {
            seed_prefill_elapsed_seconds: 0.4,
            decode_elapsed_seconds: 4.6,
            whole_window_elapsed_seconds: 5.0,
            prefill_token_total: 512,
            decode_token_total: 128,
        }
    }

    /// THE INERTNESS PROOF for the track this branch serves. Its declared regime is decode-only,
    /// so certification is NOT armed and this function cannot refuse it — not for an absent window,
    /// not for a nonsense window, not for a contradicting reported duration.
    #[test]
    fn the_existing_track_is_report_only_and_no_refusal_can_fire() {
        let regime = scored_regime(TRACK_ID).unwrap();
        assert!(!regime.prefill_is_scored());

        // No window at all.
        assert_eq!(
            certify_prefill_window(TRACK_ID, &regime, None, None).unwrap(),
            PrefillWindowCertification::ReportOnly(None)
        );
        // A window, carried through untouched.
        assert_eq!(
            certify_prefill_window(TRACK_ID, &regime, Some(good_window()), None).unwrap(),
            PrefillWindowCertification::ReportOnly(Some(good_window()))
        );
        // Every input that WOULD refuse under an armed regime is accepted under this one.
        let nonsense = PhaseWindow {
            seed_prefill_elapsed_seconds: f64::NAN,
            decode_elapsed_seconds: -1.0,
            whole_window_elapsed_seconds: 0.0,
            prefill_token_total: 0,
            decode_token_total: 0,
        };
        for observed in [None, Some(good_window()), Some(nonsense)] {
            for reported in [None, Some(0.0), Some(999.0), Some(f64::NAN)] {
                let c = certify_prefill_window(TRACK_ID, &regime, observed, reported)
                    .expect("a decode-only track can never be refused by certification");
                assert!(
                    !c.is_certified(),
                    "report-only must not claim certification"
                );
                // Compared through `Debug` so the NaN case still proves "carried through
                // untouched" (`NaN != NaN` under `PartialEq`).
                assert_eq!(format!("{:?}", c.window()), format!("{observed:?}"));
            }
        }
    }

    /// RED-first: an ARMED regime with no observed window refuses, naming the sentinel and the
    /// track.
    #[test]
    fn armed_with_no_window_refuses_by_name() {
        let err = certify_prefill_window("some-armed-track-v1", &armed(), None, None).unwrap_err();
        assert!(err.contains(PREFILL_WINDOW_NOT_OBSERVED), "{err}");
        assert!(err.contains("some-armed-track-v1"), "{err}");
    }

    /// RED-first: an ARMED regime whose window is not measurable refuses under the same sentinel.
    #[test]
    fn armed_with_unmeasurable_window_refuses_by_name() {
        for broken in [
            PhaseWindow {
                seed_prefill_elapsed_seconds: 0.0,
                ..good_window()
            },
            PhaseWindow {
                seed_prefill_elapsed_seconds: f64::NAN,
                ..good_window()
            },
            PhaseWindow {
                decode_elapsed_seconds: -0.1,
                ..good_window()
            },
            PhaseWindow {
                whole_window_elapsed_seconds: f64::INFINITY,
                ..good_window()
            },
            PhaseWindow {
                prefill_token_total: 0,
                ..good_window()
            },
            PhaseWindow {
                decode_token_total: 0,
                ..good_window()
            },
        ] {
            let err = certify_prefill_window("armed-v1", &armed(), Some(broken), None).unwrap_err();
            assert!(
                err.contains(PREFILL_WINDOW_NOT_OBSERVED),
                "{broken:?} -> {err}"
            );
        }
    }

    /// RED-first: an ARMED regime whose two halves do not account for the whole window refuses.
    /// This is the shape that matters — a prefill half quietly smaller than the work it covers.
    #[test]
    fn armed_with_a_split_that_does_not_sum_refuses_by_name() {
        let leaky = PhaseWindow {
            seed_prefill_elapsed_seconds: 0.4,
            decode_elapsed_seconds: 2.0,
            whole_window_elapsed_seconds: 5.0,
            ..good_window()
        };
        let err = certify_prefill_window("armed-v1", &armed(), Some(leaky), None).unwrap_err();
        assert!(err.contains(PREFILL_WINDOW_DISAGREES), "{err}");
        assert!(err.contains("armed-v1"), "{err}");

        // Inside the tolerance the same shape passes: the halves are separate readings of one
        // interval, not an exact decomposition.
        let jittery = PhaseWindow {
            seed_prefill_elapsed_seconds: 0.4,
            decode_elapsed_seconds: 4.7,
            whole_window_elapsed_seconds: 5.0,
            ..good_window()
        };
        assert!(certify_prefill_window("armed-v1", &armed(), Some(jittery), None).is_ok());
    }

    /// RED-first: an ARMED regime whose independently reported prefill contradicts benchd's own
    /// measurement refuses, and agreement inside the tolerance certifies.
    #[test]
    fn armed_with_a_contradicting_reported_prefill_refuses_by_name() {
        let err = certify_prefill_window("armed-v1", &armed(), Some(good_window()), Some(0.2))
            .unwrap_err();
        assert!(err.contains(PREFILL_WINDOW_DISAGREES), "{err}");

        // A non-finite report is a disagreement, never an absent one.
        let err = certify_prefill_window("armed-v1", &armed(), Some(good_window()), Some(f64::NAN))
            .unwrap_err();
        assert!(err.contains(PREFILL_WINDOW_DISAGREES), "{err}");

        // Agreement inside the tolerance certifies, and seals that a second source AGREED.
        let c =
            certify_prefill_window("armed-v1", &armed(), Some(good_window()), Some(0.41)).unwrap();
        assert!(c.is_certified());
        assert_eq!(c.window(), Some(good_window()));
        assert_eq!(c.cross_check(), PREFILL_WINDOW_CROSS_CHECK_AGREED);
    }

    /// An ARMED pass with NO second source certifies, but seals `not-observed` — it rests on
    /// benchd's own split alone. The two facts are separate, and the record says which one it has.
    /// This is the state EVERY live call site is in: nothing on this protocol reports a duration.
    #[test]
    fn an_armed_pass_without_a_second_source_seals_not_observed_not_agreed() {
        let c = certify_prefill_window("armed-v1", &armed(), Some(good_window()), None).unwrap();
        assert!(
            c.is_certified(),
            "the checks that CAN run did run, and passed"
        );
        assert_eq!(
            c.cross_check(),
            PREFILL_WINDOW_CROSS_CHECK_NOT_OBSERVED,
            "no second measurement existed: self-consistent, NOT corroborated"
        );
        assert_ne!(c.cross_check(), PREFILL_WINDOW_CROSS_CHECK_AGREED);
    }

    /// A malformed declaration — a negative or non-finite prefill exponent — reads as ARMED, so it
    /// refuses rather than silently disarming the certification.
    #[test]
    fn a_malformed_prefill_exponent_arms_rather_than_disarms() {
        for exponent in [-0.25, f64::NAN] {
            let regime = ScoredRegime {
                prefill_gain_exponent: exponent,
                ..armed()
            };
            assert!(regime.prefill_is_scored(), "exponent {exponent} must arm");
            let err = certify_prefill_window("armed-v1", &regime, None, None).unwrap_err();
            assert!(err.contains(PREFILL_WINDOW_NOT_OBSERVED), "{err}");
        }
    }
}
