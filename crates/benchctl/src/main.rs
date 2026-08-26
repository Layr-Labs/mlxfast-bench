//! benchctl — the benchmarker CLI: setup | transform | iterate | submit | official.
//!
//! Absorbs benchmark.sh and the Swift harness targets. Only writer of score artifacts.
//! Emits score.json honoring the Yukon contract (finite `score`, optional metrics).
//!
//! This wave (WS1-8) implements `iterate`: drive a live engine (spawned over
//! `ChildStdioTransport`) through the correctness gate + WS1-6 parent-side timing and
//! write a sealed `score.json` (+ `.sha256` sidecar). `transform`/`submit`/`official`
//! are stubs.

mod byte_budget;
mod coolgate;
mod correctness;
mod editable_divergence;
mod iterate;
mod measure_job;
mod official;
mod overlay;
mod parity;
mod prefill_decompose;
mod score;
/// #63: the shared golden-document builder for unit tests (test builds only).
#[cfg(test)]
mod testgolden;
mod trusted_scope;
mod weights_preflight;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bench_core::constants::{CORRECTNESS_PROMPT_TOKENS, CORRECTNESS_STEPS};
use bench_core::golden::{
    hidden_correctness_golden_pin_from_contract, load_golden_fixture,
    reference_model_pin_from_contract, verify_correctness_golden_attestation,
    verify_golden_integrity, CorrectnessGoldenPin, GoldenFixture, GoldenIntegrityPin,
    ReferenceModelPin,
};
use bench_runner::{
    resolve_official_sandbox, ChildStdioTransport, OfficialSandboxInputs, OfficialSandboxPlan,
    RunnerError, Session, SANDBOX_EXEC_PATH,
};

use crate::iterate::{dir_digest, iterate_core, HarnessIdentity, Mode, RunDigests};
use crate::score::{sha256_hex, ScorePayload};

/// The frozen Qwen3.6 internal architecture id — single-sourced in `bench_core::constants`
/// (`bench_core::constants::REQUIRED_GOLDEN_MODEL_TYPE`) so the loader and any consumer share
/// one definition. Re-exported under the module-local name to keep call sites unchanged.
use bench_core::constants::REQUIRED_GOLDEN_MODEL_TYPE;

const ITERATE_USAGE: &str = "\
benchctl iterate — run the engine end-to-end and write a sealed score.json

USAGE:
    benchctl iterate --engine <PATH> --weights <DIR> --golden <PATH> [OPTIONS]

REQUIRED:
    --engine <PATH>              Engine executable (spawned as `<engine> runtime-worker --weights <DIR>`)
    --weights <DIR>              Transformed weights directory
    --golden <PATH>              GoldenDocument JSON (loaded + validated by bench-core)

OPTIONS:
    --baseline-prefill-spt <F>   OFFICIAL ONLY. Prefill baseline seconds/token (trusted override;
    --baseline-decode-spt <F>    both required together; else the golden's paired baselines).
                                 IGNORED on local-iterate/local-submit: those legs score against
                                 the official-runner CONSTANTS, as the reference's localIterate
                                 does — the golden's declared pair is inert there too (#127).
    --mode <local-iterate|local-submit|official>
                                 Decode window: 128 (local-iterate, default), 1023 (local-submit), 128 (official)
    --score-path <OUT>           Output score path (default: score.local-iterate.json for
                                 local-iterate; score.json for local-submit/official)
    --golden-sha256 <HEX>        Integrity pin: refuse the golden unless its sha256 matches
    --golden-bytes <N>           Integrity pin: refuse the golden unless its byte count matches
                                 (both pin flags must be given together; checked before parse)
    --cool-gate                  Force the local GPU cool-down gate ON before each timed phase.
    --no-cool-gate               Force the cool-down gate OFF (overrides the per-mode default,
                                 e.g. local-submit's default-ON).
                                 local-iterate defaults OFF (this opts in); local-submit ON.
    --strict                     local-iterate/local-submit: also evaluate the golden's
                                 anchor/free-run gates (benchctl superset). Default is Swift-exact: the
                                 correctness gate checks only the primary teacher-forced
                                 cases[]. No effect on official mode.
    -h, --help                   Show this help

The native two-leg PAIRED flow (formerly `--paired`) is REPLACED by the standalone
`benchctl measure-job` subcommand (Option-A seam 2): it takes candidate/baseline
WORKSPACES and emits results.json. See `benchctl measure-job --help`.
";

const TOP_USAGE: &str = "\
benchctl — benchmarker CLI

USAGE:
    benchctl <SUBCOMMAND> [OPTIONS]

SUBCOMMANDS:
    iterate         Run the engine end-to-end and write a sealed score.json
    correctness     Run the correctness gate end-to-end; exit 0 = pass, 1 = fail
    validate-golden Integrity-pin + load-validate a golden (no engine); exit 0 if accepted
    validate-weights Preflight the transformed-weights dir + size cap (no engine); exit 0 if accepted
    parity-diff     Diff two score.json (benchctl vs swift); exit 0 = PARITY: PASS
    prefill-decompose  Fit prefill elapsed_ms = c + m*n across sizes (M-5 residual attribution)
    measure-job     Option-A seam 2: PAIRED timing over candidate/baseline workspaces → results.json
    overlay-timing  Option-A seam 3 (LOCAL): merge gates-score.json + results.json → LOCAL/parity score.json (organizer owns the ranked seal)
    harness-hash    Print the 9-root harness identity of the PROCESS CWD (read-only; the seal's own resolution)
    transform       (not implemented in this wave)
    submit          (not implemented in this wave)
    official        run the official benchmark (alias: `iterate --mode official`)
";

const MEASURE_JOB_USAGE: &str = "\
benchctl measure-job — Option-A seam 2: paired ranked TIMING over two workspaces

USAGE:
    benchctl measure-job --candidate <WS> --baseline <WS> --golden <PATH> [--golden <PATH> ...] \\
        --contract <PATH> --min-pairs <N> --target-pairs <N> --tag <S> --out <DIR> \\
        [--correctness-golden <PATH>] \\
        [--tokens 512] [--mtp-depth 2] [--weights <DIR>] [--exactness-probe once] \\
        [--prompt <PATH> --prompt-sha256 <HEX> --target-id <ID>] [--preflight-only] \\
        [--calibration-bootstrap] [--gates-producer <NAME>]

Runs the alternating serial-control / candidate PAIR LOOP and emits <out>/results.json
(+ .sha256 sidecar, bare-basename body) and <out>/benchmark-integrity.results.json. Runs NO
correctness/gates (those come from the seam-1 GATES PRODUCER — by default the reference
benchmark.sh, per ruling Q1a) and authors NO score.json (that is the overlay, seam 3). Fails
closed (exit nonzero, die 5) when fewer than --min-pairs pairs are accepted.

REQUIRED:
    --candidate <WS>   Candidate workspace (cloned on-box; the MTP spec-decode engine). Its engine
                       is resolved as <WS>/.build/release/<bin> (bin defaults to `mlxfast-runtime-worker`,
                       overridable via MLXFAST_MEASURE_WORKER_BIN); fail-closed if absent.
    --baseline <WS>    Baseline workspace (the serial-control / depth-0 engine); same resolution.
    --golden <PATH>    The timed prompt, in EITHER accepted shape, routed by required-key signature:
                         * TIMED-PROMPT TAPE (what the live timed_prompt_pool PINS): keys
                           seed_tokens / reference_seed_token / rows [+ reference_self_consistent,
                           emitted_tokens]. The legs decode from seed_tokens, oracle the seed forward
                           against reference_seed_token, then check rows[i].sequential_argmax.
                         * GoldenDocument JSON (legacy): keys version / model_type / cases /
                           correctness_gates / benchmark; the benchmark oracle drives the legs.
                       REPEATABLE — pass once per golden; a DUPLICATE DIGEST (same bytes twice) is
                       fatal (die 8). Every golden's sha256 must resolve to EXACTLY ONE --contract
                       timed_prompt_pool entry with a positive noop_decode_speedup (die 8, pre-GPU).
                       The per-golden loop is R7; this component measures the FIRST golden.
    --contract <PATH>  Track fixture (timed_prompt_pool + track_id). Also the review-gated authority
                       for the hidden correctness golden (its `hidden_correctness_golden` sha256+bytes
                       SIBLING pin, LANE 2a) — a SIBLING of timed_prompt_pool that never changes N.

OPTIONAL (LANE 2a):
    --correctness-golden <PATH>  The run's correctness-golden ATTESTATION: the staged hidden
                       correctness golden this run verified token-fidelity against. benchd hashes it
                       (sha256 + bytes) and refuses (die 8, pre-GPU) any run whose identity does not
                       CITE the --contract fixture's hidden_correctness_golden pin. FAIL-CLOSED both
                       ways: a fixture that pins the golden REQUIRES this flag; passing it against a
                       fixture that pins none is refused. Omit only on offline/legacy tracks whose
                       fixture declares no hidden_correctness_golden.

    --min-pairs <N>        Per-prompt floor (>= 1); fail closed (die 5) below it.
                           Alias: --min-pairs-per-prompt.
    --target-pairs <N>     Per-prompt target; stop accepting once reached (>= --min-pairs).
                           Alias: --pairs-per-prompt.
    --tag <S>          Per-run identity sealed into results.json (NOT the track_id).
    --out <DIR>        Output directory for results.json (+ sidecars)

OPTIONAL:
    --tokens <N>       Depth-0 decode window both legs time (default 512).
    --mtp-depth <N>    Convenience for the candidate spec {\"mode\":\"mtp\",\"mtp\":{\"depth\":N}}
                       (default 2). Depth is a MODULE field now. MUTUALLY EXCLUSIVE with
                       --candidate-spec (pass one). Bounds-checked against the readonly 32
                       draft-depth cap.
    --candidate-spec <JSON>  Explicit per-module speculative spec for the candidate leg (e.g.
                       '{\"mode\":\"mtp\",\"mtp\":{\"depth\":4}}'), recorded spec_source cli-override.
    --baseline-spec <JSON>   Explicit baseline spec (default {\"mode\":\"serial\"}). MUST be
                       mode==serial: the baseline is the serial DENOMINATOR and is not
                       CLI-steerable off serial (a non-serial baseline is a hard error).
    --weights <DIR>    OVERRIDE for the transformed weights directory. Spawned as `<engine>
                       runtime-worker --weights <DIR>` for BOTH legs (backbone/identity case).
                       The approved draft measure-job CLI carries NO --weights: weights are
                       DERIVED on-box from the env `QMTP_TARGET_DIR` (the draft's source).
                       When --weights is omitted the env is used; when NEITHER is set this fails
                       closed with a clear message.
    --gates-producer <NAME>    WHICH seam-1 gates producer the driver used (`benchmark-sh` = the
                               organizer's reference chain and the DEFAULT per ruling Q1a,
                               `facade` = benchd's own --official, `direct-swift` = the weightless
                               fallback). SEALED VERBATIM into benchmark-integrity.results.json.
                               measure-job is seam 2 and cannot observe seam 1, so the driver
                               DECLARES this and measure-job records the declaration; omitted seals
                               `undeclared`, which is the answer for a standalone run with no seam
                               1, not a gap. Any name is accepted (provenance, not a policy gate);
                               whitespace/control characters are refused because the value lands in
                               an artifact. NOTE: a DECLARATION, not an independent verification —
                               see #140.
    --exactness-probe <MODE>   none|once|per-prompt|per-pair (default once). The untimed mtp-verify
                               gate that consumes it is R15; here it is parsed + validated + stored.
    --prompt <PATH>            An explicit prompt file. ALL-THREE-OR-NONE with --prompt-sha256 and
    --prompt-sha256 <HEX>      --target-id: the sha is 64 lowercase hex, the target-id matches
    --target-id <ID>           [A-Za-z0-9._-]+, and the prompt file's sha256 MUST equal
                               --prompt-sha256 (die 8 on mismatch).
    --preflight-only           Run the pre-GPU prereq/quiesce checks then exit 0 without measuring.
    --calibration-bootstrap    Skip the BASELINE_CALIBRATION serial-band check and mark for authoring.
    --local-dev                LOCAL-DEV mode: a failed pair retries up to a budget (target-pairs x4)
                               and MLXFAST_MAX_DRAFT_DEPTH may raise the --mtp-depth cap. ABSENT
                               (default) = OFFICIAL: a failed pair is an immediate die 5 and the
                               depth cap is the readonly submission-proof constant (env ignored).
                               It is also the ONLY way to run the paired harness against a track
                               whose --contract fixture is not armed (see below).

ARM STATE:
    An OFFICIAL (non --local-dev) measuring run REFUSES (die 8, pre-GPU) unless the --contract track
    fixture declares `official_scoring_enabled: true`. `false` and ABSENT both refuse — an absent arm
    state is not an armed one. --preflight-only is NOT gated (it seals nothing). Only the track
    fixture arms a track; benchd never overrides it.

ALLOWED MODES:
    The modes a submission may declare are the --contract track fixture's `allowed_modes` when it
    declares one, and [serial, mtp] when it does not. ABSENT is not a widening: a fixture opts IN,
    so declaring `dflash` on one track cannot enable it anywhere else. A declared list must contain
    `serial` (the baseline denominator is pinned serial), must not repeat an entry, and may only
    name serial | mtp | dflash — `dspark` is reserved and refuses by name. A candidate declaring a
    mode outside the list REFUSES die 8, pre-GPU.

    `dflash` is SINGLE-STREAM ONLY (the engine's cohort driver refuses it by name), so a dflash
    candidate keeps the single-stream free-run series even on a fixture that pins
    `scored_batch_size`. The regime it ran is sealed in results.timed_mode, and the overlay's §5
    series fence keeps the single-stream and batched-cohort series from ever being pooled.

ENV (R14):
    QMTP_TARGET_DIR            Backbone/target weights dir (the --weights fallback).
    QMTP_HEAD_DIR              Pinned native MTP head (serial leg); existence-checked when set (die 8).
    QMTP_CANDIDATE_HEAD_DIR    Candidate-leg BYO head; defaults to QMTP_HEAD_DIR when unset.
    QMTP_DFLASH_HEAD_DIR       Pinned DFlash drafter (serial leg); existence-checked when set (die 8).
                               REQUIRED when the candidate declares mode dflash — without it both
                               legs would fall back to the engine's CWD-relative ./dflash-head and
                               load the SAME drafter regardless of workspace.
    QMTP_CANDIDATE_DFLASH_HEAD_DIR
                               Candidate-leg DFlash drafter; defaults to QMTP_DFLASH_HEAD_DIR.
    BASELINE_CALIBRATION       JSON calibration file; the pooled serial mean is band-checked against
                               it after measuring (die 6 on drift or decode_tokens mismatch).
    BASELINE_BAND_ENFORCE      Default 1: a MISSING calibration fails closed (die 6). Set 0 to allow.
    GPU_LOADED_UTIL            Telemetry loaded/steady util threshold (default 0.70; env-driven).

The dropped `MLXFAST_PAIRED_BASELINE_*` env / `--baseline-*` flags (the baseline is a
WORKSPACE now) are a hard mutual-exclusion error if present alongside this subcommand.
";

const OVERLAY_TIMING_USAGE: &str = "\
benchctl overlay-timing — Option-A seam 3 (LOCAL): merge gates + timing into a LOCAL/parity score.json (the organizer owns the published ranked seal)

USAGE:
    benchctl overlay-timing --gates-score <gates-score.json> --results <results.json> \\
        --score-path <score.json> [--integrity <benchmark-integrity.json>] [--contract <fixture.json>]

benchd's LOCAL merge and the verifiable seam-3 PARITY reference: it overlays the measure-job
<results.json> (seam 2) onto the seam-1 producer's sealed <gates-score.json> and seals the ranked
<score.json> (+ bare-basename .sha256). Aggregation is the 3.8 MEDIAN regime (median of the
per-prompt raw ratio-of-means; per-pair bound 8.0; floor 0.90 / ceiling 5.0). On the RANKED path
the organizer's trusted shell authors score.json (OPEN-2); this subcommand is LOCAL-only.

Flips partial_result → false, recomputes ALL floor fields coherently for the decode-only paired
track (finding 11), stamps a `scoring_mode` discriminator, and re-anchors integrity `score_sha256`
over the merged bytes (into --integrity when given, else a fresh benchmark-integrity.json).

Exit 0 when the merged score PASSES (median in [floor, ceiling]); nonzero when a floor/ceiling
bound fails (score=null); 2 on a usage error; 1 on a load/validation/IO error.

REQUIRED:
    --gates-score <PATH>  The seam-1 producer's sealed gates-score.json (partial_result=true)
    --results <PATH>      The measure-job results.json (seam 2 superset)
    --score-path <PATH>   Output LOCAL/parity score.json (+ .sha256 sidecar; organizer owns the ranked seal)

OPTIONS:
    --integrity <PATH>    An existing benchmark-integrity.json to RE-ANCHOR (score_sha256/score_path
                          rewritten over the merged bytes). Absent ⇒ a fresh sidecar next to the score.
    --contract <PATH>     R17 pool-shape source: the fixture whose `timed_prompt_pool | length` sets
                          the expected pool_size when MLXFAST_QWEN_MTP_POOL_SIZE is unset (fail-closed).
    -h, --help            Show this help
";

const VALIDATE_GOLDEN_USAGE: &str = "\
benchctl validate-golden — integrity-pin + load-validate a golden (no engine spawned)

USAGE:
    benchctl validate-golden --golden <PATH> [--golden-sha256 <HEX> --golden-bytes <N>]
                             [--contract <PATH>] [--gates-only]

Exit 0 if the golden passes the pin (when given) AND the bench-core loader accepts it;
non-zero with the rejection reason on stderr otherwise. Used by the loader-parity harness
(Rust side) and as a standalone integrity check.

--contract <PATH> supplies the TRACK CONTRACT fixture whose `target.upstream_model_id` /
`target.upstream_revision` declare the track's reference model (#114). With it, a golden
carrying a `model_provenance` block must NAME that model — the same value gate the Swift
reference applies from its compile-time constants. Without it the block is validated for
SHAPE only, and the command says so rather than implying the values were checked.

By default the golden MUST carry a benchmark oracle block (byte-consistent with Swift
preflight, which rejects a benchmark-less golden with 'benchmark golden file must contain
a benchmark oracle'). Pass --gates-only to SKIP that requirement — validating only the
structure + correctness gates — for internal fixtures that legitimately lack a benchmark
oracle.
";

const CORRECTNESS_USAGE: &str = "\
benchctl correctness — run the correctness gate end-to-end (no score.json)

USAGE:
    benchctl correctness --engine <PATH> --weights <DIR> --golden <PATH> [OPTIONS]

Spawns the engine (`<engine> runtime-worker --weights <DIR>`) and runs the FULL correctness
set — base teacher-forced cases THEN the golden's anchor / free-run gates (Swift
`runCorrectness` → `runLayeredCorrectness`, checkGates:true). A concise JSON verdict is
printed to stdout; the EXIT CODE is authoritative: 0 = pass, 1 = fail (byte-matching Swift
`mlxfast-swift correctness`, `return report.passed ? 0 : 1`).

The golden need NOT carry a benchmark oracle (Swift `checkCorrectnessArtifacts`,
requiresBenchmarkOracle:false) — correctness is oracle-optional, unlike `benchmark`/`official`.

REQUIRED:
    --engine <PATH>              Engine executable
    --weights <DIR>              Transformed weights directory
    --golden <PATH>              GoldenDocument JSON (loaded + validated by bench-core)

OPTIONS:
    --golden-sha256 <HEX>        Integrity pin (with --golden-bytes): refuse a non-matching golden
    --golden-bytes <N>           Integrity pin (both pin flags must be given together)
    -h, --help                   Show this help
";

const VALIDATE_WEIGHTS_USAGE: &str = "\
benchctl validate-weights — preflight the transformed-weights directory (no engine spawned)

USAGE:
    benchctl validate-weights --weights <DIR> [--golden <PATH>]

Mirrors the WEIGHTS half of Swift `BenchmarkPreflight`: the weights path is a real directory
(not a symlink), `config.json` and `model.safetensors.index.json` are present as regular
files, and the directory byte-count (symlinks / non-regular files REJECTED) is enforced
against the size cap. The cap is read from `MLXFAST_MAX_WEIGHTS_BYTES` (empty ⇒ 25 GiB
default; `0` / `none` / `unlimited` ⇒ uncapped; a positive integer ⇒ that cap).

Exit 0 = accepted, 1 = rejected (with the reason on stderr), 2 = usage, 3 = IO error.
`validate-golden` covers the golden half; pass `--golden` here only to also require its
presence (Swift `requiredFiles`).
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sub = match args.first() {
        Some(s) => s.as_str(),
        None => {
            eprint!("{TOP_USAGE}");
            return ExitCode::from(2);
        }
    };
    match sub {
        "iterate" => run_iterate(&args[1..]),
        // #90 item 2: the standalone correctness gate (Swift `mlxfast-swift correctness`).
        // Exit 0 = pass, 1 = fail (byte-matching Swift `report.passed ? 0 : 1`).
        "correctness" => run_correctness(&args[1..]),
        "validate-golden" => run_validate_golden(&args[1..]),
        // #90 item 3: the WEIGHTS-half preflight (Swift `BenchmarkPreflight` weights checks +
        // `MLXFAST_MAX_WEIGHTS_BYTES`). `validate-golden` covers only the golden half.
        "validate-weights" => run_validate_weights(&args[1..]),
        // §T: the parity verdict tool (Rust port of scripts/parity-diff.py). Shares the real
        // ScoreMetrics type; the bucket roster is pinned to the schema by a cargo test.
        "parity-diff" => parity::run(&args[1..]),
        // M-5 (#68): prefill-DECOMPOSITION diagnostic — fit elapsed_ms = c + m*n across
        // synthetic prompt sizes to attribute the +2.70% single-shot prefill residual
        // (RULING A3) to the protocol/spawn floor vs per-token compute. Timing-only, no
        // oracle, no score artifact — it does NOT touch the scoring/timing production path.
        "prefill-decompose" => prefill_decompose::run(&args[1..]),
        // A-1: the Option-A measure-job component (seam 2) — replaces the old `--paired`
        // monolith. Paired TIMING over candidate/baseline workspaces → results.json; no
        // gates (seam 1), no score.json (seam 3).
        "measure-job" => run_measure_job_cli(&args[1..]),
        // A-3: the Option-A overlay component (seam 3, LOCAL). Merges the seam-1 gates-score.json
        // with the measure-job results.json into a sealed ranked score.json (3.8 median regime).
        // On the RANKED path the organizer authors score.json (OPEN-2); this is the LOCAL merge +
        // the verifiable seam-3 parity reference.
        "overlay-timing" => run_overlay_timing_cli(&args[1..]),
        // David ruling 2026-08-26 — the read-only harness-identity printer. Same resolution as the
        // seal-time cross-leg gate; lets a driver/test obtain the identity without reimplementing
        // the 9-root algorithm in shell.
        "harness-hash" => run_harness_hash(),
        // Cool-gate helper: Swift `runLocalPhaseCoolGate` dispatches to
        // `<MLXFAST_LOCAL_COOL_GATE_HELPER> --local-cool-gate-only` with the phase in
        // `MLXFAST_LOCAL_COOL_GATE_PHASE`. Pointing that helper at benchctl gives the Swift
        // leg the SAME macmon gate benchctl runs natively — identical thermal semantics on
        // both sides. Exit 0 = passed/skipped (Swift's ok contract); non-zero = thermal abort.
        "--local-cool-gate-only" => run_cool_gate_only(),
        // Official runs via `iterate --mode official` (B-2): timed-first, three fresh
        // sandboxed workers, full correctness set, official gating + sealing. The bare
        // `official` subcommand points there rather than being a second entrypoint.
        "official" => {
            eprintln!(
                "benchctl official: run the official benchmark via `benchctl iterate --mode official`"
            );
            ExitCode::from(2)
        }
        "transform" | "submit" => {
            eprintln!("benchctl {sub}: not implemented in this wave");
            ExitCode::from(2)
        }
        "-h" | "--help" | "help" => {
            print!("{TOP_USAGE}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("benchctl: unknown subcommand {other:?}");
            eprint!("{TOP_USAGE}");
            ExitCode::from(2)
        }
    }
}

/// `--local-cool-gate-only`: run the local GPU cool-down gate for the phase named in
/// `MLXFAST_LOCAL_COOL_GATE_PHASE`, then exit. Mirrors benchmark.sh's `--local-cool-gate-only`
/// so the Swift harness can dispatch its `runLocalPhaseCoolGate` to benchctl.
/// `benchctl harness-hash` — print the 9-root WORKSPACE HARNESS IDENTITY of the PROCESS CWD.
///
/// A read-only diagnostic over [`HarnessIdentity::resolve_from_current_dir`] — the SAME resolution
/// `iterate` seals with and the SAME one the overlay's David-ruled cross-leg gate recomputes at the
/// seal. It exists so a shell can obtain the identity WITHOUT a second implementation of the
/// algorithm: `scripts/test-paired-offline.sh` stamps its mock gates-score with the real identity of
/// the tree its seam-3 invocation runs from, so that test exercises the real equality instead of a
/// doctored one. CWD-only by design (no `--workspace` flag): the CWD invariant is the property worth
/// being able to check from the outside.
///
/// Feeds NO enforced value and authors NO artifact — it prints a digest and exits. Fail-closed: a
/// workspace that is not a harness tree prints the missing root to stderr and exits 1.
fn run_harness_hash() -> ExitCode {
    match HarnessIdentity::resolve_from_current_dir() {
        Ok(identity) => {
            println!("{}", identity.as_str());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "benchctl harness-hash: {e}; run it with the working directory at the engine \
                 workspace root"
            );
            ExitCode::from(1)
        }
    }
}

fn run_cool_gate_only() -> ExitCode {
    let phase =
        std::env::var("MLXFAST_LOCAL_COOL_GATE_PHASE").unwrap_or_else(|_| "local".to_string());
    match coolgate::cool_gate(&phase) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("benchctl --local-cool-gate-only: {e}");
            ExitCode::from(1)
        }
    }
}

/// R6/R14: the env the draft passes the transformed-weights dir through on-box (draft@064c0ff2:2084).
/// `--weights` is an OPTIONAL OVERRIDE of this; when neither is set, measure-job fails closed. R14 —
/// RENAMED from the dead `QWEN_MTP_TARGET_DIR` to `QMTP_TARGET_DIR` (live wrapper W:370).
const WEIGHTS_ENV_VAR: &str = "QMTP_TARGET_DIR";

/// Parsed `measure-job` flags (A-1, seam 2).
#[derive(Debug)]
struct MeasureJobArgs {
    candidate: PathBuf,
    baseline: PathBuf,
    weights: PathBuf,
    /// R13 — `--golden` is REPEATABLE → a Vec (non-empty). R7 — the pair loop measures EVERY
    /// golden in the pool (one per_prompt record per golden), with the dup-digest guard up front.
    goldens: Vec<PathBuf>,
    contract: PathBuf,
    /// LANE 2a — `--correctness-golden <path>`: the run's correctness-golden ATTESTATION. The staged
    /// hidden correctness golden the run verified token-fidelity against; benchd hashes it (sha256 +
    /// bytes) and verifies that identity CITES the `--contract` fixture's `hidden_correctness_golden`
    /// SIBLING pin (fail-closed both directions). Absent on offline/legacy tracks whose fixture pins
    /// no correctness golden; REQUIRED once the fixture declares one.
    correctness_golden: Option<PathBuf>,
    /// R13/W3 — `--tokens`. Default 512 (`DEFAULT_TOKENS`) on the teacher-forced path;
    /// `FREE_RUN_DECODE_TOKENS` (128, RULED) on the v1.1 free-run path, where any other explicit
    /// value is a usage error.
    tokens: usize,
    /// W3 — the candidate leg's TIMED REGIME, derived from `candidate_spec` by
    /// `measure_job::candidate_regime_for_spec` (the single production rule). The serial control leg
    /// is always teacher-forced.
    candidate_regime: measure_job::LegRegime,
    /// R13 — `--mtp-depth` (replaces `--depth`): candidate MTP depth, default 2. Derived from
    /// `candidate_spec.mtp.depth`; sealed as the `mtp_depth` mirror (0 for a non-mtp candidate).
    mtp_depth: usize,
    /// spec (docs/spec-config-design.md) — the resolved candidate/baseline declared specs + sources.
    candidate_spec: bench_protocol::SpecConfig,
    baseline_spec: bench_protocol::SpecConfig,
    candidate_spec_source: String,
    baseline_spec_source: String,
    /// R13 — `--min-pairs` (alias `--min-pairs-per-prompt`): PER-PROMPT floor, >= 1.
    min_pairs: usize,
    /// R13 — `--target-pairs` (alias `--pairs-per-prompt`): PER-PROMPT target, >= min.
    target_pairs: usize,
    tag: String,
    out: PathBuf,
    /// R13 — the `--prompt`/`--prompt-sha256`/`--target-id` trio (all-three-or-none). Parsed +
    /// validated + RECOGNISED here; the sealed `evaluation_target` shape is R16 (DEFERRED).
    prompt: Option<PathBuf>,
    prompt_sha256: Option<String>,
    target_id: Option<String>,
    /// R13 — `--exactness-probe` mode (default `once`), STORED. The untimed `mtp-verify` gate that
    /// consumes it is R15 (DEFERRED — parse + validate + store only).
    exactness_probe: measure_job::ExactnessProbe,
    /// R13 — `--preflight-only`: run the pre-GPU prereq/quiesce checks then exit 0 without measuring.
    preflight_only: bool,
    /// R13 — `--calibration-bootstrap`: skip the R14 serial-band check + mark the run for authoring.
    calibration_bootstrap: bool,
    /// H6/H3 (cycle-3) — `--local-dev`: enable the pair-attempt budget loop + honor the
    /// `MLXFAST_MAX_DRAFT_DEPTH` cap override. Absent (default) = OFFICIAL: immediate die-5 on a
    /// failed pair, readonly submission-proof depth cap.
    local_dev: bool,
    /// Ruling Q1a — `--gates-producer`: WHICH seam-1 gates producer the driver actually used, for
    /// the seal. measure-job is seam 2 and does not run the gates, so it cannot observe this; the
    /// driver DECLARES it and measure-job records the declaration verbatim.
    ///
    /// Absent = [`GATES_PRODUCER_UNDECLARED`], which is the ANSWER, not a gap: a standalone
    /// measure-job (no driver, no seam 1) genuinely has no producer, and #132/F3's lesson is that
    /// an empty string must never be ambiguous.
    gates_producer: String,
}

/// A-1: the Option-A MEASURE-JOB subcommand (seam 2). Parses the workspace CLI, runs the
/// alternating pair loop, and seals `<out>/results.json` (+ bare-basename `.sha256` and the
/// `benchmark-integrity.results.json` anchor). Exit 0 when the candidate is accepted
/// (`accepted >= --min-pairs`), else exit 5 (die 5 — candidate rejected); 2 on a usage error.
fn run_measure_job_cli(args: &[String]) -> ExitCode {
    let parsed = match parse_measure_job_args(args) {
        Ok(Some(p)) => p,
        Ok(None) => {
            print!("{MEASURE_JOB_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("benchctl measure-job: {msg}");
            eprint!("{MEASURE_JOB_USAGE}");
            return ExitCode::from(2);
        }
    };
    match execute_measure_job(&parsed) {
        // The verdict→exit mapping is extracted so it is unit-testable (a real ExitCode end-to-end
        // needs a live pair loop): accepted / preflight-ok → 0, die-5 (candidate rejected) → 5.
        // Finding R19 — a thermal-gate timeout is NOT a distinct exit; it folds into die-5 like
        // every reject class. A [`MeasureJobFailure`] carries its OWN honest exit (die-8 golden/sha
        // prereq, die-6 calibration, exit-1 IO/load), never collapsed to a single error code.
        Ok(verdict) => ExitCode::from(measure_job_exit_status(verdict)),
        Err(f) => {
            eprintln!("benchctl measure-job: {}", f.message);
            ExitCode::from(f.exit)
        }
    }
}

/// A measure-job execution failure that carries its OWN honest process exit code (R13/R19 exit
/// table): die-8 (prereq / golden / sha), die-6 (baseline/calibration drift — R14), or the generic
/// exit-1 load/IO path. A plain `String` error auto-converts to exit-1 via `From`, so the many
/// `?`-propagated IO errors keep the exit-1 path while the honest hard-die sites construct explicitly.
#[derive(Debug)]
struct MeasureJobFailure {
    exit: u8,
    message: String,
}

impl MeasureJobFailure {
    /// die-8 — a PRE-GPU prereq failure (golden dup-digest, `--prompt` file hash ≠ `--prompt-sha256`,
    /// a missing/unresolvable prerequisite, a missing head dir). R19 exit table: `8 prereq/golden/sha`.
    fn die8(message: impl Into<String>) -> Self {
        Self {
            exit: 8,
            message: message.into(),
        }
    }

    /// die-6 — a baseline/calibration failure caught BEFORE measuring (a malformed `BASELINE_CALIBRATION`
    /// file). The POST-measure serial-band drift is the [`MeasureJobVerdict::CalibrationDrift`] verdict
    /// (results.json sealed), also exit 6. R19 exit table: `6 baseline/calibration drift`.
    fn die6(message: impl Into<String>) -> Self {
        Self {
            exit: 6,
            message: message.into(),
        }
    }
}

impl From<String> for MeasureJobFailure {
    /// A bare error string is the generic exit-1 load/IO path (unchanged behaviour).
    fn from(message: String) -> Self {
        Self { exit: 1, message }
    }
}

/// The terminal verdicts of a measure-job run. Finding R19 (reverts R8) — a thermal-gate timeout no
/// longer produces a distinct exit-2 hard die: EVERY reject class (thermal, parity, implausible,
/// row-accounting, spawn/protocol infra) is retried once and folds into die-5 on persistence, so
/// the only rejection verdict is `RejectedDie5`. The exit `2` is reserved for a genuine usage/parse
/// error caught PRE-execution in `run_measure_job_cli`, never a mid-pair thermal event.
///
/// Honest exit table (measure-job): 0 accepted · 5 candidate rejected (pair fail incl.
/// thermal-after-retry, floor, accept-count) · 2 usage/parse error (pre-execution). (Die 6
/// calibration, die 8 prereq/golden/sha, die 9 lock, etc. are OTHER findings — hooks not
/// implemented here.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasureJobVerdict {
    /// `accepted_pairs >= --min-pairs`.
    Accepted,
    /// R13 — `--preflight-only`: the pre-GPU prereq/quiesce checks all passed and the run EXITED
    /// WITHOUT measuring (exit 0). Distinct from `Accepted` (no pair loop ran).
    PreflightOk,
    /// Fewer than `--min-pairs` accepted (die 5) — the candidate is rejected. Finding R19 — this is
    /// the ONLY candidate-rejection verdict; a persistent thermal-gate timeout folds into it (not exit-2).
    RejectedDie5,
    /// R14 — the POOLED serial mean drifted outside the `BASELINE_CALIBRATION` band (or a required
    /// calibration was missing under `BASELINE_BAND_ENFORCE`) after measuring: die-6. results.json is
    /// sealed (the calibration provenance records what was checked); the process exits 6.
    CalibrationDrift,
}

/// The verdict→process-exit contract for a measure-job run: an accepted candidate (or a passing
/// preflight-only run) exits 0; a die-5 rejected candidate exits 5 (finding R19 — the sole
/// candidate-rejection exit, thermal-after-retry included). Distinct from the exit-1 load/IO error
/// path, the exit-2 usage/parse path, and the die-8 prereq / die-6 calibration paths ([`MeasureJobFailure`]).
fn measure_job_exit_status(verdict: MeasureJobVerdict) -> u8 {
    match verdict {
        MeasureJobVerdict::Accepted => 0,
        MeasureJobVerdict::PreflightOk => 0,
        MeasureJobVerdict::RejectedDie5 => 5,
        MeasureJobVerdict::CalibrationDrift => 6,
    }
}

fn parse_measure_job_args(args: &[String]) -> Result<Option<MeasureJobArgs>, String> {
    let mut candidate: Option<PathBuf> = None;
    let mut baseline: Option<PathBuf> = None;
    let mut weights: Option<PathBuf> = None;
    // R13 — `--golden` is REPEATABLE: accumulate into a Vec (dup-digest guard runs pre-GPU in execute).
    let mut goldens: Vec<PathBuf> = Vec::new();
    let mut contract: Option<PathBuf> = None;
    // LANE 2a — `--correctness-golden`: the run's correctness-golden attestation (NOT repeatable).
    let mut correctness_golden: Option<PathBuf> = None;
    let mut tokens: Option<usize> = None;
    let mut mtp_depth: Option<usize> = None;
    let mut candidate_spec_json: Option<String> = None;
    let mut baseline_spec_json: Option<String> = None;
    let mut min_pairs: Option<usize> = None;
    let mut target_pairs: Option<usize> = None;
    let mut tag: Option<String> = None;
    let mut gates_producer: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    // R13 — recognised-and-validated flags.
    let mut prompt: Option<PathBuf> = None;
    let mut prompt_sha256: Option<String> = None;
    let mut target_id: Option<String> = None;
    let mut exactness_probe: Option<measure_job::ExactnessProbe> = None;
    let mut preflight_only = false;
    let mut calibration_bootstrap = false;
    let mut local_dev = false;

    fn value<'a>(args: &'a [String], i: usize, name: &str) -> Result<&'a str, String> {
        args.get(i + 1)
            .map(|s| s.as_str())
            .ok_or_else(|| format!("flag {name} requires a value"))
    }
    fn usize_val(args: &[String], i: usize, name: &str) -> Result<usize, String> {
        let v = value(args, i, name)?;
        v.parse::<usize>()
            .map_err(|_| format!("invalid usize for {name}: {v:?}"))
    }

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(None),
            "--candidate" => {
                candidate = Some(PathBuf::from(value(args, i, "--candidate")?));
                i += 2;
            }
            "--baseline" => {
                baseline = Some(PathBuf::from(value(args, i, "--baseline")?));
                i += 2;
            }
            "--weights" => {
                weights = Some(PathBuf::from(value(args, i, "--weights")?));
                i += 2;
            }
            // R13 — REPEATABLE: each `--golden` appends to the set (the per-golden loop is R7).
            "--golden" => {
                goldens.push(PathBuf::from(value(args, i, "--golden")?));
                i += 2;
            }
            "--contract" => {
                contract = Some(PathBuf::from(value(args, i, "--contract")?));
                i += 2;
            }
            // LANE 2a — the run's correctness-golden attestation (single, not repeatable like
            // `--golden`): the staged hidden correctness golden benchd hashes and verifies cites the
            // fixture's `hidden_correctness_golden` pin.
            "--correctness-golden" => {
                correctness_golden = Some(PathBuf::from(value(args, i, "--correctness-golden")?));
                i += 2;
            }
            "--tokens" => {
                tokens = Some(usize_val(args, i, "--tokens")?);
                i += 2;
            }
            // R13 — `--mtp-depth` replaces `--depth`. Since the spec re-home
            // (docs/spec-config-design.md) depth is a MODULE field: `--mtp-depth D` is a convenience
            // that builds the default candidate spec `{"mode":"mtp","mtp":{"depth":D}}`; it is no
            // longer a scored knob (the spec is), and a `--candidate-spec` override supersedes it.
            "--mtp-depth" => {
                mtp_depth = Some(usize_val(args, i, "--mtp-depth")?);
                i += 2;
            }
            // spec (docs/spec-config-design.md, step 5) — explicit per-module spec overrides, recorded
            // as spec_source "cli-override". `--candidate-spec` supersedes `--mtp-depth`; the baseline
            // defaults to {"mode":"serial"} and `--baseline-spec` overrides it.
            "--candidate-spec" => {
                candidate_spec_json = Some(value(args, i, "--candidate-spec")?.to_string());
                i += 2;
            }
            "--baseline-spec" => {
                baseline_spec_json = Some(value(args, i, "--baseline-spec")?.to_string());
                i += 2;
            }
            // R13 — `--depth` was RENAMED to `--mtp-depth`; a helpful hard error, not a silent accept.
            "--depth" => {
                return Err(
                    "--depth was renamed to --mtp-depth (candidate MTP depth, >= 2; serial \
                     control is the depth-0 constant)"
                        .to_string(),
                );
            }
            // R13 — `--min-pairs` and its per-prompt alias set the SAME per-prompt floor.
            "--min-pairs" | "--min-pairs-per-prompt" => {
                min_pairs = Some(usize_val(args, i, args[i].as_str())?);
                i += 2;
            }
            // R13 — `--target-pairs` and its per-prompt alias set the SAME per-prompt target.
            "--target-pairs" | "--pairs-per-prompt" => {
                target_pairs = Some(usize_val(args, i, args[i].as_str())?);
                i += 2;
            }
            "--tag" => {
                tag = Some(value(args, i, "--tag")?.to_string());
                i += 2;
            }
            // Ruling Q1a — the seam-1 producer the DRIVER used, recorded into the integrity seal.
            "--gates-producer" => {
                gates_producer = Some(validate_gates_producer(value(
                    args,
                    i,
                    "--gates-producer",
                )?)?);
                i += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(value(args, i, "--out")?));
                i += 2;
            }
            // R13 — the `--prompt`/`--prompt-sha256`/`--target-id` trio (all-three-or-none, validated below).
            "--prompt" => {
                prompt = Some(PathBuf::from(value(args, i, "--prompt")?));
                i += 2;
            }
            "--prompt-sha256" => {
                prompt_sha256 = Some(value(args, i, "--prompt-sha256")?.to_string());
                i += 2;
            }
            "--target-id" => {
                target_id = Some(value(args, i, "--target-id")?.to_string());
                i += 2;
            }
            // R13 — `--exactness-probe {none|once|per-prompt|per-pair}` (parse + validate; store).
            "--exactness-probe" => {
                exactness_probe = Some(measure_job::ExactnessProbe::parse(value(
                    args,
                    i,
                    "--exactness-probe",
                )?)?);
                i += 2;
            }
            // R13 — boolean flags (no value).
            "--preflight-only" => {
                preflight_only = true;
                i += 1;
            }
            "--calibration-bootstrap" => {
                calibration_bootstrap = true;
                i += 1;
            }
            // H6/H3 (cycle-3) — LOCAL-DEV mode: enables the pair-attempt BUDGET LOOP (up to
            // target_pairs × 4) and honors the `MLXFAST_MAX_DRAFT_DEPTH` env override of the depth
            // cap. ABSENT (the default) = OFFICIAL/ranked: a failed pair is an immediate die-5 and
            // the depth cap is the readonly submission-proof constant (env ignored).
            "--local-dev" => {
                local_dev = true;
                i += 1;
            }
            // finding 3: the dropped paired-baseline flags are a HARD mutual-exclusion error
            // (the baseline is a WORKSPACE now), not a silently-ignored flag.
            "--baseline-prefill-spt"
            | "--baseline-decode-spt"
            | "--baseline-engine"
            | "--engine" => {
                return Err(format!(
                    "{} is not a measure-job flag: the baseline is a WORKSPACE (--baseline <WS>) now, \
                     not an engine/seconds-per-token override",
                    args[i]
                ));
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }

    // finding 3: the trusted paired-baseline ENV override must not coexist with measure-job
    // either — the baseline is a workspace, so a present env is an operator wiring error.
    for key in [
        "MLXFAST_PAIRED_BASELINE_PREFILL_SECONDS_PER_TOKEN",
        "MLXFAST_PAIRED_BASELINE_DECODE_SECONDS_PER_TOKEN",
    ] {
        if std::env::var(key)
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
        {
            return Err(format!(
                "{key} is set, but measure-job takes the baseline as a WORKSPACE (--baseline <WS>); \
                 unset the paired-baseline env override (mutually exclusive)"
            ));
        }
    }

    let candidate = candidate.ok_or("missing required --candidate")?;
    let baseline = baseline.ok_or("missing required --baseline")?;
    // R6: the approved draft measure-job CLI (draft@064c0ff2:2088-2098) carries NO --weights; the
    // draft passes the weights dir on-box as env QMTP_TARGET_DIR (R14 rename). So --weights is an
    // OPTIONAL OVERRIDE: when omitted we DERIVE the weights dir from QMTP_TARGET_DIR; when
    // NEITHER is provided we fail closed with a clear message rather than guessing a path.
    // UNVERIFIED(measure-job): QMTP_TARGET_DIR is the draft's on-box weights source.
    let weights = match weights {
        Some(w) => w,
        None => {
            let env_dir = std::env::var(WEIGHTS_ENV_VAR)
                .ok()
                .filter(|s| !s.trim().is_empty());
            match env_dir {
                Some(dir) => PathBuf::from(dir.trim()),
                None => {
                    return Err(format!(
                        "no weights directory: pass --weights <DIR>, or set the env \
                         {WEIGHTS_ENV_VAR} (the draft's on-box weights source) — neither is set"
                    ));
                }
            }
        }
    };
    if goldens.is_empty() {
        return Err("missing required --golden (repeatable)".to_string());
    }
    let contract = contract.ok_or("missing required --contract")?;
    // W3 — `--tokens` stays an Option here: its DEFAULT is regime-dependent (teacher-forced 512 =
    // the live wrapper window; v1.1 free-run 128 = the RULED N), so it is resolved AFTER the
    // candidate spec (and therefore the regime) is known, below.
    let tokens_flag = tokens;
    // Medium (#105) — `--mtp-depth` is a CONVENIENCE that builds the default candidate spec; a
    // `--candidate-spec` override SUPERSEDES it. Silently discarding an explicit `--mtp-depth` when
    // `--candidate-spec` is also given hid an operator wiring conflict, so it is now an EXPLICIT hard
    // error (mutually exclusive) rather than a quiet no-op.
    if candidate_spec_json.is_some() && mtp_depth.is_some() {
        return Err(
            "--mtp-depth and --candidate-spec are mutually exclusive: --candidate-spec is the \
             explicit spec, and --mtp-depth is only the convenience that builds the default one — \
             pass exactly one (drop --mtp-depth, or fold the depth into --candidate-spec)"
                .to_string(),
        );
    }
    // #105 cycle-5 finding 5 — remember whether the operator ACTUALLY passed `--mtp-depth`, so the
    // sealed spec_source can distinguish a declared depth from benchd's built-in default.
    let mtp_depth_flag_given = mtp_depth.is_some();
    let mtp_depth = mtp_depth.unwrap_or(measure_job::DEFAULT_MTP_DEPTH);
    let min_pairs = min_pairs.ok_or("missing required --min-pairs")?;
    let target_pairs = target_pairs.ok_or("missing required --target-pairs")?;
    let tag = tag.ok_or("missing required --tag")?;
    let out = out.ok_or("missing required --out")?;
    // R13 — candidate --mtp-depth must be >= 2 (depth 0 = serial control, depth 1 = diagnostic).
    // David ruling (cycle-3) — plus a DEFENSIVE upper CAP (> cap rejected before GPU work),
    // SUBMISSION-PROOF like the engine: OFFICIAL (default) uses the readonly constant 32 and IGNORES
    // MLXFAST_MAX_DRAFT_DEPTH; --local-dev honors the env override.
    let max_draft_depth_cap = measure_job::resolve_max_draft_depth_cap(
        local_dev,
        std::env::var(measure_job::MAX_DRAFT_DEPTH_ENV)
            .ok()
            .as_deref(),
    );

    // spec (docs/spec-config-design.md, steps 4/5) — resolve the per-leg declared specs.
    // Candidate: a `--candidate-spec` JSON override (spec_source "cli-override"), else the default
    // built from `--mtp-depth` (`{"mode":"mtp","mtp":{"depth":D}}`). #105 H-C — that default's honest
    // source is the convenience flag ("mtp-depth-flag" when it was passed, "mtp-depth-default" when
    // benchd's DEFAULT_MTP_DEPTH supplied it — cycle-5 finding 5), NOT the FALSE "contract-default"
    // (no contract speculative-block parsing exists). Baseline: a `--baseline-spec` override, else
    // `{"mode":"serial"}` (spec_source "serial-default").
    let (candidate_spec, candidate_spec_source) = match &candidate_spec_json {
        Some(json) => (
            measure_job::parse_spec_override(json)?,
            measure_job::SPEC_SOURCE_CLI_OVERRIDE.to_string(),
        ),
        None => (
            // Medium (#105) — u32 TRUNCATION GUARD: `mtp.depth` is a u32 module field, so a usize
            // `--mtp-depth` that does not fit u32 must ERROR, never wrap silently to a small value
            // that would sneak under the depth cap. (A cast `as u32` truncates; try_from does not.)
            bench_protocol::SpecConfig::mtp(u32::try_from(mtp_depth).map_err(|_| {
                format!(
                    "--mtp-depth {mtp_depth} does not fit a u32 (mtp.depth is a u32 module field); \
                     a plausible depth is a small integer bounded by the {} draft-depth cap",
                    measure_job::DEFAULT_MAX_DRAFT_DEPTH_CAP
                )
            })?),
            // #105 cycle-5 finding 5 — HONEST source: `--mtp-depth` given → the flag built it;
            // omitted → benchd's DEFAULT_MTP_DEPTH built it. The old code sealed "mtp-depth-flag"
            // on both, naming a flag the operator never passed on the default path.
            if mtp_depth_flag_given {
                measure_job::SPEC_SOURCE_MTP_DEPTH_FLAG.to_string()
            } else {
                measure_job::SPEC_SOURCE_MTP_DEPTH_DEFAULT.to_string()
            },
        ),
    };
    let (baseline_spec, baseline_spec_source) = match &baseline_spec_json {
        Some(json) => (
            measure_job::parse_spec_override(json)?,
            measure_job::SPEC_SOURCE_CLI_OVERRIDE.to_string(),
        ),
        None => (
            bench_protocol::SpecConfig::serial(),
            measure_job::SPEC_SOURCE_SERIAL_DEFAULT.to_string(),
        ),
    };
    // #105 H-B — the BASELINE is the SERIAL DENOMINATOR; it must NOT be CLI-steerable off serial.
    // A non-serial `--baseline-spec` is a HARD ERROR (pre-GPU): the serial control anchors the whole
    // ratio (serial = 1.0), so a caller cannot swap the denominator for a faster/slower regime and
    // inflate the speedup. `--candidate-spec` stays free; only the baseline is pinned.
    measure_job::validate_baseline_is_serial(&baseline_spec)?;
    // Depth-0-via-serial-mode: candidate validation keys on the MODE being in the track's allowed
    // list (not a depth-int floor). The 32 cap is re-homed as a bounds-check on the module's
    // `mtp.depth`. Both are pre-GPU usage errors.
    //
    // David ruling (2026-08-26) — the ALLOWED-MODES half of that check has MOVED to
    // `execute_measure_job`, immediately after `Contract::parse`. It had to: the list is now
    // CONTRACT DATA (`Contract::allowed_modes`) and the contract file is not read here, which is
    // exactly why the override `DEFAULT_ALLOWED_MODES` had always advertised never existed. What
    // stays here is the half that is CONTRACT-INDEPENDENT and therefore still a pure usage error:
    // the mode↔module COHERENCE shape (`{"mode":"mtp"}` with no mtp block, an `mtp(0)` candidate, a
    // cross-module `{"mode":"mtp","dflash":{…}}`) — malformed on every track, whatever any fixture
    // declares. The allowed-list refusal becomes a die-8 pre-GPU prereq instead of an exit-2 usage
    // error, which is the honest classification: "this track does not admit this mode" is a fact
    // about the track fixture, not about how the operator typed the command.
    measure_job::validate_spec_module_coherent(&candidate_spec)?;
    measure_job::validate_spec_module_coherent(&baseline_spec)?;
    measure_job::validate_spec_capped(&candidate_spec, max_draft_depth_cap)?;
    measure_job::validate_spec_capped(&baseline_spec, max_draft_depth_cap)?;
    // Keep the sealed `mtp_depth` mirror consistent with the candidate spec's module depth (serial /
    // non-mtp candidates seal depth 0, matching the serial-control constant vocabulary).
    let mtp_depth = candidate_spec.mtp.map(|m| m.depth as usize).unwrap_or(0);

    // W3 — the CANDIDATE LEG'S TIMED REGIME, derived from the declared candidate spec by the single
    // production rule (`candidate_regime_for_spec`): a speculating candidate (mtp today, dflash when
    // it lands) is scored in the v1.1 FREE-RUN regime, because teacher forcing structurally cannot
    // execute speculation. A serial candidate stays teacher-forced. There is deliberately NO
    // separate `--free-run` flag: a second switch could drift from the declared spec.
    let candidate_regime = measure_job::candidate_regime_for_spec(&candidate_spec);
    // W3 — the decode window N. Its DEFAULT is regime-dependent, and on the free-run path an
    // EXPLICIT `--tokens` that is not the RULED N is a hard usage error rather than a silent
    // re-window: N divides the scored seconds-per-token, and the v1.1 series is defined at
    // N = BENCHMARK_DECODE_STEPS (PROTOCOL-v1.1 OQ3, RULED).
    // A zero window is invalid in EVERY regime, and its message must not be shadowed by the
    // regime-specific one below (an operator who typed `--tokens 0` needs to be told that, not told
    // about N).
    if tokens_flag == Some(0) {
        return Err("--tokens must be > 0 (a zero decode window is invalid)".to_string());
    }
    let tokens = match (candidate_regime.is_free_run(), tokens_flag) {
        (true, None) => measure_job::FREE_RUN_DECODE_TOKENS,
        (true, Some(t)) if t != measure_job::FREE_RUN_DECODE_TOKENS => {
            return Err(format!(
                "--tokens {t} is invalid for the v1.1 free-run series: PROTOCOL-v1.1 RULES N = {} \
                 (BENCHMARK_DECODE_STEPS). A speculating --candidate-spec/--mtp-depth selects the \
                 free-run regime, whose window is fixed; drop --tokens, or pass a serial \
                 --candidate-spec to measure the teacher-forced series at your own window.",
                measure_job::FREE_RUN_DECODE_TOKENS
            ));
        }
        (true, Some(t)) => t,
        // R13 — teacher-forced default 512 (the live wrapper window).
        (false, other) => other.unwrap_or(measure_job::DEFAULT_TOKENS),
    };
    if tokens == 0 {
        return Err("--tokens must be > 0 (a zero decode window is invalid)".to_string());
    }
    if min_pairs == 0 {
        return Err("--min-pairs (per prompt) must be >= 1".to_string());
    }
    if target_pairs < min_pairs {
        return Err(format!(
            "--target-pairs ({target_pairs}) must be >= --min-pairs ({min_pairs}) [per prompt]"
        ));
    }

    // R13 — the `--prompt`/`--prompt-sha256`/`--target-id` trio is ALL-THREE-OR-NONE; when present,
    // the sha is 64-lowercase-hex and the target-id matches [A-Za-z0-9._-]+ (the file-hash ==
    // --prompt-sha256 check is done in execute, where the file bytes are read → die-8 on mismatch).
    let trio_present = [
        prompt.is_some(),
        prompt_sha256.is_some(),
        target_id.is_some(),
    ];
    let present_count = trio_present.iter().filter(|p| **p).count();
    if present_count != 0 && present_count != 3 {
        return Err(
            "--prompt, --prompt-sha256 and --target-id are ALL-THREE-OR-NONE (an explicit prompt \
             must carry its pinned sha256 and target-id)"
                .to_string(),
        );
    }
    if let Some(s) = prompt_sha256.as_deref() {
        measure_job::validate_prompt_sha256(s)?;
    }
    if let Some(t) = target_id.as_deref() {
        measure_job::validate_target_id(t)?;
    }

    Ok(Some(MeasureJobArgs {
        candidate,
        baseline,
        weights,
        goldens,
        contract,
        correctness_golden,
        tokens,
        candidate_regime,
        mtp_depth,
        candidate_spec,
        baseline_spec,
        candidate_spec_source,
        baseline_spec_source,
        min_pairs,
        target_pairs,
        tag,
        out,
        prompt,
        prompt_sha256,
        target_id,
        exactness_probe: exactness_probe.unwrap_or_default(),
        preflight_only,
        calibration_bootstrap,
        local_dev,
        gates_producer: gates_producer.unwrap_or_else(|| GATES_PRODUCER_UNDECLARED.to_string()),
    }))
}

/// R14/H6 — resolve the `BASELINE_CALIBRATION` file into a [`ResolvedCalibration`] (for the pre-
/// measure ceiling + the AFTER-measure serial-band die-6), FAIL-CLOSED on a malformed file or a
/// declared-but-missing `--target-id` entry. H6/H2 (cycle-3): under `--calibration-bootstrap` this
/// run AUTHORS the band, so it does NOT pre-read or require the file — a MISSING file is fine
/// (returns `None`), mirroring the wrapper's bootstrap early-return (W:1423-1426). Pure over its
/// inputs (path string + target-id) so the bootstrap-skips-missing-file behavior is unit-testable.
fn resolve_calibration_env(
    calibration_bootstrap: bool,
    calibration_path: Option<&str>,
    target_id: Option<&str>,
    run_timed_mode: &str,
    run_track_id: &str,
) -> Result<Option<measure_job::ResolvedCalibration>, MeasureJobFailure> {
    if calibration_bootstrap {
        // Authoring run: skip the pre-read entirely. write_calibration_bootstrap reads/merges the
        // file itself after an accepted+parity run; a missing file here is expected, not a die-6.
        return Ok(None);
    }
    let path = match calibration_path.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p,
        None => return Ok(None),
    };
    let bytes = std::fs::read(path).map_err(|e| {
        MeasureJobFailure::die6(format!("BASELINE_CALIBRATION read failed ({path}): {e}"))
    })?;
    let parsed =
        measure_job::BaselineCalibration::parse(&bytes).map_err(MeasureJobFailure::die6)?;
    // #105 cycle-5 (HIGH) — the SERIES FENCE runs FIRST, before the file is resolved to a band and
    // long before the pooled serial mean is banded against it: a calibration measured in another
    // series (or authored for another track) must never reach `evaluate_serial_band` at all. die-6.
    measure_job::enforce_calibration_series_fence(&parsed, run_timed_mode, run_track_id)
        .map_err(MeasureJobFailure::die6)?;
    // FAIL-CLOSED: a declared --target-id with no matching per-target entry is a miswired rotation
    // (die-6), never a fallback to the top-level baseline.
    Ok(Some(
        parsed.resolve(target_id).map_err(MeasureJobFailure::die6)?,
    ))
}

/// Read the dispatch sha record (`candidate.sha`) the in-repo dispatch script authored, if any.
/// The path is threaded via `MLXFAST_CANDIDATE_SHA_FILE`, which `official-paired.sh`'s seam-2
/// invocation sets when it records the dispatched sha (`run-paired-window.sh` reaches this only by
/// invoking `official-paired.sh` — it sets nothing itself). The dispatched sha itself comes from
/// the CI/yukon dispatch context (`MLXFAST_CANDIDATE_SHA`/`GITHUB_SHA`); wiring an outer dispatch to
/// EXPORT that context on the live scoring box is a separate dispatch-lane item, so on a scoring run
/// where the env is unset the seal fails closed in [`official::author_sealed_commit`] rather than
/// falling back to git. Presence of the env var means "a dispatch promised a record", so a
/// SET-but-unreadable path is a die-8 refuse. Unset ⇒ `None`. The trimmed contents are validated by
/// [`official::author_sealed_commit`], not here (that is the single seal authority).
fn read_dispatch_sha_record() -> Result<Option<String>, MeasureJobFailure> {
    let path = match std::env::var("MLXFAST_CANDIDATE_SHA_FILE") {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return Ok(None),
    };
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        MeasureJobFailure::die8(format!(
            "author-at-seal: MLXFAST_CANDIDATE_SHA_FILE={path} is unreadable ({e}); refusing to \
             seal against an unidentified dispatch"
        ))
    })?;
    Ok(Some(raw.trim().to_string()))
}

