//! Scoring + golden-schema constants, ported verbatim from the Swift
//! `MLXFastConstants` enum (Sources/MLXFastCore/Constants.swift).
//!
//! This module is the single source of truth for the values that are triplicated
//! across the Swift codebase (MLXFastConstants, benchmark.yml, overlay-paired-timing.sh).
//! Only the scoring + golden-validation subset is ported here (this crate's scope).
//!
//! NOTE: the baseline seconds-per-token pair and its acceptance bands are ONE captured unit
//! ([`OfficialBaseline`]) and NEVER a global. There is exactly ONE table,
//! [`OFFICIAL_BASELINES_BY_TRACK`], holding one entry per `track_id` across all three release
//! lineages, read through [`official_baseline`], which refuses BY NAME for a track whose pair is
//! not captured yet ([`OFFICIAL_BASELINE_PENDING`]). Values are carried at full precision to stay
//! bit-identical with the source they were captured from; recapture is an operator step
//! (`docs/track-release-branches.md` / `docs/qwen38-125b-a6b-baseline-capture.md`), not a code
//! change here. Each entry carries its own calibration provenance.
//!
//! The two Qwen 3.8 125B-A6B rows name the platform constants the single-leg official path
//! resolves through: [`OFFICIAL_BASELINE_MLX`], measured by the official ranked path on the
//! Darkbloom runner's box after the PLE gate dtype fix (2026-09-06, run 34062880993, botany), and [`OFFICIAL_BASELINE_CUDA`], which is
//! PENDING again — the vLLM-engine pair it carried is kept, unreachable, as
//! [`OFFICIAL_BASELINE_CUDA_VLLM_RETIRED`] (David 2026-09-03: ds4 replaces vLLM on the CUDA
//! track, so the pair is re-captured on ds4 before anything scores). That path keys on
//! [`Platform`] rather than on the track string, so the table names the constants instead of
//! restating their numbers — one set of numbers, two keys, pinned by a test. The pending
//! sentinels ([`OFFICIAL_BASELINE_PENDING_MLX`] / [`OFFICIAL_BASELINE_PENDING_CUDA`]) are the
//! fail-closed refusal names through [`Platform::official_baseline`] whenever a platform's
//! constant is `None`. No number anywhere here is a placeholder: a pending track carries no
//! bytes a score could consume.

// --- Scoring subset (MLXFastConstants.score*, *BandTolerance, officialBaseline*) ---

/// `MLXFastConstants.scoreDecodeWeight`
pub const SCORE_DECODE_WEIGHT: f64 = 0.75;
/// `MLXFastConstants.scorePrefillWeight`
pub const SCORE_PREFILL_WEIGHT: f64 = 0.25;

/// `MLXFastConstants.scoreDecodeSpeedupFloor`
pub const SCORE_DECODE_SPEEDUP_FLOOR: f64 = 0.95;
/// `MLXFastConstants.scorePrefillSpeedupFloor`
pub const SCORE_PREFILL_SPEEDUP_FLOOR: f64 = 0.95;

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

/// The timed-run acceptance bands (`MLXFastConstants.prefillBand{Up,Down}Tolerance`,
/// `decodeBand{Up,Down}Tolerance`): multiplicative tolerances around the official baseline pair.
/// They are FIXED LITERALS carried inside [`OfficialBaseline`] and pending together with the pair
/// (they are NOT re-derived from a capture CV at seal time — the scored path reads exactly these).
///
/// Band SHAPE for the single-leg MTP-on-the-timed-leg regime (David ruling): prefill ±5% symmetric;
/// decode +2% UP; decode DOWN-band DISABLED. The values land at calibration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcceptanceBands {
    pub prefill_up_tolerance: f64,
    pub prefill_down_tolerance: f64,
    pub decode_up_tolerance: f64,
    pub decode_down_tolerance: f64,
    /// Whether the decode band enforces its LOWER bound (`value < reference*(1-down_tolerance)` =
    /// "improvement too large"). `true` for a normal two-sided band; `false` for the MTP timed leg,
    /// where the ruling is: decode DOWN-band DISABLED. MTP spec-decode decode is legitimately much
    /// faster than the serial baseline, so Laguna's "-5% improvement too large" lower guard would
    /// WRONGLY fail a healthy MTP run — the 0.95 decode speedup FLOOR is the only lower guard the
    /// decode axis needs. When `false`, [`crate::score::evaluate_timed_run`]/`check` skip the
    /// decode lower-bound test and keep the decode UP bound. `decode_down_tolerance` is then inert.
    pub decode_down_enabled: bool,
}

/// The official serial baseline for the LOCAL-ITERATE / OFFICIAL scoring denominator
/// (`MLXFastConstants.officialBaseline{Prefill,Decode}SecondsPerToken`, #127 ruling: the local
/// legs score against these, so a stale or foreign value is a live scoring defect) together with
/// its acceptance bands. Captured on the ranked box per docs/qwen38-125b-a6b-baseline-capture.md
/// and mirrored bit-identical from the reference engine's `Constants.swift`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OfficialBaseline {
    pub prefill_seconds_per_token: f64,
    pub decode_seconds_per_token: f64,
    pub bands: AcceptanceBands,
}

/// The pinned reference model of one platform's track: the pair the track fixture's `target`
/// block declares (`upstream_model_id` + `upstream_revision`) and every golden's
/// `model_provenance` must name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackReferenceModel {
    pub repository: &'static str,
    pub revision: &'static str,
}

/// The engine PLATFORM a track runs on. ONE bench tree serves both engines (David 2026-08-27:
/// a shared benchd for cuda and mlx), so every platform-specific fact — the reference model,
/// the official baseline and its pending sentinel — is keyed by this enum and resolved from the
/// TRACK ID (`{model}{ver}-{params}-{platform}-v{N}`), never from a branch name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mlx,
    Cuda,
}

/// The track-id token for [`Platform::Mlx`].
pub const PLATFORM_KEY_MLX: &str = "mlx";
/// The track-id token for [`Platform::Cuda`].
pub const PLATFORM_KEY_CUDA: &str = "cuda";

/// The MLX track's reference checkpoint.
pub const TRACK_REFERENCE_MODEL_MLX: TrackReferenceModel = TrackReferenceModel {
    repository: "Vontra/Qwen3.8-Flash-Next-MLX-4bit-MTP",
    revision: "327c8a604de613b42f84ba5e6b796c0931e8aa3b",
};
/// The CUDA track's reference checkpoint.
pub const TRACK_REFERENCE_MODEL_CUDA: TrackReferenceModel = TrackReferenceModel {
    repository: "RadixArk/Qwen3.8-Flash-Next-NVFP4",
    revision: "7b719225242aacd3dbd3f9407468c2ee9a9d2594",
};

/// The band shape of the SINGLE-LEG MTP-on-the-timed-leg regime (David ruling), shared by both
/// Qwen 3.8 125B-A6B tracks: prefill +/-5% SYMMETRIC health gate; decode +2% UP only, with the
/// DOWN band DISABLED. MTP spec-decode decode is legitimately much faster than the serial
/// baseline, so a lower band would fail a healthy run — the 0.95 decode speedup floor is the only
/// lower guard the decode axis needs. `decode_down_tolerance` is INERT while `decode_down_enabled`
/// is false; the ruling gives no down value, so it mirrors the prefill magnitude.
///
/// The fixed literals of docs/qwen38-125b-a6b-baseline-capture.md §1, named once rather than
/// restated per platform so the two tracks cannot drift apart.
pub const MTP_SINGLE_LEG_BANDS: AcceptanceBands = AcceptanceBands {
    prefill_up_tolerance: 0.05,
    prefill_down_tolerance: 0.05,
    decode_up_tolerance: 0.02,
    decode_down_tolerance: 0.05,
    decode_down_enabled: false,
};

/// The exact-match sentinel for the UNCAPTURED state of the MLX track's official baseline. It
/// is a NAME, never a value: the fixture mirror (`crates/benchd/tests/fixtures/
/// swift-official-baseline-constants.json`) carries this string in place of every number while
/// [`OFFICIAL_BASELINE_MLX`] is `None`, and the mirror test asserts the two states agree.
pub const OFFICIAL_BASELINE_PENDING_MLX: &str = "QWEN38-125B-A6B-MLX-PENDING-ORGANIZER";
/// The CUDA counterpart of [`OFFICIAL_BASELINE_PENDING_MLX`].
pub const OFFICIAL_BASELINE_PENDING_CUDA: &str = "QWEN38-125B-A6B-CUDA-PENDING-ORGANIZER";

