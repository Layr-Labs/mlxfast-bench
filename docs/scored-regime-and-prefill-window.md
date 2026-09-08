# Scored regime and prefill-window certification

This page has three parts. Part 1 tells you how a track declares WHAT it scores. Part 2 tells you
what benchd does with the prefill half of the timed window, and when it enforces anything on it.
Part 3 is a normative limit on part 1: no track may declare a nonzero prefill exponent yet.

Parts 1 and 2 are one mechanism. The declaration decides whether the certification is armed.

## 1. The scored regime

Each track declares ONE scored regime. The regime has three values:

| value | what it says |
|---|---|
| `scored_batch_size` | the batch size of the point the track scores. `1` is the single-stream point. |
| `prefill_gain_exponent` | the exponent on the prefill gain in the composite. |
| `decode_gain_exponent` | the exponent on the decode gain in the composite. |

The composite is:

```
composite = prefill_gain ^ prefill_gain_exponent * decode_gain ^ decode_gain_exponent
```

> **DECLARED, NOT YET COMPUTED.** `bench_core::score::composite_score` has NO production call site.
> Nothing in a run computes a composite today, and no `prefill_gain` or `decode_gain` is derived
> from the sealed windows. The published figure still comes from `score_paired_decode_only` — the
> even-n median of the per-prompt raw decode ratios — exactly as before this page existed.
>
> The declaration says what a track scores. Computing it is a later change, and it is gated on the
> work-placement invariant in part 2.

The regimes are in a per-track table in `crates/bench-core/src/constants.rs`, beside the per-track
official baseline table. The branch names its own track in `TRACK_ID` in the same file.

Read the table with `scored_regime(track_id)`. It is the only accessor. A track that is not in the
table has no regime. That state is `SCORED_REGIME_PENDING`. The accessor refuses such a track, and
the refusal names the `track_id`, the sentinel, and the tracks that DO declare a regime. Do not
read the table in any other way.

A track that is absent has no entry. It does not have a placeholder entry. There is no exponent
pair for a new track to inherit by accident.

### What this branch's track declares

| track | `scored_batch_size` | `prefill_gain_exponent` | `decode_gain_exponent` |
|---|---|---|---|
| `qwen3.8-27b-mtp-v1` | 1 | 0.0 | 1.0 |

This RECORDS what the track already does. It does not change it.

The track scores the single-stream paired point, decode only. The published figure is the even-n
median of the per-prompt raw decode ratios. There is no separately scored prefill phase: the seed
prefill runs INSIDE the one timed decode window, which is why the run seals
`prefill_component: "none"`.

Decode-only is therefore `0.0` and `1.0`. With those two exponents the prefill factor is `1.0` and
the decode factor is the decode gain, so the composite is `1.0 * decode_gain`.

That is the decode gain bit for bit for every finite and infinite decode gain, and for a QUIET NaN.
It is not bit-for-bit for a SIGNALLING NaN: multiplying by `1.0` quiets it, which flips the payload
bit. No path produces a signalling NaN here — a gain comes from a division of two measured
durations — so the identity holds for every value this code can see. The wording matters because
"for every value whatsoever" would be false.

The prefill gain is not read at all: an exponent of exactly `0.0` makes the factor `1.0` without
touching the gain, so the prefill value can be anything, including a NaN or an infinity, and the
composite does not move.

The test `constants::tests::the_declared_regime_reproduces_todays_score` pins that identity. Change
either exponent and the test fails.

### Where the fence is

A track must declare its regime before a run scores against it. Four sites refuse a pending track
by name:

| site | what it is |
|---|---|
| `official::official_resolved_baselines` | the official path's baseline resolution |
| `iterate::local_mode_baselines` | the local legs' baseline resolution |
| `measure_job::resolve_track_id` | the paired run's one track-resolution point |
| `overlay::validate_results` | benchd's own `overlay-timing` merge |

All four are inside benchd. The ORGANIZER's merge is a separate system and this page does not
fence it: a regime declared here constrains what benchd will measure and seal, not what the
organizer accepts.

