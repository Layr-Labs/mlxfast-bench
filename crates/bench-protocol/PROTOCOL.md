# Engine Protocol v1

Normative specification of the wire between the **trusted benchmarker** (`benchd` /
`mlxfast-bench`) and an **untrusted inference engine** (Swift/MLX on M5 today, CUDA
in an Ubuntu container next). This document and the accompanying JSON Schema
[`schema/engine-protocol-v1.schema.json`](schema/engine-protocol-v1.schema.json)
are the single source of truth; the Rust structs in `src/lib.rs` are a faithful port
of the Swift Codable types that define the wire today
(`RuntimeWorkerRequest` / `RuntimeWorkerResponse` in
`Sources/MLXFastHarness/QwenRuntimeWorker.swift`, `CorrectnessTraceLogit` in
`Sources/MLXFastHarness/QwenRuntime.swift`, `ExpertStreamingStats` in
`Sources/MLXFastCore/ExpertStreamingStats.swift`). See also
`docs/architecture.md` §3.

## Transport

- **Framing:** newline-delimited JSON (**NDJSON**) — exactly one JSON object per
  line, `\n`-terminated. No embedded newlines within an object.
- **Channel:** stdio when the engine is co-located with the benchmarker (default),
  or **localhost TCP** across a boundary (raw M5 via `bench-agent`, or a compose
  network to a container).
- **Correlation:** every request carries an `id`; the response echoes it. The first
  line the engine emits is the `hello` with `id = 0`.
- **What crosses the boundary:** *only* token IDs, top-K logits, and RSS. The
  benchmarker owns tokenization; the engine never sees text. All timing is
  benchmarker-side wall clock over request/response round trips — engine-reported
  numbers (including `peak_ram_gb`) are explicitly distrusted for scoring.

## Envelope

**Request** (benchmarker → engine): `{ "id": <i64>, "kind": <string>, ... }`

**Response** (engine → benchmarker): `{ "id": <i64>, "nonce": <string?>, "ok": <bool>, ... }`

Required fields: `id` and `kind` on a request; `id` and `ok` on a response.
The top level of each object is closed (`additionalProperties: false` in the
schema for the fields it defines).

### Optional-field encoding (fidelity rule)

Optional fields are **omitted entirely** when unset — never serialized as `null` —
matching Swift's `encodeIfPresent`. A missing key deserializes to "absent". Field
order on the wire follows the struct declaration order, so parsing a canonical line
and re-serializing it is byte-identical.

### Session, nonce, and error handling

- The `hello` establishes a per-session **nonce**; every subsequent response echoes
  it and the parent validates it (replay / co-tenant defense on the TCP bridge).
- On **any** error the response is `{ id, nonce, ok: false, error }` and the engine
  **discards the session state** for that phase (an errored forward may have advanced
  lazy KV/recurrent cache metadata; it must never be reusable).
- A request line that cannot be parsed yields a response with `id = -1`,
  `ok = false`, and an `error` string.

## Message kinds

Eight kinds. `hello` is the unsolicited `id = 0` response emitted once at startup;
the other seven are request kinds sent by the benchmarker.

| kind | request fields (in) | response fields (out) | notes |
|------|---------------------|-----------------------|-------|
| `hello` | — (unsolicited) | `nonce`, **`protocol_version`**, **`backend`**, **`device`**, `expert_stats` | emitted after in-engine weight/config validation; `id = 0`, `ok = true`. Bold fields are **new in v1** and meaningful only here. |
| `prefill` | `prompt_tokens[]` | `token` | must force full evaluation before responding |
| `decode_begin` | `seed_tokens[]` | `seed_token` | exactly one seed forward; no warmup permitted |
| `decode_step` | `token` | `token` | single step; **must share the code path with `correctness_step`** |
| `correctness` | `prompt_tokens[]`, `steps` | `tokens[]`, `peak_ram_gb` | free-run greedy |
| `correctness_begin` | `prompt_tokens[]` | `token`, `top_logits[8]`, `expert_stats`, `peak_ram_gb` | teacher-forced; opens the anchor-gate session |
| `correctness_step` | `token` | `token`, `top_logits[8]`, `expert_stats`, `peak_ram_gb` | teacher-forced; feeds anchor-gate rank / delta tolerance |
| `phase_diagnostics` | — | `expert_stats`, `peak_ram_gb`, `completed_work`, `cache_memory` | closes a timed phase; followed by allocator drain |

`top_logits` is an array of `{ "token": <i64>, "logit": <f64> }` (K = 8, per
`MLXFastConstants.correctnessTopLogits`). `expert_stats` is the
`ExpertStreamingStats` object; the dense RAM-resident Qwen runtime always reports the
zero struct, retained for schema stability.

### Phase-0 `hello` fields (new in v1)

Three optional fields were added to the response for v1 and are meaningful **only on
the `hello`**:

- `protocol_version` (`u32`, e.g. `1`) — the engine's implemented protocol version.
- `backend` (`string`, e.g. `"mlx"` / `"cuda"`) — the compute backend.
- `device` (`string`, e.g. `"m5"` / `"gb10"`) — the device identity.

They are appended after the pre-Phase-0 fields, so older messages that omit them
round-trip unchanged. `backend` + `device` label every score so cross-backend
numbers are marked not-comparable (per-platform baselines). The crate exposes
`pub const PROTOCOL_VERSION: u32 = 1;`.