/// Execute the measure-job: load inputs, run the pair loop over sandboxed workspace workers,
/// and seal the results. Returns `Ok(candidate_accepted)`. The live spawn wiring (workspace →
/// sandboxed worker) is un-mirrored and unit-tested via the pure `measure_job::run_measure_job`
/// core rather than here.
// UNVERIFIED(measure-job): the workspace→sandboxed-worker spawn wiring.
fn execute_measure_job(args: &MeasureJobArgs) -> Result<MeasureJobVerdict, MeasureJobFailure> {
    // #114 — the --contract track fixture is READ FIRST, before any golden, because the contract
    // is where this track's REFERENCE-MODEL IDENTITY is declared and the golden loader needs that
    // pin as an input (RULED: the contract is the pin authority, not a compiled-in constant). Read
    // ONCE here; the pool/thermal parse below and the sealed contract digest both reuse these same
    // bytes, so every contract-derived decision in this run describes one read of one file.
    let contract_bytes = std::fs::read(&args.contract)
        .map_err(|e| format!("--contract read failed ({}): {e}", args.contract.display()))?;
    // FAIL-CLOSED: an unreadable contract, or one declaring only half a reference-model pin, is an
    // error — never silently "this track pins no reference model".
    let reference_model = reference_model_pin_from_contract(&contract_bytes)
        .map_err(|e| MeasureJobFailure::die8(format!("--contract reference-model pin: {e}")))?;

    // R13 — `--golden` is REPEATABLE. Load EVERY golden fail-closed (loading also hashes its bytes),
    // then reject a DUPLICATE DIGEST (die-8, pre-GPU) — the same golden bytes passed twice. R7 — the
    // whole set is validated + dup-guarded up front, then the pair loop measures EVERY golden (one
    // per_prompt record per golden, bound by bytes).
    //
    // Each `--golden` is EITHER the live pool's teacher-forcing TAPE or a legacy GoldenDocument,
    // routed by required-key signature ([`load_timed_prompt_checked`]). The pool pins tapes, so the
    // tape is the shape a ranked run actually passes; the GoldenDocument path stays for the offline
    // fixtures and harnesses built on it.
    let mut golden_fixtures = Vec::with_capacity(args.goldens.len());
    for g in &args.goldens {
        golden_fixtures.push(
            load_timed_prompt_checked(g, reference_model.as_ref())
                .map_err(MeasureJobFailure::die8)?,
        );
    }
    let golden_digests: Vec<String> = golden_fixtures
        .iter()
        .map(|g| g.sha256().to_string())
        .collect();
    measure_job::check_golden_digests(&golden_digests).map_err(MeasureJobFailure::die8)?;
    // R7 — the pair loop measures the WHOLE pool (`&golden_fixtures`), one per_prompt record per
    // golden bound BY BYTES. `golden`/`golden_path` below name goldens[0] only for the single-golden
    // sandbox read-grant + the integrity-anchor golden_sha256 (the worker gets its tokens over the
    // protocol, not by re-reading any golden file; per-golden sandbox read-grants are a later on-box
    // refinement, R15).
    let golden = &golden_fixtures[0];
    let golden_path = &args.goldens[0];

    // R13 — the `--prompt`/`--prompt-sha256`/`--target-id` trio is validated at parse (all-three-or-
    // none, sha/target-id shape). Here the file bytes are read: the `--prompt` file's sha256 MUST
    // equal the pinned `--prompt-sha256` (die-8 on mismatch — a pre-GPU prereq/integrity failure).
    if let (Some(pf), Some(pinned)) = (args.prompt.as_ref(), args.prompt_sha256.as_deref()) {
        let bytes = std::fs::read(pf).map_err(|e| {
            MeasureJobFailure::die8(format!("--prompt read failed ({}): {e}", pf.display()))
        })?;
        let actual = sha256_hex(&bytes);
        if actual != pinned {
            return Err(MeasureJobFailure::die8(format!(
                "--prompt file {} hashes to {actual}, but --prompt-sha256 pins {pinned} (die 8)",
                pf.display()
            )));
        }
    }

    // Parse the --contract track fixture (fail-closed) → prompt pool + thermal thresholds. The
    // bytes were read at the top of this function (the reference-model pin needed them before the
    // goldens loaded); this is the typed parse of that SAME byte string.
    let contract = measure_job::Contract::parse(&contract_bytes)?;
    // David ruling (2026-08-26) — THE TRACK MODE FENCE, contract-driven, die-8 PRE-GPU.
    //
    // "why the hell do we reject dflash" — because `DEFAULT_ALLOWED_MODES` was the ONLY list that
    // existed and it was consulted at CLI-parse time, before this file was read. The list is now
    // the fixture's `allowed_modes` when it declares one, and `DEFAULT_ALLOWED_MODES` when it does
    // not, so gemma4 can admit `dflash` without widening a single other track.
    //
    // FIRST of the contract-derived checks, immediately after the parse and BEFORE
    // `effective_candidate_regime` below: the regime resolution now takes the candidate's MODE as an
    // input, and resolving a regime for a mode this track never admitted would be answering the
    // second question before the first. It is also the cheapest refusal in the file — three string
    // comparisons, no filesystem — so an inadmissible mode costs nothing to reject.
    let allowed_modes = measure_job::enforce_track_allowed_modes(
        &args.candidate_spec,
        &args.baseline_spec,
        contract.allowed_modes.as_deref(),
    )
    .map_err(MeasureJobFailure::die8)?;
    // Medium (cycle-3) — every --golden must be PINNED: its sha256 resolves to EXACTLY ONE
    // timed_prompt_pool entry with a POSITIVE noop_decode_speedup, else die-8 BEFORE any GPU work
    // (wrapper noop_reference_for_golden W:663-679). An unpinned golden would otherwise burn gated
    // box time only to seal a results.json the ranked jq rejects (missing/<=0 per-prompt noop).
    measure_job::validate_goldens_pinned(&golden_fixtures, &contract.timed_prompt_pool)
        .map_err(MeasureJobFailure::die8)?;
    // Anti-lottery ≥N-DISTINCT COVERAGE gate (die-8, pre-GPU) — benchd is the FINAL validator. The
    // published ranked score is the MEDIAN over the pool of each prompt's raw serial-relative
    // ratio-of-means (docs/measure-job-contract.md; the all-8-median aggregation of
    // docs/parity-completion-gate.md §3), which is only well-defined when the run's TIMED coverage is
    // EXACTLY the full DISTINCT pinned pool. `validate_goldens_pinned` above accepts a SUBSET (each
    // golden pins individually); this gate additionally requires FULL, DISTINCT coverage of the
    // fixture's timed_prompt_pool — refusing a subset, a duplicate (<N distinct), or a substitution
    // (a timed prompt matching no pin) — so a scoring run can never publish a median over a
    // hand-picked support. Same pre-GPU point, same exit-8 path.
    measure_job::validate_timed_pool_coverage(&golden_fixtures, &contract.timed_prompt_pool)
        .map_err(MeasureJobFailure::die8)?;
    // COHORT (batch-8 brief D9) — the fixture's `scored_batch_size` is the PINNED IDENTITY that
    // selects the batched cohort mode; resolve the regime the candidate leg will ACTUALLY run
    // (the spec-derived regime, upgraded to the batched cohort regime when the fixture declares
    // the ruled B=8; any other width refuses). Resolved HERE — before the calibration pre-read,
    // the config, and the spawn surfaces — so every regime-derived decision below (the b8 series
    // tag the calibration is fenced against, the v1.1 spawn gate, the closure selection) describes
    // the one regime this run measures.
    //
    // David ruling (2026-08-26) — the resolution now takes the candidate's MODE, because the
    // cohort upgrade is mode-aware: a SINGLE-STREAM-ONLY mode (`dflash`, which the engine's cohort
    // driver refuses by name) keeps its single-stream regime even under a fixture that pins a
    // width. Without that, gemma4's `scored_batch_size: 8` would have kept the track structurally
    // closed to dflash even after the mode fence admitted it — the refusal would just have moved
    // from benchd to the engine, one spawn and one chunk of gated box time later.
    let candidate_regime = measure_job::effective_candidate_regime(
        &args.candidate_spec.mode,
        args.candidate_regime,
        contract.scored_batch_size,
    )
    .map_err(MeasureJobFailure::die8)?;
    // Say it out loud when a PINNED cohort width was NOT applied. The regime itself is sealed in
    // `results.timed_mode` and the overlay's §5 series fence keeps the two regimes from ever being
    // pooled or compared — so this note changes no decision — but an operator reading a run of a
    // b8-pinned track must not have to infer from a series tag that this one measured a single
    // stream. Derived from the RETURNED regime, so it cannot disagree with what was resolved.
    if contract.scored_batch_size.is_some() && candidate_regime.scored_batch_point().is_none() {
        eprintln!(
            "benchctl measure-job: --contract pins scored_batch_size {:?}, but the candidate mode \
             {:?} is SINGLE-STREAM ONLY, so this run measures the single-stream series and not the \
             batched cohort. Admitted modes for this track: {allowed_modes:?}.",
            contract.scored_batch_size, args.candidate_spec.mode,
        );
    }
    // Orchestrator ruling (2026-08-23) — the composite score's exponent pair is ALSO a
    // FIXTURE-PINNED IDENTITY, exactly like `scored_batch_size` above: consulted (and REQUIRED)
    // ONLY on the batched cohort regime — a single-stream run never reads `scored_exponents` at
    // all, matching how it never reads `scored_batch_size` beyond the regime selection above.
    // Certified HERE, alongside `candidate_regime`, so the config built below never carries an
    // uncertified value.
    let scored_exponents = match candidate_regime.scored_batch_point() {
        Some(_) => Some(
            measure_job::ScoredExponents::certify(contract.scored_exponents)
                .map_err(MeasureJobFailure::die8)?,
        ),
        None => None,
    };
    // COHORT (D2) — on the batched path, the cohort-membership gate (die-8, pre-GPU): the cohort
    // is EXACTLY the fixture-pinned pool, in POOL ORDER, every slot pinned by sha256 AND bytes.
    // Produces the SEALED member list `per_cohort[].members` carries. `None` on the single-stream
    // path.
    let cohort_members = match candidate_regime.scored_batch_point() {
        Some(point) => Some(
            measure_job::validate_cohort_membership(
                &golden_fixtures,
                &contract.timed_prompt_pool,
                // The CERTIFIED width the regime carries — the same fixture data
                // `effective_candidate_regime` admitted, read back from its one certify point.
                point.batch_size(),
            )
            .map_err(MeasureJobFailure::die8)?,
        ),
        None => None,
    };
    // LANE 2a — the correctness-golden ATTESTATION gate (die-8, pre-GPU), a SEPARATE authority from
    // the anti-lottery timed-pool coverage above. The fixture pins the hidden correctness golden as
    // a SIBLING of `timed_prompt_pool` (engine PR #41), sourced here from the SAME contract bytes via
    // `hidden_correctness_golden_pin_from_contract` — the one place that field path is spelled — so
    // it never perturbs the anti-lottery cardinality (`timed_prompt_pool | length`). The run's
    // attestation is `--correctness-golden`: benchd HASHES the staged bytes (sha256 + bytes), never
    // trusting a self-declared digest, and refuses (fail-closed both directions) any run whose
    // attested identity does not CITE the fixture pin. The golden's NAME appears nowhere — the pin is
    // the only identity.
    let fixture_correctness_pin = hidden_correctness_golden_pin_from_contract(&contract_bytes)
        .map_err(|e| MeasureJobFailure::die8(format!("--contract correctness-golden pin: {e}")))?;
    let attested_correctness_pin = match args.correctness_golden.as_ref() {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| {
                MeasureJobFailure::die8(format!(
                    "--correctness-golden read failed ({}): {e}",
                    path.display()
                ))
            })?;
            Some(CorrectnessGoldenPin {
                sha256: sha256_hex(&bytes),
                bytes: bytes.len() as u64,
            })
        }
        None => None,
    };
    verify_correctness_golden_attestation(
        attested_correctness_pin.as_ref(),
        fixture_correctness_pin.as_ref(),
    )
    .map_err(|e| MeasureJobFailure::die8(e.to_string()))?;
    // #142 — the CAPTURED engine-wire crosscheck now runs AT MEASURE TIME, not only under
    // `cargo test`: benchd re-verifies its embedded captured engine-wire reference against the
    // mirror-integrity reference sha256 and re-parses it under its own CLOSED `WorkerResponse`, so a
    // drifted capture or a schema divergence dies pre-GPU (die-8) rather than being trusted on the
    // contract's self-declared pin alone. Independent of `validate_goldens_pinned`'s contract-pin.
    measure_job::crosscheck_captured_engine_wire(
        bench_runner::ENGINE_WIRE_V1_FIXTURE.as_bytes(),
        bench_runner::ENGINE_WIRE_V1_SHA256,
    )
    .map_err(MeasureJobFailure::die8)?;
    // R14 — `loaded_util` is env-driven (`GPU_LOADED_UTIL`, default 0.70, W:403); GATE_TEMP/COOL_TIMEOUT
    // stay fixed wrapper constants (R21). Resolve the util (fail-closed on an invalid value) and thread
    // it + its honest source into the thermal thresholds.
    let (loaded_util, loaded_util_source) =
        measure_job::resolve_loaded_util(std::env::var("GPU_LOADED_UTIL").ok().as_deref())?;
    let thermal = contract.thermal_thresholds(loaded_util, loaded_util_source);

    // finding 2 + WORKSPACE fix: `--candidate`/`--baseline` are WORKSPACE DIRECTORIES; each leg's
    // runtime-worker executable is resolved as `<ws>/.build/release/<bin>` (bin defaults to
    // `mlxfast-runtime-worker`, overridable via MLXFAST_MEASURE_WORKER_BIN), FAIL-CLOSED if absent.
    // MLXFAST_RUNTIME_WORKER_EXECUTABLE remains an override that must not CONFLICT with the
    // workspace-resolved path. The engine is spawned as `<engine> runtime-worker --weights <DIR>`
    // where `<DIR>` is the SEPARATE `--weights` argument, never the workspace.
    let worker_override = std::env::var("MLXFAST_RUNTIME_WORKER_EXECUTABLE").ok();
    let worker_bin = std::env::var("MLXFAST_MEASURE_WORKER_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| measure_job::DEFAULT_MEASURE_WORKER_BIN.to_string());
    let candidate_exec = measure_job::resolve_workspace_engine(
        &args.candidate.to_string_lossy(),
        &worker_bin,
        worker_override.as_deref(),
    )?;
    let baseline_exec = measure_job::resolve_workspace_engine(
        &args.baseline.to_string_lossy(),
        &worker_bin,
        worker_override.as_deref(),
    )?;
    // #42 box-leg — Metal loads `mlx.metallib` from BESIDE the resolved worker. Verify the sibling
    // exists next to EACH resolved leg engine HERE (pre-GPU, and on the `--preflight-only` path): a
    // missing metallib does not fail at spawn — it kills the run LATE, at the first MLXArray inside
    // the GPU window, after gated box time is spent. Resolution is unchanged; this only asserts the
    // adjacency the run will silently depend on.
    measure_job::verify_worker_metallib_sibling(&candidate_exec)
        .map_err(MeasureJobFailure::die8)?;
    measure_job::verify_worker_metallib_sibling(&baseline_exec).map_err(MeasureJobFailure::die8)?;

    // DECIDE-1 — the general trusted-source-scope freeze, homed HERE (benchd measure-job). The
    // BASELINE workspace IS the trusted ref (sub-decision 2), so its `benchmark.json` editable
    // surface is the contract to freeze, and the roster-of-EIGHT trusted paths are resolved against
    // that same trusted root (sub-decision 3). A manifest whose editablePaths / optionalEditablePaths
    // / exemptPaths overlap ANY roster-of-eight trusted path — directly, cased, or via an inode-
    // identical spelling — is REFUSED here (die-8, pre-GPU), before any gated box time is spent. The
    // eighth roster path is `benchmark.json` itself: a manifest may not declare its own file editable.
    //
    // The manifest is read from the trusted ref, never from a submission, so a candidate can not
    // steer this check. When the trusted ref carries NO `benchmark.json` (an engine-only tree
    // declares its editable surface elsewhere), there is no editable-surface declaration to freeze
    // and nothing to refuse — the freeze binds exactly when the manifest is present, which is the
    // ranked reality; a present-but-overlapping OR malformed manifest is a hard refusal.
    //
    // The absent-manifest SKIP is correct-by-construction (the manifest is `args.baseline`, an
    // operator-controlled arg; a candidate can not suppress it), but the skip is SILENT, so an audit
    // can not tell "checked and passed" from "did not bind". Emit a one-line stderr NOTICE on the
    // skip path so the two are distinguishable in a log.
    let trusted_manifest = args.baseline.join("benchmark.json");
    if trusted_manifest.is_file() {
        let manifest_bytes = std::fs::read(&trusted_manifest).map_err(|e| {
            MeasureJobFailure::die8(format!(
                "trusted-ref benchmark.json read failed ({}): {e}",
                trusted_manifest.display()
            ))
        })?;
        trusted_scope::verify_editable_surface_within_trusted_scope(
            &args.baseline,
            &manifest_bytes,
        )
        .map_err(MeasureJobFailure::die8)?;

        // WIRE-1 item 1a — the AUTHORITATIVE editable-surface BYTE BUDGET (native Rust, executes NO
        // engine-repo code). The caps + editablePaths are read from the TRUSTED --baseline manifest
        // (a candidate can not steer its own budget); the surface WALKED is the --candidate workspace
        // (the submission whose surface we bound). Ported from
        // EditableSurfaceByteBudget.swift@736781ea and pinned against it by tests/byte_budget_parity.rs.
        // An overshoot of maxTotalBytes / maxFileBytes / exemptPathMaxBytes is a die-8 refusal, pre-GPU.
        match byte_budget::verify_byte_budget_over(&manifest_bytes, &args.candidate) {
            byte_budget::BudgetVerification::Verified { .. } => {}
            // Skipped can only arise from a missing contract, impossible here (the bytes are in
            // hand); the variant exists for the Swift-parity test surface, so its arm is test-gated.
            #[cfg(test)]
            byte_budget::BudgetVerification::Skipped(_) => {}
            byte_budget::BudgetVerification::Exceeded(reason) => {
                return Err(MeasureJobFailure::die8(format!(
                    "editable-surface byte budget: {reason}"
                )));
            }
        }
        // WIRE-1 item 1a (growth) — the benchd-native maxGrowthBytes bound the launch-time Swift
        // enforcer resolves but can NOT consume (no review base at launch). benchd HAS the base:
        // growth = candidate − baseline editable code bytes. An overshoot is a die-8 refusal, pre-GPU.
        match byte_budget::verify_growth_over(&manifest_bytes, &args.baseline, &args.candidate) {
            byte_budget::BudgetVerification::Verified { .. } => {}
            #[cfg(test)]
            byte_budget::BudgetVerification::Skipped(_) => {}
            byte_budget::BudgetVerification::Exceeded(reason) => {
                return Err(MeasureJobFailure::die8(format!(
                    "editable-surface growth: {reason}"
                )));
            }
        }
        // WIRE-1 item 1b — the AUTHORITATIVE write-outside-editablePaths gate. Any file changed,
        // added or deleted between the trusted --baseline and the --candidate that is NOT under
        // editablePaths is a die-8 refusal, pre-GPU (the same overlap discipline as #147's
        // trusted-scope: casefold + device:inode, not substring).
        editable_divergence::verify_no_write_outside_editable(
            &manifest_bytes,
            &args.baseline,
            &args.candidate,
        )
        .map_err(MeasureJobFailure::die8)?;
    } else {
        // #150 — the absent-manifest SKIP is silent, so emit a one-line stderr NOTICE so an audit can
        // tell "checked and passed" from "did not bind" (the manifest is operator-controlled, so the
        // skip is correct-by-construction).
        eprintln!(
            "NOTICE trusted-scope freeze: no benchmark.json under the trusted ref {} — not binding \
             (editable surface declared elsewhere; the manifest is operator-controlled, so this \
             skip is correct-by-construction)",
            args.baseline.display()
        );
    }

    // R14 — `BASELINE_CALIBRATION` is a JSON FILE path (env), REPLACING the dead scalar
    // `MLXFAST_QWEN_MTP_SERIAL_CALIBRATION_SPT`. Parse FAIL-CLOSED (a malformed file is die-6, pre-
    // measure) and RESOLVE it for `--target-id` (or the top-level default). The die-6 serial-band
    // ENFORCEMENT against the pooled serial mean runs AFTER measuring. `BASELINE_BAND_ENFORCE`
    // (default 1) makes a MISSING calibration fail closed (die-6). `--calibration-bootstrap` skips it.
    // H6/H2 (cycle-3) — an EMPTY-STRING `BASELINE_BAND_ENFORCE=""` must map to ENFORCED (fail-closed),
    // same as unset; only an explicit `"0"` disables. (The old parse treated "" as disabled.)
    let band_enforce =
        measure_job::band_enforce_from_env(std::env::var("BASELINE_BAND_ENFORCE").ok().as_deref());
    // #105 cycle-5 — the calibration PRE-READ now happens further down, once `track_id` is resolved:
    // the series fence cross-checks the file's `timed_mode`/`track_id` against this run's, so the
    // read cannot precede the track resolution.

    // R14 — resolve the per-leg native-MTP head dirs (QMTP_HEAD_DIR = pinned serial head;
    // QMTP_CANDIDATE_HEAD_DIR = candidate BYO head, defaulting to the pinned head). Existence-check
    // both when present (die-8 prereq). The actual head-into-timed-verb spawn wiring is R15.
    // UNVERIFIED(measure-job): the on-box head-into-verb spawn use (R15).
    let head_dirs = measure_job::resolve_head_dirs(
        std::env::var("QMTP_HEAD_DIR").ok().as_deref(),
        std::env::var("QMTP_CANDIDATE_HEAD_DIR").ok().as_deref(),
    );
    // David ruling (2026-08-26) — the DFlash drafter's OWN per-leg pair, resolved by the SAME
    // function with the SAME defaulting rule (candidate BYO falls back to the pinned dir). Optional
    // here for every mode; REQUIRED for a `dflash` candidate, enforced below by
    // `enforce_dflash_head_present` at the point the MTP head's own unset-refusal lives.
    let dflash_head_dirs = measure_job::resolve_head_dirs(
        std::env::var("QMTP_DFLASH_HEAD_DIR").ok().as_deref(),
        std::env::var("QMTP_CANDIDATE_DFLASH_HEAD_DIR")
            .ok()
            .as_deref(),
    );
    for (hd, labels) in [
        (
            head_dirs.as_ref(),
            ("QMTP_HEAD_DIR", "QMTP_CANDIDATE_HEAD_DIR"),
        ),
        (
            dflash_head_dirs.as_ref(),
            ("QMTP_DFLASH_HEAD_DIR", "QMTP_CANDIDATE_DFLASH_HEAD_DIR"),
        ),
    ] {
        let Some(hd) = hd else { continue };
        for (label, dir) in [(labels.0, &hd.head_dir), (labels.1, &hd.candidate_head_dir)] {
            if !Path::new(dir).is_dir() {
                return Err(MeasureJobFailure::die8(format!(
                    "{label} does not exist or is not a directory: {dir} (die 8)"
                )));
            }
        }
    }

    // WEIGHTS-IDENTITY provenance: digest the `--weights` DIR (the transformed weights loaded by
    // both legs) — this is the weights identity carried as `weights_hash` in results.json and
    // `weights_sha256` in the integrity anchor. Distinct from the candidate WORKSPACE digest below.
    let weights_digest = dir_digest(&args.weights)
        .map_err(|e| format!("--weights digest failed ({}): {e}", args.weights.display()))?;
    // CANDIDATE-IDENTITY provenance: digest the candidate WORKSPACE (the built engine source) —
    // recorded as `candidate_workspace_sha256` in the integrity anchor. A workspace and a weights
    // dir are different things; each carries its own provenance field.
    let candidate_ws_digest = dir_digest(&args.candidate).map_err(|e| {
        format!(
            "candidate workspace digest failed ({}): {e}",
            args.candidate.display()
        )
    })?;
    // Integrity-anchor minor: the seal must cover the BASELINE workspace too (not only the
    // candidate), so both legs' built-engine sources are pinned. Digest it here.
    let baseline_ws_digest = dir_digest(&args.baseline).map_err(|e| {
        format!(
            "baseline workspace digest failed ({}): {e}",
            args.baseline.display()
        )
    })?;
    // Integrity-anchor minor: pin the golden IDENTITY (the sha of the actual --golden bytes, ==
    // `GoldenFixture::sha256`) and the CONTRACT digest (sha of the --contract fixture bytes) so the
    // seal covers the exact prompt oracle + track fixture this run measured against.
    let golden_sha256 = golden.sha256().to_string();
    let contract_sha256 = sha256_hex(&contract_bytes);

    // AUTHOR-AT-SEAL (DECIDE-3) — the sealed `metrics.commit` is AUTHORED from the sha the in-repo
    // dispatch script RECORDED (candidate.sha, from the CI/yukon dispatch context), never from
    // participant git state (unusable under the ranked sandbox) and never trusting a
    // competitor-proposed commit. `MLXFAST_COMMIT_SHA` (the engine's `commitIdentifier` emission)
    // stays DEFENCE-IN-DEPTH: present-and-disagreeing is a die-8 refuse. On a SCORING run
    // (`!args.local_dev`, the same signal `cfg.local_pair_budget` keys on) an ABSENT record fails
    // closed — never git; only `--local-dev` keeps the un-bound resolution.
    let dispatch_record = read_dispatch_sha_record()?;
    let commit_env = std::env::var("MLXFAST_COMMIT_SHA").ok();
    let commit = official::author_sealed_commit(
        dispatch_record.as_deref(),
        commit_env.as_deref(),
        !args.local_dev,
    )
    .map_err(MeasureJobFailure::die8)?;

    // R12 — the SEALED CONSTANT `track_id` (the workflow-declared track id) is resolved from env
    // `MLXFAST_QWEN_MTP_TRACK_ID` or the `--contract` fixture's own `track_id` (constant≡contract≡env:
    // a present env≠contract is a HARD ERROR; neither present fails closed). It is NOT `--tag`: the
    // per-run `--tag` is sealed SEPARATELY as `tag`. `track_name` is optional (env/contract/omit).
    let track_id = measure_job::resolve_track_id(
        std::env::var("MLXFAST_QWEN_MTP_TRACK_ID").ok().as_deref(),
        contract.track_id.as_deref(),
    )?;
    let track_name = measure_job::resolve_track_name(
        std::env::var("MLXFAST_QWEN_MTP_TRACK_NAME").ok().as_deref(),
        contract.track_name.as_deref(),
    );

    // H6/H2 (cycle-3) — under `--calibration-bootstrap` this run AUTHORS the band; it must NOT
    // pre-read or require the existing file. The wrapper's `serial_band_check` returns immediately
    // in bootstrap mode (W:1423-1426) and `write_calibration_bootstrap` reads/merges the file
    // itself afterwards — so a MISSING calibration file in bootstrap mode is fine, not a die-6.
    // #105 cycle-5 (HIGH) — the read carries this run's SERIES and resolved `track_id` so
    // `enforce_calibration_series_fence` can die-6 a cross-series / cross-track calibration here,
    // BEFORE any measuring and therefore before any banding.
    //
    // W3 (fence reconciliation) — the series passed here is THIS RUN'S OWN series, not the
    // hardcoded teacher-forced tag. The band divides the run's pooled SERIAL mean, and under the
    // Fable ruling the serial control runs the run's series ([`measure_job::run_timed_mode`]), so a
    // free-run run bands only against free-run calibration and a TF run only against TF
    // calibration. This is the SAME decision function (`timed_modes_comparable`) the overlay's §5
    // fence uses on results/score — one series story across calibration, overlay and seal.
    let run_timed_mode = measure_job::run_timed_mode(candidate_regime);
    let calibration = resolve_calibration_env(
        args.calibration_bootstrap,
        std::env::var("BASELINE_CALIBRATION").ok().as_deref(),
        args.target_id.as_deref(),
        run_timed_mode,
        &track_id,
    )?;

    let cfg = measure_job::MeasureJobConfig {
        track_id,
        track_name,
        tag: args.tag.clone(),
        tokens: args.tokens,
        mtp_depth: args.mtp_depth,
        candidate_spec: args.candidate_spec.clone(),
        baseline_spec: args.baseline_spec.clone(),
        candidate_spec_source: args.candidate_spec_source.clone(),
        baseline_spec_source: args.baseline_spec_source.clone(),
        min_pairs: args.min_pairs,
        target_pairs: args.target_pairs,
        prompt_pool: contract.timed_prompt_pool.clone(),
        thermal,
        candidate_executable: candidate_exec.clone(),
        baseline_executable: baseline_exec.clone(),
        calibration: calibration.clone(),
        band_enforce,
        // R16 (medium cycle-3) — the sealed top-level `timestamp` (date -u), stamped now.
        run_timestamp: iterate::iso8601_now(),
        calibration_bootstrap: args.calibration_bootstrap,
        target_id: args.target_id.clone(),
        prompt_sha256: args.prompt_sha256.clone(),
        exactness_probe: args.exactness_probe,
        // H6/H3 — OFFICIAL by default (immediate die-5 on a failed pair); `--local-dev` enables the
        // budget loop.
        local_pair_budget: args.local_dev,
        // W3 — the candidate leg's timed regime: the spec-derived regime, upgraded to the batched
        // cohort regime when the fixture pins `scored_batch_size` (COHORT, D9).
        candidate_regime,
        // Orchestrator ruling (2026-08-23) — the CERTIFIED composite exponent pair, resolved
        // above alongside `candidate_regime`; `None` off the batched regime.
        scored_exponents,
    };
    // W3 — refuse an incoherent regime/spec/window combination BEFORE any GPU work (the pair loop
    // re-checks it, but a pre-GPU refusal costs no gated box time).
    measure_job::validate_candidate_regime_coherent(&cfg).map_err(MeasureJobFailure::die8)?;
    // #112 (M1) — and refuse a golden that cannot ORACLE that window, also pre-GPU. The
    // rows-vs-window rule used to live only inside the pair loop's per-prompt `timing_params`,
    // which runs AFTER the `--preflight-only` return below: a tape too short for the window
    // therefore passed preflight and died on the first prompt of the real run instead. Same
    // function, every loaded golden, now on both paths. `cfg.tokens` is the RULED window — the
    // check above has already pinned it to N = FREE_RUN_DECODE_TOKENS in the free-run series;
    // teacher-forced runs use `--tokens` as given.
    // 2b box-leg — a golden routed to the ranked GATES phase as a legacy GoldenDocument must carry
    // the `.benchmark` oracle (a benchmark/official window is TIMED against it). Refuse EARLY and
    // CLEARLY here — naming the engine's weightless attach-benchmark-oracle remedy — BEFORE the
    // generic per-prompt window refusal below, which would otherwise frame a missing oracle as a
    // token-count shortfall. Same die-8 pre-GPU point; actionable message.
    measure_job::validate_gates_goldens_carry_oracle(&golden_fixtures)
        .map_err(MeasureJobFailure::die8)?;
    measure_job::validate_prompt_windows(&golden_fixtures, cfg.tokens)
        .map_err(MeasureJobFailure::die8)?;

    // Each leg/phase spawns a FRESH sandboxed worker from its workspace engine (fail-closed),
    // loading the SHARED `--weights` DIR — the proven official spawn `<engine> runtime-worker
    // --weights <DIR>`. The engine (workspace-resolved) and the weights (the `--weights` DIR) are
    // DIFFERENT paths: the old code passed the workspace as both, so no real run could load weights.
    // UNVERIFIED(measure-job): the sandboxed-workspace spawn recipe (first exercised on-box).
    // UNVERIFIED(B-4): both legs share `--weights` for the backbone/identity case; per-side MTP
    // head weights (QMTP_HEAD_DIR / QMTP_CANDIDATE_HEAD_DIR) are a later refinement (R15).
    // R13 — `--preflight-only`: every pre-GPU prereq/quiesce check above (golden load + dup-digest,
    // prompt-hash, contract parse, workspace-engine resolution, weights/workspace digests, track-id
    // resolution, regime coherence, and — #112 (M1) — the window each golden must be able to
    // oracle) has passed. Exit 0 WITHOUT measuring — no pair loop, no results.json.
    if args.preflight_only {
        eprintln!(
            "benchctl measure-job: --preflight-only OK ({} golden(s) [{}], candidate={}, baseline={}) — \
             pre-GPU checks passed, not measuring",
            args.goldens.len(),
            golden_kind_summary(&golden_fixtures),
            candidate_exec,
            baseline_exec,
        );
        return Ok(MeasureJobVerdict::PreflightOk);
    }
    // ARM GATE (David ruling 2026-08-26) — the track fixture's `official_scoring_enabled` is a REAL
    // gate: a SCORING run (`!args.local_dev`, the same signal author-at-seal and the pair budget key
    // on) over a fixture that does not declare `true` REFUSES here, die-8, naming the flag. Absent
    // and `false` both refuse — an absent arm state is not an armed one.
    //
    // FIRST of the post-preflight pre-GPU checks, ahead of F-6, on purpose: whether the track is
    // ARMED AT ALL dominates every other precondition, and "official scoring is not enabled for
    // this track" is a far more actionable verdict than the calibration/head-dir refusals that
    // would otherwise be reported for an unarmed track that also happens to be missing a band.
    //
    // AFTER the `--preflight-only` return, also on purpose, and for the reason F-6 states just
    // below for itself: preflight opens no GPU window and SEALS NOTHING, and it is the tool the
    // track is brought up WITH — during exactly the period when the flag is legitimately false.
    // The ruling is about refusing to SEAL an official score, not about refusing to look at a
    // workspace. The gate still costs no gated box time: it fires before the first spawn.
    //
    // This one call site covers the whole ranked chain. `benchctl overlay-timing` is LOCAL-only by
    // design (the organizer's trusted shell authors the published score.json, OPEN-2) and its
    // `--contract` is OPTIONAL, but it REQUIRES `--results` — a measure-job artifact — so no ranked
    // score.json can exist without a measure-job that passed this gate.
    measure_job::enforce_official_scoring_enabled(
        !args.local_dev,
        contract.official_scoring_enabled,
        &cfg.track_id,
    )
    .map_err(MeasureJobFailure::die8)?;
    // F-6 — fail fast, PRE-GPU, on a missing baseline calibration under enforcement. The
    // post-measure band check (below, after `run_measure_job`) already die-6s when
    // `BASELINE_BAND_ENFORCE=1` (the default) and no calibration was resolved — but only AFTER the
    // GPU window has opened and both legs have measured. A missing calibration is knowable now, so
    // discovering it here costs no gated box time. Enforcement SEMANTICS are unchanged: the same
    // condition, the same die-6, only earlier. `--preflight-only` returned above and is deliberately
    // NOT gated on this (it opens no GPU window); a real run reaches here. A set-but-unreadable or
    // malformed `BASELINE_CALIBRATION` is ALREADY a pre-measure die-6 in `resolve_calibration_env`;
    // this closes the remaining gap — the env UNSET.
    if band_enforce && !args.calibration_bootstrap && calibration.is_none() {
        return Err(MeasureJobFailure::die6(
            "no BASELINE_CALIBRATION but BASELINE_BAND_ENFORCE=1 (default) — cannot validate the \
             serial baseline; failing closed PRE-GPU (die 6) before opening the timed window. Set \
             BASELINE_BAND_ENFORCE=0 or pass --calibration-bootstrap to author one."
                .to_string(),
        ));
    }
    // R15 — a real measure run needs the PINNED native-MTP head (`QMTP_HEAD_DIR`): the serial leg
    // loads it, and it is the default for the candidate leg's BYO head. Fail closed (die-8) if the
    // pinned head is unset once we are actually measuring (preflight-only already returned above).
    let head_dirs = head_dirs.ok_or_else(|| {
        MeasureJobFailure::die8(
            "QMTP_HEAD_DIR is unset: the pinned native-MTP head is required for a measure run (the \
             serial leg loads it; the candidate leg defaults to it) — die 8"
                .to_string(),
        )
    })?;
    // David ruling (2026-08-26) — the SAME refusal for the DFlash drafter, but only when the
    // candidate actually declares mode `dflash`. Placed HERE, beside the MTP head's refusal and
    // after the `--preflight-only` return, for the reason that return exists: preflight opens no
    // GPU window and seals nothing, and it is the tool a track is brought up with — during exactly
    // the period when the drafter may not be staged yet.
    measure_job::enforce_dflash_head_present(&args.candidate_spec.mode, dflash_head_dirs.as_ref())
        .map_err(MeasureJobFailure::die8)?;

    let serial_plan = resolve_official_sandbox_from_env(&baseline_exec, golden_path)?;
    let candidate_plan = resolve_official_sandbox_from_env(&candidate_exec, golden_path)?;
    let serial_weights = args.weights.to_string_lossy().to_string();
    let candidate_weights = args.weights.to_string_lossy().to_string();
    // R15 — per-side heads passed to the ONE spawned worker per leg: the serial control loads the
    // PINNED head, the candidate the DECLARED BYO head. The head is loaded on BOTH legs (residency
    // charges the denominator), so `--mtp-head` is passed on each.
    //
    // VERIFIED-on-box(window-2, #109) — the spawned verb is the engine's GENERIC `runtime-worker`,
    // and its option surface is exactly `{--weights, --mtp-head, --speculative-protocol}`
    // (`Sources/MLXFastRuntimeWorkerCLI/main.swift`, `requireOnly(values:)`). Window 2 isolated this
    // FOUR WAYS on a live box: `--weights W --mtp-head H` → exit 0 with a real hello;
    // `… --speculative-protocol v1.1` → exit 0 with `spec_modes`/`capabilities`/`head_provenance`;
    // `… --mtp-depth 0 --mtp-report P` (benchd's then-argv) → exit 1 *"unexpected participant worker
    // option --mtp-depth"*; `… --mtp-report P` alone → exit 1 on `--mtp-report`. The verb exits on the
    // FIRST unknown option, BEFORE the hello, which is why every pair of every leg died pre-GPU as
    // *"engine closed the stream before returning a response"*. Both retired flags belonged elsewhere:
    // `--mtp-depth` to a DIFFERENT binary's verb (`mlxfast-swift mtp-timed`) and `--mtp-report` to no
    // verb at all. The surviving argv is fenced by `measure_job::RUNTIME_WORKER_ACCEPTED_FLAGS`.
    //
    // Both retired channels already exist on the wire: DEPTH as the `decode_begin` /
    // `free_decode_begin` `spec` (echoed back as `effective_spec`, spec-never-ignored), and the report
    // facts as the hello's `head_provenance` plus benchd's OWN free-run histogram math. H1 is
    // untouched: benchd's parent-side wall clock (`run_decode_phase_fresh` /
    // `run_free_run_decode_phase_fresh`) was already the ONLY scored value, and is now the only
    // parent-clock number in existence.
    // #105 (Engine-can't-speculate-on-TF) — the SERIAL CONTROL leg always times a SERIAL decode
    // window: it is the depth-0 control (benchd feeds each token, or free-runs with no drafter), so
    // its wire spec is the serial spec.
    //
    // W3 — the CANDIDATE leg's wire spec follows its REGIME:
    //   * teacher-forced (a serial candidate, or the legacy Model-2 shape) → the DOWNGRADED serial
    //     spec, because sealing an mtp regime that cannot have run under teacher forcing is refused
    //     downstream (`tf_regime_is_serial`);
    //   * v1.1 free-run → the DECLARED candidate spec verbatim. The free-run window is exactly where
    //     that spec CAN run, so downgrading it here would measure a serial engine and seal it as the
    //     candidate's number. The runner enforces SPEC-NEVER-IGNORED on the echo, and the seal
    //     refuses a serial echo on a free-run candidate leg (`free_run_regime_is_speculative`).
    // The DECLARED candidate_spec/baseline_spec stay as results.json provenance (cfg carries them
    // into build_results) either way.
    //
    // Coordinator ruling (#109, leg B) — these two values are what a leg WOULD request. Whether it
    // requests anything at all is `measure_job::requested_wire_spec`, applied at the timed window
    // below: on a TF pair the answer is None for both legs (gate-off spawn ⇒ no spec, no echo, and
    // the serial regime sealed from the spawn surface), so the downgraded serial spec above is
    // computed and then deliberately not sent. It stays here because the TF branch of
    // `candidate_wire_spec` is what makes the downgrade explicit at the point a reader looks for it.
    let serial_wire_spec = measure_job::timed_decode_wire_spec();
    let candidate_wire_spec = if candidate_regime.is_free_run() {
        cfg.candidate_spec.clone()
    } else {
        measure_job::timed_decode_wire_spec()
    };

    // #105 cycle-5 finding 4, closed the subtractive way (#109 window-2 finding 3) — the spawn argv
    // no longer carries a depth AT ALL, so the two depth channels it used to have to reconcile are
    // one: the wire `spec` above. The argv's `--mtp-depth` was never a channel the spawned verb
    // could even read (it rejects the flag), so tying it to the wire spec only made a
    // never-honoured value consistent; removing it makes the wire spec the single source by
    // construction, and the runner's spec-never-ignored echo check its only guard.
    //
    // Window-prep gap (engine-train review): the engine gates ALL v1.1 wire fields behind
    // `--speculative-protocol v1.1` at spawn. Free-run legs (BOTH of them — the depth-0 serial
    // control speaks the same v1.1 session) must carry the flag; teacher-forced legs must not
    // (their gate-off spawn is the standing v1-compat proof).
    //
    // David ruling (2026-08-26) — the DFlash drafter is passed the SAME way and with the SAME
    // per-leg split: the serial control gets the PINNED drafter, the candidate its own. This is the
    // whole point of the `--dflash-head` channel — before it, the engine resolved a bare relative
    // `./dflash-head` against the WORKER's CWD, both workers inherit benchctl's CWD (the spawn sets
    // no `current_dir`), and so both legs loaded ONE directory no matter which workspace they were
    // measuring.
    //
    // BOTH legs' argv now come from ONE call. The four hand-written field accesses this replaces —
    // pinned/BYO x mtp/dflash — were the only remaining place a leg could be handed the OTHER leg's
    // head, and they lived in `execute_measure_job`, whose spawn wiring is explicitly
    // `UNVERIFIED(measure-job)` and therefore covered by no test at all. A mutation that swapped
    // the candidate leg's drafter for the pinned one passed the whole suite; against
    // `paired_leg_spawn_args` it does not.
    let (serial_base_args, candidate_base_args) =
        measure_job::paired_leg_spawn_args(&head_dirs, dflash_head_dirs.as_ref(), candidate_regime);
    // #109 window-2 finding 3 — fence BOTH legs' argv against the verb's accepted option surface
    // before any worker is spawned, so a flag the engine would reject dies here, naming itself,
    // rather than as one opaque "engine closed the stream" infra reject per leg per pair.
    for base_args in [&serial_base_args, &candidate_base_args] {
        measure_job::validate_spawn_argv(base_args).map_err(MeasureJobFailure::die8)?;
    }

    // H3 (cycle-3) — arm the RunTimeout budget for the timed decode round-trips (§2.2/§4):
    // `N × band-ceiling × margin`. The band-ceiling (upper acceptance/latency band bound, s/tok) is
    // `calibration.serial_mean × calibration.band_high` when a BASELINE_CALIBRATION is present, else
    // the deliberately-generous fallback constant. A hung/looping engine then aborts as `RunTimeout`
    // (session discarded) instead of wedging benchd inside the timed window. Liveness bound only —
    // never a score input.
    //
    // #108 (M2) — the budget is FAIL-CLOSED, not optional. A degenerate `N × ceiling × margin` used
    // to yield `None`, arming NO deadline; since the ceiling is calibration-derived, a
    // `BASELINE_CALIBRATION` file could disarm the §2.2 bound entirely. Now the arithmetic returns
    // an `Err`, and the leg fails under `RejectClass::RunTimeoutBudgetInvalid` rather than running
    // the timed window unbounded. (The band bounds are ALSO validated at parse, so a well-formed
    // calibration cannot reach this at all; this is the second fence, on the arithmetic itself.)
    let run_timeout_result = {
        let band_ceiling_spt = match calibration.as_ref() {
            Some(cal) if cal.serial_mean.is_finite() && cal.serial_mean > 0.0 => {
                cal.serial_mean * cal.band_high
            }
            _ => bench_core::constants::RUN_TIMEOUT_DEFAULT_BAND_CEILING_SECONDS_PER_TOKEN,
        };
        bench_core::score::run_timeout_budget(
            args.tokens,
            band_ceiling_spt,
            bench_core::constants::RUN_TIMEOUT_MARGIN,
        )
    };

    // R15 — each leg is ONE `runtime-worker` invocation: ONE fresh sandboxed worker, ONE cool gate
    // (finding R21: the FIXED wrapper constant 40 °C, W:422-429 — recorded in `provenance.thermal`),
    // then ONE timed decode window (the seed prefill INSIDE it, `prefill_component: "none"`). H1
    // (cycle-3): benchd's OWN parent wall clock is the scored `benchd_seconds_per_token`.
    //
    // #109 window-2 finding 3 — every ECHO/AUDIT fact now comes off the WIRE, because under the
    // generic verb there is no report file to read (and asking for one killed the spawn):
    //   * `effective_spec` — the `decode_begin` / `free_decode_begin` echo the runner captured and
    //     validated never-ignored, threaded through as `wire_effective_spec`;
    //   * `head_provenance.sha256` — the engine's `hello` echo, captured below and threaded through
    //     as `wire_head_provenance`;
    //   * draft statistics — benchd's OWN histogram math over the free-run §3 audit (already the
    //     case per W3; the TF-only report requirement is retired, since teacher forcing feeds every
    //     token and no round can draft).
    // `validate_leg_report` still FAILS CLOSED on each missing echo — nothing is ever fabricated.
    //
    // W3 — the leg's REGIME selects which runner entry point drives it:
    //   * `LegRegime::TeacherForcedV1` → `run_decode_phase_fresh` (v1: benchd feeds each golden
    //     token, N forced single-token forwards);
    //   * `LegRegime::FreeRunV1_1` → `run_free_run_decode_phase_fresh` (v1.1: the engine drives its
    //     own recurrence; benchd clocks the batched `free_decode_begin` + `free_decode_run(N)` round
    //     trip exactly as PROTOCOL-v1.1 §2.2 specifies, exact-matches every committed token, and
    //     enforces the §2.6 triple at the phase close).
    // Both paths take benchd's OWN parent clock as the only scored number (H1) and arm the same
    // §2.2 RunTimeout budget; the free-run path additionally REFUSES an engine that does not
    // advertise `free_run_decode` before the cool gate and before the clock (§2.1).
    let measure_leg = |plan: &OfficialSandboxPlan,
                       weights: &str,
                       base_args: &[String],
                       leg_spec: &bench_protocol::SpecConfig,
                       regime: measure_job::LegRegime,
                       params: &bench_runner::TimingParams|
     -> bench_runner::Result<measure_job::LegInvocation> {
        // #109 window-2 finding 3 — the spawn argv is EXACTLY the accepted surface: the transport
        // prepends `runtime-worker --weights W`, and `base_args` carries `--mtp-head H` (+ the v1.1
        // spawn gate on a free-run leg). Nothing is appended here — the per-attempt `--mtp-report`
        // path that used to be is retired with the flag.
        let extra_args = base_args.to_vec();

        let mut recorded = coolgate::GateState::SkippedNoReader;
        // #109 window-2 finding 3 — capture the hello's `head_provenance` (the engine's echo of the
        // head bytes it loaded) from the LAST spawn this leg made: the retired report file was the
        // only other channel that ever carried the candidate's head identity. Same capture pattern
        // as the cool-gate state above.
        let wire_head_provenance = std::cell::RefCell::new(None);
        let mut spawn = || -> bench_runner::Result<Session<ChildStdioTransport>> {
            let transport =
                ChildStdioTransport::spawn_official_sandboxed(plan, weights, &extra_args)?;
            let (session, hello) = Session::connect(transport)?;
            *wire_head_provenance.borrow_mut() = hello.head_provenance.clone();
            Ok(session)
        };
        let mut gate = |phase: &str| -> bench_runner::Result<()> {
            recorded = coolgate::cool_gate_report(phase)?;
            Ok(())
        };
        // H1 (cycle-3) — benchd's OWN parent-side wall clock, measured here, is the SCORED spt.
        // H3 (cycle-3) — arm the RunTimeout budget over this leg's timed decode window.
        // spec (docs/spec-config-design.md) — carry the leg's declared spec on the timed decode
        // window. The runner enforces SPEC-NEVER-IGNORED: it discards the session fail-closed
        // (RunnerError::SpecEchoDivergence → a retryable reject) if the engine's echoed effective_spec
        // is absent or diverges from what was requested, so a leg can never silently run a different
        // (or default) spec than the one declared. The echoed spec is surfaced on `timing.effective_spec`.
        // #108 (M2) — a leg NEVER opens its timed window without a §2.2 deadline: an unarmable
        // budget fails THIS leg (own reject class) instead of disarming the only wall-clock bound.
        let run_timeout =
            run_timeout_result
                .as_ref()
                .map_err(|detail| RunnerError::RunTimeoutBudgetInvalid {
                    detail: detail.clone(),
                })?;
        // Coordinator ruling (#109, leg B) — the spec is requested ONLY on a free-run leg
        // (`measure_job::requested_wire_spec`). A TF leg is spawned gate-off, and a gate-off worker
        // speaks strict v1: it rejects any wire `spec` at the session's spec guard and runs its
        // teacher-forced kinds serially regardless. Asking one for a spec would discard every TF
        // session for an echo the worker is gated out of producing; the gate-off spawn is itself the
        // proof of serial semantics, so nothing is asked for and nothing is expected back.
        let params = params
            .clone()
            .with_run_timeout(Some(*run_timeout))
            .with_spec(measure_job::requested_wire_spec(leg_spec, regime));
        // W3 — one timed window per leg, driven by the leg's regime. `free_run_audit` is `Some` only
        // on a v1.1 leg, and only after the runner's §2.6 triple passed at the phase-close barrier.
        // `_peak_ram_gb`: the worker's phase-close `phase_diagnostics` peak RAM. It reached the seal
        // only through the retired report struct's audit-only `peak_ram_gb`, which no consumer ever
        // read (no pair record, per-prompt or aggregate field is derived from it) — retired with the
        // struct. The wire still carries it for any future seal that wants it.
        let (seconds_per_token, _peak_ram_gb, wire_effective_spec, free_run_audit) = match regime {
            measure_job::LegRegime::TeacherForcedV1 => {
                let t = bench_runner::run_decode_phase_fresh(&mut spawn, &mut gate, &params)?;
                (t.seconds_per_token, t.peak_ram_gb, t.effective_spec, None)
            }
            measure_job::LegRegime::FreeRunV1_1 => {
                let t =
                    bench_runner::run_free_run_decode_phase_fresh(&mut spawn, &mut gate, &params)?;
                (
                    t.seconds_per_token,
                    t.peak_ram_gb,
                    t.effective_spec,
                    Some(t.audit),
                )
            }
            // COHORT — a batched leg times ONE window over the whole cohort and is driven by the
            // COHORT measure closure (`measure_cohort_leg`, CohortTimingParams) on the batched
            // branch of this function; this single-stream closure can never legitimately receive
            // the batched regime, so reaching it is a wiring defect, refused fail-closed rather
            // than silently timed as a single stream (which would swap the measured quantity).
            measure_job::LegRegime::BatchedFreeRunV1_2(_) => {
                return Err(RunnerError::Protocol(
                    "batched cohort legs are driven by the cohort measure closure \
                     (CohortTimingParams), never the single-stream one — wiring defect"
                        .to_string(),
                ));
            }
        };
        // R16 — the on-box per-block sampled telemetry stream (GPU temp / steady freq) is not wired
        // into this path yet; with no sample available the top-level `telemetry` seal is OMITTED
        // honestly, never fabricated from the gate state.
        // UNVERIFIED(measure-job): the on-box telemetry-sample stream is an engine/gate-protocol
        // addition; until it exists, benchd observes no sample and omits `telemetry`.
        Ok(measure_job::LegInvocation {
            // H1 (cycle-3) — the ONLY scored number: benchd's own parent clock.
            benchd_seconds_per_token: seconds_per_token,
            gate_state: recorded,
            telemetry: None,
            // The WIRE engine-echoed effective_spec benchd's runner captured + validated (equal to the
            // request; a divergence would have already discarded the session above). Sealed per leg.
            wire_effective_spec,
            // #109 window-2 finding 3 — the WIRE head echo from this leg's hello.
            wire_head_provenance: wire_head_provenance.into_inner(),
            // W3 — the regime this leg actually ran, and its §3 AUDIT when it free-ran.
            regime,
            free_run_audit,
            // COHORT — never produced by the single-stream closure (the batched regime is refused
            // above); the cohort closure on the batched branch fills it.
            cohort_audit: None,
            // COMPOSITE (Gemma cohort scoring) — the phase-split window is a batched-regime-only
            // channel; this closure never drives the batched regime (refused above), so it has
            // none to report.
            cohort_phase_windows: None,
            // Per-stream timing (gap G2) — a batched-only wire channel; none exists here.
            per_stream_timing: None,
            // (b) admission — the committed-token journal is a batched-regime-only channel; the
            // single-stream regime enforces token correctness inline in the runner, so there is no
            // journal to surface for a trusted-oracle gate here.
            cohort_committed_tokens_by_stream: None,
        })
    };
    let measure_serial = |params: &bench_runner::TimingParams| {
        measure_leg(
            &serial_plan,
            &serial_weights,
            &serial_base_args,
            &serial_wire_spec,
            // Fable ruling (same-series serial control) — the control runs THE SAME REGIME as the
            // candidate, at depth 0 (its wire spec above is the serial spec either way). Both legs
            // therefore share the same verb, the same N, the same RunTimeout arithmetic and the same
            // parent clock, so the ratio divides two numbers of one measured quantity.
            measure_job::serial_control_regime_for(candidate_regime),
            params,
        )
    };
    let measure_candidate = |params: &bench_runner::TimingParams| {
        measure_leg(
            &candidate_plan,
            &candidate_weights,
            &candidate_base_args,
            &candidate_wire_spec,
            candidate_regime,
            params,
        )
    };

    // COHORT (batch-8 brief §4.5) — one closure per leg for the BATCHED cohort window, mirroring
    // `measure_leg` line for line except that it drives the batched runner entry point with the
    // COHORT params (one fresh worker, one cool gate, ONE timed window over all B streams) and
    // fills the cohort audit channel. The RunTimeout budget scales to the window it bounds: a
    // cohort window commits B*N tokens under a per-committed-token band ceiling, so the budget is
    // `B*N × ceiling × margin` — the single-stream `N × ceiling × margin` would under-bound an
    // honest cohort window by a factor of B and trip on every run.
    let measure_cohort_leg = |plan: &OfficialSandboxPlan,
                              weights: &str,
                              base_args: &[String],
                              leg_spec: &bench_protocol::SpecConfig,
                              regime: measure_job::LegRegime,
                              params: &bench_runner::CohortTimingParams|
     -> bench_runner::Result<measure_job::LegInvocation> {
        let extra_args = base_args.to_vec();
        let mut recorded = coolgate::GateState::SkippedNoReader;
        let wire_head_provenance = std::cell::RefCell::new(None);
        let mut spawn = || -> bench_runner::Result<Session<ChildStdioTransport>> {
            let transport =
                ChildStdioTransport::spawn_official_sandboxed(plan, weights, &extra_args)?;
            let (session, hello) = Session::connect(transport)?;
            *wire_head_provenance.borrow_mut() = hello.head_provenance.clone();
            Ok(session)
        };
        let mut gate = |phase: &str| -> bench_runner::Result<()> {
            recorded = coolgate::cool_gate_report(phase)?;
            Ok(())
        };
        let band_ceiling_spt = match calibration.as_ref() {
            Some(cal) if cal.serial_mean.is_finite() && cal.serial_mean > 0.0 => {
                cal.serial_mean * cal.band_high
            }
            _ => bench_core::constants::RUN_TIMEOUT_DEFAULT_BAND_CEILING_SECONDS_PER_TOKEN,
        };
        let run_timeout = bench_core::score::run_timeout_budget(
            args.tokens * params.batch_size as usize,
            band_ceiling_spt,
            bench_core::constants::RUN_TIMEOUT_MARGIN,
        )
        .map_err(|detail| RunnerError::RunTimeoutBudgetInvalid { detail })?;
        let params = params
            .clone()
            .with_run_timeout(Some(run_timeout))
            .with_spec(measure_job::requested_wire_spec(leg_spec, regime));
        let t =
            bench_runner::run_batched_free_run_decode_phase_fresh(&mut spawn, &mut gate, &params)?;
        // COMPOSITE (Gemma cohort scoring) — the phase-split window, straight off the runner's
        // new fields (benchd's own parent clock; the engine reports nothing new here).
        let cohort_phase_windows = measure_job::CohortPhaseWindows::from(&t);
        // Per-stream timing (gap G2, REPORT-ONLY) — the PR-A carry, lifted VERBATIM for the
        // attestation seal. Untrusted for scoring; nothing enforced reads it.
        let per_stream_timing = measure_job::PerStreamTimingCarry::from(&t);
        Ok(measure_job::LegInvocation {
            benchd_seconds_per_token: t.seconds_per_token,
            gate_state: recorded,
            telemetry: None,
            wire_effective_spec: t.effective_spec,
            wire_head_provenance: wire_head_provenance.into_inner(),
            regime,
            free_run_audit: None,
            cohort_audit: Some(t.audit),
            cohort_phase_windows: Some(cohort_phase_windows),
            per_stream_timing: Some(per_stream_timing),
            // (b) admission — surface the candidate's committed rectangle UNJUDGED for benchd's
            // trusted-oracle tolerance gate. Surfaced on BOTH cohort legs; `validate_leg_report`
            // keeps it only on the candidate leg (the serial control is not token-judged).
            cohort_committed_tokens_by_stream: Some(t.tokens_by_stream),
        })
    };

    // (b) admission — the TRUSTED-ORACLE closure passed to `run_cohort_measure_job`. It is the ONLY
    // place the oracle's build + weights are fixed, and it fixes BOTH to organizer-controlled sources
    // the candidate cannot touch:
    //   * BUILD (N1): the trusted worker bin comes from `resolve_trusted_oracle_worker_bin`, which
    //     reads ONLY `MLXFAST_TRUSTED_ORACLE_WORKER_BIN` and FAILS CLOSED if unset — it shares NO
    //     fallback with the candidate/baseline resolver, so the oracle can never be the candidate
    //     build. The oracle's forward AND weight-load code are therefore the organizer's.
    //   * WEIGHTS: `--weights` is the ORGANIZER's reference weights dir (`args.weights` — the SAME
    //     organizer source both measured legs already use, main.rs ~1563), fixed HERE at spawn, NEVER
    //     derived from the candidate's response / journal / env. `run_cohort_measure_job` passes this
    //     closure only TOKENS (organizer replay seeds + the candidate journal to JUDGE), never a
    //     weights path.
    // So even a candidate that loaded rogue weights in its OWN cohort run only makes its journal
    // diverge MORE from this organizer-weights reference (→ rejected), never helps it pass.
    let oracle_weights = args.weights.to_string_lossy().to_string();
    let oracle = |replay_seeds_by_stream: &[Vec<i64>],
                  committed_by_stream: &[Vec<i64>]|
     -> bench_runner::Result<bench_protocol::CohortReferenceReplayReport> {
        // TRUSTED BUILD, FAIL-CLOSED — never a fallback to the candidate worker bin.
        let trusted_bin =
            measure_job::resolve_trusted_oracle_worker_bin().map_err(RunnerError::Protocol)?;
        // Spawn the trusted worker over the ORGANIZER reference weights on a PLAIN runtime-worker
        // argv (the verb is NOT behind the --speculative-protocol gate).
        let transport = ChildStdioTransport::spawn(&trusted_bin, &oracle_weights, &[])?;
        let (mut session, hello) = Session::connect(transport)?;
        // N1 wire half — REFUSE unless the (trusted) hello advertised the capability. The UNTRUSTED
        // candidate worker never advertises it, so benchd never asks it for a reference argmax.
        if !hello.supports_cohort_reference_replay() {
            return Err(RunnerError::CapabilityNotAdvertised {
                capability: bench_protocol::CAPABILITY_COHORT_REFERENCE_REPLAY.to_string(),
            });
        }
        session.cohort_reference_replay(replay_seeds_by_stream, committed_by_stream)
    };

    let outcome = match cohort_members {
        // COHORT — the batched cohort pair loop: same alternation/retry/die-5 machinery, cohort
        // params, cohort seal.
        Some(members) => measure_job::run_cohort_measure_job(
            &golden_fixtures,
            members,
            &weights_digest,
            &commit,
            &cfg,
            |params: &bench_runner::CohortTimingParams| {
                measure_cohort_leg(
                    &serial_plan,
                    &serial_weights,
                    &serial_base_args,
                    &serial_wire_spec,
                    measure_job::serial_control_regime_for(candidate_regime),
                    params,
                )
            },
            |params: &bench_runner::CohortTimingParams| {
                measure_cohort_leg(
                    &candidate_plan,
                    &candidate_weights,
                    &candidate_base_args,
                    &candidate_wire_spec,
                    candidate_regime,
                    params,
                )
            },
            oracle,
        )?,
        None => measure_job::run_measure_job(
            &golden_fixtures,
            &weights_digest,
            &commit,
            &cfg,
            measure_serial,
            measure_candidate,
        )?,
    };

    // Seal results.json (+ bare-basename .sha256) and anchor its digest inside the integrity
    // sidecar (finding 10 — digest INSIDE benchmark-integrity, name derived from the results
    // stem, no hardcoded clobbering sibling).
    let results_json = outcome
        .results
        .to_sealed_json()
        .map_err(|e| format!("results.json serialization failed: {e}"))?;
    let results_path = args.out.join("results.json");
    let results_sha256 = write_results_json(&results_path, &results_json)?;

    let integrity_name = format!(
        "benchmark-integrity.{}.json",
        results_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
    );
    let integrity_path = args.out.join(integrity_name);
    let integrity = build_measure_job_integrity(
        args,
        MeasureJobSealInputs {
            results_path: results_path.display().to_string(),
            results_sha256,
            candidate_executable: candidate_exec,
            baseline_executable: baseline_exec,
            candidate_workspace_sha256: candidate_ws_digest.sha256.clone(),
            baseline_workspace_sha256: baseline_ws_digest.sha256.clone(),
            golden_sha256,
            contract_sha256,
            weights_sha256: weights_digest.sha256.clone(),
            weights_file_count: weights_digest.file_count,
            weights_byte_count: weights_digest.byte_count,
        },
    );
    let integrity_json = serde_json::to_string_pretty(&integrity)
        .map_err(|e| format!("integrity serialization failed: {e}"))?;
    std::fs::write(&integrity_path, format!("{integrity_json}\n"))
        .map_err(|e| format!("could not write {}: {e}", integrity_path.display()))?;

    // finding R19 — there is no mid-pair hard die: a thermal-gate timeout (and every other reject
    // class) was retried once inside the pair loop and, on persistence, simply left the pair
    // unaccepted; too few accepted pairs is the die-5 verdict below. results.json is already sealed.
    eprintln!(
        "benchctl measure-job: wrote {} (accepted_pairs={}, candidate_accepted={})",
        results_path.display(),
        outcome.results.accepted_pair_count,
        outcome.candidate_accepted,
    );

    // R14 — the serial-band verdict (results.json is ALREADY sealed with the calibration provenance).
    // `--calibration-bootstrap` SKIPS the band check (authoring mode). Otherwise: a MISSING calibration
    // under BASELINE_BAND_ENFORCE fails closed (die-6); a present calibration enforces the pooled serial
    // mean / calibration mean band + decode_tokens match (die-6). A drifted BASELINE invalidates the
    // comparison, so die-6 takes PRECEDENCE over the die-5 candidate verdict.
    if args.calibration_bootstrap {
        // R13/R14 — bootstrap AUTHORS the band (it does not check it). Author the per-target entry
        // ONLY after a fully-accepted, parity-true run, merging into any existing BASELINE_CALIBRATION
        // file (other targets preserved) and installing it atomically. A rejected/parity-false run,
        // or a bootstrap without --target-id / without a destination, authors nothing (logged, not fatal).
        if measure_job::should_author_bootstrap(
            outcome.candidate_accepted,
            outcome.results.parity_all_ok,
        ) {
            match (
                args.target_id.as_deref(),
                std::env::var("BASELINE_CALIBRATION")
                    .ok()
                    .filter(|s| !s.trim().is_empty()),
            ) {
                (Some(tid), Some(path)) => {
                    let path = std::path::PathBuf::from(path.trim());
                    let existing = std::fs::read(&path).ok();
                    let json = measure_job::build_bootstrap_calibration(
                        existing.as_deref(),
                        &measure_job::BootstrapAuthorInput {
                            target_id: tid,
                            // #105 cycle-5 — author the file's REQUIRED series + track identity from
                            // the run that measured the band, so the authored file passes its own
                            // fence on the next (same-series, same-track) run and dies on any other.
                            // W3 — the run's OWN series, not the hardcoded TF tag: a free-run
                            // bootstrap that stamped `teacher_forced_v1` would author a file that
                            // die-6s every subsequent free-run run against its own band.
                            timed_mode: measure_job::run_timed_mode(candidate_regime),
                            track_id: &outcome.results.track_id,
                            pooled_serial_mean: outcome
                                .results
                                .aggregate
                                .baseline_serial_seconds_per_token_mean,
                            tokens: args.tokens,
                            mtp_depth: args.mtp_depth,
                            serial_control_depth: measure_job::SERIAL_CONTROL_DEPTH,
                            pairs_total: outcome.results.accepted_pair_count,
                        },
                    )
                    .map_err(MeasureJobFailure::die6)?;
                    measure_job::write_bootstrap_calibration(&path, &json)
                        .map_err(MeasureJobFailure::die6)?;
                    eprintln!(
                        "benchctl measure-job: --calibration-bootstrap authored targets[{tid}] in {} \
                         (serial_mean={}, decode_tokens={})",
                        path.display(),
                        outcome.results.aggregate.baseline_serial_seconds_per_token_mean,
                        args.tokens,
                    );
                }
                _ => eprintln!(
                    "benchctl measure-job: --calibration-bootstrap needs both --target-id and a \
                     BASELINE_CALIBRATION destination path to author; skipping the write."
                ),
            }
        } else {
            eprintln!(
                "benchctl measure-job: --calibration-bootstrap SKIPPED authoring — the run was not \
                 fully accepted + parity-true (candidate_accepted={}, parity_all_ok={}).",
                outcome.candidate_accepted, outcome.results.parity_all_ok,
            );
        }
    } else {
        match calibration.as_ref() {
            None => {
                if band_enforce {
                    return Err(MeasureJobFailure::die6(
                        "no BASELINE_CALIBRATION but BASELINE_BAND_ENFORCE=1 (default) — cannot \
                         validate the serial baseline; failing closed (die 6). Set \
                         BASELINE_BAND_ENFORCE=0 or pass --calibration-bootstrap to author one."
                            .to_string(),
                    ));
                }
            }
            Some(cal) => {
                let pooled_serial_mean = outcome
                    .results
                    .aggregate
                    .baseline_serial_seconds_per_token_mean;
                // Only meaningful once the candidate is accepted (a valid pooled serial mean); a
                // rejected candidate is the die-5 verdict below, not a baseline-drift die-6.
                if outcome.candidate_accepted {
                    if let Err(reason) = measure_job::enforce_serial_band(
                        pooled_serial_mean,
                        args.tokens,
                        cal,
                        band_enforce,
                    ) {
                        eprintln!("benchctl measure-job: {reason}");
                        return Ok(MeasureJobVerdict::CalibrationDrift);
                    }
                }
            }
        }
    }

    Ok(if outcome.candidate_accepted {
        MeasureJobVerdict::Accepted
    } else {
        MeasureJobVerdict::RejectedDie5
    })
}

