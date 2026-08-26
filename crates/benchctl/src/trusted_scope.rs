//! DECIDE-1 — the general trusted-source-scope freeze, homed in benchd.
//!
//! The broad invariant is: NO editable-surface entry may overlap the TRUSTED SCOPE — the
//! timer/gates/score sources plus the frozen dependency graph plus the machinery that judges a
//! submission. A submission (or a manifest that drifts) which declared any of that editable would
//! be able to rewrite the thing that scores it. Upstream enforced this at dispatch in a shell
//! tripwire (`.github/scripts/verify-trusted-source-scope.sh`,
//! `verify_contract_does_not_expose_trusted_scope`); that enforcer was removed from the tree and
//! the surviving guard narrowed to the benchd gitlink only. This module restores the general
//! freeze on the benchd side, where the ruling homes it: "all final validation in benchd since
//! engine is where people do submissions."
//!
//! REFERENCE LOGIC (ported, not copied). Two references, both read for their LOGIC only:
//!   * the surviving mirror shell `verify-trusted-source-scope.sh` — its `paths_overlap` is the
//!     base relation: `a == b || a under b || b under a`, separator-anchored so a mere name-prefix
//!     sibling (`Package.swift.bak`, `Sources/MLXFastCoreExtras`) does NOT overlap; and its
//!     `verify_contract_does_not_expose_trusted_scope` walks the contract's editable entries
//!     against the scope roster;
//!   * the engine-side rest-state linter `tools/lint-benchmark-manifest.py`
//!     (`_trusted_scope_overlap` + `_normalize` / `_prefixes` / `_illegal_editable_entry` /
//!     `_same_file` / `_editable_buckets`), which is where the roster machinery already carries the
//!     two additions this repository needs and that the shell base relation lacks: CASEFOLD (the
//!     ranked box is macOS/APFS, case-INSENSITIVE by default, so `sources/mlxfastcore` names the
//!     real directory) and DEVICE:INODE identity over each side's ancestors (for the spellings
//!     ASCII/Unicode folding does not reach — a symlink, a decomposed Unicode name).
//!
//! The overlap relation here is that same relation, ported into Rust: it is NOT a substring test.
//! No engine-side Rust copy of it existed to reuse (the only sibling with casefold in benchctl is
//! `weights_preflight`'s single `to_ascii_lowercase`, unrelated), so the relation is ported cleanly
//! into benchd rather than duplicated.

use std::path::Path;

use serde::Deserialize;

/// The ROSTER-OF-EIGHT trusted scope (David, DECIDE-1 sub-decision 1; extended by the roster-of-
/// eight ruling). The first five are upstream's `TRUSTED_SCOPE_FILES` + `TRUSTED_SCOPE_DIRS` (the
/// manifests that decide which sources feed which product, plus the timer/gates/score source trees);
/// `.github` and `tools` are the deliberate, ruled divergence from upstream's five — the gates
/// themselves (overlay, static review, surface gate, the manifest linter, CI tripwires) live in
/// those two directories, and a contract that could declare them editable would let a submission
/// rewrite the machinery that judges it.
///
/// The EIGHTH entry is `benchmark.json` itself (David ruled). The manifest that DEFINES the editable
/// surface can never itself be declared editable: a submission listing `benchmark.json` in
/// `editablePaths` (or any bucket) could redefine its own limits — widen the surface, or rewrite the
/// exempt/byte-budget rules — from inside the very file the freeze reads. Freezing the manifest
/// against itself closes that self-reference. `benchmark.json` is a FILE, so the overlap relation
/// treats it exactly as it treats `Package.swift`/`Package.resolved` (lexical `equals`, plus the
/// device:inode arm for a symlinked spelling); no new overlap logic is needed.
///
/// This is the SECURITY SURFACE: a silently-shortened roster is the hole, so
/// [`tests::roster_is_exactly_the_ruled_eight`] cross-checks this constant against an independent
/// enumeration of the ruled eight and fails loudly on any drift (a dropped eighth entry included).
pub const ROSTER_OF_EIGHT: [&str; 8] = [
    "Package.swift",
    "Package.resolved",
    "Sources/MLXFastTrustedHarness",
    "Sources/MLXFastCLI",
    "Sources/MLXFastCore",
    ".github",
    "tools",
    "benchmark.json",
];

