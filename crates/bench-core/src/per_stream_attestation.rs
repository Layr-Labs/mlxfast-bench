//! Per-stream timing attestation — per-stream-instrumentation-spec.md steps 1-2, REPORT-ONLY.
//!
//! The engine half (step 1) adds two additive, capability-gated wire vectors to the v1.2
//! BATCHED (cohort) free-run responses: `prefill_ns_by_stream` (on `free_decode_begin`) and
//! `decode_ns_by_stream` (on `free_decode_run`) — per-slot monotonic nanoseconds, raw engine
//! clock reads, nothing summed/ratioed/converted engine-side. This module is the benchd half
//! (step 2): the ATTESTATION treatment those engine-reported durations require before they may
//! ever influence a score (parent-clock doctrine — engine-reported time is untrusted for
//! scoring; benchd cross-checks it against its own parent-side windows).
//!
//! # Report-only posture (binding, this increment)
//!
//! ε/δ/δ′ tolerances are MEASURED, not invented — a real value needs box session 4's dual-reading
//! capture (parent windows + first real per-stream vectors) to calibrate. Until then this module
//! computes every clause (c)-(f) as a [`ClauseVerdict`] carrying the RAW ratio the clause reduces
//! to, plus [`ClauseVerdict::flagged_at_zero_tolerance`] — a proxy computed at ZERO slack (the one
//! boundary that is not invented: ε/δ/δ′ are non-negative SLACK terms, so the zero-slack version
//! of each inequality is the strictest any real calibration can produce, and a ratio that already
//! crosses it will almost certainly still cross a real, looser, calibrated bound). This module
//! REFUSES nothing on clauses (c)-(f) — see [`PerStreamAttestationError`] for the only refusals
//! this increment issues (clauses (a)/(b), structural impossibilities). The composite score
//! (`prefill_gain^0.25 * decode_gain^0.75`) is computed elsewhere, from benchd's OWN parent-clocked
//! shared windows (benchctl `measure_job::shared_window_composite`, the SHARED-WINDOW ruling) — it
//! never reads this module's engine-reported evidence, and nothing here writes to a scored field.
//! This data's job is BOX CALIBRATION: engine self-timing measured against the parent clock.
//!
//! # Clause map (spec lettering, kept verbatim so the two documents stay cross-referenceable)
//!
//! - (a) capability absent but per-stream scoring requested → [`PerStreamAttestationError::CapabilityAbsent`].
//! - (b) malformed wire (vector length != B, a zero-duration entry, a non-positive/non-finite
//!   parent window, or zero rounds) → the other [`PerStreamAttestationError`] variants.
//! - (c) bounding, PER-STREAM, both phases → [`PerStreamAttestation::prefill_bounding`] /
//!   [`PerStreamAttestation::decode_bounding`].
//! - (d) coverage (the slowest stream must approximately span the window), both phases →
//!   [`PerStreamAttestation::prefill_coverage`] / [`PerStreamAttestation::decode_coverage`].
//! - (e) per-stream token-count floor (decode-only; closes selective compression) →
//!   [`PerStreamAttestation::token_count_floor`] — see that field's doc for the attack this
//!   closes, quoted verbatim from the spec.
//! - (f) lockstep-interleave cross-check (secondary, decode-only, best-effort proxy — see
//!   [`PerStreamAttestation::lockstep_interleave`]'s doc for why this increment cannot compute
//!   the literal per-round interleave the spec describes) →
//!   [`PerStreamAttestation::lockstep_interleave`].

use serde::Serialize;

/// Which phase a per-stream duration or window belongs to. Used only to name refusals and
/// verdicts — never itself part of a computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerStreamPhase {
    /// Cohort-prefill: `prefill_ns_by_stream`, bounded against the parent's measured prefill
    /// window.
    Prefill,
    /// Decode: `decode_ns_by_stream`, bounded against the parent's measured decode window.
    Decode,
}

impl core::fmt::Display for PerStreamPhase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PerStreamPhase::Prefill => write!(f, "prefill"),
            PerStreamPhase::Decode => write!(f, "decode"),
        }
    }
}

/// The ONLY refusals per-stream attestation issues in this increment (spec clauses (a)/(b)) —
/// structural impossibilities. Every other named clause ((c)-(f)) computes a REPORT-ONLY
/// [`ClauseVerdict`] instead: tolerances are not yet pinned, so nothing beyond a structural
/// impossibility may refuse a leg on this data.
#[derive(Debug, Clone, PartialEq)]
pub enum PerStreamAttestationError {
    /// Clause (a): per-stream scoring was requested for this leg but the engine did not
    /// advertise `per_stream_timing` (or the wire simply carried no vector for a phase) —
    /// refused BEFORE the cool gate and BEFORE the clock, the same advertise-before-use posture
    /// the `batched_free_run_decode` capability gate already uses for the cohort form itself.
    CapabilityAbsent,
    /// Clause (a) companion: the capability WAS advertised but this phase's vector is simply
    /// absent from the response — a wiring bug (an engine that advertises the capability must
    /// carry the vector on every batched begin/run it emits), reported distinctly from
    /// [`CapabilityAbsent`] so the two causes are not conflated in a refusal message.
    VectorMissing { phase: PerStreamPhase },
    /// Clause (b): a per-slot vector's length disagrees with the cohort width B.
    VectorLength {
        phase: PerStreamPhase,
        batch_size: u32,
        got: usize,
    },
    /// Clause (b): `tokens_len_by_stream.len() != B` — the oracle-validated per-slot committed
    /// count vector (K_slot, already sealed by `verify_cohort_consistency` elsewhere) must cover
    /// every slot for clause (e) to have a floor to check.
    TokensLenByStreamWidth { batch_size: u32, got: usize },
    /// Clause (b): a reported per-slot duration was exactly zero. Every real cohort-prefill or
    /// decode-phase commit takes a positive amount of monotonic wall time; a reported zero is a
    /// malformed wire value (the shape-validation posture this module mirrors: refuse a
    /// structural impossibility, never silently treat it as "instant").
    ZeroDuration { phase: PerStreamPhase, slot: usize },
    /// Clause (b): the parent-measured window for this phase was non-positive or non-finite — a
    /// window of zero (or less) makes every bounding/coverage ratio in this module undefined,
    /// which is a benchd-side measurement failure, not evidence about the engine.
    NonPositiveWindow { phase: PerStreamPhase, seconds: f64 },
    /// Clause (b): `rounds == 0` — the decode phase's own round count (R), benchd-observed, is
    /// the divisor of `step_time` (clause (e)); a real decode phase that ran at all has R >= 1.
    ZeroRounds,
}

