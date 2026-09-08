# Per-module speculative configuration (MTP / DFlash / DSpark)

> **STATUS: NORMATIVE — signed off and IMPLEMENTED.** This document is the definition of the
> `spec` / `effective_spec` wire surface. It is cited as the contract by the frozen Engine
> Protocol v1 JSON Schema (`crates/bench-protocol/schema/engine-protocol-v1.schema.json`) and
> by 14 sites across `bench-protocol` and `bench-runner`; a change here is a wire change.
> Ranked-side operational claims remain UNVERIFIED(B-4). Repo facts cited at
> `mlxfast-bench@3fbca3a`.
>
> **Ruling (David, 2026-08-19): the depth setting is coded into each module.** There is no
> global depth. Each speculative path owns its configuration block — schema, defaults,
> bounds — and the benchmarker treats the block as opaque module input, sealed only as
> echoed back by the engine.

## 1. Why

- Review finding **R1**: `measure-job` seals `serial_depth: 0, candidate_depth: N` while
  neither leg ever configures a depth — provenance describes a run that never happened
  (`crates/benchd/src/measure_job.rs:48,369`). The point-fix (plumb a `--depth` flag)
  would harden a design error: depth is not a benchmarker concept; it is a property of a
  specific drafter module.
- David direction (2026-08-19): "better configuration for dspark, dflash, and mtp
  working with this" — one surface that configures all three paths across contract,
  CLI, wire, and both engines.

## 2. The config object — a tagged union, one block per module

```jsonc
// speculative config ("spec"): exactly one mode, config nested UNDER the mode key
{ "mode": "serial" }

{ "mode": "mtp",    "mtp":    { "depth": 2 } }

{ "mode": "dflash", "dflash": { "depth": 4,
                                "draft": { "artifact": "…", "sha256": "…" } } }

{ "mode": "dspark", "dspark": { /* RESERVED — schema pending cudafast#26 ruling
                                   (port-to-Qwen vs DeepSeek-only) */ } }
```

Rules:

- **Depth is a module field.** `mtp.depth` and `dflash.depth` are separate fields with
  separate defaults and bounds, owned by their modules. `serial` has no depth. `dspark`
  defines its own if/when #26 rules it in.
