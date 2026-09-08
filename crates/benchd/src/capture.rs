//! B5 — `--capture-baseline`: the fail-closed OFFICIAL-BASELINE CAPTURE MODE.
//!
//! B1 made the official baseline one pair per `track_id`
//! ([`bench_core::constants::official_baseline`]), and a track with no captured pair is
//! [`bench_core::constants::OFFICIAL_BASELINE_PENDING`] — the accessor refuses by name instead
//! of falling back. That closes the "a new track inherits another track's numbers" hole and
//! opens a smaller one: the local checked-timing legs resolve the pair BEFORE they spawn
//! anything (`main::run_baselines`), so a PENDING track could never run the very measurement
//! that ends its pending state. The pair could not be captured through a scored run.
//!
//! This mode inverts that gate the ONE safe way:
//!
//! * it runs ONLY while the track's pair is PENDING ([`refuse_unless_pending`]) — a CAPTURED
//!   track refuses the mode by name, so the mode can never double as a scoring bypass;
//! * it writes ONLY the capture record — the caller returns before the score and integrity
//!   writers, so no artifact a scored run would produce exists;
//! * the record is values-only: the identity ([`CaptureIdentity`]), every pass's
//!   parent-measured pair, the run count, and the per-axis CVs.
//!
//! The record is the INPUT to a later, reviewed sentinel→value PR against
//! `OFFICIAL_BASELINES_BY_TRACK`. It is not itself a baseline: nothing reads it back.
//!
//! CV DEFINITION: the SAMPLE coefficient of variation, as a percent — the standard deviation
//! with the N−1 denominator, divided by the mean. Sample, not population: the capture passes
//! are a sample of the box's run-to-run distribution, and N is small. The tests here pin that
//! definition.

use crate::score::ScoreMetrics;
use serde::{Deserialize, Serialize};

/// One capture pass's parent-measured pair (seconds per token, both axes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureRun {
    pub prefill_seconds_per_token: f64,
    pub decode_seconds_per_token: f64,
}

/// What every pass of one capture session must agree on. A mismatch on ANY field refuses the
/// merge BY NAME.
///
/// The record's only purpose is to state, to a later reviewer, WHICH configuration produced the
/// pair. Everything the pair depends on therefore has to be here, or two passes over different
/// inputs would average silently into a mean that describes neither of them: different WEIGHTS
/// (a different checkpoint measures differently), a different DECODE WINDOW (`local-submit`'s
/// 1023 steps are not `local-iterate`'s 128, and per-token cost differs across the window), or a
/// different BENCHD (the parent does the timing, so the harness that measured is part of the
/// measurement).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureIdentity {
    pub track_id: String,
    pub mode: String,
    /// The mode's decode window in steps (`Mode::decode_steps`). `mode` already implies it
    /// today, but it is sealed as its own value: the window is what the pair actually depends
    /// on, and a mode whose window changes must not merge into a record measured at the old one.
    pub decode_steps: i64,
    pub engine_sha256: String,
    pub weights_sha256: String,
    pub golden_sha256: String,
    pub benchd_sha256: String,
}

/// The capture record file: identity + every pass + the derived statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRecord {
    #[serde(flatten)]
    pub identity: CaptureIdentity,
    pub run_count: usize,
    pub runs: Vec<CaptureRun>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefill_cv_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode_cv_percent: Option<f64>,
}

/// The mode's ARMING gate: `--capture-baseline` runs ONLY while the baseline this run would
/// score against is PENDING.
///
/// `captured` is the declared state, passed as a VALUE so both controls are testable: the
/// negative one (a captured track refuses) is the live state of the track `main` serves, and the
/// positive one (a pending track admits the mode) is the state a newly cut track branch is in.
///
/// `pending_sentinel` is the NAME of the pending state the caller resolves by, so the refusal
/// quotes the sentinel an operator can actually grep for. Today one resolution path reaches this
/// gate and it carries the per-track table's
/// [`bench_core::constants::OFFICIAL_BASELINE_PENDING`]; the sentinel stays a parameter so a
/// second resolution can name its own state rather than borrow this one's.
pub fn refuse_unless_pending(
    track_id: &str,
    captured: bool,
    pending_sentinel: &str,
) -> Result<(), String> {
    if captured {
        return Err(format!(
            "--capture-baseline refused: track {track_id:?} is not {pending_sentinel} — its \
             official baseline pair is already captured, so there is nothing to capture. The \
             capture mode never runs on a captured track and never doubles as a scoring bypass"
        ));
    }
    Ok(())
}

