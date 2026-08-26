//! `measure-job --golden` accepts the LIVE POOL-TAPE document — proven end to end, OFFLINE.
//!
//! This is the CLI-level counterpart to the unit tests in `measure_job.rs`: it drives the real
//! `benchctl measure-job` binary over `--preflight-only` (every pre-GPU check: golden load,
//! dup-digest guard, contract parse, R4 pin, workspace-engine resolution, digests, track id) with
//! NO GPU, NO engine and NO network, and asserts the 20260819 window's blocking finding is fixed:
//!
//!   1. a POOL-TAPE `--golden` whose sha256 is pinned by the contract's `timed_prompt_pool`
//!      PASSES preflight (exit 0) — the invocation that used to die-8 at load with
//!      "unknown field `emitted_tokens`";
//!   2. a legacy `GoldenDocument` against that same tape-pinned pool still dies-8 (its bytes are
//!      not what the pool pins), now with a diagnostic that NAMES both shapes;
//!   3. R4 is otherwise unchanged: an UNPINNED tape is die-8, and a duplicate `--golden` digest
//!      is die-8.
//!
//! Every fixture here is SYNTHESIZED to the schema (derived from the reference Swift decoder
//! `QwenMTPReferenceGolden` and cross-checked against the live pinned objects) with INVENTED
//! content. No organizer bytes are copied into this repository.

use std::path::{Path, PathBuf};
use std::process::Command;

const DIE_PREREQ: i32 = 8;

/// PROTOCOL-v1.1's RULED free-run window (`BENCHMARK_DECODE_STEPS`), which is the default
/// candidate regime's fixed N — so every synthesized tape carries at least this many rows.
const DECODE_STEPS: usize = bench_core::constants::BENCHMARK_DECODE_STEPS;

/// A synthesized timed-prompt tape: 8 seed tokens, `rows` reference chain, both optional keys
/// present (as every live pinned object carries them). `marker` varies the bytes ⇒ distinct sha.
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

/// A structurally valid legacy `GoldenDocument` with a benchmark oracle (the shape `--golden`
/// modelled before this change).
fn golden_document_json(steps: usize) -> String {
    golden_document_json_with_provenance(steps, None)
}

/// #114 — the same GoldenDocument, optionally carrying a `model_provenance` block naming a
/// (repository, revision). That block is the ONLY place a golden states which model produced it,
/// and the contract's `target` is what it is now held to.
fn golden_document_json_with_provenance(steps: usize, provenance: Option<(&str, &str)>) -> String {
    golden_document_json_full(steps, None, provenance)
}

/// The fully-parameterised builder: `model_type` (defaulting to the required `gemma4_text`) AND
/// `model_provenance`, so a fixture can be wrong in one gate, the other, or BOTH (case A8).
fn golden_document_json_full(
    steps: usize,
    model_type: Option<&str>,
    provenance: Option<(&str, &str)>,
) -> String {
    let mut doc = serde_json::json!({
        "version": 1,
        "model_type": model_type.unwrap_or("gemma4_text"),
        "cases": [{
            "name": "case-a",
            "prompt_tokens": vec![1i64; bench_core::constants::CORRECTNESS_PROMPT_TOKENS],
            "expected_tokens": vec![2i64; 64],
        }],
        "benchmark": {
            "prefill_prompt_tokens": vec![1i64; bench_core::constants::BENCHMARK_PREFILL_PROMPT_TOKENS],
            "expected_prefill_token": 5,
            "decode_seed_tokens": vec![1i64; bench_core::constants::BENCHMARK_DECODE_SEED_TOKENS],
            "expected_decode_seed_token": 6,
            "expected_decode_tokens": (0..steps as i64).map(|i| 700 + i).collect::<Vec<_>>(),
        }
    });
    if let Some((repository, revision)) = provenance {
        doc.as_object_mut().unwrap().insert(
            "model_provenance".into(),
            serde_json::json!({"repository": repository, "revision": revision}),
        );
    }
    serde_json::to_string(&doc).unwrap()
}

/// A structurally valid legacy `GoldenDocument` with NO `benchmark` oracle block — the shape a
/// golden generated for correctness / local-iterate carries (correctness is oracle-optional). It
/// loads and pins exactly like [`golden_document_json`]; only the `.benchmark` oracle is absent.
fn golden_document_json_without_oracle() -> String {
    serde_json::to_string(&serde_json::json!({
        "version": 1,
        "model_type": "gemma4_text",
        "cases": [{
            "name": "case-a",
            "prompt_tokens": vec![1i64; bench_core::constants::CORRECTNESS_PROMPT_TOKENS],
            "expected_tokens": vec![2i64; 64],
        }],
    }))
    .unwrap()
}

use bench_core::hash::sha256_hex;

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A workspace whose `.build/release/mlxfast-runtime-worker` exists and is executable — the
/// engine resolution is a real pre-GPU check, and it is never SPAWNED on the preflight path.
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

/// One `timed_prompt_pool[]` pin, in the LIVE shape: sha256 AND bytes AND the no-op reference.
struct Pin {
    sha256: String,
    bytes: u64,
    noop: f64,
}

/// The pin a given golden body satisfies — both identity halves read off the SAME bytes.
fn pin(body: &str, noop: f64) -> Pin {
    Pin {
        sha256: sha256_hex(body.as_bytes()),
        bytes: body.len() as u64,
        noop,
    }
}

