//! WIRE-1 item 1b — the AUTHORITATIVE write-outside-editablePaths gate, homed in benchd's
//! measure-job as a CANDIDATE-vs-BASELINE divergence refusal.
//!
//! David's RULING (WIRE-1): benchd is authoritative for the write-outside check too — any file
//! CHANGED, ADDED, or DELETED between the trusted `--baseline` reference and the `--candidate`
//! submission that is NOT under the contract's `editablePaths` is refused (die-class, pre-GPU). The
//! in-repo dispatch script may ALSO run this early as defence-in-depth, but never as authority; that
//! DiD stage is DEFERRED (the ranked dispatch script is not provisioned yet) and tracked separately.
//!
//! REFERENCE SEMANTICS (ported, not copied). The reference is the engine's diff-level surface
//! allowlist `\.github/scripts/enforce-modifiable-surface.sh` at
//! the qwen-era engine fork `@736781ea` (a re-implementation of
//! `Layr-Labs/qwen-3.8-mtp-challenge@bfab0de`, read-only): it reads `editablePaths` from the BASE
//! (trusted) contract — never the submission's own — and refuses any `git diff --name-only BASE
//! HEAD` path that is neither an allowed path nor inside one. Here the "diff" is computed by
//! comparing the two workspace trees directly (benchd has both on disk, not two commits).
//!
//! TWO DELIBERATE benchd HARDENINGS over the shell reference, both directed by the ruling:
//!   1. The membership relation is #147's trusted-scope discipline — CASEFOLD (the ranked box is
//!      macOS/APFS, case-INSENSITIVE) plus DEVICE:INODE identity — not the shell's byte-exact
//!      prefix test. A path that reaches an editable dir only by case or by a symlink/hardlink
//!      spelling is still recognised as inside it. The helpers are REUSED from `trusted_scope`
//!      (`normalize`/`prefixes`/`same_file`), never re-derived.
//!   2. GENERATED, GITIGNORED trees (`.git`, `.build`, `.build-worker`, `weights`) are held out of
//!      the comparison: they are not part of the reviewable SOURCE surface (the shell diffs
//!      git-TRACKED files, so none of them ever appears there, and a legit candidate's rebuilt
//!      binaries and independently-transformed weights necessarily differ from the baseline's).
//!      Each is covered by a STRONGER content-aware gate — the binaries by the separate
//!      content-pin gate (`pin-trusted-harness.sh`), the weights by the target quantization bind
//!      (LOADED geometry) and the correctness gate (emitted tokens). This gate owns SOURCE only.
//!
//! The forbidden-surface arm of the shell (`benchd`, `.gitmodules`) is NOT re-homed here: that is
//! the trusted-scope roster's job (#147 / the roster follow-up), enforced against the manifest's
//! DECLARATION. This gate enforces the realized DIFF against that same declared surface.
//!
//! TWO REFERENCE MODES (David ruling 2026-08-27). The tree-diff above is the ORIGINAL mode and stays
//! the default; [`verify_no_write_outside_editable_from_git`] is the ruled mode, selected by
//! `measure-job --write-gate-base <SHA>`, which judges the SUBMISSION'S OWN COMMITTED DIFF
//! (`<SHA>..HEAD`, its fork point from harness main) and never looks at the staged workspace at all.
//! See that function for why the tree-diff mode coupled submission validity to box-staging freshness.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::trusted_scope::{self, normalize, prefixes, same_file};

/// Top-level path segments held OUT of the divergence comparison — VCS metadata and the
/// GENERATED, GITIGNORED outputs each workspace produces for itself. A path whose FIRST segment is
/// one of these never contributes a divergence (see hardening 2). Kept tiny and explicit so the
/// boundary is auditable.
///
/// The common property, and the ONLY one that admits a segment here: the tree is (a) gitignored,
/// (b) produced INDEPENDENTLY by each workspace from gated inputs, and therefore (c) expected to
/// differ between two legitimate workspaces for reasons that have nothing to do with the reviewable
/// source surface. A tree that fails any of the three does NOT belong.
///
/// `.build-worker` JOINED THE LIST from the 2026-08-26 box evidence run. The engine builds the
/// SCORED participant worker into its own root, separate from `.build`, precisely so the trusted
/// harness and the participant worker cannot share build products (the bench-560 isolation). That
/// root is gitignored and is a BUILD OUTPUT by exactly the same argument `.build` is here — but it
/// was not excluded, so the documented ranked path produced **2474 differing files** on a run where
/// nothing about the source diverged at all, and the gate refused a legitimate candidate. The
/// binaries built into it remain bound by the separate content-pin gate.
///
/// `weights/` JOINED FROM THE SAME RUN (reviewer ruling, 2026-08-26). It is the TRANSFORMED weights
/// tree: gitignored (`weights/*`), and each workspace runs its own `mlxfast-swift transform` over
/// the same pinned reference checkpoint, so the candidate and baseline produce it independently.
/// The evidence run's inventory listed it as only-in-candidate, and the organizer had to APFS-clone
/// the tree candidate → baseline to get past this gate. That mirror was an INTERIM WORKAROUND and
/// is retired by this exclusion.
///
/// WEIGHTS ARE LOAD-BEARING MODEL CONTENT, so excluding them needs more than the false-positive
/// argument the two build roots rest on. The ruling's rationale, on record: this gate's coverage of
/// `weights/` was REDUNDANT, and the two properties it appeared to protect are each held by a
/// STRONGER, CONTENT-AWARE gate that reads what was actually loaded rather than what is on disk:
///
/// * TAMPERING WITH THE WEIGHTS is caught by the target quantization bind, which validates the
///   LOADED geometry against the pinned per-path pins at startup and re-validates before every
///   measured window. Proven on the box, not argued: R6i baked a requant INTO the transform, and
///   the bind refused it at startup — a disk-level diff was never what stood between that candidate
///   and a score.
/// * DIVERGENT OUTPUT from whatever weights were loaded is caught by the correctness gate (R5),
///   which compares emitted tokens against the pinned goldens.
///
/// So the divergence gate was contributing false positives on this tree and no unique protection.
///
/// WHAT THIS EXCLUSION ASSUMES, stated so it can be checked rather than trusted: that two clean
/// workspaces at the same gated source, over the same pinned reference checkpoint, transform to
/// BYTE-IDENTICAL `weights/`. If they do not, the transform has an UNGATED INPUT — and that is a
/// bug in the transform, not a reason to keep a disk-diff gate that would only report it as an
/// anonymous file count. Verifying it needs the real 26B checkpoint, so it is a NAMED ITEM in the
/// box re-run checklist (byte-compare both trees BEFORE anything else; if they differ, STOP), not
/// something these unit tests can stand in for.
const EXCLUDED_TOP_SEGMENTS: [&str; 4] = [".git", ".build", ".build-worker", "weights"];

