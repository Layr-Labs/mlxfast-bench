//! WIRE-1 item 1 — the AUTHORITATIVE editable-surface gate binds on the LIVE benchd path.
//!
//! Drives the REAL `benchctl measure-job --preflight-only` binary (never a mock) over a synthesized
//! trusted ref + submission. The trusted `--baseline` carries the roster-of-eight (so the #147
//! trusted-scope freeze passes) plus a `benchmark.json` whose `editablePaths` + byte-budget caps are
//! the contract in force; the `--candidate` is a submission checkout (baseline + edits). Every case
//! below is the SAME passing bed with ONE dimension mutated to overshoot a cap or escape the surface,
//! so the gate under test is the ONLY thing that can move the verdict — the fixtures actually
//! overshoot / escape (not trivial defaults), and the refusal asserted is the real process exit +
//! stderr, not an in-process return value.
//!
//! REVERT-PROOFS (fix-bar, both directions). Each die-8 case differs from
//! [`control_passing_bed_passes_preflight`] only in the mutated dimension, so NEUTERING the matching
//! check in `execute_measure_job` greens that case back to the control's exit 0:
//!   * fix-bar (a) BYTE BUDGET — `candidate_overshooting_max_file_bytes_is_refused`,
//!     `candidate_overshooting_max_total_bytes_is_refused`, `candidate_growth_over_max_growth_is_refused`
//!     die-8 via `byte_budget::verify_byte_budget_over` / `verify_growth_over`; deleting those calls
//!     (or forcing them to `Verified`) makes each pass exit 0.
//!   * fix-bar (b) WRITE-OUTSIDE — `candidate_writing_outside_editable_paths_is_refused` die-8 via
//!     `editable_divergence::verify_no_write_outside_editable`; deleting that call makes it pass.
//!
//! Both directions were captured in the PR's red-team notes.
//!
//! No organizer bytes are copied here: the tape/contract are SYNTHESIZED to the schema (as in
//! `trusted_scope_freeze.rs` / `measure_job_tape_golden.rs`); the roster/editable trees are
//! placeholder files.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const DIE_PREREQ: i32 = 8;

/// PROTOCOL-v1.1's ruled free-run window: every synthesized tape carries at least this many rows so
/// the window is satisfiable from the tape alone.
const DECODE_STEPS: usize = bench_core::constants::BENCHMARK_DECODE_STEPS;

// Post-#150 the trusted-scope roster is EIGHT (the eighth entry is `benchmark.json` itself). It must
// match `trusted_scope::ROSTER_OF_EIGHT` so the anti-vacuous check (every roster path must exist)
// passes and the byte-budget / write-outside gates are what bind.
const ROSTER_OF_EIGHT: [&str; 8] = [
    "Package.swift",
    "Package.resolved",
    "Sources/MLXFastTrustedHarness",
    "Sources/MLXFastCLI",
    "Sources/MLXFastCore",
    ".github",
    "tools",
    "benchmark.json",
];
const ROSTER_MANIFEST: &str = "benchmark.json";
const ROSTER_FILES: [&str; 2] = ["Package.swift", "Package.resolved"];

