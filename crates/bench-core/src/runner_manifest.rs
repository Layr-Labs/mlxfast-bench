//! The runner manifest and the `hello`-against-manifest conformance checks
//! (Darkbloom runner contract §6, §6.1, §13).
//!
//! A runner declares what it can do ONCE, in a static manifest. The worker derives every
//! `hello` field from that manifest (§6.1), so the manifest and the `hello` are two views of
//! the same claim. This module parses the manifest, computes its CANONICAL digest, and checks
//! the two views against each other. Every failure is a named, distinct entry in the report —
//! a divergent `hello` names WHICH claim diverged, not just that something did.
//!
//! Nothing here feeds scoring. The checks are a conformance gate over the wire contract.
//!
//! # Canonical manifest bytes (digest definition, PINNED)
//!
//! `manifest_digest` hashes the manifest's CANONICAL FORM, not the file bytes, so formatting
//! and key order in a hand-written file never change the digest. The canonical form is:
//!
//! 1. Parse the file into [`RunnerManifest`]. Every field is required; an unknown field is a
//!    hard error.
//! 2. Serialize with `serde_json` in the DECLARED field order of §6 — `schemaVersion`,
//!    `runnerID`, `modelTypes`, `backend`, `engine`, `kvBackends`, `decoders`, `regimes`,
//!    `multimodal`, `recurrentLayers`, `requiresKeepMask` — and the declared order of each
//!    nested object. Keys are NEVER sorted.
//! 3. The bytes are compact UTF-8: no whitespace between tokens, no trailing newline.
//!    Integers carry no sign, no exponent, and no fraction. Booleans are `true` / `false`.
//!    An absent `depth` is `null`.
//! 4. Array order is the declared order. It is part of the digest.
//! 5. The digest is the sha256 of those bytes, in 64 lowercase hex characters.
//!
//! [`QWEN4EXP_125B_A6B_MANIFEST_SHA256`] pins the digest of the contract §11 manifest as a
//! test vector.

use crate::hash::sha256_hex;
use crate::BenchError;
use bench_protocol::{
    RunnerIdentity, CAPABILITY_BATCHED_FREE_RUN_DECODE, CAPABILITY_COHORT_REFERENCE_REPLAY,
    CAPABILITY_FREE_RUN_DECODE, CAPABILITY_PER_STREAM_TIMING,
};
use serde::{Deserialize, Serialize};

/// The runner manifest (contract §6). Field order is FROZEN: it defines the canonical bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerManifest {
    /// Manifest schema version (`1`).
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Vendor-namespaced runner id, e.g. `"layr/qwen4exp-125b-a6b"`.
    #[serde(rename = "runnerID")]
    pub runner_id: String,
    /// The `config.json` `model_type` values this runner claims.
    #[serde(rename = "modelTypes")]
    pub model_types: Vec<String>,
    /// Compute backend, e.g. `"mlx"`.
    pub backend: String,
    /// The engine capabilities the runner declares. Explicit, never defaulted (§6.2 rule 2).
    pub engine: EngineCapabilities,
    /// The KV backends the runner supports.
    #[serde(rename = "kvBackends")]
    pub kv_backends: Vec<KvBackendKind>,
    /// The decoders the runner declares.
    pub decoders: Vec<DecoderDeclaration>,
    /// The regimes the runner can serve. A regime it cannot serve is omitted (§6.2 rule 3).
    pub regimes: Vec<RegimeDeclaration>,
    /// Does the runner take non-text input?
    pub multimodal: bool,
    /// Does the model carry recurrent layers?
    #[serde(rename = "recurrentLayers")]
    pub recurrent_layers: bool,
    /// Does the runner need the keep mask (§10)?
    #[serde(rename = "requiresKeepMask")]
    pub requires_keep_mask: bool,
}

