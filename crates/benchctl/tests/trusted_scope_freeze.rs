//! DECIDE-1 — the trusted-source-scope freeze binds on the LIVE benchd path.
//!
//! Drives the REAL `benchctl measure-job --preflight-only` binary (never a mock) over a synthesized
//! on-disk TRUSTED REF: a baseline workspace carrying the roster-of-eight trusted paths plus a
//! `benchmark.json` whose editable surface either DRIFTS to declare a trusted path editable (must be
//! REFUSED, die-8) or stays clear of it (must PASS, exit 0). Every other pre-GPU check is held
//! identical between the drift and pass fixtures, so the trusted-scope guard is the ONLY thing that
//! can move the verdict — the fixtures actually drift (no trivial default), and the refusal asserted
//! is the real process exit + stderr, not an in-process return value.
//!
//! REVERT-PROOF (acceptance mutation). Reverting the wire in `execute_measure_job` — deleting the
//! `trusted_scope::verify_editable_surface_within_trusted_scope(...)?` call — makes every `*_is_
//! refused` case below GREEN as exit 0 (`--preflight-only OK`): the drift is no longer caught. The
//! `legitimate_maintainer_surface_still_passes` case stays exit 0 either way. Both directions were
//! captured in the PR's red-team notes.
//!
//! No organizer bytes are copied here: the tape/contract are SYNTHESIZED to the schema (as in
//! `measure_job_tape_golden.rs`), and the roster tree is empty placeholder files.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const DIE_PREREQ: i32 = 8;

/// PROTOCOL-v1.1's RULED free-run window: every synthesized tape carries at least this many rows so
/// the window is satisfiable from the tape alone (no `--tokens`).
const DECODE_STEPS: usize = bench_core::constants::BENCHMARK_DECODE_STEPS;

/// The roster-of-eight, enumerated INDEPENDENTLY of the crate constant (a private module can not be
/// reached from an integration test) — the test tree must create exactly these so the freeze's
/// anti-vacuous check passes and the overlap arm is what binds. Kept in lockstep with
/// `trusted_scope::ROSTER_OF_EIGHT` by the crate's own `roster_is_exactly_the_ruled_eight` unit test.
/// The eighth entry, `benchmark.json`, is the trusted manifest itself — it is materialized by
/// [`Fixture::set_manifest`] rather than by [`populate_roster`], so an absent-manifest case can leave
/// it off (and a placeholder for it would not be valid JSON).
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

/// The trusted manifest itself — a roster entry, but written per-case by [`Fixture::set_manifest`]
/// (or deliberately absent), never by [`populate_roster`].
const ROSTER_MANIFEST: &str = "benchmark.json";

