//! Golden schema + strict loader, ported from Sources/MLXFastCore/Golden.swift.
//!
//! Every golden struct carries `#[serde(deny_unknown_fields)]`, so serde rejects
//! unknown keys at EVERY level — including per-case objects — during a single typed
//! parse. This closes an anti-cheat hole: a typo'd per-case key (e.g. `acepted_tokens`
//! on an anchor) was silently dropped by the old `serde_json::Value` key-set pass,
//! which never descended into the case objects. The loader now keeps only the
//! semantic checks serde cannot express (cross-field invariants, non-empty
//! requirements, token/vocab ranges, tensor-count contracts, sha256 of the raw bytes).
//! Optional sections the Swift loader rejected when explicitly `null` (`model_type`,
//! `correctness_gates`, `benchmark`, and the gate sections) use `deny_explicit_null`
//! to keep that rejection while still allowing the field to be absent.

use crate::constants::*;
use crate::hash::sha256_hex;
use crate::{BenchError, Result};
use serde::{Deserialize, Serialize};

/// Token IDs are ports of Swift `Int`; kept as `i64` so the vocab range check
/// (`token < 0 || token >= vocab_size`) is exact.
pub type Token = i64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenCase {
    pub name: String,
    pub prompt_tokens: Vec<Token>,
    pub expected_tokens: Vec<Token>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenAnchorCase {
    pub name: String,
    pub context_tokens: Vec<Token>,
    pub expected_token: Token,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub accepted_tokens: Option<Vec<Token>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_expected_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_top_logit_delta: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenFreeRunCase {
    pub name: String,
    pub prompt_tokens: Vec<Token>,
    pub expected_tokens: Vec<Token>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exact_prefix_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenBehaviorCase {
    pub name: String,
    pub prompt_tokens: Vec<Token>,
    pub accepted_token_sequences: Vec<Vec<Token>>,
    pub max_new_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub semantic_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub semantic_answer_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub semantic_reference_answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub semantic_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub semantic_subdomain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenCorrectnessGates {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deny_explicit_null"
    )]
    pub anchors: Option<Vec<GoldenAnchorCase>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deny_explicit_null"
    )]
    pub free_run: Option<Vec<GoldenFreeRunCase>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deny_explicit_null"
    )]
    pub behavior: Option<Vec<GoldenBehaviorCase>>,
}

impl GoldenCorrectnessGates {
    pub fn anchor_cases(&self) -> &[GoldenAnchorCase] {
        self.anchors.as_deref().unwrap_or(&[])
    }
    pub fn free_run_cases(&self) -> &[GoldenFreeRunCase] {
        self.free_run.as_deref().unwrap_or(&[])
    }
    pub fn behavior_cases(&self) -> &[GoldenBehaviorCase] {
        self.behavior.as_deref().unwrap_or(&[])
    }
    pub fn total_case_count(&self) -> usize {
        self.anchor_cases().len() + self.free_run_cases().len() + self.behavior_cases().len()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkGolden {
    pub prefill_prompt_tokens: Vec<Token>,
    pub expected_prefill_token: Token,
    pub decode_seed_tokens: Vec<Token>,
    pub expected_decode_seed_token: Token,
    pub expected_decode_tokens: Vec<Token>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub baseline_prefill_seconds_per_token: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub baseline_decode_seconds_per_token: Option<f64>,
}

/// `model_provenance` — ADDITIVE provenance metadata: the pinned reference MODEL's
/// repository + revision.
///
/// HISTORY (why this key comes and goes). benchd first accepted it in `69adcad`, purely
/// because an R2-mirror file carried it — mirror-driven, matching no integrity pin — and that
/// loosening was REVERTED in `3270ae6` under the standing rule that a mirror artifact is
/// evidence, never a schema input. It is BACK now on the opposite basis: the REFERENCE loader
/// itself was corrected to carry both keys (`mlxfast-qwen-38-27b-mtp-engine` #11 → PR #12,
/// `2be4e21`, merged `151eb11`: `model_type` restored as the schema key, `model_provenance`
/// RETAINED as an additional OPTIONAL key, unknown keys still rejected). Swift is the
/// reference; benchd mirrors it.
///
/// Shape held to the reference (`Golden.swift` `validateGoldenModelProvenanceKeys`): a JSON
/// object with EXACTLY `repository` + `revision` (an explicit `null` and any other key both
/// reject), a non-empty `repository`, and a `revision` matching `^[0-9a-f]{40}$`.
///
/// **VALUE PIN (issue #114, RULED by David 2026-08-20 — "R2 like challenger"):** the reference
/// ALSO compares the two values against `MLXFastConstants.referenceModelRepository` /
/// `referenceModelRevision` and rejects a provenance naming a different model. benchd now makes
/// that same comparison, but its pin comes from the TRACK CONTRACT fixture
/// ([`ReferenceModelPin`], sourced by [`reference_model_pin_from_contract`]) rather than a
/// compiled-in constant: the Swift constant is the fork's compile-time convenience, the contract
/// is the cross-platform authority, and only a contract pin can be PER-TRACK (the CUDA NVFP4
/// target declares its own). A caller that HOLDS a contract pin therefore gets exactly the Swift
/// decision; a caller with no contract in hand (`None`) still validates the SHAPE only — that
/// residual looseness is now scoped to the contract-less commands, not to benchd as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenModelProvenance {
    /// The reference model repository (non-empty).
    pub repository: String,
    /// The reference model revision: 40 lowercase hex characters.
    pub revision: String,
}

/// The reference-model IDENTITY a track contract pins its goldens to — the contract-driven
/// counterpart of Swift's `MLXFastConstants.referenceModel{Repository,Revision}` pair.
///
/// Issue #114 ruling: the pin authority is the track contract fixture, not a bench-core constant.
/// Nothing in this crate ever names a model; the identity arrives from the `--contract` the run
/// was dispatched with, so a second track (CUDA's NVFP4 target) pins its own reference by
/// shipping its own fixture rather than by editing benchd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceModelPin {
    /// The reference model repository, e.g. the contract's `target.upstream_model_id`.
    pub repository: String,
    /// That repository's pinned revision, e.g. the contract's `target.upstream_revision`.
    pub revision: String,
}

/// The track contract's `target` block, modelled with ONLY the two keys that carry the
/// reference-model identity.
///
/// Shape mirrored from the challenger's track fixture — `qwen3_8_27b_mtp_track.json` `target`:
/// `{"upstream_model_id": "…", "upstream_revision": "<40-hex>", …}` — which is the same pair the
/// reference fork hard-codes in `Sources/MLXFastCore/Constants.swift`
/// (`referenceModelRepository` / `referenceModelRevision`). The challenger's sibling `mtp_head`
/// block uses the identical two keys, so a head-identity pin can reuse this type unchanged.
///
/// **NOT `deny_unknown_fields`, deliberately** (F2 review point, answered differently): a real
/// `target` block is mostly PROSE and unrelated pins — the live 3.8 fixture carries
/// `upstream_pin_note`, `upstream_source_model_id`, `upstream_source_revision`, `geometry_note`,
/// `quantization`, …, and the CUDA fixture adds `format`, `ships_mtp_head`, `manifest_path` and
/// their `_note` siblings. Denying unknown keys here would refuse every real contract, so the
/// hazard the reviewer named — an upstream key RENAME silently disabling the pin — is closed by
/// the rule below instead: a `target` that is PRESENT but declares NEITHER half is an ERROR, so a
/// renamed `upstream_model_id` fails LOUD rather than falling open. The two mechanisms cover the
/// same attack; only this one survives contact with the fixtures.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ContractReferenceTarget {
    #[serde(default)]
    pub upstream_model_id: Option<String>,
    #[serde(default)]
    pub upstream_revision: Option<String>,
}

impl ContractReferenceTarget {
    /// The pin this `target` block declares. NEVER `Ok(None)`: reaching this function means the
    /// contract HAS a `target`, and a present-but-undeclaring `target` is a contract DEFECT.
    ///
    /// FAIL-CLOSED, in three ways the review found the first draft falling open:
    /// - **Neither half declared** (`target: {}`, both keys `null`, both keys renamed) is an
    ///   ERROR. The first draft returned `Ok(None)` here, which meant making the contract MORE
    ///   broken turned a hard error into a silent unpinned pass. "This block exists but names
    ///   nothing" is a defect, not a decision to skip the pin — declining to pin is spelled by
    ///   omitting `target` entirely.
    /// - **Half declared** (one key present) is an ERROR: a contract that LOOKS pinned while
    ///   enforcing nothing is the one shape that must not exist.
    /// - **A malformed VALUE** is an ERROR ATTRIBUTED TO THE CONTRACT. A revision that is not
    ///   40 lowercase hex can never equal a well-formed golden's revision, so accepting it as a
    ///   pin would reject every provenance-bearing golden with a diagnostic pointing at the
    ///   GOLDEN — and the CUDA track fixture ships exactly that today
    ///   (`upstream_revision: "QWEN-MTP-CUDA-PENDING-ORGANIZER"`, a pending marker). The defect
    ///   is named where it lives.
    ///
    /// Values are NOT trimmed. The reference's analogous rule for `model_type`
    /// (`Golden.swift:783-800`) is that a padded `" qwen "` is a corpus defect "not something to
    /// normalize away"; a padded contract value gets the same treatment rather than the opposite.
    pub fn pin(&self) -> Result<ReferenceModelPin> {
        let repository = declared_half(self.upstream_model_id.as_deref(), "upstream_model_id")?;
        let revision = declared_half(self.upstream_revision.as_deref(), "upstream_revision")?;
        let (repository, revision) = match (repository, revision) {
            (Some(repository), Some(revision)) => (repository, revision),
            (None, None) => {
                return Err(invalid(
                    "contract declares a `target` block that names NO reference model: \
                     upstream_model_id and upstream_revision are both absent. A track that \
                     deliberately pins no reference model omits the `target` block entirely — \
                     a present-but-empty target is a contract defect, not an opt-out",
                ))
            }
            _ => {
                return Err(invalid(
                    "contract target declares only half a reference-model pin: \
                     upstream_model_id and upstream_revision must both be present and non-empty \
                     (a half-declared pin is never treated as no pin)",
                ))
            }
        };
        // The revision must be a real commit id. A pending marker or placeholder is a CONTRACT
        // defect: pinning to it would make every provenance-bearing golden reject and would blame
        // the golden for the contract's unfinished field.
        if !is_forty_lowercase_hex(revision) {
            return Err(invalid(format!(
                "contract target.upstream_revision {revision:?} is not a 40-character lowercase \
                 hex commit id — a placeholder or pending marker cannot pin anything, and \
                 accepting it would reject every provenance-bearing golden while blaming the \
                 golden for a contract defect"
            )));
        }
        Ok(ReferenceModelPin {
            repository: repository.to_string(),
            revision: revision.to_string(),
        })
    }
}

