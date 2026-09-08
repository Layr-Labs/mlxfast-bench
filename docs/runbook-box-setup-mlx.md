# Runbook: stand up a Mac (MLX) ranked box

**Class:** runbook. Follow it as written. Overview and the Spark counterpart:
[`box-setup-runbook.md`](box-setup-runbook.md), [`runbook-box-setup-cuda.md`](runbook-box-setup-cuda.md).

A Mac joins the ranked network by becoming a self-hosted GitHub Actions
runner of the MLX engine repository. Yukon dispatches that repository's
`benchmark.yml` for every submission; the workflow selects a runner by label.
The engine is the mlxfast Swift runtime; the model is the published MLX 4-bit
checkpoint, which the engine's `setup.sh` downloads and verifies in the job.

## 1. Base box

- An Apple Silicon Mac with enough unified memory for the target model and its
  working set (the checkpoint is 113 GB across 32 pinned files) and at least
  260 GiB of free disk before the first download.
- macOS 14 or later; Swift 6 (`swift-tools-version: 6.3`).
- Full `Xcode.app` with the Metal toolchain. Full Xcode cannot be installed
  unattended (an Apple ID is needed), so an operator stages it. Then
  `xcode-select -p` must point at its Developer directory (the Command Line
  Tools instance cannot build Metal), the license must be accepted
  (`sudo xcodebuild -license accept`), and `xcrun -sdk macosx metal -v` must
  run. The `m5-machine-scripts` `xcode-toolchain` converge unit checks these
  four facts and fails closed when `Xcode.app` is absent.
- CMake and Git (`setup.sh` installs CMake through Homebrew when missing).
- `macmon` at `/opt/homebrew/bin/macmon`: the workflow reads GPU utilization
  and temperature from it for quiescence and the cool gate, and refuses to
  time when the reader stops returning samples.
- No Rust: the pair arrives prebuilt.

## 2. Accounts and the GPU lock

Same posture as the Spark: an operator account owns the staged assets, a
`bench` group reads them, and `/tmp/mtplx-gpu-exclusive.lock` is a regular
file taken with `flock` by every GPU user, including calibration and local
runs.

## 3. Weights

Nothing to stage by hand. The engine's `./setup.sh` fetches the public MLX
4-bit checkpoint anonymously from Hugging Face (no token by default;
`MLXFAST_REFERENCE_AUTH_HEADER` is an optional fallback for a private mirror or
rate limiting), verifies every file by sha256 and bytes, and the engine's
`transform` command writes the `weights/` tree the worker loads. The MTP head
ships inside the checkpoint; there is no separate head file.

## 4. The benchmarker pair

The ranked job holds no credential and the bench repository is private, so
the job cannot download the pair. Stage the pair on the box before the first
job:

1. On a machine with access to the bench repository, check out the track's
   dist channel (`qwen3.8-125b-a6b-v1`) and copy `dist/benchd` and
   `dist/benchd.manifest.json` to the box, into the directory that
   `BENCHD_BIN_DIR` names. Set the mode of `benchd` to 755.
2. The job runs `./tools/fetch-benchd.sh`. When the pair is present, the tool
   verifies the binary against the manifest beside it and uses it. It does not
   contact the channel.

To move the box to a new channel tip, replace both files. A binary that does
not match its manifest refuses to run.

## 5. Goldens

The ranked job reads the eight pool goldens from the checked-out repository,
directory `correctness_prompts/<track id>/`. The workflow sets
`MLXFAST_QWEN38_GOLDEN_DIR` to that directory. The preflight verifies each
file against the sha256 and byte pins in the track fixture and refuses on a
mismatch or on an extra `.json` file. Re-authoring the goldens is an engine
pull request that changes the files and the fixture pins together. The
hidden correctness golden is not part of the ranked job.

## 6. Runner registration and service

- Register the runner to the MLX engine repository with the labels
  `self-hosted, macOS, <track id>` (today `qwen3.8-125b-a6b-mlx-v1`).
