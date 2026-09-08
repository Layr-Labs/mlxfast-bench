//! `calibrate-baseline` — turn a track's CAPTURE RECORDS into the pinned official baseline.
//!
//! The calibration has two halves, and this verb is the SECOND one. The first half already
//! exists and is not re-invented here: `benchd iterate --capture-baseline <REC>
//! --capture-passes <W,A,A,...>` runs the listed legs over ONE resident engine on the official
//! measurement path — same `iterate_flow_windowed`, same decode window, same golden-oracle
//! workload the scored run times — and appends each leg's parent-measured
//! prefill/decode seconds-per-token to the record ([`crate::capture`]). Every one of those legs
//! is SERIAL by construction: `run_capture_passes_over_session` puts no spec on the wire, the
//! spec flags are refused beside `--capture-baseline` at parse, and
//! [`crate::capture::refuse_spec_armed_engine`] refuses a leg whose engine speculated anyway.
//!
//! What was missing is the DECISION the records feed: is the spread small enough for the mean to
//! describe the box, and what exactly gets pinned where. That is this verb:
//!
//! * it recomputes each record's per-axis SAMPLE CV from the legs themselves (never trusting the
//!   number the record carries) and refuses by name — [`CALIBRATION_CV_EXCEEDED`] — above
//!   [`CALIBRATION_MAX_CV_PERCENT`];
//! * it prints the baseline PAIR (the mean of the legs, per axis, at full f64 precision);
//! * it prints the exact `OFFICIAL_BASELINES_BY_TRACK` constants patch for the track's platform
//!   constant, and the `baseline_*_seconds_per_token` fields each golden's `benchmark` block
//!   needs (the golden recorder writes both as `null`).
//!
//! It APPLIES NOTHING. The patch is text for a reviewed PR against `bench-core`, and the golden
//! fields are text for the golden re-author — pinning a scored denominator is David's call, not a
//! tool's.

use bench_core::constants::{Platform, CALIBRATION_CV_EXCEEDED, CALIBRATION_MAX_CV_PERCENT};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
benchd calibrate-baseline — gate a track's capture records and print the baseline pin

USAGE:
    benchd calibrate-baseline --record <REC> [--record <REC> ...]
                                [--track <TRACK-ID>] [--pin <RECORD-STEM>]
                                [--json-out <PATH>]

The records are the files `benchd iterate --capture-baseline <REC> --capture-passes <SPEC>`
wrote — one per timed-pool golden, each holding that golden's N serial legs measured over one
resident engine on the official measurement path.

OPTIONS:
    --record <REC>       A capture record (repeatable; at least one). Its file STEM names the
                         golden in the report and in the golden-field block.
    --track <TRACK-ID>   The track being calibrated (default: env MLXFAST_QWEN_MTP_TRACK_ID).
                         Its `-{platform}-v{N}` suffix names the constant the patch patches, and
                         every record must declare this same track.
    --pin <STEM>         WHICH record's pair becomes the platform constant (the track's required
                         default; each golden still carries its own pair). Required when more
                         than one record is given.
    --json-out <PATH>    Also write the whole report as JSON.
    -h, --help           Show this help

REFUSALS (by name):
    CALIBRATION-CV-EXCEEDED   a record's per-axis sample CV exceeds the fixed maximum
    CALIBRATION-SPEC-ARMED    (raised by `iterate --capture-baseline`) a leg was not serial
";

/// One record's verdict: the legs, their per-axis sample CV, and the mean pair they pin.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordVerdict {
    /// The record file's stem — the golden this pair belongs to.
    pub name: String,
    pub path: String,
    pub run_count: usize,
    pub prefill_seconds_per_token: f64,
    pub decode_seconds_per_token: f64,
    pub prefill_cv_percent: f64,
    pub decode_cv_percent: f64,
    pub golden_sha256: String,
    pub engine_sha256: String,
    pub weights_sha256: String,
    pub benchd_sha256: String,
    pub decode_steps: i64,
    pub mode: String,
}