/// The editable dir the manifests below declare — deliberately NOT a roster path, so the trusted-
/// scope freeze passes and THIS gate is what binds.
const EDITABLE_DIR: &str = "Sources/MLXFastModel";

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn write(path: &Path, body: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A workspace whose `.build/release/mlxfast-runtime-worker` exists and is executable — a real
/// pre-GPU check, never spawned on the preflight path.
fn workspace(root: &Path, name: &str) -> PathBuf {
    let ws = root.join(name);
    let engine = ws.join(".build/release/mlxfast-runtime-worker");
    write(&engine, b"#!/bin/sh\nexit 0\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // A pinned release ships the worker binary and its `mlx.metallib` shader library TOGETHER; the
    // #42 pre-GPU adjacency guard refuses at preflight if the sibling is absent. Stage it here so
    // the passing beds model a real release; the metallib-guard case removes it deliberately.
    write(&ws.join(".build/release/mlx.metallib"), b"");
    ws
}

/// Populate the roster-of-eight under a workspace so the trusted-scope freeze's anti-vacuous check
/// passes. Both baseline and candidate get an IDENTICAL roster (a submission mirrors the trusted
/// ref's non-editable surface), so the write-outside gate sees no divergence there.
fn populate_roster(ws: &Path) {
    for entry in ROSTER_OF_EIGHT {
        // `benchmark.json` (the eighth entry) is materialized by `set_manifest`, not here — a
        // placeholder for it would not be valid JSON.
        if entry == ROSTER_MANIFEST {
            continue;
        }
        if ROSTER_FILES.contains(&entry) {
            write(&ws.join(entry), b"// trusted manifest placeholder\n");
        } else {
            write(&ws.join(entry).join(".keep"), b"placeholder\n");
        }
    }
}

fn tape_json(marker: i64, rows: usize) -> String {
    let chain: Vec<i64> = (0..rows as i64).map(|i| 7_000 + i).collect();
    let row_objs: Vec<serde_json::Value> = chain
        .iter()
        .map(|t| {
            serde_json::json!({
                "sequential_argmax": t,
                "top1_logit": 19.5,
                "top2_logits": [19.5, 18.375],
                "top2_tokens": [t, 321],
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "emitted_tokens": chain,
        "reference_seed_token": 4_625,
        "reference_self_consistent": true,
        "rows": row_objs,
        "seed_tokens": vec![marker; 8],
    }))
    .unwrap()
}

/// Which workspace a fixture edit targets.
enum Leg {
    Baseline,
    Candidate,
    Both,
}

struct Fixture {
    root: PathBuf,
    candidate: PathBuf,
    baseline: PathBuf,
    weights: PathBuf,
}

impl Fixture {
    /// A fully passing preflight bed: stub engines, weights, roster-populated + mirrored baseline and
    /// candidate. The `benchmark.json` + editable trees are written per-case.
    fn new(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "benchctl-esgate-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let candidate = workspace(&root, "candidate-ws");
        let baseline = workspace(&root, "baseline-ws");
        populate_roster(&baseline);
        populate_roster(&candidate);
        let weights = root.join("weights");
        write(&weights.join("config.json"), b"{}");
        Fixture {
            root,
            candidate,
            baseline,
            weights,
        }
    }

    /// Write `benchmark.json` (the editable-surface manifest) to BOTH legs — a submission ships the
    /// same contract it is judged against, so the contract file itself never diverges.
    fn set_manifest(&self, manifest: &serde_json::Value) {
        let body = serde_json::to_string_pretty(manifest).unwrap();
        write(&self.baseline.join("benchmark.json"), body.as_bytes());
        write(&self.candidate.join("benchmark.json"), body.as_bytes());
    }

    /// Write a file of `size` bytes at repo-relative `rel` into the chosen leg(s).
    fn put(&self, leg: Leg, rel: &str, size: usize) {
        let body = vec![b'x'; size];
        match leg {
            Leg::Baseline => write(&self.baseline.join(rel), &body),
            Leg::Candidate => write(&self.candidate.join(rel), &body),
            Leg::Both => {
                write(&self.baseline.join(rel), &body);
                write(&self.candidate.join(rel), &body);
            }
        }
    }

    /// Add a symlink at repo-relative `rel` (pointing at `target`) into the candidate leg.
    #[cfg(unix)]
    fn put_symlink(&self, rel: &str, target: &str) {
        let p = self.candidate.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, p).unwrap();
    }

    fn contract_fixture(&self, tape: &str, noop: f64) -> PathBuf {
        let path = self.root.join("contract.json");
        write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "track_id": "qwen3.8-27b-mtp-v1",
                "timed_prompt_pool": [{
                    "r2_path": "correctness_prompts/synthetic/x.json",
                    "sha256": sha256_hex(tape.as_bytes()),
                    "bytes": tape.len() as u64,
                    "noop_decode_speedup": noop,
                }],
            }))
            .unwrap()
            .as_bytes(),
        );
        path
    }

    fn golden(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        write(&path, body.as_bytes());
        path
    }

    /// Run the REAL `benchctl measure-job --preflight-only`; returns (exit, stderr).
    fn preflight(&self) -> (i32, String) {
        let tape = tape_json(1, DECODE_STEPS);
        let contract = self.contract_fixture(&tape, 0.9206);
        let golden = self.golden("pool-alpha.json", &tape);
        let out = Command::new(env!("CARGO_BIN_EXE_benchctl"))
            .arg("measure-job")
            .arg("--candidate")
            .arg(&self.candidate)
            .arg("--baseline")
            .arg(&self.baseline)
            .arg("--weights")
            .arg(&self.weights)
            .arg("--contract")
            .arg(&contract)
            .arg("--golden")
            .arg(&golden)
            .arg("--min-pairs")
            .arg("1")
            .arg("--target-pairs")
            .arg("1")
            .arg("--tag")
            .arg("esgate")
            .arg("--out")
            .arg(self.root.join("out"))
            .arg("--preflight-only")
            // `--local-dev` for the same reason as `preflight_correctness` below (#149): this
            // fixture's job is the editable-surface gate, not commit binding; without it the
            // author-at-seal guard refuses the record-less bed before the gate under test runs.
            .arg("--local-dev")
            .env_remove("QMTP_HEAD_DIR")
            .env_remove("QMTP_CANDIDATE_HEAD_DIR")
            .env_remove("BASELINE_CALIBRATION")
            .env_remove("MLXFAST_QWEN_MTP_TRACK_ID")
            .env_remove("MLXFAST_RUNTIME_WORKER_EXECUTABLE")
            .env_remove("MLXFAST_CANDIDATE_SHA_FILE")
            .output()
            .expect("spawn benchctl");
        (
            out.status.code().expect("benchctl exited via signal"),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    // The dispatched-commit record for `--local-dev` author-at-seal (#149): these LANE 2a cases run
    // under `--local-dev` so the author-at-seal gate (which fires AFTER the correctness-golden gate,
    // and whose scoring-path env is the harness's known-pending fixture) resolves locally, letting a
    // clean correctness-golden run reach exit 0. The correctness-golden gate itself is unconditional,
    // so the die-8 cases still die at it first.

    /// LANE 2a — like [`Fixture::preflight`], but drives the correctness-golden ATTESTATION gate:
    /// `fixture_pin` optionally pins a `hidden_correctness_golden` SIBLING in the contract, and
    /// `attestation` optionally passes `--correctness-golden <PATH>`. Every other input is the same
    /// passing bed, so this gate is the only thing that can move the verdict.
    fn preflight_correctness(
        &self,
        fixture_pin: Option<(&str, u64)>,
        attestation: Option<&Path>,
    ) -> (i32, String) {
        let tape = tape_json(1, DECODE_STEPS);
        // Base contract (1-entry pool matching the golden), optionally carrying the SIBLING pin.
        let mut contract = serde_json::json!({
            "track_id": "qwen3.8-27b-mtp-v1",
            "timed_prompt_pool": [{
                "r2_path": "correctness_prompts/synthetic/x.json",
                "sha256": sha256_hex(tape.as_bytes()),
                "bytes": tape.len() as u64,
                "noop_decode_speedup": 0.9206,
            }],
        });
        if let Some((sha, bytes)) = fixture_pin {
            contract.as_object_mut().unwrap().insert(
                "hidden_correctness_golden".into(),
                serde_json::json!({ "sha256": sha, "bytes": bytes }),
            );
        }
        let contract_path = self.root.join("contract-hcg.json");
        write(
            &contract_path,
            serde_json::to_string(&contract).unwrap().as_bytes(),
        );
        let golden = self.golden("pool-alpha.json", &tape);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_benchctl"));
        cmd.arg("measure-job")
            .arg("--candidate")
            .arg(&self.candidate)
            .arg("--baseline")
            .arg(&self.baseline)
            .arg("--weights")
            .arg(&self.weights)
            .arg("--contract")
            .arg(&contract_path)
            .arg("--golden")
            .arg(&golden);
        if let Some(att) = attestation {
            cmd.arg("--correctness-golden").arg(att);
        }
        let out = cmd
            .arg("--min-pairs")
            .arg("1")
            .arg("--target-pairs")
            .arg("1")
            .arg("--tag")
            .arg("hcg")
            .arg("--out")
            .arg(self.root.join("out"))
            .arg("--preflight-only")
            .arg("--local-dev")
            .env_remove("QMTP_HEAD_DIR")
            .env_remove("QMTP_CANDIDATE_HEAD_DIR")
            .env_remove("BASELINE_CALIBRATION")
            .env_remove("MLXFAST_QWEN_MTP_TRACK_ID")
            .env_remove("MLXFAST_RUNTIME_WORKER_EXECUTABLE")
            .output()
            .expect("spawn benchctl");
        (
            out.status.code().expect("benchctl exited via signal"),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A generous manifest that binds nothing on its own — the control bed and the anchor every die-8
/// case is a one-dimension mutation of.
fn generous_manifest() -> serde_json::Value {
    serde_json::json!({
        "editablePaths": [EDITABLE_DIR],
        "editableSurfaceByteBudget": {
            "maxTotalBytes": 3_000_000,
            "maxFileBytes": 524_288,
            "maxGrowthBytes": 262_144
        }
    })
}

/// The ANCHOR: a submission whose editable file is small, identical in both legs, and within every
/// cap passes preflight (exit 0). This is the case that STAYS green when any single gate is neutered,
/// proving the die-8 cases below are attributable to the gate under test.
#[test]
fn control_passing_bed_passes_preflight() {
    let fx = Fixture::new("control");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, 0,
        "the control bed must pass preflight; stderr:\n{stderr}"
    );
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}

/// #42 box-leg — the metallib-sibling PRE-GPU adjacency guard. The control bed above passes; this is
/// that SAME bed with ONLY the resolved worker's sibling `mlx.metallib` removed, so this guard is the
/// only thing that can move the verdict. A worker without its sibling metallib dies LATE (first
/// MLXArray inside the GPU window) — the guard turns that into a clear pre-GPU die-8. RED if the
/// `verify_worker_metallib_sibling` check is reverted: the bed would then pass preflight (exit 0).
#[test]
fn worker_missing_sibling_metallib_is_refused() {
    let fx = Fixture::new("no-metallib");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    // Remove the sibling metallib the passing bed stages next to the candidate worker.
    std::fs::remove_file(fx.candidate.join(".build/release/mlx.metallib")).unwrap();
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "a worker missing its sibling mlx.metallib must die-8 pre-GPU; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("mlx.metallib"),
        "the refusal must name the missing mlx.metallib sibling; stderr:\n{stderr}"
    );
}

/// fix-bar (a) — a candidate editable file above `maxFileBytes` is REFUSED (die-8), and the refusal
/// names the byte budget + the per-file bound.
#[test]
fn candidate_overshooting_max_file_bytes_is_refused() {
    let fx = Fixture::new("maxfile");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": [EDITABLE_DIR],
        "editableSurfaceByteBudget": { "maxTotalBytes": 3_000_000, "maxFileBytes": 512 }
    }));
    // 2000 > 512; identical in both legs so ONLY the per-file cap (not growth/divergence) can fire.
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 2000);
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "oversized editable file must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("editable-surface byte budget") && stderr.contains("per-file"),
        "refusal must name the byte budget + per-file bound; stderr:\n{stderr}"
    );
}

