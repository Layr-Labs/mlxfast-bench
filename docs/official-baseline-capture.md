# Official baseline capture

Class: runbook. **Stored-pair tracks only.**

This page tells you how to capture a STORED-PAIR track's official baseline pair. The pair is the
serial prefill and decode seconds-per-token that such a track scores against.

The Qwen 3.8 125B-A6B tracks store no pair. They measure their denominator live, on the box, in
the same job, and they refuse `--capture-baseline` by name
(`CAPTURE-RETIRED-FOR-LIVE-CONTROL-LEG`). What their boxes calibrate instead is a health band —
see [`qwen38-125b-a6b-baseline-capture.md`](qwen38-125b-a6b-baseline-capture.md).

Read [`track-release-branches.md`](track-release-branches.md) first. It gives the rules for the
per-track baseline table. This page gives the procedure.

## 1. When you use this procedure

Use it when a track is `OFFICIAL_BASELINE_PENDING`. A pending track has no entry in
`OFFICIAL_BASELINES_BY_TRACK` in `crates/bench-core/src/constants.rs`.

Do not use it on a track that has an entry. A captured track refuses this mode by name.

## 2. Why the mode exists

The local checked-timing legs resolve the track's official baseline before they start the
engine. A pending track therefore refuses the run before it measures anything. The measurement
that ends the pending state could not run.

`benchd iterate --capture-baseline` is the one safe way out of that loop. The mode has three
properties. Together they make sure that it cannot become a way to score without a baseline.

1. The mode runs only while the track is pending. A captured track refuses the mode by name.
2. The mode runs the checked-timing legs with an inert baseline pair. It resolves no official
   baseline, and it computes no score.
3. The mode writes one file: the capture record you name. It writes no `score.json`, no
   `.sha256` sidecar, and no integrity sidecar. It stops before those writers.

## 3. Before you start

1. Run this procedure on the track's own benchmark hardware. Do not run it on a laptop.
2. Merge the engine change first. Then verify it independently. Record the merged head SHA.
   A capture from an unmerged engine or an unverified engine is void.
3. Build benchd from `main`. Record its commit SHA.
4. Confirm that the track is pending in `crates/bench-core/src/constants.rs`.
5. Stage the correctness golden and the transformed weights on the box.

## 4. Capture the pair

Run four passes. Use one fresh invocation for each pass. The mode starts its own worker for each
pass. Obey the box cool-down rule between the passes.

Give every pass the same record path. The mode merges each pass into that one record.

```sh
benchd iterate --mode local-iterate \
  --engine "$STOCK_WORKER" \
  --weights "$WEIGHTS_DIR" \
  --golden "$CORRECTNESS_GOLDEN" \
  --capture-baseline "$CAPTURE_DIR/official-baseline.json"
```

Use the stock tree. Do not modify the engine for the capture.

`--capture-baseline` is a local-iterate mode. `--mode local-submit` and `--mode official` refuse
the flag when you parse the command line.

Know the regime you capture in, and keep it the same for all four passes. The mode runs
`local-iterate`, whose cool gate is OFF by default. Add `--cool-gate` to make it ON. The box
cool-down rule tells you which of the two to use. Use the same one for every pass, and name it in
the pull request.

One pair serves BOTH local legs. `local-submit` scores against the same captured pair, but it
runs its cool gate ON by default, and it uses a decode window of 1023 steps in place of 128. This
difference is not new, and this page does not change it. Ruling #127 makes both local legs score
against the table, and the table holds one pair for each track. Record the regime you captured
in, so that a later reader knows what the number means.

## 5. What the record holds

The record is a JSON object. It holds values only.

| field | what it holds |
|---|---|
| `track_id` | the track this tree serves |
| `mode` | the mode the passes ran in |
| `decode_steps` | the decode window the passes measured over |
| `engine_sha256` | the sha256 of the engine binary |
| `weights_sha256` | the sha256 of the transformed weights directory |
| `golden_sha256` | the sha256 of the golden bytes |
| `benchd_sha256` | the sha256 of the benchd binary that did the timing |
| `run_count` | the number of passes in the record |
| `runs` | each pass's prefill and decode seconds-per-token |
| `prefill_cv_percent`, `decode_cv_percent` | the per-axis CV of the passes |

The seven identity fields must agree across the passes. A pass with a different value in any one
of them refuses to merge. The refusal names the field. Start a new record file for a new engine,
a new checkpoint, a new golden, a new window, or a new benchd.

Each identity field is there because the pair depends on it. Different weights measure
differently. A different decode window has a different cost for each token. benchd does the
timing, so the harness that measured is part of the measurement. Two passes over different inputs
must not average into one mean that describes neither of them.

`CV` means the SAMPLE coefficient of variation, as a percent. It is the standard deviation with
the N−1 denominator, divided by the mean. It is the sample form, not the population form,
because the passes are a sample of the box's run-to-run distribution and N is small. The CV is
absent from the record until the record holds two passes. Two passes are the minimum for the
statistic.

The mode writes the record with a temporary file and a rename. An interrupted pass therefore
cannot leave a part-written record.

## 6. If a pass refuses

The mode refuses a pass that did not measure. It prints the cause. Correct the cause, then run
the pass again.

| refusal | cause |
|---|---|
| `--capture-baseline refused: track ... is not OFFICIAL-BASELINE-PENDING-CAPTURE` | the track has a captured pair; there is nothing to capture |
| `--capture-baseline refused: the engine ... did not resolve` | benchd could not read the engine binary, so it cannot name the engine |
| `--capture-baseline refused: the correctness gate failed` | the run is not healthy; do not capture from it |
| `capture refused: timed phase failed: ...` | the timed phase did not produce a measurement; the text after the colon is the cause |

A refused pass does not change the record.

## 7. What you do with the record

The record is the INPUT to a separate pull request. That pull request replaces the track's
pending state with a value: it adds the track's pair to `OFFICIAL_BASELINES_BY_TRACK` in
`crates/bench-core/src/constants.rs`.

The record is not itself a baseline. No code reads it back. A person reads it, and a reviewer
checks the pull request against it.

Put this in the pull request body:

* the whole capture record, with every identity field: `track_id`, `mode`, `decode_steps`,
  `engine_sha256`, `weights_sha256`, `golden_sha256`, `benchd_sha256`;
* the merged engine head SHA and the benchd commit SHA;
* the cool-gate setting the passes used.

The record is OPERATOR-ATTESTED. It is not machine-attested. benchd merges each pass into
whatever record parses at the path you give it, and no code reads the record again after that. A
person can thus write a record by hand, or change one. The reviewer of the sentinel-to-value pull
request must therefore use `engine_sha256`, `weights_sha256` and `benchd_sha256` as the
load-bearing evidence. Check each of the three against the merged engine, the staged weights, and
the built benchd. Do not read them as labels.

The band tolerances that go with the pair are track policy, not part of this mechanism. This
page does not give the band arithmetic. Ask the track's ruling for it.
