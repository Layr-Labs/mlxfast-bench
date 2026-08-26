//! M-4 structure-aware fuzz corpus — Rust-side gate.
//!
//! Two guarantees, both fail loud:
//!   1. FREEZE + PIN: every committed fuzz fixture's sha256 + byte count matches its
//!      manifest entry. The corpus is frozen; regenerating with the same seed
//!      (`python3 scripts/gen-fuzz-corpus.py`, SEED pinned) yields byte-identical
//!      fixtures. A drifted / added / removed fixture fails this test.
//!   2. STABLE LOADER VERDICTS: the bench-core golden loader's accept/reject on every
//!      pinned fixture matches the manifest's `expected_rust` — the SAME code path
//!      `benchctl validate-golden` runs. This is the local (Rust) half of the
//!      dual-loader parity; the Swift half runs on the box via
//!      `scripts/loader-parity.sh` / `scripts/fuzz-corpus-check.sh`, where fixtures
//!      whose manifest sets `swift_diverges` are recorded as declared divergences
//!      (Swift's JSONDecoder drops unknown keys inside inner objects; Rust's serde
//!      `deny_unknown_fields` rejects them — an intentional, anti-cheat strictness).
//!
//! Corpus + manifest: `crates/bench-core/tests/fixtures/golden_fuzz/`.

use std::path::PathBuf;

use bench_core::constants::{CORRECTNESS_PROMPT_TOKENS, CORRECTNESS_STEPS};
use bench_core::golden::load_golden_fixture;
use bench_core::hash::sha256_hex;

const REQUIRED_MODEL_TYPE: &str = "gemma4_text";

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden_fuzz")
}

fn manifest() -> serde_json::Value {
    let dir = corpus_dir();
    serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest.json"))
        .expect("manifest parses")
}

#[test]
fn fuzz_corpus_is_frozen_and_pinned() {
    let dir = corpus_dir();
    let m = manifest();
    let fixtures = m["fixtures"].as_array().expect("fixtures array");
    assert!(
        fixtures.len() >= 100,
        "M-4 requires N>=100; manifest has {}",
        fixtures.len()
    );

    // Manifest count field agrees with the array length.
    assert_eq!(
        m["count"].as_u64().unwrap() as usize,
        fixtures.len(),
        "manifest count field disagrees with fixtures array length"
    );

    // Every fixture's committed bytes match the pinned sha256 + byte count.
    let mut names = std::collections::HashSet::new();
    for fx in fixtures {
        let file = fx["file"].as_str().unwrap();
        assert!(
            names.insert(file),
            "duplicate fixture filename {file} in manifest"
        );
        let bytes = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        assert_eq!(
            bytes.len() as u64,
            fx["bytes"].as_u64().unwrap(),
            "fixture {file}: byte count drifted from pin (corpus not frozen — regenerate + commit)"
        );
        assert_eq!(
            sha256_hex(&bytes),
            fx["sha256"].as_str().unwrap(),
            "fixture {file}: sha256 drifted from pin (corpus not frozen — regenerate + commit)"
        );
    }

    // No orphan fixture on disk that the manifest does not pin.
    for entry in std::fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if name.ends_with(".json") && name != "manifest.json" {
            assert!(
                names.contains(name.as_str()),
                "orphan fixture {name} on disk is not pinned in the manifest"
            );
        }
    }
}

