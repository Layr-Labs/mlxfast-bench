//! Acceptance tests driving full runs through the in-process mock engine.
//!
//! WS1-5: full correctness / decode runs, handshake rejection, nonce/id/engine-error
//! session-discard, non-JSON preamble skipping, EOF handling.
//! WS1-7: phase-close barrier — matched counter passes; under/over/missing report fails
//! with `CompletedWorkMismatch`; the counter resets across phases.

use bench_runner::error::RunnerError;
use bench_runner::mock::{MockEngine, MOCK_NONCE};
use bench_runner::session::Session;

// ---- WS1-5: full runs -----------------------------------------------------

#[test]
fn full_teacher_forced_correctness_run() {
    let (mut session, hello) = Session::connect(MockEngine::new()).unwrap();
    assert_eq!(hello.backend.as_deref(), Some("mock"));
    assert_eq!(hello.device.as_deref(), Some("test"));
    assert_eq!(hello.protocol_version, Some(1));
    // #106 (passthrough MODEL): the conformant mock echoes head_provenance on the hello; the runner
    // parses it under the closed envelope instead of rejecting the line.
    let hp = hello
        .head_provenance
        .as_ref()
        .expect("mock echoes head_provenance");
    assert_eq!(hp.sha256, "mock-head-sha256");
    assert_eq!(hp.file_count, 1);

    session.begin_phase();

    let begin = session.correctness_begin(&[7, 8, 9]).unwrap();
    assert_eq!(begin.id, 1);
    assert!(begin.token.is_some());
    assert_eq!(begin.top_logits.as_ref().unwrap().len(), 8);
    // #106: the correctness step carries the always-present top_logit_margin + the conditional
    // expected-token logit/rank; the runner tolerates them under deny_unknown_fields.
    assert_eq!(begin.top_logit_margin, Some(1.75));
    assert_eq!(begin.expected_token_logit, Some(9.25));
    assert_eq!(begin.expected_token_rank, Some(0));

    // Several teacher-forced steps; ids must be strictly monotonic starting at 1.
    for (expected_id, tok) in (2..).zip([11, 12, 13, 14]) {
        let step = session.correctness_step(tok).unwrap();
        assert_eq!(step.id, expected_id);
        assert!(step.token.is_some());
        assert_eq!(step.top_logits.as_ref().unwrap().len(), 8);
        assert_eq!(step.top_logit_margin, Some(1.75));
    }

    // 1 begin + 4 steps = 5 timed steps.
    assert_eq!(session.issued_steps(), 5);
    let diag = session.close_phase().unwrap();
    assert_eq!(diag.completed_work, Some(5));
    // #106: the phase_diagnostics carries the PRE-drain MLX allocator ints, distinct from the
    // post-drain cache_memory. The runner accepts them under the closed envelope.
    assert_eq!(diag.mlx_active_memory_bytes, Some(2_147_483_648));
    assert_eq!(diag.mlx_cache_memory_bytes, Some(268_435_456));
    assert_eq!(diag.mlx_peak_memory_bytes, Some(3_221_225_472));
}

#[test]
fn full_decode_run() {
    let (mut session, _hello) = Session::connect(MockEngine::new()).unwrap();
    session.begin_phase();

    // prefill is NOT a timed step.
    let pre = session.prefill(&[1, 2, 3]).unwrap();
    assert!(pre.token.is_some());
    assert_eq!(session.issued_steps(), 0);

    let begin = session.decode_begin(&[4, 5, 6]).unwrap();
    assert!(begin.seed_token.is_some());

    for tok in [10, 11, 12] {
        let step = session.decode_step(tok).unwrap();
        assert!(step.token.is_some());
    }

    // 1 decode_begin + 3 decode_step = 4 timed steps.
    assert_eq!(session.issued_steps(), 4);
    let diag = session.close_phase().unwrap();
    assert_eq!(diag.completed_work, Some(4));
}

#[test]
fn free_run_correctness_returns_tokens() {
    let (mut session, _hello) = Session::connect(MockEngine::new()).unwrap();
    session.begin_phase();
    let resp = session.correctness(&[5, 6], 8).unwrap();
    assert_eq!(resp.tokens.as_ref().unwrap().len(), 8);
    // correctness free-run is not a timed step.
    assert_eq!(session.issued_steps(), 0);
}

// ---- WS1-5: handshake rejection -------------------------------------------

#[test]
fn hello_rejects_nonzero_id() {
    let engine = MockEngine::new().with_hello(1, true, Some(MOCK_NONCE));
    let result = Session::connect(engine);
    assert!(matches!(result, Err(RunnerError::Protocol(_))));
}

