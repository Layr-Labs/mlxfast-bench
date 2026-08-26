//! WS1-8 — `benchctl iterate`: drive the engine end-to-end and assemble a sealed
//! `score.json`.
//!
//! Flow (mirrors Swift `QwenRuntime.localIterate` +
//! `runLocalIterateCheckedTimingWithWorker`): connect a [`Session`] → run the
//! correctness gate against the golden (bench-core `conformance` anchor + free-run
//! checks) → run the WS1-6 parent-side timing (prefill + decode) → compute the
//! estimated score with `score_default_weights` → assemble the [`ScorePayload`].
//!
//! Like Swift local-iterate, the pass/fail decision is: correctness passed AND the
//! timings + baselines + score are finite and positive. Acceptance bands and
//! speedup floors are reported in the metrics (`passed_*_speedup_floor`) but do not
//! gate the local estimated score (that is the official/ranked path's job).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bench_core::conformance::{
    run_conformance, AnchorOutput, ConformanceReport, CorrectnessScope, EngineHandle, TopLogit,
};
use bench_core::constants::{
    OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN, OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
    SCORE_DECODE_SPEEDUP_FLOOR, SCORE_PREFILL_SPEEDUP_FLOOR,
};
use bench_core::golden::{GoldenFixture, Token};
use bench_core::score::{score_default_weights, speedup};
use bench_core::BenchError;
use bench_runner::{
    run_timed_benchmark_fresh_per_phase, run_timed_benchmark_fresh_per_phase_time_only,
    scrub_reason_for_seal, Hello, LineTransport, RunnerError, Session, TimingParams, TimingResult,
};
use sha2::{Digest, Sha256};

use crate::score::{ScoreMetrics, ScorePayload};

/// Swift `MLXFastConstants.numHiddenLayers` (Qwen3.6): 64. Audit-only in the score.
const NUM_HIDDEN_LAYERS: i64 = 64;
/// Swift `QwenRuntime.bandwidthSource` for the RAM-resident dense runtime.
const BANDWIDTH_SOURCE: &str = "ram_resident_model";
/// Swift local-iterate/local-submit `timingRepeats` (the CLI passes 1). benchd runs a
/// single prefill+decode timing pass, so the checked-timing repeat count is always 1.
const TIMING_REPEATS: i64 = 1;

/// F1 — the WORKSPACE HARNESS IDENTITY this run seals into `metrics.harness_hash`.
///
/// A newtype rather than a bare `String` because the invariant is the whole point: an instance
/// EXISTS only for a workspace whose nine harness roots were all present and hashed. There is no
/// `Default`, no empty constructor and no public field, so no code path can reintroduce the
/// pre-F1 `String::new()` stub by accident — sealing an empty or partial harness identity now
/// requires deleting this type, not forgetting a line.
///
/// **Why benchd computes this at all.** `metrics.harness_hash` gates publication: the seam-3
/// overlay ([`crate::overlay::validate_gates`]) refuses a gates-score whose harness identity is
/// empty or malformed, so before F1 no benchd-authored score could publish. And it MUST be
/// computed here rather than read off the engine wire — the worker is participant-built, so a
/// wire-reported hash would be attacker-controlled. Trusted value, trusted-side computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessIdentity(String);

impl HarnessIdentity {
    /// Resolve the harness identity of the workspace rooted at `workspace_root`, FAIL-CLOSED.
    ///
    /// `Err` (naming the missing root) when the workspace is not a harness tree — the caller's
    /// contract is to REFUSE THE RUN, mirroring the reference's `fatalError` posture
    /// (`QwenRuntimePreflight.swift:89`), never to fall back to a stub. The well-formedness
    /// re-check is belt-and-braces: it makes "a `HarnessIdentity` exists" mean "a 64-lowercase-hex
    /// digest exists", which is exactly the predicate the overlay will apply downstream.
    pub fn resolve(workspace_root: &Path) -> Result<HarnessIdentity, String> {
        let hash = bench_core::harness_hash::harness_hash(workspace_root)?;
        if !bench_core::harness_hash::is_well_formed_harness_hash(&hash) {
            return Err(format!(
                "harnessHash produced a malformed digest (expected 64 lowercase hex characters, \
                 got length {}); refusing to seal a harness identity that will not validate",
                hash.len()
            ));
        }
        Ok(HarnessIdentity(hash))
    }

    /// [`HarnessIdentity::resolve`] over the process's current working directory — the reference's
    /// own production resolution, where the nine roots are CWD-relative. This is the workspace an
    /// `iterate` run drives: the facade invokes benchd from the engine workspace root, and the
    /// integrity sidecar already treats the CWD as that root (see `relativize_for_seal`).
    pub fn resolve_from_current_dir() -> Result<HarnessIdentity, String> {
        // `getcwd()` is what Foundation prepends to a relative `URL(fileURLWithPath:)`, so
        // resolving the CWD explicitly and resolving it implicitly are the same computation —
        // verified equal against the reference. Delegating keeps ONE resolution path rather than a
        // second copy of the well-formedness contract.
        let cwd = std::env::current_dir().map_err(|e| {
            format!("harnessHash: cannot resolve the current working directory: {e}")
        })?;
        HarnessIdentity::resolve(&cwd)
    }

    /// The 64-lowercase-hex digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// TEST-ONLY construction from a literal digest, so a payload builder can be exercised without
    /// a harness workspace on disk. `#[cfg(test)]`, so no production path can reach it.
    ///
    /// It ASSERTS well-formedness rather than accepting any string: a test fixture must not be able
    /// to smuggle in the empty identity this change exists to eliminate, which keeps the
    /// "`iterate` seals a 64-hex non-empty hash" assertions honest.
    #[cfg(test)]
    pub(crate) fn for_test(hash: &str) -> HarnessIdentity {
        assert!(
            bench_core::harness_hash::is_well_formed_harness_hash(hash),
            "a test HarnessIdentity must still be a 64-lowercase-hex digest, got {hash:?}"
        );
        HarnessIdentity(hash.to_string())
    }

    /// The identity every payload-builder test passes. A RECOGNISABLE constant, so an assertion can
    /// prove a payload sealed THIS value rather than an incidental 64-hex string.
    #[cfg(test)]
    pub(crate) const TEST_HASH: &'static str =
        "7e57000000000000000000000000000000000000000000000000000000000000";

    /// [`HarnessIdentity::TEST_HASH`] as an identity. One definition for all three test modules.
    /// `&'static` so a test can hand it to a borrowing [`RunDigests`] without a local binding.
    #[cfg(test)]
    pub(crate) fn test_default() -> &'static HarnessIdentity {
        static IDENTITY: std::sync::LazyLock<HarnessIdentity> =
            std::sync::LazyLock::new(|| HarnessIdentity::for_test(HarnessIdentity::TEST_HASH));
        &IDENTITY
    }
}

/// The two identity DIGESTS benchd computes for a run and seals into its score: which weights it
/// measured (`metrics.weights_hash` + its byte/file counts) and which harness produced the result
/// (`metrics.harness_hash`).
///
/// They travel together through every payload builder — the correctness-failure builders, the
/// preflight refusal, the official gating failures, the passing assembly — and neither is ever
/// used without the other, so they are one parameter rather than two. That is also what keeps F1
/// from pushing five payload builders past the argument-count limit: adding the harness identity
/// alongside the weights digest costs those signatures nothing instead of forcing a lint waiver.
///
/// `golden_hash` is deliberately NOT in here: it comes from the loaded golden fixture, is blanked
/// on the failure paths (#132(b) MIRROR-BLANK-STRICTLY), and has different provenance from these
/// two, which benchd computes itself and seals unconditionally.
#[derive(Debug, Clone, Copy)]
pub struct RunDigests<'a> {
    pub weights: &'a DirDigest,
    pub harness: &'a HarnessIdentity,
}

#[cfg(test)]
impl<'a> RunDigests<'a> {
    /// A `RunDigests` over `weights` carrying the shared test harness identity.
    pub(crate) fn for_test(weights: &'a DirDigest) -> RunDigests<'a> {
        RunDigests {
            weights,
            harness: HarnessIdentity::test_default(),
        }
    }
}

/// Which decode window + runtime label this run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 128-step checked decode (`BENCHMARK_DECODE_STEPS`); `runtime = "rust-local-iterate"`.
    LocalIterate,
    /// 1023-step checked decode; `runtime = "rust-local-submit"` (Swift
    /// `QwenRuntime.localIterate` with `decodeSteps = 1023`, main.swift:264-291). Reuses the
    /// local-iterate checked-timing machinery over a long continuous decode of `cases[0]`.
    LocalSubmit,
    /// 128-step decode; `runtime = "rust"`. (Stub-only in this wave; iterate uses it
    /// so the timing/score plumbing is mode-parametric.)
    Official,
}

impl Mode {
    pub const fn decode_steps(self) -> usize {
        match self {
            Mode::LocalIterate => bench_core::constants::LOCAL_ITERATE_BENCHMARK_DECODE_STEPS,
            Mode::LocalSubmit => bench_core::constants::LOCAL_SUBMIT_BENCHMARK_DECODE_STEPS,
            Mode::Official => bench_core::constants::BENCHMARK_DECODE_STEPS,
        }
    }

    /// How many `expected_tokens` the golden LOADER must require for this mode, mirroring the
    /// reference's own load call. Swift `QwenRuntime.localIterate` loads with
    /// `requiredSteps: options.benchmarkDecodeSteps + 1`
    /// (`mlxfast-qwen-38-27b-mtp-engine/Sources/MLXFastTrustedHarness/QwenRuntimeLocalIterate.swift@ebe3446:101-105`)
    /// — the `+ 1` is the SEED token: `expected_tokens[0]` is the prefill/decode-seed argmax and
    /// `expected_tokens[k + 1]` is decode step `k`'s output (`:776-777`, `:854-858`). Swift's
    /// official `benchmark` path takes the loader DEFAULT (`correctnessSteps`,
    /// `QwenRuntimeBenchmark.swift:88`), so Official does too.
    ///
    /// EXHAUSTIVE match (LANDMINE #60): a new `Mode` must state its loader arity explicitly.
    pub fn golden_required_steps(self) -> usize {
        match self {
            Mode::LocalIterate | Mode::LocalSubmit => self.decode_steps() + 1,
            Mode::Official => bench_core::constants::CORRECTNESS_STEPS,
        }
    }

    /// The `runtime` audit label. benchd is a Rust producer, so this is
    /// `rust-*` rather than the Swift `swift-*` — the one intentional value
    /// difference from the Swift score (field NAMES are identical).
    pub fn runtime(self) -> &'static str {
        match self {
            Mode::LocalIterate => "rust-local-iterate",
            Mode::LocalSubmit => "rust-local-submit",
            Mode::Official => "rust",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "local-iterate" => Some(Mode::LocalIterate),
            "local-submit" => Some(Mode::LocalSubmit),
            "official" => Some(Mode::Official),
            _ => None,
        }
    }

    /// The Swift `modeName` (`options.modeName`) — used in error strings that must match
    /// Swift byte-for-byte (e.g. the missing-baseline preflight error). Not the `runtime`
    /// audit label (which is `rust-*`).
    pub fn mode_name(self) -> &'static str {
        match self {
            Mode::LocalIterate => "local-iterate",
            Mode::LocalSubmit => "local-submit",
            Mode::Official => "official",
        }
    }

    /// Whether the local GPU cool gate runs by default for this mode (David 2026-08-17):
    /// local-iterate **OFF** (opt-in via `--cool-gate`; the A3 window showed gating degrades
    /// its single-shot measurement); local-submit **ON** (long continuous decode, thermal
    /// consistency matters). Official has no cool gate (that path never calls it). The facade
    /// always passes `--cool-gate` so it matches benchmark.sh regardless of the native default.
    ///
    /// EXHAUSTIVE match (LANDMINE #60): no wildcard `_`, so a new `Mode` variant must decide
    /// its cool-gate default explicitly rather than silently inheriting `false`.
    pub fn cool_gate_on_by_default(self) -> bool {
        match self {
            Mode::LocalIterate => false,
            Mode::LocalSubmit => true, // (P6 RULING: submit ON)
            Mode::Official => false,
        }
    }

    /// Whether this mode brands a primary `cases[]` (teacher-forced) base-case failure
    /// Swift-style — i.e. it derives correctness from the fused checked-timing pass rather
    /// than a standalone conformance report, so a base-case mismatch is reported with the
    /// `modeName`-keyed case id, Swift checked-step numbering, and the
    /// `"<modeName> teacher-forced token mismatch"` error (§F1). True for BOTH
    /// `LocalIterate` and `LocalSubmit` (Swift `QwenRuntime.localIterate` drives both; the
    /// branding keys on `modeName`, so it is mode-DERIVED on the Swift side too — the Rust
    /// F1 arm must not hard-code `Mode::LocalIterate`). Official runs the standalone superset
    /// and keeps its own case/step report.
    pub fn is_local_checked_timing(self) -> bool {
        match self {
            Mode::LocalIterate | Mode::LocalSubmit => true,
            Mode::Official => false,
        }
    }
}

/// A directory digest of the transformed weights (Swift `DirectoryDigest`).
///
/// P2: a **byte-for-byte port** of Swift `directoryDigest`
/// (mlxfast-challenge-dev/Sources/MLXFastHarness/QwenRuntimePreflight.swift:172-239) so
/// `weights_hash` MATCHES the Swift score (moves to the DETERMINISTIC differ bucket). The
/// tree hash is, over files sorted by relative path (ignoring exact relative paths
/// `.benchmark-source.sha256` and `.gitkeep`): `relpath || 0x00 || SHA256(file bytes) ||
/// 0x00`, then SHA256 of that stream, hex-encoded. Each file's own SHA256 is streamed in
/// 8 MiB chunks (Swift `fileDigest`). `byte_count`/`file_count` are exact.
#[derive(Debug, Clone, PartialEq)]
pub struct DirDigest {
    pub sha256: String,
    pub byte_count: i64,
    pub file_count: i64,
}

impl DirDigest {
    /// The empty digest (sha256 of no files), for when no weights dir is provided.
    #[allow(dead_code)]
    pub fn empty() -> Self {
        DirDigest {
            sha256: crate::score::sha256_hex(&[]),
            byte_count: 0,
            file_count: 0,
        }
    }
}

/// Compute a digest over the files under `root`, ignoring `.benchmark-source.sha256`
/// and `.gitkeep` (matching the Swift `ignoredRelativePaths`).
pub fn dir_digest(root: &Path) -> io::Result<DirDigest> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(root, root, &mut files)?;
    // Swift ignores by EXACT relative path (root-level), not by basename — a nested
    // `subdir/.gitkeep` is NOT ignored. Match that.
    files.retain(|(rel, _)| rel != ".benchmark-source.sha256" && rel != ".gitkeep");
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut tree = Sha256::new();
    let mut byte_count = 0i64;
    let mut file_count = 0i64;
    for (rel, path) in &files {
        // Per-file SHA256 (streamed, 8 MiB chunks — Swift `fileDigest`), then fold
        // `relpath || 0x00 || file_sha256(32 raw) || 0x00` into the tree hasher.
        let (file_sha, size) = sha256_file_streaming(path)?;
        byte_count += size as i64;
        file_count += 1;
        tree.update(rel.as_bytes());
        tree.update([0u8]);
        tree.update(file_sha);
        tree.update([0u8]);
    }
    // #58: the tree hash is STREAMED (multi-GB safetensors are never buffered whole), so it
    // cannot use the one-shot `sha256_hex` — but it renders through the same `hex_lower` the
    // one-shot helper uses, so both spell a digest identically.
    let sha256 = bench_core::hash::hex_lower(&tree.finalize());
    Ok(DirDigest {
        sha256,
        byte_count,
        file_count,
    })
}

/// Stream a file's SHA256 in 8 MiB chunks (Swift `fileDigest` chunk size), returning the
/// 32 raw digest bytes and the byte count. Streaming avoids loading multi-GB safetensors.
fn sha256_file_streaming(path: &Path) -> io::Result<([u8; 32], u64)> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    Ok((out, size))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

// --- Session -> EngineHandle adapter ---------------------------------------

/// Adapts a worker [`Session`] to the bench-core [`EngineHandle`] the conformance
/// gate needs: `correctness_begin`/`correctness_step` for teacher-forced primary + anchor
/// cases, and `correctness` for free runs. Also carries the per-sequence barrier: each
/// correctness sequence is its own sub-phase (see `drain_allocator`).
pub(crate) struct SessionEngine<'a, T: LineTransport> {
    pub(crate) session: &'a mut Session<T>,
    /// Whether `drain_allocator` has opened at least one sub-phase yet (so the first
    /// call opens without a preceding close).
    pub(crate) drained_once: bool,
}

fn to_bench_err(e: RunnerError) -> BenchError {
    BenchError::InvalidInput(format!("{e}"))
}

/// Convert a teacher-forced worker response (`correctness_begin`/`correctness_step`) into
/// the bench-core `AnchorOutput` (argmax token + top-k logits) the gates evaluate.
fn resp_to_anchor_output(resp: bench_protocol::WorkerResponse) -> Result<AnchorOutput, BenchError> {
    let token = resp.token.ok_or_else(|| {
        BenchError::InvalidInput("teacher-forced response missing token".to_string())
    })?;
    let top_logits = resp
        .top_logits
        .unwrap_or_default()
        .into_iter()
        .map(|l| TopLogit {
            token: l.token,
            logit: l.logit,
        })
        .collect();
    Ok(AnchorOutput { token, top_logits })
}

