//! WS1-10 loader-parity (Rust side): run one fixture corpus through the bench-core golden
//! loader and assert every accept/reject decision matches the Swift `Golden.swift` spec
//! byte-for-byte (required_model_type = "gemma4_text", the benchmark loader).
//!
//! Regenerate the corpus with `python3 scripts/gen-loader-parity-corpus.py`. The
//! cross-language companion — the SAME corpus through `benchctl validate-golden` (this
//! loader) AND `mlxfast-swift preflight` (the live Swift loader), asserting identical
//! decisions — is `scripts/loader-parity.sh`, run on a box with mlxfast-swift built.
//! Fixtures whose manifest entry sets `swift_diverges` are KNOWN intentional divergences
//! (e.g. Rust's per-case deny_unknown_fields is stricter than Swift's JSONDecoder, which
//! silently drops unknown per-case keys — an anti-cheat strengthening, surfaced not hidden).
//!
//! #114 — the corpus is run TWICE. The PINNED pass supplies the corpus's own track-contract
//! fixture (manifest `reference_model_contract`), which is the configuration Swift is compared
//! against: the reference always applies its reference-model pin, so a benchd run that holds a
//! contract must decide identically. The UNPINNED pass runs with no contract and asserts
//! `expected_rust_unpinned` where the manifest declares one — that field exists on exactly the
//! rows where the contract pin changes the decision, which is what keeps the residual looseness of
//! benchd's contract-less commands a stated fact rather than an untested assumption.

use std::path::PathBuf;

use bench_core::constants::{CORRECTNESS_PROMPT_TOKENS, CORRECTNESS_STEPS};
use bench_core::golden::{
    load_golden_fixture, reference_model_pin_from_contract, ReferenceModelPin,
};

const REQUIRED_MODEL_TYPE: &str = "gemma4_text";

/// The corpus dir + its parsed manifest.
fn corpus() -> (PathBuf, serde_json::Value) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden_parity");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest.json"))
            .expect("manifest parses");
    (dir, manifest)
}

/// The corpus's declared track-contract pin. The manifest names the fixture; the pin itself is
/// read out of THAT file by the production path (`reference_model_pin_from_contract`), never
/// re-spelled here — a test that hard-coded the identity would pass even if the contract-reading
/// code stopped finding it.
fn corpus_reference_pin(dir: &std::path::Path, manifest: &serde_json::Value) -> ReferenceModelPin {
    let file = manifest["reference_model_contract"]
        .as_str()
        .expect("manifest declares reference_model_contract");
    let bytes = std::fs::read(dir.join(file)).expect("contract fixture readable");
    reference_model_pin_from_contract(&bytes)
        .expect("contract fixture parses")
        .expect("contract fixture declares a reference-model pin")
}

#[test]
fn loader_parity_corpus_decisions_match_swift_spec() {
    let (dir, manifest) = corpus();
    let pin = corpus_reference_pin(&dir, &manifest);
    let fixtures = manifest["fixtures"].as_array().expect("fixtures array");
    assert!(fixtures.len() >= 10, "corpus should be non-trivial");

    // Every corpus fixture must load exactly as its manifest decision says, WITH the track
    // contract's reference-model pin supplied — the configuration Swift is compared against.
    let mut accepted = 0;
    let mut rejected = 0;
    for fx in fixtures {
        let file = fx["file"].as_str().unwrap();
        let expected_accept = match fx["expected_rust"].as_str().unwrap() {
            "ACCEPT" => true,
            "REJECT" => false,
            other => panic!("bad expected_rust {other:?} in manifest for {file}"),
        };
        let bytes = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        let got = load_golden_fixture(
            &bytes,
            CORRECTNESS_STEPS,
            CORRECTNESS_PROMPT_TOKENS,
            Some(REQUIRED_MODEL_TYPE),
            None,
            Some(&pin),
        );
        assert_eq!(
            got.is_ok(),
            expected_accept,
            "fixture {file}: manifest says {} but the loader {} — {}",
            if expected_accept { "ACCEPT" } else { "REJECT" },
            if got.is_ok() { "accepted" } else { "rejected" },
            fx["note"].as_str().unwrap_or("")
        );
        // #114 (F1) — a row may pin the DIAGNOSTIC as well as the decision. Rows carrying more
        // than one defect are decision-identical across the two loaders no matter which gate
        // fires, so the accept/reject harness cannot see an ordering divergence; the manifest
        // states which gate must win and the assertion holds the loader to it.
        if let Some(needle) = fx["expected_rust_message_contains"].as_str() {
            let err = got
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("fixture {file} pins a message but was ACCEPTED"))
                .to_string();
            assert!(
                err.contains(needle),
                "fixture {file}: the reject diagnostic must contain {needle:?} \
                 (the gate the reference fires first) — got {err:?}"
            );
        }
        if expected_accept {
            accepted += 1;
        } else {
            rejected += 1;
        }
    }
    // Sanity: the corpus exercises BOTH decisions (not vacuously all-accept/all-reject).
    assert!(accepted >= 1, "corpus must contain an accepted fixture");
    assert!(
        rejected >= 5,
        "corpus must contain several rejected fixtures"
    );
}

/// #114 — the contract-LESS decisions. Without a contract the loader validates `model_provenance`
/// for SHAPE only, so the rows carrying `expected_rust_unpinned` decide differently; every other
/// row must decide the same either way (the pin must not be silently changing unrelated
/// decisions). At least one row must actually differ, or the pinned pass above is proving nothing.
#[test]
fn loader_parity_corpus_unpinned_decisions_are_shape_only() {
    let (dir, manifest) = corpus();
    let fixtures = manifest["fixtures"].as_array().expect("fixtures array");

    let mut pin_sensitive = 0;
    for fx in fixtures {
        let file = fx["file"].as_str().unwrap();
        let declared = fx["expected_rust_unpinned"].as_str();
        if declared.is_some() {
            pin_sensitive += 1;
        }
        let expected = declared.unwrap_or_else(|| fx["expected_rust"].as_str().unwrap());
        let expected_accept = match expected {
            "ACCEPT" => true,
            "REJECT" => false,
            other => panic!("bad expected decision {other:?} in manifest for {file}"),
        };
        let bytes = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        let got = load_golden_fixture(
            &bytes,
            CORRECTNESS_STEPS,
            CORRECTNESS_PROMPT_TOKENS,
            Some(REQUIRED_MODEL_TYPE),
            None,
            None,
        );
        assert_eq!(
            got.is_ok(),
            expected_accept,
            "fixture {file} (no contract): manifest says {expected} but the loader {} — {}",
            if got.is_ok() { "accepted" } else { "rejected" },
            fx["note"].as_str().unwrap_or("")
        );
    }
    assert!(
        pin_sensitive >= 1,
        "the corpus must carry at least one row whose decision DEPENDS on the contract pin, \
         else the pinned pass is not exercising #114 at all"
    );
}
