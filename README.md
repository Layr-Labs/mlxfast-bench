# mlxfast-bench

The trusted, reproducible benchmarker — a Rust workspace — for the MLXFast inference
challenge. It replaces the coupled `benchmark.sh` + Swift harness with one component
that drives interchangeable engines behind a frozen wire protocol: a Swift/MLX engine
raw on Apple M5, and `cudafast-engine` (CUDA) on the ruled RTX PRO 6000 Blackwell class
box.

This repo owns the normative Engine Protocol v1, the scoring core, and the measurement
logic. It is consumed by each track's engine repo as a SHA-pinned `benchd` submodule.

## Documentation

Start at [docs/README.md](docs/README.md) — the index, with each document's class
(normative contract · runbook · architecture · history) and the citation rule.

The two entry points:

- [docs/architecture.md](docs/architecture.md) — the split: the target design, Engine
  Protocol v1, engine-consistency layers (MLX ↔ CUDA), the privilege/ring security
  model, and six red/green teaming cycles.
- [docs/track-release-branches.md](docs/track-release-branches.md) — how model tracks
  bind to this repo: benchd is developed and published from `main`, the track id is the
  platform namespace and the R2 key prefix, and how an engine repo resolves benchd.

Superseded planning material lives under [docs/history/](docs/history/) and is retained
as a record, not as guidance.

## Branches, tracks and the dist channel

For the Qwen 3.8 125B-A6B project, a **branch is a project** and a **track id is a
platform namespace**. They are not the same string.

| Thing | Value | Rule |
|---|---|---|
| project branch | `qwen3.8-125b-a6b-v1` | ONE branch serves both engines (MLX and CUDA). |
| track id | `qwen3.8-125b-a6b-mlx-v1`, `qwen3.8-125b-a6b-cuda-v1` | The platform token before `-v{N}` keys every platform fact (`bench_core::constants::Platform`). The R2 prefix is the track id. |
| dist channel `branch` field | `qwen3.8-125b-a6b-v1` | The channel manifest names the PROJECT branch, never a track id. One channel carries one binary for each platform; the platform is keyed by directory, not by branch. |

A run resolves its platform from the track id it declares (`--contract` `track_id` or
`MLXFAST_QWEN_MTP_TRACK_ID`). Nothing in the code reads a branch name.

## Workspace layout

| crate | role | state |
|-------|------|-------|
| `bench-protocol` | Engine Protocol v1 wire types + JSON Schema (normative) | live |
| `bench-core` | golden schema · score formula · floors · bands · sealing · conformance kit | live |
| `bench-runner` | engine lifecycle · parent-side timing · phase barriers · paired baseline | live |
| `bench-telemetry` | telemetry provider trait + the **macmon** (M5) provider | live; the `nvml` provider is **not written yet** |
| `bench-transform` | safetensors staging + validation; per-target quant emit | **PLACEHOLDER** — a doc comment, no code. Weight transform still happens in the harness. |
| `bench-agent` | native M5 timing peer (aarch64 macOS); no-op on Linux | **PLACEHOLDER** — a `main` that prints "scaffold" |
| `benchd` | the CLI; everything below runs through it | live |

### `benchd` subcommands

Implemented: `iterate` (engine end-to-end → sealed `score.json`, with the
`--mode official` / local-iterate / local-submit flows, plus the
`--capture-baseline` calibration mode), `official` (alias for
`iterate --mode official`), `correctness`, `validate-golden`, `validate-weights`,
`parity-diff`, `prefill-decompose`, `harness-hash`, `weights-digest`,
`measure-job` (Option-A seam 2: paired ranked timing → `results.json`) and
`overlay-timing` (Option-A seam 3, LOCAL merge).