## Invariants (normative, engine-agnostic)

- **Parent-side wall-clock timing.** All timing is benchmarker-side over round trips;
  worker-reported numbers are distrusted. The decode clock starts *before*
  `decode_begin` so speculative setup is charged.
- **Token-IDs-only boundary.** Only token IDs, top-K logits, and RSS cross the wire;
  the harness owns tokenization.
- **Session-discard-on-error.** Any error discards the phase's session state so a
  half-advanced cache can never be reused.
- **Allocator-drain-to-zero at phase start, fail-closed.** At the start of every new
  correctness / prefill / decode sequence the engine resets the phase-start cache
  limit, drains the allocator, and fails closed unless it is verified zero — on Metal
  `Memory.clearCache()` then `Memory.cacheMemory == 0`; on CUDA a
  `cudaDeviceSynchronize` + pool-trim + verified-zero analogue. Not reset during
  `decode_step` / `correctness_step`, which legitimately reuse state created inside
  the charged sequence.
- **No cross-phase memoization / no repeated identical forwards.** Each of prefill,
  decode, and correctness runs in its own worker process, so no model-owned memo
  persists across phases; and no two identical forwards are issued inside one timed
  window (e.g. `decode_begin` runs exactly one seed forward with no warmup, so there
  is no identical predecessor to serve from a memo).
- **Materialized-token / phase-close barrier + completed-work counter.** The returned
  token must be a *materialized* ID (you cannot serialize an argmax you have not
  computed — reading it forces the sync). Every timed phase ends with a
  `phase_diagnostics` barrier plus a monotonic completed-work counter that must equal
  the issued step count; a mismatch fails the run. The barrier does not add timing
  trust — it only makes deferred/async work observable.
- **Allocator drain (#54).** `phase_diagnostics` also reports `cache_memory` — the MLX
  free-buffer cache size at the phase boundary (Swift `Memory.cacheMemory`, read after
  `Memory.clearCache()` in `resetRuntimeWorkerAllocatorForPhaseStart`). Swift fails the
  run CLOSED unless it is exactly `0`; the parent asserts the same on the barrier so
  unscored initialization buffers cannot subsidize the first charged forward. The field
  is back-compat optional: a pre-#54 engine that omits it is not asserted, but any engine
  that reports it MUST report `0`.
- **`decode_step` and `correctness_step` share the same `forward`.** An engine must
  not be able to tell it is being timed; the timed decode path routes through the same
  editable entry points as correctness (no phase-specific decode hook).

## v1.1 additive extension (oracle-verified free-run timed decode)

Protocol **v1.1** (`cudafast-engine/docs/PROTOCOL-v1.1.md`, SIGNED 2026-08-17 incl.
Amendment 4) adds one timed decode regime **additively** — same wire, same envelope,
same determinism rules. `hello.protocol_version` **stays `1`**; the mode is advertised
by a **capability flag** and selected by two new request kinds. A v1-only engine that
omits the capability and never receives these kinds is unaffected; a v1-only line
round-trips byte-identically (every v1.1 field is `skip_serializing_if` / absent ⇒
None). benchd **REFUSES** to issue the `free_decode_*` kinds to an engine that did not
advertise `free_run_decode` (an unadvertised capability is a hard protocol error, never
a silent fallback).

| kind | request fields (in) | response fields (out) | notes |
|------|---------------------|-----------------------|-------|
| `hello` (additive) | — | `capabilities[]` | carries `"free_run_decode"` when the engine implements v1.1; meaningful only on the hello |
| `free_decode_begin` | `seed_tokens[]` | `seed_token` | one seed forward; same contract as `decode_begin`. NOT an `is_timed_step` (Amendment 4) |
| `free_decode_run` | `count` (= N) | `tokens[]` (len N), `acceptance_lengths[]`, `drafted_total`, `accepted_total`, `committed_total` | engine free-runs its own MTP loop, commits N tokens, returns them + AUDIT counters |

- **Timing / oracle (§2.2, §2.7):** the decode clock starts before `free_decode_begin`;
  benchd exact-matches **every committed token** against `expected_decode_tokens[i]` — a
  single wrong free-run token is a HARD failure (`TokenMismatch`).
- **Consistency TRIPLE (§2.6):** at the phase-close barrier benchd enforces
  `R == acceptance_lengths.len()`, `sum(acceptance_lengths) == N`, and
  `completed_work == R + 1` (seed forward + R MTP verify rounds), plus
  `committed_total == N == tokens.len()`. A doctored acceptance histogram is internally
  falsifiable and fails closed.
- **AUDIT (§3):** `acceptance_lengths` + the `*_total` counters are engine-self-reported
  and **never scored** (same posture as `peak_ram_gb`); the benchd wall clock is the only
  score input. Derived `audit_spec_*` metrics are computed benchd-side.
- **Comparability (§5):** a v1.1 free-run score is a NEW SERIES (`timed_mode =
  "free_run_v1_1"`) and MUST NEVER be compared to a v1 teacher-forced number.

## Schema

The machine-readable contract is
[`schema/engine-protocol-v1.schema.json`](schema/engine-protocol-v1.schema.json)
(JSON Schema Draft 2020-12), modeling request and response as a top-level `oneOf`
over `$defs`. It is embedded into the crate as `bench_protocol::JSON_SCHEMA`.