/// Roster entries that are FILES (the rest are directories). `benchmark.json` is a file too, but is
/// materialized by `set_manifest`, so it is excluded from `populate_roster`'s file handling.
const ROSTER_FILES: [&str; 2] = ["Package.swift", "Package.resolved"];

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A workspace whose `.build/release/mlxfast-runtime-worker` exists and is executable — a real
/// pre-GPU check, never spawned on the preflight path.
fn workspace(root: &Path, name: &str) -> PathBuf {
    let ws = root.join(name);
    let engine = ws.join(".build/release/mlxfast-runtime-worker");
    write(&engine, "#!/bin/sh\nexit 0\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // A pinned release ships the worker binary and its `mlx.metallib` sibling together; the #42
    // pre-GPU adjacency guard refuses at preflight when it is absent. Stage it so the passing beds
    // model a real release.
    write(&ws.join(".build/release/mlx.metallib"), "");
    ws
}

/// Populate the roster-of-eight under a trusted-ref workspace, so the freeze's anti-vacuous check
/// (every roster path must exist) passes and the overlap arm is the one under test. `benchmark.json`
/// (the eighth entry) is SKIPPED here — it is the manifest, materialized by
/// [`Fixture::set_manifest`]; a placeholder for it would not be valid JSON and would defeat the
/// absent-manifest case.
fn populate_roster(ws: &Path) {
    for entry in ROSTER_OF_EIGHT {
        if entry == ROSTER_MANIFEST {
            continue;
        }
        if ROSTER_FILES.contains(&entry) {
            write(&ws.join(entry), "// trusted manifest placeholder\n");
        } else {
            write(&ws.join(entry).join(".keep"), "placeholder\n");
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

struct Fixture {
    root: PathBuf,
    candidate: PathBuf,
    baseline: PathBuf,
    weights: PathBuf,
}

impl Fixture {
    /// A fully passing preflight bed (stub engines, weights, roster-populated trusted baseline).
    /// The baseline's `benchmark.json` is written per-case by [`Self::set_manifest`].
    fn new(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "benchctl-tsscope-{tag}-{}-{}",
            std::process::id(),
            // a monotonic-ish suffix so parallel cases never collide on the same dir
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let candidate = workspace(&root, "candidate-ws");
        let baseline = workspace(&root, "baseline-ws");
        populate_roster(&baseline);
        // WIRE-1 item 1b — the candidate is a submission checkout, i.e. the baseline PLUS edits
        // confined to editablePaths, so its non-editable surface MIRRORS the baseline. Populating the
        // same roster here keeps the write-outside-editablePaths gate a no-op for the passing case
        // (nothing diverges outside editablePaths) while leaving the trusted-scope drift cases — which
        // die on the baseline manifest before that gate runs — unchanged.
        populate_roster(&candidate);
        let weights = root.join("weights");
        write(&weights.join("config.json"), "{}");
        Fixture {
            root,
            candidate,
            baseline,
            weights,
        }
    }

    /// Write the trusted ref's `benchmark.json` editable surface for this case. The candidate carries
    /// an IDENTICAL contract (a real submission ships the same benchmark.json it was judged against),
    /// so the write-outside-editablePaths gate sees no divergence on the contract file itself.
    fn set_manifest(&self, manifest: &serde_json::Value) {
        let body = serde_json::to_string_pretty(manifest).unwrap();
        write(&self.baseline.join("benchmark.json"), &body);
        write(&self.candidate.join("benchmark.json"), &body);
    }

    /// Write RAW bytes as the trusted ref's `benchmark.json` — for the malformed-JSON case, where a
    /// serde-serialized value could never be malformed.
    fn set_manifest_raw(&self, raw: &str) {
        write(&self.baseline.join("benchmark.json"), raw);
    }

    /// Remove a roster path from the trusted ref, so the freeze's anti-vacuous check fires — the
    /// freeze must refuse rather than silently cease to guard a renamed/removed trusted tree.
    fn remove_roster_path(&self, rel: &str) {
        let p = self.baseline.join(rel);
        if p.is_dir() {
            std::fs::remove_dir_all(&p).unwrap();
        } else {
            std::fs::remove_file(&p).unwrap();
        }
        assert!(
            !p.exists(),
            "roster path {rel:?} must be gone for this case"
        );
    }

    /// A symlink under the baseline root pointing at a trusted path, named so NO lexical arm can
    /// match it — only device:inode identity catches it. Returns the link's repo-relative name.
    #[cfg(unix)]
    fn add_symlink_into_scope(&self, link_name: &str, target_rel: &str) -> String {
        std::os::unix::fs::symlink(
            self.baseline.join(target_rel),
            self.baseline.join(link_name),
        )
        .unwrap();
        link_name.to_string()
    }

    fn contract(&self, tape: &str, noop: f64) -> PathBuf {
        let path = self.root.join("contract.json");
        write(
            &path,
            &serde_json::to_string(&serde_json::json!({
                "track_id": "qwen3.8-27b-mtp-v1",
                "timed_prompt_pool": [{
                    "r2_path": "correctness_prompts/synthetic/x.json",
                    "sha256": sha256_hex(tape.as_bytes()),
                    "bytes": tape.len() as u64,
                    "noop_decode_speedup": noop,
                }],
            }))
            .unwrap(),
        );
        path
    }

    fn golden(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        write(&path, body);
        path
    }

    /// Run the REAL `benchctl measure-job --preflight-only`; returns (exit, stderr).
    fn preflight(&self) -> (i32, String) {
        let tape = tape_json(1, DECODE_STEPS);
        let contract = self.contract(&tape, 0.9206);
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
            .arg(&contract)
            .arg("--golden")
            .arg(&golden)
            .arg("--min-pairs")
            .arg("1")
            .arg("--target-pairs")
            .arg("1")
            .arg("--tag")
            .arg("tsscope")
            .arg("--out")
            .arg(self.root.join("out"))
            .arg("--preflight-only")
            .env_remove("QMTP_HEAD_DIR")
            .env_remove("QMTP_CANDIDATE_HEAD_DIR")
            .env_remove("BASELINE_CALIBRATION")
            .env_remove("MLXFAST_QWEN_MTP_TRACK_ID")
            .env_remove("MLXFAST_RUNTIME_WORKER_EXECUTABLE")
            .env_remove("MLXFAST_COMMIT_SHA");
        // AUTHOR-AT-SEAL: this is a SCORING preflight (no --local-dev), so it needs a valid recorded
        // dispatch sha or it would die-8 at the seal guard before reaching the trusted-scope freeze.
        // Give it one (candidate.sha via MLXFAST_CANDIDATE_SHA_FILE) so these tests bind the freeze.
        let record = self.root.join("candidate.sha");
        write(&record, "0123456789abcdef0123456789abcdef01234567\n");
        cmd.env("MLXFAST_CANDIDATE_SHA_FILE", &record);
        let out = cmd.output().expect("spawn benchctl");
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

/// A benign editable surface that overlaps NO roster path — the shape a legitimate maintainer ships.
fn benign_manifest() -> serde_json::Value {
    serde_json::json!({
        "editablePaths": ["Sources/MLXFastModel", "Sources/MLXFastTransform"],
        "optionalEditablePaths": ["Sources/MLXFastModel"],
        "editableSurfaceByteBudget": { "exemptPaths": ["Sources/MLXFastTransform"] },
    })
}

/// ALL EIGHT roster paths, one drift case each: a `benchmark.json` whose `editablePaths` declares
/// the trusted path itself editable is REFUSED (die-8), and the refusal names the trusted path. The
/// eighth path, `benchmark.json`, is covered here too — a manifest declaring its own file editable is
/// refused (see also the dedicated, documented `benchmark_json_declared_editable_is_refused`).
///
/// REVERT-PROOF, both ways: (1) deleting the wire greens every case as exit 0; (2) removing
/// `benchmark.json` from the crate's `ROSTER_OF_EIGHT` greens the `benchmark.json` iteration here as
/// exit 0 (the guard no longer refuses it) while this test's own independent roster still declares
/// it editable — the die-8 assertion then catches the drop.
#[test]
fn every_roster_path_declared_editable_is_refused() {
    for scope in ROSTER_OF_EIGHT {
        let fx = Fixture::new(&format!("roster-{}", scope.replace(['/', '.'], "_")));
        fx.set_manifest(&serde_json::json!({
            // a legit path alongside the drift, so this is a realistic manifest that DRIFTED
            "editablePaths": ["Sources/MLXFastModel", scope],
        }));
        let (code, stderr) = fx.preflight();
        assert_eq!(
            code, DIE_PREREQ,
            "roster path {scope:?} declared editable must die-8; stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("trusted scope:") && stderr.contains(scope),
            "refusal must name the trusted path {scope:?}; stderr:\n{stderr}"
        );
    }
}

/// ITEM-1, dedicated + named. A `benchmark.json` that lists ITS OWN FILE (`benchmark.json`) in
/// `editablePaths` is REFUSED (die-8) and the refusal names the path. Rationale (roster-of-eight
/// ruling): the manifest that DEFINES the editable surface can not itself be declared editable, or a
/// submission could redefine its own limits from inside the very file the freeze reads.
///
/// REVERT-PROOF, both ways: (1) delete the wire in `execute_measure_job` → this greens as exit 0
/// (`--preflight-only OK`); (2) drop `benchmark.json` from the crate's `ROSTER_OF_EIGHT` → the guard
/// stops refusing it and this greens as exit 0. Either mutation flips the die-8 assertion below.
#[test]
fn benchmark_json_declared_editable_is_refused() {
    let fx = Fixture::new("self-manifest");
    fx.set_manifest(&serde_json::json!({
        // a legit path alongside the drift, so this is a realistic manifest that DRIFTED to declare
        // its own file editable
        "editablePaths": ["Sources/MLXFastModel", "benchmark.json"],
    }));
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "benchmark.json declared editable must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("trusted scope:") && stderr.contains("benchmark.json"),
        "refusal must name benchmark.json; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("equals"),
        "the manifest's own file overlaps by lexical equality; stderr:\n{stderr}"
    );
}

/// An entry NESTED inside a trusted dir (`Sources/MLXFastCore/Timer.swift`) overlaps it ("is
/// inside") and is refused.
#[test]
fn entry_inside_a_trusted_dir_is_refused() {
    let fx = Fixture::new("inside");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": ["Sources/MLXFastCore/Timer.swift"],
    }));
    let (code, stderr) = fx.preflight();
    assert_eq!(code, DIE_PREREQ, "nested-in-scope must die-8; {stderr}");
    assert!(stderr.contains("is inside"), "{stderr}");
}

/// An entry that CONTAINS a trusted path (`Sources`, which contains the three trusted source
/// trees) overlaps it ("contains") and is refused.
#[test]
fn entry_containing_a_trusted_path_is_refused() {
    let fx = Fixture::new("contains");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": ["Sources"],
    }));
    let (code, stderr) = fx.preflight();
    assert_eq!(code, DIE_PREREQ, "scope-under-entry must die-8; {stderr}");
    assert!(stderr.contains("contains"), "{stderr}");
}