Two scored paths coexist. `iterate --mode official` is the ranked path of the
Qwen 3.8 125B-A6B tracks. It is **paired and per box**: one run measures
`official_pairs` pairs — 2 on both platforms — on one box in one job, and each
pair is a serial-control leg on the organizer-staged reference tree followed by
the candidate leg. The score is the live ratio, `prefill_gain ^ 0.25 *
decode_gain ^ 0.75`, at batch size 1 on one stream. These tracks read no stored
baseline pair. The `measure-job` → `overlay-timing` seam is the flow the earlier
tracks score through; it resolves its denominator from the per-track table in
`crates/bench-core/src/constants.rs`, and the 125B tracks never enter it.

Declared but **not implemented**: `transform`, `submit`. Both print
"not implemented in this wave".

`deploy/` holds the benchd Dockerfile (a containerized-deployment stub; the live flows
run `benchd` natively on the box) and `Dockerfile.dist-linux`, the reproducible Linux
aarch64 dist build. `targets/` is where the signed per-(model,platform) `target.toml`
bundles will go — **today it holds only a README**; those values still live in
`crates/bench-core/src/constants.rs` and the track fixtures.

## Status

Shipped and driving live ranked windows — ~44.9k lines of Rust across the seven crates.
**Measurement and scoring live here, not in the engine repo.** On the Qwen 3.8
125B-A6B tracks `benchd iterate --mode official` measures the pairs and seals
`score.json`; on the earlier tracks `benchd measure-job` seals `results.json` and
the A-3 overlay computes the published score over it. An engine reports raw
profiling only. `scripts/benchmark.sh` is the harness root the engine repo's
`benchmark.json` invokes, and its hash is load-bearing.

Known incomplete surfaces, stated plainly:

- `bench-transform` and `bench-agent` are placeholder crates (see the table above).
- `bench-telemetry` ships the macmon provider only; the CUDA/`nvml` provider is unwritten.
- `benchd transform` and `benchd submit` are declared and unimplemented.
- One `TODO(phase-N)` marker remains in the tree: `deploy/Dockerfile.benchd`
  (phase-5: multi-stage build, sign + publish by digest).

Build host is an M5 (native aarch64).

## Publishing `dist/`

Consumers do not build benchd. They download the binaries for their platform and
verify each sha256 against the one manifest beside them.

The channel publishes **two binaries**, from one `source_commit`, for each platform.
The roster is `DIST_BINARIES` in [`scripts/dist-lib.sh`](scripts/dist-lib.sh):

| binary | role |
|---|---|
| `benchd` | the measurement harness every run drives. |
| `record-correctness-golden` | the golden **author**. An engine repo's golden re-author tooling drives it, and it must be the same build that later validates the goldens it writes. Building it on the box from source is the "the box builds its own harness" hole the channel exists to close, so it ships here. |

One directory per platform, holding both binaries and the manifest:

| path | platform | build with |
|---|---|---|
| `dist/benchd`, `dist/record-correctness-golden`, `dist/benchd.manifest.json` | macOS aarch64 (`aarch64-apple-darwin`) | `./scripts/build-dist.sh`, on a Mac |
| `dist/linux-aarch64/` — the same three names | Linux aarch64 (`aarch64-unknown-linux-gnu`) | `./scripts/build-dist-linux.sh`, on any machine with Docker |
| `dist/linux-x86_64/` — the same three names | Linux x86_64 (`x86_64-unknown-linux-gnu`), for a participant's workstation; no ranked box runs it | `BENCHD_DIST_TARGET=x86_64-unknown-linux-gnu ./scripts/build-dist-linux.sh`, on any machine with Docker |

The manifests have the same shape. Publish every platform from the same
`source_commit`, so that one commit describes the whole channel.

```json
{
  "version": "0.0.0",
  "branch": "qwen3.8-125b-a6b-v1",
  "source_commit": "<40 hex>",
  "target_triple": "aarch64-apple-darwin",
  "sha256": "<benchd sha256>",
  "bytes": <benchd bytes>,
  "binaries": {
    "benchd": {"sha256": "<64 hex>", "bytes": <int>},
    "record-correctness-golden": {"sha256": "<64 hex>", "bytes": <int>}
  }
}
```