/// Parsed `overlay-timing` flags (A-3, seam 3 LOCAL).
struct OverlayTimingArgs {
    gates_score: PathBuf,
    results: PathBuf,
    score_path: PathBuf,
    integrity: Option<PathBuf>,
    /// R17 — the contract fixture whose `timed_prompt_pool | length` gives the expected pool_size
    /// when env `MLXFAST_QWEN_MTP_POOL_SIZE` is unset. Optional; if neither is available the overlay
    /// fails closed (pool_size unknown ⇒ no score).
    contract: Option<PathBuf>,
}

/// A-3: the Option-A OVERLAY subcommand (seam 3, LOCAL). Parses the flags, merges the sealed
/// gates-score.json with the measure-job results.json (3.8 median regime), seals the ranked
/// score.json (+ bare-basename .sha256), and re-anchors integrity `score_sha256` over the merged
/// bytes. Exit 0 when the merged score PASSES, nonzero (1) when a floor/ceiling bound fails; 2 on
/// a usage error; 1 on a load/validation/IO error.
fn run_overlay_timing_cli(args: &[String]) -> ExitCode {
    let parsed = match parse_overlay_timing_args(args) {
        Ok(Some(p)) => p,
        Ok(None) => {
            print!("{OVERLAY_TIMING_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("benchctl overlay-timing: {msg}");
            eprint!("{OVERLAY_TIMING_USAGE}");
            return ExitCode::from(2);
        }
    };
    match execute_overlay_timing(&parsed) {
        // The merged score encodes any bound failure; a non-pass exits nonzero so callers notice
        // (a floor/ceiling fail sets passed=false, score=null), matching the iterate contract.
        Ok(passed) => ExitCode::from(iterate_exit_status(passed)),
        Err(msg) => {
            eprintln!("benchctl overlay-timing: {msg}");
            ExitCode::from(1)
        }
    }
}

fn parse_overlay_timing_args(args: &[String]) -> Result<Option<OverlayTimingArgs>, String> {
    let mut gates_score: Option<PathBuf> = None;
    let mut results: Option<PathBuf> = None;
    let mut score_path: Option<PathBuf> = None;
    let mut integrity: Option<PathBuf> = None;
    let mut contract: Option<PathBuf> = None;

    fn value<'a>(args: &'a [String], i: usize, name: &str) -> Result<&'a str, String> {
        args.get(i + 1)
            .map(|s| s.as_str())
            .ok_or_else(|| format!("flag {name} requires a value"))
    }

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(None),
            "--gates-score" => {
                gates_score = Some(PathBuf::from(value(args, i, "--gates-score")?));
                i += 2;
            }
            "--results" => {
                results = Some(PathBuf::from(value(args, i, "--results")?));
                i += 2;
            }
            "--score-path" => {
                score_path = Some(PathBuf::from(value(args, i, "--score-path")?));
                i += 2;
            }
            "--integrity" => {
                integrity = Some(PathBuf::from(value(args, i, "--integrity")?));
                i += 2;
            }
            "--contract" => {
                contract = Some(PathBuf::from(value(args, i, "--contract")?));
                i += 2;
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }

    let gates_score = gates_score.ok_or("missing required --gates-score")?;
    let results = results.ok_or("missing required --results")?;
    let score_path = score_path.ok_or("missing required --score-path")?;
    Ok(Some(OverlayTimingArgs {
        gates_score,
        results,
        score_path,
        integrity,
        contract,
    }))
}