/// The whole calibration: every record's verdict, plus the pair that gets pinned as the
/// platform constant and the two patches an operator applies by hand.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CalibrationReport {
    pub track_id: String,
    pub platform: String,
    pub max_cv_percent: f64,
    pub records: Vec<RecordVerdict>,
    pub pinned_record: String,
    pub constants_patch: String,
    pub golden_fields: Vec<String>,
}

pub fn run(args: &[String]) -> std::process::ExitCode {
    match execute(args) {
        Ok(None) => {
            print!("{USAGE}");
            std::process::ExitCode::SUCCESS
        }
        Ok(Some(report)) => {
            print!("{}", render(&report));
            std::process::ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("benchd calibrate-baseline: {msg}");
            std::process::ExitCode::from(1)
        }
    }
}

fn execute(args: &[String]) -> Result<Option<CalibrationReport>, String> {
    let mut records: Vec<PathBuf> = Vec::new();
    let mut track: Option<String> = None;
    let mut pin: Option<String> = None;
    let mut json_out: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(None),
            "--record" => records.push(PathBuf::from(value(args, &mut i, "--record")?)),
            "--track" => track = Some(value(args, &mut i, "--track")?),
            "--pin" => pin = Some(value(args, &mut i, "--pin")?),
            "--json-out" => json_out = Some(PathBuf::from(value(args, &mut i, "--json-out")?)),
            other => return Err(format!("unknown argument {other:?}")),
        }
        i += 1;
    }
    if records.is_empty() {
        return Err(
            "missing required --record: a calibration is read from the capture records \
                    `iterate --capture-baseline` wrote, and there is nothing to gate without one"
                .to_string(),
        );
    }
    let track_id = track
        .or_else(|| std::env::var("MLXFAST_QWEN_MTP_TRACK_ID").ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or(
            "missing track: pass --track or set MLXFAST_QWEN_MTP_TRACK_ID. The track names the \
             platform constant this calibration pins, and a pair pinned under the wrong track is \
             a live scoring defect",
        )?;
    let platform = Platform::from_track_id(&track_id)?;
    refuse_track_without_a_platform_constant(&track_id, platform)?;

    let verdicts = records
        .iter()
        .map(|p| verdict_for(p, &track_id))
        .collect::<Result<Vec<_>, String>>()?;
    let pinned = resolve_pin(&verdicts, pin.as_deref())?;
    let report = CalibrationReport {
        constants_patch: constants_patch(platform, &verdicts[pinned]),
        golden_fields: verdicts.iter().map(golden_fields).collect(),
        pinned_record: verdicts[pinned].name.clone(),
        track_id,
        platform: platform.key().to_string(),
        max_cv_percent: CALIBRATION_MAX_CV_PERCENT,
        records: verdicts,
    };
    if let Some(path) = json_out.as_ref() {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| format!("calibration report serialize failed: {e}"))?;
        std::fs::write(path, format!("{json}\n"))
            .map_err(|e| format!("calibration report write failed ({}): {e}", path.display()))?;
    }
    Ok(Some(report))
}

fn value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

/// The patch this verb emits names the PLATFORM constant (`OFFICIAL_BASELINE_{MLX,CUDA}`), which
/// is what the two Qwen 3.8 125B-A6B rows of `OFFICIAL_BASELINES_BY_TRACK` resolve through. A
/// track whose row carries its OWN literal pair (the gemma row, say) would be mis-patched by that
/// text — the platform constant is not where its number lives — so it refuses instead of printing
/// a patch that edits the wrong constant. The test is the two accessors AGREEING: a track that
/// resolves through the platform constant reads the same state through both keys.
fn refuse_track_without_a_platform_constant(
    track_id: &str,
    platform: Platform,
) -> Result<(), String> {
    let through_track = bench_core::constants::official_baseline(track_id).ok();
    if through_track == platform.official_baseline_declared() {
        return Ok(());
    }
    Err(format!(
        "track {track_id:?} does not resolve its official baseline through the platform constant \
         OFFICIAL_BASELINE_{}: its OFFICIAL_BASELINES_BY_TRACK row carries its own pair, so the \
         constants patch this verb prints would edit a constant that track never reads. Pin that \
         row by hand",
        platform.key().to_uppercase()
    ))
}

