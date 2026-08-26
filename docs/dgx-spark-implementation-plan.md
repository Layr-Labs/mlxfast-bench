# Benchmark split, realized on DGX Spark

**Implementation plan — companion to [`architecture.md`](architecture.md)**

A concrete, phased build plan: the Rust benchmarker (`mlxfast-bench`), the extracted
Metal engine (`mlxfast-engine`), and a new CUDA engine (`cudafast-engine`) on NVIDIA
DGX Spark (GB10, ARM64, unified LPDDR5X) — with Yukon platform integration and
per-box deployment.

> **Orientation.** The architecture doc argued *what* to build and *why* it's safe.
> This plan is *how* and *in what order*, pinned to DGX Spark. Everything below
> assumes the frozen Engine Protocol v1, the five-function seam, and the golden
> conformance kit from that doc.

---

## 0. DGX Spark: what the hardware dictates

Four GB10 facts drive concrete decisions. None are cosmetic — each changes code,
config, or baselines.

| GB10 fact | value | consequence for this build |
|---|---|---|
| CPU ISA | ARM64 (20-core: 10× X925 + 10× A725) | Every image and binary is `aarch64` — **and so is the M5 fleet**, so there is one Rust build story for the whole system. Linux target `aarch64-unknown-linux-gnu` for benchd/CUDA; macOS `aarch64-apple-darwin` for bench-agent. Build host: the **`ai-server` M5** (native aarch64) for Rust builds; CUDA base images `nvcr.io/nvidia/cuda:…-arm64`. |
| Unified memory | 128 GB LPDDR5X, **273 GB/s**, coherent (NVLink-C2C) | Decode score is memory-bandwidth-bound. 273 GB/s is *below* an M5 Max, so baselines **must be measured on Spark** — never carried from Apple. Coherent unified memory maps cleanly to the existing `bandwidth_source = "ram_resident_model"` metric. |
| Tensor cores | Blackwell 5th-gen, native FP4/FP6 (~1 PFLOP sparse FP4) | The CUDA *performance* quant target is **NVFP4**. The MLX checkpoint's int4 affine-group-64 format does not transfer byte-wise; parity is at the logits/golden level, so `cudafast-engine` emits its own quant. |
| Form factor | single desktop box, DGX OS (Ubuntu-based) | Per-box deployment, not a rack. Enables a reproducibility win Apple lacks: **lock GPU clocks** (`nvidia-smi -lgc`) for deterministic timing instead of relying only on thermal gating. |

> **Bandwidth ceiling, concretely.** Decode ≈ (resident bytes touched per token) ÷
> 273 GB/s. For a dense 27B 4-bit read (~13.5 GB) that's a ~50 ms/token floor (~20
> tok/s). But this model is MoE with expert routing (`expert_stats`/`expert_*`
> counters exist) — active bytes/token are a fraction of the full weights, so the
> real ceiling is higher. *Measure bytes-touched/token on Spark first*; don't assume
> it from parameter count. This number sets the baseline and the plausibility bound
> for every submitted speedup.

---

## 1. Repos & workspace

Three git repos, replacing the single coupled challenge repo.

```
mlxfast-bench/            # Rust workspace — the trusted benchmarker (Deliverable 1)
  Cargo.toml              # [workspace] members below; resolver = "2"
  crates/
    bench-protocol/       # Engine Protocol v1: wire types + JSON Schema (normative)
    bench-core/           # golden schema · score formula · floors · bands · sealing
                          #   + the engine conformance kit
    bench-runner/         # engine lifecycle · parent-side timing · phase barriers
    bench-transform/      # safetensors staging + validation; per-target quant emit
    bench-telemetry/      # provider trait: nvml (Spark) | macmon (M5)
    bench-agent/          # native M5 timing peer (aarch64 macOS) — no-op on Linux
    benchctl/             # CLI: setup · transform · iterate · submit · official
  deploy/
    Dockerfile.benchd     # distroless arm64; cargo build --locked
    compose.dgx.yml       # benchd + engine on an internal network
  docs/                   # architecture.md + this plan
  targets/                # signed target.toml bundles (one per model×platform)

mlxfast-engine/           # Swift package — extracted Metal engine (Deliverable 2)
                          #   MLXFastModel/** + worker server half + portable core

cudafast-engine/          # Ubuntu/CUDA engine (Deliverable 3)
  harness/                # PINNED: protocol loop · phase barriers · allocator drain
  engine/                 # EDITABLE submission surface (the CUDA editablePaths)
  Dockerfile              # nvcr.io/nvidia/cuda:13.x-devel-…-arm64 base
```

Rationale: `bench-protocol` is the one normative schema both engines conform to;
`bench-agent` lives in the same workspace so the M5 timing peer shares the protocol
types exactly. `cudafast-engine` mirrors `mlxfast-engine`'s harness/editable split
so the same static-review gate applies.

