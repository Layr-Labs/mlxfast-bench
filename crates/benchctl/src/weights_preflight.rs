//! `benchctl validate-weights` — the WEIGHTS-half preflight (Swift
//! `BenchmarkPreflight.checkArtifacts` + `transformedWeightsByteCount` +
//! `MLXFAST_MAX_WEIGHTS_BYTES`, BenchmarkSupport.swift:109-216, QwenRuntimePreflight.swift:
//! 161-170, ByteLimitParsing.swift).
//!
//! benchctl's `validate-golden` covers only the GOLDEN half of preflight; this covers the
//! weights half, mirroring Swift's checks:
//! - the weights path is a real directory, NOT a symlink (Swift `transformedWeightsByteCount`
//!   symlink/dir guards, BenchmarkSupport.swift:163-169);
//! - `config.json` (transformed config) and `model.safetensors.index.json` (dense safetensors
//!   index) are present as regular files (Swift `requiredFiles`, BenchmarkSupport.swift:115-122);
//! - the directory byte-count is summed with symlinks and non-regular files REJECTED, and it
//!   is enforced against the size cap (Swift `transformedWeightsByteCount`
//!   :180-214 + `enforceTransformedWeightsByteLimit`);
//! - the cap comes from `MLXFAST_MAX_WEIGHTS_BYTES` with Swift's `parseTransformedWeightsByteLimit`
//!   semantics: empty ⇒ the 25 GiB default; `0` / `none` / `unlimited` ⇒ no cap; a positive
//!   integer ⇒ that cap; anything else ⇒ a hard error.
//!
//! Scope note: this is the WEIGHTS-artifact + size-cap surface only. The deeper in-engine
//! weight validation Swift does (`DenseTensorStore.validateReadableByteRanges`,
//! `validateRequiredMetadata`, the 1,847-tensor contract) needs the real loader and is B-3;
//! this preflight is filesystem + env, fully macOS-testable. The `--golden` presence check
//! is intentionally NOT duplicated here (that is `validate-golden`'s job); pass `--golden`
//! only if you want the Swift `requiredFiles` golden-presence check too.

use std::path::Path;

use bench_core::constants::DEFAULT_MAX_TRANSFORMED_WEIGHTS_BYTES;

/// Parse a transformed-weights byte cap (Swift `parseTransformedWeightsByteLimit`,
/// ByteLimitParsing.swift:5-25). `raw` is the flag/env string; empty ⇒ `default_cap`;
/// `0`/`none`/`unlimited` (case-insensitive) ⇒ `None` (uncapped); a positive integer ⇒
/// `Some(n)`; anything else is an error.
pub fn parse_weights_byte_limit(
    raw: &str,
    default_cap: Option<u64>,
) -> Result<Option<u64>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(default_cap);
    }
    let lowercased = trimmed.to_ascii_lowercase();
    if lowercased == "0" || lowercased == "none" || lowercased == "unlimited" {
        return Ok(None);
    }
    match trimmed.parse::<u64>() {
        Ok(v) if v > 0 => Ok(Some(v)),
        _ => Err(
            "MLXFAST_MAX_WEIGHTS_BYTES must be a positive byte count, 0, none, or unlimited"
                .to_string(),
        ),
    }
}

/// Resolve the cap from the `MLXFAST_MAX_WEIGHTS_BYTES` env value (or `None` if unset),
/// defaulting to the 25 GiB Swift default when empty/unset.
pub fn weights_byte_limit_from_env(env_value: Option<&str>) -> Result<Option<u64>, String> {
    parse_weights_byte_limit(
        env_value.unwrap_or(""),
        Some(DEFAULT_MAX_TRANSFORMED_WEIGHTS_BYTES),
    )
}

/// The accepted-weights report (mirrors the load-bearing fields of Swift
/// `BenchmarkPreflightReport`).
#[derive(Debug, Clone, PartialEq)]
pub struct WeightsPreflightReport {
    pub weights_byte_count: u64,
    pub max_weights_byte_count: Option<u64>,
    pub file_count: u64,
}