/// R17 — resolve the expected pool SHAPE fail-closed. `pool_size` comes from env
/// `MLXFAST_QWEN_MTP_POOL_SIZE` when set, else the `--contract` fixture's `timed_prompt_pool |
/// length`; if NEITHER is available the overlay refuses to score (pool_size unknown). `min_per_prompt`
/// is env `MLXFAST_QWEN_MTP_MIN_PAIRS_PER_PROMPT` (default 1).
fn resolve_pool_expectation(contract: Option<&Path>) -> Result<overlay::PoolExpectation, String> {
    let pool_size = match std::env::var("MLXFAST_QWEN_MTP_POOL_SIZE") {
        Ok(v) if !v.trim().is_empty() => v.trim().parse::<usize>().map_err(|e| {
            format!("MLXFAST_QWEN_MTP_POOL_SIZE ({v:?}) is not a valid pool size: {e}")
        })?,
        _ => {
            let path = contract.ok_or(
                "pool_size unknown: set MLXFAST_QWEN_MTP_POOL_SIZE or pass --contract <fixture> \
                 (fail-closed, no score)",
            )?;
            let bytes = std::fs::read(path)
                .map_err(|e| format!("--contract read failed ({}): {e}", path.display()))?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("--contract parse failed: {e}"))?;
            let pool = value
                .get("timed_prompt_pool")
                .and_then(|p| p.as_array())
                .ok_or("--contract fixture has no `timed_prompt_pool` array")?;
            pool.len()
        }
    };
    if pool_size == 0 {
        return Err("resolved pool_size is 0: the pool must have at least one prompt".to_string());
    }
    let min_per_prompt = match std::env::var("MLXFAST_QWEN_MTP_MIN_PAIRS_PER_PROMPT") {
        Ok(v) if !v.trim().is_empty() => v.trim().parse::<usize>().map_err(|e| {
            format!("MLXFAST_QWEN_MTP_MIN_PAIRS_PER_PROMPT ({v:?}) is not a valid count: {e}")
        })?,
        _ => 1,
    };
    if min_per_prompt == 0 {
        return Err(
            "MLXFAST_QWEN_MTP_MIN_PAIRS_PER_PROMPT is 0: at least 1 pair per prompt \
                    is required"
                .to_string(),
        );
    }
    Ok(overlay::PoolExpectation {
        pool_size,
        min_per_prompt,
    })
}

/// Execute the overlay: load + validate the two inputs (fail-closed), merge them (pure
/// `overlay::merge_overlay`), seal the ranked score.json via the SHARED bare-basename sealed-write
/// (finding 14 — same recipe the measure-job results.json uses), and re-anchor integrity
/// `score_sha256` over the merged bytes. Returns `Ok(passed)`.
fn execute_overlay_timing(args: &OverlayTimingArgs) -> Result<bool, String> {
    // Load + deserialize the sealed gates-score.json into a typed ScorePayload.
    let gates_bytes = std::fs::read(&args.gates_score).map_err(|e| {
        format!(
            "--gates-score read failed ({}): {e}",
            args.gates_score.display()
        )
    })?;
    let gates: crate::score::ScorePayload = serde_json::from_slice(&gates_bytes)
        .map_err(|e| format!("--gates-score parse failed: {e}"))?;

    // Load + deserialize the measure-job results.json (fail-closed on a missing aggregate).
    let results_bytes = std::fs::read(&args.results)
        .map_err(|e| format!("--results read failed ({}): {e}", args.results.display()))?;
    let results = overlay::ResultsView::parse(&results_bytes)?;

    // R12 — the EXPECTED track the overlay was told to score: env `MLXFAST_QWEN_MTP_TRACK_ID` when
    // set (the same var the ranked yml passes as `$track`). When set, the merge REJECTS a
    // results.json sealed for a different track; when unset, the sealed track_id must still be
    // non-empty (never trust an arbitrary one).
    let expected_track = std::env::var("MLXFAST_QWEN_MTP_TRACK_ID")
        .ok()
        .filter(|s| !s.trim().is_empty());

    // W3 (§5) — the EXPECTED TIMED SERIES the overlay was told to score: env
    // `MLXFAST_QWEN_MTP_TIMED_SERIES` when set. §5 makes baselines/floors/bands PER-SERIES, so when
    // the operator states the series, a results.json sealed for a different one is REFUSED. When
    // unset, the fence still enforces the file's INTERNAL series coherence (known tags, per-pair
    // agreement, recomputed comparability) — it just does not pin which series is expected.
    let expected_series = std::env::var("MLXFAST_QWEN_MTP_TIMED_SERIES")
        .ok()
        .filter(|s| !s.trim().is_empty());

    // R17 — resolve the expected pool SHAPE fail-closed (env / --contract fixture length).
    let pool = resolve_pool_expectation(args.contract.as_deref())?;

    // The pure merge: validation is fail-closed inside merge_overlay.
    let outcome = overlay::merge_overlay(
        &gates,
        &results,
        expected_track.as_deref(),
        expected_series.as_deref(),
        &pool,
    )?;

    // Seal the ranked score.json (+ bare-basename `.sha256`) via the SHARED sealed-write, and use
    // the returned digest as the integrity re-anchor value (over the merged bytes).
    let score_sha256 = write_results_json(&args.score_path, &outcome.sealed_json)?;

    // Emit the sealed payload to STDOUT (parity with the iterate/benchmark.sh cat) — no trailing
    // newline; the write_results_json already wrote the exact bytes to disk.
    print!("{}", outcome.sealed_json);

    // Re-anchor integrity `score_sha256` over the merged bytes (GEMMA-OVL `@67699fc4:177-181`).
    // With --integrity, REWRITE the existing anchor's score_sha256/score_path in place (preserving
    // the measure-job's weights/workspace provenance); else write a fresh sidecar next to the score.
    reanchor_overlay_integrity(args, &score_sha256)?;

    eprintln!(
        "benchctl overlay-timing: wrote {} (passed={}, score={})",
        args.score_path.display(),
        outcome.passed,
        outcome
            .score
            .map(|s| s.to_string())
            .unwrap_or_else(|| "null".to_string()),
    );
    Ok(outcome.passed)
}

/// Reduce a filesystem path to a WORKSPACE-RELATIVE form before it is sealed into an integrity
/// artifact, so no operator home directory (`/Users/<home>/…`) travels with a run.
///
/// A sealed `benchmark-integrity.*.json` is an artifact and travels with the run, and the
/// secret-tier rule keeps home/box paths out of every sealed artifact. These path fields are
/// PROVENANCE only — the sha256 digests beside them carry identity, and no consumer resolves the
/// strings back to files (the sole reader, [`reanchor_overlay_integrity`], only rewrites the score
/// fields and preserves the rest) — so relativising changes what a human reads, never what a gate
/// checks.
///
/// A relative path is already leak-free and returned unchanged. An absolute path is made relative
/// to the run's current working directory (its workspace root) when it lies under it; otherwise the
/// operator's own `$HOME` prefix is stripped; and as a final guard any residual `/Users/<user>/` or
/// `/home/<user>/` head is dropped. The result therefore never begins with a user-home segment. A
/// path that is absolute but outside any home (e.g. `/opt/…`) is kept as-is — it carries no home to
/// leak.
fn relativize_for_seal(path: &Path) -> String {
    if path.is_relative() {
        return path.display().to_string();
    }
    // (1) workspace-relative: under the run's current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(rel) = path.strip_prefix(&cwd) {
            return rel_or_dot(rel);
        }
    }
    // (2) home-relative: strip the operator's own $HOME (drops the username with it).
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        if let Ok(rel) = path.strip_prefix(PathBuf::from(home)) {
            return rel_or_dot(rel);
        }
    }
    // (3) final guard: a foreign home ($HOME unset, or a path under another user's home) still must
    // not seal a `/Users/<user>/` or `/home/<user>/` head.
    drop_home_head(path)
}

/// `rel` rendered, or `"."` when stripping the prefix left it empty (the path WAS the prefix).
fn rel_or_dot(rel: &Path) -> String {
    if rel.as_os_str().is_empty() {
        ".".to_string()
    } else {
        rel.display().to_string()
    }
}

/// Drop a leading `/Users/<user>/` or `/home/<user>/` head, returning the remaining tail; the path
/// unchanged when it has no such head. The last safety net for [`relativize_for_seal`].
fn drop_home_head(path: &Path) -> String {
    use std::path::Component;
    let comps: Vec<Component> = path.components().collect();
    if comps.len() >= 3 {
        if let (Component::RootDir, Component::Normal(top), Component::Normal(_user)) =
            (&comps[0], &comps[1], &comps[2])
        {
            if *top == "Users" || *top == "home" {
                let tail: PathBuf = comps[3..].iter().collect();
                return rel_or_dot(&tail);
            }
        }
    }
    path.display().to_string()
}