/// One half of a declared pin: `None` when the key is absent or JSON `null`, `Some(value)` when it
/// is present. A present-but-EMPTY or WHITESPACE-PADDED value is a contract defect (see
/// [`ContractReferenceTarget::pin`] on why it is not trimmed away).
fn declared_half<'a>(value: Option<&'a str>, key: &str) -> Result<Option<&'a str>> {
    match value {
        None => Ok(None),
        Some("") => Err(invalid(format!(
            "contract target.{key} is an EMPTY string. Absent means \"not declared\"; present \
             means \"this is the value\" — an empty value is neither, and is a contract defect"
        ))),
        Some(value) if value != value.trim() => Err(invalid(format!(
            "contract target.{key} {value:?} has leading or trailing whitespace — a padded value \
             is a contract defect, not something to normalize away (the reference holds \
             `model_type` to the same rule)"
        ))),
        Some(value) => Ok(Some(value)),
    }
}

/// `^[0-9a-f]{40}$` — the same revision shape the reference requires of a golden's
/// `model_provenance.revision`, applied to the contract's declared pin so the two sides of the
/// comparison are held to ONE rule.
fn is_forty_lowercase_hex(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_lowercase() && b <= b'f'))
}

/// Envelope for [`reference_model_pin_from_contract`]: the ONE place the contract field PATH
/// (`target.upstream_model_id` / `target.upstream_revision`) is spelled, so benchd has a single
/// source of truth for where a track declares its reference model.
#[derive(Deserialize)]
struct ContractReferenceEnvelope {
    /// Absent ⇒ the track declares no reference model (legacy/offline fixtures). An explicit
    /// `"target": null` is REJECTED rather than read as absent, matching the loader's standing
    /// treatment of explicit nulls on optional sections — "declared as nothing" is not "undeclared".
    #[serde(default, deserialize_with = "deny_explicit_null")]
    target: Option<ContractReferenceTarget>,
}

/// Read a track contract fixture's declared reference-model pin from the contract BYTES.
///
/// `Ok(None)` means the fixture carries NO `target` key at all (offline/legacy fixtures) — the
/// golden loader then validates `model_provenance` SHAPE only, exactly as before. Every other
/// outcome is an error: a `target` that declares neither half, half a pin, a padded/empty value, a
/// non-sha revision (see [`ContractReferenceTarget::pin`]), an explicit `"target": null`, or
/// contract JSON that will not decode. A contract that cannot be read is never treated as an
/// unpinned one.
pub fn reference_model_pin_from_contract(bytes: &[u8]) -> Result<Option<ReferenceModelPin>> {
    let envelope: ContractReferenceEnvelope = serde_json::from_slice(bytes).map_err(|e| {
        invalid(format!(
            "contract could not be decoded for its reference-model pin: {e}"
        ))
    })?;
    match envelope.target {
        Some(target) => target.pin().map(Some),
        None => Ok(None),
    }
}

/// The reference's model-identity gate on `model_provenance` VALUES, with the pin supplied by the
/// track contract instead of a compiled-in constant.
///
/// Port of `Golden.swift:386-393` — the diagnostic is the reference's string byte-for-byte
/// (`"correctness golden model_provenance does not match the pinned reference model"`), so a
/// harness diffing the two loaders' stderr sees one message, not two spellings of one rule.
pub fn verify_model_provenance_pinned(
    provenance: &GoldenModelProvenance,
    pin: &ReferenceModelPin,
) -> Result<()> {
    if provenance.repository != pin.repository || provenance.revision != pin.revision {
        return Err(invalid(
            "correctness golden model_provenance does not match the pinned reference model",
        ));
    }
    Ok(())
}

/// The hidden correctness golden's IDENTITY, pinned by a track contract as a NAME-FREE integrity
/// pin (`sha256` + `bytes`) — the contract-driven authority for WHICH bytes are the hidden serial
/// trajectory the trusted parent re-checks every emitted token against.
///
/// LANE 2a: this is the SIBLING of `timed_prompt_pool`, NOT a ninth pool entry. The anti-lottery
/// pool cardinality is `timed_prompt_pool | length` ALONE, so sourcing this pin never perturbs the
/// pinned pool count `N`, and a pool-membership join over `timed_prompt_pool[].sha256` never sees
/// it. Nothing here names the golden: the pin (sha256+bytes) is the only identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectnessGoldenPin {
    /// The correctness golden's sha256, 64 lowercase hex — the NAME-FREE identity.
    pub sha256: String,
    /// That golden's exact byte count — the other half of the canonical sha256+bytes pin.
    pub bytes: u64,
}

/// Envelope for [`hidden_correctness_golden_pin_from_contract`]: the ONE place the contract field
/// PATH (`hidden_correctness_golden`) is spelled, so benchd has a single source of truth for where a
/// track pins its correctness golden's identity — the correctness-golden counterpart of
/// [`ContractReferenceEnvelope`]. Read by field name off the contract ROOT (a SIBLING of
/// `timed_prompt_pool`), never joined against the pool, so sourcing it can never change the pinned
/// pool count.
#[derive(Deserialize)]
struct ContractCorrectnessGoldenEnvelope {
    /// Absent ⇒ the track pins no correctness golden (legacy/offline fixtures). An explicit
    /// `"hidden_correctness_golden": null` is REJECTED rather than read as absent, matching the
    /// loader's standing treatment of explicit nulls on optional sections.
    #[serde(default, deserialize_with = "deny_explicit_null")]
    hidden_correctness_golden: Option<ContractCorrectnessGoldenSibling>,
}

/// The contract's `hidden_correctness_golden` sibling block, modelled with ONLY the two keys that
/// carry the integrity pin. NOT `deny_unknown_fields` (the live 3.8 fixture pairs it with a
/// `hidden_correctness_golden_note` prose sibling), and half/neither-declared is a contract DEFECT
/// closed the same way [`ContractReferenceTarget`] closes it: a present-but-undeclaring block fails
/// LOUD rather than falling open.
#[derive(Deserialize)]
struct ContractCorrectnessGoldenSibling {
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    bytes: Option<u64>,
}

impl ContractCorrectnessGoldenSibling {
    /// The pin this block declares. NEVER `Ok(None)`: reaching here means the contract HAS a
    /// `hidden_correctness_golden` block, and a present-but-undeclaring block is a contract DEFECT.
    /// FAIL-CLOSED on neither-half (`{}`, both null), on half a pin, on a non-sha256 value, and on a
    /// zero byte count — every shape that would let a run pin nothing while LOOKING pinned.
    fn pin(&self) -> Result<CorrectnessGoldenPin> {
        let sha256 = declared_half(self.sha256.as_deref(), "hidden_correctness_golden.sha256")?;
        let (sha256, bytes) = match (sha256, self.bytes) {
            (Some(sha256), Some(bytes)) => (sha256, bytes),
            (None, None) => {
                return Err(invalid(
                    "contract declares a `hidden_correctness_golden` block that pins NOTHING: \
                     sha256 and bytes are both absent. A track that pins no correctness golden omits \
                     the block entirely — a present-but-empty block is a contract defect, not an \
                     opt-out",
                ))
            }
            _ => {
                return Err(invalid(
                    "contract hidden_correctness_golden declares only HALF a pin: sha256 and bytes \
                     must both be present (a half-declared pin is never treated as no pin)",
                ))
            }
        };
        if !is_sixtyfour_lowercase_hex(sha256) {
            return Err(invalid(format!(
                "contract hidden_correctness_golden.sha256 {sha256:?} is not a 64-character \
                 lowercase hex digest — a NAME-FREE integrity pin is sha256+bytes, and a malformed \
                 sha256 can pin nothing"
            )));
        }
        if bytes == 0 {
            return Err(invalid(
                "contract hidden_correctness_golden.bytes is 0 — a zero byte count pins no artifact",
            ));
        }
        Ok(CorrectnessGoldenPin {
            sha256: sha256.to_string(),
            bytes,
        })
    }
}

/// `^[0-9a-f]{64}$` — the sha256 shape a NAME-FREE integrity pin must carry, the 64-hex analogue of
/// [`is_forty_lowercase_hex`].
fn is_sixtyfour_lowercase_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_lowercase() && b <= b'f'))
}

/// Read a track contract fixture's pinned correctness-golden identity from the contract BYTES.
///
/// `Ok(None)` means the fixture carries NO `hidden_correctness_golden` key at all (offline/legacy
/// fixtures) — the track pins no correctness golden and the caller enforces nothing. Every other
/// outcome is an error: a block that declares neither half, half a pin, a non-sha256 value, a zero
/// byte count, an explicit `null`, or contract JSON that will not decode. A contract that cannot be
/// read is never treated as one pinning no correctness golden.
///
/// This reads a SIBLING of `timed_prompt_pool`; it decodes only the one field and touches the pool
/// not at all, so the anti-lottery cardinality is provably unperturbed by it.
pub fn hidden_correctness_golden_pin_from_contract(
    bytes: &[u8],
) -> Result<Option<CorrectnessGoldenPin>> {
    let envelope: ContractCorrectnessGoldenEnvelope =
        serde_json::from_slice(bytes).map_err(|e| {
            invalid(format!(
                "contract could not be decoded for its correctness-golden pin: {e}"
            ))
        })?;
    match envelope.hidden_correctness_golden {
        Some(sibling) => sibling.pin().map(Some),
        None => Ok(None),
    }
}

