//! Timed-prompt TAPE schema + strict loader — the document the LIVE `timed_prompt_pool`
//! actually pins.
//!
//! ## Why this type exists
//!
//! `measure-job --golden` used to model ONE shape: [`crate::golden::GoldenDocument`]
//! (`{version, model_type, cases, correctness_gates, benchmark}`, `deny_unknown_fields`).
//! But the objects the ranked track's `timed_prompt_pool[].sha256` entries pin are NOT
//! GoldenDocuments — they are teacher-forcing TAPES emitted by the engine's
//! `mtp-verify --generate` pass, whose top level is
//! `{emitted_tokens, reference_seed_token, reference_self_consistent, rows, seed_tokens}`.
//! Both conditions were mandatory (`GoldenDocument` parse AND pool-pin match), and no file
//! could satisfy both, so every `measure-job` invocation died pre-GPU (die-8). This module
//! is the golden input the pool pins, so the contract becomes satisfiable without loosening
//! the GoldenDocument loader by a single key.
//!
//! ## Schema provenance (derived, not guessed)
//!
//! The struct below is derived from the REFERENCE Swift decoder, and cross-checked against
//! the live pinned objects:
//!
//! * REFERENCE DECODER — `Sources/MLXFastTrustedHarness/QwenRuntimeMTP.swift`,
//!   `struct QwenMTPReferenceGolden` + its `CodingKeys`: `seed_tokens` `[Int]`,
//!   `reference_seed_token` `Int`, `rows` `[Row]` REQUIRED; `reference_self_consistent`
//!   `Bool?` and `emitted_tokens` `[Int]?` OPTIONAL. `Row` = `sequential_argmax` `Int`
//!   REQUIRED; `top2_tokens` `[Int]?`, `top2_logits` `[Double]?`, `top1_logit` `Double?`
//!   OPTIONAL.
//! * LIVE OBJECTS — all 8 pool objects pinned by the mirrored track fixture
//!   `live-qwen3_6_27b_mtp_track.json` (`timed_prompt_pool[]`, 8 entries carrying
//!   `r2_path`/`sha256`/`bytes`/`noop_decode_speedup`), read read-only on the box: every one
//!   carries EXACTLY the five top-level keys and EXACTLY the four row keys above — no
//!   optional key is ever absent in practice, and no extra key ever appears
//!   (`rows` key-tuple histogram: one bucket, 513/513 rows, on each of the 8 objects).
//!
//! ## Semantics (what the fields MEAN to the legs)
//!
//! From the reference driver (`QwenRuntimeMTPDriver.swift`):
//!
//! * `seed_tokens` is the PROMPT: the driver opens the timed window with
//!   `beginMTPDecode(seedTokens:)` (W:`--golden` → `mtp-timed`).
//! * `reference_seed_token` is the seed forward's argmax = the run's FIRST emitted token.
//!   The driver hard-fails `seed_token_mismatch` when the engine's seed forward disagrees.
//! * `rows[i].sequential_argmax` is the token emitted at index `i + 1` — the driver's own
//!   exactness rule is `expected[i] = i == 0 ? reference_seed_token : rows[i - 1].sequential_argmax`.
//! * `emitted_tokens` is the reference chain AFTER the seed token (so
//!   `emitted_tokens[i] == rows[i].sequential_argmax`); the generator sets
//!   `reference_self_consistent = false` when its own replay contradicts that.
//!   The driver REFUSES a tape whose `reference_self_consistent` is explicitly `false`
//!   ("an operator fault, not a submission fault").
//!
//! Those are exactly benchd's [`bench_runner::TimingParams`] decode fields (seed tokens →
//! `decode_seed_tokens`, seed argmax → `expected_decode_seed_token`, row argmax chain →
//! `expected_decode_tokens`), which is why a tape drives the existing timed legs unchanged.
//!
//! ## Strictness vs the Swift decoder (deliberate, and inert on pinned bytes)
//!
//! Swift's `JSONDecoder` IGNORES unknown keys; this loader carries `deny_unknown_fields` at
//! both levels, matching the house rule that a golden-side document is parsed strictly (the
//! GoldenDocument loader is `deny_unknown_fields` for the same anti-cheat reason). This is
//! the one place benchd is TIGHTER than the reference decoder, and it cannot false-reject a
//! real input: every `--golden` in a ranked run must ALSO match a `timed_prompt_pool` pin BY
//! BYTES, and all 8 currently-pinned objects were verified to carry exactly the modelled key
//! set. Should the organizer ever pin a tape with a new key, this loader fails LOUD (naming
//! the key) instead of silently dropping it — the schema change then lands deliberately, in
//! both loaders, per the standing "Swift is the reference" rule.

