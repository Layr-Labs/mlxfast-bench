//! Adapter tests, driven entirely against the deterministic mock (no GPU).
//!
//! Three layers:
//!  1. BEHAVIOR — message framing/ordering, unsolicited hello, phase-close
//!     barrier + `completed_work`, fresh-engine-per-phase lifecycle, error /
//!     early-EOF / fail-closed handling.
//!  2. WIRE SHAPE — real-shaped requests (`id` + `kind` + `token`), `id` echo,
//!     nonce echo, flat responses carrying NO `kind`.
//!  3. CONFORMANCE — every emitted response line validates against the
//!     authoritative JSON Schema `bench-protocol` owns, with a negative control
//!     proving the validator bites.

use std::io::Cursor;

use bench_protocol::{
    HeadProvenance, RequestKind, RunnerIdentity, SpecConfig, WorkerRequest, JSON_SCHEMA,
};
use serde_json::Value;

use crate::adapter::Adapter;
use crate::mock::{
    Event, Method, MockConfig, MockFactory, MOCK_PEAK_RAM_GB, PREFILL_BASE, SEED_BASE,
};

const NONCE: &str = "testnonce";

/// Run the adapter (pinned nonce) over `lines` and return parsed response lines.
fn run(lines: &[&str]) -> Vec<Value> {
    let (factory, _log) = MockFactory::new();
    run_with(Adapter::with_session(factory, "mock", "mock", NONCE), lines)
}

fn run_with<F: crate::engine::EngineFactory>(
    mut adapter: Adapter<F>,
    lines: &[&str],
) -> Vec<Value> {
    let input = lines.join("\n");
    let mut out: Vec<u8> = Vec::new();
    adapter
        .run(Cursor::new(input), &mut out)
        .expect("no I/O error");
    parse_lines(&out)
}

fn parse_lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each response line is valid JSON"))
        .collect()
}

// ---------------------------------------------------------------------------
// Schema conformance validator — interprets the subset of JSON Schema Draft
// 2020-12 the schema uses ($ref, type, properties, required,
// additionalProperties:false, items, enum, minimum). Validates an emitted
// response against $defs/WorkerResponse.
//
// SCOPE: this is a conformance oracle for the lines THIS adapter EMITS, not a
// general benchd-deserialization oracle. It does not enforce
// `minimum`/unsignedness, so a hypothetical negative `protocol_version` would
// pass here yet be rejected by bench-protocol's `Option<u32>` serde — the
// adapter only ever emits `1`.
// ---------------------------------------------------------------------------

fn schema_root() -> Value {
    serde_json::from_str(JSON_SCHEMA).expect("the protocol schema is valid JSON")
}

fn resolve<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    if let Some(r) = node.get("$ref").and_then(Value::as_str) {
        let mut cur = root;
        for seg in r.trim_start_matches("#/").split('/') {
            cur = &cur[seg];
        }
        cur
    } else {
        node
    }
}

fn validate(root: &Value, schema: &Value, inst: &Value, path: &str) -> Result<(), String> {
    let schema = resolve(root, schema);
    let Some(ty) = schema.get("type").and_then(Value::as_str) else {
        return Ok(());
    };
    match ty {
        "object" => {
            let obj = inst
                .as_object()
                .ok_or_else(|| format!("{path}: expected object"))?;
            if let Some(req) = schema.get("required").and_then(Value::as_array) {
                for r in req {
                    let k = r.as_str().unwrap();
                    if !obj.contains_key(k) {
                        return Err(format!("{path}: missing required '{k}'"));
                    }
                }
            }
            let props = schema.get("properties").and_then(Value::as_object);
            let allow_additional =
                !matches!(schema.get("additionalProperties"), Some(Value::Bool(false)));
            for (k, v) in obj {
                match props.and_then(|p| p.get(k)) {
                    Some(sub) => validate(root, sub, v, &format!("{path}.{k}"))?,
                    None if allow_additional => {}
                    None => return Err(format!("{path}: additional property '{k}' not allowed")),
                }
            }
        }
        "array" => {
            let arr = inst
                .as_array()
                .ok_or_else(|| format!("{path}: expected array"))?;
            if let Some(items) = schema.get("items") {
                for (i, el) in arr.iter().enumerate() {
                    validate(root, items, el, &format!("{path}[{i}]"))?;
                }
            }
        }
        "integer" => {
            if !(inst.is_i64() || inst.is_u64()) {
                return Err(format!("{path}: expected integer, got {inst}"));
            }
        }
        "number" => {
            if !inst.is_number() {
                return Err(format!("{path}: expected number, got {inst}"));
            }
        }
        "string" => {
            let s = inst
                .as_str()
                .ok_or_else(|| format!("{path}: expected string"))?;
            if let Some(en) = schema.get("enum").and_then(Value::as_array) {
                if !en.iter().any(|e| e.as_str() == Some(s)) {
                    return Err(format!("{path}: '{s}' not in enum"));
                }
            }
        }
        "boolean" => {
            if !inst.is_boolean() {
                return Err(format!("{path}: expected boolean"));
            }
        }
        other => return Err(format!("{path}: unhandled schema type {other}")),
    }
    Ok(())
}

fn validate_response(root: &Value, resp: &Value) -> Result<(), String> {
    validate(root, &root["$defs"]["WorkerResponse"], resp, "response")
}

/// Assert every response line conforms to the authoritative schema.
fn assert_all_conform(resp: &[Value]) {
    let root = schema_root();
    for (i, r) in resp.iter().enumerate() {
        validate_response(&root, r)
            .unwrap_or_else(|e| panic!("response[{i}] {r} fails schema: {e}"));
    }
}

fn kind_field(v: &Value) -> Option<&str> {
    v.get("kind").and_then(Value::as_str)
}

// ---------------------------------------------------------------------------
// 1. BEHAVIOR
// ---------------------------------------------------------------------------

#[test]
fn startup_emits_unsolicited_hello_id_zero() {
    let resp = run(&[]);
    assert_eq!(resp.len(), 1, "only the unsolicited hello");
    let hello = &resp[0];
    assert_eq!(hello["id"], 0);
    assert_eq!(hello["ok"], true);
    assert_eq!(hello["nonce"], NONCE);
    assert_eq!(hello["protocol_version"], 1);
    assert_eq!(hello["backend"], "mock");
    assert_eq!(hello["device"], "mock");
    assert!(hello.get("expert_stats").is_some());
    // hello is a WorkerResponse, NOT a request kind — it carries no `kind`.
    assert!(kind_field(hello).is_none());

    // THE TWO ADVERTISEMENTS ARE REQUIRED, NOT OPTIONAL. benchd will not issue
    // a free-run verb without `free_run_decode` in `capabilities`, and refuses
    // a spec whose mode is absent from `spec_modes` before the timed seed
    // forward.
    let capabilities = hello["capabilities"]
        .as_array()
        .expect("the hello advertises capabilities");
    assert!(
        capabilities.iter().any(|c| c == "free_run_decode"),
        "the hello must advertise free_run_decode or benchd refuses the free-run verbs: {capabilities:?}"
    );
    let modes: Vec<&str> = hello["spec_modes"]
        .as_array()
        .expect("the hello advertises spec_modes")
        .iter()
        .map(|m| m.as_str().expect("mode is a string"))
        .collect();
    assert_eq!(
        modes,
        vec!["serial", "mtp"],
        "the hello must advertise both legs of the paired measurement"
    );

    // An engine that declares no identity blocks omits them (back-compat).
    assert!(hello.get("runner").is_none());
    assert!(hello.get("head_provenance").is_none());

    assert_all_conform(&resp);
}

