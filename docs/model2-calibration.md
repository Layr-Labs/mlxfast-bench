# Model-2 New-Series Calibration Plan

Status: DESIGN (doc-first). Cycle-4 item 4. This is the spec the on-box calibration legs
will follow; no code changes accompany it. Every mechanism claim below is grounded in the
repo at `path.rs:line`. The bands themselves are NOT authored here — they are MEASURED on
the box per §4.

Terminology used throughout:

- **Native regime** — the old box-wrapper flow where `live-measure-qwen-mtp-job.sh` (cited
  as `W:` in the code) owned the clock and authored the numbers. Its calibration is the
  track-fixture parity data baked into `crates/bench-core/src/constants.rs`.
- **Model-2** — the benchd-driven flow where **benchd owns the clock**: the scored
  seconds-per-token is benchd's own parent-side wall clock
  (`crates/bench-runner/src/timing.rs:603` `measure_decode`, `:683` `measure_free_run_decode`,
  both timing with `std::time::Instant`), and no worker-reported duration can be the score
  source. Since #109 window-2 finding 3 retired the `--mtp-report` file (and with it
  `MtpTimedReport.parent_measured_seconds_per_token`, which was already demoted to audit/echo
  only), benchd's clock is the ONLY parent-clock number that exists in the path at all.

---

## 1. Why a new series (hard rule)

Model-2 is a **declared series break** from the native regime. A number produced under
benchd's clock measures a different physical quantity than a number produced under the box
wrapper's clock, so:

> **HARD RULE.** Model-2 numbers are NEVER compared to native-regime numbers. The board
> frontier `1.376`, the native calibration constants
> (`QWEN_MTP_EXPECTED_RAW_MEDIAN = 0.9940390645`,
> `crates/bench-core/src/constants.rs:67`; band `±2.0%`, `:71`), and any pre-existing
> BASELINE_CALIBRATION serial band authored under the wrapper are **native-regime artifacts**.
> They do not gate, seed, or sanity-check a Model-2 run. A Model-2 run is gated ONLY against
> Model-2 calibration.

### Where the code segregates the series

The segregation is machine-enforced by the `timed_mode` series tag, not by convention:

- `crates/bench-core/src/free_run.rs:22` — `TIMED_MODE_FREE_RUN_V1_1 = "free_run_v1_1"`,
  the tag every v1.1 free-run score record carries.
- `crates/bench-core/src/free_run.rs:27` — `TIMED_MODE_TEACHER_FORCED_V1 = "teacher_forced_v1"`,
  its counterpart.
- `crates/bench-core/src/free_run.rs:35` — `timed_modes_comparable(a, b)` returns `a == b`:
  two timed numbers are comparable ONLY if they carry the same series tag. The doc-comment
  states the intent verbatim — "Baselines, speedup floors, and acceptance bands are all
  per-series; a v1.1 run is gated only against v1.1 calibration."
- `crates/bench-runner/src/timing.rs:243` / `:309` — the free-run result stamps
  `timed_mode: TIMED_MODE_FREE_RUN_V1_1` onto every `FreeRunTimingResult`, so downstream
  aggregation "can never silently mix this number with the v1 teacher-forced series."

Model-2's scored decode leg IS the free-run series (see §5). Its records therefore carry
`timed_mode = free_run_v1_1`, and `timed_modes_comparable` makes any cross-series comparison
a code-level false. That predicate is the enforcement point for the hard rule above.

---

## 2. What CARRIES (David's rulings, unchanged)

The scoring *shape* is a ruling, independent of which clock produced the seconds-per-token.
It carries into Model-2 unchanged. Only the calibration *bands* are re-measured (§3).

