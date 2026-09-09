# Runbook: stand up a DGX Spark (CUDA) ranked box

**Class:** runbook. Do the steps in the order given. For the overview and the
Mac procedure, see [`box-setup-runbook.md`](box-setup-runbook.md) and
[`runbook-box-setup-mlx.md`](runbook-box-setup-mlx.md).

A Spark joins the ranked network as a self-hosted GitHub Actions runner of
the CUDA engine repository. Yukon dispatches that repository's
`benchmark.yml` for each submission. The workflow selects a runner by label.
The job builds the ds4 engine. The model is the pinned unsloth GGUF
snapshot, staged once on the box.

## Terms and conventions

- `<op>`: the operator account on the box.
- `<box>`: the name of the box. It is also the runner name.
- `<root>`: the staging directory. On the fleet it is `/home/<op>/qwen38-125b-bringup`.
- `<peer>`: a fleet Spark that already has a verified snapshot.
- Token commands read the token from stdin. Do not put a token on a
  command line. Do not write a token to a file on the box.
- The link from a laptop to a Spark can be slow (50 KB/s on the fleet's
  tailnet). A 15 MB copy can take minutes. `scp` can report success for a
  truncated file. Verify each file that you copy from the laptop by byte
  count or sha256 before you use it. The boxes reach GitHub and Hugging
  Face at 58 MB/s.
- Time budget: one fresh Spark took 51 minutes on 2026-09-04, 33 of them
  the weights download. Seven fleet boxes took 25 to 36 minutes each on
  2026-09-05 with the weights copied from a peer.

## 1. Base box

A fresh Ubuntu 24.04 image on the GB10 has `git`, `jq`, `python3`, `curl`,
`sha256sum`, `cc`, `gcc`, `nvidia-smi` and the CUDA 13 toolkit at
`/usr/local/cuda`. Do not install packages with apt.

`/usr/local/cuda/bin` is not on the default login PATH. `cargo` is not
installed. The runner PATH (step 7) and the operator shell PATH (steps 5
and 10) must contain both. The workflow checks each tool in its first step
and refuses the job when one is missing.

Install Rust under the operator account:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh
sh /tmp/rustup.sh -y --no-modify-path --default-toolchain stable
rm /tmp/rustup.sh
```

`tools/ds4/build.sh` builds upstream's `cuda-spark` target for `sm_121`.

## 2. Accounts and the GPU lock

The operator account owns the staged assets. The GPU lock is
`/tmp/mtplx-gpu-exclusive.lock`. It is a regular file, root-owned, group
`bench`, mode 0660, recreated at boot. Each GPU user on the box takes it
with `flock`. The workflow takes it for the measurement. Calibration takes
it for the whole run. Local checks take it too. A box release never waives
it.

```bash
sudo groupadd -f bench
sudo usermod -aG bench <op>
sudo touch /tmp/mtplx-gpu-exclusive.lock
sudo chown root:bench /tmp/mtplx-gpu-exclusive.lock
sudo chmod 0660 /tmp/mtplx-gpu-exclusive.lock
echo "f /tmp/mtplx-gpu-exclusive.lock 0660 root bench -" | sudo tee /etc/tmpfiles.d/mtplx-gpu-lock.conf >/dev/null
sudo systemd-tmpfiles --create /etc/tmpfiles.d/mtplx-gpu-lock.conf
```

The `bench` group is effective for the operator after a new login. Open a
new ssh session before step 10.

## 3. Weights

Stage the five pinned files flat in one directory: the four
`Qwen3.8-Flash-Next-UD-Q4_K_XL-0000N-of-00004.gguf` shards and the draft
head `mtp-Qwen3.8-Flash-Next-shared-Q8_0.gguf`. The pins (bytes and sha256
per file) are in `fixtures/qwen3_8_125b_a6b_track.json` in the engine
repository. The engine's `./setup.sh` (step 5) verifies bytes first, then
sha256. It fails closed on a miss. It never downloads.

Budget 114 GB of disk plus a 20 GiB margin. The 26.8 GiB n-gram table
inside the snapshot stays on the SSD. The job reads the directory from
`MLXFAST_TARGET_SNAPSHOT_DIR`. Shard `00001` is 10,946,624 bytes. That is
its pinned size, not a truncated download.

Source: Hugging Face repository `unsloth/Qwen3.8-Flash-Next-GGUF` at
revision `38bb39ee97821de2c9009abb7e93950eec396e66`. The files are not flat
there. The shards are under `UD-Q4_K_XL/`. The head is under `MTP/`. The
tree API reports each file's `lfs.oid`. That value equals the fixture's
sha256. Check the listing against the pins before the transfer:

```bash
curl -s "https://huggingface.co/api/models/unsloth/Qwen3.8-Flash-Next-GGUF/tree/38bb39ee97821de2c9009abb7e93950eec396e66?recursive=true" \
  | jq -r '.[] | select(.type=="file") | "\(.size)\t\(.lfs.oid // "-")\t\(.path)"' | grep -E 'UD-Q4_K_XL/|MTP/'
