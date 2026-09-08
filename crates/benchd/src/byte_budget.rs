//! WIRE-1 item 1a — the AUTHORITATIVE editable-surface BYTE BUDGET, a NATIVE RUST port homed in
//! benchd's measure-job.
//!
//! David's RULING (WIRE-1): benchd's measure-job is the AUTHORITATIVE gate for the editable-surface
//! byte budget, and it executes NO engine-repo code to enforce it — the trust boundary runs this
//! Rust port, never the frozen Swift CLI. The engine's Swift enforcer either retires or stays a
//! DEV-TIME pre-check; the single-source-of-truth concern is managed by a PARITY TEST
//! (`tests/byte_budget_parity.rs`) that pins THIS implementation against the Swift enforcer over
//! SHARED FIXTURES and asserts identical accept/reject + identical numbers on each. The #16
//! "no second implementation" design rule is SUPERSEDED by that ruling.
//!
//! REFERENCE SEMANTICS (ported, not copied). The reference is the engine's Swift enforcer
//! `Sources/MLXFastTrustedHarness/EditableSurfaceByteBudget.swift`, read at
//! the qwen-era engine fork of `Layr-Labs/qwen-3.8-mtp-challenge`, at `736781ea`
//! (read-only). Every rule below carries the reference rule id from
//! `docs/submission-restriction-spec.md@736781ea` §2 (R1.x). A pinned byte-for-byte copy of the
//! reference lives under `tests/parity_fixtures/swift/` so the parity test is hermetic.
//!
//! WHAT IS PORTED vs what is benchd-native:
//!   * `resolve_limits*` / `verify_byte_budget*` — a faithful port of the Swift resolution + walk
//!     (R1.1-R1.14), including the fail-closed decode divergences D1/D2 the Swift enforcer added
//!     over the original challenger (an absent cap takes the pinned default; a PRESENT-but-malformed
//!     cap — `null`, a string, a float, a non-positive int — fails CLOSED). These are what the
//!     parity test pins.
//!   * `verify_growth*` — the `maxGrowthBytes` bound (R1's growth cap). The Swift LAUNCH-time
//!     enforcer resolves this cap but does NOT consume it: there is no review base at launch. benchd
//!     HAS the review base (the `--baseline` trusted ref) alongside the `--candidate`, so it can and
//!     does enforce growth = candidate-code-bytes − baseline-code-bytes ≤ maxGrowthBytes. This is a
//!     benchd-native extension with no Swift counterpart, so it is NOT part of the parity test; it
//!     carries its own revert-proof.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use crate::trusted_scope;

/// R1.1 — the pinned default total editable-surface cap (bytes). `EditableSurfaceByteBudget.swift`
/// `defaultMaxTotalBytes` (spec R1.1).
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 4_404_587;
/// R1.2 — the pinned default per-file cap (bytes). `defaultMaxFileBytes` (spec R1.2).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 524_288;
/// The pinned default growth cap (bytes) — bytes a submission may ADD versus its review base.
/// `defaultMaxGrowthBytes`.
pub const DEFAULT_MAX_GROWTH_BYTES: u64 = 262_144;
/// R1.11 — the pinned default AGGREGATE cap over the exempt editable paths. 512 MB decimal
/// (David BYO-512 ruling 2026-08-26). This no longer tracks the head manifest's `max_bytes`
/// (still 2 GiB): this cap bounds the bytes a submission ARCHIVES, while `max_bytes` bounds what
/// the runner LOADS, including organizer-pinned heads staged on-box that never enter a submission.
/// Yukon's expanded-archive cap is 512 MiB and `maxSubmissionBytes` can only lower it.
/// `defaultExemptPathMaxBytes`.
pub const DEFAULT_EXEMPT_PATH_MAX_BYTES: u64 = 512_000_000;
/// R1.11a — the pinned default PER-FILE cap for a submitted file under an exempt path. 100 MB
/// decimal. Promotion commits the head directories into the repository and GitHub refuses a single
/// blob over 100 MB, so a head shipped as one monolithic shard fit the aggregate and then failed
/// the push. `defaultExemptPathMaxFileBytes`.
pub const DEFAULT_EXEMPT_PATH_MAX_FILE_BYTES: u64 = 100_000_000;

/// The caps in force for one contract, resolved from its declaration or the pinned defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub max_growth_bytes: u64,
    pub exempt_path_max_bytes: u64,
    pub exempt_path_max_file_bytes: u64,
}

/// The outcome of resolving a contract's caps. Mirrors the Swift
/// `EditableSurfaceBudgetLimitsResolution` (and the CLI's `limits` exit map: resolved→0, missing→2,
/// invalid→1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitsResolution {
    Resolved(Limits),
    /// Nothing to read (no contract on disk). Official runs treat this as fatal upstream; here the
    /// caller decides. Only produced by the file-path (Swift-parity) entry point, which is itself
    /// test-only — the live path always has the trusted-baseline bytes in hand — so the variant is
    /// gated to tests to keep the bin build warning-clean under `-D warnings`.
    #[cfg(test)]
    MissingContract(String),
    /// A contract that exists but cannot be trusted to state its own caps (fail-closed).
    Invalid(String),
}

/// The outcome of walking a contract's editable surface against its caps. Mirrors the Swift
/// `EditableSurfaceBudgetVerification` (and the CLI's `verify` exit map: verified→0, skipped→2,
/// exceeded→1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetVerification {
    Verified {
        total_bytes: u64,
        file_count: u64,
    },
    /// Nothing to check (no contract on disk). Only produced by the Swift-parity file-path entry
    /// point, which is test-only — the live path always has the trusted-baseline bytes in hand — so
    /// the variant is gated to tests to keep the bin build warning-clean under `-D warnings`.
    #[cfg(test)]
    Skipped(String),
    Exceeded(String),
}

/// The subset of the contract this enforcer reads, decoded fail-closed. `serde_json::Value` (not a
/// `#[derive(Deserialize)]` struct) is used deliberately: it is the only way to reproduce the Swift
/// hand-written initializer's rule that a PRESENT-but-`null` cap fails CLOSED rather than taking the
/// default (divergence D2) — serde's `Option<T>` maps JSON `null` to `None` (= absent), which is the
/// exact hole the Swift enforcer closes.
struct Contract {
    /// `editablePaths` — decode-if-present semantics: absent or explicit `null` ⇒ `None`.
    editable_paths: Option<Vec<String>>,
    budget: Budget,
}

#[derive(Default)]
struct Budget {
    /// `editableSurfaceByteBudget.exemptPaths` — decode-if-present: absent/`null` ⇒ `None`.
    exempt_paths: Option<Vec<String>>,
    /// The four caps. `None` = key ABSENT (take the default). A present-but-malformed cap never
    /// reaches here — it is a load-time `Invalid`.
    max_total_bytes: Option<i64>,
    max_file_bytes: Option<i64>,
    max_growth_bytes: Option<i64>,
    exempt_path_max_bytes: Option<i64>,
    exempt_path_max_file_bytes: Option<i64>,
}

/// `decodeIfPresent([String])` — absent or explicit `null` ⇒ `Ok(None)`; a present non-array or a
/// present array with a non-string element fails closed.
fn opt_string_array(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    other => {
                        return Err(format!(
                            "{key} must be an array of strings; found element {other}"
                        ))
                    }
                }
            }
            Ok(Some(out))
        }
        Some(other) => Err(format!("{key} must be an array of strings; found {other}")),
    }
}

/// `container.contains(key) ? decode(Int) : nil` — absent ⇒ `Ok(None)`; PRESENT (explicit `null`,
/// a string, a float, a bool included) must decode as an integer or it fails closed (divergence D2).
/// This is the whole point of the manual decode: only a genuinely MISSING key may take the default.
fn present_int(obj: &serde_json::Map<String, Value>, key: &str) -> Result<Option<i64>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(v) => match v.as_i64() {
            Some(i) => Ok(Some(i)),
            None => Err(format!(
                "editableSurfaceByteBudget.{key} is present but not an integer ({v}); a malformed \
                 cap fails closed"
            )),
        },
    }
}