/// Verify a run's correctness-golden ATTESTATION cites the contract's pinned identity (LANE 2a).
///
/// `attested` is the identity (sha256+bytes) the run declares it verified correctness against —
/// benchd computes it from the staged `--correctness-golden` bytes, so it is the REAL artifact's
/// identity, not a self-declared label. `fixture_pin` is the contract's review-gated authority.
///
/// FAIL-CLOSED in BOTH directions, so neither the run nor the fixture can fall open:
/// - fixture pins it, attestation ABSENT ⇒ error — a track that pins the correctness golden REQUIRES
///   the run to attest it (a scoring run may not silently skip the correctness authority);
/// - attestation present, fixture pins NOTHING ⇒ error — benchd cannot authorize an un-pinned
///   correctness golden against a track that declares no authority for it;
/// - both present, sha256 OR bytes differ ⇒ error — the wrong-digest REFUSAL;
/// - both present and equal ⇒ Ok;
/// - NEITHER present ⇒ Ok — an offline/legacy track with no correctness-golden authority, exactly as
///   [`reference_model_pin_from_contract`] returning `None` leaves a contract-less caller unbound.
///
/// Diagnostics carry the pin (sha256+bytes) only; the golden's NAME never appears.
pub fn verify_correctness_golden_attestation(
    attested: Option<&CorrectnessGoldenPin>,
    fixture_pin: Option<&CorrectnessGoldenPin>,
) -> Result<()> {
    match (attested, fixture_pin) {
        (None, None) => Ok(()),
        (None, Some(pin)) => Err(invalid(format!(
            "the --contract fixture pins a hidden correctness golden (sha256 {} / {} bytes) but the \
             run carries no correctness-golden attestation — a track that pins the correctness \
             golden requires the run to attest it (fail-closed)",
            pin.sha256, pin.bytes
        ))),
        (Some(att), None) => Err(invalid(format!(
            "the run attests a correctness golden (sha256 {} / {} bytes) but the --contract fixture \
             pins no hidden correctness golden to authorize it against — benchd cannot authorize an \
             un-pinned correctness golden (fail-closed)",
            att.sha256, att.bytes
        ))),
        (Some(att), Some(pin)) => {
            if att.sha256 != pin.sha256 || att.bytes != pin.bytes {
                return Err(invalid(format!(
                    "the run's correctness-golden attestation (sha256 {} / {} bytes) does not cite \
                     the --contract fixture's pinned hidden correctness golden (sha256 {} / {} \
                     bytes) — wrong-digest, refused",
                    att.sha256, att.bytes, pin.sha256, pin.bytes
                )));
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenDocument {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deny_explicit_null"
    )]
    pub model_type: Option<String>,
    /// Optional additive provenance (see [`GoldenModelProvenance`]); rejected when explicitly
    /// `null`, exactly as the reference rejects a non-object `model_provenance`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deny_explicit_null"
    )]
    pub model_provenance: Option<GoldenModelProvenance>,
    pub cases: Vec<GoldenCase>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deny_explicit_null"
    )]
    pub correctness_gates: Option<GoldenCorrectnessGates>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deny_explicit_null"
    )]
    pub benchmark: Option<BenchmarkGolden>,
}

/// A validated golden plus the sha256 of the exact bytes it was loaded from.
#[derive(Debug, Clone, PartialEq)]
pub struct GoldenFixture {
    pub model_type: Option<String>,
    /// The document's optional `model_provenance` block, carried through the load.
    pub model_provenance: Option<GoldenModelProvenance>,
    pub cases: Vec<GoldenCase>,
    pub correctness_gates: Option<GoldenCorrectnessGates>,
    pub benchmark: Option<BenchmarkGolden>,
    pub sha256: String,
    /// #112 (L3) — the COUNT of those same bytes. A canonical golden is identified by
    /// `sha256` + `bytes` TOGETHER (see [`GoldenIntegrityPin`], which carries exactly that
    /// pair), so the loader records both halves rather than making a consumer re-stat the file.
    pub byte_len: u64,
}

impl GoldenFixture {
    pub fn total_correctness_case_count(&self) -> usize {
        self.cases.len()
            + self
                .correctness_gates
                .as_ref()
                .map(|g| g.total_case_count())
                .unwrap_or(0)
    }
}

fn invalid(msg: impl Into<String>) -> BenchError {
    BenchError::InvalidInput(msg.into())
}

/// Port of `loadGoldenFixture`. Validates raw bytes and returns a `GoldenFixture`.
///
/// When `pin` is `Some`, the sha256+byte-count integrity gate is enforced on the RAW
/// BYTES BEFORE the parse (see [`verify_golden_integrity`]) — folded into the loader so
/// a pinned load is a single call that cannot be accidentally skipped by a consumer.
/// When `pin` is `None` the golden is loaded unpinned, exactly as before.
///
/// `reference_model` is the TRACK CONTRACT's declared reference-model identity (#114). When
/// `Some`, a `model_provenance` block must NAME that model or the load fails with the reference's
/// own diagnostic ([`verify_model_provenance_pinned`]) — folded into the loader for the same
/// reason the integrity pin is: a consumer holding a contract cannot forget to apply it. When
/// `None` (a caller with no contract in hand) the provenance block is validated for SHAPE only.
pub fn load_golden_fixture(
    bytes: &[u8],
    required_steps: usize,
    required_prompt_tokens: usize,
    required_model_type: Option<&str>,
    pin: Option<&GoldenIntegrityPin>,
    reference_model: Option<&ReferenceModelPin>,
) -> Result<GoldenFixture> {
    // Integrity pin (when given) is checked on the raw bytes BEFORE any parse, so a
    // tampered/unknown-provenance golden is rejected before its contents are ever trusted.
    //
    // #58: the digest computed for the pin check is KEPT and reused as the fixture's
    // `sha256` below — a pinned load used to hash the same bytes twice (once here, once
    // when building the fixture). Unpinned loads still hash lazily, after validation, so a
    // rejected golden is never hashed at all.
    let pinned_digest = match pin {
        Some(pin) => {
            let digest = sha256_hex(bytes);
            verify_golden_integrity_precomputed(bytes.len() as u64, &digest, pin)?;
            Some(digest)
        }
        None => None,
    };

    if required_steps == 0 {
        return Err(invalid("correctness required steps must be positive"));
    }
    if required_prompt_tokens == 0 {
        return Err(invalid(
            "correctness required prompt tokens must be positive",
        ));
    }

    let decoded: GoldenDocument = serde_json::from_slice(bytes)
        .map_err(|e| invalid(format!("correctness golden file could not be decoded: {e}")))?;

    if decoded.version != Some(1) {
        return Err(invalid("correctness golden file version must be 1"));
    }

    if let Some(model_type) = &decoded.model_type {
        if model_type.is_empty() || model_type != model_type.trim() {
            return Err(invalid(
                "correctness golden file model_type must be a non-empty trimmed string",
            ));
        }
    }
    // SHAPE of `model_provenance`. In the reference this is part of the pre-decode key/value pass
    // (`validateGoldenFixtureKeys` → `validateGoldenModelProvenanceKeys`, Golden.swift:803-829),
    // which runs BEFORE the version and model_type gates — so a malformed block reports its own
    // defect no matter what else is wrong with the document.
    if let Some(provenance) = &decoded.model_provenance {
        validate_model_provenance(provenance)?;
    }
    if let Some(required) = required_model_type {
        if decoded.model_type.as_deref() != Some(required) {
            return Err(invalid(format!(
                "correctness golden file model_type={:?} expected {}",
                decoded.model_type, required
            )));
        }
    }
    // #114 (F1) — the IDENTITY half, and it belongs HERE: the reference interleaves
    // `requiredModelType` (Golden.swift:377-385) BEFORE the provenance identity guard (:386-393).
    // The first draft ran this before the model_type gate, which was decision-identical but
    // emitted a DIFFERENT diagnostic than Swift for a golden wrong in BOTH — a divergence
    // invisible to a harness that only diffs accept/reject. Corpus row:
    // `wrong_model_type_and_provenance.json`.
    if let Some(provenance) = &decoded.model_provenance {
        if let Some(reference_model) = reference_model {
            verify_model_provenance_pinned(provenance, reference_model)?;
        }
    }

    validate_golden_cases(&decoded.cases, required_steps, required_prompt_tokens)?;

    if let Some(gates) = &decoded.correctness_gates {
        validate_golden_correctness_gates(gates, &decoded.cases, required_prompt_tokens)?;
    }
    if let Some(benchmark) = &decoded.benchmark {
        validate_benchmark_golden(benchmark)?;
    }

    // Reuse the pin-check digest when there was one (#58); otherwise hash now.
    let hash = pinned_digest.unwrap_or_else(|| sha256_hex(bytes));

    Ok(GoldenFixture {
        model_type: decoded.model_type,
        model_provenance: decoded.model_provenance,
        cases: decoded.cases,
        correctness_gates: decoded.correctness_gates,
        benchmark: decoded.benchmark,
        sha256: hash,
        byte_len: bytes.len() as u64,
    })
}

/// Convenience: read the file at `path` and validate it. Uses the correctness
/// defaults (`CORRECTNESS_STEPS`, `CORRECTNESS_PROMPT_TOKENS`).
pub fn load_golden_fixture_from_path(
    path: &std::path::Path,
    required_steps: usize,
    required_prompt_tokens: usize,
    required_model_type: Option<&str>,
    pin: Option<&GoldenIntegrityPin>,
    reference_model: Option<&ReferenceModelPin>,
) -> Result<GoldenFixture> {
    let bytes = std::fs::read(path).map_err(|e| {
        invalid(format!(
            "correctness golden file could not be read ({}): {e}",
            path.display()
        ))
    })?;
    load_golden_fixture(
        &bytes,
        required_steps,
        required_prompt_tokens,
        required_model_type,
        pin,
        reference_model,
    )
}

// --- integrity pin gate (ported from .github/scripts/verify-correctness-golden.sh) ---

/// The exact identity a target's signed `target.toml` commits a golden to: the
/// sha256 of the raw bytes and their count. Canonical goldens are identified by
/// this pin, never by what happens to parse — a mirror artifact or a tampered
/// golden that decodes cleanly must still fail this gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenIntegrityPin {
    pub sha256: String,
    pub bytes: u64,
}

/// Verify a golden's raw bytes against its pin BEFORE any parse. Ports
/// `verify-correctness-golden.sh`: byte count then sha256, both fail-closed. Prefer
/// passing the pin to [`load_golden_fixture`] (which calls this before the parse) so a
/// pinned load is one call and cannot be skipped; call this directly only when a caller
/// needs the pin check as a distinct step (e.g. a bespoke exit-code/diagnostic contract).
pub fn verify_golden_integrity(bytes: &[u8], pin: &GoldenIntegrityPin) -> Result<()> {
    verify_golden_integrity_precomputed(bytes.len() as u64, &sha256_hex(bytes), pin)
}

/// [`verify_golden_integrity`] against an ALREADY-COMPUTED digest, so a caller that has
/// just hashed the bytes for another purpose does not hash them a second time (#58).
/// Byte count is still checked first, then the sha256 — both fail-closed, same messages.
///
/// `actual_bytes` and `actual_sha256` MUST describe the same buffer; callers inside this
/// module derive both from one `bytes` slice.
fn verify_golden_integrity_precomputed(
    actual_bytes: u64,
    actual_sha256: &str,
    pin: &GoldenIntegrityPin,
) -> Result<()> {
    if actual_bytes != pin.bytes {
        return Err(invalid(format!(
            "correctness golden byte count mismatch: expected {}, actual {actual_bytes}",
            pin.bytes
        )));
    }
    if !actual_sha256.eq_ignore_ascii_case(pin.sha256.trim()) {
        return Err(invalid(format!(
            "correctness golden sha256 mismatch: expected {}, actual {actual_sha256}",
            pin.sha256
        )));
    }
    Ok(())
}