/// `CBv2ModelCapabilities` (contract §6). All six flags are explicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EngineCapabilities {
    pub supports_prefix_reuse: bool,
    #[serde(rename = "supportsPagedKV")]
    pub supports_paged_kv: bool,
    pub supports_compiled_decode: bool,
    pub supports_packed_prefill: bool,
    #[serde(rename = "supportsMTP")]
    pub supports_mtp: bool,
    #[serde(rename = "supportsCompactRecurrentMTPReplay")]
    pub supports_compact_recurrent_mtp_replay: bool,
}

/// `KVBackendKind` (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KvBackendKind {
    Contiguous,
    Paged,
}

/// One decoder the runner declares (contract §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecoderDeclaration {
    /// The spec mode string, e.g. `"serial"` / `"mtp"`.
    pub mode: String,
    /// Where the drafts come from.
    pub drafter: DrafterKind,
    /// Does the drafter carry per-request state?
    pub state: DrafterState,
    /// Inclusive draft-depth range as `[lower, upper]`; `null` for a serial decoder.
    pub depth: Option<(u32, u32)>,
}

/// `DrafterKind` (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DrafterKind {
    None,
    EmbeddedHead,
    AssistantCheckpoint,
    Ngram,
}

/// `DrafterState` (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DrafterState {
    Stateless,
    RequestStateful,
}

/// One regime the runner can serve (contract §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegimeDeclaration {
    /// Cohort width: `"single"`, or `{"upTo": n}`.
    pub batch: BatchDeclaration,
    /// How the regime is timed.
    pub timing: TimingKind,
    /// Does the regime report per-slot timing?
    #[serde(rename = "perStreamTiming")]
    pub per_stream_timing: bool,
}

/// `BatchDeclaration` (contract §6). `Single` is the JSON string `"single"`; `UpTo(n)` is the
/// JSON object `{"upTo": n}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatchDeclaration {
    Single,
    UpTo(u32),
}

/// `TimingKind` (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimingKind {
    FreeRun,
    TeacherForced,
}

impl RunnerManifest {
    /// The spec modes the manifest declares, in declared order.
    pub fn declared_modes(&self) -> Vec<&str> {
        self.decoders.iter().map(|d| d.mode.as_str()).collect()
    }

    /// The `hello.capabilities` the derivation table (§6.1) produces from this manifest.
    /// `trusted` is the kit's `--trusted` posture: `cohort_reference_replay` belongs to the
    /// trusted build alone.
    pub fn derived_capabilities(&self, trusted: bool) -> Vec<String> {
        let mut caps = Vec::new();
        if self.regimes.iter().any(|r| r.timing == TimingKind::FreeRun) {
            caps.push(CAPABILITY_FREE_RUN_DECODE.to_string());
        }
        if self
            .regimes
            .iter()
            .any(|r| matches!(r.batch, BatchDeclaration::UpTo(n) if n > 1))
        {
            caps.push(CAPABILITY_BATCHED_FREE_RUN_DECODE.to_string());
        }
        if self.regimes.iter().any(|r| r.per_stream_timing) {
            caps.push(CAPABILITY_PER_STREAM_TIMING.to_string());
        }
        if trusted {
            caps.push(CAPABILITY_COHORT_REFERENCE_REPLAY.to_string());
        }
        caps
    }

    /// The `hello.max_batch_size` the derivation table (§6.1) produces: the largest `n` over the
    /// `upTo(n)` regimes, or `None` when every regime is single.
    pub fn derived_max_batch_size(&self) -> Option<u32> {
        self.regimes
            .iter()
            .filter_map(|r| match r.batch {
                BatchDeclaration::Single => None,
                BatchDeclaration::UpTo(n) => Some(n),
            })
            .max()
    }

    /// The canonical manifest bytes. See the module docs for the pinned definition.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // `serde_json` emits struct fields in declaration order and never sorts them, and its
        // compact form carries no whitespace — exactly the canonical form the module docs pin.
        serde_json::to_vec(self).expect("a parsed RunnerManifest always serializes")
    }

    /// The canonical digest: sha256 of [`canonical_bytes`](Self::canonical_bytes), 64 lowercase
    /// hex characters.
    pub fn digest(&self) -> String {
        sha256_hex(&self.canonical_bytes())
    }
}