/// Every key `editableSurfaceByteBudget` is allowed to carry. Anything else is refused by
/// [`deny_unknown_budget_keys`].
const KNOWN_BUDGET_KEYS: &[&str] = &[
    "exemptPaths",
    "exemptPathMaxBytes",
    "exemptPathMaxFileBytes",
    "maxTotalBytes",
    "maxFileBytes",
    "maxGrowthBytes",
];

/// DENY UNKNOWN KEYS, scoped to the budget block (benchd-native fence, 2026-08-26; no Swift
/// counterpart, so it is not part of the parity contract).
///
/// Every key in this block is an ENFORCED VALUE. An unrecognised one is either a typo — in which
/// case the cap the author meant to set is silently not in force — or a key a NEWER engine
/// understands and this validator does not, in which case benchd would silently under-enforce a
/// bound the manifest claims. Both are the same failure: a budget that reads stricter than it is.
/// Refusing converts that class into a hard, named stop.
///
/// ACCEPTED CONSEQUENCE: this makes key deployment validator-first BY CONSTRUCTION. An
/// engine-first budget-key addition will make the pinned benchd refuse every run until a dist
/// re-cut ships a benchd that knows the key. That coupling is the point, not a bug.
///
/// SCOPED TO THIS BLOCK ONLY. The manifest legitimately carries yukon-facing and track-facing
/// fields at the top level that benchd neither reads nor should constrain, so the fence stops at
/// the budget object's edge.
fn deny_unknown_budget_keys(bobj: &serde_json::Map<String, Value>) -> Result<(), String> {
    // BTreeSet: a deterministic, sorted list so the refusal is byte-stable run to run.
    let unknown: BTreeSet<&str> = bobj
        .keys()
        .map(String::as_str)
        .filter(|k| !KNOWN_BUDGET_KEYS.contains(k))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(format!(
        "editableSurfaceByteBudget declares unknown key(s) [{}]; every key in this block is an \
         enforced value, so an unrecognised one is either a typo or a cap this validator does not \
         implement — both mean the budget is not what it reads like. Known keys: [{}]",
        unknown.into_iter().collect::<Vec<_>>().join(", "),
        KNOWN_BUDGET_KEYS.join(", ")
    ))
}

/// Load + decode a contract's bytes fail-closed. Malformed JSON, a non-object document, a present-
/// but-`null` `editableSurfaceByteBudget`, or a present-but-malformed cap are all `Err(reason)` —
/// the caller decides whether that maps to `Invalid` (resolution) or `Exceeded` (verification).
fn load_contract(bytes: &[u8]) -> Result<Contract, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("benchmark contract is not valid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "benchmark contract is not a JSON object".to_string())?;
    let editable_paths = opt_string_array(obj, "editablePaths")?;
    // `container.contains(.editableSurfaceByteBudget) ? decode(object) : nil` — a present `null` or
    // a present non-object is a malformed budget, not "declares no caps".
    let budget = match obj.get("editableSurfaceByteBudget") {
        None => Budget::default(),
        Some(Value::Object(bobj)) => {
            // The fence runs BEFORE any cap is read: an unknown key means the block does not mean
            // what it reads like, so there is nothing worth decoding out of it.
            deny_unknown_budget_keys(bobj)?;
            Budget {
                exempt_paths: opt_string_array(bobj, "exemptPaths")?,
                max_total_bytes: present_int(bobj, "maxTotalBytes")?,
                max_file_bytes: present_int(bobj, "maxFileBytes")?,
                max_growth_bytes: present_int(bobj, "maxGrowthBytes")?,
                exempt_path_max_bytes: present_int(bobj, "exemptPathMaxBytes")?,
                exempt_path_max_file_bytes: present_int(bobj, "exemptPathMaxFileBytes")?,
            }
        }
        Some(other) => {
            return Err(format!(
                "editableSurfaceByteBudget is present but not an object ({other}); a malformed \
                 budget fails closed"
            ))
        }
    };
    Ok(Contract {
        editable_paths,
        budget,
    })
}

/// Resolve one cap: absent ⇒ the pinned default; a declared value must be a positive integer or the
/// whole resolution fails closed (Swift `positiveCap`). A declaration may RAISE as well as lower the
/// default — Swift-exact, and safe here because the manifest is read from the TRUSTED `--baseline`
/// (see the call site in `execute_measure_job`), never a submission, so a candidate can not widen its
/// own budget. (#151 CAP-SOURCE: closed as moot by both reviewers — no lower-only clamp.)
fn positive_cap(declared: Option<i64>, name: &str, fallback: u64) -> Result<u64, LimitsResolution> {
    match declared {
        None => Ok(fallback),
        Some(v) if v > 0 => Ok(v as u64),
        Some(v) => Err(LimitsResolution::Invalid(format!(
            "editableSurfaceByteBudget.{name} is {v}; it must be a positive integer"
        ))),
    }
}

/// Resolve the caps a contract's DECODED form declares (Swift `resolveEditableSurfaceBudgetLimits`
/// minus the file read). Order matches the reference so the FIRST malformed cap named is the same.
fn resolve_limits_from_contract(contract: &Contract) -> LimitsResolution {
    let b = &contract.budget;
    let max_total_bytes =
        match positive_cap(b.max_total_bytes, "maxTotalBytes", DEFAULT_MAX_TOTAL_BYTES) {
            Ok(v) => v,
            Err(e) => return e,
        };
    let max_file_bytes =
        match positive_cap(b.max_file_bytes, "maxFileBytes", DEFAULT_MAX_FILE_BYTES) {
            Ok(v) => v,
            Err(e) => return e,
        };
    let max_growth_bytes = match positive_cap(
        b.max_growth_bytes,
        "maxGrowthBytes",
        DEFAULT_MAX_GROWTH_BYTES,
    ) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let exempt_path_max_bytes = match positive_cap(
        b.exempt_path_max_bytes,
        "exemptPathMaxBytes",
        DEFAULT_EXEMPT_PATH_MAX_BYTES,
    ) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let exempt_path_max_file_bytes = match positive_cap(
        b.exempt_path_max_file_bytes,
        "exemptPathMaxFileBytes",
        DEFAULT_EXEMPT_PATH_MAX_FILE_BYTES,
    ) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // R1: the per-file cap can never bind if it exceeds the total — refuse rather than resolve a
    // budget with a dead cap.
    if max_file_bytes > max_total_bytes {
        return LimitsResolution::Invalid(format!(
            "editableSurfaceByteBudget.maxFileBytes ({max_file_bytes}) exceeds maxTotalBytes \
             ({max_total_bytes}); the per-file cap can never bind"
        ));
    }
    // DELIBERATELY no `exempt_path_max_file_bytes <= exempt_path_max_bytes` guard (Swift-exact):
    // the exempt pair does not share a consistent default pair the way maxFileBytes/maxTotalBytes
    // does, so a contract that declares only the aggregate would be refused for a default it never
    // wrote. An unbindable per-file cap is harmless — the aggregate refuses first.
    LimitsResolution::Resolved(Limits {
        max_total_bytes,
        max_file_bytes,
        max_growth_bytes,
        exempt_path_max_bytes,
        exempt_path_max_file_bytes,
    })
}

/// The `editablePaths` this parser reads from a manifest's bytes (`None` bucket ⇒ empty), exposed so
/// the write-divergence gate can pin that THIS parser and `trusted_scope::EditableSurface::parse`
/// agree about the declared surface (#151 LOW). Test-only — the live path never needs it.
#[cfg(test)]
pub fn editable_paths_for_parity(bytes: &[u8]) -> Result<Vec<String>, String> {
    load_contract(bytes).map(|c| c.editable_paths.unwrap_or_default())
}

/// Resolve the caps a contract's bytes declare (no file read). `Invalid` on any malformed input;
/// never `MissingContract` (the bytes are already in hand). Exercised by the parity test + unit
/// tests; the live wiring resolves through [`resolve_limits_from_contract`] directly.
#[cfg(test)]
pub fn resolve_limits_from_bytes(bytes: &[u8]) -> LimitsResolution {
    match load_contract(bytes) {
        Ok(contract) => resolve_limits_from_contract(&contract),
        Err(reason) => LimitsResolution::Invalid(reason),
    }
}

