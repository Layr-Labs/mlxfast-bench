//! In-process conformant engine for tests.
//!
//! There is no live worker on this box, so [`MockEngine`] implements
//! [`LineTransport`](crate::transport::LineTransport) directly: it parses each written
//! [`WorkerRequest`] line and buffers the [`WorkerResponse`] line(s) that `read_line`
//! drains. It echoes the request `id` and a fixed session `nonce`, computes a sensible
//! per-kind payload, and keeps its OWN completed-work counter (incremented on the same
//! timed-step kinds the runner counts) which it reports on `phase_diagnostics`.
//!
//! Misbehavior is opt-in via the builder so acceptance tests can force each failure
//! mode the runner must catch.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use bench_protocol::{
    CorrectnessTraceLogit, ExpertStreamingStats, RequestKind, SpecConfig, WorkerRequest,
    WorkerResponse,
};

use crate::transport::LineTransport;

/// Fixed nonce a conformant [`MockEngine`] uses for its whole session.
pub const MOCK_NONCE: &str = "mock-nonce-0001";

/// Configurable (mis)behavior for the mock engine. All misbehaviors are keyed to the
/// FIRST request of a given `kind` (one-shot), so tests can target, e.g., the first
/// `correctness_step`.
#[derive(Debug, Clone, Default)]
pub struct MockEngine {
    nonce: String,
    hello_id: i64,
    hello_ok: bool,
    hello_nonce: Option<String>,
    hello_protocol_version: Option<u32>,
    hello_backend: Option<String>,
    hello_device: Option<String>,
    /// v1.1 (additive): capability flags advertised on the hello. `None` (default) models a
    /// v1-only engine that advertises no capabilities; `Some([...])` a v1.1-capable engine.
    hello_capabilities: Option<Vec<String>>,
    /// spec (additive, `docs/spec-config-design.md`): the runnable modes advertised on
    /// `hello.spec_modes`. Defaults to `["serial","mtp"]` (a conformant spec-aware engine).
    hello_spec_modes: Option<Vec<String>>,
    /// spec: when set, echo THIS `effective_spec` on `decode_begin` / `free_decode_begin` instead of
    /// the requested spec — models an engine that ran a DIFFERENT spec, driving benchd's
    /// spec-never-ignored divergence reject (negative control). `None` = conformant (echo the request).
    diverge_spec_echo: Option<SpecConfig>,
    /// spec: when true, DROP the `effective_spec` echo entirely on `decode_begin` even though a spec
    /// was requested — models an engine that ignored the spec (missing echo), another divergence case.
    suppress_spec_echo: bool,
    /// #106 (passthrough MODEL): the `head_provenance` a conformant engine echoes on the `hello`.
    /// `None` models a pre-#106 engine that omits it; `new()` sets a conformant value so the suite
    /// exercises the modeled passthrough field.
    hello_head_provenance: Option<bench_protocol::HeadProvenance>,