/// Read one capture record, recompute its statistics from the LEGS, and apply the CV gate.
fn verdict_for(path: &Path, track_id: &str) -> Result<RecordVerdict, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("capture record read failed ({}): {e}", path.display()))?;
    let record: crate::capture::CaptureRecord = serde_json::from_slice(&bytes)
        .map_err(|e| format!("capture record at {} did not parse: {e}", path.display()))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    if record.identity.track_id.trim() != track_id {
        return Err(format!(
            "capture record {} declares track {:?} but this calibration is for {track_id:?} — one \
             calibration pins ONE track's pair, and mixing records would average two tracks into a \
             number that describes neither",
            path.display(),
            record.identity.track_id
        ));
    }
    let prefill: Vec<f64> = record
        .runs
        .iter()
        .map(|r| r.prefill_seconds_per_token)
        .collect();
    let decode: Vec<f64> = record
        .runs
        .iter()
        .map(|r| r.decode_seconds_per_token)
        .collect();
    // Recomputed from the legs, never read off the record: the record's own CV fields are a
    // convenience the capture writer stamped, and a gate that trusts its input gates nothing.
    let mut cvs = Vec::with_capacity(2);
    for (axis, values) in [("prefill", &prefill), ("decode", &decode)] {
        let cv = crate::capture::sample_cv_percent(values).ok_or_else(|| {
            format!(
                "capture record {} carries {} {axis} leg(s): the sample CV is undefined below two \
                 legs, so there is no evidence the mean describes the box",
                path.display(),
                values.len()
            )
        })?;
        if cv > CALIBRATION_MAX_CV_PERCENT {
            return Err(format!(
                "{CALIBRATION_CV_EXCEEDED}: capture record {} has {axis} sample CV {cv:.4}% over \
                 {} leg(s), above the {CALIBRATION_MAX_CV_PERCENT}% maximum. The box was not quiet \
                 enough for the mean to describe it; re-run the calibration rather than pinning a \
                 pair the next run would not reproduce",
                path.display(),
                values.len()
            ));
        }
        cvs.push(cv);
    }
    let mean = |values: &[f64], axis: &str| -> Result<f64, String> {
        crate::capture::mean(values).ok_or_else(|| {
            format!(
                "capture record {} has no finite {axis} mean over its legs",
                path.display()
            )
        })
    };
    Ok(RecordVerdict {
        name,
        path: path.display().to_string(),
        run_count: record.run_count,
        prefill_seconds_per_token: mean(&prefill, "prefill")?,
        decode_seconds_per_token: mean(&decode, "decode")?,
        prefill_cv_percent: cvs[0],
        decode_cv_percent: cvs[1],
        golden_sha256: record.identity.golden_sha256,
        engine_sha256: record.identity.engine_sha256,
        weights_sha256: record.identity.weights_sha256,
        benchd_sha256: record.identity.benchd_sha256,
        decode_steps: record.identity.decode_steps,
        mode: record.identity.mode,
    })
}

/// WHICH record's pair becomes the platform constant. One record needs no choice; more than one
/// does, and guessing (the first, the fastest) would pin a denominator nobody chose.
fn resolve_pin(verdicts: &[RecordVerdict], pin: Option<&str>) -> Result<usize, String> {
    match (pin, verdicts.len()) {
        (None, 1) => Ok(0),
        (None, n) => Err(format!(
            "--pin is required with {n} records: the platform constant is ONE pair (the track's \
             required default), and this calibration measured {n} goldens. Name the record whose \
             pair it takes: {:?}",
            verdicts.iter().map(|v| &v.name).collect::<Vec<_>>()
        )),
        (Some(stem), _) => verdicts.iter().position(|v| v.name == stem).ok_or_else(|| {
            format!(
                "--pin {stem:?} names no record in this calibration: {:?}",
                verdicts.iter().map(|v| &v.name).collect::<Vec<_>>()
            )
        }),
    }
}

