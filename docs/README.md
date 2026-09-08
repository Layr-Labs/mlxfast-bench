# docs/

Every document here carries a **class**. The class tells you what the document is for and
how much weight a citation to it carries.

| class | meaning |
|---|---|
| **normative contract** | Binding. Code cites it as its definition; a change here is a change to behaviour. |
| **runbook** | Operational procedure. Follow it as written. |
| **architecture** | Describes the shape of the system. Explanatory, not binding. |
| **governance** | Records a ruling or a ledger. Binding as a rule, not as a spec. |
| **history** | A record of superseded work. Kept for reasoning and provenance. **Never guidance.** |

## Index

| document | class | what it is |
|---|---|---|
| [`architecture.md`](architecture.md) | architecture | The benchmarker/engine split: target design, Engine Protocol v1, engine-consistency layers, the privilege/ring security model, six red/green teaming cycles. §7 and §9 are marked executed/resolved in place. |
| [`spec-config-design.md`](spec-config-design.md) | normative contract | The per-module speculative configuration wire surface (`spec` / `effective_spec`). Cited as the contract by the frozen Protocol v1 JSON Schema and by 14 sites in `bench-protocol` / `bench-runner`. |
| [`model2-calibration.md`](model2-calibration.md) | normative contract | The model-2 new-series calibration regime: the series fence, native-vs-model-2 segregation, the series-scoped band gate. Enforced in `crates/benchd/src/measure_job.rs`. The band VALUES are still to be measured on box. |
| [`scored-regime-and-prefill-window.md`](scored-regime-and-prefill-window.md) | normative contract | What a track declares that it scores (batch size + composite exponents, one regime per `track_id`), and what benchd does with the prefill half of the free-run timed window. The declaration arms the certification. §3 is the standing limit: no track may declare a nonzero prefill exponent until a work-placement invariant exists. Enforced in `crates/bench-core/src/{constants,prefill_window}.rs` and at four scoring seams in `crates/benchd`. |
| [`track-release-branches.md`](track-release-branches.md) | governance | How model tracks bind to this repo: the release branch is the project (it can serve more than one platform), the `track_id` is the platform namespace and the R2 key prefix. Branch lifecycle, the two benchd resolution channels, engine-repo counterpart, golden authoring. |
| [`parity-completion-gate.md`](parity-completion-gate.md) | governance | **SIGNED, frozen.** The program's definition of done for MLX parity. Changes require a new ruling, not an edit. |
| [`window-preflight.md`](window-preflight.md) | runbook | The mandatory pre-lock gate for every GPU window. |
| [`box-setup-runbook.md`](box-setup-runbook.md) | runbook | Overview of box setup for both platforms: shared pieces, baseline calibration, differences, faults. |
| [`runbook-box-setup-cuda.md`](runbook-box-setup-cuda.md) | runbook | Stand up a DGX Spark (CUDA) ranked box: readiness checklist, fleet peer copy, baseline calibration. |
| [`runbook-box-setup-mlx.md`](runbook-box-setup-mlx.md) | runbook | Stand up a Mac (MLX) ranked box, with a readiness checklist. |
| [`official-baseline-capture.md`](official-baseline-capture.md) | runbook | How to capture a PENDING track's official baseline pair with `benchd iterate --capture-baseline`, and what the capture record is for. |
| [`EVICTED.md`](EVICTED.md) | governance | The redirect ledger for paths evicted from `main`, plus the eviction and citation rules. |
| [`history/`](history/) | history | Superseded planning material — see below. |

The Engine Protocol v1 wire definition itself is **not** here: it lives with the crate that
owns it, at [`crates/bench-protocol/PROTOCOL.md`](../crates/bench-protocol/PROTOCOL.md) plus
the JSON Schema beside it. Both are frozen.

## `history/`

| document | superseded by |
|---|---|
| [`history/dgx-spark-implementation-plan.md`](history/dgx-spark-implementation-plan.md) | The DGX Spark platform was RETIRED as the CUDA target on 2026-08-20; the ruled box is RTX PRO 6000 Blackwell class. Its performance numbers and NVFP4 path do not transfer. |
| [`history/execution-plan.md`](history/execution-plan.md) | The split shipped. Kept for the ticket decomposition and acceptance criteria. |
| [`history/dependency-graph.md`](history/dependency-graph.md) | A snapshot of the build-out issue graph, not a live tracker view. |
| [`history/fuzz-corpus-report.md`](history/fuzz-corpus-report.md), [`history/fuzz-corpus-report.txt`](history/fuzz-corpus-report.txt) | The M-4 loader fuzz corpus freeze report. Stale relative to later corpus changes; the live check is `scripts/fuzz-corpus-check.sh` against the frozen corpus fixture. |

## The citation rule

1. **Unpinned citations may only target LIVING documents.** Writing `docs/foo.md` or
   `docs/foo.md:120` in code, a script, or emitted output promises the reader can open that
   path in the working tree.
2. **Citations to `history/` must be `@sha`-pinned.** History documents are frozen records;
   an unpinned cite to one implies it still describes the system, which it does not.
3. **Citations to evicted paths must be `@sha`-pinned and listed in
   [`EVICTED.md`](EVICTED.md).** A pin must resolve *in this repo* — a sha naming a
   pre-squash branch commit is a dangling pointer, not a citation.
4. **Line-numbered pins are re-verified when repinned.** If you move a pin to a different
   sha, open the file at that sha and confirm the lines still say what the citation claims.
