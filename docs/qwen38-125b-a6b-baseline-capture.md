# Qwen 3.8 125B-A6B baseline capture

This page tells the box agent how to capture the official baselines for the
Qwen 3.8 125B-A6B tracks. One bench tree serves two engines. The track id
selects the platform: `qwen3.8-125b-a6b-mlx-v1` or `qwen3.8-125b-a6b-cuda-v1`.
Do the procedure once for each platform.

Do not run this procedure on a laptop. Run it on the ranked box only. This page
names that box `ai-server`.

## 1. What you measure

The tracks are single-stream. Each track fixture declares `scored_batch_size: 1`.
benchd never runs the batched cohort for these tracks.

Scoring is SINGLE-LEG. `benchd iterate --mode official` times one leg (MTP on
the timed leg) and scores it against the pinned baseline. The composite formula
is unchanged. There is no paired serial-control leg.

You capture ONE thing for each platform: the official baseline. It is the serial
prefill and decode seconds-per-token pair plus the four acceptance-band
tolerances, held in `OfficialBaseline` in `crates/bench-core/src/constants.rs`.
Every scored `benchd iterate` run scores against it. Until you capture them,
`OFFICIAL_BASELINE_MLX` and `OFFICIAL_BASELINE_CUDA` are `None`, and benchd
refuses to score by name:

- `QWEN38-125B-A6B-MLX-PENDING-ORGANIZER`
- `QWEN38-125B-A6B-CUDA-PENDING-ORGANIZER`

The pool has EIGHT prompts. Each prompt carries its OWN baseline in its golden:
`baseline_prefill_seconds_per_token` and `baseline_decode_seconds_per_token`.
A scored run of one prompt scores against that prompt's own pair.

### Band shape

The bands are FIXED LITERALS. Do NOT derive them from a session CV. The shape is
the one the MTP timed leg needs (David ruling):

- Prefill: +/-5 %, SYMMETRIC.
- Decode UP: +2 %.
- Decode DOWN: DISABLED. MTP spec-decode decode is much faster than the serial
  baseline, so a lower band would wrongly fail a healthy run as "improvement too
  large". The 0.95 decode speedup FLOOR is the only lower guard the decode axis
  needs.

The `decode_down_enabled` field on `AcceptanceBands` carries the disabled state.
Set it `false` for these tracks; the scored path then skips the decode lower
bound and keeps the decode upper bound. The band VALUES land at calibration; the
`OFFICIAL_BASELINE_*` constants stay `None` until then.

## 2. Before you start

1. Merge the engine PR first. Then verify it independently. Record the merged
   head SHA. Write that SHA into the capture record. A capture from an
   unmerged or unverified engine is void.
2. Confirm the track fixture (`benchmark.json` in the engine repository)
   declares:
   - `track_id` = `qwen3.8-125b-a6b-mlx-v1` or `qwen3.8-125b-a6b-cuda-v1`;
   - `scored_batch_size` = `1`;
   - `target.upstream_model_id` and `target.upstream_revision` equal to the
     platform's `TrackReferenceModel` in `constants.rs`.
   benchd refuses a fixture that pins another checkpoint (die 8, before any
   golden loads).
3. Build benchd from the merged bench branch. Record its commit SHA.
4. Export the track id for every command below:

```sh
export MLXFAST_QWEN_MTP_TRACK_ID=qwen3.8-125b-a6b-mlx-v1   # or ...-cuda-v1
```

## 3. GPU lock protocol on `ai-server`

The box agent fills in the concrete commands. Keep the order.

1. Take the lock.
2. Unload the resident Qwen service.
3. Run the capture (sections 4 and 5).
4. Reload the resident Qwen service.
5. Release the lock.

`scripts/baseline-capture.sh` has one function for each step. The functions
`lock`, `unload`, `reload` and `release` exit with code 2 until the box agent
fills them in.

## 4. Capture the per-prompt baselines

Preconditions. Confirm all of them before the first run:

1. The hidden pool material is staged on the box. Staging is organizer-gated;
   ask David before you stage anything new.
2. The track is NOT armed yet (`official_scoring_enabled` is absent or false).
   The capture uses `--capture-baseline`, which writes no score and no integrity
   sidecar, so it never seals a ranked artifact while the track is unarmed.
3. The official correctness golden is staged; every run passes it as `--golden`.

The pool has EIGHT prompts. Each prompt gets its OWN baseline pair. Capture uses
`benchd iterate --capture-baseline <RECORD>` on the stock (unmodified) tree.
The mode appends the run's prefill and decode seconds-per-token to the capture
record and writes NO score. On a track whose baseline is already captured the
mode refuses by name, so it can never double as a scoring bypass.

