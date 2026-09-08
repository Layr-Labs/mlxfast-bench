# Track release branches

How model tracks bind to this repo. Ruled 2026-08-22 (engine fork = new repo per track family;
bench side = a release branch here — no per-track bench forks).

## Two names, two jobs

A track uses two names. Do not treat them as one name.

| name | what it identifies | where it is used |
|---|---|---|
| **release branch** | the PROJECT — one model generation's measurement work in this repo | the branch in this repo; the `.gitmodules` tracking hint; the dist channel |
| **`track_id`** | the PLATFORM — one model generation on one hardware platform | the sealed `track_id` field; the R2 key prefix; the ranked-board row |

A release branch can serve more than one platform. The MLX track and the CUDA track of the same
model generation measure the same way, so they CAN share one branch and one benchd. Sharing is a
choice, not a rule: the two qwen 3.8 27B tracks in the table below each took their own branch.
Platforms that share a branch still do not share a `track_id`. Each platform keeps its own
`track_id`, its own R2 key prefix, and its own goldens. This is why the two names are separate.

Every track in the table below currently serves ONE platform. Where such a track also cut its own
branch, the two names came out as the same string. The rule is the same rule; the two names simply
agree here. The grandfathered `main` track shows that they do not always agree, even for one
platform.

## Name construction

Build both names the same way:

```
{model}{ver}-{params}-{platform}-v{N}
```

Names are **decoder-neutral**. Never put a spec-decoder kind (`mtp`, `dflash`, `dspark`) in a
branch name or a track name. A version bump (`-v{N+1}`) is a new track generation: a new branch, a
new R2 key namespace, and re-authored goldens.

When one branch serves two platforms, name the branch for the platform that cut it. The second
platform then adds its own `track_id` on that same branch. The branch name does not change.

| Track | Release branch | `track_id` | Engine repo |
|---|---|---|---|
| qwen 3.8 27B MLX | `main` (grandfathered) | `qwen3.8-27b-mtp-v1` | `mlxfast-qwen-38-27b-mtp-engine` (rename pending) |
| qwen 3.8 27B CUDA | `qwen3.8-27b-cuda-v1` | `qwen3.8-27b-cuda-v1` | `cudafast-engine` family |
| gemma 4 26B A4B MLX | `gemma4-26b-a4b-mlx-v1` | `gemma4-26b-a4b-mlx-v1` | `mlxfast-gemma4-26b-a4b-engine` |

Read the first row: the grandfathered qwen 3.8 27B MLX track runs on branch `main` but seals
`track_id` `qwen3.8-27b-mtp-v1`. The two names already differ on a live track today. That track
also predates the decoder-neutral rule, which is why its `track_id` still carries `mtp`.

## Branch lifecycle

1. **Cut** from current `main` at the track's re-baseline step — a deliberate, reviewed baseline
   choice made once, when the track's engine repo advances its benchd gitlink off the pin it
   inherited from its seed.
2. **Carries** the track's measurement work: measure modes and their series tags, the track's
   wire-crosscheck fixture, calibration entries. Calibration series tags encode the measurement
   regime (and, for batched tracks, the batch size) — the series fence refuses cross-series
   comparison, so a tag change is a semantic act.
3. **Consumed** by the track's engine repo through one of the two channels below.

## How an engine repo resolves this repo

An engine repo gets benchd through ONE of two channels. Both channels resolve to exact bytes.
Neither channel lets the engine repo choose a different benchd at run time.

**Channel 1 — SHA-pinned submodule.** The engine repo carries this repo as a submodule. The
gitlink IS the pin. `.gitmodules` names the release branch as the tracking hint only; the hint
does not select the commit. Advance the pin in a deliberate two-verdict PR on the engine side.
Verify containment with a remote-tracking branch:

```
git -C benchd branch -r --contains $(git rev-parse HEAD:benchd)
```

**Channel 2 — dist channel.** A public engine repo cannot resolve a gitlink into a private repo.
It reads a published benchd instead. The engine repo names the channel in the
`BENCHD_DIST_BRANCH` environment variable, and resolves that channel through a manifest.

The manifest carries four fields. Two fields verify the bytes. Two fields record where the bytes
came from:

