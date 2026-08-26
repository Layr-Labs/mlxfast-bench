//! F1 CHANGE 1 — the TRUSTED-SIDE harness identity: a byte-compatible Rust port of the engine's
//! Swift `harnessHash()`.
//!
//! ## Why this lives in benchd (and must never come off the wire)
//!
//! `metrics.harness_hash` is a PUBLICATION-GATING value: the seam-3 overlay refuses to publish an
//! official score whose gates-score carries an empty or malformed harness identity
//! ([`crate::harness_hash`]'s consumer is `benchctl::overlay::validate_gates`). Until F1 every
//! benchd-authored `gates-score.json` sealed `harness_hash = ""` — benchd had NO source for the
//! value — so no official score could publish.
//!
//! The value could NOT be sourced from the engine. The worker is PARTICIPANT-BUILT, so a
//! wire-reported hash would be attacker-controlled: a candidate could report the digest of a
//! harness it is not running. The computation therefore has to happen trusted-side, over the
//! workspace benchd itself drives — which is what this module does.
//!
//! ## The authoritative algorithm
//!
//! Ported from
//! `mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastTrustedHarness/QwenRuntimePreflight.swift`
//! — `QwenRuntime.harnessHashRoots` (`:63-73`), `QwenRuntime.harnessHashRootFiles(baseDirectory:)`
//! (`:116-146`) and `QwenRuntime.harnessHash()` (`:75-101`). The port is byte-compatible: the same
//! bytes go into SHA256, in the same order, so a benchd-computed hash equals the one the Swift
//! harness would compute over the same tree at the same absolute location.
//!
//! Every semantic below was VERIFIED against a Swift transcription of those three symbols compiled
//! with the Xcode 6.3.3 toolchain and run against synthetic trees (see the module tests for the
//! resulting cross-implementation vector and its provenance) — not inferred from reading:
//!
//! 1. **The hashed path strings are ABSOLUTE.** `URL(fileURLWithPath: root)` resolves a relative
//!    root against the process CWD and `.path` renders it absolute, so the digest covers the
//!    workspace's absolute location as well as its bytes. A harness hash is therefore an identity
//!    of *this tree at this path on this box*, NOT a portable content digest. (Surprising, but it
//!    is what the reference does, and byte-compatibility is the requirement.)
//! 2. **Production (`baseDirectory: nil`, CWD-relative) and an explicit fully-resolved
//!    `baseDirectory` produce the SAME digest** — verified equal on the same tree. That is the
//!    bridge this port stands on: [`harness_hash`] takes the workspace root explicitly, and
//!    [`harness_hash_of_current_dir`] passes `getcwd()` (always fully symlink-resolved, exactly
//!    what Foundation prepends), reproducing the reference's production path.
//! 3. **Ordering is over the FULL absolute path strings, globally** — `files.sorted()` in Swift,
//!    AFTER collection across all nine roots, so files from different roots interleave and the
//!    root declaration order does NOT affect the digest. ASCII ordering (`Sources/…` before
//!    `TASK.md` before `benchmark.json`) — Swift's `String` `<` and Rust's byte-wise `Ord` agree on
//!    ASCII, which every root name and every real workspace path is.
//! 4. **The hashed byte format is bare concatenation**: for each path in sorted order,
//!    `sha256.update(path.utf8)` then `sha256.update(file_bytes)`. NO separator, NO length prefix,
//!    NO trailing newline. Rendered as 64 lowercase hex characters.
//! 5. **An UNREADABLE file is skipped whole** — Swift's `guard let data = try? Data(contentsOf:)
//!    else { continue }` drops the path bytes too, not just the contents.
//! 6. **Hidden entries are skipped** (`.skipsHiddenFiles`): a dot-file inside a root, and a
//!    dot-directory's entire subtree, contribute nothing.
//! 7. **Symlinks inside a root directory are excluded entirely** — verified: a symlink to a file is
//!    not a regular file to `URLResourceValues` (no-follow), and the enumerator does not descend
//!    into a symlink to a directory. A symlinked root DIRECTORY therefore contributes zero files
//!    while still satisfying the existence probe (`fileExists` follows). A symlinked root FILE, by
//!    contrast, IS hashed with its target's bytes — the non-directory branch appends
//!    unconditionally, with no regular-file check, and the content read follows the link.
//! 8. **FAIL-CLOSED on a missing root.** Swift throws `MLXFastError.missingFile` and
//!    `harnessHash()` turns that into a `fatalError` rather than seal a digest that quietly
//!    collapsed toward the empty-set value (the 92bdeccc `benchmark.sh` delete silently dropped
//!    9 roots to 8). This port returns `Err` with the SAME message text; the fail-closed action at
//!    the call site is to REFUSE THE RUN, never to seal `""` or a partial hash.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::hash::hex_lower;