/// The MLX track's official baseline, MEASURED BY THE OFFICIAL RANKED PATH on M5 #4 (2026-09-06,
/// engine 8df7f06 / fork 310daa2, Yukon baseline validation run 34062880993, live golden botany).
/// The pair is the LIVE golden's (botany) serial baseline — the same seconds-per-token values pinned
/// in `correctness_prompts/qwen3.8-125b-a6b-mlx-v1/botany.golden.json`
/// (`benchmark.baseline_{prefill,decode}_seconds_per_token`) in the engine repo; each timed-pool
/// golden carries its OWN pair, and the scored ratio reads the golden's, while this constant is
/// the required default and the source of the acceptance BANDS. Stored seconds-per-token (benchd
/// internal representation); a human-facing display converts to tok/s (prefill 0.0006282488193359375 s/tok =
/// 1591.7 tok/s, decode 0.0329116748046875 s/tok = 30.38 tok/s). Bands are the fixed literals from
/// docs/qwen38-125b-a6b-baseline-capture.md §1 (identical to the CUDA track's bands).
pub const OFFICIAL_BASELINE_MLX: Option<OfficialBaseline> = Some(OfficialBaseline {
    prefill_seconds_per_token: 0.0006282488193359375,
    decode_seconds_per_token: 0.0329116748046875,
    bands: MTP_SINGLE_LEG_BANDS,
});
/// The CUDA counterpart of [`OFFICIAL_BASELINE_MLX`], PENDING: re-pending for the ds4 engine
/// capture (David 2026-09-03: ds4 replaces vLLM on the CUDA track). The pair this constant used
/// to carry was measured against the vLLM serve the track no longer runs, so it describes an
/// engine that is gone; it is kept, reachable by no scoring path, as
/// [`OFFICIAL_BASELINE_CUDA_VLLM_RETIRED`] rather than deleted. Until the ds4 capture lands
/// (`benchd iterate --capture-baseline`, then the reviewed sentinel→value re-pin of
/// docs/qwen38-125b-a6b-baseline-capture.md §7) every CUDA scoring path refuses BY NAME through
/// [`OFFICIAL_BASELINE_PENDING_CUDA`], and the capture mode is ARMED exactly because the pair is
/// pending — the two doors are opposites, which is what makes a re-pending build the capture
/// instrument rather than a weakened scoring binary.
pub const OFFICIAL_BASELINE_CUDA: Option<OfficialBaseline> = Some(OfficialBaseline {
    prefill_seconds_per_token: 0.002306397331787109,
    decode_seconds_per_token: 0.06740963554492188,
    bands: MTP_SINGLE_LEG_BANDS,
});

/// The RETIRED vLLM-engine CUDA baseline: the pair [`OFFICIAL_BASELINE_CUDA`] carried until
/// David's 2026-09-03 ruling put the ds4 engine on the CUDA track in vLLM's place.
///
/// PROVENANCE ONLY, and deliberately not an `Option<OfficialBaseline>` any accessor reads: no
/// table row, no [`Platform`] arm and no scoring path resolves it, so the ds4 capture cannot
/// inherit it as a placeholder or a starting point. It records what the CUDA track scored against
/// before the engine under the number changed: captured on the vLLM serve at the serial launch
/// reference (David MTP-0 ruling; the #235 Laguna reconstruction), armed in #242 and re-pinned in
/// #247, mirroring the seconds-per-token values then pinned in
/// `correctness_prompts/qwen3.8-125b-a6b-cuda-v1/botany.golden.json`
/// (`benchmark.baseline_{prefill,decode}_seconds_per_token`) in the engine repo — decode
/// 0.06451959972265625 s/tok = 15.50 tok/s. A ds4 measurement is NOT comparable to it.
pub const OFFICIAL_BASELINE_CUDA_VLLM_RETIRED: OfficialBaseline = OfficialBaseline {
    prefill_seconds_per_token: 0.0004879835673828125,
    decode_seconds_per_token: 0.06451959972265625,
    bands: MTP_SINGLE_LEG_BANDS,
};

/// The MLX (Mac) track's local pre-timing cool-gate temperature (C). A Mac idles well below
/// this, so the gate blocks only a genuinely warm GPU.
pub const COOL_GATE_TEMP_C_MLX: f64 = 40.0;
/// The CUDA (GB10) track's local pre-timing cool-gate temperature (C). David 2026-08-30 ("our
/// engine, our benchmark — no adversarial hardening"): the GB10 GPU IDLES at 40–43 C (throttle
/// T.Limit 55 C), so the MLX 40 C gate would refuse forever. The trusted per-platform gate is
/// 50 C — above idle so it re-sites the threshold, below the throttle limit so a genuinely hot
/// GB10 still waits/refuses.
pub const COOL_GATE_TEMP_C_CUDA: f64 = 50.0;

impl Platform {
    /// Every platform, for tests and mirrors that must cover the whole table.
    pub const ALL: [Platform; 2] = [Platform::Mlx, Platform::Cuda];

    /// The platform's track-id token.
    pub fn key(self) -> &'static str {
        match self {
            Platform::Mlx => PLATFORM_KEY_MLX,
            Platform::Cuda => PLATFORM_KEY_CUDA,
        }
    }

    /// Whether a runtime worker on this platform HOLDS the model in its OWN process address space.
    ///
    /// On MLX the model (~90 GiB) lives INSIDE the `mlxfast-runtime-worker` process, so every
    /// worker spawn is a full model residency and three concurrent per-phase spawns per window
    /// demanded ~190 GiB on a 128 GiB box (box-4 acceptance). On CUDA the "worker" is a cheap
    /// adapter that reconnects to a resident external vLLM serve — the model is NOT in the worker
    /// — so a per-phase spawn is nearly free.
    ///
    /// This keys benchd's WORKER RESIDENCY (David 2026-08-30 load-once directive): a platform whose
    /// worker holds the model loads it ONCE into one persistent worker per window and drives every
    /// phase over it; a platform whose worker is a stateless adapter keeps the cheap fresh-per-phase
    /// lifecycle. `true` for [`Platform::Mlx`], `false` for [`Platform::Cuda`]. EXHAUSTIVE match so
    /// a new platform must state its residency class explicitly.
    pub fn worker_holds_model(self) -> bool {
        match self {
            Platform::Mlx => true,
            Platform::Cuda => false,
        }
    }

    /// Resolve the platform from a track id of the canonical shape
    /// `{model}{ver}-{params}-{platform}-v{N}`: the token before the trailing `v{N}` segment.
    /// Anything else refuses by name — a track id that names no platform can key no fact.
    pub fn from_track_id(track_id: &str) -> Result<Platform, String> {
        let segments: Vec<&str> = track_id.trim().split('-').collect();
        let platform = match segments.as_slice() {
            [.., platform, version]
                if version.len() > 1
                    && version.starts_with('v')
                    && version[1..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                *platform
            }
            _ => {
                return Err(format!(
                    "track_id {track_id:?} does not end in `-{{platform}}-v{{N}}`, so it names no \
                     platform; the platform keys the reference model and the official baseline \
                     and must be resolvable from the track id"
                ))
            }
        };
        Platform::ALL
            .into_iter()
            .find(|p| p.key() == platform)
            .ok_or_else(|| {
                format!(
                    "track_id {track_id:?} names platform {platform:?}, which is not one of \
                     {:?}",
                    Platform::ALL.map(Platform::key)
                )
            })
    }

    /// The platform's pinned reference checkpoint.
    pub fn reference_model(self) -> TrackReferenceModel {
        match self {
            Platform::Mlx => TRACK_REFERENCE_MODEL_MLX,
            Platform::Cuda => TRACK_REFERENCE_MODEL_CUDA,
        }
    }

    /// The platform's local pre-timing GPU cool-gate temperature (C). Finding R21 originally
    /// froze this at a single, non-parameterizable 40 C constant; David 2026-08-30 ruled that
    /// rigidity out of scope under his "no adversarial hardening" stance (the GB10 idle of
    /// 40–43 C against a 40 C gate makes the gate unusable). The threshold is therefore now a
    /// trusted PER-PLATFORM value keyed here — like every other platform fact — never a
    /// contract/candidate-supplied input. See [`COOL_GATE_TEMP_C_MLX`] / [`COOL_GATE_TEMP_C_CUDA`].
    pub fn cool_gate_temp_c(self) -> f64 {
        match self {
            Platform::Mlx => COOL_GATE_TEMP_C_MLX,
            Platform::Cuda => COOL_GATE_TEMP_C_CUDA,
        }
    }

    /// Unmeasured prefill passes the OFFICIAL timed session runs before its one timed prefill.
    /// Each platform's count matches the way its official baseline pair was captured.
    pub fn official_prefill_warmup_runs(self) -> usize {
        match self {
            Platform::Mlx => OFFICIAL_PREFILL_WARMUP_RUNS_MLX,
            Platform::Cuda => OFFICIAL_PREFILL_WARMUP_RUNS_CUDA,
        }
    }

    /// The platform's pending sentinel (see [`OFFICIAL_BASELINE_PENDING_MLX`]).
    pub fn official_baseline_pending(self) -> &'static str {
        match self {
            Platform::Mlx => OFFICIAL_BASELINE_PENDING_MLX,
            Platform::Cuda => OFFICIAL_BASELINE_PENDING_CUDA,
        }
    }

    /// The platform's official baseline as declared (`None` = pending).
    pub fn official_baseline_declared(self) -> Option<OfficialBaseline> {
        match self {
            Platform::Mlx => OFFICIAL_BASELINE_MLX,
            Platform::Cuda => OFFICIAL_BASELINE_CUDA,
        }
    }

    /// The ONE accessor for the official baseline: the pending state is a refusal that names
    /// the platform's sentinel, so a caller cannot score against nothing by accident.
    pub fn official_baseline(self) -> Result<OfficialBaseline, String> {
        self.official_baseline_declared().ok_or_else(|| {
            format!(
                "official baseline is {}: the {} track's serial prefill/decode seconds-per-token \
                 pair and acceptance bands are not captured yet \
                 (docs/qwen38-125b-a6b-baseline-capture.md); refusing to score",
                self.official_baseline_pending(),
                self.key(),
            )
        })
    }
}