fn is_excluded(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    EXCLUDED_TOP_SEGMENTS.contains(&first)
}

/// True when repo-relative `rel` is WITHIN some declared editable path — equal to it, or inside it —
/// under `root`, using the #147 discipline: casefold lexical arms plus device:inode identity for the
/// spellings folding does not reach. `rel` is always a file leaf here (the walk yields regular
/// files), so only the equals / is-inside directions apply; the "contains" direction (rel is an
/// ancestor of the editable path) cannot occur and is not tested.
fn path_within_editable(root: &Path, rel: &str, editable_paths: &[String]) -> bool {
    let a = normalize(rel).to_lowercase();
    if a.is_empty() {
        return false;
    }
    for entry in editable_paths {
        let e = normalize(entry).to_lowercase();
        if e.is_empty() {
            continue; // a root-ish editable entry is refused by trusted_scope; grant nothing here.
        }
        // equals, or rel is inside the editable dir (separator-anchored, so `Foo` never matches
        // `FooBar`).
        if a == e || a.starts_with(&format!("{e}/")) {
            return true;
        }
        // device:inode — an ANCESTOR DIRECTORY of rel is an inode-identical spelling of the editable
        // path (rel sits inside a symlinked/hardlinked editable dir). The leaf itself is deliberately
        // NOT resolved: a symlink placed at an outside path is judged by its LOCATION (the write is
        // into the dir the link LIVES in); following it to its target would let a symlink injected
        // into a trusted dir self-grant membership merely by pointing at the editable surface.
        let ancestors = prefixes(rel);
        let dir_ancestors = ancestors.split_last().map(|(_, rest)| rest).unwrap_or(&[]);
        for ancestor in dir_ancestors {
            if same_file(root, ancestor, entry) {
                return true;
            }
        }
    }
    false
}

/// A type-tagged content digest for one tracked entry. The leading tag byte distinguishes a regular
/// file (`0`) from a SYMLINK (`1`) so a file↔symlink swap of coincidentally-equal content still
/// registers as a change — this mirrors git, which stores a file as a mode-100644 blob and a symlink
/// as a mode-120000 blob whose content IS the link target, so the two are never equal even with the
/// same bytes.
fn digest(kind: u8, bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([kind]);
    h.update(bytes);
    h.finalize().into()
}

/// Every REGULAR file AND SYMLINK under `root`, keyed by repo-relative slash path, valued by a
/// type-tagged content digest. A symlink is tracked (not skipped) and NEVER traversed: git records it
/// as a mode-120000 blob whose content is the link target, so a candidate that ADDS a symlink at a
/// path outside `editablePaths` (a source-injection vector into a trusted dir) must register as an
/// added path here — exactly as the ported `git diff --name-only` reference would report it. Other
/// non-regular entries (fifos, sockets, devices) are skipped: git cannot track them, so they are not
/// diff surface. The excluded top segments (`.git`/`.build`) are skipped. Fail-closed: an unreadable
/// entry is a hard error, never a silently-dropped divergence.
fn hash_tree(root: &Path) -> Result<BTreeMap<String, [u8; 32]>, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("write-divergence: cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("write-divergence: dir entry error: {e}"))?;
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("write-divergence: strip_prefix: {e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            if is_excluded(&rel) {
                continue;
            }
            let md = std::fs::symlink_metadata(&path)
                .map_err(|e| format!("write-divergence: lstat {}: {e}", path.display()))?;
            let ft = md.file_type();
            if ft.is_symlink() {
                // The link target IS the tracked content (mode-120000 blob). Do NOT follow it.
                let target = std::fs::read_link(&path)
                    .map_err(|e| format!("write-divergence: readlink {}: {e}", path.display()))?;
                out.insert(rel, digest(1, target.to_string_lossy().as_bytes()));
            } else if ft.is_dir() {
                stack.push(path);
            } else if md.is_file() {
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("write-divergence: read {}: {e}", path.display()))?;
                out.insert(rel, digest(0, &bytes));
            }
        }
    }
    Ok(out)
}