```

If a peer holds a verified snapshot, go to step 3a. If not, download from
Hugging Face with resume and retries, four files at a time:

```bash
BASE=https://huggingface.co/unsloth/Qwen3.8-Flash-Next-GGUF/resolve/38bb39ee97821de2c9009abb7e93950eec396e66
mkdir -p <root>/weights/gguf-unsloth-q4 && cd <root>/weights/gguf-unsloth-q4
printf '%s\n' \
  UD-Q4_K_XL/Qwen3.8-Flash-Next-UD-Q4_K_XL-00001-of-00004.gguf \
  UD-Q4_K_XL/Qwen3.8-Flash-Next-UD-Q4_K_XL-00002-of-00004.gguf \
  UD-Q4_K_XL/Qwen3.8-Flash-Next-UD-Q4_K_XL-00003-of-00004.gguf \
  UD-Q4_K_XL/Qwen3.8-Flash-Next-UD-Q4_K_XL-00004-of-00004.gguf \
  MTP/mtp-Qwen3.8-Flash-Next-shared-Q8_0.gguf \
  | xargs -P 4 -I{} bash -c 'curl -sS -L -C - --retry 5 --retry-delay 10 -o "$(basename {})" "'"$BASE"'/{}"'
```

Measured: 58 MB/s from Hugging Face on the fleet uplink, 33 minutes.

### 3a. Copy the weights from a fleet peer

The fleet Sparks share a 200 GbE link. A copy from a peer runs at 1.2 to
2.3 GB/s and takes one to two minutes. Do not add any file to the snapshot
directory on the peer: the weights digest covers the whole directory.

On the peer, serve the snapshot directory for the duration of the fleet
rollout:

```bash
cd <root>/weights/gguf-unsloth-q4
python3 -m http.server 8765 --bind <peer LAN address> >/tmp/wmirror.log 2>&1 &
echo $! >/tmp/wmirror.pid
```

On the new box, make sure that the peer answers. Then fetch the five names.
The server does not honor byte ranges. Do not resume with `-C -`. Delete a
failed file and fetch it again from the start.

```bash
curl -sI http://<peer LAN address>:8765/ | head -1     # HTTP/1.0 200 OK
mkdir -p <root>/weights/gguf-unsloth-q4 && cd <root>/weights/gguf-unsloth-q4
for n in Qwen3.8-Flash-Next-UD-Q4_K_XL-0000{1,2,3,4}-of-00004.gguf mtp-Qwen3.8-Flash-Next-shared-Q8_0.gguf; do
  curl -sS -f --retry 3 -o "$n.part" "http://<peer LAN address>:8765/$n" && mv "$n.part" "$n" || rm -f "$n.part"
