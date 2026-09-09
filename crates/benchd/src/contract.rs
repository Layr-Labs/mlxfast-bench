//! The `--contract` track fixture and the David 2026-08-26 ARM GATE.
//!
//! Moved out of the retired measure-job (`measure_job.rs`) so the SOLE scored path — flow A,
//! `benchd iterate --mode official` — inherits the arm gate. The struct and the gate behavior
//! are byte-identical to the measure-job originals; only the fields the surviving official path
//! consumes (`track_id`, `official_scoring_enabled`) are modelled. serde ignores every other
//! fixture key exactly as before (no `deny_unknown_fields`), so the real track fixtures — which
//! still carry `timed_prompt_pool`, `allowed_modes`, `calibration`, … — parse unchanged.

use bench_core::score::SpeedupFloors;
use serde::Deserialize;

/// The parsed `--contract` track fixture. Only the fields the official scored path consumes are
/// modelled — serde ignores the rest, so a fixture carrying the full measure-job schema
/// (`timed_prompt_pool`, `scored_batch_size`, `allowed_modes`, …) still parses.
#[derive(Debug, Clone, Deserialize)]
pub struct Contract {
    /// The track fixture's own workflow-declared track id (e.g. `qwen3.8-125b-a6b-mlx-v1`).
    #[serde(default)]
    pub track_id: Option<String>,
    /// David ruling (2026-08-26) — the track's ARM STATE, and the one contract field that decides
    /// whether benchd may seal a SCORING artifact for this track at all
    /// ([`enforce_official_scoring_enabled`]).
    ///
    /// `Option<bool>` rather than `bool`, deliberately: ABSENT and `false` are BOTH refusals, but
    /// they are DIFFERENT diagnoses (a track that has not been armed yet vs. a fixture that never
    /// declares an arm state at all) and the refusal says which. Collapsing them into
    /// `#[serde(default)] bool` would make a fixture that forgot the key indistinguishable from one
    /// that deliberately declared `false` — and, worse, would make ABSENCE look like a decision.
    /// Absence is never armed.
    #[serde(default)]
    pub official_scoring_enabled: Option<bool>,
    /// PAIRS PER SCORED RUN on the paired per-box path (David 2026-09-09: "2 pairs on both mlx and
    /// cuda"; "1 pair is not sufficient"). Each pair is one serial-control leg on the reference
    /// tree followed by one candidate leg, same prompt, same box. The fixture is the ONLY source
    /// of this count: no flag, no environment, no default — so a box cannot silently run fewer
    /// pairs than the track declares.
    #[serde(default)]
    pub official_pairs: Option<u32>,
    /// THE DECODE SPEEDUP FLOOR this project's scored run must clear (David 2026-09-09: 0.95).
    /// The fixture is the ONLY source on the scoring path — no flag, no environment, no default —
    /// so a track cannot be scored against a floor it never declared, and each project sets its
    /// own. See [`speedup_floors`].
    #[serde(default)]
    pub decode_speedup_floor: Option<f64>,
    /// THE PREFILL SPEEDUP FLOOR this project's scored run must clear (David 2026-09-09: 0.95).
    /// Declared and enforced exactly as [`Contract::decode_speedup_floor`]; the prefill axis is a
    /// floor of its own, not a decode side effect.
    #[serde(default)]
    pub prefill_speedup_floor: Option<f64>,
}

/// The scored run's speedup floors, or the refusal naming what the fixture must declare.
///
/// Both floors are REQUIRED and each is refused on its own: an absent floor is not 0.95, and a
/// fixture that declares one axis has still not declared the other. A declared value must be
/// finite and positive — a floor of 0, NaN or a negative number gates nothing.
pub fn speedup_floors(contract: &Contract, track_id: &str) -> Result<SpeedupFloors, String> {
    Ok(SpeedupFloors {
        decode: one_floor(
            contract.decode_speedup_floor,
            "decode_speedup_floor",
            track_id,
        )?,
        prefill: one_floor(
            contract.prefill_speedup_floor,
            "prefill_speedup_floor",
            track_id,
        )?,
    })
}