/// The EXACT-MATCH name of the refusal "this track measures its own denominator, so it has no
/// pair to capture".
pub const CAPTURE_RETIRED_FOR_LIVE_CONTROL_LEG: &str = "CAPTURE-RETIRED-FOR-LIVE-CONTROL-LEG";

/// The mode's DESIGN gate: `--capture-baseline` captures a STORED pair, and a
/// [`bench_core::constants::LIVE_CONTROL_LEG_TRACKS`] track stores none — its ranked run measures
/// a serial-control leg on the box instead (David 2026-09-08). Such a track refuses the mode BY
/// NAME and is pointed at the verb that replaced it, rather than being allowed to write a record
/// nothing can consume.
pub fn refuse_live_control_leg_track(track_id: &str) -> Result<(), String> {
    if bench_core::constants::scores_against_live_control_leg(track_id) {
        return Err(format!(
            "{CAPTURE_RETIRED_FOR_LIVE_CONTROL_LEG}: track {track_id:?} scores against a \
             serial-control leg measured on the box in the same job, so it stores no baseline pair \
             and there is nothing for --capture-baseline to capture; what this box needs is its \
             health band — run `benchd calibrate-baseline`"
        ));
    }
    Ok(())
}

/// The mode's ENGINE-IDENTITY gate: the record pins the engine by sha256, so a run whose engine
/// binary did not resolve is refused BEFORE anything spawns. An unresolved engine is fine for a
/// SCORED run — `resolve_runner_identity` is deliberately total and seals the weaker identity,
/// saying so in `candidate_executable_resolution` — but it is not fine here, because the whole
/// value of the record is that a later reviewer can tell WHICH binary produced the pair.
///
/// The gate keys on the RESOLUTION SENTINEL (`main::ENGINE_RESOLUTION_UNRESOLVED`), which is the
/// field whose job is to say whether the identity is trustworthy. An empty digest is the second
/// check, not the first: it is a consequence of the unresolved state today, so keying on it
/// would work by coincidence and would stop working the moment a resolution class seals an empty
/// digest for some other reason. Both are checked, and each refuses in its own words — a
/// resolution that claims to be canonical while carrying no digest is incoherent, and a record
/// must not absorb an incoherent identity either.
pub fn refuse_unresolved_engine(
    engine_path: &str,
    resolution: &str,
    engine_sha256: &str,
) -> Result<(), String> {
    if resolution.trim() == crate::ENGINE_RESOLUTION_UNRESOLVED {
        return Err(format!(
            "--capture-baseline refused: the engine {engine_path:?} did not resolve to a \
             readable binary (candidate_executable_resolution = {resolution:?}), so its sha256 \
             is unknown. The capture record pins the engine by sha256 and never records a pair \
             whose engine identity cannot be named"
        ));
    }
    if engine_sha256.trim().is_empty() {
        return Err(format!(
            "--capture-baseline refused: the engine {engine_path:?} reports \
             candidate_executable_resolution = {resolution:?} but carries no sha256. An identity \
             that claims to be resolved and names no digest is incoherent; the capture record \
             never absorbs one"
        ));
    }
    Ok(())
}

/// The mode's CORRECTNESS gate: a baseline is captured only from a healthy stock run.
pub fn refuse_unless_correctness_passed(metrics: &ScoreMetrics) -> Result<(), String> {
    if !metrics.passed_correctness {
        return Err(format!(
            "--capture-baseline refused: the correctness gate failed ({:?}); a baseline is \
             captured only from a healthy stock run",
            metrics.error
        ));
    }
    Ok(())
}