/// Parse a manifest from JSON text. An unknown or missing field is a hard error.
pub fn parse_manifest(text: &str) -> Result<RunnerManifest, BenchError> {
    serde_json::from_str(text)
        .map_err(|e| BenchError::InvalidInput(format!("runner manifest: {e}")))
}

/// The `hello` facts the manifest checks read. Kept local so this crate stays free of the
/// session layer; the caller fills it from the decoded `hello`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HelloFacts<'a> {
    /// `hello.backend`.
    pub backend: Option<&'a str>,
    /// `hello.capabilities`.
    pub capabilities: &'a [String],
    /// `hello.spec_modes`.
    pub spec_modes: &'a [String],
    /// `hello.max_batch_size`.
    pub max_batch_size: Option<u32>,
    /// `hello.runner`.
    pub runner: Option<&'a RunnerIdentity>,
}

/// One failed manifest check. Each variant is a distinct, named failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestCheckFailure {
    /// The `hello` advertises a spec mode the manifest does not declare (§6.2 rule 1).
    SpecModesNotDeclared {
        undeclared: Vec<String>,
        declared: Vec<String>,
    },
    /// The `hello` capabilities are not the set the derivation table produces (§6.1).
    CapabilitiesMismatch {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    /// The `hello` cohort-width ceiling is not the one the regimes produce (§6.1).
    MaxBatchSizeMismatch {
        expected: Option<u32>,
        actual: Option<u32>,
    },
    /// The `hello` backend is not the manifest backend (§6.1).
    BackendMismatch {
        expected: String,
        actual: Option<String>,
    },
    /// The `hello` carries no runner identity.
    RunnerIdentityAbsent,
    /// The runner identity cites a different manifest digest.
    ManifestDigestMismatch { expected: String, actual: String },
}

impl ManifestCheckFailure {
    /// The stable check name, for the kit's report.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SpecModesNotDeclared { .. } => "spec_modes_declared",
            Self::CapabilitiesMismatch { .. } => "capabilities_derived",
            Self::MaxBatchSizeMismatch { .. } => "max_batch_size_derived",
            Self::BackendMismatch { .. } => "backend_matches",
            Self::RunnerIdentityAbsent => "runner_identity_present",
            Self::ManifestDigestMismatch { .. } => "manifest_digest_matches",
        }
    }
}

impl std::fmt::Display for ManifestCheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpecModesNotDeclared {
                undeclared,
                declared,
            } => write!(
                f,
                "hello advertises spec modes {undeclared:?} the manifest does not declare \
                 (declared: {declared:?})"
            ),
            Self::CapabilitiesMismatch { expected, actual } => write!(
                f,
                "hello capabilities {actual:?} are not the derived set {expected:?}"
            ),
            Self::MaxBatchSizeMismatch { expected, actual } => write!(
                f,
                "hello max_batch_size {actual:?} is not the derived {expected:?}"
            ),
            Self::BackendMismatch { expected, actual } => write!(
                f,
                "hello backend {actual:?} is not the manifest backend {expected:?}"
            ),
            Self::RunnerIdentityAbsent => write!(f, "hello carries no runner identity"),
            Self::ManifestDigestMismatch { expected, actual } => write!(
                f,
                "hello runner.manifest_sha256 {actual:?} is not the manifest digest {expected:?}"
            ),
        }
    }
}

/// The manifest half of the conformance report: the checks that failed, in check order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestConformanceReport {
    /// The failed checks. Empty means the `hello` matches the manifest.
    pub failures: Vec<ManifestCheckFailure>,
}