/// The FIXED root set the harness hash covers — Swift `QwenRuntime.harnessHashRoots`
/// (`QwenRuntimePreflight.swift:63-73`), transcribed in ORDER.
///
/// The order does NOT affect the digest (the collected paths are sorted globally before hashing),
/// but it is pinned anyway because the reference pins it: `benchmark.sh` sits at index 4 and is a
/// TOP-LEVEL file, not benchd's facade. Deleting it — as engine commit 92bdeccc did — used to drop
/// the hashed set from 9 roots to 8 with no error; since the fail-closed ruling it hard-fails.
pub const HARNESS_HASH_ROOTS: [&str; 9] = [
    "Package.swift",
    "Sources",
    "Tests",
    "benchmark.json",
    "benchmark.sh",
    "setup.sh",
    "tools",
    "README.md",
    "TASK.md",
];

/// The Swift refusal message, verbatim: `MLXFastError.missingFile("harnessHash root missing from
/// disk: \(root)")` (`QwenRuntimePreflight.swift:125`). Kept byte-identical so a benchd refusal and
/// a reference refusal read the same in an operator's log.
fn missing_root_error(root: &str) -> String {
    format!("harnessHash root missing from disk: {root}")
}

/// Collect the regular files under [`HARNESS_HASH_ROOTS`], resolved against `workspace_root`, as
/// absolute path strings — the port of Swift `harnessHashRootFiles(baseDirectory:)`.
///
/// Returned UNSORTED, in the reference's collection order (root by root); [`harness_hash`] applies
/// the global sort. FAIL-CLOSED: a root that is absent from disk is an `Err`, never a silent skip.
///
/// `workspace_root` is used AS GIVEN — never canonicalized — because the path strings it produces
/// go into the digest verbatim. Callers wanting the reference's production semantics must pass a
/// fully-resolved absolute root; [`harness_hash_of_current_dir`] does exactly that.
pub fn harness_hash_root_files(workspace_root: &Path) -> Result<Vec<String>, String> {
    let mut files: Vec<String> = Vec::new();
    for root in HARNESS_HASH_ROOTS {
        let path = workspace_root.join(root);
        // Swift `FileManager.fileExists(atPath:isDirectory:)` — FOLLOWS symlinks, so a symlinked
        // root satisfies the probe. An absent root (or a broken symlink) is the fail-closed throw.
        let meta = fs::metadata(&path).map_err(|_| missing_root_error(root))?;
        if meta.is_dir() {
            // Swift `FileManager.enumerator(at:)` does NOT descend into a symlink to a directory,
            // so a symlinked root directory contributes ZERO files while still passing the probe
            // above (verified against the reference). `walk` reproduces that by testing the
            // no-follow type of every directory it is about to enter, this root included.
            walk_directory(&path, &mut files);
        } else {
            // Swift's non-directory branch appends UNCONDITIONALLY — no regular-file check — so a
            // symlinked root file is included here and hashed with its target's bytes (the content
            // read follows the link).
            files.push(path_string(&path));
        }
    }
    Ok(files)
}

