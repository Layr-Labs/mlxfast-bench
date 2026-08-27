# Pinned Swift enforcer — parity reference (read-only)

These two files are a **byte-for-byte pinned copy** of the engine's Swift editable-surface byte-budget
enforcer, vendored here so `tests/byte_budget_parity.rs` is hermetic (no dependency on a sibling repo
checkout). They are the *reference* the native Rust port in `crates/benchctl/src/byte_budget.rs` is
pinned against per David's WIRE-1 ruling ("a PARITY TEST pinning the two implementations against
SHARED FIXTURES").

Origin (read-only), engine `Layr-Labs/mlxfast-gemma4-26b-a4b-engine` (the commit below is from its
development history, so identity here is carried by the sha256 column, not by the repo name):

| file | origin path | commit | sha256 (vendored == git object) |
|---|---|---|---|
| `EditableSurfaceByteBudget.swift` | `Sources/MLXFastTrustedHarness/EditableSurfaceByteBudget.swift` | `feb2b092e76776acfe3ff27a3b649d96848fae2a` | `73e7b51bae57183508489803379bc16ff7558da11f45f89a206ad52f78cb8955` |
| `main.swift` | `tools/editable-surface-budget-cli/main.swift` | `feb2b092e76776acfe3ff27a3b649d96848fae2a` | `a24110fc55dd9fa7c373dea9765ec94f1cd14e1cb0a7c8372021cee8b1516009` |

RE-PINNED 2026-08-26 (David BYO-512 ruling). The previous pin was
the qwen-era engine fork at `736781ea`, a QWEN-ERA copy that had drifted from the live
enforcer: it predated the D8 absent-surface backstop (issue #20 Q3) and carried the older cap
defaults. Because the Rust port's own constants had been matched to that stale copy, the parity test
was green while benchd was in fact LOOSER than the engine on an all-absent editable surface — a
fail-open hole in the FINAL validator. Re-pinning to the live enforcer surfaced it; the port now
carries the backstop and the current defaults. The lesson the old note below gets wrong is recorded
here deliberately: pinning the oracle to a copy that no longer tracks the reference does not freeze
the contract, it hides drift.

The engine re-implements `Layr-Labs/qwen-3.8-mtp-challenge@bfab0de`; see the source headers.

Reproduce:

```bash
git -C <engine-checkout> show feb2b092:Sources/MLXFastTrustedHarness/EditableSurfaceByteBudget.swift
git -C <engine-checkout> show feb2b092:tools/editable-surface-budget-cli/main.swift
```

The parity test compiles these two files with `swiftc -O` into `editable-surface-budget` (the enforcer
imports Foundation only, ~1 s) and runs `verify` / `limits` over the shared fixtures. **Pinned on
purpose**: the copy does not float with the engine, so an engine change cannot silently redefine what
benchd is measured against. Update the copy + this table (and re-run the parity test) only as a
deliberate, reviewed re-pin — but DO re-pin when the engine enforcer changes semantics, because a pin
left behind stops being a parity oracle and starts masking divergence (see the 2026-08-26 note
above). No goldens, endpoints, or secret-tier material are present in these files.
