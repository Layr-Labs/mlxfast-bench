# DRAFT — Per-module speculative configuration (MTP / DFlash / DSpark)

> **STATUS: DRAFT for Fable review → David sign-off. Not implemented.**
> Ranked-side claims herein are UNVERIFIED(B-4). Repo facts cited at `mlxfast-bench@3fbca3a`.
>
> **Ruling (David, 2026-08-19): the depth setting is coded into each module.** There is no
> global depth. Each speculative path owns its configuration block — schema, defaults,
> bounds — and the benchmarker treats the block as opaque module input, sealed only as
> echoed back by the engine.

## 1. Why

- Review finding **R1**: `measure-job` seals `serial_depth: 0, candidate_depth: N` while
  neither leg ever configures a depth — provenance describes a run that never happened
  (`crates/benchctl/src/measure_job.rs:48,369`). The point-fix (plumb a `--depth` flag)
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
  fields → the module's coded default (MTP: depth 2 per the Option A naive baseline;
  DFlash: the module's block-size-derived default). The benchmarker never interprets
  block contents — it forwards bytes.
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
| **v1.1 free-run mode** | orthogonal; unchanged | scoring any of the three still requires v1.1 (mlxfast-bench#100) |

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