    /// Emit a response carrying the wrong nonce for the first request of this kind.
    wrong_nonce_on: Option<String>,
    /// Emit a response carrying the wrong (id+1) id for the first request of this kind.
    wrong_id_on: Option<String>,
    /// Emit an `ok:false` error response for the first request of this kind.
    error_on: Option<(String, String)>,
    /// Emit this many non-JSON log lines before the response to the first request of this kind.
    log_lines_before: Option<(String, usize)>,
    /// Do not respond to the first request of this kind (stream then EOFs).
    eof_on: Option<String>,
    /// Emit the spec's unparseable-line response `{id:-1, ok:false, error}` on the first
    /// request of this kind.
    unparseable_on: Option<String>,
    /// Return a wrong-length `top_logits` (TOP_LOGITS_K - 1) on the first request of this kind.
    bad_top_logits_on: Option<String>,
    /// Offset the reported `completed_work` by this delta on `phase_diagnostics`.
    completed_work_delta: i64,
    /// Omit the `completed_work` field entirely on `phase_diagnostics` (report None).
    suppress_completed_work: bool,
    /// #54: report this `cache_memory` (MLX allocator free-buffer bytes) on
    /// `phase_diagnostics`. `None` (default) omits the field — a pre-#54 engine the parent
    /// does not assert; `Some(0)` models a conformant drained engine; `Some(n>0)` models a
    /// worker that failed to clear its allocator cache.
    cache_memory_report: Option<i64>,
    /// When set, the engine returns these exact timed-benchmark tokens instead of the
    /// default id-derived ones: `prefill` → `prefill_token`, `decode_begin` → `seed_token`,
    /// and the Nth `decode_step` → `decode_tokens[n]`. Lets a test model both a conformant
    /// engine (tokens equal the golden oracle) and a divergent one (a wrong entry).
    oracle_tokens: Option<OracleTokens>,
    /// When set, the engine returns these exact teacher-forced tokens for the
    /// correctness_begin/step stream (B3), keyed PER SEQUENCE. Each `correctness_begin`
    /// starts a new correctness sequence (a primary teacher-forced case or an anchor), so
    /// the oracle advances to the next inner vec and resets its within-sequence index: the
    /// Nth response of the Kth sequence returns `teacher_forced_sequences[K][N]`. A test
    /// gives each sequence its OWN token list — `[[case tokens…], [anchor token], …]` — so
    /// a conformant engine on a mixed cases+anchors golden is expressible without
    /// hand-concatenating one flat stream in issue order (#55). Perturb one entry to model
    /// a divergent sequence. See `teacher_forced_tokens` (single-sequence convenience) and
    /// `teacher_forced_sequences` (explicit per-sequence).
    teacher_forced_sequences: Option<Vec<Vec<i64>>>,
    /// v1.1: per-round `acceptance_lengths` the engine reports on `free_decode_run`. `None`
    /// defaults to `vec![1; N]` (R = N rounds, each committing one token) — a conformant
    /// histogram whose sum equals N. A test overrides this to model a doctored histogram
    /// (sum != N) for a negative control. The mock advances its `completed_work` counter by
    /// the round count R (plus 1 for `free_decode_begin`), so a conformant engine reports
    /// `completed_work == R + 1`.
    free_run_acceptance_lengths: Option<Vec<u32>>,
    /// v1.1: override `(drafted_total, accepted_total, committed_total)` on `free_decode_run`.
    /// `None` derives a conformant set (committed = N). A test sets e.g. `committed != N` for
    /// a negative control against the §2.4 count invariant.
    free_run_totals: Option<(u64, u64, u64)>,
    /// v1.2 (COHORT): the `max_batch_size` advertised on the hello (set by
    /// [`batched_free_run_capable`](Self::batched_free_run_capable)). `None` omits the field.
    hello_max_batch_size: Option<u32>,
    /// v1.2 (COHORT): per-slot oracle for the batched verbs — `(seed_token, decode_tokens)` per
    /// cohort slot in SLOT ORDER. `None` derives slot-distinct defaults. A test sets these equal
    /// to the cohort golden (conformant) or perturbs one slot (divergent).
    cohort_oracle: Option<Vec<(i64, Vec<i64>)>>,
    /// v1.2 (COHORT): echo THIS `effective_batch_size` instead of the requested one — models an
    /// engine that silently narrowed (or widened) the cohort, driving benchd's
    /// batch-never-ignored divergence reject. Negative control.
    diverge_batch_echo: Option<u32>,
    /// v1.2 (COHORT): DROP the `effective_batch_size` echo entirely on the batched verbs even
    /// though a `batch_size` was requested. Negative control (missing echo = divergence).
    suppress_batch_echo: bool,
    /// v1.2 (COHORT): return THIS many streams in `tokens_by_stream` instead of the requested B —
    /// models an engine whose committed rectangle is the wrong width. Negative control.
    cohort_width_override: Option<usize>,
    /// v1.2 (COHORT): report this exact `depth_clamp_reasons` histogram on the batched
    /// `free_decode_run`. `None` omits the field (a legitimate nothing-to-report engine).
    cohort_depth_clamp_reasons: Option<std::collections::BTreeMap<String, u32>>,
    /// Per-stream timing (per-stream-instrumentation-spec.md step 1): the exact
    /// `prefill_ns_by_stream` vector reported on the batched `free_decode_begin`. `None` (default)
    /// omits the field — an engine without the instrumentation. Set (with the capability) by
    /// [`per_stream_timing_capable`](Self::per_stream_timing_capable).
    cohort_prefill_ns_by_stream: Option<Vec<u64>>,
    /// Per-stream timing: the exact `decode_ns_by_stream` vector reported on the batched
    /// `free_decode_run`. Same default/None semantics as `cohort_prefill_ns_by_stream`.
    cohort_decode_ns_by_stream: Option<Vec<u64>>,
    /// H3 (cycle-3): NEVER respond to requests of this kind — the engine hangs (produces no
    /// response line and no EOF). Models a hung/looping engine; with a RunTimeout deadline armed,
    /// `read_line_deadline` returns `TimedOut` so benchd raises `RunTimeout` instead of wedging.
    stall_on: Option<String>,

    // --- runtime state ---
    outbox: VecDeque<String>,
    pending_eof: bool,
    /// H3 (cycle-3): set once a `stall_on` request is seen — the engine is now hung (no response).
    stalling: bool,
    completed_work: i64,
    /// Index of the next `decode_step` response drawn from `oracle_tokens.decode_tokens`.
    oracle_decode_index: usize,
    /// Index of the CURRENT teacher-forced sequence (which inner vec of
    /// `teacher_forced_sequences`). Advanced on each `correctness_begin` after the first.
    teacher_forced_seq: usize,
    /// Position WITHIN the current teacher-forced sequence. Reset to 0 on each
    /// `correctness_begin`, advanced on each `correctness_step`.
    teacher_forced_step: usize,
    /// Whether a `correctness_begin` has been seen yet (so the first one selects sequence 0
    /// without advancing the sequence index).
    teacher_forced_started: bool,
    /// Shared counter bumped on each received `phase_diagnostics` (lets a test prove the
    /// per-sequence drain actually round-trips to the worker — V1).
    phase_diagnostics_seen: Option<Rc<Cell<usize>>>,
    /// Shared record of the TIMED workload this engine received (prefill prompt, decode seed,
    /// and the teacher-forced decode-step inputs) — lets a test prove local-iterate times
    /// cases[0]'s stream (Ruling 1), not the benchmark oracle.
    recorded_timing: Option<Rc<RefCell<RecordedTiming>>>,
    triggered: HashSet<String>,
}

/// The exact tokens a [`MockEngine`] received during the timed phases (see `record_timing`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RecordedTiming {
    pub prefill_prompt: Vec<i64>,
    pub decode_seed: Vec<i64>,
    pub decode_step_inputs: Vec<i64>,
}

/// Fixed timed-benchmark tokens an [`OracleTokens`]-configured [`MockEngine`] returns.
#[derive(Debug, Clone)]
struct OracleTokens {
    prefill_token: i64,
    seed_token: i64,
    decode_tokens: Vec<i64>,
}