impl<T: LineTransport> EngineHandle for SessionEngine<'_, T> {
    fn anchor_forward(&mut self, context_tokens: &[Token]) -> Result<AnchorOutput, BenchError> {
        let resp = self
            .session
            .correctness_begin(context_tokens)
            .map_err(to_bench_err)?;
        resp_to_anchor_output(resp)
    }

    fn free_run(
        &mut self,
        prompt_tokens: &[Token],
        steps: usize,
    ) -> Result<Vec<Token>, BenchError> {
        let resp = self
            .session
            .correctness(prompt_tokens, steps as i64)
            .map_err(to_bench_err)?;
        Ok(resp.tokens.unwrap_or_default())
    }

    fn teacher_forced(
        &mut self,
        prompt_tokens: &[Token],
        forced_tokens: &[Token],
        steps: usize,
    ) -> Result<Vec<AnchorOutput>, BenchError> {
        // B3 / Swift compareTeacherForcedWithWorker: begin(prompt) -> prediction 0, then
        // feed the GOLDEN token each step -> prediction i+1, for `steps` predictions.
        let mut outputs = Vec::with_capacity(steps);
        let first = self
            .session
            .correctness_begin(prompt_tokens)
            .map_err(to_bench_err)?;
        outputs.push(resp_to_anchor_output(first)?);
        for &forced in forced_tokens.iter().take(steps.saturating_sub(1)) {
            let resp = self
                .session
                .correctness_step(forced)
                .map_err(to_bench_err)?;
            outputs.push(resp_to_anchor_output(resp)?);
        }
        Ok(outputs)
    }

    /// V1: route the per-sequence anti-memoization drain to a real worker. Each correctness
    /// sequence is its own barrier sub-phase: on the boundary BEFORE a new sequence, close
    /// the previous sub-phase — `phase_diagnostics` reaches the worker (a conformant engine
    /// drains its allocator at the sequence boundary) and the completed-work barrier is
    /// reconciled for the sequence just finished — then open the next. The first call only
    /// opens (nothing precedes it). The final sub-phase is closed by iterate_core after the
    /// gate, before prefill, so no correctness completed-work leaks into the timed phases.
    /// (Verifying the engine reports `cacheMemory == 0` needs a new response field and is a
    /// tracked follow-up; this makes the drain a real worker round-trip instead of a no-op.)
    fn drain_allocator(&mut self) -> Result<(), BenchError> {
        if self.drained_once {
            self.session.close_phase().map_err(to_bench_err)?;
        }
        self.drained_once = true;
        self.session.begin_phase();
        Ok(())
    }
}

// --- iterate core ----------------------------------------------------------

/// Run the full iterate flow and return the assembled payload. Pure over the transport,
/// so tests drive it with an in-process `MockEngine`.
///
/// Engine lifecycle is a PER-MODE property, matching Swift exactly (§A):
/// - The correctness gate always runs on the shared `session` (Swift `--local-iterate`
///   derives correctness from the timing pass and has no separate correctness worker;
///   benchd's gate is a superset, so it keeps one session for it).
/// - LOCAL-ITERATE timing spawns a FRESH engine PER timed phase via `spawn_timed`
///   (prefill worker, then decode worker), mirroring Swift
///   `runLocalIterateCheckedTimingWithWorker`
///   (mlxfast-challenge-dev/Sources/MLXFastHarness/QwenRuntimeLocalIterate.swift:704/750).
/// - OFFICIAL timing stays on the single shared `session` (single-worker path; its
///   lifecycle parity is §B).
///
/// `spawn_timed` yields a freshly-connected [`Session`] (post-hello). It is invoked
/// exactly twice on the local-iterate path (once per timed phase) and not at all on a
/// run that fails before timing. `cool_gate` runs the local GPU cool-down gate before each
/// timed phase (Swift `runLocalPhaseCoolGate`, `QwenRuntimeLocalIterate.swift:708`/`:754`);
/// it returns `Err` on a thermal abort, which fails the run. Tests pass a no-op.
///
/// R3 (`strict`): native `--local-iterate` correctness is Swift-exact BY DEFAULT — the
/// gate evaluates ONLY the primary teacher-forced `cases[]` (Swift `localIterate` derives
/// correctness solely from the `cases.first` timing stream and never checks
/// `correctness_gates.anchors` / `.free_run` / `.behavior`). Passing `strict = true`
/// restores benchctl's historical SUPERSET (base cases + anchors + free-run). Official
/// mode always runs the full superset regardless of `strict` (out of R3 scope).
#[allow(clippy::too_many_arguments)]
pub fn iterate_core<T, F, G>(
    session: &mut Session<T>,
    _hello: &Hello,
    golden: &GoldenFixture,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
    mode: Mode,
    strict: bool,
    digests: RunDigests<'_>,
    mut spawn_timed: F,
    mut cool_gate: G,
) -> ScorePayload
where
    T: LineTransport,
    F: FnMut() -> bench_runner::Result<Session<T>>,
    G: FnMut(&str) -> bench_runner::Result<()>,
{
    // 1. Correctness gate (primary teacher-forced cases + anchor + free-run) via bench-core
    // conformance. B1+V1: each correctness sequence's teacher-forced steps
    // (correctness_begin/step) are timed steps that bump the engine's completed_work, and
    // each sequence must be drained. `SessionEngine::drain_allocator` runs each sequence in
    // its own barrier sub-phase (close-previous-then-open-next via phase_diagnostics); the
    // LAST sub-phase is closed here, after the gate, before prefill — otherwise a
    // sequence's completed-work leaks into prefill's close_phase and trips the WS1-7 barrier
    // against a conformant engine. Swift's monolith runs correctness in-process with no
    // benchd↔engine split, so this bracketing is benchd-specific, not a Swift port.
    // R3: Swift-exact by default for the local checked-timing modes (local-iterate AND
    // local-submit: base cases ONLY); the superset is opt-in via `--strict`. Official always
    // runs the full superset.
    // Swift-exact by default for the local checked-timing modes (local-iterate AND
    // local-submit are both `QwenRuntime.localIterate`, which derives correctness solely from
    // the primary `cases.first` timing stream): evaluate base cases ONLY. `--strict` restores
    // benchctl's superset (base cases + anchors + free-run). Official always runs the superset.
    let scope = if mode.is_local_checked_timing() && !strict {
        CorrectnessScope::BaseCasesOnly
    } else {
        CorrectnessScope::Full
    };
    let report = {
        let mut adapter = SessionEngine {
            session: &mut *session,
            drained_once: false,
        };
        match run_conformance(
            &mut adapter,
            golden,
            bench_core::constants::CORRECTNESS_STEPS,
            scope,
        ) {
            Ok(r) => r,
            Err(e) => {
                return failed_payload(
                    mode,
                    FailureReport::message(format!("{e}"), false),
                    golden,
                    digests,
                    None,
                    None,
                )
            }
        }
    };
    // #132(b) FINAL: the fail-path case count used to be computed here (the golden's TOTAL
    // correctness case count) and threaded into every blanked failure payload. Under
    // MIRROR-BLANK-STRICTLY it is zero on every one of those paths, so the binding is gone rather
    // than kept and ignored — the paths that DO report real counts derive their own from the
    // checked-timing pass, not from the golden's roster.

    // B1/V1 — ONE OWNER for the bracketing: close the LAST correctness sub-phase here,
    // reconciling the final sequence's timed steps and resetting the engine's completed_work
    // before prefill, on BOTH the pass exit AND the correctness-fail exit below (earlier
    // sub-phases were closed by drain_allocator between sequences; the conformance-Err exit
    // above already left a tainted, discarded session that cannot be closed). Do it before
    // branching on report.passed so a failed gate can never leak completed-work past here.
    if let Err(e) = session.close_phase() {
        return failed_payload(
            mode,
            FailureReport::message(format!("{e}"), true),
            golden,
            digests,
            None,
            None,
        );
    }

    if !report.passed {
        let failure = first_conformance_failure(&report);
        // §F1: local-iterate reports a base-case (`cases[]`) failure EXACTLY as Swift's fused
        // timing/correctness pass does — case = "local-iterate", checked-step numbering,
        // expected/actual populated, descriptive error. Anchor/free-run failures are the
        // benchctl SUPERSET (Swift `--local-iterate` skips those gates), so they keep
        // benchctl's own case/step report.
        let failure_report = match &failure {
            // Only WITHIN Swift's checked window (prefill/seed + `decode_steps` decode steps,
            // i.e. expected index ≤ decode_steps) does Swift see the token — beyond it Swift's
            // timing pass never reaches that index, so it PASSES and a base-case failure there
            // is the benchctl SUPERSET (falls through to the arm below, keeping the case id).
            // MODE-DERIVED (§F1 / M-6): keys on `mode.is_local_checked_timing()` (true for BOTH
            // local-iterate AND local-submit) and brands with `mode.mode_name()` — Swift's
            // branding uses `modeName`, so submit's case id / error read "local-submit".
            Some(f)
                if mode.is_local_checked_timing()
                    && f.is_base_case
                    && f.step.is_some_and(|i| i as usize <= mode.decode_steps()) =>
            {
                FailureReport::correctness(
                    format!("{} teacher-forced token mismatch", mode.mode_name()),
                    Some(mode.mode_name().to_string()),
                    f.step.map(|i| swift_local_iterate_checked_step(i as usize)),
                    f.expected,
                    f.actual,
                )
            }
            // The benchctl SUPERSET report: the error IS the failing case's name.
            Some(f) => FailureReport::correctness(
                f.case.clone(),
                Some(f.case.clone()),
                f.step,
                f.expected,
                f.actual,
            ),
            None => FailureReport::correctness("correctness gate failed", None, None, None, None),
        };
        // ITEM 1 (David's ruling — "Swift is the reference"): on a LOCAL-ITERATE correctness
        // FAILURE, RETAIN real timing/baselines/speedup-floor flags instead of blanking them
        // to 0/false. Swift's `--local-iterate` measures timing by TEACHER-FORCING the expected
        // tokens and timing wall-clock — correctness is a SEPARATE judgment — so a corrupted
        // cases[0] still yields real `decode_spt`, real baselines, and real floor flags. benchd
        // must match: run the TIME-ONLY (mismatch-tolerant) timing phase and emit a
        // failed-WITH-real-timing payload. Official/Track-B keep the blanked failed payload.
        if mode.is_local_checked_timing() {
            match local_iterate_timing_params(golden, mode) {
                // The `expected_tokens.len() <= decode_steps` early-fail genuinely cannot time
                // (no full window to teacher-force), so it keeps the blanked failed payload.
                Err(_too_short) => {
                    return failed_payload(mode, failure_report, golden, digests, None, None);
                }
                Ok(params) => {
                    match run_timed_benchmark_fresh_per_phase_time_only(
                        &mut spawn_timed,
                        &mut cool_gate,
                        &params,
                    ) {
                        Ok(timing) => {
                            return failed_with_real_timing_payload(
                                mode,
                                failure_report,
                                golden,
                                digests,
                                &timing,
                                baseline_prefill_spt,
                                baseline_decode_spt,
                            );
                        }
                        // The time-only timing pass itself failed (protocol / thermal abort /
                        // completed-work barrier) — those are real infra faults, not a token
                        // mismatch. Fall back to the blanked failed payload; the correctness
                        // failure is still reported.
                        Err(_timing_err) => {
                            return failed_payload(
                                mode,
                                failure_report,
                                golden,
                                digests,
                                None,
                                None,
                            );
                        }
                    }
                }
            }
        }

        // #132/F-5 — UNREACHABLE in practice, and deliberately kept. Every arm of the
        // `mode.is_local_checked_timing()` block above returns, so this fall-through needs a mode
        // that is neither local-iterate nor local-submit — i.e. Official, which `execute_iterate`
        // routes to `official::official_core` and which `iterate_core` `unreachable!()`s on two
        // matches below. So it has NO test: there is no way to reach it without first defeating
        // that routing, and a test that defeated the routing would be pinning a fiction.
        //
        // It is covered structurally instead: it calls the same `failed_payload`, which applies
        // the #132(b) blank seal unconditionally, so it cannot diverge from the six tested sites
        // even if a future mode makes it live.
        return failed_payload(mode, failure_report, golden, digests, None, None);
    }

    // 2. Timing (WS1-6). RULING 1: LOCAL-ITERATE times cases[0] EXACTLY as Swift
    // `--local-iterate` (`runLocalIterateCheckedTimingWithWorker`): prefill cases[0].
    // prompt_tokens -> expected_tokens[0]; decode SEEDS with the same prompt_tokens, then
    // teacher-forces expected_tokens[1..=decode_steps] — the F1 token verification applies
    // unchanged to this aligned stream. The benchmark-oracle timing (golden.benchmark) is the
    // OFFICIAL/submit workload (Swift's official benchmark) and is kept, gated to Official.
    let params = match mode {
        // local-submit reuses the local-iterate checked-timing machinery (Swift
        // `QwenRuntime.localIterate` with `decodeSteps = 1023`), so it times `cases[0]`
        // identically — only `decode_steps` differs (via `mode.decode_steps()`).
        Mode::LocalIterate | Mode::LocalSubmit => match local_iterate_timing_params(golden, mode) {
            Ok(p) => p,
            // The `expected_tokens.len() <= decode_steps` early-fail (Swift guard): a genuine
            // "can't time" — correctness PASSED here, so `passed_correctness = true`.
            Err(e) => {
                return failed_payload(
                    mode,
                    FailureReport::message(e, true),
                    golden,
                    digests,
                    None,
                    None,
                )
            }
        },
        // Official is NOT a local checked-timing run: it uses the timed-FIRST, three-fresh-
        // sandboxed-worker flow in `crate::official::official_core` (B-2), never iterate_core.
        // The CLI routes official there; iterate_core is local-iterate / local-submit only.
        Mode::Official => {
            unreachable!("official mode must run through official::official_core, not iterate_core")
        }
    };
    // §A — engine lifecycle is a PER-MODE property mirroring Swift exactly.
    let timing_result = match mode {
        // LOCAL-ITERATE: a FRESH engine process per timed phase (Swift `--local-iterate`
        // spawns prefillWorker at QwenRuntimeLocalIterate.swift:704 and a separate
        // decodeWorker at :750). Neither timed phase inherits the warm correctness
        // session's graph/allocator caches — that persistent-session warmth was the
        // residual ~1.6% gap in the aligned run. The cool gate runs before each timed
        // phase (Swift :708/:754) so neither phase times a hot/sequencing-warmed GPU.
        // local-submit shares this fresh-engine-per-phase lifecycle (it IS
        // `QwenRuntime.localIterate`, just with a 1023-step decode window).
        Mode::LocalIterate | Mode::LocalSubmit => {
            run_timed_benchmark_fresh_per_phase(&mut spawn_timed, &mut cool_gate, &params)
        }
        // Official never reaches iterate_core (see the params match above).
        Mode::Official => {
            unreachable!("official mode must run through official::official_core, not iterate_core")
        }
    };
    let timing = match timing_result {
        Ok(t) => t,
        Err(e) => {
            return failed_payload(
                mode,
                FailureReport::message(format!("{e}"), true),
                golden,
                digests,
                None,
                None,
            )
        }
    };

    // 3. Assemble the local estimated score.
    local_iterate_score(
        mode,
        &timing,
        baseline_prefill_spt,
        baseline_decode_spt,
        golden,
        digests,
    )
}

/// Build the LOCAL-ITERATE timing params from cases[0], EXACTLY as Swift `--local-iterate`
/// (`runLocalIterateCheckedTimingWithWorker`): prefill cases[0].prompt_tokens → expected[0];
/// decode SEEDS with the same prompt, then teacher-forces expected_tokens[1..=decode_steps].
/// Returns `Err(message)` for the Swift guard `expected_tokens.count > decode_steps` (a full
/// window is required to time) — that case genuinely cannot be timed and stays a hard fail.
///
/// Shared by the correctness-PASS timing path AND the correctness-FAILURE time-only path
/// (ITEM 1): the timed workload is identical; only whether tokens are VERIFIED differs.
fn local_iterate_timing_params(
    golden: &GoldenFixture,
    mode: Mode,
) -> std::result::Result<TimingParams, String> {
    let case = &golden.cases[0]; // the loader guarantees a non-empty cases[]
    let decode_steps = mode.decode_steps();
    if case.expected_tokens.len() <= decode_steps {
        return Err(format!(
            "primary case {} has {} expected_tokens; local-iterate timing needs more than {decode_steps}",
            case.name,
            case.expected_tokens.len()
        ));
    }
    let seed_token = case.expected_tokens[0];
    let decode_tokens = case.expected_tokens[1..=decode_steps].to_vec();
    Ok(TimingParams::new(
        case.prompt_tokens.clone(),
        seed_token,
        case.prompt_tokens.clone(),
        seed_token,
        decode_tokens,
        decode_steps,
    ))
}

/// The shared timing→metrics body: writes the measured decode/prefill seconds-per-token, the
/// external Qwen baselines, the derived speedups, and the speedup-floor flags into `metrics`.
///
/// Factored out of `local_iterate_score` so BOTH the correctness-PASS score builder AND the
/// correctness-FAILURE builder (`failed_with_real_timing_payload`, ITEM 1) populate these
/// fields identically — the whole point of David's ruling is that a correctness failure must
/// carry the SAME real timing surface as a pass, not blanked zeros. Does NOT touch
/// `case_count`/`checked_steps` (those differ pass vs. fail) or `passed_correctness`/`error`.
pub(crate) fn apply_timing_metrics(
    metrics: &mut ScoreMetrics,
    timing: &TimingResult,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
) {
    metrics.peak_ram_gb = finite_nonneg(timing.peak_ram_gb);
    metrics.bandwidth_gb_per_token = 0.0;
    metrics.bandwidth_source = BANDWIDTH_SOURCE.to_string();
    metrics.decode_seconds_per_token = finite_nonneg(timing.decode_seconds_per_token);
    metrics.prefill_seconds_per_token = finite_nonneg(timing.prefill_seconds_per_token);
    metrics.baseline_decode_seconds_per_token = finite_nonneg(baseline_decode_spt);
    metrics.baseline_prefill_seconds_per_token = finite_nonneg(baseline_prefill_spt);
    let decode_speedup = speedup(baseline_decode_spt, timing.decode_seconds_per_token);
    let prefill_speedup = speedup(baseline_prefill_spt, timing.prefill_seconds_per_token);
    metrics.decode_speedup = decode_speedup;
    metrics.prefill_speedup = prefill_speedup;
    metrics.passed_decode_speedup_floor = decode_speedup >= SCORE_DECODE_SPEEDUP_FLOOR;
    metrics.passed_prefill_speedup_floor = prefill_speedup >= SCORE_PREFILL_SPEEDUP_FLOOR;
    let timed = timing.prefill_elapsed_seconds + timing.decode_elapsed_seconds;
    metrics.timed_benchmark_seconds = finite_nonneg(timed);
    metrics.benchmark_wall_seconds = finite_nonneg(timed);
}

