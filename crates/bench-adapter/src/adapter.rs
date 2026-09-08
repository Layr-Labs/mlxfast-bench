//! The Engine Protocol v1 run loop and phase state machine.
//!
//! Reads `WorkerRequest` lines from a `BufRead`, drives an [`Engine`] minted
//! fresh per phase, and writes `WorkerResponse` lines to a `Write`.
//!
//! ## THE FREE-RUN PAIR
//!
//! `free_decode_begin` opens a single-stream phase on a resolved spec and
//! ECHOES the spec that will run; `free_decode_run(count)` commits `count`
//! tokens in ONE request and returns RAW COUNTERS ONLY. benchd brackets that
//! one request with its own clock and splits it itself -- the adapter emits no
//! rate, no ratio, no elapsed time and no speedup, on any verb. A paired
//! serial-vs-MTP comparison is two runs of this same phase under two specs,
//! and benchd is what compares them.
//!
//! Enforces:
//!
//! * the unsolicited `hello` (`id = 0`) at startup, establishing the session
//!   nonce echoed on every subsequent response, and carrying the optional
//!   `head_provenance` / `runner` identity blocks;
//! * fresh-engine-per-phase, drained to verified-zero at each phase opener;
//! * the `completed_work` counter — one per *timed step* as
//!   `RequestKind::is_timed_step` defines it, plus the free-run phase's own
//!   `R + 1` rule — reported at `phase_diagnostics`;
//! * session-discard-on-error and on early EOF (fail-closed);
//! * fail-closed on a step with no matching opener and on a double-open;
//! * one JSON object per line, echoing the request `id`.

use std::io::{BufRead, Write};

use bench_protocol::{
    ExpertStreamingStats, HeadProvenance, RequestKind, RunnerIdentity, SpecConfig, WorkerRequest,
    WorkerResponse, CAPABILITY_FREE_RUN_DECODE, PROTOCOL_VERSION,
};

use crate::engine::{Engine, EngineError, EngineFactory, Route, Step};

/// The DEFAULT bound on `free_decode_run`'s `count`. The wire leaves N
/// unbounded, so the adapter bounds it rather than letting a caller ask for an
/// arbitrary phase length. Same value the Swift reference worker uses
/// (`MLXFastConstants.freeRunMaxConfiguredTotalTokens`).
///
/// THIS IS A TRACK VALUE, NOT A PROTOCOL ONE. It belongs to the phase length a
/// given track measures, so it moves to the target bundle later and the bundle
/// becomes its one authority. Until then a track overrides it per adapter with
/// [`Adapter::with_free_run_max_count`], and this constant is only the default.
pub const FREE_RUN_MAX_COUNT: u32 = 1_536;

/// Which opener started the currently-open phase. Steps must match their
/// opener (`decode_step` only inside a `Decode` phase, `correctness_step` only
/// inside a `CorrectnessAnchor` phase); an opener while a phase is already open
/// is a fail-closed double-open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhaseKind {
    Prefill,
    Decode,
    CorrectnessFreeRun,
    CorrectnessAnchor,
    /// The v1.1 free-run phase: `free_decode_begin` opens it,
    /// `free_decode_run` commits N and is the phase's last work unit.
    FreeRun,
}

/// A fatal-to-this-request error. The adapter emits a matching `ok:false`
/// response, discards the session (fail-closed), and continues reading.
#[derive(Debug)]
enum ReqError {
    Malformed(String),
    Protocol(String),
    Engine(EngineError),
}

impl ReqError {
    fn message(&self) -> String {
        match self {
            ReqError::Malformed(m) | ReqError::Protocol(m) => m.clone(),
            ReqError::Engine(e) => e.to_string(),
        }
    }
}