// --- explicit-null rejection helper ---

/// Rejects an explicit JSON `null` for an optional field while still allowing the
/// field to be absent. serde uses the field's `default` when the key is missing and
/// only invokes this when the key is present, so `Some(_)` passes through, a present
/// `null` is rejected, and an absent field stays `None`. Ports the Swift loader's
/// `must not be null` guards for `model_type`, `correctness_gates`, `benchmark`, and
/// the gate sections without a second parse pass. Per-case optional fields are left
/// lenient (as the old key-set pass never descended into case objects).
fn deny_explicit_null<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    match Option::<T>::deserialize(deserializer)? {
        Some(value) => Ok(Some(value)),
        None => Err(serde::de::Error::custom("must not be null")),
    }
}

// --- typed validation ---

fn validate_tokens(tokens: &[Token], field: &str) -> Result<()> {
    for (index, token) in tokens.iter().enumerate() {
        if *token < 0 || *token >= VOCAB_SIZE as i64 {
            return Err(invalid(format!(
                "{field}[{index}]={token} is outside configured vocab range 0..<{VOCAB_SIZE}"
            )));
        }
    }
    Ok(())
}

/// Port of Swift `validateGoldenModelProvenanceKeys` VALUE semantics (the key-set half is
/// `deny_unknown_fields` on [`GoldenModelProvenance`]; both keys being required is serde's own
/// missing-field error): a non-empty `repository` and a 40-character lowercase-hex `revision`.
/// The reference's additional comparison of the VALUES against a pinned reference model is
/// [`verify_model_provenance_pinned`], applied by the loader when the caller supplies the track
/// contract's pin (#114) — shape first, identity second, the reference's own order.
fn validate_model_provenance(provenance: &GoldenModelProvenance) -> Result<()> {
    let revision_ok = provenance.revision.len() == 40
        && provenance
            .revision
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() && b <= b'f');
    if provenance.repository.is_empty() || !revision_ok {
        return Err(invalid(
            "model_provenance requires repository and a 40-character lowercase revision",
        ));
    }
    Ok(())
}

fn validate_case_name(name: &str, field: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("{field} must not be empty")));
    }
    if name != trimmed {
        return Err(invalid(format!(
            "{field} {name:?} must not have leading or trailing whitespace"
        )));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(invalid(format!(
            "{field} {name:?} must not contain control characters"
        )));
    }
    Ok(())
}

fn validate_golden_cases(
    cases: &[GoldenCase],
    required_steps: usize,
    required_prompt_tokens: usize,
) -> Result<()> {
    if cases.is_empty() {
        return Err(invalid(
            "correctness golden file must contain at least one case",
        ));
    }
    let mut names = std::collections::HashSet::new();
    for c in cases {
        validate_case_name(&c.name, "correctness golden case name")?;
        if !names.insert(c.name.clone()) {
            return Err(invalid(format!(
                "duplicate correctness golden case name {}",
                c.name
            )));
        }
        if c.prompt_tokens.len() != required_prompt_tokens {
            return Err(invalid(format!(
                "{}.prompt_tokens has {} tokens; need exactly {}",
                c.name,
                c.prompt_tokens.len(),
                required_prompt_tokens
            )));
        }
        if c.expected_tokens.len() < required_steps {
            return Err(invalid(format!(
                "{}.expected_tokens has {} tokens; need at least {}",
                c.name,
                c.expected_tokens.len(),
                required_steps
            )));
        }
        validate_tokens(&c.prompt_tokens, &format!("{}.prompt_tokens", c.name))?;
        validate_tokens(&c.expected_tokens, &format!("{}.expected_tokens", c.name))?;
    }
    Ok(())
}

fn validate_golden_correctness_gates(
    gates: &GoldenCorrectnessGates,
    base_cases: &[GoldenCase],
    required_prompt_tokens: usize,
) -> Result<()> {
    // Structural guards the old key-set pass enforced but serde cannot express: the
    // gates object must declare at least one section, and any declared section must
    // be non-empty. (Explicit-null sections are already rejected at decode via
    // `deny_explicit_null`.)
    if gates.anchors.is_none() && gates.free_run.is_none() && gates.behavior.is_none() {
        return Err(invalid(
            "correctness_gates must contain at least one gate section",
        ));
    }
    for (section, is_empty) in [
        ("anchors", gates.anchors.as_ref().is_some_and(Vec::is_empty)),
        (
            "free_run",
            gates.free_run.as_ref().is_some_and(Vec::is_empty),
        ),
        (
            "behavior",
            gates.behavior.as_ref().is_some_and(Vec::is_empty),
        ),
    ] {
        if is_empty {
            return Err(invalid(format!(
                "correctness_gates.{section} must not be empty when present"
            )));
        }
    }
    validate_golden_anchor_cases(gates.anchor_cases())?;
    validate_golden_free_run_cases(gates.free_run_cases(), required_prompt_tokens)?;
    validate_golden_behavior_cases(gates.behavior_cases())?;
    validate_unique_layered_case_names(base_cases, gates)?;
    Ok(())
}

fn validate_golden_anchor_cases(cases: &[GoldenAnchorCase]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for c in cases {
        validate_case_name(&c.name, "correctness anchor case name")?;
        if !names.insert(c.name.clone()) {
            return Err(invalid(format!(
                "duplicate correctness anchor case name {}",
                c.name
            )));
        }
        if c.context_tokens.is_empty() {
            return Err(invalid(format!(
                "{}.context_tokens must not be empty",
                c.name
            )));
        }
        if c.context_tokens.len() > CORRECTNESS_MAX_ANCHOR_CONTEXT_TOKENS {
            return Err(invalid(format!(
                "{}.context_tokens has {} tokens; maximum is {}",
                c.name,
                c.context_tokens.len(),
                CORRECTNESS_MAX_ANCHOR_CONTEXT_TOKENS
            )));
        }
        validate_tokens(&c.context_tokens, &format!("{}.context_tokens", c.name))?;
        validate_tokens(&[c.expected_token], &format!("{}.expected_token", c.name))?;
        if let Some(accepted) = &c.accepted_tokens {
            if accepted.is_empty() {
                return Err(invalid(format!(
                    "{}.accepted_tokens must not be empty when present",
                    c.name
                )));
            }
            validate_tokens(accepted, &format!("{}.accepted_tokens", c.name))?;
        }
        if let Some(rank) = c.max_expected_rank {
            if rank == 0 || rank > CORRECTNESS_TOP_LOGITS {
                return Err(invalid(format!(
                    "{}.max_expected_rank must be in 1...{}",
                    c.name, CORRECTNESS_TOP_LOGITS
                )));
            }
        }
        if let Some(delta) = c.max_top_logit_delta {
            if !delta.is_finite() || delta < 0.0 {
                return Err(invalid(format!(
                    "{}.max_top_logit_delta must be finite and non-negative",
                    c.name
                )));
            }
            if c.max_expected_rank.is_none() {
                return Err(invalid(format!(
                    "{}.max_top_logit_delta requires max_expected_rank",
                    c.name
                )));
            }
        }
    }
    Ok(())
}

fn validate_golden_free_run_cases(
    cases: &[GoldenFreeRunCase],
    required_prompt_tokens: usize,
) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for c in cases {
        validate_case_name(&c.name, "correctness free-run case name")?;
        if !names.insert(c.name.clone()) {
            return Err(invalid(format!(
                "duplicate correctness free-run case name {}",
                c.name
            )));
        }
        if c.prompt_tokens.len() != required_prompt_tokens {
            return Err(invalid(format!(
                "{}.prompt_tokens has {} tokens; need exactly {}",
                c.name,
                c.prompt_tokens.len(),
                required_prompt_tokens
            )));
        }
        if c.expected_tokens.is_empty() {
            return Err(invalid(format!(
                "{}.expected_tokens must not be empty",
                c.name
            )));
        }
        if c.expected_tokens.len() > CORRECTNESS_MAX_FREE_RUN_STEPS {
            return Err(invalid(format!(
                "{}.expected_tokens has {} tokens; maximum is {}",
                c.name,
                c.expected_tokens.len(),
                CORRECTNESS_MAX_FREE_RUN_STEPS
            )));
        }
        if let Some(prefix) = c.exact_prefix_tokens {
            if prefix == 0 || prefix > c.expected_tokens.len() {
                return Err(invalid(format!(
                    "{}.exact_prefix_tokens must be in 1...{}",
                    c.name,
                    c.expected_tokens.len()
                )));
            }
        }
        validate_tokens(&c.prompt_tokens, &format!("{}.prompt_tokens", c.name))?;
        validate_tokens(&c.expected_tokens, &format!("{}.expected_tokens", c.name))?;
    }
    Ok(())
}

fn validate_golden_behavior_cases(cases: &[GoldenBehaviorCase]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for c in cases {
        validate_case_name(&c.name, "correctness behavior case name")?;
        if !names.insert(c.name.clone()) {
            return Err(invalid(format!(
                "duplicate correctness behavior case name {}",
                c.name
            )));
        }
        if c.prompt_tokens.is_empty() {
            return Err(invalid(format!(
                "{}.prompt_tokens must not be empty",
                c.name
            )));
        }
        if c.prompt_tokens.len() > CORRECTNESS_MAX_BEHAVIOR_PROMPT_TOKENS {
            return Err(invalid(format!(
                "{}.prompt_tokens has {} tokens; maximum is {}",
                c.name,
                c.prompt_tokens.len(),
                CORRECTNESS_MAX_BEHAVIOR_PROMPT_TOKENS
            )));
        }
        if c.max_new_tokens == 0 || c.max_new_tokens > CORRECTNESS_MAX_BEHAVIOR_STEPS {
            return Err(invalid(format!(
                "{}.max_new_tokens must be in 1...{}",
                c.name, CORRECTNESS_MAX_BEHAVIOR_STEPS
            )));
        }
        if c.accepted_token_sequences.is_empty() {
            return Err(invalid(format!(
                "{}.accepted_token_sequences must not be empty",
                c.name
            )));
        }
        for (index, sequence) in c.accepted_token_sequences.iter().enumerate() {
            if sequence.is_empty() {
                return Err(invalid(format!(
                    "{}.accepted_token_sequences[{index}] must not be empty",
                    c.name
                )));
            }
            if sequence.len() > c.max_new_tokens {
                return Err(invalid(format!(
                    "{}.accepted_token_sequences[{index}] has {} tokens; maximum is max_new_tokens {}",
                    c.name,
                    sequence.len(),
                    c.max_new_tokens
                )));
            }
            validate_tokens(
                sequence,
                &format!("{}.accepted_token_sequences[{index}]", c.name),
            )?;
        }
        validate_tokens(&c.prompt_tokens, &format!("{}.prompt_tokens", c.name))?;
    }
    Ok(())
}