The fence in `official_resolved_baselines` is OUTSIDE the resolution it wraps. The baseline pair
has three sources and only the last one reads the track table, so a fence inside the resolution
would be stepped around by an environment pair or by a golden that declares a pair. The fence
outside it is not.

## 2. Prefill-window certification

### The split

benchd owns the only clock that scores anything. On a free-run leg it opens the window immediately
before it sends `free_decode_begin`, and it closes the window when `free_decode_run(N)` returns.

Inside that one window the engine does two things: the seed prefill, then the free-run decode. The
boundary between them is a message boundary that benchd drives. So benchd SPLITS ITS OWN CLOCK at
that boundary.

| window | opens | closes | tokens |
|---|---|---|---|
| `prefill_elapsed_seconds` | immediately before `free_decode_begin` is sent | immediately after the response `seed_token` is validated against the golden | the seed length |
| `decode_elapsed_seconds` | the instant the prefill window closes | when `free_decode_run(N)` returns | N |

Rules:

* There is no new message and no new wire field. The verbs stay `free_decode_begin` and
  `free_decode_run`.
* The engine reports no duration. No message on this protocol carries one.
* One clock reading serves as both the close of the prefill window and the open of the decode
  window. There is no untimed gap between them.
* The seed oracle check is charged to the prefill window. The run oracle check is outside both
  windows.
* The WHOLE window is measured once, end to end. It is not rebuilt as the sum of the two halves.
  `seconds_per_token` divides that whole window, exactly as before. The split cannot move it.

### When certification is armed

`bench_core::prefill_window::certify_prefill_window` decides what the split is worth. The track's
declared regime arms it, and nothing else does.

**ARMED** — `prefill_gain_exponent` is not `0.0`. The track's score moves when the prefill window
moves, so the window is held to three checks. Each breach refuses BY NAME and quotes an
exact-match sentinel:

| condition | sentinel |
|---|---|
| no window was observed on the leg | `PREFILL-WINDOW-NOT-OBSERVED` |
| a part of the window is not finite and positive, or a token total is zero | `PREFILL-WINDOW-NOT-OBSERVED` |
| the two halves do not account for the whole window, beyond `PREFILL_WINDOW_TOLERANCE` | `PREFILL-WINDOW-DISAGREES` |
| an independently reported prefill duration disagrees with benchd's own, beyond the same tolerance | `PREFILL-WINDOW-DISAGREES` |

A leg refused this way records the reject class `prefill-window-uncertified`.

A malformed exponent — negative, or not a number — reads as ARMED. A bad declaration must refuse,
not disarm the checks.

**Arming prefill is a TRACK-WIDE switch, not a per-pair one.** A teacher-forced leg drives neither
free-run verb, so it opens no prefill window at all. Under an armed regime that is
`PREFILL-WINDOW-NOT-OBSERVED` — so declaring a nonzero prefill exponent refuses EVERY teacher-forced
pair on the track, including the calibration path. A track that arms prefill is a track that runs
the free-run series only. Decide that before you change the exponent, not after the first run.

`PREFILL_WINDOW_TOLERANCE` is a pinned relative tolerance. It covers the arithmetic of a split and
ordinary clock jitter between two readings. It is NOT a performance band.

The reported-duration check has no source today, because no message carries a duration. Every
current call site supplies `None`. The parameter is the seam a reported duration would arrive
through. If one ever does, it is checked against benchd's clock, not trusted in place of it.

### Two facts, sealed separately

An armed pass records TWO things, and they are not the same thing:

| sealed field | what it says |
|---|---|
| `certified` | the checks were ARMED and they passed |
| `cross_check` | `"not-observed"` — no second measurement of this window existed; or `"agreed"` — one existed and agreed inside the tolerance |

Today `cross_check` is ALWAYS `"not-observed"`, because no message carries a duration. So an armed
pass means SELF-CONSISTENT — the two halves account for the whole window benchd measured — and NOT
corroborated. The state is sealed as a name, not inferred from a `false`, because "we compared and
it matched" and "there was nothing to compare" are different claims.