| field | what it carries | kind |
|---|---|---|
| `sha256` | the digest of the published bytes | integrity |
| `bytes` | the byte count of the published bytes | integrity |
| `branch` | which release branch published this benchd | provenance |
| `source_commit` | which commit on that branch the build came from | provenance |

`sha256` and `bytes` together are the integrity pin. Verify both before use. Refuse a manifest
whose `sha256` or `bytes` does not agree with the published bytes. Do not fall back to an
unverified benchd.

`branch` and `source_commit` are provenance. They are not checks, and they cannot be verified
against the bytes. They record which release branch published the benchd, and which commit the
build came from, so a reader can find the source. Keep them correct: a wrong value here does not
stop a run, but it sends the next reader to the wrong source.

The channel publishes more than one binary from that one `source_commit`. The four fields above
describe `benchd`. A fifth field, `binaries`, gives the same integrity pair — `sha256` and
`bytes` — for every published binary, one line for each: `benchd`, and
`record-correctness-golden`, which an engine repo runs to author its track goldens. A consumer
that reads only the four fields above still resolves `benchd` correctly. A consumer that needs
another binary reads its entry and verifies it the same way. See the repository README,
"Publishing `dist/`", for the shape and the reader.

The dist channel is CONSUMER-side machinery. This repo is the SOURCE the channel publishes, which
is what `branch` and `source_commit` name. No code in this repo reads the channel or resolves
`BENCHD_DIST_BRANCH`. Do not look for the resolver here.

## Engine-repo counterpart