/// The exact `bench-core` constants patch: the platform constant `OFFICIAL_BASELINES_BY_TRACK`
/// resolves the track through, at full f64 precision (`{:?}` is the shortest text that round-trips
/// to the same bits, so the pinned value IS the measured mean).
pub fn constants_patch(platform: Platform, pinned: &RecordVerdict) -> String {
    format!(
        "pub const OFFICIAL_BASELINE_{}: Option<OfficialBaseline> = Some(OfficialBaseline {{\n    \
         prefill_seconds_per_token: {:?},\n    decode_seconds_per_token: {:?},\n    bands: \
         MTP_SINGLE_LEG_BANDS,\n}});\n",
        platform.key().to_uppercase(),
        pinned.prefill_seconds_per_token,
        pinned.decode_seconds_per_token,
    )
}

/// The two fields the golden recorder writes as `null` (`record-correctness-golden.rs`,
/// `BenchmarkGolden::baseline_*_seconds_per_token`) and the official run REQUIRES
/// (`resolve_paired_baselines`): each timed-pool golden carries its OWN measured pair.
pub fn golden_fields(v: &RecordVerdict) -> String {
    format!(
        "{}.golden.json  (\"benchmark\" block)\n    \"baseline_prefill_seconds_per_token\": \
         {:?},\n    \"baseline_decode_seconds_per_token\": {:?}\n",
        v.name, v.prefill_seconds_per_token, v.decode_seconds_per_token
    )
}