struct Fixture {
    root: PathBuf,
    candidate: PathBuf,
    baseline: PathBuf,
    weights: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "benchctl-tape-preflight-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let candidate = workspace(&root, "candidate-ws");
        let baseline = workspace(&root, "baseline-ws");
        let weights = root.join("weights");
        write(&weights.join("config.json"), "{}");
        Fixture {
            root,
            candidate,
            baseline,
            weights,
        }
    }

    /// A `--contract` track fixture pinning the given pool entries. Only the fields measure-job
    /// consumes are modelled, exactly as the live fixture carries them — including `bytes`, which
    /// every live entry carries and which #112 (L3) made an ENFORCED half of the pin.
    fn contract(&self, pins: &[Pin]) -> PathBuf {
        self.contract_with_target(pins, None)
    }

    /// #114 — the same contract plus the track's declared REFERENCE MODEL, in the challenger
    /// fixture's shape: `target.upstream_model_id` + `target.upstream_revision`. This is the pin a
    /// `model_provenance`-carrying golden is held to, and it lives HERE — in the fixture — rather
    /// than in benchd.
    fn contract_pinning(&self, pins: &[Pin], repository: &str, revision: &str) -> PathBuf {
        self.contract_with_target(
            pins,
            Some(serde_json::json!({
                "upstream_model_id": repository,
                "upstream_revision": revision,
            })),
        )
    }

    /// The general form: an ARBITRARY `target` block (or none), so the adversarial cases can send
    /// a half-declared, empty, renamed-key or placeholder-revision block through the real CLI.
    fn contract_with_target(&self, pins: &[Pin], target: Option<serde_json::Value>) -> PathBuf {
        let pool: Vec<serde_json::Value> = pins
            .iter()
            .map(|p| {
                serde_json::json!({
                    "r2_path": format!("correctness_prompts/synthetic/{}.json", &p.sha256[..8]),
                    "sha256": p.sha256,
                    "bytes": p.bytes,
                    "noop_decode_speedup": p.noop,
                })
            })
            .collect();
        let mut doc = serde_json::json!({
            "track_id": "qwen3.8-27b-mtp-v1",
            "timed_prompt_pool": pool,
            // ARM GATE (David ruling 2026-08-26) — an ARMED track, so every OTHER pre-GPU gate in
            // this file is measured against a fixture that clears the arm gate. Added here rather
            // than in each test: these fixtures exist to exercise pins, windows, coverage and the
            // seal, and an unarmed fixture would refuse them all at the arm gate first and hide
            // what they are actually testing. The arm gate's OWN cases build their contracts with
            // `contract_with_arm_state` below.
            "official_scoring_enabled": true,
        });
        if let Some(target) = target {
            doc.as_object_mut().unwrap().insert("target".into(), target);
        }
        let path = self.root.join("contract.json");
        write(&path, &serde_json::to_string(&doc).unwrap());
        path
    }

    /// ARM GATE — the same well-formed, fully-pinned contract with an ARBITRARY arm state:
    /// `Some(true)` armed, `Some(false)` declared unarmed, `None` the key omitted entirely.
    ///
    /// Written to its OWN filename so an arm-state case cannot clobber (or be clobbered by) the
    /// shared `contract.json` the rest of this fixture uses.
    fn contract_with_arm_state(&self, pins: &[Pin], armed: Option<bool>) -> PathBuf {
        let pool: Vec<serde_json::Value> = pins
            .iter()
            .map(|p| {
                serde_json::json!({
                    "r2_path": format!("correctness_prompts/synthetic/{}.json", &p.sha256[..8]),
                    "sha256": p.sha256,
                    "bytes": p.bytes,
                    "noop_decode_speedup": p.noop,
                })
            })
            .collect();
        let mut doc = serde_json::json!({
            "track_id": "qwen3.8-27b-mtp-v1",
            "timed_prompt_pool": pool,
        });
        if let Some(armed) = armed {
            doc.as_object_mut()
                .unwrap()
                .insert("official_scoring_enabled".into(), serde_json::json!(armed));
        }
        let path = self.root.join("contract-arm.json");
        write(&path, &serde_json::to_string(&doc).unwrap());
        path
    }

    /// MODE FENCE (David ruling 2026-08-26) — the same well-formed, ARMED contract with an
    /// arbitrary `allowed_modes` declaration: `Some(list)` declares it, `None` omits the key
    /// entirely (the state every other track's fixture is in).
    ///
    /// Written to its OWN filename so a mode case cannot clobber (or be clobbered by) the shared
    /// `contract.json` or the arm-state fixture.
    fn contract_with_allowed_modes(&self, pins: &[Pin], modes: Option<&[&str]>) -> PathBuf {
        let pool: Vec<serde_json::Value> = pins
            .iter()
            .map(|p| {
                serde_json::json!({
                    "r2_path": format!("correctness_prompts/synthetic/{}.json", &p.sha256[..8]),
                    "sha256": p.sha256,
                    "bytes": p.bytes,
                    "noop_decode_speedup": p.noop,
                })
            })
            .collect();
        let mut doc = serde_json::json!({
            "track_id": "qwen3.8-27b-mtp-v1",
            "timed_prompt_pool": pool,
            // ARMED, so the mode fence is the only gate these cases can be stopped by before the
            // bed's standing F-6 refusal.
            "official_scoring_enabled": true,
        });
        if let Some(modes) = modes {
            doc.as_object_mut()
                .unwrap()
                .insert("allowed_modes".into(), serde_json::json!(modes));
        }
        let path = self.root.join("contract-modes.json");
        write(&path, &serde_json::to_string(&doc).unwrap());
        path
    }

    fn golden(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        write(&path, body);
        path
    }

    /// Run `benchctl measure-job --preflight-only` over the given goldens; returns (exit, stderr).
    /// SCORING mode (no `--local-dev`) with a VALID recorded dispatch sha, so the AUTHOR-AT-SEAL
    /// fail-closed guard (official.rs) is satisfied and these tests exercise the PREREQ gates — not
    /// the seal. The record-absent and disagreeing-proposal seal refusals are their own tests below.
    fn preflight(&self, contract: &Path, goldens: &[&PathBuf]) -> (i32, String) {
        self.preflight_seal(contract, goldens, Some(FIXTURE_RECORD_SHA), None)
    }

    /// The general form: `record` is the dispatched sha written to `candidate.sha` and threaded via
    /// `MLXFAST_CANDIDATE_SHA_FILE` (`None` = no dispatch context, the fail-closed case); `proposed`
    /// is the untrusted `MLXFAST_COMMIT_SHA` (`None` = unset). Both parent-process values are cleared
    /// first so the child sees exactly what the test declares.
    fn preflight_seal(
        &self,
        contract: &Path,
        goldens: &[&PathBuf],
        record: Option<&str>,
        proposed: Option<&str>,
    ) -> (i32, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_benchctl"));
        cmd.arg("measure-job")
            .arg("--candidate")
            .arg(&self.candidate)
            .arg("--baseline")
            .arg(&self.baseline)
            .arg("--weights")
            .arg(&self.weights)
            .arg("--contract")
            .arg(contract);
        for g in goldens {
            cmd.arg("--golden").arg(g);
        }
        // No `--tokens`: the default candidate regime is the v1.1 free-run series, whose window
        // PROTOCOL-v1.1 fixes at N = BENCHMARK_DECODE_STEPS (128). The tapes below carry that many
        // reference rows, so the window is satisfiable from the tape alone.
        cmd.arg("--min-pairs")
            .arg("1")
            .arg("--target-pairs")
            .arg("1")
            .arg("--tag")
            .arg("tape-preflight")
            .arg("--out")
            .arg(self.root.join("out"))
            .arg("--preflight-only")
            // The head dirs / calibration are separate pre-GPU checks with their own tests; keep
            // this run's environment minimal and deterministic.
            .env_remove("QMTP_HEAD_DIR")
            .env_remove("QMTP_CANDIDATE_HEAD_DIR")
            .env_remove("BASELINE_CALIBRATION")
            .env_remove("MLXFAST_QWEN_MTP_TRACK_ID")
            .env_remove("MLXFAST_RUNTIME_WORKER_EXECUTABLE")
            // AUTHOR-AT-SEAL inputs are declared per test, never inherited from the runner.
            .env_remove("MLXFAST_CANDIDATE_SHA_FILE")
            .env_remove("MLXFAST_COMMIT_SHA");
        if let Some(rec) = record {
            let rec_path = self.root.join("candidate.sha");
            write(&rec_path, &format!("{rec}\n"));
            cmd.env("MLXFAST_CANDIDATE_SHA_FILE", &rec_path);
        }
        if let Some(p) = proposed {
            cmd.env("MLXFAST_COMMIT_SHA", p);
        }
        let out = cmd.output().expect("spawn benchctl");
        (
            out.status.code().expect("benchctl exited via signal"),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    /// MODE FENCE — the measuring bed with an explicit `--candidate-spec`, so a case can declare a
    /// mode the CLI would otherwise never build. Same minimal, deterministic environment as
    /// [`Self::measuring_run`]: no head dirs, no calibration, so the run cannot reach a GPU window
    /// and dies at the first pre-GPU refusal, whose exit + message this returns.
    fn measuring_run_with_spec(
        &self,
        contract: &Path,
        goldens: &[&PathBuf],
        candidate_spec: &str,
    ) -> (i32, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_benchctl"));
        cmd.arg("measure-job")
            .arg("--candidate")
            .arg(&self.candidate)
            .arg("--baseline")
            .arg(&self.baseline)
            .arg("--weights")
            .arg(&self.weights)
            .arg("--contract")
            .arg(contract)
            .arg("--candidate-spec")
            .arg(candidate_spec);
        for g in goldens {
            cmd.arg("--golden").arg(g);
        }
        cmd.arg("--min-pairs")
            .arg("1")
            .arg("--target-pairs")
            .arg("1")
            .arg("--tag")
            .arg("tape-modes")
            .arg("--out")
            .arg(self.root.join("out"))
            .env_remove("QMTP_HEAD_DIR")
            .env_remove("QMTP_CANDIDATE_HEAD_DIR")
            .env_remove("QMTP_DFLASH_HEAD_DIR")
            .env_remove("QMTP_CANDIDATE_DFLASH_HEAD_DIR")
            .env_remove("BASELINE_CALIBRATION")
            .env_remove("BASELINE_BAND_ENFORCE")
            .env_remove("MLXFAST_QWEN_MTP_TRACK_ID")
            .env_remove("MLXFAST_RUNTIME_WORKER_EXECUTABLE")
            .env_remove("MLXFAST_COMMIT_SHA");
        let rec_path = self.root.join("candidate.sha");
        write(&rec_path, &format!("{FIXTURE_RECORD_SHA}\n"));
        cmd.env("MLXFAST_CANDIDATE_SHA_FILE", &rec_path);
        let out = cmd.output().expect("spawn benchctl");
        (
            out.status.code().expect("benchctl exited via signal"),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    /// F-6 — a REAL measure run (NO `--preflight-only`), used to exercise the pre-GPU checks that
    /// live AFTER the preflight-only return. The environment is minimal and deterministic:
    /// `BASELINE_CALIBRATION` and `BASELINE_BAND_ENFORCE` are removed (unset ⇒ ENFORCED), and the
    /// head dirs are removed so the run cannot proceed to the GPU window regardless. It never
    /// reaches a live engine spawn: it dies at the first pre-GPU refusal, whose exit + message this
    /// returns. SCORING mode with a valid dispatch sha so AUTHOR-AT-SEAL is satisfied.
    fn measuring_run(&self, contract: &Path, goldens: &[&PathBuf]) -> (i32, String) {
        self.measuring_run_mode(contract, goldens, false)
    }

    /// ARM GATE — the same measuring bed with `--local-dev` appended when `local_dev` is set, so a
    /// single test can drive the SCORING and LOCAL-DEV paths over identical inputs and attribute
    /// any difference in verdict to that one flag.
    fn measuring_run_mode(
        &self,
        contract: &Path,
        goldens: &[&PathBuf],
        local_dev: bool,
    ) -> (i32, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_benchctl"));
        cmd.arg("measure-job");
        if local_dev {
            cmd.arg("--local-dev");
        }
        cmd.arg("--candidate")
            .arg(&self.candidate)
            .arg("--baseline")
            .arg(&self.baseline)
            .arg("--weights")
            .arg(&self.weights)
            .arg("--contract")
            .arg(contract);
        for g in goldens {
            cmd.arg("--golden").arg(g);
        }
        cmd.arg("--min-pairs")
            .arg("1")
            .arg("--target-pairs")
            .arg("1")
            .arg("--tag")
            .arg("tape-measuring")
            .arg("--out")
            .arg(self.root.join("out"))
            .env_remove("QMTP_HEAD_DIR")
            .env_remove("QMTP_CANDIDATE_HEAD_DIR")
            .env_remove("BASELINE_CALIBRATION")
            .env_remove("BASELINE_BAND_ENFORCE")
            .env_remove("MLXFAST_QWEN_MTP_TRACK_ID")
            .env_remove("MLXFAST_RUNTIME_WORKER_EXECUTABLE")
            .env_remove("MLXFAST_COMMIT_SHA");
        let rec_path = self.root.join("candidate.sha");
        write(&rec_path, &format!("{FIXTURE_RECORD_SHA}\n"));
        cmd.env("MLXFAST_CANDIDATE_SHA_FILE", &rec_path);
        let out = cmd.output().expect("spawn benchctl");
        (
            out.status.code().expect("benchctl exited via signal"),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }
}

/// A valid recorded dispatch sha (candidate.sha shape: strict 40-hex) for the scoring-mode fixtures.
const FIXTURE_RECORD_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pinned_pool_tape_satisfies_the_golden_contract_offline() {
    let fx = Fixture::new("ok");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract = fx.contract(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)]);
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.preflight(&contract, &[&ga, &gb]);
    assert_eq!(
        code, 0,
        "a pinned pool tape must satisfy --golden end to end; stderr:\n{stderr}"
    );
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
    // The preflight line reports WHICH shape was accepted, so satisfiability is readable, not
    // inferred.
    assert!(stderr.contains("timed-prompt-tape=2"), "{stderr}");
    assert!(stderr.contains("golden-document=0"), "{stderr}");
}

/// M2 — the anti-lottery ≥N-DISTINCT COVERAGE gate, bound at its CLI CALL SITE (subprocess).
///
/// The pure-function unit test in `measure_job.rs` proves `validate_timed_pool_coverage` REJECTS a
/// subset — but a pure-fn test can NOT prove `execute_measure_job` still CALLS it (the #149 lesson).
/// Deleting the `validate_timed_pool_coverage(...)` call at main.rs (right after
/// `validate_goldens_pinned`) leaves every offline/unit suite green, because those stubbed seams
/// never reach the live call. This drives the REAL `benchctl measure-job` binary end to end: a
/// contract whose `timed_prompt_pool` pins TWO DISTINCT tapes, but a run supplying only ONE of them
/// — a strict SUBSET. `validate_goldens_pinned` PASSES that (each supplied golden pins
/// individually), so the ONLY thing that can refuse it is the coverage gate at its call site. The
/// run must die-8, naming the exact-coverage breach.
///
/// REVERT-PROOF: deleting the coverage-gate CALL from `execute_measure_job` makes this run reach
/// `--preflight-only OK` (exit 0) — RED here — while the pure-fn unit test stays green. Restoring
/// the call greens it again. That green→red-on-call-deletion is exactly the call-site binding the
/// pure-fn test can not assert.
#[test]
fn a_subset_of_the_pinned_pool_dies8_at_the_coverage_gate_call_site() {
    let fx = Fixture::new("coverage-subset");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    // The pool pins BOTH distinct tapes; the run times only tape_a — a strict subset of the pool.
    let contract = fx.contract(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)]);
    let ga = fx.golden("pool-alpha.json", &tape_a);

    let (code, stderr) = fx.preflight(&contract, &[&ga]);
    assert_eq!(
        code, DIE_PREREQ,
        "a run timing a SUBSET of the pinned pool must die-8 at the coverage gate; stderr:\n{stderr}"
    );
    // The refusal is the COVERAGE gate's — not `validate_goldens_pinned`'s, which ACCEPTS a subset.
    assert!(
        stderr.contains("timed coverage is not EXACTLY"),
        "the refusal must be the coverage gate's exact-coverage breach: {stderr}"
    );
    assert!(
        stderr.contains("SUBSET"),
        "and must name the un-timed pinned prompt(s) as a SUBSET: {stderr}"
    );
    assert!(stderr.contains("die 8"), "{stderr}");
}