use crate::constants::VOCAB_SIZE;
use crate::golden::{verify_golden_integrity, GoldenIntegrityPin, Token};
use crate::hash::sha256_hex;
use crate::{BenchError, Result};
use serde::{Deserialize, Serialize};

/// One reference row: the serial trajectory's decision at one step.
///
/// `sequential_argmax` is the only REQUIRED key (Swift `Row.sequentialArgmax`); the top-2
/// diagnostics are optional in the reference decoder and present in every live object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimedPromptTapeRow {
    /// The token the SERIAL reference emitted for this row (Swift `sequential_argmax`).
    pub sequential_argmax: Token,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub top2_tokens: Option<Vec<Token>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub top2_logits: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub top1_logit: Option<f64>,
}

/// Wire shape of a timed-prompt tape. Port of Swift `QwenMTPReferenceGolden`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimedPromptTapeDocument {
    /// The PROMPT fed to the timed seed prefill.
    pub seed_tokens: Vec<Token>,
    /// The seed forward's argmax — the run's first emitted token.
    pub reference_seed_token: Token,
    /// `rows[i]` describes the token emitted at index `i + 1`.
    pub rows: Vec<TimedPromptTapeRow>,
    /// The generator's self-consistency verdict. An explicit `false` is an OPERATOR fault
    /// and is refused (Swift `referenceNotSelfConsistent`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reference_self_consistent: Option<bool>,
    /// The reference chain AFTER the seed token (informational; equals the row argmax chain).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub emitted_tokens: Option<Vec<Token>>,
}

/// A validated tape plus the sha256 of the exact bytes it was loaded from (the identity the
/// `timed_prompt_pool` pin is matched against — BIND BY BYTES).
#[derive(Debug, Clone, PartialEq)]
pub struct TimedPromptTape {
    pub seed_tokens: Vec<Token>,
    pub reference_seed_token: Token,
    pub rows: Vec<TimedPromptTapeRow>,
    pub reference_self_consistent: Option<bool>,
    pub emitted_tokens: Option<Vec<Token>>,
    pub sha256: String,
    /// #112 (L3) — the COUNT of those same bytes. A canonical golden is identified by
    /// `sha256` + `bytes` TOGETHER (the pin rule the `timed_prompt_pool` entries and
    /// [`GoldenIntegrityPin`](crate::golden::GoldenIntegrityPin) both carry), so the loader
    /// records both halves of the identity rather than making a consumer re-stat the file.
    pub byte_len: u64,
}

impl TimedPromptTape {
    /// The reference chain AFTER the seed token, in emitted order: `rows[i].sequential_argmax`
    /// is the token at emitted index `i + 1`. This is precisely benchd's
    /// `TimingParams::expected_decode_tokens` (`decode_step` `i` must return element `i`), with
    /// `reference_seed_token` as `expected_decode_seed_token`.
    pub fn row_argmax_chain(&self) -> Vec<Token> {
        self.rows.iter().map(|r| r.sequential_argmax).collect()
    }