/// fix-bar (a) — a candidate editable surface above `maxTotalBytes` is REFUSED (die-8), naming the
/// total bound.
#[test]
fn candidate_overshooting_max_total_bytes_is_refused() {
    let fx = Fixture::new("maxtotal");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": [EDITABLE_DIR],
        "editableSurfaceByteBudget": { "maxTotalBytes": 1500, "maxFileBytes": 1000 }
    }));
    // Two 900-byte files (each < per-file 1000) sum to 1800 > total 1500; identical in both legs.
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/a.swift"), 900);
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/b.swift"), 900);
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "oversized total surface must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("editable-surface byte budget") && stderr.contains("at least"),
        "refusal must name the byte budget + total bound; stderr:\n{stderr}"
    );
}

/// fix-bar (a) — a candidate whose editable CODE grew by more than `maxGrowthBytes` over the trusted
/// baseline is REFUSED (die-8). The per-file/total caps are generous so GROWTH is the sole bound that
/// can fire; the growth stays inside editablePaths so the write-outside gate stays silent.
#[test]
fn candidate_growth_over_max_growth_is_refused() {
    let fx = Fixture::new("growth");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": [EDITABLE_DIR],
        "editableSurfaceByteBudget": {
            "maxTotalBytes": 3_000_000,
            "maxFileBytes": 524_288,
            "maxGrowthBytes": 100
        }
    }));
    // baseline Head = 200 bytes, candidate Head = 2000 bytes ⇒ growth 1800 > 100. A change WITHIN
    // editablePaths, so the write-outside gate does not fire; both sizes are under the per-file cap.
    fx.put(Leg::Baseline, &format!("{EDITABLE_DIR}/Head.swift"), 200);
    fx.put(Leg::Candidate, &format!("{EDITABLE_DIR}/Head.swift"), 2000);
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "over-growth surface must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("editable-surface growth") && stderr.contains("grew by"),
        "refusal must name the growth bound; stderr:\n{stderr}"
    );
}