/// Swift-PARITY entry: resolve the caps a contract FILE declares (Swift
/// `resolveEditableSurfaceBudgetLimits(contractPath:)`). Absent file ⇒ `MissingContract`. This is a
/// parity-test entry point (the live path reads the trusted-baseline bytes, never a path); gated to
/// tests so the bin build stays warning-clean under `-D warnings`.
#[cfg(test)]
pub fn resolve_limits_at(contract_path: &Path) -> LimitsResolution {
    match std::fs::read(contract_path) {
        Ok(bytes) => resolve_limits_from_bytes(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LimitsResolution::MissingContract(
            format!("no benchmark contract at {}", contract_path.display()),
        ),
        Err(e) => LimitsResolution::Invalid(format!(
            "benchmark contract at {} is not readable: {e}",
            contract_path.display()
        )),
    }
}

/// Whether a filesystem type is a REGULAR file. Symlinks and other non-regular entries never count
/// (spec R1.7): they are SKIPPED, not rejected.
fn is_regular(md: &std::fs::Metadata) -> bool {
    md.is_file()
}

/// The running accumulators of one surface walk.
struct Walk {
    total_bytes: u64,
    file_count: u64,
    exempt_bytes: u64,
}

impl Walk {
    fn new() -> Walk {
        Walk {
            total_bytes: 0,
            file_count: 0,
            exempt_bytes: 0,
        }
    }

    /// A NON-exempt regular file (spec R1.12/R1.13): the per-file cap is checked BEFORE the running
    /// total, so an oversized file names itself; the total is checked after each file, so its
    /// message says "at least N bytes".
    fn account(&mut self, path: &Path, size: u64, limits: &Limits) -> Option<BudgetVerification> {
        if size > limits.max_file_bytes {
            return Some(BudgetVerification::Exceeded(format!(
                "editable file {} is {size} bytes, above the per-file static review limit {}",
                path.display(),
                limits.max_file_bytes
            )));
        }
        self.total_bytes += size;
        self.file_count += 1;
        if self.total_bytes > limits.max_total_bytes {
            return Some(BudgetVerification::Exceeded(format!(
                "editable surface is at least {} bytes, above the static review limit {}",
                self.total_bytes, limits.max_total_bytes
            )));
        }
        None
    }

    /// An EXEMPT regular file (spec R1.10): held OUT of the code budget and charged to the exempt
    /// caps instead. Two bounds apply, per-file FIRST so an oversize blob names itself (R1.11a,
    /// matching R1.12's posture for the code budget). Exempt files are held out of `maxFileBytes`,
    /// so before R1.11a they had no per-file bound at all.
    fn account_exempt(
        &mut self,
        path: &str,
        file_path: &Path,
        size: u64,
        limits: &Limits,
    ) -> Option<BudgetVerification> {
        if size > limits.exempt_path_max_file_bytes {
            return Some(BudgetVerification::Exceeded(format!(
                "exempt editable file {} is {size} bytes, above the exempt per-file limit {}",
                file_path.display(),
                limits.exempt_path_max_file_bytes
            )));
        }
        self.exempt_bytes += size;
        self.file_count += 1;
        if self.exempt_bytes > limits.exempt_path_max_bytes {
            return Some(BudgetVerification::Exceeded(format!(
                "exempt editable path {path} is at least {} bytes, above the exempt-path limit {}",
                self.exempt_bytes, limits.exempt_path_max_bytes
            )));
        }
        None
    }
}

/// Recursively account every REGULAR file under `dir`, skipping symlinks and other non-regular
/// entries (spec R1.7). A directory that cannot be enumerated is `.exceeded` (spec R1.9), fail-
/// closed. Returns `Some(verdict)` to short-circuit, `None` to continue.
fn walk_dir(
    dir: &Path,
    exempt_label: Option<&str>,
    walk: &mut Walk,
    limits: &Limits,
) -> Option<BudgetVerification> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            return Some(BudgetVerification::Exceeded(format!(
                "could not enumerate editable path {}: {e}",
                dir.display()
            )))
        }
    };
    // Deterministic order so a `.exceeded` message is reproducible run-to-run.
    let mut names: Vec<std::fs::DirEntry> = match entries.collect::<Result<Vec<_>, _>>() {
        Ok(v) => v,
        Err(e) => {
            return Some(BudgetVerification::Exceeded(format!(
                "could not enumerate editable path {}: {e}",
                dir.display()
            )))
        }
    };
    names.sort_by_key(std::fs::DirEntry::file_name);
    for entry in names {
        let path = entry.path();
        // lstat: never follow a symlink (spec R1.7 — symlinks are skipped, not chased).
        let md = match std::fs::symlink_metadata(&path) {
            Ok(md) => md,
            Err(_) => continue,
        };
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            if let Some(v) = walk_dir(&path, exempt_label, walk, limits) {
                return Some(v);
            }
        } else if is_regular(&md) {
            let verdict = match exempt_label {
                Some(label) => walk.account_exempt(label, &path, md.len(), limits),
                None => walk.account(&path, md.len(), limits),
            };
            if verdict.is_some() {
                return verdict;
            }
        }
    }
    None
}

/// The shared walk: enforce `limits` over the contract's `editablePaths`, rooted at `surface_root`
/// (Swift `verifyEditableSurfaceByteBudget(contractPath:limits:)`, where its root is the contract's
/// own directory). In benchd the root is the CANDIDATE workspace while the contract (caps + paths)
/// comes from the trusted `--baseline`; when `surface_root` IS the contract's directory the walk is
/// byte-for-byte the Swift walk, which is what the parity test exercises.
fn walk_budget(contract: &Contract, limits: &Limits, surface_root: &Path) -> BudgetVerification {
    let editable_paths = match &contract.editable_paths {
        Some(paths) if !paths.is_empty() => paths,
        _ => {
            return BudgetVerification::Exceeded(
                "benchmark contract has no usable editablePaths".to_string(),
            )
        }
    };
    let exempt: BTreeSet<&str> = contract
        .budget
        .exempt_paths
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .collect();
    let mut walk = Walk::new();
    for editable_path in editable_paths {
        // R1.10: exemption keys on the exact editablePaths ENTRY string (Swift `Set.contains`).
        let exempt_label = if exempt.contains(editable_path.as_str()) {
            Some(editable_path.as_str())
        } else {
            None
        };
        let root = surface_root.join(editable_path);
        // R1.8: a missing editable path is skipped silently.
        let md = match std::fs::symlink_metadata(&root) {
            Ok(md) => md,
            Err(_) => continue,
        };
        // A symlinked editable ENTRY is a non-regular entry — skipped (spec R1.7).
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            if let Some(v) = walk_dir(&root, exempt_label, &mut walk, limits) {
                return v;
            }
        } else if is_regular(&md) {
            let verdict = match exempt_label {
                Some(label) => walk.account_exempt(label, &root, md.len(), limits),
                None => walk.account(&root, md.len(), limits),
            };
            if let Some(v) = verdict {
                return v;
            }
        }
    }
    // D8 / issue #20 Q3 — PARITY FIX (surfaced 2026-08-26 by re-pinning the parity oracle to the
    // LIVE engine enforcer; the previous oracle was a qwen-era copy that predated this guard).
    // A surface where every editable path is absent, or every present path is an empty directory,
    // walks to totalBytes=0 fileCount=0. Returning `Verified` there reads absence as a clean pass;
    // a real submission always carries at least one editable source file. The Swift reference and
    // the shell whole-surface gate both refuse it, and benchd is the FINAL validator, so a looser
    // benchd is a fail-open hole. Swift-exact.
    if walk.file_count == 0 {
        return BudgetVerification::Exceeded(format!(
            "no editable file found under any editablePath at {}; every editable path is absent \
             or empty (absence is a refusal, not a zero-byte pass)",
            surface_root.display()
        ));
    }
    BudgetVerification::Verified {
        total_bytes: walk.total_bytes,
        file_count: walk.file_count,
    }
}