/// The drift is caught in the `optionalEditablePaths` bucket too, and the refusal NAMES that bucket
/// (three buckets are walked; an entry alone does not say which to fix).
#[test]
fn drift_in_optional_editable_paths_is_refused_and_names_the_bucket() {
    let fx = Fixture::new("optional");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": ["Sources/MLXFastModel"],
        "optionalEditablePaths": ["Package.resolved"],
    }));
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "optional-bucket drift must die-8; {stderr}"
    );
    assert!(stderr.contains("optionalEditablePaths"), "{stderr}");
}

/// And in `editableSurfaceByteBudget.exemptPaths` — an exempt entry is exempt from the BYTE budget,
/// not from this freeze (it is still overlaid).
#[test]
fn drift_in_exempt_paths_is_refused_and_names_the_bucket() {
    let fx = Fixture::new("exempt");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": ["Sources/MLXFastModel"],
        "editableSurfaceByteBudget": { "exemptPaths": [".github"] },
    }));
    let (code, stderr) = fx.preflight();
    assert_eq!(code, DIE_PREREQ, "exempt-bucket drift must die-8; {stderr}");
    assert!(
        stderr.contains("editableSurfaceByteBudget.exemptPaths"),
        "{stderr}"
    );
}

/// A DRIFTED, non-lexical spelling that resolves to a trusted path only by device:inode identity
/// (a symlink named so no casefold/prefix arm can match it) is caught (#24's B3). This is the
/// "inode-resolves" arm the substring test would miss.
#[test]
#[cfg(unix)]
fn inode_identical_spelling_is_refused() {
    let fx = Fixture::new("inode");
    let link = fx.add_symlink_into_scope("z-unrelated-name", "Sources/MLXFastCore");
    fx.set_manifest(&serde_json::json!({
        "editablePaths": [link],
    }));
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "inode-identical entry must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("trusted scope:") && stderr.contains("resolves to"),
        "refusal must be by device:inode identity; stderr:\n{stderr}"
    );
}

