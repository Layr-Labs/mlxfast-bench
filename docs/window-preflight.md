# The window-preflight gate

**Every GPU window runs `scripts/window-preflight.sh` before the lock. No exceptions.**
That includes the Proof A retry, every measure-job window, every parity re-verify, and every
one-off diagnostic that touches the GPU.

---

## Why this exists

The program pins and fails closed at every **code** seam — golden `sha256`+`bytes`, spawn argv,
protocol version, the #123 roster superset — and never extended that rigor to the
**environment** seam. Proof A (2026-08-20) paid for the gap twice in one window:

1. **Neither required pin was provisioned.** The bench checkout was on the wrong branch and the
   parity engine repo was absent from the box entirely, shipped ad hoc by bundle mid-window.
   This was discovered *under the lock*, on the clock.
2. **Every live leg then died at the spawn seam** — `engine hello handshake failed: protocol
   violation: engine closed the stream before returning a response` (#134). The GPU-free
   preflight the window did run, `measure-job --preflight-only`, had returned `EXIT 0` minutes
   earlier, because it returns at `main.rs:1205-1215`, *before the first spawn*.

Both failures are cheap to catch and expensive to discover late. The gate catches them in
about a minute, and seals what it saw.

---

## Phase order

Each phase fails closed before the next. The lock is taken exactly once, **after** everything
that can be checked without it has already passed.

| # | phase | lock? | what it establishes |
|---|---|---|---|
| 1 | **PINS** | no | The tree seam. Bench + engine checkouts present, at the pinned commit, **clean**, with provenance (remote or a pinned bundle). Weights `sha256`+`file_count`+`byte_count`. Goldens and pool tapes `sha256`+`bytes`. Contract `sha256`. |
| 2 | **BASICS** | no | The box seam. Binaries present, executable, and **interrogated for their own identity**. Goldens accepted by the loader the run will use; pool tapes carrying the signature the legs route on. Disk above an explicit floor. Box quiet. Serving model in the expected state. Session lock **acquirable**, no stale holder. |
| 3 | **ACQUIRE** | **takes it** | Atomic `mkdir` on **the box lock**, recording holder tag, pid, user, and the **box's own** acquisition timestamp. Reaps a provably-dead holder first (see below). |
| 4 | **UNLOAD** | holds | `qwen_unload`, then poll until the process is *actually gone*. `rc=0` is not proof. |
| 5 | **SMOKE** | holds | The spawn seam, for real. See below. The **leg** is waivable (`WP_SMOKE_RECIPE=none` / `--no-smoke`); **acquisition is not** — a waived smoke leg still takes and holds the lock, and the waiver is recorded. |
| 6 | **HOLD** | holds | On success the gate **exits still holding the lock**. The window proper runs inside it — there is no gap in which another session could take the box. The driver must then **adopt** it (below). |
| 7 | **RELEASE** | releases | `--release` reloads the serving model, verifies it is back, and releases the lock **this session** took. |

If the gate fails at or after phase 3, its trap **unwinds**: reload, release. A failed gate
never leaves the box locked-and-unloaded.

**Both ends trap.** The laptop side unwinds on its own failure; the **box side** traps
`HUP`/`INT`/`TERM` (never `EXIT` — a normal exit is the handoff) and, if it had unloaded the
serving model, **reloads it before releasing**. That ordering matters: the box side is what took
the model down, so it is the only thing in a position to put it back. An ssh drop therefore ends
with the box unlocked *and serving*, not merely unlocked. When that happens the laptop's
subsequent release correctly reports `not-held`, and the gate warns that the reload is
unconfirmed rather than staying silent.

---

## The smoke leg

A miniature of the **real** path: the actual `benchctl` binary spawning the actual worker
binary over the actual transport — env scrub, hello handshake, at least one real round trip.
Roughly 60 seconds of GPU that would have caught #134 before a whole window was spent on it.

Recipes are named and pinned (`WP_SMOKE_RECIPE`), and are built from pins the gate has *already
asserted*, so the smoke leg cannot drift from the tree that was just cleared.

| recipe | invocation | proves | notes |
|---|---|---|---|
| `handshake` | `benchctl prefill-decompose --engine <E> --weights <W> --sizes 1,2 --reps 1` | spawn, env scrub, hello (`id=0`, `ok`, `nonce`, `protocol_version=1`), 2 real round trips | The cheapest real spawn in the tree. Runs the **local** spawn path, where `forward_worker_stderr` is `true` (`transport.rs:476`), so a dying worker's own stderr reaches the gate prefixed `mlxfast-worker:`. **This is the diagnosable one — always run it.** |
| `decode` | `benchctl measure-job … --candidate-spec '{"mode":"mtp","mtp":{"depth":2}}' --min-pairs 1 --target-pairs 1` | a real `decode_begin` + `decode_step` over the **sandboxed** transport | The smallest invocation that reaches a decode verb **with the real leg's argv shape**. The spec is a FREE-RUN regime on purpose: a `{"mode":"serial"}` spec is teacher-forced and its argv is a strict prefix that omits `--speculative-protocol`, so an engine rejecting that flag would sail through the smoke leg and then kill every real leg pre-GPU. `--tokens` is deliberately absent — on the free-run branch benchd makes it a hard usage error unless it equals the ruled `FREE_RUN_DECODE_TOKENS`. Needs `WP_CONTRACT_PATH` (its `timed_prompt_pool` must pin the first comparison input) and `QMTP_HEAD_DIR`. On this path benchd forwards **no** worker stderr (`sandbox.rs:277`), so a failure yields only the `RunnerError` string. |
| `both` | handshake, then decode | both of the above | Recommended for a measure-job window: the cheap diagnosable leg runs first. |
| `custom` | `WP_SMOKE_ARGV` verbatim | whatever you pinned | |
| `none` | — | nothing | A **declared waiver**, recorded in the attestation. The lock is still **acquired and held** — only the leg is waived. |

There is deliberately **no new `benchctl smoke` subcommand**: adding one would mean editing the
spawn-seam files the #134 lane owns. The gate builds on the CLI surface that exists today.
If a first-class one-shot smoke verb is added later, `WP_SMOKE_RECIPE` gains a name and nothing
else changes.

### Verdicts the gate distinguishes

| observed | verdict | exit |
|---|---|---|
| `rc=0`, no error signature | **PASS** | 0 |
| stderr matches `closed the stream before returning a response` / `hello handshake failed` | **FAIL** — spawn seam broken (the #134 signature). The diagnostic names *both* causes it could be, because the engine also exits 1 on an unknown argv flag *before* the hello and that surfaces identically (`main.rs:1338-1348`). Forwarded worker stderr is quoted. | 8 |
| stderr mentions `protocol_version` | **FAIL** — engine speaks a different protocol version | 8 |
| no response within `WP_SMOKE_TIMEOUT_S` | **FAIL** — hung at the handshake (killed; the gate does not hang) | 8 |
| `rc≠0` otherwise | **FAIL** — handshook but did not complete the round trip | 8 |
| the handshake signature, with `WP_EXPECT_SMOKE_FAIL=<ref>` | **EXPECTED-FAIL(\<ref\>)** — the declared dependency reproduced. Labelled, **not** waived | 8 |
| an engine process survives the leg | **FAIL** — a stray worker holds GPU memory into the next window | 8 |

Worker stderr is sealed into the attestation **either way**. A green leg that logged warnings
is evidence too, and on the sandboxed path benchd drops the drained tail on `Drop`, so the
attestation may be the only place a failing worker's own words survive.

---

## The environment seam

Pinning the engine binary's sha256 is worth nothing if the environment can redirect the spawn
somewhere else. `WP_ENV_<NAME>` pins an expected **value**, or the literal `unset`; `unset` and
empty are different observations, and the distinction is preserved. The whole `MLXFAST_*` /
`QMTP_*` namespace as observed on the box is sealed into the attestation regardless.

Six are required — declare `unset` rather than omitting, so the expectation is on the record:

| variable | why it is spawn-critical |
|---|---|
| `MLXFAST_RUNTIME_WORKER_EXECUTABLE` | on `iterate --mode official` the sandbox takes it **verbatim** in preference to the resolved path (`sandbox.rs:229-236`) — defeats `WP_ENGINE_BIN_SHA256` outright |
| `MLXFAST_MEASURE_WORKER_BIN` | changes **which file inside the workspace** is spawned (`main.rs:1035-1038`) |
| `MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE` | short-circuits profile generation **and** the `sandbox-exec` probe (`sandbox.rs:257-259`): nominally sandboxed, effectively not. The highest-value variable here to assert `unset` |
| `MLXFAST_NO_SANDBOX` | `"1"` turns the whole window into an instant refusal |
| `MLXFAST_USE_RUNTIME_WORKER` | `"0"`/`"false"` likewise |
| `QMTP_HEAD_DIR` | the engine's `--mtp-head` argv value. Checked **after** `--preflight-only` returns (`main.rs:1219-1225`), so a GPU-free preflight cannot catch it unset — and a real leg then dies 8 *before the spawn*, which reads as a post-handshake failure to anyone not looking |

## The smoke leg must match the leg's SHAPE

A real candidate leg spawns, through `sandbox-exec`:

```
<worker> runtime-worker --weights W --mtp-head H --speculative-protocol v1.1
```

`--mtp-head` is emitted on every leg; `--speculative-protocol v1.1` **only** when the candidate
regime is free-run, i.e. `spec.mode != "serial"` (`measure_job.rs:179-185`, `:699-706`). The
engine fences its own argv against `RUNTIME_WORKER_ACCEPTED_FLAGS` and exits 1 on an unknown
option **before writing the hello** — which surfaces as exactly the same "engine closed the
stream before returning a response" that #134 reported.

So a `{"mode":"serial"}` smoke leg is a strict **prefix** of a real leg: it omits
`--speculative-protocol` entirely, and an engine that rejects that flag would sail through the
smoke leg and then kill every real leg. The `decode` recipe therefore **speculates**
(`{"mode":"mtp","mtp":{"depth":2}}`) and omits `--tokens` (a hard usage error on the free-run
branch unless it equals the ruled `FREE_RUN_DECODE_TOKENS`, `main.rs:838-849`).

Offline, only the recipe's **argv** is exercised — the leg itself needs a real contract whose
`timed_prompt_pool` pins the golden, plus `QMTP_HEAD_DIR`. The argv is asserted to survive one
round of shell parsing with its spec JSON intact.

## Expected posture against the current tree

Until PR #136 (the observability half of #134) lands and the engine seam is fixed, the
`handshake` recipe is **EXPECTED to FAIL** against a real box. That is the gate working, not
the gate broken.

This is a **verdict, not a note**. Pin `WP_EXPECT_SMOKE_FAIL=#134` and a reproduction of that
signature is graded `EXPECTED-FAIL(#134)` — in the row, in `smoke.verdict`, and in the
diagnostic. It does **not** waive the failure: the exit code stays 8 and the gate still fails,
because the window still must not proceed. What it buys is that a known-open dependency reads
as a known-open dependency instead of being re-diagnosed from scratch. Set it back to `none`
once the fix lands. The fixture matrix proves the classifier independently of any box.

## BARE PROBES ARE PROHIBITED

**Any** diagnostic that touches the GPU — a full window, a smoke leg, a one-off measurement, a
hypothesis probe — runs under the box lock and in the same environment class as the legs it
is reasoning about.

The Proof A **D1-class standalone probes** are the cautionary case: they were run bare, outside
the lock and outside the residency conditions of the legs they were meant to explain, and so
produced an **invalid control**. The number they returned described a different machine state
than the one under test, and no amount of care in the arithmetic afterwards could recover that.

The rule follows directly: **a control is only valid if it shares the lock and residency
conditions of what it controls for.** If a probe is worth running, it is worth running inside
the window. If it is not worth the lock, its result is not worth citing.

Operationally: take the lock with the gate (`WP_SMOKE_RECIPE=handshake` at minimum), run the
probe, then `--release`. The attestation and release record together prove single-flight.

---

## One lock, and who may reap it

**RULED (David, 2026-08-20).** Two decisions, both load-bearing.

### Single lock, drivers inherit

The gate acquires and holds **`/tmp/mtplx-box-exclusive.lock.d`** — *the* box lock, the one
every actor already respects — not a private session lock beside it. Single-flight therefore
covers the whole span from preflight through the window, with no gap in which another session
could take the box.

`scripts/run-paired-window.sh` gained **holder-tag inheritance** (`:148-163`), and nothing else:

* `mkdir` succeeds → acquire as before, `BOX_LOCK_HELD=1`;
* `mkdir` fails **and** `$WP_WINDOW_TAG` matches the `tag=` line in the lock's `holder` file →
  **inherit**: proceed without acquiring, `BOX_LOCK_HELD` stays `0`;
* anything else → **abort 3**, exactly as before.

Because cleanup releases only when `BOX_LOCK_HELD=1`, a driver **never releases a lock it
inherited**. Release belongs to the gate's `--release`, which also reloads the serving model.
An untagged lock is never inherited.

**The non-gated path is behavior-identical.** With no `WP_WINDOW_TAG` in the environment the
new `elif` is false, so the driver acquires or aborts exactly as it did before — the offline
suite asserts this directly against the driver's real lines.

**No sibling driver needed the edit.** `run-official-window.sh`, `run-manual-test.sh` and
`run-variant-window.sh` take only the fd-scoped flock via `parity_take_gpu_lock`; none takes
`BOX_LOCK`, so none has an abort-if-lock-exists check to teach. The suite asserts that this
stays true.

### Reap the provably dead, refuse the ambiguous

A stale lock is reaped **only** when all three hold, and the evidence for each is sealed into
the attestation *before* anything is removed:

| condition | how it is established |
|---|---|
| holder is **verifiable** | the `pid` file exists and is a plain integer |
| holder is **provably dead** | `ps -p <pid>` returns no process — never `kill -0`, which returns `EPERM` for a live cross-uid holder |
| lock is **old enough** | age ≥ `WP_LOCK_REAP_AGE_S`, an explicit threshold argument |

Everything else refuses and reports, with a machine-readable reason:

| case | reason | outcome |
|---|---|---|
| pid is running | `holder-alive` | refuse (3) |
| no pid file, or non-numeric | `unverifiable-holder` | refuse (3) |
| mtime unreadable | `unverifiable-age` | refuse (3) |
| dead but younger than the threshold | `too-fresh` | refuse (3) — a holder that died seconds ago may be mid-restart |

This matches the upstream measure-job contract's auto-reap while
keeping the no-unprompted-cleanup spirit: **nothing is removed without a recorded proof of
death.** A lock that vanished with no record would be the same unaccountable box-state change
the refuse-and-report policy exists to prevent.

**The reap is atomic with respect to acquisition**, which is not a detail — the first
implementation was not, and it broke the one guarantee this lane exists to provide. It read the
lock's state, decided "reapable", then removed it *before* the `mkdir`; two probes deciding from
the same pre-acquisition snapshot both proceeded, and the loser deleted the files of a lock the
winner had legitimately created in between. Multiple simultaneous holders, each sealing a
`verified_dead_how` for a lock it had taken from a **live** peer.

The shape that holds:

1. **Plain `mkdir` first.** With no stale lock, no reap code runs at all.
2. **On contention only**, contend for a separate **reap mutex** (`<lock>.reapmutex`, `mkdir`,
   atomic). At most one process may reap at a time; everyone else reports and stops.
3. **Re-read the lock's state under the mutex.** The pre-mutex snapshot is precisely the stale
   information that caused the bug. A peer that acquired in the meantime is now visible twice
   over: its pid is alive, *and* its directory is seconds old.
4. **Reap by `mv`, not `rm`.** Rename is atomic, preserves the reaped lock as inspectable
   evidence (`lock.reaped.moved_to`), and cannot clobber files a peer wrote afterwards.
5. **Acquisition remains, and only ever is, the `mkdir`.** Reaping never confers ownership; if a
   peer wins the mkdir in the gap after our rename, we report not-acquired and it holds.

Exactly one winner falls out of `mkdir`'s atomicity, which is the one guarantee worth resting
on. The concurrency suite asserts it directly — 3-way and 20-way races against a reapable lock,
25 trials each, exactly one `lock.acquired=1` and at most one `reaped=1`.

The attestation's `lock.reaped` block carries the prior holder's tag, pid, user and
`acquired_utc`, the lock's age, *how* death was verified, and the reap timestamp.
`lock.reap_refused` carries the reason when a reap was declined.

---

### The handoff must be adopted

The `pid` in the lock means one thing: **the process whose liveness vouches for this lock.**

During the gate that is the box-side probe. But the probe exits when the gate returns — by
design, since a normal exit *is* the handoff — so from that moment the lock names a dead process.
Left alone, the reap predicate reads *verifiable* + *provably-dead* + *old-enough*, and the
protection a window enjoys **decreases the longer it runs**: any window outlasting
`WP_LOCK_REAP_AGE_S` can have its lock reaped by a second gate, which will unload the serving
model underneath a live run and seal a `verified_dead_how` that is a forgery.

So the handoff transfers the duty, and **the driver signs for it**: on the inheritance branch
`run-paired-window.sh` writes its own pid into the lock, appends `adopted_pid`/`adopted_utc` to
the holder record, and refreshes the directory mtime. For the window's true duration the holder
is alive, and the reaper refuses on the strongest axis it has.

Two consequences, both deliberate:

* **Driver crashes** → its pid dies → the lock becomes reapable again. This is why adoption is
  preferable to a never-reapable handoff marker: a marker that outlives its process turns every
  abandoned window into a box nobody can use without SSHing in to clear it by hand.
* **Handoff with no driver** (the gate passes, nothing ever adopts) → the lock keeps the exited
  probe's pid and *is* reapable once aged out. **Ruled deliberate:** nothing is alive to protect,
  so an abandoned handoff should self-heal. A window that intends to hold the box must run a
  driver that adopts — which is what the paired driver now does.

Adoption is about who **vouches**, not who releases: `BOX_LOCK_HELD` stays `0`, so the driver
still never releases an inherited lock. That remains `--release`'s job, because releasing also
reloads the serving model.

---

### Residual: nothing refreshes the lock's mtime during a window

The driver refreshes the mtime once, at adoption. Nothing touches it again, so a long window's
lock ages past `WP_LOCK_REAP_AGE_S` while the window is still running.

That is safe **only because the liveness check precedes the age check** — a live holder is
refused on `holder-alive` before age is ever consulted, so age becomes irrelevant while a process
is vouching for the lock. The ordering is therefore load-bearing, not stylistic: **if those two
checks are ever reordered, or age is promoted into a short-circuit, X-1 comes straight back** —
a live window's lock becomes reapable again purely by getting old.

The probe's reap ladder is written in that order deliberately and carries a comment saying so.
Heartbeating the mtime was considered and rejected: it adds a writer to a path every prober
reads, to defend a property the ordering already guarantees.

---

### The reap mutex can itself go stale

Reaping happens under a separate `.reapmutex`, so only one prober ever considers a reap. A trap
releases it on `HUP`/`INT`/`TERM` — but **not** on `SIGKILL`, and a stranded mutex is quietly
catastrophic: every later probe stands down with `reap-in-progress`, so one hard-killed run
disables reaping on that box permanently.

So the mutex records its holder's pid and is reclaimed only on evidence: the holder is **provably
gone** *and* the mutex is older than `WP_REAP_MUTEX_STALE_S` (default 120s — orders of magnitude
longer than a real reap, which is a stat, a `ps` and a rename). A live reaper is never disturbed,
and the reclamation is recorded in `lock.reap_mutex_reclaimed`.

---


## Goldens and pool tapes are different kinds

Per the binding terminology directive, **"golden" is reserved for an artifact carrying the
weights-hash + prompts + prompt-SHAs binding.** The R2 track-pool objects are **tapes**:
contract-pinned timed comparison inputs. The gate keeps two pin families for them, and the
distinction is load-bearing rather than cosmetic:

| | `WP_GOLDEN_*` | `WP_POOL_TAPE_*` |
|---|---|---|
| what it is | carries the weights-hash + prompts + prompt-SHAs binding | an R2 track-pool timed-prompt tape the **contract** pins |
| pin | `sha256` + `bytes` | `sha256` + `bytes` (identical shape) |
| shape check | handed to `benchctl validate-golden` — the same loader the run uses | required-key **signature** (`seed_tokens`, `reference_seed_token`, `rows`) — what `measure-job` actually routes on |

`validate-golden` **rejects every tape** by construction (`unknown field \`seed_tokens\``). Filing
tapes under `WP_GOLDEN_*` would therefore have meant one of two bad outcomes: a false loader
failure on every tape, or dropping the shape check altogether and pinning only bytes. Two
families gets both kinds a real check and makes the attestation and diagnostics say which kind
was rejected.

At least one comparison input — golden or tape — is required. A measure-job window normally
pins tapes, and the `decode` smoke recipe times the first tape when one is pinned.

Note that benchd's CLI spells both as `--golden`, routing by required-key signature. That is the
existing wire contract and this gate does not change it; the naming discipline is in the pins,
the attestation, and the diagnostics, where a human reads them.

### A gates-phase golden MUST carry the `.benchmark` oracle

A `benchmark`/`official` (ranked, **gates-phase**) window is TIMED against the golden's
`.benchmark` oracle, so any golden routed to the gates phase **must carry one**. Goldens generated
for correctness / local-iterate legitimately do **not** carry it (correctness is oracle-optional) —
so the m3-regen public goldens are correct for their real consumers (loader strict-load,
correctness, local-iterate), but need the oracle **attached first** before they feed a ranked run.
Attach it with the engine's weightless **`attach-benchmark-oracle`** remedy.

benchd fails closed on this pre-GPU: a benchmark-less **golden** (the legacy `GoldenDocument` shape)
routed to the gates phase is refused before the GPU window, with a message that names the
`attach-benchmark-oracle` remedy (`crates/benchctl/src/measure_job.rs`,
`validate_gates_goldens_carry_oracle`). A pool **tape** carries its reference rows directly and is
exempt — it needs no `.benchmark` oracle.

### The correctness golden's pin authority is the fixture (LANE 2a)

The **hidden correctness golden** — the serial trajectory the trusted parent re-checks every
emitted token against — is identified by the track contract's `hidden_correctness_golden`
`sha256`+`bytes` pin, a **sibling of `timed_prompt_pool`** (never a ninth pool entry, so it never
perturbs the anti-lottery pool count). The gate **sources that pin from the sha256-verified
`--contract`**, not from an operator `WP_GOLDEN_*` line, and **pin-verifies the staged correctness
golden against it**. A staged golden whose bytes do not cite the fixture pin is refused (exit 4)
even when the operator's own `WP_GOLDEN_*` pin matches — machine-state is never the authority.

benchd re-enforces the identical pin on the run itself: `measure-job --correctness-golden <PATH>`
is the run's **attestation**, and benchd hashes those bytes and refuses (die-8, pre-GPU) any run
whose identity does not cite the fixture pin — **fail-closed both ways** (a fixture that pins the
golden requires the attestation; an attestation against a fixture that pins none is refused). The
golden's **name appears nowhere**; the pin (sha256+bytes) is the only identity.

`WP_GOLDEN_*` is unchanged and still carries its loader/signature coverage — the fixture authority
is layered on top of it, not a replacement.

---

## The bundle rule

The box has no GitHub credentials, so shipping a tree as a **git bundle is a legal path** — the
one Proof A actually used. It stays legal only under one condition:

> A bundle-shipped tree is accepted **only** when the bundle's own content hash is pinned in
> advance and recorded in the tree. Otherwise it is an unprovenanced tree wearing a commit sha,
> and the gate **refuses** it (exit 7).

Mechanically:

* `scripts/window-provision.sh` verifies the bundle's `sha256` against
  `WP_<ROLE>_BUNDLE_SHA256` **on the laptop before shipping** and **again on the box after it
  lands** — the wire is part of the trust chain.
* It records `bundle_sha256`, `bundle_path`, `commit`, `provisioned_utc` into
  `<git-dir>/window-bundle-provenance`. This lives in the **git dir, not the working tree**: an
  untracked file in the worktree would fail the clean-tree assertion, so a provenance record
  written there would defeat the provenance check it exists to support.
* The gate reads that record back and requires it to match the pin, **and** requires the
  record's `commit=` to equal the checkout's actual HEAD — a marker naming a different commit
  is incoherent whatever its hash says.
* **The marker is unauthenticated provenance text**: a writable file our own tooling wrote,
  which anyone with box access can edit. It is a *record*, not a signature. When the bundle
  itself is still on the box the gate re-derives the claim from its bytes (re-digest, plus
  `git bundle list-heads` must carry the pinned commit) and grades the row **PASS**; when the
  bundle is gone the row is graded **CLAIMED** — recorded, visibly weaker than PASS, and never
  dressed up as a verification that was not performed.
* A tree with **no origin remote and no bundle record** is refused outright — its provenance
  cannot be claimed from evidence at all.

---

## Provisioning

`scripts/window-provision.sh --pins <FILE>` syncs the box to the pins, so the gate has
something to pass. It is a **separate motion**: the gate never assumes provisioning ran, and
provisioning never reports a verdict.

Three rules, in order of importance:

1. **Never switch a checkout someone else owns.** If the pinned path exists at the wrong
   commit, provisioning **refuses** (exit 7) and says so. No `checkout`, no `reset`, no `pull`.
   The Proof A precedent is the pattern: build in a separate worktree
   (`~/wt-proofA-bench`, `~/wt-proofA-engine`) and leave the box's own checkout alone.
2. **Never delete anything.** No `rm`, no `worktree remove`, no `worktree prune`, no `clean`.
   If something is in the way, that is a fact for a human, not a thing to tidy — and that holds
   even when the obstruction looks obviously stale.
3. **A bundle is only legal with a pinned hash** (above).

Provisioning paths, in preference order: a detached `git worktree add` off an existing box
clone (`WP_<ROLE>_SOURCE_CLONE`), else a pinned bundle (`WP_<ROLE>_BUNDLE`). Build commands are
pinned per role (`WP_<ROLE>_BUILD_CMD`), with the literal `none` declaring "no build".

`--dry-run` prints every action without taking any.

---

## The attestation — `window-provenance.json`

Written to `WP_OUT` and copied to `WP_BOX_OUT`, **next to the run artifacts**. Provenance is
claimed *from evidence*, not assumption: every pin appears with what was expected, what was
actually observed, and a verdict.

The attestation is written **after** phases 1–3 complete, which is after the lock is acquired —
the phrase "before the lock" describes when the *assertions* run, not when the file lands. On a
run that fails while holding the lock, the gate unwinds **before** writing, so the sealed
`lock.state` and `released_utc` describe what actually happened rather than the moment of
acquisition.

This is a **separate sealed file**. It does not touch `benchmark-integrity.*.json` or the #123
runner-identity roster: that sidecar answers *what binary ran*; this answers *what tree ran*,
and the two are pinned independently on purpose.

```jsonc
{
  "schema": "window-provenance/v1",
  "verdict": "PASS",                  // PASS | FAIL
  "failed_items": 0,
  "gate": {                           // digests of the scripts that produced this record
    "script": "scripts/window-preflight.sh",
    "script_sha256": "<64-hex>",
    "probe": "scripts/window-probe.sh",   "probe_sha256": "<64-hex>",
    "provision": "scripts/window-provision.sh", "provision_sha256": "<64-hex>",
    "pins_file": "<path>", "pins_file_sha256": "<64-hex>",
    "driver": "ssh", "laptop_timestamp_utc": "..."
  },
  "box": {                            // the BOX's clock, not the laptop's, so a record
    "alias": "ai-server",             // cannot be silently re-dated by re-running elsewhere
    "timestamp_utc": "2026-08-21T...Z",
    "uname": "...", "user": "..."
  },
  "phases": { "pins": "PASS", "basics": "PASS", "smoke": "PASS" },
  "lock": {                           // single-flight PROVED, not asserted
    "dialect": "mkdir-session-lock",
    "path": "/tmp/mtplx-box-exclusive.lock.d",   // THE box lock — drivers inherit it by tag
    "window_tag": "proofA-retry-2026-08-21",
    "state": "held",                  // held | not-taken | released-by-probe
    "acquired_utc": "2026-08-21T...Z",
    "holder": "tag=...\npid=...\nuser=...\nacquired_utc=...",
    "reap_age_threshold_s": "900",
    "reaped": {                       // null unless a provably-dead holder was reaped
      "moved_to": "<lock>.reaped.<pid>.<epoch>",  // renamed aside, never deleted
      "prior_holder": "tag=...\npid=...\nuser=...\nacquired_utc=...",
      "prior_tag": "...", "prior_pid": "...", "prior_user": "...",
      "prior_acquired_utc": "...", "age_seconds": "...",
      "verified_dead_how": "ps -p 41234 returned no process; lock age 4210s >= threshold 900s",
      "reaped_utc": "2026-08-21T...Z"
    },
    "reap_refused": null,             // else {reason, detail} — holder-alive | unverifiable-holder
                                      //      | unverifiable-age | too-fresh
    "released_utc": null              // filled by window-release.json (see below)
  },
  "qwen": { "unloaded": "1", "unload_rc": "0" },
  "smoke": {
    "recipe": "both", "verdict": "PASS", "argv": "<the exact command>",
    "exit_code": "0", "started_utc": "...",
    "benchd_stderr": "...", "benchd_stdout": "...", "worker_stderr": "..."
  },
  "items": [                          // one row per check — nothing asserted without being
    { "phase": "pins",                //   recorded, nothing recorded without being judged
      "id": "ENGINE.head",
      "expected": "3a579b30...",
      "observed": "3a579b30...",
      "verdict": "PASS",              // PASS | FAIL | REFUSED | NOTE
      "diagnostic": "" }
  ],
  "lock_taken": true
}
```

`--release` writes `window-release.json` alongside it:

```jsonc
{
  "schema": "window-release/v1",
  "verdict": "released",              // released | not-held | not-ours | release-failed
  "window_tag": "proofA-retry-2026-08-21",
  "session_lock": "/tmp/mtplx-window-session.lock.d",
  "held_tag": "proofA-retry-2026-08-21",   // ownership proof: a release whose tag does not
  "released_utc": "2026-08-21T...Z",       //   match REFUSES and leaves the lock alone
  "box_timestamp_utc": "...",
  "qwen": { "reload_rc": "0", "reloaded": "1", "health": "..." }
}
```

A third file, `window-unwind.json`, is written on **every** trap path — gate failure, `INT`,
`TERM`, `HUP`:

```jsonc
{
  "schema": "window-unwind/v1",
  "trigger": "gate-failure-or-signal",
  "release_verdict": "released",     // released | not-held | not-ours | transport-error
  "window_tag": "...", "box_lock": "...",
  "resulting_state": "released-on-failure",
  "released_utc": "...", "qwen_reloaded": "1",
  "unwound_at_utc": "..."
}
```

Together these prove single-flight: who held the box, from when, until when — **including** the
runs that ended badly, which is exactly when the claim matters.

---

## Exit codes

Aligned with the window drivers' de-facto convention (`run-official-window.sh`,
`run-paired-window.sh`).

| code | meaning |
|---|---|
| 0 | PASS — the window may proceed; the lock is **held** |
| 1 | a pinned assertion mismatched (SHA, digest, byte count, dirty tree) |
| 2 | usage, or a required pin was not supplied |
| 3 | box unavailable — a lock is held, or the box is not quiet |
| 4 | a pinned comparison input was rejected — a golden (pin or loader), or a pool tape (pin or required-key signature) |
| 5 | a required tree, binary or prerequisite is missing |
| 6 | serving-model state is not what the window expects — including `--release` finding the **pinned** qwen service file absent or defining no `qwen_reload`. That is a **broken pin**, not the declared `none` waiver, and it fails closed: the box would otherwise be handed on free but not serving, with an exit-0 "OK". |
| 7 | **REFUSED** — a bundle-shipped tree whose bundle hash is not pinned, or a bundle whose own bytes contradict its provenance marker |
| 8 | **SMOKE LEG FAILED** — spawn/handshake/decode broke |
| 9 | transport error — the probe could not be run |

---

## Design notes

**What the gate's self-digests do and do not prove.** `gate.script_sha256` and friends are
computed *by the running scripts, over themselves*. A tampered gate seals its own tampered
digest and the record is internally consistent — self-attestation is not tamper-evidence. What
these digests are good for is **drift**: comparing them against the committed
`scripts/window-preflight.sh` shows whether the gate that ran is the gate in the repo. Treat
them as an integrity check against the committed tree, not as proof of honesty. The same
caveat, in stronger form, applies to the bundle marker — see the `CLAIMED` grading above.

**TOCTOU boundary.** Phases 1–2 observe the trees; phase 3 takes the lock. Nothing re-checks
the trees, weights or goldens *after* acquisition, so a change made in that interval is not
caught. The window is small (one probe round trip) and the box is single-flighted from
acquisition onward, but the guarantee is precisely "these were the pins at observation time,
and the box has been locked since acquisition" — not "these were the pins for the whole run".

**The probe observes; the laptop judges.** `scripts/window-probe.sh` runs on the box and emits
raw facts as `key=<base64>` lines. Every comparison against a pin happens on the laptop. Two
consequences: the assertion logic is testable without a box, and the box is never told what the
right answer is, so it cannot launder a mismatch into a pass.

**Why `key=<base64>` and not JSON.** The probe must run on a box that may lack `jq`, carrying
values (porcelain output, worker stderr, `uname -a`) with newlines and arbitrary bytes.
base64 makes the transport total.

**Why the flock is judged by `lsof`.** The gpu-exclusive flock cannot be probed by *taking* it
— that is acquisition, and the fd would die with the probe's shell anyway. But a flock holder
must hold the file OPEN, so open-fd inspection is sound in the direction that matters: any
process listing the lock file as open is evidence of a holder. It is deliberately conservative
— an open fd without an actual flock reads as held and refuses, the fail-closed direction — and
an `lsof`-less box yields `UNDETERMINED`, which also refuses. Mere *existence* of the lock file
is never judged; it survives every window ever run.

**Why the mkdir lock dialect.** A `flock` fd dies with the shell that opened it, so it cannot be
held across ssh sessions. A lock **directory** persists until removed — which is why the box
lock is this dialect, and why it is the one the gate can hold from preflight through the window.
The flock (`/tmp/mtplx-gpu-exclusive.lock`) is **observed and required free** — by open-fd
inspection, see above — but never taken by the gate; the driver takes it itself, inside its own
shell, where an fd lock actually works.

**Reap the provably dead, refuse the ambiguous** — see the dedicated section above.

**No pin has a default.** A missing pin is exit 2, naming the pin. Where a check does not
apply, it is *declared* (`none`, `0`) and the waiver is recorded — never omitted.

**The weights digest is a port, and it is pinned to the Rust vector.** The shell `dir_digest`
in the probe reproduces `crates/benchctl/src/iterate.rs:180-212` exactly, including the
exact-relative-path ignore rule (a *nested* `.gitkeep` is **not** ignored). The offline suite
asserts it against the same hand-computed vector the Rust unit test uses
(`iterate.rs:2228-2265`), because a digest that disagreed with benchd's would clear a tree the
run rejects — or, far worse, the reverse.

---

## Testing

`scripts/test-window-preflight-offline.sh` — no box, no GPU, no network. Builds a fake box
tree, drives the gate with `DRIVER=local`, and exercises the list below.

**Run one at a time.** The suite asserts over machine-**global** state — `pgrep` for stray engine
and serving-model processes, box-side temp paths — so two concurrent copies see each other's
fixtures and both report the other's processes as failures. This is enforced, not merely
requested: an atomic `mkdir` singleton at startup refuses a second run and says why. (It is not a
hypothetical; concurrent runs corrupted two separate review batteries in a single day.)

The suite also detects SIGHUP arriving as `SIG_IGN` (what `nohup` does, inherited by every child)
and fails with that named cause, because a shell cannot trap a signal ignored at exec time and the
N-4 signal-trap case would otherwise fail looking exactly like a handler regression. This is not
theoretical: a reviewer launched their verification battery under `nohup`, their N-4 cases were
poisoned by it, and the detector named the cause directly.

**Build `benchctl` first** (`cargo build --release -p benchctl`). The real-transport sections need
it, a SKIP fails this suite by design, and the check is made up front rather than 20 minutes in.

Exercises:

* the `dir_digest` port against the Rust vector, and the exact-rel ignore rule;
* all-match → PASS, and a well-formed attestation;
* every single-pin mismatch → the right exit code and a **named** diagnostic (one pin broken at
  a time, so no test can ride on another's failure);
* dirty tree, absent repo, absent binary, non-executable binary, identity mismatch;
* the bundle rule in all four states (no provenance / unpinned / wrong hash / pinned+recorded);
* the reap matrix: provably-dead+old → **reaped and sealed**; live pid → refused;
  unverifiable holder → refused; dead-but-fresh → refused (and each refusal attested);
* **concurrency**: 3-way and 20-way races against a reapable lock, 25 trials each, asserting
  exactly one `lock.acquired=1` and at most one `reaped=1` — a class no single-process case can
  see;
* **the box-side signal trap**: `SIGHUP` mid-window reloads the serving model *and then*
  releases, so a dropped connection cannot leave the box unlocked-and-not-serving;
* **provisioning a bundle over the ssh driver**, which is the only path that exercises the
  box-side `mktemp` (every other bundle case runs `--driver local` and bypasses it), plus
  `--dry-run` over ssh digesting nothing it did not ship;
* disk floor, box quiet, serving state;
* the **smoke leg against fake workers driven by the real `benchctl` over the real transport** —
  healthy, dies-at-hello, garbage-hello, hangs;
* the full lock lifecycle: acquire, hold, refuse a second session, refuse a foreign-tag
  release, release, no-op release, and trap-unwind on failure;
* `window-provision`: refuses to switch a checkout it does not own, refuses an unpinned bundle,
  accepts a pinned one, and deletes nothing;
* **driver holder-tag inheritance**, evaluated against the *real* lock block extracted from
  `run-paired-window.sh` rather than a copy: standalone acquires, standalone-vs-existing-lock
  still aborts 3, matching tag inherits without acquiring, foreign tag aborts, untagged lock is
  never inherited, and an inherited lock is never released;
* **the ssh driver path**, via a stub `ssh` that runs `bash -s` locally: the piped probe, the
  packed argv, the quote-safe attestation delivery (including a box path containing quotes and
  `$dollar`), and the release round trip;
* **red-team regressions**, one per defect that shipped broken — the present-but-empty request
  key, a real bundle-clone tree, `assume-unchanged`, a live holder of the real box lock, a
  lock-create failure, a release with the model down, ssh argv flattening, hostname leakage,
  unsealed `--pin` overrides, the unwind record, waived-smoke-still-acquires, and the two
  `dir_digest` divergences.

**A SKIP fails the suite.** An unbuilt `benchctl` used to substitute an always-pass stub, so the
entire real-transport section could go green without exercising the transport. A skipped check
is an untested claim, not a pass.

The fake-worker matrix is how the #134 verdicts are proven without a GPU. Against a real box
the `handshake` recipe is *expected to fail* until #134's fix lands — that is the gate working.

---

## Running it

```bash
# 0. author ./my-window.pins with every value filled in, no defaults —
#    the required keys are the ones window-provision.sh / window-preflight.sh
#    read from --pins (they refuse a pins file that is missing any of them)

# 1. sync the box to the pins (refuses to touch anything it does not own)
scripts/window-provision.sh --pins ./my-window.pins --box ai-server

# 2. the gate. On success it exits HOLDING the lock, with qwen unloaded.
scripts/window-preflight.sh --pins ./my-window.pins --box ai-server

# 3. run the window inside the held lock

# 4. ALWAYS. Reloads the serving model, verifies it, releases the lock.
#    This is ALSO the recovery command for a stranded lock: if a gate or window died
#    mid-flight, run it to reload the model and release. It refuses if the lock's holder tag
#    is not yours, so it is safe to run speculatively.
scripts/window-preflight.sh --pins ./my-window.pins --box ai-server --release
```

`--release` exits non-zero if the serving model did not come back, and says so: the lock coming
off is not the whole job, and a box handed over quietly not-serving is a failure, not a success.

`--provision` runs step 1 first. `--no-smoke` declares a smoke waiver (and takes no lock).