/// The protocol adapter. Generic over the engine factory, so a track engine
/// and the mock share every line of this loop.
pub struct Adapter<F: EngineFactory> {
    factory: F,
    nonce: String,
    backend: String,
    device: String,
    /// The current phase's engine, minted at its opener and dropped at the
    /// barrier / on session discard.
    engine: Option<Box<dyn Engine>>,
    /// Which opener started the open phase (`None` between phases).
    phase: Option<PhaseKind>,
    /// The routes this worker advertises as runnable, in hello order. The
    /// ENGINE is what decides whether a declared mode is actually runnable on a
    /// given instance -- so a mode advertised here can still be refused by name
    /// at resolution.
    spec_modes: Vec<Route>,
    /// The loaded-head provenance, advertised on the hello when the engine
    /// knows it. AUDIT only, never scored.
    head_provenance: Option<HeadProvenance>,
    /// The runner identity, advertised on the hello when the engine serves a
    /// declared runner manifest. AUDIT only, never scored.
    runner: Option<RunnerIdentity>,
    /// The bound this adapter puts on `free_decode_run`'s `count`
    /// ([`FREE_RUN_MAX_COUNT`] unless the track overrides it).
    free_run_max_count: u32,
    /// Monotonic count of timed steps completed in the current phase.
    completed_work: i64,
}

impl<F: EngineFactory> Adapter<F> {
    /// Build an adapter with a freshly generated session nonce.
    /// `backend`/`device` are AUDIT strings: benchd records them, and nothing
    /// scores on them.
    pub fn new(factory: F, backend: impl Into<String>, device: impl Into<String>) -> Self {
        Self::with_session(factory, backend, device, generate_nonce())
    }

    /// Build an adapter with an explicit backend/device/nonce (tests pin the
    /// nonce for determinism).
    pub fn with_session(
        factory: F,
        backend: impl Into<String>,
        device: impl Into<String>,
        nonce: impl Into<String>,
    ) -> Self {
        Adapter {
            factory,
            nonce: nonce.into(),
            backend: backend.into(),
            device: device.into(),
            engine: None,
            phase: None,
            spec_modes: vec![Route::Serial, Route::Mtp],
            head_provenance: None,
            runner: None,
            free_run_max_count: FREE_RUN_MAX_COUNT,
            completed_work: 0,
        }
    }

    /// Narrow the advertised spec modes. An engine that cannot run a mode must
    /// not advertise it: benchd refuses a mode absent from `spec_modes` at the
    /// handshake, which is earlier and clearer than a mid-session refusal.
    pub fn advertising(mut self, modes: Vec<Route>) -> Self {
        self.spec_modes = modes;
        self
    }

    /// Carry the loaded-head provenance on the hello.
    pub fn with_head_provenance(mut self, head_provenance: HeadProvenance) -> Self {
        self.head_provenance = Some(head_provenance);
        self
    }

    /// Carry the runner identity on the hello.
    pub fn with_runner(mut self, runner: RunnerIdentity) -> Self {
        self.runner = Some(runner);
        self
    }

    /// Set this track's bound on `free_decode_run`'s `count`. The default is
    /// [`FREE_RUN_MAX_COUNT`]; the value is the track's, not the protocol's,
    /// and moves to the target bundle later.
    pub fn with_free_run_max_count(mut self, free_run_max_count: u32) -> Self {
        self.free_run_max_count = free_run_max_count;
        self
    }

    /// The unsolicited startup hello (`id = 0`): announces the protocol
    /// version, the backend and the device, ADVERTISES the runnable spec modes
    /// and the optional surfaces, carries the audit identity blocks, and
    /// establishes the session nonce.
    ///
    /// THE TWO ADVERTISEMENTS ARE NOT DECORATION. benchd gates on both, before
    /// any timed work:
    ///
    ///   * it refuses `free_decode_begin` / `free_decode_run` outright unless
    ///     `capabilities` carries `free_run_decode`; and
    ///   * it refuses a spec whose `mode` is absent from `spec_modes`, before
    ///     the timed seed forward.
    ///
    /// So a hello that omits them describes an engine that can run the serial
    /// control leg and nothing else.
    pub fn hello(&self) -> WorkerResponse {
        WorkerResponse {
            id: 0,
            nonce: Some(self.nonce.clone()),
            ok: true,
            expert_stats: Some(ExpertStreamingStats::zero()),
            protocol_version: Some(PROTOCOL_VERSION),
            backend: Some(self.backend.clone()),
            device: Some(self.device.clone()),
            spec_modes: Some(
                self.spec_modes
                    .iter()
                    .map(|r| r.as_str().to_string())
                    .collect(),
            ),
            capabilities: Some(vec![CAPABILITY_FREE_RUN_DECODE.to_string()]),
            head_provenance: self.head_provenance.clone(),
            runner: self.runner.clone(),
            ..Default::default()
        }
    }