/// One axis of [`speedup_floors`].
fn one_floor(declared: Option<f64>, field: &str, track_id: &str) -> Result<f64, String> {
    match declared {
        Some(v) if v.is_finite() && v > 0.0 => Ok(v),
        Some(v) => Err(format!(
            "the --contract track fixture for {track_id:?} declares {field}: {v}; a speedup floor \
             must be finite and greater than 0 (David 2026-09-09 ruled 0.95/0.95)"
        )),
        None => Err(format!(
            "the --contract track fixture for {track_id:?} declares no {field}; the official \
             scored run refuses to guess a speedup floor (David 2026-09-09 ruled 0.95/0.95, \
             configurable per project) — pin it in the fixture"
        )),
    }
}

/// The paired path's pair count, or the refusal naming what the fixture must declare.
pub fn official_pairs(contract: &Contract, track_id: &str) -> Result<usize, String> {
    match contract.official_pairs {
        Some(n) if n >= 1 => Ok(n as usize),
        Some(n) => Err(format!(
            "the --contract track fixture for {track_id:?} declares official_pairs: {n}; the paired \
             official run needs at least 1 pair (David 2026-09-09 ruled 2)"
        )),
        None => Err(format!(
            "the --contract track fixture for {track_id:?} declares no official_pairs; the paired \
             official run refuses to guess a pair count (David 2026-09-09 ruled 2 on both \
             platforms) — pin it in the fixture"
        )),
    }
}

impl Contract {
    /// Parse a `--contract` file's bytes, FAIL-CLOSED on malformed JSON (never fall open).
    pub fn parse(bytes: &[u8]) -> Result<Contract, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("--contract parse failed: {e}"))
    }
}