/// fix-bar (b) — a candidate that ADDS a file OUTSIDE editablePaths (here, into a trusted source dir)
/// is REFUSED (die-8) by the write-outside gate, naming the escaping path. Byte-budget + growth stay
/// silent (the injected file is not under editablePaths, so it never counts toward either).
#[test]
fn candidate_writing_outside_editable_paths_is_refused() {
    let fx = Fixture::new("escape");
    fx.set_manifest(&generous_manifest());
    // A legit small edit inside the surface (identical in both) + an INJECTED file outside it.
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    fx.put(Leg::Candidate, "Sources/MLXFastCore/injected.swift", 64);
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "write outside editablePaths must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("write-divergence")
            && stderr.contains("added")
            && stderr.contains("injected.swift"),
        "refusal must name the escaping added path; stderr:\n{stderr}"
    );
}

/// fix-bar (b) / #151 MEDIUM — a candidate that ADDS a SYMLINK OUTSIDE editablePaths (a
/// source-injection vector into a trusted dir) is REFUSED (die-8) live. git tracks the symlink as a
/// mode-120000 blob, so the ported `git diff --name-only` reference reports it added; this gate must
/// too. REVERT-PROOF: restoring the `is_symlink { continue }` skip in `editable_divergence::hash_tree`
/// greens this case back to exit 0 (the added symlink is no longer enumerated).
#[test]
#[cfg(unix)]
fn candidate_adding_symlink_outside_editable_paths_is_refused() {
    let fx = Fixture::new("symescape");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    // A symlink placed in a trusted source dir, pointing into the editable surface — the exact
    // laptop reproduction. Its LOCATION (trusted dir) is what makes it a write outside the surface.
    fx.put_symlink(
        "Sources/MLXFastCore/Extra.swift",
        "../MLXFastModel/Head.swift",
    );
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "added symlink outside editablePaths must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("write-divergence")
            && stderr.contains("added")
            && stderr.contains("Extra.swift"),
        "refusal must name the escaping symlink; stderr:\n{stderr}"
    );
}