/// F1/F2 revert-proof at the CLI (subprocess). A SCORING run (no `--local-dev`) over an OTHERWISE
/// VALID pool whose dispatch record is ABSENT dies-8 at the AUTHOR-AT-SEAL guard — it does NOT fall
/// back to the box git identity (which could seal an empty `metrics.commit`). The pool is the exact
/// one that passes end to end above, so the only difference is the missing record: this proves the
/// refusal is the seal guard, not a prereq failure. Mutation (restore the git fallback on the
/// scoring path) makes this exit 0 and greens.
#[test]
fn scoring_seal_without_dispatch_record_dies8() {
    let fx = Fixture::new("seal-absent");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract = fx.contract(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)]);
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    // record = None ⇒ no MLXFAST_CANDIDATE_SHA_FILE ⇒ absent dispatch context on a scoring run.
    let (code, stderr) = fx.preflight_seal(&contract, &[&ga, &gb], None, None);
    assert_eq!(
        code, DIE_PREREQ,
        "a scoring seal with no dispatch record must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("scoring/ranked seal requires"),
        "the refusal must name the missing dispatch record, not a prereq: {stderr}"
    );
}

/// F2 revert-proof at the CLI (subprocess). A SCORING run whose dispatch record is present but whose
/// untrusted `MLXFAST_COMMIT_SHA` names a DIFFERENT commit dies-8 — benchd authors from the record
/// and refuses the disagreeing proposal. Mutation (neuter the disagreement bind) makes this exit 0.
#[test]
fn scoring_seal_with_disagreeing_proposed_commit_dies8() {
    let fx = Fixture::new("seal-mismatch");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract = fx.contract(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)]);
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let foreign = "89abcdef0123456789abcdef0123456789abcdef"; // valid 40-hex, not the record
    let (code, stderr) = fx.preflight_seal(
        &contract,
        &[&ga, &gb],
        Some(FIXTURE_RECORD_SHA),
        Some(foreign),
    );
    assert_eq!(
        code, DIE_PREREQ,
        "a proposed commit that disagrees with the record must die-8; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("disagrees with the dispatched record"),
        "the refusal must name the disagreement: {stderr}"
    );
}