- **Module owns validation.** The engine-side module for each mode parses its own block:
  unknown fields → error (deny_unknown_fields, matching bench-protocol posture); missing
  fields → the module's coded default (MTP: the envelope's own resolution/clamp;
  DFlash: the module's block-size-derived default). The benchmarker never interprets
  block contents — it forwards bytes. Benchd itself supplies NO depth default (David
  ruling 2026-08-27): an omitted `--mtp-depth` sends `{"mode":"mtp","mtp":{}}` and the
  echo reports the module-resolved depth. (The pre-ruling text here said "MTP: depth 2
  per the Option A naive baseline" — that was benchd's retired injected default.)
- **Cross-module keys are rejected.** `{"mode":"mtp","dflash":{…}}` is an error, not
  ignored — fail-closed against config drift.
- **Artifacts are pinned.** Any module block that names an external artifact (DFlash
  draft gguf) carries a mandatory `sha256`; the engine verifies before load (same
  posture as golden pins).

## 3. Where the block lives, seam by seam

| Surface | Change | Notes |
|---|---|---|
| **Contract / track fixture** | `speculative: { candidate: <spec>, baseline: <spec> }` | `baseline` defaults to `{"mode":"serial"}`. Ranked shape UNVERIFIED(B-4). |
| **measure-job CLI** | reads from `--contract`; `--candidate-spec / --baseline-spec <json>` as explicit overrides (recorded as `spec_source: "cli-override"` in provenance) | replaces the R1 point-fix; no `--depth` flag anywhere |
| **Wire (Engine Protocol v1, additive)** | `decode_begin` request gains optional `spec` object; the **response echoes `effective_spec`** — the module-parsed, default-filled block the engine will actually run. `hello` gains optional `spec_modes: ["serial","mtp",…]` capability list. | Additive like `cache_memory`; absent `spec` = engine default (v1 engines unchanged/valid). Unsupported mode → error + session discard (fail-closed). |
| **results.json provenance** | seals **only the echoed `effective_spec`** per leg, never the requested config | this is the R1 class-closure: provenance is what the engine acknowledged, not what the caller asked |
| **score.json `metrics.per_prompt`** | the single-leg official path AND the local iterate/submit paths seal one record for each timed prompt they measured: `prompt_sha256`, `effective_mean_draft_len` (the free-run audit value, step 3), `mtp_seconds_per_token_mean` (the same enforced decode seconds-per-token the score uses), the per-prompt `spec_rounds` / `spec_drafted_total` / `spec_accepted_total`, and `head_provenance_sha256` | additive and audit-only; the key is absent when the run measured no timed prompt |
| **`benchd iterate` CLI** | `--mtp-depth <N>` builds `{"mode":"mtp","mtp":{"depth":N}}` for the TIMED free-run decode window; `--candidate-spec <json>` is the explicit override. The two are mutually exclusive, and the depth obeys the same cap as measure-job | ABSENT = no `spec` on the wire = the engine's default (serial). The teacher-forced correctness gate never carries a spec |
| **score.json spec + identity seal** | `effective_spec_mode`, `effective_spec_depth`, `spec_rounds`, `spec_drafted_total`, `spec_accepted_total`, `spec_acceptance_rate`, `acceptance_lengths`, `engine_backend`, `engine_device`, `engine_protocol_version`, `head_provenance_sha256` | additive and audit-only; every key is omitted when unset, so a run that produces none seals the historical bytes |
| **v1.1 free-run mode** | orthogonal; unchanged | scoring any of the three still requires v1.1 (mlxfast-bench#100) |

### 3.1 The `score.json` speculative-decode and engine-identity keys

The sealed `score.json` records what the timed leg ran. `effective_spec_mode` and
`effective_spec_depth` come from the engine's own `effective_spec` echo, which benchd has already
checked against its request; a leg that sent no `spec` seals `serial` and depth `0`, because the
protocol defines an absent `spec` as the engine's default path. A leg that drafted also seals its
acceptance facts: `spec_rounds` is the verify-round count R, `spec_drafted_total` and
`spec_accepted_total` are the engine's own draft counters, `spec_acceptance_rate` is
accepted/drafted, and `acceptance_lengths` is the per-round committed-token histogram, kept
verbatim (one entry per round, thus at most as many entries as the decode window has tokens). A
serial leg drafts nothing, so it seals none of those five keys: its counters are always zero and
its histogram is always all ones. `spec_acceptance_rate` is also absent when nothing was drafted,
because a zero denominator has no rate. The `engine_backend`, `engine_device`,
`engine_protocol_version` and `head_provenance_sha256` keys hold the identity the TIMED worker
announced in its `hello` — the worker whose leg the score measured. Each of these keys is written
only when it has a value, so a run that produces none seals exactly the bytes it sealed before
these keys existed. None of them is an input to any score, speedup, floor or band.

## 4. Engine-side module mapping

- **mlxfast-engine (Metal):** the MTP driver module consumes `mtp.depth` (Swift-side
  clamp `depth_max.min(ctx − pos − 2)` stays module-internal). `dflash`/`dspark` modes
  → capability-absent → wire error (correct: Metal has no such modules today).
- **cudafast-engine (adapter → Pulsar):** the adapter translates block → module
  activation: `mtp` → nextn path with the block's depth; `dflash` → `PULSAR_DFLASH` +
  sha-verified draft gguf (enablement: cudafast#25); `dspark` → reserved pending
  cudafast#26. Translation lives in the adapter (`harness/protocol-adapter/`), NOT in
  vendored `engine/` — the fork stays clean.

## 4.5 Stub modules + a drafting-model slot for each path (David, 2026-08-19)

Every engine ships **one module per mode — implemented or STUB** — so the structure
exists everywhere and enablement is fill-in, never restructure:

- **Module table is total.** Both engines register `serial`, `mtp`, `dflash`, `dspark`
  modules. A stub module still **parses and validates its full config block** (so schema
  enforcement is uniform across engines) and then fails closed with a distinct
  `mode not implemented on this engine` wire error. `hello.spec_modes` lists only
  *runnable* modes — stubs are visible in code, never in capability claims.
- **Drafting-model slot per module.** Each module — stub included — declares its
  drafting-model manifest entry: `mtp` → the native head (Metal: fork weights; CUDA:
  the reference bf16 MTP head, 15 tensors under `mtp.`); `dflash` → draft gguf
  (artifact + sha256); `dspark` → reserved slot, schema TBD by cudafast#26. The slot
  defines where the artifact mounts, what pins it, and what the module loads — filling
  the slot + deleting the stub error IS the enablement task (cudafast#25 for DFlash).
- **Stubs are contestant-visible structure.** On the CUDA side the module table sits in
  the adapter (`harness/`, pinned), while the modules' engine hooks live in `engine/`
  (editable) — a contestant improving a path edits the module body, never the table.
- **The submission picks which module to import (David, 2026-08-19).** The candidate
  workspace declares its chosen path in a manifest field —
  `speculative: { "mode": "dflash", "dflash": { … } }` — and measure-job takes that as
  the candidate-leg spec: the contestant selects the module (and supplies its drafting
  model, BYO per the declared-head policy precedent); the track contract's role shrinks
  to (a) the **allowed-modes list** for the track and (b) the baseline spec (serial).
  A submission declaring a mode outside the track's allowed list, or one the engine
  reports as stub/unsupported, REJECTS before any timed work. Provenance seals the
  submission-declared spec, the track constraint it satisfied, and the engine-echoed
  `effective_spec` — three fields, so a mismatch anywhere is visible in the artifact.

## 5. What this deliberately does not do

- No global depth, no benchmarker-side interpretation of module blocks.
- No scoring change: mode selection never alters the regime; comparability rules
  (v1.1 judge-less series) unchanged.
- No ranked-wrapper claim: whether the organizer contract carries `speculative` blocks
  is UNVERIFIED(B-4); until then this is the local + Spark surface, and measure-job's
  contract parser treats an absent `speculative` block as serial-vs-engine-default with
  provenance saying exactly that.

## 6. Acceptance

1. Unit: per-module parse/default/bounds tests; cross-module-key rejection; unknown-mode
   fail-closed; artifact-pin mismatch fail-closed.
2. Conformance kit: `decode_begin.spec` echo semantics + capability gating, positive and
   negative controls, mock engine.
3. measure-job: sealed `effective_spec` matches the engine echo byte-for-byte on both
   legs; a leg whose echo differs from request records `spec_source` divergence and the
   run REJECTS (no silent fallback).