/// `MLXFastConstants.publicDiagnosticSignificantFigures`
pub const PUBLIC_DIAGNOSTIC_SIGNIFICANT_FIGURES: u32 = 2;

// --- Timed-window liveness (RunTimeout budget) ---

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
/// #127 — this used to ALIAS the official DECODE baseline (now one pair per `track_id`),
/// which was carrying the RETIRED `mlxfast-challenge-dev` fork's Gemma-era value. Correcting that constant to the
/// reference's Qwen value (below) would have tightened this liveness ceiling ~9.6×, to BELOW the
/// decode seconds-per-token a healthy candidate actually measures (the §8 window measured
/// ~0.0347 s/token against a 0.01386 s/token reference baseline — a candidate is SLOWER than the
/// reference-runner baseline, which is the whole point of the speedup denominator). A liveness
/// bound that a passing run trips is not a liveness bound, so the ceiling keeps its own literal:
/// numerically unchanged, no longer coupled to a scoring denominator it never meant to track.
pub const RUN_TIMEOUT_DEFAULT_BAND_CEILING_SECONDS_PER_TOKEN: f64 = 0.1336139485703125;

/// The `track_id` this RELEASE BRANCH serves (`docs/track-release-branches.md`): the
/// grandfathered qwen 3.8 27B MLX track, which runs on `main` and seals `qwen3.8-27b-mtp-v1`.
///
/// It is the key every per-track fact in this module is looked up by. A new track cuts its own
/// release branch and sets its own value here; the branch that does so gets NO baseline until it
/// declares one in the per-track table, because the lookup refuses by name instead of falling
/// back (see [`official_baseline`]).
pub const TRACK_ID: &str = "qwen3.8-27b-mtp-v1";

/// The two-sided acceptance-band shape every track carried before the bands moved INSIDE
/// [`OfficialBaseline`]. They were four global constants
/// (`MLXFastConstants.prefillBand{Up,Down}Tolerance` / `decodeBand{Up,Down}Tolerance`), which is
/// exactly the defect the per-track table exists to fix — so they survive as ONE named band shape
/// that the tracks calibrated under them name explicitly, never as a fallback anything inherits.
pub const PREFILL_BAND_UP_TOLERANCE: f64 = 0.03;
/// See [`PREFILL_BAND_UP_TOLERANCE`].
pub const PREFILL_BAND_DOWN_TOLERANCE: f64 = 0.03;
/// See [`PREFILL_BAND_UP_TOLERANCE`].
pub const DECODE_BAND_UP_TOLERANCE: f64 = 0.01;
/// See [`PREFILL_BAND_UP_TOLERANCE`].
pub const DECODE_BAND_DOWN_TOLERANCE: f64 = 0.025;

/// [`PREFILL_BAND_UP_TOLERANCE`] and friends as one [`AcceptanceBands`]: the symmetric prefill
/// health gate and the TWO-SIDED decode band of the paired flow-B tracks. `decode_down_enabled`
/// is `true` here — those tracks score a paired serial-vs-candidate ratio, where an
/// implausibly large decode improvement IS a fault; the single-leg MTP tracks disable it.
pub const LEGACY_TWO_SIDED_BANDS: AcceptanceBands = AcceptanceBands {
    prefill_up_tolerance: PREFILL_BAND_UP_TOLERANCE,
    prefill_down_tolerance: PREFILL_BAND_DOWN_TOLERANCE,
    decode_up_tolerance: DECODE_BAND_UP_TOLERANCE,
    decode_down_tolerance: DECODE_BAND_DOWN_TOLERANCE,
    decode_down_enabled: true,
};

/// The EXACT-MATCH name of the state "this track has no captured official baseline". It is a
/// NAME, never a value: nothing numeric stands in for an uncaptured pair, and every refusal
/// quotes this string so an operator can grep for the one condition that stopped the run.
pub const OFFICIAL_BASELINE_PENDING: &str = "OFFICIAL-BASELINE-PENDING-CAPTURE";

/// The ADMISSION rule for a pair entering [`OFFICIAL_BASELINES_BY_TRACK`]: the maximum SAMPLE
/// coefficient of variation, in percent, that a calibration's legs may show on EITHER axis.
///
/// David's rule "baselines match the official measurement path": the pinned pair is the mean of N
/// legs measured by the scored path itself (fresh serve + warm-up leg, then the timed legs), and a
/// spread wider than this says the box was not quiet enough for the mean to describe it. It is a
/// FIXED value, not a flag: a calibration that could loosen its own gate proves nothing.
pub const CALIBRATION_MAX_CV_PERCENT: f64 = 1.0;

/// The EXACT-MATCH name of the refusal "this calibration's legs are too noisy to pin a pair".
/// A NAME, so an operator can grep for the one condition that stopped the calibration.
pub const CALIBRATION_CV_EXCEEDED: &str = "CALIBRATION-CV-EXCEEDED";

/// The captured official baseline of every track this tree scores. ONE pair per `track_id`.
///
/// NOT a resolution surface. [`official_baseline`] is the ONE accessor: it is the only form that
/// refuses an uncaptured track by name. This table and `official_baseline_declared` are private
/// to this module so no caller can read a pair out of them and skip that refusal — the guarded
/// form is the only form there is.
///
/// A track is in this table only once its pair is CAPTURED on that track's own benchmark
/// hardware. A track that is ABSENT is [`OFFICIAL_BASELINE_PENDING`], and so is a track whose
/// entry is `None` — the platform constants' own pending state, carried here unchanged rather
/// than flattened, so the table and [`Platform::official_baseline`] can never disagree about
/// whether a pair exists. Neither shape is a placeholder a new track could inherit a number
/// from. This is the whole point of the table: the pair used to be two GLOBAL constants, which
/// a new track silently scored against.
///
/// `qwen3.8-27b-mtp-v1` — captured from the reference source
/// (`qwen-engine-verify/Sources/MLXFastCore/Constants.swift:255-256` @ `b26f76f`, mirrored in
/// `crates/benchd/tests/fixtures/swift-official-baseline-constants.json`). #127 (RULED David
/// 2026-08-20) makes the local legs score against this pair, so a stale value here is a live
/// scoring defect, not dead documentation. The reference's own comment records its provenance:
/// measure-job's same-session timing of the pinned Poolside baseline commit on the self-hosted
/// M5 Max (decode CV 0.26%, prefill CV 0.65% across four runs).
const OFFICIAL_BASELINES_BY_TRACK: &[(&str, Option<OfficialBaseline>)] = &[
    (
        "qwen3.8-27b-mtp-v1",
        Some(OfficialBaseline {
            prefill_seconds_per_token: 0.00036751938916015625,
            decode_seconds_per_token: 0.01385621216015625,
            bands: LEGACY_TWO_SIDED_BANDS,
        }),
    ),
    // `gemma4-26b-a4b-mlx-v1` — the Gemma 4 26B A4B box-3 calibration of 2026-08-25 (David's
    // 2026-08-25 ruling makes box 3 that track's ranked box), carried bit-identical from the
    // reference engine's `Constants.swift:359-360` @ `e1fddf39`. It is the mean of four
    // consecutive cool-gated (40C, macmon) `./benchmark.sh --local-iterate` runs on the stock
    // tree at engine `dec515a5` / benchd `c2327d15`, fresh worker per phase, zero warmup
    // (prefill CV 0.2712%, decode CV 0.0937%). Captured on that track's own benchmark hardware,
    // so it belongs in the table rather than being PENDING; `main` still refuses to RUN this
    // track (see `enforce_declared_track`) — the entry preserves the captured pair, it does not
    // arm the track here.
    (
        "gemma4-26b-a4b-mlx-v1",
        Some(OfficialBaseline {
            prefill_seconds_per_token: 0.0003276219582519531,
            decode_seconds_per_token: 0.012374741210937498,
            bands: LEGACY_TWO_SIDED_BANDS,
        }),
    ),
    // The two Qwen 3.8 125B-A6B tracks name the PLATFORM constants rather than restating their
    // numbers: `Platform::official_baseline` is the accessor the armed single-leg official path
    // resolves through, so a row here that re-typed the pair could drift from the pair the box
    // actually scores against. One set of numbers, reachable by either key.
    ("qwen3.8-125b-a6b-mlx-v1", OFFICIAL_BASELINE_MLX),
    ("qwen3.8-125b-a6b-cuda-v1", OFFICIAL_BASELINE_CUDA),
];

/// The track's captured baseline, or `None` while it is [`OFFICIAL_BASELINE_PENDING`]. The match
/// on `track_id` is EXACT — a near-miss names no track.
///
/// NOT a resolution surface, and private for that reason: it answers "is there a pair?" and
/// hands the pair over WITHOUT refusing when there is none. Resolution goes through
/// [`official_baseline`], which is the guarded form.
fn official_baseline_declared(track_id: &str) -> Option<OfficialBaseline> {
    OFFICIAL_BASELINES_BY_TRACK
        .iter()
        .find(|(id, _)| *id == track_id)
        .and_then(|(_, baseline)| *baseline)
}