#[test]
fn hello_rejects_not_ok() {
    let engine = MockEngine::new().with_hello(0, false, Some(MOCK_NONCE));
    let result = Session::connect(engine);
    assert!(matches!(result, Err(RunnerError::Protocol(_))));
}

#[test]
fn hello_rejects_missing_nonce() {
    let engine = MockEngine::new().with_hello(0, true, None);
    let result = Session::connect(engine);
    assert!(matches!(result, Err(RunnerError::Protocol(_))));
}

#[test]
fn hello_rejects_empty_nonce() {
    let engine = MockEngine::new().with_hello(0, true, Some(""));
    let result = Session::connect(engine);
    assert!(matches!(result, Err(RunnerError::Protocol(_))));
}

// ---- WS1-5: nonce / id / engine-error session discard ---------------------

#[test]
fn nonce_mismatch_discards_session() {
    let engine = MockEngine::new().wrong_nonce_on("correctness_step");
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();

    match session.correctness_step(11) {
        Err(RunnerError::NonceMismatch { expected, got }) => {
            assert_eq!(expected, MOCK_NONCE);
            assert_eq!(got.as_deref(), Some("mock-nonce-0001-WRONG"));
        }
        other => panic!("expected NonceMismatch, got {other:?}"),
    }

    // A subsequent request must be refused.
    match session.correctness_step(12) {
        Err(RunnerError::SessionDiscarded) => {}
        other => panic!("expected SessionDiscarded, got {other:?}"),
    }
    assert!(session.is_discarded());
}

#[test]
fn id_mismatch_discards_session() {
    let engine = MockEngine::new().wrong_id_on("decode_step");
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.decode_begin(&[1, 2, 3]).unwrap();

    match session.decode_step(10) {
        Err(RunnerError::Protocol(_)) => {}
        other => panic!("expected Protocol, got {other:?}"),
    }
    match session.decode_step(11) {
        Err(RunnerError::SessionDiscarded) => {}
        other => panic!("expected SessionDiscarded, got {other:?}"),
    }
}

#[test]
fn engine_error_discards_session() {
    let engine = MockEngine::new().error_on("decode_step", "kernel exploded");
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.decode_begin(&[1, 2, 3]).unwrap();

    match session.decode_step(10) {
        Err(RunnerError::Engine { kind, message }) => {
            assert_eq!(kind, "decode_step");
            assert_eq!(message, "kernel exploded");
        }
        other => panic!("expected Engine, got {other:?}"),
    }
    match session.decode_step(11) {
        Err(RunnerError::SessionDiscarded) => {}
        other => panic!("expected SessionDiscarded, got {other:?}"),
    }
}

// ---- WS1-5: non-JSON preamble + EOF ---------------------------------------

#[test]
fn non_json_preamble_is_skipped() {
    let engine = MockEngine::new().log_lines_before("correctness_begin", 3);
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    // The 3 log lines before the response are skipped and the run still succeeds.
    let begin = session.correctness_begin(&[1, 2, 3]).unwrap();
    assert_eq!(begin.id, 1);
    let step = session.correctness_step(11).unwrap();
    assert_eq!(step.id, 2);
    session.close_phase().unwrap();
}

#[test]
fn eof_before_response_is_protocol_error() {
    let engine = MockEngine::new().eof_on("decode_begin");
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    match session.decode_begin(&[1, 2, 3]) {
        Err(RunnerError::Protocol(_)) => {}
        other => panic!("expected Protocol, got {other:?}"),
    }
    assert!(session.is_discarded());
}

// ---- WS1-7: phase-close barrier -------------------------------------------

#[test]
fn matched_counter_closes_ok() {
    let (mut session, _hello) = Session::connect(MockEngine::new()).unwrap();
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();
    session.correctness_step(11).unwrap();
    session.correctness_step(12).unwrap();
    let diag = session.close_phase().unwrap();
    assert_eq!(diag.completed_work, Some(3));
    assert!(!session.is_discarded());
}

#[test]
fn under_report_fails_and_discards() {
    let engine = MockEngine::new().completed_work_delta(-1);
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.decode_begin(&[1, 2, 3]).unwrap();
    session.decode_step(10).unwrap();
    session.decode_step(11).unwrap();
    // issued = 3, reported = 2.
    match session.close_phase() {
        Err(RunnerError::CompletedWorkMismatch { issued, reported }) => {
            assert_eq!(issued, 3);
            assert_eq!(reported, Some(2));
        }
        other => panic!("expected CompletedWorkMismatch, got {other:?}"),
    }
    assert!(session.is_discarded());
}