impl core::fmt::Display for PerStreamAttestationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PerStreamAttestationError::CapabilityAbsent => write!(
                f,
                "per-stream scoring was requested but the engine did not advertise \
                 per_stream_timing — refused pre-clock, advertise-before-use"
            ),
            PerStreamAttestationError::VectorMissing { phase } => write!(
                f,
                "per_stream_timing was advertised but the {phase} per-slot ns vector is absent \
                 from the response"
            ),
            PerStreamAttestationError::VectorLength {
                phase,
                batch_size,
                got,
            } => write!(
                f,
                "{phase}_ns_by_stream has {got} entries, expected B={batch_size}"
            ),
            PerStreamAttestationError::TokensLenByStreamWidth { batch_size, got } => write!(
                f,
                "tokens_len_by_stream (K_slot) has {got} entries, expected B={batch_size}"
            ),
            PerStreamAttestationError::ZeroDuration { phase, slot } => write!(
                f,
                "{phase}_ns_by_stream[{slot}] is zero — no real commit takes zero monotonic ns"
            ),
            PerStreamAttestationError::NonPositiveWindow { phase, seconds } => write!(
                f,
                "the parent-measured {phase} window ({seconds}s) is non-positive or non-finite"
            ),
            PerStreamAttestationError::ZeroRounds => write!(
                f,
                "rounds == 0 — step_time (parent decode window / rounds) is undefined for a \
                 decode phase that ran zero rounds"
            ),
        }
    }
}

impl std::error::Error for PerStreamAttestationError {}

/// One clause's REPORT-ONLY verdict over one raw ratio.
///
/// `flagged_at_zero_tolerance` is NOT a scored pass/fail and NOT a stand-in for a real,
/// calibrated ε/δ/δ′ — see the module doc for why zero slack is the one boundary this increment
/// may compute without inventing a number. A `false` here means only "this ratio does not
/// already cross the strictest possible bound," not "this leg is clean"; a `true` means the
/// leg would fail even the most lenient real tolerance a non-negative calibration could produce.
///
/// `Serialize` (report-only seal, gap G2): the verdict is sealed VERBATIM into `results.json` by
/// benchctl's measure-job as an additive diagnostic — the same struct the box-session-4
/// calibration reads its ε/δ/δ′ derivation inputs (`raw_ratio`) from, so there is no second,
/// drift-prone sealed shape. NOTE: `serde_json` renders a non-finite `f64` (the `+inf` a
/// zero-`K_slot` floor seals) as JSON `null`; the paired `flagged_at_zero_tolerance` still
/// carries that clause's zero-slack reading.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ClauseVerdict {
    /// The raw ratio this clause reduces to — sealed for box-session-4 calibration, never
    /// itself a scored quantity. See each field on [`PerStreamAttestation`] for what the ratio
    /// means for that clause.
    pub raw_ratio: f64,
    /// Whether `raw_ratio` already crosses this clause's zero-slack (ε=δ=δ′=0) boundary.
    pub flagged_at_zero_tolerance: bool,
}