/// Validate the WEIGHTS half of preflight. `cap` is the resolved size cap (`None` = uncapped).
/// If `golden` is `Some`, its presence is also required (Swift `requiredFiles` includes the
/// golden). Returns the byte count on success, or the verbatim Swift-style rejection message.
pub fn validate_weights(
    weights: &Path,
    golden: Option<&Path>,
    cap: Option<u64>,
) -> Result<WeightsPreflightReport, String> {
    // The weights path must be a real directory, not a symlink (Swift symlink/dir guards).
    let root_meta = std::fs::symlink_metadata(weights)
        .map_err(|e| format!("transformed weights path could not be stat'd: {e}"))?;
    if root_meta.file_type().is_symlink() {
        return Err(format!(
            "transformed weights path must not be a symlink: {}",
            weights.display()
        ));
    }
    if !root_meta.is_dir() {
        return Err(format!(
            "transformed weights path must be a directory: {}",
            weights.display()
        ));
    }

    // Required artifacts (Swift `requiredFiles`): the two transformed-weights files, plus the
    // golden when supplied. Each must exist as a regular (non-symlink) file.
    require_regular_file(
        &weights.join("config.json"),
        "transformed config (config.json)",
    )?;
    require_regular_file(
        &weights.join("model.safetensors.index.json"),
        "dense safetensors index (model.safetensors.index.json)",
    )?;
    if let Some(g) = golden {
        require_regular_file(g, "correctness golden file")?;
    }

    // Walk the tree: reject symlinks and non-regular files, sum bytes, enforce the cap
    // incrementally (Swift `transformedWeightsByteCount`).
    let mut byte_count: u64 = 0;
    let mut file_count: u64 = 0;
    let mut stack = vec![weights.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("could not read weights dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("weights dir entry error: {e}"))?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| format!("could not stat {}: {e}", path.display()))?;
            let ft = meta.file_type();
            if ft.is_symlink() {
                return Err(format!(
                    "transformed weights must not contain symlink {}",
                    path.display()
                ));
            }
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                return Err(format!(
                    "transformed weights contains non-regular file {}",
                    path.display()
                ));
            }
            byte_count = byte_count
                .checked_add(meta.len())
                .ok_or_else(|| "transformed weights byte count overflow".to_string())?;
            file_count += 1;
            if let Some(max) = cap {
                if byte_count > max {
                    return Err(format!(
                        "transformed weights are {byte_count} bytes, above MLXFAST_MAX_WEIGHTS_BYTES={max}"
                    ));
                }
            }
        }
    }

    Ok(WeightsPreflightReport {
        weights_byte_count: byte_count,
        max_weights_byte_count: cap,
        file_count,
    })
}

/// Require a regular (non-symlink) file at `path` (Swift `requireFile`/`requireRegularFile`).
fn require_regular_file(path: &Path, description: &str) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| format!("{description} missing at {}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{description} must not be a symlink: {}",
            path.display()
        ));
    }
    if !meta.is_file() {
        return Err(format!("{description} missing at {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "benchctl-weights-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn valid_weights_dir(tag: &str) -> std::path::PathBuf {
        let d = tmpdir(tag);
        fs::write(d.join("config.json"), b"{}").unwrap();
        fs::write(d.join("model.safetensors.index.json"), b"{}").unwrap();
        fs::write(d.join("model-00001.safetensors"), vec![0u8; 4096]).unwrap();
        d
    }

    #[test]
    fn parse_cap_semantics_match_swift() {
        // empty ⇒ default; 0/none/unlimited ⇒ None; positive ⇒ Some; else error.
        assert_eq!(parse_weights_byte_limit("", Some(25)).unwrap(), Some(25));
        assert_eq!(parse_weights_byte_limit("  ", Some(25)).unwrap(), Some(25));
        assert_eq!(parse_weights_byte_limit("0", Some(25)).unwrap(), None);
        assert_eq!(parse_weights_byte_limit("none", Some(25)).unwrap(), None);
        assert_eq!(
            parse_weights_byte_limit("UNLIMITED", Some(25)).unwrap(),
            None
        );
        assert_eq!(
            parse_weights_byte_limit("4096", Some(25)).unwrap(),
            Some(4096)
        );
        assert!(parse_weights_byte_limit("-1", Some(25)).is_err());
        assert!(parse_weights_byte_limit("cheese", Some(25)).is_err());
        // unset env ⇒ 25 GiB default.
        assert_eq!(
            weights_byte_limit_from_env(None).unwrap(),
            Some(DEFAULT_MAX_TRANSFORMED_WEIGHTS_BYTES)
        );
    }

    #[test]
    fn accepts_a_wellformed_weights_dir() {
        let d = valid_weights_dir("ok");
        let report = validate_weights(&d, None, Some(1 << 30)).unwrap();
        assert_eq!(report.weights_byte_count, 4096 + 2 + 2);
        assert_eq!(report.file_count, 3);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_missing_index() {
        let d = tmpdir("noindex");
        fs::write(d.join("config.json"), b"{}").unwrap();
        let err = validate_weights(&d, None, None).unwrap_err();
        assert!(err.contains("dense safetensors index"), "got: {err}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_missing_config() {
        let d = tmpdir("noconfig");
        fs::write(d.join("model.safetensors.index.json"), b"{}").unwrap();
        let err = validate_weights(&d, None, None).unwrap_err();
        assert!(err.contains("transformed config"), "got: {err}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn enforces_the_size_cap() {
        let d = valid_weights_dir("cap");
        // Cap below the 4 KiB safetensors payload ⇒ rejected with the Swift-shaped message.
        let err = validate_weights(&d, None, Some(100)).unwrap_err();
        assert!(
            err.contains("above MLXFAST_MAX_WEIGHTS_BYTES=100"),
            "got: {err}"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn uncapped_accepts_any_size() {
        let d = valid_weights_dir("uncapped");
        assert!(validate_weights(&d, None, None).is_ok());
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_in_tree() {
        use std::os::unix::fs::symlink;
        let d = valid_weights_dir("symlink");
        symlink(d.join("config.json"), d.join("alias.json")).unwrap();
        let err = validate_weights(&d, None, None).unwrap_err();
        assert!(err.contains("must not contain symlink"), "got: {err}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_golden_when_required_is_rejected() {
        let d = valid_weights_dir("golden");
        let missing = d.join("nope-golden.json");
        let err = validate_weights(&d, Some(&missing), None).unwrap_err();
        assert!(err.contains("correctness golden file"), "got: {err}");
        let _ = fs::remove_dir_all(&d);
    }
}