fn validate_unique_layered_case_names(
    base_cases: &[GoldenCase],
    gates: &GoldenCorrectnessGates,
) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for c in base_cases {
        names.insert(c.name.clone());
    }
    let layered = gates
        .anchor_cases()
        .iter()
        .map(|c| &c.name)
        .chain(gates.free_run_cases().iter().map(|c| &c.name))
        .chain(gates.behavior_cases().iter().map(|c| &c.name));
    for name in layered {
        if !names.insert(name.clone()) {
            return Err(invalid(format!(
                "duplicate layered correctness case name {name}"
            )));
        }
    }
    Ok(())
}

/// Port of `validateBenchmarkGolden`.
pub fn validate_benchmark_golden(benchmark: &BenchmarkGolden) -> Result<()> {
    if benchmark.prefill_prompt_tokens.len() != BENCHMARK_PREFILL_PROMPT_TOKENS {
        return Err(invalid(format!(
            "benchmark.prefill_prompt_tokens has {} tokens; need exactly {}",
            benchmark.prefill_prompt_tokens.len(),
            BENCHMARK_PREFILL_PROMPT_TOKENS
        )));
    }
    if benchmark.decode_seed_tokens.len() != BENCHMARK_DECODE_SEED_TOKENS {
        return Err(invalid(format!(
            "benchmark.decode_seed_tokens has {} tokens; need exactly {}. Replace stale local goldens with an updated precomputed golden fixture.",
            benchmark.decode_seed_tokens.len(),
            BENCHMARK_DECODE_SEED_TOKENS
        )));
    }
    if benchmark.expected_decode_tokens.len() < BENCHMARK_DECODE_STEPS {
        return Err(invalid(format!(
            "benchmark.expected_decode_tokens has {} tokens; need at least {}. Replace stale local goldens with an updated precomputed golden fixture.",
            benchmark.expected_decode_tokens.len(),
            BENCHMARK_DECODE_STEPS
        )));
    }
    validate_tokens(
        &benchmark.prefill_prompt_tokens,
        "benchmark.prefill_prompt_tokens",
    )?;
    validate_tokens(
        &[benchmark.expected_prefill_token],
        "benchmark.expected_prefill_token",
    )?;
    validate_tokens(
        &benchmark.decode_seed_tokens,
        "benchmark.decode_seed_tokens",
    )?;
    validate_tokens(
        &[benchmark.expected_decode_seed_token],
        "benchmark.expected_decode_seed_token",
    )?;
    validate_tokens(
        &benchmark.expected_decode_tokens,
        "benchmark.expected_decode_tokens",
    )?;
    validate_benchmark_golden_baselines(benchmark)?;
    Ok(())
}