/// The REPORT-ONLY per-leg attestation verdict (spec clauses (c)-(f)). Sealed evidence for
/// calibration — nothing here refuses a leg or feeds a score. One instance covers ONE leg (the
/// serial leg and the candidate leg of a pair each get their own). `Serialize` — see
/// [`ClauseVerdict`]'s note: this exact struct is the sealed `results.json` shape (gap G2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PerStreamAttestation {
    /// The cohort width B this verdict covers.
    pub batch_size: u32,
    /// Clause (c), prefill phase, PER-STREAM: `prefill_ns_by_stream[slot] / prefill_window_ns`.
    /// Zero-slack boundary: `raw_ratio <= 1.0` (the real bound is `<= 1 + ε`); flagged when the
    /// stream's prefill commit landed AFTER the parent's own measured prefill window closed.
    pub prefill_bounding: Vec<ClauseVerdict>,
    /// Clause (c), decode phase, PER-STREAM: `decode_ns_by_stream[slot] / decode_window_ns`.
    /// Same zero-slack boundary and reading as [`prefill_bounding`](Self::prefill_bounding).
    pub decode_bounding: Vec<ClauseVerdict>,
    /// Clause (d), prefill phase: `max(prefill_ns_by_stream) / prefill_window_ns`. Zero-slack
    /// boundary: `raw_ratio >= 1.0` (the real bound is `>= 1 - δ`); flagged when even the
    /// slowest stream finished well short of the window a concurrent closed cohort should have
    /// filled end-to-end (under-reporting or an idled stream).
    pub prefill_coverage: ClauseVerdict,
    /// Clause (d), decode phase: `max(decode_ns_by_stream) / decode_window_ns`. Same reading as
    /// [`prefill_coverage`](Self::prefill_coverage).
    pub decode_coverage: ClauseVerdict,
    /// Clause (e), decode-only, PER-STREAM (the token-count floor; spec verbatim):
    ///
    /// > for each slot, `reported_stream_time >= K_slot * step_time * (1-δ')`, where `K_slot` =
    /// > that slot's committed-token count (already on the wire AND oracle-validated) and
    /// > `step_time = parent decode window / total rounds`. ... Without this, a participant
    /// > keeps one candidate stream at full window (satisfying coverage) and compresses the
    /// > other seven, shrinking candidate_sum -> inflating gain.
    ///
    /// `raw_ratio = decode_ns_by_stream[slot] / (K_slot * step_time_ns)`. Zero-slack boundary:
    /// `raw_ratio >= 1.0`; flagged when a stream's reported decode time is compressed below what
    /// its OWN committed-token count implies — closes selective compression that (b)-(d) alone
    /// only bound on the maximum. THE mutation-testing target this increment must be non-vacuous
    /// on: a deliberately-compressed non-max stream must flag here.
    pub token_count_floor: Vec<ClauseVerdict>,
    /// Clause (f), decode-only, PER-STREAM, secondary and best-effort in this increment.
    ///
    /// The spec's literal description ("per-slot commit timestamps should interleave
    /// consistently with the common-width round cadence... bunched commits on one slot vs the
    /// lockstep pattern") describes a per-ROUND per-slot timeline. The step-1 wire carries only
    /// ONE scalar per slot per phase (the FINAL commit, not a per-round series), so this
    /// increment cannot compute the literal interleave check — that needs a wire increment this
    /// spec does not ask for yet. As a stand-in with the data actually available: the per-slot
    /// ratio of that slot's `decode_ns_by_stream` to the cohort's OWN mean decode time
    /// (`decode_ns_by_stream[slot] / mean(decode_ns_by_stream)`) — under true lockstep every
    /// slot advances on the same shared round cadence, so absent compression every slot's total
    /// decode time should cluster tightly around the cohort mean. Deliberately carries NO
    /// `flagged_at_zero_tolerance` boundary: unlike (c)/(d)/(e), there is no physically fixed
    /// reference this ratio is bounded against (1.0 is a statistical center, not a structural
    /// floor or ceiling), so inventing a flag threshold here would be exactly the placeholder
    /// tolerance the spec forbids. Raw ratios only.
    pub lockstep_interleave: Vec<f64>,
    /// `sum(prefill_ns_by_stream)` — an UNATTESTED/UNSCORED diagnostic, sealed alongside the raw
    /// per-slot vectors (spec: "per_stream sums MAY be computed and SEALED as diagnostics...
    /// alongside the raw vectors"). NOT the composite's input: the composite is computed from
    /// benchd's own parent clock (benchctl `measure_job::shared_window_composite`).
    pub prefill_sum_ns: u64,
    /// `sum(decode_ns_by_stream)` — same UNATTESTED/UNSCORED posture as
    /// [`prefill_sum_ns`](Self::prefill_sum_ns).
    pub decode_sum_ns: u64,
    /// The tolerance-pinning state this verdict was computed under. ALWAYS
    /// `"unpinned-tolerances"` in this increment — never a placeholder numeric tolerance. A
    /// `&'static str` rather than a bool/enum so a future pinned-tolerance verdict cannot be
    /// confused with this one by an incomplete match (the string itself must change, which a
    /// reviewer sees).
    pub tolerance_state: &'static str,
}

/// `1.0` in nanoseconds-per-second — named so every window-unit conversion in this module reads
/// the same way rather than repeating a bare magic literal.
const NANOS_PER_SECOND: f64 = 1_000_000_000.0;

/// The inputs [`attest_leg`] needs for ONE leg — struct-ified (rather than eight positional
/// parameters) so a caller assembles them by name once, from data already sitting in different
/// places (the wire response, the parent-side clock, the consistency-checked audit).
pub struct PerStreamAttestationInputs<'a> {
    /// The leg's engine hello's `per_stream_timing` capability membership.
    pub capability_advertised: bool,
    /// The cohort width B this leg ran.
    pub batch_size: u32,
    /// The (already `deny_unknown_fields`-parsed) `prefill_ns_by_stream` wire vector, `None`
    /// when the response simply omitted the additive field.
    pub prefill_ns_by_stream: Option<&'a [u64]>,
    /// The (already `deny_unknown_fields`-parsed) `decode_ns_by_stream` wire vector, same
    /// absence convention as [`prefill_ns_by_stream`](Self::prefill_ns_by_stream).
    pub decode_ns_by_stream: Option<&'a [u64]>,
    /// K_slot per slot — benchd's own oracle-validated per-slot committed count
    /// (`CohortFreeRunResponse::tokens_len_by_stream`, already checked equal to N by
    /// `verify_cohort_consistency` elsewhere; this function does not re-derive that, it trusts
    /// the caller passed the SAME vector that consistency check ran over).
    pub tokens_len_by_stream: &'a [usize],
    /// The parent-side wall-clock PREFILL window over the WHOLE cohort
    /// (`BatchedFreeRunPhaseTiming::prefill_elapsed_seconds`).
    pub prefill_window_seconds: f64,
    /// The parent-side wall-clock DECODE window over the WHOLE cohort
    /// (`BatchedFreeRunPhaseTiming::decode_elapsed_seconds`).
    pub decode_window_seconds: f64,
    /// R, benchd-observed (`CohortFreeRunAudit::rounds()` or the wire's own `rounds`, already
    /// cross-checked against `acceptance_lengths.len()`).
    pub rounds: usize,
}