pub fn render(report: &CalibrationReport) -> String {
    let mut out = format!(
        "benchd calibrate-baseline — track {} (platform {})\n\nCV GATE: sample CV <= {}% on \
         BOTH axes, {} record(s) — PASS\n\n",
        report.track_id,
        report.platform,
        report.max_cv_percent,
        report.records.len()
    );
    for v in &report.records {
        out.push_str(&format!(
            "{}  legs={}  mode={}  decode_steps={}\n  prefill {:?} s/tok  (CV {:.4}%)\n  decode  \
             {:?} s/tok  (CV {:.4}%)\n  golden {}  engine {}\n  weights {}  benchd {}\n",
            v.name,
            v.run_count,
            v.mode,
            v.decode_steps,
            v.prefill_seconds_per_token,
            v.prefill_cv_percent,
            v.decode_seconds_per_token,
            v.decode_cv_percent,
            v.golden_sha256,
            v.engine_sha256,
            v.weights_sha256,
            v.benchd_sha256,
        ));
    }
    out.push_str(&format!(
        "\n--- crates/bench-core/src/constants.rs (pinned from record {:?}) ---\n{}",
        report.pinned_record, report.constants_patch
    ));
    out.push_str("\n--- correctness_prompts/<track>/*.golden.json ---\n");
    for fields in &report.golden_fields {
        out.push_str(fields);
    }
    out.push_str("\nAPPLIED NOTHING: pinning a scored denominator is a reviewed PR, not a tool.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureIdentity, CaptureRecord, CaptureRun};

    const CUDA_TRACK: &str = "qwen3.8-125b-a6b-cuda-v1";

    fn record(track: &str, runs: &[(f64, f64)]) -> CaptureRecord {
        let mut acc: Option<CaptureRecord> = None;
        for (p, d) in runs {
            acc = Some(
                crate::capture::merge(
                    acc,
                    CaptureIdentity {
                        track_id: track.to_string(),
                        mode: "local-iterate".to_string(),
                        decode_steps: 128,
                        engine_sha256: "e".repeat(64),
                        weights_sha256: "w".repeat(64),
                        golden_sha256: "g".repeat(64),
                        benchd_sha256: "b".repeat(64),
                    },
                    CaptureRun {
                        prefill_seconds_per_token: *p,
                        decode_seconds_per_token: *d,
                    },
                )
                .unwrap(),
            );
        }
        acc.unwrap()
    }

    fn write(dir: &Path, name: &str, record: &CaptureRecord) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(record).unwrap()).unwrap();
        path
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("benchd-calibrate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// The HAPPY path end to end: four quiet legs pass the gate, the pair is their MEAN at full
    /// precision, and the emitted patch is the exact `OFFICIAL_BASELINE_CUDA` text a reviewer
    /// applies. The golden-field block carries the same pair, which is what the golden recorder
    /// left as `null`.
    #[test]
    fn quiet_legs_pin_their_mean_and_print_both_patches() {
        let dir = tmpdir("happy");
        let rec = record(
            CUDA_TRACK,
            &[
                (0.000488, 0.06450),
                (0.000488, 0.06452),
                (0.000488, 0.06451),
                (0.000488, 0.06453),
            ],
        );
        let path = write(&dir, "botany.json", &rec);
        let report = execute(&argv(&[
            "--track",
            CUDA_TRACK,
            "--record",
            path.to_str().unwrap(),
        ]))
        .unwrap()
        .unwrap();

        assert_eq!(report.records.len(), 1);
        let v = &report.records[0];
        assert_eq!(v.name, "botany");
        assert_eq!(v.run_count, 4);
        assert_eq!(v.prefill_seconds_per_token, 0.000488);
        assert_eq!(
            v.decode_seconds_per_token,
            (0.06450 + 0.06452 + 0.06451 + 0.06453) / 4.0
        );
        assert_eq!(report.pinned_record, "botany");

        // The patch is the CONSTANT the armed single-leg path resolves through, with the mean
        // round-tripped at full precision.
        assert!(
            report
                .constants_patch
                .contains("pub const OFFICIAL_BASELINE_CUDA: Option<OfficialBaseline>"),
            "{}",
            report.constants_patch
        );
        assert!(report
            .constants_patch
            .contains("bands: MTP_SINGLE_LEG_BANDS"));
        assert!(
            report
                .constants_patch
                .contains(&format!("{:?}", v.decode_seconds_per_token)),
            "{}",
            report.constants_patch
        );
        // The golden's two null fields, named exactly as the golden schema spells them.
        let fields = &report.golden_fields[0];
        assert!(
            fields.contains("\"baseline_prefill_seconds_per_token\""),
            "{fields}"
        );
        assert!(
            fields.contains("\"baseline_decode_seconds_per_token\""),
            "{fields}"
        );
        // The rendered report states the gate it passed and that it changed nothing.
        let text = render(&report);
        assert!(text.contains("CV GATE"), "{text}");
        assert!(text.contains("APPLIED NOTHING"), "{text}");
    }

    /// The CV REFUSAL, by name, with its NEGATIVE CONTROL: the same record shape with a spread
    /// just inside the maximum is accepted, so the gate discriminates rather than always firing.
    #[test]
    fn noisy_legs_refuse_by_name_and_quiet_ones_do_not() {
        let dir = tmpdir("cv");
        // ~7% decode spread — far outside the 1% maximum.
        let noisy = write(
            &dir,
            "noisy.json",
            &record(CUDA_TRACK, &[(0.000488, 0.060), (0.000488, 0.070)]),
        );
        let err = execute(&argv(&[
            "--track",
            CUDA_TRACK,
            "--record",
            noisy.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.contains(CALIBRATION_CV_EXCEEDED), "{err}");
        assert!(err.contains("decode sample CV"), "{err}");

        // NEGATIVE CONTROL: a quiet pair of legs passes the same gate.
        let quiet = write(
            &dir,
            "quiet.json",
            &record(CUDA_TRACK, &[(0.000488, 0.06450), (0.000488, 0.06452)]),
        );
        assert!(execute(&argv(&[
            "--track",
            CUDA_TRACK,
            "--record",
            quiet.to_str().unwrap()
        ]))
        .is_ok());
    }

    /// A single leg has no sample CV, so there is no evidence at all — refused rather than
    /// silently pinned as "perfectly stable".
    #[test]
    fn one_leg_is_not_a_calibration() {
        let dir = tmpdir("single");
        let path = write(&dir, "one.json", &record(CUDA_TRACK, &[(0.000488, 0.0645)]));
        let err = execute(&argv(&[
            "--track",
            CUDA_TRACK,
            "--record",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.contains("undefined below two legs"), "{err}");
    }

    /// A record from another track can never average into this track's pair.
    #[test]
    fn a_foreign_track_record_refuses() {
        let dir = tmpdir("foreign");
        let path = write(
            &dir,
            "other.json",
            &record(
                "qwen3.8-125b-a6b-mlx-v1",
                &[(0.000488, 0.0645), (0.000488, 0.0646)],
            ),
        );
        let err = execute(&argv(&[
            "--track",
            CUDA_TRACK,
            "--record",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.contains("one calibration pins ONE track"), "{err}");
    }

    /// More than one record and no `--pin`: the platform constant is ONE pair, so the verb refuses
    /// to guess which golden it comes from. With `--pin` it takes exactly that record's pair, and
    /// every record still gets its own golden-field block.
    #[test]
    fn the_pinned_record_is_named_never_guessed() {
        let dir = tmpdir("pin");
        let a = write(
            &dir,
            "botany.json",
            &record(CUDA_TRACK, &[(0.000488, 0.0645), (0.000488, 0.0646)]),
        );
        let b = write(
            &dir,
            "chess.json",
            &record(CUDA_TRACK, &[(0.000500, 0.0700), (0.000500, 0.0701)]),
        );
        let both = argv(&[
            "--track",
            CUDA_TRACK,
            "--record",
            a.to_str().unwrap(),
            "--record",
            b.to_str().unwrap(),
        ]);
        let err = execute(&both).unwrap_err();
        assert!(err.contains("--pin is required with 2 records"), "{err}");

        let mut with_pin = both.clone();
        with_pin.extend(argv(&["--pin", "chess"]));
        let report = execute(&with_pin).unwrap().unwrap();
        assert_eq!(report.pinned_record, "chess");
        assert!(
            report.constants_patch.contains(&format!("{:?}", 0.07005)),
            "{}",
            report.constants_patch
        );
        assert_eq!(report.golden_fields.len(), 2);

        let mut bad_pin = both;
        bad_pin.extend(argv(&["--pin", "nope"]));
        assert!(execute(&bad_pin).unwrap_err().contains("names no record"));
    }

    /// The patch text names the PLATFORM constant, so a track whose row carries its own literal
    /// pair must not be handed it. `gemma4-26b-a4b-mlx-v1` resolves to `Platform::Mlx` but reads
    /// its own row — it refuses. NEGATIVE CONTROL: the CUDA track, which DOES resolve through the
    /// platform constant, passes the same check.
    #[test]
    fn a_track_with_its_own_row_refuses_the_platform_patch() {
        assert!(refuse_track_without_a_platform_constant(CUDA_TRACK, Platform::Cuda).is_ok());
        let err = refuse_track_without_a_platform_constant("gemma4-26b-a4b-mlx-v1", Platform::Mlx)
            .unwrap_err();
        assert!(err.contains("OFFICIAL_BASELINE_MLX"), "{err}");
    }

    /// `--json-out` writes the whole report so a driver never parses the human text.
    #[test]
    fn json_out_writes_the_report() {
        let dir = tmpdir("json");
        let path = write(
            &dir,
            "botany.json",
            &record(CUDA_TRACK, &[(0.000488, 0.0645), (0.000488, 0.0646)]),
        );
        let out = dir.join("calibration.json");
        execute(&argv(&[
            "--track",
            CUDA_TRACK,
            "--record",
            path.to_str().unwrap(),
            "--json-out",
            out.to_str().unwrap(),
        ]))
        .unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(written["track_id"], CUDA_TRACK);
        assert_eq!(written["platform"], "cuda");
        assert_eq!(written["pinned_record"], "botany");
        assert!(written["constants_patch"]
            .as_str()
            .unwrap()
            .contains("OFFICIAL_BASELINE_CUDA"));
    }

    /// No records at all, and a track that names no platform, both refuse before anything is read.
    #[test]
    fn missing_inputs_refuse() {
        assert!(execute(&argv(&["--track", CUDA_TRACK]))
            .unwrap_err()
            .contains("missing required --record"));
        let dir = tmpdir("noplatform");
        let path = write(
            &dir,
            "x.json",
            &record(CUDA_TRACK, &[(0.000488, 0.0645), (0.000488, 0.0646)]),
        );
        let err = execute(&argv(&[
            "--track",
            "qwen3.8-27b-mtp-v1",
            "--record",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.contains("is not one of"), "{err}");
    }
}