/// A FAILED payload that RETAINS the real timing surface (ITEM 1 / David's ruling): a
/// correctness FAILURE on the local-iterate path still carries the measured decode/prefill
/// seconds-per-token, the real baselines, the derived speedups, and the real speedup-floor
/// flags (via `apply_timing_metrics`) — NOT blanked to 0/false as `failed_payload` does —
/// while still reporting `passed = false`, `passed_correctness = false`, and the correctness
/// `error` + `first_failing_*` fields. `score` stays `null` (a failed run has no score).
fn failed_with_real_timing_payload(
    mode: Mode,
    failure: FailureReport,
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    timing: &TimingResult,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
) -> ScorePayload {
    let mut metrics = base_metrics(mode, golden, digests);
    apply_timing_metrics(
        &mut metrics,
        timing,
        baseline_prefill_spt,
        baseline_decode_spt,
    );
    // Swift's LOCAL-ITERATE FAILURE report (QwenRuntimeLocalIterate.swift:636-644), NOT the
    // standalone-correctness counts: `caseCount = timingRepeats`, and
    // `checkedSteps = failureStep + 1` (else the full checked-timing count). benchctl's
    // `first_failing_step` IS Swift's `failureStep`, so checked_steps = first_failing_step + 1.
    // Both are Det-gated, so this is what closes the local-iterate failure DET surface (#66/Fable
    // + item-1 red-team Finding 3): before this, benchctl reported golden-total and diverged.
    //
    // `passed_correctness` is false by construction: this builder is only ever reached with a
    // `FailureReport::correctness`, which cannot claim otherwise.
    metrics.passed_correctness = failure.passed_correctness;
    // #134 — SEAL BOUNDARY. `failure.error` is engine-controlled text (a `RunnerError` Display,
    // which since #134 carries the worker's own stderr tail). Scrub secrets and cap it here, at
    // the one point where it stops being a log line and becomes a persisted artifact.
    metrics.error = scrub_reason_for_seal(&failure.error);
    metrics.case_count = TIMING_REPEATS;
    metrics.checked_steps = failure
        .step
        .map(|s| s + 1)
        .unwrap_or((mode.decode_steps() as i64 + 2) * TIMING_REPEATS);
    metrics.first_failing_case = failure.case;
    metrics.first_failing_step = failure.step;
    metrics.expected_token = failure.expected_token;
    metrics.actual_token = failure.actual_token;
    ScorePayload {
        score: None,
        passed: false,
        metrics,
    }
}

/// Port of Swift `localIterateScore`: publish the estimated
/// decode_speedup^0.75 * prefill_speedup^0.25 score. Returns a failed payload
/// (score `null`) if any timing/baseline/score value is non-finite or non-positive.
pub fn local_iterate_score(
    mode: Mode,
    timing: &TimingResult,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
) -> ScorePayload {
    let decode_spt = timing.decode_seconds_per_token;
    let prefill_spt = timing.prefill_seconds_per_token;
    let est = score_default_weights(
        decode_spt,
        prefill_spt,
        baseline_decode_spt,
        baseline_prefill_spt,
    );

    let valid = decode_spt.is_finite()
        && decode_spt > 0.0
        && prefill_spt.is_finite()
        && prefill_spt > 0.0
        && baseline_decode_spt.is_finite()
        && baseline_decode_spt > 0.0
        && baseline_prefill_spt.is_finite()
        && baseline_prefill_spt > 0.0
        && est.is_finite()
        && est > 0.0;

    let mut metrics = base_metrics(mode, golden, digests);
    // Shared timing→metrics body (decode/prefill spt, baselines, speedups, floor flags) —
    // identical to the correctness-FAILURE builder, so a fail retains the same real surface.
    apply_timing_metrics(
        &mut metrics,
        timing,
        baseline_prefill_spt,
        baseline_decode_spt,
    );
    // Report the CHECKED-TIMING counts, not the conformance case counts. Swift
    // `runLocalIterateCheckedTimingWithWorker` reports `caseCount = timingRepeats` and
    // `checkedSteps = (decodeSteps + 2) * timingRepeats` (the +2 is the measured prefill
    // token plus the decode seed forward). parity-diff.py lists both in the deterministic
    // must-match set, so anything else fails parity against a correct engine.
    metrics.case_count = TIMING_REPEATS;
    metrics.checked_steps = (mode.decode_steps() as i64 + 2) * TIMING_REPEATS;

    if !valid {
        metrics.passed_correctness = true;
        metrics.error =
            "local estimated score is invalid: timing metrics and external Qwen baselines must be finite and positive"
                .to_string();
        return ScorePayload {
            score: None,
            passed: false,
            metrics,
        };
    }

    metrics.passed_correctness = true;
    ScorePayload {
        score: Some(est),
        passed: true,
        metrics,
    }
}

/// Why a run failed, in the shape the score's metrics report it.
///
/// #65: these five values used to be threaded through every failure path as a positional
/// `(fc, fs, exp, act, err)` tuple and then as five more parameters on `failed_payload`,
/// where nothing but argument order said which `Option<i64>` was the expected token and
/// which was the actual. Naming them removes that whole class of transposition.
///
/// `passed_correctness` rides along because it is part of the same story: whether the
/// correctness gate had already passed when this failure happened. The two constructors
/// encode the invariant every call site already obeyed — a failure carrying a case identity
/// IS a correctness failure, so it can never claim `passed_correctness = true`.
#[derive(Debug, Clone, Default)]
pub(crate) struct FailureReport {
    /// `metrics.error` — the human-readable refusal, byte-matched to Swift where it must be.
    pub error: String,
    /// Whether the correctness gate itself passed before this (later) failure.
    pub passed_correctness: bool,
    /// `metrics.first_failing_case`.
    pub case: Option<String>,
    /// `metrics.first_failing_step`, in Swift's checked-step numbering.
    pub step: Option<i64>,
    /// `metrics.expected_token` — what the golden said.
    pub expected_token: Option<i64>,
    /// `metrics.actual_token` — what the engine emitted.
    pub actual_token: Option<i64>,
}

impl FailureReport {
    /// A failure with a message only and no per-case identity: a protocol/transport fault, a
    /// preflight refusal, or a "cannot time this golden" guard. `passed_correctness` says
    /// whether the correctness gate had already passed when it happened.
    fn message(error: impl Into<String>, passed_correctness: bool) -> Self {
        FailureReport {
            error: error.into(),
            passed_correctness,
            ..Default::default()
        }
    }

    /// A CORRECTNESS failure: the failing case, the checked step, and the diverging token
    /// pair. `passed_correctness` is necessarily false — this IS the gate failing.
    fn correctness(
        error: impl Into<String>,
        case: Option<String>,
        step: Option<i64>,
        expected_token: Option<i64>,
        actual_token: Option<i64>,
    ) -> Self {
        FailureReport {
            error: error.into(),
            passed_correctness: false,
            case,
            step,
            expected_token,
            actual_token,
        }
    }
}

/// Swift's §F2 missing-paired-baselines refusal, minus the leading mode name.
///
/// Port of `QwenRuntime.requiredGoldenBenchmarkBaselines(_:context:)`, where the message is
/// built as three concatenated string literals and `context` is interpolated at the FRONT.
/// #62: this used to be an inline byte-literal in `execute_iterate` that only a
/// `starts_with` assertion guarded, so drift in the TAIL could ship. It is now single-sourced
/// here and pinned in full against a fixture captured from the Swift source
/// (`crates/benchctl/tests/fixtures/swift-missing-paired-baselines.json`).
const MISSING_PAIRED_BASELINES_SUFFIX: &str =
    " requires external Qwen benchmark baselines for both prefill and decode; \
     Gemma baseline constants are not valid for Qwen";

/// The full Swift refusal for `mode` — `"<mode-name><suffix>"`, byte-for-byte what Swift
/// throws when a golden carries neither paired baseline and no override supplies them.
pub fn missing_paired_baselines_error(mode: Mode) -> String {
    format!("{}{MISSING_PAIRED_BASELINES_SUFFIX}", mode.mode_name())
}

/// The baseline pair a LOCAL checked-timing run (`local-iterate` / `local-submit`) scores
/// against: the compile-time official-runner constants, never the golden.
///
/// **#127 — RULED (David 2026-08-20, "MIRROR REFERENCE"):** the reference's `localIterate` reads
/// `MLXFastConstants.officialBaseline{Prefill,Decode}SecondsPerToken` directly
/// (`QwenRuntimeLocalIterate.swift@b26f76f:34,36`, used at `:291,317,382,386`) and never consults
/// the golden's `benchmark.baseline_*_seconds_per_token` — it does not even reach the
/// `resolvedBaseline*` accessor its own official path uses. benchd used to source the pair from
/// the golden, so a golden declaring any pair silently rescored benchd while leaving the reference
/// untouched (the §8 window split ~12× on score off a ~5% timing split, because the golden carried
/// the retired fork's Gemma-era constants). Under the ruling the golden's pair is INERT LEGACY
/// DATA on this leg: not required, not validated against these values, simply ignored.
///
/// Both local modes share this because the reference routes both through that one function
/// (`main.swift@b26f76f:315-340`); only the OFFICIAL path stays golden-authoritative.
pub fn local_mode_baselines() -> (f64, f64) {
    (
        OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
        OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
    )
}

/// The reference's EARLY-REFUSE failure record: a refusal that happens BEFORE anything ran
/// (`score = null`, `passed = false`, `passed_correctness = false`) — e.g. §F2's missing
/// required baselines.
///
/// **#74 — RULED (David 2026-08-20, "MIRROR SWIFT EXACTLY"):** when the run refuses before it
/// runs, benchd seals the reference's record and nothing more. Swift derives all three
/// run-shaped fields from the `CorrectnessReport`, which on this path is `nil`, so they seal
/// EMPTY/ZERO — `goldenHash: correctness?.goldenHash ?? ""`, `checkedSteps:
/// correctness?.checkedSteps ?? 0`, `caseCount: correctness?.caseCount ?? 0`
/// (`QwenRuntimeBenchmark.swift@b26f76f:1161-1162,1176`, reached from
/// `QwenRuntimeLocalIterate.swift@b26f76f:40-74,197-198`). benchd used to seal the golden's
/// digest and the golden's TOTAL case count here, so a refusal that never ran anything still
/// read like a run that had (`golden_hash` present, `case_count`/`checked_steps` = 4 where the
/// reference sealed `""`/0/0) — the fail-POINT divergence #74 records.
///
/// **Who actually reaches this, and why the baseline pair is NOT zeroed.** Since the #127 ruling
/// the local legs take their baselines from the constants and never refuse for want of them, so the
/// only live caller is the OFFICIAL arm — and the official path is where the justification has to
/// come from. It holds there directly: this refusal fires exactly where the reference's official
/// path would have fallen back to the constants, because `resolvedBaseline*` is
/// `baseline* ?? officialBaseline*` (`Golden.swift@b26f76f:220-226`, consumed at
/// `QwenRuntimeBenchmark.swift@b26f76f:155-157,443-445`). benchd refuses instead of falling back
/// (deliberate ranked-path strictness — see `resolve_paired_baselines`), but the DENOMINATOR the
/// reference would have used at that same point is the constant pair, which is what the record
/// seals. The reference's own official refusal agrees independently: its
/// `baseline*SecondsPerToken` locals are initialised to `MLXFastConstants.officialBaseline*`
/// (`QwenRuntimeBenchmark.swift@b26f76f:33-34,350-351`) and any failure before the timed phase
/// seals them unchanged.
///
/// The LOCAL path reaches the same values by its own route — `failedScore`'s baseline parameters
/// DEFAULT to those constants (`QwenRuntimeBenchmark.swift@b26f76f:1130-1131`) and the local
/// refusal site passes no override (`QwenRuntimeLocalIterate.swift@b26f76f:49-68` names every
/// argument it forwards; the baselines are not among them). Both paths agree, which is why one
/// record shape serves both. The pre-#74 comment here asserted the opposite ("Swift emits
/// `baseline_* = 0`"), which was true of the RETIRED `mlxfast-challenge-dev` fork — it held
/// mutable `baseline*SecondsPerToken` locals initialised to `0.0` and forwarded them explicitly —
/// and is false of the reference this repo now mirrors.
///
/// NOTE that this therefore CHANGES THE OFFICIAL REFUSAL ARTIFACT too: an official run that
/// refuses for want of a baseline pair used to seal the golden's digest and case count and a zero
/// baseline pair, and now seals the reference's empty record with the constant pair. That is
/// #74's ruling applied where its only live caller is, not a widening of it — but "the official
/// path is untouched" is a statement about #127's baseline SOURCING, not about this artifact.
///
/// The whole record is pinned byte-for-byte against a constructed reference capture by
/// `tests::early_refuse_record_byte_matches_the_reference_capture`.
pub fn preflight_failed_payload(
    mode: Mode,
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    error: String,
) -> ScorePayload {
    // Neither the blank seal nor the baseline pair needs a local override any more. `failed_payload`
    // blanks `golden_hash`/counts on EVERY local failure path (#132(b), FINAL), and `base_metrics`
    // seals the reference's baseline constants on every one (#132(a)). This function used to carry
    // both overrides, which is precisely how the rest of the surface drifted away from it.
    failed_payload(
        mode,
        FailureReport::message(error, false),
        golden,
        digests,
        None,
        None,
    )
}

/// A failed payload (`score = null`, `passed = false`) for a local failure on which the
/// REFERENCE's fused checked-timing pass did not complete — the blanked local failure surface.
///
/// The criterion is the REFERENCE's, not benchd's, and the difference is not cosmetic: at `:455`
/// and `:581` benchd's OWN conformance report exists (its gate ran separately and passed), so
/// "produced no correctness report" would be false of benchd at those two sites while the blank
/// is still correct. What decides is whether the reference — which derives correctness solely
/// from `runLocalIterateCheckedTiming*` — would have a `CorrectnessReport` at the equivalent
/// point. It does not: `correctnessReport` is assigned only at
/// `QwenRuntimeLocalIterate.swift@b26f76f:152`, after that pass returns.
///
/// **RULED (David 2026-08-20, interview, FINAL) — #132(b): MIRROR BLANK STRICTLY.** Every local
/// failure path that reaches here seals `golden_hash = ""` and zero counts, byte-matching the
/// reference. Zero DECLARED cells on this surface.
///
/// This REVERSES an earlier same-day ruling (keep-real-values + declare) once the field semantics
/// were laid out: the reference's `goldenHash` carries an INVARIANT — non-empty means correctness
/// completed — because it is only ever populated from a `CorrectnessReport` that exists
/// (`QwenRuntimeBenchmark.swift@b26f76f:1161-1162,1176` read `?? ""` / `?? 0` off a nil report).
/// Sealing benchd's real digest on a path where correctness did NOT complete is not extra
/// information, it is the same field meaning something weaker, and every downstream consumer that
/// trusts the invariant silently inherits the weakening. benchd's real data lives in LOGS.
/// A benchd-only provenance superset field (`loaded_golden_sha256` or similar) was explicitly
/// considered and REJECTED: failure records carry the reference's fields, and nothing else.
///
/// `case_count` is deliberately NOT a parameter. It was one, and every caller passed the golden's
/// roster into a record that describes no run; making the zero structural is what stops that
/// coming back. The paths that DO have a report build their payloads elsewhere
/// ([`failed_with_real_timing_payload`], [`local_iterate_score`]) and keep their real values —
/// the reference keeps its there too, because on that arm its `correctnessReport` IS set
/// (`QwenRuntimeLocalIterate.swift@b26f76f:152`, checked at `:164-174`).
fn failed_payload(
    mode: Mode,
    failure: FailureReport,
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    decode_spt: Option<f64>,
    prefill_spt: Option<f64>,
) -> ScorePayload {
    let mut metrics = base_metrics(mode, golden, digests);
    metrics.passed_correctness = failure.passed_correctness;
    // #134 — SEAL BOUNDARY. `failure.error` is engine-controlled text (a `RunnerError` Display,
    // which since #134 carries the worker's own stderr tail). Scrub secrets and cap it here, at
    // the one point where it stops being a log line and becomes a persisted artifact.
    metrics.error = scrub_reason_for_seal(&failure.error);
    // The blank seal. No caller may opt out — see the doc comment above.
    metrics.golden_hash = String::new();
    metrics.case_count = 0;
    metrics.checked_steps = 0;
    metrics.first_failing_case = failure.case;
    metrics.first_failing_step = failure.step;
    metrics.expected_token = failure.expected_token;
    metrics.actual_token = failure.actual_token;
    if let Some(d) = decode_spt {
        metrics.decode_seconds_per_token = finite_nonneg(d);
    }
    if let Some(p) = prefill_spt {
        metrics.prefill_seconds_per_token = finite_nonneg(p);
    }
    ScorePayload {
        score: None,
        passed: false,
        metrics,
    }
}