/// The declared modifiable surface, read from the TRUSTED manifest bytes (never the candidate's
/// own), fail-closed. Shared by BOTH reference modes so they can never disagree about what the
/// allowlist is: the tree-diff mode ([`verify_no_write_outside_editable`]) and the ruled fork-point
/// mode ([`verify_no_write_outside_editable_from_git`]) enforce the SAME declaration and differ only
/// in how the diff is obtained.
fn parse_usable_editable_paths(manifest_bytes: &[u8]) -> Result<Vec<String>, String> {
    let surface = trusted_scope::EditableSurface::parse(manifest_bytes)?;
    let editable = surface.editable_paths;
    if editable.iter().all(|e| normalize(e).is_empty()) {
        // Empty (or root-only) editablePaths gives this gate no allowlist — fail loud, as the shell
        // reference does, rather than reject every divergent file while naming no surface.
        return Err(
            "write-divergence: benchmark.json lists no usable editablePaths — the modifiable \
             surface is undefined"
                .to_string(),
        );
    }
    Ok(editable)
}

/// True when `sha` is a FULL 40-character ASCII-hex object name. The `--write-gate-base` value is
/// operator/CI-supplied and is handed to `git` as an argument, so it is validated to this shape
/// BEFORE it ever reaches a `Command` — a short sha, a ref name, a `--flag` spelling or a pathspec
/// is refused rather than resolved, which keeps the base UNAMBIGUOUS (no rev-parse guessing) and
/// keeps argument-injection off the table. Shared by the growth bound in
/// [`crate::byte_budget::verify_growth_over_from_git`] so both halves of the gate accept exactly the
/// same base spellings.
pub(crate) fn is_full_commit_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// AUTHORITATIVE write-outside-editablePaths refusal. Reads `editablePaths` from the trusted
/// `--baseline` manifest bytes (never the candidate's own), computes the candidate-vs-baseline file
/// divergence, and returns `Err` (a die-class refusal) naming the FIRST path that changed, was added
/// or was deleted OUTSIDE the declared editable surface. Fail-closed on a malformed manifest or a
/// manifest with no usable `editablePaths`.
pub fn verify_no_write_outside_editable(
    manifest_bytes: &[u8],
    baseline_root: &Path,
    candidate_root: &Path,
) -> Result<(), String> {
    let editable = parse_usable_editable_paths(manifest_bytes)?;

    let base = hash_tree(baseline_root)?;
    let cand = hash_tree(candidate_root)?;

    // Deterministic scan order: union of both key sets, sorted (BTreeMap keys are already sorted).
    let mut keys: Vec<&String> = base.keys().chain(cand.keys()).collect();
    keys.sort();
    keys.dedup();

    for rel in keys {
        let in_base = base.get(rel);
        let in_cand = cand.get(rel);
        let (kind, resolve_root) = match (in_base, in_cand) {
            (Some(b), Some(c)) if b != c => ("changed", candidate_root),
            (None, Some(_)) => ("added", candidate_root),
            (Some(_), None) => ("deleted", baseline_root),
            _ => continue, // identical content, or absent from both — no divergence.
        };
        if !path_within_editable(resolve_root, rel, &editable) {
            return Err(format!(
                "write-divergence: {rel:?} was {kind} outside the modifiable surface \
                 (editablePaths) — a submission may only change files it declared editable"
            ));
        }
    }
    Ok(())
}