/// benchd entry: verify a contract's bytes (caps + `editablePaths` from the trusted `--baseline`)
/// against the editable surface materialized under `surface_root` (the `--candidate` workspace). No
/// file read here — the bytes are already in hand — so this never returns `Skipped`.
pub fn verify_byte_budget_over(manifest_bytes: &[u8], surface_root: &Path) -> BudgetVerification {
    let (contract, limits) = match contract_and_limits(manifest_bytes) {
        Ok(pair) => pair,
        Err(verification) => return verification,
    };
    walk_budget(&contract, &limits, surface_root)
}

/// Swift-PARITY entry: verify a contract FILE, walking its `editablePaths` rooted at the contract's
/// OWN directory (Swift `verifyEditableSurfaceByteBudget(contractPath:)`). Absent file ⇒ `Skipped`.
/// Parity-test entry point (the live path walks the candidate workspace via
/// [`verify_byte_budget_over`]); gated to tests so the bin build stays warning-clean.
#[cfg(test)]
pub fn verify_byte_budget_at(contract_path: &Path) -> BudgetVerification {
    let bytes = match std::fs::read(contract_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return BudgetVerification::Skipped(format!(
                "no benchmark contract at {}",
                contract_path.display()
            ))
        }
        Err(e) => {
            return BudgetVerification::Exceeded(format!(
                "benchmark contract at {} is not readable: {e}",
                contract_path.display()
            ))
        }
    };
    let root = contract_path.parent().unwrap_or_else(|| Path::new("."));
    verify_byte_budget_over(&bytes, root)
}

// ---------------------------------------------------------------------------
// Growth (maxGrowthBytes) — benchd-native (no Swift counterpart at launch time).
// ---------------------------------------------------------------------------

/// The NON-exempt code bytes of the editable surface rooted at `surface_root` — the same accounting
/// as [`walk_budget`]'s `total_bytes`, but with NO caps, so the full sum is available for a growth
/// comparison regardless of whether it would trip a cap on its own.
fn code_bytes(contract: &Contract, surface_root: &Path) -> u64 {
    let editable_paths = match &contract.editable_paths {
        Some(paths) => paths,
        None => return 0,
    };
    let exempt: BTreeSet<&str> = contract
        .budget
        .exempt_paths
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .collect();
    let mut total = 0u64;
    for editable_path in editable_paths {
        if exempt.contains(editable_path.as_str()) {
            continue; // exempt paths are not CODE; they never count toward growth.
        }
        sum_regular_files(&surface_root.join(editable_path), &mut total);
    }
    total
}

/// Add the sizes of every regular file at or under `path` to `total`, skipping symlinks and non-
/// regular entries (spec R1.7). Best-effort: an unreadable entry contributes nothing.
fn sum_regular_files(path: &Path, total: &mut u64) {
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(_) => return,
    };
    if md.file_type().is_symlink() {
        return;
    }
    if md.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            sum_regular_files(&entry.path(), total);
        }
    } else if is_regular(&md) {
        *total += md.len();
    }
}