impl MockEngine {
    /// A fully conformant engine: valid hello (id=0, ok, fixed nonce, protocol 1,
    /// backend `"mock"`, device `"test"`) already queued for the handshake.
    pub fn new() -> Self {
        let mut engine = MockEngine {
            nonce: MOCK_NONCE.to_string(),
            hello_id: 0,
            hello_ok: true,
            hello_nonce: Some(MOCK_NONCE.to_string()),
            hello_protocol_version: Some(bench_protocol::PROTOCOL_VERSION),
            hello_backend: Some("mock".to_string()),
            hello_device: Some("test".to_string()),
            hello_spec_modes: Some(vec![
                bench_protocol::SPEC_MODE_SERIAL.to_string(),
                bench_protocol::SPEC_MODE_MTP.to_string(),
            ]),
            hello_head_provenance: Some(bench_protocol::HeadProvenance {
                sha256: "mock-head-sha256".to_string(),
                bytes: 1_048_576,
                file_count: 1,
            }),
            ..Default::default()
        };
        engine.queue_hello();
        engine
    }

    fn queue_hello(&mut self) {
        let mut hello = WorkerResponse {
            id: self.hello_id,
            ok: self.hello_ok,
            nonce: self.hello_nonce.clone(),
            expert_stats: Some(ExpertStreamingStats::zero()),
            protocol_version: self.hello_protocol_version,
            backend: self.hello_backend.clone(),
            device: self.hello_device.clone(),
            capabilities: self.hello_capabilities.clone(),
            spec_modes: self.hello_spec_modes.clone(),
            // #106: a conformant engine echoes its loaded-head provenance on the hello.
            head_provenance: self.hello_head_provenance.clone(),
            // v1.2 (COHORT): the advertised cohort-width ceiling, when batch-capable.
            max_batch_size: self.hello_max_batch_size,
            ..Default::default()
        };
        if !self.hello_ok {
            hello.error = Some("mock hello forced failure".to_string());
        }
        self.outbox
            .push_back(serde_json::to_string(&hello).unwrap());
    }

    /// Rebuild with a customized hello (used by the handshake-rejection tests). Clears
    /// any already-queued hello and re-queues the new one.
    pub fn with_hello(mut self, id: i64, ok: bool, nonce: Option<&str>) -> Self {
        self.hello_id = id;
        self.hello_ok = ok;
        self.hello_nonce = nonce.map(|s| s.to_string());
        self.outbox.clear();
        self.queue_hello();
        self
    }

    /// Give this engine a DISTINCT session nonce — both the hello nonce AND the nonce it
    /// echoes on every response — so a test can tell one spawned engine process from
    /// another by identity (§A lifecycle parity: prove local-iterate spawns a fresh engine
    /// per timed phase). Re-queues the hello so the handshake carries the new nonce.
    pub fn with_session_nonce(mut self, nonce: &str) -> Self {
        self.nonce = nonce.to_string();
        self.hello_nonce = Some(nonce.to_string());
        self.outbox.clear();
        self.queue_hello();
        self
    }

    /// Override the hello's `protocol_version` and re-queue the hello (for the C4
    /// protocol-version validation tests). `None` emits a hello with no version field.
    pub fn with_hello_protocol_version(mut self, version: Option<u32>) -> Self {
        self.hello_protocol_version = version;
        self.outbox.clear();
        self.queue_hello();
        self
    }

    /// v1.1: advertise the `free_run_decode` capability on the hello, so the parent will
    /// issue `free_decode_begin` / `free_decode_run` to this engine (else it refuses). Re-queues
    /// the hello so the handshake carries the capability.
    pub fn free_run_capable(mut self) -> Self {
        self.hello_capabilities =
            Some(vec![bench_protocol::CAPABILITY_FREE_RUN_DECODE.to_string()]);
        self.outbox.clear();
        self.queue_hello();
        self
    }

    /// v1.2 (COHORT): advertise BOTH free-run capabilities (`free_run_decode` +
    /// `batched_free_run_decode`) and this `max_batch_size` on the hello, so the parent will issue
    /// the cohort form of the free-run verbs to this engine. Re-queues the hello.
    pub fn batched_free_run_capable(mut self, max_batch_size: u32) -> Self {
        self.hello_capabilities = Some(vec![
            bench_protocol::CAPABILITY_FREE_RUN_DECODE.to_string(),
            bench_protocol::CAPABILITY_BATCHED_FREE_RUN_DECODE.to_string(),
        ]);
        self.hello_max_batch_size = Some(max_batch_size);
        self.outbox.clear();
        self.queue_hello();
        self
    }

    /// v1.2 (COHORT): fix the per-slot oracle for the batched verbs — one `(seed_token,
    /// decode_tokens)` per cohort slot in SLOT ORDER. A test sets these equal to the
    /// [`CohortTimingParams`](crate::CohortTimingParams) oracle (conformant engine) or perturbs
    /// one slot's entry (divergent stream).
    pub fn cohort_oracle(mut self, slots: Vec<(i64, Vec<i64>)>) -> Self {
        self.cohort_oracle = Some(slots);
        self
    }

    /// v1.2 (COHORT): echo a DIVERGENT `effective_batch_size` (not the requested one) on the
    /// batched verbs, so benchd's batch-never-ignored check rejects the leg. Negative control.
    pub fn diverge_batch_echo(mut self, echoed: u32) -> Self {
        self.diverge_batch_echo = Some(echoed);
        self
    }

    /// v1.2 (COHORT): DROP the `effective_batch_size` echo entirely on the batched verbs even
    /// though a `batch_size` was requested (models an engine that ignored it). Negative control.
    pub fn suppress_batch_echo(mut self) -> Self {
        self.suppress_batch_echo = true;
        self
    }

    /// v1.2 (COHORT): return this many streams in the batched `free_decode_run`'s
    /// `tokens_by_stream` instead of the requested B (a wrong-width committed rectangle).
    /// Negative control against the cohort consistency quadruple.
    pub fn cohort_width_override(mut self, width: usize) -> Self {
        self.cohort_width_override = Some(width);
        self
    }