#[test]
fn over_report_fails() {
    let engine = MockEngine::new().completed_work_delta(1);
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();
    session.correctness_step(11).unwrap();
    // issued = 2, reported = 3.
    match session.close_phase() {
        Err(RunnerError::CompletedWorkMismatch { issued, reported }) => {
            assert_eq!(issued, 2);
            assert_eq!(reported, Some(3));
        }
        other => panic!("expected CompletedWorkMismatch, got {other:?}"),
    }
}

// ---- #54: allocator-drain assertion on phase_diagnostics -------------------

#[test]
fn drained_cache_memory_zero_closes_ok() {
    // A conformant engine reports cache_memory == 0 (Swift's fail-closed drain
    // postcondition, Memory.cacheMemory == 0): close_phase passes, session healthy.
    let engine = MockEngine::new().cache_memory(0);
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();
    session.correctness_step(11).unwrap();
    let diag = session.close_phase().unwrap();
    assert_eq!(diag.cache_memory, Some(0));
    assert!(!session.is_discarded());
}

#[test]
fn nonzero_cache_memory_fails_closed_and_discards() {
    // An undrained allocator cache (cache_memory > 0) fails the run closed and discards
    // the session — the parent-side mirror of resetRuntimeWorkerAllocatorForPhaseStart.
    let engine = MockEngine::new().cache_memory(4096);
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();
    session.correctness_step(11).unwrap();
    match session.close_phase() {
        Err(RunnerError::AllocatorCacheNotDrained { reported }) => {
            assert_eq!(reported, 4096);
        }
        other => panic!("expected AllocatorCacheNotDrained, got {other:?}"),
    }
    assert!(session.is_discarded());
}

#[test]
fn absent_cache_memory_is_not_asserted_backcompat() {
    // A pre-#54 engine omits cache_memory (None): the drain is NOT asserted, so an honest
    // run with a matched completed-work counter still closes cleanly.
    let (mut session, _hello) = Session::connect(MockEngine::new()).unwrap();
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();
    session.correctness_step(11).unwrap();
    let diag = session.close_phase().unwrap();
    assert_eq!(diag.cache_memory, None);
    assert!(!session.is_discarded());
}

#[test]
fn missing_completed_work_fails() {
    // A phase_diagnostics that omits the completed_work field entirely.
    let engine = MockEngine::new().suppress_completed_work();
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.decode_begin(&[1, 2, 3]).unwrap();
    match session.close_phase() {
        Err(RunnerError::CompletedWorkMismatch { issued, reported }) => {
            assert_eq!(issued, 1);
            assert_eq!(reported, None);
        }
        other => panic!("expected CompletedWorkMismatch, got {other:?}"),
    }
}

#[test]
fn counter_resets_across_phases() {
    let (mut session, _hello) = Session::connect(MockEngine::new()).unwrap();

    // Phase A: 2 steps.
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();
    session.correctness_step(11).unwrap();
    let a = session.close_phase().unwrap();
    assert_eq!(a.completed_work, Some(2));

    // Phase B: 3 steps. No cross-phase leakage.
    session.begin_phase();
    session.decode_begin(&[4, 5, 6]).unwrap();
    session.decode_step(10).unwrap();
    session.decode_step(11).unwrap();
    let b = session.close_phase().unwrap();
    assert_eq!(b.completed_work, Some(3));
    // C6: close_phase resets the counter on success.
    assert_eq!(session.issued_steps(), 0);
}

// ---- C4: hello protocol_version validation ---------------------------------

#[test]
fn hello_rejects_missing_protocol_version() {
    let engine = MockEngine::new().with_hello_protocol_version(None);
    match Session::connect(engine) {
        Err(RunnerError::Protocol(msg)) => assert!(msg.contains("protocol_version")),
        Err(other) => panic!("expected Protocol(protocol_version), got {other:?}"),
        Ok(_) => panic!("expected rejection, got Ok"),
    }
}

#[test]
fn hello_rejects_wrong_protocol_version() {
    let engine = MockEngine::new().with_hello_protocol_version(Some(2));
    match Session::connect(engine) {
        Err(RunnerError::Protocol(msg)) => assert!(msg.contains("protocol_version")),
        Err(other) => panic!("expected Protocol(protocol_version), got {other:?}"),
        Ok(_) => panic!("expected rejection, got Ok"),
    }
}