/// NO over-rejection: a legitimate maintainer editable surface that overlaps no roster path passes
/// preflight (exit 0). This is the fixture that stays green when the guard is reverted, so the
/// die-8 cases above are provably the guard, not some other pre-GPU check.
#[test]
fn legitimate_maintainer_surface_still_passes() {
    let fx = Fixture::new("legit");
    fx.set_manifest(&benign_manifest());
    // A real submission carries at least one editable source file. Populate one under a declared
    // editablePath in BOTH workspaces (inside editablePaths, so the write-outside gate stays a
    // no-op). Without this the surface walks to zero files and the D8 absent-surface backstop
    // refuses it — correctly, but for a reason this test is not about.
    write(
        &fx.baseline.join("Sources/MLXFastModel/Edited.swift"),
        "// editable\n",
    );
    write(
        &fx.candidate.join("Sources/MLXFastModel/Edited.swift"),
        "// editable\n",
    );
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, 0,
        "a benign editable surface must pass preflight; stderr:\n{stderr}"
    );
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}

/// ITEM-2(a): the ANTI-VACUOUS guard binds on the live path. A trusted ref MISSING a roster path
/// (here `tools` is removed) with an otherwise-benign present manifest is REFUSED (die-8) — the
/// freeze would be VACUOUS against a renamed/removed trusted tree, so it fails loudly rather than
/// silently ceasing to guard — and the refusal names the missing path and says "vacuous".
///
/// REVERT-CONTROLLED: neuter the anti-vacuous guard in `trusted_scope.rs` (make the `missing` filter
/// never retain a path) → this greens as exit 0.
#[test]
fn missing_roster_path_is_refused_as_vacuous() {
    let fx = Fixture::new("vacuous");
    // a benign, present manifest — so ONLY the anti-vacuous arm can move the verdict
    fx.set_manifest(&benign_manifest());
    fx.remove_roster_path("tools");
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "a missing roster path must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("vacuous") && stderr.contains("tools"),
        "refusal must say vacuous and name the missing path; stderr:\n{stderr}"
    );
}