/// Baseline metrics with the constant / default fields filled (Swift ScoreMetrics
/// defaults). Callers overwrite the fields their phase measured.
///
/// **The baseline pair starts at the official-runner CONSTANTS, not at zero (#132(a)).** This is
/// the same fact #74 ruled on, applied where it actually reaches: the reference has NO failure
/// path that emits `baseline_* = 0`. Every local failure — the conformance `Err`, the
/// `close_phase` `Err`, the time-only pass failing after a correctness failure, the
/// window-too-short guard, the timed pass failing — returns through the ONE `failed()` closure at
/// `QwenRuntimeLocalIterate.swift@b26f76f:40-74`, which forwards error/correctness/digests and
/// names no baseline argument, so `failedScore`'s defaults stand
/// (`QwenRuntimeBenchmark.swift@b26f76f:1130-1131`). The official side agrees independently: its
/// `baseline*SecondsPerToken` locals are INITIALISED to those constants
/// (`QwenRuntimeBenchmark.swift@b26f76f:33-34,350-351`), so a failure before the timed phase seals
/// them unchanged.
///
/// Setting them HERE rather than at each exit is deliberate: #74's fix landed only on
/// `preflight_failed_payload`, which left five locally-reachable sites still zeroing the pair
/// (#132) — a per-site override is exactly the shape that let them drift apart. Callers whose
/// phase actually resolved a pair overwrite these via `apply_timing_metrics`; callers that never
/// got that far now inherit the reference's values instead of a zero the reference never emits.
///
/// **The OFFICIAL path does not take this default (#132/F-2).** There the reference overwrites its
/// baseline locals with `pairedBaseline ?? benchmarkGolden.resolvedBaseline*`
/// (`QwenRuntimeBenchmark.swift@b26f76f:442-445`) before the gates-only branch and before every
/// failure record, so those payloads carry the RESOLVED pair, not these constants. The three
/// official sites nothing else overwrites set it explicitly — see
/// `official::official_resolved_baselines`. This default is the LOCAL surface's answer.
///
/// NOTE this does NOT touch `golden_hash` / `case_count` / `checked_steps`, and must not.
/// **RULED (David 2026-08-20, interview — FINAL) — #132(b): MIRROR BLANK STRICTLY.** Every local
/// failure path where the REFERENCE's fused checked-timing pass did not complete seals
/// `golden_hash = ""` and zero counts, byte-matching it; zero DECLARED cells on that surface. This
/// reverses an earlier same-day ruling (keep-real-values + declare) — the deciding point is that
/// the reference's `goldenHash` carries an INVARIANT, *non-empty means correctness completed*,
/// because it is only ever populated from a `CorrectnessReport` that exists
/// (`QwenRuntimeBenchmark.swift@b26f76f:1161-1162,1176` read `?? ""` / `?? 0` off a nil report).
/// Sealing benchd's real digest where correctness did not complete makes that field mean something
/// weaker for every consumer that trusts it. benchd's real data lives in LOGS.
///
/// The blank is applied by [`failed_payload`], not here, and that placement is load-bearing:
/// `base_metrics` also feeds the #73 RETAIN-TIMING arm and the PASSING path, where the reference
/// DOES seal real values because its `correctnessReport` is set
/// (`QwenRuntimeLocalIterate.swift@b26f76f:152`, checked at `:164-174`). Blanking here would take
/// both with it. Pinned by `assert_ruled_blank_seal` (per-site) and by
/// `correctness_failure_retains_what_early_refuse_seals_empty` (the arm that must NOT blank).
///
/// **F1 — `harness_hash` is SEALED HERE, on every path.** This is the one funnel every `iterate`
/// payload passes through — local-iterate, local-submit, official, official gates-only, and every
/// failure builder — so sealing the resolved [`HarnessIdentity`] here covers all modes with one
/// assignment. It replaces a `String::new()` stub that made `metrics.harness_hash` empty on every
/// benchd-authored score, which the seam-3 overlay correctly refuses to publish. The identity is
/// resolved FAIL-CLOSED before the run starts (see `HarnessIdentity::resolve_from_current_dir` and
/// its call site in `execute_iterate`), so by the time control reaches here a real digest exists
/// and cannot be `""`.
pub(crate) fn base_metrics(
    mode: Mode,
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
) -> ScoreMetrics {
    ScoreMetrics {
        peak_ram_gb: 0.0,
        bandwidth_gb_per_token: 0.0,
        decode_seconds_per_token: 0.0,
        prefill_seconds_per_token: 0.0,
        baseline_decode_seconds_per_token: OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
        baseline_prefill_seconds_per_token: OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
        decode_speedup: 0.0,
        prefill_speedup: 0.0,
        decode_speedup_floor: SCORE_DECODE_SPEEDUP_FLOOR,
        prefill_speedup_floor: SCORE_PREFILL_SPEEDUP_FLOOR,
        passed_decode_speedup_floor: false,
        passed_prefill_speedup_floor: false,
        benchmark_wall_seconds: 0.0,
        preflight_seconds: 0.0,
        correctness_seconds: 0.0,
        timed_benchmark_seconds: 0.0,
        gpqa_ttft_passed: false,
        gpqa_ttft_pass_count: 0,
        gpqa_ttft_case_count: 0,
        gpqa_ttft_seconds: 0.0,
        gpqa_ttft_p50_seconds: 0.0,
        gpqa_ttft_max_seconds: 0.0,
        gpqa_ttft_source: String::new(),
        semantic_gpqa_passed: false,
        semantic_gpqa_pass_count: 0,
        semantic_gpqa_case_count: 0,
        semantic_gpqa_model: String::new(),
        process_resident_memory_gb: 0.0,
        passed_correctness: false,
        num_layers: NUM_HIDDEN_LAYERS,
        checked_steps: 0,
        case_count: 0,
        expert_cache_hits: 0,
        expert_cache_misses: 0,
        expert_cache_evictions: 0,
        expert_bytes_read: 0,
        expert_read_seconds: 0.0,
        expert_peak_cached_tensors: 0,
        expert_hit_rate: 0.0,
        first_failing_layer: None,
        first_failing_case: None,
        first_failing_step: None,
        expected_token: None,
        actual_token: None,
        max_abs_diff: 0.0,
        golden_hash: golden.sha256.clone(),
        bandwidth_source: String::new(),
        error: String::new(),
        commit: String::new(),
        timestamp: iso8601_now(),
        harness_hash: digests.harness.as_str().to_string(),
        weights_hash: digests.weights.sha256.clone(),
        weights_byte_count: digests.weights.byte_count,
        weights_file_count: digests.weights.file_count,
        runtime: mode.runtime().to_string(),
        partial_result: false,
    }
}

pub(crate) fn finite_nonneg(v: f64) -> f64 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        0.0
    }
}

/// First failing (case name, step) from a conformance report, in Swift's layered
/// evaluation order: primary teacher-forced cases first (first mismatch step), then
/// anchors (no step), then free-run (first mismatch step).
pub(crate) struct ConformanceFailure {
    pub(crate) case: String,
    pub(crate) step: Option<i64>,
    pub(crate) expected: Option<i64>,
    pub(crate) actual: Option<i64>,
    /// A primary/`cases[]` (teacher-forced) failure — the one local-iterate reports
    /// Swift-style (§F1). Anchor/free-run failures are the benchctl superset.
    pub(crate) is_base_case: bool,
}

pub(crate) fn first_conformance_failure(report: &ConformanceReport) -> Option<ConformanceFailure> {
    if let Some(c) = report.base_cases.iter().find(|r| !r.passed) {
        return Some(ConformanceFailure {
            case: c.name.clone(),
            step: c.first_mismatch_step.map(|s| s as i64),
            expected: c.expected_token,
            actual: c.actual_token,
            is_base_case: true,
        });
    }
    if let Some(a) = report.anchors.iter().find(|r| !r.passed) {
        return Some(ConformanceFailure {
            case: a.name.clone(),
            step: None,
            expected: None,
            actual: None,
            is_base_case: false,
        });
    }
    if let Some(f) = report.free_run.iter().find(|r| !r.passed) {
        return Some(ConformanceFailure {
            case: f.name.clone(),
            step: f.first_mismatch_step.map(|s| s as i64),
            expected: None,
            actual: None,
            is_base_case: false,
        });
    }
    None
}

/// Map a benchctl teacher-forced base-case mismatch index (the `expected_tokens` index) to
/// Swift's local-iterate checked-step numbering (`QwenRuntimeLocalIterate.swift:777-797`):
/// prefill produces `expected_tokens[0]` at step 0; decode step k produces
/// `expected_tokens[k+1]` at checked-step `k+2`. So index 0 → 0 (prefill), index i≥1 → i+1.
fn swift_local_iterate_checked_step(expected_index: usize) -> i64 {
    if expected_index == 0 {
        0
    } else {
        expected_index as i64 + 1
    }
}

// --- timestamp -------------------------------------------------------------

/// UTC ISO-8601 `yyyy-MM-ddTHH:mm:ssZ` for "now" (matches Swift `ISO8601DateFormatter`).
pub(crate) fn iso8601_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_iso8601(secs)
}