    /// Per-stream timing (per-stream-instrumentation-spec.md step 1): advertise the
    /// `per_stream_timing` capability on the hello AND report these exact per-slot monotonic-ns
    /// vectors on the batched verbs (`prefill_ns_by_stream` on `free_decode_begin`,
    /// `decode_ns_by_stream` on `free_decode_run`). Layered on top of
    /// [`batched_free_run_capable`](Self::batched_free_run_capable) — call that first so the
    /// capability is APPENDED to the free-run pair rather than replacing it. Re-queues the hello
    /// so the handshake carries the capability.
    pub fn per_stream_timing_capable(
        mut self,
        prefill_ns_by_stream: Vec<u64>,
        decode_ns_by_stream: Vec<u64>,
    ) -> Self {
        self.hello_capabilities
            .get_or_insert_with(Vec::new)
            .push(bench_protocol::CAPABILITY_PER_STREAM_TIMING.to_string());
        self.cohort_prefill_ns_by_stream = Some(prefill_ns_by_stream);
        self.cohort_decode_ns_by_stream = Some(decode_ns_by_stream);
        self.outbox.clear();
        self.queue_hello();
        self
    }

    /// v1.2 (COHORT): report this exact `depth_clamp_reasons` histogram on the batched
    /// `free_decode_run` (sealed verbatim by benchd; audit-only).
    pub fn cohort_depth_clamp_reasons(
        mut self,
        reasons: std::collections::BTreeMap<String, u32>,
    ) -> Self {
        self.cohort_depth_clamp_reasons = Some(reasons);
        self
    }

    /// spec: advertise these exact `hello.spec_modes` (runnable modes). `None` advertises no modes.
    pub fn spec_modes(mut self, modes: Option<Vec<String>>) -> Self {
        self.hello_spec_modes = modes;
        self.outbox.clear();
        self.queue_hello();
        self
    }

    /// spec: echo a DIVERGENT `effective_spec` (not the requested one) on the timed seed forward, so
    /// benchd's spec-never-ignored check rejects the leg. Negative control.
    pub fn diverge_spec_echo(mut self, echo: SpecConfig) -> Self {
        self.diverge_spec_echo = Some(echo);
        self
    }

    /// spec: DROP the `effective_spec` echo entirely on the timed seed forward even though a spec was
    /// requested (models an engine that ignored the spec). Negative control.
    pub fn suppress_spec_echo(mut self) -> Self {
        self.suppress_spec_echo = true;
        self
    }

    /// v1.1: report this exact per-round `acceptance_lengths` on `free_decode_run` (and advance
    /// `completed_work` by its length R). A conformant histogram sums to N; a test perturbs it
    /// (sum != N, or a different R) to drive the §2.6 triple to fail.
    pub fn free_run_acceptance_lengths(mut self, lengths: Vec<u32>) -> Self {
        self.free_run_acceptance_lengths = Some(lengths);
        self
    }

    /// v1.1: override the `(drafted_total, accepted_total, committed_total)` counters on
    /// `free_decode_run` — e.g. `committed_total != N` for a §2.4 negative control.
    pub fn free_run_totals(mut self, drafted: u64, accepted: u64, committed: u64) -> Self {
        self.free_run_totals = Some((drafted, accepted, committed));
        self
    }

    /// Emit the spec's unparseable-line response `{id:-1, ok:false, error}` on the first
    /// request of `kind` (still carrying the session nonce, as the Swift worker does).
    pub fn unparseable_on(mut self, kind: &str) -> Self {
        self.unparseable_on = Some(kind.to_string());
        self
    }

    /// Return a wrong-length `top_logits` on the first request of `kind` (S2 test).
    pub fn bad_top_logits_on(mut self, kind: &str) -> Self {
        self.bad_top_logits_on = Some(kind.to_string());
        self
    }

    /// Force a wrong-nonce response on the first request of `kind`.
    pub fn wrong_nonce_on(mut self, kind: &str) -> Self {
        self.wrong_nonce_on = Some(kind.to_string());
        self
    }

    /// Force a wrong-id response on the first request of `kind`.
    pub fn wrong_id_on(mut self, kind: &str) -> Self {
        self.wrong_id_on = Some(kind.to_string());
        self
    }

    /// Force an `ok:false` error response on the first request of `kind`.
    pub fn error_on(mut self, kind: &str, message: &str) -> Self {
        self.error_on = Some((kind.to_string(), message.to_string()));
        self
    }

    /// Emit `n` non-JSON log lines before responding to the first request of `kind`.
    pub fn log_lines_before(mut self, kind: &str, n: usize) -> Self {
        self.log_lines_before = Some((kind.to_string(), n));
        self
    }

    /// EOF (no response) on the first request of `kind`.
    pub fn eof_on(mut self, kind: &str) -> Self {
        self.eof_on = Some(kind.to_string());
        self
    }

    /// H3 (cycle-3): NEVER respond to requests of `kind` — the engine hangs (no response, no EOF).
    /// With a RunTimeout deadline armed, the read aborts as `TimedOut` (→ `RunTimeout`) rather than
    /// hanging. Distinct from [`eof_on`](Self::eof_on), which closes the stream (EOF, not a hang).
    pub fn stall_on(mut self, kind: &str) -> Self {
        self.stall_on = Some(kind.to_string());
        self
    }

    /// Offset the reported `completed_work` counter by `delta` (under/over-report).
    pub fn completed_work_delta(mut self, delta: i64) -> Self {
        self.completed_work_delta = delta;
        self
    }

