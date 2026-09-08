# Box setup runbook: DGX Spark (CUDA) and Mac (MLX)

**Class:** runbook. Follow it as written.

This document is the overview: what both box types share, the calibration of
the official baseline pair, the differences at a glance, and the faults seen
in practice. The step-by-step procedure per platform, each with a readiness
checklist, is in its own runbook:

- [`runbook-box-setup-cuda.md`](runbook-box-setup-cuda.md) for a DGX Spark
- [`runbook-box-setup-mlx.md`](runbook-box-setup-mlx.md) for a Mac

## 1. What both box types need

| item | where it comes from | notes |
|---|---|---|
| the benchmarker pair (`benchd`, `record-correctness-golden`) | this repository's dist channel, one directory per platform, resolved by the engine's `tools/fetch-benchd.sh` | the pair is verified against its own `benchd.manifest.json` (sha256, bytes, `target_triple`); while the track is internal the fetch needs `GITHUB_TOKEN` or `BENCHD_DIST_TOKEN` |
| the GPU lock | `/tmp/mtplx-gpu-exclusive.lock`, a regular file taken with `flock` | every GPU user on the box takes it, including calibration and local runs; a box release never waives it |
| the engine checkout | the ranked job checks it out fresh from the submission; an operator checkout is separate | never measure with an operator checkout that has local edits |
| the runner | a self-hosted GitHub Actions runner whose labels include the track id | the label set is what the workflow's `runs-on` selects |
| the official baseline pair | pinned in `crates/bench-core/src/constants.rs` (`OFFICIAL_BASELINES_BY_TRACK`) after one calibration on the box | see section 4 |

## 2. Calibration: the official baseline pair

Both tracks score a candidate against one pinned serial pair per track. A
PENDING track refuses scored runs by name until the pair is pinned.

- **Spark:** the engine's `tools/qwen4exp-calibrate.sh` takes the lock, boots
  one serial resident, hashes the weights once, runs every pool golden as one
  warm-up pass plus four timed legs on that one load, and gates each golden's
  legs at a 1 percent coefficient of variation. It writes the report and the
  constants patch and applies nothing. Apply the printed pair to
  `OFFICIAL_BASELINE_CUDA` in `crates/bench-core/src/constants.rs`, write each
  golden's pair into its `benchmark.baseline_*_seconds_per_token` fields, set
  `official_scoring_enabled` in the fixture, and republish the dist for both
  platforms at the pinning commit. Expect about 12 to 15 minutes on the box at
  today's engine speeds; the weights digest is 100 seconds of that.
- **Mac:** the converge `calibration-driver` unit's MLX profile runs one
  `benchd` invocation per prompt with `--capture-passes W,A,A,B,B` over one
  worker load (8 loads per window, not 40), gateless. The analyzer reports the
  coefficient of variation and the A-versus-B split.

See `docs/official-baseline-capture.md` for the capture procedure and
`docs/qwen38-125b-a6b-baseline-capture.md` for the track's own history.

## 3. Differences at a glance

| | DGX Spark (CUDA) | Mac (MLX) |
|---|---|---|
| engine | ds4, built in the job (nvcc + cargo) | mlxfast Swift, built by `setup.sh` (Xcode + Metal) |
| model on the box | organizer-staged GGUF snapshot, verified, never fetched by the job | downloaded and verified by `setup.sh` from the organizer mirror |
| MTP head | separate `mtp-*.gguf` beside the shards | embedded in the checkpoint |
| who holds the weights in a window | `ds4-resident`, one connection at a time | the worker process itself |
| worker lifecycle | one attached worker per window (`DS4_RESIDENT_SOCKET` set) | one persistent worker per window |
| goldens | from the checkout, pinned; copy in R2 | fetched from R2 by pins |
| cool gate | 50 C | 40 C |
| runner supervisor | systemd | LaunchDaemon |
| pair platform | `linux-aarch64` | darwin |

## 4. Faults seen in practice

| symptom | cause | fix |
|---|---|---|
| ranked job fails at "runner environment" with `cargo`/`nvcc` not on PATH | listener started without the toolchain paths | add `PATH` to the runner `.env`, restart through the supervisor |
| runner shows offline, log says a session already exists | a second listener was started by hand | stop the extra listener; let the supervisor's one reconnect |
| preflight refuses an unpinned `*.json` in the pool directory | a non-pool golden placed beside the pool | move it out of `correctness_prompts/<track>/` |
| calibration or a window stalls with the GPU idle and two workers | a second connection waiting on the one-connection resident | run the window on one attached worker (benchd does this when `DS4_RESIDENT_SOCKET` is set) |
| speculative candidate misses the prefill band while serial passes | the engine re-uploaded the draft head on first use inside the timed prefill | fixed in the engine (ds4 pin 5f36517); keep the head resident from load |