| Ruling | Value | Code |
|---|---|---|
| Serial-anchored scoring (serial control = 1.0, no normalization) | `serial = 1.0` | `crates/benchctl/src/measure_job.rs:90` `SCORE_ANCHOR_SERIAL_ONE`; serial leg runs the timed verb at depth 0, `:47` `SERIAL_CONTROL_DEPTH = 0` |
| Per-prompt RAW serial-relative decode ratio | `serial_decode_spt / candidate_decode_spt` | `crates/bench-core/src/score.rs:339` `paired_decode_raw_ratio` (reuses `speedup`, `:14`) |
| Median rule — even-n mean of the two central order statistics | `even_n_mean_of_two_central_order_statistics` | `crates/bench-core/src/score.rs:348` `paired_decode_only_median`; sealed name `crates/benchctl/src/measure_job.rs:100` `MEDIAN_RULE_EVEN_N`, aggregation `:94` `SCORING_AGGREGATION_MEDIAN_OF_PER_PROMPT` |
| Submission floor on the raw median | `0.90` | `crates/bench-core/src/constants.rs:52` `QWEN_MTP_DECODE_SPEEDUP_FLOOR` |
| Ceiling on the raw median | `5.0` | `crates/bench-core/src/constants.rs:56` `QWEN_MTP_DECODE_SPEEDUP_CEILING` |
| Per-pair plausibility bound (rejects before aggregation) | `8.0` | `crates/bench-core/src/constants.rs:61` `QWEN_MTP_PER_PAIR_RATIO_BOUND` |

The gate that applies them is `crates/bench-core/src/score.rs:420`
`score_paired_decode_only`, with the exact priority order **per-pair plausibility (8.0) →
non-finite median → floor (0.90) → ceiling (5.0)** (`:433`–`:457`). Its failure taxonomy is
`PairedDecodeFailure` (`:367`: `PerPairBound` / `NonFiniteMedian` / `Floor` / `Ceiling`),
each of which names its failing bound in `metrics.error` (`:381` `message()`). The identity
run (candidate == serial ⇒ ratio 1.0 ⇒ median 1.0) scores exactly `1.0`
(`:664` test `paired_identity_run_scores_one`). Floor/ceiling edges are inclusive
(`:713` test `paired_floor_and_ceiling_edges_inclusive`).

None of these constants or functions change for Model-2. A candidate that cannot beat serial
is expected to stop drafting and take `1.0`; below `0.90` the run floor-fails (score null).

---

## 3. What is FRESH: the Model-2 calibration bands

**The expected-median calibration band and the serial band must be RE-MEASURED under
benchd's clock. No native band gates a Model-2 run.**

There are two distinct calibration mechanisms in the code, and BOTH need fresh Model-2
measurements. They serve different purposes and neither is derived from the other.

### 3a. The serial band (`BASELINE_CALIBRATION`) — the live die-6 gate

This is the enforcement path that actually gates a run on the box. It checks that the box's
pooled serial denominator has not drifted from a calibrated reference at the SAME window.

- **File shape** — `crates/benchctl/src/measure_job.rs:848` `BaselineCalibration`:
  optional top-level `serial_decode_seconds_per_token_mean` + `serial_band_low` (default
  `0.95`, `:825`) / `serial_band_high` (default `1.05`, `:828`) + optional `decode_tokens`,
  plus a `targets` map of per-target entries. Per-target entry `:871` `TargetCalibration`
  carries its own `serial_decode_seconds_per_token_mean`, optionally overrides the band, and
  REQUIRES `decode_tokens` (no inherit — a target without it is a parse error). Parsed
  fail-closed (`:897` `parse`).