**The six top-level fields are unchanged and still describe `benchd` alone.** A
fetcher written before the second binary existed reads the same `sha256` and the same
`bytes`, for the same file, out of the new manifest — nothing it parses moved.
`binaries` adds one line per published binary.

**Every per-binary entry is one line, and that is load-bearing.** Consumers parse this
file with anchored, one-key-per-line `sed` (the offline path on a ranked box has a
shell and `shasum` and nothing else). Pretty-printing the nested objects would put a
bare `"sha256":` at the start of a line, an old fetcher's `manifest_field sha256` would
then return three values, and it would refuse. `scripts/test-dist-manifest.sh` holds
that down, with a pretty-printed negative control.

A consumer reads a per-binary entry with the same `sed` style it already uses for the
top-level fields:

```bash
# manifest_binary_field <manifest> <binary name> <sha256|bytes>
manifest_binary_field() {
  sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*{.*\"$3\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",}]*\)\"\{0,1\}.*}[[:space:]]*,\{0,1\}[[:space:]]*\$/\1/p" "$1"
}
```

To publish, run the build, then `git add -f dist` and commit. Enable the
pre-commit hook once for each clone with `git config core.hooksPath .githooks`.
The hook rebuilds the staged macOS binaries — every name in the roster, not only
`benchd` — and refuses a commit that the current source does not produce. The hook
cannot rebuild the Linux set, because the container builds a pushed commit and not the
working tree; for that set it checks that the staged manifest describes every staged
binary.

`scripts/build-dist-linux.sh` fetches `source_commit` by sha in the container
(`deploy/Dockerfile.dist-linux`). Push the commit before you build it, and keep it
reachable from the branch tip after you build it. **Merge a pull request that
carries `dist/` with a merge commit. A squash merge makes `source_commit`
unreachable, and the manifest then makes a claim that no one can check.**

### What the engines read

The Mac paths do not move and the six top-level manifest fields do not move, so
`tools/fetch-benchd.sh` in the MLX engine repo needs no change to keep resolving
`benchd`. An engine that also wants `record-correctness-golden` fetches that name
from the same directory and verifies it against its `binaries` entry; that is an
engine-repo change, and this repo only states what the channel offers.

The CUDA engine runs on Linux aarch64, so its `tools/fetch-benchd.sh` must select
the platform. That engine repo owns the change; this repo only states what the
channel offers. The change has two parts:

1. Read the channel from the platform directory, not from `dist/`:

       ${BASE_URL}/refs/heads/${BRANCH}/dist/linux-aarch64/benchd.manifest.json
       ${BASE_URL}/refs/heads/${BRANCH}/dist/linux-aarch64/benchd

2. Read `target_triple` from the manifest, and refuse a manifest that does not
   name `aarch64-unknown-linux-gnu`. Without this check the Mac pair passes every
   other test, and the box installs a binary that it cannot run.

Nothing else changes. The offline path, `BENCHD_DIST_LOCAL`, and the sha256 and
bytes checks are the same for the two platforms.

The CUDA engine already resolves the channel and already selects its platform
directory; `benchd.pin` was removed there, so nothing pins one binary any more.

## Related repos

- [`mlxfast-qwen-38-27b-mtp-engine`](../mlxfast-qwen-38-27b-mtp-engine) — the live
  Swift/MLX engine and parity reference; consumes this repo as its `benchd` submodule.
- [`mlxfast-gemma4-26b-a4b-engine`](../mlxfast-gemma4-26b-a4b-engine) — the gemma 4
  track's engine repo.
- [`cudafast-engine`](../cudafast-engine) — the CUDA engine, targeting the ruled
  RTX PRO 6000 Blackwell class box.
- [`mlxfast-engine`](../mlxfast-engine) — the earlier extracted Metal engine. Frozen;
  superseded by the per-track engine repos above.