// =====================================================================================
// LANE 2a — the hidden correctness golden's SIBLING pin (engine PR #41).
//
// These drive the REAL binary's correctness-golden ATTESTATION gate in `execute_measure_job`
// (`verify_correctness_golden_attestation`, die-8, pre-GPU). Every case is the SAME passing bed as
// `control_passing_bed_passes_preflight` with only the correctness-golden dimension moved, so
// NEUTERING the gate (deleting the `verify_correctness_golden_attestation(...)?` call) greens each
// refusal back to the control's exit 0 — the load-bearing revert-proof, both directions.
//
// The SIBLING pin never perturbs the anti-lottery cardinality: the 1-entry `timed_prompt_pool`
// coverage gate passes identically whether or not `hidden_correctness_golden` is present (the
// `correct_*` and `pin_absent_*` cases differ only in the sibling / attestation, never the pool).

/// A staged correctness golden and its NAME-FREE identity (sha256 + bytes).
fn stage_correctness_golden(fx: &Fixture, body: &[u8]) -> (PathBuf, String, u64) {
    let path = fx.root.join("hidden-correctness-golden.json");
    write(&path, body);
    (path, sha256_hex(body), body.len() as u64)
}

/// REVERT-PROOF — correct golden PASSES (non-vacuous, baseline-gated): the staged bytes hash to the
/// fixture's pinned sha256+bytes, and the full preflight bed clears exit 0.
#[test]
fn correctness_golden_matching_the_fixture_pin_passes_preflight() {
    let fx = Fixture::new("hcg-ok");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    let (golden, sha, bytes) = stage_correctness_golden(&fx, b"the hidden serial trajectory\n");
    let (code, stderr) = fx.preflight_correctness(Some((&sha, bytes)), Some(&golden));
    assert_eq!(
        code, 0,
        "a correctness golden matching the fixture pin must pass preflight; stderr:\n{stderr}"
    );
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}