done
```

Several boxes can copy from one peer at the same time. Stop the server once,
after the last box has copied: `kill "$(cat /tmp/wmirror.pid)"`. Step 5
verifies the copy against the fixture pins. Do not trust a peer copy
without that verification.

## 4. The engine checkout and the benchmarker pair

The engine repository is internal. The box holds no GitHub credential. The
ranked job does not need one: Actions supplies its own token to the
checkout. The operator checkout for the local checks (step 10) comes from a
bundle. Make the bundle on a machine that has access, at the tip to be
dispatched (the engine branch):

```bash
# on the machine with access
git -C <engine clone> fetch origin
git -C <engine clone> bundle create engine-<sha>.bundle <engine branch>
shasum -a 256 engine-<sha>.bundle
scp engine-<sha>.bundle <box>:~/
# on the box
sha256sum ~/engine-<sha>.bundle
git clone ~/engine-<sha>.bundle -b <engine branch> <root>/engine
```

Make sure that the two sha256 values are equal before the clone. A truncated
bundle clones with `fatal: early EOF` or `index-pack died`. Copy it again
(`rsync --partial`, or in chunks) until the sha256 matches. `git bundle
verify` does not work outside a repository.

Keep the bundle in place. It is the checkout's `origin`. If you remove it,
`git fetch` from that checkout fails. To move the checkout to a newer tip,
make a new bundle, copy it, and run `git fetch <bundle> <branch>`.

The benchmarker pair (`benchd`, `record-correctness-golden`, target
`aarch64-unknown-linux-gnu`) comes from the track's dist channel. The
engine's `tools/fetch-benchd.sh` downloads it and verifies it against its
own `benchd.manifest.json`. While the bench repository is private, the
fetch needs a token for that one call. Pass it on stdin:

```bash
ssh <box> 'cd <root>/engine && GITHUB_TOKEN=$(cat) BENCHD_BIN_DIR=<root>/benchd-bin ./tools/fetch-benchd.sh' <<< "$(gh auth token)"
```

An installed pair stays in place. After a dist republish, set
`BENCHD_REFRESH=1` or point `BENCHD_BIN_DIR` at a new directory.

## 5. Build and verify the snapshot

Run from the operator checkout:

```bash
cd <root>/engine
export PATH=$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH
MLXFAST_TARGET_SNAPSHOT_DIR=<root>/weights/gguf-unsloth-q4 ./setup.sh
```

This builds ds4 and the adapter (two to four minutes from cold). It then
verifies the five staged files by bytes and sha256 against the fixture. It
exits non-zero on any miss.

## 6. Runner registration

Install the GitHub Actions runner under the operator account. Register it
to the CUDA engine repository. Pass `--labels <track id>` only (today
`qwen3.8-125b-a6b-cuda-v1`). The runner adds `self-hosted`, `Linux` and
`ARM64` itself. The label is what `runs-on` selects. A second Spark with
the same label adds capacity.

Register now. Start the service only after the checks of step 10 pass. A
job dispatched to the new label must not contend for the GPU lock with a
check. The registration token is single-use and short-lived. Mint it on
the machine with access and pass it on stdin:

```bash
V=$(gh api repos/actions/runner/releases/latest --jq .tag_name | sed 's/^v//')
ssh <box> "mkdir -p ~/actions-runner-qwen38-cuda && cd ~/actions-runner-qwen38-cuda \
  && curl -sS -L -o rn.tar.gz https://github.com/actions/runner/releases/download/v$V/actions-runner-linux-arm64-$V.tar.gz \
  && tar xzf rn.tar.gz && rm rn.tar.gz"
ssh <box> 'cd ~/actions-runner-qwen38-cuda && ./config.sh --unattended \
  --url https://github.com/Layr-Labs/cudafast-qwen38-125b-a6b-engine-dev \
  --token "$(cat)" --name <box> --labels qwen3.8-125b-a6b-cuda-v1 --work _work --replace' \
  <<< "$(gh api -X POST repos/Layr-Labs/cudafast-qwen38-125b-a6b-engine-dev/actions/runners/registration-token --jq .token)"