/// The editable surface declared by a `benchmark.json` manifest. Only the three buckets an overlay
/// writes from are modelled; serde ignores the rest of the manifest. `exemptPaths` is a real bucket
/// here — it exempts BYTES from the code budget, not the PATH from the overlay, so an exempt entry
/// is still overlaid and must still not reach the trusted scope.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EditableSurface {
    #[serde(default, rename = "editablePaths")]
    pub editable_paths: Vec<String>,
    #[serde(default, rename = "optionalEditablePaths")]
    pub optional_editable_paths: Vec<String>,
    #[serde(default, rename = "editableSurfaceByteBudget")]
    pub byte_budget: ByteBudget,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ByteBudget {
    #[serde(default, rename = "exemptPaths")]
    pub exempt_paths: Vec<String>,
}

impl EditableSurface {
    /// Parse a manifest's bytes fail-closed (never fall open on malformed JSON).
    pub fn parse(bytes: &[u8]) -> Result<EditableSurface, String> {
        serde_json::from_slice(bytes)
            .map_err(|e| format!("benchmark.json editable-surface parse failed: {e}"))
    }

    /// The three buckets, each named, so a refusal can say WHICH one to fix.
    fn buckets(&self) -> [(&'static str, &[String]); 3] {
        [
            ("editablePaths", &self.editable_paths),
            ("optionalEditablePaths", &self.optional_editable_paths),
            (
                "editableSurfaceByteBudget.exemptPaths",
                &self.byte_budget.exempt_paths,
            ),
        ]
    }
}

/// A repo-relative path reduced to the join of its non-empty, non-`.` segments. THE SINGLE
/// REDUCTION: every lexical comparison and [`prefixes`] runs on this form so the string arm and the
/// filesystem arm can never disagree about the same entry (the `Sources//MLXFastCore` hole in the
/// reference linter's history).
pub(crate) fn normalize(rel: &str) -> String {
    rel.trim_matches('/')
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Every ancestor of a repo-relative path, shortest first, INCLUDING itself, built from the
/// normalized form.
pub(crate) fn prefixes(rel: &str) -> Vec<String> {
    let norm = normalize(rel);
    if norm.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = norm.split('/').collect();
    (0..parts.len()).map(|i| parts[..=i].join("/")).collect()
}

/// Why `entry` is not a legal repo-relative editable path, or `None`. Checked BEFORE the overlap
/// arithmetic because every spelling here DEFEATS that arithmetic rather than failing it (mirrors
/// the reference linter's `_illegal_editable_entry`, itself mirroring the overlay's validity rule):
///
///   * `""` / `"."` / `"./"` resolve to the repo ROOT, which contains every trusted path, and
///     normalize to the empty join so no comparison happens at all;
///   * an ABSOLUTE path is re-rooted under the repo by the join, so an absolute spelling of the
///     real `Sources/MLXFastCore` compares as neither equal nor same-file;
///   * a `:pathspec` is git pathspec magic, not a path;
///   * a backslash or a `.`/`..` segment is only caught downstream when it happens to resolve.
///
/// None of these is a live hole (the overlay refuses them at run time) but the freeze must not be
/// the layer that says yes — it refuses the shape rather than reasoning about it.
fn illegal_editable_entry(entry: &str) -> Option<&'static str> {
    if entry.trim().is_empty() {
        return Some("is empty");
    }
    if entry.starts_with('/') {
        return Some("is an absolute path, not a repo-relative one");
    }
    if entry.starts_with(':') {
        return Some("is a pathspec, not a path");
    }
    if entry.contains('\\') {
        return Some("contains a backslash");
    }
    let padded = format!("/{entry}/");
    if padded.contains("/./") || padded.contains("/../") {
        return Some("contains a '.' or '..' segment");
    }
    None
}

/// True when two repo-relative paths are the SAME FILE under `root` on this filesystem — device +
/// inode identity, following symlinks. This is the arm that catches the spellings casefold does not
/// reach (a symlink or hardlink into the trusted scope, a Unicode-decomposed name on APFS). Both
/// sides must resolve; a path that does not resolve is left to the folded string arm. A root-ish
/// `a` (`""`/`.`/`/`) is never same-file'd — its identity is the repo root, handled by the shape
/// check.
#[cfg(unix)]
pub(crate) fn same_file(root: &Path, a: &str, b: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    if a.is_empty() || a == "." || a == "/" {
        return false;
    }
    let (ma, mb) = match (
        std::fs::metadata(root.join(a)),
        std::fs::metadata(root.join(b)),
    ) {
        (Ok(ma), Ok(mb)) => (ma, mb),
        _ => return false,
    };
    ma.dev() == mb.dev() && ma.ino() == mb.ino()
}

#[cfg(not(unix))]
pub(crate) fn same_file(root: &Path, a: &str, b: &str) -> bool {
    if a.is_empty() || a == "." || a == "/" {
        return false;
    }
    match (
        std::fs::canonicalize(root.join(a)),
        std::fs::canonicalize(root.join(b)),
    ) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// The relation word if `entry` reaches trusted `scope` under `root`, else `None`. SEMANTICS stated
/// rather than inherited — an editable entry overlaps a trusted path when either contains the other
/// or they are equal:
///
///   equals    entry == scope                      `Package.swift`
///   is inside entry is under scope                 `Sources/MLXFastCore/X.swift`
///   contains  scope is under entry                 `Sources`
///
/// The separator is appended before the prefix test, so a sibling whose name merely STARTS with a
/// scope path (`Sources/MLXFastCoreExtras`, `Package.swift.bak`) does NOT overlap. Same base
/// relation as the mirror shell's `paths_overlap`, plus the two additions the reference linter
/// already carries and that apply here for the identical reason: CASEFOLD (macOS/APFS is
/// case-insensitive), and DEVICE:INODE identity over each side's ancestors against the whole other
/// path. Ancestor-to-ancestor identity is deliberately NOT tested (every legit entry shares the
/// `Sources` ancestor with the scope).
fn trusted_scope_overlap(root: &Path, entry: &str, scope: &str) -> Option<&'static str> {
    let a = normalize(entry).to_lowercase();
    let b = normalize(scope).to_lowercase();
    if a.is_empty() {
        return None;
    }
    if a == b {
        return Some("equals");
    }
    if a.starts_with(&format!("{b}/")) {
        return Some("is inside");
    }
    if b.starts_with(&format!("{a}/")) {
        return Some("contains");
    }
    // Inode identity: an ancestor of the entry that IS the scope (entry equals or is inside it).
    for ancestor in prefixes(entry) {
        if same_file(root, &ancestor, scope) {
            return Some(if ancestor.to_lowercase() == a {
                "resolves to"
            } else {
                "is inside"
            });
        }
    }
    // Inode identity the other way: the whole entry IS a strict ancestor of the scope (entry
    // contains it). The scope's own last prefix (itself) is excluded — that is the "is inside"
    // case handled above.
    let scope_prefixes = prefixes(scope);
    let strict_scope_ancestors = scope_prefixes
        .split_last()
        .map(|(_, rest)| rest)
        .unwrap_or(&[]);
    let entry_norm = normalize(entry);
    for ancestor in strict_scope_ancestors {
        if same_file(root, &entry_norm, ancestor) {
            return Some("contains");
        }
    }
    None
}

/// Refuse a manifest whose editable surface overlaps the trusted scope, resolved against the
/// TRUSTED REF (`trusted_root` — the baseline workspace, DECIDE-1 sub-decision 2). This is the WIRE
/// benchd's measure-job calls (DECIDE-1 sub-decision 3): a manifest that DECLARES any roster-of-
/// seven trusted path editable — directly, cased, or via an inode-identical spelling — is REFUSED
/// before any GPU work.
///
/// FAIL-CLOSED in two further ways beyond the overlap itself:
///   * malformed manifest JSON is a refusal, never a fall-open;
///   * a roster path that does not exist under the trusted ref is a refusal — the freeze would be
///     VACUOUS against a renamed/removed trusted tree, so a drop of the security surface fails
///     loudly here rather than silently ceasing to guard.
pub fn verify_editable_surface_within_trusted_scope(
    trusted_root: &Path,
    manifest_bytes: &[u8],
) -> Result<(), String> {
    let surface = EditableSurface::parse(manifest_bytes)?;

    // Anti-vacuous: every roster path must still exist under the trusted ref. `benchmark.json` is
    // in the roster and is present by construction here (this fn runs only when the manifest is a
    // file), so its existence is never the missing one — but the check stays uniform over the eight.
    let missing: Vec<&str> = ROSTER_OF_EIGHT
        .iter()
        .copied()
        .filter(|scope| !trusted_root.join(scope).exists())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "trusted scope: roster path(s) [{}] do not exist under the trusted ref {} — the \
             trusted-scope freeze would be vacuous; update ROSTER_OF_EIGHT if the trusted tree was \
             renamed",
            missing.join(", "),
            trusted_root.display()
        ));
    }

    for (bucket, entries) in surface.buckets() {
        for entry in entries {
            if let Some(why) = illegal_editable_entry(entry) {
                return Err(format!(
                    "trusted scope: {bucket} entry {entry:?} {why} — an entry this shape cannot be \
                     shown NOT to reach the trusted harness, so it is refused rather than reasoned \
                     about"
                ));
            }
            for scope in ROSTER_OF_EIGHT {
                if let Some(how) = trusted_scope_overlap(trusted_root, entry, scope) {
                    return Err(format!(
                        "trusted scope: {bucket} entry {entry:?} {how} trusted path {scope:?} — a \
                         submission must never be able to edit the timer, the gates or the frozen \
                         dependency graph"
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The roster IS the security surface: cross-check the constant against an INDEPENDENT
    /// enumeration of David's ruled eight (DECIDE-1 sub-decision 1 + the roster-of-eight ruling). A
    /// dropped, added, or renamed roster entry fails here loudly — a silently-shortened roster is the
    /// hole this test closes. In particular, dropping the eighth entry (`benchmark.json`) fails here:
    /// the exactly-N length assert and the set-equality both catch it (anti-tautology — the ruled set
    /// is written out by hand, not derived from the constant).
    #[test]
    fn roster_is_exactly_the_ruled_eight() {
        // Written out independently from ROSTER_OF_EIGHT, from the ruling text — including the
        // eighth entry, `benchmark.json` (the manifest that defines the editable surface can not be
        // declared editable in that same manifest).
        let ruled: BTreeSet<&str> = [
            "Sources/MLXFastCLI",
            "Sources/MLXFastTrustedHarness",
            "Sources/MLXFastCore",
            "Package.swift",
            "Package.resolved",
            ".github",
            "tools",
            "benchmark.json",
        ]
        .into_iter()
        .collect();
        let constant: BTreeSet<&str> = ROSTER_OF_EIGHT.iter().copied().collect();
        assert_eq!(ROSTER_OF_EIGHT.len(), 8, "the roster is EIGHT entries");
        assert_eq!(
            constant.len(),
            ROSTER_OF_EIGHT.len(),
            "ROSTER_OF_EIGHT has a duplicate entry"
        );
        assert_eq!(
            constant, ruled,
            "ROSTER_OF_EIGHT drifted from David's ruled eight (DECIDE-1 + roster-of-eight)"
        );
    }

    #[test]
    fn normalize_reduces_to_segment_join() {
        assert_eq!(normalize("Sources//MLXFastCore"), "Sources/MLXFastCore");
        assert_eq!(normalize("/Sources/MLXFastCore/"), "Sources/MLXFastCore");
        assert_eq!(normalize("Sources/./MLXFastCore"), "Sources/MLXFastCore");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("/"), "");
    }

    #[test]
    fn prefixes_are_ancestors_shortest_first_including_self() {
        assert_eq!(
            prefixes("Sources/MLXFastCore/X.swift"),
            vec![
                "Sources",
                "Sources/MLXFastCore",
                "Sources/MLXFastCore/X.swift"
            ]
        );
        assert!(prefixes("").is_empty());
    }

    #[test]
    fn illegal_shapes_are_refused_before_arithmetic() {
        assert_eq!(illegal_editable_entry(""), Some("is empty"));
        assert_eq!(illegal_editable_entry("   "), Some("is empty"));
        assert!(illegal_editable_entry("/abs/Sources/MLXFastCore").is_some());
        assert!(illegal_editable_entry(":pathspec").is_some());
        assert!(illegal_editable_entry("a\\b").is_some());
        assert_eq!(
            illegal_editable_entry("a/../b"),
            Some("contains a '.' or '..' segment")
        );
        assert_eq!(
            illegal_editable_entry("."),
            Some("contains a '.' or '..' segment")
        );
        // A legit participant path is not illegal.
        assert_eq!(illegal_editable_entry("Sources/MLXFastModel"), None);
    }

    /// The overlap RELATION (lexical arms), independent of any filesystem. `root` is unused by the
    /// lexical arms, so a bogus root is fine here.
    #[test]
    fn overlap_relation_equals_inside_contains_and_siblings() {
        let root = Path::new("/nonexistent-root-for-lexical-only");
        // equals
        assert_eq!(
            trusted_scope_overlap(root, "Package.swift", "Package.swift"),
            Some("equals")
        );
        // entry inside scope
        assert_eq!(
            trusted_scope_overlap(
                root,
                "Sources/MLXFastCore/Timer.swift",
                "Sources/MLXFastCore"
            ),
            Some("is inside")
        );
        // scope inside entry
        assert_eq!(
            trusted_scope_overlap(root, "Sources", "Sources/MLXFastCore"),
            Some("contains")
        );
        // CASEFOLD: macOS/APFS case-insensitive spelling names the real dir
        assert_eq!(
            trusted_scope_overlap(root, "sources/mlxfastcore", "Sources/MLXFastCore"),
            Some("equals")
        );
        // name-prefix SIBLINGS do NOT overlap (separator-anchored)
        assert_eq!(
            trusted_scope_overlap(root, "Sources/MLXFastCoreExtras", "Sources/MLXFastCore"),
            None
        );
        assert_eq!(
            trusted_scope_overlap(root, "Package.swift.bak", "Package.swift"),
            None
        );
        // The EIGHTH roster entry `benchmark.json` is a FILE and overlaps exactly like the other
        // file entries: an exact (cased) spelling equals it, a name-prefix sibling does NOT.
        assert_eq!(
            trusted_scope_overlap(root, "benchmark.json", "benchmark.json"),
            Some("equals")
        );
        assert_eq!(
            trusted_scope_overlap(root, "BENCHMARK.JSON", "benchmark.json"),
            Some("equals")
        );
        assert_eq!(
            trusted_scope_overlap(root, "benchmark.json.bak", "benchmark.json"),
            None
        );
        // a genuinely disjoint participant path
        assert_eq!(
            trusted_scope_overlap(root, "Sources/MLXFastModel", "Sources/MLXFastCore"),
            None
        );
    }

    #[test]
    fn every_roster_entry_is_caught_verbatim() {
        let root = Path::new("/nonexistent-root-for-lexical-only");
        for scope in ROSTER_OF_EIGHT {
            assert_eq!(
                trusted_scope_overlap(root, scope, scope),
                Some("equals"),
                "roster entry {scope:?} must overlap itself"
            );
        }
    }
}