fn validate_benchmark_golden_baselines(benchmark: &BenchmarkGolden) -> Result<()> {
    // All-or-nothing: a half-calibrated oracle would mix two calibration regimes.
    match (
        benchmark.baseline_prefill_seconds_per_token,
        benchmark.baseline_decode_seconds_per_token,
    ) {
        (None, None) => Ok(()),
        (None, _) | (_, None) => Err(invalid(
            "benchmark.baseline_prefill_seconds_per_token and benchmark.baseline_decode_seconds_per_token must be provided together",
        )),
        (Some(prefill), Some(decode)) => {
            if !prefill.is_finite() || prefill <= 0.0 {
                return Err(invalid(
                    "benchmark.baseline_prefill_seconds_per_token must be finite and positive",
                ));
            }
            if !decode.is_finite() || decode <= 0.0 {
                return Err(invalid(
                    "benchmark.baseline_decode_seconds_per_token must be finite and positive",
                ));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // Build a valid minimal base-cases-only golden with the given prompt/step sizes.
    fn minimal_doc(prompt_tokens: usize, steps: usize) -> Value {
        json!({
            "version": 1,
            "cases": [
                {
                    "name": "case-a",
                    "prompt_tokens": vec![1i64; prompt_tokens],
                    "expected_tokens": vec![2i64; steps],
                }
            ]
        })
    }

    fn load(v: &Value) -> Result<GoldenFixture> {
        let bytes = serde_json::to_vec(v).unwrap();
        // Use small required sizes so tests stay compact.
        load_golden_fixture(&bytes, 4, 3, None, None, None)
    }

    #[test]
    fn valid_minimal_parses() {
        let fx = load(&minimal_doc(3, 4)).unwrap();
        assert_eq!(fx.cases.len(), 1);
        assert_eq!(fx.total_correctness_case_count(), 1);
        assert_eq!(fx.sha256.len(), 64);
    }

    #[test]
    fn unknown_top_level_key_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert("bogus".into(), json!(1));
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("bogus")
        ));
    }

    /// A stand-in reference-model repository id. benchd names NO model in code — the real track's
    /// reference is whatever its CONTRACT declares, and the fixture behind it is identified by
    /// sha256 + bytes (see `HCG_SHA`/`HCG_BYTES`), never by name. These tests therefore only need
    /// a well-formed `org/name` to exercise the loader's shape, value and pin-comparison rules;
    /// the specific string is arbitrary and deliberately carries no real repository.
    const REF_REPO: &str = "reference-org/Reference-27B-4bit";

    /// A well-formed `model_provenance` block: exactly `repository` + a 40-hex `revision`.
    fn provenance() -> serde_json::Value {
        json!({
            "repository": REF_REPO,
            "revision": "eda45ab47f465d08d6558f0353a2346e2eb9d5b3",
        })
    }

    #[test]
    fn model_provenance_accepted_matching_swift() {
        // FLIPPED (engine #11 → PR #12, `2be4e21`): this test used to assert REJECTION, whose
        // premise was that the reference's allowed keys are version/model_type/cases/
        // correctness_gates/benchmark. The REFERENCE loader has since been corrected —
        // `model_type` restored as the schema key and `model_provenance` RETAINED as an
        // additional OPTIONAL key — so rejecting it is now the parity bug. (History: benchd
        // accepted it in 69adcad off a MIRROR file, reverted in 3270ae6 as mirror-driven, and
        // re-lands here off the corrected REFERENCE.)
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("model_provenance".into(), provenance());
        let fx = load(&v).expect("a well-formed model_provenance must be ACCEPTED");
        let p = fx
            .model_provenance
            .expect("provenance carried through the load");
        assert_eq!(p.repository, REF_REPO);
        assert_eq!(p.revision, "eda45ab47f465d08d6558f0353a2346e2eb9d5b3");

        // Absent is still fine (the key is OPTIONAL, and the whole existing corpus omits it).
        let fx = load(&minimal_doc(3, 4)).unwrap();
        assert!(fx.model_provenance.is_none());
    }

    #[test]
    fn model_provenance_shape_is_held_to_the_reference() {
        // The schema widened by exactly one KNOWN key; the loader did not get looser.
        let with = |p: serde_json::Value| {
            let mut v = minimal_doc(3, 4);
            v.as_object_mut()
                .unwrap()
                .insert("model_provenance".into(), p);
            load(&v)
        };

        // An unknown key INSIDE the provenance object rejects (Swift: allowed keys are exactly
        // repository/revision). This is also the anti-cheat control for the flip above.
        let mut extra = provenance();
        extra
            .as_object_mut()
            .unwrap()
            .insert("model_id".into(), json!("x"));
        assert!(matches!(
            with(extra).unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("model_id")
        ));

        // Either key missing rejects.
        for key in ["repository", "revision"] {
            let mut p = provenance();
            p.as_object_mut().unwrap().remove(key);
            assert!(
                matches!(with(p).unwrap_err(), BenchError::InvalidInput(m) if m.contains(key)),
                "missing {key} must reject"
            );
        }

        // Explicit null rejects (Swift: "model_provenance must be a JSON object").
        assert!(with(json!(null)).is_err());
        // Non-object rejects.
        assert!(with(json!(REF_REPO)).is_err());

        // Value semantics: non-empty repository, 40 lowercase-hex revision.
        for bad in [
            json!({"repository": "", "revision": "eda45ab47f465d08d6558f0353a2346e2eb9d5b3"}),
            json!({"repository": "r", "revision": "y"}),
            json!({"repository": "r", "revision": "EDA45AB47F465D08D6558F0353A2346E2EB9D5B3"}),
            json!({"repository": "r", "revision": "eda45ab47f465d08d6558f0353a2346e2eb9d5b"}),
        ] {
            assert!(
                matches!(with(bad.clone()).unwrap_err(), BenchError::InvalidInput(m)
                    if m.contains("40-character lowercase revision")),
                "must reject {bad}"
            );
        }
    }

    /// Load with a track-contract reference-model pin applied (#114).
    fn load_pinned(v: &Value, pin: &ReferenceModelPin) -> Result<GoldenFixture> {
        let bytes = serde_json::to_vec(v).unwrap();
        load_golden_fixture(&bytes, 4, 3, None, None, Some(pin))
    }

    /// The contract fixture the corpus ships, in miniature: only the `target` block benchd reads.
    fn contract_bytes(model_id: Value, revision: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "track_id": "unit-test-track",
            "timed_prompt_pool": [],
            "target": {"upstream_model_id": model_id, "upstream_revision": revision},
        }))
        .unwrap()
    }

    #[test]
    fn contract_declares_the_reference_model_pin() {
        // The happy path: the challenger's `target.upstream_model_id`/`upstream_revision` pair IS
        // the pin, and the block's other keys (prose notes, quantization, …) are ignored.
        let bytes = contract_bytes(
            json!(REF_REPO),
            json!("eda45ab47f465d08d6558f0353a2346e2eb9d5b3"),
        );
        let pin = reference_model_pin_from_contract(&bytes)
            .unwrap()
            .expect("a fully-declared target is a pin");
        assert_eq!(pin.repository, REF_REPO);
        assert_eq!(pin.revision, "eda45ab47f465d08d6558f0353a2346e2eb9d5b3");

        // A contract with NO target declares no pin: legacy/offline fixtures keep the shape-only
        // behaviour rather than failing closed on a key they never carried.
        let no_target =
            serde_json::to_vec(&json!({"track_id": "t", "timed_prompt_pool": []})).unwrap();
        assert!(reference_model_pin_from_contract(&no_target)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_half_declared_contract_target_is_an_error_not_an_absent_pin() {
        // The one shape that must never fall open: a contract that LOOKS pinned but enforces
        // nothing. Exactly ONE half present (the other absent/null) is the half-declared case.
        for (model_id, revision) in [
            (json!(REF_REPO), Value::Null),
            (
                Value::Null,
                json!("eda45ab47f465d08d6558f0353a2346e2eb9d5b3"),
            ),
        ] {
            let bytes = contract_bytes(model_id.clone(), revision.clone());
            assert!(
                matches!(
                    reference_model_pin_from_contract(&bytes).unwrap_err(),
                    BenchError::InvalidInput(m) if m.contains("half a reference-model pin")
                ),
                "half-declared target ({model_id}, {revision}) must be an ERROR"
            );
        }
        // A contract that is not JSON at all is an error too — never "unpinned".
        assert!(reference_model_pin_from_contract(b"{not json").is_err());
    }

    #[test]
    fn a_target_that_declares_neither_half_is_an_error_not_a_silent_opt_out() {
        // F2 — the fall-open the review found: the FIRST draft made one empty half a hard error
        // while BOTH empty returned `Ok(None)`, i.e. making the contract MORE broken turned an
        // error into a silent unpinned pass. Every one of these shapes is now an ERROR.
        let neither = [
            // `target: {}` — the block exists and names nothing.
            serde_json::to_vec(&json!({"track_id": "t", "target": {}})).unwrap(),
            // both keys explicitly null.
            contract_bytes(Value::Null, Value::Null),
            // an upstream key RENAME (the deny_unknown_fields hazard, closed from the other side):
            // the pin's keys are simply absent, so the block declares nothing and fails LOUD.
            serde_json::to_vec(&json!({
                "track_id": "t",
                "target": {"model_id": REF_REPO,
                           "revision": "eda45ab47f465d08d6558f0353a2346e2eb9d5b3"},
            }))
            .unwrap(),
        ];
        for bytes in neither {
            assert!(
                matches!(
                    reference_model_pin_from_contract(&bytes).unwrap_err(),
                    BenchError::InvalidInput(m) if m.contains("names NO reference model")
                ),
                "a present-but-undeclaring target must be an ERROR: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        // An explicit `"target": null` is likewise NOT read as "undeclared".
        let explicit_null = serde_json::to_vec(&json!({"track_id": "t", "target": null})).unwrap();
        assert!(reference_model_pin_from_contract(&explicit_null).is_err());

        // Only the ABSENCE of the key is the opt-out, and it still is.
        let absent = serde_json::to_vec(&json!({"track_id": "t"})).unwrap();
        assert!(reference_model_pin_from_contract(&absent)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_placeholder_revision_is_a_contract_defect_not_a_pin() {
        // F3 — the CUDA track fixture ships `upstream_revision:
        // "QWEN-MTP-CUDA-PENDING-ORGANIZER"` TODAY. Accepting a non-sha value as a pin would make
        // every provenance-bearing golden reject with a diagnostic pointing at the GOLDEN. The
        // defect is named where it lives, and the revision is held to the same `^[0-9a-f]{40}$`
        // rule the reference applies to a golden's own revision.
        for bad_revision in [
            "QWEN-MTP-CUDA-PENDING-ORGANIZER",
            "EDA45AB47F465D08D6558F0353A2346E2EB9D5B3", // uppercase
            "eda45ab47f465d08d6558f0353a2346e2eb9d5b",  // 39 chars
            "eda45ab47f465d08d6558f0353a2346e2eb9d5b3f", // 41 chars
            "zda45ab47f465d08d6558f0353a2346e2eb9d5b3", // non-hex
        ] {
            let bytes = contract_bytes(json!(REF_REPO), json!(bad_revision));
            assert!(
                matches!(
                    reference_model_pin_from_contract(&bytes).unwrap_err(),
                    BenchError::InvalidInput(m)
                        if m.contains("is not a 40-character lowercase") && m.contains("contract")
                ),
                "revision {bad_revision:?} must be a CONTRACT defect"
            );
        }
    }

    #[test]
    fn contract_values_are_not_trimmed_padding_is_a_contract_defect() {
        // F4 — the reference's analogous rule is that a padded value is a corpus defect "not
        // something to normalize away" (`validateGoldenModelType`, Golden.swift:783-800). The
        // first draft silently `trim()`ed contract values, which is the opposite policy: it would
        // have accepted a fixture whose bytes do not say what the pin enforces.
        for (model_id, revision, key) in [
            (
                json!(format!(" {REF_REPO}")),
                json!("eda45ab47f465d08d6558f0353a2346e2eb9d5b3"),
                "upstream_model_id",
            ),
            (
                json!(REF_REPO),
                json!("eda45ab47f465d08d6558f0353a2346e2eb9d5b3\n"),
                "upstream_revision",
            ),
        ] {
            let bytes = contract_bytes(model_id, revision);
            assert!(
                matches!(
                    reference_model_pin_from_contract(&bytes).unwrap_err(),
                    BenchError::InvalidInput(m)
                        if m.contains(key) && m.contains("leading or trailing whitespace")
                ),
                "a padded {key} must be a contract defect"
            );
        }
        // An EMPTY value is neither "absent" nor "a value" — also a defect, and it is reported as
        // an empty value rather than being folded into the half-declared message.
        for (model_id, revision) in [
            (json!(""), json!("eda45ab47f465d08d6558f0353a2346e2eb9d5b3")),
            (json!(REF_REPO), json!("   ")),
        ] {
            let bytes = contract_bytes(model_id, revision);
            let err = reference_model_pin_from_contract(&bytes).unwrap_err();
            assert!(
                matches!(&err, BenchError::InvalidInput(m)
                    if m.contains("EMPTY string") || m.contains("leading or trailing whitespace")),
                "empty/blank contract value must be a defect, got {err}"
            );
        }
    }

    #[test]
    fn a_real_shaped_target_block_with_prose_keys_still_yields_its_pin() {
        // Why `ContractReferenceTarget` is NOT `deny_unknown_fields` (F2, answered differently):
        // a REAL `target` block is mostly prose and unrelated pins. This is the live 3.8 fixture's
        // key set (values elided) plus the CUDA fixture's extra keys — denying unknown fields here
        // would refuse every real contract. The rename hazard is closed by the
        // "declares-neither-half" rule proven above instead.
        let bytes = serde_json::to_vec(&json!({
            "schema_version": 1,
            "track_id": "qwen3.8-27b-mtp-v1",
            "target": {
                "upstream_model_id": REF_REPO,
                "upstream_revision": "eda45ab47f465d08d6558f0353a2346e2eb9d5b3",
                "upstream_pin_note": "OUR OWN CONVERSION, decided 2026-08-14 …",
                "upstream_source_model_id": "Qwen/Qwen3.8-27B",
                "upstream_source_revision": "1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0",
                "geometry_note": "VERIFIED byte-identical to 3.6",
                "quantization": {"group_size": 64, "bits": 4, "mode": "affine"},
                "format": "safetensors",
                "ships_mtp_head": false,
                "manifest_path": "mtp-head.manifest.json",
            },
        }))
        .unwrap();
        let pin = reference_model_pin_from_contract(&bytes).unwrap().unwrap();
        assert_eq!(pin.repository, REF_REPO);
        assert_eq!(pin.revision, "eda45ab47f465d08d6558f0353a2346e2eb9d5b3");
    }

    // -- LANE 2a: the hidden correctness golden's SIBLING pin --------------------------------

    /// The live 3.8 track fixture's pinned identity (engine PR #41). NAME-FREE: sha256 + bytes only.
    const HCG_SHA: &str = "d7bebe67231e4e66a3134b25322f1dfaaf24543298c05f1d79e6166a48af1713";
    const HCG_BYTES: u64 = 16949;

    fn contract_with_hcg(hcg: Value) -> Vec<u8> {
        let mut root = json!({
            "schema_version": 1,
            "track_id": "qwen3.8-27b-mtp-v1",
            "timed_prompt_pool": [
                {"sha256": "aa", "bytes": 10, "noop_decode_speedup": 1.1},
                {"sha256": "bb", "bytes": 20, "noop_decode_speedup": 1.2},
            ],
        });
        root.as_object_mut()
            .unwrap()
            .insert("hidden_correctness_golden".into(), hcg);
        serde_json::to_vec(&root).unwrap()
    }

    #[test]
    fn contract_declares_the_correctness_golden_pin_as_a_sibling() {
        // The happy path (engine PR #41 shape): a top-level `hidden_correctness_golden` SIBLING of
        // `timed_prompt_pool`, pinned by sha256+bytes, its prose `_note` sibling ignored.
        let bytes = contract_with_hcg(json!({
            "sha256": HCG_SHA,
            "bytes": HCG_BYTES,
            "note": "prose that is ignored",
        }));
        let pin = hidden_correctness_golden_pin_from_contract(&bytes)
            .unwrap()
            .expect("a fully-declared sibling is a pin");
        assert_eq!(pin.sha256, HCG_SHA);
        assert_eq!(pin.bytes, HCG_BYTES);

        // ANTI-LOTTERY UNPERTURBED: the SIBLING pin must not join the pool. Decoding the pin does
        // not touch `timed_prompt_pool`, and the pool cardinality (N) is unchanged by its presence.
        let without = serde_json::to_vec(&json!({
            "timed_prompt_pool": [
                {"sha256": "aa", "bytes": 10, "noop_decode_speedup": 1.1},
                {"sha256": "bb", "bytes": 20, "noop_decode_speedup": 1.2},
            ],
        }))
        .unwrap();
        assert!(hidden_correctness_golden_pin_from_contract(&without)
            .unwrap()
            .is_none());

        // A contract with NO sibling key pins no correctness golden (offline/legacy).
        let no_key =
            serde_json::to_vec(&json!({"track_id": "t", "timed_prompt_pool": []})).unwrap();
        assert!(hidden_correctness_golden_pin_from_contract(&no_key)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_half_or_empty_correctness_golden_block_is_a_contract_defect() {
        // Present-but-undeclaring must fail LOUD, never fall open to "unpinned".
        for (block, needle) in [
            (json!({}), "pins NOTHING"),
            (json!({"sha256": null, "bytes": null}), "pins NOTHING"),
            (json!({"sha256": HCG_SHA}), "only HALF"),
            (json!({"bytes": HCG_BYTES}), "only HALF"),
        ] {
            let bytes = contract_with_hcg(block.clone());
            assert!(
                matches!(
                    hidden_correctness_golden_pin_from_contract(&bytes).unwrap_err(),
                    BenchError::InvalidInput(m) if m.contains(needle)
                ),
                "block {block} must be a contract defect ({needle})"
            );
        }
        // A non-sha256 value can pin nothing; a zero byte count pins no artifact.
        assert!(matches!(
            hidden_correctness_golden_pin_from_contract(&contract_with_hcg(
                json!({"sha256": "not-hex", "bytes": HCG_BYTES})
            ))
            .unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("lowercase hex")
        ));
        assert!(matches!(
            hidden_correctness_golden_pin_from_contract(&contract_with_hcg(
                json!({"sha256": HCG_SHA, "bytes": 0})
            ))
            .unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("byte count")
        ));
        // An explicit null sibling is NOT read as "undeclared"; unreadable JSON is never "unpinned".
        let explicit_null =
            serde_json::to_vec(&json!({"hidden_correctness_golden": null})).unwrap();
        assert!(hidden_correctness_golden_pin_from_contract(&explicit_null).is_err());
        assert!(hidden_correctness_golden_pin_from_contract(b"{not json").is_err());
    }

    #[test]
    fn correctness_golden_attestation_is_verified_both_directions() {
        let pin = CorrectnessGoldenPin {
            sha256: HCG_SHA.into(),
            bytes: HCG_BYTES,
        };
        // correct golden → PASSES (non-vacuous: a matching attestation clears).
        assert!(verify_correctness_golden_attestation(Some(&pin), Some(&pin)).is_ok());
        // neither present → OK (offline/legacy, no authority).
        assert!(verify_correctness_golden_attestation(None, None).is_ok());
        // wrong-digest (sha differs) → REFUSE.
        let wrong_sha = CorrectnessGoldenPin {
            sha256: "0".repeat(64),
            bytes: HCG_BYTES,
        };
        assert!(matches!(
            verify_correctness_golden_attestation(Some(&wrong_sha), Some(&pin)).unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("wrong-digest")
        ));
        // wrong-digest (bytes differ) → REFUSE.
        let wrong_bytes = CorrectnessGoldenPin {
            sha256: HCG_SHA.into(),
            bytes: HCG_BYTES + 1,
        };
        assert!(verify_correctness_golden_attestation(Some(&wrong_bytes), Some(&pin)).is_err());
        // fixture pins it, attestation ABSENT → FAIL-CLOSED.
        assert!(matches!(
            verify_correctness_golden_attestation(None, Some(&pin)).unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("fail-closed") && m.contains("no correctness-golden attestation")
        ));
        // attestation present, fixture pins NOTHING → FAIL-CLOSED.
        assert!(matches!(
            verify_correctness_golden_attestation(Some(&pin), None).unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("fail-closed") && m.contains("un-pinned correctness golden")
        ));
    }

    #[test]
    fn model_provenance_values_are_pinned_to_the_contract() {
        // #114 — the divergence this closes: a well-formed block naming a DIFFERENT model. The
        // shape check passes it; the contract pin rejects it, with the reference's own message.
        let pin = ReferenceModelPin {
            repository: REF_REPO.into(),
            revision: "eda45ab47f465d08d6558f0353a2346e2eb9d5b3".into(),
        };
        let with = |p: Value| {
            let mut v = minimal_doc(3, 4);
            v.as_object_mut()
                .unwrap()
                .insert("model_provenance".into(), p);
            v
        };

        // Names the pinned model → ACCEPT.
        assert!(load_pinned(&with(provenance()), &pin).is_ok());

        // Same valid SHAPE, different model → REJECT, byte-for-byte the reference's diagnostic
        // (Golden.swift:386-393). Both halves are pinned: a wrong repository and a wrong revision
        // each reject on their own, so neither is being compared vacuously.
        for wrong in [
            json!({"repository": "NotTheOrganizer/Some-Other-Model-4bit",
                   "revision": "eda45ab47f465d08d6558f0353a2346e2eb9d5b3"}),
            json!({"repository": REF_REPO,
                   "revision": "0123456789abcdef0123456789abcdef01234567"}),
        ] {
            assert!(
                matches!(
                    load_pinned(&with(wrong.clone()), &pin).unwrap_err(),
                    BenchError::InvalidInput(m)
                        if m == "correctness golden model_provenance does not match the pinned reference model"
                ),
                "a provenance naming {wrong} must be REJECTED against the contract pin"
            );
            // ...and the SAME bytes ACCEPT with no contract in hand: the strictness is the
            // contract's, not a constant benchd carries.
            assert!(load(&with(wrong)).is_ok());
        }

        // A golden with NO provenance block is untouched by the pin (the key stays OPTIONAL —
        // the pin is a value gate, not a requirement to declare provenance).
        assert!(load_pinned(&minimal_doc(3, 4), &pin).is_ok());

        // The pin is PER-TRACK: a second contract declaring a different reference accepts what the
        // first rejects, which is the property a compiled-in constant could never have.
        let other_track = ReferenceModelPin {
            repository: "NotTheOrganizer/Some-Other-Model-4bit".into(),
            revision: "0123456789abcdef0123456789abcdef01234567".into(),
        };
        let other = json!({"repository": "NotTheOrganizer/Some-Other-Model-4bit",
                           "revision": "0123456789abcdef0123456789abcdef01234567"});
        assert!(load_pinned(&with(other.clone()), &other_track).is_ok());
        assert!(load_pinned(&with(other), &pin).is_err());
    }

    #[test]
    fn the_shape_check_runs_before_the_contract_pin() {
        // Reference ORDER (Golden.swift): the key-set/shape validation precedes the identity
        // comparison, so a MALFORMED provenance reports its shape defect — not "does not match the
        // pinned reference model", which would send a submitter looking at the wrong thing.
        let pin = ReferenceModelPin {
            repository: REF_REPO.into(),
            revision: "eda45ab47f465d08d6558f0353a2346e2eb9d5b3".into(),
        };
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert(
            "model_provenance".into(),
            json!({"repository": "NotTheOrganizer/Some-Other-Model-4bit", "revision": "nope"}),
        );
        assert!(matches!(
            load_pinned(&v, &pin).unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("40-character lowercase revision")
        ));
    }

    #[test]
    fn required_model_type_is_reported_before_the_provenance_identity() {
        // F1 — the reference INTERLEAVES these two gates: `requiredModelType`
        // (Golden.swift:377-385) fires BEFORE the provenance identity guard (:386-393). A golden
        // wrong in BOTH must therefore report the MODEL_TYPE defect. The first draft checked the
        // pin first: still a REJECT, so loader-parity (which diffs decisions) stayed green while
        // the two loaders printed different diagnostics — a divergence invisible to the harness.
        let pin = ReferenceModelPin {
            repository: REF_REPO.into(),
            revision: "eda45ab47f465d08d6558f0353a2346e2eb9d5b3".into(),
        };
        let mut v = minimal_doc(3, 4);
        {
            let obj = v.as_object_mut().unwrap();
            obj.insert("model_type".into(), json!("gemma_text"));
            obj.insert(
                "model_provenance".into(),
                json!({"repository": "NotTheOrganizer/Some-Other-Model-4bit",
                       "revision": "0123456789abcdef0123456789abcdef01234567"}),
            );
        }
        let bytes = serde_json::to_vec(&v).unwrap();
        let err = load_golden_fixture(&bytes, 4, 3, Some("gemma4_text"), None, Some(&pin))
            .expect_err("a golden wrong in BOTH must reject");
        assert!(
            matches!(&err, BenchError::InvalidInput(m)
                if m.starts_with("correctness golden file model_type=")),
            "the MODEL_TYPE defect must be the one reported (reference interleave), got: {err}"
        );

        // Control: with the model_type CORRECT, the very same provenance reports the identity
        // defect — so the ordering above is an ordering, not the pin being unreachable.
        let mut ok_type = v.clone();
        ok_type
            .as_object_mut()
            .unwrap()
            .insert("model_type".into(), json!("gemma4_text"));
        let bytes = serde_json::to_vec(&ok_type).unwrap();
        let err = load_golden_fixture(&bytes, 4, 3, Some("gemma4_text"), None, Some(&pin))
            .expect_err("wrong model still rejects");
        assert!(matches!(&err, BenchError::InvalidInput(m)
            if m == "correctness golden model_provenance does not match the pinned reference model"));
    }

    #[test]
    fn an_unknown_top_level_key_that_is_not_model_provenance_still_rejects() {
        // CONTROL for the flip: widening by one known key must not widen anything else.
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("model_manifest".into(), provenance());
        assert!(matches!(
            load(&v).unwrap_err(),
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("model_manifest")
        ));
    }

    #[test]
    fn integrity_pin_accepts_exact_and_rejects_drift() {
        let bytes = br#"{"golden":true}"#;
        let sha = sha256_hex(bytes);
        let n = bytes.len() as u64;
        // Exact pin passes; sha match is case-insensitive.
        assert!(verify_golden_integrity(
            bytes,
            &GoldenIntegrityPin {
                sha256: sha.clone(),
                bytes: n
            }
        )
        .is_ok());
        assert!(verify_golden_integrity(
            bytes,
            &GoldenIntegrityPin {
                sha256: sha.to_uppercase(),
                bytes: n
            }
        )
        .is_ok());
        // Wrong byte count or wrong sha fails closed.
        assert!(verify_golden_integrity(
            bytes,
            &GoldenIntegrityPin {
                sha256: sha.clone(),
                bytes: n + 1
            }
        )
        .is_err());
        assert!(verify_golden_integrity(
            bytes,
            &GoldenIntegrityPin {
                sha256: "00".repeat(32),
                bytes: n
            }
        )
        .is_err());
    }

    #[test]
    fn loader_pin_param_enforces_before_parse() {
        // A structurally valid golden: pin present + matching -> loads identically to unpinned.
        let v = minimal_doc(3, 4);
        let bytes = serde_json::to_vec(&v).unwrap();
        let sha = sha256_hex(&bytes);
        let n = bytes.len() as u64;
        let pin = GoldenIntegrityPin {
            sha256: sha.clone(),
            bytes: n,
        };
        let pinned = load_golden_fixture(&bytes, 4, 3, None, Some(&pin), None).unwrap();
        let unpinned = load_golden_fixture(&bytes, 4, 3, None, None, None).unwrap();
        assert_eq!(pinned.sha256, unpinned.sha256);

        // Pin mismatch is caught BEFORE the parse, with the exact verify_golden_integrity
        // message (byte-count checked first) — even on bytes that would ALSO fail to parse.
        let garbage = b"not json at all";
        let bad_bytes_pin = GoldenIntegrityPin {
            sha256: sha.clone(),
            bytes: garbage.len() as u64 + 1,
        };
        let err = load_golden_fixture(garbage, 4, 3, None, Some(&bad_bytes_pin), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("correctness golden byte count mismatch"),
            "pin byte-count check must fire before parse: {err}"
        );
        // Wrong sha on otherwise-valid bytes fails closed with the sha message, not a parse error.
        let bad_sha_pin = GoldenIntegrityPin {
            sha256: "00".repeat(32),
            bytes: n,
        };
        let err = load_golden_fixture(&bytes, 4, 3, None, Some(&bad_sha_pin), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("correctness golden sha256 mismatch"),
            "pin sha check must reject before parse: {err}"
        );
    }

    #[test]
    fn version_not_one_rejected() {
        let mut v = minimal_doc(3, 4);
        v["version"] = json!(2);
        assert!(load(&v).is_err());
    }

    #[test]
    fn missing_version_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().remove("version");
        let err = load(&v).unwrap_err();
        assert!(matches!(err, BenchError::InvalidInput(m) if m.contains("version must be 1")));
    }

    #[test]
    fn wrong_prompt_token_count_rejected() {
        let v = minimal_doc(2, 4); // required is 3
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("prompt_tokens has 2 tokens"))
        );
    }

    #[test]
    fn too_few_expected_tokens_rejected() {
        let v = minimal_doc(3, 3); // required steps is 4
        let err = load(&v).unwrap_err();
        assert!(matches!(err, BenchError::InvalidInput(m) if m.contains("need at least 4")));
    }

    #[test]
    fn out_of_range_token_rejected() {
        let mut v = minimal_doc(3, 4);
        v["cases"][0]["expected_tokens"][0] = json!(VOCAB_SIZE as i64);
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("outside configured vocab range"))
        );
    }

    #[test]
    fn negative_token_rejected() {
        let mut v = minimal_doc(3, 4);
        v["cases"][0]["prompt_tokens"][0] = json!(-1);
        assert!(load(&v).is_err());
    }

    #[test]
    fn duplicate_case_name_rejected() {
        let mut v = minimal_doc(3, 4);
        let dup = v["cases"][0].clone();
        v["cases"].as_array_mut().unwrap().push(dup);
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("duplicate correctness golden case name"))
        );
    }

    #[test]
    fn case_name_with_whitespace_rejected() {
        let mut v = minimal_doc(3, 4);
        v["cases"][0]["name"] = json!(" case-a");
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("leading or trailing whitespace"))
        );
    }

    #[test]
    fn model_type_untrimmed_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("model_type".into(), json!(" qwen "));
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("non-empty trimmed string"))
        );
    }

    #[test]
    fn required_model_type_mismatch_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("model_type".into(), json!("gemma"));
        let bytes = serde_json::to_vec(&v).unwrap();
        let err = load_golden_fixture(&bytes, 4, 3, Some("gemma4_text"), None, None).unwrap_err();
        assert!(matches!(err, BenchError::InvalidInput(m) if m.contains("expected gemma4_text")));
    }

    // --- benchmark block ---

    fn valid_benchmark() -> Value {
        json!({
            "prefill_prompt_tokens": vec![1i64; BENCHMARK_PREFILL_PROMPT_TOKENS],
            "expected_prefill_token": 5,
            "decode_seed_tokens": vec![1i64; BENCHMARK_DECODE_SEED_TOKENS],
            "expected_decode_seed_token": 6,
            "expected_decode_tokens": vec![7i64; BENCHMARK_DECODE_STEPS],
        })
    }

    #[test]
    fn valid_benchmark_parses() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("benchmark".into(), valid_benchmark());
        let fx = load(&v).unwrap();
        let b = fx.benchmark.unwrap();
        // Absent baselines stay None (no official-constant fallback — F2); the caller
        // requires the golden's paired baselines.
        assert!(b.baseline_decode_seconds_per_token.is_none());
        assert!(b.baseline_prefill_seconds_per_token.is_none());
    }

    #[test]
    fn benchmark_only_one_baseline_rejected() {
        let mut b = valid_benchmark();
        b.as_object_mut()
            .unwrap()
            .insert("baseline_prefill_seconds_per_token".into(), json!(0.01));
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert("benchmark".into(), b);
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("must be provided together"))
        );
    }

    #[test]
    fn benchmark_both_baselines_ok() {
        let mut b = valid_benchmark();
        b.as_object_mut()
            .unwrap()
            .insert("baseline_prefill_seconds_per_token".into(), json!(0.01));
        b.as_object_mut()
            .unwrap()
            .insert("baseline_decode_seconds_per_token".into(), json!(0.1));
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert("benchmark".into(), b);
        let fx = load(&v).unwrap();
        let b = fx.benchmark.unwrap();
        assert_eq!(b.baseline_prefill_seconds_per_token, Some(0.01));
        assert_eq!(b.baseline_decode_seconds_per_token, Some(0.1));
    }

    #[test]
    fn benchmark_unknown_key_rejected() {
        let mut b = valid_benchmark();
        b.as_object_mut()
            .unwrap()
            .insert("baseline_prefil_typo".into(), json!(0.01));
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert("benchmark".into(), b);
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("baseline_prefil_typo")
        ));
    }

    // --- correctness gates ---

    #[test]
    fn empty_gate_section_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("correctness_gates".into(), json!({ "anchors": [] }));
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("must not be empty when present"))
        );
    }

    #[test]
    fn gates_unknown_key_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("correctness_gates".into(), json!({ "anchorz": [] }));
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("anchorz")
        ));
    }

    #[test]
    fn valid_anchor_gate_parses() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert(
            "correctness_gates".into(),
            json!({
                "anchors": [
                    {
                        "name": "anchor-1",
                        "context_tokens": [1, 2, 3],
                        "expected_token": 9,
                        "accepted_tokens": [9, 10],
                        "max_expected_rank": 3,
                        "max_top_logit_delta": 1.5
                    }
                ]
            }),
        );
        let fx = load(&v).unwrap();
        assert_eq!(fx.total_correctness_case_count(), 2);
    }

    #[test]
    fn anchor_delta_without_rank_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert(
            "correctness_gates".into(),
            json!({
                "anchors": [
                    { "name": "a", "context_tokens": [1], "expected_token": 9, "max_top_logit_delta": 1.0 }
                ]
            }),
        );
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("requires max_expected_rank"))
        );
    }

    #[test]
    fn layered_duplicate_name_rejected() {
        let mut v = minimal_doc(3, 4);
        // reuse the base case name "case-a" as a free-run name
        v.as_object_mut().unwrap().insert(
            "correctness_gates".into(),
            json!({
                "free_run": [
                    { "name": "case-a", "prompt_tokens": [1, 2, 3], "expected_tokens": [4, 5] }
                ]
            }),
        );
        let err = load(&v).unwrap_err();
        assert!(
            matches!(err, BenchError::InvalidInput(m) if m.contains("duplicate layered correctness case name"))
        );
    }

    // (#58) The sha256("abc") known-vector test moved to `crate::hash`, which now owns the
    // one implementation this module calls.

    #[test]
    fn fixture_sha256_is_of_raw_bytes() {
        let v = minimal_doc(3, 4);
        let bytes = serde_json::to_vec(&v).unwrap();
        let fx = load_golden_fixture(&bytes, 4, 3, None, None, None).unwrap();
        assert_eq!(fx.sha256, sha256_hex(&bytes));
    }

    /// #58: a PINNED load hashes the bytes ONCE and reuses that digest as the fixture's
    /// `sha256`. Behaviourally this must be indistinguishable from the unpinned load —
    /// same digest, same byte_len — which is what makes the de-duplication safe.
    #[test]
    fn pinned_load_reuses_the_pin_digest_as_the_fixture_sha256() {
        let v = minimal_doc(3, 4);
        let bytes = serde_json::to_vec(&v).unwrap();
        let expected = sha256_hex(&bytes);
        let pin = GoldenIntegrityPin {
            // Upper-case pin: the comparison is case-insensitive, but the digest the
            // fixture RECORDS must stay lowercase-hex (never the pin's spelling).
            sha256: expected.to_uppercase(),
            bytes: bytes.len() as u64,
        };
        let fx = load_golden_fixture(&bytes, 4, 3, None, Some(&pin), None).unwrap();
        assert_eq!(fx.sha256, expected);
        assert_eq!(fx.byte_len, bytes.len() as u64);
    }

    // --- per-case unknown-key strictness (S1) ---
    //
    // These typo'd per-case keys were SILENTLY ACCEPTED before `deny_unknown_fields`
    // descended into the case objects, quietly changing gate semantics (anti-cheat hole).

    #[test]
    fn base_case_misspelled_key_rejected() {
        let mut v = minimal_doc(3, 4);
        v["cases"][0]
            .as_object_mut()
            .unwrap()
            .insert("acepted_tokens".into(), json!([1, 2, 3]));
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("acepted_tokens")
        ));
    }

    #[test]
    fn anchor_case_misspelled_accepted_tokens_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert(
            "correctness_gates".into(),
            json!({
                "anchors": [
                    {
                        "name": "anchor-1",
                        "context_tokens": [1, 2, 3],
                        "expected_token": 9,
                        "accepted_tokenss": [9, 10]
                    }
                ]
            }),
        );
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("accepted_tokenss")
        ));
    }

    #[test]
    fn anchor_case_misspelled_rank_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert(
            "correctness_gates".into(),
            json!({
                "anchors": [
                    {
                        "name": "anchor-1",
                        "context_tokens": [1, 2, 3],
                        "expected_token": 9,
                        "max_expected_ranke": 3
                    }
                ]
            }),
        );
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("max_expected_ranke")
        ));
    }

    #[test]
    fn free_run_case_misspelled_key_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert(
            "correctness_gates".into(),
            json!({
                "free_run": [
                    {
                        "name": "fr-1",
                        "prompt_tokens": [1, 2, 3],
                        "expected_tokens": [4, 5],
                        "exact_prefix_tokenss": 1
                    }
                ]
            }),
        );
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("exact_prefix_tokenss")
        ));
    }

    #[test]
    fn behavior_case_misspelled_key_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut().unwrap().insert(
            "correctness_gates".into(),
            json!({
                "behavior": [
                    {
                        "name": "b-1",
                        "prompt_tokens": [1, 2],
                        "accepted_token_sequences": [[3, 4]],
                        "max_new_tokens": 4,
                        "semantic_prompts": "x"
                    }
                ]
            }),
        );
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("unknown field") && m.contains("semantic_prompts")
        ));
    }

    // --- preserved null/empty-section guards (formerly the Value key-set pass) ---

    #[test]
    fn null_correctness_gates_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("correctness_gates".into(), Value::Null);
        assert!(load(&v).is_err());
    }

    #[test]
    fn null_benchmark_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("benchmark".into(), Value::Null);
        assert!(load(&v).is_err());
    }

    #[test]
    fn empty_correctness_gates_object_rejected() {
        let mut v = minimal_doc(3, 4);
        v.as_object_mut()
            .unwrap()
            .insert("correctness_gates".into(), json!({}));
        let err = load(&v).unwrap_err();
        assert!(matches!(
            err,
            BenchError::InvalidInput(m) if m.contains("at least one gate section")
        ));
    }
}