/// The hello carries the AUDIT identity blocks when the engine declares them:
/// the runner identity (Darkbloom runner contract §6.1) and the loaded-head
/// provenance. Both are provenance only, never a scoring input.
#[test]
fn hello_carries_the_declared_runner_and_head_provenance() {
    let (factory, _log) = MockFactory::new();
    let runner = RunnerIdentity {
        id: "layr/mock-runner".to_string(),
        model_type: "qwen4_exp".to_string(),
        manifest_sha256: "a".repeat(64),
        build: "mock-build".to_string(),
    };
    let head = HeadProvenance {
        sha256: "b".repeat(64),
        bytes: 4096,
        file_count: 2,
    };
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE)
            .with_runner(runner)
            .with_head_provenance(head),
        &[],
    );
    let hello = &resp[0];
    assert_eq!(hello["runner"]["id"], "layr/mock-runner");
    assert_eq!(hello["runner"]["model_type"], "qwen4_exp");
    assert_eq!(hello["runner"]["build"], "mock-build");
    assert_eq!(hello["head_provenance"]["bytes"], 4096);
    assert_eq!(hello["head_provenance"]["file_count"], 2);
    assert_all_conform(&resp);
}

#[test]
fn happy_path_decode_phase_completed_work_is_one_plus_n() {
    let n = 4i64;
    let mut lines = vec![r#"{"id":1,"kind":"decode_begin","seed_tokens":[7,8,9]}"#.to_string()];
    for step in 0..n {
        lines.push(format!(
            r#"{{"id":{},"kind":"decode_step","token":{}}}"#,
            step + 2,
            1000 + step
        ));
    }
    lines.push(format!(r#"{{"id":{},"kind":"phase_diagnostics"}}"#, n + 2));
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();

    let resp = run(&refs);
    assert_all_conform(&resp);

    // hello, decode_begin, N decode_step, barrier
    assert_eq!(resp.len(), 1 + 1 + n as usize + 1);

    let begin = &resp[1];
    assert_eq!(begin["id"], 1);
    assert_eq!(begin["seed_token"], SEED_BASE + 3);
    assert!(
        begin.get("token").is_none(),
        "decode_begin carries seed_token, not token"
    );

    for step in 0..n as usize {
        let r = &resp[2 + step];
        assert_eq!(r["id"], step as i64 + 2);
        // decode_step response is token-only (shared forward, but no top_logits on the wire)
        assert_eq!(r["token"], 1000 + step as i64 + 1);
        assert!(
            r.get("top_logits").is_none(),
            "decode_step response must NOT carry top_logits"
        );
        assert_eq!(r["nonce"], NONCE);
    }

    let barrier = resp.last().unwrap();
    assert_eq!(barrier["completed_work"], 1 + n);
    assert_eq!(barrier["peak_ram_gb"], MOCK_PEAK_RAM_GB);
    assert!(barrier.get("expert_stats").is_some());
}

/// Per is_timed_step, prefill is NOT a timed step: a prefill-only phase reports
/// completed_work == 0.
#[test]
fn prefill_phase_completed_work_is_zero() {
    let resp = run(&[
        r#"{"id":1,"kind":"prefill","prompt_tokens":[1,2,3,4,5]}"#,
        r#"{"id":2,"kind":"phase_diagnostics"}"#,
    ]);
    assert_all_conform(&resp);
    assert_eq!(resp[1]["token"], PREFILL_BASE + 5);
    assert_eq!(resp[2]["completed_work"], 0, "prefill is NOT a timed step");
}

/// A correctness ANCHOR phase (correctness_begin + N correctness_step) is timed:
/// completed_work == 1 + N. Each step carries token + top_logits[8] + expert_stats.
#[test]
fn correctness_anchor_phase_completed_work_is_one_plus_n() {
    let resp = run(&[
        r#"{"id":1,"kind":"correctness_begin","prompt_tokens":[7,8,9]}"#,
        r#"{"id":2,"kind":"correctness_step","token":50}"#,
        r#"{"id":3,"kind":"correctness_step","token":51}"#,
        r#"{"id":4,"kind":"phase_diagnostics"}"#,
    ]);
    assert_all_conform(&resp);

    let begin = &resp[1];
    assert_eq!(begin["token"], PREFILL_BASE + 3);
    assert_eq!(begin["top_logits"].as_array().unwrap().len(), 8);
    assert!(begin.get("expert_stats").is_some());
    assert!(begin.get("peak_ram_gb").is_some());

    let step = &resp[2];
    assert_eq!(step["token"], 51); // 50 + 1
    assert_eq!(step["top_logits"].as_array().unwrap().len(), 8);
    assert!(step.get("expert_stats").is_some());

    assert_eq!(
        resp[4]["completed_work"], 3,
        "correctness_begin + 2 steps = 1 + 2"
    );
}

/// A free-run `correctness` request returns tokens[] + peak_ram_gb, is NOT timed
/// (completed_work == 0), and carries NO expert_stats (per schema note).
#[test]
fn correctness_freerun_returns_tokens_untimed() {
    let resp = run(&[
        r#"{"id":1,"kind":"correctness","prompt_tokens":[1,2],"steps":3}"#,
        r#"{"id":2,"kind":"phase_diagnostics"}"#,
    ]);
    assert_all_conform(&resp);
    let cor = &resp[1];
    assert_eq!(
        cor["tokens"],
        serde_json::json!([PREFILL_BASE, PREFILL_BASE + 1, PREFILL_BASE + 2])
    );
    assert!(cor.get("peak_ram_gb").is_some());
    assert!(
        cor.get("expert_stats").is_none(),
        "plain correctness carries no expert_stats"
    );
    assert_eq!(
        resp[2]["completed_work"], 0,
        "free-run correctness is NOT timed"
    );
}

#[test]
fn fresh_engine_per_phase_lifecycle() {
    let (factory, log) = MockFactory::new();
    let adapter = Adapter::with_session(factory, "mock", "mock", NONCE);
    let resp = run_with(
        adapter,
        &[
            r#"{"id":1,"kind":"decode_begin","seed_tokens":[1]}"#,
            r#"{"id":2,"kind":"decode_step","token":50}"#,
            r#"{"id":3,"kind":"phase_diagnostics"}"#,
            r#"{"id":4,"kind":"decode_begin","seed_tokens":[1,2]}"#,
            r#"{"id":5,"kind":"decode_step","token":60}"#,
            r#"{"id":6,"kind":"decode_step","token":61}"#,
            r#"{"id":7,"kind":"phase_diagnostics"}"#,
        ],
    );
    assert_all_conform(&resp);

    assert_eq!(log.created_count(), 2, "fresh engine per timed phase");
    let barriers: Vec<i64> = resp
        .iter()
        .filter(|r| r.get("completed_work").is_some())
        .map(|r| r["completed_work"].as_i64().unwrap())
        .collect();
    assert_eq!(barriers, vec![2, 3], "completed_work resets between phases");

    let events = log.events();
    assert_eq!(events[0], Event::Created(0));
    assert_eq!(events[1], Event::Drained(0));
    let created1 = events.iter().position(|e| *e == Event::Created(1)).unwrap();
    let dropped0 = events.iter().position(|e| *e == Event::Dropped(0)).unwrap();
    assert!(
        dropped0 < created1,
        "phase-1 engine dropped at its barrier before phase-2 is minted"
    );
    assert!(
        matches!(events[created1 + 1], Event::Drained(1)),
        "every fresh engine drains before any forward"
    );
}

// ---------------------------------------------------------------------------
// 2. WIRE SHAPE — id echo, nonce echo, flat responses
// ---------------------------------------------------------------------------

#[test]
fn real_shaped_request_echoes_id_and_produces_flat_shape() {
    let resp = run(&[r#"{"id":42,"kind":"prefill","prompt_tokens":[1,2,3]}"#]);
    assert_all_conform(&resp);
    let r = &resp[1];
    assert_eq!(r["id"], 42, "id is echoed");
    assert_eq!(r["ok"], true);
    assert_eq!(r["nonce"], NONCE);
    assert_eq!(r["token"], PREFILL_BASE + 3);
    assert!(kind_field(r).is_none(), "responses carry NO kind tag");
}

#[test]
fn every_response_echoes_nonce() {
    let resp = run(&[
        r#"{"id":1,"kind":"decode_begin","seed_tokens":[1]}"#,
        r#"{"id":2,"kind":"decode_step","token":5}"#,
        r#"{"id":3,"kind":"phase_diagnostics"}"#,
    ]);
    for r in &resp {
        assert_eq!(r["nonce"], NONCE, "response {r} missing session nonce");
    }
}

// ---------------------------------------------------------------------------
// 3. CONFORMANCE — schema validator + negative control
// ---------------------------------------------------------------------------

/// The negative control: the schema validator must REJECT a response with a
/// renamed field (additionalProperties:false). If this passed, the conformance
/// assertions above would be worthless.
#[test]
fn conformance_validator_bites_on_renamed_field() {
    let root = schema_root();
    // A genuine, valid emitted response.
    let resp = run(&[r#"{"id":7,"kind":"prefill","prompt_tokens":[1]}"#]);
    let good = resp[1].clone();
    validate_response(&root, &good).expect("the real emitted response conforms");

    // Mutate one field name: token -> tokenX. Now it violates the closed envelope.
    let mut bad = good.as_object().unwrap().clone();
    let v = bad.remove("token").unwrap();
    bad.insert("tokenX".to_string(), v);
    let bad = Value::Object(bad);
    let err = validate_response(&root, &bad).expect_err("renamed field MUST fail the schema");
    assert!(
        err.contains("tokenX"),
        "error should name the offending key: {err}"
    );
}

/// A wrong-typed field is also caught (token as a string).
#[test]
fn conformance_validator_bites_on_wrong_type() {
    let root = schema_root();
    let bad = serde_json::json!({"id": 1, "ok": true, "token": "not-an-int"});
    let err = validate_response(&root, &bad).expect_err("string token MUST fail");
    assert!(err.contains("token"), "{err}");
}

// ---------------------------------------------------------------------------
// Error handling + fail-closed hardening
// ---------------------------------------------------------------------------

#[test]
fn unparseable_line_answers_id_minus_one() {
    let resp = run(&[r#"{ this is not json"#]);
    assert_all_conform(&resp);
    let err = &resp[1];
    assert_eq!(err["id"], -1);
    assert_eq!(err["ok"], false);
    assert_eq!(err["nonce"], NONCE);
    assert!(err["error"].is_string());
}

#[test]
fn unknown_kind_errors_with_echoed_id() {
    let resp = run(&[r#"{"id":9,"kind":"teleport"}"#]);
    assert_all_conform(&resp);
    assert_eq!(resp[1]["id"], 9);
    assert_eq!(resp[1]["ok"], false);
}

#[test]
fn unknown_field_is_rejected_closed_envelope() {
    // deny_unknown_fields: a smuggled field fails the parse (id = -1).
    let resp = run(&[r#"{"id":3,"kind":"prefill","prompt_tokens":[1],"smuggled":true}"#]);
    assert_eq!(resp[1]["id"], -1);
    assert_eq!(resp[1]["ok"], false);
}

/// Malformed line mid-phase discards the session (fail-closed).
#[test]
fn malformed_line_discards_session() {
    let (factory, log) = MockFactory::new();
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE),
        &[
            r#"{"id":1,"kind":"decode_begin","seed_tokens":[1,2]}"#,
            r#"{ not json"#,
            // this step now has no open phase -> fail-closed
            r#"{"id":3,"kind":"decode_step","token":9}"#,
        ],
    );
    assert_all_conform(&resp);
    assert!(
        log.events().contains(&Event::Dropped(0)),
        "session discarded on malformed line"
    );
    // The trailing decode_step gets a no-open-phase error, not a success.
    let last = resp.last().unwrap();
    assert_eq!(last["ok"], false);
    assert_eq!(last["id"], 3);
}

/// A timed step with no open phase fails closed.
#[test]
fn decode_step_without_phase_fails_closed() {
    let resp = run(&[r#"{"id":1,"kind":"decode_step","token":1}"#]);
    assert_all_conform(&resp);
    assert_eq!(resp[1]["ok"], false);
    assert!(resp[1]["error"].as_str().unwrap().contains("no open phase"));
}

/// A decode_step must not run on a correctness-minted engine — the step's
/// opener must match. Fails closed, no timed count accrued.
#[test]
fn decode_step_on_correctness_phase_fails_closed() {
    let resp = run(&[
        r#"{"id":1,"kind":"correctness_begin","prompt_tokens":[1,2]}"#,
        r#"{"id":2,"kind":"decode_step","token":5}"#,
        r#"{"id":3,"kind":"phase_diagnostics"}"#,
    ]);
    assert_all_conform(&resp);
    // decode_step rejected (wrong opener) and session discarded.
    assert_eq!(resp[2]["id"], 2);
    assert_eq!(resp[2]["ok"], false);
    assert!(resp[2]["error"]
        .as_str()
        .unwrap()
        .contains("no matching opener"));
    // The barrier then has no open phase to close -> also fails closed.
    assert_eq!(resp[3]["ok"], false);
}

/// A double-open (two openers with no barrier between) fails closed.
#[test]
fn double_open_fails_closed() {
    let (factory, log) = MockFactory::new();
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE),
        &[
            r#"{"id":1,"kind":"prefill","prompt_tokens":[1,2]}"#,
            // second opener with no phase_diagnostics between -> double-open
            r#"{"id":2,"kind":"decode_begin","seed_tokens":[3]}"#,
        ],
    );
    assert_all_conform(&resp);
    assert_eq!(resp[2]["id"], 2);
    assert_eq!(resp[2]["ok"], false);
    assert!(resp[2]["error"].as_str().unwrap().contains("double-open"));
    // The prefill engine was discarded fail-closed.
    assert!(log.events().contains(&Event::Dropped(0)));
}

/// Early EOF mid-phase: no barrier synthesized; the in-flight engine is dropped.
#[test]
fn early_eof_discards_session_without_barrier() {
    let (factory, log) = MockFactory::new();
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE),
        &[
            r#"{"id":1,"kind":"decode_begin","seed_tokens":[1,2,3]}"#,
            r#"{"id":2,"kind":"decode_step","token":10}"#,
        ],
    );
    assert_all_conform(&resp);
    assert!(
        resp.iter().all(|r| r.get("completed_work").is_none()),
        "no barrier fabricated"
    );
    assert_eq!(
        *log.events().last().unwrap(),
        Event::Dropped(0),
        "session dropped at EOF"
    );
}

/// Fail-closed drain: a non-zero allocator residual aborts the phase before any
/// forward runs.
#[test]
fn nonzero_drain_fails_closed() {
    let (factory, log) = MockFactory::with_config(MockConfig {
        drain_residual: 4096,
        ..Default::default()
    });
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE),
        &[r#"{"id":1,"kind":"decode_begin","seed_tokens":[1]}"#],
    );
    assert_all_conform(&resp);
    assert_eq!(resp[1]["ok"], false);
    assert!(resp[1]["error"].as_str().unwrap().contains("residual"));
    let events = log.events();
    assert!(events.contains(&Event::Drained(0)));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Forward(_, Method::DecodeBegin, _))),
        "no forward ran after a failed drain"
    );
}

/// A mid-phase engine fault surfaces as an error and discards the session.
#[test]
fn engine_fault_mid_phase_discards_session() {
    let (factory, log) = MockFactory::with_config(MockConfig {
        fault_on_input: Some(42),
        ..Default::default()
    });
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE),
        &[
            r#"{"id":1,"kind":"decode_begin","seed_tokens":[1]}"#,
            r#"{"id":2,"kind":"decode_step","token":42}"#,
        ],
    );
    assert_all_conform(&resp);
    assert_eq!(resp[2]["ok"], false);
    assert!(log.events().contains(&Event::Dropped(0)));
}

/// After an error discards a session, a fresh opener starts cleanly (the loop
/// keeps serving).
#[test]
fn recovers_after_error_with_fresh_phase() {
    let resp = run(&[
        r#"{"id":1,"kind":"decode_step","token":1}"#, // error: no phase
        r#"{"id":2,"kind":"prefill","prompt_tokens":[1,2]}"#, // fresh phase, ok
        r#"{"id":3,"kind":"phase_diagnostics"}"#,
    ]);
    assert_all_conform(&resp);
    assert_eq!(resp[1]["ok"], false);
    assert_eq!(resp[2]["ok"], true);
    assert_eq!(resp[2]["token"], PREFILL_BASE + 2);
    assert_eq!(resp[3]["completed_work"], 0);
}

#[test]
fn blank_lines_are_skipped() {
    let resp = run(&[
        "",
        r#"{"id":1,"kind":"prefill","prompt_tokens":[1]}"#,
        "   ",
        r#"{"id":2,"kind":"phase_diagnostics"}"#,
    ]);
    assert_all_conform(&resp);
    assert_eq!(resp.last().unwrap()["completed_work"], 0);
}

// ---------------------------------------------------------------------------
// 4. THE v1.1 FREE-RUN PAIR
// ---------------------------------------------------------------------------
//
// Three things are proven here, and they are the three the free-run pair adds
// over base v1:
//
//   * THE VERB SEQUENCE. begin -> run -> barrier, with the phase counting
//     R + 1 where R is the number of ROUNDS (not the token count), and every
//     out-of-order form failing closed.
//   * THE SPEC ECHO. What comes back is what will RUN: an absent spec is
//     serial, an absent depth resolves to the engine's default, an
//     out-of-envelope explicit depth is REFUSED, and a mode the worker does not
//     carry is REFUSED BY NAME.
//   * NO DERIVED METRIC. The response carries raw counters and nothing that
//     divides two of them.

/// The keys a response is allowed to carry on the free-run path. Anything else
/// on a `free_decode_run` response is either a derived metric or an accident,
/// and both are worth failing on.
const FREE_RUN_RESPONSE_KEYS: &[&str] = &[
    "id",
    "nonce",
    "ok",
    "tokens",
    "acceptance_lengths",
    "drafted_total",
    "accepted_total",
    "committed_total",
];

fn free_begin(id: i64, spec: Option<&str>) -> String {
    match spec {
        Some(s) => {
            format!(r#"{{"id":{id},"kind":"free_decode_begin","seed_tokens":[1,2,3],"spec":{s}}}"#)
        }
        None => format!(r#"{{"id":{id},"kind":"free_decode_begin","seed_tokens":[1,2,3]}}"#),
    }
}

fn free_run_line(id: i64, count: i64) -> String {
    format!(r#"{{"id":{id},"kind":"free_decode_run","count":{count}}}"#)
}

#[test]
fn free_run_phase_verb_sequence_and_completed_work() {
    let resp = run(&[
        &free_begin(1, Some(r#"{"mode":"mtp","mtp":{"depth":2}}"#)),
        &free_run_line(2, 16),
        r#"{"id":3,"kind":"phase_diagnostics"}"#,
    ]);
    assert_eq!(resp.len(), 4, "hello + three answers");
    assert_all_conform(&resp);

    // begin: seed token + the resolved spec, and NO counters yet.
    let begin = &resp[1];
    assert_eq!(begin["id"], 1);
    assert_eq!(begin["ok"], true);
    assert_eq!(begin["seed_token"], SEED_BASE + 3);
    assert_eq!(begin["effective_spec"]["mode"], "mtp");
    assert_eq!(begin["effective_spec"]["mtp"]["depth"], 2);
    assert!(begin.get("tokens").is_none(), "the opener commits nothing");

    // run: exactly `count` tokens, and the consistency triple holds.
    let run_resp = &resp[2];
    assert_eq!(run_resp["id"], 2);
    assert_eq!(run_resp["ok"], true);
    assert_eq!(run_resp["committed_total"], 16);
    let tokens = run_resp["tokens"].as_array().expect("tokens array");
    assert_eq!(tokens.len(), 16);
    let lengths: Vec<i64> = run_resp["acceptance_lengths"]
        .as_array()
        .expect("acceptance_lengths array")
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(
        lengths.iter().sum::<i64>(),
        16,
        "sum(acceptance_lengths) == N"
    );
    assert!(
        lengths.iter().all(|&n| n > 0),
        "no round commits zero tokens"
    );
    assert!(
        run_resp["drafted_total"].as_i64().unwrap() >= run_resp["accepted_total"].as_i64().unwrap(),
        "drafted_total >= accepted_total"
    );
    assert!(
        run_resp["drafted_total"].as_i64().unwrap() > 0,
        "the mtp leg must actually draft, or this case proves nothing about drafting"
    );

    // THE BARRIER COUNTS ROUNDS, NOT TOKENS: benchd requires
    // `completed_work == R + 1`, where R is the number of ROUNDS. This case is
    // the one that discriminates, because the mtp leg commits 16 tokens over
    // FEWER rounds -- an adapter counting tokens would report 17 here and be
    // refused by benchd.
    let rounds = lengths.len() as i64;
    assert!(
        rounds < 16,
        "this case only proves the rule if R < N; got R={rounds}"
    );
    assert_eq!(resp[3]["completed_work"], rounds + 1);
}

#[test]
fn free_run_serial_leg_drafts_nothing_and_commits_one_per_round() {
    let resp = run(&[
        &free_begin(1, Some(r#"{"mode":"serial"}"#)),
        &free_run_line(2, 5),
        r#"{"id":3,"kind":"phase_diagnostics"}"#,
    ]);
    assert_eq!(resp[1]["effective_spec"]["mode"], "serial");
    assert!(
        resp[1]["effective_spec"].get("mtp").is_none(),
        "a serial echo must not carry an mtp block"
    );
    assert_eq!(resp[2]["drafted_total"], 0);
    assert_eq!(resp[2]["accepted_total"], 0);
    assert_eq!(resp[2]["committed_total"], 5);
    let lengths: Vec<i64> = resp[2]["acceptance_lengths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(lengths, vec![1, 1, 1, 1, 1]);
    // R == N on the serial leg, because every round commits exactly one token.
    // That coincidence is why a token-counting adapter passes this case and
    // fails the mtp one above.
    assert_eq!(resp[3]["completed_work"], 6);
}

#[test]
fn absent_spec_is_the_serial_control_leg() {
    let resp = run(&[&free_begin(1, None), &free_run_line(2, 3)]);
    assert_eq!(resp[1]["ok"], true);
    assert_eq!(resp[1]["effective_spec"]["mode"], "serial");
    assert_eq!(resp[2]["drafted_total"], 0);
}

#[test]
fn absent_mtp_depth_resolves_to_the_engines_default_and_is_echoed() {
    let resp = run(&[&free_begin(1, Some(r#"{"mode":"mtp"}"#))]);
    assert_eq!(resp[1]["ok"], true);
    assert_eq!(
        resp[1]["effective_spec"]["mtp"]["depth"],
        crate::mock::MTP_DEFAULT_DEPTH
    );
}

#[test]
fn an_explicitly_requested_depth_is_echoed_verbatim() {
    // benchd holds the echo of an EXPLICIT depth to the request. Every value
    // inside the envelope must therefore come back unchanged.
    for depth in [crate::mock::MTP_MIN_DEPTH, 2, crate::mock::MTP_MAX_DEPTH] {
        let spec = format!(r#"{{"mode":"mtp","mtp":{{"depth":{depth}}}}}"#);
        let resp = run(&[&free_begin(1, Some(&spec))]);
        assert_eq!(resp[1]["ok"], true, "depth {depth} is inside the envelope");
        assert_eq!(resp[1]["effective_spec"]["mtp"]["depth"], depth);
    }
}

#[test]
fn an_out_of_envelope_explicit_depth_is_refused_not_clamped() {
    // CLAMPING AN EXPLICIT REQUEST IS THE BUG THIS CATCHES. benchd requires an
    // explicitly requested depth to be echoed verbatim, so an engine that
    // clamped 9 to 3 and echoed 3 would produce an echo DIVERGENCE: benchd
    // discards the leg with a message about the echo, not about the depth, and
    // the operator is left with an opaque failure on a paired measurement.
    // Refusing by name is what makes the fault readable.
    for depth in [0u32, 4, 9] {
        let spec = format!(r#"{{"mode":"mtp","mtp":{{"depth":{depth}}}}}"#);
        let resp = run(&[&free_begin(1, Some(&spec))]);
        assert_eq!(resp[1]["ok"], false, "depth {depth} must be refused");
        let err = resp[1]["error"].as_str().unwrap();
        assert!(
            err.contains(&depth.to_string()) && err.contains("envelope"),
            "the refusal names the depth and the envelope: {err}"
        );
        assert!(
            resp[1].get("effective_spec").is_none(),
            "a refused begin echoes no spec"
        );
    }

    // A NEGATIVE depth is refused one layer earlier: `mtp.depth` is a `u32` on
    // the authoritative wire, so the line does not parse at all and the refusal
    // carries id = -1.
    let resp = run(&[&free_begin(1, Some(r#"{"mode":"mtp","mtp":{"depth":-1}}"#))]);
    assert_eq!(resp[1]["ok"], false);
    assert_eq!(
        resp[1]["id"], -1,
        "a negative depth is not a parseable spec"
    );
}

#[test]
fn an_undeclared_mode_is_refused_by_name() {
    // `dflash` is the mode a stale caller most plausibly asks for, and it must
    // be refused rather than quietly run as serial.
    let resp = run(&[&free_begin(1, Some(r#"{"mode":"dflash"}"#))]);
    assert_eq!(resp[1]["ok"], false);
    assert_eq!(resp[1]["id"], 1, "the refusal answers the request's own id");
    let err = resp[1]["error"].as_str().unwrap();
    assert!(err.contains("dflash"), "the refusal names the mode: {err}");
    assert!(
        err.contains("serial") && err.contains("mtp"),
        "the refusal names what IS runnable: {err}"
    );
}

#[test]
fn a_real_retired_arm_spec_still_gets_the_named_refusal() {
    // THE SHAPE A STALE CALLER ACTUALLY SENDS. A retired-arm spec carries its
    // own block -- `{"mode":"dflash","dflash":{...}}` -- not a bare mode. The
    // block is a known key on the authoritative wire, so the line PARSES and
    // the MODE is what is refused: the operator gets the named refusal with the
    // request's own id, not a parse error about a well-formed line.
    let resp = run(&[&free_begin(
        1,
        Some(r#"{"mode":"dflash","dflash":{"depth":4,"draft":{"artifact":"x","sha256":"y"}}}"#),
    )]);
    assert_eq!(resp[1]["ok"], false);
    assert_eq!(
        resp[1]["id"], 1,
        "the refusal carries the request's id, not -1: the line parsed, the MODE is what is wrong"
    );
    let err = resp[1]["error"].as_str().unwrap();
    assert!(err.contains("dflash"), "the refusal names the mode: {err}");
    assert!(
        !err.contains("not a valid WorkerRequest"),
        "a well-formed line naming a retired mode must not be reported as a parse error: {err}"
    );
}

#[test]
fn a_serial_spec_carrying_an_mtp_block_is_refused() {
    let resp = run(&[&free_begin(
        1,
        Some(r#"{"mode":"serial","mtp":{"depth":2}}"#),
    )]);
    assert_eq!(resp[1]["ok"], false);
    assert!(resp[1]["error"].as_str().unwrap().contains("mtp"));
}

#[test]
fn an_engine_that_cannot_run_mtp_refuses_by_name() {
    // The capability half of the same rule: the SPEC parsed fine, and the
    // ENGINE is what cannot run it.
    let (factory, _log) = MockFactory::with_config(MockConfig {
        runnable: Some(vec![crate::engine::Route::Serial]),
        ..MockConfig::default()
    });
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE)
            .advertising(vec![crate::engine::Route::Serial]),
        &[&free_begin(1, Some(r#"{"mode":"mtp"}"#))],
    );
    assert_eq!(resp[1]["ok"], false);
    let err = resp[1]["error"].as_str().unwrap();
    assert!(err.contains("mtp") && err.contains("serial"), "{err}");

    // AND THE HELLO SAID SO FIRST. benchd refuses a mode absent from
    // `spec_modes` before it ever issues the request, so an engine that cannot
    // run mtp must not advertise it -- otherwise the refusal lands mid-session
    // instead of at the handshake.
    let modes: Vec<&str> = resp[0]["spec_modes"]
        .as_array()
        .expect("the hello advertises spec_modes")
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["serial"]);
}

#[test]
fn free_decode_run_without_an_opener_fails_closed() {
    let resp = run(&[&free_run_line(1, 4)]);
    assert_eq!(resp[1]["ok"], false);
    assert!(resp[1]["error"]
        .as_str()
        .unwrap()
        .contains("free_decode_run"));
}

#[test]
fn free_decode_run_on_a_teacher_forced_phase_fails_closed() {
    // The two decode regimes must not be crossable: a free-run request inside a
    // teacher-forced phase would commit tokens the caller is timing under the
    // other regime's rules.
    let resp = run(&[
        r#"{"id":1,"kind":"decode_begin","seed_tokens":[1,2]}"#,
        &free_run_line(2, 4),
    ]);
    assert_eq!(resp[2]["ok"], false);
    let err = resp[2]["error"].as_str().unwrap();
    assert!(err.contains("Decode") && err.contains("FreeRun"), "{err}");
}

#[test]
fn decode_step_inside_a_free_run_phase_fails_closed() {
    let resp = run(&[
        &free_begin(1, None),
        r#"{"id":2,"kind":"decode_step","token":7}"#,
    ]);
    assert_eq!(resp[2]["ok"], false);
}

#[test]
fn free_decode_begin_while_a_phase_is_open_is_a_double_open() {
    let resp = run(&[&free_begin(1, None), &free_begin(2, None)]);
    assert_eq!(resp[2]["ok"], false);
    assert!(resp[2]["error"].as_str().unwrap().contains("double-open"));
}

#[test]
fn a_missing_or_out_of_range_count_is_refused() {
    let resp = run(&[&free_begin(1, None), r#"{"id":2,"kind":"free_decode_run"}"#]);
    assert_eq!(resp[2]["ok"], false);
    assert!(resp[2]["error"].as_str().unwrap().contains("count"));

    for bad in [0i64, i64::from(crate::adapter::FREE_RUN_MAX_COUNT) + 1] {
        let resp = run(&[&free_begin(1, None), &free_run_line(2, bad)]);
        assert_eq!(resp[2]["ok"], false, "count {bad} must be refused");
        assert_eq!(resp[2]["id"], 2);
        assert!(resp[2]["error"].as_str().unwrap().contains("count"));
    }

    // A NEGATIVE count is refused one layer earlier: `count` is a `u32` on the
    // authoritative wire, so the line does not parse and the refusal carries
    // id = -1.
    let resp = run(&[&free_begin(1, None), &free_run_line(2, -3)]);
    assert_eq!(resp[2]["ok"], false);
    assert_eq!(
        resp[2]["id"], -1,
        "a negative count is not a parseable line"
    );
}

/// The bound is the TRACK's, and the default is the SDK's. Both halves are
/// proven: an adapter that says nothing bounds at [`crate::adapter::FREE_RUN_MAX_COUNT`],
/// and an adapter that names its own bound is held to that instead.
#[test]
fn the_free_run_count_bound_defaults_and_overrides() {
    // DEFAULT: the constant is the bound, and the refusal names it.
    let at_default = i64::from(crate::adapter::FREE_RUN_MAX_COUNT);
    let resp = run(&[&free_begin(1, None), &free_run_line(2, at_default + 1)]);
    assert_eq!(resp[2]["ok"], false);
    assert!(resp[2]["error"]
        .as_str()
        .unwrap()
        .contains(&crate::adapter::FREE_RUN_MAX_COUNT.to_string()));

    // OVERRIDE: a track that names 4 refuses 5 and runs 4. The default value
    // is no longer the bound, which is the half a hardcoded constant fails.
    let (factory, _log) = MockFactory::new();
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE).with_free_run_max_count(4),
        &[&free_begin(1, None), &free_run_line(2, 5)],
    );
    assert_eq!(resp[2]["ok"], false, "5 is above this track's bound of 4");
    let err = resp[2]["error"].as_str().unwrap();
    assert!(err.contains("count 5") && err.contains("bound 4"), "{err}");

    let (factory, _log) = MockFactory::new();
    let resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE).with_free_run_max_count(4),
        &[&free_begin(1, None), &free_run_line(2, 4)],
    );
    assert_eq!(resp[2]["ok"], true, "4 is at the bound and runs");
    assert_eq!(resp[2]["committed_total"], 4);
}

#[test]
fn a_refused_free_run_discards_the_session() {
    // Fail-closed: the phase's half-advanced state must not survive to be
    // reused by the next request.
    let resp = run(&[
        &free_begin(1, None),
        &free_run_line(2, 0), // refused
        r#"{"id":3,"kind":"phase_diagnostics"}"#,
    ]);
    assert_eq!(resp[2]["ok"], false);
    assert_eq!(
        resp[3]["ok"], false,
        "the barrier finds no open phase, because the refusal discarded it"
    );
}

#[test]
fn free_run_response_carries_no_derived_metric() {
    // THE MEASUREMENT BOUNDARY, as a tripwire. The engine reports raw counters;
    // benchd times the request and does every division. A field whose name
    // implies a rate, a ratio, a duration or a speedup means the engine started
    // measuring itself.
    let resp = run(&[&free_begin(1, None), &free_run_line(2, 8)]);
    let run_resp = resp[2].as_object().expect("object");

    let mut unexpected: Vec<&String> = run_resp
        .keys()
        .filter(|k| !FREE_RUN_RESPONSE_KEYS.contains(&k.as_str()))
        .collect();
    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "free_decode_run response carries keys outside the raw-counter set: {unexpected:?}"
    );

    // Belt and braces, over the TOP-LEVEL keys of EVERY response the adapter
    // can emit rather than just this one.
    //
    // TOP-LEVEL, and the scope is deliberate: `expert_stats` is a
    // benchd-DEFINED v1 sub-struct and one of its members is
    // `expert_read_seconds`. That is benchd's own counter shape, not a number
    // this engine invented about its own speed, so the check does not descend
    // into it. What this refuses is a derived quantity appearing where the
    // ENGINE decides the key.
    for (i, r) in resp.iter().enumerate() {
        for key in r.as_object().expect("object").keys() {
            let lower = key.to_ascii_lowercase();
            for needle in DERIVED_METRIC_NEEDLES {
                assert!(
                    !lower.contains(needle),
                    "response[{i}] carries key {key:?}, which names a DERIVED quantity; \
                     measurement lives in benchd"
                );
            }
        }
    }
}

/// Name fragments that mean "the engine divided two numbers". Used by the
/// tripwire above and by its negative control below.
const DERIVED_METRIC_NEEDLES: &[&str] = &[
    "speedup",
    "ratio",
    "rate",
    "tps",
    "tokens_per_second",
    "seconds",
    "elapsed",
    "duration",
    "throughput",
    "latency",
    "score",
    "gain",
    "median",
    "composite",
];

#[test]
fn the_derived_metric_tripwire_bites() {
    // NEGATIVE CONTROL for the check above: a response that DID carry a derived
    // key must be caught by both halves. Built by hand, because the adapter
    // cannot emit one.
    let forged: Value = serde_json::from_str(
        r#"{"id":2,"nonce":"testnonce","ok":true,"tokens":[1],"decode_tps":42.0}"#,
    )
    .unwrap();
    let obj = forged.as_object().unwrap();

    let outside: Vec<&String> = obj
        .keys()
        .filter(|k| !FREE_RUN_RESPONSE_KEYS.contains(&k.as_str()))
        .collect();
    assert_eq!(
        outside.len(),
        1,
        "the key-set half must flag the forged derived key"
    );

    let caught = obj.keys().any(|k| {
        let lower = k.to_ascii_lowercase();
        DERIVED_METRIC_NEEDLES.iter().any(|n| lower.contains(n))
    });
    assert!(caught, "the needle half must flag the forged derived key");
}

#[test]
fn a_free_run_phase_mints_its_own_engine_and_drops_it_at_the_barrier() {
    let (factory, log) = MockFactory::new();
    let _resp = run_with(
        Adapter::with_session(factory, "mock", "mock", NONCE),
        &[
            &free_begin(1, None),
            &free_run_line(2, 4),
            r#"{"id":3,"kind":"phase_diagnostics"}"#,
            &free_begin(4, None),
            &free_run_line(5, 4),
            r#"{"id":6,"kind":"phase_diagnostics"}"#,
        ],
    );
    assert_eq!(
        log.created_count(),
        2,
        "one fresh engine per free-run phase"
    );
    let events = log.events();
    assert!(
        events.contains(&Event::Forward(0, Method::FreeDecodeRun, 4)),
        "the first phase's run reached the engine"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Dropped(_)))
            .count(),
        2,
        "both engines were dropped at their barriers"
    );
}

/// The trusted-oracle replay verb is a benchd-side kind this SDK does not
/// serve: it is refused by name rather than dispatched to the engine.
#[test]
fn cohort_reference_replay_is_refused_by_name() {
    let resp = run(&[r#"{"id":1,"kind":"cohort_reference_replay"}"#]);
    assert_all_conform(&resp);
    assert_eq!(resp[1]["id"], 1);
    assert_eq!(resp[1]["ok"], false);
    assert!(resp[1]["error"]
        .as_str()
        .unwrap()
        .contains("cohort_reference_replay"));
}

// ---------------------------------------------------------------------------
// 5. THE SHARED CONFORMANCE TRANSCRIPT
// ---------------------------------------------------------------------------
//
// One NDJSON file beside the schema, holding a whole session: the unsolicited
// hello, then every verb this adapter serves as a request line followed by the
// response line it produced, and last a refusal.
//
// IT IS REPLAYED BYTE FOR BYTE by the Swift bench-worker, so the bytes are the
// artifact: key order, number formatting and the omit-not-null shape must be
// what the adapter emits at RUNTIME, not a hand-written approximation. The test
// below therefore regenerates the transcript from the real loop over the real
// mock and asserts byte equality with the checked-in file. Nothing is derived
// from a clock: the ids and the nonce are pinned, the mock's tokens are a pure
// function of the input, and no verb reports a time.

/// The transcript's pinned session nonce.
const FIXTURE_NONCE: &str = "fixturenonce";

/// The checked-in transcript, beside the schema it conforms to.
const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../bench-protocol/schema/engine-wire-v1-adapter.ndjson"
);

/// The session the transcript records, in order. It walks every kind the
/// adapter serves, opening and closing each phase, and ends on the one kind it
/// REFUSES.
///
/// There is no `preflight` request: the protocol's kinds are the ten in
/// `RequestKind`, and preflight is not one of them.
fn fixture_requests() -> Vec<WorkerRequest> {
    let of = |id: i64, kind: RequestKind| WorkerRequest {
        id,
        kind: kind.as_str().to_string(),
        ..Default::default()
    };
    vec![
        // A prefill phase.
        WorkerRequest {
            prompt_tokens: Some(vec![11, 12, 13]),
            ..of(1, RequestKind::Prefill)
        },
        of(2, RequestKind::PhaseDiagnostics),
        // A teacher-forced decode phase.
        WorkerRequest {
            seed_tokens: Some(vec![21, 22]),
            ..of(3, RequestKind::DecodeBegin)
        },
        WorkerRequest {
            token: Some(3001),
            ..of(4, RequestKind::DecodeStep)
        },
        of(5, RequestKind::PhaseDiagnostics),
        // A free-run correctness phase.
        WorkerRequest {
            prompt_tokens: Some(vec![31, 32]),
            steps: Some(3),
            ..of(6, RequestKind::Correctness)
        },
        of(7, RequestKind::PhaseDiagnostics),
        // A correctness anchor phase.
        WorkerRequest {
            prompt_tokens: Some(vec![41, 42, 43]),
            ..of(8, RequestKind::CorrectnessBegin)
        },
        WorkerRequest {
            token: Some(5001),
            ..of(9, RequestKind::CorrectnessStep)
        },
        of(10, RequestKind::PhaseDiagnostics),
        // The v1.1 free-run pair, on the mtp leg so the counters are not all
        // the serial leg's ones.
        WorkerRequest {
            seed_tokens: Some(vec![51, 52, 53]),
            spec: Some(SpecConfig::mtp(2)),
            ..of(11, RequestKind::FreeDecodeBegin)
        },
        WorkerRequest {
            count: Some(6),
            ..of(12, RequestKind::FreeDecodeRun)
        },
        of(13, RequestKind::PhaseDiagnostics),
        // The verb this adapter does not serve: a named refusal, not a crash.
        of(14, RequestKind::CohortReferenceReplay),
    ]
}

/// The manifest the transcript's worker serves: the contract §11 manifest with
/// this worker's own `backend` and `runnerID`. It sits beside the transcript,
/// because a hello and its manifest are two views of one claim (§6.1) and a
/// transcript whose manifest lives somewhere else can drift from it.
const FIXTURE_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../bench-protocol/schema/engine-wire-v1-adapter.manifest.json"
));

/// The runner identity the transcript's hello carries. Pinned, because the
/// Swift bench-worker replays this file and matches every response line byte
/// for byte: the hello is the one line where the two workers could differ, and
/// the `runner` block is what it differs by. The block serializes LAST, which
/// is where the Darkbloom runner contract §6.0 puts it and where the Swift
/// worker appends it.
///
/// THE DIGEST IS COMPUTED, NOT TYPED. §6.0's canonical rule lives in
/// `bench-core`, so the test parses the manifest beside the transcript and asks
/// for its digest. A hand-copied digest is how a hello comes to quote a
/// manifest it does not serve: this transcript quoted the §11 manifest's
/// digest, whose `backend` is `mlx`, while saying `"backend":"mock"` — two
/// claims no honest worker can make at once (§6.1 derives the one from the
/// other).
///
/// `head_provenance` is NOT set: the mock loads no head, so it has no
/// provenance to state, and a fabricated digest in a replayed fixture would be
/// a false statement about bytes nobody loaded.
fn fixture_runner() -> RunnerIdentity {
    let manifest = bench_core::runner_manifest::parse_manifest(FIXTURE_MANIFEST)
        .expect("the transcript's manifest parses");
    RunnerIdentity {
        id: manifest.runner_id.clone(),
        model_type: "qwen4_exp_text".to_string(),
        manifest_sha256: manifest.digest(),
        build: "fixture".to_string(),
    }
}

/// Render the transcript by running the REAL loop over the REAL mock: hello,
/// then request/response pairs in order.
fn render_transcript() -> String {
    let requests = fixture_requests();
    let request_lines: Vec<String> = requests
        .iter()
        .map(|r| serde_json::to_string(r).expect("WorkerRequest serializes"))
        .collect();

    let (factory, _log) = MockFactory::new();
    let mut adapter =
        Adapter::with_session(factory, "mock", "mock", FIXTURE_NONCE).with_runner(fixture_runner());
    let mut out: Vec<u8> = Vec::new();
    adapter
        .run(Cursor::new(request_lines.join("\n")), &mut out)
        .expect("no I/O error");
    let out = String::from_utf8(out).expect("the adapter emits UTF-8");
    let response_lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        response_lines.len(),
        request_lines.len() + 1,
        "one hello plus one response per request"
    );

    let mut transcript = String::new();
    transcript.push_str(response_lines[0]); // the unsolicited hello
    transcript.push('\n');
    for (i, request) in request_lines.iter().enumerate() {
        transcript.push_str(request);
        transcript.push('\n');
        transcript.push_str(response_lines[i + 1]);
        transcript.push('\n');
    }
    transcript
}

/// The transcript on disk is the bytes this adapter emits. Set
/// `BENCH_ADAPTER_WRITE_FIXTURE=1` to rewrite it after an intended wire change;
/// with the variable unset the test compares and fails on any drift.
#[test]
fn the_checked_in_transcript_is_what_the_adapter_emits() {
    let rendered = render_transcript();
    if std::env::var_os("BENCH_ADAPTER_WRITE_FIXTURE").is_some() {
        std::fs::write(FIXTURE_PATH, &rendered).expect("the transcript is writable");
        return;
    }
    let on_disk = std::fs::read_to_string(FIXTURE_PATH).expect("the transcript is checked in");
    assert_eq!(
        on_disk, rendered,
        "the checked-in transcript is not what the adapter emits; the Swift bench-worker \
         replays these bytes, so regenerate it with BENCH_ADAPTER_WRITE_FIXTURE=1 when the \
         wire change is intended"
    );
}


#[test]
fn every_transcript_line_conforms_to_the_schema() {
    let root = schema_root();
    let transcript = std::fs::read_to_string(FIXTURE_PATH).expect("the transcript is checked in");
    let mut requests = 0;
    let mut responses = 0;
    for (i, line) in transcript.lines().enumerate() {
        let value: Value = serde_json::from_str(line).expect("each transcript line is JSON");
        let def = if kind_field(&value).is_some() {
            requests += 1;
            "WorkerRequest"
        } else {
            responses += 1;
            "WorkerResponse"
        };
        let outcome = validate(&root, &root["$defs"][def], &value, "line");
        // #270 added cohort_reference_replay to the schema's kind enum, so the
        // transcript's replay line validates like every other; the drift this
        // test once pinned (SCHEMA_KIND_ENUM_DRIFT) is closed.
        outcome.unwrap_or_else(|e| panic!("transcript line {i} ({line}) fails the schema: {e}"));
    }
    let expected = fixture_requests().len();
    assert_eq!(requests, expected, "one line per request");
    assert_eq!(responses, expected + 1, "one hello plus one per request");
}

/// The transcript reports nothing derived from a clock. Same needles as the
/// free-run tripwire, over every top-level key in the file.
#[test]
fn the_transcript_carries_no_timing_field() {
    let transcript = std::fs::read_to_string(FIXTURE_PATH).expect("the transcript is checked in");
    for (i, line) in transcript.lines().enumerate() {
        let value: Value = serde_json::from_str(line).expect("each transcript line is JSON");
        for key in value.as_object().expect("object").keys() {
            let lower = key.to_ascii_lowercase();
            for needle in DERIVED_METRIC_NEEDLES {
                assert!(
                    !lower.contains(needle),
                    "transcript line {i} carries key {key:?}, which names a DERIVED quantity"
                );
            }
        }
    }
}

/// THE TRANSCRIPT CANNOT CONTRADICT ITS MANIFEST. A hello and the runner
/// manifest are two views of one claim: §6.1 DERIVES `hello.backend`,
/// `capabilities` and `max_batch_size` from the manifest, and §6.2 holds
/// `spec_modes` to the declared decoders. This drives the real conformance gate
/// (`bench_core::runner_manifest::check_hello_against_manifest`) over the
/// checked-in manifest and the transcript's own hello line, so all six checks
/// have to hold on the bytes that ship.
///
/// It exists because they once did not: the hello said `"backend":"mock"` while
/// quoting the §11 manifest's digest, whose backend is `mlx`.
#[test]
fn the_transcript_hello_conforms_to_the_manifest_beside_it() {
    use bench_core::runner_manifest::{check_hello_against_manifest, parse_manifest, HelloFacts};
    use bench_protocol::WorkerResponse;

    let manifest = parse_manifest(FIXTURE_MANIFEST).expect("the transcript's manifest parses");
    let transcript = std::fs::read_to_string(FIXTURE_PATH).expect("the transcript is checked in");
    let hello_line = transcript
        .lines()
        .next()
        .expect("the transcript opens on a hello");
    let hello: WorkerResponse =
        serde_json::from_str(hello_line).expect("the hello line is a WorkerResponse");
    assert_eq!(hello.id, 0, "the hello is the unsolicited id = 0 response");

    let capabilities = hello.capabilities.clone().unwrap_or_default();
    let spec_modes = hello.spec_modes.clone().unwrap_or_default();
    let facts = HelloFacts {
        backend: hello.backend.as_deref(),
        capabilities: &capabilities,
        spec_modes: &spec_modes,
        max_batch_size: hello.max_batch_size,
        runner: hello.runner.as_ref(),
    };

    // The kit's untrusted posture: the mock advertises no trusted-build verb.
    let report = check_hello_against_manifest(&manifest, &facts, false);
    let failed: Vec<&str> = report.failures.iter().map(|f| f.name()).collect();
    assert!(
        report.passed(),
        "the transcript's hello does not match the manifest beside it: {failed:?}"
    );

    // NEGATIVE CONTROL: the gate must actually read these bytes. Move the
    // manifest's backend and the backend check fires by name.
    let mut divergent = manifest.clone();
    divergent.backend = "cuda".to_string();
    let report = check_hello_against_manifest(&divergent, &facts, false);
    let failed: Vec<&str> = report.failures.iter().map(|f| f.name()).collect();
    assert!(
        failed.contains(&"backend_matches"),
        "the backend check must bite on a divergent manifest: {failed:?}"
    );
}
