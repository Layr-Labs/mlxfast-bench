//! (b) admission — the PER-STREAM token-tolerance gate (David's blanket-10% ruling, 2026-08-25).
//!
//! This module is the PURE decision core of (b): given the candidate's committed tokens per stream
//! and the TRUSTED oracle's per-stream reference argmax, it decides PASS/REJECT under the ≤10%
//! per-stream rule ([`crate::constants::COHORT_TOKEN_TOLERANCE_PER_THOUSAND`]). It holds no I/O and
//! spawns nothing — the trusted-oracle spawn and the organizer-weights provenance live benchd-side;
//! here we only count argmax mismatches and apply the integer threshold.
//!
//! Correctness bar (maintainer note): "per stream, ≤10% token divergence from the trusted reference".
//! There is NO exact-correctness guarantee — a degraded model wrong on ≤10% of tokens PER STREAM can
//! pass and win on speed. This is a similar-output speedup bar, not lossless decoding. Concentration
//! gaming (all divergence in one stream) is closed because the rule is per-stream: any single stream
//! over 10% rejects the whole run.
//!
//! Two decisions live here and MUST NOT be conflated:
//!  * TOLERANCE ([`evaluate_cohort_token_tolerance`]): the ≤10% argmax-mismatch decision.
//!  * N2 INTEGRITY ([`verify_replay_echo_matches_committed`]): the oracle's echoed committed token
//!    must equal the candidate's own journal byte/id BEFORE any counting — a divergence means the
//!    oracle replayed a DIFFERENT journal, a HARD integrity error, never a tolerance outcome.

use std::fmt;

/// One stream's tolerance tally: how many of its committed tokens diverged from the reference argmax,
/// over how many committed tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamToleranceCount {
    /// The cohort slot (SLOT ORDER) this tally describes.
    pub slot: usize,
    /// The number of positions where `committed != reference_argmax`.
    pub mismatches: usize,
    /// The stream's committed-token count (the per-stream denominator; never 0 — an empty stream is
    /// a structural error, not a tally).
    pub committed_len: usize,
}

impl StreamToleranceCount {
    /// This stream FAILS iff `mismatches * 1000 > tolerance_per_thousand * committed_len` — STRICT
    /// `>`, so exactly the threshold (e.g. exactly 10%) PASSES. Pure integer arithmetic in `u64` to
    /// avoid overflow on long streams and any float rounding at the boundary.
    pub fn fails(&self, tolerance_per_thousand: u32) -> bool {
        (self.mismatches as u64) * 1000
            > (tolerance_per_thousand as u64) * (self.committed_len as u64)
    }
}

/// The gate verdict: the per-stream tallies plus the FIRST failing stream (slot order), if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CohortToleranceVerdict {
    /// The threshold applied (tokens-per-thousand), sealed for the diagnostic.
    pub tolerance_per_thousand: u32,
    /// One tally per stream, in SLOT ORDER.
    pub per_stream: Vec<StreamToleranceCount>,
    /// The FIRST stream (slot order) that exceeded the threshold. `None` ⇒ the run PASSES.
    pub first_failing: Option<StreamToleranceCount>,
}

impl CohortToleranceVerdict {
    /// PASS iff NO stream exceeded the threshold.
    pub fn passed(&self) -> bool {
        self.first_failing.is_none()
    }
}

/// STRUCTURAL faults in the tolerance inputs — NOT a tolerance decision (a shape the gate cannot
/// evaluate), surfaced distinctly so a caller never files them as a pass or a tolerance reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CohortToleranceError {
    /// The committed and reference-argmax stream counts differ.
    StreamCountMismatch { committed: usize, reference: usize },
    /// A stream's committed and reference-argmax token counts differ (the oracle did not report one
    /// reference argmax per committed position).
    StreamLengthMismatch {
        slot: usize,
        committed: usize,
        reference: usize,
    },
    /// A stream has zero committed tokens (denominator 0). A hard structural error — NEVER a pass.
    EmptyStream { slot: usize },
}

impl fmt::Display for CohortToleranceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CohortToleranceError::StreamCountMismatch {
                committed,
                reference,
            } => write!(
                f,
                "cohort tolerance: committed carries {committed} streams but the reference argmax \
                 carries {reference} (structural mismatch, not a tolerance decision)"
            ),
            CohortToleranceError::StreamLengthMismatch {
                slot,
                committed,
                reference,
            } => write!(
                f,
                "cohort tolerance: stream {slot} committed {committed} tokens but the reference \
                 argmax has {reference} (structural mismatch, not a tolerance decision)"
            ),
            CohortToleranceError::EmptyStream { slot } => write!(
                f,
                "cohort tolerance: stream {slot} has zero committed tokens (empty stream is a \
                 structural error, never a pass)"
            ),
        }
    }
}