/// Compute the REPORT-ONLY per-stream attestation for ONE leg (spec clauses (a)-(f)). See
/// [`PerStreamAttestationInputs`] for what each field means and where it comes from.
pub fn attest_leg(
    inputs: PerStreamAttestationInputs<'_>,
) -> Result<PerStreamAttestation, PerStreamAttestationError> {
    let PerStreamAttestationInputs {
        capability_advertised,
        batch_size,
        prefill_ns_by_stream,
        decode_ns_by_stream,
        tokens_len_by_stream,
        prefill_window_seconds,
        decode_window_seconds,
        rounds,
    } = inputs;
    if !capability_advertised {
        return Err(PerStreamAttestationError::CapabilityAbsent);
    }
    let prefill = prefill_ns_by_stream.ok_or(PerStreamAttestationError::VectorMissing {
        phase: PerStreamPhase::Prefill,
    })?;
    let decode = decode_ns_by_stream.ok_or(PerStreamAttestationError::VectorMissing {
        phase: PerStreamPhase::Decode,
    })?;

    if prefill.len() != batch_size as usize {
        return Err(PerStreamAttestationError::VectorLength {
            phase: PerStreamPhase::Prefill,
            batch_size,
            got: prefill.len(),
        });
    }
    if decode.len() != batch_size as usize {
        return Err(PerStreamAttestationError::VectorLength {
            phase: PerStreamPhase::Decode,
            batch_size,
            got: decode.len(),
        });
    }
    if tokens_len_by_stream.len() != batch_size as usize {
        return Err(PerStreamAttestationError::TokensLenByStreamWidth {
            batch_size,
            got: tokens_len_by_stream.len(),
        });
    }
    for (slot, &ns) in prefill.iter().enumerate() {
        if ns == 0 {
            return Err(PerStreamAttestationError::ZeroDuration {
                phase: PerStreamPhase::Prefill,
                slot,
            });
        }
    }
    for (slot, &ns) in decode.iter().enumerate() {
        if ns == 0 {
            return Err(PerStreamAttestationError::ZeroDuration {
                phase: PerStreamPhase::Decode,
                slot,
            });
        }
    }
    if !prefill_window_seconds.is_finite() || prefill_window_seconds <= 0.0 {
        return Err(PerStreamAttestationError::NonPositiveWindow {
            phase: PerStreamPhase::Prefill,
            seconds: prefill_window_seconds,
        });
    }
    if !decode_window_seconds.is_finite() || decode_window_seconds <= 0.0 {
        return Err(PerStreamAttestationError::NonPositiveWindow {
            phase: PerStreamPhase::Decode,
            seconds: decode_window_seconds,
        });
    }
    if rounds == 0 {
        return Err(PerStreamAttestationError::ZeroRounds);
    }

    let prefill_window_ns = prefill_window_seconds * NANOS_PER_SECOND;
    let decode_window_ns = decode_window_seconds * NANOS_PER_SECOND;
    let step_time_ns = decode_window_ns / rounds as f64;

    // Clause (c): PER-STREAM bounding, both phases. Zero-slack: duration <= window (ratio <= 1).
    let prefill_bounding: Vec<ClauseVerdict> = prefill
        .iter()
        .map(|&ns| {
            let raw_ratio = ns as f64 / prefill_window_ns;
            ClauseVerdict {
                raw_ratio,
                flagged_at_zero_tolerance: raw_ratio > 1.0,
            }
        })
        .collect();
    let decode_bounding: Vec<ClauseVerdict> = decode
        .iter()
        .map(|&ns| {
            let raw_ratio = ns as f64 / decode_window_ns;
            ClauseVerdict {
                raw_ratio,
                flagged_at_zero_tolerance: raw_ratio > 1.0,
            }
        })
        .collect();

    // Clause (d): coverage, both phases. Zero-slack: max(duration) >= window (ratio >= 1).
    let prefill_coverage = {
        let max_ns = prefill.iter().copied().max().unwrap_or(0);
        let raw_ratio = max_ns as f64 / prefill_window_ns;
        ClauseVerdict {
            raw_ratio,
            flagged_at_zero_tolerance: raw_ratio < 1.0,
        }
    };
    let decode_coverage = {
        let max_ns = decode.iter().copied().max().unwrap_or(0);
        let raw_ratio = max_ns as f64 / decode_window_ns;
        ClauseVerdict {
            raw_ratio,
            flagged_at_zero_tolerance: raw_ratio < 1.0,
        }
    };

    // Clause (e): decode-only, PER-STREAM token-count floor. Zero-slack: decode >= K_slot *
    // step_time (ratio >= 1). K_slot == 0 is a degenerate slot (nothing committed, nothing to
    // floor) — ratio sealed as +inf (trivially satisfied, never flagged) rather than a NaN from
    // a 0/0 division.
    let token_count_floor: Vec<ClauseVerdict> = decode
        .iter()
        .zip(tokens_len_by_stream.iter())
        .map(|(&ns, &k_slot)| {
            let floor_ns = k_slot as f64 * step_time_ns;
            let raw_ratio = if floor_ns > 0.0 {
                ns as f64 / floor_ns
            } else {
                f64::INFINITY
            };
            ClauseVerdict {
                raw_ratio,
                flagged_at_zero_tolerance: raw_ratio < 1.0,
            }
        })
        .collect();

    // Clause (f): decode-only, PER-STREAM lockstep-interleave proxy (see the field doc for why
    // this is a stand-in, not the literal spec check, and why it carries no zero-tolerance flag).
    let mean_decode_ns = decode.iter().copied().sum::<u64>() as f64 / batch_size as f64;
    let lockstep_interleave: Vec<f64> = decode
        .iter()
        .map(|&ns| ns as f64 / mean_decode_ns)
        .collect();

    Ok(PerStreamAttestation {
        batch_size,
        prefill_bounding,
        decode_bounding,
        prefill_coverage,
        decode_coverage,
        token_count_floor,
        lockstep_interleave,
        prefill_sum_ns: prefill.iter().sum(),
        decode_sum_ns: decode.iter().sum(),
        tolerance_state: "unpinned-tolerances",
    })
}