#[test]
fn legacy_golden_document_against_a_tape_pinned_pool_dies8_with_an_honest_diagnostic() {
    // The window's exact repro: a REAL GoldenDocument, a contract whose pool pins tapes.
    let fx = Fixture::new("legacy");
    let tape = tape_json(1, DECODE_STEPS);
    let contract = fx.contract(&[pin(&tape, 0.9206)]);
    let g = fx.golden(
        "official-calibrated.json",
        &golden_document_json(DECODE_STEPS),
    );

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(
        code, DIE_PREREQ,
        "must be die-8, pre-GPU; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("no pinned per-prompt no-op reference"),
        "{stderr}"
    );
    // The 20260819 diagnostic stopped here and read like a wrong-fixture mistake. It now names
    // both shapes and says why they can never meet.
    assert!(stderr.contains("golden-document"), "{stderr}");
    assert!(stderr.contains("timed-prompt-tape"), "{stderr}");
    assert!(
        stderr.contains("can therefore never match a pool pin"),
        "{stderr}"
    );
}

#[test]
fn unpinned_tape_and_duplicate_digests_stay_die8() {
    // R4 is UNCHANGED by the tape work: pin by the sha of the RAW BYTES, exactly one, fail-closed.
    let fx = Fixture::new("unpinned");
    let pinned = tape_json(1, DECODE_STEPS);
    let contract = fx.contract(&[pin(&pinned, 0.9206)]);

    let stranger = fx.golden("stranger.json", &tape_json(99, DECODE_STEPS));
    let (code, stderr) = fx.preflight(&contract, &[&stranger]);
    assert_eq!(code, DIE_PREREQ, "unpinned tape must die-8; {stderr}");
    assert!(
        stderr.contains("no pinned per-prompt no-op reference"),
        "{stderr}"
    );

    let g = fx.golden("pool-alpha.json", &pinned);
    let (code, stderr) = fx.preflight(&contract, &[&g, &g]);
    assert_eq!(code, DIE_PREREQ, "duplicate digest must die-8; {stderr}");
    assert!(stderr.contains("duplicate --golden digest"), "{stderr}");
}

#[test]
fn a_broken_tape_is_reported_as_a_broken_tape_not_as_a_bad_golden_document() {
    // Signature routing (not "whatever parses") keeps the diagnostic honest: a tape with one
    // defective row must name THAT defect, never fall through to the GoldenDocument loader and be
    // reported as an unknown `emitted_tokens` field.
    let fx = Fixture::new("broken");
    let mut doc: serde_json::Value = serde_json::from_str(&tape_json(1, DECODE_STEPS)).unwrap();
    doc["rows"][3]["sequental_argmax"] = serde_json::json!(9);
    let body = serde_json::to_string(&doc).unwrap();
    let contract = fx.contract(&[pin(&body, 0.9206)]);
    let g = fx.golden("pool-broken.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, DIE_PREREQ, "{stderr}");
    assert!(stderr.contains("timed-prompt tape"), "{stderr}");
    assert!(stderr.contains("sequental_argmax"), "{stderr}");
}

#[test]
fn a_pool_entry_whose_bytes_disagree_with_the_golden_dies8() {
    // #112 (L3) — the pool pins sha256 AND bytes. The byte half was parsed past and never
    // checked; a golden matching the sha but not the declared byte count now dies-8, pre-GPU.
    let fx = Fixture::new("bytes");
    let tape = tape_json(1, DECODE_STEPS);
    let mut p = pin(&tape, 0.9206);
    p.bytes += 1; // right sha, wrong declared byte count
    let contract = fx.contract(&[p]);
    let g = fx.golden("pool-alpha.json", &tape);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, DIE_PREREQ, "byte-pin mismatch must die-8; {stderr}");
    assert!(stderr.contains("sha256 AND bytes"), "{stderr}");
    assert!(
        stderr.contains(&format!("{} bytes", tape.len() + 1)),
        "the diagnostic names the PINNED count; {stderr}"
    );
    assert!(
        stderr.contains(&format!("{} bytes", tape.len())),
        "and the ACTUAL count; {stderr}"
    );
}