    /// Number of reference rows — the ceiling on a timed decode window this tape can oracle.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

fn invalid(msg: impl Into<String>) -> BenchError {
    BenchError::InvalidInput(msg.into())
}

fn validate_tokens(tokens: &[Token], field: &str) -> Result<()> {
    for (index, token) in tokens.iter().enumerate() {
        if *token < 0 || *token >= VOCAB_SIZE as i64 {
            return Err(invalid(format!(
                "timed-prompt tape {field}[{index}]={token} is outside configured vocab range \
                 0..<{VOCAB_SIZE}"
            )));
        }
    }
    Ok(())
}

/// The tape's REQUIRED-KEY SIGNATURE, used to route a `--golden` to this loader instead of
/// the legacy GoldenDocument one.
///
/// The two shapes are DISJOINT and the detection is therefore unambiguous, in both
/// directions:
/// * a tape must carry `seed_tokens` + `reference_seed_token` + `rows`, none of which is a
///   GoldenDocument key — and GoldenDocument is `deny_unknown_fields`, so a tape can never
///   parse as a GoldenDocument;
/// * a GoldenDocument must carry `cases`, which is not a tape key — and this document is
///   `deny_unknown_fields`, so a GoldenDocument can never parse as a tape.
///
/// Detection is by KEY PRESENCE only (never by "whatever parses"): a tape with a broken
/// `rows` element must fail as a BROKEN TAPE, naming the real defect, rather than silently
/// falling through to the other loader and being reported as a bad GoldenDocument.
pub const TAPE_REQUIRED_KEYS: [&str; 3] = ["seed_tokens", "reference_seed_token", "rows"];

/// GoldenDocument's own required-key signature (`cases`), for the same routing decision.
pub const GOLDEN_DOCUMENT_REQUIRED_KEYS: [&str; 1] = ["cases"];

/// Which document shape a `--golden` file's bytes DECLARE themselves to be, by required-key
/// signature. See [`TAPE_REQUIRED_KEYS`] for why this is unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoldenInputKind {
    /// Carries every tape required key → route to [`load_timed_prompt_tape`].
    TimedPromptTape,
    /// Carries the GoldenDocument required key → route to the legacy golden loader.
    GoldenDocument,
    /// Neither signature — the caller reports BOTH expected shapes rather than guessing.
    Unrecognized,
}

/// Classify a `--golden` file's raw bytes by required-key signature (see [`GoldenInputKind`]).
/// Non-JSON / non-object bytes classify as [`GoldenInputKind::Unrecognized`]; the chosen
/// loader then produces the real parse diagnostic.
pub fn classify_golden_input(bytes: &[u8]) -> GoldenInputKind {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(bytes)
    else {
        return GoldenInputKind::Unrecognized;
    };
    if TAPE_REQUIRED_KEYS.iter().all(|k| map.contains_key(*k)) {
        return GoldenInputKind::TimedPromptTape;
    }
    if GOLDEN_DOCUMENT_REQUIRED_KEYS
        .iter()
        .all(|k| map.contains_key(*k))
    {
        return GoldenInputKind::GoldenDocument;
    }
    GoldenInputKind::Unrecognized
}