- Run it under the root LaunchDaemon supervisor from
  `m5-machine-scripts/runner-isolation`: single-use registration through the
  GitHub App, one job as the unprivileged `benchrunner`, reset between jobs.
- The runner environment file (`.env` in the runner directory, one
  `NAME=value` per line; restart the service after a change) must carry:
  - `BENCHD_BIN_DIR`: the directory with the staged pair (section 4).
  - `MLXFAST_REFERENCE_DIR`: the reference checkpoint directory itself. The
    first job verifies the full checkpoint hash once and writes a trusted
    stamp in that directory; later jobs skip the hash while the stamp is
    present.
  - `MLXFAST_MACMON_BIN`: the `macmon` binary when it is not at
    `/opt/homebrew/bin/macmon`.
  - `MLXFAST_FORK_MIRROR`: a bare mirror of the engine fork repository,
    staged out of band (`git clone --mirror`). The job resolves the
    `Vendor/mlx-swift-lm` submodule from it without a credential. A mirror
    that does not carry the pinned commit fails the job; re-stage it.
  - `MLXFAST_METALLIB_STAGE`, on a box without full Xcode: a directory with
    `mlx.metallib` and `mlx.metallib.fingerprint` built from the engine's
    vendored tree. Dispatch such a box with the workflow input
    `prestaged_metallib: true`; the job accepts the library only when the
    fingerprint matches the checkout.
- The runner service PATH must carry `swift`, `git`, `jq`, `python3`, `bc`
  and `shasum`. The Command Line Tools are sufficient when the Metal library
  is pre-staged.
- The job keeps the worker build between jobs in a content-keyed cache under
  the runner user's `~/.cache/mlxfast-engine-build`. A box with a staged
  engine checkout can seed the cache with `tools/build-cache.sh save`.

## 7. The measurement topology

The engine's `bench-worker` runs as one resident per window, started by
`tools/resident-up.sh`; every phase attaches to it, so the model loads once
per window. The measure script starts the resident itself and holds the GPU
lock for the whole window. Three concurrent fresh workers would need about
190 GiB, which is what the resident prevents.

## 8. Local checks

```bash
MLXFAST_ENGINE_BIN=.build/release/mlxfast-runtime-worker \
MLXFAST_CORRECTNESS_GOLDEN_PATH=correctness_prompts/public_longcopy_gate_english_1024_256.json \
  ./benchmark.sh --local-iterate
```

The local test has no default golden; the variable must be set. Do not pass
`--golden`, `--weights` or `--score-path`; the script rejects them.

## 9. Dispatch and receipt

```bash
gh workflow run benchmark.yml --ref <release branch>
```

The box job runs the preflight, fetches the pair, runs `setup.sh` (toolchain
check, Swift build, `mlx.metallib`, checkpoint download and verification),
waits for quiescence, and measures. A passing serial run near 1.0 against the
track's pinned baseline pair is the readiness receipt.

## 10. Readiness checklist

- [ ] macOS 14+, Swift 6, full `Xcode.app` selected, license accepted, `xcrun -sdk macosx metal -v` runs
- [ ] CMake, Git, and `macmon` at `/opt/homebrew/bin/macmon`
- [ ] at least 260 GiB free before the first `setup.sh`
- [ ] operator account and `bench` group; GPU lock present and group-readable
- [ ] benchd pair installed by `tools/fetch-benchd.sh` for darwin, manifest beside it
- [ ] pool goldens staged in `MLXFAST_QWEN38_GOLDEN_DIR` and passing `tools/ranked-box-preflight.sh`
- [ ] runner registered with labels `self-hosted, macOS, <track id>` and online, under the LaunchDaemon supervisor
- [ ] iogpu wired limit pinned by the boot daemon (`sysctl iogpu.wired_limit_mb`)
- [ ] `./setup.sh` completes: toolchain, Metal kernels, checkpoint verified, `weights/` transformed
- [ ] local `benchmark.sh --local-iterate` passes correctness on the public golden
- [ ] one `workflow_dispatch` of `benchmark.yml` passes end to end
- [ ] sealed serial score near 1.0 against the pinned baseline pair