#[test]
fn a_tape_too_short_for_the_ruled_window_fails_preflight_and_a_live_length_tape_passes() {
    // #112 (M1). The rows-vs-window rule lived only in the pair loop's per-prompt `timing_params`,
    // which runs AFTER `--preflight-only` returns — so an operator could prove a pool "satisfiable"
    // offline and still lose the gated run on its first prompt. Preflight now applies the same
    // check to every loaded golden, under the RULED window (default regime = v1.1 free-run, whose
    // N PROTOCOL-v1.1 fixes at BENCHMARK_DECODE_STEPS).
    let fx = Fixture::new("short-window");

    // A pinned, well-formed, self-consistent tape — defective ONLY in that 4 reference rows
    // cannot oracle a 128-token window. Every other pre-GPU check passes, so preflight's verdict
    // is about the window and nothing else.
    let short = tape_json(1, 4);
    let contract = fx.contract(&[pin(&short, 0.9206)]);
    let g = fx.golden("pool-short.json", &short);
    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_ne!(
        code, 0,
        "a tape too short for the ruled window must NOT pass preflight; stderr:\n{stderr}"
    );
    assert_eq!(
        code, DIE_PREREQ,
        "pre-GPU prereq failure; stderr:\n{stderr}"
    );
    // The diagnostic is HONEST about the real defect: how many rows there are, what window was
    // asked for, and which golden is at fault.
    assert!(
        stderr.contains("cannot oracle this run's 128-token decode window"),
        "{stderr}"
    );
    assert!(stderr.contains("4 reference rows"), "{stderr}");
    assert!(stderr.contains(&sha256_hex(short.as_bytes())), "{stderr}");

    // The LIVE tape length (513 rows — the 8 pinned pool objects' shape) passes: the window is
    // satisfiable, so preflight says so.
    let live = tape_json(2, 513);
    let contract = fx.contract(&[pin(&live, 0.797)]);
    let g = fx.golden("pool-live-length.json", &live);
    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(
        code, 0,
        "a 513-row tape oracles the window; stderr:\n{stderr}"
    );
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}

#[test]
fn a_document_matching_neither_shape_names_both() {
    let fx = Fixture::new("neither");
    let contract = fx.contract(&[Pin {
        sha256: "00".repeat(32),
        bytes: 1,
        noop: 0.99,
    }]);
    let g = fx.golden("mystery.json", "{\"version\": 1}");

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, DIE_PREREQ, "{stderr}");
    assert!(
        stderr.contains("matches NEITHER accepted shape"),
        "{stderr}"
    );
    assert!(stderr.contains("seed_tokens"), "{stderr}");
    assert!(stderr.contains("cases"), "{stderr}");
}