impl std::error::Error for CohortToleranceError {}

/// N2 — the oracle's echoed committed token diverged from the candidate's own committed journal: the
/// oracle replayed a DIFFERENT journal than the candidate committed. A HARD integrity error, distinct
/// from any tolerance decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CohortReplayIntegrityError {
    /// The candidate and echoed stream counts differ.
    StreamCountMismatch { committed: usize, echoed: usize },
    /// A stream's candidate and echoed token counts differ.
    StreamLengthMismatch {
        slot: usize,
        committed: usize,
        echoed: usize,
    },
    /// A specific position's echoed token differs from the candidate's committed token — the oracle
    /// replayed a different token at this slot × position.
    TokenMismatch {
        slot: usize,
        position: usize,
        committed: i64,
        echoed: i64,
    },
}

impl fmt::Display for CohortReplayIntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CohortReplayIntegrityError::StreamCountMismatch { committed, echoed } => write!(
                f,
                "cohort replay integrity (N2): candidate committed {committed} streams but the \
                 oracle echoed {echoed} — the oracle replayed a different journal (hard integrity \
                 error, not a tolerance decision)"
            ),
            CohortReplayIntegrityError::StreamLengthMismatch {
                slot,
                committed,
                echoed,
            } => write!(
                f,
                "cohort replay integrity (N2): stream {slot} committed {committed} tokens but the \
                 oracle echoed {echoed} — the oracle replayed a different journal (hard integrity \
                 error)"
            ),
            CohortReplayIntegrityError::TokenMismatch {
                slot,
                position,
                committed,
                echoed,
            } => write!(
                f,
                "cohort replay integrity (N2): stream {slot} position {position} committed token \
                 {committed} but the oracle echoed {echoed} — the oracle replayed a different token \
                 (hard integrity error, not a tolerance decision)"
            ),
        }
    }
}

impl std::error::Error for CohortReplayIntegrityError {}

/// N2 INTEGRITY PRECONDITION — verify the oracle report's ECHOED committed tokens EQUAL the
/// candidate's own committed journal, per stream × position (byte/id equality), BEFORE any tolerance
/// counting runs. This proves the reference argmax was produced by replaying the candidate's REAL
/// journal; a mismatch is a hard integrity error, never a tolerance reject.
///
/// `committed_by_stream` is the candidate's own `tokens_by_stream`; `echoed_committed_by_stream` is
/// the oracle's echoed `committed_token` per stream × position (SLOT ORDER on both).
pub fn verify_replay_echo_matches_committed(
    committed_by_stream: &[Vec<i64>],
    echoed_committed_by_stream: &[Vec<i64>],
) -> Result<(), CohortReplayIntegrityError> {
    if committed_by_stream.len() != echoed_committed_by_stream.len() {
        return Err(CohortReplayIntegrityError::StreamCountMismatch {
            committed: committed_by_stream.len(),
            echoed: echoed_committed_by_stream.len(),
        });
    }
    for (slot, (committed, echoed)) in committed_by_stream
        .iter()
        .zip(echoed_committed_by_stream.iter())
        .enumerate()
    {
        if committed.len() != echoed.len() {
            return Err(CohortReplayIntegrityError::StreamLengthMismatch {
                slot,
                committed: committed.len(),
                echoed: echoed.len(),
            });
        }
        for (position, (&c, &e)) in committed.iter().zip(echoed.iter()).enumerate() {
            if c != e {
                return Err(CohortReplayIntegrityError::TokenMismatch {
                    slot,
                    position,
                    committed: c,
                    echoed: e,
                });
            }
        }
    }
    Ok(())
}