**Run the correctness gate ONCE per prompt per window (a8/David ruling).** The
engine cannot change between passes of your own calibration, so the teacher-forced
correctness gate is redundant after the first pass. That gate is PLE-SSD-bound and
takes about 7 to 8 minutes per pass. Run it on the WARMUP pass of each prompt (the
`[W]`-labeled pass, WITHOUT `--capture-timed-only`); its timing is EXCLUDED. Run
every measured pass — the two A passes and the two B passes — WITH
`--capture-timed-only`. That flag skips the gate and runs ONLY the timed prefill
and decode pass, then appends the pair. The timed pass and its free-run seed check
are unchanged on every pass.

**Hash the weights ONCE per window (Option B digest-hoist).** The weights tree is
immutable for the whole window, and hashing the ~105 GB tree costs time on every
pass. benchd hashes the files in parallel, and it uses the CPU SHA-256
instructions: an M5 digests 6.4 GB in about 1.5 seconds, where one thread of
portable code needs about 23 seconds. The cost is still paid once per pass, so
the hoist still removes it from every pass but the first. Compute the digest ONCE
at window start with
`benchd weights-digest --weights "$WEIGHTS_DIR"`, which prints the stable form
`<sha256>:<bytes>:<files>`, and pass that value to every subsequent pass via
`--weights-digest`. The passed digest is BYTE-IDENTICAL to the per-pass
`dir_digest` a run would compute for itself — it is produced by the SAME
`dir_digest`, not a shell-side sha reimplementation — so no measured or scored
number changes. Each weights digest also prints one line to stderr —
`weights digest: <bytes> in <s> s, <threads> threads` — so you can see what the
seal cost on this box; stderr is not sealed. `--weights-digest` is refused at
parse WITHOUT `--capture-baseline`
(mirroring `--capture-timed-only`): an official/scored run always hashes for
itself, so a passed-in digest can never reach a scored seal.

Calibration shape (David ruling). Per prompt: **1 unmeasured warmup + A×2 + B×2**.
Over the EIGHT prompts that is **40 runs total, 32 scored into the mean** (four
measured passes per prompt). The CV is computed over the four MEASURED passes per
prompt. A==B is compared 2-vs-2: `mean(A1, A2)` against `mean(B1, B2)`. The one
warmup pass per prompt is `[W]`-labeled and EXCLUDED from the mean, the CV, and the
A==B comparison.

Cost: one per-prompt teacher-forced warmup gate plus cheap timed passes, and ONE
weights hash for the whole window. A full window is about boot + 1 weights hash + 8
warmup gates (one per prompt) + 32 fast timed passes — not the roughly 9 to 10
hours a re-gated, re-hashed window would hold the GPU lock.

Rules for the capture:

1. **Five runs per prompt: one warmup, then A×2 and B×2.** Each is a fresh
   invocation gated on the box's cool-down rule. Only the warmup runs the
   correctness gate; the four measured passes use `--capture-timed-only`. The
   warmup is `[W]`-labeled and excluded; only the four measured passes score.
2. **One window per box.** Measure ALL EIGHT prompts in ONE calibration window
   — a single model boot. Do not reboot the model between prompts; the weights
   load once for the whole window and are hashed once for the whole window.
3. **Arm ONE live prompt first.** Author the baseline for the one live prompt
   first. Author the other seven and keep them ready for the organizer to rotate
   in; do not arm them yet.

`--capture-timed-only` and `--weights-digest` both need `--capture-baseline`. Each
is refused by name without it, so neither can ever reach a scored or official run.

```sh
export MLXFAST_QWEN_MTP_TRACK_ID=qwen3.8-125b-a6b-mlx-v1   # or ...-cuda-v1

# Hash the immutable weights ONCE at window start; every pass below reuses this.
WEIGHTS_DIGEST=$(benchd weights-digest --weights "$WEIGHTS_DIR")

# WARMUP [W] — runs the correctness gate once for this prompt; timing EXCLUDED.
benchd iterate --mode local-iterate \
  --engine "$STOCK_WORKER" --weights "$WEIGHTS_DIR" \
  --golden "$POOL_DIR/<prompt>.json" \
  --capture-baseline "$CAPTURE_DIR/baseline.<prompt-id>.W.json" \
  --weights-digest "$WEIGHTS_DIGEST"

# A record, PASSES A1 A2 (repeat x2) — skip the gate, time and append only.
benchd iterate --mode local-iterate \
  --engine "$STOCK_WORKER" --weights "$WEIGHTS_DIR" \
  --golden "$POOL_DIR/<prompt>.json" \
  --capture-baseline "$CAPTURE_DIR/baseline.<prompt-id>.A.json" \
  --capture-timed-only --weights-digest "$WEIGHTS_DIGEST"

# B record, PASSES B1 B2 (repeat x2) — skip the gate; the warmup already gated
# this prompt for the window.
benchd iterate --mode local-iterate \
  --engine "$STOCK_WORKER" --weights "$WEIGHTS_DIR" \
  --golden "$POOL_DIR/<prompt>.json" \
  --capture-baseline "$CAPTURE_DIR/baseline.<prompt-id>.B.json" \
  --capture-timed-only --weights-digest "$WEIGHTS_DIGEST"
```