/// ITEM-2(a): the MALFORMED-MANIFEST guard binds on the live path. A trusted-ref `benchmark.json`
/// that is not valid JSON is REFUSED (die-8, fail-closed) — never a fall-open — and the refusal says
/// the parse failed.
///
/// REVERT-CONTROLLED: make `EditableSurface::parse` fall open (return `Ok(default)` on a parse
/// error) → this greens as exit 0.
#[test]
fn malformed_manifest_is_refused_as_parse_failed() {
    let fx = Fixture::new("malformed");
    fx.set_manifest_raw("{ this is not valid json ]");
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, DIE_PREREQ,
        "a malformed trusted manifest must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("parse failed"),
        "refusal must report the parse failure; stderr:\n{stderr}"
    );
}

/// A trusted ref that carries NO benchmark.json declares its editable surface elsewhere — there is
/// nothing to freeze, and preflight passes (the freeze binds exactly when the manifest is present).
///
/// ITEM-2(b): the skip is correct-by-construction (the manifest is the operator-controlled
/// `--baseline` arg; a candidate can not suppress it), but it must not be SILENT — an audit has to
/// tell "checked and passed" from "did not bind". Assert the one-line stderr NOTICE appears on the
/// skip path AND that preflight still exits 0.
///
/// REVERT-CONTROLLED (the notice): delete the `eprintln!` in the absent-manifest else-branch of
/// `execute_measure_job` → the NOTICE assertion below fails.
#[test]
fn absent_manifest_leaves_preflight_passing() {
    let fx = Fixture::new("no-manifest");
    // deliberately no set_manifest()
    let (code, stderr) = fx.preflight();
    assert_eq!(
        code, 0,
        "absent trusted manifest must not fail preflight; {stderr}"
    );
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
    assert!(
        stderr.contains("NOTICE trusted-scope freeze: no benchmark.json"),
        "the silent skip must emit an audit NOTICE; stderr:\n{stderr}"
    );
}