/// The RULED reference mode (David, 2026-08-27): judge the SUBMISSION'S OWN COMMITTED DIFF —
/// `<base_sha>..HEAD` in the candidate workspace, where `base_sha` is the submission's FORK POINT
/// from harness main — instead of tree-diffing the box-staged `--baseline` workspace. The staged
/// workspace keeps exactly one job under this mode: it is the PAIRED TIMING baseline, and it has no
/// say in whether a submission is well-formed.
///
/// WHY (the failure class this kills). The tree-diff mode compares the candidate against whatever
/// snapshot happens to be staged on the box, so submission validity moved whenever the ORGANIZER
/// moved harness main. On 2026-08-27 an organizer commit to the gemma engine repo's main deleted
/// `Tests/MLXFastTests/Gemma4SubmissionDraftDepthTests.swift`; every submission cut afterwards then
/// tree-diverged from the stale staged snapshot at a path no participant had touched, and benchd
/// refused all of them pre-GPU ("... was deleted outside the modifiable surface"). A fork-point diff
/// cannot express that failure: organizer commits sit BELOW the fork point, so they are common
/// history and simply do not appear in `<base_sha>..HEAD`. What DOES appear is exactly the
/// participant's own work — submission branches are built from `editablePaths`-only archives, so
/// participant content can only arrive as commits on top of the fork point.
///
/// FAIL-CLOSED, always. A base that is not a full 40-hex object name, a `git` that cannot be spawned
/// (git absent), a `git diff` that exits non-zero (base unknown to this repo, `candidate_root` not a
/// work tree), or a `-z` stream that does not parse are each an `Err` — never a silent pass. Nothing
/// here falls open on an unreadable history.
pub fn verify_no_write_outside_editable_from_git(
    manifest_bytes: &[u8],
    candidate_root: &Path,
    base_sha: &str,
) -> Result<(), String> {
    let editable = parse_usable_editable_paths(manifest_bytes)?;
    if !is_full_commit_sha(base_sha) {
        return Err(format!(
            "write-divergence: --write-gate-base {base_sha:?} is not a 40-character hex commit sha \
             — the fork point must be named exactly, never resolved from a ref or a short sha"
        ));
    }

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(candidate_root)
        .args(["diff", "--name-status", "--no-renames", "-z"])
        .arg(base_sha)
        .arg("HEAD")
        .output()
        .map_err(|e| {
            format!(
                "write-divergence: cannot run git in {}: {e} — --write-gate-base needs a git work \
                 tree and a git executable",
                candidate_root.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "write-divergence: git diff {base_sha}..HEAD failed in {}: {}",
            candidate_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stream = String::from_utf8(output.stdout).map_err(|e| {
        format!("write-divergence: git diff --name-status -z emitted non-UTF-8 output: {e}")
    })?;

    // `--name-status -z` emits STATUS NUL PATH NUL, repeating, with no trailing record. `-z` also
    // means paths are RAW (never C-quoted), which is why this mode can compare them byte-for-byte
    // against the declared surface. `--no-renames` keeps the record shape at one path per status:
    // a rename is reported as its delete + its add, so no `R`/`C` record with a second path (and
    // therefore no third field) can appear.
    let mut fields = stream.split('\0');
    while let Some(status) = fields.next() {
        if status.is_empty() {
            // The stream is NUL-TERMINATED, so the final split always yields one empty tail field
            // (and an EMPTY diff yields only that). Anything non-empty after it is not a shape this
            // parser understands, so refuse instead of guessing where the record boundary went.
            if fields.any(|rest| !rest.is_empty()) {
                return Err(format!(
                    "write-divergence: git diff --name-status -z stream is malformed (trailing \
                     data after the final record) for {base_sha}..HEAD"
                ));
            }
            break;
        }
        let path = match fields.next() {
            Some(path) if !path.is_empty() => path,
            _ => {
                return Err(format!(
                    "write-divergence: git diff --name-status -z stream is malformed (status \
                     {status:?} with no following path) for {base_sha}..HEAD"
                ))
            }
        };
        // T = the type changed (regular file ↔ symlink ↔ gitlink). That is a CONTENT change to the
        // reviewable surface exactly as a byte edit is — the tree-diff mode's type-tagged digest
        // calls the same swap "changed" — so the two modes report it identically.
        let kind = match status {
            "A" => "added",
            "M" | "T" => "changed",
            "D" => "deleted",
            other => {
                return Err(format!(
                    "write-divergence: git diff --name-status reported the unhandled status \
                     {other:?} for {path:?} — refusing rather than guessing what it changed"
                ))
            }
        };
        // The SAME membership relation as the tree-diff mode, deliberately reused rather than
        // re-derived: it carries the #147 casefold (the ranked box is APFS, case-INSENSITIVE) plus
        // the device:inode arms. For a DELETED path the leaf is absent from the work tree, so the
        // inode arms simply find nothing and the lexical arms decide — which is the correct answer,
        // because a deletion is judged by WHERE THE FILE WAS.
        //
        // NOTE the deliberate divergence: the `.git`/`.build` EXCLUDED_TOP_SEGMENTS hold-out is NOT
        // applied in this mode. Those exclusions exist because the tree-diff mode compares WORK
        // TREES, where VCS metadata and gitignored SwiftPM output necessarily differ; a COMMITTED
        // path under `.git`/`.build` cannot arise legitimately, so it must refuse here. Untracked
        // box-staged assets never appear in a committed diff at all, which is the whole point of
        // this mode.
        if !path_within_editable(candidate_root, path, &editable) {
            return Err(format!(
                "write-divergence: {path:?} was {kind} outside the modifiable surface \
                 (editablePaths) — a submission may only change files it declared editable"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ed-div-{tag}-{}-{}",
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

    /// A baseline + a candidate that mirrors it, differing ONLY inside editablePaths, passes.
    #[test]
    fn edits_confined_to_editable_paths_pass() {
        let base = tmp("ok-base");
        let cand = tmp("ok-cand");
        for root in [&base, &cand] {
            write(root, "Package.swift", b"// pinned\n");
            write(root, "Sources/MLXFastCore/Timer.swift", b"trusted\n");
            write(root, "benchmark.json", b"{}"); // identical contract in both
        }
        // candidate edits ONLY inside the editable dir + a rebuilt (excluded) binary
        write(
            &cand,
            "Sources/MLXFastModel/Head.swift",
            b"my candidate head\n",
        );
        write(&base, "Sources/MLXFastModel/Head.swift", b"baseline head\n");
        write(&cand, ".build/release/worker", b"DIFFERENT BINARY");
        write(&base, ".build/release/worker", b"baseline binary");
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        assert_eq!(verify_no_write_outside_editable(m, &base, &cand), Ok(()));
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    #[test]
    fn a_changed_file_outside_the_surface_is_refused() {
        let base = tmp("chg-base");
        let cand = tmp("chg-cand");
        write(&base, "Sources/MLXFastCore/Timer.swift", b"trusted\n");
        write(&cand, "Sources/MLXFastCore/Timer.swift", b"TAMPERED\n");
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        let e = verify_no_write_outside_editable(m, &base, &cand).unwrap_err();
        assert!(e.contains("changed") && e.contains("Timer.swift"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    #[test]
    fn an_added_file_outside_the_surface_is_refused() {
        let base = tmp("add-base");
        let cand = tmp("add-cand");
        write(&cand, "tools/evil.sh", b"#!/bin/sh\n");
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        let e = verify_no_write_outside_editable(m, &base, &cand).unwrap_err();
        assert!(e.contains("added") && e.contains("evil.sh"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    #[test]
    fn a_deleted_file_outside_the_surface_is_refused() {
        let base = tmp("del-base");
        let cand = tmp("del-cand");
        write(&base, "Package.resolved", b"pins\n");
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        let e = verify_no_write_outside_editable(m, &base, &cand).unwrap_err();
        assert!(
            e.contains("deleted") && e.contains("Package.resolved"),
            "{e}"
        );
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    #[test]
    fn casefolded_editable_dir_still_admits_the_edit() {
        let base = tmp("case-base");
        let cand = tmp("case-cand");
        write(&base, "Sources/MLXFastModel/Head.swift", b"a\n");
        write(&cand, "Sources/MLXFastModel/Head.swift", b"b\n");
        // editablePaths spelled with a different case — APFS names the same dir.
        let m = br#"{"editablePaths":["sources/mlxfastmodel"]}"#;
        assert_eq!(verify_no_write_outside_editable(m, &base, &cand), Ok(()));
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    #[test]
    fn empty_editable_paths_fails_closed() {
        let base = tmp("empty-base");
        let cand = tmp("empty-cand");
        assert!(
            verify_no_write_outside_editable(br#"{"editablePaths":[]}"#, &base, &cand).is_err()
        );
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    #[test]
    fn excluded_build_and_git_never_diverge() {
        let base = tmp("excl-base");
        let cand = tmp("excl-cand");
        write(&cand, ".build/x", b"1");
        write(&cand, ".git/config", b"2");
        write(&base, ".build/x", b"DIFF");
        // No editable edits at all — only excluded churn — must pass.
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        assert_eq!(verify_no_write_outside_editable(m, &base, &cand), Ok(()));
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    /// `.build-worker` — the SCORED participant worker's own build root, kept separate from
    /// `.build` by the bench-560 isolation so the trusted harness and the participant worker cannot
    /// share build products.
    ///
    /// FROM THE BOX. The 2026-08-26 evidence run followed the DOCUMENTED ranked path and this gate
    /// reported **2474 differing files** — every one of them a rebuilt artifact under
    /// `.build-worker`, on a run where no source diverged at all. A false positive that refuses
    /// every legitimate candidate is not a conservative gate; it is a gate that cannot be used.
    ///
    /// The NEGATIVE CONTROL is the whole point and is asserted in the same test: excluding a build
    /// root must not excuse a real source divergence sitting next to it. A candidate that edits
    /// outside `editablePaths` is still refused, and the refusal still names the offending path —
    /// so this cannot be read as "the gate stopped looking".
    #[test]
    fn build_worker_output_is_excluded_but_real_divergence_still_refuses() {
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;

        // (a) POSITIVE — the box's exact shape: the two workspaces' worker build roots differ,
        // both trees carry the SAME source, and nothing else changed. Verified.
        let base = tmp("bw-base");
        let cand = tmp("bw-cand");
        write(
            &cand,
            ".build-worker/release/mlxfast-runtime-worker",
            b"CANDIDATE BINARY",
        );
        write(
            &cand,
            ".build-worker/release/mlx.metallib",
            b"cand metallib",
        );
        write(
            &base,
            ".build-worker/release/mlxfast-runtime-worker",
            b"baseline binary",
        );
        write(
            &base,
            ".build-worker/release/mlx.metallib",
            b"base metallib",
        );
        // A file only the candidate has, still under the excluded root — an added build product is
        // as ordinary as a changed one.
        write(&cand, ".build-worker/release/extra.o", b"only in candidate");
        // Identical source on both sides, inside AND outside the editable dir.
        write(&cand, "Sources/MLXFastModel/Model.swift", b"same source");
        write(&base, "Sources/MLXFastModel/Model.swift", b"same source");
        write(
            &cand,
            "Sources/MLXFastHarness/Trusted.swift",
            b"trusted, untouched",
        );
        write(
            &base,
            "Sources/MLXFastHarness/Trusted.swift",
            b"trusted, untouched",
        );
        assert_eq!(
            verify_no_write_outside_editable(m, &base, &cand),
            Ok(()),
            "a differing .build-worker must not diverge — this is the 2474-file false positive"
        );

        // (b) NEGATIVE CONTROL — same excluded churn, PLUS one real edit to a trusted source file.
        // The gate must still refuse, and must name the source path rather than the build root.
        write(&cand, "Sources/MLXFastHarness/Trusted.swift", b"TAMPERED");
        let err = verify_no_write_outside_editable(m, &base, &cand)
            .expect_err("a real source divergence must still refuse");
        assert!(
            err.contains("Sources/MLXFastHarness/Trusted.swift"),
            "the refusal must name the offending source path: {err}"
        );
        assert!(
            !err.contains(".build-worker"),
            "the excluded build root must not appear in the refusal: {err}"
        );
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    /// `weights/` — the TRANSFORMED weights tree (reviewer ruling, 2026-08-26).
    ///
    /// FROM THE BOX. The evidence run's inventory listed `weights/` as only-in-candidate, and the
    /// organizer had to APFS-clone the tree candidate → baseline to get past this gate. That mirror
    /// was an interim workaround; this exclusion retires it.
    ///
    /// Unlike the two build roots, weights are LOAD-BEARING MODEL CONTENT, so the exclusion rests
    /// on redundancy rather than irrelevance: tampering is caught by the target quantization bind,
    /// which reads the LOADED geometry (R6i baked a requant into the transform and the bind refused
    /// it at startup), and divergent output by the correctness gate (R5). Neither of those lives in
    /// this file, which is exactly why this test's job is to keep the NEGATIVE alive — to show that
    /// excluding weights did not quietly stop the gate looking at everything else.
    ///
    /// The determinism premise (two clean workspaces transform to byte-identical `weights/`) needs
    /// the real 26B checkpoint and is a named item in the box re-run checklist, NOT something this
    /// test claims.
    #[test]
    fn weights_tree_is_excluded_but_real_divergence_still_refuses() {
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        let base = tmp("w-base");
        let cand = tmp("w-cand");

        // (a) POSITIVE — the box's exact shape. Both workspaces transformed independently, so the
        // trees differ in content, in file set, and in both directions at once.
        write(
            &cand,
            "weights/model-00001-of-00003.safetensors",
            b"CANDIDATE SHARD BYTES",
        );
        write(
            &base,
            "weights/model-00001-of-00003.safetensors",
            b"baseline shard bytes",
        );
        write(&cand, "weights/config.json", br#"{"from":"candidate"}"#);
        write(&base, "weights/config.json", br#"{"from":"baseline"}"#);
        // Only-in-candidate and only-in-baseline, the two directions the inventory reported.
        write(
            &cand,
            "weights/extra-candidate-only.safetensors",
            b"only here",
        );
        write(
            &base,
            "weights/stale-baseline-only.safetensors",
            b"only there",
        );
        // Identical source on both sides, inside AND outside the editable dir.
        write(&cand, "Sources/MLXFastModel/Model.swift", b"same source");
        write(&base, "Sources/MLXFastModel/Model.swift", b"same source");
        write(
            &cand,
            "Sources/MLXFastHarness/Trusted.swift",
            b"trusted, untouched",
        );
        write(
            &base,
            "Sources/MLXFastHarness/Trusted.swift",
            b"trusted, untouched",
        );
        assert_eq!(
            verify_no_write_outside_editable(m, &base, &cand),
            Ok(()),
            "an independently-transformed weights/ must not diverge — this is the APFS-clone \
             workaround being retired"
        );

        // (b) NEGATIVE CONTROL — the same weights churn, PLUS one real edit to a trusted source
        // file. THE point of this half: excluding a load-bearing tree must not be readable as "the
        // gate stopped looking". It must still refuse, and still name the SOURCE path.
        write(&cand, "Sources/MLXFastHarness/Trusted.swift", b"TAMPERED");
        let err = verify_no_write_outside_editable(m, &base, &cand)
            .expect_err("a real source divergence must still refuse");
        assert!(
            err.contains("Sources/MLXFastHarness/Trusted.swift"),
            "the refusal must name the offending source path: {err}"
        );
        assert!(
            !err.contains("weights"),
            "the excluded weights tree must not appear in the refusal: {err}"
        );
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    /// The excluded set is EXACTLY the four generated/VCS roots — nothing has silently joined it.
    ///
    /// A gate whose exclusion list can grow unnoticed is the failure mode this whole file guards
    /// against, and every entry here was added under a ruling. A fifth would need one too.
    #[test]
    fn the_excluded_set_is_exactly_the_four_ruled_roots() {
        assert_eq!(
            EXCLUDED_TOP_SEGMENTS,
            [".git", ".build", ".build-worker", "weights"]
        );
        // And the exclusion is a WHOLE-SEGMENT match, not a prefix one: a source directory whose
        // name merely STARTS with an excluded segment is still compared.
        assert!(is_excluded("weights/model.safetensors"));
        assert!(!is_excluded("weights-notes/model.safetensors"));
        assert!(!is_excluded("Sources/weights/Thing.swift"));
        assert!(!is_excluded(".buildkite/pipeline.yml"));
    }

    #[cfg(unix)]
    fn symlink(root: &Path, link_rel: &str, target: &str) {
        let p = root.join(link_rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, p).unwrap();
    }

    /// #151 MEDIUM — a candidate that ADDS a symlink OUTSIDE editablePaths (a source-injection vector
    /// into a trusted dir) is a divergence, NOT skipped. git tracks the symlink as a mode-120000
    /// blob, so the ported `git diff --name-only` reference reports it as an added file; this gate
    /// must too. This is the laptop's live reproduction.
    #[test]
    #[cfg(unix)]
    fn added_symlink_outside_the_surface_is_refused() {
        let base = tmp("sym-add-base");
        let cand = tmp("sym-add-cand");
        symlink(
            &cand,
            "Sources/MLXFastCore/Extra.swift",
            "../MLXFastModel/payload.swift",
        );
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        let e = verify_no_write_outside_editable(m, &base, &cand).unwrap_err();
        assert!(e.contains("added") && e.contains("Extra.swift"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    /// The REPLACEMENT direction (a trusted regular file swapped for a symlink) stays refused: the
    /// type-tagged digest makes a file→symlink swap a `changed` divergence even if the link target
    /// text equals the file's bytes.
    #[test]
    #[cfg(unix)]
    fn symlink_replacing_a_trusted_file_is_refused() {
        let base = tmp("sym-repl-base");
        let cand = tmp("sym-repl-cand");
        write(&base, "Package.swift", b"payload"); // regular file in the trusted ref
        symlink(&cand, "Package.swift", "payload"); // candidate makes it a symlink to "payload"
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        let e = verify_no_write_outside_editable(m, &base, &cand).unwrap_err();
        assert!(e.contains("changed") && e.contains("Package.swift"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    /// No over-rejection: a symlink added INSIDE editablePaths is a legitimate edit and is accepted
    /// (its path is within the surface, so it never matters where it points).
    #[test]
    #[cfg(unix)]
    fn legit_symlink_inside_the_surface_is_accepted() {
        let base = tmp("sym-ok-base");
        let cand = tmp("sym-ok-cand");
        symlink(&cand, "Sources/MLXFastModel/alias.swift", "./Head.swift");
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        assert_eq!(verify_no_write_outside_editable(m, &base, &cand), Ok(()));
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    /// The residual inode hole is closed: a symlink at an OUTSIDE path that POINTS AT the editable
    /// dir must still be refused — membership is judged by where the link LIVES, not where it points,
    /// so the leaf is never inode-resolved to self-grant membership.
    #[test]
    #[cfg(unix)]
    fn symlink_outside_pointing_at_the_editable_dir_is_still_refused() {
        let base = tmp("sym-point-base");
        let cand = tmp("sym-point-cand");
        // The editable dir must exist so the symlink can resolve to its inode (the tempting hole).
        write(&base, "Sources/MLXFastModel/.keep", b"x");
        write(&cand, "Sources/MLXFastModel/.keep", b"x");
        symlink(&cand, "Sources/MLXFastCore/inject", "../MLXFastModel");
        let m = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;
        let e = verify_no_write_outside_editable(m, &base, &cand).unwrap_err();
        assert!(e.contains("added") && e.contains("inject"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&cand).unwrap();
    }

    // -----------------------------------------------------------------------
    // The RULED fork-point mode (`--write-gate-base`, David 2026-08-27).
    // -----------------------------------------------------------------------

    /// Run one git command in `repo` and return its trimmed stdout, asserting success. Every
    /// invocation pins its own identity and turns SIGNING OFF (`commit.gpgsign=false`): the fixtures
    /// must build identically on a laptop whose global config signs every commit and on a runner
    /// with no key at all. `core.excludesFile=/dev/null` keeps a developer's global ignore file from
    /// silently dropping a fixture file out of the commit the assertion depends on.
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

    /// Commit everything currently in the work tree and return the new commit's full sha.
    fn commit_all(repo: &Path, message: &str) -> String {
        git(repo, &["add", "-A", "."]);
        git(repo, &["commit", "-q", "-m", message]);
        git(repo, &["rev-parse", "HEAD"])
    }

    /// A fresh git repo under a temp dir.
    fn git_repo(tag: &str) -> std::path::PathBuf {
        let repo = tmp(tag);
        git(&repo, &["init", "-q"]);
        repo
    }

    /// The manifest every fork-point fixture below declares.
    const GIT_MANIFEST: &[u8] = br#"{"editablePaths":["Sources/MLXFastModel"]}"#;

    /// INCIDENT REGRESSION (2026-08-27) — the organizer moved harness main by DELETING a file no
    /// participant ever touched (`Tests/pin.swift`, standing in for the real
    /// `Tests/MLXFastTests/Gemma4SubmissionDraftDepthTests.swift` @ a9bd041). Under the tree-diff
    /// mode that deletion made every fresh submission diverge from the stale staged snapshot and
    /// benchd refused them all pre-GPU. Judged against the submission's OWN fork point the deletion
    /// is common history, so it cannot be attributed to the submission: this must PASS.
    ///
    /// The second assertion is the mechanism check — pointed at the STALE base (c0, before the
    /// organizer's deletion) the very same submission refuses, naming that same untouched file.
    /// That is the failure class, reproduced, and it is exactly what the correct base makes
    /// unreachable.
    #[test]
    fn organizer_deletion_below_the_fork_point_does_not_refuse_the_submission() {
        let repo = git_repo("git-incident");
        write(&repo, "Tests/pin.swift", b"organizer test\n");
        write(&repo, "Sources/MLXFastModel/Head.swift", b"stock head\n");
        let c0 = commit_all(&repo, "c0: harness main");

        // The organizer's commit on harness main: the test file is deleted upstream.
        git(&repo, &["rm", "-q", "Tests/pin.swift"]);
        let c1 = commit_all(&repo, "c1: organizer deletes the draft-depth test");

        // The submission forks c1 and only ever touches its declared editable surface.
        git(&repo, &["checkout", "-q", "-b", "submission"]);
        write(
            &repo,
            "Sources/MLXFastModel/Head.swift",
            b"my candidate head\n",
        );
        commit_all(&repo, "submission: tune the head");

        assert_eq!(
            verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &c1),
            Ok(()),
            "an organizer commit BELOW the fork point must never be charged to the submission"
        );
        let stale =
            verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &c0).unwrap_err();
        assert!(
            stale.contains("deleted") && stale.contains("Tests/pin.swift"),
            "the stale base must reproduce the incident refusal: {stale}"
        );
        std::fs::remove_dir_all(&repo).unwrap();
    }

    /// NEGATIVE CONTROL — the gate still refuses real writes outside the surface when they are the
    /// SUBMISSION'S OWN commits, in all three directions. Without this the fork-point mode could be
    /// silently defanged and the incident regression above would still pass.
    #[test]
    fn submission_commits_outside_the_surface_are_still_refused() {
        // changed
        let repo = git_repo("git-neg-changed");
        write(&repo, "tools/x.sh", b"#!/bin/sh\ntrue\n");
        write(&repo, "Sources/MLXFastModel/Head.swift", b"stock head\n");
        let base = commit_all(&repo, "base");
        write(&repo, "tools/x.sh", b"#!/bin/sh\nTAMPERED\n");
        commit_all(&repo, "submission: edit a trusted tool");
        let e = verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &base).unwrap_err();
        assert!(e.contains("tools/x.sh") && e.contains("changed"), "{e}");
        std::fs::remove_dir_all(&repo).unwrap();

        // added
        let repo = git_repo("git-neg-added");
        write(&repo, "Sources/MLXFastModel/Head.swift", b"stock head\n");
        let base = commit_all(&repo, "base");
        write(&repo, "tools/evil.sh", b"#!/bin/sh\n");
        commit_all(&repo, "submission: add a tool");
        let e = verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &base).unwrap_err();
        assert!(e.contains("tools/evil.sh") && e.contains("added"), "{e}");
        std::fs::remove_dir_all(&repo).unwrap();

        // deleted — the deletion is IN the submission's own commit, so it IS the submission's.
        let repo = git_repo("git-neg-deleted");
        write(&repo, "Package.resolved", b"pins\n");
        write(&repo, "Sources/MLXFastModel/Head.swift", b"stock head\n");
        let base = commit_all(&repo, "base");
        git(&repo, &["rm", "-q", "Package.resolved"]);
        commit_all(&repo, "submission: drop the pins");
        let e = verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &base).unwrap_err();
        assert!(
            e.contains("Package.resolved") && e.contains("deleted"),
            "{e}"
        );
        std::fs::remove_dir_all(&repo).unwrap();
    }

    /// No over-rejection: inside `editablePaths` a submission is free — add, modify and delete, over
    /// several commits — and the accumulated fork-point diff still passes.
    #[test]
    fn the_editable_surface_stays_fully_writable_across_commits() {
        let repo = git_repo("git-editable-free");
        write(&repo, "Package.swift", b"// pinned\n");
        write(&repo, "Sources/MLXFastModel/Head.swift", b"stock head\n");
        write(&repo, "Sources/MLXFastModel/Old.swift", b"to be removed\n");
        let base = commit_all(&repo, "base");

        write(&repo, "Sources/MLXFastModel/Head.swift", b"tuned head\n");
        commit_all(&repo, "submission 1: modify");
        write(&repo, "Sources/MLXFastModel/New.swift", b"new module\n");
        commit_all(&repo, "submission 2: add");
        git(&repo, &["rm", "-q", "Sources/MLXFastModel/Old.swift"]);
        commit_all(&repo, "submission 3: delete");

        assert_eq!(
            verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &base),
            Ok(())
        );
        std::fs::remove_dir_all(&repo).unwrap();
    }

    /// FAIL-CLOSED — a base this gate cannot pin down is a refusal, never a pass. Short sha (the
    /// spelling git would happily resolve, and the one an injection would hide behind), a
    /// well-formed 40-hex object this repo has never seen, and a candidate root that is not a work
    /// tree at all.
    #[test]
    fn an_unusable_write_gate_base_fails_closed() {
        let repo = git_repo("git-failclosed");
        write(&repo, "Sources/MLXFastModel/Head.swift", b"stock head\n");
        let base = commit_all(&repo, "base");

        let short = verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &base[..12])
            .unwrap_err();
        assert!(short.contains("--write-gate-base"), "{short}");
        assert!(verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, "not-hex").is_err());

        let unknown = "0123456789abcdef0123456789abcdef01234567";
        assert!(verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, unknown).is_err());
        std::fs::remove_dir_all(&repo).unwrap();

        // A directory with no .git: `git diff` exits non-zero and the gate refuses.
        let bare = tmp("git-notarepo");
        assert!(verify_no_write_outside_editable_from_git(GIT_MANIFEST, &bare, unknown).is_err());
        std::fs::remove_dir_all(&bare).unwrap();
    }

    /// A COMMITTED path under `.git`/`.build` is NOT held out in this mode (unlike the tree-diff
    /// mode, whose hold-out exists only because it compares work trees). Such a path cannot arise
    /// from a legitimate submission, so it must refuse.
    #[test]
    fn a_committed_build_output_is_not_excused_in_git_mode() {
        let repo = git_repo("git-build-committed");
        write(&repo, "Sources/MLXFastModel/Head.swift", b"stock head\n");
        let base = commit_all(&repo, "base");
        write(
            &repo,
            ".build/release/worker",
            b"a binary a submission must not commit",
        );
        commit_all(&repo, "submission: commit a build output");
        let e = verify_no_write_outside_editable_from_git(GIT_MANIFEST, &repo, &base).unwrap_err();
        assert!(
            e.contains(".build/release/worker") && e.contains("added"),
            "{e}"
        );
        std::fs::remove_dir_all(&repo).unwrap();
    }

    /// LOW (#151 ledger) — `editablePaths` is read by TWO parsers over the SAME trusted bytes:
    /// `byte_budget::load_contract` (the byte-budget walk) and `trusted_scope::EditableSurface::parse`
    /// (this gate). They must never disagree about the declared surface. No steering risk (both read
    /// the trusted baseline), but this pins the agreement so a future divergence fails loudly.
    #[test]
    fn the_two_editable_paths_parsers_agree() {
        let m = br#"{"editablePaths":["Sources/MLXFastModel","Sources/MLXFastTransform"],
                     "optionalEditablePaths":["x"],
                     "editableSurfaceByteBudget":{"exemptPaths":["Sources/MLXFastModel"],"maxTotalBytes":1000}}"#;
        let via_scope = trusted_scope::EditableSurface::parse(m)
            .unwrap()
            .editable_paths;
        let via_budget = crate::byte_budget::editable_paths_for_parity(m).unwrap();
        assert_eq!(
            via_scope, via_budget,
            "the two editablePaths parsers must agree"
        );
    }
}