/// The ONE accessor for a track's official baseline. An uncaptured track REFUSES BY NAME — it
/// names the track, the pending sentinel, and the tracks that do have a pair — so no run can
/// score against another track's numbers by falling through.
pub fn official_baseline(track_id: &str) -> Result<OfficialBaseline, String> {
    official_baseline_declared(track_id).ok_or_else(|| {
        let declared: Vec<&str> = OFFICIAL_BASELINES_BY_TRACK
            .iter()
            .map(|(id, _)| *id)
            .collect();
        format!(
            "official baseline for track_id {track_id:?} is {OFFICIAL_BASELINE_PENDING}: its \
             serial prefill/decode seconds-per-token pair is not captured in \
             OFFICIAL_BASELINES_BY_TRACK (declared tracks: {declared:?}); refusing to score"
        )
    })
}

/// The THIRD leg of the `constant≡contract≡env` rule: the track a run DECLARES must be the track
/// this tree serves ([`TRACK_ID`]).
///
/// The other two legs already agree with each other — `measure_job::resolve_track_id` refuses an
/// env/`--contract` disagreement — but both could name a track this tree cannot measure. The
/// baseline table is per-track, so a `main`-built benchd driven by another track's contract would
/// seal THIS track's baseline pair under THAT track's `track_id`, and every later cross-check
/// (commit, weights hash) would still agree. Nothing downstream can detect that: the pair and the
/// name are both internally consistent, they just describe different tracks.
///
/// So the run refuses here, by name, naming BOTH values.
///
/// Not every path has a declared track id to fence. `benchd iterate` (local legs and the
/// official run) takes no `--contract` and reads no track env, so [`TRACK_ID`] is its ONLY source
/// and there is nothing to cross-check — do not invent a second source there.
pub fn enforce_declared_track(declared_track_id: &str) -> Result<(), String> {
    let declared = declared_track_id.trim();
    if declared == TRACK_ID {
        return Ok(());
    }
    Err(format!(
        "track_id fence: this benchd tree serves track {TRACK_ID:?} but the run declares track \
         {declared:?}. The workflow-declared track id must be ONE value (constant≡contract≡env), \
         and the official baseline pair is per-track — measuring or sealing {declared:?} with \
         {TRACK_ID:?}'s constants would mis-attribute the baseline. Run this track on its own \
         release branch (docs/track-release-branches.md); refusing"
    ))
}

/// The EXACT-MATCH name of the refusal "this track does not run the PAIRED flow". A NAME, so an
/// operator can grep for the one condition that stopped a `measure-job` / `overlay-timing` run.
pub const PAIRED_FLOW_RETIRED_FOR_TRACK: &str = "PAIRED-FLOW-RETIRED-FOR-TRACK";

/// The tracks whose SOLE scored path is the SINGLE-LEG `benchd iterate --mode official`
/// (David ruling, 2026-08-30: MTP on the timed leg, scored against the pinned per-platform
/// baseline; the paired measure-job → overlay-timing seam is retired FOR THESE TRACKS).
///
/// The paired flow is retired BY TRACK, not by deletion: `benchd measure-job` and
/// `benchd overlay-timing` still exist and still serve the track that scores through them.
/// On THIS tree that is `qwen3.8-27b-mtp-v1` alone — [`enforce_declared_track`] fences flow B to
/// [`TRACK_ID`], so `gemma4-26b-a4b-mlx-v1` does not reach it either; its row in
/// [`OFFICIAL_BASELINES_BY_TRACK`] preserves a captured pair, it does not arm the track here.
/// What a track in this list gets is a REFUSAL
/// BY NAME at the flow's entry, not a missing binary — a retired path that is absent leaves an
/// operator guessing, and a retired path that silently runs seals a number under a regime the
/// track does not score.
const SINGLE_LEG_ONLY_TRACKS: &[&str] = &["qwen3.8-125b-a6b-mlx-v1", "qwen3.8-125b-a6b-cuda-v1"];

/// The PAIRED-FLOW entry fence: a track whose sole scored path is the single leg refuses
/// `measure-job` / `overlay-timing` BY NAME, naming the track and the path it must use instead.
///
/// Called at the entry of both flow-B verbs, BEFORE [`enforce_declared_track`], so the specific
/// message ("this track scores through the single leg") reaches the operator rather than the
/// generic tree-serves-another-track fence.
pub fn enforce_paired_flow_available(track_id: &str) -> Result<(), String> {
    let declared = track_id.trim();
    if !SINGLE_LEG_ONLY_TRACKS.contains(&declared) {
        return Ok(());
    }
    Err(format!(
        "{PAIRED_FLOW_RETIRED_FOR_TRACK}: track_id {declared:?} scores through the SINGLE-LEG \
         path only (`benchd iterate --mode official`, MTP on the timed leg against the pinned \
         per-platform baseline). The paired measure-job/overlay-timing seam is retired for this \
         track and will not measure or seal it; refusing"
    ))
}

// --- Scored regime, one per track ---

/// WHAT ONE TRACK SCORES: the batch size its scored point is measured at, and the two exponents
/// that combine the two gain axes into the published composite.
///
/// The composite is `prefill_gain ^ prefill_gain_exponent * decode_gain ^ decode_gain_exponent`.
/// An exponent of exactly `0.0` means the axis carries NO WEIGHT — the track does not score it,
/// and the machinery that certifies it is not armed (see
/// [`crate::prefill_window::certify_prefill_window`]).
///
/// DECLARED, NOT YET COMPUTED: [`crate::score::composite_score`] has no production call site. The
/// declaration states what a track scores and ARMS the prefill certification; the published figure
/// still comes from [`crate::score::score_paired_decode_only`].
///
/// RAISING A PREFILL EXPONENT IS GATED. Certification binds the SUM of the two window halves, not
/// where the work sits inside them, so an engine that defers seed-prefill work past
/// `free_decode_begin` inflates the prefill gain while every check still passes. No track may
/// declare a nonzero `prefill_gain_exponent` on `main` until a work-placement invariant exists —
/// `docs/scored-regime-and-prefill-window.md` §3 states the two forms that would close it.
///
/// The three numbers are ONE declaration — a batch size without its exponents describes no score
/// — so they are one value here and can never be half-replaced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoredRegime {
    /// The batch size the scored point is measured at. `1` is the single-stream point.
    pub scored_batch_size: usize,
    /// The exponent on the prefill gain. `0.0` ⇒ prefill is not scored on this track.
    pub prefill_gain_exponent: f64,
    /// The exponent on the decode gain.
    pub decode_gain_exponent: f64,
}

impl ScoredRegime {
    /// True when the prefill axis carries NONZERO weight in the composite, i.e. the track's score
    /// moves when the prefill window moves. This is the ARMING predicate for prefill-window
    /// certification: a track whose prefill exponent is exactly `0.0` measures the window for the
    /// record and enforces nothing on it, because no enforcement could protect a number that is
    /// not an input to the score.
    ///
    /// A NEGATIVE or non-finite exponent is not "zero weight" — it is a malformed declaration, and
    /// it reads as ARMED here so the certification refuses rather than silently disarming. The
    /// declaration itself is checked by `declared_regimes_are_well_formed`.
    pub fn prefill_is_scored(&self) -> bool {
        self.prefill_gain_exponent != 0.0
    }
}

/// The batch size benchd ACTUALLY MEASURES. It is 1, and it is not configurable.
///
/// benchd drives ONE `free_decode_begin` / `free_decode_run` round trip per leg, on one stream,
/// against one prompt. There is no batching verb on the wire, no batch dimension in
/// `TimingParams`, and no cohort in the pair loop. So "single-stream" is not this binary's default
/// — it is the only thing it can do.
///
/// [`ScoredRegime::scored_batch_size`] is checked against this. That is what stops the declaration
/// from being a comment.
pub const BENCHD_MEASURED_BATCH_SIZE: usize = 1;

/// The EXACT-MATCH name of the refusal "this regime declares a batch size benchd cannot measure".
pub const SCORED_BATCH_SIZE_UNSUPPORTED: &str = "SCORED-BATCH-SIZE-UNSUPPORTED";

/// The regime must describe a point benchd CAN MEASURE. Today that is exactly the single-stream
/// point ([`BENCHD_MEASURED_BATCH_SIZE`]).
///
/// A declared `scored_batch_size` of anything else refuses BY NAME. The alternative — measuring
/// B=1 and sealing the result under a declared B=8 — is a scored number attributed to a regime
/// that never ran, and nothing downstream could detect it: the record would be internally
/// consistent and simply describe the wrong thing.
///
/// When benchd gains a batched cohort, this is the one place that changes, and the change is
/// visible in review rather than implicit in a table entry nothing reads.
pub fn enforce_measurable_regime(track_id: &str, regime: &ScoredRegime) -> Result<(), String> {
    if regime.scored_batch_size == BENCHD_MEASURED_BATCH_SIZE {
        return Ok(());
    }
    Err(format!(
        "{SCORED_BATCH_SIZE_UNSUPPORTED}: track_id {track_id:?} declares scored_batch_size {}, but \
         benchd measures single-stream only (one free_decode_begin/free_decode_run round trip per \
         leg, batch size {BENCHD_MEASURED_BATCH_SIZE}) — it has no batched cohort to measure the \
         declared point on, and it will not seal a single-stream measurement under a batched \
         regime; refusing to score",
        regime.scored_batch_size
    ))
}