/// Re-anchor the integrity `score_sha256` over the merged score bytes. When `--integrity` names an
/// existing sidecar (e.g. the measure-job's `benchmark-integrity.results.json`), its
/// `score_sha256`/`score_path` are rewritten in place and every other field is preserved. Absent,
/// a fresh minimal `benchmark-integrity.json` is written next to the ranked score.
fn reanchor_overlay_integrity(args: &OverlayTimingArgs, score_sha256: &str) -> Result<(), String> {
    // F-5 — relativise at seal: the score path is provenance and must not carry a home directory.
    let score_path_str = relativize_for_seal(&args.score_path);
    if let Some(integrity_path) = args.integrity.as_ref() {
        let bytes = std::fs::read(integrity_path).map_err(|e| {
            format!(
                "--integrity read failed ({}): {e}",
                integrity_path.display()
            )
        })?;
        let mut value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("--integrity parse failed: {e}"))?;
        let obj = value
            .as_object_mut()
            .ok_or("--integrity is not a JSON object")?;
        obj.insert(
            "score_sha256".to_string(),
            serde_json::Value::String(score_sha256.to_string()),
        );
        obj.insert(
            "score_path".to_string(),
            serde_json::Value::String(score_path_str),
        );
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| format!("--integrity serialization failed: {e}"))?;
        std::fs::write(integrity_path, format!("{json}\n"))
            .map_err(|e| format!("could not write {}: {e}", integrity_path.display()))?;
    } else {
        let path = args.score_path.with_file_name("benchmark-integrity.json");
        let sidecar = OverlayIntegrity {
            score_path: score_path_str,
            score_sha256: score_sha256.to_string(),
        };
        let json = serde_json::to_string_pretty(&sidecar)
            .map_err(|e| format!("integrity serialization failed: {e}"))?;
        std::fs::write(&path, format!("{json}\n"))
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// A fresh overlay integrity sidecar (when no `--integrity` to re-anchor is given): the ranked
/// score path + its sha256 over the merged bytes.
#[derive(serde::Serialize)]
struct OverlayIntegrity {
    score_path: String,
    score_sha256: String,
}

/// `gates_producer` when no driver declared one — a standalone `measure-job` with no seam 1.
///
/// A DECLARED sentinel rather than an empty string, for the #132/F3 reason: a reader must never
/// have to guess whether benchd failed to record the producer or there was genuinely none.
const GATES_PRODUCER_UNDECLARED: &str = "undeclared";

/// Validate a `--gates-producer` value before it is sealed into an artifact.
///
/// Deliberately NOT an allowlist of the three known producer names. measure-job does not own the
/// driver's producer vocabulary, and hardcoding it here would mean a new producer could not be
/// recorded without changing this file — the seal is PROVENANCE, not a policy gate.
///
/// What is enforced is that the value can be read back as what it claims to be: non-empty, and no
/// whitespace or control characters, so a declaration cannot smuggle a second field, a newline or a
/// terminal escape into a sealed record that a human or a parser later reads.
fn validate_gates_producer(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err(
            "--gates-producer must not be empty (omit the flag to record 'undeclared')".into(),
        );
    }
    if let Some(bad) = raw.chars().find(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "--gates-producer {raw:?} contains {bad:?}: the value is sealed into an artifact, \
             so whitespace and control characters are refused"
        ));
    }
    Ok(raw.to_string())
}

/// The measure-job integrity sidecar (finding 10): the sealed `results.json` digest lives
/// INSIDE this anchor, alongside the workspace/executable provenance.
#[derive(serde::Serialize)]
struct MeasureJobIntegrity {
    results_path: String,
    results_sha256: String,
    candidate_workspace: String,
    baseline_workspace: String,
    candidate_executable: String,
    baseline_executable: String,
    /// CANDIDATE-IDENTITY provenance: the sha256 of the candidate WORKSPACE tree (the built engine
    /// source). Distinct from the weights identity below.
    candidate_workspace_sha256: String,
    /// BASELINE-IDENTITY provenance (integrity-anchor minor): the sha256 of the baseline WORKSPACE
    /// tree, so the seal pins BOTH legs' built-engine sources, not only the candidate's.
    baseline_workspace_sha256: String,
    /// GOLDEN-IDENTITY provenance (integrity-anchor minor): the sha256 of the actual `--golden`
    /// bytes (== `GoldenFixture::sha256`) — the prompt oracle this run measured against.
    golden_sha256: String,
    /// CONTRACT-IDENTITY provenance (integrity-anchor minor): the sha256 of the `--contract` track
    /// fixture bytes — the thresholds/pool the run was configured from.
    contract_sha256: String,
    /// WEIGHTS-IDENTITY provenance: the `--weights` DIR and its digest (the transformed weights
    /// both legs load). `weights_sha256`/`weights_file_count`/`weights_byte_count` digest THIS dir,
    /// not the workspace.
    weights_dir: String,
    weights_sha256: String,
    weights_file_count: i64,
    weights_byte_count: i64,
    /// RULING Q1a — WHICH seam-1 gates producer made the gates this run was scored against
    /// (`benchmark-sh` = the organizer's reference chain and the default, `facade` = benchd's own
    /// `--official`, `direct-swift` = the weightless fallback), or
    /// [`GATES_PRODUCER_UNDECLARED`] when no driver declared one.
    ///
    /// WHY THIS ARTIFACT. The producer is a seam-1 fact, but seam 1's own output (`gates-score.json`)
    /// is written BY the producer — so it cannot be trusted to name itself. This sidecar is the
    /// first artifact in the chain that benchd writes and the driver anchors, which makes it the
    /// earliest honest home for the declaration.
    ///
    /// WHY IT MATTERS. The opt-in is an ENVIRONMENT variable, so an exported `GATES_PRODUCER=facade`
    /// can select the parity-test producer for a scoring run without appearing in any command line.
    /// Sealing it here does not prevent that — it makes it AUDITABLE after the fact, which is what
    /// lets the env-var opt-in stand instead of forcing an argv-only interface.
    gates_producer: String,
}

/// The RUN-DERIVED half of the measure-job integrity seal — everything measured or digested
/// during the run, as opposed to declared on the command line.
///
/// Separate from [`MeasureJobArgs`] so [`build_measure_job_integrity`] states exactly which fields
/// come from the operator and which come from the run.
struct MeasureJobSealInputs {
    results_path: String,
    results_sha256: String,
    candidate_executable: String,
    baseline_executable: String,
    candidate_workspace_sha256: String,
    baseline_workspace_sha256: String,
    golden_sha256: String,
    contract_sha256: String,
    weights_sha256: String,
    weights_file_count: i64,
    weights_byte_count: i64,
}

/// Assemble `benchmark-integrity.results.json` from the declared args + the run-derived values.
///
/// EXTRACTED FROM [`execute_measure_job`] DELIBERATELY. This is the only production code that puts
/// the declared `--gates-producer` into an artifact, and while it lived inline in
/// `execute_measure_job` — which needs a GPU, two workspaces and a real pair loop to reach — no
/// test could execute it. A mutation hardcoding the sealed producer left the whole workspace suite
/// AND the offline driver suite green, because the driver suite's sidecar comes from a bash stub
/// that re-implements the behaviour: stub agreeing with stub.
///
/// Pulling the assembly out makes the seal a pure function over its inputs, so the ruling-Q1a
/// audit trail is pinned by a test that runs the REAL line rather than a copy of it.
fn build_measure_job_integrity(
    args: &MeasureJobArgs,
    run: MeasureJobSealInputs,
) -> MeasureJobIntegrity {
    MeasureJobIntegrity {
        // F-5 — relativise every path at seal so the anchor carries no operator home directory.
        results_path: relativize_for_seal(Path::new(&run.results_path)),
        results_sha256: run.results_sha256,
        candidate_workspace: relativize_for_seal(&args.candidate),
        baseline_workspace: relativize_for_seal(&args.baseline),
        candidate_executable: relativize_for_seal(Path::new(&run.candidate_executable)),
        baseline_executable: relativize_for_seal(Path::new(&run.baseline_executable)),
        gates_producer: args.gates_producer.clone(),
        candidate_workspace_sha256: run.candidate_workspace_sha256,
        baseline_workspace_sha256: run.baseline_workspace_sha256,
        golden_sha256: run.golden_sha256,
        contract_sha256: run.contract_sha256,
        weights_dir: relativize_for_seal(&args.weights),
        weights_sha256: run.weights_sha256,
        weights_file_count: run.weights_file_count,
        weights_byte_count: run.weights_byte_count,
    }
}

/// Parsed `iterate` flags.
struct IterateArgs {
    engine: String,
    weights: PathBuf,
    golden: PathBuf,
    golden_pin: Option<GoldenIntegrityPin>,
    baseline_prefill_spt: Option<f64>,
    baseline_decode_spt: Option<f64>,
    mode: Mode,
    score_path: PathBuf,
    /// Tri-state local GPU cool gate (#60.3): `None` = per-mode default (local-iterate OFF,
    /// local-submit ON, official n/a); `Some(true)` = `--cool-gate` forces ON; `Some(false)` =
    /// `--no-cool-gate` forces OFF (needed to force submit's default-ON gate off). Per David's
    /// ruling (2026-08-17) the facade always passes `--cool-gate` to match benchmark.sh.
    cool_gate: Option<bool>,
    /// R3: opt into benchctl's correctness SUPERSET for local-iterate. Default OFF makes
    /// the gate Swift-exact (primary teacher-forced `cases[]` only); `--strict` also
    /// evaluates the golden's anchor/free-run gates. No effect on official mode.
    strict: bool,
}

fn run_iterate(args: &[String]) -> ExitCode {
    let parsed = match parse_iterate_args(args) {
        Ok(Some(p)) => p,
        Ok(None) => {
            print!("{ITERATE_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("benchctl iterate: {msg}");
            eprint!("{ITERATE_USAGE}");
            return ExitCode::from(2);
        }
    };

    match execute_iterate(&parsed) {
        // Score was written (it encodes any failure); a run that did not pass exits nonzero so
        // callers notice — including a paired REJECT / floor / ceiling / serial-band fail, all of
        // which set `passed = false`.
        Ok(passed) => ExitCode::from(iterate_exit_status(passed)),
        Err(msg) => {
            eprintln!("benchctl iterate: {msg}");
            ExitCode::from(1)
        }
    }
}

/// The boolean→process-exit contract for an iterate run: a passing run exits 0, a run that did
/// NOT pass (any fail-closed verdict — paired REJECT, floor/ceiling fail, serial-band breach)
/// exits 1 so callers notice. Extracted so the exact mapping is unit-testable (constructing a
/// real `ExitCode` end-to-end needs a live engine).
fn iterate_exit_status(passed: bool) -> u8 {
    if passed {
        0
    } else {
        1
    }
}

/// The LOCAL arm of an iterate run (`local-iterate` / `local-submit`): spawn the engine for
/// the correctness gate, wire the fresh-engine-per-timed-phase spawner and the per-mode cool
/// gate, and hand all of it to `iterate_core`.
///
/// #65: lifted out of `execute_iterate`'s baseline match. That match's job is choosing HOW a
/// run ends — preflight refusal, gates-only, official, or local — and this arm's engine
/// lifecycle detail buried the choice.
#[allow(clippy::too_many_arguments)]
fn run_local_iterate(
    args: &IterateArgs,
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    baseline_prefill: f64,
    baseline_decode: f64,
) -> Result<ScorePayload, String> {
    // Spawn the engine for the CORRECTNESS gate and connect (the shared session, §A).
    let weights_str = args.weights.to_string_lossy().to_string();
    let transport = ChildStdioTransport::spawn(&args.engine, &weights_str, &[])
        .map_err(|e| format!("failed to spawn engine {:?}: {e}", args.engine))?;
    let (mut session, hello) =
        Session::connect(transport).map_err(|e| format!("engine hello handshake failed: {e}"))?;

    // §A — local-iterate timing spawns a FRESH engine process per timed phase (Swift
    // prefillWorker/decodeWorker). Each call launches a new `runtime-worker` child and
    // completes the hello handshake; the hello is discarded (timing needs only the
    // session). A spawn/handshake failure surfaces as a RunnerError into the timing path.
    let spawn_timed = || -> Result<Session<ChildStdioTransport>, RunnerError> {
        let transport = ChildStdioTransport::spawn(&args.engine, &weights_str, &[])?;
        let (session, _hello) = Session::connect(transport)?;
        Ok(session)
    };

    // Per-mode cool-gate default (David 2026-08-17): local-iterate OFF unless
    // `--cool-gate`; local-submit ON; official never calls it. `--no-cool-gate` forces
    // OFF regardless of mode (#60.3 tri-state). Disabled → a no-op closure, so the gate
    // machinery stays wired for the facade (which always passes it).
    let cool_gate_enabled = args
        .cool_gate
        .unwrap_or_else(|| args.mode.cool_gate_on_by_default());
    let cool_gate_fn = move |phase: &str| -> Result<(), RunnerError> {
        if cool_gate_enabled {
            coolgate::cool_gate(phase)
        } else {
            Ok(())
        }
    };

    Ok(iterate_core(
        &mut session,
        &hello,
        golden,
        baseline_prefill,
        baseline_decode,
        args.mode,
        args.strict,
        digests,
        spawn_timed,
        cool_gate_fn,
    ))
}

fn execute_iterate(args: &IterateArgs) -> Result<bool, String> {
    // F1 — resolve the WORKSPACE HARNESS IDENTITY, fail-closed, before anything else happens.
    //
    // Every payload this run can produce seals `metrics.harness_hash` from this value (the single
    // `iterate::base_metrics` funnel), on ALL modes — official, official gates-only, local-iterate
    // and local-submit alike. Before F1 that field was a `String::new()` stub, and the seam-3
    // overlay correctly refuses to publish a gates-score with no harness identity, so no benchd
    // score could ever be published. The value is computed HERE, trusted-side, over the workspace
    // benchd drives — never read off the engine wire, because the worker is participant-built and
    // a wire-reported hash would be attacker-controlled.
    //
    // The workspace is the process CWD, which is exactly how the reference resolves its own nine
    // roots (`QwenRuntimePreflight.swift:44-56`: "this hash is only produced when the benchmark
    // process runs with CWD == the repo/workspace root") and how this file already treats the CWD
    // elsewhere (`relativize_for_seal`: "the run's current working directory (its workspace root)").
    //
    // FAIL-CLOSED, and resolved FIRST on purpose. The reference's fail-closed action is a
    // `fatalError` at seal time; benchd refuses the run instead, and refusing at the top means a
    // run that cannot be identified produces NO artifacts at all rather than dying after minutes of
    // weights hashing and correctness work. A missing root is named in the error. There is no path
    // from here that seals `""` or a partial hash.
    let harness = HarnessIdentity::resolve_from_current_dir().map_err(|e| {
        format!(
            "harness identity could not be resolved for this workspace ({e}); benchctl iterate \
             must run with its working directory at the engine workspace root, and refuses the run \
             rather than seal an empty or partial metrics.harness_hash"
        )
    })?;

    // Integrity-pin (when given) + load + validate the golden. The pin is checked on the
    // raw bytes BEFORE the parse, so a golden that does not match the caller's pin is
    // refused before benchd ever trusts its contents.
    //
    // The arity is MODE-PARAMETRIC, exactly as the reference's own load call is: Swift
    // `QwenRuntime.localIterate` demands `benchmarkDecodeSteps + 1` expected tokens (seed +
    // one per decode step) at LOAD time, so a golden that cannot cover this mode's decode
    // window is refused BEFORE the correctness gate runs and before the golden is hashed —
    // `case_count = 0`, `checked_steps = 0`, `golden_hash = ""`. Passing the flat
    // `CORRECTNESS_STEPS` here made benchd accept goldens the reference rejects.
    //
    // #114 — reference-model pin `None`: `iterate` has NO `--contract` surface, and the ruling put
    // the reference-model identity in the track contract, so this command has no pin to apply.
    // That is a SCOPED residual, not a hidden one — recorded as the
    // contract-less half of the #114 row (the ranked path, `measure-job`, always carries one).
    let golden = load_golden_checked(
        &args.golden,
        args.golden_pin.as_ref(),
        args.mode.golden_required_steps(),
        None,
    )?;

    // B-2: an OFFICIAL run FAILS CLOSED on any missing sandbox prerequisite (worker disabled,
    // MLXFAST_NO_SANDBOX=1, no engine exe, no derivable profile, no sandbox-exec) — resolved
    // UP FRONT, before any score is written (Swift `runtimeWorkerOptions` throws before the
    // worker spawns). Surfaced as a hard error (exit 1, no artifacts), verbatim the Swift
    // message. Local modes never resolve a sandbox.
    let official_sandbox: Option<OfficialSandboxPlan> = if args.mode == Mode::Official {
        Some(resolve_official_sandbox_from_env(
            &args.engine,
            &args.golden,
        )?)
    } else {
        None
    };

    // Digest the weights directory. #64: this runs AFTER the cheap arg/preflight validation
    // above, because it is the expensive step — a full recursive walk that streams every
    // safetensors byte through sha256 (tens of GB). Everything before it can hard-reject the
    // run (bad/unpinned golden, missing sandbox prerequisite), and there is no reason to spend
    // minutes hashing weights for a run that was never going to start. It stays BELOW the
    // sandbox resolution (Swift resolves `runtimeWorkerOptions` first) and ABOVE the §F2
    // baseline resolution, which is the reference's own order — on doubly-broken input Swift
    // reports the weights-digest failure, not the baseline one
    // (QwenRuntimeBenchmark.swift:406→428, QwenRuntimeLocalIterate.swift:77→100 @242b19d).
    // It also stays BEFORE the payload branch below, which needs the digest for both the
    // preflight-failed score and the real run.
    let weights_digest = dir_digest(&args.weights)
        .map_err(|e| format!("weights digest failed ({}): {e}", args.weights.display()))?;

    // The two digests benchd computes and seals for this run: WHICH WEIGHTS it measured and WHICH
    // HARNESS produced the result. Bundled once here so every payload builder below — passing,
    // failing, gates-only, preflight-refused — seals the same pair.
    let digests = RunDigests {
        weights: &weights_digest,
        harness: &harness,
    };

    // #123 — RULED (David 2026-08-20): pin the RUNNER, not just the inputs. Resolved here, before
    // anything spawns, so the sidecar names the binary that actually ran and an unreadable engine
    // refuses the run instead of surfacing after a score exists. On official the spawned binary is
    // the sandbox plan's, which `MLXFAST_RUNTIME_WORKER_EXECUTABLE` can redirect away from
    // `--engine`.
    let runner_engine = official_sandbox
        .as_ref()
        .map(|p| p.executable_path.clone())
        .unwrap_or_else(|| args.engine.clone());
    let runner = resolve_runner_identity(&runner_engine);

    // Gates-only OFFICIAL run (seam 1): MLXFAST_BENCHMARK_SKIP_TIMED=1 skips the timed phases and
    // runs ONLY the correctness gates, sealing a partial_result=true gates-score the paired
    // overlay later completes. This is benchd's parity of the reference mlxfast-swift SKIP_TIMED
    // path — env read at `main.swift@b26f76f:386` (`MLXFAST_BENCHMARK_SKIP_TIMED`, defaulting to
    // "1"), threaded into the options at `:397`, consumed at
    // `QwenRuntimeBenchmark.swift@b26f76f:457`. (#132/F-7: this used to cite `main.swift:321-322`,
    // which is the LOCAL branch's `QwenRuntime.localIterate` call — an unrelated code path.)
    // No timed phase runs here, so no paired baselines are resolved.
    let payload = if args.mode == Mode::Official && official_gates_only_from_env() {
        let plan = official_sandbox
            .as_ref()
            .expect("official sandbox resolved above for Mode::Official");
        let weights_str = args.weights.to_string_lossy().to_string();
        let commit_env = std::env::var("MLXFAST_COMMIT_SHA").ok();
        let commit = official::commit_identifier(commit_env.as_deref());
        let spawn_correctness = || -> bench_runner::Result<Session<ChildStdioTransport>> {
            let transport = ChildStdioTransport::spawn_official_sandboxed(plan, &weights_str, &[])?;
            let (session, _hello) = Session::connect(transport)?;
            Ok(session)
        };
        official::official_gates_only(&golden, digests, &commit, spawn_correctness)
    } else if let RunBaselines::Decided {
        prefill,
        decode,
        flags_ignored,
    } = run_baselines(
        args.mode,
        &golden,
        args.baseline_prefill_spt.zip(args.baseline_decode_spt),
    ) {
        eprintln!(
            "benchctl iterate: {} baseline prefill_seconds_per_token={prefill} \
             decode_seconds_per_token={decode} (official-runner constants; local \
             speedups are directional)",
            args.mode.mode_name()
        );
        if flags_ignored {
            eprintln!(
                "benchctl iterate: --baseline-prefill-spt/--baseline-decode-spt are IGNORED on \
                 {} (#127: local baselines come from the constants, as the reference's \
                 localIterate does); the run scores against the constants above",
                args.mode.mode_name()
            );
        }
        run_local_iterate(args, &golden, digests, prefill, decode)?
    } else {
        // §F2 (OFFICIAL only): resolve the REQUIRED paired baselines. Explicit --baseline flags
        // override; else the golden's benchmark must carry both. Missing → a preflight-failed
        // score, NO engine run (Swift throws during validation, before spawning the worker).
        let flag_override = args.baseline_prefill_spt.zip(args.baseline_decode_spt);
        // #61: the trusted `MLXFAST_PAIRED_BASELINE_*` env override takes precedence over the
        // flags (the measure-job contract: the reference is measured on the same runner
        // immediately before the candidate). Resolved FAIL-CLOSED on a half-set/invalid pair — a
        // hard error (exit 1, NO artifacts), verbatim the Swift message
        // (BenchmarkSupport.swift PairedBaselineOverride.fromEnvironment). Local modes never read
        // it, and since #127 they never reach this branch at all. (The old two-leg `--paired`
        // monolith that bypassed this is REMOVED — it is now the standalone `benchctl measure-job`
        // subcommand, seam 2.)
        let effective_override = official::paired_baseline_from_env(
            std::env::var("MLXFAST_PAIRED_BASELINE_PREFILL_SECONDS_PER_TOKEN")
                .ok()
                .as_deref(),
            std::env::var("MLXFAST_PAIRED_BASELINE_DECODE_SECONDS_PER_TOKEN")
                .ok()
                .as_deref(),
        )?
        .or(flag_override);
        match resolve_paired_baselines(effective_override, &golden) {
            None => iterate::preflight_failed_payload(
                args.mode,
                &golden,
                digests,
                iterate::missing_paired_baselines_error(args.mode),
            ),
            // B-2 OFFICIAL: timed-first, three fresh SANDBOXED workers, full correctness set,
            // benchmark-oracle checks, official floor/band/finite gating, and a stamped commit.
            // This is now the ONLY resolved arm — the local modes branch off above (#127).
            Some((baseline_prefill, baseline_decode)) => {
                let plan = official_sandbox
                    .as_ref()
                    .expect("official sandbox resolved above for Mode::Official");
                let weights_str = args.weights.to_string_lossy().to_string();
                // metrics.commit: valid-hex MLXFAST_COMMIT_SHA, else `git rev-parse --short HEAD`.
                let commit_env = std::env::var("MLXFAST_COMMIT_SHA").ok();
                let commit = official::commit_identifier(commit_env.as_deref());
                // Each phase spawns a FRESH worker under `sandbox-exec -f <profile>`, worker
                // stderr forwarding forced OFF (redacted + retained, never echoed). Two identical
                // closures — official_core takes the timed + correctness spawners separately (the
                // timed one is invoked twice: prefill worker, decode worker; correctness once).
                let spawn_timed = || -> bench_runner::Result<Session<ChildStdioTransport>> {
                    let transport =
                        ChildStdioTransport::spawn_official_sandboxed(plan, &weights_str, &[])?;
                    let (session, _hello) = Session::connect(transport)?;
                    Ok(session)
                };
                let spawn_correctness = || -> bench_runner::Result<Session<ChildStdioTransport>> {
                    let transport =
                        ChildStdioTransport::spawn_official_sandboxed(plan, &weights_str, &[])?;
                    let (session, _hello) = Session::connect(transport)?;
                    Ok(session)
                };
                official::official_core(
                    &golden,
                    baseline_prefill,
                    baseline_decode,
                    digests,
                    &commit,
                    spawn_timed,
                    spawn_correctness,
                )
            }
        }
    };

    // Write the sealed score + sha256 sidecar (benchd is the sole writer).
    let json = payload
        .to_sealed_json()
        .map_err(|e| format!("score serialization failed: {e}"))?;
    let score_sha256 = write_score(&args.score_path, &json)?;

    // Emit the sealed payload to STDOUT (Swift binary `emitScorePayloadToStdout`; benchmark.sh
    // `cat "${SCORE_PATH}"`) — no trailing newline, matching the on-disk bytes.
    print!("{json}");

    // Integrity sidecar (benchmark.sh benchmark-integrity.*.json). golden_sha256 =
    // sha256 of the raw golden bytes (== `shasum -a 256 GOLDEN`); transform_source_sha256 =
    // the `<weights>/.benchmark-source.sha256` marker content, or "" if the marker is absent
    // (benchctl cannot recompute the Swift source hash without the source tree).
    let golden_sha256 = sha256_hex(
        &std::fs::read(&args.golden)
            .map_err(|e| format!("golden re-read for integrity failed: {e}"))?,
    );
    let transform_source_sha256 =
        std::fs::read_to_string(args.weights.join(".benchmark-source.sha256"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
    write_integrity_sidecar(
        &args.score_path,
        args.mode,
        &IntegritySidecar {
            // F-5 — relativise every path at seal so the sidecar carries no operator home directory
            // (the same helper the measure-job anchor uses, so the two records agree).
            score_path: relativize_for_seal(&args.score_path),
            score_sha256,
            weights_path: relativize_for_seal(&args.weights),
            weights_sha256: weights_digest.sha256.clone(),
            weights_file_count: weights_digest.file_count,
            weights_byte_count: weights_digest.byte_count,
            golden_path: "[private]".to_string(),
            golden_sha256,
            transform_source_sha256,
            candidate_executable: relativize_for_seal(Path::new(&runner.candidate_executable)),
            candidate_executable_sha256: runner.candidate_executable_sha256,
            baseline_executable: String::new(),
            baseline_executable_sha256: String::new(),
            candidate_executable_resolution: runner.candidate_executable_resolution,
            benchd_executable: relativize_for_seal(Path::new(&runner.benchd_executable)),
            benchd_executable_sha256: runner.benchd_executable_sha256,
            candidate_workspace_sha256: String::new(),
        },
    )?;

    eprintln!(
        "benchctl iterate: wrote {} (passed={}, score={})",
        args.score_path.display(),
        payload.passed,
        payload
            .score
            .map(|s| s.to_string())
            .unwrap_or_else(|| "null".to_string())
    );
    Ok(payload.passed)
}

/// Where THIS run's baseline pair comes from — the #127 decision seam, in one place both the
/// runner and its test go through.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RunBaselines {
    /// The LOCAL legs: the pair is already decided, and it is the compile-time official-runner
    /// constants. Nothing downstream may reconsider it.
    Decided {
        prefill: f64,
        decode: f64,
        /// The caller passed `--baseline-*` and they are being ignored — worth saying out loud.
        flags_ignored: bool,
    },
    /// OFFICIAL: golden-authoritative, resolved downstream from the trusted env/flag override
    /// ahead of the golden's own declaration.
    ResolveFromOverrideOrGolden,
}

/// The #127 routing decision, extracted so it is TESTABLE rather than only observable through a
/// live engine run.
///
/// **Why it is its own function (#132/F2).** The ruling's whole content is *which source wins on
/// which leg*, and that lived inline in `execute_iterate` — reachable only by spawning an engine.
/// The merged test injected baselines into `iterate_core` directly, which exercises what the run
/// DOES with a pair, not where the pair came from: reverting the routing left the suite green.
/// Now the runner and `run_baselines_ignores_the_goldens_pair_on_the_local_legs` call the same
/// function, so a revert fails a test.
///
/// RULED (David 2026-08-20, "MIRROR REFERENCE"): the LOCAL legs take the constants, full stop.
/// The golden's declared `benchmark.baseline_*_seconds_per_token` is INERT LEGACY DATA here — not
/// cross-checked, not required, not consulted; a stale pair no longer refuses, it is simply
/// ignored. That mirrors the reference's `localIterate`, which reads
/// `MLXFastConstants.officialBaseline*` directly and never reaches the `resolvedBaseline*`
/// accessor (`QwenRuntimeLocalIterate.swift@b26f76f:34,36`, used at `:291,317,382,386`;
/// `grep -c resolvedBaseline` over BOTH harness copies of that file returns 0). BOTH local modes
/// take this path because the reference routes both through that one function — `--local-submit`
/// and `--local-iterate` differ only by decode steps, repeats and labels
/// (`main.swift@b26f76f:315-322`).
///
/// SCOPED DELIBERATELY: OFFICIAL stays golden-authoritative via `resolvedBaseline*`
/// (`Golden.swift@b26f76f:220-226`, consumed at `QwenRuntimeBenchmark.swift@b26f76f:155-157,443-445`).
/// The reference is asymmetric between its own two paths; the ruling mirrors that asymmetry rather
/// than fixing it.
///
/// `golden` is taken and deliberately unused on the local arm: the signature is the claim. A
/// future edit that wants the golden's pair back has to reach for it explicitly, in a function
/// whose doc comment says why it must not.
fn run_baselines(
    mode: Mode,
    _golden: &GoldenFixture,
    flag_override: Option<(f64, f64)>,
) -> RunBaselines {
    if mode.is_local_checked_timing() {
        let (prefill, decode) = iterate::local_mode_baselines();
        return RunBaselines::Decided {
            prefill,
            decode,
            flags_ignored: flag_override.is_some(),
        };
    }
    RunBaselines::ResolveFromOverrideOrGolden
}

/// §F2: resolve the REQUIRED paired baselines for an OFFICIAL run. `flag_override` (the
/// `MLXFAST_PAIRED_BASELINE_*` env pair, else both `--baseline-*` flags, already validated as a
/// pair at parse time) is a trusted override; otherwise the golden's benchmark must carry both.
/// `Some` when resolved, `None` when neither supplies them (→ a preflight-failed score).
///
/// **#127 (F8) — the old doc comment here was wrong and is corrected.** It justified the
/// requirement by appeal to Swift `requiredGoldenBenchmarkBaselines` ("no fallback to the official
/// constant"), which is a symbol of the RETIRED `mlxfast-challenge-dev` fork. The reference does
/// not work that way in either direction:
///
/// - its OFFICIAL path is golden-authoritative but FALLS BACK to the constants when the golden
///   declares no pair — `resolvedBaseline*` is `baseline* ?? officialBaseline*`
///   (`Golden.swift@b26f76f:220-226`), consumed at
///   `QwenRuntimeBenchmark.swift@b26f76f:155-157,443-445`;
/// - its LOCAL path never reads the golden's pair at all.
///
/// So this function is now OFFICIAL-ONLY (the local callers were removed by the #127 ruling), and
/// what it still enforces beyond the reference — refusing an official run whose golden declares no
/// pair and whose caller passed no override, where the reference would fall back to the constants
/// — is benchd being STRICTER on the ranked path, deliberately: the ranked runner is required to
/// measure its baseline in the same session (#61), so an official run with no pair in sight is a
/// missing measurement, not a cue to score against a cached constant. Recorded on #127 and NOT
/// changed by that ruling, which scoped itself to the local leg.
fn resolve_paired_baselines(
    flag_override: Option<(f64, f64)>,
    golden: &GoldenFixture,
) -> Option<(f64, f64)> {
    flag_override.or_else(|| {
        golden.benchmark.as_ref().and_then(|b| {
            match (
                b.baseline_prefill_seconds_per_token,
                b.baseline_decode_seconds_per_token,
            ) {
                (Some(p), Some(d)) => Some((p, d)),
                _ => None,
            }
        })
    })
}

/// Whether this OFFICIAL run is a gates-only (seam 1) run: `MLXFAST_BENCHMARK_SKIP_TIMED=1`
/// skips the timed phases so only the correctness gates run, producing a `partial_result=true`
/// gates-score. Mirrors the reference `mlxfast-swift` env contract
/// (`main.swift@b26f76f:386` reads it, defaulting to "1"; `:397` threads it into the options;
/// `QwenRuntimeBenchmark.swift@b26f76f:457` consumes it — #132/F-7 corrected this from
/// `main.swift:321-322`, which is the local branch); the
/// paired driver sets it alongside `MLXFAST_BENCHMARK_CHECK_GATES=1`. Only `"1"` is truthy.
fn official_gates_only_from_env() -> bool {
    std::env::var("MLXFAST_BENCHMARK_SKIP_TIMED")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Resolve the official-run Seatbelt sandbox from the process env + the engine/golden paths,
/// FAIL-CLOSED (Swift `runtimeWorkerOptions`, main.swift:1143-1219). Reads the `MLXFAST_*`
/// knobs, probes `/usr/bin/sandbox-exec` for executability, and returns the resolved plan or
/// the verbatim Swift refusal message. Official forces worker-stderr forwarding OFF.
fn resolve_official_sandbox_from_env(
    engine: &str,
    golden: &Path,
) -> Result<OfficialSandboxPlan, String> {
    let use_rw = std::env::var("MLXFAST_USE_RUNTIME_WORKER").ok();
    let no_sb = std::env::var("MLXFAST_NO_SANDBOX").ok();
    let exec_ov = std::env::var("MLXFAST_RUNTIME_WORKER_EXECUTABLE").ok();
    let prof_ov = std::env::var("MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE").ok();
    let priv_dir = std::env::var("MLXFAST_PRIVATE_DIR").ok();
    let golden_str = golden.to_string_lossy().to_string();
    let inputs = OfficialSandboxInputs {
        use_runtime_worker: use_rw.as_deref(),
        no_sandbox: no_sb.as_deref(),
        executable_override: exec_ov.as_deref(),
        profile_override: prof_ov.as_deref(),
        private_dir: priv_dir.as_deref(),
        fallback_executable: engine,
        golden_path: &golden_str,
        sandbox_exec_available: sandbox_exec_is_executable(),
    };
    // forwards_worker_stderr = false: official never echoes worker stderr (the plan also
    // forces it off, so this is belt-and-braces).
    resolve_official_sandbox(&inputs, false).map_err(|e| e.to_string())
}

/// Whether `/usr/bin/sandbox-exec` exists and is executable (Swift
/// `FileManager.isExecutableFile`). On unix, requires an execute bit; elsewhere, existence.
fn sandbox_exec_is_executable() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(SANDBOX_EXEC_PATH)
            .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(SANDBOX_EXEC_PATH)
            .map(|m| m.is_file())
            .unwrap_or(false)
    }
}

/// Build the optional integrity pin from CLI flag strings. Both or neither must be given.
fn parse_golden_pin(
    sha256: Option<String>,
    bytes: Option<String>,
) -> Result<Option<GoldenIntegrityPin>, String> {
    match (sha256, bytes) {
        (None, None) => Ok(None),
        (Some(sha256), Some(bytes)) => {
            let bytes: u64 = bytes
                .parse()
                .map_err(|_| format!("invalid u64 for --golden-bytes: {bytes:?}"))?;
            Ok(Some(GoldenIntegrityPin { sha256, bytes }))
        }
        _ => Err("--golden-sha256 and --golden-bytes must be given together".to_string()),
    }
}

/// Read + integrity-pin (when given) + load-validate a golden. The pin is checked on the
/// RAW BYTES BEFORE any parse (port of verify-correctness-golden.sh), so an unexpected,
/// tampered, or unknown-provenance golden fails closed before its contents are trusted.
///
/// `required_steps` is the caller's `expected_tokens` arity, because the REFERENCE's arity is
/// per-consumer, not global: `QwenRuntime.localIterate` loads with `benchmarkDecodeSteps + 1`
/// while the standalone `correctness`/`benchmark` paths take the `correctnessSteps` default
/// (see [`Mode::golden_required_steps`]).
fn load_golden_checked(
    path: &Path,
    pin: Option<&GoldenIntegrityPin>,
    required_steps: usize,
    reference_model: Option<&ReferenceModelPin>,
) -> Result<GoldenFixture, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("golden read failed ({}): {e}", path.display()))?;
    load_golden_bytes_checked(&bytes, pin, required_steps, reference_model)
}