/// The PURE per-stream tolerance decision. For each stream: count the positions where the committed
/// token differs from the reference argmax; the stream FAILS iff that count exceeds
/// `tolerance_per_thousand` of its committed-token count (STRICT `>`; exactly the threshold passes).
/// The run PASSES iff NO stream fails; the FIRST failing stream (slot order) is carried for the
/// diagnostic.
///
/// `committed_by_stream[s]` and `reference_argmax_by_stream[s]` must be the SAME length (the oracle
/// reports one reference argmax per committed position) and non-empty; a shape violation is a
/// [`CohortToleranceError`] (structural), never folded into a pass or a tolerance reject.
///
/// N2 IS THE CALLER'S RESPONSIBILITY FIRST: the caller MUST have already run
/// [`verify_replay_echo_matches_committed`] so `committed_by_stream` is proven to be the journal the
/// oracle actually replayed. This function judges argmax mismatches only.
pub fn evaluate_cohort_token_tolerance(
    committed_by_stream: &[Vec<i64>],
    reference_argmax_by_stream: &[Vec<i64>],
    tolerance_per_thousand: u32,
) -> Result<CohortToleranceVerdict, CohortToleranceError> {
    if committed_by_stream.len() != reference_argmax_by_stream.len() {
        return Err(CohortToleranceError::StreamCountMismatch {
            committed: committed_by_stream.len(),
            reference: reference_argmax_by_stream.len(),
        });
    }
    let mut per_stream = Vec::with_capacity(committed_by_stream.len());
    let mut first_failing = None;
    for (slot, (committed, reference)) in committed_by_stream
        .iter()
        .zip(reference_argmax_by_stream.iter())
        .enumerate()
    {
        if committed.len() != reference.len() {
            return Err(CohortToleranceError::StreamLengthMismatch {
                slot,
                committed: committed.len(),
                reference: reference.len(),
            });
        }
        let committed_len = committed.len();
        if committed_len == 0 {
            return Err(CohortToleranceError::EmptyStream { slot });
        }
        let mismatches = committed
            .iter()
            .zip(reference.iter())
            .filter(|(c, r)| c != r)
            .count();
        let count = StreamToleranceCount {
            slot,
            mismatches,
            committed_len,
        };
        if first_failing.is_none() && count.fails(tolerance_per_thousand) {
            first_failing = Some(count);
        }
        per_stream.push(count);
    }
    Ok(CohortToleranceVerdict {
        tolerance_per_thousand,
        per_stream,
        first_failing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::COHORT_TOKEN_TOLERANCE_PER_THOUSAND;

    // The tolerance the ruling pins (10% == 100 per-thousand); the tests below construct fixtures
    // relative to this so a change to the constant is exercised, not hard-coded around.
    const THR: u32 = COHORT_TOKEN_TOLERANCE_PER_THOUSAND;

    /// A stream of `len` committed tokens whose first `mismatches` positions differ from the
    /// reference and the rest match. Returns `(committed, reference_argmax)`. Distinct base values
    /// per slot keep a "reads only slot 0 / multiplies by B" bug visible: each slot's fixtures use a
    /// different token space.
    fn stream(base: i64, len: usize, mismatches: usize) -> (Vec<i64>, Vec<i64>) {
        let reference: Vec<i64> = (0..len as i64).map(|i| base + i).collect();
        let mut committed = reference.clone();
        for c in committed.iter_mut().take(mismatches) {
            // Perturb into a token that cannot collide with any reference token in this slot's space.
            *c += 1_000_000;
        }
        (committed, reference)
    }

    #[test]
    fn exactly_ten_percent_accepts() {
        // (a) one stream at EXACTLY 10% (10 of 100) — strict `>` means the boundary PASSES.
        let (c0, r0) = stream(100, 100, 10);
        let (c1, r1) = stream(500, 100, 0);
        let verdict =
            evaluate_cohort_token_tolerance(&[c0, c1], &[r0, r1], THR).expect("well-shaped");
        assert!(verdict.passed(), "exactly 10% must pass: {verdict:?}");
        assert_eq!(verdict.per_stream[0].mismatches, 10);
        assert_eq!(verdict.per_stream[0].committed_len, 100);
    }

    #[test]
    fn one_token_over_ten_percent_rejects_naming_that_slot() {
        // (b) one stream at 10%+1 (11 of 100) rejects the WHOLE run and names slot 0. Slot 1 is
        // perfect, proving the reject came from slot 0 specifically.
        let (c0, r0) = stream(100, 100, 11);
        let (c1, r1) = stream(500, 100, 0);
        let verdict =
            evaluate_cohort_token_tolerance(&[c0, c1], &[r0, r1], THR).expect("well-shaped");
        assert!(!verdict.passed());
        let failing = verdict.first_failing.expect("a failing stream");
        assert_eq!(failing.slot, 0, "the OVER-threshold slot names itself");
        assert_eq!(failing.mismatches, 11);
        assert_eq!(failing.committed_len, 100);
    }

    #[test]
    fn concentration_in_one_stream_rejects_proving_per_stream_not_average() {
        // (c) stream 0 at 80% mismatch, streams 1-7 perfect. A per-COHORT-AVERAGE gate would see
        // 80/8 = 10% and PASS; the per-STREAM gate must REJECT on slot 0. This is the mutation
        // target for "averages instead of per-stream".
        let (c0, r0) = stream(100, 100, 80);
        let mut committed = vec![c0];
        let mut reference = vec![r0];
        for slot in 1..8 {
            let (c, r) = stream(1000 * slot as i64, 100, 0);
            committed.push(c);
            reference.push(r);
        }
        let verdict =
            evaluate_cohort_token_tolerance(&committed, &reference, THR).expect("well-shaped");
        assert!(
            !verdict.passed(),
            "concentration in one stream must reject (per-stream, not average)"
        );
        assert_eq!(verdict.first_failing.unwrap().slot, 0);
        // The average IS within tolerance — proving the gate did not average.
        let total_mismatch: usize = verdict.per_stream.iter().map(|s| s.mismatches).sum();
        let total_committed: usize = verdict.per_stream.iter().map(|s| s.committed_len).sum();
        assert!(
            (total_mismatch as u64) * 1000 <= (THR as u64) * (total_committed as u64),
            "the cohort AVERAGE is within tolerance — a per-cohort gate would have passed this"
        );
    }

    #[test]
    fn all_streams_just_under_accepts_with_distinct_per_slot_fixtures() {
        // (e) every stream just UNDER threshold (9 of 100 each), each in its own token space so a
        // "reads only slot 0" bug would misjudge the others. All pass.
        let mut committed = Vec::new();
        let mut reference = Vec::new();
        for slot in 0..8 {
            let (c, r) = stream(10_000 * (slot as i64 + 1), 100, 9);
            committed.push(c);
            reference.push(r);
        }
        let verdict =
            evaluate_cohort_token_tolerance(&committed, &reference, THR).expect("well-shaped");
        assert!(
            verdict.passed(),
            "all just-under streams accept: {verdict:?}"
        );
        for (slot, s) in verdict.per_stream.iter().enumerate() {
            assert_eq!(s.slot, slot);
            assert_eq!(s.mismatches, 9, "each slot judged on its OWN tokens");
        }
    }

    #[test]
    fn later_slot_over_threshold_still_rejects() {
        // Mutation guard against "only checks slot 0": slot 0 perfect, slot 5 over threshold.
        let mut committed = Vec::new();
        let mut reference = Vec::new();
        for slot in 0..8 {
            let mismatches = if slot == 5 { 11 } else { 0 };
            let (c, r) = stream(10_000 * (slot as i64 + 1), 100, mismatches);
            committed.push(c);
            reference.push(r);
        }
        let verdict =
            evaluate_cohort_token_tolerance(&committed, &reference, THR).expect("well-shaped");
        assert!(!verdict.passed());
        assert_eq!(
            verdict.first_failing.unwrap().slot,
            5,
            "a later slot over threshold must be the named failure"
        );
    }

    #[test]
    fn n2_echo_mismatch_is_integrity_error_distinct_from_tolerance() {
        // (d) N2: the oracle echoes a committed_token that differs from the candidate journal at
        // slot 1 position 3. This is a HARD integrity error, NOT a tolerance decision — even though
        // the tolerance gate alone would have PASSED (only one token differs).
        let committed = vec![vec![1, 2, 3, 4], vec![10, 11, 12, 13]];
        let mut echoed = committed.clone();
        echoed[1][3] = 999; // the oracle replayed a different journal here.
        let err = verify_replay_echo_matches_committed(&committed, &echoed)
            .expect_err("an echo divergence must be a hard integrity error");
        match err {
            CohortReplayIntegrityError::TokenMismatch {
                slot,
                position,
                committed,
                echoed,
            } => {
                assert_eq!((slot, position, committed, echoed), (1, 3, 13, 999));
            }
            other => panic!("expected a TokenMismatch, got {other:?}"),
        }
        // The identical journals (the honest case) pass N2 AND the tolerance gate.
        verify_replay_echo_matches_committed(&committed, &committed).expect("identical passes N2");
        let verdict = evaluate_cohort_token_tolerance(&committed, &committed, THR).unwrap();
        assert!(verdict.passed(), "identical journal is a clean pass");
    }

    #[test]
    fn empty_stream_is_structural_error_not_a_pass() {
        // denom == 0 must be a hard structural error, never silently a pass (0 mismatches / 0).
        let err = evaluate_cohort_token_tolerance(&[vec![]], &[vec![]], THR)
            .expect_err("an empty stream is structural");
        assert_eq!(err, CohortToleranceError::EmptyStream { slot: 0 });
    }

    #[test]
    fn shape_mismatches_are_structural_errors() {
        // Stream-count and per-stream-length mismatches are structural, not tolerance outcomes.
        assert_eq!(
            evaluate_cohort_token_tolerance(&[vec![1]], &[vec![1], vec![2]], THR).unwrap_err(),
            CohortToleranceError::StreamCountMismatch {
                committed: 1,
                reference: 2
            }
        );
        assert_eq!(
            evaluate_cohort_token_tolerance(&[vec![1, 2]], &[vec![1]], THR).unwrap_err(),
            CohortToleranceError::StreamLengthMismatch {
                slot: 0,
                committed: 2,
                reference: 1
            }
        );
    }
}