---

## 2. Phase plan

Sequenced so each phase has a hard acceptance gate and nothing downstream starts on
an unproven upstream. Phases 1–2 are hardware-agnostic; 3+ are on Spark.

| Phase | Deliverable | Where | Acceptance gate |
|---|---|---|---|
| **0 · Freeze the seam** | `protocol_version`/`backend`/`device` in `hello`; `PROTOCOL.md` + JSON Schema; split `QwenRuntimeWorker.swift` client↔server; split `Constants.swift`; push + pin `mlx-swift-lm` fork | current repo, small diffs | current `benchmark.sh` still green on M5 with split targets; schema validates live worker traffic |
| **1 · Rust workspace to scoring parity** | `bench-protocol` + `bench-core` with property tests ported from Swift; conformance-kit skeleton; `benchctl iterate` driving the existing Swift engine over stdio | hardware-agnostic | `benchctl` score fields byte-match `benchmark.sh --local-iterate` and `--local-submit` on identical weights+golden |
| **2 · Extract Metal engine + benchd parity** | ship `mlxfast-engine` as pinned release (binary + metallib); `bench-agent` owns native timing; reconstruct the `/opt` measure-job thermal/paired contract into `bench-runner` | M5 | paired baseline+candidate reproduces today's official numbers within noise; conformance kit green on Metal |
| **3 · CUDA correctness reference** | `cudafast-engine` at the five-function seam, correctness-first stack, bf16 (dequantized) transform target | DGX Spark | conformance kit green on Spark — anchors within `max_expected_rank`/`max_top_logit_delta`, free-run `exact_prefix` match |
| **4 · CUDA performance engine + baselines** | NVFP4 quant target; fused GatedDeltaNet + attention kernels; NVML telemetry (clock-lock, thermal gate, util quiescence); write `qwen36-27b.dgx-spark.toml` | DGX Spark | timed runs stable across repeats under clock-lock; baseline recorded; bytes-touched/token measured; still conformance-green |
| **5 · Security + official pipeline** | rootless podman + CDI GPU under the two-user model; throwaway build container; `benchctl official`; janitor + quarantine; retire `benchmark.sh` + jq copies | DGX Spark | a submission cannot read the golden, reach the network, or write outside scratch (red-team probe passes); official run produces a sealed, signed score |
| **6 · Yukon integration** | runner-provider wiring, score ingestion, one-command new-contest path via `target.toml` (see §6) | platform | Yukon dispatches → benchd runs → `score.json` artifact ingested and ranked |

---

## 3. DGX Spark environment bring-up

Phase-3 prerequisite. Concrete, in order. Pin exact versions on first run and record
them in the target file.

1. **Base OS & driver.** DGX OS (Ubuntu 22.04/24.04 aarch64). Confirm the GB10
   driver + CUDA runtime: `nvidia-smi`, `nvcc --version` (expect CUDA 13.x). Record
   both in the target.
2. **Container runtime.** Install `podman` + `nvidia-container-toolkit`; generate a
   CDI spec: `sudo nvidia-ctk cdi generate --output=/etc/cdi/nvidia.yaml`. Verify:
   `podman run --rm --device nvidia.com/gpu=all nvcr.io/nvidia/cuda:13.x-base-…-arm64 nvidia-smi`.
3. **Two service users** (§7): `bench-runner` (ring 0, *not* in any
   container-admin group, runs rootless podman) and `bench-engine` (owns
   engine/build containers, subordinate uid/gid range for userns remap:
   `/etc/subuid`, `/etc/subgid`).
4. **Clock & thermal control.** Confirm NVML clock-lock works:
   `sudo nvidia-smi -lgc <min>,<max>` and query
   `nvidia-smi --query-gpu=clocks.sm,temperature.gpu,utilization.gpu --format=csv`.
   This is the reproducibility spine of Spark timing.
5. **Weights + goldens stores.** Ring-0-owned dirs; goldens are runtime read-only
   mounts, never in an image (architecture.md §8 cycle 4).

```bash
# smoke test the full path once the engine image exists
podman run --rm \
  --device nvidia.com/gpu=all \
  --network none --read-only \
  -v weights:/weights:ro --tmpfs /scratch \
  --cap-drop ALL --security-opt no-new-privileges \
  cudafast-engine:dev  runtime-worker --weights /weights   # expect: hello{backend:"cuda",device:"gb10"}
```

---

## 4. The CUDA engine — stack & quant decisions

### Two engines, one seam

Split the problem the way the seam already invites: a **correctness reference**
(Phase 3, optimize nothing) and a **performance engine** (Phase 4). They implement
the same five functions; only the kernels differ.