/// #112 (L2) — the BYTES half of [`load_golden_checked`], for a caller that ALREADY HOLDS the
/// file's bytes. The pin, the sha256 the loader records, and the parsed document then all come
/// from ONE read of ONE byte string: a second `std::fs::read` of the same path could observe a
/// DIFFERENT file (a golden swapped between the two reads), which would let the identity checked
/// upstream and the document actually loaded diverge. The tape branch of
/// [`load_timed_prompt_checked`] already worked this way; this makes the GoldenDocument branch
/// match it.
fn load_golden_bytes_checked(
    bytes: &[u8],
    pin: Option<&GoldenIntegrityPin>,
    required_steps: usize,
    reference_model: Option<&ReferenceModelPin>,
) -> Result<GoldenFixture, String> {
    // The pin (when given) is now folded INTO the loader: a single call checks it on the raw
    // bytes BEFORE the parse, so this "load for use" path can no longer forget the pin. The
    // mismatch reason (byte-count / sha256) is emitted verbatim by the loader.
    //
    // #114 — `reference_model` is the SAME arrangement for the track contract's reference-model
    // identity: a caller that parsed a `--contract` hands its pin here and the loader applies
    // the reference's own value gate; a caller with no contract passes `None` and gets the
    // shape-only check.
    load_golden_fixture(
        bytes,
        required_steps,
        CORRECTNESS_PROMPT_TOKENS,
        Some(REQUIRED_GOLDEN_MODEL_TYPE),
        pin,
        reference_model,
    )
    .map_err(|e| format!("golden load failed: {e}"))
}

/// Per-shape counts of the loaded `--golden` set, for the `--preflight-only` line: an operator
/// proving satisfiability offline should be able to READ which shape was accepted, not infer it.
fn golden_kind_summary(prompts: &[measure_job::TimedPrompt]) -> String {
    let tapes = prompts
        .iter()
        .filter(|p| p.kind() == measure_job::PROMPT_KIND_TAPE)
        .count();
    format!(
        "{}={} {}={}",
        measure_job::PROMPT_KIND_TAPE,
        tapes,
        measure_job::PROMPT_KIND_GOLDEN,
        prompts.len() - tapes,
    )
}

/// Load ONE `measure-job --golden` as EITHER document shape, routed by REQUIRED-KEY SIGNATURE.
///
/// The live `timed_prompt_pool` pins TEACHER-FORCING TAPES (`{seed_tokens,
/// reference_seed_token, rows, reference_self_consistent, emitted_tokens}`), not
/// `GoldenDocument`s — so `--golden` accepts both, and the loader must decide which is in front
/// of it BEFORE parsing.
///
/// DETECTION IS BY REQUIRED-KEY SIGNATURE, never by "whatever parses first"
/// ([`bench_core::tape::classify_golden_input`]). The two signatures are disjoint — a tape must
/// carry `seed_tokens`/`reference_seed_token`/`rows`, a GoldenDocument must carry `cases`, and
/// both structs are `deny_unknown_fields`, so neither document can parse as the other — which is
/// why no `--golden-kind` flag is needed. Signature routing also keeps the DIAGNOSTIC honest: a
/// tape with one broken row is reported as a broken TAPE, naming the real defect, instead of
/// being retried as a GoldenDocument and reported as "unknown field `emitted_tokens`" (exactly
/// the misleading message the 20260819 window hit). A file matching NEITHER signature is
/// refused naming BOTH shapes rather than guessed at.
///
/// Both branches hash the RAW BYTES they were handed, so the identity R4 pins against the
/// contract pool is the same quantity for either shape.
///
/// #114 — `reference_model` is the track contract's declared reference-model identity. It only
/// bites on the GoldenDocument branch: a `model_provenance` block is a GoldenDocument key, and a
/// timed-prompt tape carries no model identity at all (its identity is the pool's sha256+bytes
/// pin). Passing it here rather than checking after the load keeps the reference's ordering — a
/// golden naming the wrong model is refused by the LOADER, not accepted and re-judged later.
fn load_timed_prompt_checked(
    path: &Path,
    reference_model: Option<&ReferenceModelPin>,
) -> Result<measure_job::TimedPrompt, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("golden read failed ({}): {e}", path.display()))?;
    match bench_core::tape::classify_golden_input(&bytes) {
        bench_core::tape::GoldenInputKind::TimedPromptTape => {
            // Pin `None` here: measure-job's pin is the contract's `timed_prompt_pool` (R4,
            // exactly-one, fail-closed), enforced on these same raw bytes right after loading.
            bench_core::tape::load_timed_prompt_tape(&bytes, None)
                .map(measure_job::TimedPrompt::Tape)
                .map_err(|e| format!("timed-prompt tape load failed ({}): {e}", path.display()))
        }
        // #112 (L2) — SINGLE-READ, like the tape branch above: the bytes are already in hand, so
        // the classification, the loader's sha256 and the parsed document all describe the SAME
        // read. (Pin `None` for the same reason as the tape branch: measure-job's pin is the
        // contract pool, enforced on these bytes right after loading.)
        // Arity: the loader DEFAULT (`CORRECTNESS_STEPS`) — a measure-job golden is a timed
        // prompt source, not a local-iterate checked-decode window.
        bench_core::tape::GoldenInputKind::GoldenDocument => {
            load_golden_bytes_checked(&bytes, None, CORRECTNESS_STEPS, reference_model)
                .map(measure_job::TimedPrompt::Golden)
                .map_err(|e| format!("{e} ({})", path.display()))
        }
        bench_core::tape::GoldenInputKind::Unrecognized => Err(format!(
            "--golden {} matches NEITHER accepted shape: a {} needs {:?}, a {} needs {:?} \
             (the live timed_prompt_pool pins tapes)",
            path.display(),
            measure_job::PROMPT_KIND_TAPE,
            bench_core::tape::TAPE_REQUIRED_KEYS,
            measure_job::PROMPT_KIND_GOLDEN,
            bench_core::tape::GOLDEN_DOCUMENT_REQUIRED_KEYS,
        )),
    }
}

/// `validate-golden`: integrity-pin + load-validate a golden, no engine. Exit 0 = accepted,
/// 1 = rejected (pin or schema), 2 = usage error. The loader-parity harness compares this
/// accept/reject against `mlxfast-swift preflight` on the same fixture corpus.
fn run_validate_golden(args: &[String]) -> ExitCode {
    let mut golden: Option<PathBuf> = None;
    let mut sha256: Option<String> = None;
    let mut bytes: Option<String> = None;
    let mut contract: Option<PathBuf> = None;
    let mut gates_only = false;
    let mut i = 0;
    while i < args.len() {
        let need = |i: usize, name: &str| -> Result<String, ExitCode> {
            args.get(i + 1).cloned().ok_or_else(|| {
                eprintln!("benchctl validate-golden: {name} requires a value");
                ExitCode::from(2)
            })
        };
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{VALIDATE_GOLDEN_USAGE}");
                return ExitCode::SUCCESS;
            }
            "--golden" => match need(i, "--golden") {
                Ok(v) => {
                    golden = Some(PathBuf::from(v));
                    i += 2;
                }
                Err(code) => return code,
            },
            "--golden-sha256" => match need(i, "--golden-sha256") {
                Ok(v) => {
                    sha256 = Some(v);
                    i += 2;
                }
                Err(code) => return code,
            },
            "--golden-bytes" => match need(i, "--golden-bytes") {
                Ok(v) => {
                    bytes = Some(v);
                    i += 2;
                }
                Err(code) => return code,
            },
            "--contract" => match need(i, "--contract") {
                Ok(v) => {
                    contract = Some(PathBuf::from(v));
                    i += 2;
                }
                Err(code) => return code,
            },
            "--gates-only" => {
                gates_only = true;
                i += 1;
            }
            other => {
                eprintln!("benchctl validate-golden: unknown flag {other:?}");
                eprint!("{VALIDATE_GOLDEN_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let golden = match golden {
        Some(g) => g,
        None => {
            eprintln!("benchctl validate-golden: missing required --golden");
            return ExitCode::from(2);
        }
    };
    let pin = match parse_golden_pin(sha256, bytes) {
        Ok(p) => p,
        Err(m) => {
            eprintln!("benchctl validate-golden: {m}");
            return ExitCode::from(2);
        }
    };
    // #114 — the track contract's declared reference-model identity, when a `--contract` was
    // given. A contract that cannot be READ is exit 3 (IO, same as an unreadable golden); one that
    // cannot be PARSED, or that declares only half a pin, is exit 2 — the caller's ARGUMENT is
    // invalid, which is not the same event as the golden being rejected, and collapsing the two
    // would let a broken contract read as "this golden is fine".
    let reference_model = match contract.as_ref() {
        None => None,
        Some(path) => match std::fs::read(path) {
            Err(e) => {
                eprintln!(
                    "benchctl validate-golden: IO ERROR reading contract {}: {e}",
                    path.display()
                );
                return ExitCode::from(3);
            }
            Ok(contract_bytes) => match reference_model_pin_from_contract(&contract_bytes) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "benchctl validate-golden: contract {} is unusable: {e}",
                        path.display()
                    );
                    return ExitCode::from(2);
                }
            },
        },
    };
    // Distinct exit codes so a harness can tell an IO failure apart from a rejection:
    //   0 = accepted, 1 = rejected (integrity pin or schema), 2 = usage, 3 = IO error.
    let bytes = match std::fs::read(&golden) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "benchctl validate-golden: IO ERROR reading {}: {e}",
                golden.display()
            );
            return ExitCode::from(3);
        }
    };
    // validate-golden keeps the pin as an EXPLICIT step (not folded into the loader) because it
    // has a bespoke contract the folded form cannot express: a distinct "(integrity pin)" reject
    // label vs "(schema)", and it already owns the raw bytes for the IO-vs-reject exit-code split
    // (3 = IO, 1 = reject). The loader is therefore called unpinned (`None`) here; the pin is
    // still enforced, just at this call site with richer diagnostics.
    if let Some(pin) = pin.as_ref() {
        if let Err(e) = verify_golden_integrity(&bytes, pin) {
            eprintln!(
                "benchctl validate-golden: REJECT {} (integrity pin): {e}",
                golden.display()
            );
            return ExitCode::from(1);
        }
    }
    // #114 — the reference-model pin, unlike the integrity pin above, is passed INTO the loader
    // rather than checked as a separate step: the reference applies it mid-load (after the shape
    // check, before the case validation), so folding it in keeps benchd's evaluation ORDER equal
    // to Swift's, and the reject carries the reference's own message. It is deliberately NOT given
    // its own reject label — Swift raises it as a plain loader `invalidInput` too, so labelling it
    // "(schema)" is what makes the two loaders' stderr comparable.
    match load_golden_fixture(
        &bytes,
        CORRECTNESS_STEPS,
        CORRECTNESS_PROMPT_TOKENS,
        Some(REQUIRED_GOLDEN_MODEL_TYPE),
        None,
        reference_model.as_ref(),
    ) {
        Ok(fx) => {
            // #77: by default a benchmark golden MUST carry a benchmark oracle block, byte-
            // consistent with Swift preflight (which rejects a benchmark-less golden with the
            // same message). `--gates-only` skips this for internal structural fixtures that
            // legitimately lack a benchmark oracle (structure + gates already validated above).
            if !gates_only && fx.benchmark.is_none() {
                eprintln!(
                    "benchctl validate-golden: REJECT {} (schema): benchmark golden file must contain a benchmark oracle",
                    golden.display()
                );
                return ExitCode::from(1);
            }
            eprintln!(
                "benchctl validate-golden: ACCEPT {} (sha256={}, cases={}, gate_cases={})",
                golden.display(),
                fx.sha256,
                fx.cases.len(),
                fx.correctness_gates
                    .as_ref()
                    .map(|g| g.total_case_count())
                    .unwrap_or(0)
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "benchctl validate-golden: REJECT {} (schema): {e}",
                golden.display()
            );
            ExitCode::from(1)
        }
    }
}