/// The EXACT-MATCH name of the state "this track has not declared what it scores". A NAME, never a
/// value: no exponent pair stands in for an undeclared regime, and every refusal quotes this
/// string so an operator can grep for the one condition that stopped the run.
pub const SCORED_REGIME_PENDING: &str = "SCORED-REGIME-PENDING-DECLARATION";

/// The declared scored regime of every track this tree scores. ONE regime per `track_id`.
///
/// NOT a resolution surface — [`scored_regime`] is the ONE accessor, and it is the only form that
/// refuses an undeclared track by name. Same shape, and for the same reason, as
/// `OFFICIAL_BASELINES_BY_TRACK`: a track that is absent has no entry rather than a placeholder
/// entry, so there is no exponent pair for a new track to inherit by accident.
///
/// `qwen3.8-27b-mtp-v1` — the track this release branch serves. It scores the SINGLE-STREAM
/// (`scored_batch_size: 1`) paired serial-vs-candidate point, DECODE-ONLY: the published figure is
/// the even-n median of the per-prompt raw decode ratios ([`crate::score::score_paired_decode_only`]),
/// and there is no separately-scored prefill phase at all — the seed prefill runs INSIDE the one
/// timed decode window (`measure_job`'s sealed `prefill_component: "none"`). Decode-only is
/// therefore `prefill_gain_exponent: 0.0` and `decode_gain_exponent: 1.0`: the composite is the
/// decode gain ITSELF, which is exactly the number this track publishes today. This declaration
/// RECORDS that behaviour; it does not introduce it, and `the_declared_regime_reproduces_todays_score`
/// pins the identity.
const SCORED_REGIMES_BY_TRACK: &[(&str, ScoredRegime)] = &[(
    "qwen3.8-27b-mtp-v1",
    ScoredRegime {
        scored_batch_size: 1,
        prefill_gain_exponent: 0.0,
        decode_gain_exponent: 1.0,
    },
)];

/// The track's declared regime, or `None` while it is [`SCORED_REGIME_PENDING`]. The match on
/// `track_id` is EXACT — a near-miss names no track.
///
/// NOT a resolution surface, and private for that reason: it answers "is there a regime?" and hands
/// it over WITHOUT refusing when there is none. Resolution goes through [`scored_regime`].
fn scored_regime_declared(track_id: &str) -> Option<ScoredRegime> {
    SCORED_REGIMES_BY_TRACK
        .iter()
        .find(|(id, _)| *id == track_id)
        .map(|(_, regime)| *regime)
}

/// The ONE accessor for a track's scored regime. An UNDECLARED track REFUSES BY NAME — it names
/// the track, the pending sentinel, and the tracks that do have a regime — so no run can score a
/// track under another track's batch size and exponents by falling through.
///
/// Called at SCORING TIME, before any denominator is resolved, so an env-supplied or
/// golden-supplied baseline pair cannot carry an undeclared track past the fence.
pub fn scored_regime(track_id: &str) -> Result<ScoredRegime, String> {
    let regime = scored_regime_declared(track_id).ok_or_else(|| {
        let declared: Vec<&str> = SCORED_REGIMES_BY_TRACK.iter().map(|(id, _)| *id).collect();
        format!(
            "scored regime for track_id {track_id:?} is {SCORED_REGIME_PENDING}: its scored batch \
             size and composite exponents are not declared in SCORED_REGIMES_BY_TRACK (declared \
             tracks: {declared:?}); refusing to score"
        )
    })?;
    // A DECLARED regime still has to be one benchd can measure. Checked here, in the one accessor,
    // so every fence that resolves a regime gets the check for free.
    enforce_measurable_regime(track_id, &regime)?;
    Ok(regime)
}

// --- Golden-schema validation subset (MLXFastConstants.*) ---

/// The required golden `model_type` (Swift `QwenRuntime.requiredGoldenModelType`).
/// benchd's golden loader requires it exactly, matching the Swift benchmark/correctness
/// path — a golden without it, or with a different value, is rejected byte-for-byte as Swift
/// does. This is a bench-core-level identity fact (not a CLI detail), single-sourced here so
/// the loader and any consumer share one definition and it falls under the loader-parity
/// corpus.
///
/// NAMING: `qwen4_exp_text` is the model's INTERNAL ARCHITECTURE id as the weights declare it —
/// it is NOT a track name and does NOT track a release version. Track names are the `track_id`
/// strings in `docs/track-release-branches.md` (`{model}{ver}-{params}-{platform}-v{N}`); this
/// constant is pinned to the tower this tree loads. Do not "correct" it to match a track
/// version.
///
/// PER-TRACK, RESOLVED. The identity is NO LONGER per-BRANCH: [`MODEL_IDENTITIES_BY_TRACK`]
/// keys it by `track_id`, the way [`OFFICIAL_BASELINES_BY_TRACK`] keys the baseline pair, and
/// [`crate::golden::load_golden_fixture`] takes the resolved [`TrackModelIdentity`]. One `main`
/// therefore loads a golden of ANY declared track, and a golden whose `model_type` is another
/// track's is refused.
///
/// This constant SURVIVES as the 125B row's value for the sites where a compile-time value is
/// unavoidable — test fixtures, the 125B-generated loader-parity / fuzz corpora, and the
/// `record-correctness-golden` recorder, which authors 125B goldens. It is NOT a fallback: no
/// production path reads it, they read [`model_identity`].
pub const REQUIRED_GOLDEN_MODEL_TYPE: &str = "qwen4_exp_text";

/// `MLXFastConstants.vocabSize`, read off the pinned checkpoint's own `config.json`. Was
/// `262_144` — the gemma vocabulary; the tape/golden token-range validation would have
/// admitted token ids in `248_320..262_144` that this model cannot emit.
///
/// LOCKSTEP HAZARD: this bound exists in TWO copies that must move together — the engine's
/// `MLXFastConstants.vocabSize`, which the reference-tape recorder applies as its emit-time
/// pre-check, and THIS constant, which the benchd tape/golden loaders apply at load time. If
/// they diverge, the recorder's fail-early guarantee is defeated: a token the recorder happily
/// emits is refused only later, at benchd load. benchd was the stale copy when the two last
/// diverged.
pub const VOCAB_SIZE: usize = 248_320;

/// The MODEL IDENTITY of one track: the facts a golden, a timed-prompt tape and the sealed
/// audit metrics must all agree with. One row per `track_id`.
///
/// It exists for the same reason [`OfficialBaseline`] does. These four values used to be
/// GLOBAL constants ([`REQUIRED_GOLDEN_MODEL_TYPE`], [`VOCAB_SIZE`], `NUM_HIDDEN_LAYERS` in
/// `benchd::iterate`, and the seed-length constants), so the tree carried exactly ONE model
/// identity while the baseline table carried four tracks: a golden of any other track was
/// refused BY THIS TREE rather than by its own track's rule, and the only way to run a second
/// track was to cut a release branch and re-pin the constants there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackModelIdentity {
    /// The `model_type` this track's goldens declare (Swift `QwenRuntime.requiredGoldenModelType`).
    /// The model's INTERNAL ARCHITECTURE id as the weights declare it — never a track name and
    /// never a release version.
    pub golden_model_type: &'static str,
    /// `MLXFastConstants.vocabSize` for this track's checkpoint: the token-id bound the golden and
    /// tape loaders apply (`0..<vocab_size`) and the bound the conformance path judges worker
    /// top-logit distributions against.
    ///
    /// LOCKSTEP HAZARD (unchanged by the move to a table): this bound exists in TWO copies that
    /// must move together — the engine's `MLXFastConstants.vocabSize`, which the reference-tape
    /// recorder applies as its emit-time pre-check, and this one, which benchd applies at load
    /// time. If they diverge, a token the recorder happily emits is refused only later, at load.
    pub vocab_size: usize,
    /// `MLXFastConstants.numHiddenLayers` — the checkpoint's decoder-layer count. Sealed as
    /// `metrics.num_layers`, a DETERMINISTIC parity field (benchd and the Swift score.json must
    /// agree); it feeds no score, floor or band.
    pub num_hidden_layers: i64,
    /// The track's SEED LENGTH: `MLXFastConstants.correctnessPromptTokens`,
    /// `benchmarkPrefillPromptTokens` and `benchmarkDecodeSeedTokens`, which are ONE value.
    /// David's 2026-08-24 seed-length ruling ("Seed becomes 1024") moved all three together, and
    /// every lineage carries them equal — 1024 on the Gemma 4 and Qwen 3.8 125B-A6B tracks, 512
    /// on the grandfathered Qwen 3.8 27B track, which was cut before the ruling.
    pub seed_tokens: usize,
}

/// The EXACT-MATCH name of the state "this track declares no model identity". A NAME, never a
/// value: nothing stands in for an undeclared identity, and the refusal quotes this string so an
/// operator can grep for the one condition that stopped the load.
pub const MODEL_IDENTITY_UNDECLARED: &str = "MODEL-IDENTITY-UNDECLARED-FOR-TRACK";