// ---- C6: close_phase resets the counter without an explicit begin_phase -----

#[test]
fn close_phase_resets_counter_for_next_phase_without_begin() {
    // A conformant engine resets completed_work after phase_diagnostics; the runner must
    // too. Second phase opened WITHOUT begin_phase() must not carry the stale count.
    let (mut session, _hello) = Session::connect(MockEngine::new()).unwrap();

    session.begin_phase();
    session.decode_begin(&[1, 2, 3]).unwrap();
    session.decode_step(10).unwrap();
    let a = session.close_phase().unwrap();
    assert_eq!(a.completed_work, Some(2));
    assert_eq!(session.issued_steps(), 0); // reset by close_phase

    // NOTE: no begin_phase() here — before the fix, issued_steps stayed at 2 and this
    // phase would fail (issued 2+1=3 vs engine-reported 1).
    session.decode_begin(&[4, 5, 6]).unwrap();
    let b = session.close_phase().unwrap();
    assert_eq!(b.completed_work, Some(1));
    assert!(!session.is_discarded());
}

// ---- C8: unparseable-line (id=-1) surfaces the engine's error ---------------

#[test]
fn unparseable_line_surfaces_engine_error() {
    let engine = MockEngine::new().unparseable_on("decode_step");
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.decode_begin(&[1, 2, 3]).unwrap();
    match session.decode_step(10) {
        Err(RunnerError::Engine { kind, message }) => {
            assert_eq!(kind, "decode_step");
            assert!(message.contains("not valid JSON"));
        }
        other => panic!("expected Engine(engine error), got {other:?}"),
    }
    // fail-closed: session discarded.
    match session.decode_step(11) {
        Err(RunnerError::SessionDiscarded) => {}
        other => panic!("expected SessionDiscarded, got {other:?}"),
    }
}

// ---- S2: top_logits length validation on teacher-forced responses -----------

#[test]
fn correctness_step_rejects_wrong_top_logits_length() {
    let engine = MockEngine::new().bad_top_logits_on("correctness_step");
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    session.correctness_begin(&[1, 2, 3]).unwrap();
    match session.correctness_step(11) {
        Err(RunnerError::Protocol(msg)) => assert!(msg.contains("top_logits")),
        other => panic!("expected Protocol(top_logits), got {other:?}"),
    }
    assert!(session.is_discarded());
}

// ---- v1.1: oracle-verified free-run timed mode (end-to-end via the mock) -----

#[test]
fn free_run_capability_is_advertised_and_gated() {
    // A v1.1-capable engine advertises the flag; a v1-only engine does not, and issuing a
    // free_decode_* request to it is REFUSED fail-closed (§2.1).
    let (capable, hello) = Session::connect(MockEngine::new().free_run_capable()).unwrap();
    assert!(hello.supports_free_run_decode());
    assert!(capable.supports_free_run_decode());

    let (mut v1_only, hello_v1) = Session::connect(MockEngine::new()).unwrap();
    assert!(!hello_v1.supports_free_run_decode());
    match v1_only.free_decode_run(128) {
        Err(RunnerError::CapabilityNotAdvertised { capability }) => {
            assert_eq!(capability, "free_run_decode");
        }
        other => panic!("expected CapabilityNotAdvertised, got {other:?}"),
    }
    assert!(v1_only.is_discarded());
}

#[test]
fn free_run_positive_control_round_trip() {
    // A valid free-run round trip: seed forward oracle-checks, the committed stream returns,
    // and the §2.6 triple holds (R+1 completed_work). N=4, default one-token-per-round.
    let engine = MockEngine::new()
        .oracle_tokens(50, 60, vec![700, 701, 702, 703])
        .free_run_capable();
    let (mut session, _hello) = Session::connect(engine).unwrap();
    session.begin_phase();
    let begin = session.free_decode_begin(&[9, 9, 9]).unwrap();
    assert_eq!(begin.seed_token, Some(60));
    let run = session.free_decode_run(4).unwrap();
    assert_eq!(run.tokens, Some(vec![700, 701, 702, 703]));
    assert_eq!(run.committed_total, Some(4));
    assert_eq!(run.acceptance_lengths.as_ref().unwrap().len(), 4);
    // Phase-close: the engine reports completed_work == R + 1 == 5 (seed + 4 rounds).
    let diag = session.phase_diagnostics_raw().unwrap();
    assert_eq!(diag.completed_work, Some(5));
    assert!(!session.is_discarded());
}
