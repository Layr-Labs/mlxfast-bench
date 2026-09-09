# Evicted paths — the redirect ledger

Files removed from `main` under the eviction ruling are **not** removed from history. This
ledger is the redirect layer: it names the evicting commit, the last revision at which the
content lived, and where the thing the file was doing lives now.

## The standing rule

1. **Eviction requires a redirect row.** A file may not leave `main` without an entry here
   giving its evicting commit and its successor (or, if there is none, saying so explicitly).
2. **Unpinned code citations may only target LIVING docs.** A bare `docs/foo.md` or
   `docs/foo.md:120-140` in code or a script is a promise the reader can open that path in
   the working tree. Once a doc is evicted, every surviving citation to it must be
   `path@sha` (or `path@sha:line`) so it resolves with `git show <sha>:<path>`.
3. **Pins must resolve in THIS repo.** A `@sha` naming a pre-squash branch commit is a
   dangling pin, not a citation. Repin to the merged sha and re-verify line correspondence
   before writing it down.

## Ledger

| evicted path | evicted at | last living revision | successor / resolution |
|---|---|---|---|
| `docs/measure-job-contract.md` | `cd5782e` (#155) | `fe2da64` | **Normative in code**: `crates/benchctl/src/measure_job.rs` (paired flow, retry, rejection classes), `docs/architecture.md` §8. Surviving cites are pinned `@fe2da64` (final revision) or `@7c6be14` (the WS2-5 revision the line-numbered cites were written against). |
| `docs/paired-flow-design-note.md` | `cd5782e` (#155) | `3af70c8` | **Implemented**: the three seams are `scripts/official-paired.sh` (driver), `benchctl measure-job` (A-1), the gates producer (A-2), `benchctl overlay-timing` (A-3). Surviving cites pinned `@3af70c8`. |
| `docs/parity-matrix.md` | `fe2da64` (#159) | `fe2da64^` | **`crates/benchctl/tests/fixtures/waiver-ledger.json`** — the §13 waiver rows as structured data, read directly by `parity.rs`'s sign-off test. `docs/parity-completion-gate.md`'s `@35c100a` cites resolve unchanged via git. |
| `docs/iterate-128-window-reverify.md` | `cd5782e` (#155) | `2bb88c0` | Record doc; its result is carried by `docs/parity-completion-gate.md` §8 and by the pinned baseline constants. No living successor doc. |
| `docs/manual-driver-acceptance.md` | `cd5782e` (#155) | `4818991` | Machine-state record. No successor; superseded by the window runbooks under `scripts/`. |
| `docs/manual-driver-acceptance-submit.md` | `cd5782e` (#155) | `ff3f03c` | As above. |
| `docs/manual-driver-acceptance-closer.md` | `cd5782e` (#155) | `91a9575` | As above. |
| `scripts/fixtures/window-pins.example` | `cd5782e` (#155) | `4edc1cb` | Machine-state fixture. Live window pins come from the signed track fixture, not a checked-in example. |
| `scripts/fixtures/window-pins.retry-draft` | `cd5782e` (#155) | `b41cc2a` | As above. |

## Known outstanding reference

`crates/benchctl/tests/fixtures/swift-official-baseline-constants.json` carries an unpinned
prose reference to `docs/iterate-128-window-reverify.md`. That file is a **verbatim reference
copy** and is out of scope for edits; the reference is recorded here instead so it stays
resolvable.