    /// Run the protocol loop to EOF. Returns `Err` only on a hard I/O error;
    /// protocol/engine errors are reported inline (`ok:false`) and the loop
    /// continues after discarding the phase session. On EOF any in-flight
    /// session is discarded fail-closed.
    pub fn run<R: BufRead, W: Write>(&mut self, input: R, mut output: W) -> std::io::Result<()> {
        let hello = self.hello();
        self.emit(&mut output, &hello)?;

        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = self.service(&line);
            self.emit(&mut output, &response)?;
        }

        // Clean EOF: discard any half-advanced in-flight session (no barrier
        // synthesized).
        self.discard_session();
        Ok(())
    }

    /// Service one request line, always producing a `WorkerResponse`. On any
    /// error this discards the phase session (fail-closed) before returning the
    /// `ok:false` response.
    fn service(&mut self, line: &str) -> WorkerResponse {
        // Parse first: an unparseable line answers with id = -1.
        let request: WorkerRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                self.discard_session();
                return self.error(
                    -1,
                    format!("request line was not a valid WorkerRequest: {e}"),
                );
            }
        };
        let id = request.id;

        match self.dispatch(&request) {
            Ok(resp) => resp,
            Err(err) => {
                // Fail-closed: any error discards the phase's session state.
                self.discard_session();
                let msg = match &err {
                    ReqError::Malformed(_) => format!("malformed request: {}", err.message()),
                    _ => err.message(),
                };
                self.error(id, msg)
            }
        }
    }

    fn dispatch(&mut self, request: &WorkerRequest) -> Result<WorkerResponse, ReqError> {
        let kind = RequestKind::from_wire(&request.kind).ok_or_else(|| {
            ReqError::Malformed(format!("unknown request kind {:?}", request.kind))
        })?;
        let id = request.id;

        let response = match kind {
            // ---- opener: prefill (single forward, NOT a timed step) ----
            RequestKind::Prefill => {
                let prompt = require_tokens(&request.prompt_tokens, "prompt_tokens")?;
                let mut engine = self.open_phase(PhaseKind::Prefill)?;
                let token = engine.prefill(prompt).map_err(ReqError::Engine)?;
                self.install(engine, kind);
                let mut r = self.ok(id);
                r.token = Some(token);
                r
            }

            // ---- opener: decode_begin (seed forward, TIMED) ----
            RequestKind::DecodeBegin => {
                let seed = require_tokens(&request.seed_tokens, "seed_tokens")?;
                let mut engine = self.open_phase(PhaseKind::Decode)?;
                let seed_token = engine.decode_begin(seed).map_err(ReqError::Engine)?;
                self.install(engine, kind);
                let mut r = self.ok(id);
                r.seed_token = Some(seed_token);
                r
            }

            // ---- step: decode_step (TIMED, requires an open Decode phase) ----
            RequestKind::DecodeStep => {
                let token_in = require_token(&request.token)?;
                let engine = self.require_step_engine(kind, PhaseKind::Decode)?;
                let step = engine.step(token_in).map_err(ReqError::Engine)?;
                self.completed_work += 1; // is_timed_step
                let mut r = self.ok(id);
                r.token = Some(step.token); // decode_step response is token-only
                r
            }

            // ---- opener: correctness (free-run greedy, NOT timed) ----
            RequestKind::Correctness => {
                let prompt = require_tokens(&request.prompt_tokens, "prompt_tokens")?;
                let steps = request
                    .steps
                    .ok_or_else(|| ReqError::Malformed("correctness missing steps".into()))?;
                let mut engine = self.open_phase(PhaseKind::CorrectnessFreeRun)?;
                let tokens = engine
                    .correctness_freerun(prompt, steps)
                    .map_err(ReqError::Engine)?;
                let peak = engine.peak_ram_gb();
                self.install(engine, kind);
                let mut r = self.ok(id);
                r.tokens = Some(tokens);
                r.peak_ram_gb = Some(peak); // NOTE: plain correctness carries NO expert_stats
                r
            }

            // ---- opener: correctness_begin (teacher-forced, TIMED) ----
            RequestKind::CorrectnessBegin => {
                let prompt = require_tokens(&request.prompt_tokens, "prompt_tokens")?;
                let mut engine = self.open_phase(PhaseKind::CorrectnessAnchor)?;
                let step = engine.correctness_begin(prompt).map_err(ReqError::Engine)?;
                let stats = engine.expert_stats();
                let peak = engine.peak_ram_gb();
                self.install(engine, kind);
                self.correctness_gate_response(id, step, stats, peak)
            }

            // ---- step: correctness_step (TIMED, requires CorrectnessAnchor) ----
            RequestKind::CorrectnessStep => {
                let token_in = require_token(&request.token)?;
                let engine = self.require_step_engine(kind, PhaseKind::CorrectnessAnchor)?;
                let step = engine.step(token_in).map_err(ReqError::Engine)?;
                let stats = engine.expert_stats();
                let peak = engine.peak_ram_gb();
                self.completed_work += 1; // is_timed_step
                self.correctness_gate_response(id, step, stats, peak)
            }

            // ---- opener: free_decode_begin (seed forward, TIMED) ----
            RequestKind::FreeDecodeBegin => {
                let seed = require_tokens(&request.seed_tokens, "seed_tokens")?;
                let (route, depth) = self.resolve_spec(request.spec.as_ref())?;
                let mut engine = self.open_phase(PhaseKind::FreeRun)?;
                let (seed_token, effective) = engine
                    .free_decode_begin(seed, route, depth)
                    .map_err(ReqError::Engine)?;
                self.install(engine, kind);
                let mut r = self.ok(id);
                r.seed_token = Some(seed_token);
                // THE ECHO IS A STATEMENT OF WHAT WILL RUN. The engine
                // resolved it; the adapter forwards what came back, never what
                // was asked for.
                r.effective_spec = Some(effective);
                r
            }

            // ---- step: free_decode_run (commits N) ----
            RequestKind::FreeDecodeRun => {
                let count = request
                    .count
                    .ok_or_else(|| ReqError::Malformed("free_decode_run missing count".into()))?;
                if count == 0 {
                    return Err(ReqError::Malformed(
                        "free_decode_run count must be positive, got 0".into(),
                    ));
                }
                if count > self.free_run_max_count {
                    return Err(ReqError::Malformed(format!(
                        "free_decode_run count {count} is above the bound {}",
                        self.free_run_max_count
                    )));
                }
                let engine = self.require_step_engine(kind, PhaseKind::FreeRun)?;
                let result = engine.free_decode_run(count).map_err(ReqError::Engine)?;

                // THE CONSISTENCY TRIPLE, re-checked here before anything is
                // serialized. A backend that returned an inconsistent phase
                // would otherwise publish counters benchd cannot reconcile,
                // and benchd would refuse the RUN rather than name the bug.
                let count64 = u64::from(count);
                let committed: u64 = result
                    .acceptance_lengths
                    .iter()
                    .copied()
                    .map(u64::from)
                    .sum();
                if result.committed_total != count64 {
                    return Err(ReqError::Protocol(format!(
                        "free_decode_run committed_total {} != count {count}",
                        result.committed_total
                    )));
                }
                if result.tokens.len() as u64 != count64 {
                    return Err(ReqError::Protocol(format!(
                        "free_decode_run returned {} tokens, expected count {count}",
                        result.tokens.len()
                    )));
                }
                if committed != count64 {
                    return Err(ReqError::Protocol(format!(
                        "free_decode_run sum(acceptance_lengths) {committed} != count {count}"
                    )));
                }
                if result.drafted_total < result.accepted_total {
                    return Err(ReqError::Protocol(format!(
                        "free_decode_run drafted_total {} < accepted_total {}",
                        result.drafted_total, result.accepted_total
                    )));
                }
                // NOT CHECKED HERE: that every acceptance length is positive.
                // benchd's own schema gives `acceptance_lengths` a minimum of
                // 0, so a zero-length round is a shape benchd ACCEPTS. An
                // adapter-side rejection of it would invent an invariant the
                // contract does not have, and would refuse a run benchd would
                // have scored.

                // COMPLETED_WORK COUNTS ROUNDS, NOT TOKENS.
                //
                // benchd requires `completed_work == R + 1`, where R is the
                // number of ROUNDS the phase ran -- one per
                // `acceptance_lengths` entry. The opener already counted the
                // seed forward as 1; each round is one more unit.
                //
                // THIS IS NOT `count`. On the serial leg the two agree, because
                // every round commits exactly one token and R == N -- which is
                // exactly why counting tokens looks correct until a DRAFTING
                // leg runs. On the mtp leg a round commits several tokens, so
                // R < N, and counting tokens overstates the work by the
                // difference. A 10-token mtp phase over 4 rounds must report 5,
                // not 11.
                self.completed_work += result.acceptance_lengths.len() as i64;

                let mut r = self.ok(id);
                r.tokens = Some(result.tokens);
                r.acceptance_lengths = Some(result.acceptance_lengths);
                r.drafted_total = Some(result.drafted_total);
                r.accepted_total = Some(result.accepted_total);
                r.committed_total = Some(result.committed_total);
                r
            }

            // ---- barrier: phase_diagnostics (closes the open phase) ----
            RequestKind::PhaseDiagnostics => {
                let engine = self.engine.as_ref().ok_or_else(|| {
                    ReqError::Protocol("phase_diagnostics with no open phase to close".into())
                })?;
                let stats = engine.expert_stats();
                let peak = engine.peak_ram_gb();
                let completed_work = self.completed_work;
                // Report-then-reset: drop the fresh-per-phase engine, clear the
                // phase, zero the counter.
                self.discard_session();
                let mut r = self.ok(id);
                r.expert_stats = Some(stats);
                r.peak_ram_gb = Some(peak);
                r.completed_work = Some(completed_work);
                r
            }

            // ---- the trusted-oracle replay verb, which this SDK does not serve ----
            RequestKind::CohortReferenceReplay => {
                return Err(ReqError::Protocol(
                    "cohort_reference_replay is not served by this adapter".into(),
                ));
            }
        };
        Ok(response)
    }

    /// Resolve a requested `spec` into a route plus an optional requested
    /// depth. THE SPEC RIDES ONLY ON A DECODE OPENER. A mode this adapter does
    /// not carry is refused BY NAME rather than run as serial; whether a
    /// carried mode is RUNNABLE is the engine's decision. An ABSENT spec is
    /// `serial`, which is the v1 behaviour and the control leg.
    fn resolve_spec(&self, spec: Option<&SpecConfig>) -> Result<(Route, Option<u32>), ReqError> {
        let Some(spec) = spec else {
            return Ok((Route::Serial, None));
        };
        match Route::from_wire(&spec.mode) {
            Some(Route::Serial) => {
                if spec.mtp.is_some() {
                    return Err(ReqError::Malformed(
                        "spec mode \"serial\" carries an mtp block; the two disagree about what would run".into(),
                    ));
                }
                Ok((Route::Serial, None))
            }
            Some(Route::Mtp) => Ok((Route::Mtp, spec.mtp.and_then(|m| m.depth))),
            None => Err(ReqError::Malformed(format!(
                "spec mode {:?} is not a mode this worker runs; advertised modes are {}",
                spec.mode,
                self.advertised_modes()
            ))),
        }
    }

    fn advertised_modes(&self) -> String {
        self.spec_modes
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A base `ok:true` response carrying `id` and the session nonce.
    fn ok(&self, id: i64) -> WorkerResponse {
        WorkerResponse {
            nonce: Some(self.nonce.clone()),
            ..WorkerResponse::ok(id)
        }
    }

    /// A failure response carrying `id`, the session nonce and the message.
    fn error(&self, id: i64, message: impl Into<String>) -> WorkerResponse {
        WorkerResponse {
            nonce: Some(self.nonce.clone()),
            ..WorkerResponse::error(id, message)
        }
    }

    /// Both correctness gate kinds share the same response shape: token +
    /// top_logits[8] + expert_stats + peak_ram_gb.
    fn correctness_gate_response(
        &self,
        id: i64,
        step: Step,
        stats: ExpertStreamingStats,
        peak: f64,
    ) -> WorkerResponse {
        let mut r = self.ok(id);
        r.token = Some(step.token);
        r.top_logits = Some(step.top_logits);
        r.expert_stats = Some(stats);
        r.peak_ram_gb = Some(peak);
        r
    }

    /// Open a phase: reject a double-open (fail-closed), mint a cold engine, and
    /// drain it to verified-zero, resetting the completed-work counter.
    fn open_phase(&mut self, phase: PhaseKind) -> Result<Box<dyn Engine>, ReqError> {
        if let Some(open) = self.phase {
            return Err(ReqError::Protocol(format!(
                "double-open: {phase:?} arrived while a {open:?} phase was still open (missing phase_diagnostics)"
            )));
        }
        self.completed_work = 0;
        let mut engine = self.factory.create();
        let residual = engine.drain_to_zero().map_err(ReqError::Engine)?;
        if residual != 0 {
            return Err(ReqError::Engine(EngineError::DrainNonZero {
                residual_bytes: residual,
            }));
        }
        Ok(engine)
    }

    /// Commit a freshly-opened engine as the current phase, counting the
    /// opener's own forward when it is one.
    ///
    /// `RequestKind::is_timed_step` classifies the teacher-forced kinds
    /// (`decode_begin`, `decode_step`, `correctness_begin`, `correctness_step`)
    /// and is the authority for them. The free-run pair is NOT in that set,
    /// because a free-run phase counts `R + 1` rather than one unit per
    /// request: the seed forward is that `+ 1`, and the adapter adds it here.
    /// The rounds are added when `free_decode_run` returns.
    fn install(&mut self, engine: Box<dyn Engine>, opener: RequestKind) {
        self.engine = Some(engine);
        self.phase = Some(match opener {
            RequestKind::Prefill => PhaseKind::Prefill,
            RequestKind::DecodeBegin => PhaseKind::Decode,
            RequestKind::Correctness => PhaseKind::CorrectnessFreeRun,
            RequestKind::CorrectnessBegin => PhaseKind::CorrectnessAnchor,
            RequestKind::FreeDecodeBegin => PhaseKind::FreeRun,
            // steps/barrier/replay never install a phase.
            RequestKind::DecodeStep
            | RequestKind::CorrectnessStep
            | RequestKind::FreeDecodeRun
            | RequestKind::PhaseDiagnostics
            | RequestKind::CohortReferenceReplay => unreachable!("not an opener"),
        });
        if opener.is_timed_step() || opener == RequestKind::FreeDecodeBegin {
            self.completed_work += 1;
        }
    }

    /// Borrow the open engine for a step, fail-closed unless the open phase was
    /// started by the matching opener (no timed count on unverified state).
    fn require_step_engine(
        &mut self,
        step_kind: RequestKind,
        want: PhaseKind,
    ) -> Result<&mut Box<dyn Engine>, ReqError> {
        match self.phase {
            Some(p) if p == want => Ok(self.engine.as_mut().expect("open phase implies an engine")),
            Some(p) => Err(ReqError::Protocol(format!(
                "{} has no matching opener: current phase is {:?}, not {:?}",
                step_kind.as_str(),
                p,
                want
            ))),
            None => Err(ReqError::Protocol(format!(
                "{} with no open phase",
                step_kind.as_str()
            ))),
        }
    }

    /// Fail-closed session discard: drop the engine, clear the phase, zero the
    /// counter.
    fn discard_session(&mut self) {
        self.engine = None;
        self.phase = None;
        self.completed_work = 0;
    }

    /// Serialize one response as a single NDJSON line (object + '\n') and flush.
    fn emit<W: Write>(&self, output: &mut W, response: &WorkerResponse) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(response).expect("WorkerResponse serializes");
        line.push(b'\n');
        output.write_all(&line)?;
        output.flush()
    }
}

fn require_tokens<'a>(field: &'a Option<Vec<i64>>, name: &str) -> Result<&'a [i64], ReqError> {
    field
        .as_deref()
        .ok_or_else(|| ReqError::Malformed(format!("missing {name}")))
}

fn require_token(field: &Option<i64>) -> Result<i64, ReqError> {
    field.ok_or_else(|| ReqError::Malformed("missing token".into()))
}

/// A best-effort unpredictable session nonce, zero-dependency (time + pid,
/// hashed). Adequate for the replay/co-tenant defense on the TCP bridge; tests
/// pin it via [`Adapter::with_session`].
fn generate_nonce() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut hasher = DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    if let Ok(dur) = SystemTime::now().duration_since(UNIX_EPOCH) {
        dur.as_nanos().hash(&mut hasher);
    }
    // A second sample decorrelates same-nanosecond starts.
    let addr = &hasher as *const _ as usize;
    addr.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