    /// Omit `completed_work` from `phase_diagnostics` entirely (report None).
    pub fn suppress_completed_work(mut self) -> Self {
        self.suppress_completed_work = true;
        self
    }

    /// #54: report `cache_memory` on `phase_diagnostics`. `0` models a conformant drained
    /// engine; a non-zero value models a worker that failed to clear its MLX allocator
    /// cache (the parent's `close_phase` then fails the run closed).
    pub fn cache_memory(mut self, bytes: i64) -> Self {
        self.cache_memory_report = Some(bytes);
        self
    }

    /// Return fixed timed-benchmark tokens: `prefill` → `prefill_token`, `decode_begin`
    /// → `seed_token`, and the Nth `decode_step` → `decode_tokens[n]`. A test sets these
    /// equal to the [`TimingParams`](crate::TimingParams) oracle to model a conformant
    /// engine, or perturbs one entry to model an engine that returns a wrong timed token.
    pub fn oracle_tokens(
        mut self,
        prefill_token: i64,
        seed_token: i64,
        decode_tokens: Vec<i64>,
    ) -> Self {
        self.oracle_tokens = Some(OracleTokens {
            prefill_token,
            seed_token,
            decode_tokens,
        });
        self
    }

    /// Return fixed teacher-forced tokens for a SINGLE correctness sequence (B3): the Nth of
    /// {begin, step, …} returns `tokens[n]`. Convenience for a golden with one teacher-forced
    /// sequence (one primary case, no anchors) — the common case. A test sets these to the
    /// case's expected tokens (conformant engine) or perturbs one (divergent). For a golden
    /// with MULTIPLE correctness sequences (primary cases AND anchors), use
    /// [`teacher_forced_sequences`](Self::teacher_forced_sequences) so each sequence gets its
    /// own token list without hand-concatenation (#55).
    pub fn teacher_forced_tokens(self, tokens: Vec<i64>) -> Self {
        self.teacher_forced_sequences(vec![tokens])
    }

    /// Return fixed teacher-forced tokens keyed PER correctness sequence (#55): each
    /// `correctness_begin` advances to the next inner vec and resets its within-sequence
    /// index, so the Nth response of the Kth sequence returns `sequences[K][N]`. Sequences
    /// are consumed in the golden's evaluation order (primary teacher-forced `cases[]` then
    /// anchors). A test gives each sequence its own token list — e.g. `[[2; 64], [999]]` for
    /// a base case conformant to `[2; 64]` followed by an anchor whose argmax is `999` — so a
    /// conformant engine on a mixed cases+anchors golden is expressible WITHOUT concatenating
    /// one flat stream in exact issue order. A sequence (or step) past its list's end reports
    /// the `i64::MIN` sentinel, surfacing a mismatch rather than panicking.
    pub fn teacher_forced_sequences(mut self, sequences: Vec<Vec<i64>>) -> Self {
        self.teacher_forced_sequences = Some(sequences);
        self
    }

    /// Share a counter that is bumped on every `phase_diagnostics` this engine receives,
    /// so a test can prove the per-sequence drain reaches the worker (V1).
    pub fn count_phase_diagnostics(mut self, counter: Rc<Cell<usize>>) -> Self {
        self.phase_diagnostics_seen = Some(counter);
        self
    }

    /// Share a record of the timed workload (prefill prompt / decode seed / decode-step
    /// inputs) this engine receives, so a test can prove which stream is being timed.
    pub fn record_timing(mut self, rec: Rc<RefCell<RecordedTiming>>) -> Self {
        self.recorded_timing = Some(rec);
        self
    }

    /// Has the one-shot misbehavior for `kind` already fired?
    fn take_trigger(&mut self, kind: &str) -> bool {
        if self.triggered.contains(kind) {
            return false;
        }
        self.triggered.insert(kind.to_string());
        true
    }

    /// v1.1: the per-round `acceptance_lengths` this engine reports for a `free_decode_run`
    /// of N committed tokens — the configured override, or the conformant default `vec![1; N]`
    /// (R = N single-token rounds). Used both to build the response and to advance the
    /// `completed_work` counter, so the two always agree.
    fn free_run_acceptance_for(&self, n: u32) -> Vec<u32> {
        self.free_run_acceptance_lengths
            .clone()
            .unwrap_or_else(|| vec![1; n as usize])
    }

    /// v1.2 (COHORT): the seed token this engine reports for cohort `slot` — the configured
    /// per-slot oracle, or a slot-distinct default.
    fn cohort_seed_for_slot(&self, slot: usize) -> i64 {
        self.cohort_oracle
            .as_ref()
            .and_then(|o| o.get(slot).map(|(seed, _)| *seed))
            .unwrap_or(2000 + slot as i64)
    }

    /// v1.2 (COHORT): the first `n` committed tokens this engine reports for cohort `slot` — the
    /// configured per-slot oracle continuation, or a slot-distinct default. A short oracle list
    /// yields a short stream (surfacing a driver-side rectangle failure rather than panicking).
    fn cohort_tokens_for_slot(&self, slot: usize, n: usize) -> Vec<i64> {
        match self.cohort_oracle.as_ref().and_then(|o| o.get(slot)) {
            Some((_, tokens)) => tokens.iter().take(n).copied().collect(),
            None => (0..n as i64)
                .map(|i| 3000 + slot as i64 * 10_000 + i)
                .collect(),
        }
    }

    /// v1.2 (COHORT): the `effective_batch_size` a conformant engine echoes — the REQUESTED width
    /// verbatim. A `diverge_batch_echo` / `suppress_batch_echo` mock returns a divergent / absent
    /// echo to drive benchd's batch-never-ignored reject.
    fn batch_echo(&self, requested: u32) -> Option<u32> {
        if self.suppress_batch_echo {
            return None;
        }
        Some(self.diverge_batch_echo.unwrap_or(requested))
    }