impl ManifestConformanceReport {
    /// Does the `hello` match the manifest?
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Check a `hello` against the manifest (§6.1 derivation + §6.2 rules). `trusted` is the kit's
/// `--trusted` posture.
pub fn check_hello_against_manifest(
    manifest: &RunnerManifest,
    hello: &HelloFacts<'_>,
    trusted: bool,
) -> ManifestConformanceReport {
    let mut failures = Vec::new();

    let declared = manifest.declared_modes();
    let undeclared: Vec<String> = hello
        .spec_modes
        .iter()
        .filter(|m| !declared.contains(&m.as_str()))
        .cloned()
        .collect();
    if !undeclared.is_empty() {
        failures.push(ManifestCheckFailure::SpecModesNotDeclared {
            undeclared,
            declared: declared.iter().map(|m| m.to_string()).collect(),
        });
    }

    let mut expected_caps = manifest.derived_capabilities(trusted);
    expected_caps.sort();
    let mut actual_caps: Vec<String> = hello.capabilities.to_vec();
    actual_caps.sort();
    if expected_caps != actual_caps {
        failures.push(ManifestCheckFailure::CapabilitiesMismatch {
            expected: expected_caps,
            actual: actual_caps,
        });
    }

    let expected_batch = manifest.derived_max_batch_size();
    if expected_batch != hello.max_batch_size {
        failures.push(ManifestCheckFailure::MaxBatchSizeMismatch {
            expected: expected_batch,
            actual: hello.max_batch_size,
        });
    }

    if hello.backend != Some(manifest.backend.as_str()) {
        failures.push(ManifestCheckFailure::BackendMismatch {
            expected: manifest.backend.clone(),
            actual: hello.backend.map(|b| b.to_string()),
        });
    }

    match hello.runner {
        None => failures.push(ManifestCheckFailure::RunnerIdentityAbsent),
        Some(runner) => {
            let expected = manifest.digest();
            if runner.manifest_sha256 != expected {
                failures.push(ManifestCheckFailure::ManifestDigestMismatch {
                    expected,
                    actual: runner.manifest_sha256.clone(),
                });
            }
        }
    }

    ManifestConformanceReport { failures }
}

/// The pinned canonical digest of the contract §11 manifest (the first runner,
/// `layr/qwen4exp-125b-a6b`). The test vector for the canonical serialization.
pub const QWEN4EXP_125B_A6B_MANIFEST_SHA256: &str =
    "474efd9965aef3453e1e8324e99f9711d8e44bb2dceb0366d9c14c7d8e9ecebe";

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract §11 manifest, the first runner under this contract.
    const EXAMPLE: &str = include_str!("../tests/fixtures/runner_manifest/qwen4exp-125b-a6b.json");

    fn example() -> RunnerManifest {
        parse_manifest(EXAMPLE).expect("the §11 example manifest parses")
    }

    fn identity(digest: &str) -> RunnerIdentity {
        RunnerIdentity {
            id: "layr/qwen4exp-125b-a6b".to_string(),
            model_type: "qwen4_exp".to_string(),
            manifest_sha256: digest.to_string(),
            build: "c4089870".to_string(),
        }
    }

