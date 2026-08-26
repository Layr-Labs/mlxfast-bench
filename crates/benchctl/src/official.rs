//! B-2 — `benchctl` OFFICIAL benchmark path, at parity with the Swift official run
//! (`QwenRuntimeBenchmark.benchmarkWithWorker`, main.swift official gating, benchmark.sh).
//!
//! The official path diverges from the local checked-timing path (`crate::iterate`) in
//! several load-bearing ways, all corrected from the original steer against the read-first
//! Swift map:
//!
//! 1. **Timed-runs FIRST, then gates** (cold-path / anti-memoization defense,
//!    Constants.swift:82-88): prefill → decode → floor/band/finite evaluation →
//!    correctness. The timed phases run before any correctness so the measured path is
//!    never warmed by the gates (QwenRuntimeBenchmark.swift:466-558 precede :560-581).
//! 2. **THREE fresh sandboxed workers** — a dedicated `RuntimeWorkerClient` per phase
//!    (prefill :469-484, decode :486-503, correctness :565-581), each closed/reaped before
//!    the next. No shared session, no warm caches across phases.
//! 3. **Full correctness set** — base cases + anchors + free_run (+ behavior/GPQA on the
//!    GPU path): `checkGates == true` ⇒ `caseCount = totalCorrectnessCaseCount`,
//!    `gates = correctnessGates` (QwenRuntimeCorrectness.swift:351/435). Contrast local's
//!    base-cases-only default.
//! 4. **128-step decode window + benchmark-ORACLE checks** (Constants.swift:70, seed 512):
//!    every timed token is verified against `golden.benchmark.expected_*`; a corrupted
//!    oracle FAILS official (`BenchmarkTokenMismatchError`) — the failure class the local
//!    path structurally cannot test.
//! 5. **Official gating** — 0.95 speedup floors, prefill band ±5% / decode +2%−5%, and a
//!    non-finite score fails, evaluated BEFORE correctness (Score.swift:50-126).
//! 6. **Sealing/integrity** — the sealed stdout payload is the coarsened score (2-sig-fig
//!    diagnostics, ranking fields untouched); the `metrics.commit` carries the resolved
//!    commit identifier (official-only).
//!
//! The SANDBOX fail-closed spawn + Seatbelt profile live in `bench_runner::sandbox`; this
//! module owns the phase orchestration + scoring. The REAL timed measurement and the
//! end-to-end both-sides run are B-3 on the GPU box — this module is macOS-buildable and
//! unit-tested against a stub `MockEngine` (no real engine, no GPU).

use bench_core::conformance::{run_conformance, ConformanceReport, CorrectnessScope};
use bench_core::constants::{
    OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN, OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
};
use bench_core::golden::GoldenFixture;
use bench_core::score::evaluate_timed_run;
use bench_runner::{
    run_timed_benchmark_fresh_per_phase, scrub_reason_for_seal, LineTransport, RunnerError,
    Session, TimingParams, TimingResult,
};

use crate::iterate::{
    apply_timing_metrics, base_metrics, finite_nonneg, first_conformance_failure, Mode, RunDigests,
    SessionEngine,
};
use crate::score::ScorePayload;

/// Whether an official run REQUIRES the runtime worker (Swift
/// `benchmarkRequiresRuntimeWorker`, QwenRuntimeBenchmark.swift:295-300): true iff the
/// golden declares any correctness BEHAVIOR case (hidden GPQA TTFT). Behavior TTFT is
/// measured in the trusted parent around sandboxed worker calls; the in-process path cannot
/// produce an equivalent trusted measurement, so its presence forces the worker path (and
/// benchctl's official path fails closed without a sandboxed worker regardless).
///
/// Parity port + unit-tested; benchctl's official run ALWAYS spawns sandboxed workers
/// (fail-closed), so this predicate is subsumed by that guarantee here and reserved for a
/// future worker-optional decision — hence `allow(dead_code)`.
#[allow(dead_code)]
pub fn benchmark_requires_runtime_worker(golden: &GoldenFixture) -> bool {
    golden
        .correctness_gates
        .as_ref()
        .is_some_and(|g| !g.behavior_cases().is_empty())
}

/// #61 — the OFFICIAL paired-baseline trusted override (Swift `PairedBaselineOverride`,
/// BenchmarkSupport.swift:35-80). The official timing machine measures the pinned reference
/// implementation immediately before the candidate and passes its seconds-per-token through
/// `MLXFAST_PAIRED_BASELINE_PREFILL_SECONDS_PER_TOKEN` /
/// `MLXFAST_PAIRED_BASELINE_DECODE_SECONDS_PER_TOKEN`, so candidate speedups and floors are
/// priced against the same runner VM / hour / thermal state (docs/measure-job-contract.md).
///
/// FAIL-CLOSED, byte-for-byte with Swift `fromEnvironment` (:50-79):
/// - both unset (after trim) ⇒ `Ok(None)` (no override; fall through to flags/golden);
/// - exactly one set ⇒ error "…must be provided together" (a half-set pair is an operator
///   wiring error and must stop the run, never silently degrade);
/// - a value that is not a FINITE POSITIVE double ⇒ error (mispricing the whole session).
///
/// Official-only: local modes never consult this (Swift strips both keys from the sandboxed
/// worker env, and only the benchmark/official paths read them). `prefill_raw`/`decode_raw`
/// are the raw env values (`None` if unset), passed in so the resolution is a pure,
/// testable function.
pub fn paired_baseline_from_env(
    prefill_raw: Option<&str>,
    decode_raw: Option<&str>,
) -> Result<Option<(f64, f64)>, String> {
    const PREFILL_KEY: &str = "MLXFAST_PAIRED_BASELINE_PREFILL_SECONDS_PER_TOKEN";
    const DECODE_KEY: &str = "MLXFAST_PAIRED_BASELINE_DECODE_SECONDS_PER_TOKEN";
    let prefill = prefill_raw.unwrap_or("").trim();
    let decode = decode_raw.unwrap_or("").trim();
    if prefill.is_empty() && decode.is_empty() {
        return Ok(None);
    }
    if prefill.is_empty() || decode.is_empty() {
        return Err(format!(
            "{PREFILL_KEY} and {DECODE_KEY} must be provided together"
        ));
    }
    let prefill_v = parse_finite_positive(prefill).ok_or_else(|| {
        format!("{PREFILL_KEY} must be a finite positive seconds-per-token value")
    })?;
    let decode_v = parse_finite_positive(decode)
        .ok_or_else(|| format!("{DECODE_KEY} must be a finite positive seconds-per-token value"))?;
    Ok(Some((prefill_v, decode_v)))
}

/// A finite, strictly-positive `f64` from a string, else `None` (Swift `Double(raw)` +
/// `isFinite` + `> 0`).
fn parse_finite_positive(s: &str) -> Option<f64> {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => Some(v),
        _ => None,
    }
}