    /// spec: the `effective_spec` a conformant engine echoes on the seed forward — the REQUESTED
    /// spec verbatim (module ran exactly what was asked). A `diverge_spec_echo` / `suppress_spec_echo`
    /// mock returns a divergent / absent echo to drive benchd's spec-never-ignored reject. A request
    /// that carried no spec echoes none (a pre-spec/no-spec decode).
    fn effective_spec_echo(&self, req: &WorkerRequest) -> Option<SpecConfig> {
        req.spec.as_ref()?;
        if self.suppress_spec_echo {
            return None;
        }
        if let Some(echo) = &self.diverge_spec_echo {
            return Some(echo.clone());
        }
        req.spec.clone()
    }

    /// Build the conformant payload for a request kind (before misbehavior overrides).
    fn build_response(&self, req: &WorkerRequest) -> WorkerResponse {
        let mut resp = WorkerResponse {
            id: req.id,
            ok: true,
            nonce: Some(self.nonce.clone()),
            ..Default::default()
        };
        match req.kind.as_str() {
            "prefill" => {
                resp.token = Some(1000 + req.id);
            }
            "decode_begin" => {
                resp.seed_token = Some(2000 + req.id);
                resp.effective_spec = self.effective_spec_echo(req);
            }
            "decode_step" => {
                resp.token = Some(3000 + req.id);
            }
            "free_decode_begin" => {
                if let Some(b) = req.batch_size {
                    // v1.2 COHORT form: B seed forwards, one per slot in slot order, plus the
                    // never-ignored effective_batch_size echo.
                    resp.seed_token_by_stream = Some(
                        (0..b as usize)
                            .map(|slot| self.cohort_seed_for_slot(slot))
                            .collect(),
                    );
                    resp.effective_batch_size = self.batch_echo(b);
                    // Per-stream timing: the per-slot prefill ns vector, verbatim when configured.
                    resp.prefill_ns_by_stream = self.cohort_prefill_ns_by_stream.clone();
                } else {
                    // Same contract as decode_begin: one seed forward. Oracle override replaces
                    // this default with the golden seed token below.
                    resp.seed_token = Some(2000 + req.id);
                }
                resp.effective_spec = self.effective_spec_echo(req);
            }
            "free_decode_run" => {
                let n = req.count.unwrap_or(0);
                let accept = self.free_run_acceptance_for(n);
                if let Some(b) = req.batch_size {
                    // v1.2 COHORT form: the B x N committed rectangle, the SINGLE common-width
                    // acceptance_lengths vector, cohort-sum totals, and the audit vectors.
                    let width = self.cohort_width_override.unwrap_or(b as usize);
                    let rounds = accept.len();
                    resp.tokens_by_stream = Some(
                        (0..width)
                            .map(|slot| self.cohort_tokens_for_slot(slot, n as usize))
                            .collect(),
                    );
                    resp.effective_batch_size = self.batch_echo(b);
                    // Conformant naturals: every row walked exactly the committed common width.
                    resp.natural_accepted_by_stream = Some(vec![accept.clone(); b as usize]);
                    resp.active_streams_by_round = Some(vec![b; rounds]);
                    resp.rounds = Some(rounds as u32);
                    resp.depth_clamp_reasons = self.cohort_depth_clamp_reasons.clone();
                    let (drafted, accepted, committed) =
                        self.free_run_totals.unwrap_or_else(|| {
                            // Conformant cohort model: committed == B*N; one base-model fallback
                            // per round per stream is NOT an accepted draft, so
                            // accepted = B*(N - R); drafted >= accepted.
                            let committed = b as u64 * n as u64;
                            let accepted = b as u64 * (n as u64).saturating_sub(rounds as u64);
                            (committed, accepted, committed)
                        });
                    resp.acceptance_lengths = Some(accept);
                    resp.drafted_total = Some(drafted);
                    resp.accepted_total = Some(accepted);
                    resp.committed_total = Some(committed);
                    // Per-stream timing: the per-slot decode ns vector, verbatim when configured.
                    resp.decode_ns_by_stream = self.cohort_decode_ns_by_stream.clone();
                } else {
                    // Default (no oracle) tokens; the oracle override replaces these with the
                    // golden continuation so the driver's per-token exact-match sees a match.
                    resp.tokens = Some((0..n as i64).map(|i| 3000 + i).collect());
                    let (drafted, accepted, committed) =
                        self.free_run_totals.unwrap_or_else(|| {
                            // Conformant model: committed == N; one base-model fallback per round
                            // is NOT an accepted draft, so accepted = committed - R;
                            // drafted >= accepted.
                            let rounds = accept.len() as u64;
                            let committed = n as u64;
                            let accepted = committed.saturating_sub(rounds);
                            (committed, accepted, committed)
                        });
                    resp.acceptance_lengths = Some(accept);
                    resp.drafted_total = Some(drafted);
                    resp.accepted_total = Some(accepted);
                    resp.committed_total = Some(committed);
                }
            }
            "correctness" => {
                let steps = req.steps.unwrap_or(0).max(0);
                resp.tokens = Some((0..steps).map(|i| 4000 + i).collect());
                resp.peak_ram_gb = Some(18.0);
            }
            "correctness_begin" | "correctness_step" => {
                resp.token = Some(5000 + req.id);
                resp.top_logits = Some(mock_top_logits(req.id));
                // C9: PROTOCOL.md:68 + the schema list expert_stats as produced by
                // correctness_begin / correctness_step; a conformant engine attaches it.
                resp.expert_stats = Some(ExpertStreamingStats::zero());
                resp.peak_ram_gb = Some(18.5);
                // #106 (passthrough MODEL): the ALWAYS-present top-logit margin (top - second) plus
                // the conditional expected-token logit/rank, so the suite exercises the modeled fields.
                resp.top_logit_margin = Some(1.75);
                resp.expected_token_logit = Some(9.25);
                resp.expected_token_rank = Some(0);
            }
            "phase_diagnostics" => {
                resp.expert_stats = Some(ExpertStreamingStats::zero());
                resp.peak_ram_gb = Some(20.25);
                if !self.suppress_completed_work {
                    resp.completed_work = Some(self.completed_work + self.completed_work_delta);
                }
                // #54: surface the allocator-cache size when configured (else omit, back-compat).
                resp.cache_memory = self.cache_memory_report;
                // #106 (passthrough MODEL): the PRE-drain MLX allocator memory ints (distinct from the
                // POST-drain cache_memory). Conformant fixed values so the suite exercises them.
                resp.mlx_active_memory_bytes = Some(2_147_483_648);
                resp.mlx_cache_memory_bytes = Some(268_435_456);
                resp.mlx_peak_memory_bytes = Some(3_221_225_472);
            }
            _ => {
                // Unknown kind: still a well-formed ok response.
            }
        }
        resp
    }
}