```

The fleet posture is a persistent registration under the operator account,
run as a systemd service (step 11). The supervisor model in
`m5-machine-scripts/runner-isolation` is an alternative. It is not used on
the fleet. Never start a second listener by hand. A second listener
conflicts with the first ("a session for this runner already exists") and
the runner flaps offline.

## 7. The runner environment file

`.env` in the runner root holds values only. The listener reads it when it
starts. `config.sh` has already written the file with a `LANG` line. Append
these four lines:

```
MLXFAST_TARGET_SNAPSHOT_DIR=<root>/weights/gguf-unsloth-q4
MLXFAST_QWEN38_GOLDEN_DIR=<root>/goldens/qwen3.8-125b-a6b-cuda-v1
BENCHD_BIN_DIR=<root>/benchd-bin
PATH=/home/<op>/.cargo/bin:/usr/local/cuda/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
```

Do not add a `DS4_*` variable. The engine's own `tools/serve-up.sh` sets
those. The preflight refuses a job when they are preset. Do not add
`MLXFAST_LOCAL_COOL_GATE`. The preflight refuses a job when it is preset.

The listener applies the file after it starts. `/proc/<pid>/environ` of
`Runner.Listener` does not show these values. The job inherits them. The
receipt is the workflow's "runner environment" step. To change the file,
edit it, then restart the service (`sudo ./svc.sh stop && sudo ./svc.sh start`).

## 8. Goldens

The timed-pool goldens and the per-depth oracles are organizer material.
They are in R2, under the object keys that `r2_path` names in the track
fixture. They are never in git, and the ranked job holds no credential, so
the operator stages them one time, out of band.

Download each pinned object with the organizer's credentialed download.
Put the files in `<root>/goldens/qwen3.8-125b-a6b-cuda-v1`, one file per
pin, named by the last part of its `r2_path`. Put nothing else there. Then
run `tools/ranked-box-preflight.sh` from the engine checkout to verify the
directory.

Set `MLXFAST_QWEN38_GOLDEN_DIR` to that directory in the runner environment
file (section 7). The workflow refuses to start when the variable is
missing. Before every ranked run, the preflight compares the byte count and
the sha256 of each staged file with the fixture pin. It also refuses the
directory when the directory holds one more `*.json`, because the job passes
every `*.json` there as a golden.

## 9. The measurement topology

Each measured window boots one `ds4-resident` through the engine's
`tools/serve-up.sh`, inside the job's lock window. The weights load once.
Each phase attaches a thin worker over a Unix socket. The resident serves
one connection at a time. When `DS4_RESIDENT_SOCKET` is set, benchd runs
the whole window on one attached worker. `tools/serve-up.sh` does not take
the GPU lock. Its caller does.

The local checks have a pre-timing gate. The gate waits for the GPU to idle
and to cool to 50 C. The ranked path has no local gate.

## 10. Local checks

Run two checks from the operator checkout, one after the other, each under
the lock. Both need `BENCHD_BIN_DIR`. Without it, `benchmark.sh` tries a
network fetch of the pair. Both need the snapshot directory. The first
check also needs `MLXFAST_WEIGHTS_PATH`. That is the directory that
`benchmark.sh` digests. `SERVE_UP_WEIGHTS_DIR` only feeds the resident.

The serve must match the tree's declared spec. `benchmark.sh` derives the
declaration from `mtp-head.manifest.json` (`spec.enabled`,
`spec.num_speculative_tokens`). A resident booted serial against a tree
that declares a depth fails with `spec mode "mtp" is not runnable on this
engine`. Derive the two serve values the way the ranked job does. Do not
write them by hand.

```bash
cd <root>/engine
export PATH=$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH
export BENCHD_BIN_DIR=<root>/benchd-bin
export MLXFAST_TARGET_SNAPSHOT_DIR=<root>/weights/gguf-unsloth-q4
export MLXFAST_WEIGHTS_PATH="$MLXFAST_TARGET_SNAPSHOT_DIR"
export SERVE_UP_SPECULATIVE="$(./tools/spec-declaration.sh speculative)"
export SERVE_UP_SPEC_DRAFT_LEN="$(./tools/spec-declaration.sh draft-len)"
./tools/spec-declaration.sh describe
```

The last command prints `serial` or the declared depth. The engine branch
prints `mtp1` since the depth-1 promotion of 2026-09-04.

### 10a. The correctness check

```bash
flock /tmp/mtplx-gpu-exclusive.lock -c '
  MLXFAST_ENGINE_BIN=.build/release/mlxfast-runtime-worker \
  MLXFAST_CORRECTNESS_GOLDEN_PATH=correctness_prompts/public-longcopy-gate-english-1024.golden.json \
  SERVE_UP_WEIGHTS_DIR="$MLXFAST_WEIGHTS_PATH" \
  tools/serve-up.sh ./benchmark.sh --local-iterate'