/// `correctness` (#90 item 2): spawn the engine, run the FULL correctness set, print a JSON
/// verdict, and exit 0 (pass) / 1 (fail). Byte-matches Swift `mlxfast-swift correctness`'s
/// exit contract. `2` on usage error. The golden is loaded oracle-optional (Swift
/// `checkCorrectnessArtifacts`).
fn run_correctness(args: &[String]) -> ExitCode {
    let mut engine: Option<String> = None;
    let mut weights: Option<PathBuf> = None;
    let mut golden: Option<PathBuf> = None;
    let mut sha256: Option<String> = None;
    let mut bytes: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let need = |i: usize, name: &str| -> Result<String, ExitCode> {
            args.get(i + 1).cloned().ok_or_else(|| {
                eprintln!("benchctl correctness: {name} requires a value");
                ExitCode::from(2)
            })
        };
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{CORRECTNESS_USAGE}");
                return ExitCode::SUCCESS;
            }
            "--engine" => match need(i, "--engine") {
                Ok(v) => {
                    engine = Some(v);
                    i += 2;
                }
                Err(c) => return c,
            },
            "--weights" => match need(i, "--weights") {
                Ok(v) => {
                    weights = Some(PathBuf::from(v));
                    i += 2;
                }
                Err(c) => return c,
            },
            "--golden" => match need(i, "--golden") {
                Ok(v) => {
                    golden = Some(PathBuf::from(v));
                    i += 2;
                }
                Err(c) => return c,
            },
            "--golden-sha256" => match need(i, "--golden-sha256") {
                Ok(v) => {
                    sha256 = Some(v);
                    i += 2;
                }
                Err(c) => return c,
            },
            "--golden-bytes" => match need(i, "--golden-bytes") {
                Ok(v) => {
                    bytes = Some(v);
                    i += 2;
                }
                Err(c) => return c,
            },
            other => {
                eprintln!("benchctl correctness: unknown flag {other:?}");
                eprint!("{CORRECTNESS_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (engine, weights, golden) = match (engine, weights, golden) {
        (Some(e), Some(w), Some(g)) => (e, w, g),
        _ => {
            eprintln!("benchctl correctness: --engine, --weights, and --golden are all required");
            eprint!("{CORRECTNESS_USAGE}");
            return ExitCode::from(2);
        }
    };
    let pin = match parse_golden_pin(sha256, bytes) {
        Ok(p) => p,
        Err(m) => {
            eprintln!("benchctl correctness: {m}");
            return ExitCode::from(2);
        }
    };
    // Load the golden ORACLE-OPTIONAL (Swift checkCorrectnessArtifacts): load_golden_checked
    // integrity-pins + structurally validates but does NOT require a benchmark oracle. Arity is
    // the loader DEFAULT here — Swift `correctness` calls `loadGoldenFixture(from:)` with no
    // `requiredSteps:` override (`QwenRuntimeCorrectness.swift:80`), i.e. `correctnessSteps`.
    // #114 — reference-model pin `None`: `correctness` takes no `--contract` (same scoped residual
    // as `iterate`).
    let golden_fx = match load_golden_checked(&golden, pin.as_ref(), CORRECTNESS_STEPS, None) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("benchctl correctness: {e}");
            return ExitCode::from(1);
        }
    };
    let weights_str = weights.to_string_lossy().to_string();
    let transport = match ChildStdioTransport::spawn(&engine, &weights_str, &[]) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("benchctl correctness: failed to spawn engine {engine:?}: {e}");
            return ExitCode::from(1);
        }
    };
    let (mut session, _hello) = match Session::connect(transport) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("benchctl correctness: engine hello handshake failed: {e}");
            return ExitCode::from(1);
        }
    };
    let outcome = correctness::correctness_core(&mut session, &golden_fx);
    // Emit the JSON verdict to stdout; the exit code is the authoritative pass/fail.
    print!("{}", outcome.to_json());
    eprintln!(
        "benchctl correctness: passed={} case_count={}{}",
        outcome.passed,
        outcome.case_count,
        if outcome.passed {
            String::new()
        } else {
            format!(" error={:?}", outcome.error)
        }
    );
    if outcome.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `validate-weights` (#90 item 3): the WEIGHTS-half preflight. Exit 0 = accepted,
/// 1 = rejected, 2 = usage, 3 = IO error (mirrors `validate-golden`'s distinct codes).
fn run_validate_weights(args: &[String]) -> ExitCode {
    let mut weights: Option<PathBuf> = None;
    let mut golden: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let need = |i: usize, name: &str| -> Result<String, ExitCode> {
            args.get(i + 1).cloned().ok_or_else(|| {
                eprintln!("benchctl validate-weights: {name} requires a value");
                ExitCode::from(2)
            })
        };
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{VALIDATE_WEIGHTS_USAGE}");
                return ExitCode::SUCCESS;
            }
            "--weights" => match need(i, "--weights") {
                Ok(v) => {
                    weights = Some(PathBuf::from(v));
                    i += 2;
                }
                Err(c) => return c,
            },
            "--golden" => match need(i, "--golden") {
                Ok(v) => {
                    golden = Some(PathBuf::from(v));
                    i += 2;
                }
                Err(c) => return c,
            },
            other => {
                eprintln!("benchctl validate-weights: unknown flag {other:?}");
                eprint!("{VALIDATE_WEIGHTS_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let weights = match weights {
        Some(w) => w,
        None => {
            eprintln!("benchctl validate-weights: missing required --weights");
            return ExitCode::from(2);
        }
    };
    // Resolve the size cap from MLXFAST_MAX_WEIGHTS_BYTES (Swift parseTransformedWeightsByteLimit).
    let cap = match weights_preflight::weights_byte_limit_from_env(
        std::env::var("MLXFAST_MAX_WEIGHTS_BYTES").ok().as_deref(),
    ) {
        Ok(c) => c,
        Err(m) => {
            eprintln!("benchctl validate-weights: {m}");
            return ExitCode::from(2);
        }
    };
    match weights_preflight::validate_weights(&weights, golden.as_deref(), cap) {
        Ok(report) => {
            eprintln!(
                "benchctl validate-weights: ACCEPT {} (bytes={}, files={}, cap={})",
                weights.display(),
                report.weights_byte_count,
                report.file_count,
                report
                    .max_weights_byte_count
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unlimited".to_string()),
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "benchctl validate-weights: REJECT {}: {e}",
                weights.display()
            );
            ExitCode::from(1)
        }
    }
}

/// Write the sealed score + its `.sha256` sidecar. Returns the score sha256 (for the
/// integrity sidecar). The sidecar format is the shasum two-space form
/// `"<hex>  <score_path>\n"`, byte-matching benchmark.sh
/// (`printf '%s  %s\n' "${score_hash}" "${SCORE_PATH}"`, benchmark.sh:1269-1270), not the
/// bare `"<hex>\n"` benchctl used before.
fn write_score(score_path: &Path, json: &str) -> Result<String, String> {
    if let Some(parent) = score_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(score_path, json.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", score_path.display()))?;
    let sidecar = score_path.with_file_name(format!(
        "{}.sha256",
        score_path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let digest = sha256_hex(json.as_bytes());
    std::fs::write(&sidecar, format!("{digest}  {}\n", score_path.display()))
        .map_err(|e| format!("could not write {}: {e}", sidecar.display()))?;
    Ok(digest)
}

/// The SHARED sealed-write for the measure-job `results.json` (+ `.sha256` sidecar), used by
/// `measure-job` (A-1, finding 14 — one sealed-write recipe, not a forked copy). UNLIKE the
/// [`write_score`] sidecar — whose BODY carries the full score path to byte-match benchmark.sh
/// (`printf '%s  %s\n' "${score_hash}" "${SCORE_PATH}"`, benchmark.sh:1269-1270), pinned by the
/// failing-run sidecar test — this sidecar uses the BARE BASENAME (`<hex>  results.json\n`), so
/// `shasum -c results.json.sha256` verifies the downloaded artifact in its own directory.
/// Returns the sealed digest so the caller can anchor it inside `benchmark-integrity` (finding 10).
fn write_results_json(results_path: &Path, json: &str) -> Result<String, String> {
    if let Some(parent) = results_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(results_path, json.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", results_path.display()))?;
    let basename = results_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let sidecar = results_path.with_file_name(format!("{basename}.sha256"));
    let digest = sha256_hex(json.as_bytes());
    std::fs::write(&sidecar, format!("{digest}  {basename}\n"))
        .map_err(|e| format!("could not write {}: {e}", sidecar.display()))?;
    Ok(digest)
}

/// The benchmark integrity sidecar (Swift/`benchmark.sh` `benchmark-integrity.*.json`,
/// benchmark.sh:1289-1308). `weights_*_count` are JSON numbers.
///
/// The first nine fields are the reference's, in the reference's `jq -n` order. Everything after
/// them is benchd's RUNNER-IDENTITY superset.
///
/// **#123 — RULED (David 2026-08-20): EXTEND THE SIDECAR.** The sidecar sealed the score, the
/// weights and the golden but named no executable, so the runner behind a local-iterate number was
/// not pinned and an E2 parity claim rested on inference (window 4: the engine binaries had been
/// rebuilt ~2h before the run, which makes them the overwhelmingly likely ones — but "likely" is
/// not a seal). Every `benchmark-integrity.results.json` the measure-job legs write DOES pin them;
/// this brings the local sidecar up to that bar. The issue recommended a separate
/// `benchmark-runners.*.json` to preserve the 9-field byte-match; David chose the superset, so
/// **the byte-match row is RETIRED and re-graded VERIFIED (superset)** —
/// the sidecar is a strict superset of the reference's object, not a byte-for-byte twin.
///
/// BACKWARD-READABLE by construction: the nine reference fields keep their names, types, order and
/// values, so a consumer that reads them still reads them. The only in-repo reader,
/// `reanchor_overlay_integrity`, parses to `serde_json::Value` and rewrites two keys, preserving
/// everything else — it carries the new fields through untouched.
#[derive(serde::Serialize)]
struct IntegritySidecar {
    score_path: String,
    score_sha256: String,
    weights_path: String,
    weights_sha256: String,
    weights_file_count: i64,
    weights_byte_count: i64,
    golden_path: String,
    golden_sha256: String,
    transform_source_sha256: String,
    /// The engine executable this run actually spawned, and the sha256 of its BYTES. Named as
    /// measure-job names it (`candidate_executable`) so one vocabulary covers both surfaces.
    /// Resolved and digested BEFORE the run, so the seal names the binary that ran and a
    /// missing/unreadable engine fails early rather than after the score exists.
    candidate_executable: String,
    candidate_executable_sha256: String,
    /// Empty by construction on this command, and empty is the ANSWER, not a gap: `iterate` runs
    /// ONE engine. There is no baseline leg to pin — on the local legs the baseline is the
    /// compile-time constant pair (#127), and on official it is the golden's declaration or the
    /// `MLXFAST_PAIRED_BASELINE_*` override, neither of which is a runner. A two-leg measurement
    /// pins its baseline runner in `benchmark-integrity.results.json` instead.
    baseline_executable: String,
    baseline_executable_sha256: String,
    /// Whether `candidate_executable` above is a canonical path with a real digest
    /// (`"canonical"`), or a path benchd could not resolve or read, recorded verbatim with an
    /// empty digest (`"unresolved"`) — #132/F3.
    ///
    /// This exists so an empty `candidate_executable_sha256` is never AMBIGUOUS. Without it, a
    /// reader cannot tell "benchd sealed no engine digest" from "the engine had no digest to
    /// seal", and the honest answer to a PATH-resolved bare name or an exec-but-not-readable
    /// binary is the second one. Sealing the weaker identity beats refusing the run: #123 exists
    /// to make artifacts say MORE about the runner, not to make runs that used to work fail.
    candidate_executable_resolution: String,
    /// benchd's own executable — the OTHER half of the runner identity, and the half no artifact
    /// pinned before. `metrics.commit` names a source revision; this names the binary.
    benchd_executable: String,
    benchd_executable_sha256: String,
    /// Empty on this command: `iterate` takes no workspace argument, so there is no built-engine
    /// SOURCE tree to digest — only the built binary above. Carried as a declared empty rather
    /// than omitted, so the field roster matches measure-job's and a consumer can tell "this run
    /// had no workspace" from "this run forgot to record one". Populating it would need a new
    /// `--candidate-workspace` flag, which the ruling did not ask for.
    candidate_workspace_sha256: String,
}

/// The runner identity a `benchctl iterate` run seals into its integrity sidecar (#123).
struct RunnerIdentity {
    candidate_executable: String,
    candidate_executable_sha256: String,
    candidate_executable_resolution: String,
    benchd_executable: String,
    benchd_executable_sha256: String,
}

/// `candidate_executable_resolution` when the engine path canonicalised AND read.
const ENGINE_RESOLUTION_CANONICAL: &str = "canonical";
/// `candidate_executable_resolution` when it did not: the path is sealed as GIVEN, with no digest.
const ENGINE_RESOLUTION_UNRESOLVED: &str = "unresolved";

/// Resolve + digest the executables behind an `iterate` run.
///
/// `engine` is the path this run will spawn: `args.engine` on the local legs, and the sandbox
/// plan's `executable_path` on official (where `MLXFAST_RUNTIME_WORKER_EXECUTABLE` can point
/// somewhere else entirely — sealing `args.engine` there would name a binary that never ran).
///
/// Paths are canonicalised so the seal is location-stable. This is TOTAL — it never fails the run.
///
/// **#132/F3 — it used to.** The first cut hard-errored when `canonicalize` or `read` failed on
/// the engine, reasoning that an unreadable engine is a run about to fail anyway. That reasoning
/// is wrong for two shapes that worked before #123 and stopped working after it, both of them
/// pre-run, before anything was even attempted:
///
/// * a bare name like `mlxfast-engine`, resolved by the spawner through `PATH` — `canonicalize`
///   resolves against the CWD, not `PATH`, so it fails on a name the run would have spawned fine;
/// * a binary that is executable but not readable by this user — `spawn` needs `--x--x--x`,
///   `read` needs `r`.
///
/// Neither is a broken run, and #123 exists to make artifacts say MORE about the runner, not to
/// turn working invocations into exit 1. So an unresolvable engine now seals the WEAKER identity
/// — the path exactly as given, no digest — and `candidate_executable_resolution` says which of
/// the two it is, so an empty digest is never ambiguous. If the engine really is broken, the
/// spawn a moment later reports it, in the words that actually describe it.
///
/// benchd's OWN path is best-effort for the same reason: `current_exe` can legitimately fail (a
/// deleted or relinked image), and losing a run over the identity of the binary that is reporting
/// it would be the wrong trade.
fn resolve_runner_identity(engine: &str) -> RunnerIdentity {
    let resolved = std::fs::canonicalize(engine)
        .ok()
        .and_then(|p| std::fs::read(&p).ok().map(|bytes| (p, bytes)));
    let (candidate_executable, candidate_executable_sha256, candidate_executable_resolution) =
        match resolved {
            Some((path, bytes)) => (
                path.display().to_string(),
                sha256_hex(&bytes),
                ENGINE_RESOLUTION_CANONICAL.to_string(),
            ),
            None => (
                engine.to_string(),
                String::new(),
                ENGINE_RESOLUTION_UNRESOLVED.to_string(),
            ),
        };
    let (benchd_executable, benchd_executable_sha256) = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .ok()
        .map(|p| {
            let sha = std::fs::read(&p)
                .map(|b| sha256_hex(&b))
                .unwrap_or_default();
            (p.display().to_string(), sha)
        })
        .unwrap_or_default();
    RunnerIdentity {
        candidate_executable,
        candidate_executable_sha256,
        candidate_executable_resolution,
        benchd_executable,
        benchd_executable_sha256,
    }
}

/// Write the integrity sidecar next to the score, named as benchmark.sh does per mode
/// (`benchmark-integrity.local-iterate.json` for local-iterate ONLY; `benchmark-integrity.json`
/// for local-submit and official). `jq -n` emits pretty (2-space) + a trailing newline; serde_json pretty
/// matches the 2-space form, and we append the newline.
fn write_integrity_sidecar(
    score_path: &Path,
    mode: Mode,
    sidecar: &IntegritySidecar,
) -> Result<(), String> {
    // Only local-ITERATE gets the `.local-iterate` suffix (benchmark.sh:135-137). local-SUBMIT
    // writes the DEFAULT `benchmark-integrity.json` (benchmark.sh:92-95); official likewise.
    let name = match mode {
        Mode::LocalIterate => "benchmark-integrity.local-iterate.json",
        Mode::LocalSubmit | Mode::Official => "benchmark-integrity.json",
    };
    let path = score_path.with_file_name(name);
    let json = serde_json::to_string_pretty(sidecar)
        .map_err(|e| format!("integrity serialization failed: {e}"))?;
    std::fs::write(&path, format!("{json}\n"))
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(())
}

/// Parse `iterate` flags. `Ok(None)` means `--help` was requested.
fn parse_iterate_args(args: &[String]) -> Result<Option<IterateArgs>, String> {
    let mut engine: Option<String> = None;
    let mut weights: Option<PathBuf> = None;
    let mut golden: Option<PathBuf> = None;
    let mut baseline_prefill_spt: Option<f64> = None;
    let mut baseline_decode_spt: Option<f64> = None;
    let mut mode = Mode::LocalIterate;
    let mut score_path: Option<PathBuf> = None;
    let mut golden_sha256: Option<String> = None;
    let mut golden_bytes: Option<String> = None;
    let mut cool_gate: Option<bool> = None;
    let mut strict = false;

    // A flag that needs a value reads args[i+1] and advances the index by 2.
    fn value<'a>(args: &'a [String], i: usize, name: &str) -> Result<&'a str, String> {
        args.get(i + 1)
            .map(|s| s.as_str())
            .ok_or_else(|| format!("flag {name} requires a value"))
    }

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(None),
            "--engine" => {
                engine = Some(value(args, i, "--engine")?.to_string());
                i += 2;
            }
            "--weights" => {
                weights = Some(PathBuf::from(value(args, i, "--weights")?));
                i += 2;
            }
            "--golden" => {
                golden = Some(PathBuf::from(value(args, i, "--golden")?));
                i += 2;
            }
            "--baseline-prefill-spt" => {
                let v = value(args, i, "--baseline-prefill-spt")?;
                baseline_prefill_spt = Some(
                    v.parse()
                        .map_err(|_| format!("invalid f64 for --baseline-prefill-spt: {v:?}"))?,
                );
                i += 2;
            }
            "--baseline-decode-spt" => {
                let v = value(args, i, "--baseline-decode-spt")?;
                baseline_decode_spt = Some(
                    v.parse()
                        .map_err(|_| format!("invalid f64 for --baseline-decode-spt: {v:?}"))?,
                );
                i += 2;
            }
            "--mode" => {
                let v = value(args, i, "--mode")?;
                mode = Mode::parse(v).ok_or_else(|| {
                    format!(
                        "invalid --mode {v:?} (expected local-iterate, local-submit, or official)"
                    )
                })?;
                i += 2;
            }
            "--score-path" => {
                score_path = Some(PathBuf::from(value(args, i, "--score-path")?));
                i += 2;
            }
            "--golden-sha256" => {
                golden_sha256 = Some(value(args, i, "--golden-sha256")?.to_string());
                i += 2;
            }
            "--golden-bytes" => {
                golden_bytes = Some(value(args, i, "--golden-bytes")?.to_string());
                i += 2;
            }
            "--cool-gate" => {
                if cool_gate == Some(false) {
                    return Err("--cool-gate conflicts with --no-cool-gate".to_string());
                }
                cool_gate = Some(true);
                i += 1;
            }
            "--no-cool-gate" => {
                if cool_gate == Some(true) {
                    return Err("--no-cool-gate conflicts with --cool-gate".to_string());
                }
                cool_gate = Some(false);
                i += 1;
            }
            "--strict" => {
                strict = true;
                i += 1;
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }

    // Baselines are a PAIRED override (Swift `MLXFAST_PAIRED_BASELINE_*` must be provided
    // together); a lone `--baseline-*` is a usage error (exit 2 + usage). CONTRACT CHANGE:
    // pre-PR a single `--baseline-*` flag was accepted (it paired with a golden/constant
    // fallback); F2 removed the fallback, so the pair is now required together.
    if baseline_prefill_spt.is_some() != baseline_decode_spt.is_some() {
        return Err(
            "--baseline-prefill-spt and --baseline-decode-spt must be given together".to_string(),
        );
    }

    // (The old `--paired` / `--baseline-engine` two-leg monolith flags are REMOVED — the paired
    // flow is now the standalone `benchctl measure-job` subcommand, seam 2.)

    let engine = engine.ok_or("missing required --engine")?;
    let weights = weights.ok_or("missing required --weights")?;
    let golden = golden.ok_or("missing required --golden")?;
    let golden_pin = parse_golden_pin(golden_sha256, golden_bytes)?;
    // Default score name mirrors benchmark.sh: local-ITERATE writes `score.local-iterate.json`
    // (benchmark.sh:92-95); local-SUBMIT writes the DEFAULT `score.json`, as does official.
    let score_path = score_path.unwrap_or_else(|| {
        PathBuf::from(match mode {
            Mode::LocalIterate => "score.local-iterate.json",
            Mode::LocalSubmit | Mode::Official => "score.json",
        })
    });

    Ok(Some(IterateArgs {
        engine,
        weights,
        golden,
        golden_pin,
        baseline_prefill_spt,
        baseline_decode_spt,
        mode,
        score_path,
        cool_gate,
        strict,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M-3 — FAILING-RUN ARTIFACT COMPLETENESS. A run that fails (correctness or preflight:
    /// `passed=false`, `score=null`) must write the SAME artifact set as a passing run —
    /// `score.json`, its `.sha256` sidecar, AND the `benchmark-integrity.*.json` — byte-shaped
    /// identically; only the payload's `passed`/`score` differ. This locks the invariant that
    /// benchctl's fail path reaches the same `write_score`/`write_integrity_sidecar` as the
    /// pass path (no early-return skips an artifact), matching the Swift reference: Swift
    /// `localIterate` catches every failure into a `failedScore` payload (never throws), so
    /// `writeScorePayload`+`emitScorePayloadToStdout` always run and the binary exits 0; then
    /// `benchmark.sh` writes BOTH sidecars UNCONDITIONALLY (`:1269-1270` `.sha256`, `:1289-1308`
    /// integrity JSON) and only THEN exits 1 on `.passed != true` (`:1310-1312`). The failing
    /// artifacts are written before that exit, exactly as benchctl writes them before returning
    /// `Ok(false)` → exit 1.
    #[test]
    fn failing_iterate_writes_full_byte_shaped_artifact_set() {
        use crate::iterate::{preflight_failed_payload, DirDigest, Mode};
        use crate::score::ScorePayload;

        // A genuine FAILING run: the §F2 missing-baseline PREFLIGHT failure (score=null,
        // passed=false, passed_correctness=false) — the same payload execute_iterate feeds to
        // the common writers when the golden carries no baselines and no --baseline flags.
        // A benchmark-less golden: this test only needs a loadable fixture to hang the
        // preflight refusal on, and that refusal is precisely "no baselines".
        let doc = crate::testgolden::TestGolden::new().without_benchmark();
        let bytes = doc.bytes();
        let golden = doc.fixture();
        let weights = DirDigest::empty();
        let failing = preflight_failed_payload(
            Mode::LocalIterate,
            &golden,
            RunDigests::for_test(&weights),
            "local-iterate requires external Qwen benchmark baselines".to_string(),
        );
        assert!(!failing.passed, "precondition: this is a FAILING payload");
        assert!(
            failing.score.is_none(),
            "precondition: a failing run has no score"
        );

        // A passing-shaped twin (same metrics, flipped verdict) proves the SAME three
        // filenames are produced whether the run passed or failed — the completeness claim.
        let passing = ScorePayload {
            score: Some(1.5),
            passed: true,
            metrics: failing.metrics.clone(),
        };

        // Exercise the EXACT common write sequence execute_iterate runs after building the
        // payload: to_sealed_json -> write_score (score.json + .sha256) -> write_integrity_sidecar.
        let write_artifacts = |payload: &ScorePayload, dir: &Path| -> (PathBuf, String) {
            let score_path = dir.join("score.local-iterate.json");
            let json = payload.to_sealed_json().unwrap();
            let score_sha256 = write_score(&score_path, &json).unwrap();
            let golden_sha256 = sha256_hex(&bytes);
            write_integrity_sidecar(
                &score_path,
                Mode::LocalIterate,
                &IntegritySidecar {
                    score_path: score_path.display().to_string(),
                    score_sha256: score_sha256.clone(),
                    weights_path: "weights".to_string(),
                    weights_sha256: weights.sha256.clone(),
                    weights_file_count: weights.file_count,
                    weights_byte_count: weights.byte_count,
                    golden_path: "[private]".to_string(),
                    golden_sha256,
                    transform_source_sha256: String::new(),
                    candidate_executable: "engine".to_string(),
                    candidate_executable_sha256: "e0".to_string(),
                    baseline_executable: String::new(),
                    baseline_executable_sha256: String::new(),
                    candidate_executable_resolution: ENGINE_RESOLUTION_CANONICAL.to_string(),
                    benchd_executable: "benchctl".to_string(),
                    benchd_executable_sha256: "b0".to_string(),
                    candidate_workspace_sha256: String::new(),
                },
            )
            .unwrap();
            (score_path, json)
        };

        let fail_dir = std::env::temp_dir().join(format!(
            "benchctl-m3-fail-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let pass_dir = std::env::temp_dir().join(format!(
            "benchctl-m3-pass-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&fail_dir);
        let _ = std::fs::remove_dir_all(&pass_dir);
        std::fs::create_dir_all(&fail_dir).unwrap();
        std::fs::create_dir_all(&pass_dir).unwrap();

        let (fail_score_path, fail_json) = write_artifacts(&failing, &fail_dir);
        let (_pass_score_path, _pass_json) = write_artifacts(&passing, &pass_dir);

        // The three artifact filenames present after a FAILING run.
        let score = fail_dir.join("score.local-iterate.json");
        let sidecar = fail_dir.join("score.local-iterate.json.sha256");
        let integrity = fail_dir.join("benchmark-integrity.local-iterate.json");
        assert!(score.exists(), "FAILING run must still write score.json");
        assert!(
            sidecar.exists(),
            "FAILING run must still write the .sha256 sidecar"
        );
        assert!(
            integrity.exists(),
            "FAILING run must still write the integrity JSON sidecar"
        );

        // Completeness: the passing twin writes the SAME three filenames — no artifact is
        // skipped on failure (nor added).
        let names_in = |dir: &Path| -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            names_in(&fail_dir),
            names_in(&pass_dir),
            "a FAILING run must write the identical artifact SET as a passing run"
        );

        // 1. score.json bytes == the sealed JSON, and it encodes the failure verbatim.
        let on_disk = std::fs::read_to_string(&score).unwrap();
        assert_eq!(
            on_disk, fail_json,
            "score.json is the sealed JSON byte-for-byte"
        );
        assert!(
            on_disk.contains("\"passed\": false"),
            "failing score.json records passed=false"
        );
        assert!(
            on_disk.contains("\"score\": null"),
            "failing score.json records score=null"
        );

        // 2. The .sha256 sidecar is the shasum two-space form `<hex>  <score_path>\n` over the
        //    ON-DISK score bytes (byte-matching benchmark.sh:1269-1270), unchanged by failure.
        let sidecar_bytes = std::fs::read_to_string(&sidecar).unwrap();
        let expected_hex = sha256_hex(fail_json.as_bytes());
        assert_eq!(
            sidecar_bytes,
            format!("{expected_hex}  {}\n", fail_score_path.display()),
            "failing .sha256 is `<hex>  <path>\\n` over the score bytes"
        );

        // 3. The integrity sidecar is well-formed on failure: exactly the 9 fields, in order,
        //    and score_sha256 pins the SAME digest the .sha256 sidecar carries.
        let integrity_bytes = std::fs::read_to_string(&integrity).unwrap();
        assert!(
            integrity_bytes.ends_with("}\n"),
            "integrity JSON ends with the jq trailing newline"
        );
        // The 9 fields appear in benchmark.sh insertion order (a parsed Value would re-sort,
        // so assert order over the raw serialized bytes).
        let field_order = [
            "score_path",
            "score_sha256",
            "weights_path",
            "weights_sha256",
            "weights_file_count",
            "weights_byte_count",
            "golden_path",
            "golden_sha256",
            "transform_source_sha256",
        ];
        let mut cursor = 0usize;
        for field in field_order {
            let needle = format!("\"{field}\":");
            let at = integrity_bytes[cursor..].find(&needle).unwrap_or_else(|| {
                panic!("failing integrity sidecar missing {field} in benchmark.sh order")
            });
            cursor += at + needle.len();
        }
        let v: serde_json::Value = serde_json::from_str(&integrity_bytes).unwrap();
        let obj = v.as_object().unwrap();
        // #123: the 9 reference fields + the runner-identity roster (8 since #132/F3 added the
        // resolution sentinel). The roster itself is pinned by
        // `integrity_sidecar_is_a_superset_of_the_jq_pretty_object`, and its LENGTH is read from
        // the same single-source file rather than restated, so this count cannot drift from it.
        // The claim HERE is only that a FAILING run writes the same shape a passing one does.
        const ROSTER_DOC: &str =
            include_str!("../../../scripts/fixtures/integrity-runner-keys.json");
        let roster_len = serde_json::from_str::<serde_json::Value>(ROSTER_DOC).unwrap()["keys"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(
            obj.len(),
            9 + roster_len,
            "failing integrity sidecar must carry the 9 reference fields + the full runner roster"
        );
        assert_eq!(
            obj["score_sha256"].as_str().unwrap(),
            expected_hex,
            "integrity score_sha256 matches the score bytes' digest on a failing run"
        );

        let _ = std::fs::remove_dir_all(&fail_dir);
        let _ = std::fs::remove_dir_all(&pass_dir);
    }

    /// #123 (RULED David 2026-08-20, EXTEND THE SIDECAR) — the sidecar is now a strict SUPERSET
    /// of `benchmark.sh`'s `jq -n` object, not a byte-for-byte twin, and the old byte-match test
    /// is re-graded to say exactly that.
    ///
    /// The claim it still enforces is the one the superset has to keep: the reference's nine
    /// fields come FIRST, in the reference's `jq -n` order, with the reference's names, types and
    /// values — so the reference's object is a literal PREFIX of benchd's bytes, and a consumer
    /// reading those nine reads them unchanged. What was a whole-document equality is now a
    /// prefix equality plus an exhaustive roster for the extension, so neither half can drift
    /// silently: a reordered/renamed reference field breaks the prefix, and an extra field nobody
    /// declared breaks the roster.
    #[test]
    fn integrity_sidecar_is_a_superset_of_the_jq_pretty_object() {
        let s = IntegritySidecar {
            score_path: "score.local-iterate.json".into(),
            score_sha256: "aaa".into(),
            weights_path: "weights".into(),
            weights_sha256: "fde4f615".into(),
            weights_file_count: 14,
            weights_byte_count: 15_159_954_417,
            golden_path: "[private]".into(),
            golden_sha256: "32045f7e".into(),
            transform_source_sha256: String::new(),
            candidate_executable: "/opt/engine/mlxfast-engine".into(),
            candidate_executable_sha256: "c0ffee".into(),
            baseline_executable: String::new(),
            baseline_executable_sha256: String::new(),
            candidate_executable_resolution: ENGINE_RESOLUTION_CANONICAL.into(),
            benchd_executable: "/opt/bin/benchctl".into(),
            benchd_executable_sha256: "b0b0".into(),
            candidate_workspace_sha256: String::new(),
        };
        let got = format!("{}\n", serde_json::to_string_pretty(&s).unwrap());

        // The reference's object, verbatim, minus its closing brace — benchd's bytes must open
        // with exactly this.
        let reference_prefix = "{\n  \"score_path\": \"score.local-iterate.json\",\n  \"score_sha256\": \"aaa\",\n  \"weights_path\": \"weights\",\n  \"weights_sha256\": \"fde4f615\",\n  \"weights_file_count\": 14,\n  \"weights_byte_count\": 15159954417,\n  \"golden_path\": \"[private]\",\n  \"golden_sha256\": \"32045f7e\",\n  \"transform_source_sha256\": \"\"";
        assert!(
            got.starts_with(reference_prefix),
            "the reference's 9 fields must remain a byte-exact PREFIX of the sidecar; got:\n{got}"
        );

        // …and the extension is exactly the declared runner-identity roster, nothing more, in
        // order. Read off the EMITTED BYTES rather than a parsed map: `serde_json::Value` sorts
        // its keys, which would silently discard the very ordering this test exists to hold.
        let keys: Vec<&str> = got
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"'))
            .filter_map(|l| l.split_once("\":"))
            .map(|(k, _)| k)
            .collect();
        const REFERENCE_KEYS: [&str; 9] = [
            "score_path",
            "score_sha256",
            "weights_path",
            "weights_sha256",
            "weights_file_count",
            "weights_byte_count",
            "golden_path",
            "golden_sha256",
            "transform_source_sha256",
        ];
        assert_eq!(
            &keys[..REFERENCE_KEYS.len()],
            &REFERENCE_KEYS[..],
            "the reference's 9 fields changed name or order"
        );

        // C3 — the runner roster is NOT restated here. It is read from the SAME file the two live
        // parity legs read (`facade-leg.sh`, `official-parity.sh`), so the shell and Rust encodings
        // cannot drift apart: a key added to the struct but not the roster fails here, and a key
        // added to the roster but not the struct fails here too. What this test still states in its
        // own words is the ORDER, which the roster file deliberately does not describe.
        const ROSTER: &str = include_str!("../../../scripts/fixtures/integrity-runner-keys.json");
        let roster_doc: serde_json::Value = serde_json::from_str(ROSTER).unwrap();
        let roster: Vec<String> = roster_doc["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        // #132/F6 — the file documents itself as SORTED, because both shell legs compare it
        // against `jq 'keys'` output, which is sorted. That was documentation only; an unsorted
        // edit would have broken the live legs at a distance instead of here.
        let mut sorted = roster.clone();
        sorted.sort();
        assert_eq!(
            roster, sorted,
            "scripts/fixtures/integrity-runner-keys.json must stay sorted — the shell legs \
             compare it against jq 'keys' output, which is"
        );
        let mut surplus: Vec<String> = keys[REFERENCE_KEYS.len()..]
            .iter()
            .map(|k| k.to_string())
            .collect();
        surplus.sort();
        assert_eq!(
            surplus, roster,
            "the sidecar's runner roster and scripts/fixtures/integrity-runner-keys.json disagree — \
             the live parity legs read that file, so this is the shell check going out of sync"
        );
        assert_eq!(
            keys.len(),
            REFERENCE_KEYS.len() + roster.len(),
            "the sidecar roster changed without the parity-matrix row changing with it"
        );

        // Backward-readability, mechanically: the only in-repo reader round-trips through
        // `serde_json::Value` and rewrites two keys. Prove the extension survives that.
        let mut round: serde_json::Value = serde_json::from_str(&got).unwrap();
        let obj = round.as_object_mut().unwrap();
        obj.insert("score_sha256".into(), serde_json::json!("rewritten"));
        obj.insert("score_path".into(), serde_json::json!("elsewhere.json"));
        let after = serde_json::to_string_pretty(&round).unwrap();
        let after: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            after["candidate_executable_sha256"],
            serde_json::json!("c0ffee")
        );
        assert_eq!(
            after["benchd_executable"],
            serde_json::json!("/opt/bin/benchctl")
        );
        assert_eq!(after["weights_sha256"], serde_json::json!("fde4f615"));
    }

    #[test]
    fn resolve_paired_baselines_requires_golden_or_flags() {
        // §F2: a golden must carry both benchmark baselines (or explicit flags override);
        // otherwise resolution yields None → a preflight-failed score (Swift's behavior).
        let golden = |with_baselines: bool| {
            let g = crate::testgolden::TestGolden::new();
            let g = if with_baselines {
                g.baselines(0.01, 0.1)
            } else {
                g
            };
            g.fixture()
        };
        let with = golden(true);
        let without = golden(false);
        // Golden carries the paired baselines → resolved from the golden (no override).
        assert_eq!(resolve_paired_baselines(None, &with), Some((0.01, 0.1)));
        // Golden lacks them, no override → None (preflight fail; no official-constant fallback).
        assert_eq!(resolve_paired_baselines(None, &without), None);
        // An explicit paired override wins even over a baseline-less golden.
        assert_eq!(
            resolve_paired_baselines(Some((0.02, 0.2)), &without),
            Some((0.02, 0.2))
        );
    }

    #[test]
    fn half_set_baseline_flags_are_a_usage_error() {
        // Review fix: a lone --baseline-* is a USAGE error at parse (→ exit 2 + usage), not a
        // silent exit-1. Both-together or neither is fine.
        let mk = |extra: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = ["--engine", "e", "--weights", "w", "--golden", "g"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            v.extend(extra.iter().map(|s| s.to_string()));
            v
        };
        assert!(parse_iterate_args(&mk(&["--baseline-prefill-spt", "0.01"])).is_err());
        assert!(parse_iterate_args(&mk(&["--baseline-decode-spt", "0.1"])).is_err());
        assert!(parse_iterate_args(&mk(&[
            "--baseline-prefill-spt",
            "0.01",
            "--baseline-decode-spt",
            "0.1"
        ]))
        .unwrap()
        .is_some());
        assert!(parse_iterate_args(&mk(&[])).unwrap().is_some());
    }

    /// #132/F2 — the #127 ruling AT ITS DECISION SEAM.
    ///
    /// The merged #127 test injected baselines into `iterate_core` directly, which proves what a
    /// run does with a pair, not where the pair came from — reverting the routing in
    /// `execute_iterate` left the suite green. This one calls the SAME function the runner
    /// calls, with a golden that declares the retired fork's pair AND an explicit flag override,
    /// and asserts neither reaches the local legs. Route local back through the golden and it
    /// fails.
    #[test]
    fn run_baselines_ignores_the_goldens_pair_on_the_local_legs() {
        use crate::iterate::Mode;
        const CAPTURE: &str =
            include_str!("../tests/fixtures/swift-official-baseline-constants.json");
        let capture: serde_json::Value = serde_json::from_str(CAPTURE).unwrap();
        let stale_prefill = capture["retired_fork"]["officialBaselinePrefillSecondsPerToken"]
            .as_f64()
            .unwrap();
        let stale_decode = capture["retired_fork"]["officialBaselineDecodeSecondsPerToken"]
            .as_f64()
            .unwrap();
        let want_prefill = capture["reference"]["officialBaselinePrefillSecondsPerToken"]
            .as_f64()
            .unwrap();
        let want_decode = capture["reference"]["officialBaselineDecodeSecondsPerToken"]
            .as_f64()
            .unwrap();

        // A golden that declares the exact pair which split the §8 window.
        let golden = crate::testgolden::TestGolden::new()
            .baselines(stale_prefill, stale_decode)
            .fixture();
        assert_eq!(
            resolve_paired_baselines(None, &golden),
            Some((stale_prefill, stale_decode)),
            "precondition: the OFFICIAL resolver really would have taken the golden's pair"
        );

        // BOTH local modes ignore it — and ignore an explicit flag override too, which the
        // reference has no counterpart for on a local run.
        for mode in [Mode::LocalIterate, Mode::LocalSubmit] {
            for flags in [None, Some((0.5_f64, 0.6_f64))] {
                match run_baselines(mode, &golden, flags) {
                    RunBaselines::Decided {
                        prefill,
                        decode,
                        flags_ignored,
                    } => {
                        assert_eq!(
                            (prefill, decode),
                            (want_prefill, want_decode),
                            "{} took a pair that is not the reference's constants",
                            mode.mode_name()
                        );
                        assert_ne!(prefill, stale_prefill, "{}", mode.mode_name());
                        assert_ne!(decode, stale_decode, "{}", mode.mode_name());
                        if let Some((fp, fd)) = flags {
                            assert_ne!(prefill, fp, "{}: a --baseline flag won", mode.mode_name());
                            assert_ne!(decode, fd, "{}: a --baseline flag won", mode.mode_name());
                        }
                        assert_eq!(
                            flags_ignored,
                            flags.is_some(),
                            "{}: the ignored-flags notice does not match reality",
                            mode.mode_name()
                        );
                    }
                    other => panic!(
                        "{} must not defer to the golden/override resolver, got {other:?}",
                        mode.mode_name()
                    ),
                }
            }
        }

        // OFFICIAL is the other half of the ruled asymmetry and must still defer.
        assert_eq!(
            run_baselines(Mode::Official, &golden, None),
            RunBaselines::ResolveFromOverrideOrGolden,
            "official must stay golden-authoritative (#127 scoped itself to the local leg)"
        );
    }

    /// #132/F3 — the strong direction: a real, readable engine seals a FULL identity.
    #[test]
    fn runner_identity_seals_a_canonical_engine_in_full() {
        let dir = std::env::temp_dir().join(format!(
            "benchctl-f3-ok-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let engine = dir.join("engine-bin");
        let bytes = b"#!/bin/sh\nexit 0\n";
        std::fs::write(&engine, bytes).unwrap();

        // Deliberately a NON-canonical spelling of the same file, so the test proves
        // canonicalisation rather than string passthrough.
        let noisy = dir.join(".").join("engine-bin").display().to_string();
        let id = resolve_runner_identity(&noisy);

        assert_eq!(
            id.candidate_executable_resolution,
            ENGINE_RESOLUTION_CANONICAL
        );
        assert_eq!(id.candidate_executable_sha256, sha256_hex(bytes));
        assert_eq!(
            id.candidate_executable,
            std::fs::canonicalize(&engine)
                .unwrap()
                .display()
                .to_string(),
            "the seal must name the canonical path, not the spelling the caller used"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #132/F3 — the direction that used to kill the run: an engine benchd cannot canonicalise or
    /// read seals the WEAKER identity and the run PROCEEDS.
    ///
    /// This covers the two shapes that worked before #123 and broke after it — a bare name the
    /// spawner would have resolved through `PATH` (`canonicalize` resolves against the CWD, not
    /// `PATH`), and an executable-but-not-readable binary (`spawn` needs `--x`, `read` needs `r`).
    /// The function is TOTAL, which is the "it still runs" claim: there is no error type left for
    /// it to abort `execute_iterate` with.
    #[test]
    fn runner_identity_falls_back_to_a_sentinel_instead_of_failing_the_run() {
        // A bare name that is not a path relative to the CWD — the PATH-resolved shape.
        let bare = "mlxfast-engine-that-is-not-in-this-directory";
        let id = resolve_runner_identity(bare);
        assert_eq!(
            id.candidate_executable_resolution, ENGINE_RESOLUTION_UNRESOLVED,
            "an unresolvable engine must be SEALED as unresolved, not silently blank"
        );
        assert_eq!(
            id.candidate_executable, bare,
            "the path is sealed exactly as the caller gave it — that is the identity we have"
        );
        assert!(
            id.candidate_executable_sha256.is_empty(),
            "no digest exists, so none is fabricated"
        );

        // An executable-but-unreadable file, on the platforms where that is expressible.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!(
                "benchctl-f3-noread-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let engine = dir.join("engine-bin");
            std::fs::write(&engine, b"x").unwrap();
            std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o111)).unwrap();
            // Skip rather than fail when running as root, where the read succeeds regardless.
            if std::fs::read(&engine).is_err() {
                let id = resolve_runner_identity(&engine.display().to_string());
                assert_eq!(
                    id.candidate_executable_resolution, ENGINE_RESOLUTION_UNRESOLVED,
                    "an exec-but-not-readable engine must seal the sentinel, not abort the run"
                );
                assert!(id.candidate_executable_sha256.is_empty());
            }
            let _ = std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o644));
            let _ = std::fs::remove_dir_all(&dir);
        }

        // The sentinel is a DISTINCT value, so an empty digest is never ambiguous.
        assert_ne!(ENGINE_RESOLUTION_UNRESOLVED, ENGINE_RESOLUTION_CANONICAL);
    }

    #[test]
    fn preflight_failed_payload_is_swift_shaped() {
        use crate::iterate::{DirDigest, Mode};
        let golden = crate::testgolden::TestGolden::new()
            .without_benchmark()
            .fixture();
        let err = crate::iterate::missing_paired_baselines_error(Mode::LocalIterate);
        let p = crate::iterate::preflight_failed_payload(
            Mode::LocalIterate,
            &golden,
            RunDigests::for_test(&DirDigest::empty()),
            err.clone(),
        );
        assert!(!p.passed);
        assert!(p.score.is_none());
        assert!(!p.metrics.passed_correctness);
        // #74 (RULED 2026-08-20): the reference's early-refuse record carries the official
        // baseline CONSTANTS, not zeros — `failedScore`'s baseline parameters default to them
        // and the local refusal site overrides neither. (The pre-ruling assertion pinned 0.0,
        // which described the RETIRED fork.) What the record must be in full is pinned by
        // `iterate::tests::early_refuse_record_byte_matches_the_reference_capture`.
        assert_eq!(
            p.metrics.baseline_prefill_seconds_per_token,
            bench_core::constants::OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN
        );
        assert_eq!(
            p.metrics.baseline_decode_seconds_per_token,
            bench_core::constants::OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN
        );
        // The two fields #74 names: nothing ran, so no run is described.
        assert_eq!(p.metrics.golden_hash, "");
        assert_eq!(p.metrics.case_count, 0);
        assert_eq!(p.metrics.checked_steps, 0);
        // #62: the payload carries the message IN FULL. What that message must BE is pinned
        // separately, against the Swift capture, by
        // `iterate::tests::missing_paired_baselines_error_matches_swift_capture` — this
        // assertion would otherwise just restate whatever benchd happens to produce.
        assert_eq!(p.metrics.error, err);
    }

    #[test]
    fn cool_gate_flag_defaults_off_and_opts_in() {
        let base: Vec<String> = ["--engine", "e", "--weights", "w", "--golden", "g"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Tri-state (#60.3): unset → None (per-mode default decides).
        assert_eq!(parse_iterate_args(&base).unwrap().unwrap().cool_gate, None);
        // --cool-gate forces ON.
        let mut with = base.clone();
        with.push("--cool-gate".to_string());
        assert_eq!(
            parse_iterate_args(&with).unwrap().unwrap().cool_gate,
            Some(true)
        );
        // --no-cool-gate forces OFF (overrides submit's default-ON).
        let mut without = base.clone();
        without.push("--no-cool-gate".to_string());
        assert_eq!(
            parse_iterate_args(&without).unwrap().unwrap().cool_gate,
            Some(false)
        );
        // Conflicting flags are rejected (either order).
        let mut both = base.clone();
        both.push("--cool-gate".to_string());
        both.push("--no-cool-gate".to_string());
        assert!(parse_iterate_args(&both).is_err());
        // Per-mode default: local-iterate OFF, official OFF (no cool gate on official),
        // local-submit ON (P6 RULING).
        assert!(!Mode::LocalIterate.cool_gate_on_by_default());
        assert!(!Mode::Official.cool_gate_on_by_default());
        assert!(Mode::LocalSubmit.cool_gate_on_by_default());
    }

    #[test]
    fn local_submit_defaults_to_plain_score_json_and_integrity() {
        // M-6 NAMING: local-submit writes the DEFAULT `score.json` (+ the default
        // `benchmark-integrity.json` sidecar), NOT the `.local-iterate`-suffixed names —
        // only local-iterate carries that suffix (benchmark.sh:92-95,135-137).
        let ok = parse_iterate_args(&[
            "--engine".into(),
            "e".into(),
            "--weights".into(),
            "w".into(),
            "--golden".into(),
            "g".into(),
            "--mode".into(),
            "local-submit".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(ok.mode, Mode::LocalSubmit);
        assert_eq!(ok.score_path, PathBuf::from("score.json"));
        // local-iterate keeps the suffixed default; contrast the two.
        let iter = parse_iterate_args(&[
            "--engine".into(),
            "e".into(),
            "--weights".into(),
            "w".into(),
            "--golden".into(),
            "g".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(iter.score_path, PathBuf::from("score.local-iterate.json"));
    }

    #[test]
    fn strict_flag_defaults_off_and_opts_in() {
        let base: Vec<String> = ["--engine", "e", "--weights", "w", "--golden", "g"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // R3: correctness is Swift-exact (base cases only) by DEFAULT.
        assert!(!parse_iterate_args(&base).unwrap().unwrap().strict);
        // --strict opts into the anchor/free-run superset.
        let mut with = base.clone();
        with.push("--strict".to_string());
        assert!(parse_iterate_args(&with).unwrap().unwrap().strict);
    }

    #[test]
    fn parse_requires_engine_weights_golden() {
        assert!(parse_iterate_args(&["--engine".into(), "e".into()]).is_err());
        let ok = parse_iterate_args(&[
            "--engine".into(),
            "e".into(),
            "--weights".into(),
            "w".into(),
            "--golden".into(),
            "g".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(ok.engine, "e");
        assert_eq!(ok.mode, Mode::LocalIterate);
        assert_eq!(ok.score_path, PathBuf::from("score.local-iterate.json"));
    }

    #[test]
    fn parse_help_returns_none() {
        assert!(parse_iterate_args(&["--help".into()]).unwrap().is_none());
    }

    #[test]
    fn parse_mode_and_baselines() {
        let ok = parse_iterate_args(&[
            "--engine".into(),
            "e".into(),
            "--weights".into(),
            "w".into(),
            "--golden".into(),
            "g".into(),
            "--mode".into(),
            "official".into(),
            // Baselines are a paired override now (contract change) — pass both.
            "--baseline-prefill-spt".into(),
            "0.01".into(),
            "--baseline-decode-spt".into(),
            "0.13".into(),
            "--score-path".into(),
            "out/score.json".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(ok.mode, Mode::Official);
        assert_eq!(ok.baseline_prefill_spt, Some(0.01));
        assert_eq!(ok.baseline_decode_spt, Some(0.13));
        assert_eq!(ok.score_path, PathBuf::from("out/score.json"));
    }

    #[test]
    fn golden_pin_requires_both_or_neither() {
        assert!(parse_golden_pin(None, None).unwrap().is_none());
        let pin = parse_golden_pin(Some("abc".into()), Some("10".into()))
            .unwrap()
            .unwrap();
        assert_eq!(pin.bytes, 10);
        // exactly one -> error; non-numeric bytes -> error.
        assert!(parse_golden_pin(Some("abc".into()), None).is_err());
        assert!(parse_golden_pin(None, Some("10".into())).is_err());
        assert!(parse_golden_pin(Some("abc".into()), Some("nan".into())).is_err());
    }

    #[test]
    fn iterate_parses_golden_pin_flags() {
        let ok = parse_iterate_args(&[
            "--engine".into(),
            "e".into(),
            "--weights".into(),
            "w".into(),
            "--golden".into(),
            "g".into(),
            "--golden-sha256".into(),
            "deadbeef".into(),
            "--golden-bytes".into(),
            "42".into(),
        ])
        .unwrap()
        .unwrap();
        let pin = ok.golden_pin.unwrap();
        assert_eq!(pin.sha256, "deadbeef");
        assert_eq!(pin.bytes, 42);
    }

    /// RULING Q1a — the seam-1 producer must be SEALED into `benchmark-integrity.results.json`.
    ///
    /// This is the conformance floor that lets the opt-in stay an ENV VAR. An exported
    /// `GATES_PRODUCER=facade` selects the parity-test producer without appearing on any command
    /// line; sealing the declaration here does not stop that, it makes it auditable afterwards.
    #[test]
    fn measure_job_integrity_seals_the_declared_gates_producer() {
        // Split from one string rather than a compact array literal: rustfmt explodes the array
        // form one element per line, which is unreadable for a 22-token argv.
        let base: Vec<String> = "--candidate cand --baseline base --weights w --golden g \
             --contract c --tokens 128 --mtp-depth 2 --min-pairs 3 --target-pairs 4 \
             --tag trk --out o"
            .split_whitespace()
            .map(str::to_string)
            .collect();

        // Omitted → the DECLARED sentinel, never an empty string (#132/F3: empty must not be
        // ambiguous between "benchd forgot" and "there was genuinely no seam 1").
        let defaulted = parse_measure_job_args(&base).unwrap().unwrap();
        assert_eq!(defaulted.gates_producer, GATES_PRODUCER_UNDECLARED);
        assert!(!defaulted.gates_producer.is_empty());

        // Declared → carried verbatim, for every producer the driver can select.
        for producer in ["benchmark-sh", "facade", "direct-swift"] {
            let mut argv = base.clone();
            argv.push("--gates-producer".to_string());
            argv.push(producer.to_string());
            let parsed = parse_measure_job_args(&argv).unwrap().unwrap();
            assert_eq!(parsed.gates_producer, producer);
        }

        // NOT an allowlist: measure-job does not own the driver's vocabulary, so an unknown
        // producer is RECORDED, not refused. The seal is provenance, not a policy gate.
        let mut unknown = base.clone();
        unknown.push("--gates-producer".to_string());
        unknown.push("some-future-producer".to_string());
        let recorded = parse_measure_job_args(&unknown).unwrap().unwrap();
        assert_eq!(recorded.gates_producer, "some-future-producer");

        // What IS refused: anything that could smuggle a second field, a newline or a terminal
        // escape into a sealed record a human or a parser later reads.
        for bad in [
            "",
            "bench mark-sh",
            "facade\n",
            "facade\u{1b}[31m",
            "\tfacade",
        ] {
            let mut argv = base.clone();
            argv.push("--gates-producer".to_string());
            argv.push(bad.to_string());
            assert!(
                parse_measure_job_args(&argv).is_err(),
                "a sealable producer must reject {bad:?}"
            );
        }
    }

    /// N3 — the REAL seal path, not a re-implementation of it.
    ///
    /// The parse test and the key-set test above pin different things; neither executes the
    /// production line that copies the declared producer into the artifact. That line lived inside
    /// `execute_measure_job` (GPU, two workspaces, a real pair loop), so hardcoding it left the
    /// workspace suite AND the offline driver suite green — the driver suite's sidecar comes from a
    /// bash stub re-implementing the behaviour, so that was stub agreeing with stub.
    ///
    /// This drives args through the REAL parser and the REAL [`build_measure_job_integrity`], then
    /// reads the field back off the SERIALIZED bytes — the same bytes the driver's seam-2 check
    /// parses. Mutating the seal line fails here, by name.
    #[test]
    fn the_real_seal_path_writes_the_declared_producer_into_the_artifact() {
        fn seal_with(flag: Option<&str>) -> serde_json::Value {
            let mut argv: Vec<String> = "--candidate cand --baseline base --weights w --golden g \
                 --contract c --min-pairs 1 --target-pairs 1 --tag trk --out o"
                .split_whitespace()
                .map(str::to_string)
                .collect();
            if let Some(p) = flag {
                argv.push("--gates-producer".to_string());
                argv.push(p.to_string());
            }
            let args = parse_measure_job_args(&argv)
                .expect("args parse")
                .expect("args present");
            let sidecar = build_measure_job_integrity(
                &args,
                MeasureJobSealInputs {
                    results_path: "o/results.json".into(),
                    results_sha256: "rs".into(),
                    candidate_executable: "ce".into(),
                    baseline_executable: "be".into(),
                    candidate_workspace_sha256: "cws".into(),
                    baseline_workspace_sha256: "bws".into(),
                    golden_sha256: "gs".into(),
                    contract_sha256: "cs".into(),
                    weights_sha256: "ws".into(),
                    weights_file_count: 1,
                    weights_byte_count: 2,
                },
            );
            let bytes = serde_json::to_string(&sidecar).expect("sidecar serializes");
            serde_json::from_str(&bytes).expect("sidecar round-trips")
        }

        // Every producer the driver can select must arrive in the artifact UNCHANGED. A hardcoded
        // seal passes for at most one of these.
        for producer in ["benchmark-sh", "facade", "direct-swift"] {
            assert_eq!(
                seal_with(Some(producer))["gates_producer"].as_str(),
                Some(producer),
                "the sealed producer must be the DECLARED one, not a constant"
            );
        }
        // The no-flag path seals the sentinel through the same line.
        assert_eq!(
            seal_with(None)["gates_producer"].as_str(),
            Some(GATES_PRODUCER_UNDECLARED)
        );
        // Not bought by breaking a neighbour: an args-derived and a run-derived field either side.
        let sealed = seal_with(Some("facade"));
        assert_eq!(sealed["candidate_workspace"].as_str(), Some("cand"));
        assert_eq!(sealed["results_sha256"].as_str(), Some("rs"));
    }

    // ------------------------------------------------------------------------------------------
    // F-5 — path relativisation at seal (no operator home directory in any sealed artifact).
    // ------------------------------------------------------------------------------------------

    /// The engine of the fix. A relative path is untouched; an absolute path under $HOME is reduced
    /// to its home-relative tail (dropping the username); a foreign `/Users/<u>/` head is dropped by
    /// the final guard; a non-home absolute path is kept. RED if the helper is reverted to a plain
    /// `.display()`.
    #[test]
    fn relativize_for_seal_strips_home_but_keeps_relative_and_foreign_paths() {
        // Relative stays byte-for-byte (the common CI shape: `--candidate candidate`).
        assert_eq!(relativize_for_seal(Path::new("candidate/x")), "candidate/x");

        // Under the operator's own $HOME → home-relative, no `/Users/<home>`.
        temp_env_home("/Users/operator", || {
            let out = relativize_for_seal(Path::new("/Users/operator/ws/candidate"));
            assert_eq!(out, "ws/candidate");
            assert!(!out.contains("/Users/"), "home stripped: {out}");
        });

        // A foreign home ($HOME elsewhere) still must not seal a `/Users/<user>/` head.
        temp_env_home("/Users/operator", || {
            let out = relativize_for_seal(Path::new("/Users/someoneelse/models/qwen"));
            assert_eq!(out, "models/qwen");
            assert!(!out.contains("/Users/"), "foreign home head dropped: {out}");
        });
        temp_env_home("/home/operator", || {
            let out = relativize_for_seal(Path::new("/home/other/w"));
            assert_eq!(out, "w");
        });

        // Absolute but outside any home carries nothing to leak, so it is left intact.
        temp_env_home("/Users/operator", || {
            assert_eq!(
                relativize_for_seal(Path::new("/opt/weights")),
                "/opt/weights"
            );
        });
    }

    /// RULING C — the STRUCTURAL property this side of the parity relies on: every home-shaped input
    /// reduces to a RELATIVE, leak-free string (no leading `/`, no `/Users/`, no `/home/`). These are
    /// the exact divergence vectors the re-review named; the shell mirror is asserted over the SAME
    /// vectors in test-official-offline.sh. C requires both impls leak-free, NOT byte-equal — so
    /// basename-comparing `weights_path` across the two sidecars can never diverge, and neither seals
    /// a home path. The foreign home ROOT (`/Users/<other>`, no trailing component) is the mandatory
    /// leak case: it must reduce to `.`, never survive as an absolute `/Users/<other>`.
    #[test]
    fn relativize_for_seal_reduces_every_home_shaped_input_to_relative_and_leakfree() {
        fn is_relative_leakfree(s: &str) -> bool {
            !s.starts_with('/') && !s.contains("/Users/") && !s.contains("/home/")
        }
        temp_env_home("/Users/operator", || {
            // (trailing slash, $HOME-exact, foreign+slash, foreign ROOT, a symlinked-CWD-shaped path
            //  that cannot prefix-match the real CWD and therefore reduces via the $HOME/head arms).
            for v in [
                "/Users/operator/models/qwen/",
                "/Users/operator/",
                "/Users/someoneelse/models/qwen/",
                "/Users/someoneelse",
                "/Users/operator/via-symlink/models/qwen",
                "/home/someoneelse",
            ] {
                let out = relativize_for_seal(Path::new(v));
                assert!(
                    is_relative_leakfree(&out),
                    "relativize_for_seal({v:?}) = {out:?} must be relative + leak-free"
                );
            }
        });
    }

    /// The REAL measure-job seal line, driven with ABSOLUTE `/Users/<home>` inputs: the serialized
    /// anchor must carry no such path. RED if `build_measure_job_integrity` drops the relativisation
    /// at any of its path fields.
    #[test]
    fn measure_job_anchor_seals_no_home_path() {
        temp_env_home("/Users/operator", || {
            let argv: Vec<String> = "--candidate /Users/operator/ws/candidate \
                 --baseline /Users/operator/ws/baseline --weights /Users/operator/models/qwen \
                 --golden /Users/operator/g --contract /Users/operator/c --min-pairs 1 --target-pairs 1 \
                 --tag trk --out /Users/operator/ws/out"
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let args = parse_measure_job_args(&argv)
                .expect("args parse")
                .expect("args present");
            let anchor = build_measure_job_integrity(
                &args,
                MeasureJobSealInputs {
                    results_path: "/Users/operator/ws/out/results.json".into(),
                    results_sha256: "rs".into(),
                    candidate_executable: "/Users/operator/ws/candidate/.build/release/engine"
                        .into(),
                    baseline_executable: "/Users/operator/ws/baseline/.build/release/engine".into(),
                    candidate_workspace_sha256: "cws".into(),
                    baseline_workspace_sha256: "bws".into(),
                    golden_sha256: "gs".into(),
                    contract_sha256: "cs".into(),
                    weights_sha256: "ws".into(),
                    weights_file_count: 1,
                    weights_byte_count: 2,
                },
            );
            let bytes = serde_json::to_string(&anchor).expect("anchor serializes");
            assert!(
                !bytes.contains("/Users/"),
                "the sealed measure-job anchor must carry no absolute home path: {bytes}"
            );
            // The digests are untouched — only the display paths are relativised.
            assert_eq!(anchor.weights_sha256, "ws");
            assert_eq!(anchor.weights_dir, "models/qwen");
        });
    }

    /// The overlay re-anchor writes `score_path` into an existing measure-job anchor
    /// (`--integrity`). That inserted path must AGREE with the anchor's own relativisation: no
    /// absolute home path, and an identical reduction of a shared input. RED if
    /// `reanchor_overlay_integrity` drops the relativisation.
    #[test]
    fn overlay_reanchor_seals_no_home_path_and_agrees_with_the_anchor() {
        temp_env_home("/Users/operator", || {
            let dir = std::env::temp_dir().join(format!(
                "benchctl-f5-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // An existing anchor to re-anchor in place (the measure-job's own integrity file).
            let integrity_path = dir.join("benchmark-integrity.results.json");
            std::fs::write(
                &integrity_path,
                "{\"results_path\":\"ws/out/results.json\"}\n",
            )
            .unwrap();
            // The ranked score path is an ABSOLUTE home path — pure provenance, never resolved.
            let score_path = PathBuf::from("/Users/operator/ws/out/score.json");
            let overlay_args = OverlayTimingArgs {
                gates_score: dir.join("gates.json"),
                results: dir.join("results.json"),
                score_path: score_path.clone(),
                integrity: Some(integrity_path.clone()),
                contract: None,
            };
            reanchor_overlay_integrity(&overlay_args, "deadbeef").expect("reanchor rewrites");
            let written = std::fs::read_to_string(&integrity_path).expect("anchor rewritten");
            assert!(
                !written.contains("/Users/"),
                "the re-anchored sidecar must carry no absolute home path: {written}"
            );
            // AGREEMENT: the inserted score_path is relativised the same way the anchor relativises.
            let value: serde_json::Value = serde_json::from_str(&written).unwrap();
            assert_eq!(
                value["score_path"].as_str().unwrap(),
                relativize_for_seal(&score_path)
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Serialise `$HOME` mutation across these tests: the process env is global, so two threads
    /// racing on it would flake. `cargo test` runs a module's tests on separate threads, so the
    /// three home-sensitive tests share this mutex and restore the prior value.
    fn temp_env_home(value: &str, f: impl FnOnce()) {
        use std::sync::Mutex;
        static HOME_LOCK: Mutex<()> = Mutex::new(());
        let _guard = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", value);
        f();
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// The sealed sidecar's KEY SET, pinned exactly — the #123 discipline applied to the
    /// measure-job anchor. `benchmark-integrity.results.json` is governed by `MeasureJobIntegrity`,
    /// NOT by `scripts/fixtures/integrity-runner-keys.json` (that roster is the ITERATE sidecar's
    /// surplus over the reference's nine fields, and is compared against the REFERENCE object by
    /// `facade-leg.sh` / `official-parity.sh`). The two are different artifacts with different
    /// consumers, so this struct needs its own floor rather than borrowing that one.
    #[test]
    fn measure_job_integrity_key_set_is_pinned_and_carries_the_producer() {
        let sidecar = MeasureJobIntegrity {
            results_path: "r.json".into(),
            results_sha256: "d".into(),
            candidate_workspace: "cw".into(),
            baseline_workspace: "bw".into(),
            candidate_executable: "ce".into(),
            baseline_executable: "be".into(),
            candidate_workspace_sha256: "cws".into(),
            baseline_workspace_sha256: "bws".into(),
            golden_sha256: "gs".into(),
            contract_sha256: "cs".into(),
            weights_dir: "wd".into(),
            weights_sha256: "ws".into(),
            weights_file_count: 1,
            weights_byte_count: 2,
            gates_producer: "benchmark-sh".into(),
        };
        let encoded = serde_json::to_string(&sidecar).expect("sidecar serializes");
        let v: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        let mut got: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        got.sort_unstable();
        let mut want = vec![
            "baseline_executable",
            "baseline_workspace",
            "baseline_workspace_sha256",
            "candidate_executable",
            "candidate_workspace",
            "candidate_workspace_sha256",
            "contract_sha256",
            "gates_producer",
            "golden_sha256",
            "results_path",
            "results_sha256",
            "weights_byte_count",
            "weights_dir",
            "weights_file_count",
            "weights_sha256",
        ];
        want.sort_unstable();
        assert_eq!(
            got, want,
            "the measure-job integrity key set changed — update the consumers that read it \
             (official-paired.sh seam-2 checks, test-paired-offline.sh's stub) with it"
        );
        assert_eq!(v["gates_producer"].as_str().unwrap(), "benchmark-sh");
    }

    #[test]
    fn measure_job_parse_requires_all_workspace_flags_and_rejects_baseline_overrides() {
        let full: Vec<String> = [
            "--candidate",
            "cand",
            "--baseline",
            "base",
            "--weights",
            "w",
            "--golden",
            "g",
            "--contract",
            "c",
            "--tokens",
            "128",
            "--mtp-depth",
            "2",
            "--min-pairs",
            "3",
            "--target-pairs",
            "4",
            "--tag",
            "trk",
            "--out",
            "o",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let ok = parse_measure_job_args(&full).unwrap().unwrap();
        assert_eq!(ok.tokens, 128);
        assert_eq!(ok.mtp_depth, 2);
        // #105 cycle-5 finding 5 — `--mtp-depth` WAS passed here, so the flag is the honest source.
        assert_eq!(
            ok.candidate_spec_source,
            measure_job::SPEC_SOURCE_MTP_DEPTH_FLAG
        );
        // ...and with the flag OMITTED the spec comes from DEFAULT_MTP_DEPTH, which must NOT seal
        // "mtp-depth-flag" (naming a flag the operator never passed).
        let no_flag: Vec<String> = full
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                full[*i] != "--mtp-depth" && (*i == 0 || full[*i - 1] != "--mtp-depth")
            })
            .map(|(_, s)| s.clone())
            .collect();
        let defaulted = parse_measure_job_args(&no_flag).unwrap().unwrap();
        assert_eq!(defaulted.mtp_depth, measure_job::DEFAULT_MTP_DEPTH);
        assert_eq!(
            defaulted.candidate_spec_source,
            measure_job::SPEC_SOURCE_MTP_DEPTH_DEFAULT,
            "the no-flag path seals the DEFAULT source, not the flag source"
        );
        assert_eq!(ok.goldens, vec![PathBuf::from("g")]);
        assert_eq!(ok.min_pairs, 3);
        assert_eq!(ok.target_pairs, 4);
        assert_eq!(ok.tag, "trk");
        assert_eq!(ok.weights, PathBuf::from("w"));
        // finding 3: the dropped paired-baseline flags are a hard mutual-exclusion error.
        let mut with_baseline = full.clone();
        with_baseline.push("--baseline-decode-spt".to_string());
        with_baseline.push("0.1".to_string());
        assert!(parse_measure_job_args(&with_baseline).is_err());
        let mut with_engine = full.clone();
        with_engine.push("--engine".to_string());
        with_engine.push("x".to_string());
        assert!(parse_measure_job_args(&with_engine).is_err());
        // Missing a required flag is a usage error; target < min is rejected.
        assert!(parse_measure_job_args(&full[..2]).is_err());
        let mut bad_target = full.clone();
        // target-pairs is index 15 value; set it below min-pairs.
        for k in 0..bad_target.len() {
            if bad_target[k] == "--target-pairs" {
                bad_target[k + 1] = "1".to_string();
            }
        }
        assert!(parse_measure_job_args(&bad_target).is_err());
        // Minor: --tokens 0 (a zero decode window) is rejected at parse.
        let mut zero_tokens = full.clone();
        for k in 0..zero_tokens.len() {
            if zero_tokens[k] == "--tokens" {
                zero_tokens[k + 1] = "0".to_string();
            }
        }
        match parse_measure_job_args(&zero_tokens) {
            Err(e) => assert!(
                e.contains("--tokens must be > 0"),
                "zero decode window rejected: {e}"
            ),
            Ok(_) => panic!("--tokens 0 must be rejected at parse"),
        }
    }

    #[test]
    fn measure_job_weights_optional_derives_from_env_else_fails_closed() {
        // R6/R14: the approved draft CLI (draft@064c0ff2:2088-2098) has NO --weights; the draft passes
        // the weights dir on-box as env QMTP_TARGET_DIR (R14 rename of the dead QWEN_MTP_TARGET_DIR).
        // So --weights is an OPTIONAL OVERRIDE: derived from the env when omitted, override wins when
        // present, fail-closed clear error when neither is set. (Serialized under one test so the
        // process-global env is not raced by a sibling — no other test reads WEIGHTS_ENV_VAR.)
        let with_weights: Vec<String> = [
            "--candidate",
            "cand",
            "--baseline",
            "base",
            "--weights",
            "override-dir",
            "--golden",
            "g",
            "--contract",
            "c",
            "--tokens",
            "128",
            "--mtp-depth",
            "2",
            "--min-pairs",
            "3",
            "--target-pairs",
            "4",
            "--tag",
            "trk",
            "--out",
            "o",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let no_weights: Vec<String> = with_weights
            .iter()
            .filter(|&s| s != "--weights" && s != "override-dir")
            .cloned()
            .collect();

        // Neither --weights nor QMTP_TARGET_DIR → fail closed with a clear message.
        std::env::remove_var(WEIGHTS_ENV_VAR);
        match parse_measure_job_args(&no_weights) {
            Err(e) => {
                assert!(e.contains("--weights"), "message names --weights: {e}");
                assert!(
                    e.contains(WEIGHTS_ENV_VAR),
                    "message names the env var: {e}"
                );
            }
            Ok(_) => panic!("neither --weights nor {WEIGHTS_ENV_VAR} set must fail closed"),
        }

        // Env set, --weights omitted → weights DERIVED from QMTP_TARGET_DIR.
        std::env::set_var(WEIGHTS_ENV_VAR, "/on-box/derived-weights");
        let derived = parse_measure_job_args(&no_weights).unwrap().unwrap();
        assert_eq!(derived.weights, PathBuf::from("/on-box/derived-weights"));

        // --weights present → it OVERRIDES the env (override wins).
        let overridden = parse_measure_job_args(&with_weights).unwrap().unwrap();
        assert_eq!(overridden.weights, PathBuf::from("override-dir"));

        std::env::remove_var(WEIGHTS_ENV_VAR);
    }

    #[test]
    fn results_json_sidecar_body_is_bare_basename() {
        // The paired results.json sidecar uses the BARE BASENAME `<hex>  results.json\n` (reference
        // overlay parity, so `shasum -c` works on the downloaded artifact), NOT the full path the
        // shared write_score uses.
        let dir = std::env::temp_dir().join(format!(
            "benchctl-results-sidecar-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("results.json");
        let json = "{\"mode\":\"qwen-mtp-paired-decode-only\"}";
        write_results_json(&path, json).unwrap();
        let sidecar = std::fs::read_to_string(dir.join("results.json.sha256")).unwrap();
        let expected_hex = sha256_hex(json.as_bytes());
        assert_eq!(
            sidecar,
            format!("{expected_hex}  results.json\n"),
            "results.json sidecar body is the bare basename, not the full path"
        );
        assert!(
            !sidecar.contains(&dir.display().to_string()),
            "results.json sidecar must NOT contain the directory path"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn h6_calibration_bootstrap_skips_the_pre_read_of_a_missing_file() {
        // H6/H2 (cycle-3) — under --calibration-bootstrap the run AUTHORS the band; it must NOT
        // pre-read or require the file. A MISSING path in bootstrap mode returns Ok(None), never a
        // die-6 (the wrapper's serial_band_check returns immediately in bootstrap, W:1423-1426).
        let missing = std::env::temp_dir().join(format!(
            "benchctl-cal-missing-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&missing);
        let missing_str = missing.to_string_lossy().to_string();

        // Bootstrap mode: a missing file is fine (None), no read attempted.
        let boot = resolve_calibration_env(
            true,
            Some(&missing_str),
            Some("t"),
            measure_job::TIMED_MODE,
            "t1",
        )
        .unwrap();
        assert!(
            boot.is_none(),
            "bootstrap mode skips the pre-read; missing file → None"
        );

        // NON-bootstrap mode: the SAME missing file fails closed (die-6).
        let strict = resolve_calibration_env(
            false,
            Some(&missing_str),
            Some("t"),
            measure_job::TIMED_MODE,
            "t1",
        );
        let e = strict.expect_err("non-bootstrap missing calibration must die-6");
        assert_eq!(
            e.exit, 6,
            "missing calibration read is die-6 outside bootstrap"
        );
    }

    #[test]
    fn h105_c5_calibration_pre_read_die6s_a_cross_series_or_cross_track_file() {
        // #105 cycle-5 (HIGH) — the fence WIRED at the seam that actually gates a run: the
        // BASELINE_CALIBRATION pre-read. This is the production caller of
        // `bench_core::free_run::timed_modes_comparable` (via `enforce_calibration_series_fence`),
        // and it runs BEFORE any measuring — so a cross-series file exits 6 without ever reaching
        // the band check, rather than banding a Model-2 number against a native-regime mean.
        let dir = std::env::temp_dir().join(format!(
            "benchctl-fence-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, v: serde_json::Value| -> String {
            let p = dir.join(name);
            std::fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
            p.to_string_lossy().to_string()
        };
        let body = |timed_mode: &str, track: &str| {
            serde_json::json!({
                "timed_mode": timed_mode,
                "track_id": track,
                "serial_decode_seconds_per_token_mean": 0.038,
                "decode_tokens": 512,
                "targets": { "t1": { "serial_decode_seconds_per_token_mean": 0.038, "decode_tokens": 512 } }
            })
        };
        let run_track = "qwen3.8-27b-mtp-v1";

        // The review's attack input: a native-regime file aimed at this (Model-2) run → die-6.
        let native = write("native.json", body("native_mtp_v1", run_track));
        let e = resolve_calibration_env(
            false,
            Some(&native),
            Some("t1"),
            measure_job::TIMED_MODE,
            run_track,
        )
        .expect_err("a native-regime calibration must not band this run");
        assert_eq!(e.exit, 6, "cross-series calibration is die-6");
        assert!(e.message.contains("NOT comparable"), "{}", e.message);

        // Right series, WRONG track → die-6 too.
        let other_track = write(
            "other-track.json",
            body(measure_job::TIMED_MODE, "qwen3.6-27b-mtp-v1"),
        );
        let e = resolve_calibration_env(
            false,
            Some(&other_track),
            Some("t1"),
            measure_job::TIMED_MODE,
            run_track,
        )
        .expect_err("another track's calibration must not band this run");
        assert_eq!(e.exit, 6, "cross-track calibration is die-6");

        // Matching series + track → resolves normally (the fence is a gate, not a wall).
        let ok = write("match.json", body(measure_job::TIMED_MODE, run_track));
        let resolved = resolve_calibration_env(
            false,
            Some(&ok),
            Some("t1"),
            measure_job::TIMED_MODE,
            run_track,
        )
        .expect("a same-series, same-track calibration must resolve")
        .expect("a present file resolves to a band");
        assert_eq!(resolved.timed_mode, measure_job::TIMED_MODE);
        assert_eq!(resolved.track_id, run_track);

        // W3 (fence reconciliation) — THE CROSS TEST at the PRODUCTION seam. main.rs no longer
        // passes a hardcoded series here: it passes `run_timed_mode(args.candidate_regime)`. So the
        // very same teacher-forced file that legitimately bands a TF run is die-6 for a FREE-RUN
        // run, and vice versa. Without this the v1.1 scored path would have banded its free-run
        // pooled serial mean against a v1 teacher-forced band — the §5 cross-series bug, upstream of
        // the overlay fence that catches the same class in results/score.
        let free_run_series = measure_job::run_timed_mode(measure_job::LegRegime::FreeRunV1_1);
        let e = resolve_calibration_env(false, Some(&ok), Some("t1"), free_run_series, run_track)
            .expect_err("a teacher-forced band must not gate a free-run-series run");
        assert_eq!(
            e.exit, 6,
            "cross-series calibration is die-6 on the free-run path too"
        );
        assert!(e.message.contains("free_run_v1_1"), "{}", e.message);

        let v11 = write("v11.json", body(free_run_series, run_track));
        let resolved =
            resolve_calibration_env(false, Some(&v11), Some("t1"), free_run_series, run_track)
                .expect("a free-run-series calibration bands a free-run-series run")
                .expect("a present file resolves to a band");
        assert_eq!(resolved.timed_mode, free_run_series);
        // ...and that free-run file is equally refused for a teacher-forced run: the fence is
        // symmetric, keyed on ONE decision function on both sides.
        let e = resolve_calibration_env(
            false,
            Some(&v11),
            Some("t1"),
            measure_job::run_timed_mode(measure_job::LegRegime::TeacherForcedV1),
            run_track,
        )
        .expect_err("a free-run band must not gate a teacher-forced run");
        assert_eq!(e.exit, 6);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn iterate_exit_status_maps_pass_to_zero_and_fail_to_nonzero() {
        // The exact boolean→exit contract at the run_iterate mapping site: a passing run exits 0,
        // a run that did NOT pass (a rejected leg / floor / ceiling / serial-band fail all set
        // passed=false) exits 1 (nonzero). run_iterate feeds this into ExitCode::from, so a false
        // verdict drives a nonzero process ExitCode. (Constructing the real ExitCode end-to-end
        // needs a live engine, so the exact mapping is asserted here at its source.)
        assert_eq!(iterate_exit_status(true), 0, "passing run exits 0");
        assert_eq!(
            iterate_exit_status(false),
            1,
            "failing/rejected run exits nonzero"
        );
        assert_ne!(
            iterate_exit_status(false),
            0,
            "a non-pass must never map to success"
        );
    }

    #[test]
    fn measure_job_exit_status_maps_accept_to_zero_and_reject_to_die5() {
        // The exact verdict→exit contract at the run_measure_job_cli mapping site (ExitCode::from):
        // accepted → 0; die-5 (candidate rejected) → 5. Finding R19 (reverts R8) — there is NO
        // infra/thermal hard-die verdict: a thermal-gate timeout folds into die-5, so exit-2 is
        // never produced by a mid-pair event (it stays reserved for the pre-execution usage/parse
        // error). Neither verdict is the exit-1 load/IO error path. (A real ExitCode end-to-end
        // needs a live pair loop, so the mapping is asserted here at its source.)
        assert_eq!(
            measure_job_exit_status(MeasureJobVerdict::Accepted),
            0,
            "accepted candidate exits 0"
        );
        assert_eq!(
            measure_job_exit_status(MeasureJobVerdict::RejectedDie5),
            5,
            "die 5 exits 5"
        );
        assert_ne!(
            measure_job_exit_status(MeasureJobVerdict::RejectedDie5),
            1,
            "die 5 ≠ exit-1 error path"
        );
        assert_ne!(
            measure_job_exit_status(MeasureJobVerdict::RejectedDie5),
            2,
            "die 5 ≠ exit-2 usage/parse path"
        );
        assert_ne!(
            measure_job_exit_status(MeasureJobVerdict::Accepted),
            measure_job_exit_status(MeasureJobVerdict::RejectedDie5)
        );
        // R13 — a passing --preflight-only run exits 0 (like Accepted) but is a DISTINCT verdict.
        assert_eq!(
            measure_job_exit_status(MeasureJobVerdict::PreflightOk),
            0,
            "preflight-only exits 0"
        );
        // R14 — a serial-band calibration drift exits 6 (die-6), distinct from die-5/0/1/2.
        assert_eq!(
            measure_job_exit_status(MeasureJobVerdict::CalibrationDrift),
            6,
            "calibration drift exits 6"
        );
        assert_ne!(
            measure_job_exit_status(MeasureJobVerdict::CalibrationDrift),
            5,
            "die 6 ≠ die 5"
        );
    }

    #[test]
    fn measure_job_r13_flag_surface_parses_and_validates() {
        // R13 — the new INPUT surface: --mtp-depth (>=2, default 2), repeatable --golden, the
        // min/target per-prompt aliases, the prompt trio (all-three-or-none + shape),
        // --exactness-probe, and the boolean --preflight-only / --calibration-bootstrap flags.
        // W3 — `--tokens`' DEFAULT is now regime-dependent, so this test asserts BOTH defaults.
        let base: Vec<String> = [
            "--candidate",
            "cand",
            "--baseline",
            "base",
            "--weights",
            "w",
            "--golden",
            "g",
            "--contract",
            "c",
            "--min-pairs",
            "3",
            "--target-pairs",
            "4",
            "--tag",
            "trk",
            "--out",
            "o",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        // Defaults: --mtp-depth 2, once, no trio, both booleans off.
        let d = parse_measure_job_args(&base).unwrap().unwrap();
        assert_eq!(
            d.mtp_depth,
            measure_job::DEFAULT_MTP_DEPTH,
            "default --mtp-depth is 2"
        );
        // W3 — the DEFAULT `--mtp-depth 2` candidate spec is `mode=mtp`, which selects the v1.1
        // FREE-RUN regime, whose window is the RULED N = 128. The 512 teacher-forced default now
        // applies to a SERIAL candidate (the only shape that still runs teacher-forced).
        assert_eq!(d.candidate_regime, measure_job::LegRegime::FreeRunV1_1);
        assert_eq!(
            d.tokens,
            measure_job::FREE_RUN_DECODE_TOKENS,
            "a speculating candidate runs the free-run series at the RULED N = 128"
        );
        let mut serial_candidate = base.clone();
        serial_candidate.extend([
            "--candidate-spec".to_string(),
            r#"{"mode":"serial"}"#.to_string(),
        ]);
        let tf = parse_measure_job_args(&serial_candidate).unwrap().unwrap();
        assert_eq!(tf.candidate_regime, measure_job::LegRegime::TeacherForcedV1);
        assert_eq!(
            tf.tokens,
            measure_job::DEFAULT_TOKENS,
            "teacher-forced default --tokens is 512"
        );
        // W3 — an EXPLICIT non-RULED window on the free-run path is a hard usage error, never a
        // silent re-window (N divides the scored seconds-per-token).
        let mut rewindowed = base.clone();
        rewindowed.extend(["--tokens".to_string(), "512".to_string()]);
        let err = parse_measure_job_args(&rewindowed).unwrap_err();
        assert!(
            err.contains("free-run series") && err.contains("128"),
            "explicit free-run re-window is refused: {err}"
        );
        assert_eq!(d.exactness_probe, measure_job::ExactnessProbe::Once);
        assert!(!d.preflight_only && !d.calibration_bootstrap);
        assert!(d.prompt.is_none() && d.prompt_sha256.is_none() && d.target_id.is_none());

        // --depth is renamed → a helpful hard error (never silently accepted).
        let mut depth_flag = base.clone();
        depth_flag.extend(["--depth".to_string(), "2".to_string()]);
        let err = parse_measure_job_args(&depth_flag).unwrap_err();
        assert!(
            err.contains("--mtp-depth"),
            "renamed-flag error names the new flag: {err}"
        );

        // Default candidate spec is built from --mtp-depth (mode mtp), baseline defaults to serial.
        assert_eq!(d.candidate_spec, bench_protocol::SpecConfig::mtp(2));
        assert_eq!(d.baseline_spec, bench_protocol::SpecConfig::serial());
        // #105 cycle-5 finding 5 — `base` passes NO --mtp-depth, so the spec came from
        // DEFAULT_MTP_DEPTH: the honest source is "mtp-depth-default". Sealing "mtp-depth-flag"
        // here (the old assertion) named a flag this invocation never carried.
        assert_eq!(d.candidate_spec_source, "mtp-depth-default");
        assert_eq!(d.baseline_spec_source, "serial-default");

        // Depth-0-via-serial-mode (docs/spec-config-design.md step 4): the ">= 2" floor is RETIRED.
        // --mtp-depth 1 now builds a valid mtp(1) spec (the MODE gates, not a depth-int floor).
        let mut low_depth = base.clone();
        low_depth.extend(["--mtp-depth".to_string(), "1".to_string()]);
        let ld = parse_measure_job_args(&low_depth).unwrap().unwrap();
        assert_eq!(ld.candidate_spec, bench_protocol::SpecConfig::mtp(1));
        // The 32 cap is re-homed onto mtp.depth: an over-cap depth REJECTS on the official path.
        let mut over_cap = base.clone();
        over_cap.extend(["--mtp-depth".to_string(), "33".to_string()]);
        assert!(
            parse_measure_job_args(&over_cap).is_err(),
            "mtp.depth 33 exceeds the readonly 32 cap"
        );

        // --candidate-spec / --baseline-spec overrides are recorded as spec_source cli-override.
        let mut ovr = base.clone();
        ovr.extend([
            "--candidate-spec".to_string(),
            r#"{"mode":"mtp","mtp":{"depth":6}}"#.to_string(),
            "--baseline-spec".to_string(),
            r#"{"mode":"serial"}"#.to_string(),
        ]);
        let o = parse_measure_job_args(&ovr).unwrap().unwrap();
        assert_eq!(o.candidate_spec, bench_protocol::SpecConfig::mtp(6));
        assert_eq!(o.candidate_spec_source, "cli-override");
        assert_eq!(o.baseline_spec_source, "cli-override");
        // David ruling (2026-08-26) — a WELL-FORMED dflash candidate spec now PARSES. Whether the
        // track admits `dflash` is CONTRACT data (`Contract::allowed_modes`), and the contract is
        // not read at CLI-parse time, so this boundary can no longer answer that question — the
        // refusal moved to `execute_measure_job` as a die-8 pre-GPU prereq (truth table:
        // `measure_job::enforce_track_allowed_modes` and its unit tests).
        //
        // This assertion is the REVERT-PROOF half of the ruling on the CLI side: restore the old
        // `validate_spec_mode_allowed(&spec, &DEFAULT_ALLOWED_MODES)` here and this goes red,
        // because `dflash` is not in the default list and never will be.
        let mut dflash_mode = base.clone();
        dflash_mode.extend([
            "--candidate-spec".to_string(),
            r#"{"mode":"dflash","dflash":{}}"#.to_string(),
        ]);
        let d = parse_measure_job_args(&dflash_mode)
            .expect("a well-formed dflash spec parses; the track fence is contract-driven")
            .unwrap();
        assert_eq!(d.candidate_spec.mode, "dflash");
        // …and the CONTRACT-INDEPENDENT half of the old check stays exactly where it was: a
        // CROSS-MODULE spec is malformed on every track, whatever any fixture declares, so it is
        // still a parse-time usage error.
        let mut cross_module = base.clone();
        cross_module.extend([
            "--candidate-spec".to_string(),
            r#"{"mode":"dflash","dflash":{},"mtp":{"depth":2}}"#.to_string(),
        ]);
        let e = parse_measure_job_args(&cross_module).unwrap_err();
        assert!(e.contains("cross-module key"), "{e}");

        // #105 H-B — a NON-serial --baseline-spec is a hard error (the serial denominator is not
        // CLI-steerable), even for an otherwise-allowed mode like mtp.
        let mut bad_baseline = base.clone();
        bad_baseline.extend([
            "--baseline-spec".to_string(),
            r#"{"mode":"mtp","mtp":{"depth":2}}"#.to_string(),
        ]);
        let e = parse_measure_job_args(&bad_baseline).unwrap_err();
        assert!(
            e.contains("must be {\"mode\":\"serial\"}"),
            "non-serial baseline rejected: {e}"
        );

        // Medium (#105) — --mtp-depth and --candidate-spec are MUTUALLY EXCLUSIVE (explicit conflict,
        // never a silent discard of --mtp-depth).
        let mut both = base.clone();
        both.extend([
            "--mtp-depth".to_string(),
            "4".to_string(),
            "--candidate-spec".to_string(),
            r#"{"mode":"mtp","mtp":{"depth":6}}"#.to_string(),
        ]);
        let e = parse_measure_job_args(&both).unwrap_err();
        assert!(
            e.contains("mutually exclusive"),
            "--mtp-depth + --candidate-spec conflict: {e}"
        );

        // Medium (#105) — an mtp(0) candidate spec rejects (depth 0 is the serial control).
        let mut mtp0 = base.clone();
        mtp0.extend([
            "--candidate-spec".to_string(),
            r#"{"mode":"mtp","mtp":{"depth":0}}"#.to_string(),
        ]);
        assert!(
            parse_measure_job_args(&mtp0).is_err(),
            "mtp(0) candidate rejects"
        );

        // Medium (#105) — a cross-module candidate spec (mtp mode + a stray dflash block) rejects.
        let mut cross = base.clone();
        cross.extend([
            "--candidate-spec".to_string(),
            r#"{"mode":"mtp","mtp":{"depth":2},"dflash":{}}"#.to_string(),
        ]);
        assert!(
            parse_measure_job_args(&cross).is_err(),
            "cross-module candidate spec rejects"
        );

        // Repeatable --golden → a Vec; the per-prompt aliases set the same budgets.
        let mut multi = base.clone();
        multi.extend(["--golden".to_string(), "g2".to_string()]);
        let m = parse_measure_job_args(&multi).unwrap().unwrap();
        assert_eq!(m.goldens, vec![PathBuf::from("g"), PathBuf::from("g2")]);
        let aliased: Vec<String> = [
            "--candidate",
            "cand",
            "--baseline",
            "base",
            "--weights",
            "w",
            "--golden",
            "g",
            "--contract",
            "c",
            "--min-pairs-per-prompt",
            "2",
            "--pairs-per-prompt",
            "5",
            "--tag",
            "trk",
            "--out",
            "o",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let al = parse_measure_job_args(&aliased).unwrap().unwrap();
        assert_eq!(
            (al.min_pairs, al.target_pairs),
            (2, 5),
            "per-prompt aliases set the budgets"
        );

        // The prompt trio is all-three-or-none.
        let mut partial = base.clone();
        partial.extend(["--prompt".to_string(), "p.txt".to_string()]);
        assert!(
            parse_measure_job_args(&partial).is_err(),
            "prompt alone is rejected"
        );
        let mut trio = base.clone();
        trio.extend([
            "--prompt".to_string(),
            "p.txt".to_string(),
            "--prompt-sha256".to_string(),
            "a".repeat(64),
            "--target-id".to_string(),
            "qwen3.8-27b-mtp-v1".to_string(),
        ]);
        let t = parse_measure_job_args(&trio).unwrap().unwrap();
        assert_eq!(t.target_id.as_deref(), Some("qwen3.8-27b-mtp-v1"));
        // A bad sha (not 64 lowercase hex) and a bad target-id are rejected.
        let mut bad_sha = base.clone();
        bad_sha.extend([
            "--prompt".to_string(),
            "p.txt".to_string(),
            "--prompt-sha256".to_string(),
            "XYZ".to_string(),
            "--target-id".to_string(),
            "ok".to_string(),
        ]);
        assert!(parse_measure_job_args(&bad_sha).is_err());

        // --exactness-probe parses valid modes and rejects unknown ones.
        let mut probe = base.clone();
        probe.extend(["--exactness-probe".to_string(), "per-pair".to_string()]);
        assert_eq!(
            parse_measure_job_args(&probe)
                .unwrap()
                .unwrap()
                .exactness_probe,
            measure_job::ExactnessProbe::PerPair
        );
        let mut bad_probe = base.clone();
        bad_probe.extend(["--exactness-probe".to_string(), "hourly".to_string()]);
        assert!(parse_measure_job_args(&bad_probe).is_err());

        // The boolean flags toggle without a value.
        let mut booleans = base.clone();
        booleans.push("--preflight-only".to_string());
        booleans.push("--calibration-bootstrap".to_string());
        let b = parse_measure_job_args(&booleans).unwrap().unwrap();
        assert!(b.preflight_only && b.calibration_bootstrap);
    }

    #[test]
    fn parse_rejects_bad_mode_and_unknown_flag() {
        assert!(parse_iterate_args(&[
            "--engine".into(),
            "e".into(),
            "--weights".into(),
            "w".into(),
            "--golden".into(),
            "g".into(),
            "--mode".into(),
            "turbo".into()
        ])
        .is_err());
        assert!(parse_iterate_args(&["--bogus".into()]).is_err());
    }
}