The record carries the identity (track id, mode, engine sha256, golden sha256 —
a run from a different engine or golden refuses to merge), every run's pair, and
the run count.

If the timed pass fails (for example, the cool gate stops because the GPU stays
hot), the mode records nothing and prints `capture refused: timed phase failed:`
with the cause. Correct the cause and run the pass again.

## 5. Assemble the official baseline

For each prompt, the baseline pair is the MEAN of the four MEASURED passes
(A1, A2, B1, B2), per axis — the warmup pass is excluded. Write
`baseline_prefill_seconds_per_token` and `baseline_decode_seconds_per_token` into
that prompt's golden. Use the fixed band shape from section 1 (prefill +/-5 %
symmetric; decode +2 % up; decode down disabled) for every prompt.

The A and B passes are captured into separate record files within the one window,
so the 2-vs-2 A==B check (section 6) can read them apart.

## 6. Double generation: A must equal B

Do not commit a value until A equals B.

- For every prompt, compare the two record halves 2-vs-2 — `mean(A1, A2)` against
  `mean(B1, B2)` — per axis. They must agree within 1 % per axis. If any prompt is
  outside 1 % on either axis, discard that prompt's passes and run section 4 again
  for it.
- Compute the CV over the four measured passes per prompt (A1, A2, B1, B2); the
  warmup pass is not part of it.
- The bands are fixed literals (section 1), so they are identical by
  construction — there is nothing to reconcile there.

Keep the A and B artifacts. Attach them to the pull request that commits the
values.

## 7. Replace the sentinel with the value

Do these steps in one commit. The mirror test fails if you do only one side.

1. In `crates/bench-core/src/constants.rs`, set `OFFICIAL_BASELINE_MLX` (or
   `OFFICIAL_BASELINE_CUDA`) to `Some(OfficialBaseline { ... })`. Use the exact
   bits from the capture for the pair. The bands are the FIXED LITERALS from
   section 1 (prefill +/-5 % symmetric; decode +2 % up; `decode_down_enabled:
   false`) — do not derive them.
2. In `crates/benchd/tests/fixtures/swift-official-baseline-constants.json`,
   under the platform key (`mlx` or `cuda`), replace each sentinel string with
   the same value (including `decodeBandDownEnabled`). Replace `source_commit`
   with the engine commit SHA that carries the same values in its
   `Constants.swift`. Replace `captured` with the capture date. Replace
   `source_lines` with the line range in that file.
3. Run:

```sh
cargo test -p benchd official_baseline_mirrors_the_reference_constants_capture_per_platform
```

4. Run the negative controls again. They must stay red for gemma and green for
   the track:

```sh
cargo test -p bench-core --test loader_parity negative_control_gemma_golden_refuses_by_name
cargo test -p bench-core --test loader_parity positive_control_pinned_track_golden_is_accepted
cargo test -p bench-core --test loader_parity cuda_platform_pin_controls
cargo test -p benchd run_baselines_refuses_by_name_while_the_official_baseline_is_pending
```

   The last test skips a platform whose baseline is captured. It must still
   pass for the other platform while that one is pending.

5. Stage each prompt's golden — carrying its `baseline_prefill_seconds_per_token`
   and `baseline_decode_seconds_per_token` (section 5) — to the place the track's
   runner reads its goldens from. Staging is organizer-gated. Arm the one live
   prompt first; keep the other seven ready for rotation. Do not commit the
   goldens into this repository.

6. Run the full suites and clippy:

```sh
cargo test -p benchd --bin benchd
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 8. What a value must never be

- Never type a number from memory or from another track. The gemma values
  and the Qwen 3.8 27B values are not valid for these tracks.
- Never put a number in place of a sentinel on one side only.
- Never capture on a box that is not `ai-server`, or from an engine head that
  is not the merged and verified head.