/// Load + validate a timed-prompt tape from raw bytes, returning it bound to the sha256 of
/// those exact bytes.
///
/// When `pin` is `Some`, the sha256 + byte-count integrity gate runs on the RAW BYTES BEFORE
/// the parse (same ordering as [`crate::golden::load_golden_fixture`]), so an
/// unknown-provenance tape is refused before its contents are trusted. `measure-job` passes
/// `None` here and pins through the contract's `timed_prompt_pool` instead (R4: exactly-one
/// match, fail-closed die-8) — the pin is on the same raw bytes either way.
///
/// Semantic checks (everything serde cannot express), each mirroring the reference driver:
/// * `seed_tokens` non-empty — the driver opens the window with this prompt;
/// * `rows` non-empty — a tape with no rows can oracle no decode window;
/// * `reference_self_consistent == Some(false)` REFUSED (Swift `referenceNotSelfConsistent`:
///   "an operator fault, not a submission fault");
/// * `emitted_tokens`, when present, must AGREE with the row argmax chain — that identity is
///   what the generator's own self-consistency replay asserts, so a tape that claims
///   self-consistency while contradicting itself is refused rather than measured;
/// * every token id inside `0..<VOCAB_SIZE` (same range check the golden loader applies;
///   verified to hold across all 8 live pinned objects, whose ids span 0..=248046).
pub fn load_timed_prompt_tape(
    bytes: &[u8],
    pin: Option<&GoldenIntegrityPin>,
) -> Result<TimedPromptTape> {
    if let Some(pin) = pin {
        verify_golden_integrity(bytes, pin)?;
    }

    let decoded: TimedPromptTapeDocument = serde_json::from_slice(bytes)
        .map_err(|e| invalid(format!("timed-prompt tape could not be decoded: {e}")))?;

    if decoded.seed_tokens.is_empty() {
        return Err(invalid(
            "timed-prompt tape seed_tokens must not be empty (it is the prompt the timed seed \
             prefill is opened with)",
        ));
    }
    if decoded.rows.is_empty() {
        return Err(invalid(
            "timed-prompt tape rows must not be empty (rows[i] is the reference token emitted at \
             index i+1; a tape with no rows can oracle no decode window)",
        ));
    }
    if decoded.reference_self_consistent == Some(false) {
        return Err(invalid(
            "timed-prompt tape reports reference_self_consistent=false: the reference rows failed \
             their own self-consistency replay — an OPERATOR fault (mismatched reference build or \
             weights), never a submission fault, so this prompt must not be measured",
        ));
    }

    validate_tokens(&decoded.seed_tokens, "seed_tokens")?;
    validate_tokens(
        std::slice::from_ref(&decoded.reference_seed_token),
        "reference_seed_token",
    )?;
    let row_chain: Vec<Token> = decoded.rows.iter().map(|r| r.sequential_argmax).collect();
    validate_tokens(&row_chain, "rows[].sequential_argmax")?;

    if let Some(emitted) = &decoded.emitted_tokens {
        validate_tokens(emitted, "emitted_tokens")?;
        // The generator writes `emitted_tokens` AS the row chain and flags any disagreement by
        // setting `reference_self_consistent=false`; a tape that disagrees here while claiming
        // consistency contradicts itself, and the two answers to "what did the reference emit?"
        // must never be silently reconciled at measure time.
        if let Some((index, (e, r))) = emitted
            .iter()
            .zip(row_chain.iter())
            .enumerate()
            .find(|(_, (e, r))| e != r)
        {
            return Err(invalid(format!(
                "timed-prompt tape contradicts itself at index {index}: emitted_tokens[{index}]={e} \
                 but rows[{index}].sequential_argmax={r} (the reference chain and its own rows must \
                 agree; the generator records that disagreement as reference_self_consistent=false)"
            )));
        }
        if emitted.len() != row_chain.len() {
            return Err(invalid(format!(
                "timed-prompt tape carries {} emitted_tokens but {} rows: the reference chain after \
                 the seed token IS the row argmax chain, so the two must have equal length",
                emitted.len(),
                row_chain.len()
            )));
        }
    }

    let hash = sha256_hex(bytes);

    Ok(TimedPromptTape {
        seed_tokens: decoded.seed_tokens,
        reference_seed_token: decoded.reference_seed_token,
        rows: decoded.rows,
        reference_self_consistent: decoded.reference_self_consistent,
        emitted_tokens: decoded.emitted_tokens,
        sha256: hash,
        byte_len: bytes.len() as u64,
    })
}