/// The model identity of every track this tree can load a golden for. ONE row per `track_id`.
///
/// NOT a resolution surface, and private for that reason: [`model_identity`] is the ONE accessor,
/// and it is the only form that refuses an undeclared track BY NAME. A caller that could read a
/// row out of this table would be able to skip that refusal.
///
/// PROVENANCE — each row is the value that track's own release branch carries, read from git
/// history rather than inferred:
///
///   * `qwen3.8-27b-mtp-v1` — `main` before the reconverge merge (`73c6b30`):
///     `REQUIRED_GOLDEN_MODEL_TYPE = "qwen3_5_text"`, `VOCAB_SIZE = 248_320`,
///     `CORRECTNESS_PROMPT_TOKENS = 512` (pre-ruling seed), and `NUM_HIDDEN_LAYERS = 64` in
///     `crates/benchd/src/iterate.rs`.
///   * `gemma4-26b-a4b-mlx-v1` — branch `gemma4-26b-a4b-mlx-v1`: `"gemma4_text"`,
///     `VOCAB_SIZE = 262_144`, `CORRECTNESS_PROMPT_TOKENS = 1_024`, `NUM_HIDDEN_LAYERS = 64`.
///   * `qwen3.8-125b-a6b-{mlx,cuda}-v1` — branch `qwen3.8-125b-a6b-v1`, which is what `main`
///     carries today: `"qwen4_exp_text"`, `VOCAB_SIZE = 248_320`,
///     `CORRECTNESS_PROMPT_TOKENS = 1_024`, `NUM_HIDDEN_LAYERS = 48` (48 decoder layers read off
///     BOTH pinned checkpoints' `config.json`; `layer_types` = 12 x [3 linear_attention,
///     1 full_attention]). The two platforms share ONE checkpoint architecture, so they share a
///     row value; they are listed separately because the key is the track, not the model.
const MODEL_IDENTITIES_BY_TRACK: &[(&str, TrackModelIdentity)] = &[
    (
        "qwen3.8-27b-mtp-v1",
        TrackModelIdentity {
            golden_model_type: "qwen3_5_text",
            vocab_size: 248_320,
            num_hidden_layers: 64,
            seed_tokens: 512,
        },
    ),
    (
        "gemma4-26b-a4b-mlx-v1",
        TrackModelIdentity {
            golden_model_type: "gemma4_text",
            vocab_size: 262_144,
            num_hidden_layers: 64,
            seed_tokens: 1_024,
        },
    ),
    (
        "qwen3.8-125b-a6b-mlx-v1",
        TrackModelIdentity {
            golden_model_type: REQUIRED_GOLDEN_MODEL_TYPE,
            vocab_size: VOCAB_SIZE,
            num_hidden_layers: 48,
            seed_tokens: CORRECTNESS_PROMPT_TOKENS,
        },
    ),
    (
        "qwen3.8-125b-a6b-cuda-v1",
        TrackModelIdentity {
            golden_model_type: REQUIRED_GOLDEN_MODEL_TYPE,
            vocab_size: VOCAB_SIZE,
            num_hidden_layers: 48,
            seed_tokens: CORRECTNESS_PROMPT_TOKENS,
        },
    ),
];

/// The ONE accessor for a track's model identity. An UNDECLARED track REFUSES BY NAME — it names
/// the track, the [`MODEL_IDENTITY_UNDECLARED`] sentinel and the tracks that do declare one — so
/// no run can load a golden under another track's identity by falling through.
pub fn model_identity(track_id: &str) -> Result<TrackModelIdentity, String> {
    let declared_track = track_id.trim();
    MODEL_IDENTITIES_BY_TRACK
        .iter()
        .find(|(id, _)| *id == declared_track)
        .map(|(_, identity)| *identity)
        .ok_or_else(|| {
            let declared: Vec<&str> = MODEL_IDENTITIES_BY_TRACK
                .iter()
                .map(|(id, _)| *id)
                .collect();
            format!(
                "model identity for track_id {track_id:?} is {MODEL_IDENTITY_UNDECLARED}: its \
                 golden model_type, vocabulary bound, layer count and seed length are not declared \
                 in MODEL_IDENTITIES_BY_TRACK (declared tracks: {declared:?}); refusing to load a \
                 golden"
            )
        })
}

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
/// The OFFICIAL (ranked) path's unmeasured prefill passes inside the TIMED session, run before the
/// one timed prefill (David 2026-09-07). The timed leg attaches a fresh worker session, and a
/// session's FIRST prefill pays the drain-at-accept and any page-cache state the previous
/// session left (on the Qwen 3.8 125B ranked box: 0.68-0.91 ms/token across boots against
/// 0.64 ±0.2 % from the second pass on), so a one-pass timed prefill refused the ±5 % band at
/// random even after the process was warm. ONE unmeasured pass in the timed session, then the
/// timed one, mirrors what every serving engine does before it measures. Fixed count, no
/// settling loop; the local modes keep [`BENCHMARK_PREFILL_WARMUP_RUNS`].
pub const OFFICIAL_PREFILL_WARMUP_RUNS_MLX: usize = 1;
/// The CUDA counterpart of [`OFFICIAL_PREFILL_WARMUP_RUNS_MLX`]: ZERO. The CUDA track runs against a
/// resident engine that serve-up boots and health-checks before the window, so its first prefill
/// is already a steady reading, and its official baseline pair was captured with no warm-up pass.
/// The ds4 adapter also fails closed on a second `prefill` opener without a `phase_diagnostics`
/// barrier between them, so a warm-up pass there is a protocol error, not a warmer number.
pub const OFFICIAL_PREFILL_WARMUP_RUNS_CUDA: usize = 0;
/// `MLXFastConstants.benchmarkPrefillTimedRuns` — one measured prefill run.
pub const BENCHMARK_PREFILL_TIMED_RUNS: usize = 1;

#[cfg(test)]
mod tests {
    use super::*;

    /// FIXTURE-INERT. Every track the table already carries resolves to exactly the pair the
    /// GLOBAL constants that track carried before the table existed gave. The literals below are
    /// those constants' own values, spelled out here rather than read from the table, so the
    /// assertion cannot be satisfied by whatever the table happens to say.
    #[test]
    fn every_declared_track_resolves_to_the_pair_the_globals_gave() {
        const WAS_GLOBAL_PREFILL: f64 = 0.00036751938916015625;
        const WAS_GLOBAL_DECODE: f64 = 0.01385621216015625;
        // The gemma 4 26B A4B MLX track's own pair, as its release branch carried it in the two
        // global constants before this merge folded that branch back in.
        const WAS_GEMMA_GLOBAL_PREFILL: f64 = 0.0003276219582519531;
        const WAS_GEMMA_GLOBAL_DECODE: f64 = 0.012374741210937498;
        // The Qwen 3.8 125B-A6B MLX track's pair, as calibrated on the Darkbloom runner on the
        // track's ranked box (M5 #4, 2026-09-06, ranked run 34062880993, engine 8df7f06 / fork 310daa2).
        // Spelled out here for the same reason as the two above: the assertion must not be
        // satisfiable by whatever `OFFICIAL_BASELINE_MLX` happens to say.
        const WAS_125B_MLX_PREFILL: f64 = 0.0006282488193359375;
        const WAS_125B_MLX_DECODE: f64 = 0.0329116748046875;
        // The CUDA track's pair is DELIBERATELY absent from this enumeration: it is PENDING again
        // for the ds4 engine capture (David 2026-09-03), so there is no pair for it to resolve to.
        // Its retired vLLM values appear below as the NEGATIVE control.

        // The tracks this tree serves, and the pair each one must resolve to — `None` for a track
        // whose pair is pending. One row per declared track: a track added to the table without a
        // row here fails the length check below, so the enumeration cannot silently fall behind.
        let expected = [
            (
                "qwen3.8-27b-mtp-v1",
                Some(OfficialBaseline {
                    prefill_seconds_per_token: WAS_GLOBAL_PREFILL,
                    decode_seconds_per_token: WAS_GLOBAL_DECODE,
                    bands: LEGACY_TWO_SIDED_BANDS,
                }),
            ),
            (
                "gemma4-26b-a4b-mlx-v1",
                Some(OfficialBaseline {
                    prefill_seconds_per_token: WAS_GEMMA_GLOBAL_PREFILL,
                    decode_seconds_per_token: WAS_GEMMA_GLOBAL_DECODE,
                    bands: LEGACY_TWO_SIDED_BANDS,
                }),
            ),
            (
                "qwen3.8-125b-a6b-mlx-v1",
                Some(OfficialBaseline {
                    prefill_seconds_per_token: WAS_125B_MLX_PREFILL,
                    decode_seconds_per_token: WAS_125B_MLX_DECODE,
                    bands: MTP_SINGLE_LEG_BANDS,
                }),
            ),
            (
                "qwen3.8-125b-a6b-cuda-v1",
                Some(OfficialBaseline {
                    prefill_seconds_per_token: 0.002306397331787109,
                    decode_seconds_per_token: 0.06740963554492188,
                    bands: MTP_SINGLE_LEG_BANDS,
                }),
            ),
        ];
        assert_eq!(
            OFFICIAL_BASELINES_BY_TRACK.len(),
            expected.len(),
            "a track was added to or removed from the table without updating this enumeration"
        );
        for (track_id, want) in expected {
            match want {
                Some(want) => assert_eq!(
                    official_baseline(track_id).unwrap(),
                    want,
                    "{track_id} no longer resolves to the pair it scored against"
                ),
                None => {
                    let err = official_baseline(track_id).unwrap_err();
                    assert!(
                        err.contains(track_id) && err.contains(OFFICIAL_BASELINE_PENDING),
                        "{track_id} is pending, so it must refuse BY NAME: {err}"
                    );
                }
            }
        }

        // NEGATIVE CONTROL for the ds4 re-pending: the retired vLLM pair is preserved BIT-EXACT
        // for provenance, and NOTHING resolves to it. A re-pending that merely moved the numbers
        // to another readable slot would pass every assertion above and still let a scored run
        // reach a pair measured on an engine the track no longer runs.
        const RETIRED_VLLM_PREFILL: f64 = 0.0004879835673828125;
        const RETIRED_VLLM_DECODE: f64 = 0.06451959972265625;
        assert_eq!(
            OFFICIAL_BASELINE_CUDA_VLLM_RETIRED,
            OfficialBaseline {
                prefill_seconds_per_token: RETIRED_VLLM_PREFILL,
                decode_seconds_per_token: RETIRED_VLLM_DECODE,
                bands: MTP_SINGLE_LEG_BANDS,
            },
            "the retired vLLM pair must be preserved exactly as it was pinned"
        );
        for (_, declared) in OFFICIAL_BASELINES_BY_TRACK {
            assert_ne!(
                *declared,
                Some(OFFICIAL_BASELINE_CUDA_VLLM_RETIRED),
                "the retired vLLM pair is reachable through the table"
            );
        }
        for platform in Platform::ALL {
            assert_ne!(
                platform.official_baseline_declared(),
                Some(OFFICIAL_BASELINE_CUDA_VLLM_RETIRED),
                "the retired vLLM pair is reachable through Platform::{platform:?}"
            );
        }

        // The branch's own track is one of them: this tree can score itself.
        assert!(
            official_baseline_declared(TRACK_ID).is_some(),
            "the release branch's own track {TRACK_ID} has no captured baseline"
        );

        // ONE set of numbers, two keys: the 125B rows and `Platform::official_baseline` are the
        // same value, so a re-pin on either side cannot leave the other scoring the old pair.
        for (platform, track_id) in [
            (Platform::Mlx, "qwen3.8-125b-a6b-mlx-v1"),
            (Platform::Cuda, "qwen3.8-125b-a6b-cuda-v1"),
        ] {
            assert_eq!(
                platform.official_baseline_declared(),
                official_baseline_declared(track_id),
                "{track_id} and Platform::{platform:?} resolve to different baselines"
            );
        }
    }