fn mock_top_logits(seed: i64) -> Vec<CorrectnessTraceLogit> {
    (0..bench_protocol::TOP_LOGITS_K as i64)
        .map(|i| CorrectnessTraceLogit::new(5000 + seed + i, 10.0 - i as f64))
        .collect()
}

impl LineTransport for MockEngine {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let req: WorkerRequest = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e));
            }
        };
        let kind = req.kind.clone();

        // Advance the mock's own completed-work counter on the same timed-step kinds
        // the runner counts, and reset it when the phase closes. The timed set comes
        // from `RequestKind::is_timed_step` (the single definition), not a local list.
        if RequestKind::from_wire(&kind).is_some_and(|k| k.is_timed_step()) {
            self.completed_work += 1;
        }
        // v1.1 free-run: the phase's completed_work is the verify-round forward count, seed + R
        // rounds. free_decode_begin is the seed forward (+1); free_decode_run runs R rounds
        // (+R, R = acceptance_lengths.len()). So a conformant engine reports R+1. These kinds
        // are NOT is_timed_step (Amendment 4), so they are counted here explicitly.
        if kind == "free_decode_begin" {
            self.completed_work += 1;
        }
        if kind == "free_decode_run" {
            let n = req.count.unwrap_or(0);
            self.completed_work += self.free_run_acceptance_for(n).len() as i64;
        }
        if kind == "phase_diagnostics" {
            // The counter for this phase is read into the response below; reset after.
            if let Some(seen) = &self.phase_diagnostics_seen {
                seen.set(seen.get() + 1);
            }
        }
        // Record the TIMED workload (prefill prompt / decode seed / decode-step inputs).
        if let Some(rec) = &self.recorded_timing {
            let mut r = rec.borrow_mut();
            match kind.as_str() {
                "prefill" => {
                    if let Some(p) = &req.prompt_tokens {
                        r.prefill_prompt = p.clone();
                    }
                }
                "decode_begin" => {
                    if let Some(s) = &req.seed_tokens {
                        r.decode_seed = s.clone();
                    }
                }
                "decode_step" => {
                    if let Some(t) = req.token {
                        r.decode_step_inputs.push(t);
                    }
                }
                _ => {}
            }
        }

        // Non-JSON preamble lines before this response.
        if let Some((target, n)) = self.log_lines_before.clone() {
            if target == kind && self.take_trigger(&format!("log:{kind}")) {
                for i in 0..n {
                    self.outbox
                        .push_back(format!("[mock] log line {i} before {kind}"));
                }
            }
        }

        // EOF instead of responding.
        if let Some(target) = self.eof_on.clone() {
            if target == kind && self.take_trigger(&format!("eof:{kind}")) {
                self.pending_eof = true;
                return Ok(());
            }
        }

        // H3 (cycle-3) — STALL: never respond to this kind (no response line, no EOF). The engine is
        // now hung; a deadline-bounded read returns `TimedOut`. Not trigger-gated, so retries stall too.
        if self.stall_on.as_deref() == Some(kind.as_str()) {
            self.stalling = true;
            return Ok(());
        }

        let mut resp = self.build_response(&req);

        // Oracle-token override: replace the default id-derived timed tokens with the
        // fixed oracle set so a test can drive a conformant (matching) or divergent
        // (perturbed) engine through the parent-side oracle check in `run_timed_benchmark`.
        // The v1.2 COHORT form of the free-run verbs (`batch_size` present) is oracled by
        // `cohort_oracle` instead, so the single-stream override must not clobber it.
        if req.batch_size.is_none() {
            if let Some(oracle) = self.oracle_tokens.clone() {
                match kind.as_str() {
                    "prefill" => resp.token = Some(oracle.prefill_token),
                    "decode_begin" | "free_decode_begin" => {
                        resp.seed_token = Some(oracle.seed_token)
                    }
                    "decode_step" => {
                        let idx = self.oracle_decode_index;
                        self.oracle_decode_index += 1;
                        // A short decode_tokens list past its end reports a sentinel so the
                        // runner surfaces a mismatch rather than the mock panicking.
                        resp.token =
                            Some(oracle.decode_tokens.get(idx).copied().unwrap_or(i64::MIN));
                    }
                    "free_decode_run" => {
                        // Return the first N golden tokens as the committed stream. A perturbed
                        // entry (or a short list) surfaces a driver-side oracle mismatch.
                        //
                        // #109 W3 finding 6 — THE SEAM, and it is deliberate: this stream starts at
                        // `decode_tokens[0]`, NOT at `seed_token`. PROTOCOL-v1.1 §2.2 verifies
                        // `seed_token` on the `free_decode_begin` line against its own oracle field and
                        // then matches `tokens[i]` against `expected_decode_tokens[i]`; §2.1 says the
                        // begin "establishes the last-committed state" and the run commits N MORE. An
                        // engine that re-emits the seed here is one position late from step 0 — set
                        // `decode_tokens` to `[seed] + golden[..N-1]` to model exactly that (see
                        // `timing::tests::free_run_engine_that_reemits_the_seed_token_hard_fails_at_step_0`).
                        let n = req.count.unwrap_or(0) as usize;
                        resp.tokens = Some(oracle.decode_tokens.iter().take(n).copied().collect());
                    }
                    _ => {}
                }
            }
        }

        // Teacher-forced oracle (B3, #55): replace the default id-derived
        // correctness_begin/step token with the fixed PER-SEQUENCE teacher-forced set, so a
        // test drives a conformant (matching) or divergent case/anchor through the gate.
        // Each `correctness_begin` opens a fresh correctness sequence, so it advances to the
        // next inner vec and resets the within-sequence index; `correctness_step` advances
        // within the current sequence. This keys the oracle by sequence rather than a single
        // global index, so a mixed cases+anchors golden needs no hand-concatenation.
        if let Some(sequences) = self.teacher_forced_sequences.clone() {
            if kind == "correctness_begin" || kind == "correctness_step" {
                if kind == "correctness_begin" {
                    if self.teacher_forced_started {
                        self.teacher_forced_seq += 1;
                    }
                    self.teacher_forced_started = true;
                    self.teacher_forced_step = 0;
                } else {
                    self.teacher_forced_step += 1;
                }
                let t = sequences
                    .get(self.teacher_forced_seq)
                    .and_then(|seq| seq.get(self.teacher_forced_step))
                    .copied()
                    .unwrap_or(i64::MIN);
                resp.token = Some(t);
                // Keep the top-k consistent with the oracle token so BOTH the
                // teacher-forced tie path (base cases) and the argmax path
                // (anchors, which read canonical_argmax(top_logits)) see a
                // conformant engine: top-1 is `t` with the highest logit.
                resp.top_logits = Some(
                    (0..bench_protocol::TOP_LOGITS_K as i64)
                        .map(|i| CorrectnessTraceLogit::new(t + i, 10.0 - i as f64))
                        .collect(),
                );
            }
        }

        if let Some(target) = self.wrong_nonce_on.clone() {
            if target == kind && self.take_trigger(&format!("nonce:{kind}")) {
                resp.nonce = Some(format!("{}-WRONG", self.nonce));
            }
        }
        if let Some(target) = self.wrong_id_on.clone() {
            if target == kind && self.take_trigger(&format!("id:{kind}")) {
                resp.id = req.id + 1;
            }
        }
        if let Some((target, message)) = self.error_on.clone() {
            if target == kind && self.take_trigger(&format!("err:{kind}")) {
                resp.ok = false;
                resp.error = Some(message);
            }
        }
        if let Some(target) = self.unparseable_on.clone() {
            if target == kind && self.take_trigger(&format!("unparseable:{kind}")) {
                resp.id = -1;
                resp.ok = false;
                resp.error = Some("runtime worker protocol line was not valid JSON".to_string());
            }
        }
        if let Some(target) = self.bad_top_logits_on.clone() {
            if target == kind && self.take_trigger(&format!("toplogits:{kind}")) {
                if let Some(tl) = resp.top_logits.as_mut() {
                    tl.truncate(bench_protocol::TOP_LOGITS_K - 1);
                }
            }
        }

        // Reset the per-phase counter after phase_diagnostics has captured it.
        if kind == "phase_diagnostics" {
            self.completed_work = 0;
        }

        self.outbox.push_back(serde_json::to_string(&resp).unwrap());
        Ok(())
    }

    fn read_line(&mut self) -> std::io::Result<Option<String>> {
        if let Some(line) = self.outbox.pop_front() {
            return Ok(Some(line));
        }
        if self.pending_eof {
            return Ok(None);
        }
        // Nothing queued and no explicit EOF: report EOF so a stuck reader terminates.
        Ok(None)
    }

    fn read_line_deadline(
        &mut self,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<crate::transport::ReadOutcome> {
        use crate::transport::ReadOutcome;
        if let Some(line) = self.outbox.pop_front() {
            return Ok(ReadOutcome::Line(line));
        }
        // H3 (cycle-3) — a hung engine: honor the deadline (wait until it, then `TimedOut`) so the
        // caller raises `RunTimeout`. Without a deadline armed we return EOF defensively rather than
        // hang a test (the RunTimeout path always arms one).
        if self.stalling {
            return Ok(match deadline {
                Some(d) => {
                    let now = std::time::Instant::now();
                    if now < d {
                        std::thread::sleep(d - now);
                    }
                    ReadOutcome::TimedOut
                }
                None => ReadOutcome::Eof,
            });
        }
        // Otherwise mirror `read_line` (queued line already handled above; nothing left ⇒ EOF).
        Ok(ReadOutcome::Eof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    #[test]
    fn conformant_hello_connects() {
        let (session, hello) = Session::connect(MockEngine::new()).unwrap();
        assert_eq!(hello.nonce, MOCK_NONCE);
        assert_eq!(hello.protocol_version, Some(1));
        assert_eq!(hello.backend.as_deref(), Some("mock"));
        assert_eq!(hello.device.as_deref(), Some("test"));
        assert_eq!(session.nonce(), MOCK_NONCE);
    }
}