- **Resolution** — `crates/benchctl/src/measure_job.rs:911` `BaselineCalibration::resolve`:
  a declared `--target-id` resolves `targets[<tid>]` (band inheriting top-level defaults,
  window from the target); no target resolves the top level. A `--target-id` with no matching
  entry is a **miswired rotation → die-6** (a target must never be banded against another
  entry's baseline). The resolved value is `:885` `ResolvedCalibration`
  (`serial_mean`, `band_low`, `band_high`, `decode_tokens`, honest `source`).
- **Band-enforcement path** — after measuring, `crates/benchctl/src/measure_job.rs:1016`
  `evaluate_serial_band` computes `ratio = pooled_serial_mean / calibration_mean` and checks
  `ratio ∈ [band_low, band_high]` (`:1112`–`:1113`), with the window check HARD:
  `decode_tokens` must be present AND equal `--tokens` (`:1046`–`:1075`) — seconds/token is
  not comparable across token counts because the seed prefill is charged inside the decode
  window. The verdict is `:972` `SerialBandVerdict` (`Pass` / `WarnOutOfBand` / `Die6`), sealed
  into provenance as `:985` `SerialBandOutcome`. `crates/benchctl/src/measure_job.rs:1154`
  `enforce_serial_band` turns a `Die6` into an `Err` (the die/reject).
- **Die / reject semantics** —
  - Window missing/mismatched → `Die6` **always** (never downgraded), `:1046`–`:1075`.
  - Measured pooled mean non-finite/≤0 → `Die6` **always**, `:1079`.
  - Out-of-band ratio, or an invalid/unrecorded calibration mean → `Die6` under enforcement,
    `WarnOutOfBand` (does not die) only when `BASELINE_BAND_ENFORCE=0`, `:1092`, `:1126`.
  - `BASELINE_BAND_ENFORCE` parsed fail-closed: UNSET and empty-string both mean ENFORCED;
    only an explicit `"0"` disables — `:837` `band_enforce_from_env`. "A missing band must
    never read as in band."
  - A MISSING calibration under enforcement fails closed via the config
    (`:801` `MeasureJobConfig.calibration: Option<...>`, `:803` `band_enforce`).

**Model-2 adaptation.** What is fresh is the file's *contents*: the per-target
`serial_decode_seconds_per_token_mean` and `decode_tokens` must be authored from Model-2 legs
(benchd's clock), at the Model-2 window, in the series the run measures. A native-regime
`serial_mean` would drift-fail every honest Model-2 run (the two clocks measure different
quantities) and must not be installed.

**AMENDED — the schema is NOT unchanged (the approved divergence).** This section originally
said the `BASELINE_CALIBRATION` *schema* is unchanged. That could not survive §1: a rule that
"a Model-2 run is gated ONLY against Model-2 calibration" is unenforceable while the file
carries no statement of which series it was measured in. The band-gating code (PR #105
cycle-5, on `measurement-in-benchd`) therefore gave the schema that identity, and this doc's
§1 enforcement point is now real rather than advisory. What changed:

- **Two REQUIRED file-wide fields.** `timed_mode` (the series the means were measured in) and
  `track_id` (the track the file was authored for) —
  `crates/benchctl/src/measure_job.rs@ffea0d3:1100` and `:1105`, both plain `String` with no
  `#[serde(default)]`. They are file-wide identity, not per-target: one file holds one
  series and one track.
- **An UNTAGGED file is REFUSED, not defaulted.** Because the two fields have no serde
  default, a legacy file that omits either fails `BaselineCalibration::parse`
  (`@ffea0d3:1169`) outright. That is deliberate and is the whole point: inferring a missing
  `timed_mode` — from the file's age, its path, or the run reading it — would reinstate
  exactly the silent cross-series banding §1 forbids. A calibration that will not say which
  quantity it measured cannot gate anything.
- **The fence itself.** `enforce_calibration_series_fence` (`@ffea0d3:1280`) runs on the
  `BASELINE_CALIBRATION` **pre-read**, before any measuring and therefore before any banding,
  and die-6s on either mismatch. The series half delegates to
  `bench_core::free_run::timed_modes_comparable` — the predicate §1 and §5 name, and this is
  its production caller.
- **Wrapper-authored files RE-AUTHOR; they are not retro-tagged.** The live wrapper's
  `write_calibration_bootstrap` shape (`{track_id, targets{...}}`, W:1468-1528) has no
  `timed_mode`, so every such file is untagged and now refused. The migration is to
  **re-measure under `--calibration-bootstrap`**, which authors the band together with the
  series and track of the run that measured it (`build_bootstrap_calibration`
  `@ffea0d3:1548`); the authored file then passes its own fence on the next same-series
  same-track run and die-6s against any other. Authoring also refuses to merge a band into a
  file declaring a different `timed_mode`/`track_id` (`@ffea0d3:1595`) — the authoring path
  must not manufacture the mislabeling the fence exists to catch. Per §4 this is not a
  hardship: no Model-2 band exists to preserve, so every Model-2 file is authored fresh
  anyway.
- **Which series gets stamped.** The run's own — teacher-forced for a Model-2 TF run,
  `free_run_v1_1` for a v1.1 free-run run (§5). Both the pre-read check and the bootstrap
  stamp key on that one value, so a free-run run bands only against free-run calibration.

§3a's substantive requirement is unaffected by the divergence: fresh Model-2 *contents*, no
native band installed. It is now machine-enforced rather than advisory.

### 3b. The expected-median envelope — the aggregate serial-denominator sanity band

The median-level analogue of the serial band: what an UNMODIFIED (stock depth-2) tree scores
under the raw serial-anchored semantics. In the native regime this is
`QWEN_MTP_EXPECTED_RAW_MEDIAN = 0.9940390645` ± `QWEN_MTP_CALIBRATION_BAND_PCT = 2.0%`
(`crates/bench-core/src/constants.rs:67`, `:71` — both stamped
`// UNVERIFIED(measure-job)`: track-fixture parity data measured on the native wrapper over
six gated sessions, **NOT re-derived against benchd's clock**).

- **Predicate** — `crates/bench-core/src/score.rs:299` `within_calibration_band(measured,
  expected, band_pct)`: `measured ∈ expected·(1 ± band_pct/100)` inclusive, FALSE on any
  non-finite input (fail-loud — a NaN measurement is never "in band"). This is the primitive
  behind the authoritative spec's `serial_denominator_banding`; per its own doc-comment
  (`:296`–`:298`) live single-box enforcement needs an on-box serial calibration reference,
  so the caller enforces it only when a reference is actually supplied.

**Model-2 adaptation.** The stock-tree expected raw median must be re-measured under benchd's
clock and a Model-2 `expected`/`band_pct` established. Until then this envelope is
report-only for Model-2 (no native `0.9940…` value gates a Model-2 run).

### 3c. Calibration procedure (the legs the box will run)

The bands come from serial-anchored legs measured on the box under benchd's clock:

1. **Serial-vs-serial legs** — run the timed verb at depth 0 on BOTH legs (serial control vs
   serial control) at the Model-2 window. The per-pair ratio is ~1.0 by construction; the
   pooled serial mean these legs produce is the `serial_decode_seconds_per_token_mean` written
   into `BASELINE_CALIBRATION` for §3a. Serial-vs-serial isolates the box's serial denominator
   from any drafter behavior.
2. **Serial-anchored stock legs** — run the stock depth-2 candidate against the serial
   control, aggregate to the even-n raw median (`paired_decode_only_median`). That median is
   the Model-2 `expected` for §3b's envelope; its spread over gated sessions sets the
   Model-2 `band_pct`.
3. **Authoring** — the band is authored ONLY off a fully-accepted, parity-true run:
   `crates/benchctl/src/measure_job.rs:1171` `should_author_bootstrap(candidate_accepted,
   parity_all_ok)`. `--calibration-bootstrap` (`:805` `MeasureJobConfig.calibration_bootstrap`)
   is an authoring path, not merely "skip the check." The merged file is built by
   `:1199` `build_bootstrap_calibration` (preserving every other target, recording the entry's
   own `decode_tokens` + depth, marked `provisional`) and installed atomically by
   `:1273` `write_bootstrap_calibration` (temp-sibling + rename).

Windows/pairs: the legs run at the Model-2 `--tokens` window and target/min pairs; the
authored `decode_tokens` MUST equal that window or §3a's HARD window check die-6s every
subsequent run.

---

## 4. Sequencing

1. **Now (this doc).** Specify the plan. No bands are authored; no code changes.
2. **Wire-seam repair + real handshake.** Model-2 bands are measured on the box only AFTER
   the benchd↔engine wire seam is repaired and a real handshake completes end-to-end. The
   handshake mechanics that must work first:
   - the free-run capability advertisement — `crates/bench-runner/src/timing.rs:286`
     refuses the free-run mode fail-closed with `CapabilityNotAdvertised`
     (`CAPABILITY_FREE_RUN_DECODE`) when the engine's hello does not advertise it;
   - spec-never-ignored echo — `measure_free_run_decode` (`:701`
     `free_decode_begin_spec`) discards the session unless the engine's echoed
     `effective_spec` equals the request, and benchd seals that echo per leg
     (`crates/benchctl/src/measure_job.rs:1341` `EffectiveSpec`).
   A band measured before a real handshake would be measuring plumbing, not the engine.
3. **Author + enforce.** Run the §3c legs, author the provisional Model-2 band via
   `--calibration-bootstrap`, then enforce it (die-6) on subsequent Model-2 runs.

> **Until step 3 lands, no Model-2 run is band-gated by any old band.** A native-regime
> `BASELINE_CALIBRATION` must not be installed for Model-2, and the native
> `QWEN_MTP_EXPECTED_RAW_MEDIAN` envelope is report-only for Model-2. The other Model-2
> gates that do NOT depend on a measured band (per-pair `8.0` bound, non-finite median,
> floor `0.90`, ceiling `5.0`, parity, the completed-work barrier) remain live throughout —
> only the serial/median *drift* band waits for step 3.

---

## 5. v1.1 free-run note (why the scored path is free-run)

The scored Model-2 MTP path is the **v1.1 free-run** series specifically, because
teacher-forced decode cannot let a drafter speculate:

- `crates/bench-runner/src/timing.rs:603` `measure_decode` (v1, teacher-forced) feeds the
  ORACLE token forward as each next input (`:633`–`:641`:
  `input_token = expected_decode_seed_token` for step 0, else
  `expected_decode_tokens[decoded_step - 1]`). Because every next input is fixed to the oracle,
  a speculative drafter has nothing to speculate — each step is a single forced advance. MTP
  acceptance cannot register, so a genuine MTP speedup is invisible in the teacher-forced
  series.
- `crates/bench-runner/src/timing.rs:683` `measure_free_run_decode` (v1.1) drives
  `free_decode_begin` then `free_decode_run(N)`, letting the engine free-run its OWN committed
  tokens, and verifies the committed stream against the golden (`:728`–`:743`, §2.7 hard fail).
  Only here does MTP acceptance count. This result carries `timed_mode = free_run_v1_1`
  (`:309`).
- The runner doc-comment states it directly (`crates/bench-runner/src/timing.rs:220`–`:222`):
  the free-run decode leg is a NEW SERIES whose `decode_seconds_per_token` MUST NEVER be
  compared to a v1 teacher-forced number.

**Consequence for calibration.** The calibration legs in §3c MUST be measured on the
**free-run series** (`timed_mode = free_run_v1_1`) — the same regime the scored candidate runs
under. A serial band or expected-median envelope measured under the teacher-forced series would
be a different physical quantity and, by `timed_modes_comparable`
(`crates/bench-core/src/free_run.rs:35`), is not comparable to the scored free-run number. The
Model-2 serial-vs-serial and stock-anchored legs therefore run through
`run_free_run_timed_benchmark` (`crates/bench-runner/src/timing.rs:258`), and the authored
`BASELINE_CALIBRATION` is a free-run-series calibration.

---

## Appendix: liveness budget (not a calibration input)

For completeness — the RunTimeout budget shares the ceiling vocabulary but is a LIVENESS
bound, never a score or calibration input. `crates/bench-core/src/score.rs:314`
`run_timeout_budget(n, band_ceiling_spt, margin)` = `N × band-ceiling × margin`
(`RUN_TIMEOUT_MARGIN = 4.0`, `crates/bench-core/src/constants.rs:78`; fallback per-token
ceiling `:84` when no `BASELINE_CALIBRATION` is available, e.g. the free-run path). On timeout
benchd raises `RunTimeout`, discards the session fail-closed, and the pair fails — it never
enters the median. Calibration bands (§3) do not depend on it.