/// 2b box-leg — the guard-2 revert-proof, END TO END through the real `measure-job` preflight
/// path (not the pure fn). A benchmark-oracle-LESS `GoldenDocument` is a valid, pinned, fully-
/// covering pool member, so it clears every pre-GPU gate up to the ranked-gates ("gates" =
/// ranked/timed benchmark phase, NOT the golden's `correctness_gates`) oracle check
/// (`main.rs` call site → `validate_gates_goldens_carry_oracle`). The refusal must be die-8 and
/// must NAME the `attach-benchmark-oracle` remedy, and that specific diagnostic must WIN over the
/// generic per-prompt window refusal (`measure_job.rs` `validate_prompt_windows`), which is what
/// would otherwise fire on the very same input.
///
/// This exercises the PRODUCTION call site: removing the `validate_gates_goldens_carry_oracle`
/// call in `main.rs` makes this go RED — the run then reaches `validate_prompt_windows` and dies
/// with the generic "cannot oracle this run's window" message instead of naming the remedy. (The
/// pure-fn unit test alone would stay green under that disarm; this closes that gap.)
#[test]
fn gates_routed_golden_without_the_benchmark_oracle_is_refused_naming_the_remedy_e2e() {
    let fx = Fixture::new("no-oracle-e2e");
    let body = golden_document_json_without_oracle();
    // Pinned + fully-covering (single golden ⇒ single pool entry), so nothing before the gates-
    // oracle check can be the cause. No `target`/provenance, so the #114 pin stays untouched.
    let contract = fx.contract(&[pin(&body, 0.9206)]);
    let g = fx.golden("no-oracle.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(
        code, DIE_PREREQ,
        "a gates-routed oracle-less GoldenDocument must die-8 pre-GPU; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("attach-benchmark-oracle"),
        "the refusal must name the attach-benchmark-oracle remedy; stderr:\n{stderr}"
    );
    // The gates-oracle diagnostic must WIN over the generic per-prompt window refusal — proving the
    // check fires at its own early call site, not as a side effect of the window check downstream.
    assert!(
        !stderr.contains("cannot oracle this run"),
        "the remedy-naming refusal must precede the generic window refusal; stderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// #114 — the contract-declared reference-model pin, end to end through measure-job
// ---------------------------------------------------------------------------

/// The reference model this synthetic track declares. It is a FIXTURE value here, exactly as it
/// is on the real track — benchd names no model in code, which is what makes the pin per-track.
/// The string is arbitrary and deliberately carries no real repository: every case below derives
/// both the contract's `target` and the golden's `model_provenance` from THIS constant, so the
/// pin is proven by construction rather than by any particular name.
const REF_REPO: &str = "reference-org/Reference-27B-4bit";
const REF_REVISION: &str = "eda45ab47f465d08d6558f0353a2346e2eb9d5b3";

/// A provenance block naming a model the track does NOT declare.
const OTHER_REPO: &str = "NotTheOrganizer/Some-Other-Model-4bit";
const OTHER_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

// The adversarial matrix. Each case drives the REAL `benchctl measure-job --preflight-only`
// binary; the "must fail" ones are the point of #114 and each names WHICH defect it is.
//
//   A1  no model_provenance, contract pinned                 -> PASS  (baseline)
//   A2  provenance names the declared reference              -> PASS  (positive control)
//   A3  provenance names a DIFFERENT model                   -> FAIL  (the pin itself)
//   A4  target half-declared (one key only)                  -> FAIL  (contract defect)
//   A5  target present, declares NEITHER half / renamed keys -> FAIL  (contract defect, F2)
//   A6  no `target` key at all                               -> PASS  (legacy opt-out)
//   A7  wrong model_type only                                -> FAIL  (model_type gate)
//   A8  wrong model_type AND wrong provenance                -> FAIL  (model_type diagnostic, F1)
//   A9  placeholder revision in the contract                 -> FAIL  (contract defect, F3)
//   A10 padded contract value                                -> FAIL  (contract defect, F4)

#[test]
fn a1_a_golden_with_no_provenance_is_untouched_by_the_pin() {
    let fx = Fixture::new("a1");
    let body = golden_document_json(DECODE_STEPS);
    let contract = fx.contract_pinning(&[pin(&body, 0.9206)], REF_REPO, REF_REVISION);
    let g = fx.golden("no-provenance.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, 0, "A1 must PASS; stderr:\n{stderr}");
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}

#[test]
fn a2_a_golden_naming_the_contracts_reference_model_passes_preflight() {
    // Positive control: without it, every "must fail" case below would also pass if the pin
    // simply rejected everything.
    let fx = Fixture::new("a2");
    let body = golden_document_json_with_provenance(DECODE_STEPS, Some((REF_REPO, REF_REVISION)));
    let contract = fx.contract_pinning(&[pin(&body, 0.9206)], REF_REPO, REF_REVISION);
    let g = fx.golden("right-model.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, 0, "A2 must PASS; stderr:\n{stderr}");
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}

#[test]
fn a3_a_golden_naming_a_different_model_than_the_contract_dies8_before_any_gpu_work() {
    // The #114 divergence, closed: a GoldenDocument that is otherwise PERFECT — pinned in the
    // pool by sha256 AND bytes, right window, right model_type — but whose `model_provenance`
    // names a model the track does not declare. Swift rejects this from its constants; benchd now
    // rejects it from the CONTRACT, pre-GPU.
    let fx = Fixture::new("a3");
    let body =
        golden_document_json_with_provenance(DECODE_STEPS, Some((OTHER_REPO, OTHER_REVISION)));
    let contract = fx.contract_pinning(&[pin(&body, 0.9206)], REF_REPO, REF_REVISION);
    let g = fx.golden("wrong-model.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, DIE_PREREQ, "A3 must FAIL die-8; stderr:\n{stderr}");
    // The reference's own diagnostic, so a submitter reads ONE message from either loader.
    assert!(
        stderr.contains("model_provenance does not match the pinned reference model"),
        "{stderr}"
    );
}

#[test]
fn a4_a_contract_declaring_half_a_reference_model_pin_refuses_the_run() {
    // FAIL-CLOSED: a `target` naming a repository with no revision is the one shape that could let
    // a track LOOK pinned while enforcing nothing. Refused before any golden is even loaded.
    let fx = Fixture::new("a4");
    let body = golden_document_json_with_provenance(DECODE_STEPS, Some((REF_REPO, REF_REVISION)));
    let contract = fx.contract_with_target(
        &[pin(&body, 0.9206)],
        Some(serde_json::json!({"upstream_model_id": REF_REPO})),
    );
    let g = fx.golden("right-model.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, DIE_PREREQ, "A4 must FAIL die-8; stderr:\n{stderr}");
    assert!(stderr.contains("half a reference-model pin"), "{stderr}");
}

#[test]
fn a5_a_target_that_declares_neither_half_refuses_the_run() {
    // F2 — the fall-open the review found. `target: {}`, both keys null, and an upstream key
    // RENAME all reach this: the block is present and names nothing. Making the contract MORE
    // broken must not turn a hard error into a silent unpinned PASS.
    let body = golden_document_json_with_provenance(DECODE_STEPS, Some((REF_REPO, REF_REVISION)));
    for (tag, target) in [
        ("empty", serde_json::json!({})),
        (
            "nulls",
            serde_json::json!({"upstream_model_id": null, "upstream_revision": null}),
        ),
        (
            "renamed",
            serde_json::json!({"model_id": REF_REPO, "revision": REF_REVISION}),
        ),
    ] {
        let fx = Fixture::new(&format!("a5-{tag}"));
        let contract = fx.contract_with_target(&[pin(&body, 0.9206)], Some(target));
        let g = fx.golden("right-model.json", &body);

        let (code, stderr) = fx.preflight(&contract, &[&g]);
        assert_eq!(
            code, DIE_PREREQ,
            "A5/{tag} must FAIL die-8; stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("names NO reference model"),
            "A5/{tag}: {stderr}"
        );
    }
}

#[test]
fn a6_a_contract_with_no_target_leaves_the_provenance_shape_checked_only() {
    // Legacy/offline contracts carry no `target` block. Those runs keep the pre-#114 behaviour
    // (shape-only) rather than failing closed on a key the fixture never had — the residual is
    // scoped to contracts that decline to declare a reference, and it is stated, not hidden.
    // Omitting `target` is the ONLY opt-out; A5 proves the near-miss spellings are errors.
    let fx = Fixture::new("a6");
    let body =
        golden_document_json_with_provenance(DECODE_STEPS, Some((OTHER_REPO, OTHER_REVISION)));
    let contract = fx.contract(&[pin(&body, 0.9206)]);
    let g = fx.golden("unpinned-model.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, 0, "A6 must PASS (shape-only); stderr:\n{stderr}");
    assert!(stderr.contains("--preflight-only OK"), "{stderr}");
}

#[test]
fn a7_a8_the_model_type_gate_fires_before_the_provenance_identity() {
    // F1 — the reference interleaves `requiredModelType` (Golden.swift:377-385) BEFORE the
    // provenance identity guard (:386-393). A7 (model_type wrong only) and A8 (BOTH wrong) must
    // therefore produce the SAME diagnostic — the model_type one. The first draft reported the
    // provenance mismatch for A8: decision-identical to Swift, so the accept/reject harness stayed
    // green while the two loaders printed different messages.
    let a7_body = golden_document_json_full(DECODE_STEPS, Some("gemma_text"), None);
    let a8_body = golden_document_json_full(
        DECODE_STEPS,
        Some("gemma_text"),
        Some((OTHER_REPO, OTHER_REVISION)),
    );
    for (tag, body) in [("a7", a7_body), ("a8", a8_body)] {
        let fx = Fixture::new(tag);
        let contract = fx.contract_pinning(&[pin(&body, 0.9206)], REF_REPO, REF_REVISION);
        let g = fx.golden("wrong-type.json", &body);

        let (code, stderr) = fx.preflight(&contract, &[&g]);
        assert_eq!(code, DIE_PREREQ, "{tag} must FAIL die-8; stderr:\n{stderr}");
        assert!(
            stderr.contains("correctness golden file model_type="),
            "{tag} must report the MODEL_TYPE defect (reference interleave): {stderr}"
        );
        assert!(
            !stderr.contains("does not match the pinned reference model"),
            "{tag} must NOT report the provenance mismatch first: {stderr}"
        );
    }
}

#[test]
fn a9_a_placeholder_revision_is_a_contract_defect_not_a_golden_defect() {
    // F3 — the CUDA track fixture ships `upstream_revision: "QWEN-MTP-CUDA-PENDING-ORGANIZER"`
    // today. Accepting a non-sha value as a pin would reject every provenance-bearing golden with
    // a diagnostic pointing at the GOLDEN. The defect is named where it lives, before any golden
    // is loaded — so a fixture with an unfinished field cannot masquerade as a bad submission.
    let fx = Fixture::new("a9");
    let body = golden_document_json_with_provenance(DECODE_STEPS, Some((REF_REPO, REF_REVISION)));
    let contract = fx.contract_pinning(
        &[pin(&body, 0.9206)],
        REF_REPO,
        "QWEN-MTP-CUDA-PENDING-ORGANIZER",
    );
    let g = fx.golden("right-model.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, DIE_PREREQ, "A9 must FAIL die-8; stderr:\n{stderr}");
    assert!(
        stderr.contains("is not a 40-character lowercase hex commit id"),
        "{stderr}"
    );
    // The blame is on the CONTRACT, and the golden is never named as the offender.
    assert!(
        stderr.contains("--contract reference-model pin"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("model_provenance does not match"),
        "A9 must not blame the golden: {stderr}"
    );
}

#[test]
fn a10_a_padded_contract_value_is_a_contract_defect() {
    // F4 — the reference's analogous rule (`validateGoldenModelType`, Golden.swift:783-800) is
    // that a padded value is a defect "not something to normalize away". The first draft silently
    // trim()ed contract values, i.e. enforced something the fixture's bytes did not say.
    let fx = Fixture::new("a10");
    let body = golden_document_json_with_provenance(DECODE_STEPS, Some((REF_REPO, REF_REVISION)));
    let contract =
        fx.contract_pinning(&[pin(&body, 0.9206)], REF_REPO, &format!("{REF_REVISION} "));
    let g = fx.golden("right-model.json", &body);

    let (code, stderr) = fx.preflight(&contract, &[&g]);
    assert_eq!(code, DIE_PREREQ, "A10 must FAIL die-8; stderr:\n{stderr}");
    assert!(
        stderr.contains("leading or trailing whitespace"),
        "{stderr}"
    );
}

/// F-6 — a missing `BASELINE_CALIBRATION` under enforcement (`BASELINE_BAND_ENFORCE` unset ⇒ ON)
/// fails PRE-GPU, at its call site in `execute_measure_job`.
///
/// The post-measure band check has always die-6'd this case — but only AFTER `run_measure_job`
/// opened the GPU window and both legs measured. This drives the REAL binary on the MEASURING path
/// (no `--preflight-only`) with a valid, fully-pinned pool so every earlier pre-GPU prereq passes,
/// no `BASELINE_CALIBRATION`, and enforcement at its default. The run must die-6 at the new pre-GPU
/// check, naming it, BEFORE reaching the GPU window.
///
/// REVERT-PROOF: the head dirs are unset, so deleting the pre-GPU calibration check makes the run
/// fall through to the very next refusal — the `QMTP_HEAD_DIR is unset` die-8 — instead. exit 6 +
/// the PRE-GPU message go RED (to exit 8) the moment the check is reverted, GREEN when restored.
#[test]
fn missing_calibration_under_enforcement_fails_pre_gpu() {
    const DIE_BAND: i32 = 6;
    let fx = Fixture::new("f6-precal");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract = fx.contract(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)]);
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.measuring_run(&contract, &[&ga, &gb]);
    assert_eq!(
        code, DIE_BAND,
        "missing calibration under enforcement must die-6 PRE-GPU (not fall through to the \
         head-dir die-8); stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("PRE-GPU") && stderr.contains("BASELINE_BAND_ENFORCE"),
        "the die-6 must name the pre-GPU calibration refusal; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("QMTP_HEAD_DIR is unset"),
        "the calibration check must fire BEFORE the head-dir check; stderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// ARM GATE — `official_scoring_enabled` (David ruling 2026-08-26)
// ---------------------------------------------------------------------------
//
// The track fixture's `official_scoring_enabled` was carried by every live fixture since the 3.6
// era and consulted by NOTHING: it read like a safety gate while gating nothing. These cases drive
// the REAL `benchctl` binary on the MEASURING path (no `--preflight-only`) over an otherwise
// identical, fully-pinned, passing bed, so the arm state is the ONLY thing that can move the
// verdict.
//
// The bed cannot open a GPU window (no head dirs, no calibration), so "seals normally" is proven
// the only way it can be proven offline and the strongest way it needs to be: an ARMED fixture
// falls THROUGH the arm gate into the pre-existing next pre-GPU refusal, byte-identically to how
// the same run behaved before this gate existed. That is exactly the claim the arm-time procedure
// rests on — flipping the fixture to `true` is SUFFICIENT, and changes nothing else.

/// The die code the ARMED runs fall through to: F-6's pre-GPU missing-calibration refusal, the
/// next check after the arm gate on this bed. (A refused arm state exits with the file's existing
/// [`DIE_PREREQ`] — die 8, the pre-GPU prereq/integrity class.)
const DIE_BAND_AFTER_ARM: i32 = 6;

/// (a) FALSE ⇒ an official/ranked measuring run REFUSES, die-8, naming the flag.
///
/// This is the live `fixtures/gemma4_26b_a4b_track.json` shape today (`official_scoring_enabled:
/// false`). The refusal must be the REPORTED one — not a downstream calibration/head-dir message
/// that happens to also fire on this bed — so the assertions pin both the wording and the ABSENCE
/// of the later refusals.
#[test]
fn unarmed_track_refuses_the_official_seal() {
    let fx = Fixture::new("arm-false");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract =
        fx.contract_with_arm_state(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)], Some(false));
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.measuring_run(&contract, &[&ga, &gb]);
    assert_eq!(
        code, DIE_PREREQ,
        "a scoring run over official_scoring_enabled: false must die-8 at the arm gate; \
         stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("official scoring is not enabled for this track"),
        "the refusal must carry the ruled wording; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("official_scoring_enabled"),
        "the refusal must NAME the flag; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("BASELINE_BAND_ENFORCE") && !stderr.contains("QMTP_HEAD_DIR is unset"),
        "the arm gate must fire BEFORE the calibration and head-dir refusals, so an unarmed track \
         is diagnosed as unarmed rather than as mis-configured; stderr:\n{stderr}"
    );
}

/// (b) TRUE ⇒ the run proceeds normally: the arm gate is transparent and the run falls through to
/// the pre-existing F-6 refusal, exactly as it did before this gate existed.
///
/// The ARM-TIME PROCEDURE rests on this: the only change at arm time is the engine fixture flipping
/// `false → true`. Nothing here passes a new flag, sets a new env var, or rebuilds benchd.
#[test]
fn armed_track_passes_the_arm_gate_untouched() {
    let fx = Fixture::new("arm-true");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract =
        fx.contract_with_arm_state(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)], Some(true));
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.measuring_run(&contract, &[&ga, &gb]);
    assert_eq!(
        code, DIE_BAND_AFTER_ARM,
        "an ARMED track must pass the arm gate and reach the pre-existing F-6 refusal; \
         stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("official scoring is not enabled"),
        "an armed track must not be refused by the arm gate; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("PRE-GPU") && stderr.contains("BASELINE_BAND_ENFORCE"),
        "the armed run must reach the SAME next refusal it reached before the arm gate existed; \
         stderr:\n{stderr}"
    );
}

/// (c) ABSENT ⇒ refused exactly like `false` — an absent arm state is not an armed one.
///
/// The most important half of the ruling: the flag was invisible to benchd for its entire life, so
/// "the fixture simply never mentions it" is the likeliest way a track would slip into scoring
/// unarmed. The message must additionally be the ABSENT one, so the operator is told to ADD the key
/// rather than to wait for a flip that is never coming.
#[test]
fn contract_without_the_arm_flag_refuses_fail_closed() {
    let fx = Fixture::new("arm-absent");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract = fx.contract_with_arm_state(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)], None);
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.measuring_run(&contract, &[&ga, &gb]);
    assert_eq!(
        code, DIE_PREREQ,
        "a contract that declares NO arm state must refuse a scoring run (absence != armed); \
         stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("official scoring is not enabled for this track")
            && stderr.contains("NO official_scoring_enabled"),
        "the absent case must be diagnosed as absent, not as a declared false; stderr:\n{stderr}"
    );
}

/// (d) LOCAL-DEV under a FALSE flag is UNAFFECTED — the load-bearing NEGATIVE control.
///
/// Participants and organizers must be able to drive the real paired harness against an unarmed
/// track; that is the entire purpose of the unarmed period. This is the case that proves the gate
/// refuses SCORING rather than refusing the TRACK.
///
/// It is also what keeps the gate from silently defanging itself in the other direction: the run
/// below differs from `unarmed_track_refuses_the_official_seal` by the single `--local-dev` flag
/// and by nothing else — same contract shape, same goldens, same env — so a gate that ignored the
/// scoring/local distinction would make one of the two tests red whichever way it erred.
#[test]
fn local_dev_is_unaffected_by_an_unarmed_track() {
    let fx = Fixture::new("arm-localdev");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract =
        fx.contract_with_arm_state(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)], Some(false));
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.measuring_run_mode(&contract, &[&ga, &gb], true);
    assert!(
        !stderr.contains("official scoring is not enabled"),
        "--local-dev must never be refused by the arm gate; stderr:\n{stderr}"
    );
    assert_eq!(
        code, DIE_BAND_AFTER_ARM,
        "a --local-dev run over an UNARMED track must reach the same pre-existing F-6 refusal an \
         armed scoring run reaches — i.e. the arm gate is transparent to it; stderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// MODE FENCE — contract-driven `allowed_modes` (David ruling 2026-08-26)
// ---------------------------------------------------------------------------
//
// "why the hell do we reject dflash". Because `DEFAULT_ALLOWED_MODES = [serial, mtp]` was the only
// list that existed and it was consulted at CLI-parse time, before the `--contract` fixture was
// read — so no fixture could speak for its own track. These cases drive the REAL `benchctl` binary
// over one bed that differs ONLY in whether the contract declares `allowed_modes`, so the
// declaration is the only thing that can move the verdict.
//
// The bed cannot open a GPU window (no head dirs, no calibration), so "admitted" is proven the same
// way the arm gate proves it: an ADMITTED dflash candidate falls THROUGH the mode fence into the
// bed's pre-existing next pre-GPU refusal, byte-identically to how an mtp candidate behaves.

/// (a) DECLARED ⇒ a `dflash` candidate is ADMITTED and the run proceeds to the next pre-GPU check.
#[test]
fn contract_declared_dflash_mode_is_admitted() {
    let fx = Fixture::new("modes-declared");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract = fx.contract_with_allowed_modes(
        &[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)],
        Some(&["serial", "mtp", "dflash"]),
    );
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) =
        fx.measuring_run_with_spec(&contract, &[&ga, &gb], r#"{"mode":"dflash","dflash":{}}"#);
    assert_eq!(
        code, DIE_BAND_AFTER_ARM,
        "an admitted dflash candidate must fall through to the bed's standing F-6 refusal, not be \
         stopped by the mode fence; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("allowed-modes list"),
        "the mode fence must be transparent when the contract declares the mode; stderr:\n{stderr}"
    );
}

/// (b) ABSENT ⇒ the SAME `dflash` candidate is REFUSED, die-8, pre-GPU.
///
/// THE other-track protection, and the negative control for (a): the only difference between the
/// two beds is the fixture's `allowed_modes` key. Every track whose fixture never declared a list —
/// qwen3.8, laguna — is in exactly this state and keeps exactly this behaviour.
#[test]
fn undeclared_dflash_mode_is_refused_pre_gpu() {
    let fx = Fixture::new("modes-absent");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract =
        fx.contract_with_allowed_modes(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)], None);
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) =
        fx.measuring_run_with_spec(&contract, &[&ga, &gb], r#"{"mode":"dflash","dflash":{}}"#);
    assert_eq!(
        code, DIE_PREREQ,
        "an undeclared mode must die-8 at the mode fence; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("allowed-modes list") && stderr.contains("dflash"),
        "the refusal must name the list and the mode; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("DEFAULT_ALLOWED_MODES"),
        "the refusal must say the list came from the DEFAULT, not from the fixture, so an operator \
         knows the remedy is a fixture declaration; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("BASELINE_BAND_ENFORCE") && !stderr.contains("QMTP_HEAD_DIR is unset"),
        "the mode fence must fire BEFORE the calibration and head-dir refusals; stderr:\n{stderr}"
    );
}