/// The mode's TIMED-PHASE gate: capture only a pass whose timed phase actually measured
/// something.
///
/// A timing failure — a cool-gate stall abort, a prefill/decode token mismatch, a worker spawn
/// error — produces a payload with `passed_correctness = TRUE` (the correctness gate had
/// already passed), a ZEROED pair, and the real cause in `metrics.error`.
/// [`refuse_unless_correctness_passed`] therefore admits it, and [`merge`] then refuses it as
/// "not a finite positive value" — a correct refusal that names the SYMPTOM and hides the CAUSE.
/// This gate refuses first and quotes `metrics.error` verbatim, so the operator reads the stall
/// text, not the zero.
///
/// FAIL-CLOSED on the error text: the ONLY error a captured pass may carry is
/// [`crate::iterate::INVALID_LOCAL_SCORE_ERROR`], which a HEALTHY capture pass ALWAYS carries,
/// because the mode runs with inert `(0.0, 0.0)` baselines by construction and
/// `iterate::local_iterate_score` sets that exact text whenever the denominator is not finite
/// and positive. A real timed failure returns before that text is ever set, so any other text
/// refuses. [`merge`]'s non-finite refusal stays behind this as the backstop.
pub fn refuse_unless_timed_phase_succeeded(metrics: &ScoreMetrics) -> Result<(), String> {
    let error = metrics.error.trim();
    if !(error.is_empty() || error == crate::iterate::INVALID_LOCAL_SCORE_ERROR) {
        return Err(format!("capture refused: timed phase failed: {error}"));
    }
    for (axis, v) in [
        ("prefill", metrics.prefill_seconds_per_token),
        ("decode", metrics.decode_seconds_per_token),
    ] {
        if !(v.is_finite() && v > 0.0) {
            return Err(format!(
                "capture refused: timed phase failed: the run reported no timing error, but its \
                 {axis} seconds-per-token is {v}, not a finite positive value"
            ));
        }
    }
    Ok(())
}

/// The SAMPLE coefficient of variation as a percent (N−1 denominator; see the module doc).
/// `None` when fewer than two values exist (the statistic is undefined) — never `0.0`, which
/// would read as "perfectly stable".
pub fn sample_cv_percent(values: &[f64]) -> Option<f64> {
    if values.len() < 2 {
        return None;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    if !(mean.is_finite() && mean > 0.0) {
        return None;
    }
    let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / (n - 1.0);
    Some(variance.sqrt() / mean * 100.0)
}

/// The EXACT-MATCH name of the refusal "the engine speculated on a leg that must be SERIAL".
pub const CALIBRATION_SPEC_ARMED: &str = "CALIBRATION-SPEC-ARMED";

/// The mode's SERIAL-LEG gate: a capture pass must have measured the SERIAL path.
///
/// The official baseline is the serial denominator every scored MTP leg is divided by, so a pair
/// measured while the engine speculated is not that number. benchd requests no spec on a capture
/// leg (`run_capture_passes_over_session` passes `None`, and `--capture-baseline` refuses the spec
/// flags at parse), but the REQUEST is only half of it: a resident serve started with speculation
/// on drafts anyway, and benchd would seal that leg as the serial reference. The engine's own
/// `effective_spec` ECHO is what says which path actually ran — `seal_timing_surface_facts` writes
/// it into `effective_spec_mode`/`effective_spec_depth` on every timed leg, echoed or defaulted —
/// so the gate reads the echo, not the request.
///
/// A leg with no echo AT ALL is `None`/`None`, which is a leg that sealed no timing surface; that
/// is [`refuse_unless_timed_phase_succeeded`]'s case, not this one, and it passes here.
pub fn refuse_spec_armed_engine(metrics: &ScoreMetrics) -> Result<(), String> {
    let mode = metrics.effective_spec_mode.as_deref().unwrap_or_default();
    let depth = metrics.effective_spec_depth.unwrap_or_default();
    if (mode.is_empty() || mode == bench_protocol::SPEC_MODE_SERIAL) && depth == 0 {
        return Ok(());
    }
    Err(format!(
        "{CALIBRATION_SPEC_ARMED}: the engine echoed effective_spec mode {mode:?} depth {depth} on \
         a capture leg, but the official baseline is the SERIAL denominator every scored \
         speculative leg is divided by. benchd requested no spec, so the engine is armed on its own \
         (a serve started with speculation on); restart it serial and re-run the calibration"
    ))
}

/// THE ONE WAY a timed payload becomes a [`CaptureRun`].
///
/// Both recorders — the single-pass `--capture-baseline` branch and the `--capture-passes` window
/// — used to apply the per-leg gates as loose statements and then build the pair from
/// `payload.metrics` themselves. That made the gates DELETABLE: removing both calls left the
/// recorders compiling and the whole suite green, because nothing downstream needed the gates to
/// have run. A gate a caller may forget is documentation, not a gate.
///
/// So the gates now PRODUCE the value the recorder needs. There is no other way to get a
/// `CaptureRun` out of a payload, which makes the guarded form the only form there is (the same
/// shape `bench_core::constants::official_baseline` uses for the baseline pair): delete the gate
/// from this function and [`refuse_spec_armed_engine`]'s own tests go red; delete the CALL from a
/// recorder and it no longer compiles, because it has no pair to record.
///
/// ORDER MATTERS. [`refuse_unless_timed_phase_succeeded`] runs FIRST so a leg that failed its
/// timing (cool-gate stall, token mismatch, worker spawn error) reports its real cause rather
/// than being described by the echo of a leg that never finished.
pub fn capture_run_from(metrics: &ScoreMetrics) -> Result<CaptureRun, String> {
    refuse_unless_timed_phase_succeeded(metrics)?;
    refuse_spec_armed_engine(metrics)?;
    Ok(CaptureRun {
        prefill_seconds_per_token: metrics.prefill_seconds_per_token,
        decode_seconds_per_token: metrics.decode_seconds_per_token,
    })
}

/// The arithmetic MEAN of a calibration's legs — the value that becomes the pinned pair.
/// `None` for an empty set or a non-finite result, so nothing numeric stands in for "no legs".
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    mean.is_finite().then_some(mean)
}