jq '.metrics.passed_correctness, .passed, .score' score.local-iterate.json
```

The receipt is exit code 0 and `passed_correctness` true in
`score.local-iterate.json`. The flag is under `.metrics`. The score is not a
readiness figure: a local run with no reference tree measures the candidate leg
only and seals `score: null`. Read `passed_correctness`, not the score.

Time: three to six minutes. The weights digest of 114 GB takes 85 seconds.
The gate then waits for the GPU to cool.

Some boxes idle warm. On two fleet boxes the loaded resident held the die
at 51 to 52 C with nothing else on the GPU. The gate then rejects the run:
`gate rejected (prefill): GPU is hot and not cooling down`. On such a box,
run the correctness check with `MLXFAST_LOCAL_COOL_GATE=0` inside the
`flock -c` environment. `passed_correctness` stays valid. The timings are
then hot-start and not comparable. The gate is local-only. The ranked path
is not affected.

### 10b. The score check

This is the ranked job's own measurement.

```bash
flock /tmp/mtplx-gpu-exclusive.lock -c '
  MLXFAST_QWEN38_GOLDEN_DIR=<root>/goldens/qwen3.8-125b-a6b-cuda-v1 \
  BENCHD_BIN=<root>/benchd-bin/benchd \
  SERVE_UP_WEIGHTS_DIR="$MLXFAST_TARGET_SNAPSHOT_DIR" \
  tools/serve-up.sh tools/qwen38-125b-a6b-measure-and-score.sh'
jq '.passed, .score, .metrics.effective_spec_depth' score.json
```

The receipt is `passed: true` in `score.json`. The run is paired, so the score
is the live ratio of the reference legs to the candidate legs: a serial
declaration puts two serial legs against each other and scores near 1.00, and a
depth declaration scores above it. Time: two to three minutes once the engine is
built.

Both files are gitignored. The tree stays clean.

## 11. Start the runner service

Run this after both receipts of step 10 are in hand:

```bash
ssh <box> 'cd ~/actions-runner-qwen38-cuda && sudo ./svc.sh install <op> && sudo ./svc.sh start'
```

Make sure that exactly one `Runner.Listener` runs and that the runner shows
online:

```bash
gh api repos/Layr-Labs/cudafast-qwen38-125b-a6b-engine-dev/actions/runners --jq '.runners[] | "\(.name) \(.status)"'
```

## 12. Dispatch and receipt

```bash
gh workflow run benchmark.yml --ref <engine branch>
```

The box job checks the runner environment, runs the preflight, verifies the
pair, builds the engine, and measures under the lock. The build takes 90
seconds from cold. A content-keyed cache skips it when the tree is
unchanged. A passing run of step 10b is the readiness receipt. One dispatch
per fleet is sufficient. GitHub assigns the job to any idle runner with the
label.

## 13. The reference tree and this box's health band

The CUDA track pins no serial pair. A ranked run measures the pairs the track
fixture declares in `official_pairs` — 2 on both platforms — and every pair
times a SERIAL-CONTROL leg on the reference tree and then the candidate leg. The
score is the live ratio.

So the box carries two things instead of a pinned pair:

| name | what it holds |
|---|---|
| `MLXFAST_BASELINE_WORKSPACE` | the organizer-staged reference tree, built |
| `MLXFAST_BASELINE_CALIBRATION` | this box's `baseline-calibration.json` |

Write the calibration file once on the box, and again whenever the organizer
moves the reference tree:

```bash
flock /tmp/mtplx-gpu-exclusive.lock -c '
  benchd calibrate-baseline \
    --baseline-workspace "$REFERENCE_WORKSPACE" \
    --engine "$ENGINE_RELATIVE_PATH" \
    --golden "$LIVE_GOLDEN" \
    --passes 4 \
    --out "$REFERENCE_WORKSPACE/baseline-calibration.json"'
