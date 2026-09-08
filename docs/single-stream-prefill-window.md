# Single-stream prefill window (wire contract)

Ruling (David, 2026-08-27): score the Qwen 3.8 125B-A6B tracks (mlx and cuda)
single-stream, paired serial vs the built-in mtp, on
`prefill_gain ^ 0.25 * decode_gain ^ 0.75`. This page is the wire contract for the
prefill half of that score on the single-stream series. The engine side and the
bench side implement it from this page.

## 1. Summary

- No new message. No new field on the wire. The single-stream free-run verbs
  stay `free_decode_begin` and `free_decode_run`.
- benchd splits its own parent clock at the boundary between the two verbs.
  This is the same split the batched verbs already use.
- The engine does not emit a timing. The engine must do all seed-prefill work
  inside `free_decode_begin`.
- The track fixture declares `scored_batch_size: 1` and `scored_exponents`
  `{prefill_gain_exponent: 0.25, decode_gain_exponent: 0.75}`. benchd
  certifies both and seals the composite on the paired single-stream series.

## 2. The two windows

benchd runs one fresh worker for each leg. For each leg it opens two
contiguous windows on its own clock (`std::time::Instant`):

| Window | Opens | Closes | Token count |
|---|---|---|---|
| `prefill_elapsed_seconds` | immediately before benchd sends `free_decode_begin` (with the golden's `decode_seed_tokens`, 1024 tokens, and the requested `spec`) | immediately after benchd validates the response `seed_token` against the golden's `expected_decode_seed_token` | `prefill_token_total` = `decode_seed_tokens.len()` |
| `decode_elapsed_seconds` | the instant the prefill window closes | when `free_decode_run(N)` returns | `decode_token_total` = `N` (128) |

Rules:

- There is no untimed gap between the two windows.
- `elapsed_seconds` = `prefill_elapsed_seconds + decode_elapsed_seconds`, by
  construction. benchd never measures the whole window a second time.
- `seconds_per_token` = `elapsed_seconds / N`. This is unchanged. It stays
  the enforced whole-window figure for the serial band, the run timeout and
  the paired decode-only median.
- The seed oracle check is charged to the prefill window. The run oracle check
  is outside both windows.
- The RunTimeout deadline is armed when the prefill window opens and covers
  both windows, as today.

## 3. Engine obligations

1. `free_decode_begin` must run the full seed prefill (all `decode_seed_tokens`)
   and must reply only after the seed forward is complete. The reply carries
   `seed_token` (the argmax after the full seed) and the echoed `effective_spec`.
2. `free_decode_run` must not prefill. It must decode from the state that
   `free_decode_begin` left. It must not re-run any part of the seed.
3. The engine must not do any prefill work before `free_decode_begin` arrives.
   benchd spawns a fresh worker for each timed leg, so there is no earlier
   state to reuse.
4. The hello must advertise `free_run_decode`. The single-stream series does
   not use `batched_free_run_decode` or `max_batch_size`. benchd does not read
   them on this series.
5. Units on the wire stay as they are. benchd's clock is in seconds (f64).

If the engine moves prefill work into `free_decode_run`, the prefill window
gets smaller and the decode window gets larger by the same amount. The whole
window does not change. The composite then moves against the engine, because
the decode exponent (0.75) is larger than the prefill exponent (0.25).

## 4. What benchd seals

In `results.json`, per accepted pair (field name unchanged from the batched
series; the shape is the B=1 point of the same type):

```
pairs[].cohort_phase_windows = {
  serial_prefill_window_seconds, candidate_prefill_window_seconds,
  serial_decode_window_seconds,  candidate_decode_window_seconds,
  prefill_token_total, decode_token_total
}
```

At the top level of `results.json`, when the fixture declares the B=1 point:

```
composite_scored_exponents = { prefill_gain_exponent: 0.25, decode_gain_exponent: 0.75 }
composite = { prefill_gain, decode_gain, composite_score,
              composite_speedup_floor, composite_speedup_floor_met }
composite_absent_reason = <string>     # present only when composite is absent
```

Exactly one of `composite` and `composite_absent_reason` is present.

The gains are ratios of sums over the accepted pairs of the whole pinned pool:

```
prefill_gain = sum(serial_prefill_window_seconds) / sum(candidate_prefill_window_seconds)
decode_gain  = sum(serial_decode_window_seconds)  / sum(candidate_decode_window_seconds)
composite    = prefill_gain ^ 0.25 * decode_gain ^ 0.75
```

A serial candidate (`teacher_forced_v1` series, the calibration path) opens
no prefill window on either leg. benchd then seals `composite_absent_reason`
(naming the series) and no `composite`; the record stays valid. A single-stream
free-run leg that carries no phase window is refused before the pair is
accepted. A teacher-forced leg that carries a phase window is
refused. The B=8 batched series keeps its meaning: its windows and its
composite are unchanged.

## 5. What does not change

- `free_decode_begin` / `free_decode_run` request and response fields.
- The captured engine-wire fixture (`ENGINE_WIRE_V1_SHA256`). No re-pin.
- The serial band, the decode-only median, the floor (0.90) and the ceiling
  (5.0) on the single-stream series.
- The batched series (`scored_batch_size: 8`).