#[test]
fn fuzz_corpus_loader_verdicts_stable() {
    let dir = corpus_dir();
    let m = manifest();
    let fixtures = m["fixtures"].as_array().expect("fixtures array");

    let mut accepted = 0;
    let mut rejected = 0;
    let mut declared_div = 0;
    for fx in fixtures {
        let file = fx["file"].as_str().unwrap();
        let expect_accept = match fx["expected_rust"].as_str().unwrap() {
            "ACCEPT" => true,
            "REJECT" => false,
            other => panic!("bad expected_rust {other:?} for {file}"),
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
        // #77: `benchctl validate-golden` (default, no --gates-only) requires the benchmark
        // oracle on top of the structural load — byte-consistent with Swift preflight, which
        // rejects a benchmark-less golden. Model that here so the manifest `expected_rust`
        // tracks the SAME accept/reject decision the parity harness runs. A benchmark-less but
        // structurally-valid golden (e.g. valid_cases_only.json) is therefore REJECT.
        let got_accept = got
            .as_ref()
            .map(|fx| fx.benchmark.is_some())
            .unwrap_or(false);
        assert_eq!(
            got_accept,
            expect_accept,
            "fixture {file}: manifest says {} but the loader {} — mutation: {} / {}",
            if expect_accept { "ACCEPT" } else { "REJECT" },
            if got_accept { "accepted" } else { "rejected" },
            fx["mutation"].as_str().unwrap_or(""),
            fx["note"].as_str().unwrap_or("")
        );
        if expect_accept {
            accepted += 1;
        } else {
            rejected += 1;
        }
        if fx["swift_diverges"].as_bool().unwrap_or(false) {
            declared_div += 1;
        }
    }

    // The corpus must exercise BOTH decisions (not vacuously all-reject) and carry the
    // declared Swift-divergence family (unknown keys in inner objects).
    assert!(
        accepted >= 1,
        "corpus must contain at least one accepted fixture"
    );
    assert!(rejected >= 50, "corpus must contain many rejected fixtures");
    assert!(
        declared_div >= 1,
        "corpus must carry at least one declared Swift-divergence fixture"
    );
    assert_eq!(
        m["accept"].as_u64().unwrap() as usize,
        accepted,
        "manifest accept count disagrees with loader"
    );
    assert_eq!(
        m["reject"].as_u64().unwrap() as usize,
        rejected,
        "manifest reject count disagrees with loader"
    );
    // #113: the summary field must agree with the rows it summarises. This one silently went
    // stale (it still read 21 after six `gates_unknown_section_*` / `benchmark_unknown_key_*`
    // rows were flipped to box-confirmed MATCHES) because nothing asserted it — the count is
    // how many divergences we CLAIM are declared, so an inflated one hides an undeclared
    // mismatch. Regenerating via `scripts/gen-fuzz-corpus.py` recomputes it; this pins it.
    assert_eq!(
        m["predicted_swift_divergences"].as_u64().unwrap() as usize,
        declared_div,
        "manifest predicted_swift_divergences disagrees with the rows carrying \
         swift_diverges=true (regenerate with scripts/gen-fuzz-corpus.py)"
    );
}

/// #114 (F5) — the fuzz corpus is run UNPINNED, here and by `scripts/fuzz-corpus-check.sh`, while
/// the Swift leg always applies its reference-model pin. That is only safe as long as no fuzz
/// fixture carries a WELL-FORMED `model_provenance`: a malformed one rejects on shape in both
/// loaders regardless of any pin, but a well-formed one naming a non-reference model would ACCEPT
/// here and REJECT in Swift — an undeclared divergence that neither this test nor the box harness
/// would attribute correctly.
///
/// Rather than leaving that as prose in the parity matrix, it is an ENFORCED invariant: adding such
/// a fixture fails this test, and the message says what to do about it. (Today exactly one fuzz
/// fixture carries the key at all — `unknown_top_key_01.json`, with `{model_id, revision}`, which
/// is a shape reject.)
#[test]
fn no_fuzz_fixture_carries_a_well_formed_model_provenance_while_the_corpus_runs_unpinned() {
    let dir = corpus_dir();
    let m = manifest();
    let mut offenders = Vec::new();
    for fx in m["fixtures"].as_array().expect("fixtures array") {
        let file = fx["file"].as_str().unwrap();
        let bytes = match std::fs::read(dir.join(file)) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue; // deliberately unparseable fuzz input
        };
        let Some(provenance) = doc.get("model_provenance") else {
            continue;
        };
        // "Well-formed" by the reference's own rule: exactly repository+revision, non-empty
        // repository, 40-hex lowercase revision. Anything else rejects on shape, pin or no pin.
        let well_formed = provenance
            .as_object()
            .map(|o| {
                o.len() == 2
                    && o.get("repository")
                        .and_then(|v| v.as_str())
                        .is_some_and(|r| !r.is_empty())
                    && o.get("revision").and_then(|v| v.as_str()).is_some_and(|r| {
                        r.len() == 40
                            && r.bytes().all(|b| {
                                b.is_ascii_digit() || (b.is_ascii_lowercase() && b <= b'f')
                            })
                    })
            })
            .unwrap_or(false);
        if well_formed {
            offenders.push(file.to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "these fuzz fixtures carry a WELL-FORMED model_provenance while the fuzz corpus is run \
         unpinned: {offenders:?}. Either drop the block, or give the fuzz corpus a \
         `reference_model_contract` (as golden_parity has) and pin both legs — leaving it is an \
         undeclared Rust-vs-Swift divergence (#114 F5)."
    );
}
