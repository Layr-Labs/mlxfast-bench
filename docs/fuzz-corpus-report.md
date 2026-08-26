# M-4 — structure-aware fuzz corpus for the golden loader (issue #69)

Extends the 12-fixture loader-parity corpus (the §6 loader-parity surface) to a large,
deterministic, frozen fuzz corpus that probes the golden **loader's** accept/reject
decision. Both loaders (Rust `benchctl validate-golden`, Swift `mlxfast-swift preflight`)
run on the same corpus; verdicts must **agree or be declared**. No engine, no GPU.

## Artifacts

| piece | path |
|---|---|
| generator (deterministic, seeded) | `scripts/gen-fuzz-corpus.py` |
| frozen corpus + pinned manifest | `crates/bench-core/tests/fixtures/golden_fuzz/` |
| local check (freeze + benchctl verdicts) | `scripts/fuzz-corpus-check.sh` |
| dual-loader harness (box, both loaders) | `scripts/loader-parity.sh` (drop-in; §6's harness) |
| Rust freeze + verdict test | `crates/bench-core/tests/loader_fuzz.rs` |
| benchctl-side + freeze report | `docs/fuzz-corpus-report.txt` |

## Generator design

- **Structure-aware, not engine-facing.** Every fixture mutates the golden's *structure*
  so the loader itself must accept or reject: wrong field types, out-of-bounds counts,
  missing required keys, extra unknown keys, malformed gate shapes, boundary token values
  (`0`, negative, `VOCAB_SIZE` edge, `2^40`), duplicate JSON keys, truncated / non-JSON
  payloads, wrong/`null` `model_type`, explicit nulls, layered duplicate case names. (This
  is orthogonal to `gen-failure-corpus.py`, whose variants stay *valid* and fail later at
  run time.)
- **Deterministic.** Seeded `random.Random(SEED=20260817)` supplies only in-vocab filler
  tokens; the mutation set itself is enumerated, not sampled. No wall-clock, no unseeded
  randomness. Re-running yields **byte-identical** fixtures (verified: two regenerations
  hash equal).
- **Labeled.** Each fixture carries its `mutation`, a `note`, `expected_rust`
  (ACCEPT/REJECT), and `swift_diverges`.
- **Families:** valid variants (Q), model_type (A), version (B), top-level structure (C),
  base-case fields (D) + unknown keys (E), correctness_gates shape (F), anchor (G/H),
  free_run (I/J), behavior (K/L), benchmark block (M/N), layered duplicate names (O),
  JSON-level malformations (P).

## Corpus

- **N = 183** fixtures (`ACCEPT = 5`, `REJECT = 178`).
- **Frozen pin** — sha256+bytes of every fixture recorded in `manifest.json`; the
  aggregate corpus hash (all fixture bytes) is
  `b4cacec0c082b177d06a9d6a680d29ac755322c7b4f1cc563e41ffd3a7ec7042`.

## benchctl-side results (VERIFIED locally)

Ran `benchctl validate-golden` on all 183 fixtures: **183/183 match `expected_rust`**,
freeze-drift = 0. The Rust loader half of the parity is fully verified locally. (`benchctl`
is exercised by `scripts/fuzz-corpus-check.sh`; `load_golden_fixture` — the same code path —
by the Rust test.)

## Declared divergences (21 predicted)

The **only** class where the two loaders can differ is **unknown keys inside inner
objects**: Rust's serde `deny_unknown_fields` rejects them at every level, while Swift's
`JSONDecoder` silently **drops** unknown keys inside `Codable` structs. Swift key-set-
validates only the **top level**, so an unknown *top-level* key is a shared REJECT (no
divergence) — that class is included as agreeing fixtures.

- **Confirmed family (15)** — per-case unknown keys on base / anchor / free_run / behavior
  cases (`case_unknown_key_*`, `anchor_unknown_key_*`, `free_run_unknown_key_*`,
  `behavior_unknown_key_*`). Same class as the box-VERIFIED `per_case_unknown_key` in §6:
  Rust REJECT, Swift ACCEPT (drops the key). Intentional, anti-cheat: Rust is stricter.
- **Predicted, confirm on box (6)** — unknown keys in the **benchmark block**
  (`benchmark_unknown_key_*`) and the **gates section object** (`gates_unknown_section_*`).
  Same drop-vs-reject mechanism, one level in; not yet box-verified. Marking them
  `swift_diverges` means the dual harness records them as MATCH (if Swift also rejects) or
  KNOWN-DIV (if Swift drops) — **never** an undeclared MISMATCH, so the box run cannot fail
  on this prediction either way.

All other 162 fixtures are `swift_diverges=false`: both loaders do identical explicit
semantic validation (counts, ranges, non-empty, duplicate-name, paired-baseline, explicit-
null guards) or fail identically at JSON decode, so both REJECT (or both ACCEPT the 5 valid
variants).

## Box command — full dual-loader run (both binaries present)

The Swift leg needs the Swift binary + a transformed-weights dir (preflight is weights-
coupled but model-free — no GPU, no Qwen unload). On the box:

```
BENCHCTL=target/release/benchctl SWIFT=<mlxfast-swift> WEIGHTS=<weights-dir> \
  scripts/loader-parity.sh \
  crates/bench-core/tests/fixtures/golden_fuzz \
  docs/fuzz-dual-loader-report.txt
```

or, to also re-verify the freeze pin + benchctl verdicts in the same pass:

```
BENCHCTL=target/release/benchctl SWIFT=<mlxfast-swift> WEIGHTS=<weights-dir> \
  scripts/fuzz-corpus-check.sh
```

`loader-parity.sh` first sanity-gates the Swift leg on `valid.json` (aborts if the known-
good golden is rejected — a broken weights setup, not a golden decision), then reports
`match` / `known-divergence` / `MISMATCH` counts. Expected: 162 match, 21 known-divergence
(15 confirmed + 6 to confirm), 0 MISMATCH.