    #[test]
    fn canonical_bytes_are_the_declared_field_order() {
        // The pinned canonical form: compact, declared order, no sorted keys.
        let bytes = example().canonical_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            r#"{"schemaVersion":1,"runnerID":"layr/qwen4exp-125b-a6b","modelTypes":["qwen4_exp","qwen4_exp_text"],"backend":"mlx","engine":{"supportsPrefixReuse":false,"supportsPagedKV":false,"supportsCompiledDecode":false,"supportsPackedPrefill":false,"supportsMTP":true,"supportsCompactRecurrentMTPReplay":false},"kvBackends":["contiguous"],"decoders":[{"mode":"serial","drafter":"none","state":"stateless","depth":null},{"mode":"mtp","drafter":"embeddedHead","state":"requestStateful","depth":[1,3]}],"regimes":[{"batch":"single","timing":"freeRun","perStreamTiming":false},{"batch":"single","timing":"teacherForced","perStreamTiming":false}],"multimodal":false,"recurrentLayers":true,"requiresKeepMask":true}"#
        );
    }

    #[test]
    fn digest_is_the_pinned_test_vector() {
        assert_eq!(example().digest(), QWEN4EXP_125B_A6B_MANIFEST_SHA256);
        assert_eq!(QWEN4EXP_125B_A6B_MANIFEST_SHA256.len(), 64);
        assert!(QWEN4EXP_125B_A6B_MANIFEST_SHA256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn digest_ignores_file_formatting_and_key_order() {
        // The digest is over the CANONICAL form, so a reordered, re-indented file digests the same.
        let reordered = r#"{
            "backend": "mlx",
            "runnerID": "layr/qwen4exp-125b-a6b",
            "schemaVersion": 1,
            "modelTypes": ["qwen4_exp", "qwen4_exp_text"],
            "engine": {
                "supportsMTP": true,
                "supportsPrefixReuse": false,
                "supportsPagedKV": false,
                "supportsCompiledDecode": false,
                "supportsPackedPrefill": false,
                "supportsCompactRecurrentMTPReplay": false
            },
            "kvBackends": ["contiguous"],
            "decoders": [
                { "mode": "serial", "drafter": "none", "state": "stateless", "depth": null },
                { "mode": "mtp", "drafter": "embeddedHead", "state": "requestStateful", "depth": [1, 3] }
            ],
            "regimes": [
                { "batch": "single", "timing": "freeRun", "perStreamTiming": false },
                { "batch": "single", "timing": "teacherForced", "perStreamTiming": false }
            ],
            "multimodal": false,
            "recurrentLayers": true,
            "requiresKeepMask": true
        }"#;
        assert_eq!(
            parse_manifest(reordered).unwrap().digest(),
            QWEN4EXP_125B_A6B_MANIFEST_SHA256
        );
    }

    #[test]
    fn manifest_rejects_unknown_and_missing_fields() {
        let unknown = EXAMPLE.replace(
            "\"multimodal\": false",
            "\"multimodal\": false, \"extra\": 1",
        );
        assert!(parse_manifest(&unknown).is_err(), "unknown key must reject");
        let missing = EXAMPLE.replace("  \"multimodal\": false,\n", "");
        assert!(parse_manifest(&missing).is_err(), "missing key must reject");
    }

    #[test]
    fn conformant_hello_passes_every_check() {
        let manifest = example();
        let runner = identity(&manifest.digest());
        let caps = manifest.derived_capabilities(false);
        let modes = vec!["serial".to_string(), "mtp".to_string()];
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: None,
            runner: Some(&runner),
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert!(report.passed(), "{:?}", report.failures);
    }

    /// The failure names a conformant hello would never carry, one check at a time.
    fn names(report: &ManifestConformanceReport) -> Vec<&'static str> {
        report.failures.iter().map(|f| f.name()).collect()
    }

    #[test]
    fn undeclared_spec_mode_fails_its_own_check() {
        let manifest = example();
        let runner = identity(&manifest.digest());
        let caps = manifest.derived_capabilities(false);
        // The worker advertises a mode whose drafter the manifest never declared (§6.2 rule 1).
        let modes = vec!["serial".to_string(), "dflash".to_string()];
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: None,
            runner: Some(&runner),
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert_eq!(names(&report), vec!["spec_modes_declared"]);
        assert_eq!(
            report.failures[0],
            ManifestCheckFailure::SpecModesNotDeclared {
                undeclared: vec!["dflash".to_string()],
                declared: vec!["serial".to_string(), "mtp".to_string()],
            }
        );
    }

    #[test]
    fn capability_set_that_is_not_derived_fails_its_own_check() {
        let manifest = example();
        let runner = identity(&manifest.digest());
        // The §11 manifest has no cohort regime, so batched_free_run_decode is not derivable.
        let caps = vec![
            CAPABILITY_FREE_RUN_DECODE.to_string(),
            CAPABILITY_BATCHED_FREE_RUN_DECODE.to_string(),
        ];
        let modes = vec!["serial".to_string(), "mtp".to_string()];
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: None,
            runner: Some(&runner),
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert_eq!(names(&report), vec!["capabilities_derived"]);
    }

    #[test]
    fn cohort_reference_replay_is_derived_only_under_trusted() {
        let manifest = example();
        let runner = identity(&manifest.digest());
        let caps = vec![
            CAPABILITY_FREE_RUN_DECODE.to_string(),
            CAPABILITY_COHORT_REFERENCE_REPLAY.to_string(),
        ];
        let modes = vec!["serial".to_string(), "mtp".to_string()];
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: None,
            runner: Some(&runner),
        };
        // Untrusted: the replay capability is not derivable, so the check fails...
        assert_eq!(
            names(&check_hello_against_manifest(&manifest, &hello, false)),
            vec!["capabilities_derived"]
        );
        // ...and under --trusted the same hello passes.
        assert!(check_hello_against_manifest(&manifest, &hello, true).passed());
    }

    #[test]
    fn max_batch_size_is_derived_from_the_up_to_regimes() {
        let mut manifest = example();
        let runner = identity(&manifest.digest());
        let caps = manifest.derived_capabilities(false);
        let modes = vec!["serial".to_string(), "mtp".to_string()];
        // Every §11 regime is single, so an advertised ceiling is a divergence.
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: Some(8),
            runner: Some(&runner),
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert_eq!(names(&report), vec!["max_batch_size_derived"]);
        assert_eq!(
            report.failures[0],
            ManifestCheckFailure::MaxBatchSizeMismatch {
                expected: None,
                actual: Some(8),
            }
        );
        // With cohort regimes the ceiling is the largest declared n, and the capability follows.
        manifest.regimes.push(RegimeDeclaration {
            batch: BatchDeclaration::UpTo(8),
            timing: TimingKind::FreeRun,
            per_stream_timing: false,
        });
        let runner = identity(&manifest.digest());
        let caps = manifest.derived_capabilities(false);
        assert_eq!(manifest.derived_max_batch_size(), Some(8));
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: Some(8),
            runner: Some(&runner),
        };
        assert!(check_hello_against_manifest(&manifest, &hello, false).passed());
    }

    #[test]
    fn backend_divergence_fails_its_own_check() {
        let manifest = example();
        let runner = identity(&manifest.digest());
        let caps = manifest.derived_capabilities(false);
        let modes = vec!["serial".to_string(), "mtp".to_string()];
        let hello = HelloFacts {
            backend: Some("cuda"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: None,
            runner: Some(&runner),
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert_eq!(names(&report), vec!["backend_matches"]);
    }

    #[test]
    fn absent_runner_identity_fails_its_own_check() {
        let manifest = example();
        let caps = manifest.derived_capabilities(false);
        let modes = vec!["serial".to_string(), "mtp".to_string()];
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: None,
            runner: None,
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert_eq!(names(&report), vec!["runner_identity_present"]);
    }

    #[test]
    fn digest_divergence_fails_its_own_check() {
        let manifest = example();
        let runner = identity(&"a".repeat(64));
        let caps = manifest.derived_capabilities(false);
        let modes = vec!["serial".to_string(), "mtp".to_string()];
        let hello = HelloFacts {
            backend: Some("mlx"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: None,
            runner: Some(&runner),
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert_eq!(names(&report), vec!["manifest_digest_matches"]);
        assert_eq!(
            report.failures[0],
            ManifestCheckFailure::ManifestDigestMismatch {
                expected: QWEN4EXP_125B_A6B_MANIFEST_SHA256.to_string(),
                actual: "a".repeat(64),
            }
        );
    }

    #[test]
    fn every_failed_check_is_reported_distinctly() {
        // A hello that diverges on every check reports all six names, once each.
        let manifest = example();
        let caps = vec![CAPABILITY_PER_STREAM_TIMING.to_string()];
        let modes = vec!["dspark".to_string()];
        let hello = HelloFacts {
            backend: Some("cuda"),
            capabilities: &caps,
            spec_modes: &modes,
            max_batch_size: Some(4),
            runner: None,
        };
        let report = check_hello_against_manifest(&manifest, &hello, false);
        assert_eq!(
            names(&report),
            vec![
                "spec_modes_declared",
                "capabilities_derived",
                "max_batch_size_derived",
                "backend_matches",
                "runner_identity_present",
            ]
        );
        assert!(!report.passed());
    }
}