A disagreement never seals. It refuses, so there is no third sealed state.

**NOT ARMED** — `prefill_gain_exponent` is exactly `0.0`. The track scores no prefill, so nothing
can be protected by enforcing anything on the window. The window is REPORT-ONLY: benchd measures
it, records it, and enforces nothing. No refusal in this module can fire.

### What is sealed

Each accepted pair whose two legs both opened a free-run window seals one block in `results.json`:

```
pairs[].phase_windows = {
  serial_prefill_window_seconds, candidate_prefill_window_seconds,
  serial_decode_window_seconds,  candidate_decode_window_seconds,
  prefill_token_total, decode_token_total,
  certified, cross_check
}
```

`certified` states whether the checks were applied, and `cross_check` states whether a second
measurement corroborated them. A reader never has to infer either.

The prefill half is named `serial_prefill_window_seconds` / `candidate_prefill_window_seconds` on
this block. In `bench_runner` the same quantity is `PhaseWindow::seed_prefill_elapsed_seconds` — the
`seed_` prefix is deliberate, because `FreeRunTimingResult` also carries a `prefill_elapsed_seconds`
and that one is a DIFFERENT quantity: the v1 prefill PHASE, a separate timed round trip on its own
prompt, outside this window entirely.

The block is OMITTED on a teacher-forced pair. Those legs drive neither free-run verb, so they open
no prefill window, and a half-populated block would invite a gain computed against a leg that never
measured one.

A prefill-scoring regime WOULD compute its gains from these sums over the accepted pairs. Nothing
computes them today (see part 1):

```
prefill_gain = sum(serial_prefill_window_seconds) / sum(candidate_prefill_window_seconds)
decode_gain  = sum(serial_decode_window_seconds)  / sum(candidate_decode_window_seconds)
```

### On this branch

`qwen3.8-27b-mtp-v1` declares `prefill_gain_exponent: 0.0`. Certification is not armed. Every pair
seals `certified: false` and `cross_check: "not-observed"`, no scored value reads the window, and the `prefill-window-uncertified`
class is unreachable. The test
`measure_job::tests::prefill_certification_is_report_only_on_this_branchs_track` proves this end to
end, from the mock engine through the runner's clock split to the sealed pair record.

## 3. NORMATIVE: no track may declare a nonzero prefill exponent yet

Certification binds the SUM of the two halves. It does not bind WHERE THE WORK SITS inside them.

The split is a message boundary, not a work boundary. An engine that already knows the seed token —
and the golden's seed token is a fixed, repeated value — can reply to `free_decode_begin`
immediately and do the seed prefill inside `free_decode_run` instead. Every certification check
still passes: both halves are finite and positive, and they still sum to the whole window benchd
measured, because the work only MOVED between them. The whole window does not change, and
`seconds_per_token` does not change.

What changes is the ratio. The prefill half shrinks toward the round-trip cost of one message, so
`prefill_gain` grows without bound while `decode_gain` absorbs the moved work. A composite that
weights prefill at all therefore rewards deferral, and certification cannot see it.

**Therefore: no track may declare a nonzero `prefill_gain_exponent` on `main` until a
WORK-PLACEMENT INVARIANT exists.** Either of these closes it; neither is built:

1. a per-token FLOOR on the prefill half, from a calibrated reference measured on the track's own
   hardware — a seed prefill cannot be faster than the reference by more than the calibration band;
2. a golden-derived prefill/decode RATIO BAND — the golden fixes the seed length and N, so the
   expected ratio of the two halves is a property of the fixture, and a run outside the band is
   refused.

Hidden prompts MITIGATE this — an engine that cannot predict the seed token has less to gain from
deferring — but they do NOT close it. The deferral does not need the token's value: the engine can
reply with whatever it computes and move the remaining forward work past the boundary. Hidden
prompts raise the cost of the attack; they do not make the measurement sound.

The `0.0` exponent on `qwen3.8-27b-mtp-v1` is what makes this a future concern rather than a live
one. It is not a placeholder to be raised casually.