```

`--engine` is the path the candidate uses, relative to its own root: on this
platform the adapter that `tools/stage-cuda-engine.sh` stages under
`.build/release/`. benchd re-roots that same path into the reference tree.

The verb runs the ranked path's own control leg four times and refuses by name
(`CALIBRATION-CV-EXCEEDED`) when the box is too noisy for a mean to describe it.
The file is a health band for the control leg, never a denominator.

Export both names in the runner service environment. The full procedure, the
refusal names and the file format are in
[`qwen38-125b-a6b-baseline-capture.md`](qwen38-125b-a6b-baseline-capture.md).

## 14. Readiness checklist

- [ ] CUDA 13 (`nvcc`), `cargo`, `cc`, `git`, `jq`, `python3`, `curl`, `sha256sum`, `nvidia-smi` on the runner's PATH
- [ ] operator account in group `bench`; GPU lock root-owned, group `bench`, mode 0660, tmpfiles rule in place
- [ ] five pinned snapshot files staged flat; `./setup.sh` verified them by bytes and sha256
- [ ] operator engine checkout at the engine branch tip, from a bundle with a matching sha256, clean tree
- [ ] benchd pair installed by `tools/fetch-benchd.sh` for `aarch64-unknown-linux-gnu`, manifest beside it
- [ ] runner registered with label `<track id>` (the runner adds `self-hosted, Linux, ARM64`)
- [ ] runner `.env` carries `MLXFAST_TARGET_SNAPSHOT_DIR`, `BENCHD_BIN_DIR`, `MLXFAST_BASELINE_WORKSPACE`, `MLXFAST_BASELINE_CALIBRATION`, the toolchain `PATH`, no `DS4_*` and no `MLXFAST_LOCAL_COOL_GATE`
- [ ] serve values derived from `tools/spec-declaration.sh`, not written by hand
- [ ] correctness check under the lock: exit 0, `.metrics.passed_correctness` true
- [ ] score check under the lock: `passed: true`; a serial declaration scores near 1.00, a depth declaration above it
- [ ] runner service started after the checks; exactly one `Runner.Listener`; runner online
- [ ] one `workflow_dispatch` of `benchmark.yml` passes end to end (one per fleet)

## 15. Faults seen in practice

| symptom | cause | fix |
|---|---|---|
| job fails at "runner environment": `cargo` or `nvcc` not on PATH | listener started without the toolchain paths | add `PATH` to the runner `.env`; restart the service |
| `benchd iterate: weights digest failed (weights): No such file or directory` | `MLXFAST_WEIGHTS_PATH` not set for `benchmark.sh --local-iterate` | export it (step 10); `SERVE_UP_WEIGHTS_DIR` alone is not sufficient |
| a local check tries a network fetch of the pair and fails | `BENCHD_BIN_DIR` not set; `fetch-benchd.sh` defaults to `<repo>/benchd-bin` | export `BENCHD_BIN_DIR` to the pair directory of step 4 |
| a local check fails with `spec mode "mtp" is not runnable on this engine` | the resident was booted serial against a tree that declares a depth | derive `SERVE_UP_SPECULATIVE` and `SERVE_UP_SPEC_DRAFT_LEN` from `tools/spec-declaration.sh` (step 10) |
| correctness check: `gate rejected (prefill): GPU is hot and not cooling down` with nothing else on the GPU | the loaded resident holds the die above the 50 C local gate on this box | `MLXFAST_LOCAL_COOL_GATE=0` for that local check only (step 10a); never in the runner `.env` |
| `git clone` of the bundle: `fatal: early EOF` or `index-pack died` | the bundle was truncated in transit; `scp` over a relayed link can return 0 on a partial file | compare sha256 on both ends; copy again with `rsync --partial` or in chunks |
| `git ls-remote` of the engine repository fails on the box | the box has no GitHub credential | expected; the operator checkout comes from a bundle; the ranked job uses the Actions token |
| `fetch-benchd.sh`: "manifest sha256 is not 64 lowercase hex characters" | a hand-made `benchd.manifest.json` with a `binaries` entry split over lines | one entry per line, the channel's layout |
| `benchd calibrate-baseline` refuses with `CALIBRATION-CV-EXCEEDED` | the four passes differed by more than 1% on one axis | a result, not a fault; find out why the box is noisy, then calibrate again |
| runner offline; log says a session already exists | a second listener started by hand | stop it; let the service's listener reconnect |
| preflight refuses an unpinned `*.json` in the pool directory | a non-pool golden placed beside the pool | move it out of the directory that `MLXFAST_QWEN38_GOLDEN_DIR` names |
| a window stalls, GPU idle, two worker processes | a second connection waiting on the one-connection resident | one attached worker per window (benchd does this when `DS4_RESIDENT_SOCKET` is set) |
| speculative candidate misses the prefill band while serial passes | the engine re-uploaded the draft head on first use inside the timed prefill | fixed at ds4 pin 5f36517; the head stays resident from load |
| `nvidia-smi --query-gpu=memory.total` prints `[N/A]` | GB10 unified memory | read `/proc/meminfo`; `serve-up.sh` does (107 GiB available, 91 GiB required) |