fn format_iso8601(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Howard Hinnant's `civil_from_days` (epoch 1970-01-01 == day 0).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    use bench_core::constants::{
        BENCHMARK_PREFILL_PROMPT_TOKENS, CORRECTNESS_PROMPT_TOKENS,
        OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN, OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
        REQUIRED_GOLDEN_MODEL_TYPE,
    };
    // The golden-arity test calls the loader DIRECTLY, at two different consumer arities over
    // ONE byte string — the contract it pins is precisely that those two answers differ, which
    // a builder that owns the arity cannot express.
    use bench_core::golden::load_golden_fixture;
    use bench_core::score::evaluate_timed_run;
    use bench_runner::mock::MockEngine;
    use serde_json::json;

    use crate::testgolden::{benchmark_oracle, TestGolden};

    /// #134 — the `score.*.json` `metrics.error` SINK (the sibling of measure-job's
    /// `rejected_pairs[].reason`). A timed-phase spawn/handshake failure seals a `RunnerError`
    /// Display here, and since #134 that carries the worker's own stderr — so the seal boundary
    /// must scrub it. Secret-SHAPED without any `expected`/`actual` trigger word, so the
    /// pre-existing keyword filter would pass every byte of it through.
    #[test]
    fn failed_payload_scrubs_engine_text_before_sealing_metrics_error() {
        let weights = DirDigest {
            sha256: "w".repeat(64),
            file_count: 1,
            byte_count: 1,
        };
        let golden = TestGolden::new().fixture();
        let failure = FailureReport::message(
            format!(
                "engine hello handshake failed: protocol violation: engine closed the stream \
                 before returning a response (worker exited with status 9; worker stderr tail: \
                 open /Users/operator/pool-goldens/sample-001.json failed | \
                 AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCY | \
                 host=api.example.internal | {})",
                "P".repeat(8192)
            ),
            false,
        );
        let payload = failed_payload(
            Mode::LocalIterate,
            failure,
            &golden,
            RunDigests::for_test(&weights),
            None,
            None,
        );
        let sealed = payload.metrics.error;

        for secret in [
            "/Users/operator/pool-goldens",
            "wJalrXUtnFEMIK7MDENGbPxRfiCY",
            "api.example.internal",
        ] {
            assert!(
                !sealed.contains(secret),
                "secret-tier content sealed into metrics.error: {secret:?}"
            );
        }
        assert!(
            sealed.len() <= bench_runner::SEALED_REASON_BYTE_LIMIT,
            "sealed metrics.error not capped: {} bytes",
            sealed.len()
        );
        assert!(
            sealed.starts_with("engine hello handshake failed"),
            "signature lost: {sealed}"
        );
        assert!(
            sealed.contains("sample-001.json"),
            "diagnosis lost: {sealed}"
        );
    }

    /// The local-iterate checked decode window, mode-derived rather than literal so a future
    /// window change moves every fixture with it (the `16` these tests used to hard-code was
    /// the retired Laguna fork's value, and is exactly how the window drift stayed invisible).
    const LI_STEPS: usize = Mode::LocalIterate.decode_steps();
    /// The `expected_tokens` arity the REFERENCE loader demands for that window: seed + one per
    /// decode step ([`Mode::golden_required_steps`]). Every local-iterate fixture below is
    /// built AND loaded at this arity via `TestGolden::steps`.
    const LI_EXPECTED: usize = LI_STEPS + 1;

    /// A no-op cool gate for tests (never reads the GPU). The real macmon gate lives in
    /// `crate::coolgate` and is exercised by its own unit tests.
    fn no_cool_gate(_phase: &str) -> bench_runner::Result<()> {
        Ok(())
    }

    /// The two-anchor `correctness_gates` block the phase-bracketing tests share: two
    /// single-token anchors over the same 8-token context, expecting 100 then 200.
    fn anchor_gates(first: &str, second: &str) -> serde_json::Value {
        json!({
            "anchors": [
                { "name": first, "context_tokens": vec![1i64; 8], "expected_token": 100, "accepted_tokens": [100] },
                { "name": second, "context_tokens": vec![1i64; 8], "expected_token": 200, "accepted_tokens": [200] }
            ]
        })
    }

    /// A minimal valid golden with a benchmark oracle and NO correctness gates
    /// (so the conformance gate passes vacuously against the mock).
    fn benchmark_golden() -> GoldenFixture {
        TestGolden::new()
            .case_name("case-a")
            .steps(LI_EXPECTED)
            .fixture()
    }

    /// R3 helper: a golden whose primary case `p1` (expects `[2; LI_EXPECTED]`) is conformant, plus a
    /// `correctness_gates` block the mock engine CANNOT satisfy (anchor argmax ≠ 999 /
    /// free-run ≠ the golden's tokens). Under Swift-exact default the gate skips it; under
    /// `--strict` the superset evaluates and fails it.
    fn golden_with_bad_gates(gates: serde_json::Value) -> GoldenFixture {
        TestGolden::new().steps(LI_EXPECTED).gates(gates).fixture()
    }

    /// R3 helper: run local-iterate against `golden` with the given `strict` flag on a mock
    /// whose primary case + timed phases are conformant to `p1` (seed=2, window `[2; LI_STEPS]`).
    fn run_r3(golden: &GoldenFixture, strict: bool) -> ScorePayload {
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        iterate_core(
            &mut session,
            &hello,
            golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            strict,
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        )
    }

    #[test]
    fn r3_local_iterate_default_ignores_corrupted_anchor_but_strict_fails() {
        // R3 (Swift-exact by default): Swift `--local-iterate`
        // (QwenRuntimeLocalIterate.swift:106-108 / runLocalIterateCheckedTiming) judges
        // correctness SOLELY from the primary `cases.first` timing stream — it never
        // evaluates `correctness_gates.anchors`. A golden with a CORRUPTED anchor (engine
        // argmax ≠ 999, no accepted/rank tolerance) must therefore PASS by default, exactly
        // like a run with intact anchors, and FAIL only under the `--strict` superset.
        let corrupt = golden_with_bad_gates(json!({
            "anchors": [
                { "name": "bad-anchor", "context_tokens": vec![1i64; 8], "expected_token": 999, "accepted_tokens": [999] }
            ]
        }));

        // Default (NOT strict): the anchor gate is skipped → passes with a real score, and no
        // gate failure is recorded.
        let default_payload = run_r3(&corrupt, false);
        assert!(
            default_payload.passed,
            "Swift-exact default skips the anchor gate; error={}",
            default_payload.metrics.error
        );
        assert!(default_payload.metrics.passed_correctness);
        assert!(default_payload.score.is_some());
        assert_eq!(default_payload.metrics.first_failing_case, None);

        // A golden with NO anchor gate at all (same conformant primary case) produces the
        // SAME default result — proving the corrupt anchor is genuinely not evaluated.
        let no_gate_default = run_r3(&benchmark_golden(), false);
        assert!(no_gate_default.passed);
        assert_eq!(
            default_payload.passed, no_gate_default.passed,
            "default result is identical whether or not a (corrupt) anchor gate is present"
        );

        // --strict: the superset evaluates the anchor gate and FAILS on the corruption.
        let strict_payload = run_r3(&corrupt, true);
        assert!(
            !strict_payload.passed,
            "--strict must evaluate the anchor gate and fail the corruption"
        );
        assert!(!strict_payload.metrics.passed_correctness);
        assert_eq!(
            strict_payload.metrics.first_failing_case.as_deref(),
            Some("bad-anchor")
        );
    }

    #[test]
    fn r3_local_iterate_default_ignores_corrupted_free_run_but_strict_fails() {
        // R3: a CORRUPTED free-run gate (mock free-run returns [4000, 4001…], golden expects
        // [999, 998]) must PASS by default (Swift never evaluates free-run in local-iterate)
        // and FAIL only under `--strict`.
        let corrupt = golden_with_bad_gates(json!({
            "free_run": [
                { "name": "bad-free-run", "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS], "expected_tokens": [999, 998] }
            ]
        }));

        let default_payload = run_r3(&corrupt, false);
        assert!(
            default_payload.passed,
            "Swift-exact default skips the free-run gate; error={}",
            default_payload.metrics.error
        );
        assert!(default_payload.metrics.passed_correctness);
        assert!(default_payload.score.is_some());
        assert_eq!(default_payload.metrics.first_failing_case, None);

        let strict_payload = run_r3(&corrupt, true);
        assert!(
            !strict_payload.passed,
            "--strict must evaluate the free-run gate and fail the corruption"
        );
        assert_eq!(
            strict_payload.metrics.first_failing_case.as_deref(),
            Some("bad-free-run")
        );
    }

    #[test]
    fn r3_local_iterate_default_still_fails_corrupted_primary_case() {
        // R3 must NOT weaken the base-case check: a corrupted primary `cases[0]` (golden
        // expects 999 at index 3, engine returns 2) still FAILS by default, reported
        // Swift-style. Only anchor/free-run/behavior corruptions now pass by default.
        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .corrupt_expected_at(3, 999)
            .fixture();
        let payload = run_r3(&golden, false);
        assert!(!payload.passed, "a corrupted primary case must always fail");
        assert!(!payload.metrics.passed_correctness);
        assert_eq!(
            payload.metrics.first_failing_case.as_deref(),
            Some("local-iterate")
        );
    }

    #[test]
    fn iterate_end_to_end_floors_pass_and_score_matches_core() {
        let golden = benchmark_golden();
        // The timed path now verifies each engine token against the golden oracle
        // (prefill 5, seed 6, decode 7s), so the mock must return exactly those.
        // primary case-a expects [2; LI_EXPECTED]; a conformant engine returns 2 for all teacher-forced
        // steps (correctness) AND for the local-iterate TIMING, which now times cases[0] (seed
        // = expected[0] = 2, decode window = [2; LI_STEPS]) — not the benchmark oracle.
        // Correctness gate runs on the shared session (teacher-forced [2; LI_EXPECTED]); the timed
        // phases each spawn a FRESH conformant engine (oracle seed=2, window [2; LI_STEPS]).
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let weights = DirDigest::empty();
        let baseline_prefill = OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN;
        let baseline_decode = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN;

        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            baseline_prefill,
            baseline_decode,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&weights),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );

        assert!(payload.passed, "mock timing -> huge speedup should pass");
        let score = payload.score.expect("passing run carries a numeric score");
        assert!(score.is_finite() && score > 0.0);
        let m = &payload.metrics;
        // mock timing is tiny -> speedup huge -> both floors clear.
        assert!(m.passed_decode_speedup_floor);
        assert!(m.passed_prefill_speedup_floor);
        assert!(m.passed_correctness);
        assert_eq!(m.runtime, "rust-local-iterate");
        assert_eq!(m.num_layers, NUM_HIDDEN_LAYERS);
        assert_eq!(m.golden_hash, golden.sha256);
        assert_eq!(m.bandwidth_source, BANDWIDTH_SOURCE);
        // Checked-timing counts must match Swift local-iterate for parity:
        // case_count = timingRepeats (1); checked_steps = (decode_steps + 2) * repeats.
        // NOT the conformance case count.
        assert_eq!(m.case_count, 1);
        assert_eq!(m.checked_steps, LI_STEPS as i64 + 2);

        // The score equals exactly what bench-core computes for (mock timing, baseline).
        let expected = score_default_weights(
            m.decode_seconds_per_token,
            m.prefill_seconds_per_token,
            baseline_decode,
            baseline_prefill,
        );
        assert_eq!(score, expected);

        // Sealed JSON: all metrics fields present + typed, and the sha256 sidecar
        // matches the exact bytes.
        let json = payload.to_sealed_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let metrics_obj = value.get("metrics").unwrap().as_object().unwrap();
        assert_eq!(
            metrics_obj.len(),
            56,
            "all ScoreMetrics fields must be present"
        );
        for key in [
            "decode_seconds_per_token",
            "prefill_seconds_per_token",
            "decode_speedup",
            "prefill_speedup",
            "passed_decode_speedup_floor",
            "passed_prefill_speedup_floor",
            "baseline_decode_seconds_per_token",
            "baseline_prefill_seconds_per_token",
            "passed_correctness",
            "golden_hash",
            "runtime",
        ] {
            assert!(metrics_obj.contains_key(key), "missing metrics key {key}");
        }
        // nullable fields serialize as null, not omitted.
        assert!(metrics_obj.get("first_failing_layer").unwrap().is_null());

        let sidecar = crate::score::sha256_hex(json.as_bytes());
        assert_eq!(sidecar.len(), 64);
        // Recomputing over the same bytes is stable.
        assert_eq!(sidecar, crate::score::sha256_hex(json.as_bytes()));
    }

    #[test]
    fn slow_decode_fails_floor_and_band_but_score_still_published() {
        // Inject a slow decode (10x baseline) directly into the score assembly:
        // the decode speedup drops below the 0.95 floor and out of the acceptance
        // band, but local-iterate still publishes a positive estimated score.
        let golden = benchmark_golden();
        let weights = DirDigest::empty();
        let baseline_prefill = OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN;
        let baseline_decode = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN;
        let slow_decode = baseline_decode * 10.0;

        let timing = TimingResult {
            prefill_seconds_per_token: baseline_prefill,
            decode_seconds_per_token: slow_decode,
            decode_steps: LI_STEPS,
            prefill_prompt_tokens: BENCHMARK_PREFILL_PROMPT_TOKENS,
            prefill_elapsed_seconds: baseline_prefill * BENCHMARK_PREFILL_PROMPT_TOKENS as f64,
            decode_elapsed_seconds: slow_decode * 16.0,
            peak_ram_gb: 20.25,
            effective_spec: None,
        };

        let payload = local_iterate_score(
            Mode::LocalIterate,
            &timing,
            baseline_prefill,
            baseline_decode,
            &golden,
            RunDigests::for_test(&weights),
        );

        // decode speedup ~0.1 -> below the 0.95 floor.
        assert!(!payload.metrics.passed_decode_speedup_floor);
        assert!(payload.metrics.passed_prefill_speedup_floor);

        // The decode acceptance band fails (value far above +2% of reference).
        let eval = evaluate_timed_run(
            slow_decode,
            baseline_prefill,
            baseline_decode,
            baseline_prefill,
        );
        assert!(!eval.decode_band.passed);
        assert!(eval.prefill_band.passed);

        // local-iterate still publishes a finite positive score (bands/floors are
        // reported, not gated, on the local path).
        let score = payload.score.expect("finite positive score");
        assert!(score.is_finite() && score > 0.0);
    }

    #[test]
    fn invalid_timing_yields_null_score() {
        let golden = benchmark_golden();
        let weights = DirDigest::empty();
        let timing = TimingResult {
            prefill_seconds_per_token: 0.0, // non-positive -> invalid
            decode_seconds_per_token: 0.1,
            decode_steps: LI_STEPS,
            prefill_prompt_tokens: BENCHMARK_PREFILL_PROMPT_TOKENS,
            prefill_elapsed_seconds: 0.0,
            decode_elapsed_seconds: 1.6,
            peak_ram_gb: 0.0,
            effective_spec: None,
        };
        let payload = local_iterate_score(
            Mode::LocalIterate,
            &timing,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            &golden,
            RunDigests::for_test(&weights),
        );
        assert!(!payload.passed);
        assert!(payload.score.is_none());
        assert!(payload
            .metrics
            .error
            .contains("local estimated score is invalid"));
    }

    #[test]
    fn checked_steps_and_case_count_match_swift_checked_timing() {
        // (decode_steps + 2) * repeats, repeats = 1 for the LOCAL checked-timing modes:
        // decode_steps + 2 per mode (130 for local-iterate, 1025 for local-submit). case_count =
        // repeats = 1. (Official does NOT use this formula — it reports correctness case
        // counts via official::official_core, so it is not exercised here.)
        let golden = benchmark_golden();
        let weights = DirDigest::empty();
        let baseline_prefill = OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN;
        let baseline_decode = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN;
        for (mode, expected_checked) in [
            (
                Mode::LocalIterate,
                Mode::LocalIterate.decode_steps() as i64 + 2,
            ),
            (
                Mode::LocalSubmit,
                Mode::LocalSubmit.decode_steps() as i64 + 2,
            ),
        ] {
            let timing = TimingResult {
                prefill_seconds_per_token: baseline_prefill,
                decode_seconds_per_token: baseline_decode,
                decode_steps: mode.decode_steps(),
                prefill_prompt_tokens: BENCHMARK_PREFILL_PROMPT_TOKENS,
                prefill_elapsed_seconds: baseline_prefill * BENCHMARK_PREFILL_PROMPT_TOKENS as f64,
                decode_elapsed_seconds: baseline_decode * mode.decode_steps() as f64,
                peak_ram_gb: 20.25,
                effective_spec: None,
            };
            let payload = local_iterate_score(
                mode,
                &timing,
                baseline_prefill,
                baseline_decode,
                &golden,
                RunDigests::for_test(&weights),
            );
            assert_eq!(payload.metrics.case_count, 1, "case_count for {mode:?}");
            assert_eq!(
                payload.metrics.checked_steps, expected_checked,
                "checked_steps for {mode:?}"
            );
        }
    }

    #[test]
    fn correctness_gate_failure_produces_failed_payload() {
        // A golden with a free-run gate whose expected tokens the mock cannot match
        // (mock free-run returns [4000, 4001, ...]).
        let golden = TestGolden::new()
            .without_model_type()
            .case_name("case-a")
            .steps(LI_EXPECTED)
            .gates(json!({
                "free_run": [
                    { "name": "fr-1", "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS], "expected_tokens": [999, 998] }
                ]
            }))
            .fixture_any_model_type();
        // Primary case-a expects [2; LI_EXPECTED]; make the engine conformant on it (B3) so the
        // FREE-RUN case fr-1 remains the first failure this test asserts.
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        // Correctness fails first, so spawn_timed is never invoked (a run that fails before
        // timing spawns no timed engines).
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            true, // R3: --strict exercises the free-run superset gate
            RunDigests::for_test(&DirDigest::empty()),
            || Session::connect(MockEngine::new()).map(|(s, _)| s),
            no_cool_gate,
        );
        assert!(!payload.passed);
        assert!(payload.score.is_none());
        assert!(!payload.metrics.passed_correctness);
        assert_eq!(payload.metrics.first_failing_case.as_deref(), Some("fr-1"));
    }

    #[test]
    fn b1_anchor_gate_steps_do_not_leak_into_the_prefill_barrier() {
        // TWO anchors (each a timed correctness_begin) + the benchmark oracle, no primary
        // cases. B1: iterate_core must close the correctness phase (phase_diagnostics) so
        // the engine's completed_work (=2) is reconciled and reset BEFORE prefill. Without
        // the close_phase those 2 steps leak into prefill's barrier -> CompletedWorkMismatch
        // -> the run FAILS against a fully conformant engine. This asserts it passes.
        // At least one primary case is required (loader parity); it also runs teacher-
        // forced, so the correctness phase issues LI_EXPECTED (base) + 2 (anchor) timed steps.
        let golden = TestGolden::new()
            .case_name("c1")
            .steps(LI_EXPECTED)
            .expected_fill(3)
            .gates(anchor_gates("anc-1", "anc-2"))
            .fixture();
        // Conformant engine: base case returns 3 for all LI_EXPECTED teacher-forced steps, then the
        // anchor argmax = 100 then 200 (teacher_forced oracle sets the top-1); timing oracle
        // matches the benchmark block. Per-sequence oracle (#55): one token list per
        // sequence (base case, anc-1, anc-2) — no hand-concatenation of a flat stream.
        let tf = vec![vec![3i64; LI_EXPECTED], vec![100], vec![200]];
        // Correctness on the shared session; local-iterate times cases[0] (primary "p1",
        // expected [3; LI_EXPECTED]) on FRESH engines per phase: seed = 3, window [3; LI_STEPS].
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_sequences(tf)).unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            true, // R3: --strict so the anchor gate actually runs
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(3, 3, vec![3i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(
            payload.passed,
            "anchor gate steps must be reconciled by close_phase before prefill (B1); error={}",
            payload.metrics.error
        );
        assert!(payload.metrics.passed_correctness);
        // The score's case_count stays the checked-timing repeats (parity), not anchors.
        assert_eq!(payload.metrics.case_count, 1);
    }

    #[test]
    fn v1_drain_allocator_round_trips_phase_diagnostics_to_the_worker() {
        // V1: SessionEngine::drain_allocator must REACH a real worker, not be a no-op. With
        // a 3-sequence golden (one primary case + two anchors) the per-sequence barrier
        // closes a sub-phase at each boundary via phase_diagnostics — base->anchor1,
        // anchor1->anchor2, and the final close before prefill = 3 correctness round-trips
        // (the first sequence only opens) — plus the prefill + decode closes = 5 total. A
        // no-op drain would send ZERO phase_diagnostics during the correctness gate.
        let golden = TestGolden::new()
            .case_name("c1")
            .steps(LI_EXPECTED)
            .expected_fill(3)
            .gates(anchor_gates("anc-1", "anc-2"))
            .fixture();
        let seen = std::rc::Rc::new(std::cell::Cell::new(0usize));
        // Per-sequence oracle (#55): one token list per sequence (base case, anc-1, anc-2).
        let tf = vec![vec![3i64; LI_EXPECTED], vec![100], vec![200]];
        // Correctness on the shared session; local-iterate times cases[0] (primary "c1",
        // expected [3; LI_EXPECTED]) on FRESH engines per phase: seed = 3, window [3; LI_STEPS]. The SAME
        // phase-diagnostics counter is shared across the correctness session and both fresh
        // timed engines, so it observes every drain regardless of which process handled it.
        let (mut session, hello) = Session::connect(
            MockEngine::new()
                .teacher_forced_sequences(tf)
                .count_phase_diagnostics(seen.clone()),
        )
        .unwrap();
        let seen_timed = seen.clone();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            true, // R3: --strict so the anchor sequences drain
            RunDigests::for_test(&DirDigest::empty()),
            move || {
                Session::connect(
                    MockEngine::new()
                        .oracle_tokens(3, 3, vec![3i64; LI_STEPS])
                        .count_phase_diagnostics(seen_timed.clone()),
                )
                .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(
            payload.passed,
            "conformant multi-sequence run must pass; error={}",
            payload.metrics.error
        );
        assert_eq!(
            seen.get(),
            5,
            "3 correctness-sequence drains (shared session) + 1 prefill close + 1 decode \
             close, the timing closes now on FRESH per-phase engines, all reach a worker (V1)"
        );
    }

    #[test]
    fn fail_path_closes_final_subphase_and_reports_swift_failure_counts() {
        // 1 primary case + 2 anchors; the engine returns a WRONG token at primary step 0 so
        // correctness FAILS. 2.2: the final correctness sub-phase is still closed on the fail
        // exit -> 3 correctness phase_diagnostics (2 inter-sequence drains + 1 final close) on
        // THIS session; the retain-timing phase runs on FRESH separate sessions (item 1), so it
        // does not touch `seen`. Item-1 red-team Finding 3: the DET counts follow Swift's
        // LOCAL-ITERATE FAILURE report — caseCount = timingRepeats (1), checkedSteps =
        // failureStep + 1 — NOT the golden's total correctness cases.
        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .expected_fill(7)
            .gates(anchor_gates("a1", "a2"))
            .fixture();
        let seen = std::rc::Rc::new(std::cell::Cell::new(0usize));
        // Per-sequence oracle (#55): primary step 0 = 999 (WRONG; expected 7), rest 7; the
        // anchor sequences (100, 200) are conformant — one token list per sequence.
        let mut primary = vec![7i64; LI_EXPECTED];
        primary[0] = 999;
        let tf = vec![primary, vec![100], vec![200]];
        let (mut session, hello) = Session::connect(
            MockEngine::new()
                .teacher_forced_sequences(tf)
                .count_phase_diagnostics(seen.clone()),
        )
        .unwrap();
        // Correctness fails at primary step 0; item 1 then runs the TIME-ONLY timing phase on
        // FRESH engines (oracle set to cases[0]'s stream), so timing is retained. Those fresh
        // sessions have no `seen` counter, so only the correctness session's sub-phase closes
        // count toward `seen`.
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            true, // R3: --strict so the anchor sub-phases run
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(7, 7, vec![7i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(!payload.passed);
        assert!(!payload.metrics.passed_correctness);
        // §F1: a local-iterate base-case failure is reported Swift-style. primary "p1" is
        // corrupted at index 0 (engine returns 999, golden expects 7) → case "local-iterate",
        // checked-step 0 (prefill), expected/actual populated, descriptive error.
        assert_eq!(
            payload.metrics.first_failing_case.as_deref(),
            Some("local-iterate")
        );
        assert_eq!(payload.metrics.first_failing_step, Some(0));
        assert_eq!(payload.metrics.expected_token, Some(7));
        assert_eq!(payload.metrics.actual_token, Some(999));
        assert_eq!(
            payload.metrics.error,
            "local-iterate teacher-forced token mismatch"
        );
        // Swift local-iterate FAILURE counts: caseCount = timingRepeats (1); checkedSteps =
        // failureStep + 1 = 0 + 1 = 1. (Before item-1 Finding 3, this was the golden total, 3.)
        assert_eq!(payload.metrics.case_count, TIMING_REPEATS);
        assert_eq!(
            payload.metrics.checked_steps, 1,
            "checkedSteps = first_failing_step(0) + 1"
        );
        // timing is RETAINED (baselines are the real constants, not blanked 0.0).
        assert!(payload.metrics.baseline_decode_seconds_per_token > 0.0);
        // 2.2: the final sub-phase was closed on the fail exit (2 inter-drains + 1 close).
        assert_eq!(
            seen.get(),
            3,
            "the final correctness sub-phase must be closed even on the fail path"
        );
    }

    #[test]
    fn local_iterate_correctness_failure_retains_real_timing() {
        // ITEM 1 (David's ruling — "Swift is the reference"): a local-iterate CORRECTNESS
        // FAILURE must RETAIN real timing/baselines/speedup-floor flags (Swift teacher-forces
        // the expected tokens and times anyway; correctness is judged separately) instead of
        // blanking them to 0/false. This mirrors the `primary` divergence: cases[0] carries a
        // flipped token so correctness fails, but the TIME-ONLY timing phase still runs on
        // FRESH engines and records real timing.
        //
        // The golden's cases[0] "p1" expects [2; LI_EXPECTED]; we corrupt index 3 (golden 999, engine
        // 2) so the teacher-forced correctness gate fails at a checked step, exactly as the
        // §F1 test does. The timed engines are conformant to cases[0]'s stream (seed=2, window
        // [2; 16]) — but under TIME-ONLY they'd be timed even if they diverged.
        // golden expects 999 at index 3; the conformant engine returns 2.
        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .corrupt_expected_at(3, 999)
            .fixture();
        let baseline_prefill = OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN;
        let baseline_decode = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN;
        // Correctness on the shared session (engine returns 2 everywhere → mismatch at index
        // 3). The timed phases spawn FRESH engines whose oracle matches cases[0] (seed=2,
        // window [2; LI_STEPS]); the time-only path would tolerate a divergence, but here they match.
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let spawned = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let spawned_c = spawned.clone();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            baseline_prefill,
            baseline_decode,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            move || {
                spawned_c.set(spawned_c.get() + 1);
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );

        // Correctness FAILED, so no score and the F1 fields are populated (unchanged behavior).
        assert!(!payload.passed);
        assert!(payload.score.is_none());
        let m = &payload.metrics;
        assert!(!m.passed_correctness);
        assert_eq!(m.first_failing_case.as_deref(), Some("local-iterate"));
        assert_eq!(
            m.first_failing_step,
            Some(4),
            "index 3 → Swift checked-step 4"
        );
        assert_eq!(m.expected_token, Some(999));
        assert_eq!(m.actual_token, Some(2));
        // Item-1 red-team Finding 3: the DET correctness counts follow Swift's LOCAL-ITERATE
        // FAILURE report — caseCount = timingRepeats, checkedSteps = failureStep + 1 — NOT the
        // standalone golden-total the blanked path used (which diverged from Swift).
        assert_eq!(
            m.case_count, TIMING_REPEATS,
            "Swift caseCount = timingRepeats on a local-iterate failure"
        );
        assert_eq!(
            m.checked_steps, 5,
            "Swift checkedSteps = first_failing_step(4) + 1"
        );

        // ITEM 1: the timing surface is REAL, not blanked. The time-only phase spawned two
        // fresh engines (prefill + decode) — a blanked fail path spawns none.
        assert_eq!(
            spawned.get(),
            2,
            "time-only timing spawns one fresh engine per phase"
        );
        // Baselines are the real external Qwen constants (blanked path emits 0.0).
        assert!(m.baseline_decode_seconds_per_token > 0.0);
        assert!(m.baseline_prefill_seconds_per_token > 0.0);
        assert_eq!(m.baseline_decode_seconds_per_token, baseline_decode);
        assert_eq!(m.baseline_prefill_seconds_per_token, baseline_prefill);
        // Measured spt are finite (mock timing is tiny). speedup(baseline, 0.0) == 0, so a
        // NON-ZERO speedup + a TRUE floor together prove the spt is real and > 0 (not blanked).
        assert!(m.decode_seconds_per_token.is_finite());
        assert!(m.prefill_seconds_per_token.is_finite());
        assert!(m.decode_speedup > 0.0, "real decode speedup, not blanked 0");
        assert!(
            m.prefill_speedup > 0.0,
            "real prefill speedup, not blanked 0"
        );
        assert!(
            m.passed_decode_speedup_floor,
            "floor reflects the real (huge) mock speedup — proves decode_spt > 0"
        );
        assert!(
            m.passed_prefill_speedup_floor,
            "floor reflects the real (huge) mock speedup — proves prefill_spt > 0"
        );
    }

    #[test]
    fn local_iterate_primary_failure_reports_swift_checked_step() {
        // §F1 regression (deterministic; replaces the integration window). A primary
        // teacher-forced mismatch at expected_tokens[3] is reported EXACTLY as Swift
        // `--local-iterate` — case="local-iterate", checked-step 4 (prefill 0 / seed 1 /
        // decode k → k+2; index 3 = decode k=2 → 4), expected=<golden>, actual=<engine>,
        // error="local-iterate teacher-forced token mismatch". Target values captured from
        // the live failure-map window (Swift step 4, expected/actual populated).
        // golden expects 999 at index 3; the conformant engine returns 2.
        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .corrupt_expected_at(3, 999)
            .fixture();
        // Engine returns 2 for every teacher-forced step → matches golden at 0/1/2, mismatches
        // at index 3 (golden 999 vs engine 2). Correctness fails before timing, so the timed
        // spawner is never invoked.
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || Session::connect(MockEngine::new()).map(|(s, _)| s),
            no_cool_gate,
        );
        assert!(!payload.passed);
        let m = &payload.metrics;
        assert_eq!(m.first_failing_case.as_deref(), Some("local-iterate"));
        assert_eq!(
            m.first_failing_step,
            Some(4),
            "index 3 → Swift checked-step 4"
        );
        assert_eq!(m.expected_token, Some(999), "golden's expected token");
        assert_eq!(m.actual_token, Some(2), "engine's actual token");
        assert_eq!(m.error, "local-iterate teacher-forced token mismatch");
    }

    #[test]
    fn base_case_mismatches_are_always_inside_the_local_checked_window() {
        // §F1 window bound, restated at the REFERENCE window. The arm guard is
        // `f.step <= mode.decode_steps()`: a base-case mismatch BEYOND the checked window is the
        // benchctl SUPERSET and must not be branded "local-iterate" with an impossible step.
        //
        // At the reference window that guard can no longer fire for a BASE case: the conformance
        // gate only ever reports indices `< CORRECTNESS_STEPS` (64), and both local modes decode
        // at least that far (local-iterate 128, local-submit 1023). So every base-case index the
        // gate can produce is inside the window and takes the F1 branding arm. The guard stays as
        // a fail-closed bound on any future shorter window; this test pins WHY it is quiet.
        //
        // (It used to fire because benchd held the retired Laguna fork's 16-step window, where
        // index 40 was out of range. The superset arm is still exercised end-to-end by the
        // anchor/free-run `--strict` tests, which are `is_base_case = false`.)
        for mode in [Mode::LocalIterate, Mode::LocalSubmit] {
            assert!(
                mode.decode_steps() >= bench_core::constants::CORRECTNESS_STEPS,
                "{mode:?} decode window {} must cover every index the correctness gate reports \
                 (< {})",
                mode.decode_steps(),
                bench_core::constants::CORRECTNESS_STEPS
            );
        }
        // The mapping itself is unchanged and still exact at the window edge.
        assert_eq!(swift_local_iterate_checked_step(0), 0, "prefill");
        assert_eq!(swift_local_iterate_checked_step(1), 2, "decode step 0");
        assert_eq!(
            swift_local_iterate_checked_step(LI_STEPS),
            LI_STEPS as i64 + 1,
            "last in-window decode step"
        );
    }

    #[test]
    fn local_iterate_golden_arity_matches_the_reference_loader() {
        // #109 window-4 E2 ROOT CAUSE. Swift `QwenRuntime.localIterate` loads the golden with
        // `requiredSteps: benchmarkDecodeSteps + 1` — `expected_tokens[0]` is the seed and
        // `expected_tokens[k + 1]` is decode step `k`'s output — so a golden carrying exactly
        // `decode_steps` tokens is one SHORT and the reference refuses it at LOAD time:
        //
        //   "primary-1.expected_tokens has 128 tokens; need at least 129"
        //
        // benchd loaded every mode at the flat `CORRECTNESS_STEPS` (64) and timed local-iterate
        // over a 16-step window, so it ACCEPTED that golden and reported a token mismatch from a
        // stream the reference never validated at all. Pin both halves of the contract.
        assert_eq!(
            Mode::LocalIterate.golden_required_steps(),
            Mode::LocalIterate.decode_steps() + 1
        );
        assert_eq!(
            Mode::LocalSubmit.golden_required_steps(),
            Mode::LocalSubmit.decode_steps() + 1
        );
        assert_eq!(
            Mode::Official.golden_required_steps(),
            bench_core::constants::CORRECTNESS_STEPS,
            "official takes the reference loader DEFAULT"
        );

        // The window-4 golden's shape: a well-formed document whose primary case supplies
        // exactly `decode_steps` expected tokens. Built through the shared builder so the rest
        // of the document is the canonical shape and only the arity under test differs.
        let one_short = Mode::LocalIterate.decode_steps();
        let bytes = TestGolden::new()
            .case_name("primary-1")
            .expected_tokens(vec![2i64; one_short])
            .bytes();

        // It still loads for the CORRECTNESS consumer (that one needs only 64) ...
        assert!(load_golden_fixture(
            &bytes,
            bench_core::constants::CORRECTNESS_STEPS,
            CORRECTNESS_PROMPT_TOKENS,
            Some(REQUIRED_GOLDEN_MODEL_TYPE),
            None,
            None,
        )
        .is_ok());

        // ... and is refused, verbatim as the reference refuses it, for local-iterate.
        let err = load_golden_fixture(
            &bytes,
            Mode::LocalIterate.golden_required_steps(),
            CORRECTNESS_PROMPT_TOKENS,
            Some(REQUIRED_GOLDEN_MODEL_TYPE),
            None,
            None,
        )
        .expect_err("a decode_steps-long expected_tokens is one token short of the window");
        assert!(
            err.to_string().contains(&format!(
                "primary-1.expected_tokens has {one_short} tokens; need at least {}",
                Mode::LocalIterate.golden_required_steps()
            )),
            "reference message shape, got: {err}"
        );
    }

    #[test]
    fn local_iterate_times_cases0_stream_not_the_benchmark_oracle() {
        // RULING 1: local-iterate must TIME cases[0]'s stream, exactly as Swift
        // --local-iterate (prefill cases[0].prompt_tokens; decode SEEDS with the same prompt;
        // teacher-forces expected_tokens[1..]). cases[0].prompt_tokens (11s) differ from the
        // benchmark oracle prompt (22s), so the recorded timed workload must be cases[0]'s.
        let expected: Vec<i64> = (0..LI_EXPECTED as i64).map(|i| 3000 + i).collect();
        let golden = TestGolden::new()
            .required_steps(LI_EXPECTED)
            .prompt_fill(11)
            .expected_tokens(expected.clone())
            .benchmark(benchmark_oracle(22, 9))
            .fixture();
        let rec = std::rc::Rc::new(std::cell::RefCell::new(
            bench_runner::mock::RecordedTiming::default(),
        ));
        // Correctness on the shared session (teacher-forced returns expected[..]); the timed
        // prefill/decode phases run on FRESH engines that return cases[0]'s seed + window and
        // record the timed workload they receive.
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(expected.clone())).unwrap();
        let rec_timed = rec.clone();
        let seed = expected[0];
        let window = expected[1..=LI_STEPS].to_vec();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            move || {
                Session::connect(
                    MockEngine::new()
                        .oracle_tokens(seed, seed, window.clone())
                        .record_timing(rec_timed.clone()),
                )
                .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(
            payload.passed,
            "conformant cases[0] run must pass; error={}",
            payload.metrics.error
        );
        let r = rec.borrow();
        // The TIMED workload is cases[0]'s, NOT the benchmark oracle's.
        assert_eq!(r.prefill_prompt, golden.cases[0].prompt_tokens);
        assert_eq!(r.decode_seed, golden.cases[0].prompt_tokens); // Swift seeds decode with the prompt
        assert_ne!(
            r.prefill_prompt,
            golden.benchmark.as_ref().unwrap().prefill_prompt_tokens,
            "must NOT time the benchmark oracle prompt"
        );
        // Teacher-forced decode inputs = expected_tokens[0..decode_steps] (seed, then expected[1..]).
        assert_eq!(r.decode_step_inputs, expected[0..LI_STEPS].to_vec());
    }

    #[test]
    fn local_iterate_spawns_a_fresh_engine_per_timed_phase() {
        // §A1/A2 — LIFECYCLE PARITY. Swift `--local-iterate`
        // (runLocalIterateCheckedTimingWithWorker) spawns a FRESH RuntimeWorkerClient for
        // the prefill phase (QwenRuntimeLocalIterate.swift:704) and a SEPARATE one for the
        // decode phase (:750): two distinct timed-phase processes, neither of them the warm
        // session that ran correctness. This asserts benchd matches that lifecycle exactly:
        // (1) exactly 2 spawn_timed calls (Swift's timed-worker count), (2) each a distinct
        // process identity (hello nonce), (3) both distinct from the correctness session.
        let golden = benchmark_golden(); // cases[0] expects [2; LI_EXPECTED]; timing seed=2, window [2; LI_STEPS]
        let weights = DirDigest::empty();
        let baseline_prefill = OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN;
        let baseline_decode = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN;

        // The correctness gate keeps the shared session — its own, stable identity.
        let (mut session, hello) = Session::connect(
            MockEngine::new()
                .with_session_nonce("correctness-session")
                .teacher_forced_tokens(vec![2i64; LI_EXPECTED]),
        )
        .unwrap();
        assert_eq!(session.nonce(), "correctness-session");

        // Each timed phase spawns a FRESH engine with a distinct nonce; record the
        // identities in spawn order.
        let spawned: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let spawned_c = spawned.clone();
        let spawn_timed = move || -> bench_runner::Result<Session<MockEngine>> {
            let n = spawned_c.borrow().len();
            let nonce = format!("timed-engine-{n}");
            spawned_c.borrow_mut().push(nonce.clone());
            Session::connect(MockEngine::new().with_session_nonce(&nonce).oracle_tokens(
                2,
                2,
                vec![2i64; LI_STEPS],
            ))
            .map(|(s, _)| s)
        };

        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            baseline_prefill,
            baseline_decode,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&weights),
            spawn_timed,
            no_cool_gate,
        );

        assert!(
            payload.passed,
            "conformant run must pass; error={}",
            payload.metrics.error
        );
        let ids = spawned.borrow();
        // (1) Swift's timed-worker count: exactly two fresh spawns (prefill + decode).
        assert_eq!(
            ids.len(),
            2,
            "local-iterate must spawn one fresh engine per timed phase (Swift :704 + :750)"
        );
        // (2) Distinct process identities: prefill and decode are separate engines.
        assert_ne!(
            ids[0], ids[1],
            "the prefill and decode phases must be DISTINCT engine processes"
        );
        // (3) Neither timed phase reuses the warm correctness session.
        assert!(
            !ids.contains(&"correctness-session".to_string()),
            "timed phases must NOT reuse the correctness session (the warm-session residual)"
        );
    }

    // NOTE (B-2): the retired `official_mode_times_on_the_shared_session_not_a_fresh_engine`
    // test enshrined the WRONG single-worker assumption. The corrected Swift map shows official
    // spawns THREE fresh sandboxed workers (prefill, decode, correctness) in a timed-FIRST
    // order, so `iterate_core` no longer handles Official at all — it routes to
    // `crate::official::official_core`, which owns the official lifecycle and its own tests.

    #[test]
    fn iso8601_known_epoch() {
        // 1970-01-01T00:00:00Z
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
        // 2026-08-15T00:00:00Z == 1_786_752_000 (verified against a civil calendar)
        assert_eq!(format_iso8601(1_786_752_000), "2026-08-15T00:00:00Z");
        // 2026-08-15T12:34:56Z
        assert_eq!(
            format_iso8601(1_786_752_000 + 45_296),
            "2026-08-15T12:34:56Z"
        );
    }

    #[test]
    fn dir_digest_counts_and_ignores() {
        let tmp = std::env::temp_dir().join(format!("benchctl-digest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.bin"), b"hello").unwrap();
        fs::write(tmp.join(".gitkeep"), b"").unwrap();
        let d = dir_digest(&tmp).unwrap();
        assert_eq!(d.file_count, 1); // .gitkeep ignored
        assert_eq!(d.byte_count, 5);
        assert_eq!(d.sha256.len(), 64);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn dir_digest_matches_swift_tree_formula() {
        // P2: the tree hash is Swift `directoryDigest`'s exactly — over files sorted by
        // relative path (ignoring exact `.gitkeep` / `.benchmark-source.sha256`), fold
        // `relpath || 0x00 || sha256(file bytes) || 0x00`, then hex(sha256(stream)).
        let tmp = std::env::temp_dir().join(format!("benchctl-digest-fmt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("a.bin"), b"hello").unwrap();
        fs::write(tmp.join("sub/b.bin"), b"world").unwrap();
        fs::write(tmp.join(".gitkeep"), b"ignored").unwrap();
        fs::write(tmp.join(".benchmark-source.sha256"), b"ignored").unwrap();

        let d = dir_digest(&tmp).unwrap();
        assert_eq!(
            d.file_count, 2,
            ".gitkeep + .benchmark-source.sha256 ignored by exact rel"
        );
        assert_eq!(d.byte_count, 10);

        // Independently hand-compute the Swift tree hash.
        let sha = |b: &[u8]| {
            let mut h = Sha256::new();
            h.update(b);
            h.finalize()
        };
        let mut tree = Sha256::new();
        // Byte-lexicographic sort: "a.bin" < "sub/b.bin".
        for (rel, content) in [("a.bin", &b"hello"[..]), ("sub/b.bin", &b"world"[..])] {
            tree.update(rel.as_bytes());
            tree.update([0u8]);
            tree.update(sha(content));
            tree.update([0u8]);
        }
        let expected: String = tree.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            d.sha256, expected,
            "tree hash must match the Swift directoryDigest formula"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn correctness_failure_payload_seals_as_writable_failing_json() {
        // M-3: the OTHER failing class (a correctness mismatch, not preflight). iterate_core
        // returns a failed payload (score=None, passed=false) that must serialize to the sealed
        // score.json the common writers seal — proving the failing payload reaching
        // write_score/write_integrity_sidecar is well-formed. The writers do NOT branch on
        // failure class, so this + the preflight case in main.rs cover both.
        // corrupt cases[0] so the base-case gate fails
        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .corrupt_expected_at(3, 999)
            .fixture();
        let payload = run_r3(&golden, false);
        assert!(
            !payload.passed,
            "a corrupted primary case fails correctness"
        );
        assert!(payload.score.is_none(), "a failing run carries score=null");
        assert!(!payload.metrics.passed_correctness);

        // The failing payload seals to the byte-shaped JSON the writers persist as score.json.
        let json = payload
            .to_sealed_json()
            .expect("failing payload must serialize");
        assert!(json.contains("\"passed\": false"));
        assert!(json.contains("\"score\": null"));
        // The golden_hash field is still populated on failure (a real audit surface).
        assert_eq!(payload.metrics.golden_hash, golden.sha256);
    }

    /// #62: the §F2 missing-baseline refusal, asserted IN FULL against a fixture captured
    /// from the Swift source — for every mode, since Swift interpolates the mode name into
    /// the message. The old assertion was `starts_with("local-iterate requires external
    /// Qwen")`, which left the whole tail ("...both prefill and decode; Gemma baseline
    /// constants are not valid for Qwen") free to drift unnoticed, and compared benchd's
    /// output against a Rust literal restating it rather than against Swift.
    #[test]
    fn missing_paired_baselines_error_matches_swift_capture() {
        const CAPTURE: &str = include_str!("../tests/fixtures/swift-missing-paired-baselines.json");
        let capture: serde_json::Value = serde_json::from_str(CAPTURE).unwrap();
        let expected = &capture["messages"];

        for mode in [Mode::LocalIterate, Mode::LocalSubmit, Mode::Official] {
            let want = expected[mode.mode_name()]
                .as_str()
                .unwrap_or_else(|| panic!("Swift capture has no message for {}", mode.mode_name()));
            assert_eq!(
                missing_paired_baselines_error(mode),
                want,
                "benchd's missing-baseline refusal for {} diverged from the Swift capture \
                 ({}:{} @ {})",
                mode.mode_name(),
                capture["source_file"].as_str().unwrap(),
                capture["source_lines"].as_str().unwrap(),
                capture["source_commit"].as_str().unwrap(),
            );
        }

        // The capture must cover every mode that can reach the refusal — a mode added later
        // must not silently go unpinned.
        assert_eq!(
            expected.as_object().unwrap().len(),
            3,
            "Swift capture covers a different set of modes than benchd has"
        );
    }

    // --- M-6: local-submit -------------------------------------------------

    #[test]
    fn local_submit_mode_properties() {
        // The Mode plumbing: 1023-step decode window, `rust-local-submit` runtime label,
        // `local-submit` modeName, and the P6 cool-gate RULING (submit ON by default).
        assert_eq!(Mode::LocalSubmit.decode_steps(), 1023);
        assert_eq!(Mode::LocalSubmit.runtime(), "rust-local-submit");
        assert_eq!(Mode::LocalSubmit.mode_name(), "local-submit");
        assert_eq!(Mode::parse("local-submit"), Some(Mode::LocalSubmit));
        assert!(
            Mode::LocalSubmit.cool_gate_on_by_default(),
            "P6 RULING: submit ON by default"
        );
        // Contrast: iterate OFF, official OFF; both local modes brand failures Swift-style.
        assert!(!Mode::LocalIterate.cool_gate_on_by_default());
        assert!(!Mode::Official.cool_gate_on_by_default());
        assert!(Mode::LocalSubmit.is_local_checked_timing());
        assert!(Mode::LocalIterate.is_local_checked_timing());
        assert!(!Mode::Official.is_local_checked_timing());
    }

    /// A golden whose primary `cases[0]` carries enough expected_tokens to time a full
    /// 1023-step local-submit decode window (needs len > 1023). All tokens are `2` so the
    /// conformant mock (teacher-forced `2`, oracle decode `[2; 1023]`) passes.
    fn submit_golden(expected: Vec<i64>) -> GoldenFixture {
        // `required_steps` (not `steps`) — the caller supplies its own long window, but the
        // fixture is still validated against the local checked-decode arity.
        TestGolden::new()
            .required_steps(LI_EXPECTED)
            .expected_tokens(expected)
            .fixture()
    }

    #[test]
    fn local_submit_scores_cases0_with_1023_steps() {
        // M-6: local-submit reuses the local-iterate checked-timing machinery over a 1023-step
        // decode of cases[0] (Swift `QwenRuntime.localIterate` with decodeSteps=1023). A
        // conformant run scores, labels `runtime=rust-local-submit`, and reports the Swift
        // checked-timing counts: case_count = timingRepeats = 1, checked_steps = (1023+2)*1.
        let golden = submit_golden(vec![2i64; 1025]); // > 1023 → full window available
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; 1025])).unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalSubmit,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; 1023]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(
            payload.passed,
            "conformant local-submit run scores; error={}",
            payload.metrics.error
        );
        assert!(payload.score.is_some());
        assert_eq!(payload.metrics.runtime, "rust-local-submit");
        assert_eq!(payload.metrics.case_count, 1, "timingRepeats");
        assert_eq!(
            payload.metrics.checked_steps,
            (1023 + 2) * TIMING_REPEATS,
            "(decodeSteps + 2) * timingRepeats"
        );
    }

    #[test]
    fn local_submit_primary_failure_is_branded_local_submit() {
        // §F1 mode-derived (M-6): a corrupted primary cases[0] under local-submit is branded
        // with the modeName — case="local-submit", error="local-submit teacher-forced token
        // mismatch", Swift checked-step numbering — NOT hard-coded "local-iterate". A short
        // (64-token) case can't time a 1023-step window, so it takes the blanked failed
        // payload, but the mode-derived branding survives on that path too.
        let mut expected = vec![2i64; LI_EXPECTED];
        expected[3] = 999; // golden expects 999 at index 3; the conformant engine returns 2.
        let golden = submit_golden(expected);
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalSubmit,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || Session::connect(MockEngine::new()).map(|(s, _)| s),
            no_cool_gate,
        );
        assert!(!payload.passed);
        let m = &payload.metrics;
        assert_eq!(m.first_failing_case.as_deref(), Some("local-submit"));
        assert_eq!(
            m.first_failing_step,
            Some(4),
            "index 3 → Swift checked-step 4"
        );
        assert_eq!(m.expected_token, Some(999));
        assert_eq!(m.actual_token, Some(2));
        assert_eq!(m.error, "local-submit teacher-forced token mismatch");
    }

    #[test]
    fn local_submit_correctness_failure_retains_real_timing() {
        // M-6 coverage gap (red-team): local-submit INHERITS the local-iterate
        // correctness-failure retain-timing path (`mode.is_local_checked_timing()` is true for
        // both), but that path was only exercised under local-iterate. This is the submit twin
        // of `local_iterate_correctness_failure_retains_real_timing`: a CORRECTNESS FAILURE on a
        // golden with a FULL 1023-step window must RETAIN real timing/baselines/speedup-floor
        // flags (Swift teacher-forces + times regardless of correctness) — NOT blank them.
        //
        // cases[0] carries > 1023 expected_tokens (so a full window CAN be timed → retain-timing
        // path, not the too-short blank path) with the corrupt token at index 3, WITHIN the
        // 1023-step checked window (index ≤ 1023) so it brands "local-submit". The timed engines
        // are conformant to cases[0]'s stream (seed=2, window [2; 1023]); under TIME-ONLY they'd
        // time even if they diverged.
        let mut expected = vec![2i64; 1025]; // > 1023 → full window available
        expected[3] = 999; // golden expects 999 at index 3; the conformant engine returns 2.
        let golden = submit_golden(expected);
        let baseline_prefill = OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN;
        let baseline_decode = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN;
        // Correctness on the shared session (engine returns 2 everywhere → mismatch at index 3).
        // The timed phases spawn FRESH engines whose oracle matches cases[0] (seed=2, window
        // [2; 1023]); the time-only path tolerates a divergence, but here they match.
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; 1025])).unwrap();
        let spawned = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let spawned_c = spawned.clone();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            baseline_prefill,
            baseline_decode,
            Mode::LocalSubmit,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            move || {
                spawned_c.set(spawned_c.get() + 1);
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; 1023]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );

        // Correctness FAILED, so no score and the F1 fields are populated, branded "local-submit".
        assert!(!payload.passed);
        assert!(payload.score.is_none());
        let m = &payload.metrics;
        assert!(!m.passed_correctness);
        assert_eq!(m.first_failing_case.as_deref(), Some("local-submit"));
        assert_eq!(
            m.first_failing_step,
            Some(4),
            "index 3 → Swift checked-step 4"
        );
        assert_eq!(m.expected_token, Some(999));
        assert_eq!(m.actual_token, Some(2));
        assert_eq!(m.error, "local-submit teacher-forced token mismatch");
        // DET correctness counts follow Swift's LOCAL failure report: caseCount = timingRepeats,
        // checkedSteps = failureStep + 1 — NOT the blanked golden-total.
        assert_eq!(
            m.case_count, TIMING_REPEATS,
            "Swift caseCount = timingRepeats on a local-submit failure"
        );
        assert_eq!(
            m.checked_steps, 5,
            "Swift checkedSteps = first_failing_step(4) + 1"
        );

        // The timing surface is REAL, not blanked. The time-only phase spawned two fresh engines
        // (prefill + decode) — a blanked fail path spawns none.
        assert_eq!(
            spawned.get(),
            2,
            "time-only timing spawns one fresh engine per phase"
        );
        // Baselines are the real external Qwen constants (blanked path emits 0.0).
        assert!(m.baseline_decode_seconds_per_token > 0.0);
        assert!(m.baseline_prefill_seconds_per_token > 0.0);
        assert_eq!(m.baseline_decode_seconds_per_token, baseline_decode);
        assert_eq!(m.baseline_prefill_seconds_per_token, baseline_prefill);
        // A NON-ZERO speedup + a TRUE floor together prove the spt is real and > 0 (not blanked).
        assert!(m.decode_seconds_per_token.is_finite());
        assert!(m.prefill_seconds_per_token.is_finite());
        assert!(m.decode_speedup > 0.0, "real decode speedup, not blanked 0");
        assert!(
            m.prefill_speedup > 0.0,
            "real prefill speedup, not blanked 0"
        );
        assert!(
            m.passed_decode_speedup_floor,
            "floor reflects the real (huge) mock speedup — proves decode_spt > 0"
        );
        assert!(
            m.passed_prefill_speedup_floor,
            "floor reflects the real (huge) mock speedup — proves prefill_spt > 0"
        );
    }

    // ---- #74 failure-surface: the TWO adjacent failure classes -------------------------
    //
    // RULED (David 2026-08-20, via structured interview) — per-class byte fidelity:
    //
    //   PRIMARY (a run HAPPENED and its correctness failed) → RETAIN the real measurements.
    //     Ruled 2026-08-17 and already implemented (retain-timing #73 + the R3 default flip);
    //     the reference retains because `--local-iterate` TEACHER-FORCES the expected tokens
    //     and times the pass regardless, judging correctness separately
    //     (`QwenRuntimeLocalIterate.swift@b26f76f:164-174` returns through `failed(...)` with
    //     the measured decode/prefill spt in hand).
    //
    //   EARLY REFUSE (nothing ran) → seal the reference's empty record.
    //     `QwenRuntimeLocalIterate.swift@b26f76f:197-198` catches into `failed(...)` with
    //     `correctnessReport` still nil, and `failedScore` reads all three run-shaped fields
    //     off that nil (`QwenRuntimeBenchmark.swift@b26f76f:1161-1162,1176`).
    //
    // The two must not bleed into each other, so they are pinned together, adjacent, below.

    /// The reference's EARLY-REFUSE record, pinned BYTE FOR BYTE against the constructed
    /// capture in `tests/fixtures/swift-early-refuse-failure-record.json`.
    ///
    /// The comparison is over the WHOLE sealed document, not a field sample: the fixture's
    /// `nondeterministic_or_runner_identity` list is the only escape hatch (wall-clock fields
    /// that have no fixed value on either side, plus the `commit`/`harness_hash`/`runtime`
    /// labels graded as environmental / runner-identity fields), the test substitutes exactly those
    /// from the actual payload, and everything else must match to the byte.
    #[test]
    fn early_refuse_record_byte_matches_the_reference_capture() {
        const CAPTURE: &str =
            include_str!("../tests/fixtures/swift-early-refuse-failure-record.json");
        let capture: serde_json::Value = serde_json::from_str(CAPTURE).unwrap();
        let inputs = &capture["test_inputs"];

        let weights = DirDigest {
            sha256: inputs["weights_hash"].as_str().unwrap().to_string(),
            file_count: inputs["weights_file_count"].as_i64().unwrap(),
            byte_count: inputs["weights_byte_count"].as_i64().unwrap(),
        };
        // A golden that loaded fine and carries a full case roster: the point of the pin is
        // that NEITHER its digest NOR its case count reaches the record, because the refusal
        // happened before anything consumed them.
        let golden = TestGolden::new().fixture();
        assert!(
            !golden.sha256.is_empty() && golden.total_correctness_case_count() > 0,
            "precondition: the golden has both a digest and cases to (wrongly) leak"
        );
        let actual = preflight_failed_payload(
            Mode::LocalIterate,
            &golden,
            RunDigests::for_test(&weights),
            inputs["error"].as_str().unwrap().to_string(),
        );
        let actual: serde_json::Value =
            serde_json::from_str(&actual.to_sealed_json().unwrap()).unwrap();

        let mut expected = capture["record"].clone();
        let substituted: Vec<&str> = capture["nondeterministic_or_runner_identity"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for key in &substituted {
            let live = actual["metrics"][key].clone();
            assert!(
                !live.is_null(),
                "{key} is declared substitutable but absent from the sealed payload"
            );
            expected["metrics"][*key] = live;
        }

        // Serialised the same way the writer does (sorted-key pretty), so this is a
        // byte comparison of the document, not a structural one.
        let want = serde_json::to_string_pretty(&expected).unwrap();
        let got = serde_json::to_string_pretty(&actual).unwrap();
        assert_eq!(
            got.as_bytes(),
            want.as_bytes(),
            "the early-refuse record diverged from the reference capture \
             ({} @ {}); substituted fields were {substituted:?}",
            capture["source_files"][0].as_str().unwrap(),
            capture["source_commit"].as_str().unwrap(),
        );

        // Spelled out, because these three ARE the #74 divergence and a future edit to the
        // capture must not quietly relax them.
        assert_eq!(actual["metrics"]["golden_hash"], serde_json::json!(""));
        assert_eq!(actual["metrics"]["case_count"], serde_json::json!(0));
        assert_eq!(actual["metrics"]["checked_steps"], serde_json::json!(0));
    }

    /// The other direction of the same contract, deliberately adjacent: a run that DID happen
    /// and failed correctness keeps its measurements. Guards the #74 fix against regressing
    /// the 2026-08-17 retain-timing ruling — the two classes seal opposite things, and a change
    /// that blanks this one is as wrong as one that populates the other.
    #[test]
    fn correctness_failure_retains_what_early_refuse_seals_empty() {
        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .corrupt_expected_at(3, 999)
            .fixture();
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let ran = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        let refused = preflight_failed_payload(
            Mode::LocalIterate,
            &golden,
            RunDigests::for_test(&DirDigest::empty()),
            missing_paired_baselines_error(Mode::LocalIterate),
        );

        // Both are failures with no score — the verdict is the same; only the SEAL differs.
        assert!(!ran.passed && ran.score.is_none());
        assert!(!refused.passed && refused.score.is_none());

        // RAN: the golden was hashed, the counts describe the checked pass, the timing and
        // baselines are real.
        assert_eq!(ran.metrics.golden_hash, golden.sha256);
        assert_eq!(ran.metrics.case_count, TIMING_REPEATS);
        assert_eq!(ran.metrics.checked_steps, 5);
        assert!(ran.metrics.decode_seconds_per_token > 0.0);
        assert_eq!(
            ran.metrics.baseline_decode_seconds_per_token,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN
        );

        // REFUSED: nothing ran, so nothing about a run is claimed.
        assert_eq!(refused.metrics.golden_hash, "");
        assert_eq!(refused.metrics.case_count, 0);
        assert_eq!(refused.metrics.checked_steps, 0);
        assert_eq!(refused.metrics.decode_seconds_per_token, 0.0);
        // …except the baseline pair, which the reference's `failedScore` defaults to the
        // official constants on BOTH classes (`QwenRuntimeBenchmark.swift@b26f76f:1130-1131`).
        assert_eq!(
            refused.metrics.baseline_decode_seconds_per_token,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN
        );
    }

    // ---- #127: the local baseline pair's SOURCE OF TRUTH ------------------------------

    /// The constants the local legs score against, pinned against a capture of the reference
    /// source rather than against a Rust literal restating them.
    ///
    /// This is the pin that would have caught the split the moment the reference forked: the
    /// capture also records the RETIRED fork's pair, and the test asserts benchd is not still
    /// carrying it.
    #[test]
    fn local_mode_baselines_match_the_reference_constants_capture() {
        const CAPTURE: &str =
            include_str!("../tests/fixtures/swift-official-baseline-constants.json");
        let capture: serde_json::Value = serde_json::from_str(CAPTURE).unwrap();
        let want = &capture["reference"];
        let stale = &capture["retired_fork"];

        let (prefill, decode) = local_mode_baselines();
        assert_eq!(
            prefill,
            want["officialBaselinePrefillSecondsPerToken"]
                .as_f64()
                .unwrap(),
            "local prefill baseline diverged from {}:{} @ {}",
            capture["source_file"].as_str().unwrap(),
            capture["source_lines"].as_str().unwrap(),
            capture["source_commit"].as_str().unwrap(),
        );
        assert_eq!(
            decode,
            want["officialBaselineDecodeSecondsPerToken"]
                .as_f64()
                .unwrap(),
            "local decode baseline diverged from {}:{} @ {}",
            capture["source_file"].as_str().unwrap(),
            capture["source_lines"].as_str().unwrap(),
            capture["source_commit"].as_str().unwrap(),
        );
        // The specific wrong answer this issue is about.
        assert_ne!(
            prefill,
            stale["officialBaselinePrefillSecondsPerToken"]
                .as_f64()
                .unwrap(),
            "benchd is scoring against the RETIRED fork's prefill baseline"
        );
        assert_ne!(
            decode,
            stale["officialBaselineDecodeSecondsPerToken"]
                .as_f64()
                .unwrap(),
            "benchd is scoring against the RETIRED fork's decode baseline"
        );
    }

    /// A golden's DECLARED baseline pair is inert on the local legs: whatever it says, the run
    /// scores against the constants, and the two are not required to agree.
    ///
    /// The fixture declares the retired fork's pair — the exact payload that split the §8
    /// window — and the assertion is that it changes nothing. Under the pre-ruling behavior this
    /// golden WOULD have supplied the denominators, which is what the second half checks: the
    /// old sourcing function still reads them, so this is a live contrast, not a tautology.
    #[test]
    fn a_goldens_declared_baseline_pair_does_not_reach_a_local_run() {
        const CAPTURE: &str =
            include_str!("../tests/fixtures/swift-official-baseline-constants.json");
        let capture: serde_json::Value = serde_json::from_str(CAPTURE).unwrap();
        let stale_prefill = capture["retired_fork"]["officialBaselinePrefillSecondsPerToken"]
            .as_f64()
            .unwrap();
        let stale_decode = capture["retired_fork"]["officialBaselineDecodeSecondsPerToken"]
            .as_f64()
            .unwrap();

        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .baselines(stale_prefill, stale_decode)
            .fixture();
        let declared = golden.benchmark.as_ref().expect("fixture has an oracle");
        assert_eq!(
            declared.baseline_prefill_seconds_per_token,
            Some(stale_prefill),
            "precondition: the golden really does declare the stale pair"
        );

        // What the local leg scores against — unchanged by the declaration above.
        let (prefill, decode) = local_mode_baselines();
        assert_eq!(prefill, OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN);
        assert_eq!(decode, OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN);
        assert_ne!(prefill, stale_prefill);
        assert_ne!(decode, stale_decode);

        // And the run really does use it: a full local pass over this golden reports the
        // constants as its denominators, never the golden's declaration.
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            prefill,
            decode,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(payload.passed, "error={}", payload.metrics.error);
        assert_eq!(payload.metrics.baseline_prefill_seconds_per_token, prefill);
        assert_eq!(payload.metrics.baseline_decode_seconds_per_token, decode);
        assert_ne!(
            payload.metrics.baseline_decode_seconds_per_token, stale_decode,
            "the golden's declared decode baseline reached the sealed score"
        );
    }

    // ---- #132(a): the baseline pair on EVERY local failure path -----------------------
    //
    // #74's ruling said the reference's failure record carries the official-runner CONSTANTS,
    // never zeros. The fix landed only on `preflight_failed_payload`, leaving five locally
    // reachable sites still sealing `baseline_* = 0` — a value the reference emits nowhere,
    // because every one of these returns through the SAME `failed()` closure
    // (`QwenRuntimeLocalIterate.swift@b26f76f:40-74`), which names no baseline argument, so
    // `failedScore`'s defaults stand (`QwenRuntimeBenchmark.swift@b26f76f:1130-1131`).
    //
    // One assertion per site, keyed on that site's DISTINGUISHING signature rather than on
    // "some failure happened" — otherwise a test could pass by reaching a different exit than
    // the one it claims to cover.

    /// The two fields under test, plus the signature that says WHICH exit produced them.
    fn assert_reference_baselines(p: &ScorePayload, site: &str) {
        assert!(
            !p.passed,
            "{site}: precondition — this is a failing payload"
        );
        assert!(
            p.score.is_none(),
            "{site}: precondition — a failing run has no score"
        );
        assert_eq!(
            p.metrics.baseline_prefill_seconds_per_token,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            "{site}: sealed a prefill baseline the reference never emits"
        );
        assert_eq!(
            p.metrics.baseline_decode_seconds_per_token, OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            "{site}: sealed a decode baseline the reference never emits"
        );
    }

    /// The #132(b) RULED seal: a local failure record on which the REFERENCE's fused
    /// checked-timing pass did not complete describes no run.
    ///
    /// The criterion is the reference's, deliberately: at `:455` and `:581` benchd's own
    /// conformance report EXISTS (its gate is a separate phase and it ran), so a benchd-centric
    /// reading of "no report" would be false there while the blank is still right.
    ///
    /// **RULED (David 2026-08-20, interview, FINAL): MIRROR BLANK STRICTLY.** Every local failure
    /// class — all six below — seals `golden_hash = ""` and zero counts, byte-matching the
    /// reference. Zero DECLARED cells on this surface.
    ///
    /// This REVERSES an earlier same-day ruling on the same question (keep-real-values + declare
    /// on the three separated-phase classes), and the reason it reversed is worth keeping next to
    /// the assertions: the reference's `goldenHash` carries an INVARIANT — non-empty means
    /// correctness completed — because it is only ever populated from a `CorrectnessReport` that
    /// exists (`QwenRuntimeBenchmark.swift@b26f76f:1161-1162,1176` read `?? ""` / `?? 0` off a nil
    /// report). Sealing benchd's real digest where correctness did NOT complete is not extra
    /// information; it is the same field meaning something weaker, inherited by every consumer
    /// that trusts the invariant. benchd's real data lives in LOGS, never in sealed records, and a
    /// benchd-only provenance field was explicitly considered and rejected.
    ///
    /// Pinned per-site because "the run knows the digest, so seal it" is the intuitive direction
    /// and it is the ruled-AGAINST one — twice over now.
    fn assert_ruled_blank_seal(p: &ScorePayload, site: &str) {
        assert_eq!(
            p.metrics.golden_hash, "",
            "{site}: #132(b) RULED — a record for a run that produced no correctness report must \
             seal an EMPTY golden_hash; populating it weakens the reference's invariant"
        );
        assert_eq!(
            p.metrics.case_count, 0,
            "{site}: #132(b) RULED — no correctness report ⇒ no case count"
        );
        assert_eq!(
            p.metrics.checked_steps, 0,
            "{site}: #132(b) RULED — no correctness report ⇒ no checked-step count"
        );
    }

    /// A spawner that cannot produce a timed session — for the two sites whose timing phase
    /// fails outright rather than mismatching.
    fn failing_spawner() -> bench_runner::Result<Session<MockEngine>> {
        Err(bench_runner::RunnerError::Protocol(
            "timed engine spawn failed".to_string(),
        ))
    }

    #[test]
    fn site_431_conformance_error_seals_the_reference_baselines() {
        // The conformance gate itself errored (engine protocol fault), so no correctness
        // report exists: `case_count` is the literal 0 this exit passes.
        let golden = TestGolden::new().steps(LI_EXPECTED).fixture();
        let (mut session, hello) = Session::connect(
            MockEngine::new()
                .teacher_forced_tokens(vec![2i64; LI_EXPECTED])
                .error_on("correctness_step", "engine exploded"),
        )
        .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(!payload.metrics.passed_correctness, "site :431 signature");
        assert!(
            payload.metrics.error.contains("engine exploded"),
            "site :431 signature — reached a different exit: {:?}",
            payload.metrics.error
        );
        assert_reference_baselines(&payload, "iterate.rs:431 conformance Err");
        assert_ruled_blank_seal(&payload, "iterate.rs:431 conformance Err");
    }

    #[test]
    fn site_455_close_phase_error_seals_the_reference_baselines() {
        // The gate PASSED and then the phase close failed, so this exit is the one that
        // reports `passed_correctness = true` with the golden's total case count.
        let golden = TestGolden::new().steps(LI_EXPECTED).fixture();
        let (mut session, hello) = Session::connect(
            MockEngine::new()
                .teacher_forced_tokens(vec![2i64; LI_EXPECTED])
                .cache_memory(4096),
        )
        .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(payload.metrics.passed_correctness, "site :455 signature");
        assert!(
            payload
                .metrics
                .error
                .contains("clear the MLX allocator cache"),
            "site :455 signature — reached a different exit: {:?}",
            payload.metrics.error
        );
        assert_reference_baselines(&payload, "iterate.rs:455 close_phase Err");
        assert_ruled_blank_seal(&payload, "iterate.rs:455 close_phase Err");
    }

    #[test]
    fn site_539_timeonly_failure_after_correctness_failure_seals_the_reference_baselines() {
        // Correctness FAILED, and the time-only retry could not spawn — so the run cannot
        // retain real timing and falls back to the blanked payload. Signature: the F1 primary
        // failure fields are populated AND the timing is absent.
        let golden = TestGolden::new()
            .steps(LI_EXPECTED)
            .corrupt_expected_at(3, 999)
            .fixture();
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            failing_spawner,
            no_cool_gate,
        );
        assert!(!payload.metrics.passed_correctness, "site :539 signature");
        assert_eq!(
            payload.metrics.first_failing_case.as_deref(),
            Some("local-iterate"),
            "site :539 signature — the correctness failure is still reported"
        );
        assert_eq!(
            payload.metrics.decode_seconds_per_token, 0.0,
            "site :539 signature — no timing was retained"
        );
        assert!(
            payload
                .metrics
                .error
                .contains("teacher-forced token mismatch"),
            "site :539 signature — reached a different exit: {:?}",
            payload.metrics.error
        );
        assert_reference_baselines(&payload, "iterate.rs:539 time-only Err");
        assert_ruled_blank_seal(&payload, "iterate.rs:539 time-only Err");
    }

    /// #132/F-5 — the sibling site swept in with the five named ones: window-too-short on the
    /// CORRECTNESS-FAILED arm (`iterate.rs:515`), as opposed to `:581` which is the same guard on
    /// the correctness-PASSED arm. Distinguished from `:581` by `passed_correctness`, and from
    /// `:539` by never reaching the timing spawner at all.
    #[test]
    fn site_515_window_too_short_after_correctness_failure_seals_the_blank_record() {
        // A golden that is BOTH too short to time AND corrupt at an index inside its window:
        // correctness fails first, then the retain-timing attempt hits the guard.
        let mut expected = vec![2i64; LI_STEPS];
        expected[3] = 999;
        let golden = TestGolden::new()
            .required_steps(LI_STEPS)
            .expected_tokens(expected)
            .fixture();
        assert_eq!(
            golden.cases[0].expected_tokens.len(),
            LI_STEPS,
            "precondition: at the guard boundary (len <= decode_steps), so timing is impossible"
        );
        let spawned = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let spawned_c = spawned.clone();
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            move || {
                spawned_c.set(spawned_c.get() + 1);
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(
            !payload.metrics.passed_correctness,
            "site :515 signature — correctness FAILED (this is what separates it from :581)"
        );
        assert!(
            payload
                .metrics
                .error
                .contains("teacher-forced token mismatch"),
            "site :515 signature — reached a different exit: {:?}",
            payload.metrics.error
        );
        assert_eq!(
            spawned.get(),
            0,
            "site :515 signature — the guard fires BEFORE any timed engine spawns \
             (that is what separates it from :539, where the spawn is attempted and fails)"
        );
        assert_reference_baselines(&payload, "iterate.rs:515 window too short after fail");
        assert_ruled_blank_seal(&payload, "iterate.rs:515 window too short after fail");
    }

    #[test]
    fn site_581_window_too_short_seals_the_reference_baselines() {
        // Correctness PASSED, but `expected_tokens.len() <= decode_steps` leaves no full
        // window to teacher-force, so the run cannot be timed at all. Signature:
        // `passed_correctness = true` with the Swift guard's message.
        let golden = TestGolden::new()
            .required_steps(LI_STEPS)
            .expected_fill(2)
            .fixture();
        assert_eq!(
            golden.cases[0].expected_tokens.len(),
            LI_STEPS,
            "precondition: exactly at the guard boundary (len <= decode_steps)"
        );
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            || {
                Session::connect(MockEngine::new().oracle_tokens(2, 2, vec![2i64; LI_STEPS]))
                    .map(|(s, _)| s)
            },
            no_cool_gate,
        );
        assert!(payload.metrics.passed_correctness, "site :581 signature");
        assert!(
            payload.metrics.error.contains("timing needs more than"),
            "site :581 signature — reached a different exit: {:?}",
            payload.metrics.error
        );
        assert_reference_baselines(&payload, "iterate.rs:581 window too short");
        assert_ruled_blank_seal(&payload, "iterate.rs:581 window too short");
    }

    #[test]
    fn site_619_timed_benchmark_error_seals_the_reference_baselines() {
        // Correctness PASSED and the timed phase itself failed. Signature:
        // `passed_correctness = true` with the golden's total case count and no timing.
        let golden = TestGolden::new().steps(LI_EXPECTED).fixture();
        let (mut session, hello) =
            Session::connect(MockEngine::new().teacher_forced_tokens(vec![2i64; LI_EXPECTED]))
                .unwrap();
        let payload = iterate_core(
            &mut session,
            &hello,
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            Mode::LocalIterate,
            false,
            RunDigests::for_test(&DirDigest::empty()),
            failing_spawner,
            no_cool_gate,
        );
        assert!(payload.metrics.passed_correctness, "site :619 signature");
        assert_eq!(
            payload.metrics.decode_seconds_per_token, 0.0,
            "site :619 signature — the timed phase produced nothing"
        );
        assert!(
            payload.metrics.error.contains("timed engine spawn failed"),
            "site :619 signature — reached a different exit: {:?}",
            payload.metrics.error
        );
        assert_reference_baselines(&payload, "iterate.rs:619 timed benchmark Err");
        assert_ruled_blank_seal(&payload, "iterate.rs:619 timed benchmark Err");
    }
    // -----------------------------------------------------------------------
    // F1 — the workspace harness identity
    // -----------------------------------------------------------------------

    /// F1 MUTATION PROOF (b) — EVERY MODE seals a 64-hex NON-EMPTY harness identity, and seals the
    /// one the run resolved.
    ///
    /// `base_metrics` is the single funnel every `iterate` payload passes through — official,
    /// official gates-only, local-iterate and local-submit alike — so this is the assertion that
    /// reds the moment `harness_hash: String::new()` comes back. It is stated per-mode rather than
    /// once because "all modes" was the ruling, and a mode-conditional stub would otherwise pass.
    #[test]
    fn base_metrics_seals_a_64_hex_harness_identity_on_every_mode() {
        let golden = benchmark_golden();
        let weights = DirDigest::empty();
        for mode in [Mode::LocalIterate, Mode::LocalSubmit, Mode::Official] {
            let metrics = base_metrics(mode, &golden, RunDigests::for_test(&weights));
            assert!(
                !metrics.harness_hash.is_empty(),
                "{}: harness_hash must never be sealed empty — the overlay refuses an empty \
                 harness identity, which is the whole reason F1 exists",
                mode.mode_name()
            );
            assert_eq!(
                metrics.harness_hash.len(),
                64,
                "{}: harness_hash must be a 64-character digest",
                mode.mode_name()
            );
            assert!(
                bench_core::harness_hash::is_well_formed_harness_hash(&metrics.harness_hash),
                "{}: harness_hash must be 64 LOWERCASE hex",
                mode.mode_name()
            );
            assert_eq!(
                metrics.harness_hash,
                HarnessIdentity::TEST_HASH,
                "{}: the sealed identity must be the one the run resolved, not an incidental value",
                mode.mode_name()
            );
        }
    }

    /// The FAILURE builders seal it too. A failed run still writes a score.json, and an artifact
    /// that cannot say which harness produced it is exactly what F1 removes — so the blank-seal
    /// rule (#132(b), which blanks `golden_hash` and the counts) must NOT extend to this field.
    #[test]
    fn failure_payloads_seal_the_harness_identity_too() {
        let golden = benchmark_golden();
        let weights = DirDigest::empty();
        let preflight = preflight_failed_payload(
            Mode::Official,
            &golden,
            RunDigests::for_test(&weights),
            "no baselines".to_string(),
        );
        assert_eq!(preflight.metrics.harness_hash, HarnessIdentity::TEST_HASH);
        // The blank-seal surface is untouched by F1 — proving this is the identity field only.
        assert!(preflight.metrics.golden_hash.is_empty());
        assert!(!preflight.passed);

        let failed = failed_payload(
            Mode::LocalIterate,
            FailureReport::message("boom".to_string(), false),
            &golden,
            RunDigests::for_test(&weights),
            None,
            None,
        );
        assert_eq!(failed.metrics.harness_hash, HarnessIdentity::TEST_HASH);
    }

    /// `HarnessIdentity::resolve` over a REAL synthetic workspace: a 64-hex identity that equals
    /// what `bench_core` computes for the same tree (no second algorithm in benchctl).
    #[test]
    fn harness_identity_resolves_over_a_real_workspace() {
        let root = std::env::temp_dir().join(format!("benchd-f1-ws-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for rel in [
            "Package.swift",
            "Sources/A.swift",
            "Tests/T.swift",
            "benchmark.json",
            "benchmark.sh",
            "setup.sh",
            "tools/t.sh",
            "README.md",
            "TASK.md",
        ] {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(&p, rel).expect("write");
        }
        let identity = HarnessIdentity::resolve(&root).expect("a complete workspace resolves");
        assert!(bench_core::harness_hash::is_well_formed_harness_hash(
            identity.as_str()
        ));
        assert_eq!(
            identity.as_str(),
            bench_core::harness_hash::harness_hash(&root).expect("hash"),
            "benchctl must not carry a second copy of the algorithm"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// F1 FAIL-CLOSED — a workspace missing a harness root REFUSES BY NAME. There is no
    /// `HarnessIdentity` to seal, so the run cannot start: no `""`, no partial hash, no artifact.
    #[test]
    fn a_workspace_missing_a_root_refuses_instead_of_yielding_an_identity() {
        let root = std::env::temp_dir().join(format!("benchd-f1-nows-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("mkdir");
        let err = HarnessIdentity::resolve(&root).expect_err("an empty directory is no workspace");
        assert!(
            err.contains("harnessHash root missing from disk"),
            "the refusal must name the missing root: {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