/// Port of Swift `QwenRuntimePreflight.isCommitSHAHex` (:40-45): a lowercase-hex string of
/// 7..=40 chars (short-to-full commit SHA). NOTE the correction to the original "40-hex"
/// steer — Swift's `metrics.commit` predicate accepts 7-40 (only benchmark.sh's
/// `candidate.sha` recovery is strict-40); this mirrors the field's real source.
pub fn is_commit_sha_hex(s: &str) -> bool {
    let n = s.len();
    (7..=40).contains(&n)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Port of Swift `QwenRuntimePreflight.commitIdentifier` (:26-45): a trimmed, valid-hex
/// `MLXFAST_COMMIT_SHA` wins; otherwise fall back to `git rev-parse --short HEAD`, or `""`.
/// `commit_sha_env` is the raw `MLXFAST_COMMIT_SHA` value (or `None` if unset), passed in so
/// the resolution is a pure, testable function; the git fallback is only consulted when the
/// env value is absent or malformed.
pub fn commit_identifier(commit_sha_env: Option<&str>) -> String {
    if let Some(v) = commit_sha_env {
        let trimmed = v.trim();
        if is_commit_sha_hex(trimmed) {
            return trimmed.to_string();
        }
    }
    git_short_head().unwrap_or_default()
}

/// `git rev-parse --short HEAD`, trimmed; `None` on any failure (Swift's `(try? …) ?? ""`).
fn git_short_head() -> Option<String> {
    let out = std::process::Command::new("/usr/bin/git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// AUTHOR-AT-SEAL (DECIDE-3). The SOLE authority for the sealed `metrics.commit` is the sha the
/// in-repo dispatch script RECORDED from the CI/yukon dispatch context — the challenger
/// `candidate.sha` shape: a trusted-workflow-authored 40-hex commit id. benchd AUTHORS the sealed
/// commit FROM that record; a competitor-proposed commit — `MLXFAST_COMMIT_SHA`, which the engine's
/// `commitIdentifier` emits — is DEFENCE-IN-DEPTH ONLY: present-and-disagreeing is a die-class
/// refuse, never a silent override of the record. Participant git state is deliberately NOT an
/// input here: `git rev-parse` is unusable under the ranked sandbox (dubious-ownership under
/// `env -i`), which is precisely why the trusted dispatch context, not the checkout, is authority.
///
/// - `dispatch_record`: the RECORDED dispatched sha (trimmed `candidate.sha` contents), or `None`
///   when no dispatch context is present. `Some(_)` means a dispatch PROMISED a record, so a
///   malformed/empty value is a refuse — never a silent fall-through to the git identity.
/// - `proposed`: the raw, untrusted `MLXFAST_COMMIT_SHA` (or `None`).
/// - `scoring_mode`: `true` on a SCORING/ranked seal (the default measure-job mode; `false` only
///   under `--local-dev`). A scoring seal is FAIL-CLOSED: an absent dispatch record REFUSES rather
///   than falling back to the box git identity — the `git_short_head().unwrap_or_default()`
///   fallback could otherwise seal an EMPTY `metrics.commit` on a scoring run whose outer dispatch
///   never exported the context (the present-but-unwired threat). The git fallback survives ONLY in
///   dev/local mode, gated on the SAME `--local-dev` signal the official pair loop keys on
///   (`cfg.local_pair_budget`).
///
/// Contract:
/// - record present → it MUST be strict 40-char lowercase-hex (the workflow-authored shape); else
///   REFUSE. The sealed commit is AUTHORED from it. If `proposed` is present, valid-hex, and
///   neither equal to nor a hex prefix of the record (the engine may emit a short form of the SAME
///   commit), the seal is REFUSED — the proposal disagrees with what was dispatched.
/// - record absent + `scoring_mode` → REFUSE (die-class): a scoring seal must not fall back to git.
/// - record absent + dev/local → fallback to [`commit_identifier`] (no bind), behaviour unchanged.
pub fn author_sealed_commit(
    dispatch_record: Option<&str>,
    proposed: Option<&str>,
    scoring_mode: bool,
) -> Result<String, String> {
    let record = match dispatch_record {
        // A dispatch context was present: this record is the authority even if it is junk (a
        // dispatch that promised a record must not fall through to the git identity).
        Some(r) => r.trim(),
        // No dispatch context. A SCORING/ranked seal fails closed here — never git_short_head, which
        // can seal an empty commit on a scoring run whose dispatch never wired the context.
        None if scoring_mode => {
            return Err(
                "author-at-seal: a scoring/ranked seal requires the dispatched commit record \
                 (candidate.sha via MLXFAST_CANDIDATE_SHA_FILE); none was present — refusing to \
                 fall back to the box git identity on a scoring run (pass --local-dev for the \
                 unbound local resolution)"
                    .to_string(),
            );
        }
        // Dev/local only: the pre-existing, un-bound resolution.
        None => return Ok(commit_identifier(proposed)),
    };
    if !is_dispatch_record_sha(record) {
        return Err(format!(
            "author-at-seal: the dispatch record (candidate.sha) is {record:?}, not a \
             40-character lowercase-hex commit sha; refusing to seal a score against an \
             unidentified dispatch"
        ));
    }
    // Defence-in-depth cross-check. The record is already the authority; this only decides whether
    // a PRESENT proposal is a benign short form of the same commit or an actual DISAGREEMENT.
    if let Some(p) = proposed.map(str::trim).filter(|s| !s.is_empty()) {
        if is_commit_sha_hex(p) && !commit_agrees_with_record(record, p) {
            return Err(format!(
                "author-at-seal: the proposed commit (MLXFAST_COMMIT_SHA) {p:?} disagrees with the \
                 dispatched record {record:?}; refusing (the dispatch record is the sole authority \
                 for metrics.commit)"
            ));
        }
    }
    Ok(record.to_string())
}

/// The strict 40-char lowercase-hex shape of a RECORDED dispatch sha (the workflow-authored
/// `candidate.sha`). Distinct from the 7..=40 `is_commit_sha_hex` predicate that gates the
/// `metrics.commit` FIELD: a recorded dispatch sha is always a full commit id, so the record is
/// held to the strict shape the trusted shell validates before it writes the file.
fn is_dispatch_record_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A proposed commit AGREES with the record iff it equals the record or is a hex PREFIX of it (the
/// engine's `commitIdentifier` may emit `git rev-parse --short HEAD`). The 40-hex record is the
/// authority either way; this is the disagreement predicate the seal binds on.
fn commit_agrees_with_record(record: &str, proposed: &str) -> bool {
    record == proposed || record.starts_with(proposed)
}

/// Run the full OFFICIAL benchmark and return the sealed-ready [`ScorePayload`]. Pure over
/// the transport so tests drive it with an in-process `MockEngine`.
///
/// Lifecycle (Swift `benchmarkWithWorker`, timed-FIRST):
/// - `spawn_timed` yields a fresh (sandboxed, in production) worker; it is invoked TWICE by
///   [`run_timed_benchmark_fresh_per_phase`] — once for the prefill worker, once for the
///   decode worker. Both timed phases VERIFY every token against the golden benchmark oracle.
/// - `spawn_correctness` yields the THIRD fresh worker for the full correctness set.
/// - Official has NO cool gate (that path never calls it) — a no-op is threaded through.
///
/// Gating order matches Swift exactly: oracle mismatch (timed) → non-finite/floors/bands →
/// correctness. A failure at any stage returns a failed payload (`score = null`,
/// `passed = false`) that still RETAINS the real timing surface where it was measured.
#[allow(clippy::too_many_arguments)]
pub fn official_core<T, FT, FC>(
    golden: &GoldenFixture,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
    digests: RunDigests<'_>,
    commit: &str,
    mut spawn_timed: FT,
    spawn_correctness: FC,
) -> ScorePayload
where
    T: LineTransport,
    FT: FnMut() -> bench_runner::Result<Session<T>>,
    FC: FnMut() -> bench_runner::Result<Session<T>>,
{
    // Official times the golden's benchmark ORACLE workload (Swift's official benchmark),
    // NOT cases[0]. A benchmark-less golden fails preflight (Swift throws before spawning).
    let benchmark = match &golden.benchmark {
        Some(b) => b,
        None => {
            return official_failed(
                golden,
                digests,
                commit,
                "benchmark golden file must contain a benchmark oracle".to_string(),
                false,
                None,
                None,
                None,
                None,
                (baseline_prefill_spt, baseline_decode_spt),
            )
        }
    };
    let params = TimingParams::new(
        benchmark.prefill_prompt_tokens.clone(),
        benchmark.expected_prefill_token,
        benchmark.decode_seed_tokens.clone(),
        benchmark.expected_decode_seed_token,
        benchmark.expected_decode_tokens.clone(),
        Mode::Official.decode_steps(),
    );

    // 1. TIMED phases FIRST — prefill worker then decode worker, each fresh (2 workers),
    //    every token VERIFIED against the oracle. Official has no cool gate.
    let mut no_cool_gate = |_phase: &str| -> bench_runner::Result<()> { Ok(()) };
    let measured =
        match run_timed_benchmark_fresh_per_phase(&mut spawn_timed, &mut no_cool_gate, &params) {
            Ok(t) => t,
            Err(RunnerError::TokenMismatch { label, step, .. }) => {
                // The benchmark-ORACLE failure class the local path cannot test: a corrupted
                // oracle (or a fast-garbage engine) diverges and FAILS official. Byte-match Swift
                // `makeFailedScore` for a `BenchmarkTokenMismatchError`
                // (QwenRuntimeBenchmark.swift:668-676): `error = mismatch.description`,
                // `firstFailingCase = "benchmark"`, `firstFailingStep = mismatch.step`, and
                // `expectedToken`/`actualToken` are ALWAYS nil.
                //
                // Swift's PREFILL and decode-SEED comparisons go through `compareOne`
                // (Golden.swift:560-581) with `step: nil`, so `description` has NO " at step N"
                // suffix and `firstFailingStep = nil`. Only the decode-TOKEN class
                // (`compareDecodeTokens`, Golden.swift:533-551) carries a step. benchctl's
                // `RunnerError::TokenMismatch.step` is non-optional, so distinguish by label: the
                // decode-token phase is labelled "benchmark decode token"; prefill/seed are
                // step-less. (Descriptions matched verbatim: "benchmark prefill token mismatch",
                // "benchmark decode seed token mismatch", "benchmark decode token mismatch at
                // step N".)
                let is_decode_token_class = label == "benchmark decode token";
                let (error, first_failing_step) = if is_decode_token_class {
                    (
                        format!("{label} mismatch at step {step}"),
                        Some(step as i64),
                    )
                } else {
                    (format!("{label} mismatch"), None)
                };
                // The benchmark-ORACLE mismatch is a TIMED-phase failure BEFORE correctness runs
                // (official is timed-first). Like RULING-2's band/floor/finite path, Swift returns
                // via makeFailedScore(correctness: nil) — BLANKING the correctness audit fields
                // (golden_hash="", case_count=0, checked_steps=0) — but RETAINS the resolved
                // baselines (baselinePrefill/DecodeSecondsPerToken, set at :434-435 before the timed
                // phase). See official_failed_timed_oracle.
                return official_failed_timed_oracle(
                    golden,
                    digests,
                    commit,
                    error,
                    first_failing_step,
                    baseline_prefill_spt,
                    baseline_decode_spt,
                );
            }
            Err(e) => {
                // A non-oracle timed failure (protocol / completed-work barrier / spawn): fail
                // closed with the runner's message. No trustworthy timing to retain.
                return official_failed(
                    golden,
                    digests,
                    commit,
                    format!("{e}"),
                    false,
                    None,
                    None,
                    None,
                    None,
                    (baseline_prefill_spt, baseline_decode_spt),
                );
            }
        };

    // Steps 2-4 (gating → correctness → assembly) operate on the measured `timing`; they are
    // factored into `finish_official` so they can be unit-tested with a SYNTHETIC in-band
    // TimingResult (a mock's ~0 wall-clock can never sit inside the acceptance band).
    finish_official(
        golden,
        baseline_prefill_spt,
        baseline_decode_spt,
        digests,
        commit,
        &measured,
        spawn_correctness,
    )
}

/// Steps 2-4 of the official flow, given the already-measured `timing`: official GATING
/// (non-finite → floors → bands), then FULL-scope correctness on a fresh worker, then the
/// passing-score assembly. Separated from the timed phase so both the gating (with synthetic
/// in-band timings) and the orchestration (with a mock timed phase) are unit-testable.
fn finish_official<T, FC>(
    golden: &GoldenFixture,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
    digests: RunDigests<'_>,
    commit: &str,
    timing: &TimingResult,
    mut spawn_correctness: FC,
) -> ScorePayload
where
    T: LineTransport,
    FC: FnMut() -> bench_runner::Result<Session<T>>,
{
    // 2. Official GATING, evaluated BEFORE correctness (Swift Score.swift:50-126,
    //    QwenRuntimeBenchmark.swift:513-558): non-finite score → floors (0.95) → acceptance
    //    bands (prefill ±5%, decode +2%/−5%). The FIRST failure reason (in that priority)
    //    fails the run; real timing is retained.
    let eval = evaluate_timed_run(
        timing.decode_seconds_per_token,
        timing.prefill_seconds_per_token,
        baseline_decode_spt,
        baseline_prefill_spt,
    );
    if let Some(reason) = eval.first_failure_reason() {
        // RULING 2 (trusted-core, official path): this is the TIMED-band failure — non-finite
        // score / speedup floor / acceptance band — which Swift evaluates BEFORE correctness
        // runs. `official_failed_timed_band` retains the real measured timing surface but
        // BLANKS the correctness-derived audit fields (golden_hash="", case_count=0,
        // checked_steps=0) to byte-match Swift's `correctness == nil` failed score. This is
        // DISTINCT from the correctness-failure path below, which is left unchanged.
        return official_failed_timed_band(
            golden,
            digests,
            commit,
            reason,
            timing,
            baseline_prefill_spt,
            baseline_decode_spt,
        );
    }

    // 3. CORRECTNESS on the THIRD fresh worker, FULL scope.
    //
    // ⚠️ MINOR-1 (B-2): `CorrectnessScope::Full` evaluates base cases + anchors + free_run,
    // but NOT the BEHAVIOR / GPQA-TTFT gates — bench-core conformance does not yet execute
    // them (the report carries no `behavior` vector; `benchmark_requires_runtime_worker`
    // DETECTS a behavior-carrying golden but nothing evaluates it). Behavior/GPQA execution is
    // deferred to B-3. UNTIL THEN, official is NOT a complete correctness gate for a
    // behavior-carrying golden: a corrupted behavior case would PASS. Do not treat a passing
    // official run over such a golden as full-correctness evidence.
    //
    // A spawn failure fails closed.
    let mut correctness_session = match spawn_correctness() {
        Ok(s) => s,
        Err(e) => {
            return official_failed_with_timing(
                golden,
                digests,
                commit,
                format!("correctness worker spawn failed: {e}"),
                timing,
                baseline_prefill_spt,
                baseline_decode_spt,
            )
        }
    };
    let report = {
        let mut adapter = SessionEngine {
            session: &mut correctness_session,
            drained_once: false,
        };
        match run_conformance(
            &mut adapter,
            golden,
            bench_core::constants::CORRECTNESS_STEPS,
            CorrectnessScope::Full,
        ) {
            Ok(r) => r,
            Err(e) => {
                return official_failed_with_timing(
                    golden,
                    digests,
                    commit,
                    format!("{e}"),
                    timing,
                    baseline_prefill_spt,
                    baseline_decode_spt,
                )
            }
        }
    };
    // Close the final correctness sub-phase (the per-sequence barrier owner, as in
    // iterate_core) so no completed-work leaks; a barrier failure fails the run.
    if let Err(e) = correctness_session.close_phase() {
        return official_failed_with_timing(
            golden,
            digests,
            commit,
            format!("{e}"),
            timing,
            baseline_prefill_spt,
            baseline_decode_spt,
        );
    }

    if !report.passed {
        let (case, step, error) = official_correctness_failure(&report);
        return official_failed_with_timing_and_case(
            golden,
            digests,
            commit,
            error,
            timing,
            baseline_prefill_spt,
            baseline_decode_spt,
            case,
            step,
            // OFFICIAL correctness failure leaves expected/actual NULL (Swift failedScore reads
            // only the explicit param — nil here — never `correctness?.expectedToken`;
            // QwenRuntimeBenchmark.swift:1155-1156). This is the LOCAL/OFFICIAL split: the local
            // path (iterate.rs) DOES populate them.
            None,
            None,
            // The REAL partial per-case checked-step sum through the failing gate (Swift
            // `correctness?.checkedSteps`, :1143): anchor-fail and free-run-fail yield the
            // accumulated sum, NOT the placeholder case count.
            report.checked_steps(),
        );
    }

    // 4. PASSING official score: the weighted-geometric-mean score (never coarsened),
    //    real timing surface, full correctness case counts, and the resolved commit.
    let mut metrics = base_metrics(Mode::Official, golden, digests);
    apply_timing_metrics(
        &mut metrics,
        timing,
        baseline_prefill_spt,
        baseline_decode_spt,
    );
    metrics.passed_correctness = true;
    metrics.commit = commit.to_string();
    // checkGates == true ⇒ caseCount = totalCorrectnessCaseCount (Swift
    // QwenRuntimeCorrectness.swift:351). checked_steps is now the REAL per-case checked-step
    // SUM (Swift `runLayeredCorrectness` accumulator, :192-306) — a passing official run sums
    // every evaluated base/anchor/free-run case, NOT the placeholder case count. (Behavior/GPQA
    // steps remain the documented B-3 gap; see ConformanceReport::checked_steps.)
    metrics.case_count = golden.total_correctness_case_count() as i64;
    metrics.checked_steps = report.checked_steps();
    ScorePayload {
        score: Some(eval.score),
        passed: true,
        metrics,
    }
}

/// GATES-ONLY official run (seam 1): `MLXFAST_BENCHMARK_SKIP_TIMED=1` skips the timed phases and
/// runs ONLY the correctness gates, sealing a `partial_result=true` gates-score — the seam-1
/// shape the paired overlay (`overlay-timing`) later completes with the measured timing. This is
/// benchd's parity implementation of the reference `mlxfast-swift` SKIP_TIMED path
/// (`main.swift@b26f76f:386,397` → `QwenRuntimeBenchmark.swift@b26f76f:457`; #132/F-7 corrected
/// this from `main.swift:321-322`, which is the local branch): a passing gates run is
/// `passed=true`, `partial_result=true`,
/// `passed_correctness=true`, `error==""`, `score=null`, with the timing fields left at their
/// zero placeholders (the overlay owns them). No timed phase runs, so no paired baselines are
/// needed here (correctness is oracle-only). Pure over the transport so tests drive it with an
/// in-process `MockEngine`.
/// The OFFICIAL path's baseline pair, in the reference's own resolution order (#132/F-2).
///
/// `pairedBaseline ?? benchmarkGolden.resolvedBaseline*`, where `resolvedBaseline*` is itself
/// `golden's declared pair ?? MLXFastConstants.officialBaseline*`
/// (`QwenRuntimeBenchmark.swift@b26f76f:441-445`, `Golden.swift@b26f76f:220-226`).
///
/// **Why this exists.** The reference OVERWRITES its baseline locals at `:442-445`, and it does so
/// BEFORE the `skipTimedBenchmark` branch at `:457` and before every `makeFailedScore` in
/// `benchmarkWithWorker` (`:533,:545,:560,:625,:638,:680,:690`). So on the official path the
/// initialiser values at `:350-351` are never what a sealed record carries — the resolved pair is.
/// #132(a) moved benchd's `base_metrics` default from `0.0` to the CONSTANTS, which is right for
/// the LOCAL surface (there the reference genuinely seals its constants) but leaked into the three
/// official payloads that no later step overwrites: the gates-only PASSING partial, the gates
/// failure, and `official_failed`. Those now resolve properly rather than inheriting either the
/// old `0.0` or the local surface's constants.
///
/// The env override is read here, mirroring the reference's inline
/// `try PairedBaselineOverride.fromEnvironment()` at `:441`. A half-set/invalid pair resolves to
/// `None` (falling through to the golden) rather than erroring: on the TIMED official path
/// `execute_iterate` has already hard-errored on exactly that condition before reaching this code,
/// and on the gates-only path benchd has never validated the env pair at all. That second gap is
/// PRE-EXISTING and untouched here — noted so it is not mistaken for something this helper
/// introduced.
fn official_resolved_baselines(golden: &GoldenFixture) -> (f64, f64) {
    let paired = paired_baseline_from_env(
        std::env::var("MLXFAST_PAIRED_BASELINE_PREFILL_SECONDS_PER_TOKEN")
            .ok()
            .as_deref(),
        std::env::var("MLXFAST_PAIRED_BASELINE_DECODE_SECONDS_PER_TOKEN")
            .ok()
            .as_deref(),
    )
    .ok()
    .flatten();
    if let Some((prefill, decode)) = paired {
        return (prefill, decode);
    }
    // Per-FIELD `?? officialBaseline*`, exactly as `resolvedBaseline*` is defined. The loader
    // already enforces the pair all-or-nothing, so the two spellings cannot diverge in practice —
    // this one just cannot drift from the reference's definition if that ever changes.
    let declared = golden.benchmark.as_ref();
    (
        declared
            .and_then(|b| b.baseline_prefill_seconds_per_token)
            .unwrap_or(OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN),
        declared
            .and_then(|b| b.baseline_decode_seconds_per_token)
            .unwrap_or(OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN),
    )
}

pub fn official_gates_only<T, FC>(
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    commit: &str,
    mut spawn_correctness: FC,
) -> ScorePayload
where
    T: LineTransport,
    FC: FnMut() -> bench_runner::Result<Session<T>>,
{
    // CORRECTNESS on a fresh (sandboxed, in production) worker, FULL scope — identical to the
    // timed path's correctness step (official_core step 3), just with no preceding timed phase.
    // A spawn failure fails closed as a gates-only failed score.
    let mut correctness_session = match spawn_correctness() {
        Ok(s) => s,
        Err(e) => {
            return official_gates_failed(
                golden,
                digests,
                commit,
                format!("correctness worker spawn failed: {e}"),
                None,
                None,
                0,
            )
        }
    };
    let report = {
        let mut adapter = SessionEngine {
            session: &mut correctness_session,
            drained_once: false,
        };
        match run_conformance(
            &mut adapter,
            golden,
            bench_core::constants::CORRECTNESS_STEPS,
            CorrectnessScope::Full,
        ) {
            Ok(r) => r,
            Err(e) => {
                return official_gates_failed(
                    golden,
                    digests,
                    commit,
                    format!("{e}"),
                    None,
                    None,
                    0,
                )
            }
        }
    };
    // Close the final correctness sub-phase (the per-sequence barrier owner) so no completed-work
    // leaks; a barrier failure fails the run.
    if let Err(e) = correctness_session.close_phase() {
        return official_gates_failed(golden, digests, commit, format!("{e}"), None, None, 0);
    }

    if !report.passed {
        let (case, step, error) = official_correctness_failure(&report);
        return official_gates_failed(
            golden,
            digests,
            commit,
            error,
            case,
            step,
            report.checked_steps(),
        );
    }

    // PASSING gates-only score: partial_result=true (awaiting the timed overlay), null score,
    // full correctness case counts, resolved commit, zero timing placeholders.
    let mut metrics = base_metrics(Mode::Official, golden, digests);
    // #132/F-2: the BASELINE pair is not a zero placeholder and never was — the reference has
    // already overwritten it with `pairedBaseline ?? golden.resolvedBaseline*` before it reaches
    // the gates-only branch (`QwenRuntimeBenchmark.swift@b26f76f:442-445` then `:457`).
    let (baseline_prefill_spt, baseline_decode_spt) = official_resolved_baselines(golden);
    metrics.baseline_prefill_seconds_per_token = baseline_prefill_spt;
    metrics.baseline_decode_seconds_per_token = baseline_decode_spt;
    metrics.passed_correctness = true;
    metrics.partial_result = true;
    metrics.commit = commit.to_string();
    metrics.case_count = golden.total_correctness_case_count() as i64;
    metrics.checked_steps = report.checked_steps();
    ScorePayload {
        score: None,
        passed: true,
        metrics,
    }
}

/// A FAILED gates-only payload (`score = null`, `passed = false`) that keeps `partial_result=true`
/// (it is still a gates-only shape, just fail-closed) with the correctness audit surface. No
/// timing is retained (the timed phase never ran).
fn official_gates_failed(
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    commit: &str,
    error: String,
    first_failing_case: Option<String>,
    first_failing_step: Option<i64>,
    checked_steps: i64,
) -> ScorePayload {
    let mut metrics = base_metrics(Mode::Official, golden, digests);
    // #132/F-2 — same resolution as the passing gates score above; the reference's failure
    // records in `benchmarkWithWorker` are all reached AFTER the `:442-445` overwrite.
    let (baseline_prefill_spt, baseline_decode_spt) = official_resolved_baselines(golden);
    metrics.baseline_prefill_seconds_per_token = baseline_prefill_spt;
    metrics.baseline_decode_seconds_per_token = baseline_decode_spt;
    metrics.passed_correctness = false;
    metrics.partial_result = true;
    metrics.commit = commit.to_string();
    // #134 — SEAL BOUNDARY (see `iterate::failed_payload`). Official is the MOST exposed sink:
    // its score.json travels, and worker stderr is never forwarded here, so this scrub is the
    // only thing between engine-controlled bytes and the artifact.
    metrics.error = scrub_reason_for_seal(&error);
    metrics.case_count = golden.total_correctness_case_count() as i64;
    metrics.checked_steps = checked_steps;
    metrics.first_failing_case = first_failing_case;
    metrics.first_failing_step = first_failing_step;
    ScorePayload {
        score: None,
        passed: false,
        metrics,
    }
}

/// A FAILED official payload (`score = null`, `passed = false`) with NO retained timing
/// (used for oracle mismatch, benchmark-less golden, and pre-timing spawn faults).
#[allow(clippy::too_many_arguments)]
/// `baselines` is the pair `official_core` already resolved (env ?? `--baseline-*` flags ??
/// golden) — #132/F-2. Threaded rather than re-derived, because `official_core`'s value is the
/// only one that has seen the flags; re-resolving from the golden here would silently drop them.
/// The reference does the same: its failure records read the locals it overwrote at
/// `QwenRuntimeBenchmark.swift@b26f76f:442-445`, not the initialisers at `:350-351`.
fn official_failed(
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    commit: &str,
    error: String,
    passed_correctness: bool,
    first_failing_case: Option<String>,
    first_failing_step: Option<i64>,
    expected_token: Option<i64>,
    actual_token: Option<i64>,
    baselines: (f64, f64),
) -> ScorePayload {
    let mut metrics = base_metrics(Mode::Official, golden, digests);
    metrics.baseline_prefill_seconds_per_token = baselines.0;
    metrics.baseline_decode_seconds_per_token = baselines.1;
    metrics.passed_correctness = passed_correctness;
    metrics.commit = commit.to_string();
    // #134 — SEAL BOUNDARY (see `iterate::failed_payload`). Official is the MOST exposed sink:
    // its score.json travels, and worker stderr is never forwarded here, so this scrub is the
    // only thing between engine-controlled bytes and the artifact.
    metrics.error = scrub_reason_for_seal(&error);
    metrics.case_count = golden.total_correctness_case_count() as i64;
    metrics.checked_steps = golden.total_correctness_case_count() as i64;
    metrics.first_failing_case = first_failing_case;
    metrics.first_failing_step = first_failing_step;
    metrics.expected_token = expected_token;
    metrics.actual_token = actual_token;
    ScorePayload {
        score: None,
        passed: false,
        metrics,
    }
}

/// A FAILED official payload that RETAINS the real timing surface (floor/band/finite failure
/// and post-timing correctness/barrier faults): the timed phases DID measure real numbers,
/// so the payload carries them (Swift's failed score keeps the measured decode/prefill spt +
/// speedups) while `score` stays `null` and `passed = false`.
#[allow(clippy::too_many_arguments)]
fn official_failed_with_timing(
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    commit: &str,
    error: String,
    timing: &TimingResult,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
) -> ScorePayload {
    official_failed_with_timing_and_case(
        golden,
        digests,
        commit,
        error,
        timing,
        baseline_prefill_spt,
        baseline_decode_spt,
        None,
        None,
        None,
        None,
        // Pre-correctness/barrier timed faults have no ConformanceReport to sum; retain the
        // prior placeholder (golden case-count) for these UNLISTED fault paths — the listed
        // correctness-failure path below passes the real `report.checked_steps()`.
        golden.total_correctness_case_count() as i64,
    )
}

/// RULING 2 (trusted-core, official path) — a FAILED official payload for a TIMED-BAND
/// failure (non-finite score / speedup floor / acceptance band), which Swift's official path
/// evaluates BEFORE correctness runs. Retains the real measured timing surface (like
/// [`official_failed_with_timing`]) but ALIGNS TO SWIFT by BLANKING the correctness-derived
/// audit fields: `golden_hash = ""`, `case_count = 0`, `checked_steps = 0`.
///
/// Why Swift blanks them here: `benchmarkWithWorker` is timed-FIRST, so the floor/band/finite
/// gates return via `makeFailedScore(correctness: correctnessReport, …)` while
/// `correctnessReport` is still `nil` — it is only assigned AFTER the correctness worker runs
/// (QwenRuntimeBenchmark.swift:520-558 precede :596). `failedScore` then defaults
/// `checkedSteps → correctness?.checkedSteps ?? 0` = 0, `caseCount → … ?? 0` = 0, and
/// `goldenHash → correctness?.goldenHash ?? ""` = "" (:1143/1144/1158). Official artifacts
/// feed downstream/organizer tooling that expects Swift's shape, so benchctl (the PRODUCER)
/// aligns to Swift rather than zeroing in the differ (differ-side zeroing rejected).
///
/// SCOPE: only the timed-band-BEFORE-correctness path. The correctness-FAILURE path
/// ([`official_failed_with_timing_and_case`], where correctness DID run and Swift populates
/// these fields from the real report) is intentionally left unchanged.
fn official_failed_timed_band(
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    commit: &str,
    error: String,
    timing: &TimingResult,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
) -> ScorePayload {
    let mut metrics = base_metrics(Mode::Official, golden, digests);
    apply_timing_metrics(
        &mut metrics,
        timing,
        baseline_prefill_spt,
        baseline_decode_spt,
    );
    metrics.passed_correctness = false;
    metrics.commit = commit.to_string();
    // #134 — SEAL BOUNDARY (see `iterate::failed_payload`). Official is the MOST exposed sink:
    // its score.json travels, and worker stderr is never forwarded here, so this scrub is the
    // only thing between engine-controlled bytes and the artifact.
    metrics.error = scrub_reason_for_seal(&error);
    // Align to Swift: correctness never ran at the timed band, so the correctness-derived
    // audit fields carry Swift's `correctness == nil` defaults.
    metrics.golden_hash = String::new();
    metrics.case_count = 0;
    metrics.checked_steps = 0;
    ScorePayload {
        score: None,
        passed: false,
        metrics,
    }
}

/// A FAILED official payload for a benchmark-ORACLE token mismatch — a TIMED-phase failure
/// that Swift's timed-first official path hits BEFORE correctness runs, so it returns via
/// `makeFailedScore(correctness: correctnessReport /* still nil */)`
/// (QwenRuntimeBenchmark.swift:668-676 → failedScore :1143-1158).
///
/// Aligns to Swift on BOTH sides of the RULING-2 principle (producer matches Swift):
/// - BLANK the correctness-derived audit fields: `golden_hash = ""`, `case_count = 0`,
///   `checked_steps = 0` (`correctness?.… ?? default`).
/// - RETAIN the resolved baselines: `baseline_{prefill,decode}_seconds_per_token` carry the
///   golden/paired baseline values (resolved at :434-435, BEFORE the timed phase, and passed
///   straight into `failedScore`). The MEASURED decode/prefill spt stay 0 (the timed phase
///   never completed a trustworthy measurement), so `apply_timing_metrics` is NOT used here.
///
/// `first_failing_case` is always "benchmark"; `first_failing_step` is the decode-token step
/// (or None for the prefill/seed classes); expected/actual tokens are ALWAYS null (Swift
/// makeFailedScore).
fn official_failed_timed_oracle(
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    commit: &str,
    error: String,
    first_failing_step: Option<i64>,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
) -> ScorePayload {
    let mut metrics = base_metrics(Mode::Official, golden, digests);
    metrics.passed_correctness = false;
    metrics.commit = commit.to_string();
    // #134 — SEAL BOUNDARY (see `iterate::failed_payload`). Official is the MOST exposed sink:
    // its score.json travels, and worker stderr is never forwarded here, so this scrub is the
    // only thing between engine-controlled bytes and the artifact.
    metrics.error = scrub_reason_for_seal(&error);
    // Correctness never ran (timed-first): blank the correctness audit surface to Swift's
    // `correctness == nil` defaults.
    metrics.golden_hash = String::new();
    metrics.case_count = 0;
    metrics.checked_steps = 0;
    // Retain the resolved baselines (Swift keeps them; only the MEASURED spt stay 0).
    metrics.baseline_prefill_seconds_per_token = finite_nonneg(baseline_prefill_spt);
    metrics.baseline_decode_seconds_per_token = finite_nonneg(baseline_decode_spt);
    metrics.first_failing_case = Some("benchmark".to_string());
    metrics.first_failing_step = first_failing_step;
    metrics.expected_token = None;
    metrics.actual_token = None;
    ScorePayload {
        score: None,
        passed: false,
        metrics,
    }
}

/// As [`official_failed_with_timing`], plus the correctness `first_failing_*` fields for a
/// correctness-gate failure.
#[allow(clippy::too_many_arguments)]
fn official_failed_with_timing_and_case(
    golden: &GoldenFixture,
    digests: RunDigests<'_>,
    commit: &str,
    error: String,
    timing: &TimingResult,
    baseline_prefill_spt: f64,
    baseline_decode_spt: f64,
    first_failing_case: Option<String>,
    first_failing_step: Option<i64>,
    expected_token: Option<i64>,
    actual_token: Option<i64>,
    checked_steps: i64,
) -> ScorePayload {
    let mut metrics = base_metrics(Mode::Official, golden, digests);
    apply_timing_metrics(
        &mut metrics,
        timing,
        baseline_prefill_spt,
        baseline_decode_spt,
    );
    // A floor/band failure means correctness never ran (Swift returns before it); a
    // correctness failure means it ran and failed. Either way passed_correctness = false.
    metrics.passed_correctness = false;
    metrics.commit = commit.to_string();
    // #134 — SEAL BOUNDARY (see `iterate::failed_payload`). Official is the MOST exposed sink:
    // its score.json travels, and worker stderr is never forwarded here, so this scrub is the
    // only thing between engine-controlled bytes and the artifact.
    metrics.error = scrub_reason_for_seal(&error);
    // caseCount stays the golden TOTAL (Swift correctness-fail `caseCount =
    // totalCorrectnessCaseCount`); checked_steps is the caller-supplied real per-case sum
    // (partial through the failing gate) or, for report-less timed faults, the placeholder.
    metrics.case_count = golden.total_correctness_case_count() as i64;
    metrics.checked_steps = checked_steps;
    metrics.first_failing_case = first_failing_case;
    metrics.first_failing_step = first_failing_step;
    metrics.expected_token = expected_token;
    metrics.actual_token = actual_token;
    ScorePayload {
        score: None,
        passed: false,
        metrics,
    }
}

/// Extract the official correctness failure branding from a conformance report, using the
/// Swift per-gate message (QwenRuntimeCorrectness.swift): base → "teacher-forced token
/// mismatch", anchor → "anchor token mismatch", free-run → "free-run token mismatch". Runs
/// in the layered order Swift evaluates (base → anchors → free_run).
fn official_correctness_failure(
    report: &ConformanceReport,
) -> (Option<String>, Option<i64>, String) {
    // Reuse the shared first-failure walk (base → anchors → free_run) for case/step, then
    // attach the Swift per-gate message keyed on which vector produced it. Expected/actual
    // tokens are NOT returned: the OFFICIAL correctness-fail path leaves them null (see the
    // call site). first_failing_step follows Swift `correctness.firstFailingStep`.
    if let Some(f) = first_conformance_failure(report) {
        let is_anchor = report.anchors.iter().any(|a| a.name == f.case && !a.passed);
        let error = if f.is_base_case {
            "teacher-forced token mismatch".to_string()
        } else if is_anchor {
            "anchor token mismatch".to_string()
        } else {
            "free-run token mismatch".to_string()
        };
        // Swift `compareAnchorToken` reports firstFailingStep = 0 on an anchor fail
        // (QwenRuntimeCorrectnessCompare.swift:481) — NOT nil. `first_conformance_failure`
        // returns None for anchors (an anchor has no per-token step index), so override to 0
        // here to byte-match Swift's official anchor-fail shape. Base and free-run keep their
        // mismatch step (Swift base/free-run firstFailingStep = comparison step).
        let step = if is_anchor { Some(0) } else { f.step };
        (Some(f.case), step, error)
    } else {
        (None, None, "correctness gate failed".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iterate::{DirDigest, HarnessIdentity};

    use bench_core::constants::{
        BENCHMARK_DECODE_SEED_TOKENS, BENCHMARK_DECODE_STEPS, BENCHMARK_PREFILL_PROMPT_TOKENS,
        CORRECTNESS_PROMPT_TOKENS, OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
        OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
    };
    use bench_core::golden::load_golden_fixture;
    use bench_runner::mock::MockEngine;
    use serde_json::json;

    const PREFILL_TOKEN: i64 = 5;
    const SEED_TOKEN: i64 = 6;

    /// Distinct oracle decode tokens so a corrupted-oracle test can target one step.
    fn oracle_decode_tokens() -> Vec<i64> {
        (0..BENCHMARK_DECODE_STEPS as i64)
            .map(|i| 700 + i)
            .collect()
    }

    /// A golden whose benchmark oracle is (5, 6, [700..828)) and whose primary case is
    /// conformant to teacher-forced [2; 64]. `gates` is spliced into `correctness_gates`.
    fn official_golden(gates: Option<serde_json::Value>) -> GoldenFixture {
        let mut doc = json!({
            "version": 1,
            "model_type": "gemma4_text",
            "cases": [
                { "name": "case-a", "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS], "expected_tokens": vec![2i64; 64] }
            ],
            "benchmark": {
                "prefill_prompt_tokens": vec![1i64; BENCHMARK_PREFILL_PROMPT_TOKENS],
                "expected_prefill_token": PREFILL_TOKEN,
                "decode_seed_tokens": vec![1i64; BENCHMARK_DECODE_SEED_TOKENS],
                "expected_decode_seed_token": SEED_TOKEN,
                "expected_decode_tokens": oracle_decode_tokens(),
            }
        });
        if let Some(g) = gates {
            doc["correctness_gates"] = g;
        }
        let bytes = serde_json::to_vec(&doc).unwrap();
        load_golden_fixture(
            &bytes,
            64,
            CORRECTNESS_PROMPT_TOKENS,
            Some("gemma4_text"),
            None,
            None,
        )
        .unwrap()
    }

    /// #134 — the OFFICIAL `score.json` `metrics.error` SINK. This is the most exposed of the
    /// three seal boundaries: official worker stderr is never forwarded (so the retained tail is
    /// its only channel) and the artifact travels. Secret-SHAPED without any `expected`/`actual`
    /// trigger word, so the pre-existing keyword filter would pass every byte through.
    #[test]
    fn official_failed_scrubs_engine_text_before_sealing_metrics_error() {
        let payload = official_failed(
            &official_golden(None),
            RunDigests::for_test(&DirDigest::empty()),
            "commit",
            format!(
                "protocol violation: engine closed the stream before returning a response (worker \
                 exited with status 9; worker stderr tail: open \
                 /Users/operator/pool-goldens/sample-001.json failed | \
                 AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCY | host=api.example.internal | \
                 {})",
                "P".repeat(8192)
            ),
            false,
            None,
            None,
            None,
            None,
            (
                OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
                OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            ),
        );
        let sealed = payload.metrics.error;

        for secret in [
            "/Users/operator/pool-goldens",
            "wJalrXUtnFEMIK7MDENGbPxRfiCY",
            "api.example.internal",
        ] {
            assert!(
                !sealed.contains(secret),
                "secret-tier content sealed into official metrics.error: {secret:?}"
            );
        }
        assert!(
            sealed.len() <= bench_runner::SEALED_REASON_BYTE_LIMIT,
            "sealed metrics.error not capped: {} bytes",
            sealed.len()
        );
        assert!(
            sealed.starts_with("protocol violation: engine closed the stream"),
            "signature lost: {sealed}"
        );
        assert!(
            sealed.contains("sample-001.json"),
            "diagnosis lost: {sealed}"
        );
    }

    /// A stub engine conformant on BOTH the timed oracle and the teacher-forced base case.
    fn conformant_engine() -> MockEngine {
        MockEngine::new()
            .teacher_forced_tokens(vec![2i64; 64])
            .oracle_tokens(PREFILL_TOKEN, SEED_TOKEN, oracle_decode_tokens())
    }

    /// Run the full official_core against `golden` with `timed`/`correctness` engine
    /// factories and the official Qwen baselines. NOTE: a MockEngine's ~0 wall-clock cannot
    /// sit inside the acceptance band, so a conformant mock FAILS on the band — used to prove
    /// the timed-first orchestration + oracle passed and the band gate is wired. The passing
    /// SCORE / correctness-scope assertions go through `finish_official` with a synthetic
    /// in-band TimingResult (see below).
    fn run_official<FT, FC>(golden: &GoldenFixture, timed: FT, correctness: FC) -> ScorePayload
    where
        FT: Fn() -> MockEngine,
        FC: Fn() -> MockEngine,
    {
        official_core(
            golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            || Session::connect(timed()).map(|(s, _)| s),
            || Session::connect(correctness()).map(|(s, _)| s),
        )
    }

    /// A synthetic TimingResult sitting EXACTLY on the baselines (speedups 1.0, in-band,
    /// floors pass, score 1.0) — the only way to drive the passing gate deterministically.
    fn in_band_timing() -> TimingResult {
        TimingResult {
            prefill_seconds_per_token: OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            decode_seconds_per_token: OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            decode_steps: BENCHMARK_DECODE_STEPS,
            prefill_prompt_tokens: BENCHMARK_PREFILL_PROMPT_TOKENS,
            prefill_elapsed_seconds: OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN * 512.0,
            decode_elapsed_seconds: OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN
                * BENCHMARK_DECODE_STEPS as f64,
            peak_ram_gb: 20.25,
            effective_spec: None,
        }
    }

    /// Drive `finish_official` (gating → correctness → assembly) with an in-band timing and a
    /// correctness engine — deterministic, no wall-clock dependence.
    fn finish_with<FC>(golden: &GoldenFixture, correctness: FC) -> ScorePayload
    where
        FC: Fn() -> MockEngine,
    {
        finish_official(
            golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            &in_band_timing(),
            || Session::connect(correctness()).map(|(s, _)| s),
        )
    }

    /// An official golden that DECLARES a baseline pair — the fixture that can tell the
    /// reference's resolution chain apart from the constants fallback (#132/F-2).
    fn official_golden_with_baselines(prefill: f64, decode: f64) -> GoldenFixture {
        let mut doc = json!({
            "version": 1,
            "model_type": "gemma4_text",
            "cases": [
                { "name": "case-a", "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS], "expected_tokens": vec![2i64; 64] }
            ],
            "benchmark": {
                "prefill_prompt_tokens": vec![1i64; BENCHMARK_PREFILL_PROMPT_TOKENS],
                "expected_prefill_token": PREFILL_TOKEN,
                "decode_seed_tokens": vec![1i64; BENCHMARK_DECODE_SEED_TOKENS],
                "expected_decode_seed_token": SEED_TOKEN,
                "expected_decode_tokens": oracle_decode_tokens(),
            }
        });
        doc["benchmark"]["baseline_prefill_seconds_per_token"] = json!(prefill);
        doc["benchmark"]["baseline_decode_seconds_per_token"] = json!(decode);
        let bytes = serde_json::to_vec(&doc).unwrap();
        load_golden_fixture(
            &bytes,
            64,
            CORRECTNESS_PROMPT_TOKENS,
            Some("gemma4_text"),
            None,
            None,
        )
        .unwrap()
    }

    /// Drive `official_gates_only` (correctness-only, no timed phase) with a correctness engine.
    fn gates_only_with<FC>(golden: &GoldenFixture, correctness: FC) -> ScorePayload
    where
        FC: Fn() -> MockEngine,
    {
        official_gates_only(
            golden,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            || Session::connect(correctness()).map(|(s, _)| s),
        )
    }

    #[test]
    fn official_gates_only_conformant_is_partial_result_true_null_score() {
        // SKIP_TIMED gates-only (seam 1): a conformant correctness run seals passed=true,
        // partial_result=true, passed_correctness=true, error empty, NULL score, and the timing
        // placeholders stay zero (the paired overlay owns them).
        let golden = official_golden(None);
        let payload = gates_only_with(&golden, conformant_engine);
        assert!(payload.passed, "error={}", payload.metrics.error);
        assert!(
            payload.metrics.partial_result,
            "gates score must be partial"
        );
        assert!(payload.metrics.passed_correctness);
        assert!(payload.metrics.error.is_empty());
        assert!(payload.score.is_none(), "gates-only score must be null");
        assert_eq!(payload.metrics.commit, "deadbeef");
        assert_eq!(payload.metrics.runtime, "rust");
        // No timed phase ran: the MEASURED timing surface stays at its zero placeholders.
        assert_eq!(payload.metrics.decode_seconds_per_token, 0.0);
        assert_eq!(payload.metrics.prefill_seconds_per_token, 0.0);
        assert_eq!(payload.metrics.decode_speedup, 0.0);
        assert_eq!(payload.metrics.prefill_speedup, 0.0);
        // #132/F-2 — the BASELINE pair is NOT part of that zero surface, and its absence from
        // this enumeration is how a whole class of change stayed green: the reference resolves it
        // (`pairedBaseline ?? golden.resolvedBaseline*`) BEFORE the gates-only branch, so it is
        // never a placeholder. This golden declares no pair, so the resolution lands on the
        // constants; `official_gates_only_seals_the_goldens_declared_baselines` covers the arm
        // where the golden DOES declare one, which is what tells the chain apart.
        assert_eq!(
            payload.metrics.baseline_prefill_seconds_per_token,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN
        );
        assert_eq!(
            payload.metrics.baseline_decode_seconds_per_token,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN
        );
        assert_ne!(payload.metrics.baseline_prefill_seconds_per_token, 0.0);
        assert_ne!(payload.metrics.baseline_decode_seconds_per_token, 0.0);
        // Correctness DID run: full case count + real checked-step sum.
        assert_eq!(
            payload.metrics.case_count,
            golden.total_correctness_case_count() as i64
        );
        assert_eq!(
            payload.metrics.checked_steps,
            bench_core::constants::CORRECTNESS_STEPS as i64
        );
    }

    /// #132/F-2 — the official path resolves its baseline pair the REFERENCE's way, on all three
    /// payloads that no later step overwrites.
    ///
    /// The reference overwrites its baseline locals with
    /// `pairedBaseline ?? benchmarkGolden.resolvedBaseline*`
    /// (`QwenRuntimeBenchmark.swift@b26f76f:442-445`) BEFORE the `skipTimedBenchmark` branch at
    /// `:457` and before every `makeFailedScore` in `benchmarkWithWorker`. So the golden's declared
    /// pair — not the constants, and not zero — is what these records carry. A golden declaring a
    /// DISTINCT pair is the only fixture that can tell those three answers apart.
    #[test]
    fn official_gates_only_seals_the_goldens_declared_baselines() {
        const DECLARED_PREFILL: f64 = 0.000123456;
        const DECLARED_DECODE: f64 = 0.00987654;
        // Distinct from BOTH wrong answers, so the assertions below are not satisfiable by
        // accident.
        assert_ne!(
            DECLARED_PREFILL,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN
        );
        assert_ne!(DECLARED_DECODE, OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN);

        let golden = official_golden_with_baselines(DECLARED_PREFILL, DECLARED_DECODE);

        // 1. the PASSING gates-only partial.
        let passing = gates_only_with(&golden, conformant_engine);
        assert!(passing.passed, "error={}", passing.metrics.error);
        assert!(passing.metrics.partial_result);
        assert_eq!(
            passing.metrics.baseline_prefill_seconds_per_token, DECLARED_PREFILL,
            "gates-only partial must seal the GOLDEN's pair, not the constants"
        );
        assert_eq!(
            passing.metrics.baseline_decode_seconds_per_token,
            DECLARED_DECODE
        );

        // 2. the gates-only FAILURE record.
        let failing = gates_only_with(&golden, || {
            MockEngine::new().error_on("correctness_step", "boom")
        });
        assert!(!failing.passed);
        assert!(failing.metrics.partial_result);
        assert_eq!(
            failing.metrics.baseline_prefill_seconds_per_token, DECLARED_PREFILL,
            "gates-only failure must seal the GOLDEN's pair too — the reference's failure records \
             are all reached after the :442-445 overwrite"
        );
        assert_eq!(
            failing.metrics.baseline_decode_seconds_per_token,
            DECLARED_DECODE
        );
    }

    /// An official golden with NO benchmark oracle — the fixture that reaches `official_core`'s
    /// oracle-less branch (`official.rs:192`) directly.
    fn official_golden_without_oracle() -> GoldenFixture {
        let doc = json!({
            "version": 1,
            "model_type": "gemma4_text",
            "cases": [
                { "name": "case-a", "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS], "expected_tokens": vec![2i64; 64] }
            ]
        });
        let bytes = serde_json::to_vec(&doc).unwrap();
        load_golden_fixture(
            &bytes,
            64,
            CORRECTNESS_PROMPT_TOKENS,
            Some("gemma4_text"),
            None,
            None,
        )
        .unwrap()
    }

    /// #132/F-8 — `official_failed` seals the pair `official_core` RESOLVED (env ?? `--baseline-*`
    /// flags ?? golden), on BOTH branches that reach it.
    ///
    /// **This replaces a test that never reached the code it claimed to cover.** The previous
    /// version drove `finish_official` with an oracle-BEARING golden, so the oracle-less branch at
    /// `:192` was never taken and the failing spawner routed to `official_failed_timed_oracle`
    /// (`:760`) — which already threaded the pair before this PR. It pinned pre-existing behavior:
    /// reverting `official_failed` to re-derive from the golden left the whole suite green.
    ///
    /// Each arm below carries a DISTINGUISHING assertion on the error string that only that branch
    /// can produce, so neither can silently start passing through some other exit.
    #[test]
    fn official_failed_seals_the_resolved_pair_on_both_branches_that_reach_it() {
        const RESOLVED_PREFILL: f64 = 0.000222222;
        const RESOLVED_DECODE: f64 = 0.00333333;
        const DECLARED_PREFILL: f64 = 0.000123456;
        const DECLARED_DECODE: f64 = 0.00987654;
        // All three candidate answers must be mutually distinct or the assertions prove nothing.
        assert_ne!(RESOLVED_PREFILL, DECLARED_PREFILL);
        assert_ne!(
            RESOLVED_PREFILL,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN
        );
        assert_ne!(RESOLVED_DECODE, DECLARED_DECODE);
        assert_ne!(RESOLVED_DECODE, OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN);

        let conformant = || Session::connect(conformant_engine()).map(|(s, _)| s);

        // --- branch 1: official.rs:192, the ORACLE-LESS golden -------------------------------
        // Nothing spawns on this path; the golden declares no pair, so a re-derive would land on
        // the CONSTANTS and the resolved pair is what tells them apart.
        let no_oracle = official_golden_without_oracle();
        let a = official_core(
            &no_oracle,
            RESOLVED_PREFILL,
            RESOLVED_DECODE,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            || Session::connect(conformant_engine()).map(|(s, _)| s),
            conformant,
        );
        assert!(!a.passed);
        assert_eq!(
            a.metrics.error, "benchmark golden file must contain a benchmark oracle",
            "branch :192 signature — reached a different exit"
        );
        assert_eq!(
            a.metrics.baseline_prefill_seconds_per_token, RESOLVED_PREFILL,
            ":192 must seal the RESOLVED pair; the constants here would mean official_failed \
             re-derived it and dropped the --baseline-* flags"
        );
        assert_eq!(a.metrics.baseline_decode_seconds_per_token, RESOLVED_DECODE);
        assert_ne!(
            a.metrics.baseline_prefill_seconds_per_token,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN
        );

        // --- branch 2: official.rs:266, a TIMED failure on an oracle-bearing golden -----------
        // The golden DECLARES a pair here, so all three answers are distinguishable at once.
        let with_oracle = official_golden_with_baselines(DECLARED_PREFILL, DECLARED_DECODE);
        let b = official_core(
            &with_oracle,
            RESOLVED_PREFILL,
            RESOLVED_DECODE,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            || -> bench_runner::Result<Session<MockEngine>> {
                Err(RunnerError::Protocol(
                    "timed worker spawn failed".to_string(),
                ))
            },
            conformant,
        );
        assert!(!b.passed);
        assert!(
            b.metrics.error.contains("timed worker spawn failed"),
            "branch :266 signature — reached a different exit: {:?}",
            b.metrics.error
        );
        assert_eq!(
            b.metrics.baseline_prefill_seconds_per_token, RESOLVED_PREFILL,
            ":266 must seal the RESOLVED pair, not the golden's declaration"
        );
        assert_eq!(b.metrics.baseline_decode_seconds_per_token, RESOLVED_DECODE);
        assert_ne!(
            b.metrics.baseline_prefill_seconds_per_token,
            DECLARED_PREFILL
        );
        assert_ne!(
            b.metrics.baseline_prefill_seconds_per_token,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN
        );
    }

    #[test]
    fn official_gates_only_correctness_fail_is_failed_but_partial() {
        // A correctness mismatch in gates-only mode fails closed: passed=false,
        // passed_correctness=false, null score — but still the gates-only shape (partial_result
        // stays true; a failing gates-score never reaches the overlay).
        let golden = official_golden(None);
        let bad_correctness = || {
            MockEngine::new()
                .teacher_forced_tokens(vec![999i64; 64])
                .oracle_tokens(PREFILL_TOKEN, SEED_TOKEN, oracle_decode_tokens())
        };
        let payload = gates_only_with(&golden, bad_correctness);
        assert!(!payload.passed);
        assert!(!payload.metrics.passed_correctness);
        assert!(payload.metrics.partial_result);
        assert!(payload.score.is_none());
        assert!(!payload.metrics.error.is_empty());
    }

    #[test]
    fn official_conformant_run_passes_with_score_and_commit() {
        // In-band timing (speedups 1.0) + conformant correctness ⇒ PASS, score ~1.0, commit
        // stamped, runtime = "rust".
        let golden = official_golden(None);
        let payload = finish_with(&golden, conformant_engine);
        assert!(payload.passed, "error={}", payload.metrics.error);
        assert!(payload.metrics.passed_correctness);
        let score = payload.score.expect("passing official run has a score");
        assert!((score - 1.0).abs() < 1e-9, "score={score}");
        assert_eq!(payload.metrics.commit, "deadbeef");
        assert_eq!(payload.metrics.runtime, "rust");
        // case_count = full correctness total (checkGates == true), not the timing repeats.
        assert_eq!(
            payload.metrics.case_count,
            golden.total_correctness_case_count() as i64
        );
        // checked_steps is now the REAL per-case sum (one base case, no gates → the full
        // teacher-forced window of 64), NOT the placeholder case count (1).
        assert_eq!(
            payload.metrics.checked_steps,
            bench_core::constants::CORRECTNESS_STEPS as i64
        );
        assert_ne!(
            payload.metrics.checked_steps,
            golden.total_correctness_case_count() as i64,
            "checked_steps must be the real step sum, not the placeholder case count"
        );
    }

    /// A golden with a base case + 2 anchors + 1 free-run case (4 declared correctness cases —
    /// the same count that used to be the checked_steps PLACEHOLDER). The conformant per-case
    /// checked-step SUM is 64 (base window) + 1 + 1 (anchors) + 5 (free-run prefix) = 71.
    fn full_gates_golden(free_run_expected: Vec<i64>) -> GoldenFixture {
        official_golden(Some(json!({
            "anchors": [
                { "name": "anc-1", "context_tokens": vec![1i64; 8], "expected_token": 7, "accepted_tokens": [7] },
                { "name": "anc-2", "context_tokens": vec![1i64; 8], "expected_token": 9, "accepted_tokens": [9] }
            ],
            "free_run": [
                { "name": "fr-1", "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS], "expected_tokens": free_run_expected }
            ]
        })))
    }

    /// Conformant on base [2;64], anchors (argmax 7 then 9 via per-sequence teacher forcing),
    /// and the free-run stream (the mock's fixed `correctness` tokens 4000,4001,4002,…).
    fn full_gates_conformant_engine() -> MockEngine {
        MockEngine::new()
            .teacher_forced_sequences(vec![vec![2i64; 64], vec![7], vec![9]])
            .oracle_tokens(PREFILL_TOKEN, SEED_TOKEN, oracle_decode_tokens())
    }

    #[test]
    fn official_passing_checked_steps_is_real_per_case_sum() {
        // Leg-1 PASSING parity: a fully-conformant official run reports the REAL checked-step
        // SUM (64 + 1 + 1 + 5 = 71), NOT the placeholder total_correctness_case_count (4).
        let golden = full_gates_golden(vec![4000, 4001, 4002, 4003, 4004]);
        assert_eq!(
            golden.total_correctness_case_count(),
            4,
            "the old placeholder value"
        );
        let payload = finish_with(&golden, full_gates_conformant_engine);
        assert!(payload.passed, "error={}", payload.metrics.error);
        assert!(payload.metrics.passed_correctness);
        assert_eq!(
            payload.metrics.checked_steps, 71,
            "64 base + 1 + 1 anchors + 5 free-run"
        );
        assert_eq!(
            payload.metrics.case_count, 4,
            "caseCount stays the declared total"
        );
    }

    #[test]
    fn official_anchor_fail_partial_checked_steps_and_step_zero() {
        // Anchor-gate failure: base passes (64), anc-1 fails (+1) → partial sum 65. Swift also
        // reports first_failing_step = 0 (compareAnchorToken) and NULL expected/actual tokens.
        let golden = full_gates_golden(vec![4000, 4001, 4002, 4003, 4004]);
        // Corrupt anc-1: engine argmax 8 ∉ accepted {7}.
        let engine = || {
            MockEngine::new()
                .teacher_forced_sequences(vec![vec![2i64; 64], vec![8], vec![9]])
                .oracle_tokens(PREFILL_TOKEN, SEED_TOKEN, oracle_decode_tokens())
        };
        let payload = finish_with(&golden, engine);
        assert!(!payload.passed);
        assert_eq!(payload.metrics.error, "anchor token mismatch");
        assert_eq!(payload.metrics.first_failing_case.as_deref(), Some("anc-1"));
        assert_eq!(
            payload.metrics.checked_steps, 65,
            "64 base + 1 failing anchor"
        );
        assert_eq!(
            payload.metrics.first_failing_step,
            Some(0),
            "Swift anchor firstFailingStep=0"
        );
        assert_eq!(
            payload.metrics.expected_token, None,
            "official nulls expected_token"
        );
        assert_eq!(
            payload.metrics.actual_token, None,
            "official nulls actual_token"
        );
        // case_count + golden_hash stay populated (correctness DID run and fail).
        assert_eq!(payload.metrics.case_count, 4);
        assert_eq!(payload.metrics.golden_hash, golden.sha256);
    }

    #[test]
    fn official_free_run_fail_partial_checked_steps() {
        // Free-run-gate failure: base(64) + anc-1(1) + anc-2(1) + free-run fails at step 2 (+3)
        // → partial sum 69. Free-run stream is 4000,4001,4002,…; expected diverges at index 2.
        let golden = full_gates_golden(vec![4000, 4001, 999, 4003, 4004]);
        let payload = finish_with(&golden, full_gates_conformant_engine);
        assert!(!payload.passed);
        assert_eq!(payload.metrics.error, "free-run token mismatch");
        assert_eq!(payload.metrics.first_failing_case.as_deref(), Some("fr-1"));
        assert_eq!(
            payload.metrics.checked_steps, 69,
            "64 + 1 + 1 + 3 (free-run step 2 + 1)"
        );
        assert_eq!(
            payload.metrics.first_failing_step,
            Some(2),
            "free-run mismatch step"
        );
        assert_eq!(payload.metrics.expected_token, None);
        assert_eq!(payload.metrics.actual_token, None);
    }

    #[test]
    fn official_primary_correctness_fail_nulls_tokens() {
        // Primary (teacher-forced) correctness failure: Swift leaves expected_token/actual_token
        // NULL on the OFFICIAL path (unlike LOCAL). checked_steps = the partial base sum (fail
        // at step 0 → 1).
        let golden = official_golden(None);
        let engine = || {
            MockEngine::new()
                .teacher_forced_tokens(vec![3i64; 64]) // diverges from golden [2;64] at step 0
                .oracle_tokens(PREFILL_TOKEN, SEED_TOKEN, oracle_decode_tokens())
        };
        let payload = finish_with(&golden, engine);
        assert!(!payload.passed);
        assert_eq!(payload.metrics.error, "teacher-forced token mismatch");
        assert_eq!(
            payload.metrics.first_failing_case.as_deref(),
            Some("case-a")
        );
        assert_eq!(payload.metrics.first_failing_step, Some(0));
        assert_eq!(
            payload.metrics.expected_token, None,
            "official nulls expected_token"
        );
        assert_eq!(
            payload.metrics.actual_token, None,
            "official nulls actual_token"
        );
        assert_eq!(
            payload.metrics.checked_steps, 1,
            "fail at base step 0 → checkedSteps 1"
        );
    }

    #[test]
    fn official_timed_first_orchestration_reaches_band_gate() {
        // The full official_core with a CONFORMANT-ORACLE mock: the timed phases pass the
        // oracle (no TokenMismatch), and gating then FAILS on the acceptance band because a
        // mock's ~0 wall-clock is ~1000x faster than the baseline. This proves the timed-first
        // orchestration ran and the band gate is wired ahead of correctness.
        let golden = official_golden(None);
        let payload = run_official(&golden, conformant_engine, conformant_engine);
        assert!(!payload.passed);
        assert!(
            payload.metrics.error.starts_with("acceptance band failed:")
                || payload
                    .metrics
                    .error
                    .starts_with("performance floor failed:"),
            "expected a band/floor failure (not an oracle mismatch), got: {}",
            payload.metrics.error
        );
    }

    #[test]
    fn official_full_scope_evaluates_anchor_gate_unlike_local() {
        // GATE-SCOPE FLIP: official runs the FULL set. A golden with a CORRUPTED anchor
        // (engine argmax ≠ 999) that the LOCAL default would SKIP must FAIL official
        // correctness (reached here via in-band timing so the band does not mask it).
        let golden = official_golden(Some(json!({
            "anchors": [
                { "name": "bad-anchor", "context_tokens": vec![1i64; 8], "expected_token": 999, "accepted_tokens": [999] }
            ]
        })));
        let payload = finish_with(&golden, conformant_engine);
        assert!(
            !payload.passed,
            "official evaluates the full correctness set"
        );
        assert!(payload.score.is_none());
        assert!(!payload.metrics.passed_correctness);
        assert_eq!(payload.metrics.error, "anchor token mismatch");
        assert_eq!(
            payload.metrics.first_failing_case.as_deref(),
            Some("bad-anchor")
        );
        // Swift anchor firstFailingStep = 0 (compareAnchorToken), tokens null on the official path.
        assert_eq!(payload.metrics.first_failing_step, Some(0));
        assert_eq!(payload.metrics.expected_token, None);
        assert_eq!(payload.metrics.actual_token, None);
        // Partial checked-step sum: base window 64 + failing anchor 1 = 65.
        assert_eq!(payload.metrics.checked_steps, 65);
    }

    #[test]
    fn corrupted_benchmark_oracle_fails_official() {
        // THE failure class the local path cannot test: the golden's benchmark oracle says
        // decode step 3 → token 703, but the engine emits 999_999. Official's parent-side
        // oracle check FAILS the run (BenchmarkTokenMismatchError), branded case="benchmark".
        let golden = official_golden(None);
        let mut engine_tokens = oracle_decode_tokens();
        engine_tokens[3] = 999_999;
        let payload = run_official(
            &golden,
            move || {
                MockEngine::new()
                    .teacher_forced_tokens(vec![2i64; 64])
                    .oracle_tokens(PREFILL_TOKEN, SEED_TOKEN, engine_tokens.clone())
            },
            conformant_engine,
        );
        assert!(!payload.passed);
        assert!(payload.score.is_none());
        // Swift BenchmarkTokenMismatchError.description for the decode-TOKEN class KEEPS the
        // step suffix (compareDecodeTokens carries step). makeFailedScore nulls the tokens.
        assert_eq!(
            payload.metrics.error, "benchmark decode token mismatch at step 3",
            "Swift BenchmarkTokenMismatchError.description (decode-token class keeps the step)"
        );
        assert_eq!(
            payload.metrics.first_failing_case.as_deref(),
            Some("benchmark")
        );
        assert_eq!(payload.metrics.first_failing_step, Some(3));
        // MAJOR-1: Swift makeFailedScore sets expectedToken/actualToken = nil (ALWAYS).
        assert_eq!(payload.metrics.expected_token, None);
        assert_eq!(payload.metrics.actual_token, None);
        // ITEM B (this change): the oracle mismatch is a TIMED-phase failure BEFORE correctness,
        // so Swift returns via makeFailedScore(correctness: nil) — BLANK the correctness audit
        // fields but RETAIN the resolved baselines.
        assert_eq!(
            payload.metrics.golden_hash, "",
            "golden_hash blanked (correctness nil)"
        );
        assert_eq!(
            payload.metrics.case_count, 0,
            "case_count blanked (correctness nil)"
        );
        assert_eq!(
            payload.metrics.checked_steps, 0,
            "checked_steps blanked (correctness nil)"
        );
        assert_eq!(
            payload.metrics.baseline_decode_seconds_per_token,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            "resolved decode baseline RETAINED"
        );
        assert_eq!(
            payload.metrics.baseline_prefill_seconds_per_token,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            "resolved prefill baseline RETAINED"
        );
        // The MEASURED spt stay 0 (the timed phase never completed a trustworthy measurement).
        assert_eq!(payload.metrics.decode_seconds_per_token, 0.0);
        assert_eq!(payload.metrics.prefill_seconds_per_token, 0.0);
    }

    #[test]
    fn corrupted_prefill_oracle_fails_official_no_step_suffix() {
        // The PREFILL oracle class: Swift compareOne carries step:nil, so the description has
        // NO " at step N" suffix and firstFailingStep = nil; tokens are nulled.
        let golden = official_golden(None);
        let payload = run_official(
            &golden,
            || {
                MockEngine::new()
                    .teacher_forced_tokens(vec![2i64; 64])
                    // Wrong PREFILL token (seed + decode conformant).
                    .oracle_tokens(PREFILL_TOKEN + 1, SEED_TOKEN, oracle_decode_tokens())
            },
            conformant_engine,
        );
        assert!(!payload.passed);
        assert_eq!(
            payload.metrics.error, "benchmark prefill token mismatch",
            "prefill class has NO step suffix (Swift compareOne step:nil)"
        );
        assert_eq!(
            payload.metrics.first_failing_case.as_deref(),
            Some("benchmark")
        );
        assert_eq!(payload.metrics.first_failing_step, None);
        assert_eq!(payload.metrics.expected_token, None);
        assert_eq!(payload.metrics.actual_token, None);
    }

    #[test]
    fn corrupted_seed_oracle_fails_official_no_step_suffix() {
        // The decode-SEED oracle class: same as prefill — step:nil, no suffix, nulled tokens.
        let golden = official_golden(None);
        let payload = run_official(
            &golden,
            || {
                MockEngine::new()
                    .teacher_forced_tokens(vec![2i64; 64])
                    // Wrong SEED token (prefill + decode conformant).
                    .oracle_tokens(PREFILL_TOKEN, SEED_TOKEN + 1, oracle_decode_tokens())
            },
            conformant_engine,
        );
        assert!(!payload.passed);
        assert_eq!(
            payload.metrics.error, "benchmark decode seed token mismatch",
            "seed class has NO step suffix (Swift compareOne step:nil)"
        );
        assert_eq!(payload.metrics.first_failing_step, None);
        assert_eq!(payload.metrics.expected_token, None);
        assert_eq!(payload.metrics.actual_token, None);
    }

    #[test]
    fn benchmark_less_golden_fails_official() {
        let doc = json!({
            "version": 1, "model_type": "gemma4_text",
            "cases": [{ "name": "p1", "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS], "expected_tokens": vec![2i64; 64] }],
        });
        let bytes = serde_json::to_vec(&doc).unwrap();
        let golden = load_golden_fixture(
            &bytes,
            64,
            CORRECTNESS_PROMPT_TOKENS,
            Some("gemma4_text"),
            None,
            None,
        )
        .unwrap();
        let payload = run_official(&golden, conformant_engine, conformant_engine);
        assert!(!payload.passed);
        assert_eq!(
            payload.metrics.error,
            "benchmark golden file must contain a benchmark oracle"
        );
    }

    #[test]
    fn decode_speedup_below_floor_fails_official() {
        // Candidate decode +10% slower than baseline ⇒ speedup ≈ 0.909 < 0.95 floor ⇒ the
        // decode acceptance band (+2% ceiling) trips FIRST in the priority order (finite →
        // floors → bands), failing official BEFORE correctness with the real timing retained.
        let golden = official_golden(None);
        let mut timing = in_band_timing();
        timing.decode_seconds_per_token = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN * 1.10;
        let payload = finish_official(
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            &timing,
            || Session::connect(conformant_engine()).map(|(s, _)| s),
        );
        assert!(!payload.passed);
        assert!(payload.score.is_none());
        assert!(
            payload
                .metrics
                .error
                .starts_with("performance floor failed:")
                || payload.metrics.error.starts_with("acceptance band failed:"),
            "floor/band failure expected, got: {}",
            payload.metrics.error
        );
        // Real timing retained (not blanked): the measured decode spt is carried through.
        assert!(payload.metrics.decode_seconds_per_token > 0.0);
        assert!(!payload.metrics.passed_correctness, "correctness never ran");
    }

    #[test]
    fn official_timed_band_failure_blanks_correctness_audit_fields_to_match_swift() {
        // RULING 2: an OFFICIAL run that fails at the TIMED band (before correctness runs) must
        // byte-match Swift's `correctness == nil` failed score — golden_hash="", case_count=0,
        // checked_steps=0 — because Swift's timed-first path returns via
        // makeFailedScore(correctness: nil) and downstream/organizer tooling expects that shape.
        // Drive a floor/band failure via +10% slower decode (correctness never runs).
        let golden = official_golden(None);
        // Guard: this golden really does carry a non-empty hash and non-zero case count that a
        // populate-path (or the old official_failed_with_timing) WOULD have surfaced — so the
        // blanking below is meaningful, not vacuously matching an already-empty golden.
        assert!(
            !golden.sha256.is_empty(),
            "test golden must have a real hash to blank"
        );
        assert!(
            golden.total_correctness_case_count() > 0,
            "test golden must have >0 correctness cases to blank"
        );

        let mut timing = in_band_timing();
        timing.decode_seconds_per_token = OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN * 1.10;
        let payload = finish_official(
            &golden,
            OFFICIAL_BASELINE_PREFILL_SECONDS_PER_TOKEN,
            OFFICIAL_BASELINE_DECODE_SECONDS_PER_TOKEN,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            &timing,
            || Session::connect(conformant_engine()).map(|(s, _)| s),
        );
        assert!(!payload.passed);
        assert!(payload.score.is_none());
        assert!(
            payload
                .metrics
                .error
                .starts_with("performance floor failed:")
                || payload.metrics.error.starts_with("acceptance band failed:"),
            "timed-band failure expected, got: {}",
            payload.metrics.error
        );
        // The RULING-2 alignment: byte-match Swift's blanked correctness-audit surface.
        assert_eq!(
            payload.metrics.golden_hash, "",
            "golden_hash blanked to match Swift"
        );
        assert_eq!(
            payload.metrics.case_count, 0,
            "case_count blanked to match Swift"
        );
        assert_eq!(
            payload.metrics.checked_steps, 0,
            "checked_steps blanked to match Swift"
        );
        // The REAL timing surface is still retained (only the correctness-audit fields blank).
        assert!(
            payload.metrics.decode_seconds_per_token > 0.0,
            "measured timing retained"
        );
        assert!(
            payload.metrics.prefill_seconds_per_token > 0.0,
            "measured timing retained"
        );
        assert!(
            !payload.metrics.passed_correctness,
            "correctness never ran at the timed band"
        );
    }

    #[test]
    fn official_correctness_failure_still_populates_audit_fields_unchanged() {
        // SCOPE GUARD for RULING 2: the correctness-FAILURE path (correctness DID run and
        // failed) is NOT the timed-band path and must remain UNCHANGED — golden_hash and the
        // case counts stay populated. Reuses the corrupted-anchor golden reached via in-band
        // timing so the band does not mask the correctness failure.
        let golden = official_golden(Some(json!({
            "anchors": [
                { "name": "bad-anchor", "context_tokens": vec![1i64; 8], "expected_token": 999, "accepted_tokens": [999] }
            ]
        })));
        let payload = finish_with(&golden, conformant_engine);
        assert!(!payload.passed);
        assert_eq!(payload.metrics.error, "anchor token mismatch");
        // Correctness-failure path is untouched: golden_hash + case counts stay populated.
        assert_eq!(
            payload.metrics.golden_hash, golden.sha256,
            "correctness-fail path unchanged"
        );
        assert_eq!(
            payload.metrics.case_count,
            golden.total_correctness_case_count() as i64,
            "correctness-fail path unchanged"
        );
        assert!(
            payload.metrics.checked_steps > 0,
            "correctness-fail path unchanged"
        );
    }

    #[test]
    fn behavior_gate_presence_requires_worker() {
        // benchmarkRequiresRuntimeWorker: a behavior case forces the worker path.
        let with_behavior = official_golden(Some(json!({
            "behavior": [
                { "name": "b1", "prompt_tokens": vec![1i64; 8], "accepted_token_sequences": [[1, 2]], "max_new_tokens": 4 }
            ]
        })));
        assert!(benchmark_requires_runtime_worker(&with_behavior));
        // No gates / no behavior ⇒ not required by this predicate.
        assert!(!benchmark_requires_runtime_worker(&official_golden(None)));
    }

    #[test]
    fn paired_baseline_env_fail_closed_semantics() {
        // Both unset ⇒ None (no override).
        assert_eq!(paired_baseline_from_env(None, None).unwrap(), None);
        assert_eq!(
            paired_baseline_from_env(Some("  "), Some("")).unwrap(),
            None
        );
        // Both set + finite positive ⇒ Some (trimmed).
        assert_eq!(
            paired_baseline_from_env(Some(" 0.01 "), Some("0.13")).unwrap(),
            Some((0.01, 0.13))
        );
        // Half-set (either side) ⇒ error "must be provided together".
        let e = paired_baseline_from_env(Some("0.01"), None).unwrap_err();
        assert!(e.contains("must be provided together"), "got: {e}");
        let e = paired_baseline_from_env(None, Some("0.13")).unwrap_err();
        assert!(e.contains("must be provided together"), "got: {e}");
        // Non-finite / non-positive / non-numeric ⇒ error, per-key.
        assert!(paired_baseline_from_env(Some("0"), Some("0.13")).is_err());
        assert!(paired_baseline_from_env(Some("-1"), Some("0.13")).is_err());
        assert!(paired_baseline_from_env(Some("inf"), Some("0.13")).is_err());
        assert!(paired_baseline_from_env(Some("nan"), Some("0.13")).is_err());
        assert!(paired_baseline_from_env(Some("0.01"), Some("cheese")).is_err());
    }

    #[test]
    fn commit_identifier_prefers_valid_env_sha() {
        // 7..=40 lowercase hex from MLXFAST_COMMIT_SHA wins (trimmed).
        assert_eq!(commit_identifier(Some("  a1b2c3d  ")), "a1b2c3d");
        assert_eq!(
            commit_identifier(Some("0123456789abcdef0123456789abcdef01234567")),
            "0123456789abcdef0123456789abcdef01234567"
        );
        // is_commit_sha_hex boundaries.
        assert!(is_commit_sha_hex("abcdef0")); // 7
        assert!(!is_commit_sha_hex("abcde")); // 6 too short
        assert!(!is_commit_sha_hex("A1B2C3D")); // uppercase rejected
        assert!(!is_commit_sha_hex("g1b2c3d")); // non-hex rejected
        assert!(!is_commit_sha_hex(
            "0123456789abcdef0123456789abcdef012345678"
        )); // 41 too long
    }

    // A recorded dispatch sha (candidate.sha) and a DIFFERENT competitor-proposed commit.
    const DISPATCH_RECORD: &str = "0123456789abcdef0123456789abcdef01234567";
    const FOREIGN_COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";
    // Scoring/ranked seal (the default measure-job mode); dev/local is the negation.
    const SCORING: bool = true;
    const DEV: bool = false;

    /// AUTHOR-AT-SEAL revert-proof (DECIDE-3). A seal whose competitor-proposed commit disagrees
    /// with the dispatched-record commit is REJECTED. This is the bind that neutering greens: with
    /// the disagreement branch removed, `author_sealed_commit` returns `Ok(DISPATCH_RECORD)` for
    /// the same inputs (the mismatch is silently accepted), so this assertion fails — the test is
    /// load-bearing on the bind, not vacuous. (A present record binds identically in either mode.)
    #[test]
    fn author_at_seal_rejects_commit_that_disagrees_with_dispatch_record() {
        let refused = author_sealed_commit(Some(DISPATCH_RECORD), Some(FOREIGN_COMMIT), SCORING);
        let msg =
            refused.expect_err("a proposed commit that is not the dispatched sha must refuse");
        assert!(
            msg.contains("disagrees with the dispatched record"),
            "got: {msg}"
        );
        assert!(
            msg.contains(FOREIGN_COMMIT),
            "the refusal names the foreign commit: {msg}"
        );

        // The dispatch record is the SOLE authority: on agreement the sealed commit is AUTHORED
        // from the record, never from the proposal, and a benign short-form proposal is accepted.
        assert_eq!(
            author_sealed_commit(Some(DISPATCH_RECORD), Some(DISPATCH_RECORD), SCORING).unwrap(),
            DISPATCH_RECORD
        );
        assert_eq!(
            author_sealed_commit(Some(DISPATCH_RECORD), Some(&DISPATCH_RECORD[..7]), SCORING)
                .unwrap(),
            DISPATCH_RECORD,
            "a git-short-HEAD prefix of the SAME commit agrees"
        );
        // A valid-hex proposal that is NOT a prefix of the record disagrees. (`"0123abc"` is 7-hex
        // but record[..7] == "0123456" != "0123abc".)
        assert!(
            author_sealed_commit(Some(DISPATCH_RECORD), Some("0123abc"), SCORING).is_err(),
            "a non-prefix hex proposal disagrees"
        );
    }

    /// F1 revert-proof (fail-closed). On a SCORING run an ABSENT dispatch record REFUSES rather than
    /// falling back to the box git identity (which could seal an empty `metrics.commit`). Neutering
    /// the guard (restoring the git fallback on the scoring path) makes this `Ok(_)` and greens.
    #[test]
    fn author_at_seal_scoring_refuses_when_dispatch_record_absent() {
        let refused = author_sealed_commit(None, Some("a1b2c3d"), SCORING);
        let msg = refused.expect_err("a scoring seal with no dispatch record must fail closed");
        assert!(msg.contains("scoring/ranked seal requires"), "got: {msg}");
        // Even with NO proposal (the exact empty-commit shape) the scoring seal refuses.
        assert!(author_sealed_commit(None, None, SCORING).is_err());
    }

    /// The record is authored even with NO proposal, and a malformed/empty record (a dispatch that
    /// promised a sha but delivered junk) refuses rather than silently falling back to git.
    #[test]
    fn author_at_seal_authors_from_record_and_refuses_malformed_record() {
        assert_eq!(
            author_sealed_commit(Some(DISPATCH_RECORD), None, SCORING).unwrap(),
            DISPATCH_RECORD
        );
        // An unset MLXFAST_COMMIT_SHA modelled as an empty string is treated as no proposal.
        assert_eq!(
            author_sealed_commit(Some(DISPATCH_RECORD), Some("   "), SCORING).unwrap(),
            DISPATCH_RECORD
        );
        // Record present but not the strict 40-hex shape ⇒ refuse (matches the trusted shell's
        // `^[0-9a-f]{40}$` predicate before it writes candidate.sha).
        assert!(author_sealed_commit(Some(""), None, SCORING).is_err());
        assert!(author_sealed_commit(Some("not-a-sha"), None, SCORING).is_err());
        let uppercase = "ABCDEF0123456789abcdef0123456789abcdef01"; // 40 chars, not lowercase
        assert!(author_sealed_commit(Some(uppercase), None, SCORING).is_err());
        let short = &DISPATCH_RECORD[..39]; // 39 too short
        assert!(author_sealed_commit(Some(short), None, SCORING).is_err());
    }

    /// No dispatch record ⇒ DEV/local (`--local-dev`) fallback to the pre-existing, un-bound
    /// `commit_identifier`. Scoring mode does NOT fall back (see the fail-closed test above).
    #[test]
    fn author_at_seal_dev_falls_back_when_no_dispatch_record() {
        assert_eq!(
            author_sealed_commit(None, Some("a1b2c3d"), DEV),
            Ok("a1b2c3d".to_string())
        );
    }
    // -----------------------------------------------------------------------
    // F1 — the SEAM-1 harness identity
    // -----------------------------------------------------------------------

    /// F1 MUTATION PROOF (b), at the seam that matters most: the OFFICIAL GATES-ONLY payload — the
    /// seam-1 artifact the overlay consumes — seals the resolved harness identity.
    ///
    /// Reds if `base_metrics` goes back to `harness_hash: String::new()`, or seals anything other
    /// than the identity the run resolved.
    #[test]
    fn official_gates_only_seals_the_resolved_harness_identity() {
        let golden = official_golden(None);
        let payload = gates_only_with(&golden, conformant_engine);
        assert_eq!(
            payload.metrics.harness_hash,
            HarnessIdentity::TEST_HASH,
            "the gates score must seal the harness identity the run resolved"
        );
        assert!(!payload.metrics.harness_hash.is_empty());
        assert!(bench_core::harness_hash::is_well_formed_harness_hash(
            &payload.metrics.harness_hash
        ));
    }

    /// F1, END TO END — THE GAP THIS CHANGE CLOSES. A benchd-authored gates-score.json now PASSES
    /// the seam-3 overlay's harness-identity gate.
    ///
    /// Before F1 every benchd gates-score sealed `harness_hash = ""`, and
    /// [`crate::overlay::validate_gates`] correctly refuses an empty harness identity — so no
    /// benchd-produced score could ever be published. This test runs the real producer and hands
    /// its payload to the real consumer. Reds if the stub returns (the overlay refuses it again),
    /// which is exactly the regression that must never ship silently.
    #[test]
    fn a_benchd_gates_score_now_passes_the_overlays_harness_identity_gate() {
        let golden = official_golden(None);
        let payload = gates_only_with(&golden, conformant_engine);
        crate::overlay::validate_gates(&payload).expect(
            "a benchd gates-score must satisfy the overlay's gates predicates, F1 included",
        );

        // The REFUSAL TWIN: the same payload with the pre-F1 empty identity is still refused, so
        // this test is proving the identity, not merely that `validate_gates` is permissive.
        let mut stubbed = payload.clone();
        stubbed.metrics.harness_hash = String::new();
        let err = crate::overlay::validate_gates(&stubbed)
            .expect_err("an empty harness identity must still be refused");
        assert!(err.contains("empty metrics.harness_hash"), "{err}");
    }

    /// Every OFFICIAL failure payload seals the identity too — a failed gates score is still an
    /// artifact, and a run that cannot say which harness produced it is the thing F1 removes.
    #[test]
    fn official_failure_payloads_seal_the_harness_identity() {
        let golden = official_golden(None);
        let failed = official_gates_failed(
            &golden,
            RunDigests::for_test(&DirDigest::empty()),
            "deadbeef",
            "boom".to_string(),
            None,
            None,
            0,
        );
        assert_eq!(failed.metrics.harness_hash, HarnessIdentity::TEST_HASH);
        assert!(!failed.passed);
    }
}