/// Recursively append the regular files under `dir`, mirroring
/// `FileManager.enumerator(at:includingPropertiesForKeys:[.isRegularFileKey], options:
/// [.skipsHiddenFiles])` + the `values?.isRegularFile == true` filter.
///
/// Three reference behaviors are reproduced exactly:
/// * a directory that cannot be enumerated is SKIPPED, not an error (Swift's
///   `guard let enumerator … else { continue }`);
/// * hidden entries — leading `.` — are skipped, and a hidden directory's whole subtree with them;
/// * symlinks are excluded at every level: `file_type()` comes from `readdir` and does not follow,
///   so a symlink is neither `is_file()` nor `is_dir()` and falls through both arms.
fn walk_directory(dir: &Path, files: &mut Vec<String>) {
    // No-follow type check on the directory itself: entering through a symlink is what the
    // reference enumerator refuses to do.
    match fs::symlink_metadata(dir) {
        Ok(md) if md.is_dir() => {}
        _ => return,
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        // Not enumerable → skip this root's subtree. This is the reference's `continue`, and it is
        // a DISTINCT condition from the missing-root throw above (which the ruling targeted).
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        // `.skipsHiddenFiles`. macOS also honors the Finder "hidden" file flag; a leading dot is
        // the form that occurs in a source workspace and the only one reproduced here.
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk_directory(&path, files);
        } else if file_type.is_file() {
            files.push(path_string(&path));
        }
        // else: symlink (or a socket/fifo/device) — excluded, as `isRegularFile` excludes it.
    }
}

/// The path string that goes INTO the digest. Swift hashes `Data(path.utf8)` off `URL.path`, a
/// `String`; a non-UTF-8 byte path cannot be that string, and lossy rendering keeps this total
/// rather than dropping a file silently.
fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The DIGEST half of the algorithm, isolated from the filesystem: sort the collected absolute
/// paths, then for each one that reads, hash the path's UTF-8 bytes immediately followed by the
/// file's bytes.
///
/// `read` is the content source — production passes `fs::read`, so a file is read, hashed and
/// dropped one at a time exactly as Swift's `Data(contentsOf:)` loop does (nothing is buffered
/// whole). A `None` from `read` is the reference's unreadable-file `continue`: the path bytes are
/// dropped WITH the contents, not hashed alone.
///
/// Split out so the byte format and the ordering rule are testable with in-memory contents at
/// fixed absolute path strings — which is what makes a machine-independent cross-implementation
/// vector possible at all, given that the digest covers absolute paths. It is ONE code path: the
/// production caller and the vector test hash through this same function.
fn harness_hash_over<F>(mut paths: Vec<String>, read: F) -> String
where
    F: Fn(&str) -> Option<Vec<u8>>,
{
    // Swift `files.sorted()` — the GLOBAL sort over full absolute path strings, after collection
    // across all nine roots. Rust's byte-wise `Ord` agrees with Swift's `String` ordering on ASCII.
    paths.sort();
    let mut hasher = Sha256::new();
    for path in paths {
        let Some(bytes) = read(&path) else {
            continue;
        };
        hasher.update(path.as_bytes());
        hasher.update(&bytes);
    }
    hex_lower(&hasher.finalize())
}

/// The harness identity of the workspace rooted at `workspace_root`: 64 lowercase hex characters,
/// byte-compatible with the engine's Swift `harnessHash()` over the same tree at the same absolute
/// path.
///
/// FAIL-CLOSED: `Err` (naming the root) if any of the nine roots is missing from disk. A caller
/// must REFUSE THE RUN on `Err` — sealing `""` or a partial hash is exactly the dishonest digest
/// the reference aborts to avoid.
pub fn harness_hash(workspace_root: &Path) -> Result<String, String> {
    let files = harness_hash_root_files(workspace_root)?;
    Ok(harness_hash_over(files, |p| fs::read(p).ok()))
}

/// [`harness_hash`] over the process's current working directory — the reference's PRODUCTION
/// path, where the roots resolve CWD-relative.
///
/// `getcwd()` returns a fully symlink-resolved absolute path, which is precisely what Foundation
/// prepends to a relative `URL(fileURLWithPath:)`, so this reproduces `harnessHash()`'s own digest.
/// Verified: the reference's CWD path and an explicit fully-resolved base directory agree.
pub fn harness_hash_of_current_dir() -> Result<String, String> {
    let cwd: PathBuf = std::env::current_dir()
        .map_err(|e| format!("harnessHash: cannot resolve the current working directory: {e}"))?;
    harness_hash(&cwd)
}