The engine side of a new track is a **new, fresh-seeded org repo** (single signed commit,
tree-identical to the source engine's `main`, source commit named in the seed message — the org
ruleset's verified-signature requirement rejects history-carrying pushes). Its procedure doc
lives in the engine repo as `docs/new-track-repo-procedure.md`.

## Official baseline

Each track has ONE official baseline pair: the serial prefill and decode seconds-per-token that
the track scores against. The pair belongs to the `track_id`. It does not belong to the branch,
and it does not belong to the repo.

The pairs are in a per-track table in `crates/bench-core/src/constants.rs`. The branch names its
own track in `TRACK_ID` in the same file. A track is in the table only after you capture its pair
on that track's own benchmark hardware. Before the capture, the track has no entry. That state is
`OFFICIAL_BASELINE_PENDING`.

Read the table with `official_baseline(track_id)`. It is the only accessor. It refuses a track
that has no entry, and the refusal names the `track_id` and the sentinel. Do not read the table
in any other way.

### Where each path gets its pair

The paths do not agree, and that is deliberate. Read the row for the path you are on.

| path | sources, in order | if no source gives a pair |
|---|---|---|
| official, timed | the `MLXFAST_PAIRED_BASELINE_{PREFILL,DECODE}_SECONDS_PER_TOKEN` pair, else both `--baseline-*` flags, else the golden's `benchmark.baseline_{prefill,decode}_seconds_per_token` pair | the run PREFLIGHT-FAILS. The table is not a fallback here. |
| official, gates-only (`MLXFAST_BENCHMARK_SKIP_TIMED=1`) | the `MLXFAST_PAIRED_BASELINE_*` pair, else the golden's pair, else the TRACK's captured pair | the track is pending: the run refuses and names the track, the sentinel, and the two sources it tried |
| local (`--local-iterate`, `--local-submit`) | the TRACK's captured pair only. #127 makes these legs score against the constants and nothing else: no environment pair, no flag, no golden pair | the track is pending: the run refuses and names the track, the sentinel, and the table |

The official timed path still touches the table in one place. Its PREFLIGHT-FAILURE record carries
a baseline pair, and that pair comes from the table. A pending track therefore stops the run
before the record is written, instead of writing a record with another track's numbers in it.

Do not give a new track another track's numbers. A track that has no captured pair cannot be
scored on any path.

### Ending the pending state

A pending track cannot reach a timed phase on any scored path, so it cannot measure its own pair
there. `benchd iterate --capture-baseline` is the one mode that can. It runs only while the
track is pending, it resolves no official baseline, and it writes only its capture record. A
captured track refuses it by name.

The procedure is [`official-baseline-capture.md`](official-baseline-capture.md). The record it
writes is the input to a separate, reviewed pull request that adds the track's pair to the table
above.

### The track_id fence

The workflow-declared track id must be ONE value: `constant ≡ contract ≡ env`.

* `measure-job` resolves the track id from the `MLXFAST_QWEN_MTP_TRACK_ID` environment variable or
  from the `--contract` track fixture. The two must agree with each other, AND the result must
  equal `TRACK_ID`.
* `overlay-timing` refuses a `results.json` whose sealed `track_id` is not `TRACK_ID`, even when
  the operator names that same foreign track.
* Both refusals name BOTH values: the track this tree serves, and the track the run declares.

The fence exists because the two other legs cannot see this error. A tree built for one track,
driven by another track's contract, seals THIS track's baseline pair under THAT track's
`track_id`. The commit and the weights-hash cross-checks still agree, because both halves really
do come from one run. Only the constant can tell.

`benchd iterate` takes no `--contract`. It reads the track id from the
`MLXFAST_QWEN_MTP_TRACK_ID` environment variable. That variable is its only source, so there is
nothing to cross-check. Do not invent a second source. The variable is not optional: the run
refuses when it is not set, because the track id keys the platform and the model identity.

## Model identity

Each track ALSO declares its MODEL IDENTITY. The identity has four facts:

| fact | what it controls |
|---|---|
| `golden_model_type` | the `model_type` value the track's goldens declare |
| `vocab_size` | the token-id range (`0` to `vocab_size`) that goldens, tapes and worker logits must stay inside |
| `num_hidden_layers` | the sealed `metrics.num_layers` audit field |
| `seed_tokens` | the length of the correctness prompt, the prefill prompt and the decode seed |

The identity belongs to the `track_id`. It does not belong to the branch. The identities are in a
per-track table in `crates/bench-core/src/constants.rs`. Read the table with
`model_identity(track_id)`. It is the only accessor. It refuses a track that has no row, and the
refusal names the `track_id`, the `MODEL-IDENTITY-UNDECLARED-FOR-TRACK` sentinel and the tracks
that do have a row. Do not read the table in any other way.

benchd resolves the identity ONCE for each run. It resolves it from the same track id that gives
the platform, and it resolves it BEFORE it reads a golden. The golden loader, the timed-prompt
tape loader, the correctness gates and the sealed audit metrics all use that one identity.

The result is that ONE benchd loads a golden of ANY declared track. A golden of a different track
is refused. The refusal names the track, the `model_type` the track needs and the `model_type` the
golden declares.

These commands need a track id, because each one reads or writes a golden:

* `benchd iterate` and `benchd correctness` read the `MLXFAST_QWEN_MTP_TRACK_ID` environment
  variable.
* `benchd validate-golden` takes a `--track` option. It falls back to the same environment
  variable.
* `benchd measure-job` uses the track id it already resolves for the seal.
* `record-correctness-golden` takes a `--track` option, because it AUTHORS a golden under that
  track's identity.

## Scored regime

Each track ALSO declares what it scores: the batch size of its scored point, and the two exponents
that make the composite. That declaration is a second per-track table in the same file, read with
`scored_regime(track_id)`. A track that is not in it is `SCORED_REGIME_PENDING` and cannot be
scored on any path.

The declaration decides whether benchd enforces anything on the prefill half of the timed window.
See [`scored-regime-and-prefill-window.md`](scored-regime-and-prefill-window.md).

A new track therefore declares THREE facts before it can score: its model identity, its official
baseline pair (captured by the procedure above), and its scored regime. None of the three has a
fallback.

## Goldens

Track goldens are authored on the track's designated benchmark hardware, A≡B double-generated
(byte-identical asserted before pinning), pinned by sha256 + bytes, and uploaded append-only to
R2 under `{track_id}/{sha256}.json` once the track is stable and ready for testing — with
GET + sha + bytes round-trip verification after upload.

The R2 key prefix is the **`track_id`**, not the branch name. Two platforms that share one release
branch therefore keep two separate golden namespaces. Each platform authors its goldens on its own
hardware, so the two namespaces must not be merged.