/// REVERT-PROOF — wrong-digest golden (staged sha != fixture pin) is REFUSED (die-8).
#[test]
fn correctness_golden_wrong_digest_is_refused() {
    let fx = Fixture::new("hcg-wrong");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    let (golden, _sha, bytes) = stage_correctness_golden(&fx, b"the hidden serial trajectory\n");
    // The fixture pins a DIFFERENT sha256 than the staged bytes hash to.
    let bogus = "0".repeat(64);
    let (code, stderr) = fx.preflight_correctness(Some((&bogus, bytes)), Some(&golden));
    assert_eq!(
        code, DIE_PREREQ,
        "a correctness golden whose bytes do not cite the fixture pin must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("wrong-digest") && stderr.contains("does not cite"),
        "refusal must name the wrong-digest miss; stderr:\n{stderr}"
    );
}

/// REVERT-PROOF — pin-absent is FAIL-CLOSED: the run attests a correctness golden but the fixture
/// pins none, so benchd cannot authorize it (die-8).
#[test]
fn correctness_golden_attested_but_fixture_pins_none_is_fail_closed() {
    let fx = Fixture::new("hcg-nopin");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    let (golden, _sha, _bytes) = stage_correctness_golden(&fx, b"orphan golden\n");
    let (code, stderr) = fx.preflight_correctness(None, Some(&golden));
    assert_eq!(
        code, DIE_PREREQ,
        "an attestation with no fixture pin must fail closed (die-8); stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("fail-closed") && stderr.contains("un-pinned correctness golden"),
        "refusal must name the fail-closed un-pinned case; stderr:\n{stderr}"
    );
}

/// REVERT-PROOF — the mirror fail-closed: the fixture pins the correctness golden but the run omits
/// the `--correctness-golden` attestation, so a scoring run may not silently skip it (die-8).
#[test]
fn fixture_pins_correctness_golden_but_run_omits_attestation_is_fail_closed() {
    let fx = Fixture::new("hcg-noattest");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    let sha = "d7bebe67231e4e66a3134b25322f1dfaaf24543298c05f1d79e6166a48af1713";
    let (code, stderr) = fx.preflight_correctness(Some((sha, 16_949)), None);
    assert_eq!(
        code, DIE_PREREQ,
        "a fixture that pins the golden with no run attestation must fail closed (die-8); \
         stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("fail-closed") && stderr.contains("no correctness-golden attestation"),
        "refusal must name the missing attestation; stderr:\n{stderr}"
    );
}

/// ANTI-LOTTERY UNPERTURBED — with NO correctness golden pinned and none attested (offline/legacy),
/// the very same 1-pool bed still passes: the sibling pin gate is inert, exactly as before LANE 2a.
#[test]
fn no_correctness_golden_pin_and_none_attested_still_passes() {
    let fx = Fixture::new("hcg-legacy");
    fx.set_manifest(&generous_manifest());
    fx.put(Leg::Both, &format!("{EDITABLE_DIR}/Head.swift"), 256);
    let (code, stderr) = fx.preflight_correctness(None, None);
    assert_eq!(
        code, 0,
        "a legacy fixture (no sibling pin, no attestation) must still pass; stderr:\n{stderr}"
    );
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}
