# Parity completion gate — goal, instrument, and the frozen corpus

> # ✅ SIGNED — this is the frozen gate definition
>
> **Signed by David, 2026-08-20**, via the structured Q-block interview recorded as
> **`q-block-137-signoff-rulings`**. All nine questions (Q1, Q1a, Q2–Q8) are answered; the
> answers are folded in-place below and summarised in §7. **This document is now binding:** it is
> the program's definition of done. Changes to it require a new ruling, not an edit.
>
> **What was signed (Q1, CONFIRMED):** that **P1 security, P2 Yukon compatibility, and P3
> benchmark-execution equivalence** are what *done* means; and that **this corpus plus D-1 and
> D-2 are sufficient evidence** for those three prongs.
>
> **Citation discipline (standing).** Every claim about corpora, rosters, classes or code is
> `path@sha:line` or an issue URL. Claims no window has produced carry `UNVERIFIED`.
>
> | tag | repo / path | sha | role |
> |---|---|---|---|
> | **BENCHD** | `davidtai/mlxfast-bench` `main` | **`35c100aa`** | the frozen state — **the freeze snapshot** |
> | *(superseded)* | `98f44fa5` | | the earlier freeze point; **all `@98f44fa` / `@140878b` cites from prior drafts are re-derived here** |
> | **REF** | `davidtai/mlxfast-qwen-38-27b-mtp-engine` | `b26f76f1` | the Swift reference |
> | **TRACK** | `fixtures/qwen3_8_27b_mtp_track.json` | sha256 `5677d53f…83ad1`, 44589 B | the track contract — pins the prompt pool |
> | **DIFFER** | `benchctl parity-diff` | `parity-diff v2 roster58/8c2f2d99` | `crates/benchctl/src/parity.rs@35c100a:392-408` |
>
> **PR #133 has MERGED** — commit `35c100aa`, 2026-08-20T15:45:13Z. Its follow-up review commit
> `f8ed398` (F-1…F-7) landed *before* the first draft of this document was written and resolved
> two of that draft's open questions; both are struck here. **The freeze snapshot moves from
> `98f44fa5` to `35c100aa`.** Every line-number cite below is re-derived at `35c100aa`; the
> `failed_payload` call sites in particular **shifted** (`454→455`, `547→539`, `562→563`,
> `588→581`, `627→619`), so prior-draft numbers must not be carried forward.
>
> **The freeze snapshot is a PIN, not a tracking reference.** `main` has since advanced past
> `35c100aa` (#136 merged as `1465393`). The snapshot deliberately does **not** follow it — a gate
> that re-derives itself against a moving head is not frozen. Cites were **re-verified against
> `1465393` at signing time** and hold: the seven `failed_payload` sites are byte-identical
> (`431 455 515 539 563 581 619`), the blank seal is intact, and `official.rs` still emits six
> failure shapes. §2.1 already requires re-derivation at battery time against the recorded
> snapshot; that rule is what absorbs future drift, not edits to this document.

---

## 1. THE GOAL — three prongs

> **David, verbatim:** *"as long as security and yukon both work and the benchmark runs the same
> way… that's the goal."*

That sentence is the completion definition. Everything below serves it.

| prong | what it means | in-gate test |
|---|---|---|
| **P1 — SECURITY** | benchd must never be a softer target than the reference on anything a submission can reach | Is the divergence **submission-reachable**, and is benchd **LOOSER** there — accepts what REF rejects, fails differently on hostile input, or leaks via failure records? **Looser ⇒ IN-GATE, always.** Stricter ⇒ a compatibility question, declarable. |
| **P2 — YUKON COMPATIBILITY** | the board must be able to ingest what benchd seals | Does the surface feed board ingestion — `score.json` shape, sealed artifacts, the `.sha256` / integrity chain, the `benchmark.json` manifest, exit codes the workflow reads? **Yes ⇒ IN-GATE.** |
| **P3 — BENCHMARK-EXECUTION EQUIVALENCE** | the benchmark must run the same way | Same inputs → same accept/reject decisions, same measurement methodology, same scores / floors / calibration verdicts. |

### 1.1 The razor

**A cell or case serving NONE of the three prongs is out-of-scope BY DEFINITION** — no per-cell
ruling required. Failure-record cosmetics on a path that is not submission-reachable, not
Yukon-visible, and not scoring-bearing need no adjudication at all; they need a one-line razor
cite (§5).

**This is a purpose statement, not doctrine relaxation.** Full-parity remains the default. The
razor governs the freeze's *boundary calls* and *priority order* — submission-reachable and
Yukon-visible first.

**Reach tags.** Every case and every out-of-scope line carries
`reach ∈ {submission-reachable | yukon-visible | execution-equivalence | multiple | none}`, so
the boundary is visible per row. Case tables are ordered with `multiple` and
`submission-reachable` rows leading.

### 1.2 Byte-parity is the INSTRUMENT, not the goal

The differ battery byte-compares sealed artifacts across a frozen corpus. That is **not** the
objective — it is the cheap exhaustive check that *evidences* P1/P2/P3. Bytes are how we prove
"same accept/reject decision" without arguing each field, and how we prove "the board can ingest
this" without hand-inspecting artifacts.

The consequence matters for boundary calls: a byte difference that evidences none of the three
prongs is not a parity defect worth gating on, and a *behavioural* divergence that bytes happen
not to catch is still in-gate. §5's re-sweep applies exactly this.

### 1.3 The restructure (David's four terms)

- **(a) The matrix row set freezes NOW.** `docs/parity-matrix.md@35c100a` §1 carries **29 surface
  rows** (`:35-63`). Closed. New findings become corpus cases (§4) or out-of-scope lines (§5) —
  never new cells.
- **(b) The corpus is all modes × all failure classes × all golden variants**, seeded from
  `scripts/gen-failure-corpus.py@35c100a`, `scripts/gen-variant-corpus.{sh,py}@35c100a`, and the
  item-1-era 7-class target.
- **(c) Completion = the battery GREEN** at pinned SHAs, benchctl-vs-swift side-by-side on-box,
  byte-compared per the differ rules, **including F1(b) blank-seal semantics** once #133 lands.
- **(d) Cells the differ structurally cannot cover** close via exactly **two named
  deliverables** (§2.4).

`docs/parity-matrix.md` demotes from gate to **map**: what each surface is, where ground truth
lives, which case gates it. Its drift-gate hygiene rule survives (`@35c100a:6-7`); the completion role
moves here.

> **Freeze-snapshot correction — do not freeze known-stale prose.** The frozen row at
> `docs/parity-matrix.md@35c100a:43` (*loader decisions (accept/reject)*) still carries **M-4-era
> evidence that its own sentence then contradicts**: it opens with *"12-fixture corpus … box
> dual-loader match=167 / declared-div=16"* and closes with *"declared-div 16→15"*. Neither
> opening number is current, and the corpus is not 12. **The correct evidence, per §4.3 and §4.4
> below, is two separate corpora:**
>
> | corpus | fixtures | box result |
> |---|---|---|
> | loader-decision (`golden_parity`) | **15** | `match=14 / known-divergence=1 / MISMATCH=0` |
> | structure-aware fuzz (`golden_fuzz`) | **183** | `match=168 / known-divergence=15 / MISMATCH=0` |
>
> The `match=167 / div=16` pair was the pre-**#77** fuzz result; #77's fix moved it to 168/15, and
> the row records that move without updating its own headline. **The row freezes with this
> correction attached** — the map may carry history, but the gate must not inherit a stale
> headline. *(Bookkeeping on the map, not a new cell — term (a) is not engaged.)*

### 1.4 Secret-tier handling — binding

1. **The corpus manifest carries PINS ONLY** — filename, sha256, byte count. **Never bytes,
   never prompt content, never excerpts, never decoded text, never token sequences.**
2. **Derived artifacts inherit the tier.** Failure-class mutants generated from an R2 base live
   **only in the window workspace** and are **never committed** — a one-token flip from a
   held-out base *reveals the base*. Same for any transplant, re-provision or regeneration off
   an R2 base.
3. **The local mirror is READ-ONLY.** `~/projects/layr-labs/r2-official-inputs` is
   evidence, never a design input, never modified, never re-uploaded. Organizer material is
   report-only even where it looks defective.
4. **Fetch-and-verify happens at window time**, inside the preflight gate, against TRACK pins.
5. **Credentials** (R2, endpoint, M5) live in `.env` + ssh-config only; alias/env-name references
   everywhere else; never in repos, issues, PRs or artifacts.
6. **Synthetic corpora stay committed as-is** — `golden_fuzz` (183), `golden_parity` (15), the
   Swift-capture fixtures. They carry no organizer content and cover loader/schema dimensions the
   track pool cannot.

### 1.5 BINDING TERMINOLOGY — what may be called a "golden"

**RULED David 2026-08-20 (stated twice, `q-block-137-signoff-rulings`). This governs this
document and every artifact the program produces from here on.**

> **"Golden" is reserved for artifacts carrying the full binding: weights-hash + prompts +
> prompt-SHAs.** Nothing else may be called a golden.

The word had drifted into a generic label for "JSON fixture we compare against", which hid real
distinctions — most damagingly, it made eight **tapes** and a set of **box-generated comparison
targets** read as the same class of object as the organizer's scored reference material. They are
not, and the difference is exactly what §3.3 FT-1 turns on.

**Use the artifact's real name:**

| term | what it means | examples here |
|---|---|---|
| **golden** | full weights-hash + prompts + prompt-SHAs binding | the organizer's hidden correctness reference; the §8 comparison goldens; `GoldenDocument`-schema artifacts |
| **tape** | teacher-forcing reference rows — `seed_tokens` / `reference_seed_token` / `rows` | the 8 R2 track pool objects (§3.2) |
| **fixture** | a pinned test input, no binding claimed | `golden_parity` (15), `golden_fuzz` (183), family N's arity inputs |
| **reference output** | what REF emitted, captured for byte-compare | `swift-early-refuse-failure-record.json` |
| **comparison target** | a box-generated document a leg scores against | family V's section variants |
| **corpus case** | one enumerated row of this gate | `L-01`, `Z-137`, `R-10` … |

**Two carve-outs, both narrow.** (1) **Verbatim identifiers are never renamed** — `--golden`,
`validate-golden`, `generate-golden`, `GoldenDocument`, `golden_hash`, `golden_sha256`,
`golden_parity`, `golden_fuzz`, `Golden.swift`, `Mode::golden_required_steps`,
`classify_golden_input`. Renaming a symbol to satisfy a prose rule would break every citation.
(2) **Quoted source text and quoted rulings stay verbatim** — including David's own term (b)
wording *"all golden variants"* (§1.3), which in this document's vocabulary means *all
input-document variants*.

*Audit performed at signing: 102 occurrences reviewed, 31 renamed, 71 kept as identifiers,
quotations, or genuine goldens. Family V's label changed from "across golden SECTIONS" to
**"across input-document SECTIONS"** — noted here once for continuity.*

---

## 2. THE INSTRUMENT — battery + two deliverables

### 2.1 Pinned-SHA discipline

A run is admissible only if its REPORT records: (1) benchd commit + binary sha256; (2)
`mlxfast-swift` at REF + the standalone engine binary sha256 — the §8 re-verify used
`davidtai/mlxfast-engine@c83f9de6`, explicitly *not* `mlxfast-qwen-38-27b-mtp-engine`;
(3) the differ version string, which
fingerprints the roster and the three tolerances so a battery cannot silently run under a
changed policy; (4) every **input document** by **sha256 + byte count** — both halves, the `bytes` half
ENFORCED when declared (#112 L3); (5) the TRACK fixture sha256 (see FT-4, §3.3); (6) weights dir
digest, file count, byte count. Any leg that cannot record all six is a TOOL-ERR, never a verdict.

### 2.2 On-box side-by-side

Both runners execute on the **same box, same GPU window, same weights, same pinned inputs**, each
into its own artifact directory; then a fixed artifact set is compared file-by-file.

| driver | pairs | dirs |
|---|---|---|
| `official-parity.sh@35c100a` | `benchctl iterate --mode official` vs **direct** `mlxfast-swift benchmark` (official is env-driven; no `--official` flag on the trusted binary) | `$OUT/pair.$i/{bc,sw}` (`:65`) |
| `facade-leg.sh@35c100a` | benchd's facade `scripts/benchmark.sh` vs the **real** reference `benchmark.sh`, both local modes | `$OUT/{ref,fac}.<mode>` (`:101-117`) |
| `variant-parity.sh@35c100a` | `benchctl iterate` vs `mlxfast-swift benchmark` per variant per applicable mode | `$OUT/score.{bc,swift}.$cls.$mode.json` (`:49-70`) |
| `loader-parity.sh@35c100a` | `benchctl validate-golden` vs `mlxfast-swift preflight` — no GPU | report |
| `failure-map.sh` / `official-failure-map.sh@35c100a` | both runners over the corruption corpus, field-diffed on the shared failure surface | per-class dirs |

The swift side's seal is the **STDOUT bytes**, not the on-disk `--score-path` file
(`official-lib.sh@35c100a:172-173`, enforced by `official_seal_stdout` `:102-112`). Both sides get
the same `MLXFAST_COMMIT_SHA` so `metrics.commit` matches (`:68-94`).

### 2.3 Byte-compare rules (reference, not restatement)

The differ is `benchctl parity-diff` (`crates/benchctl/src/parity.rs@35c100a`);
`scripts/parity-diff.py@35c100a` is a one-release **shim** with no verdict logic (`:2-12`) and a
distinct `SHIM_TOOL_ERR = 9` so a tool problem never renders as a verdict (`:14-19`).

`ROSTER` at `parity.rs@35c100a:44-110` — exactly **58** entries (56 `metrics.*` + top-level
`score`, `passed`), the `roster58` in the version string (`:406`). Six buckets (`:22-38`).

| rule | effect | site | prong |
|---|---|---|---|
| unknown key either side | HARD FAIL `UNKNOWN-FIELD` | `:282-292` | P1, P2 |
| rostered key missing either side | HARD FAIL `SCHEMA-DRIFT-MISSING` | `:293-303` | P2 |
| `Det` / `Failing` | exact, or 1e-9 ⇒ `~ulp` creep line | `:347-355`, tol `:112` | P3 |
| `DetTol` (`peak_ram_gb`) | within 5% ⇒ creep line, else HARD FAIL | `:341-346` | P3 |
| `Timed` | 10% band; in FailingPair mode waived only if `failed ⇒ zeroed/null` holds for **each** side | `:321-340`, `side_ok` `:332-338` | P3 |
| `Error` | compared by failure **class**, not string | `error_class()` `:158-183` | P3 |
| non-bool `passed` | HARD FAIL `PASSED-NOT-BOOL`, never coerced | `:259-267` | P2 |
| non-string `error` | `<non-string-error:{v}>` so off-schema cannot collapse to `""` and PASS | `:162-165,181` | P1 |
| `Env` waiver | the **only** waiver; every entry needs a ☑-signed §13 ledger row | `:810-845` | — |

**GREEN = `hard_fail.is_empty()`** (`:205-209`) → `PARITY: PASS` (`:495-502`), exit 0. **Only
exit 0/1 are verdicts.** Build-time drift gates (fail `cargo test`, not a live window):
`roster_covers_score_metrics_exactly` (`:515-566`) and
`mutate_every_field_flips_verdict_unless_waived` (`:629-654`).

**The sealed artifacts — this set IS the Yukon ingestion surface (P2):**

| # | artifact | file | compared how |
|---|---|---|---|
| 1 | sealed score payload | `score.local-iterate.json` / `score.json` | `parity-diff`, 58-key roster |
| 2 | score naming | per-mode basename | same basename both sides — `official-parity.sh:85-86` |
| 3 | sha256 sidecar | `<score>.sha256` | present both sides; first field == true `shasum -a 256` of its own score — `:88-96`; writer `official-lib.sh:125` (two-space form) |
| 4 | integrity JSON | `benchmark-integrity.local-iterate.json` / `.json` | **superset**: benchd ⊇ REF's 9 fields as a byte-exact PREFIX; surplus == the runner-identity roster EXACTLY; plus deterministic VALUE compare of `score_path`(basename), `weights_sha256`/`_file_count`/`_byte_count`, `golden_sha256`, `golden_path` — `:98-123`, `facade-leg.sh:175-214` |
| 5 | exit code | `exit_code` | identical; a missing artifact fails LOUD |

**Declared exceptions:** `score_sha256` (hashes the timing-bearing payload) and
`transform_source_sha256` (Swift computes fresh `source_hash()`; benchd reads the
`<weights>/.benchmark-source.sha256` marker, `""` when absent) —
`official-parity.sh@35c100a:99-101,116-118`, `facade-leg.sh@35c100a:178-184,206-209`.

**Runner-identity roster is a moving number.** 7 keys at BENCHD
(`scripts/fixtures/integrity-runner-keys.json@35c100a:26-34`); PR133 F3 adds an 8th,
`candidate_executable_resolution`. The battery reads the roster from that file, never a copy — it
exists because the roster previously had four independent encodings (`:2-25`).

### 2.4 The two deliverables (term (d))

"Structurally cannot cover" = no second implementation to diff; or the artifact is authored by a
trusted party benchd does not control; or the surface is a live pipeline, not a document.

#### D-1 — the SEALING MEASURE-JOB WINDOW · *reach: multiple (P2 + P3)*

One on-box `measure-job` window over the track pool (§3) that runs to completion and **seals its
full artifact set**, calibration band exercised.

*Why the differ can't:* `measure-job` is **Model-2** — benchd owns the clock
(`crates/bench-runner/src/timing.rs@35c100a:603`, `:683`, both `std::time::Instant`) and there is
no Swift counterpart. Model-2 is a **declared series break**: `timed_modes_comparable(a,b)`
returns `a == b` (`crates/bench-core/src/free_run.rs@35c100a:35`), making cross-series comparison
a code-level false. The gate is "seal a self-consistent, fully-pinned artifact set", not
"byte-match REF".

*Must show:* (1) `accepted_pair_count >= min_pairs` with
`.accepted_pair_count == (.pairs | length)` — Proof A got `accepted_pairs = 0`, exit 5 (#134);
(2) the alternating pair loop actually alternated, visible per-pair in `results.pairs[]`;
(3) the sealed set `results.json` + `.sha256` + `benchmark-integrity.results.json` **(P2 — this
is board ingestion)**; (4) the calibration band **resolved and enforced**
(`measure_job.rs@35c100a:1016`, outcome as `:985` `SerialBandOutcome`), recording that bands are
`provisional: true` (#109) and that the bootstrap leg was **SUPERVISED** — the `free_run_v1_1`
series starts UNBANDED and no unattended bootstrap may author one; (5) floor/ceiling on real
hardware — `decode_speedup_floor: 0.9`, `published_speedup_ceiling: 5.0`,
`timed_mode: free_run_v1_1`, with a **measured median clearing the floor** (Proof A confirmed
seal *shape* only; medians were `0.0` because no pair was accepted,
[#117](https://github.com/davidtai/mlxfast-bench/issues/117)); (6) **runner identity pinned by
BYTES** — #135 found the first on-hardware `benchmark-integrity.results.json` names executables
by **PATH** and digests **workspaces**, with **4 of 8** roster keys absent and
`metrics.commit = ""`. **RULED (Q6): TWO ARTIFACTS** — `results.json`'s sidecar keeps the organizer shape; the local `benchmark-integrity.*.json` carries the full 8-key roster. D-1 seals both, and #123's justification stops citing measure-job as its bar.

*Closes:* M11 (GPU legs), the live half of M04 for the `results` surface, family T's GPU legs.
*Formerly blocked by #134 — **now UNBLOCKED**.* PR #136 (`1465393`) landed the worker-stderr
surfacing on the failed-hello path, together with a secret-scrubber
(`crates/bench-runner/src/scrub.rs`) hardened against real secret shapes. The original symptom —
`transport.rs@35c100a:410-412` documents forwarding with `WORKER_STDERR_FORWARD_PREFIX` (`:155`)
and a 64 KiB retained tail (`:159`), yet **not one `mlxfast-worker:` line** reached any Proof-A
leg's stderr — is the defect that fix addresses. D-1 is now gated only on a window being run.

#### D-2 — the RANKED-WORKFLOW MIRROR · *reach: multiple (P1 + P2)*

A read-only, sha-pinned mirror of the **live** ranked pipeline, diffed against benchd's
reconstruction.

*Why the differ can't:* the ranked seal is authored by the **organizer's trusted shell** (seam 3);
benchd does not author the ranked `score.json` at all. The comparison is *benchd's reconstruction
vs the deployed pipeline*.

The B-4 mirror already RESOLVED two of ten unknowns:
#1 the ranked yml (**Y**@`ebedcc72`), #2 the contract fixture (**F**@`5e67d60`); #7 is
mirror-KNOWN. D-2 closes the rest:

1. **Epoch coherence — a two-level defect, D-2's first item.** *Level one, recorded:* the wrapper
   seals `TRACK_ID="qwen3.8-27b-mtp-v1"` (W:260) while the mirrored ranked yml (Y:167) and
   fixture (F) expect `qwen3.6-27b-mtp-v1` — the 3.8 wrapper's sealed `results.json` would be
   **REJECTED** by the mirrored 3.6 overlay's `.track_id` gate (Y:557, Y:2596). *Level two, found
   by this document (FT-4, §3.3):* the mirror's own
   `qwen38-main-checkout/fixtures/qwen3_8_27b_mtp_track.json` declares
   `track_id: qwen3.8-27b-mtp-v1` but pins the **3.6-epoch prompt pool** with real per-prompt
   `noop_decode_speedup` values (0.7623 … 1.0845) instead of the ratified `1.0`. D-2 mirrors the
   go-live 3.8 yml **and** fixture. Standing rule: **do not hard-code — resolve the track_id from
   the deployed workflow.**
2. **The engine's timed-report schema** (§8 #3) — the runner validates fields it does not define
   (`parent_measured_seconds_per_token`, `all_tokens_matched`, `is_serial_control`, `mtp_depth`,
   `head_provenance.sha256`, the row-accounting ledger). The ARCH §3 protocol seam; co-designed.
3. **`rows_per_round` closure** (§8 #4) — still PROVISIONAL in the live wrapper (returns
   `depth+1`, W:1164-1170,1180); may become `depth * rounds`.
4. **The Seatbelt profile template** (§8 #6) — only its substitution contract is visible. *(P1.)*
5. **PF / sudoers / `bench-config.sh` / `manifest-lib.sh` internals** (§8 #5). *(P1.)*
6. **Per-platform calibration NUMBERS** (§8 #8) — mechanism reconstructable; the values were
   measured on box 3 and are explicitly *"do not carry across silicon"*.
7. **macmon JSON schema stability** (§8 #10).
8. **Diff the RIGHT bench-exec.** All `BENCHEXEC:` citations key to the **deployed 374-line**
   operator-repo copy of `bench-exec.sh`. The 210-line
   `home-<boxuser>-bench-runner/bench-exec.sh` is a **stale mirror** lacking the PF egress fail-closed
   self-check (exit 15) and process-group reaping entirely — diffing it surfaces a ~164-line
   phantom divergence including a whole security guard. *(P1.)*

*Closes:* the live half of M17, the ranked-path half of M14, the `UNVERIFIED(live-box)` items
behind M11.
*Constraint:* mirror files are read-only evidence, never a design input; per-prompt numeric values
are organizer material, report-only (FT-4 is reported under exactly this rule).

---

## 3. THE PRIMARY LEG — the track prompt pool, verbatim

> **These tapes are not coverage. They are the prompts.** The R2 set is THE WORKLOAD — the
> official prompt pool a ranked run scores. This document **records** its pins and count; it does
> **not** design, select, curate, or dimension it.

*reach: **multiple** — P1 (it is the submission path), P2 (its `results.json` is what the board
ingests), P3 (it is the benchmark).*

### 3.1 What the leg is

| | benchd side | reference side |
|---|---|---|
| binary | `benchctl measure-job` | `mlxfast-swift mtp-timed` |
| input | `--golden <pool object> --contract <TRACK>` | `mtp-timed --golden <pool object>` (the live wrapper's own call, `W:1615`) |
| pool authority | `TRACK.timed_prompt_pool[].{sha256,bytes}` | same |

REF's driver reads the tape as: `seed_tokens` → the timed window's seed prefill
(`beginMTPDecode(seedTokens:)`); `reference_seed_token` → the token prefill MUST produce
(`seed_token_mismatch` otherwise) = the run's FIRST emitted token; `rows[i].sequential_argmax` →
the token emitted at index `i+1`; `emitted_tokens` → the reference chain after the seed. benchd
consumes the SAME fields via `TimingParams`.

### 3.2 The pool, as pinned (PINS ONLY)

Observed in the read-only mirror
`~/projects/layr-labs/r2-official-inputs/qwen3.8-27b-mtp-v1/`, reconciled against
`TRACK.timed_prompt_pool[]`. **8/8 sha256 + bytes MATCH.**

The pool is **8 objects** — individual filenames and per-object hashes are WITHHELD (secret-tier:
the hidden official prompt pool). Each object is pinned by `sha256` + `bytes` in
`TRACK.timed_prompt_pool[]`; the aggregate **pool-pin digest** is `680d2f5ab18e0760` (see FT-4).
Uniform across all 8: `noop_decode_speedup` = **1.0**, role = `informational_diagnostic_not_scored`.

**Structural metadata, via loader metadata only** — every object carries EXACTLY the five tape
top-level keys (`seed_tokens`, `reference_seed_token`, `rows`, `emitted_tokens`,
`reference_self_consistent`) and every row EXACTLY the four row keys (`sequential_argmax`,
`top1_logit`, `top2_logits`, `top2_tokens`). Uniform across all 8: **`seed_tokens` = 512,
`rows` = 513, `emitted_tokens` = 513, `reference_self_consistent` = true** — matching
the measure-job document-shape contract exactly.

Mirror provenance (`PROVENANCE.txt:1-7`): pulled 2026-08-19T17:03:24Z from the R2 pool prefix
(`$R2_POOL_PREFIX`, held in `.env` only), per-object GET only, no sync/write; each object's
sha256 + byte count checked against its fixture pin BEFORE acceptance; **8/8 VERIFIED, 0
rejected**. Not yet pulled (workflow-env-pinned rather than fixture-pinned): the seam-1 hidden
correctness golden and the GPQA reference.

### 3.3 FACTS ABOUT THE TRACK (recorded, not solved)

**FT-1 — The pool objects are TAPES, so the local modes cannot load them at all.** Not an arity
mismatch: a *signature* mismatch. `--golden` routes by required-key signature
(`classify_golden_input`) — a tape carries `seed_tokens`/`reference_seed_token`/`rows`, a
`GoldenDocument` carries `cases`, and BOTH are `deny_unknown_fields`, so neither parses as the
other in either direction. The pool objects carry no `cases`, no `correctness_gates`, no
`benchmark` oracle. **Consequence:** `iterate` (129), `local-submit` (1024) and `official` (64)
have **no track-supplied workload**; the pool serves the `measure-job` leg only, and the local
modes' comparison targets are necessarily box-generated (the synthetic surround, §4).

> **RULED David 2026-08-20 (Q8) — FACT ACCEPTED. This does not block a leg.**
>
> The clarifying exchange settled three points, recorded here because they are the reason the
> fact is benign rather than a gap:
> 1. **It is a signature mismatch, not an arity shortfall.** A tape carries
>    `seed_tokens`/`reference_seed_token`/`rows`; a `GoldenDocument` carries `cases`. Both are
>    `deny_unknown_fields`, so neither parses as the other **in either direction**. No arity
>    adjustment could make a tape loadable by a local mode — the documents are different kinds.
> 2. **The local modes' comparison targets are reference-generated on-box, under pin +
>    attestation.** They are produced by `mlxfast-swift generate-golden` — the reference itself,
>    not benchd — then pinned sha256 + bytes and **dual-loader accepted** (`benchctl
>    validate-golden` AND `mlxfast-swift preflight`). Box-generated is not self-authored.
> 3. **Regeneration happens in-window, per the Q5 rule.** Pins come from the regenerating
>    window's REPORT, so the comparison targets are re-derived and re-attested each window rather
>    than carried as stale committed constants.
>
> Curating substitute track-shaped inputs for the local modes was considered and **rejected**: it
> would manufacture the appearance of track coverage where the track supplies none.

**FT-2 — The timed window is 512 tokens.** benchd's `--tokens N` counts `decode_step` calls and
consumes `N` rows; the Swift driver counts the seed argmax as `emitted[0]` and wants `N + 1`
rows. Both fit 513 rows for a 512-token window.

**FT-3 — `noop_decode_speedup` no longer divides anything.** All 8 carry `1.0`, role
`informational_diagnostic_not_scored`. Per `TRACK.noop_decode_speedup_role_note`, a **ROLE CHANGE
2026-08-14 (operator-ratified)**: the references are RETAINED, still pinned, still joined on the
pool object's own sha256, still sealed into every per-prompt breakdown — *but they no longer divide
anything*. Scoring is anchored at **serial = 1.0**, each prompt's score its own raw
serial-over-candidate ratio-of-means in the same thermally-gated session. Matches benchd's
`SCORE_ANCHOR_SERIAL_ONE` (`measure_job.rs@35c100a:90`).

**FT-4 — FOUR track-fixture revisions exist locally; TWO pin the WRONG EPOCH'S POOL under
CONTRADICTORY paths.**

| fixture sha256 (16) | bytes | where | pool pinned |
|---|---|---|---|
| `5677d53fb98fe4ae` | 44589 | `mlxfast-qwen-38-27b-mtp-engine/fixtures/` **+ 9 engine worktrees** (10 paths) | **`qwen3.8-27b-pool-*`** — pool-pin digest `680d2f5ab18e0760` |
| `aa8583bc8af22925` | 44423 | `qwen38-submission-work/`, `qwen38-pr7-merge/`, `qwen38-qa-fixes-pr/`, `qwen38-docs-cleanup/` | **`qwen3.8-27b-pool-*`** — same digest `680d2f5ab18e0760` |
| `a5dda6eba1e7067f` | 45662 | `b4-ranked-box-mirror/qwen38-main-checkout/fixtures/` | **`qwen3.6-27b-pool-*`** — digest `8a9623571110fa58` |
| `8a592822c64ed47b` | 48031 | `qwen-3.6-mtp-challenge-dev/fixtures/` | **`qwen3.6-27b-pool-*`** — **byte-identical pool pins**, same digest `8a9623571110fa58` |

The two 3.8-era revisions differ in bytes but **pin an identical pool**, so the `aa8583bc` cited
as pin authority in `PROVENANCE.txt:4` and the `5677d53f` benchd passed as `--contract` in Proof A
**agree on the workload**.

**The sharper defect: the two 3.6-pool copies are internally self-contradictory.** Both declare
`track_id: qwen3.8-27b-mtp-v1` *and* pin `qwen3.6-*` **filenames** under the **3.8** `r2_path`
prefix:

```
r2_path: <R2 3.8-track pool prefix>/<a 3.6-epoch pool object>
```

— a path that names the 3.8 track and the 3.6 prompt in the same string. They also carry real
per-prompt `noop_decode_speedup` values (0.7623 … 1.0845) instead of the ratified `1.0` (FT-3).
That the two disagreeing copies are **byte-identical to each other** in their pool block means
this is one propagated artifact, not two independent drifts — which strengthens rather than
weakens Q3: there is a single wrong-epoch fixture in circulation across two checkouts.

Same epoch-inconsistency class already recorded once in the measure-job B-4 analysis, now
instantiated on the **prompt pool itself**. **Reading:** neither 3.6-pool copy may be pin
authority for a 3.8 run, and the contradictory `r2_path` means a fetch driven off one of them
would request 3.6 objects from the 3.8 prefix. Reported, not corrected (organizer/mirror material
is report-only). **RULED (Q3): `5677d53f…` / 44589 B is the pin authority**; the two 3.6-pool copies stay report-only defects. Also D-2's first item.

**FT-5 — one pool object is the known hard prompt.** Entry 6 (pinned `c1ec58669d032878`; name
withheld — secret-tier) is that object, recorded on
[#109](https://github.com/davidtai/mlxfast-bench/issues/109) at ratio `1.1057` with 87 of 103
rounds non-drafting. Stated so a low number there reads as a workload property, not a defect.

### 3.4 What the primary leg must show

1. **Fetch-and-verify GREEN** — 8/8 sha256 + bytes against `TRACK.timed_prompt_pool[]`, at window
   time, inside the preflight gate. Any mismatch is die-8, pre-GPU. *(P1.)*
2. **Both implementations run the same 8 objects** at the same pins, same session, same thermal
   gate. *(P3.)*
3. **The diff.** Because scoring is serial-anchored (FT-3) and Model-2 is a declared series break,
   the compared unit is **token-chain and row-accounting agreement**, not a cross-series speedup
   number: `all_tokens_matched`, per-prompt `parity_ok`, the seed-token oracle
   (`reference_seed_token`), and the row-accounting closure
   (`reference_checked_row_total == declared_rows_total` — an EQUALITY, not `>=`,
   W:1172-1179,1218). *(P3.)*
4. **Per-prompt records sealed** in `results.pairs[]` with
   `.accepted_pair_count == (.pairs | length)`. *(P2.)*

**Not enumerated here, deliberately:** how many prompts, which domains, what seeds. The track
fixed all of that. If the pool changes, this section's table changes and nothing else does.

---

## 4. THE SYNTHETIC SURROUND — enumeration

Covers the loader/schema/failure dimensions the track pool cannot reach: the pool is eight *valid*
tapes and says nothing about what a **broken or hostile** input should do — which is exactly
prong P1.

**Families, ordered by reach (leading = `multiple` / submission-reachable):**

| order | family | gates | reach | prongs |
|---|---|---|---|---|
| 1 | **N** | loader **mode-arity** boundary | **multiple** | P1 (proven-looser class), P3 |
| 2 | **L** | loader ACCEPT/REJECT, hand-built shapes | **multiple** | P1, P3 |
| 3 | **Z** | loader decisions, structure-aware fuzz | **multiple** | P1, P3 |
| 4 | **T** | `--golden` document-shape routing | **multiple** | P1, P3 |
| 5 | **A** | artifact byte-rows | **yukon-visible** | P2 |
| 6 | **R** | failure **record shape** per failure path | **multiple** (R-10 submission-reachable) | P1, P2 |
| 7 | **C** | corruption-class failure map | **multiple** | P1, P3 |
| 8 | **V** | deterministic score parity across input-document **sections** | **multiple** | P2, P3 |

Status: **EXISTS** = fixture/generator present at BENCHD; **TO-GEN** = added here; **BLOCKED** =
gated on a named dependency.

### 4.0 Dimensions

**Modes (3)** — `crates/benchctl/src/iterate.rs@35c100a:48-58`, arities `:79-84`:

| id | mode | decode steps | golden `expected_tokens` required | runtime |
|---|---|---|---|---|
| `IT` | `local-iterate` | 128 | **129** (`decode_steps + 1`; `[0]` is the SEED) | `rust-local-iterate` |
| `SU` | `local-submit` | 1023 | **1024** | `rust-local-submit` |
| `OF` | `official` | 128 | **64** (`CORRECTNESS_STEPS`, the loader DEFAULT, mirroring `QwenRuntimeBenchmark.swift:88`) | `rust` |

The `+1` seed convention is REF's, verified (`QwenRuntimeLocalIterate.swift@ebe3446:776-777,854-858`),
mirrored as `REQUIRED_TOKENS = {"local-iterate": 129, "local-submit": 1024}`
(`gen-variant-corpus.py@35c100a:52-55`).

**Three pre-existing bounding rules:** `applicable_modes` — an input document's scale decides its modes
(`applicable_modes_for()` `gen-variant-corpus.py@35c100a:63-79` `die()`s if none qualifies; N/A is
a declared, non-FAIL cell, `variant-parity.sh@35c100a:128-131`); class applicability — `behavior`
corruption is emitted only if the source document carries the section
(`gen-failure-corpus.py@35c100a:102-107`); code-path sharing — `SU` shares `IT`'s checked-timing
machinery (`docs/parity-matrix.md@35c100a:667`), so family R is enumerated once at `IT` with
`SU` mirrors only where counts are arity-dependent.

### 4.1 Failure classes, from code, reconciled against "7"

**Axis A — `failed_payload` call sites.** Builder `iterate.rs@35c100a:1009`; wrapper
`preflight_failed_payload` `:958`. **7 production call sites, re-derived at the freeze snapshot:**

| site | trigger | `passed_correctness` | pinned? |
|---|---|---|---|
| `:431` | `run_conformance` returned `Err` | false | ✅ `site_431_*` (`:2966`) |
| `:455` | `session.close_phase()` failed after the gate | **true** | ✅ `site_454_*` (`:3002`) |
| `:515` | correctness FAILED **and** window-too-short | false | ✅ **`site_515_*` (`:3091`)** |
| `:539` | correctness FAILED, then time-only pass `Err` (protocol / thermal abort / completed-work barrier) | false | ✅ `site_547_*` (`:3041`) |
| `:563` | correctness FAILED and `!mode.is_local_checked_timing()` | false | ❌ (structurally dead) |
| `:581` | correctness PASSED, `expected_tokens.len() <= decode_steps` | **true** | ✅ `site_588_*` (`:3149`) |
| `:619` | correctness PASSED, timed benchmark `Err` | **true** | ✅ `site_627_*` (`:3191`) |

**The "five + two" is now SIX + one.** `f8ed398` added the `:515` per-site test, so
`assert_ruled_blank_seal` (`:2941`) is called from **six** places —
`:2998, :3037, :3083, :3145, :3187, :3221` — covering `:431, :455, :515, :539, :581, :619`.

The single remaining extra is **`:563`, structurally dead**: Official routes to
`official::official_core`, pinned by `unreachable!()` at `iterate.rs@35c100a:594` and `:612`.
The code says so itself (`:560-563`): it is *"covered structurally instead: it calls the same
`failed_payload`, which applies the #132(b) blank seal unconditionally, so it cannot diverge from
the five tested sites even if a future mode makes it live."* → OOS-01.

> **Two report-only nits at the freeze snapshot** (neither is a question; both are inert):
> the six test *function names and label strings* still carry the pre-`f8ed398` line numbers
> (`site_454_*` labels `"iterate.rs:454"`, etc.) while the sites are now `455/539/581/619`; and
> the `:563` comment says *"the five tested sites"* where there are now six.

**Axis B — the other two refusal paths.** *Preflight / early refuse*
(`iterate.rs@35c100a:967-990`, sole caller `main.rs@35c100a:2178`): OFFICIAL only, when
`resolve_paired_baselines` returns `None`. **An artifact IS written** (the `Ok` arm below `:2178`), exit 1,
byte-pinned by `early_refuse_record_byte_matches_the_reference_capture` (`:2637`). *Golden LOAD
refusal*: `main.rs@35c100a:2051` (`execute_iterate`) propagates the loader `Err`; the terminal handler `:1972-1975`
prints stderr and exits 1 — **no artifact at all**, where REF catches into `failedScore`
(`QwenRuntimeLocalIterate.swift@b26f76f:197-198`) and writes it unconditionally
(`main.swift@b26f76f:339-341`). [#131](https://github.com/davidtai/mlxfast-bench/issues/131),
**unruled**.

**Axis C — §7 corruption classes.** `gen-failure-corpus.py@35c100a`: **6 defined, 5 generated, 5
measured** — `primary` (`:74-79`), `anchor` (`:84-91`), `free-run` (`:94-99`), `oracle`
(`:110-115`), `baseline-missing` (`:153-156`, `declared` ref moved **#74 → #127**), and
`behavior` (`:102-107`) **never generated**. Floors: `FM_MIN_CLASSES=5`
(`failure-map.sh@35c100a:46`), official 4 (`official-failure-map.sh@35c100a:35`), and the official
map **refuses to run** without an `oracle` class (`:55-56`).

#### Reconciliation — the real count is 7, but not the 7 anyone meant

No single 7 exists in the code. Axis A has 7 sites (5 pinned); Axis C has 6 classes (5 generated);
the full set of failure *record shapes* emitted anywhere is **12** (local 6 + official 6 —
`official.rs@35c100a:564,598,721,644,686,536`). The defensible 7 is the set of **distinct
observable failure classes on the local leg**, mapping 7-for-7:

| FC | class | emitter | artifact? | seal (PR133) | reach |
|---|---|---|---|---|---|
| **FC-1** | golden LOAD refusal (read / pin / parse / model_type / provenance / arity) | `main.rs@35c100a:2051` → `:1967` | **NO** | n/a — **#131 open** | **submission-reachable** |
| **FC-2** | preflight early-refuse (missing paired baselines; OFFICIAL) | `iterate.rs@35c100a:958` | yes | blank + constants | yukon-visible |
| **FC-3** | conformance-gate `Err` | `:431` | yes | blank + constants | yukon-visible |
| **FC-4** | barrier `close_phase` `Err` | `:455` | yes | blank + constants | yukon-visible |
| **FC-5** | window-too-short | `:581` pass-arm, `:515` fail-arm (**both now pinned**) | yes | blank + constants | multiple |
| **FC-6** | timed-pass `Err` (protocol / thermal / barrier) | `:619` pass-arm, `:539` fail-arm | yes | blank + constants | yukon-visible |
| **FC-7** | correctness mismatch with a COMPLETED checked pass — the **retain** arm | `failed_with_real_timing_payload` (`:712`) | yes | **real** hash, `case_count = TIMING_REPEATS`, `checked_steps = step+1`, real timing | multiple |

Plus **FC-0** = the passing class (families V, L, Z, A).

**The F1(b) semantics (term (c) — MERGED at `35c100aa`).** **RULED David 2026-08-20 — MIRROR BLANK
STRICTLY** ([#132](https://github.com/davidtai/mlxfast-bench/issues/132#issuecomment-5357895042)),
superseding the same-day
[keep-and-declare](https://github.com/davidtai/mlxfast-bench/issues/132#issuecomment-5357754961).
Every local failure path producing no correctness report seals `golden_hash = ""`,
`case_count = 0`, `checked_steps = 0`. **Zero DECLARED cells.** Implemented structurally —
`failed_payload` blanks all three itself and no longer takes a case-count parameter
(`iterate.rs@35c100a:1009`; the blank itself at `:1020-1022`, under the comment *"The blank seal.
No caller may opt out"* at `:1019`). The deciding argument is the ruling doc comment at `:989`: REF's `goldenHash` carries
the invariant *non-empty means correctness completed*, because it is only ever populated from a
non-nil `CorrectnessReport` (`QwenRuntimeBenchmark.swift@b26f76f:1161-1162,1176`). **This is a P1
argument, not a cosmetic one:** sealing a real digest where correctness did not complete weakens
the field for every downstream consumer that trusts the invariant. A benchd-only
`loaded_golden_sha256` superset field was considered and **rejected**. Not blanked, by design: the
#73 retain arm (FC-7), built by `failed_with_real_timing_payload`, structurally unreachable by the
blank, pinned by `correctness_failure_retains_what_early_refuse_seals_empty` (`:2706`).

**Honesty notes:** (1) FC-1 emits nothing, so the differ has nothing to byte-compare → §4.7, Q4.
(2) FC-5's fail-arm twin `:515` **now HAS a per-site pin** (`site_515_*`, `iterate.rs@35c100a:3091`,
landed in `f8ed398`) — the prior draft's Q5 is **struck as already resolved**. (3) The three "DECLARED" cells from the
first #132 ruling are **gone**; after #133 merged the only non-blank class is FC-7, DECLARED by
design. (4) **Stale-doc defect on the PR133 branch, report-only:** `base_metrics`' doc comment at
`iterate.rs@140878b:1057-1068` (the pre-merge branch state) carried the overturned "KEEP REAL
VALUES" text and cited
`assert_ruled_real_seal`, a symbol deleted in the same commit. **`f8ed398` (F-1) fixed this before
merge** — the comment now states the FINAL ruling with correct test names and records why the
blank lives in `failed_payload` rather than `base_metrics`. The prior draft's Q9 is **struck as
already resolved**.

### 4.2 Family N — loader mode-arity boundary (9 cases: 6 EXISTS, 3 TO-GEN) · *reach: multiple (P1, P3)*

**Leads the table because it is the proven-looser class.** The #109 window-4 E2 drift was exactly
P1: benchd loaded EVERY mode at the flat 64 where REF loads local-iterate at 129, so **benchd
ACCEPTED an input document REF refuses** (`docs/parity-matrix.md@35c100a:47`). Fixed
(`Mode::golden_required_steps`, `iterate.rs@35c100a:79-84`) — but **no corpus family gates it**,
because L and Z both run through `validate-golden` at the flat 64. A regression would be
invisible, and it would be a regression in the looser direction.

**Surface:** ACCEPT/REJECT **and** diagnostic bytes — the identical refusal message is observed
fact from the §8 128-step re-verify:

```
rust:  benchctl iterate: golden load failed: primary-1.expected_tokens has 128 tokens; need at least 129
swift:     "error" : "primary-1.expected_tokens has 128 tokens; need at least 129",
```

| id | mode | arity | expected | source | status |
|---|---|---|---|---|---|
| N-01 | IT | 128 (one short) | REJECT both, identical message | `beefed.json` `32045f7e…` / 16940 — already refused by both | EXISTS |
| N-02 | IT | 129 (exact) | ACCEPT both | `beefed-129.json` `05a55d93…` / 16946 | EXISTS |
| N-03 | IT | 1024 (generous) | ACCEPT both | reuse `submit-1024` | EXISTS |
| N-04 | SU | 1023 (one short) | REJECT both, identical message | derive from `submit-1024` | **TO-GEN** |
| N-05 | SU | 1024 (exact) | ACCEPT both | `submit-1024` `a482f223…` | EXISTS |
| N-06 | SU | 129 | REJECT both | reuse `beefed-129.json` | EXISTS |
| N-07 | OF | 63 (one short) | REJECT both | derive from `beefed-129.json` | **TO-GEN** |
| N-08 | OF | 64 (exact) | ACCEPT both | derive from `beefed-129.json` | **TO-GEN** |
| N-09 | OF | 129 (generous) | ACCEPT both | reuse `beefed-129.json` | EXISTS |

**6 EXISTS / 3 TO-GEN.** The header's earlier "ALL TO-GEN" was wrong: six of the nine cases are
*new pairings of already-pinned fixtures*, not new fixtures. Only N-04, N-07 and N-08 need bytes
generated.

**Provenance rule for TO-GEN fixtures (non-negotiable).** Regenerate at the target arity with **the
reference itself** (`mlxfast-swift generate-golden --steps N`), transplant only
`cases[0].expected_tokens`, and **VERIFY** greedy prefix-identity to the donor rather than assuming
it — the procedure the §8 re-verify used. Had
the prefix diverged there, the step-8/24 anchors would no longer describe the case and the window
hard-stops.

### 4.3 Family L — loader decision parity (15 cases) · *reach: multiple (P1, P3)*

`crates/bench-core/tests/fixtures/golden_parity/@35c100a` — 17 files, of which `manifest.json` is
the index and `reference_model_contract.json` is the #114 contract fixture; neither is a `GoldenDocument`.
Generator `gen-loader-parity-corpus.py@35c100a`. Driver `loader-parity.sh@35c100a`:
`benchctl validate-golden --contract` vs `mlxfast-swift preflight`. **No GPU.**

**Surface:** the ACCEPT/REJECT decision — the P1 surface — plus diagnostic bytes where the
manifest declares `expected_rust_message_contains`.

| id | fixture | expected | gen@ |
|---|---|---|---|
| L-01 | `per_case_unknown_key.json` | **KNOWN-DIV** — Rust `deny_unknown_fields` REJECT vs Swift `JSONDecoder` drop→ACCEPT (typo'd `acepted_tokens`). **benchd STRICTER — declarable, and deliberately kept: it is the anti-cheat direction.** | `:214-218` |
| L-02 | `model_provenance_not_pinned.json` | REJECT both **when pinned**; `expected_rust_unpinned: ACCEPT` — the contract is what makes benchd not-looser here | `:151-161` |
| L-03 | `wrong_model_type_and_provenance.json` | REJECT both, reporting the **model_type** defect first — pins the DIAGNOSTIC via `expected_rust_message_contains: "correctness golden file model_type="`; REF's interleave `Golden.swift@b26f76f:377-385` then `:386-393` | `:169-181` |
| L-04 | `half_baseline.json` | REJECT both — the half-present pair still refuses through `Golden.swift@b26f76f:651`, keeping #74's fail-POINT ruling exercisable | `:210-212` |
| L-05 | `model_provenance.json` | REJECT both (unknown inner key `model_id`) | `:129-132` |
| L-06 | `model_provenance_valid.json` | ACCEPT both — #114 positive control | `:134-142` |
| L-07 | `valid.json` | ACCEPT both | `:123` |
| L-08 | `missing_model_type.json` | REJECT both | `:183-185` |
| L-09 | `wrong_model_type.json` | REJECT both | `:187-188` |
| L-10 | `unknown_top_key.json` | REJECT both | `:190-191` |
| L-11 | `bad_version.json` | REJECT both (`version=2`) | `:193-194` |
| L-12 | `empty_cases.json` | REJECT both | `:196-197` |
| L-13 | `short_expected_tokens.json` | REJECT both (`STEPS-1`) | `:199-201` |
| L-14 | `wrong_prompt_tokens.json` | REJECT both (`PROMPT-1`) | `:203-205` |
| L-15 | `null_benchmark.json` | REJECT both | `:207-208` |

Totals: **2 ACCEPT / 13 REJECT / 1 `swift_diverges` / 1 `expected_rust_unpinned` / 1
`expected_rust_message_contains`.** Last box result `match=14 known-divergence=1 MISMATCH=0` — PASS.

**Coverage limit, stated:** `validate-golden` validates at the flat `CORRECTNESS_STEPS` (64) —
`main.rs@35c100a:2724` passes `CORRECTNESS_STEPS` — not the
mode arity. Hence family N.

### 4.4 Family Z — fuzz loader corpus (183 cases) · *reach: multiple (P1, P3)*

`crates/bench-core/tests/fixtures/golden_fuzz/@35c100a` — **183** deterministic, seeded, frozen
fixtures (seed 20260817), generator `gen-fuzz-corpus.py@35c100a`, pinned sha256+bytes each,
frozen-corpus + stable-verdict tests in `crates/bench-core/tests/loader_fuzz.rs@35c100a`. This is
the P1 workhorse: 179 of the 183 are REJECT cases, i.e. hostile-input handling.

**17 mutation families**, Z-001…Z-183 — divergence families listed first:

| family | probe | n | `swift_diverges` | gen@ |
|---|---|---|---|---|
| E | base-case UNKNOWN key | 5 | **yes** | `:370-378` |
| H | anchor UNKNOWN key | 4 | **yes** | `:470-478` |
| J | free-run UNKNOWN key | 3 | **yes** | `:516-524` |
| L | behavior UNKNOWN key | 3 | **yes** | `:579-587` |
| D | base-case fields (name, counts, token OOB, container types, duplicate name) | 32 | no | `:293-366` |
| M | benchmark-block mutations | 19 | no | `:590-639` |
| A | `model_type` (missing / 9× wrong / empty / whitespace / null / number / bool / array / object) | 18 | no | `:208-237` |
| G | anchor-case mutations | 18 | no | `:422-466` |
| K | behavior-case mutations | 13 | no | `:527-576` |
| C | top-level structure | 12 | no | `:260-289` |
| F | `correctness_gates` structure | 12 | no | `:382-419` |
| I | free-run-case mutations | 12 | no | `:481-513` |
| B | `version` | 11 | no | `:241-256` |
| P | JSON-level malformations | 9 | no | `:671-692` |
| Q | VALID variants | 5 | no | `:165-205` |
| O | layered duplicate case names | 4 | no | `:654-667` |
| N | benchmark UNKNOWN key (predicted → **box-confirmed MATCH**) | 3 | no | `:642-651` |
| | **TOTAL** | **183** | **15** | |

**Expected:** `ACCEPT = 4 / REJECT = 179 / swift_diverges = 15` → dual-loader `match=168 /
known-divergence=15 / MISMATCH=0`. All 15 divergences are the **same direction: Rust stricter**
(`deny_unknown_fields` REJECT vs Swift drop→ACCEPT) — declarable under P1, and the anti-cheat
direction. `valid_cases_only` is **REJECT** (`:182-186`) since **#77** made `validate-golden`
require the benchmark oracle by default — that fix closed a genuine **benchd-looser** finding, the
canonical P1 case this family exists to catch.

`CORRECTNESS_STEPS = 64` (`constants.rs@35c100a:132`, mirrored `gen-fuzz-corpus.py@35c100a:54`) is
the primary-case `expected_tokens` **floor** the loader enforces (`>=`, not `==`): valid base cases
are built at exactly 64 (`:92`); `case_expected_count_*` probes `[63, 0, 1, 32]` (`:322-326`); and
`valid_extra_expected_tokens` uses `STEPS + 20` to pin that `>=` is the rule (`:199-205`).

**Prerequisite, not a gated cell:** `docs/fuzz-corpus-report.{md,txt}@35c100a` is **stale relative
to the committed manifest** — the report says `ACCEPT=5 / REJECT=178 / 21 divergences / 162 match`
(`:41,53,72,98-99`) where generator and manifest both say `ACCEPT=4 / REJECT=179 / 15` (→ 168
match); it also still calls the loader-parity corpus "12-fixture" (`:3`) where it is 15. The #77
and #113 changes landed in generator and manifest but never reached the report, and
`fuzz-corpus-check.sh` **rewrites it on every run**, dirtying any lane
([#125](https://github.com/davidtai/mlxfast-bench/issues/125)). → §6 stage 1.

### 4.5 Family T — `--golden` document-shape routing (6 cases) · *reach: multiple (P1, P3)*

`--golden` routes by **required-key signature** (`classify_golden_input`): a tape carries
`seed_tokens`/`reference_seed_token`/`rows`, a `GoldenDocument` carries `cases`, both
`deny_unknown_fields`. This is the seam that made every `measure-job` invocation die-8 pre-GPU
before #109. Runnable GPU-free via
`measure-job --preflight-only` (`main.rs@35c100a:659` (flag) / `:415` (field)).

| id | input | expected | reach |
|---|---|---|---|
| T-01 | tape whose sha256 matches no pool entry | die-8, pre-GPU | submission-reachable |
| T-02 | tape whose sha256 matches but `bytes` disagrees | die-8 naming **both** numbers (#112 L3) | submission-reachable |
| T-03 | tape + one unknown key | REJECT naming the key — **not** silently dropped, and **not** misreported as an unknown-`emitted_tokens` GoldenDocument | submission-reachable |
| T-04 | matches neither signature | REJECT naming **both** shapes | submission-reachable |
| T-05 | valid tape (5 top-level keys, 4 row keys) | ACCEPT, routed as tape | execution-equivalence |
| T-06 | valid `GoldenDocument` | ACCEPT, routed as golden | execution-equivalence |

**Declared divergence, inert:** benchd denies unknown keys where Swift's `JSONDecoder` ignores
them — **benchd STRICTER**, a deliberate golden-side house rule, declarable under P1. It cannot
false-reject a live input: a ranked `--golden` must also match a pool pin BY BYTES and all 8
pinned objects carry exactly the modelled key set (§3.2). A newly-pinned key fails LOUD.

**Secret-tier:** family T's fixtures are **SYNTHESIZED** to the schema with invented content. **No
organizer bytes are in this repository.**

### 4.6 Family A — artifact byte-rows (15 cases) · *reach: yukon-visible (P2)*

**This family IS prong P2.** The five surfaces in §2.3 are precisely what board ingestion reads.

| id | surface | mode | driver |
|---|---|---|---|
| A-01…A-05 | score payload / naming / `.sha256` / integrity JSON / exit code | IT | `facade-leg.sh` |
| A-06…A-10 | same five | SU | `facade-leg.sh` |
| A-11…A-15 | same five | OF | `official-parity.sh` |

**Not compared:** stderr (evidence only), the Seatbelt profile (recorded; same builder both sides),
`score.untrusted.json` (superseded by the stdout seal, `official-lib.sh@35c100a:172-173`).

**A-04 / A-09 / A-14 carry the live risk.** The integrity-JSON row is graded VERIFIED (superset)
on CODE evidence only — *"no box run has written one of these sidecars yet"*
(`docs/parity-matrix.md@35c100a:38`). Proof A's two `iterate` legs died at the engine handshake
(#134) before sealing, so the 9+7 local sidecar is **still unwritten on hardware**
([#135](https://github.com/davidtai/mlxfast-bench/issues/135)). These three are the first hardware
evidence for the Yukon ingestion chain and must not be waved through.

### 4.7 Family R — failure RECORD SHAPE (12 cases, 1 excluded) · *reach: multiple*

The **other axis** from family C: C asks *what benchd decides on a broken input* (P1, P3); R asks
*what benchd SEALS when a local run fails* (P2, and P1 for FC-1). **The #133 BLOCK IS CLEARED** —
merged at `35c100aa`, so all five formerly-blocked cases are runnable.

**Surface:** the whole sealed failure document, byte-compared against a capture constructed from
REF source — the pattern already established by
`crates/benchctl/tests/fixtures/swift-early-refuse-failure-record.json@35c100a` +
`early_refuse_record_byte_matches_the_reference_capture`.

| id | FC | path @35c100a | trigger | expected seal | reach | status |
|---|---|---|---|---|---|---|
| R-10 | FC-1 | `main.rs@35c100a:2051` → `:1967` | any loader refusal | **RULED (Q4) R-10a + #131 opt (2):** benchd writes the early-refuse record; `weights_hash` / `weights_byte_count` / `weights_file_count` seal **EMPTY** — declared divergence, 3 fields | **submission-reachable** | **TO-GEN** (needs the #131 fix) |
| R-03 | FC-5 | `iterate.rs:515` | correctness FAIL **and** window-too-short | `golden_hash ""`, 0/0, `baseline_* = constants` | multiple | **EXISTS** — `site_515_*` (`:3091`) landed in `f8ed398` |
| R-09 | FC-7 | `failed_with_real_timing_payload` | correctness mismatch **with** a completed checked pass | **real** hash; `case_count = TIMING_REPEATS`; `checked_steps = first_failing_step + 1` else `(decode_steps+2)*TIMING_REPEATS`; real timing | multiple | EXISTS |
| R-08 | FC-2 | `preflight_failed_payload:958` | OFFICIAL, no pair from env/flags/golden | blank + constants | yukon-visible | EXISTS |
| R-01 | FC-3 | `:431` | engine protocol fault during the gate | blank + constants | yukon-visible | EXISTS |
| R-02 | FC-4 | `:455` | barrier/transport failure after the gate | blank + constants | yukon-visible | EXISTS |
| R-04 | FC-6 | `:539` | correctness FAIL, then time-only pass `Err` | blank + constants | yukon-visible | EXISTS |
| R-06 | FC-5 | `:581` | correctness PASS, window-too-short | blank + constants | yukon-visible | EXISTS |
| R-07 | FC-6 | `:619` | correctness PASS, timed benchmark `Err` | blank + constants | yukon-visible | EXISTS |
| R-05 | — | `:563` | *structurally dead* (`unreachable!()` at `iterate.rs@35c100a:594,612`) | — | **none** | **EXCLUDED** → OOS-01 |

Plus **+3 `SU` mode-mirrors** (R-01, R-06, R-09 — the arity-dependent counts). R-08 is
OFFICIAL-only. Total 12.

#### R-10 — the no-artifact class needs explicit treatment

**The differ requires an artifact to byte-compare**, and on a golden-LOAD refusal benchd writes
nothing while REF seals a record. **This is P1, not cosmetics:** the trigger is a hostile or
malformed submission input, and the divergence is that benchd produces *no auditable record* of
having rejected it. Note also that `tests::failing_iterate_writes_full_byte_shaped_artifact_set`
asserts a FAILING run writes the same three artifacts a passing one does — **this path is the hole
in that invariant**, uncovered because the test starts from a payload.

- **R-10a — differ-comparable after #131 is fixed.** benchd catches loader errors into the #74
  early-refuse record and writes the artifact set. *Cost:* #131 option (1) also requires moving
  the weights digest above the golden load (REF's order: digest
  `QwenRuntimeLocalIterate.swift@b26f76f:89`, load `:101`; benchd's is inverted,
  `main.rs@35c100a:2051` (`execute_iterate`)→`:2093`), because REF's early-refuse record carries **populated**
  `weights_hash`/`weights_byte_count`/`weights_file_count`. That gives up a deliberate fail-fast
  saving — a malformed input document is currently rejected before hashing ~15 GB of weights. #131 option (2)
  writes the record without reordering and seals EMPTY weights fields, trading a missing artifact
  for a **diverging** one on three fields.
- **R-10b — assert the refusal behaviour itself.** Keep benchd artifact-less; assert identical
  exit code, `error_class()` equality on stderr, and REF's record present while benchd's is
  absent-by-declaration. No code change, but the sealed-record surface stays permanently
  uncompared on a submission-reachable class.

#### RULED (David 2026-08-20) — **R-10a with #131 option (2)**

> **Take the real byte-compare. Accept a sealed artifact that diverges on the three declared
> weights fields. Do NOT pay the 15 GB pre-hash.**

benchd catches loader errors into the #74 early-refuse record and writes the full artifact set, so
R-10 becomes an ordinary byte-compare case. The sealed-record surface is no longer permanently
uncompared on a submission-reachable class, and
`tests::failing_iterate_writes_full_byte_shaped_artifact_set`'s invariant stops having a hole.

**The declared divergence, named precisely.** Option (2) writes the record **without** reordering
the weights digest above the golden load, so on this path `weights_hash`, `weights_byte_count` and
`weights_file_count` seal **empty** where REF's early-refuse record carries them **populated**.
That is **three fields, declared** — they join the differ's declared-exception set alongside
`score_sha256` and `transform_source_sha256` (§2.3). Everything else in the record byte-compares.

**Why not option (1):** mirroring REF's order (digest `QwenRuntimeLocalIterate.swift@b26f76f:89`
before load `:101`) would mean hashing ~15 GB of weights *before* rejecting a malformed input
document — surrendering a real fail-fast property to make three audit fields match. The ruling
buys the comparison and pays for it in three declared fields rather than in every rejected run.

**Scope.** The ruling also governs the two siblings on the same arm — the sandbox fail-closed
refusal and the invalid `MLXFAST_PAIRED_BASELINE_*` refusal: both now seal the early-refuse
record. **#134's handshake failure is NOT in this class** — it is after the load seam, and
`measure-job` already seals on it (`results.json` + `.sha256` +
`benchmark-integrity.results.json`, cause retained in `rejected_pairs[]`). So the two surfaces
stop disagreeing: the load seam seals, and the post-load infra fault seals too.

*(The two options, as presented before the ruling, are retained below for the record.)*

Two siblings ride the same decision (sandbox fail-closed
`:2081`; invalid paired-baseline env `:2185`). A third, #134's handshake failure, is *after* the
load seam and also seals nothing — while `measure-job` on the **same** failure **does** seal
`results.json` + `.sha256` + `benchmark-integrity.results.json`, retaining the cause in
`rejected_pairs[]`. The two surfaces already disagree about whether this class is recordable.

### 4.8 Family C — corruption-class failure map (16 cases) · *reach: multiple (P1, P3)*

`gen-failure-corpus.py@35c100a` over a valid donor; each variant is still valid JSON so the loader
accepts it and the run fails at the correctness/benchmark stage, not at parse. Drivers
`failure-map.sh@35c100a`, `official-failure-map.sh@35c100a`.

**Surface:** both runners' failing `score.json`, field-diffed on the shared failure surface. In
FailingPair mode the differ waives a `Timed` field only if `failed ⇒ zeroed/null` holds for **each**
side (`parity.rs@35c100a:332-338`) — so a side that fails while retaining real timing is correctly
hard-failed, not silently waived.

| id | class | mode | expected |
|---|---|---|---|
| C-14 | `oracle` | OF | **both FAIL** — the class local cannot test; a both-PASS **aborts the harness LOUD** (`official-failure-map.sh@35c100a:97-106`) |
| C-15 | `baseline-missing` | OF | **DECLARED (#127)** — benchd refuses where REF falls back to the constants (`Golden.swift@b26f76f:220-226`). **benchd STRICTER on the ranked path — declarable under P1**, and **CONFIRMED KEEP** ([#127](https://github.com/davidtai/mlxfast-bench/issues/127#issuecomment-5357341215)): the ranked runner measures its baseline in the same session (#61), so an official run with no pair is a missing measurement, not a cue to score against a cached constant |
| C-16 | `submit-1024-band` fixture | OF | both FAIL the acceptance band identically; both carry the band-failure signature; RULING-2 blanked surfaces byte-match |
| C-11…C-13 | `primary`, `anchor`, `free-run` | OF | all FAIL, full correctness scope; anchor-fail `first_failing_step = 0` (`QwenRuntimeCorrectnessCompare.swift:481`), not null |
| C-01…C-05 | `primary`, `anchor`, `free-run`, `oracle`, `baseline-missing` | IT | primary both FAIL (FC-7 retain); anchor/free-run/oracle both PASS; baseline-missing both PASS **identically** (#127 negative control) |
| C-06…C-10 | same five | SU | as IT — shares the code path |

**Excluded, declared:** `behavior` × 3 modes = **3 cases not generated** (donor carries no behavior
gate). **This exclusion is now a P1 finding, not a cosmetic one — see OOS-MISFIT in §5.**

**Secret-tier note.** Today's donor is box-generated, so this corpus is committed. **If a mutant is
ever derived from an R2 base, it is window-workspace-only and never committed** (§1.4 rule 2).

### 4.9 Family V — deterministic score parity across input-document SECTIONS (7 cases) · *reach: multiple (P2, P3)*

Generated by `gen-variant-corpus.sh@35c100a` + `.py`; each variant differs **only** in its
`correctness_gates` section — a loader-accept + score-invariance probe. §12 runs
`CorrectnessScope::BaseCasesOnly` (the Swift-exact local default), so gate *sections* are not
evaluated here. Arity derived from the decode window, not the flat 64
(`gen-variant-corpus.sh@35c100a:54-56`, `STEPS = 128 + 1 = 129`, guard `:88-90`) — #124's fix,
closed by PR #126.

| id | variant | sections | mode | emit@ |
|---|---|---|---|---|
| V-01 | `submit-1024` (reused, not regenerated) | cases + anchors + free_run + oracle | IT | `:193-206` |
| V-02 | `submit-1024` | as above | SU | `:193-206` |
| V-03 | `official-calibrated-1024` | as V-01, baselines box-calibrated | OF | `assemble-official-golden.sh` |
| V-04 | `anchors-heavy` | + `correctness_gates.anchors` | IT | `:164-166` |
| V-05 | `free-run-only` | + `correctness_gates.free_run`, no anchors | IT | `:169-171` |
| V-06 | `minimal` | `cases` + benchmark oracle, **no** gates | IT | `:160-161` |
| V-07 | `behavior-bearing` | + **SYNTHETIC** `correctness_gates.behavior` | IT | `:175-189` |

`SU` for V-04…V-07 is **N/A** (declared, non-FAIL): 129-token iterate fixtures cannot run submit's
1023-step window (`docs/parity-matrix.md@35c100a:925`).

**Two declared caveats, both signed:** V-07 carries
`declared: "#behavior-synthetic-local-unevaluated"` (`:175-189`) — the gate is synthetic in-vocab
filler, loader-valid and, under `BaseCasesOnly`, not evaluated. V-03 is **PARITY-TEST-ONLY**,
box-calibrated, **NEVER an organizer/ranking golden**; only the two `benchmark` baselines differ
from `submit-1024`, and since the golden top level is `deny_unknown_fields` the label lives in the
`.provenance.txt` sidecar + `.manifest.json` (`docs/parity-matrix.md@35c100a:758`).

**Pins are window-produced — RULED David 2026-08-20 (Q5): the rule as written stands.** The §12
pins in `docs/parity-matrix.md@35c100a:945` predate PR #126 (`STEPS` 64 → 129) and **will change**
on regeneration. **The battery takes its pins from the regenerating window's REPORT**; the matrix
table is historical. Freezing them in-document first was considered and rejected — it would add a
pre-battery regeneration step for pin numbers the window re-derives anyway.

**Provenance attestation (retained).** These comparison targets are genuinely box-generated — the track pool
supplies no `cases`-bearing document (FT-1), so section-variant coverage cannot come from R2. Each
carries: generated by **the reference itself** (`mlxfast-swift generate-golden`), pinned
sha256+bytes, **dual-loader accepted**, reproducible across two window runs.

### 4.10 COUNTS — per-source reconciliation

Every `n` is produced by the stated command at BENCHD in a clean checkout.

#### Primary leg

| source | command | n observed | n pinned by TRACK | reconciliation |
|---|---|---|---|---|
| **R2 track prompt pool** | `ls ~/projects/layr-labs/r2-official-inputs/qwen3.8-27b-mtp-v1/*.json \| wc -l` → 8; verify `cd <mirror> && shasum -a 256 -c SHAS.txt`; cross-check `python3 -c "import json;d=json.load(open('<TRACK>'));print(len(d['timed_prompt_pool']));[print(e['r2_path'].split('/')[-1],e['sha256'],e['bytes']) for e in d['timed_prompt_pool']]"` | **8** | **8** | **8/8 sha256 + bytes MATCH.** 0 unpinned local objects, 0 pinned-but-absent. Independently attested `8/8 VERIFIED, 0 rejected` at `PROVENANCE.txt:5`. |

**Fixture-revision caveat for the checker (FT-4):** cross-check against `5677d53f…`/44589 B or
`aa8583bc…`/44423 B — both pin pool digest `680d2f5ab18e0760`. Running it against
`b4-ranked-box-mirror/qwen38-main-checkout/fixtures/qwen3_8_27b_mtp_track.json`
(`a5dda6eb…`/45662 B) yields **8/8 MISMATCH**, because that copy pins the 3.6-epoch pool (digest
`8a9623571110fa58`). **That is the defect, not a reconciliation failure.**

#### Synthetic surround

| source | command | available | contributed | excluded | reason |
|---|---|---|---|---|---|
| mode-arity boundary (N) | new pairings of existing pinned fixtures + 3 derived | **6 reusable** | 9 (**6 EXISTS / 3 TO-GEN**) | 0 | new family; §4.3's flat-64 limit explains why nothing else gates it |
| loader-decision corpus (L) | `ls crates/bench-core/tests/fixtures/golden_parity/*.json \| grep -vE '(manifest\|reference_model_contract)\.json$' \| wc -l` | **15** | 15 | 2 of 17 files | `manifest.json` is the index; `reference_model_contract.json` is the #114 contract fixture — neither is a `GoldenDocument` |
| fuzz corpus (Z) | `ls crates/bench-core/tests/fixtures/golden_fuzz/*.json \| grep -v 'manifest\.json$' \| wc -l` | **183** | 183 | 0 | — |
| document-shape routing (T) | synthesized per the measure-job document-shape contract | **6** | 6 | 0 | — |
| artifact byte-rows (A) | 5 surfaces × 3 modes (§2.3) | **15** | 15 | 0 | `score_sha256` + `transform_source_sha256` are field-level declared exceptions, not excluded cases |
| failure-path record shapes (R) | `git show 35c100aa:crates/benchctl/src/iterate.rs \| grep -c 'return failed_payload('` (7 sites) + preflight + retain arm + load refusal | **10** | 9 **+3** `SU` mirrors = **12** | 1 (R-05 `:563`) | structurally dead — `unreachable!()` at `iterate.rs@35c100a:594,612`; **reach: none** |
| failure-class corpus (C) | `grep -cE '^\s*add\("' scripts/gen-failure-corpus.py` | **6 defined** | 5 generated × 3 modes = 15, +1 band fixture = **16** | 1 class × 3 modes = **3** | donor carries no `correctness_gates.behavior`; **see OOS-MISFIT — this exclusion is P1-reachable** |
| variant corpus (V) | 4 generated (`gen-variant-corpus.py` emit sites) **+1** reused `submit-1024` (`gen-variant-corpus.sh:44-45`) | **5** | 5 variants → **7** mode-cases | 0 variants; 4 mode-cells N/A | V-04…V-07 are iterate-scale so their `SU` cell is declared N/A |
| *differ roster* (compare policy) | `grep -cE '^\s*\("(score\|passed\|metrics\.)' crates/benchctl/src/parity.rs` | **58** | — | — | compare surface for V/C/R/A |
| *runner-identity roster* | `python3 -c "import json;print(len(json.load(open('scripts/fixtures/integrity-runner-keys.json'))['keys']))"` | **8** (F3 merged) | — | — | compare surface for A-04/A-09/A-14 |

**Synthetic surround total: 9 + 15 + 183 + 6 + 15 + 12 + 16 + 7 = 263 cases.**
**Excluded: 4** (3 behavior mode-cells + R-05 dead arm).
**Primary leg: 8 track prompts.** **Battery total: 271.**

**By reach:** `multiple` **247** (primary 8, N 9, L 15, Z 183, T 6, C 16, V 7, R 3 — leading the
table); `yukon-visible` **23** (A 15, R 8); `submission-reachable` **1** (R-10);
`execution-equivalence` **0** standalone; `none` **0** in-corpus (R-05 excluded).
Sums to 271. R's 12 split 3 `multiple` (R-03, R-09, R-09/`SU`) + 8 `yukon-visible`
(R-01, R-02, R-04, R-06, R-07, R-08, R-01/`SU`, R-06/`SU`) + 1 `submission-reachable` (R-10).

**By status — buckets are now DISJOINT and the ledger SUMS EXACTLY:**

| status | n | which |
|---|---|---|
| **EXISTS** | **259** | L 15 · Z 183 · T 6 · A 15 · V 7 · C 16 · N 6 · R 11 |
| **TO-GEN** | **4** | N-04 (SU 1023), N-07 (OF 63), N-08 (OF 64), **R-10** (needs the #131 fix; ruled R-10a opt 2) |
| **UNRULED** | **0** | *Q4 ruled R-10 — nothing is left unadjudicated.* |
| **BLOCKED** | **0** | *#133 merged at `35c100aa` — the five formerly-blocked R cases are now EXISTS* |
| **SUM** | **263** | **= the synthetic-surround total. ✅ checks.** |

*Prior drafts reported 240/12/5/1 = 258, which under-counted by 5: family N was mis-bucketed as
"all TO-GEN" when six of its nine cases reuse already-pinned fixtures, and the five #133-blocked R
cases were double-counted against a BLOCKED bucket that no longer applies. Buckets are disjoint
now — every case appears exactly once.*

---

## 5. OUT-OF-SCOPE — each with a razor cite

**Razor form:** *P1?* submission-reachable **and** benchd looser · *P2?* feeds board ingestion ·
*P3?* changes a decision, methodology, score, floor or calibration verdict. **None of the three ⇒
out-of-scope by definition.**

> **Razor symmetry — a correction disclosed rather than buried.** Prior drafts demoted OOS-06
> (#119, no `--contract` on `iterate`/`correctness`) to `reach: none` on a **reachability**
> argument — "the submission path doesn't use those commands" — while denying OOS-MISFIT the
> identical argument. That was inconsistent, and the re-grounding shows *why* the argument is
> wrong in **both** cases: **reachability RELOCATES an exposure, it does not delete it.** Applied
> uniformly, OOS-06's exposure relocates onto our own paired flow (the facade seam-1 default
> routes to `benchctl iterate --mode official`, which takes no `--contract`) — the *same* root
> exposure as OOS-MISFIT. **OOS-06 is therefore withdrawn from this table and merged into
> §5.1 item (ii).** Any future demotion-by-reachability in this document must name where the
> exposure lands, not merely where it doesn't.

**Accounting (honest labels, not all "razor cites").** 18 ids, of which OOS-06 is **withdrawn**,
leaving **16 live out-of-scope lines** (OOS-16 was ruled in-corpus at signing):

| kind | n | which |
|---|---|---|
| **true razor dismissals** (serve none of the three prongs) | **14** | OOS-01…05, 07…10, 12…15, 17 |
| **deferrals** (deferred to a deliverable, *not* excluded) | **2** | OOS-11 (→ D-1/D-2), OOS-18 (→ D-2) |
| **LIVE OUT-OF-SCOPE TOTAL** | **16** | |
| *ruled IN-CORPUS at signing* (leaves the table) | 1 | OOS-16 → A-SCOPED (Q7) |
| *withdrawn* (never out-of-scope) | 1 | OOS-06 → merged into §5.1 (ii) |
| **ids assigned** | **18** | |

*Prior drafts claimed "16 out-of-scope lines, each with a razor cite". That over-claimed: a
deferral is not a dismissal, and an undecided item is not a disposition. Corrected above.*
OOS-17 and OOS-18 are **new dispositions added pre-freeze** per term (a) — `prefill-decompose`
and `overlay-timing` are real subcommands that carried no matrix row and no disposition anywhere.

> ### OOS-15 NARROWED — RULED David 2026-08-20 (Q1a)
>
> As previously drafted, OOS-15 classed the **R2 hidden correctness reference and GPQA reference**
> (present under the same prefix, unpulled — `PROVENANCE.txt:6`) as organizer-owned and therefore
> out of reach. **That foreclosed option (a)** — the behavior-gate implementation needs exactly
> that reference. The two dispositions collided, and the collision is resolved in (a)'s favour.
>
> **OOS-15 now reads: organizer material is NEVER MODIFIED and NEVER RE-UPLOADED — but a
> pin-verified READ is permitted where a gate requires it.** That is precisely how the timed
> prompt pool is already treated (§1.4, §3.2): fetch per-object, verify sha256 + byte count
> against the declared pin *before* acceptance, hold read-only, never write back.
>
> **What is now permitted:** pulling the hidden behavior-gate reference under the same
> fetch-and-verify discipline, for the purpose of evaluating the behavior gate.
> **What remains out of scope, unchanged:** authoring the ranked seal, running the judge,
> re-anchoring integrity, and *any* modification, derivation-into-commit, or re-upload of
> organizer bytes. Secret-tier rule 2 (§1.4) still binds anything derived from it: **window
> workspace only, never committed.**

| id | item | reach | kind | razor cite |
|---|---|---|---|---|
| OOS-01 | `failed_payload` site `iterate.rs@35c100a:563` | **none** | razor | Structurally dead — `unreachable!()` at `:594,612`; the code comment at `:560-563` states it is covered structurally by the same unconditional blank seal. A case would assert nothing. |
| OOS-02 | 1-ULP read→emit creep on baseline fields | **none** | razor | Not submission-reachable, not ingested, within `Det` tolerance so no decision moves. Already a differ creep-watch class; less reachable still since #127 stopped benchd emitting a value read from the golden's baseline fields on the local leg. |
| OOS-03 | benchctl **native** usage exit code `2` | **none** | razor | The **facade's** exit code is Yukon-visible and IS gated (A-05/A-10/A-15 + `compat-matrix.sh` Part 3). The native code is a developer-CLI convention the workflow never reads. RULED WAIVED 2026-08-17. |
| OOS-04 | Exit-code asymmetry on a defective input (#109 Finding 7) | **none** | razor | The **wrapper** exit code is the Yukon-visible one and is gated by family A. REF's binary returning 0 on a failing local score is by design (pass/fail lives in the payload `passed`); `benchmark.sh` maps a failing payload → exit 1. Root-caused and fixed by PR #122. Listed so it is not re-litigated. |
| OOS-05 | [#121](https://github.com/davidtai/mlxfast-bench/issues/121) `model_type` diagnostic prose (`{:?}` → `Some("gemma_text")` vs `String(describing:)` → `Optional("gemma_text")`) | **none** | razor | **The canonical razor case.** Decisions are identical, so benchd is not looser (P1 ✗); diagnostic prose is not ingested (P2 ✗); no decision, score or floor moves (P3 ✗). Out-of-scope **by definition** — no per-cell ruling needed. (The hook exists in L-03 and family N if David disagrees.) |
| ~~OOS-06~~ | ~~#119 — `iterate`/`correctness` take no `--contract`~~ | **WITHDRAWN** | merged | **Not out-of-scope.** Prior drafts demoted this on reachability; the razor-symmetry correction above shows the exposure RELOCATES onto our own paired flow (facade seam-1 → `benchctl iterate --mode official`, no `--contract`, so `model_provenance` is shape-only on a scoring-bearing default). **Merged into §5.1 item (ii)** as the second half of one root exposure, and covered by option (b′). |
| OOS-07 | `transform-if-changed` (M20), `verify-transform` (M26) | **none** | razor | No benchd code exists (`benchctl transform` is a stub; `benchmark.sh` owns the skip/rebuild + marker), so nothing to run or diff. The one Yukon-visible consequence — `transform_source_sha256` in the integrity JSON — is already handled as a declared field-level exception in family A. M26 is a DECLARED deferral signed 2026-08-18, riding on M20. |
| OOS-08 | semantic-GPQA judge (M29) | **none** *(for benchd)* | razor | RULED option (b) 2026-08-18 — workflow-owned. benchd stays judge-free and holds no judge-API credentials; the compared unit is the **pre-judge sealed score**, and `semantic_gpqa_*`/`gpqa_ttft_*` are 0/"" both sides. benchd produces no artifact here, so no prong attaches to benchd. (P1 note: keeping the judge out of benchd's trust boundary is itself the security-preferred arrangement.) |
| OOS-09 | `--local-cool-gate-only` residual divergence | **none** | razor | Facade-only selector; offline-inert; not ingested. Deliberate — dispatching would emit `benchctl…:`-prefixed stderr and break the `benchmark.sh:` impersonation, a net parity loss. Real runs still thermally gate via `benchctl iterate --cool-gate`. |
| OOS-10 | Parent-harness `MLXFAST_*` env surface | **none** *(local scope)* | razor | Recorded as a DECISION in the matrix; most are official/sandbox surface. #47 — the engine-subprocess env surface, which **is** the security-relevant one — is closed and separately gated (allowlist-from-empty + stderr redaction, GPU-verified). |
| OOS-11 | Track-B code-verified rows: M27 (`cache_memory` #54 live worker), M28 (`MLXFAST_PAIRED_BASELINE_*` #61 GPU e2e), M24 (`correctness` sandboxing/golden-blocking) | **deferred, not excluded** | **deferral** | Each is VERIFIED-code / stub-tested; the **live halves belong to D-1 and D-2**, not to the differ battery. Listed so "VERIFIED (code)" is never read as "GPU-verified". M24's golden-deny is P1 and is D-2's item 4. |
| OOS-12 | [#125](https://github.com/davidtai/mlxfast-bench/issues/125) stale fuzz report | **none** | razor | Documentation hygiene. But it **dirties the tree on every battery run**, so §6 stage 1 resolves it as a prerequisite, not a gated cell. |
| OOS-13 | rustfmt drift on `main` (~6.2k lines) | **none** | razor | Orthogonal to all three prongs; a repo-wide reformat would destroy the line-number citations this document and the matrix depend on. |
| OOS-14 | Prefill residual (M-5, 27.2 ms protocol floor) | **none** | razor | DECOMPOSED **physical** — the benchd↔engine protocol/spawn floor an in-process monolith never pays. It **cancels in scoring** under per-series benchd-measured baselines, so P3 ✗ (no scoring verdict moves). Grade A on the band with a standing band alarm. |
| OOS-15 | Anything organizer-owned: the ranked `score.json` seal, the GPQA judge + score-patch + integrity re-anchor, the track fixture's per-prompt numeric values | **none** *(for benchd)* | razor | Ownership boundary — benchd does not author these. Organizer material is **report-only: never modified, never re-uploaded**, even where it looks defective (FT-4). **NARROWED per the Q1a ruling — see below.** |
| OOS-16 | The 494-vs-303 divergence | **IN-CORPUS** | **ruled** | **RULED A-SCOPED (Q7)** — one adjudication step inside the primary-leg window, with the pre-agreed rule in §5.2. No longer out-of-scope; it leaves this table and becomes a scoped leg. |
| OOS-17 | `benchctl prefill-decompose` (`main.rs@35c100a:313`) | **none** | razor | **Disposition added pre-freeze (A-10).** A one-shot *diagnostic* subcommand: it measured prefill round-trip elapsed at n = 128/256/512/1024 to fit the M-5 intercept. It has no reference counterpart, authors no sealed artifact, and its output feeds no decision, score, floor or calibration verdict — it produced the OOS-14 argument and is not re-run. P1 ✗ (not submission-reachable) · P2 ✗ (nothing ingested) · P3 ✗ (no verdict moves). |
| OOS-18 | `benchctl overlay-timing` (`main.rs@35c100a:322`) | **deferred** | **deferral** | **Disposition added pre-freeze (A-10).** benchd's LOCAL equivalent of ranked **seam 3**, which on the ranked path is the organizer's trusted shell — benchd never authors the ranked `score.json`. So it is not differ-coverable (no second implementation on the ranked path) and not out-of-scope either: its semantics must stay faithful to seam 3 so a local estimate matches what the organizer would seal. **Deferred to D-2**, which mirrors the live overlay (Y's inline merge at `@2108`, validate `@2141-2153`) and is where the fidelity check belongs. Its `--integrity` re-anchor path is already noted in §2.3 as the reason a re-anchored sidecar loses the reference's field ORDER while keeping every field and value. |

### 5.1 OOS-MISFIT — benchd's seam-1 gates are LOOSER, and the exposure is in OUR pipeline

*reach: **submission-reachable** — but **relocated**, see below. benchd **LOOSER**. P1 ✓.*

This is the one row that fits no bucket, and the razor is why. **The conclusion of the earlier
draft stands; its evidence was wrong in both directions and is replaced here.**

#### (i) The ORGANIZER's seam-1 is SAFE — the earlier draft mislocated the exposure

REF **does** evaluate behavior gates, and it does so **inside the TRUSTED harness**:
`Sources/MLXFastTrustedHarness/QwenRuntimeCorrectness.swift@b26f76f` binds the gates at `:507`
(`let gates = checkGates ? golden.correctnessGates : nil`) and runs the behavior loops at
`:349-367` and `:546-583`, failing with `error: "behavior answer mismatch"` (`:362`, `:578`).

> **Disambiguation (matters — same filename twice).** Two files share the name at `b26f76f`:
> `Sources/MLXFastHarness/QwenRuntimeCorrectness.swift` (the participant/worker copy) and
> `Sources/MLXFastTrustedHarness/QwenRuntimeCorrectness.swift` (the trusted copy). **All line
> numbers above are the TRUSTED copy.** Citing the other one would attribute the organizer's gate
> to participant-replaceable code.

That harness is **sha-pinned and TOCTOU-verified immediately before the gates step**:
`pin-trusted-harness.sh verify "${MLXFAST_JOB_WS}" "${MLXFAST_PRIVATE_DIR}/trusted-harness.sha256"
trusted` at **Y:1588** (with the participant-worker pin at `:1589`), and the gates step itself
sets `MLXFAST_BENCHMARK_CHECK_GATES=1` at **Y:1621**. The env var **defaults TRUE** in the
reference CLI — `environmentValue("MLXFAST_BENCHMARK_CHECK_GATES", fallback: "1") != "0"`
(`Sources/MLXFastCLI/main.swift@b26f76f:385`).

**Corrections to the earlier draft:** the cite `DRAFT-WF @1423-1424` and the spelling `CHECK_GATES`
were both wrong — the variable is `MLXFAST_BENCHMARK_CHECK_GATES` and the live-mirror line is
Y:1621, preceded by the pin-verify at Y:1588-1589. So on the organizer's path the gate is run by
pinned trusted code and benchd's gap is **not** the operative check.

#### (ii) The REAL exposure is default-reachable in OUR paired flow

**Two** scripts carry an independent seam-1 default, and both read the same thing:

```sh
scripts/official-paired.sh@35c100a:110    GATES_PRODUCER="${GATES_PRODUCER:-facade}"
scripts/run-paired-window.sh@35c100a:78   GATES_PRODUCER="${GATES_PRODUCER:-facade}"
```

The second is not an inheritance of the first — it is a separate default in the **window driver**,
which is the entry point a scoring-bearing run actually uses. Fixing only `official-paired.sh`
would have left the reachable path untouched.

**Our seam-1 producer defaults to benchd's own facade.** The facade's `--official` is a
full-parity surface that routes to `benchctl iterate --mode official`
(`scripts/benchmark.sh@35c100a:31-32`). So on any scoring-bearing run of our paired flow that
does not override the env, **benchd is the gates producer** — and benchd's gates are looser than
the reference's in **two** ways at once:

1. **No behavior-gate evaluation.** `CorrectnessScope::Full` runs base + anchors + free_run;
   bench-core conformance carries no `behavior` vector. `benchmark_requires_runtime_worker`
   DETECTS a behavior-carrying golden but nothing evaluates it. A corrupted behavior case
   **PASSES**.
2. **No reference-model pin.** `run_iterate` accepts **no `--contract`** (0 occurrences in
   `main.rs@35c100a`), so on this path `model_provenance` is **shape-only** — the identity pin the
   #114 ruling placed *in the contract* is simply not applied. This is the surface prior drafts
   filed separately as OOS-06 (#119) and demoted on reachability.

**Reachability RELOCATES; it does not demote.** Both items are reachable — just in our pipeline
rather than the organizer's. Merging them is the honest accounting: they are **one root exposure**
(the facade seam-1 default), and one fix closes both.

> **Doc/code inconsistency, report-only:** `official-paired.sh`'s own header says
> `GATES_PRODUCER(benchmark-sh default | direct-swift fallback)` (`:62`, and `:55`), contradicting
> the actual default at `:110`. Whoever reads the header believes the safe producer is already the
> default. Worth fixing whichever way Q1a lands.

#### (iii) Why it fits no bucket

(a) Not differ-coverable as it stands — with no behavior vector in benchd there is nothing to run;
(b) not a D-1/D-2 deliverable — it is a benchd code gap plus a default, not a live-pipeline or
sealing question; (c) **not legitimately out-of-scope** — the razor forbids that for a reachable
looser divergence.

#### RULED (David 2026-08-20) — **(b′) NOW + (a) LATER**

> **Both are adopted, in that order.** (b′) removes the exposure immediately; (a) is the complete
> fix and becomes a named post-flip deliverable.

**(b′) — IMMEDIATE, ruled.** Flip the seam-1 default from `facade` to **`benchmark-sh`** (the
sha-pinned reference producer) for scoring-bearing runs; `facade` stays an explicit opt-in for
parity testing.

**The exposure has TWO independent sites, not one.** The paragraph above originally named only
`official-paired.sh:110`, which understated it. `scripts/run-paired-window.sh:78` carries its
**own** `GATES_PRODUCER="${GATES_PRODUCER:-facade}"` — a separate default, not an inheritance of
the first — and it is the window driver, so it is the one a scoring-bearing run actually enters
through. Both read `facade` at the freeze snapshot and at `1465393`. **The flip covers both
sites**, and both scripts' own headers (`official-paired.sh:55,62` and `run-paired-window.sh:74`)
already *claim* the safe producer, so the fix brings code into line with documentation rather than
the reverse.

**Status at signing: implementation IN FLIGHT as [PR #139](https://github.com/davidtai/mlxfast-bench/pull/139)**
(`lane/pre-window-debt`), commit **`933ed3c`** — *"Ruling Q1a: default the seam-1 gates producer to
the reference chain, not our facade"*. It flips both defaults to `benchmark-sh` and updates
`test-paired-offline.sh`. Recorded here as **signing-time fact** rather than left to a
post-freeze touch-up: this document's header forbids editing it without a ruling, so the accurate
state is landed now.

**(a) — POST-FLIP DELIVERABLE, ruled.** Implement the behavior gate in bench-core conformance and
gate it with a real corpus case. **Gated on pulling the R2 hidden behavior-gate reference**, which
requires **OOS-15 to be narrowed to a pin-verified read** — done below, so the route is no longer
foreclosed. Until (a) lands, (b′) is what holds the P1 line.

**Why not (b) alone:** it leaves our own scoring-bearing default silently accepting behavior
failures *and* running unpinned on model identity. **Why not (c):** (b′) removes the exposure
without blocking completion.

#### The options as presented (retained for the record) — (b)-as-drafted was INSUFFICIENT

- **(a) Implement the behavior gate** in bench-core conformance and gate it with a real corpus
  case. **More tractable than the earlier draft claimed:** the seam-1 hidden behavior-gate **reference**
  **EXISTS in R2** — `PROVENANCE.txt:6` records it as present under the same prefix and
  *not yet pulled* (workflow-env-pinned rather than fixture-pinned). "No real behavior gate exists
  for this model" is true only of **local generation**. **Collision to resolve:** OOS-15 currently
  forecloses this route by classing the R2 hidden behavior-gate reference as organizer-owned/out-of-scope. Choosing
  (a) means narrowing OOS-15 to *"never modified, never re-uploaded"* while permitting a
  pin-verified **read** for gate evaluation — which is exactly how the timed pool is already
  treated (§1.4, §3.2).
- **(b) Signed permanent P1 exception on the ownership argument** — *insufficient as drafted.* The
  ownership argument (the organizer's pinned trusted harness runs the gate) is now **verified
  true** for the organizer's path, but it says nothing about ours, where the facade is the
  default producer. Taking (b) alone leaves our own scoring-bearing default **silently accepting
  behavior failures and running unpinned on model identity**.
- **(b′) Flip the seam-1 default** — *new, and the cheap correct floor.* Set
  `official-paired.sh`'s `GATES_PRODUCER` default to **`benchmark-sh`** (the sha-pinned reference
  producer) for scoring-bearing runs, leaving `facade` an explicit opt-in for parity testing. This
  **converts the exposure from default-reachable to opt-in** and makes our trust boundary mirror
  the organizer's — the same pinned-reference-runs-the-gates arrangement verified in (i). It
  closes **both** looseness items at once, needs no behavior vector, and can land immediately.
- **(c) Block completion** — not recommended; (b′) removes the exposure without blocking.

**Recommended framing: (a) or (b′) — not (b)-as-drafted, not (c).** (b′) is the floor and can land
now; (a) is the complete fix and is newly tractable if OOS-15 is narrowed. They compose: (b′)
immediately, (a) when the R2 hidden behavior-gate reference is pulled.

*Historical note:* prior drafts listed this as an ordinary out-of-scope line ("signed hole"). It
was a signed hole *for coverage*; it is not a signed hole *for security*.

### 5.2 The 494-vs-303 classification — RULED: **OPTION A, SCOPED**

**494 and 303 are TOKENS, not counts.** Any reading as "494 items vs 303 items" is wrong.

**The facts.** In window 4's E2A leg
([#109 comment 5353937166](https://github.com/davidtai/mlxfast-bench/issues/109)), benchd's
local-iterate run reported `first_failing_step = 3`, `expected_token = 494`, `actual_token = 303`,
`case_count = 1`, `checked_steps = 4`. `494` is what the window-4 golden `beefed.json`
(`32045f7e…`, 16,940 B) declares at `expected_tokens[2]`; `303` is what the engine emitted on the
M5 box. The analysis rules out an alignment artifact: `first_failing_step = 3` ⇒
`expected_index = 2` ⇒ **decode step 1**, and a one-index shift would have failed at index 0
(`expected_tokens[0]` is checked against the prefill argmax). Indices 0 and 1 **matched**, then
index 2 diverged — a **genuine teacher-forced divergence** at the third token (near-tie argmax, or
a real regression).

**Why unadjudicated.** REF never ran a token on this golden: it refused at load on **arity** (128
supplied, ≥129 required). No cross-check of the `494` exists.

**Razor reading.** If this is a real engine divergence it is **P3** (same inputs → different
emitted token, i.e. the benchmark does not run the same way) and arguably **P1** (the engine
under submission produces tokens the reference does not). If it is a stale-golden artifact it
serves no prong. Which it is, is exactly what is unknown — so the razor cannot settle it; it can
only say the question matters.

**The frame has changed.** Adjudication runs against **R2-provenance material**, not a
box-generated local comparison target: the pool objects are themselves reference-generated emitted chains,
produced on box 3 by `mtp-verify --emitted <plan> --generate 513` over the transformed weights
tree plus the pinned MTP head (`TRACK.timed_prompt_pool_note`), each carrying `emitted_tokens`
(513) and `rows[].sequential_argmax` with `reference_self_consistent: true` (§3.2). That is the
provenance chain a ranked run trusts.

> **Option A — IN-CORPUS, adjudicated against R2-provenance regeneration.** Re-derive the disputed
> position using the track's own generation path (`mtp-verify --generate`, pinned head,
> transformed weights) and compare both implementations' emitted chain against that reference
> chain.
> - *PASSES:* the window-4 `303` was an artifact of a golden REF refuses plus a generation path
>   the track does not use; nothing owed.
> - *FAILS:* benchd's engine and REF disagree on a decode token under the **track's own
>   provenance** — a P3 (and likely P1) divergence at the engine seam that would block completion
>   for a reason the restructure did not anticipate.
> - *Cost:* one GPU leg inside the primary-leg window; no comparison target to curate, because the generation
>   path is the track's.
> - *Secret-tier:* the regenerated object is R2-derived → **window-workspace only, never
>   committed**; the manifest records a pin, not bytes (§1.4).
> - *Risk:* completion becomes contingent on an engine-level question that may need its own window.
>
> **Option B — OUT-OF-SCOPE.** Classify as a **stale-golden artifact** superseded by #124's arity
> fix: the observation came from a golden both loaders now correctly refuse, generated off a path
> the track does not use, so the measurement never had standing.
> - *Implication:* the `494` is never adjudicated. If it was a real engine regression the battery
>   will not catch it — the surround runs its own fixtures and would not reproduce that token
>   position, and the primary leg's tapes are a different workload.
> - *Cost:* zero.
> - *Risk:* a possible P3/P1 divergence stays unexamined behind a procedural dismissal. The §8
>   re-verify at 129 arity passed 3/3 with **zero deterministic mismatches**, which is *suggestive*
>   for B — but it ran the re-provisioned golden, whose `expected_tokens[2]` was regenerated by the
>   reference, so it did not test the disputed `494` at all.
>
> **Checker recommendation — OPTION A, SCOPED.** One adjudication step inside the
> **already-scheduled primary-leg window**: re-derive the disputed position via the track's own
> `mtp-verify --generate`, then read the regenerated token at that position.
> - regenerated **303** ⇒ close as a **stale-golden artifact**; the window-4 `494` came from a
>   golden both loaders now refuse, generated off a path the track does not use. Done, no escalation.
> - regenerated **494** ⇒ **escalate as an engine-seam divergence** — benchd's engine and the
>   reference disagree on a decode token under the track's own provenance (P3, arguably P1).
>
> This is Option A bounded to a single GPU step with a pre-agreed decision rule, so it carries
> Option A's evidentiary value at close to Option B's cost, and cannot silently expand into an
> open-ended investigation: the escalation branch is a *hand-off*, not more work inside this gate.
>
> ### RULED David 2026-08-20 — **A-SCOPED**
>
> Adopting the checker's recommendation, with its pre-agreed decision rule **verbatim**:
>
> > One adjudication step inside the already-scheduled primary-leg window, re-deriving the
> > disputed position via the track's own `mtp-verify --generate`.
> > **Regenerated `303` ⇒ close as a stale-golden artifact.**
> > **Regenerated `494` ⇒ escalate as an engine-seam divergence and hand off.**
>
> The escalation branch is a **hand-off, not more work inside this gate** — that boundary is part
> of the ruling and is what keeps A-scoped from expanding into an open-ended investigation.
> The regenerated object is R2-derived: **window workspace only, never committed**; the manifest
> records a pin, not bytes (§1.4 rule 2).

---

## 6. Sequence / board

### Stage 0 — IN-FLIGHT (clear the board)

**Board refreshed at the freeze snapshot.** The prior draft's board was written against a
board that had already moved.

| item | state | must produce |
|---|---|---|
| **PR #133** | ✅ **MERGED** `35c100aa`, 2026-08-20T15:45:13Z | *Done.* F1(b) MIRROR-BLANK-STRICTLY, F3 (roster 7→8), F4, F5/F6, plus review commit `f8ed398` (F-1 stale ruling comment, F-2 official baseline resolution, F-3…F-7) and `d8e5179` (F-8). **This cleared 5 BLOCKED corpus cases and resolved 2 sign-off questions.** |
| **#134** engine hello handshake | ✅ **MERGED** — PR #136, `1465393` | *Done.* Worker-stderr surfacing on the failed-hello path, plus a secret-scrubber (`crates/bench-runner/src/scrub.rs`) hardened against real secret shapes. **The GPU block is LIFTED** — stage 2 is now runnable. |
| **preflight gate** | OPEN → **PR #138** `lane/window-preflight-gate` | the pre-GPU gate for stage 1, including R2 fetch-and-verify. Titled *"pin the environment seam, smoke the spawn seam, enforce single-flight"*. |
| **this document** | OPEN → **PR #137** `lane/parity-completion-gate` | David's sign-off. |
| **#135** measure-job sidecar premise | **RULED (Q6)** | **TWO ARTIFACTS** — organizer-shape `results.json` sidecar + full 8-key local sidecar. Implementation is D-1's item 6. |
| **#131** no-artifact class | **RULED (Q4)** | **R-10a + option (2)** — write the early-refuse record, seal the 3 weights fields EMPTY as a declared divergence, no 15 GB pre-hash. Unblocks corpus case R-10. |
| **#125** stale fuzz report | OPEN | a regeneration commit, or a non-writing check mode. |

> **Board findings, corrected.** The prior draft asserted #133 was "the only open PR" and that
> `lane/window-preflight-gate` / `lane/134-handshake-stderr` sat at `origin/main` with zero
> commits. **Both statements were already false at authorship time.** #133 is merged; three PRs
> are open (**#136**, **#137**, **#138**); and the two lanes have real work on them. The lesson
> for the checker chain: board state is the fastest-moving claim in this document and must be
> re-derived, never carried forward.

### Stage 1 — PREFLIGHT + SMOKE (GPU-free)

**Consumes:** merged `main` (post-#133), a clean tree (post-#125), this frozen enumeration.
**Runs:** R2 fetch-and-verify (8/8 against TRACK, recording which fixture revision — FT-4; any
mismatch is die-8, pre-GPU); `measure-job --preflight-only` (`main.rs@35c100a:659` (flag) / `:415` (field));
families **N** (generation), **L** (15), **Z** (183), **T** (6); family **V** regeneration at
`STEPS=129`; the offline suites (`compat-matrix.sh`, `test-variant-offline.sh`,
`test-official-offline.sh`, `test-paired-offline.sh`, `fuzz-corpus-check.sh`); and the differ
self-test (`parity-diff.py --selftest` → `cargo test -p benchctl parity`).
**Produces:** a GREEN offline gate; verified R2 pool; family-N fixtures with fresh pins;
regenerated family-V pins; a recorded differ version string.
**Fails if:** any roster drift gate fires; any TO-GEN fixture fails dual-loader accept; the differ
self-test does not exec the real `cargo test`; or R2 verification is anything but 8/8.

### Stage 2 — ONE FULL WINDOW (the gate proper)

**Consumes:** stage 1's products and the GPU lock. **#134 is closed** (PR #136, `1465393`), so this stage is runnable.
**Runs, one window, outer-hold / inner-direct lock policy:** (1) the **PRIMARY LEG** — 8 track
prompts through both implementations, diffed; (2) the **differ battery** — families **V** (7),
**C** (16), **R** (12), **A** (15), **N** (9 live halves) via `run-variant-window.sh`,
`run-official-window.sh`, `failure-map.sh`, `official-failure-map.sh`, `facade-leg.sh`; (3) the
**sealed measure-job window (D-1)** with the calibration band exercised and `SerialBandOutcome`
recorded.
**Thermal + lock (declared, symmetric):** cool-gate OFF both sides for local-iterate (benchd by
the RULING-A3 default; Swift forced OFF via `MLXFAST_LOCAL_COOL_GATE=0`) so the residual is a
same-conditions number. `qwen` unloaded at entry, **trap-guaranteed** reload at exit with serving
verified, lock released. Artifacts replicated on completion (success **or** fail) — never fails
the run (P-3).
**Produces:** the completion evidence. Under term (d), battery GREEN closes wholesale every frozen
cell in the differ-coverable column.
**Fails if:** any UNDECLARED FAIL; any TOOL-ERR rendering as a verdict; any leg that cannot record
all six pinned-SHA items (§2.1); the oracle class failing to fail both sides (aborts LOUD by
design); or a both-PASS on any assertion whose point is a both-FAIL.

### Stage 3 — RANKED MIRROR + RETRY

**Consumes:** stage 2's outcome. **Runs:** D-2 (§2.4), epoch coherence first (both levels — the
track-id and the FT-4 pool mismatch); plus the **one gated retry** with full precondition reset for
any stage-2 leg that failed on an infra class (throttle / insufficient-telemetry / stall /
implausible s-per-tok / row-accounting) rather than a parity class.
**Produces:** closure of the two-deliverable column, and with stage 2 every frozen cell not in §5.

---

## 7. SIGN-OFF — RECORD OF RULINGS

> ### ✅ SIGNED — David, 2026-08-20
>
> Structured Q-block interview, recorded as **`q-block-137-signoff-rulings`**. All nine questions
> answered. **This document is the frozen gate definition and the program's definition of done.**
> Changing it requires a new ruling, not an edit.
>
> **What was signed (Q1):** that **P1 security, P2 Yukon compatibility, and P3
> benchmark-execution equivalence** are what *done* means; and that **this corpus plus D-1 and
> D-2 are sufficient evidence** for those three prongs. The signature is on the **definition** and
> the **sufficiency** — not on each of the 271 rows individually.

| # | question | ruling | folded at |
|---|---|---|---|
| **Q1** | Three prongs = *done*? Evidence sufficient? | ✅ **CONFIRMED** | §1, header |
| **Q1a** | Behavior-gate looseness (OOS-MISFIT) | **(b′) NOW + (a) LATER** — flip the seam-1 default immediately; behavior-gate implementation is a named post-flip deliverable | §5.1 |
| **Q2** | Any out-of-scope row pulled in? | **Ledger ACCEPTED as drafted** — no changes | §5 |
| **Q3** | FT-4 pin authority | **`5677d53f…` / 44589 B**; the two 3.6-pool copies stay **report-only defects** | §3.3, §4.10 |
| **Q4** | #131 no-artifact class | **R-10a with option (2)** — real byte-compare; seal diverges on the 3 declared weights fields; **no** 15 GB pre-hash | §4.7 |
| **Q5** | Family V pins | **From the window REPORT** — rule as written | §4.9 |
| **Q6** | #135 sidecar premise | **TWO ARTIFACTS** — `results.json` organizer-shape + full 8-key local sidecar | §2.4 D-1 |
| **Q7** | 494-vs-303 | **A-SCOPED**, checker's decision rule verbatim | §5.2 |
| **Q8** | FT-1 (tapes vs local modes) | **FACT ACCEPTED** — does not block a leg | §3.3 |

### The two follow-on deliverables this signature creates

| # | deliverable | status | gate |
|---|---|---|---|
| **(b′)** | Flip the seam-1 default `facade` → `benchmark-sh` for scoring-bearing runs at **both** sites — `official-paired.sh:110` **and** `run-paired-window.sh:78`, which carries an independent default — plus the headers that already claim `benchmark-sh` (`official-paired.sh:55,62`, `run-paired-window.sh:74`) | **RULED; implementation IN FLIGHT at signing** — both sites still read `facade` at `35c100aa` and `1465393` | [PR #139](https://github.com/davidtai/mlxfast-bench/pull/139) `lane/pre-window-debt`, commit `933ed3c` |
| **(a)** | Implement the behavior gate in bench-core conformance + gate it with a real corpus case | **RULED, post-flip** | gated on pulling the R2 hidden behavior-gate reference, now permitted by the **narrowed OOS-15** (§5) |

### Ledger at signing

| | |
|---|---|
| Primary leg | **8** track prompts (pins recorded, 8/8 reconciled) |
| Synthetic surround | **263** — 259 EXISTS · 4 TO-GEN · 0 UNRULED ✅ |
| **Battery total** | **271** |
| Frozen matrix rows | **29** (`docs/parity-matrix.md@35c100a:35-63`) |
| Out-of-scope | **16 live** — 14 razor dismissals · 2 deferrals. OOS-16 ruled **in-corpus** (Q7) and OOS-06 **withdrawn**, so 18 ids − 2 = 16 |
| Structurally-uncovered deliverables | **2** (D-1, D-2) |
| Open adjudications | **0** |

### What is NOT settled by this signature

Named explicitly, so nothing rides on silence:

- **(b′) is ruled and in flight; (a) is ruled and unbuilt.** The P1 exposure in §5.1 (ii) is
  closed *by ruling*; (b′)'s code is [PR #139](https://github.com/davidtai/mlxfast-bench/pull/139)
  (`933ed3c`), **not yet merged**. Until it lands, both facade defaults still stand and our paired
  flow's scoring-bearing default still runs benchd as the seam-1 gates producer.
- **R-10 needs the #131 fix** before its case can run.
- **#134 was merged** (PR #136, `1465393`), so the GPU block is lifted — but no window has run
  under this gate yet. Every "must show" in §2.4 and §3.4 remains `UNVERIFIED` until one does.
- **The 494-vs-303 outcome is unknown.** A-SCOPED settles *how* it gets adjudicated, not what the
  answer is; the `494` branch escalates out of this gate.
