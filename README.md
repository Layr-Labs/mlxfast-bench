# mlxfast-bench

The trusted, reproducible benchmarker — a Rust workspace shipped as a pinned Docker
image — for the MLXFast inference challenge. It replaces the coupled
`benchmark.sh` + Swift harness with a single service that drives interchangeable
engines behind a frozen wire protocol: `mlxfast-engine` (Swift/MLX, raw on Apple
M5) and `cudafast-engine` (CUDA, Ubuntu containers on NVIDIA DGX Spark).

This repo is the **hub**: it owns the normative Engine Protocol v1, the scoring
core, and the design docs for the whole split.

## Design docs (source of truth)

- [docs/architecture.md](docs/architecture.md) — the split: current-vs-target
  design, Engine Protocol v1, engine-consistency layers (MLX ↔ CUDA), the
  privilege/ring security model, and six red/green teaming cycles.
- [docs/dgx-spark-implementation-plan.md](docs/dgx-spark-implementation-plan.md) —
  concrete phased build plan on DGX Spark (GB10, ARM64, NVFP4), environment
  bring-up, baselines, and Yukon platform integration.
- [docs/execution-plan.md](docs/execution-plan.md) — granular, ticketed work
  breakdown: epics, tasks, dependencies, milestones, and the critical path.
- [docs/dependency-graph.md](docs/dependency-graph.md) — the implementation DAG
  (mermaid), mirrored as native GitHub "blocked by" relationships on the issues.

## Workspace layout

| crate | role |
|-------|------|
| `bench-protocol` | Engine Protocol v1 wire types + JSON Schema (normative) |
| `bench-core` | golden schema · score formula · floors · bands · sealing · conformance kit |
| `bench-runner` | engine lifecycle · parent-side timing · phase barriers · paired baseline |
| `bench-transform` | safetensors staging + validation; per-target quant emit |
| `bench-telemetry` | provider trait: `nvml` (Spark) \| `macmon` (M5) |
| `bench-agent` | native M5 timing peer (aarch64 macOS); no-op on Linux |
| `benchctl` | CLI: `setup` · `transform` · `iterate` · `submit` · `official` |

`deploy/` holds the benchd Dockerfile and the DGX compose sketch; `targets/` holds
the signed per-(model,platform) `target.toml` bundles.

## Status

Shipped. These crates are the benchmarker — ~34.4k lines of Rust across the seven
crates, driving live ranked windows. **Measurement and scoring live here, not in the
engine repo**: `benchctl measure-job` runs the paired timing and seals `results.json`,
and the A-3 overlay computes the published score over it. An engine reports raw
profiling only. `scripts/benchmark.sh` is the gitlink target the engine repo's
`benchmark.json` invokes.

Two `TODO(phase-N)` markers remain, and nothing else claims to be a stub:
`bench-runner/src/lib.rs:15` (phase-2: the paired flow's module-level note) and
`deploy/Dockerfile.benchd:3` (phase-5: multi-stage build, sign + publish by digest).

Build host is the `ai-server` M5 (native aarch64); the same Rust target serves both
M5 and DGX Spark.

## Publishing `dist/`

Consumers do not build `benchctl`. They download `dist/benchctl` by pin and verify
its sha256. To publish, run `./scripts/build-dist.sh`, then `git add -f dist` and
commit. Enable the pre-commit hook once per clone with `git config core.hooksPath
.githooks`; it rebuilds and byte-compares any staged `dist/benchctl` and refuses a
commit that the current source does not produce.

## Related repos

- [`mlxfast-engine`](../mlxfast-engine) — the extracted Swift/MLX Metal engine.
- [`cudafast-engine`](../cudafast-engine) — the new CUDA engine for DGX Spark.