    /// A track with no captured pair REFUSES BY NAME instead of falling back. This is the whole
    /// defect the table replaces: two global constants had no key, so a new track scored against
    /// another track's numbers and nothing refused.
    #[test]
    fn an_uncaptured_track_refuses_by_name() {
        const UNCAPTURED: &str = "qwen3.9-27b-mlx-v1";
        assert!(
            official_baseline_declared(UNCAPTURED).is_none(),
            "precondition: {UNCAPTURED} must not be a declared track"
        );
        let err = official_baseline(UNCAPTURED).unwrap_err();
        assert!(
            err.contains(UNCAPTURED),
            "the refusal must name the track: {err}"
        );
        assert!(
            err.contains(OFFICIAL_BASELINE_PENDING),
            "the refusal must name the pending sentinel: {err}"
        );
        assert!(
            err.contains(TRACK_ID),
            "the refusal must name the tracks that DO have a pair: {err}"
        );
    }

    /// The third leg of `constant≡contract≡env`: a declared track that is not this tree's track
    /// refuses BY NAME, naming BOTH values. This is the leg the env/contract cross-check cannot
    /// cover — those two can agree with each other and still both be foreign.
    #[test]
    fn a_foreign_declared_track_refuses_naming_both() {
        const FOREIGN: &str = "gemma4-26b-a4b-mlx-v1";
        assert_ne!(FOREIGN, TRACK_ID);
        let err = enforce_declared_track(FOREIGN).unwrap_err();
        assert!(err.contains("track_id fence"), "{err}");
        assert!(err.contains(FOREIGN), "must name the declared track: {err}");
        assert!(err.contains(TRACK_ID), "must name the tree's track: {err}");

        // This tree's own track passes, with or without surrounding whitespace — fixture-inert
        // for every path that already declares it.
        assert!(enforce_declared_track(TRACK_ID).is_ok());
        assert!(enforce_declared_track(&format!("  {TRACK_ID}  ")).is_ok());
        // A near-miss is foreign: the match is exact.
        assert!(enforce_declared_track(&format!("{TRACK_ID}-v2")).is_err());
        assert!(enforce_declared_track("").is_err());
    }

    /// FIXTURE-INERT. The declared regime of the track this branch serves REPRODUCES today's
    /// published score exactly.
    ///
    /// The track scores DECODE-ONLY paired: the published figure is the even-n median of the
    /// per-prompt raw decode ratios ([`crate::score::score_paired_decode_only`]), and no prefill
    /// phase is separately scored. The declaration says the same thing with two exponents —
    /// prefill `0.0`, decode `1.0` — so the composite over any prefill gain WHATSOEVER is the
    /// decode gain itself, bit-for-bit.
    ///
    /// MUTATION PROOF: change either exponent in `SCORED_REGIMES_BY_TRACK` and this test fails.
    /// A prefill exponent of anything but `0.0` makes the composite depend on a gain this track
    /// does not measure; a decode exponent of anything but `1.0` moves the published number.
    #[test]
    fn the_declared_regime_reproduces_todays_score() {
        let regime = scored_regime(TRACK_ID).expect("this branch's own track declares its regime");
        assert_eq!(
            regime.scored_batch_size, 1,
            "the track scores the SINGLE-STREAM point"
        );
        assert_eq!(
            regime.prefill_gain_exponent, 0.0,
            "decode-only: the prefill axis carries no weight"
        );
        assert_eq!(
            regime.decode_gain_exponent, 1.0,
            "decode-only: the composite IS the decode gain"
        );
        // Zero prefill weight ⇒ certification is NOT armed on this track.
        assert!(
            !regime.prefill_is_scored(),
            "a decode-only track must not arm prefill-window certification"
        );

        // The identity, over the values the paired gate actually produces AND over the pathological
        // prefill gains a track that does not measure prefill can present.
        for decode_gain in [
            0.9,
            1.0,
            1.234_567_890_123_456_7,
            crate::constants::QWEN_MTP_EXPECTED_RAW_MEDIAN,
            5.0,
        ] {
            for prefill_gain in [0.0, 0.5, 1.0, 7.0, f64::NAN, f64::INFINITY] {
                let composite = crate::score::composite_score(&regime, prefill_gain, decode_gain);
                assert_eq!(
                    composite.to_bits(),
                    decode_gain.to_bits(),
                    "composite must be the decode gain bit-for-bit \
                     (prefill_gain={prefill_gain}, decode_gain={decode_gain})"
                );
            }
        }

        // …and over the score the paired decode-only gate publishes, which is that decode gain.
        let published = crate::score::score_paired_decode_only(&[1.05, 0.97], &[1.05, 0.97]);
        let median = published.score.expect("the pair passes every bound");
        assert_eq!(
            crate::score::composite_score(&regime, f64::NAN, median).to_bits(),
            median.to_bits(),
            "the declared regime must not move the published paired decode-only median"
        );
    }

    /// The PAIRED-FLOW fence: the single-leg tracks refuse the paired verbs BY NAME, and the
    /// tracks that DO score through the paired flow still pass. Both directions, because a fence
    /// that refuses everything and a fence that refuses nothing look the same from one side.
    #[test]
    fn the_single_leg_tracks_refuse_the_paired_flow_by_name() {
        for track in SINGLE_LEG_ONLY_TRACKS {
            let err = enforce_paired_flow_available(track).unwrap_err();
            assert!(
                err.contains(PAIRED_FLOW_RETIRED_FOR_TRACK),
                "must name the sentinel: {err}"
            );
            assert!(err.contains(track), "must name the track: {err}");
            assert!(
                err.contains("iterate --mode official"),
                "must name the path the track DOES score through: {err}"
            );
            // The retirement is by TRACK, not by absence: the track still has its own row in the
            // one table (captured, or `None` while its capture is pending — `qwen3.8-125b-a6b-
            // cuda-v1` is pending for the ds4 engine), it simply does not enter the paired flow.
            assert!(
                OFFICIAL_BASELINES_BY_TRACK
                    .iter()
                    .any(|(id, _)| id == track),
                "{track}"
            );
        }
        // POSITIVE CONTROLS — the tracks that score through the paired flow are untouched.
        for track in ["qwen3.8-27b-mtp-v1", "gemma4-26b-a4b-mlx-v1"] {
            assert!(enforce_paired_flow_available(track).is_ok(), "{track}");
        }
        assert!(enforce_paired_flow_available(TRACK_ID).is_ok());
        // The match is EXACT, and surrounding whitespace does not smuggle a track past it.
        assert!(enforce_paired_flow_available("  qwen3.8-125b-a6b-mlx-v1 ").is_err());
        assert!(enforce_paired_flow_available("qwen3.8-125b-a6b-mlx-v2").is_ok());
    }