/// Whether `s` is the shape a harness hash must have: 64 LOWERCASE hex characters. The same grid
/// the overlay's gates-input refusal enforces; exposed here so a producer can assert what it sealed
/// against the identical predicate its consumer will apply.
pub fn is_well_formed_harness_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // -----------------------------------------------------------------------
    // The CROSS-IMPLEMENTATION vector and its provenance
    // -----------------------------------------------------------------------

    /// The absolute base the cross-implementation vector was computed at. It is part of the digest
    /// (see the module doc, point 1), so it is pinned as data rather than discovered at test time —
    /// which is what makes the vector reproducible on any machine and any OS.
    const VECTOR_BASE: &str = "/private/tmp/benchd-hh-vector-v1";

    /// The SYNTHETIC fixture tree the vector covers, as `(path-relative-to-VECTOR_BASE, bytes)`.
    ///
    /// This is the exact set the reference COLLECTED from the on-disk tree — note what is absent:
    /// `Sources/.hidden.swift` and `Sources/.hiddendir/C.swift` existed on disk and are not here
    /// (`.skipsHiddenFiles`), and `Sources/empty/` contributed nothing. Deliberately listed in
    /// NON-sorted order so a port that forgets the global sort cannot pass.
    const VECTOR_FILES: [(&str, &str); 11] = [
        ("Package.swift", "package-swift-bytes\n"),
        ("Sources/A.swift", "sources-a\n"),
        ("Sources/nested/B.swift", "sources-nested-b\n"),
        ("Tests/T.swift", "tests-t\n"),
        ("benchmark.json", "{\"k\":1}\n"),
        ("benchmark.sh", "benchmark-sh\n"),
        ("setup.sh", "setup-sh\n"),
        ("tools/t.sh", "tools-t\n"),
        ("tools/sub/u.sh", "tools-sub-u\n"),
        ("README.md", "readme\n"),
        ("TASK.md", "task\n"),
    ];

    /// The vector: what the ENGINE'S SWIFT `harnessHash()` produces over [`VECTOR_FILES`] laid out
    /// at [`VECTOR_BASE`].
    ///
    /// PROVENANCE — this number was not copied from anywhere; it was produced for this test:
    /// `QwenRuntime.harnessHashRoots`, `.harnessHashRootFiles(baseDirectory:)` and `.harnessHash()`
    /// were transcribed VERBATIM out of
    /// `mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastTrustedHarness/QwenRuntimePreflight.swift`
    /// into a throwaway Swift script, compiled with the local Xcode toolchain
    /// (`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swiftc`, Apple Swift 6.3.3), and
    /// run against the tree above built at `VECTOR_BASE`. The script computed the same digest by
    /// BOTH reference paths — the production CWD path (`chdir(VECTOR_BASE)`, `baseDirectory: nil`)
    /// and the explicit-base test seam — which is the evidence for the claim in the module doc that
    /// [`harness_hash_of_current_dir`] reproduces production `harnessHash()`.
    ///
    /// A validated independent reimplementation of the same algorithm exists on the reference box
    /// at `$HOME/f1-recompute/harnesshash-recompute(.swift)` (proven equal to the Swift original
    /// over the real engine workspace, with stability and sensitivity checks). It is NOT the source
    /// of this vector — that box is not reachable from here — but it is the same algorithm proven
    /// the same way, independently.
    const VECTOR_HASH: &str = "edd0ac485d9e684770dd5866183bf4fee0166c25c73d989f240ed5f591fdbcc4";

    /// MUTATION PROOF (a) — the byte format AND the ordering rule, against the Swift number.
    ///
    /// Reds if the port reorders the files (the entries are fed unsorted, so dropping
    /// `paths.sort()` changes the digest), or alters the hashed byte format in any way: a
    /// separator between path and contents, a length prefix, hashing contents before the path,
    /// omitting the path, or upper-case hex rendering.
    #[test]
    fn rust_harness_hash_matches_the_swift_cross_implementation_vector() {
        let mut contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut paths: Vec<String> = Vec::new();
        for (rel, body) in VECTOR_FILES {
            let abs = format!("{VECTOR_BASE}/{rel}");
            contents.insert(abs.clone(), body.as_bytes().to_vec());
            paths.push(abs);
        }
        let got = harness_hash_over(paths, |p| contents.get(p).cloned());
        assert_eq!(
            got, VECTOR_HASH,
            "the Rust port must be BYTE-COMPATIBLE with the engine's Swift harnessHash()"
        );
        assert!(is_well_formed_harness_hash(&got));
    }

    /// The unreadable-file rule (module doc point 5): the path bytes are dropped WITH the contents.
    /// A port that hashed the path and then skipped only the body would produce a different digest
    /// from the reference on any tree with one unreadable file.
    #[test]
    fn an_unreadable_file_contributes_neither_its_path_nor_its_bytes() {
        let present = harness_hash_over(vec!["/w/a".to_string()], |p| {
            (p == "/w/a").then(|| b"A".to_vec())
        });
        let with_unreadable =
            harness_hash_over(vec!["/w/a".to_string(), "/w/b".to_string()], |p| {
                (p == "/w/a").then(|| b"A".to_vec())
            });
        assert_eq!(
            present, with_unreadable,
            "an unreadable file must be skipped WHOLE — path bytes included"
        );
    }

    /// The nine roots, in the reference's order, with `benchmark.sh` at index 4 — the property the
    /// engine's own `HarnessHashRootSetTests.swift` pins. A root added, removed or reordered here
    /// changes what benchd hashes and must be a deliberate, reviewed act.
    #[test]
    fn the_root_set_is_the_reference_nine_in_order() {
        assert_eq!(
            HARNESS_HASH_ROOTS,
            [
                "Package.swift",
                "Sources",
                "Tests",
                "benchmark.json",
                "benchmark.sh",
                "setup.sh",
                "tools",
                "README.md",
                "TASK.md",
            ]
        );
        assert_eq!(HARNESS_HASH_ROOTS[4], "benchmark.sh");
        assert_eq!(HARNESS_HASH_ROOTS.len(), 9);
    }

    // -----------------------------------------------------------------------
    // Filesystem behavior (prefix-independent: built under a real temp dir)
    // -----------------------------------------------------------------------

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("benchd-hh-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().expect("file has a parent")).expect("mkdir -p");
        fs::write(path, body).expect("write fixture file");
    }

    /// Build the SAME synthetic tree the cross-implementation vector covers, at `root` — including
    /// the entries the reference must SKIP (a hidden file, a hidden directory's subtree, an empty
    /// directory), so the collection rules are exercised, not just the digest.
    fn build_synthetic_tree(root: &Path) {
        for (rel, body) in VECTOR_FILES {
            write(&root.join(rel), body);
        }
        write(&root.join("Sources/.hidden.swift"), "sources-hidden-file\n");
        write(
            &root.join("Sources/.hiddendir/C.swift"),
            "sources-hidden-dir-c\n",
        );
        fs::create_dir_all(root.join("Sources/empty")).expect("mkdir empty");
    }

    fn sorted_relative(root: &Path, files: &[String]) -> Vec<String> {
        let prefix = format!("{}/", root.display());
        let mut rels: Vec<String> = files
            .iter()
            .map(|f| {
                f.strip_prefix(&prefix)
                    .unwrap_or_else(|| panic!("collected path {f} is not under {prefix}"))
                    .to_string()
            })
            .collect();
        rels.sort();
        rels
    }

    /// The COLLECTION half against the reference: the exact file set the Swift transcription
    /// collected from this tree — hidden file skipped, hidden directory's subtree skipped, empty
    /// directory contributing nothing.
    #[test]
    fn collection_matches_the_reference_file_set() {
        let root = tmp_root("collect");
        build_synthetic_tree(&root);
        let files = harness_hash_root_files(&root).expect("all nine roots present");
        let mut expected: Vec<String> = VECTOR_FILES.iter().map(|(r, _)| r.to_string()).collect();
        expected.sort();
        assert_eq!(sorted_relative(&root, &files), expected);
        let _ = fs::remove_dir_all(&root);
    }

    /// STABILITY — hashing the same unchanged tree twice yields the same digest.
    #[test]
    fn hashing_the_same_tree_twice_is_stable() {
        let root = tmp_root("stable");
        build_synthetic_tree(&root);
        let a = harness_hash(&root).expect("hash");
        let b = harness_hash(&root).expect("hash");
        assert_eq!(
            a, b,
            "the harness hash must be stable over an unchanged tree"
        );
        assert!(is_well_formed_harness_hash(&a));
        let _ = fs::remove_dir_all(&root);
    }

    /// SENSITIVITY — adding a file under a root MOVES the hash; removing it RETURNS the hash.
    /// Also covers content sensitivity (editing a hashed file moves it too).
    #[test]
    fn adding_a_file_under_a_root_moves_the_hash_and_removing_it_returns() {
        let root = tmp_root("sensitive");
        build_synthetic_tree(&root);
        let before = harness_hash(&root).expect("hash");

        let added = root.join("Sources/nested/Added.swift");
        write(&added, "added\n");
        let with_added = harness_hash(&root).expect("hash");
        assert_ne!(
            before, with_added,
            "a new file under a hashed root must move the harness hash"
        );

        fs::remove_file(&added).expect("remove added file");
        assert_eq!(
            before,
            harness_hash(&root).expect("hash"),
            "removing the added file must return the original harness hash"
        );

        fs::write(root.join("TASK.md"), "task-edited\n").expect("edit");
        assert_ne!(
            before,
            harness_hash(&root).expect("hash"),
            "editing a hashed file's bytes must move the harness hash"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A HIDDEN file added under a root does NOT move the hash — the reference skips it, so a port
    /// that walked hidden entries would diverge on any workspace with a `.DS_Store` in `Sources/`.
    #[test]
    fn a_hidden_file_added_under_a_root_does_not_move_the_hash() {
        let root = tmp_root("hidden");
        build_synthetic_tree(&root);
        let before = harness_hash(&root).expect("hash");
        write(&root.join("tools/.DS_Store"), "junk\n");
        write(&root.join("Tests/.hiddendir/deep.swift"), "deep\n");
        assert_eq!(
            before,
            harness_hash(&root).expect("hash"),
            "hidden entries must contribute nothing (.skipsHiddenFiles)"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// FAIL-CLOSED — EVERY one of the nine roots, removed one at a time, refuses BY NAME. The
    /// reference turns this into a `fatalError`; benchd turns it into a run refusal. Neither seals
    /// a digest over a collapsed root set.
    #[test]
    fn a_missing_root_refuses_by_name_for_every_root() {
        for root_name in HARNESS_HASH_ROOTS {
            let root = tmp_root(&format!("missing-{}", root_name.replace(['.', '/'], "_")));
            build_synthetic_tree(&root);
            let target = root.join(root_name);
            if target.is_dir() {
                fs::remove_dir_all(&target).expect("remove root dir");
            } else {
                fs::remove_file(&target).expect("remove root file");
            }
            let err = harness_hash(&root).expect_err("a missing root must refuse");
            assert_eq!(
                err,
                format!("harnessHash root missing from disk: {root_name}"),
                "the refusal must name the missing root, verbatim as the reference does"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// An EMPTY root directory is present-but-contributes-nothing — it must NOT be confused with a
    /// missing root (which refuses).
    #[test]
    fn an_empty_root_directory_is_present_not_missing() {
        let root = tmp_root("emptyroot");
        build_synthetic_tree(&root);
        fs::remove_dir_all(root.join("tools")).expect("drop tools");
        fs::create_dir_all(root.join("tools")).expect("recreate empty tools");
        let files = harness_hash_root_files(&root).expect("an empty root is still present");
        assert!(
            !sorted_relative(&root, &files)
                .iter()
                .any(|f| f.starts_with("tools/")),
            "an empty root contributes no files"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// SYMLINK EXCLUSION (module doc point 7), verified against the reference: a symlink to a file
    /// and a symlink to a directory inside a hashed root are both excluded, so neither moves the
    /// hash.
    #[cfg(unix)]
    #[test]
    fn symlinks_inside_a_root_are_excluded() {
        let root = tmp_root("symlink");
        build_synthetic_tree(&root);
        let before = harness_hash(&root).expect("hash");

        let real_dir = root.join("outside/subdir");
        write(&real_dir.join("R.swift"), "real-file\n");
        std::os::unix::fs::symlink(&real_dir, root.join("Sources/linkdir")).expect("symlink dir");
        std::os::unix::fs::symlink(
            root.join("Package.swift"),
            root.join("tools/linkfile.swift"),
        )
        .expect("symlink file");
        std::os::unix::fs::symlink(root.join("nowhere"), root.join("Tests/brokenlink"))
            .expect("symlink broken");

        assert_eq!(
            before,
            harness_hash(&root).expect("hash"),
            "symlinks inside a hashed root are excluded entirely"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A symlinked root DIRECTORY passes the existence probe and contributes ZERO files — the
    /// reference's `fileExists` follows, its enumerator does not. Pinned because the two halves
    /// disagreeing is exactly the kind of quirk a port silently "fixes" into a divergence.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_directory_is_present_but_contributes_nothing() {
        let root = tmp_root("rootlink");
        build_synthetic_tree(&root);
        fs::rename(root.join("tools"), root.join("tools-real")).expect("rename");
        std::os::unix::fs::symlink(root.join("tools-real"), root.join("tools")).expect("symlink");
        let files = harness_hash_root_files(&root).expect("a symlinked root still exists");
        assert!(
            !sorted_relative(&root, &files)
                .iter()
                .any(|f| f.starts_with("tools")),
            "a symlinked root directory contributes no files"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The cross-implementation vector END TO END, over a REAL tree on disk — collection and
    /// digest together, not the digest half alone.
    ///
    /// The digest covers absolute paths, so this can only run where the fixture can be built at
    /// exactly [`VECTOR_BASE`]; `/private/tmp` is macOS's real `/tmp`, which is where the Swift
    /// transcription computed [`VECTOR_HASH`]. Gated to macOS because that is precisely the scope
    /// of the claim: byte-compatibility with the engine's Swift `harnessHash()`, which is a
    /// macOS-only binary running on a macOS ranked box. The portable half of the proof
    /// (`rust_harness_hash_matches_the_swift_cross_implementation_vector` +
    /// `collection_matches_the_reference_file_set`) runs everywhere.
    #[cfg(target_os = "macos")]
    #[test]
    fn end_to_end_over_a_real_tree_matches_the_swift_vector() {
        let root = PathBuf::from(VECTOR_BASE);
        let _ = fs::remove_dir_all(&root);
        build_synthetic_tree(&root);
        let got = harness_hash(&root).expect("all nine roots present");
        assert_eq!(
            got, VECTOR_HASH,
            "walking the real tree must reproduce the Swift digest, collection and format together"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The production entry point agrees with the explicit-root one over the same tree. This is the
    /// Rust side of the reference equality the vector's provenance records (CWD path == explicit
    /// fully-resolved base). `current_dir()` is process-global, so this asserts the composition
    /// rather than mutating it.
    #[test]
    fn current_dir_entry_point_hashes_the_current_directory() {
        let cwd = std::env::current_dir().expect("cwd");
        // Whatever the crate's CWD is, the two entry points must agree — including agreeing on the
        // refusal when the CWD is not a harness workspace (which it is not, under cargo).
        assert_eq!(harness_hash_of_current_dir(), harness_hash(&cwd));
    }

    #[test]
    fn well_formed_predicate_is_64_lowercase_hex() {
        assert!(is_well_formed_harness_hash(&"a".repeat(64)));
        assert!(is_well_formed_harness_hash(VECTOR_HASH));
        assert!(!is_well_formed_harness_hash(""));
        assert!(!is_well_formed_harness_hash(&"a".repeat(63)));
        assert!(!is_well_formed_harness_hash(&"a".repeat(65)));
        assert!(!is_well_formed_harness_hash(&"A".repeat(64)));
        assert!(!is_well_formed_harness_hash(&"g".repeat(64)));
    }
}