/// Convenience: read the file at `path`, then [`load_timed_prompt_tape`] its bytes.
pub fn load_timed_prompt_tape_from_path(
    path: &std::path::Path,
    pin: Option<&GoldenIntegrityPin>,
) -> Result<TimedPromptTape> {
    let bytes = std::fs::read(path).map_err(|e| {
        invalid(format!(
            "timed-prompt tape read failed ({}): {e}",
            path.display()
        ))
    })?;
    load_timed_prompt_tape(&bytes, pin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A SYNTHESIZED tape — schema-true (the key set + types derived from the reference Swift
    /// decoder and cross-checked against the live pinned objects), CONTENT INVENTED. No
    /// organizer bytes are copied into this repository.
    fn synth_tape(seed_len: usize, row_count: usize) -> serde_json::Value {
        let seed: Vec<i64> = (0..seed_len as i64).map(|i| 1_000 + i).collect();
        let chain: Vec<i64> = (0..row_count as i64).map(|i| 7_000 + i).collect();
        let rows: Vec<serde_json::Value> = chain
            .iter()
            .map(|t| {
                json!({
                    "sequential_argmax": t,
                    "top1_logit": 19.5,
                    "top2_logits": [19.5, 18.375],
                    "top2_tokens": [t, 321],
                })
            })
            .collect();
        json!({
            "emitted_tokens": chain,
            "reference_seed_token": 4_625,
            "reference_self_consistent": true,
            "rows": rows,
            "seed_tokens": seed,
        })
    }

    fn bytes_of(v: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(v).unwrap()
    }

    #[test]
    fn schema_true_tape_loads_and_exposes_the_leg_oracle() {
        let doc = synth_tape(512, 513);
        let bytes = bytes_of(&doc);
        let tape = load_timed_prompt_tape(&bytes, None).expect("schema-true tape must load");
        assert_eq!(tape.seed_tokens.len(), 512);
        assert_eq!(tape.reference_seed_token, 4_625);
        assert_eq!(tape.row_count(), 513);
        // The oracle the legs consume: seed argmax then the row chain.
        assert_eq!(tape.row_argmax_chain()[0], 7_000);
        assert_eq!(tape.row_argmax_chain().len(), 513);
        // Identity is the sha of the exact bytes (what the pool pin is matched against).
        assert_eq!(tape.sha256, sha256_hex(&bytes));
    }

    #[test]
    fn optional_keys_may_be_absent() {
        // The reference decoder makes `reference_self_consistent` / `emitted_tokens` optional,
        // and the row top-2 diagnostics optional too. Absent must load.
        let doc = json!({
            "seed_tokens": [1, 2, 3],
            "reference_seed_token": 9,
            "rows": [{ "sequential_argmax": 11 }, { "sequential_argmax": 12 }],
        });
        let tape = load_timed_prompt_tape(&bytes_of(&doc), None).expect("optionals may be absent");
        assert_eq!(tape.reference_self_consistent, None);
        assert_eq!(tape.emitted_tokens, None);
        assert_eq!(tape.row_argmax_chain(), vec![11, 12]);
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        let mut doc = synth_tape(4, 4);
        doc["surprise_field"] = json!(1);
        let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
        assert!(
            format!("{err}").contains("surprise_field"),
            "unknown key must be named: {err}"
        );
    }

    #[test]
    fn unknown_row_key_is_rejected() {
        let mut doc = synth_tape(4, 4);
        doc["rows"][0]["sequental_argmax"] = json!(3);
        let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
        assert!(
            format!("{err}").contains("sequental_argmax"),
            "a typo'd per-row key must be named, not dropped: {err}"
        );
    }

    #[test]
    fn missing_required_key_is_rejected() {
        for key in TAPE_REQUIRED_KEYS {
            let mut doc = synth_tape(4, 4);
            doc.as_object_mut().unwrap().remove(key);
            let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
            assert!(
                format!("{err}").contains(key),
                "missing required key {key} must be named: {err}"
            );
        }
    }

    #[test]
    fn self_inconsistent_reference_is_refused_as_an_operator_fault() {
        let mut doc = synth_tape(4, 4);
        doc["reference_self_consistent"] = json!(false);
        let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("reference_self_consistent=false"), "{msg}");
        assert!(msg.contains("OPERATOR fault"), "{msg}");
    }

    #[test]
    fn emitted_chain_contradicting_its_own_rows_is_refused() {
        let mut doc = synth_tape(4, 4);
        doc["emitted_tokens"][2] = json!(6_000);
        let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("contradicts itself at index 2"), "{msg}");
    }

    #[test]
    fn empty_seed_or_rows_is_refused() {
        let mut doc = synth_tape(4, 4);
        doc["seed_tokens"] = json!([]);
        let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
        assert!(format!("{err}").contains("seed_tokens must not be empty"));

        let mut doc = synth_tape(4, 4);
        doc["rows"] = json!([]);
        doc["emitted_tokens"] = json!([]);
        let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
        assert!(format!("{err}").contains("rows must not be empty"));
    }

    #[test]
    fn out_of_vocab_token_is_refused() {
        let mut doc = synth_tape(4, 4);
        doc["rows"][1]["sequential_argmax"] = json!(VOCAB_SIZE as i64);
        doc["emitted_tokens"][1] = json!(VOCAB_SIZE as i64);
        let err = load_timed_prompt_tape(&bytes_of(&doc), None).unwrap_err();
        assert!(
            format!("{err}").contains("outside configured vocab range"),
            "{err}"
        );
    }

    #[test]
    fn pin_is_checked_on_raw_bytes_before_the_parse() {
        let bytes = bytes_of(&synth_tape(4, 4));
        let good = load_timed_prompt_tape(&bytes, None).unwrap();
        let pin = GoldenIntegrityPin {
            sha256: good.sha256.clone(),
            bytes: bytes.len() as u64,
        };
        assert!(load_timed_prompt_tape(&bytes, Some(&pin)).is_ok());

        // Wrong byte count → refused on the COUNT, before any parse.
        let wrong_bytes = GoldenIntegrityPin {
            sha256: good.sha256.clone(),
            bytes: bytes.len() as u64 + 1,
        };
        assert!(format!(
            "{}",
            load_timed_prompt_tape(&bytes, Some(&wrong_bytes)).unwrap_err()
        )
        .contains("byte count mismatch"));

        // Wrong sha → refused on the digest. Proven on bytes that are NOT parseable at all,
        // so a pass could only mean the pin ran after the parse.
        let wrong_sha = GoldenIntegrityPin {
            sha256: "0".repeat(64),
            bytes: 3,
        };
        assert!(format!(
            "{}",
            load_timed_prompt_tape(b"{ [", Some(&wrong_sha)).unwrap_err()
        )
        .contains("sha256 mismatch"));
    }

    #[test]
    fn required_key_signatures_route_each_shape_and_are_disjoint() {
        let tape = bytes_of(&synth_tape(4, 4));
        assert_eq!(
            classify_golden_input(&tape),
            GoldenInputKind::TimedPromptTape
        );

        let golden = bytes_of(&json!({
            "version": 1,
            "model_type": "gemma4_text",
            "cases": [{ "name": "c", "prompt_tokens": [1], "expected_tokens": [2] }],
        }));
        assert_eq!(
            classify_golden_input(&golden),
            GoldenInputKind::GoldenDocument
        );

        // Neither signature → the caller reports BOTH shapes instead of guessing.
        assert_eq!(
            classify_golden_input(b"{\"version\": 1}"),
            GoldenInputKind::Unrecognized
        );
        assert_eq!(
            classify_golden_input(b"not json"),
            GoldenInputKind::Unrecognized
        );
        assert_eq!(classify_golden_input(b"[]"), GoldenInputKind::Unrecognized);

        // DISJOINTNESS, proven both ways: neither document can parse as the other shape.
        assert!(load_timed_prompt_tape(&golden, None).is_err());
        assert!(crate::golden::load_golden_fixture(&tape, 1, 1, None, None, None).is_err());
    }
}