/// David ruling (2026-08-26) — the ARM GATE: refuse a SCORING/ranked run whose `--contract`
/// track fixture does not declare `official_scoring_enabled: true`.
///
/// `scoring_mode` is the SAME signal every other scoring-vs-local decision keys on: a run is a
/// scoring run here exactly when it is an official (non `--local-dev`) run. It is deliberately NOT
/// a second, parallel notion of "official".
///
/// The three refusable states are kept DISTINCT in the message because they need different actions:
///
/// * `Some(true)` — armed. Proceed; this is the ONLY accepting state.
/// * `Some(false)` — declared UNARMED. The track exists and is being brought up; the fix is to
///   iterate with a local mode (or wait for the arm), never to edit the fixture locally.
/// * `None` — the fixture declares no arm state. FAIL-CLOSED, identically to `false`: a contract
///   that never says it is armed is not armed. This is the half that matters most — the flag was
///   invisible to benchd for its whole life, so "the key is simply missing" is the likeliest way a
///   track would otherwise slip into scoring unarmed.
///
/// Pure and total: it reads three values and returns a verdict, so the whole truth table is unit
/// testable without a box, a GPU, or a contract file.
pub fn enforce_official_scoring_enabled(
    scoring_mode: bool,
    official_scoring_enabled: Option<bool>,
    track_id: &str,
) -> Result<(), String> {
    // LOCAL modes are untouched, on purpose and load-bearing: the whole point of the unarmed
    // period is that participants and organizers can iterate against the real harness before the
    // track opens. Gating a local run would make the flag's `false` state mean "this track is
    // unusable", which is the opposite of what it is for.
    if !scoring_mode {
        return Ok(());
    }
    match official_scoring_enabled {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "official scoring is not enabled for this track: the --contract track fixture for \
             {track_id:?} declares official_scoring_enabled: false, so benchd refuses to seal an \
             official/ranked scoring artifact for it. This is the track's ARM STATE and only the \
             track fixture may change it — pass --local-dev to iterate against the unarmed track \
             (no scoring seal), or wait for the track to be armed."
        )),
        None => Err(format!(
            "official scoring is not enabled for this track: the --contract track fixture for \
             {track_id:?} declares NO official_scoring_enabled at all, and an absent arm state is \
             NOT an armed one (fail-closed) — benchd refuses to seal an official/ranked scoring \
             artifact for it. Add official_scoring_enabled: true to the track fixture to arm it, \
             or pass --local-dev to iterate against the unarmed track (no scoring seal)."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract parses `official_scoring_enabled` as a TRI-STATE, and the three states stay
    /// DISTINCT: ABSENT survives as `None` (not collapse into `false`) so the refusal can tell the
    /// two apart.
    #[test]
    fn contract_parses_official_scoring_enabled_as_a_tri_state() {
        let armed =
            Contract::parse(br#"{"track_id":"t","official_scoring_enabled":true}"#).unwrap();
        assert_eq!(armed.official_scoring_enabled, Some(true));

        let unarmed =
            Contract::parse(br#"{"track_id":"t","official_scoring_enabled":false}"#).unwrap();
        assert_eq!(unarmed.official_scoring_enabled, Some(false));

        // ABSENT stays ABSENT — it must not read back as `false`, because the two refusals say
        // different things.
        let silent = Contract::parse(br#"{"track_id":"t"}"#).unwrap();
        assert_eq!(silent.official_scoring_enabled, None);
    }

    /// A fixture carrying the FULL measure-job schema still parses — serde ignores the keys the
    /// surviving official path does not model (no `deny_unknown_fields`), so a real track fixture
    /// with `timed_prompt_pool`/`allowed_modes` reads its arm state exactly as a trimmed one.
    #[test]
    fn contract_ignores_unmodelled_fixture_keys() {
        let full = Contract::parse(
            br#"{"track_id":"t","official_scoring_enabled":true,
                 "timed_prompt_pool":[{"sha256":"ab","bytes":3}],
                 "allowed_modes":["mtp"],"scored_batch_size":8}"#,
        )
        .unwrap();
        assert_eq!(full.official_scoring_enabled, Some(true));
        assert_eq!(full.track_id.as_deref(), Some("t"));
    }

    /// Malformed JSON is a FAIL-CLOSED parse error, never a fall-open default.
    #[test]
    fn contract_parse_fails_closed_on_malformed() {
        assert!(Contract::parse(br#"{"track_id": "#).is_err());
        assert!(Contract::parse(b"not json").is_err());
    }

    /// ARM GATE — the whole truth table of [`enforce_official_scoring_enabled`], the pure decision
    /// the official path's pre-GPU call site is a thin wrapper over.
    ///
    /// REVERT-PROOF three ways. Delete the gate (always `Ok`) and both scoring refusals go red.
    /// Invert it (accept `false`/absent, refuse `true`) and every arm of this table goes red.
    /// Make it warn-only (`eprintln!` + `Ok`) and the two `is_err()` arms go red.
    #[test]
    fn official_scoring_arm_gate_truth_table() {
        // ARMED — the ONLY accepting scoring state.
        assert!(enforce_official_scoring_enabled(true, Some(true), "t").is_ok());

        // DECLARED UNARMED — refuses, names the flag, and points at the local escape hatch.
        let declared_false = enforce_official_scoring_enabled(true, Some(false), "gemma4-track")
            .expect_err("a scoring run over an unarmed track must refuse");
        assert!(
            declared_false.contains("official scoring is not enabled for this track"),
            "the refusal must lead with the ruled wording: {declared_false}"
        );
        assert!(
            declared_false.contains("official_scoring_enabled")
                && declared_false.contains("gemma4-track"),
            "the refusal must NAME the flag and the track: {declared_false}"
        );
        assert!(
            declared_false.contains("--local-dev"),
            "the refusal must name the un-gated local path: {declared_false}"
        );

        // ABSENT — fail-closed, identically refused, but diagnosed differently: this fixture never
        // declared an arm state, so the remedy is to ADD the key, not to wait for a flip.
        let absent = enforce_official_scoring_enabled(true, None, "silent-track")
            .expect_err("absence is not armed");
        assert!(
            absent.contains("official scoring is not enabled for this track")
                && absent.contains("official_scoring_enabled"),
            "the absent-case refusal must carry the same named verdict: {absent}"
        );
        assert_ne!(
            absent, declared_false,
            "absent and false must not produce the SAME message — they need different actions"
        );

        // LOCAL — the load-bearing NEGATIVE control. A non-scoring run is not gated, so NONE of the
        // three contract states may refuse it: iterating against an unarmed track is the entire
        // purpose of the unarmed period, and a gate that blocked it would invert the flag's meaning
        // from "not scoring yet" into "unusable".
        for state in [Some(true), Some(false), None] {
            assert!(
                enforce_official_scoring_enabled(false, state, "t").is_ok(),
                "a non-scoring run must be unaffected by official_scoring_enabled = {state:?}"
            );
        }
    }
}

#[cfg(test)]
mod official_pairs_tests {
    use super::*;

    #[test]
    fn the_pair_count_comes_from_the_fixture_alone() {
        let two =
            Contract::parse(br#"{"official_scoring_enabled": true, "official_pairs": 2}"#).unwrap();
        assert_eq!(official_pairs(&two, "t"), Ok(2));
        let absent = Contract::parse(br#"{"official_scoring_enabled": true}"#).unwrap();
        let err = official_pairs(&absent, "qwen3.8-125b-a6b-cuda-v1").unwrap_err();
        assert!(err.contains("declares no official_pairs"), "{err}");
        assert!(err.contains("qwen3.8-125b-a6b-cuda-v1"), "{err}");
        let zero =
            Contract::parse(br#"{"official_scoring_enabled": true, "official_pairs": 0}"#).unwrap();
        let err = official_pairs(&zero, "t").unwrap_err();
        assert!(err.contains("official_pairs: 0"), "{err}");
    }
}

#[cfg(test)]
mod speedup_floor_tests {
    use super::*;

    /// The floors come from the FIXTURE alone, one axis at a time, and an absent or unusable
    /// value is a refusal that names the field and the track — never a silent 0.95.
    #[test]
    fn the_floors_come_from_the_fixture_alone() {
        let ruled = Contract::parse(
            br#"{"official_scoring_enabled": true, "official_pairs": 2,
                 "decode_speedup_floor": 0.95, "prefill_speedup_floor": 0.95}"#,
        )
        .unwrap();
        assert_eq!(
            speedup_floors(&ruled, "t"),
            Ok(SpeedupFloors {
                decode: 0.95,
                prefill: 0.95
            })
        );

        // A project may declare its own pair; benchd enforces what the fixture says.
        let per_project =
            Contract::parse(br#"{"decode_speedup_floor": 0.90, "prefill_speedup_floor": 0.80}"#)
                .unwrap();
        assert_eq!(
            speedup_floors(&per_project, "t"),
            Ok(SpeedupFloors {
                decode: 0.90,
                prefill: 0.80
            })
        );

        // BOTH are required, and each refusal names its own field.
        let decode_only = Contract::parse(br#"{"decode_speedup_floor": 0.95}"#).unwrap();
        let err = speedup_floors(&decode_only, "qwen3.8-125b-a6b-cuda-v1").unwrap_err();
        assert!(err.contains("declares no prefill_speedup_floor"), "{err}");
        assert!(err.contains("qwen3.8-125b-a6b-cuda-v1"), "{err}");

        let neither = Contract::parse(br#"{"official_scoring_enabled": true}"#).unwrap();
        let err = speedup_floors(&neither, "t").unwrap_err();
        assert!(err.contains("declares no decode_speedup_floor"), "{err}");
        assert!(err.contains("David 2026-09-09"), "{err}");

        // Non-finite / non-positive declarations gate nothing, so they are refused too.
        for body in [
            &br#"{"decode_speedup_floor": 0.0, "prefill_speedup_floor": 0.95}"#[..],
            &br#"{"decode_speedup_floor": -0.5, "prefill_speedup_floor": 0.95}"#[..],
        ] {
            let c = Contract::parse(body).unwrap();
            let err = speedup_floors(&c, "t").unwrap_err();
            assert!(err.contains("finite and greater than 0"), "{err}");
        }
        // JSON has no NaN literal; a non-finite value reaches the resolver only as a struct.
        let nan = Contract {
            track_id: None,
            official_scoring_enabled: None,
            official_pairs: None,
            decode_speedup_floor: Some(f64::NAN),
            prefill_speedup_floor: Some(0.95),
        };
        assert!(speedup_floors(&nan, "t")
            .unwrap_err()
            .contains("finite and greater than 0"));
    }
}