    /// A track with no declared regime REFUSES BY NAME instead of falling back — the same fence
    /// `official_baseline` puts on the baseline pair, on the other half of "what does this track
    /// score".
    #[test]
    fn an_undeclared_track_refuses_by_name() {
        const UNDECLARED: &str = "qwen3.9-27b-mlx-v1";
        assert!(
            scored_regime_declared(UNDECLARED).is_none(),
            "precondition: {UNDECLARED} must not be a declared track"
        );
        let err = scored_regime(UNDECLARED).unwrap_err();
        assert!(
            err.contains(UNDECLARED),
            "the refusal must name the track: {err}"
        );
        assert!(
            err.contains(SCORED_REGIME_PENDING),
            "the refusal must name the pending sentinel: {err}"
        );
        assert!(
            err.contains(TRACK_ID),
            "the refusal must name the tracks that DO have a regime: {err}"
        );
    }

    /// RED-first — `scored_batch_size` must be ENFORCED, not merely declared.
    ///
    /// benchd has no batch-size concept: it drives one `free_decode_begin` / `free_decode_run` pair
    /// per leg, on one stream. So the ONE measured fact is
    /// [`BENCHD_MEASURED_BATCH_SIZE`] = 1, and a regime declaring anything else describes a point
    /// this binary cannot measure. It refuses BY NAME rather than measuring B=1 and sealing it under
    /// a declared B=8 — a declared-but-unenforced scoring parameter is exactly the defect the
    /// per-track baseline table was cut to fix.
    #[test]
    fn a_batched_regime_refuses_by_name_because_benchd_measures_single_stream() {
        let batched = ScoredRegime {
            scored_batch_size: 8,
            prefill_gain_exponent: 0.25,
            decode_gain_exponent: 0.75,
        };
        let err = enforce_measurable_regime("some-batched-track-v1", &batched).unwrap_err();
        assert!(
            err.contains(SCORED_BATCH_SIZE_UNSUPPORTED),
            "must name the sentinel: {err}"
        );
        assert!(
            err.contains("some-batched-track-v1"),
            "must name the track: {err}"
        );
        assert!(err.contains('8'), "must name the DECLARED batch size: {err}");
        assert!(
            !err.contains("  "),
            "the message must carry no padding-space runs: {err}"
        );

        // A zero batch size is not "unset", it is unmeasurable, and refuses under the same name.
        let zero = ScoredRegime {
            scored_batch_size: 0,
            ..batched
        };
        assert!(enforce_measurable_regime("t", &zero)
            .unwrap_err()
            .contains(SCORED_BATCH_SIZE_UNSUPPORTED));

        // POSITIVE CONTROL — the single-stream point benchd does measure.
        assert!(enforce_measurable_regime(
            "t",
            &ScoredRegime {
                scored_batch_size: BENCHD_MEASURED_BATCH_SIZE,
                ..batched
            }
        )
        .is_ok());

        // And the fence is on the RESOLUTION, not just the helper: this branch's own track resolves
        // because it declares the single-stream point.
        assert_eq!(
            scored_regime(TRACK_ID).unwrap().scored_batch_size,
            BENCHD_MEASURED_BATCH_SIZE
        );
    }

    /// Every declared regime is WELL FORMED: a positive batch size, finite non-negative exponents,
    /// and a positive total weight. A malformed declaration would make the composite meaningless,
    /// and a NEGATIVE prefill exponent would arm certification on an axis that pushes the score the
    /// wrong way. The regime keys are EXACT and unique, for the same reason the baseline keys are.
    #[test]
    fn declared_regimes_are_well_formed() {
        for (i, (id, regime)) in SCORED_REGIMES_BY_TRACK.iter().enumerate() {
            // A declared regime must be one benchd can MEASURE, checked at authoring time so a
            // batched entry fails the build's tests rather than a run on box.
            enforce_measurable_regime(id, regime).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(
                regime.prefill_gain_exponent.is_finite() && regime.prefill_gain_exponent >= 0.0,
                "{id}: prefill exponent must be finite and non-negative"
            );
            assert!(
                regime.decode_gain_exponent.is_finite() && regime.decode_gain_exponent >= 0.0,
                "{id}: decode exponent must be finite and non-negative"
            );
            assert!(
                regime.prefill_gain_exponent + regime.decode_gain_exponent > 0.0,
                "{id}: a regime that weights neither axis scores nothing"
            );
            assert!(
                !SCORED_REGIMES_BY_TRACK[i + 1..]
                    .iter()
                    .any(|(other, _)| other == id),
                "{id} is declared twice"
            );
            assert!(scored_regime_declared(&format!("{id}-v9")).is_none());
            assert!(scored_regime_declared(&id.to_uppercase()).is_none());
            assert!(scored_regime_declared(&format!(" {id}")).is_none());
        }
        // The branch's own track is one of them: this tree can score itself.
        assert!(
            scored_regime_declared(TRACK_ID).is_some(),
            "the release branch's own track {TRACK_ID} has no declared regime"
        );
    }

    /// The key match is EXACT (no prefix, no case folding), and no track is declared twice — a
    /// duplicate key would make the resolution depend on table order.
    #[test]
    fn track_keys_are_exact_and_unique() {
        for (i, (id, _)) in OFFICIAL_BASELINES_BY_TRACK.iter().enumerate() {
            assert!(
                !OFFICIAL_BASELINES_BY_TRACK[i + 1..]
                    .iter()
                    .any(|(other, _)| other == id),
                "{id} is declared twice"
            );
            assert!(official_baseline_declared(&format!("{id}-v9")).is_none());
            assert!(official_baseline_declared(&id.to_uppercase()).is_none());
            assert!(official_baseline_declared(&format!(" {id}")).is_none());
        }
    }

    #[test]
    fn platform_resolves_from_the_canonical_track_id_shape() {
        assert_eq!(
            Platform::from_track_id("qwen3.8-125b-a6b-mlx-v1").unwrap(),
            Platform::Mlx
        );
        assert_eq!(
            Platform::from_track_id("qwen3.8-125b-a6b-cuda-v1").unwrap(),
            Platform::Cuda
        );
        assert_eq!(
            Platform::from_track_id(" qwen3.8-125b-a6b-cuda-v12 ").unwrap(),
            Platform::Cuda
        );
        for bad in [
            "",
            "qwen3.8-125b-a6b-v1",
            "qwen3.8-125b-a6b-spark-v1",
            "qwen3.8-125b-a6b-mlx",
            "qwen3.8-125b-a6b-mlx-v",
            "qwen3.8-125b-a6b-mlx-vX",
            "gemma4-26b-a4b-MLX-v1",
        ] {
            let err = Platform::from_track_id(bad).unwrap_err();
            assert!(err.contains("platform"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn platform_facts_are_distinct_well_formed_and_pending_by_name() {
        let is_hex40 = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let mlx = Platform::Mlx.reference_model();
        let cuda = Platform::Cuda.reference_model();
        assert_ne!(mlx, cuda);
        for m in [mlx, cuda] {
            assert!(!m.repository.is_empty() && m.repository.contains('/'));
            assert!(is_hex40(m.revision), "{}", m.revision);
        }
        assert_ne!(
            Platform::Mlx.official_baseline_pending(),
            Platform::Cuda.official_baseline_pending()
        );
        for p in Platform::ALL {
            assert!(p
                .official_baseline_pending()
                .contains(&p.key().to_ascii_uppercase()));
            if p.official_baseline_declared().is_none() {
                let err = p.official_baseline().unwrap_err();
                assert!(err.contains(p.official_baseline_pending()), "{err}");
                assert!(err.contains(p.key()), "{err}");
            }
        }
    }

    #[test]
    fn cool_gate_temp_is_per_platform_mac_40_gb10_50() {
        // R21 lift (David 2026-08-30): the gate temperature is a trusted per-platform value, not a
        // frozen 40 C constant. Mac/MLX idles cool → 40 C; GB10/CUDA idles at 40–43 C, so its gate
        // is re-sited to 50 C (below the 55 C throttle limit — it re-sites, it does not defang).
        assert_eq!(Platform::Mlx.cool_gate_temp_c(), 40.0);
        assert_eq!(Platform::Cuda.cool_gate_temp_c(), 50.0);
        assert!(
            Platform::Cuda.cool_gate_temp_c() > Platform::Mlx.cool_gate_temp_c(),
            "the GB10 gate is raised above the Mac gate, keyed by platform"
        );
        // Still below the GB10 throttle limit (55 C): a genuinely hot GB10 stays above the gate.
        assert!(Platform::Cuda.cool_gate_temp_c() < 55.0);
    }

    #[test]
    fn worker_holds_model_is_mlx_only() {
        // MLX keeps the model IN the worker → persistent-window load-once residency; CUDA's worker
        // is a stateless adapter over a resident vLLM serve → cheap fresh-per-phase. This predicate
        // is what keys benchd's residency, so it must name exactly MLX (David 2026-08-30).
        assert!(Platform::Mlx.worker_holds_model());
        assert!(!Platform::Cuda.worker_holds_model());
    }
}
