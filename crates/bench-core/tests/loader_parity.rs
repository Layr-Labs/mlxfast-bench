//! WS1-10 loader-parity (Rust side): run one fixture corpus through the bench-core golden
//! loader and assert every accept/reject decision matches the Swift `Golden.swift` spec
//! byte-for-byte (required_model_type = "qwen4_exp_text", the benchmark loader).
//!
//! Regenerate the corpus with `python3 scripts/gen-loader-parity-corpus.py`. The
//! cross-language companion — the SAME corpus through `benchd validate-golden` (this
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

use bench_core::constants::CORRECTNESS_STEPS;
use bench_core::golden::{
    load_golden_fixture, reference_model_pin_from_contract, ReferenceModelPin,
};

const REQUIRED_MODEL_TYPE: &str = "qwen4_exp_text";

/// The Qwen 3.8 125B-A6B (MLX) track — the identity this corpus was GENERATED under. The rows
/// here are 125B goldens, so they are loaded under the 125B row of
/// `bench_core::constants::MODEL_IDENTITIES_BY_TRACK`, resolved through the one accessor rather
/// than restated. A corpus is never re-generated to follow a constant; it names its own track.
const TRACK_125B: &str = "qwen3.8-125b-a6b-mlx-v1";

fn identity_125b() -> bench_core::constants::TrackModelIdentity {
    bench_core::constants::model_identity(TRACK_125B).expect("the 125B MLX row is declared")
}

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
            identity_125b().seed_tokens,
            &identity_125b(),
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
            identity_125b().seed_tokens,
            &identity_125b(),
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

/// POSITIVE CONTROL (re-baseline, David 2026-08-27): a golden carrying this track's identity
/// (`model_type` = the required type) AND its pinned reference model (the corpus contract's
/// `target` pair) is ACCEPTED under that pin.
#[test]
fn positive_control_pinned_track_golden_is_accepted() {
    let (dir, manifest) = corpus();
    let pin = corpus_reference_pin(&dir, &manifest);
    let bytes = std::fs::read(dir.join("model_provenance_valid.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["model_type"], REQUIRED_MODEL_TYPE);
    assert_eq!(
        doc["model_provenance"]["repository"],
        pin.repository.as_str()
    );
    assert_eq!(doc["model_provenance"]["revision"], pin.revision.as_str());
    let fx = load_golden_fixture(
        &bytes,
        CORRECTNESS_STEPS,
        identity_125b().seed_tokens,
        &identity_125b(),
        Some(REQUIRED_MODEL_TYPE),
        None,
        Some(&pin),
    )
    .expect("the pinned track golden must be ACCEPTED");
    assert_eq!(fx.model_provenance.unwrap().repository, pin.repository);
    // The corpus contract is the MLX platform's pin (the Swift loader leg is the MLX engine),
    // and the constants table says the same thing — one fact, two places, tied here.
    let mlx = bench_core::constants::Platform::Mlx.reference_model();
    assert_eq!(
        (pin.repository.as_str(), pin.revision.as_str()),
        (mlx.repository, mlx.revision)
    );
}

/// The same two controls for the CUDA platform's pin, synthesized from the corpus: a golden naming
/// the CUDA checkpoint is ACCEPTED under the CUDA pin, and the gemma reference model refuses BY
/// NAME under it too. The MLX golden is refused under the CUDA pin by name as well — the two
/// platforms' goldens are not interchangeable.
#[test]
fn cuda_platform_pin_controls() {
    let (dir, manifest) = corpus();
    let mlx_pin = corpus_reference_pin(&dir, &manifest);
    let cuda = bench_core::constants::Platform::Cuda.reference_model();
    let cuda_pin = ReferenceModelPin {
        repository: cuda.repository.to_string(),
        revision: cuda.revision.to_string(),
    };
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("model_provenance_valid.json")).unwrap())
            .unwrap();
    let load = |doc: &serde_json::Value, pin: &ReferenceModelPin| {
        load_golden_fixture(
            &serde_json::to_vec(doc).unwrap(),
            CORRECTNESS_STEPS,
            identity_125b().seed_tokens,
            &identity_125b(),
            Some(REQUIRED_MODEL_TYPE),
            None,
            Some(pin),
        )
    };
    // The corpus golden names the MLX checkpoint: refused under the CUDA pin, by name.
    let err = load(&doc, &cuda_pin).unwrap_err().to_string();
    assert!(
        err.contains(&format!(
            "golden names {}@{}",
            mlx_pin.repository, mlx_pin.revision
        )) && err.contains(&format!(
            "the track pins {}@{}",
            cuda.repository, cuda.revision
        )),
        "{err}"
    );
    // Positive control: the CUDA checkpoint under the CUDA pin.
    doc["model_provenance"] =
        serde_json::json!({"repository": cuda.repository, "revision": cuda.revision});
    let fx = load(&doc, &cuda_pin).expect("the CUDA golden must be ACCEPTED under the CUDA pin");
    assert_eq!(fx.model_provenance.unwrap().repository, cuda.repository);
    // Negative control: the gemma reference model under the CUDA pin.
    let gemma: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("retired_track_provenance.json")).unwrap())
            .unwrap();
    doc["model_provenance"] = gemma["model_provenance"].clone();
    let err = load(&doc, &cuda_pin).unwrap_err().to_string();
    assert!(
        err.contains("golden names mlx-community/gemma-4-26B-A4B-it-qat-4bit@")
            && err.contains(&format!(
                "the track pins {}@{}",
                cuda.repository, cuda.revision
            )),
        "{err}"
    );
}

/// NEGATIVE CONTROL (re-baseline, David 2026-08-27): a gemma golden — the previous track's
/// `model_type` and reference model — REFUSES BY NAME. Both halves are proven: the whole
/// gemma golden refuses on the model_type gate (checked first, as the reference does), and a
/// golden with the right model_type but the gemma reference model refuses on the pin with a
/// message that names both models.
#[test]
fn negative_control_gemma_golden_refuses_by_name() {
    let (dir, manifest) = corpus();
    let pin = corpus_reference_pin(&dir, &manifest);
    let load = |file: &str| {
        let bytes = std::fs::read(dir.join(file)).unwrap();
        load_golden_fixture(
            &bytes,
            CORRECTNESS_STEPS,
            identity_125b().seed_tokens,
            &identity_125b(),
            Some(REQUIRED_MODEL_TYPE),
            None,
            Some(&pin),
        )
        .expect_err(&format!("{file} must be REFUSED"))
        .to_string()
    };

    let whole = load("retired_track_golden.json");
    assert!(
        whole.contains("model_type=Some(\"gemma4_text\") expected qwen4_exp_text"),
        "the gemma golden must be refused by NAME on the model_type gate: {whole}"
    );

    let provenance = load("retired_track_provenance.json");
    assert!(
        provenance.contains("golden names mlx-community/gemma-4-26B-A4B-it-qat-4bit@")
            && provenance.contains(&format!(
                "the track pins {}@{}",
                pin.repository, pin.revision
            )),
        "the gemma reference model must be refused by NAME on the pin: {provenance}"
    );
}