| concern | Phase 3 · correctness reference | Phase 4 · performance engine |
|---|---|---|
| stack | PyTorch + HF `transformers` Qwen3-Next (GatedDeltaNet supported upstream); or candle/cudarc if Python per-step overhead bites | Fused CUDA kernels behind the same seam; cutlass/cuDNN for attention, custom for gated-delta recurrence |
| weights | bf16, dequantized from the MLX int4 checkpoint (parity is at logits, not bytes) | NVFP4 (Blackwell-native), group scales; re-quantized from the fp16 source |
| goal | pass the conformance kit — nothing else | beat the Spark baseline under clock-lock, still conformance-green |

> **Decision point.** The correctness reference's stack (PyTorch vs candle) is the
> one open engineering choice. PyTorch reaches parity fastest but puts an interpreter
> in the per-token path; since timing is per-token wall clock, that overhead is real
> for the *reference* baseline. Recommendation: PyTorch for Phase-3 gate-passing,
> then make the *performance* engine (and thus the ranked baseline) native so Python
> never sits in a scored path. Confirm before Phase 3.

### The seam, on CUDA

Implement exactly the five entry points from architecture.md: `load` (validate
config + tensor inventory, fail before `hello`), `Session::new` (48 recurrent + 16
KV caches, same topology), `forward(tokens, pos_offset) → logits` (the one hot path;
`decode_step` and `correctness_step` must route through it identically),
`argmax`/`top_k(8)` with the shared canonical tie-break (lowest token id), and the
`MaterializedTensor → device tensor` quant choke point. Allocator drain analogue for
the phase barrier: `cudaDeviceSynchronize` + caching-allocator trim + a verified-zero
query (the CUDA counterpart to MLX's `cacheMemory == 0`).

---

## 5. Baselines, timing & reproducibility on Spark

- **Clock-lock every scored run.** Lock SM + memory clocks via NVML before baseline
  and candidate alike; record the locked frequencies in the score. This replaces
  Apple's thermal-variance fight with a deterministic clock, and makes the paired
  baseline+candidate genuinely comparable.
- **Keep the paired contract.** Baseline-first then candidate, each behind the same
  fixed gate (now: clock-lock + a temperature ceiling + pre-run util quiescence via
  NVML), one gated retry, calibration band — the measure-job semantics reconstructed
  in `bench-runner` in Phase 2, re-parameterized for NVML in Phase 4.
- **Measure bytes-touched/token** on Spark to set `bandwidth_gb_per_token` honestly
  for the MoE routing, and to bound plausible speedups.
- **Per-platform baselines are not comparable.** The Spark target's floor is
  meaningful only against a Spark reference; scores carry `backend`/`device` so
  cross-platform numbers are labelled, never ranked together.

---

## 6. Yukon platform integration

Yukon is a TypeScript/Bun monorepo. It selects an **execution provider** from
`benchmark.json`'s `runner.provider`. For mlxfast that's `github-actions`, and under
that provider Yukon's involvement is deliberately thin — which is exactly the "the
hook is just YAML" intuition, confirmed:

> **What Yukon actually does under `github-actions`:** pushes the submission branch,
> fires `workflow_dispatch` on `runner.workflow`, polls the run, then downloads the
> run artifact and reads a file named exactly `scorePath` (`score.json`). It does
> *not* run `setupCommand`, `benchmarkCommand`, or `preSubmitCommand` — those are
> challenge-side convention consumed by our harness, invisible to Yukon. The entire
> contract Yukon relies on is: `editablePaths`, `direction`, and a parseable
> `score.json` with a finite `score`.

### Option A — keep `github-actions`, swap hardware + harness (zero Yukon code change) · recommended

This is the "just YAML" path and the right default. Nothing in the Yukon repo
changes.

1. Register a **self-hosted GitHub Actions runner on the DGX Spark** with a new
   label — `runs-on: [self-hosted, dgx-spark]`. (In Yukon terms this is still the
   `github-actions` provider; "self-hosted" is a runner label, not a provider.)
2. Author a new workflow (the hook) that Yukon dispatches: check out the ref, overlay
   the submitted `editablePaths`, run **`benchctl official`** instead of
   `./benchmark.sh --official`, and upload `score.json` via `actions/upload-artifact`
   — same artifact/path contract the M5 workflow uses today.
3. Keep `runner.provider = "github-actions"`, `runner.workflow = "<new>.yml"`,
   `scorePath = "score.json"` in `benchmark.json`. Point `setupCommand`/
   `benchmarkCommand` at `benchctl` too, so local `yukon run` iterate loops work
   against the same binary.

Net: the DGX migration is a *workflow + harness* change, not a platform change.
benchd emits the same score artifact benchmark.sh does; Yukon can't tell the
difference. This keeps the promotion/PR path (only `github-actions` opens submission
branches) working for free.

### Option B — a native `dgx-spark` provider (Yukon drives the box directly)

Only if you want to drop GitHub Actions entirely. More surface, and you take on the
promotion path yourself. Three edits in the Yukon repo:

1. `src/benchmark/manifest.ts` — add a `{ provider: "dgx-spark", … }` variant to
   `BenchmarkRunnerSchema` (the provider enum is closed today: only `github-actions`
   + `frontiercs`).
2. `src/execution/` — new `ExecutionProvider` impl (`runBaseline`,
   `validateSubmission`, `publishAcceptedSubmission`) that ships the archive to the
   Spark box, invokes `benchd`, and returns a `BenchmarkScore` via the existing
   `parseBenchmarkScore`.
3. `src/cli/yukond.ts` — register it in `createExecutionProvider()`'s routing map
   behind an env toggle. **Also implement `publishAcceptedSubmission`** (or delegate
   to the GitHub gateway), since only the GHA provider currently lands accepted
   `editablePaths` on the benchmark branch.

### The score.json contract benchd must emit (both options)

```jsonc
{
  "score": <finite number>,          // required; ranked per `direction` ("+" = higher better)
  "scoreRatioUnbounded": <number>,   // optional; used for ranking/display when present
  "metrics": { }                     // optional JSON (our full ScoreMetrics);
}                                     //   per-case output/msg stripped for payload size
```

Yukon parses with `ScoreFileSchema` (`.passthrough()`, so our extra sealed fields
survive), writes `officialScore`/`officialMetrics`, runs `evaluateImprovement`
against `minScoreImprovementBips`, and on improvement promotes. The CLI prepends a
`Model:` line to the submission note that the rewards leaderboard parses for
attribution — preserve that if we drive submissions programmatically. Auth is a
Yukon API key (`Authorization: Bearer`) or Supabase JWT; GitHub creds live only in
Yukon for dispatch — the Spark box never sees them (it holds only goldens + the
semantic-judge key, exactly as the M5 does today).

> **Recommendation:** ship Option A for the DGX cutover (Phase 6) — smallest safe
> change, keeps promotion intact. Revisit Option B only if GitHub Actions dispatch
> becomes a bottleneck or you want Yukon-native scheduling across a Spark fleet.

---

## 7. Security realization on Spark

The architecture doc's ring model, in concrete Linux primitives:

| ring | Spark realization |
|---|---|
| Ring 0 — orchestration, secrets, sealing | `bench-runner` user runs benchd via **rootless podman**; not in any container-admin group; owns goldens/scores/telemetry binaries; holds secrets only in `official` mode via a 0600 mount. |
| Ring 2 — the untrusted engine | `bench-engine` user with a subordinate uid/gid range (**userns remap**); engine container is `--network none`, `--read-only`, `--cap-drop ALL`, `--security-opt no-new-privileges`, default seccomp, pids/memory limits, GPU via CDI device. |
| Build of submitted code | Throwaway `--network none` build container under `bench-engine`'s range; vendored/locked deps; only the built artifact handed forward. |
| Post-run | `bench-runner`-owned janitor unit destroys per-run containers + volumes; host quarantine flag checked by ring-0 preflight. |

**The three rules from the architecture doc hold literally here:** benchd never
mounts a container-control socket (it only connects to engines; podman/quadlet units
own lifecycle); the golden is never in the engine's mount namespace; telemetry
binaries stay ring-0-owned since a gamed clock/thermal reading is a scoring attack.

**Open item to validate early:** NVIDIA CDI GPU injection under rootless podman +
userns remap — device-node ownership can fight the remap. Fallback: rootful engine
containers under `bench-engine` with daemon-side userns remap, preserving the
two-user separation.

---

## 8. First-week concrete tasks & risks

### Do first (unblock everything)

1. Push + pin the `mlx-swift-lm` fork — the hard blocker for extracting the Metal
   engine (Phase 0).
2. Stand up one DGX Spark per §3 steps 1–4; record driver/CUDA/clock capabilities
   into a first `dgx-spark.toml` stub.
3. Scaffold `mlxfast-bench` with `bench-protocol` + the JSON Schema; validate it
   against captured live worker traffic from the current M5.
4. Begin reconstructing the `/opt` measure-job thermal/paired contract from the live
   boxes — long-pole, start immediately.

### Top risks (hardware-specific)

- **measure-job reconstruction** — the paired/thermal/retry logic isn't in the repo;
  it gates Phase 2 and Phase 4.
- **MoE bytes-touched/token unknown on Spark** — sets baselines and speedup
  plausibility; measure before promising numbers.
- **Rootless GPU + userns** — CDI/remap interaction; has a rootful fallback but
  validate during §3 bring-up.
- **NVFP4 parity** — the performance quant must still clear the conformance
  tolerances; keep the bf16 reference as the parity anchor.
- **Correctness-reference stack choice** — resolve PyTorch-vs-native before Phase 3
  to avoid re-baselining later.