/// (c) An `mtp` candidate on a fixture that declares NO list is UNCHANGED — the regression control
/// for every track this lane must not touch.
#[test]
fn undeclared_contract_leaves_the_mtp_arm_unchanged() {
    let fx = Fixture::new("modes-mtp-regression");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract =
        fx.contract_with_allowed_modes(&[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)], None);
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.measuring_run_with_spec(
        &contract,
        &[&ga, &gb],
        r#"{"mode":"mtp","mtp":{"depth":2}}"#,
    );
    assert_eq!(
        code, DIE_BAND_AFTER_ARM,
        "an mtp candidate on an undeclared track must behave exactly as it did before this lane; \
         stderr:\n{stderr}"
    );
    assert!(!stderr.contains("allowed-modes list"), "stderr:\n{stderr}");
}

/// (d) A MALFORMED `allowed_modes` is refused as a FIXTURE error, before either leg is judged.
#[test]
fn malformed_allowed_modes_is_a_fixture_refusal() {
    let fx = Fixture::new("modes-malformed");
    let tape_a = tape_json(1, DECODE_STEPS);
    let tape_b = tape_json(2, DECODE_STEPS);
    let contract = fx.contract_with_allowed_modes(
        &[pin(&tape_a, 0.9206), pin(&tape_b, 0.797)],
        Some(&["mtp", "dflash"]),
    );
    let ga = fx.golden("pool-alpha.json", &tape_a);
    let gb = fx.golden("pool-beta.json", &tape_b);

    let (code, stderr) = fx.measuring_run_with_spec(
        &contract,
        &[&ga, &gb],
        r#"{"mode":"mtp","mtp":{"depth":2}}"#,
    );
    assert_eq!(code, DIE_PREREQ, "stderr:\n{stderr}");
    assert!(
        stderr.contains("does not include \"serial\""),
        "a list omitting the pinned serial baseline must be refused AT THE FIXTURE; stderr:\n\
         {stderr}"
    );
}