/// EVERY identity field, as `(name, rendered value)` — the ONE roster both the presence check and
/// the drift check walk, so neither can silently stop covering a field the other covers, and a
/// field added to [`CaptureIdentity`] cannot escape both by being added to neither.
///
/// `decode_steps` is rendered rather than borrowed so it rides the same roster as the text
/// fields. Its PRESENCE rule cannot be expressed as "non-empty" — `0` renders as a perfectly
/// non-empty `"0"` — so [`merge`] carries one explicit guard for it, next to this roster's loop.
fn identity_fields(identity: &CaptureIdentity) -> [(&'static str, String); 7] {
    [
        ("track_id", identity.track_id.clone()),
        ("mode", identity.mode.clone()),
        ("decode_steps", identity.decode_steps.to_string()),
        ("engine_sha256", identity.engine_sha256.clone()),
        ("weights_sha256", identity.weights_sha256.clone()),
        ("golden_sha256", identity.golden_sha256.clone()),
        ("benchd_sha256", identity.benchd_sha256.clone()),
    ]
}

/// Merge one pass into the record (or found a new record from it). Refuses a non-finite or
/// non-positive pair, an empty identity field, and any identity drift — each BY NAME.
pub fn merge(
    existing: Option<CaptureRecord>,
    identity: CaptureIdentity,
    run: CaptureRun,
) -> Result<CaptureRecord, String> {
    for (axis, v) in [
        ("prefill", run.prefill_seconds_per_token),
        ("decode", run.decode_seconds_per_token),
    ] {
        if !(v.is_finite() && v > 0.0) {
            return Err(format!(
                "capture run's {axis} seconds-per-token is {v}, not a finite positive value — a \
                 broken measurement must never enter the baseline record"
            ));
        }
    }
    // `decode_steps` presence, which the roster loop below cannot express (see `identity_fields`):
    // a zero-step window measured nothing, so it can never identify a pair.
    if identity.decode_steps <= 0 {
        return Err(format!(
            "capture identity field decode_steps is {} — a run with no decode window measured \
             nothing and must never enter the baseline record",
            identity.decode_steps
        ));
    }
    for (field, value) in identity_fields(&identity) {
        if value.trim().is_empty() {
            return Err(format!(
                "capture identity field {field} is empty — a run whose {field} is unknown must \
                 never enter the baseline record"
            ));
        }
    }
    let mut runs = match existing {
        None => Vec::new(),
        Some(record) => {
            for ((field, old), (_, new)) in identity_fields(&record.identity)
                .into_iter()
                .zip(identity_fields(&identity))
            {
                if old != new {
                    return Err(format!(
                        "capture record identity mismatch on {field}: the record was started \
                         with {old:?} but this run carries {new:?} — one record captures ONE \
                         track, mode, decode window, engine, weights, golden and benchd; start a \
                         new record file instead"
                    ));
                }
            }
            record.runs
        }
    };
    runs.push(run);
    let prefill: Vec<f64> = runs.iter().map(|r| r.prefill_seconds_per_token).collect();
    let decode: Vec<f64> = runs.iter().map(|r| r.decode_seconds_per_token).collect();
    Ok(CaptureRecord {
        identity,
        run_count: runs.len(),
        prefill_cv_percent: sample_cv_percent(&prefill),
        decode_cv_percent: sample_cv_percent(&decode),
        runs,
    })
}

/// Read-merge-write the capture record at `path`. The write is ATOMIC — temp file + rename — so
/// an interrupted pass can never leave a half-written record that the next pass merges into.
pub fn record_run(
    path: &std::path::Path,
    identity: CaptureIdentity,
    run: CaptureRun,
) -> Result<CaptureRecord, String> {
    let existing = match std::fs::read(path) {
        Ok(bytes) => Some(
            serde_json::from_slice::<CaptureRecord>(&bytes)
                .map_err(|e| format!("capture record at {} did not parse: {e}", path.display()))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(format!(
                "capture record read failed ({}): {e}",
                path.display()
            ))
        }
    };
    let record = merge(existing, identity, run)?;
    let json = serde_json::to_string_pretty(&record)
        .map_err(|e| format!("capture record serialize failed: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{json}\n"))
        .map_err(|e| format!("capture record write failed ({}): {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("capture record rename failed ({}): {e}", path.display()))?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> CaptureIdentity {
        CaptureIdentity {
            track_id: "gemma4-26b-a4b-mlx-v1".to_string(),
            mode: "local-iterate".to_string(),
            decode_steps: 128,
            engine_sha256: "e".repeat(64),
            weights_sha256: "w".repeat(64),
            golden_sha256: "g".repeat(64),
            benchd_sha256: "b".repeat(64),
        }
    }

    fn run(prefill: f64, decode: f64) -> CaptureRun {
        CaptureRun {
            prefill_seconds_per_token: prefill,
            decode_seconds_per_token: decode,
        }
    }

    /// POSITIVE control: a PENDING track admits the mode. NEGATIVE control: a CAPTURED track
    /// refuses BY NAME — the mode must never double as a scoring bypass.
    #[test]
    fn capture_runs_only_while_the_track_is_pending() {
        const PENDING: &str = bench_core::constants::OFFICIAL_BASELINE_PENDING;
        assert!(refuse_unless_pending("gemma4-26b-a4b-mlx-v1", false, PENDING).is_ok());
        let err = refuse_unless_pending("gemma4-26b-a4b-mlx-v1", true, PENDING).unwrap_err();
        assert!(err.contains("--capture-baseline refused"), "{err}");
        assert!(err.contains("gemma4-26b-a4b-mlx-v1"), "{err}");
        assert!(
            err.contains(bench_core::constants::OFFICIAL_BASELINE_PENDING),
            "the refusal must name the sentinel: {err}"
        );
        assert!(err.contains("never doubles as a scoring bypass"), "{err}");
    }

    /// The track `main` serves is CAPTURED, so on `main` the mode refuses. That is the
    /// fixture-inert proof: this PR cannot move any scored value on the served track, because
    /// the only new path it adds is closed there. The mode opens when a track is cut PENDING.
    #[test]
    fn the_track_main_serves_refuses_the_capture_mode_today() {
        let track = bench_core::constants::TRACK_ID;
        let captured = bench_core::constants::official_baseline(track).is_ok();
        assert!(
            captured,
            "main's track {track} is expected to carry a captured pair"
        );
        assert!(
            refuse_unless_pending(
                track,
                captured,
                bench_core::constants::OFFICIAL_BASELINE_PENDING
            )
            .is_err(),
            "a captured track must refuse the capture mode"
        );
    }

    /// The FUNNEL: `capture_run_from` is the only way a payload becomes a recorded pair, so every
    /// per-leg gate it runs is unskippable by construction.
    ///
    /// * a healthy SERIAL leg yields exactly the payload's pair;
    /// * an ARMED leg (the engine speculated without being asked) yields NO pair, naming
    ///   [`CALIBRATION_SPEC_ARMED`];
    /// * a leg whose TIMING failed is refused FIRST, with its real cause, rather than being
    ///   described by the spec echo of a leg that never finished.
    #[test]
    fn capture_run_from_is_the_only_door_and_every_gate_is_on_it() {
        let healthy = |mode: Option<&str>, depth: Option<i64>| ScoreMetrics {
            // The text a HEALTHY capture leg always carries: the mode runs with inert (0.0, 0.0)
            // baselines by construction, so the local score is invalid by design.
            error: crate::iterate::INVALID_LOCAL_SCORE_ERROR.to_string(),
            prefill_seconds_per_token: 0.000488,
            decode_seconds_per_token: 0.0645,
            effective_spec_mode: mode.map(str::to_string),
            effective_spec_depth: depth,
            ..Default::default()
        };

        // SERIAL — the pair comes straight off the payload.
        let run = capture_run_from(&healthy(
            Some(bench_protocol::SPEC_MODE_SERIAL),
            Some(0),
        ))
        .expect("a serial leg must yield a capture run");
        assert_eq!(run.prefill_seconds_per_token, 0.000488);
        assert_eq!(run.decode_seconds_per_token, 0.0645);

        // ARMED — no pair, and the refusal names the sentinel.
        let err = capture_run_from(&healthy(Some("mtp"), Some(2))).unwrap_err();
        assert!(err.contains(CALIBRATION_SPEC_ARMED), "{err}");

        // TIMED-PHASE FAILURE FIRST: a stalled leg carries a real cause AND (harmlessly) an armed
        // echo. The operator must read the stall, not the echo, so the order is pinned here.
        let mut stalled = healthy(Some("mtp"), Some(2));
        stalled.error = "cool-gate stall abort".to_string();
        stalled.prefill_seconds_per_token = 0.0;
        stalled.decode_seconds_per_token = 0.0;
        let err = capture_run_from(&stalled).unwrap_err();
        assert!(err.contains("cool-gate stall abort"), "{err}");
        assert!(
            !err.contains(CALIBRATION_SPEC_ARMED),
            "the timed-phase cause must answer first: {err}"
        );
    }

    /// The engine gate keys on the RESOLUTION SENTINEL, and refuses an incoherent identity
    /// (resolved-but-no-digest) separately. POSITIVE control: a canonical engine with a digest
    /// passes, so the gate does not defang the mode.
    #[test]
    fn capture_refuses_an_unresolvable_engine_by_name() {
        assert!(refuse_unresolved_engine(
            "/opt/engine",
            crate::ENGINE_RESOLUTION_CANONICAL,
            &"a".repeat(64)
        )
        .is_ok());

        // The sentinel is the key: an unresolved engine refuses even when some digest is
        // present, so the gate cannot be defanged by keying on the empty sha alone.
        for sha in ["", &"a".repeat(64)] {
            let err = refuse_unresolved_engine(
                "mlxfast-engine",
                crate::ENGINE_RESOLUTION_UNRESOLVED,
                sha,
            )
            .unwrap_err();
            assert!(err.contains("--capture-baseline refused"), "{err}");
            assert!(err.contains("mlxfast-engine"), "{err}");
            assert!(err.contains("did not resolve"), "{err}");
            assert!(
                err.contains(crate::ENGINE_RESOLUTION_UNRESOLVED),
                "the refusal must quote the resolution: {err}"
            );
        }

        // The second check: a resolution that claims to be resolved while naming no digest.
        for blank in ["", "   "] {
            let err =
                refuse_unresolved_engine("/opt/engine", crate::ENGINE_RESOLUTION_CANONICAL, blank)
                    .unwrap_err();
            assert!(err.contains("--capture-baseline refused"), "{err}");
            assert!(err.contains("incoherent"), "{err}");
        }
    }

    /// The correctness gate: a failed gate refuses and quotes the cause.
    #[test]
    fn capture_refuses_a_failed_correctness_gate_by_name() {
        let mut metrics = ScoreMetrics {
            passed_correctness: true,
            ..Default::default()
        };
        assert!(refuse_unless_correctness_passed(&metrics).is_ok());
        metrics.passed_correctness = false;
        metrics.error = "token mismatch at step 7".to_string();
        let err = refuse_unless_correctness_passed(&metrics).unwrap_err();
        assert!(err.contains("--capture-baseline refused"), "{err}");
        assert!(err.contains("token mismatch at step 7"), "{err}");
    }

    /// The CV is the SAMPLE statistic (N−1): `[0.9, 1.1]` → mean 1.0, s = 0.1·√2 → 14.142…%.
    /// The POPULATION form would give 10%. One value → `None` (undefined, never `0.0`).
    #[test]
    fn cv_is_the_sample_statistic() {
        assert_eq!(sample_cv_percent(&[]), None);
        assert_eq!(sample_cv_percent(&[1.0]), None);
        assert_eq!(sample_cv_percent(&[2.0, 2.0, 2.0]), Some(0.0));
        let cv = sample_cv_percent(&[0.9, 1.1]).expect("two values define the sample CV");
        assert!(
            (cv - 14.142135623730951).abs() < 1e-9,
            "sample (N−1) CV expected, got {cv}"
        );
    }

    #[test]
    fn merge_appends_and_recomputes_the_statistics() {
        let first = merge(None, identity(), run(0.9, 0.010)).unwrap();
        assert_eq!(first.run_count, 1);
        assert_eq!(first.runs.len(), 1);
        assert_eq!(first.prefill_cv_percent, None);
        assert_eq!(first.decode_cv_percent, None);

        let second = merge(Some(first), identity(), run(1.1, 0.010)).unwrap();
        assert_eq!(second.run_count, 2);
        assert_eq!(second.runs.len(), 2);
        assert!((second.prefill_cv_percent.unwrap() - 14.142135623730951).abs() < 1e-9);
        assert_eq!(second.decode_cv_percent, Some(0.0));
    }

    /// Identity drift refuses PER FIELD: every one of the four is checked, and the refusal
    /// names the field that drifted.
    #[test]
    fn merge_refuses_identity_drift_on_every_field_by_name() {
        let base = merge(None, identity(), run(1.0, 1.0)).unwrap();
        let drifted = [
            ("track_id", {
                let mut i = identity();
                i.track_id = "qwen3.8-27b-mtp-v1".to_string();
                i
            }),
            ("mode", {
                let mut i = identity();
                i.mode = "local-submit".to_string();
                i
            }),
            ("decode_steps", {
                let mut i = identity();
                i.decode_steps = 1023;
                i
            }),
            ("engine_sha256", {
                let mut i = identity();
                i.engine_sha256 = "f".repeat(64);
                i
            }),
            ("weights_sha256", {
                let mut i = identity();
                i.weights_sha256 = "x".repeat(64);
                i
            }),
            ("golden_sha256", {
                let mut i = identity();
                i.golden_sha256 = "h".repeat(64);
                i
            }),
            ("benchd_sha256", {
                let mut i = identity();
                i.benchd_sha256 = "y".repeat(64);
                i
            }),
        ];
        // The list above is EXHAUSTIVE over the roster `merge` actually walks. A field added to
        // the identity and to `identity_fields` but not drift-tested here fails this assertion,
        // and a field quietly dropped from the roster fails it too.
        let covered: Vec<&str> = drifted.iter().map(|(field, _)| *field).collect();
        let roster: Vec<&str> = identity_fields(&identity())
            .iter()
            .map(|(field, _)| *field)
            .collect();
        assert_eq!(covered, roster, "every identity field must be drift-tested");
        for (field, identity) in drifted {
            let err = merge(Some(base.clone()), identity, run(1.0, 1.0)).unwrap_err();
            assert!(err.contains("identity mismatch"), "{field}: {err}");
            assert!(err.contains(field), "the refusal must name {field}: {err}");
        }
    }

    #[test]
    fn merge_refuses_a_broken_pair_and_an_empty_identity_field() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            for pair in [run(bad, 1.0), run(1.0, bad)] {
                let err = merge(None, identity(), pair).unwrap_err();
                assert!(err.contains("not a finite positive value"), "{bad}: {err}");
            }
        }
        for field in [
            "track_id",
            "mode",
            "engine_sha256",
            "weights_sha256",
            "golden_sha256",
            "benchd_sha256",
        ] {
            let mut blanked = identity();
            match field {
                "track_id" => blanked.track_id = String::new(),
                "mode" => blanked.mode = "   ".to_string(),
                "engine_sha256" => blanked.engine_sha256 = String::new(),
                "weights_sha256" => blanked.weights_sha256 = "  ".to_string(),
                "golden_sha256" => blanked.golden_sha256 = String::new(),
                _ => blanked.benchd_sha256 = String::new(),
            }
            let err = merge(None, blanked, run(1.0, 1.0)).unwrap_err();
            assert!(err.contains(field), "{field}: {err}");
            assert!(err.contains("is empty"), "{field}: {err}");
        }
        // `decode_steps` carries its own presence rule: a zero-step window measured nothing.
        for steps in [0, -1] {
            let mut no_window = identity();
            no_window.decode_steps = steps;
            let err = merge(None, no_window, run(1.0, 1.0)).unwrap_err();
            assert!(err.contains("decode_steps"), "{steps}: {err}");
            assert!(err.contains("no decode window"), "{steps}: {err}");
        }
    }

    /// A payload whose TIMED PHASE failed: the correctness gate had already passed, so
    /// `passed_correctness` is TRUE, the real cause is in `metrics.error`, and the pair is
    /// zeroed (the failure builder never applies timing metrics).
    fn failed_timing_metrics(error: &str) -> ScoreMetrics {
        ScoreMetrics {
            passed_correctness: true,
            error: error.to_string(),
            prefill_seconds_per_token: 0.0,
            decode_seconds_per_token: 0.0,
            ..Default::default()
        }
    }

    /// The defect this gate exists for: a TIMING-phase failure reaches the merge with
    /// `passed_correctness = true` and a zeroed pair, so without this gate the operator sees
    /// only "not a finite positive value" and never the real cause. The gate refuses FIRST and
    /// quotes `metrics.error` verbatim.
    #[test]
    fn capture_refuses_a_failed_timed_phase_and_quotes_the_cause() {
        let stall = "gate rejected (prefill): GPU is hot and not cooling down (current 52.0C, \
                     min seen 51.0C, target <=40C, waited 900s)";
        let err = refuse_unless_timed_phase_succeeded(&failed_timing_metrics(stall)).unwrap_err();
        assert!(err.contains("capture refused: timed phase failed"), "{err}");
        assert!(err.contains(stall), "the cause must appear verbatim: {err}");

        let spawn =
            "failed to spawn engine \"/opt/engine\": No such file or directory (os error 2)";
        let err = refuse_unless_timed_phase_succeeded(&failed_timing_metrics(spawn)).unwrap_err();
        assert!(err.contains(spawn), "{err}");
    }

    /// POSITIVE control — the gate must not defang the mode. A HEALTHY capture pass carries a
    /// real pair AND the invalid-local-score text, because the mode runs with inert `(0.0, 0.0)`
    /// baselines by construction. That exact text is benign; anything else is not.
    #[test]
    fn capture_admits_a_healthy_pass_whose_only_error_is_the_inert_score() {
        let healthy = ScoreMetrics {
            passed_correctness: true,
            error: crate::iterate::INVALID_LOCAL_SCORE_ERROR.to_string(),
            prefill_seconds_per_token: 0.9,
            decode_seconds_per_token: 0.012,
            ..Default::default()
        };
        assert!(refuse_unless_timed_phase_succeeded(&healthy).is_ok());

        let mut no_error = healthy.clone();
        no_error.error = String::new();
        assert!(refuse_unless_timed_phase_succeeded(&no_error).is_ok());

        // NEGATIVE control: a real pair does NOT license an unexpected error text.
        let mut other_error = healthy.clone();
        other_error.error = "engine hello handshake failed: unexpected eof".to_string();
        let err = refuse_unless_timed_phase_succeeded(&other_error).unwrap_err();
        assert!(err.contains("unexpected eof"), "{err}");

        // NEGATIVE control: a benign error text does NOT license a zeroed pair.
        for zeroed in ["prefill", "decode"] {
            let mut metrics = healthy.clone();
            if zeroed == "prefill" {
                metrics.prefill_seconds_per_token = 0.0;
            } else {
                metrics.decode_seconds_per_token = 0.0;
            }
            let err = refuse_unless_timed_phase_succeeded(&metrics).unwrap_err();
            assert!(err.contains("timed phase failed"), "{err}");
            assert!(err.contains(zeroed), "{err}");
        }
    }

    /// The writer round-trips: a second pass READS the record back, merges, and rewrites it,
    /// and the identity drift refusal survives the round trip.
    #[test]
    fn record_run_merges_repeated_passes_through_the_file() {
        let dir = std::env::temp_dir().join(format!("benchd-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("official-baseline.json");
        let _ = std::fs::remove_file(&path);

        let first = record_run(&path, identity(), run(0.9, 0.010)).unwrap();
        assert_eq!(first.run_count, 1);
        let second = record_run(&path, identity(), run(1.1, 0.010)).unwrap();
        assert_eq!(second.run_count, 2);

        let on_disk: CaptureRecord =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk, second);
        // The identity is FLAT in the record (a reader greps `track_id`, not `identity.track_id`).
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["track_id"], "gemma4-26b-a4b-mlx-v1");
        assert_eq!(raw["decode_steps"], 128);
        assert_eq!(raw["weights_sha256"], "w".repeat(64));
        assert_eq!(raw["benchd_sha256"], "b".repeat(64));
        assert_eq!(raw["run_count"], 2);

        let mut drifted = identity();
        drifted.engine_sha256 = "f".repeat(64);
        let err = record_run(&path, drifted, run(1.0, 0.010)).unwrap_err();
        assert!(err.contains("engine_sha256"), "{err}");
        // The refused pass left the record untouched.
        let after: CaptureRecord = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, second);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