/// The output of [`composite_diagnostic`] — same field shape as `benchctl`'s
/// `CompositeCohortScore` minus the floor/floor-met pair (an enforcement concern this diagnostic
/// does not touch), plus the same `tolerance_state` marker [`PerStreamAttestation`] carries, so a
/// reader can never mistake this for a scored value. `Serialize` — sealed per accepted pair as
/// an additive diagnostic (gap G2), same posture as [`PerStreamAttestation`]; `serde_json`
/// renders a `NaN` `composite_score` (a guard-rejected gain) as JSON `null`, never a number.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PerStreamCompositeDiagnostic {
    pub prefill_gain: f64,
    pub decode_gain: f64,
    pub composite_score: f64,
    /// Always `"unpinned-tolerances"` — see the module doc.
    pub tolerance_state: &'static str,
}

/// Per-stream timing instrumentation (spec steps 1-2) — the DIAGNOSTIC pairing of two legs'
/// [`PerStreamAttestation`] verdicts into the shape the eventual composite score will carry,
/// computed from the SUM-based aggregates (`prefill_sum_ns` / `decode_sum_ns`) each verdict
/// already seals.
///
/// Ratio-of-sums, serial-anchored, via [`crate::score::speedup`] (the SAME non-finite/
/// non-positive guard the real score path uses, so this diagnostic cannot silently divide by a
/// degenerate sum): `prefill_gain = speedup(serial.prefill_sum_ns, candidate.prefill_sum_ns)` and
/// the decode twin. `composite_score = prefill_gain^prefill_gain_exponent *
/// decode_gain^decode_gain_exponent` — `NaN` if either gain guard-rejected (mirrors
/// [`crate::score::score`]'s own NaN posture). The exponent pair is taken as plain `f64`s rather
/// than a benchctl-specific type: this crate does not depend on `benchctl`, so the CALLER is
/// responsible for passing the actually-certified pair (`ScoredExponents::certify`'s output,
/// David's ruled 0.25/0.75) rather than a fixture-declared or invented one.
///
/// UNATTESTED / UNSCORED, permanently: this function's output does NOT populate a scored field.
/// The published composite is benchctl's `measure_job::shared_window_composite`, computed from the
/// PARENT clock alone (the SHARED-WINDOW ruling: on the rectangular lockstep cohort the two
/// readings are the same quantity, and only this one carries an engine-controlled attribution
/// term). What this function produces is the per-stream reading of that quantity, kept as BOX
/// CALIBRATION evidence — agreement corroborates, disagreement is a finding, neither is a score.
pub fn composite_diagnostic(
    serial: &PerStreamAttestation,
    candidate: &PerStreamAttestation,
    prefill_gain_exponent: f64,
    decode_gain_exponent: f64,
) -> PerStreamCompositeDiagnostic {
    let prefill_gain = crate::score::speedup(
        serial.prefill_sum_ns as f64,
        candidate.prefill_sum_ns as f64,
    );
    let decode_gain =
        crate::score::speedup(serial.decode_sum_ns as f64, candidate.decode_sum_ns as f64);
    let composite_score = if prefill_gain.is_finite()
        && prefill_gain > 0.0
        && decode_gain.is_finite()
        && decode_gain > 0.0
    {
        prefill_gain.powf(prefill_gain_exponent) * decode_gain.powf(decode_gain_exponent)
    } else {
        f64::NAN
    };
    PerStreamCompositeDiagnostic {
        prefill_gain,
        decode_gain,
        composite_score,
        tolerance_state: "unpinned-tolerances",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conformant B=2 leg: both streams span the parent windows tightly (no bounding/coverage
    /// flags), and both commit exactly K_slot = N = 4 tokens over R = 4 rounds (serial cohort,
    /// one token per round) at the SAME per-slot pace as `step_time` — so clause (e) does not
    /// flag either slot.
    fn conformant_inputs() -> (Vec<u64>, Vec<u64>, Vec<usize>, f64, f64, usize) {
        // decode_window = 400_000ns over rounds=4 -> step_time = 100_000ns/round. Each slot
        // commits N=4 tokens (K_slot=4) at exactly 100_000ns/token -> floor = 400_000ns, and
        // both slots report decode_ns == 400_000 (== window, == floor): every zero-slack
        // boundary is met with equality (a genuine slot never crosses PAST it in the honest
        // case).
        let prefill = vec![50_000u64, 50_000u64];
        let decode = vec![400_000u64, 400_000u64];
        let tokens_len_by_stream = vec![4usize, 4usize];
        (prefill, decode, tokens_len_by_stream, 0.00005, 0.0004, 4)
    }

    /// Test-only shorthand for the overwhelmingly common case (capability advertised, B=2) —
    /// builds [`PerStreamAttestationInputs`] so individual tests read as the six numbers that
    /// actually vary, not eight positional arguments most of which never change.
    fn attest(
        prefill: &[u64],
        decode: &[u64],
        tokens_len_by_stream: &[usize],
        prefill_window_seconds: f64,
        decode_window_seconds: f64,
        rounds: usize,
    ) -> Result<PerStreamAttestation, PerStreamAttestationError> {
        attest_leg(PerStreamAttestationInputs {
            capability_advertised: true,
            batch_size: 2,
            prefill_ns_by_stream: Some(prefill),
            decode_ns_by_stream: Some(decode),
            tokens_len_by_stream,
            prefill_window_seconds,
            decode_window_seconds,
            rounds,
        })
    }

    #[test]
    fn conformant_leg_flags_nothing() {
        let (prefill, decode, k, pw, dw, rounds) = conformant_inputs();
        let verdict = attest(&prefill, &decode, &k, pw, dw, rounds)
            .expect("conformant inputs attest cleanly");
        assert_eq!(verdict.batch_size, 2);
        for v in &verdict.prefill_bounding {
            assert!(!v.flagged_at_zero_tolerance, "{v:?}");
            assert!((v.raw_ratio - 1.0).abs() < 1e-9);
        }
        for v in &verdict.decode_bounding {
            assert!(!v.flagged_at_zero_tolerance, "{v:?}");
        }
        assert!(!verdict.prefill_coverage.flagged_at_zero_tolerance);
        assert!(!verdict.decode_coverage.flagged_at_zero_tolerance);
        for v in &verdict.token_count_floor {
            assert!(!v.flagged_at_zero_tolerance, "{v:?}");
            assert!((v.raw_ratio - 1.0).abs() < 1e-9);
        }
        assert_eq!(verdict.lockstep_interleave, vec![1.0, 1.0]);
        assert_eq!(verdict.prefill_sum_ns, 100_000);
        assert_eq!(verdict.decode_sum_ns, 800_000);
        assert_eq!(verdict.tolerance_state, "unpinned-tolerances");
    }

    /// THE MUTATION-TESTING TARGET (non-vacuity for clause (e)): slot 0 stays at full window
    /// (satisfying bounding AND coverage — the exact "one candidate stream at full window"
    /// half of the spec's named attack), slot 1 is deliberately COMPRESSED to a quarter of what
    /// its own K_slot=4 tokens at this cohort's step_time implies. Clauses (b)-(d) alone cannot
    /// see this (the max is still the honest slot 0, satisfying coverage); clause (e) must.
    #[test]
    fn compressed_non_max_stream_flags_clause_e_and_nothing_else_masks_it() {
        let (prefill, _decode, k, pw, dw, rounds) = conformant_inputs();
        // Slot 0 honest (400_000ns, == the window == the floor). Slot 1 compressed to 100_000ns
        // (== 1/4 of its own 400_000ns floor) while still real work happened (nonzero, so it is
        // not caught by the zero-duration structural refusal).
        let decode = vec![400_000u64, 100_000u64];
        let verdict = attest(&prefill, &decode, &k, pw, dw, rounds)
            .expect("a compressed-but-nonzero stream is not a structural refusal");

        // Coverage is satisfied (the max stream, slot 0, still spans the window) — the exact gap
        // clause (e) exists to close, proven directly: coverage does NOT flag this leg.
        assert!(
            !verdict.decode_coverage.flagged_at_zero_tolerance,
            "coverage alone must NOT catch the compressed stream (that is the point of clause e)"
        );
        // Bounding does not flag either (slot 1 is UNDER the window, not over it).
        assert!(!verdict.decode_bounding[0].flagged_at_zero_tolerance);
        assert!(!verdict.decode_bounding[1].flagged_at_zero_tolerance);

        // Clause (e) IS non-vacuous: slot 1 flags, slot 0 does not.
        assert!(
            !verdict.token_count_floor[0].flagged_at_zero_tolerance,
            "the honest slot must not flag: {:?}",
            verdict.token_count_floor[0]
        );
        assert!(
            verdict.token_count_floor[1].flagged_at_zero_tolerance,
            "the compressed slot MUST flag clause (e) — non-vacuity requirement: {:?}",
            verdict.token_count_floor[1]
        );
        assert!((verdict.token_count_floor[1].raw_ratio - 0.25).abs() < 1e-9);

        // And the sum-based gain a participant is trying to inflate visibly shrank — the sealed
        // diagnostic sum makes the shrinkage itself inspectable even though nothing here scores
        // it yet.
        assert_eq!(verdict.decode_sum_ns, 500_000); // 400_000 + 100_000, not 800_000
    }

    #[test]
    fn capability_absent_refuses_before_any_vector_is_read() {
        let (prefill, decode, k, pw, dw, rounds) = conformant_inputs();
        let err = attest_leg(PerStreamAttestationInputs {
            capability_advertised: false,
            batch_size: 2,
            prefill_ns_by_stream: Some(&prefill),
            decode_ns_by_stream: Some(&decode),
            tokens_len_by_stream: &k,
            prefill_window_seconds: pw,
            decode_window_seconds: dw,
            rounds,
        })
        .unwrap_err();
        assert_eq!(err, PerStreamAttestationError::CapabilityAbsent);
    }

    #[test]
    fn missing_vector_refuses_distinctly_from_capability_absent() {
        let (_prefill, decode, k, pw, dw, rounds) = conformant_inputs();
        let err = attest_leg(PerStreamAttestationInputs {
            capability_advertised: true,
            batch_size: 2,
            prefill_ns_by_stream: None,
            decode_ns_by_stream: Some(&decode),
            tokens_len_by_stream: &k,
            prefill_window_seconds: pw,
            decode_window_seconds: dw,
            rounds,
        })
        .unwrap_err();
        assert_eq!(
            err,
            PerStreamAttestationError::VectorMissing {
                phase: PerStreamPhase::Prefill
            }
        );
    }

    #[test]
    fn wrong_length_vector_refuses_named_by_phase() {
        let (prefill, decode, k, pw, dw, rounds) = conformant_inputs();
        let short_decode = vec![decode[0]]; // len 1, expected B=2
        let err = attest(&prefill, &short_decode, &k, pw, dw, rounds).unwrap_err();
        assert_eq!(
            err,
            PerStreamAttestationError::VectorLength {
                phase: PerStreamPhase::Decode,
                batch_size: 2,
                got: 1,
            }
        );
    }

    #[test]
    fn wrong_length_tokens_len_by_stream_refuses() {
        let (prefill, decode, _k, pw, dw, rounds) = conformant_inputs();
        let short_k = vec![4usize];
        let err = attest(&prefill, &decode, &short_k, pw, dw, rounds).unwrap_err();
        assert_eq!(
            err,
            PerStreamAttestationError::TokensLenByStreamWidth {
                batch_size: 2,
                got: 1,
            }
        );
    }

    #[test]
    fn zero_duration_entry_refuses_named_by_phase_and_slot() {
        let (prefill, _decode, k, pw, dw, rounds) = conformant_inputs();
        let decode_with_zero = vec![400_000u64, 0u64];
        let err = attest(&prefill, &decode_with_zero, &k, pw, dw, rounds).unwrap_err();
        assert_eq!(
            err,
            PerStreamAttestationError::ZeroDuration {
                phase: PerStreamPhase::Decode,
                slot: 1,
            }
        );
    }

    #[test]
    fn non_positive_window_refuses() {
        let (prefill, decode, k, _pw, dw, rounds) = conformant_inputs();
        let err = attest(&prefill, &decode, &k, 0.0, dw, rounds).unwrap_err();
        assert_eq!(
            err,
            PerStreamAttestationError::NonPositiveWindow {
                phase: PerStreamPhase::Prefill,
                seconds: 0.0,
            }
        );
    }

    #[test]
    fn nan_window_refuses() {
        let (prefill, decode, k, pw, _dw, rounds) = conformant_inputs();
        let err = attest(&prefill, &decode, &k, pw, f64::NAN, rounds).unwrap_err();
        assert!(matches!(
            err,
            PerStreamAttestationError::NonPositiveWindow {
                phase: PerStreamPhase::Decode,
                ..
            }
        ));
    }

    #[test]
    fn zero_rounds_refuses() {
        let (prefill, decode, k, pw, dw, _rounds) = conformant_inputs();
        let err = attest(&prefill, &decode, &k, pw, dw, 0).unwrap_err();
        assert_eq!(err, PerStreamAttestationError::ZeroRounds);
    }

    #[test]
    fn bounding_flags_when_a_stream_runs_past_its_parent_window() {
        let (prefill, _decode, k, pw, dw, rounds) = conformant_inputs();
        // Slot 1 reports MORE decode ns than the whole cohort's parent-measured decode window —
        // structurally impossible under an honest concurrent-cohort clock (the parent window
        // spans the WHOLE cohort, so no single stream's decode phase can outlast it).
        let decode = vec![400_000u64, 500_000u64];
        let verdict = attest(&prefill, &decode, &k, pw, dw, rounds).unwrap();
        assert!(!verdict.decode_bounding[0].flagged_at_zero_tolerance);
        assert!(verdict.decode_bounding[1].flagged_at_zero_tolerance);
        assert!((verdict.decode_bounding[1].raw_ratio - 1.25).abs() < 1e-9);
    }

    #[test]
    fn coverage_flags_when_even_the_slowest_stream_falls_well_short_of_the_window() {
        let (prefill, _decode, k, pw, dw, rounds) = conformant_inputs();
        // BOTH slots well short of the 400_000ns decode window — the max (slowest) stream still
        // does not approximately span the window, so coverage flags (engine under-reported or a
        // stream idled).
        let decode = vec![100_000u64, 90_000u64];
        let verdict = attest(&prefill, &decode, &k, pw, dw, rounds).unwrap();
        assert!(verdict.decode_coverage.flagged_at_zero_tolerance);
        assert!((verdict.decode_coverage.raw_ratio - 0.25).abs() < 1e-9); // max=100_000/400_000
    }

    #[test]
    fn zero_k_slot_is_a_trivially_satisfied_floor_not_a_division_panic() {
        let (prefill, decode, _k, pw, dw, rounds) = conformant_inputs();
        let k = vec![0usize, 4usize];
        let verdict = attest(&prefill, &decode, &k, pw, dw, rounds).unwrap();
        assert_eq!(verdict.token_count_floor[0].raw_ratio, f64::INFINITY);
        assert!(!verdict.token_count_floor[0].flagged_at_zero_tolerance);
    }

    #[test]
    fn lockstep_interleave_is_one_when_every_slot_matches_the_cohort_mean() {
        let (prefill, decode, k, pw, dw, rounds) = conformant_inputs();
        let verdict = attest(&prefill, &decode, &k, pw, dw, rounds).unwrap();
        assert_eq!(verdict.lockstep_interleave, vec![1.0, 1.0]);
    }

    #[test]
    fn lockstep_interleave_diverges_for_a_slot_far_from_the_cohort_mean() {
        let (prefill, _decode, k, pw, dw, rounds) = conformant_inputs();
        let decode = vec![400_000u64, 100_000u64]; // mean = 250_000
        let verdict = attest(&prefill, &decode, &k, pw, dw, rounds).unwrap();
        assert!((verdict.lockstep_interleave[0] - 1.6).abs() < 1e-9); // 400_000/250_000
        assert!((verdict.lockstep_interleave[1] - 0.4).abs() < 1e-9); // 100_000/250_000
    }

    #[test]
    fn display_messages_name_the_phase_and_slot() {
        assert_eq!(
            PerStreamAttestationError::ZeroDuration {
                phase: PerStreamPhase::Decode,
                slot: 3
            }
            .to_string(),
            "decode_ns_by_stream[3] is zero — no real commit takes zero monotonic ns"
        );
        assert_eq!(PerStreamPhase::Prefill.to_string(), "prefill");
        assert_eq!(PerStreamPhase::Decode.to_string(), "decode");
    }

    // MARK: - composite_diagnostic (spec steps 1-2: aggregation, plumbed but not wired to score)

    /// Only the two sum fields matter to [`composite_diagnostic`]; every other field is filled
    /// with a structurally-valid, unflagged placeholder so this stays a plain data builder
    /// rather than routing through [`attest_leg`]'s full wire simulation.
    fn stream_attestation(prefill_sum_ns: u64, decode_sum_ns: u64) -> PerStreamAttestation {
        PerStreamAttestation {
            batch_size: 2,
            prefill_bounding: vec![],
            decode_bounding: vec![],
            prefill_coverage: ClauseVerdict {
                raw_ratio: 1.0,
                flagged_at_zero_tolerance: false,
            },
            decode_coverage: ClauseVerdict {
                raw_ratio: 1.0,
                flagged_at_zero_tolerance: false,
            },
            token_count_floor: vec![],
            lockstep_interleave: vec![],
            prefill_sum_ns,
            decode_sum_ns,
            tolerance_state: "unpinned-tolerances",
        }
    }

    /// The ruled exponent pair (David, 2026-08-23) — duplicated as a local test constant rather
    /// than imported, since this crate does not depend on `benchctl` (where the certified
    /// `ScoredExponents`/`PREFILL_GAIN_EXPONENT`/`DECODE_GAIN_EXPONENT` actually live); the
    /// values themselves are the same ones `benchctl`'s own `scored_exponents_certify_accepts_
    /// the_one_ruled_pair` test pins.
    const PREFILL_GAIN_EXPONENT: f64 = 0.25;
    const DECODE_GAIN_EXPONENT: f64 = 0.75;

    #[test]
    fn composite_diagnostic_matches_the_ratio_of_sums_raised_to_the_ruled_exponents() {
        // Serial takes 2x as long (both phases) as candidate => both gains are exactly 2.0, and
        // the composite is 2^0.25 * 2^0.75 == 2.0 (the SAME identity `crate::score::score`'s own
        // test already exercises for equal component speedups — this diagnostic must agree).
        let serial = stream_attestation(200_000, 800_000);
        let candidate = stream_attestation(100_000, 400_000);
        let diag = composite_diagnostic(
            &serial,
            &candidate,
            PREFILL_GAIN_EXPONENT,
            DECODE_GAIN_EXPONENT,
        );
        assert!((diag.prefill_gain - 2.0).abs() < 1e-9);
        assert!((diag.decode_gain - 2.0).abs() < 1e-9);
        assert!(
            (diag.composite_score - 2.0).abs() < 1e-9,
            "composite = 2^0.25 * 2^0.75 = 2.0, got {}",
            diag.composite_score
        );
        assert_eq!(diag.tolerance_state, "unpinned-tolerances");
    }

    #[test]
    fn composite_diagnostic_actually_consumes_the_passed_exponents() {
        // A DIFFERENT exponent pair changes the composite — proving this function actually
        // consumes its parameters rather than a hard-coded 0.25/0.75. Prefill and decode gains
        // are DELIBERATELY UNEQUAL (4.0 vs 1.0): with equal gains any exponent pair summing to 1
        // collapses to the same value (x^a * x^(1-a) == x), which would make this test vacuous.
        let serial = stream_attestation(400_000, 100_000);
        let candidate = stream_attestation(100_000, 100_000); // prefill_gain=4.0, decode_gain=1.0
        let ruled = composite_diagnostic(
            &serial,
            &candidate,
            PREFILL_GAIN_EXPONENT,
            DECODE_GAIN_EXPONENT,
        );
        let other = composite_diagnostic(&serial, &candidate, 0.5, 0.5);
        assert!((ruled.prefill_gain - 4.0).abs() < 1e-9);
        assert!((ruled.decode_gain - 1.0).abs() < 1e-9);
        assert!(
            (ruled.composite_score - 4f64.powf(0.25)).abs() < 1e-9,
            "4^0.25 * 1^0.75 = 4^0.25, got {}",
            ruled.composite_score
        );
        assert!(
            (other.composite_score - 4f64.powf(0.5)).abs() < 1e-9,
            "4^0.5 * 1^0.5 = 4^0.5 = 2, got {}",
            other.composite_score
        );
        assert!(
            (ruled.composite_score - other.composite_score).abs() > 0.1,
            "different exponent pairs must produce visibly different composites here"
        );
    }

    #[test]
    fn composite_diagnostic_is_nan_on_a_degenerate_sum() {
        // A zero candidate sum (division-by-zero territory) is guarded the SAME way
        // `crate::score::speedup` guards it — 0.0 gain, not a panic or an infinity — and the
        // composite is NaN rather than a fabricated number, mirroring `score()`'s own NaN
        // posture for a rejected component.
        let serial = stream_attestation(100_000, 100_000);
        let candidate = stream_attestation(0, 100_000);
        let diag = composite_diagnostic(
            &serial,
            &candidate,
            PREFILL_GAIN_EXPONENT,
            DECODE_GAIN_EXPONENT,
        );
        assert_eq!(diag.prefill_gain, 0.0);
        assert!(diag.composite_score.is_nan(), "{}", diag.composite_score);
    }
}