/// The NON-exempt code bytes of the editable surface AS OF one commit, read out of git's object
/// store rather than off disk. This is the baseline half of the RULED fork-point mode (David
/// 2026-08-27): the growth bound must measure the submission against ITS OWN fork point, not against
/// whatever snapshot happens to be staged on the box.
///
/// The accounting deliberately mirrors [`code_bytes`]/[`sum_regular_files`] entry for entry so the
/// two sides of the subtraction are commensurable: exempt `editablePaths` entries are skipped
/// (they are not CODE), symlinks are skipped (git spells one as mode `120000`, which is what
/// `sum_regular_files` skips on disk), and gitlinks/submodules are skipped (type `commit`, whose
/// `ls-tree -l` size field is a literal `-`, and which `sum_regular_files` never descends into).
///
/// Membership is the CASEFOLDED lexical prefix relation — equal, or under `entry/` — after the same
/// `normalize` treatment the divergence gate uses. The casefold mirrors the ranked box's APFS
/// behavior: on a case-insensitive volume the disk-side walk of `Sources/MLXFastModel` also picks up
/// a tree spelled `sources/mlxfastmodel`, so the git side must agree or the subtraction is between
/// two different surfaces. The device:inode arms of the divergence gate have no counterpart here —
/// a commit's tree has no inodes — and they are not needed: this is a SUM, not an authorization.
///
/// Every failure is an `Err`: the caller turns it into a fail-closed refusal.
fn code_bytes_at_commit(
    contract: &Contract,
    repo_root: &Path,
    base_sha: &str,
) -> Result<u64, String> {
    if !crate::editable_divergence::is_full_commit_sha(base_sha) {
        return Err(format!(
            "--write-gate-base {base_sha:?} is not a 40-character hex commit sha, so git cannot be \
             asked for the fork point's editable surface"
        ));
    }
    let editable_paths = match &contract.editable_paths {
        Some(paths) => paths,
        None => return Ok(0),
    };
    let exempt: BTreeSet<&str> = contract
        .budget
        .exempt_paths
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .collect();
    // The non-exempt surface, pre-normalized + lowercased once so the per-blob test is a plain
    // comparison. An entry that normalizes away (`""`, `"/"`, `"."`) grants nothing, exactly as it
    // grants nothing in the divergence gate.
    let surfaces: Vec<String> = editable_paths
        .iter()
        .filter(|entry| !exempt.contains(entry.as_str()))
        .map(|entry| trusted_scope::normalize(entry).to_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect();

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["ls-tree", "-r", "-l", "-z"])
        .arg(base_sha)
        .output()
        .map_err(|e| {
            format!(
                "--write-gate-base: cannot run git in {}: {e}",
                repo_root.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "--write-gate-base: git ls-tree {base_sha} failed in {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stream = String::from_utf8(output.stdout)
        .map_err(|e| format!("--write-gate-base: git ls-tree -z emitted non-UTF-8 output: {e}"))?;

    // `ls-tree -r -l -z` emits one NUL-terminated record per entry:
    //   <mode> SP <type> SP <oid> SP<pad> <size> TAB <path>
    // The size field is right-aligned with blank padding, so the metadata half is split on
    // whitespace. `-z` means the path is RAW (never C-quoted), so it compares directly.
    let mut total = 0u64;
    for record in stream.split('\0') {
        if record.is_empty() {
            continue; // the NUL-terminated tail (and an empty tree yields only that).
        }
        let (meta, path) = record.split_once('\t').ok_or_else(|| {
            format!("--write-gate-base: git ls-tree record has no path separator: {record:?}")
        })?;
        let mut parts = meta.split_whitespace();
        let (Some(mode), Some(kind), Some(_oid), Some(size)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(format!(
                "--write-gate-base: git ls-tree record is malformed: {record:?}"
            ));
        };
        if kind != "blob" || mode == SYMLINK_MODE {
            continue;
        }
        let size: u64 = size.parse().map_err(|e| {
            format!("--write-gate-base: git ls-tree reported a non-numeric size {size:?}: {e}")
        })?;
        if path_within_surfaces(path, &surfaces) {
            total += size;
        }
    }
    Ok(total)
}

/// git's mode for a SYMLINK blob (its content is the link target). Skipped by the commit-side
/// summation because [`sum_regular_files`] skips symlinks on disk (spec R1.7).
const SYMLINK_MODE: &str = "120000";

/// True when repo-relative `path` is within one of the already-normalized, already-lowercased
/// `surfaces` — equal to it, or under it (separator-anchored, so `Foo` never matches `FooBar`).
fn path_within_surfaces(path: &str, surfaces: &[String]) -> bool {
    let candidate = trusted_scope::normalize(path).to_lowercase();
    if candidate.is_empty() {
        return false;
    }
    surfaces
        .iter()
        .any(|entry| candidate == *entry || candidate.starts_with(&format!("{entry}/")))
}

/// The contract's caps, or the verification that replaces them. Shared by every entry point below so
/// a malformed contract fails closed identically no matter which one the caller reached for.
fn contract_and_limits(manifest_bytes: &[u8]) -> Result<(Contract, Limits), BudgetVerification> {
    let contract = match load_contract(manifest_bytes) {
        Ok(c) => c,
        Err(reason) => return Err(BudgetVerification::Exceeded(reason)),
    };
    let limits = match resolve_limits_from_contract(&contract) {
        LimitsResolution::Resolved(l) => l,
        LimitsResolution::Invalid(reason) => return Err(BudgetVerification::Exceeded(reason)),
        #[cfg(test)]
        LimitsResolution::MissingContract(reason) => {
            return Err(BudgetVerification::Skipped(reason))
        }
    };
    Ok((contract, limits))
}

/// The growth comparison itself, shared by both growth entry points so the two reference bases can
/// never drift apart in what counts as an overshoot or in how the overshoot is worded.
fn growth_verdict(
    limits: &Limits,
    baseline_bytes: u64,
    candidate_bytes: u64,
) -> BudgetVerification {
    let growth = candidate_bytes.saturating_sub(baseline_bytes);
    if growth > limits.max_growth_bytes {
        return BudgetVerification::Exceeded(format!(
            "editable surface grew by {growth} bytes ({baseline_bytes} → {candidate_bytes}), above \
             the growth limit {}",
            limits.max_growth_bytes
        ));
    }
    BudgetVerification::Verified {
        total_bytes: candidate_bytes,
        file_count: 0,
    }
}

/// benchd-native GROWTH bound: refuse a candidate whose editable code surface grew by more than
/// `maxGrowthBytes` over the trusted baseline. `growth = candidate_code_bytes −
/// baseline_code_bytes` (saturating at 0 — a shrink is never a growth violation). The caps + paths
/// are read from the trusted `--baseline` manifest bytes; a malformed manifest fails closed.
pub fn verify_growth_over(
    manifest_bytes: &[u8],
    baseline_root: &Path,
    candidate_root: &Path,
) -> BudgetVerification {
    let (contract, limits) = match contract_and_limits(manifest_bytes) {
        Ok(pair) => pair,
        Err(verification) => return verification,
    };
    let baseline_bytes = code_bytes(&contract, baseline_root);
    let candidate_bytes = code_bytes(&contract, candidate_root);
    growth_verdict(&limits, baseline_bytes, candidate_bytes)
}

/// The RULED fork-point form of the growth bound (David 2026-08-27), selected by
/// `measure-job --write-gate-base <SHA>`. Identical to [`verify_growth_over`] except that the
/// baseline half is the candidate repo's OWN state at `base_sha` — its fork point from harness main
/// — instead of the box-staged workspace, which under this mode is the paired TIMING baseline only.
///
/// This removes the growth bound's dependence on box-staging freshness for the same reason the
/// divergence gate's fork-point mode does (see
/// [`crate::editable_divergence::verify_no_write_outside_editable_from_git`]): an organizer commit
/// to harness main used to move the reference under every submission at once.
///
/// FAIL-CLOSED: any git failure — no git, no work tree, an unknown or badly-spelled base, an
/// unparseable listing — returns `Exceeded`, which the caller turns into the same die-8 refusal an
/// overshoot gets. The reason names both `write-gate-base` and `git` so an operator reading the
/// refusal can tell a WIRING fault from a real oversized submission.
pub fn verify_growth_over_from_git(
    manifest_bytes: &[u8],
    candidate_root: &Path,
    base_sha: &str,
) -> BudgetVerification {
    let (contract, limits) = match contract_and_limits(manifest_bytes) {
        Ok(pair) => pair,
        Err(verification) => return verification,
    };
    let baseline_bytes = match code_bytes_at_commit(&contract, candidate_root, base_sha) {
        Ok(bytes) => bytes,
        Err(reason) => return BudgetVerification::Exceeded(reason),
    };
    let candidate_bytes = code_bytes(&contract, candidate_root);
    growth_verdict(&limits, baseline_bytes, candidate_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Limits {
        Limits {
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_growth_bytes: DEFAULT_MAX_GROWTH_BYTES,
            exempt_path_max_bytes: DEFAULT_EXEMPT_PATH_MAX_BYTES,
            exempt_path_max_file_bytes: DEFAULT_EXEMPT_PATH_MAX_FILE_BYTES,
        }
    }

    /// FENCE, DIRECTION 1: an unknown key INSIDE `editableSurfaceByteBudget` is a fatal refusal
    /// that NAMES the key. Every key in that block is an enforced value, so an unrecognised one is
    /// a typo (the intended cap is silently not in force) or a cap a newer engine understands and
    /// this validator does not (benchd silently under-enforces). Both read stricter than they are.
    #[test]
    fn unknown_key_inside_the_budget_block_is_refused_and_named() {
        let m = br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"exemptPathMaxFileByte":100}}"#;
        match resolve_limits_from_bytes(m) {
            LimitsResolution::Invalid(r) => {
                // names the offending key...
                assert!(
                    r.contains("exemptPathMaxFileByte"),
                    "refusal must name the unknown key; got: {r}"
                );
                // ...and lists what IS accepted, so the fix is obvious from the message alone.
                assert!(
                    r.contains("exemptPathMaxFileBytes"),
                    "refusal must list the known keys; got: {r}"
                );
            }
            other => panic!("unknown budget key must fail closed, got {other:?}"),
        }
        // The five known caps plus exemptPaths all parse, so the fence is not simply refusing
        // everything -- the allowlist is real.
        let ok = br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":400,"maxGrowthBytes":300,"exemptPaths":["head"],"exemptPathMaxBytes":2000,"exemptPathMaxFileBytes":1000}}"#;
        match resolve_limits_from_bytes(ok) {
            LimitsResolution::Resolved(l) => {
                assert_eq!(l.max_total_bytes, 500);
                assert_eq!(l.max_file_bytes, 400);
                assert_eq!(l.max_growth_bytes, 300);
                assert_eq!(l.exempt_path_max_bytes, 2000);
                assert_eq!(l.exempt_path_max_file_bytes, 1000);
            }
            other => panic!("every known budget key must parse, got {other:?}"),
        }
    }

    /// FENCE, DIRECTION 2 — THE FENCE IS FENCED. An unknown field ELSEWHERE in `benchmark.json`
    /// (here a benign yukon-facing top-level key) parses clean and the run proceeds. The manifest
    /// is a shared document: yukon and the track own fields benchd neither reads nor should
    /// constrain, so denying at the top level would make benchd refuse manifests that are none of
    /// its business. Without this arm, direction 1 could be satisfied by a fence that is too wide.
    #[test]
    fn unknown_field_outside_the_budget_block_parses_clean() {
        let m = br#"{"editablePaths":["src"],"maxSubmissionBytes":536870912,"someFutureYukonField":{"nested":true},"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":400}}"#;
        match resolve_limits_from_bytes(m) {
            LimitsResolution::Resolved(l) => {
                assert_eq!(l.max_total_bytes, 500);
                assert_eq!(l.max_file_bytes, 400);
                // the untouched caps still take their pinned defaults
                assert_eq!(l.exempt_path_max_bytes, DEFAULT_EXEMPT_PATH_MAX_BYTES);
                assert_eq!(
                    l.exempt_path_max_file_bytes,
                    DEFAULT_EXEMPT_PATH_MAX_FILE_BYTES
                );
            }
            other => panic!("a top-level field benchd does not own must not refuse, got {other:?}"),
        }
    }

    #[test]
    fn absent_caps_take_the_pinned_defaults() {
        let m = br#"{"editablePaths":["a"]}"#;
        assert_eq!(
            resolve_limits_from_bytes(m),
            LimitsResolution::Resolved(defaults())
        );
    }

    #[test]
    fn a_declaration_may_lower_a_cap() {
        let m = br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxTotalBytes":1000,"maxFileBytes":500}}"#;
        match resolve_limits_from_bytes(m) {
            LimitsResolution::Resolved(l) => {
                assert_eq!(l.max_total_bytes, 1000);
                assert_eq!(l.max_file_bytes, 500);
                // untouched caps keep their defaults
                assert_eq!(l.max_growth_bytes, DEFAULT_MAX_GROWTH_BYTES);
                assert_eq!(l.exempt_path_max_bytes, DEFAULT_EXEMPT_PATH_MAX_BYTES);
                assert_eq!(
                    l.exempt_path_max_file_bytes,
                    DEFAULT_EXEMPT_PATH_MAX_FILE_BYTES
                );
            }
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn present_but_null_cap_fails_closed_not_default() {
        // Divergence D2: this is the exact case serde's Option would silently default.
        let m = br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxTotalBytes":null}}"#;
        assert!(matches!(
            resolve_limits_from_bytes(m),
            LimitsResolution::Invalid(_)
        ));
    }

    #[test]
    fn present_but_stringy_or_float_or_bool_cap_fails_closed() {
        for bad in [
            &br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxFileBytes":"500"}}"#[..],
            &br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxFileBytes":3.5}}"#[..],
            &br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxFileBytes":true}}"#[..],
        ] {
            assert!(
                matches!(resolve_limits_from_bytes(bad), LimitsResolution::Invalid(_)),
                "malformed cap must fail closed: {}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn non_positive_cap_fails_closed() {
        for bad in [
            &br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxTotalBytes":0}}"#[..],
            &br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxFileBytes":-1}}"#[..],
        ] {
            assert!(matches!(
                resolve_limits_from_bytes(bad),
                LimitsResolution::Invalid(_)
            ));
        }
    }

    #[test]
    fn per_file_cap_above_total_cannot_bind_and_is_refused() {
        let m = br#"{"editablePaths":["a"],"editableSurfaceByteBudget":{"maxTotalBytes":100,"maxFileBytes":200}}"#;
        assert!(matches!(
            resolve_limits_from_bytes(m),
            LimitsResolution::Invalid(_)
        ));
    }

    #[test]
    fn malformed_json_and_non_object_fail_closed() {
        assert!(matches!(
            resolve_limits_from_bytes(b"{not json"),
            LimitsResolution::Invalid(_)
        ));
        assert!(matches!(
            resolve_limits_from_bytes(b"[1,2,3]"),
            LimitsResolution::Invalid(_)
        ));
    }

    #[test]
    fn present_but_null_budget_object_fails_closed() {
        let m = br#"{"editablePaths":["a"],"editableSurfaceByteBudget":null}"#;
        assert!(matches!(
            resolve_limits_from_bytes(m),
            LimitsResolution::Invalid(_)
        ));
    }

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "bb-unit-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn walk_sums_regular_files_and_skips_symlinks() {
        let root = tmp("walk-sum");
        write(&root, "src/a.txt", b"hello"); // 5
        write(&root, "src/nested/b.txt", b"worldwide"); // 9
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("src/a.txt"), root.join("src/link.txt")).unwrap();
        let m = br#"{"editablePaths":["src"]}"#;
        match verify_byte_budget_over(m, &root) {
            BudgetVerification::Verified {
                total_bytes,
                file_count,
            } => {
                assert_eq!(total_bytes, 14, "5 + 9, symlink skipped");
                assert_eq!(file_count, 2);
            }
            other => panic!("expected verified, got {other:?}"),
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn walk_refuses_oversized_file_then_oversized_total() {
        let root = tmp("walk-over");
        write(&root, "src/big.txt", &[b'x'; 600]);
        let m = br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxTotalBytes":1000,"maxFileBytes":500}}"#;
        assert!(
            matches!(verify_byte_budget_over(m, &root), BudgetVerification::Exceeded(r) if r.contains("per-file"))
        );

        let root2 = tmp("walk-total");
        write(&root2, "src/a.txt", &[b'x'; 400]);
        write(&root2, "src/b.txt", &[b'y'; 400]);
        let m2 = br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500}}"#;
        assert!(
            matches!(verify_byte_budget_over(m2, &root2), BudgetVerification::Exceeded(r) if r.contains("at least"))
        );
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&root2).unwrap();
    }

    #[test]
    fn exempt_path_bypasses_per_file_and_total_but_pays_its_own_cap() {
        let root = tmp("walk-exempt");
        // A single 1000-byte file that would blow both the 500 per-file and 500 total code caps, but
        // it is exempt, so it pays only into the 2000 exempt cap and passes.
        write(&root, "head/w.bin", &[b'z'; 1000]);
        let ok = br#"{"editablePaths":["head"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500,"exemptPaths":["head"],"exemptPathMaxBytes":2000}}"#;
        assert!(matches!(
            verify_byte_budget_over(ok, &root),
            BudgetVerification::Verified {
                total_bytes: 0,
                file_count: 1
            }
        ));
        // Lower the exempt cap below the file — now it exceeds ITS cap.
        let bad = br#"{"editablePaths":["head"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500,"exemptPaths":["head"],"exemptPathMaxBytes":900}}"#;
        assert!(
            matches!(verify_byte_budget_over(bad, &root), BudgetVerification::Exceeded(r) if r.contains("exempt"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_editable_path_is_skipped_but_an_all_absent_surface_fails_closed() {
        let root = tmp("walk-missing");
        // R1.8: an individual missing editable path contributes nothing and is NOT an error...
        write(&root, "present/f.txt", &[b'z'; 10]);
        let mixed = br#"{"editablePaths":["does-not-exist","present"]}"#;
        assert!(matches!(
            verify_byte_budget_over(mixed, &root),
            BudgetVerification::Verified {
                total_bytes: 10,
                file_count: 1
            }
        ));
        // ...but D8 (issue #20 Q3): when EVERY path is absent the walk reaches zero files, and
        // absence is a refusal rather than a clean zero-byte pass. Swift-exact; this arm was a
        // fail-open divergence in the Rust port until the 2026-08-26 parity re-pin caught it.
        let all_absent = br#"{"editablePaths":["does-not-exist"]}"#;
        assert!(
            matches!(verify_byte_budget_over(all_absent, &root), BudgetVerification::Exceeded(r) if r.contains("absence is a refusal"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn empty_editable_paths_fails_closed() {
        let root = tmp("walk-empty");
        assert!(matches!(
            verify_byte_budget_over(br#"{"editablePaths":[]}"#, &root),
            BudgetVerification::Exceeded(_)
        ));
        assert!(matches!(
            verify_byte_budget_over(br#"{}"#, &root),
            BudgetVerification::Exceeded(_)
        ));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn growth_is_candidate_minus_baseline_over_code_bytes() {
        let base = tmp("growth-base");
        let cand = tmp("growth-cand");
        write(&base, "src/a.txt", &[b'x'; 100]);
        write(&cand, "src/a.txt", &[b'x'; 100]);
        write(&cand, "src/added.txt", &[b'y'; 300]); // +300 growth
        let m = br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxGrowthBytes":200}}"#;
        assert!(
            matches!(verify_growth_over(m, &base, &cand), BudgetVerification::Exceeded(r) if r.contains("grew by 300"))
        );
        // A larger growth cap admits it.
        let m_ok =
            br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxGrowthBytes":400}}"#;
        assert!(matches!(
            verify_growth_over(m_ok, &base, &cand),
            BudgetVerification::Verified { .. }
        ));
        // A shrink is never a violation.
        let m_shrink =
            br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxGrowthBytes":1}}"#;
        assert!(matches!(
            verify_growth_over(m_shrink, &cand, &base),
            BudgetVerification::Verified { .. }
        ));
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    // -----------------------------------------------------------------------
    // The RULED fork-point growth base (`--write-gate-base`, David 2026-08-27).
    // -----------------------------------------------------------------------

    /// Run one git command in `repo`, asserting success. Identity is pinned and SIGNING is off
    /// (`commit.gpgsign=false`) so the fixtures build the same on a signing laptop and on a keyless
    /// runner; `core.excludesFile=/dev/null` keeps a developer's global ignore file from dropping a
    /// fixture file out of the commit the byte counts depend on.
    fn git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=benchd test",
                "-c",
                "user.email=benchd@example.invalid",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "core.excludesFile=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn git_repo(tag: &str) -> std::path::PathBuf {
        let repo = tmp(tag);
        git(&repo, &["init", "-q"]);
        repo
    }

    fn commit_all(repo: &Path, message: &str) -> String {
        git(repo, &["add", "-A", "."]);
        git(repo, &["commit", "-q", "-m", message]);
        git(repo, &["rev-parse", "HEAD"])
    }

    /// The fork-point growth base reads the BASE COMMIT'S blobs, not a staged workspace: the same
    /// +300 growth is measured against the repo's own history, and the same cap decides it. The work
    /// tree here is the CANDIDATE (the base commit's 100 bytes plus the 300 the submission added),
    /// which is exactly the shape the ruled mode runs in — one workspace, two points in its history.
    #[test]
    fn growth_from_git_measures_the_base_commits_blobs() {
        let repo = git_repo("growth-git");
        write(&repo, "src/a.txt", &[b'x'; 100]);
        let base = commit_all(&repo, "base");
        write(&repo, "src/added.txt", &[b'y'; 300]); // +300 growth over the fork point
        commit_all(&repo, "submission: grow the surface");

        let m = br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxGrowthBytes":200}}"#;
        assert!(
            matches!(verify_growth_over_from_git(m, &repo, &base), BudgetVerification::Exceeded(r) if r.contains("grew by 300"))
        );
        let m_ok =
            br#"{"editablePaths":["src"],"editableSurfaceByteBudget":{"maxGrowthBytes":400}}"#;
        assert!(matches!(
            verify_growth_over_from_git(m_ok, &repo, &base),
            BudgetVerification::Verified {
                total_bytes: 400,
                ..
            }
        ));
        std::fs::remove_dir_all(&repo).unwrap();
    }

    /// The commit-side sum mirrors the disk-side one on the entries that are NOT plain files: an
    /// exempt editable entry is not code, and a symlink is skipped on both sides (git spells it as
    /// mode 120000, `sum_regular_files` lstats it). With both sides agreeing there is no growth.
    #[test]
    #[cfg(unix)]
    fn growth_from_git_skips_symlinks_and_exempt_entries_like_the_disk_walk() {
        let repo = git_repo("growth-git-skips");
        write(&repo, "src/a.txt", &[b'x'; 100]);
        std::os::unix::fs::symlink("a.txt", repo.join("src/link.txt")).unwrap();
        write(&repo, "weights/blob.bin", &[b'w'; 5000]);
        let base = commit_all(&repo, "base");

        let m = br#"{"editablePaths":["src","weights"],
                     "editableSurfaceByteBudget":{"exemptPaths":["weights"],"maxGrowthBytes":1}}"#;
        assert!(matches!(
            verify_growth_over_from_git(m, &repo, &base),
            BudgetVerification::Verified {
                total_bytes: 100,
                ..
            }
        ));
        std::fs::remove_dir_all(&repo).unwrap();
    }

    /// FAIL-CLOSED — a git failure is `Exceeded`, and its reason names BOTH `write-gate-base` and
    /// `git` so an operator can tell a wiring fault from a real oversized submission.
    #[test]
    fn a_git_failure_is_a_distinguishable_exceeded() {
        let repo = git_repo("growth-git-failclosed");
        write(&repo, "src/a.txt", &[b'x'; 100]);
        let base = commit_all(&repo, "base");
        let m = br#"{"editablePaths":["src"]}"#;

        for bad_base in [
            &base[..12],
            "not-a-sha",
            "0123456789abcdef0123456789abcdef01234567",
        ] {
            match verify_growth_over_from_git(m, &repo, bad_base) {
                BudgetVerification::Exceeded(reason) => {
                    assert!(
                        reason.contains("write-gate-base") && reason.contains("git"),
                        "the refusal must name the wiring, not read as an overshoot: {reason}"
                    );
                }
                other => panic!("expected Exceeded for base {bad_base:?}, got {other:?}"),
            }
        }
        std::fs::remove_dir_all(&repo).unwrap();

        // A directory that is not a work tree at all.
        let bare = tmp("growth-git-notarepo");
        match verify_growth_over_from_git(m, &bare, "0123456789abcdef0123456789abcdef01234567") {
            BudgetVerification::Exceeded(reason) => {
                assert!(
                    reason.contains("write-gate-base") && reason.contains("git"),
                    "{reason}"
                )
            }
            other => panic!("expected Exceeded, got {other:?}"),
        }
        std::fs::remove_dir_all(&bare).unwrap();
    }

    #[test]
    fn file_path_entries_map_missing_to_skipped_and_missingcontract() {
        let root = tmp("path-missing");
        let absent = root.join("benchmark.json");
        assert!(matches!(
            verify_byte_budget_at(&absent),
            BudgetVerification::Skipped(_)
        ));
        assert!(matches!(
            resolve_limits_at(&absent),
            LimitsResolution::MissingContract(_)
        ));
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// The PARITY TEST David's WIRE-1 ruling requires: the single-source-of-truth mechanism now that
/// benchd owns a NATIVE RUST implementation. It runs BOTH implementations — this Rust port AND the
/// engine's Swift enforcer (a byte-for-byte pinned copy under `tests/parity_fixtures/swift/`,
/// compiled with `swiftc -O` into the same `editable-surface-budget verify|limits` CLI the engine
/// ships) — over SHARED on-disk fixtures, and asserts IDENTICAL accept/reject AND identical numbers
/// (resolved caps, verified totalBytes/fileCount) on each. A divergence between the two enforcers is
/// a failure here, which is how the two-source concern is managed.
#[cfg(test)]
mod swift_parity {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::OnceLock;

    /// The Swift CLI's exit contract (its `main.swift`): 0 verified / limits resolved, 1 exceeded or
    /// invalid (fail closed), 2 no contract on disk.
    #[derive(Debug, PartialEq, Eq)]
    enum SwiftClass {
        Ok(Vec<String>), // exit 0, stdout lines
        Fail1,           // exceeded / invalid
        Fail2,           // missing contract
        Other(i32),
    }

    fn build_swift_cli() -> Option<PathBuf> {
        static CLI: OnceLock<Option<PathBuf>> = OnceLock::new();
        CLI.get_or_init(|| {
            let swiftc = which_swiftc()?;
            let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/parity_fixtures/swift");
            let out = std::env::temp_dir().join(format!(
                "esb-cli-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let status = Command::new(swiftc)
                .arg("-O")
                .arg(src.join("EditableSurfaceByteBudget.swift"))
                .arg(src.join("main.swift"))
                .arg("-o")
                .arg(&out)
                .status()
                .expect("spawn swiftc");
            assert!(
                status.success(),
                "swiftc failed to build the pinned parity CLI"
            );
            Some(out)
        })
        .clone()
    }

    fn which_swiftc() -> Option<PathBuf> {
        let out = Command::new("swiftc").arg("--version").output().ok()?;
        out.status.success().then(|| PathBuf::from("swiftc"))
    }

    fn swift(cli: &Path, cmd: &str, contract: &Path) -> SwiftClass {
        let out = Command::new(cli)
            .arg(cmd)
            .arg(contract)
            .output()
            .expect("spawn swift cli");
        match out.status.code().expect("swift cli exited via signal") {
            0 => SwiftClass::Ok(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::to_string)
                    .collect(),
            ),
            1 => SwiftClass::Fail1,
            2 => SwiftClass::Fail2,
            n => SwiftClass::Other(n),
        }
    }

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// One shared fixture directory: a `benchmark.json` plus whatever editable trees it references,
    /// or no manifest at all (the missing-contract case).
    struct Fx {
        dir: PathBuf,
    }
    impl Fx {
        fn new(bed: &Path, name: &str) -> Fx {
            let dir = bed.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            Fx { dir }
        }
        fn manifest(self, json: &str) -> Fx {
            std::fs::write(self.dir.join("benchmark.json"), json).unwrap();
            self
        }
        fn file(self, rel: &str, bytes: usize) -> Fx {
            write(&self.dir, rel, &vec![b'x'; bytes]);
            self
        }
        fn contract(&self) -> PathBuf {
            self.dir.join("benchmark.json")
        }
    }

    /// The RUST `verify` outcome mapped to the Swift exit CLASS + numbers, for a direct compare.
    fn rust_verify_class(contract: &Path) -> SwiftClass {
        match verify_byte_budget_at(contract) {
            BudgetVerification::Verified {
                total_bytes,
                file_count,
            } => SwiftClass::Ok(vec![format!(
                "verified totalBytes={total_bytes} fileCount={file_count}"
            )]),
            BudgetVerification::Skipped(_) => SwiftClass::Fail2,
            BudgetVerification::Exceeded(_) => SwiftClass::Fail1,
        }
    }

    /// The RUST `limits` outcome mapped to the Swift exit CLASS + printed cap lines.
    fn rust_limits_class(contract: &Path) -> SwiftClass {
        match resolve_limits_at(contract) {
            LimitsResolution::Resolved(l) => SwiftClass::Ok(vec![
                format!("maxTotalBytes={}", l.max_total_bytes),
                format!("maxFileBytes={}", l.max_file_bytes),
                format!("maxGrowthBytes={}", l.max_growth_bytes),
                format!("exemptPathMaxBytes={}", l.exempt_path_max_bytes),
                format!("exemptPathMaxFileBytes={}", l.exempt_path_max_file_bytes),
            ]),
            LimitsResolution::MissingContract(_) => SwiftClass::Fail2,
            LimitsResolution::Invalid(_) => SwiftClass::Fail1,
        }
    }

    #[test]
    fn rust_and_swift_agree_on_every_shared_fixture() {
        let cli = match build_swift_cli() {
            Some(cli) => cli,
            None => {
                // The parity test's oracle is the Swift enforcer; without a Swift toolchain there is
                // no oracle. On the ranked box (macOS) that is a hard failure — the test must never
                // silently pass there. Off-macOS (a Linux dev box with no Swift) it honestly skips.
                if cfg!(target_os = "macos") {
                    panic!("swiftc not found on macOS — the parity oracle cannot be built");
                }
                eprintln!("SKIP byte-budget parity: no swiftc on this non-macOS host");
                return;
            }
        };

        let bed = std::env::temp_dir().join(format!(
            "bb-parity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&bed);
        std::fs::create_dir_all(&bed).unwrap();

        // The SHARED fixtures. Each exercises a documented rule; together they cover every verify
        // class (verified / exceeded / skipped) and every limits class (resolved / invalid /
        // missing), so neither the parity nor the individual enforcers can be vacuous.
        let fixtures = vec![
            Fx::new(&bed, "verified_small")
                .manifest(r#"{"editablePaths":["Sources/A"]}"#)
                .file("Sources/A/f.txt", 5),
            Fx::new(&bed, "lowered_caps_pass")
                .manifest(r#"{"editablePaths":["Sources/A"],"editableSurfaceByteBudget":{"maxTotalBytes":10,"maxFileBytes":5}}"#)
                .file("Sources/A/f.txt", 5),
            Fx::new(&bed, "nested_dirs")
                .manifest(r#"{"editablePaths":["Sources"]}"#)
                .file("Sources/A/x.txt", 7)
                .file("Sources/B/deep/y.txt", 11),
            Fx::new(&bed, "per_file_overshoot")
                .manifest(r#"{"editablePaths":["Sources/A"],"editableSurfaceByteBudget":{"maxTotalBytes":100,"maxFileBytes":5}}"#)
                .file("Sources/A/big.txt", 8),
            Fx::new(&bed, "total_overshoot")
                .manifest(r#"{"editablePaths":["Sources/A"],"editableSurfaceByteBudget":{"maxTotalBytes":6,"maxFileBytes":5}}"#)
                .file("Sources/A/a.txt", 4)
                .file("Sources/A/b.txt", 4),
            Fx::new(&bed, "exempt_bypass")
                .manifest(r#"{"editablePaths":["head"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500,"exemptPaths":["head"],"exemptPathMaxBytes":2000}}"#)
                .file("head/w.bin", 1000),
            Fx::new(&bed, "exempt_overshoot")
                .manifest(r#"{"editablePaths":["head"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500,"exemptPaths":["head"],"exemptPathMaxBytes":900}}"#)
                .file("head/w.bin", 1000),
            // R1.11a — the exempt PER-FILE cap. One blob over it, but well under the
            // aggregate: before this cap an exempt file had no per-file bound at all.
            Fx::new(&bed, "exempt_per_file_overshoot")
                .manifest(r#"{"editablePaths":["head"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500,"exemptPaths":["head"],"exemptPathMaxBytes":100000,"exemptPathMaxFileBytes":500}}"#)
                .file("head/w.bin", 1000),
            // NEGATIVE CONTROL: the sharded shape the per-file cap exists to permit. Two
            // shards, each under the per-file cap, together under the aggregate.
            Fx::new(&bed, "exempt_sharded_ok")
                .manifest(r#"{"editablePaths":["head"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500,"exemptPaths":["head"],"exemptPathMaxBytes":100000,"exemptPathMaxFileBytes":500}}"#)
                .file("head/a.bin", 400)
                .file("head/b.bin", 400),
            // An exempt per-file cap ABOVE the aggregate is legal and simply never binds: the
            // aggregate is the tighter bound and refuses first. Pins the fail-open-on-the-pair,
            // closed-on-the-bytes posture both enforcers share.
            Fx::new(&bed, "exempt_per_file_above_aggregate_never_binds")
                .manifest(r#"{"editablePaths":["head"],"editableSurfaceByteBudget":{"maxTotalBytes":500,"maxFileBytes":500,"exemptPaths":["head"],"exemptPathMaxBytes":900,"exemptPathMaxFileBytes":1000}}"#)
                .file("head/w.bin", 1000),
            Fx::new(&bed, "missing_editable_path")
                .manifest(r#"{"editablePaths":["does-not-exist"]}"#),
            Fx::new(&bed, "empty_editable_paths").manifest(r#"{"editablePaths":[]}"#),
            Fx::new(&bed, "malformed_cap_null")
                .manifest(r#"{"editablePaths":["Sources/A"],"editableSurfaceByteBudget":{"maxTotalBytes":null}}"#)
                .file("Sources/A/f.txt", 5),
            Fx::new(&bed, "malformed_cap_string")
                .manifest(r#"{"editablePaths":["Sources/A"],"editableSurfaceByteBudget":{"maxFileBytes":"500"}}"#)
                .file("Sources/A/f.txt", 5),
            Fx::new(&bed, "per_file_above_total")
                .manifest(r#"{"editablePaths":["Sources/A"],"editableSurfaceByteBudget":{"maxTotalBytes":100,"maxFileBytes":200}}"#)
                .file("Sources/A/f.txt", 5),
            // No manifest at all — the missing-contract case.
            Fx::new(&bed, "absent_contract"),
        ];

        fn class_tag(c: &SwiftClass) -> u8 {
            match c {
                SwiftClass::Ok(_) => 0,
                SwiftClass::Fail1 => 1,
                SwiftClass::Fail2 => 2,
                SwiftClass::Other(_) => 3,
            }
        }
        let mut verify_classes = std::collections::HashSet::new();
        let mut limits_classes = std::collections::HashSet::new();

        for fx in &fixtures {
            let contract = fx.contract();
            let name = fx.dir.file_name().unwrap().to_string_lossy().to_string();

            // verify parity — identical class AND identical numbers.
            let rv = rust_verify_class(&contract);
            let sv = swift(&cli, "verify", &contract);
            assert_eq!(rv, sv, "VERIFY parity divergence on fixture {name}");
            verify_classes.insert(class_tag(&sv));

            // limits parity — identical class AND identical caps.
            let rl = rust_limits_class(&contract);
            let sl = swift(&cli, "limits", &contract);
            assert_eq!(rl, sl, "LIMITS parity divergence on fixture {name}");
            limits_classes.insert(class_tag(&sl));
        }

        // Anti-vacuity: the fixtures must have exercised every verify class (verified/exceeded/
        // skipped) and at least the resolved+invalid+missing limits classes — otherwise the parity
        // asserts nothing interesting. `Ok/Fail1/Fail2` discriminants correspond to those classes.
        assert_eq!(
            verify_classes.len(),
            3,
            "verify fixtures did not cover all three classes"
        );
        assert_eq!(
            limits_classes.len(),
            3,
            "limits fixtures did not cover all three classes"
        );

        let _ = std::fs::remove_dir_all(&bed);
        let _ = std::fs::remove_file(&cli);
    }
}
