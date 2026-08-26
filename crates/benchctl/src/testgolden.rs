//! Test-only builder for the golden documents benchctl's unit tests load.
//!
//! #63: a dozen tests in `iterate.rs` and `main.rs` each hand-rolled the same
//! `version` / `model_type` / `cases[]` / `benchmark` JSON, differing only in the one thing
//! under test — a corrupted expected token, an added gates block, a longer decode window.
//! The benchmark oracle block alone was copied verbatim more than ten times, so a schema
//! change meant finding every copy, and a test could silently drift from the shape it was
//! supposed to share with its neighbours.
//!
//! Tests now say only how they DIFFER from the canonical golden. The builder deliberately
//! goes through the real `load_golden_fixture`, so a golden a test builds is a golden the
//! loader accepted — a fixture that stops being valid fails loudly rather than quietly
//! testing nothing.

use bench_core::constants::{
    BENCHMARK_DECODE_SEED_TOKENS, BENCHMARK_DECODE_STEPS, BENCHMARK_PREFILL_PROMPT_TOKENS,
    CORRECTNESS_PROMPT_TOKENS, CORRECTNESS_STEPS, REQUIRED_GOLDEN_MODEL_TYPE,
};
use bench_core::golden::{load_golden_fixture, GoldenFixture};
use serde_json::json;

/// The standard benchmark oracle block, parameterised by the two things tests actually vary:
/// the token the prompt/seed arrays are filled with, and the token the decode window expects.
///
/// `prefill_fill` doubles as the decode SEED fill, matching every call site — the oracle's
/// prefill prompt and decode seed are the same workload in all of these fixtures.
pub fn benchmark_oracle(prefill_fill: i64, decode_expected: i64) -> serde_json::Value {
    json!({
        "prefill_prompt_tokens": vec![prefill_fill; BENCHMARK_PREFILL_PROMPT_TOKENS],
        "expected_prefill_token": 5,
        "decode_seed_tokens": vec![prefill_fill; BENCHMARK_DECODE_SEED_TOKENS],
        "expected_decode_seed_token": 6,
        "expected_decode_tokens": vec![decode_expected; BENCHMARK_DECODE_STEPS],
    })
}

/// A golden document under construction. Start from [`TestGolden::new`] and adjust.
pub struct TestGolden {
    doc: serde_json::Value,
    /// The `expected_tokens` arity this fixture is BUILT at and LOADED at. One field drives
    /// both, because the reference's arity is per-consumer (#109 window-4 E2): `correctness`
    /// takes the `correctnessSteps` default while `QwenRuntime.localIterate` loads at
    /// `benchmarkDecodeSteps + 1`. A fixture whose document arity and loader arity could drift
    /// apart is exactly how a test silently stops checking the window it claims to check.
    required_steps: usize,
}

impl TestGolden {
    /// The canonical golden: version 1, the required Qwen `model_type`, one primary case
    /// `p1` ([`CORRECTNESS_PROMPT_TOKENS`] prompt tokens of `1`, 64 expected tokens of `2`),
    /// and the standard
    /// benchmark oracle. No `correctness_gates`, so a conformance gate passes vacuously
    /// against the mock engine.
    ///
    /// The arity is the loader DEFAULT (`CORRECTNESS_STEPS`); a local-iterate/local-submit
    /// fixture must re-point it with [`TestGolden::steps`].
    pub fn new() -> Self {
        TestGolden {
            doc: json!({
                "version": 1,
                "model_type": REQUIRED_GOLDEN_MODEL_TYPE,
                "cases": [
                    {
                        "name": "p1",
                        "prompt_tokens": vec![1i64; CORRECTNESS_PROMPT_TOKENS],
                        "expected_tokens": vec![2i64; CORRECTNESS_STEPS],
                    }
                ],
                "benchmark": benchmark_oracle(1, 7),
            }),
            required_steps: CORRECTNESS_STEPS,
        }
    }

    /// Re-point the arity the LOADER requires, WITHOUT touching the document — for a fixture
    /// that supplies its own `expected_tokens` (the long local-submit window) but must still be
    /// validated against a mode's window.
    pub fn required_steps(mut self, steps: usize) -> Self {
        self.required_steps = steps;
        self
    }

    /// Size the fixture to `steps`: `expected_tokens` becomes `[2; steps]` AND the loader is
    /// asked to require that same arity. This is the local-iterate knob — the reference loads
    /// those goldens at `benchmarkDecodeSteps + 1`, so an iterate fixture built at the
    /// `correctnessSteps` default would be one the reference refuses.
    pub fn steps(self, steps: usize) -> Self {
        self.required_steps(steps).expected_fill(2)
    }

    /// Fill `expected_tokens` with `required_steps` copies of `fill`. The arity comes from the
    /// builder rather than the call site, so it cannot drift from the arity the loader checks.
    pub fn expected_fill(self, fill: i64) -> Self {
        let steps = self.required_steps;
        self.expected_tokens(vec![fill; steps])
    }

    /// Rename the primary case.
    pub fn case_name(mut self, name: &str) -> Self {
        self.doc["cases"][0]["name"] = json!(name);
        self
    }

    /// Replace the primary case's `prompt_tokens` with `len` copies of `fill`.
    pub fn prompt_fill(mut self, fill: i64) -> Self {
        self.doc["cases"][0]["prompt_tokens"] = json!(vec![fill; CORRECTNESS_PROMPT_TOKENS]);
        self
    }

    /// Replace the primary case's `expected_tokens` outright — the usual knob for the
    /// correctness-failure tests (corrupt one index) and the long local-submit window.
    pub fn expected_tokens(mut self, tokens: Vec<i64>) -> Self {
        self.doc["cases"][0]["expected_tokens"] = json!(tokens);
        self
    }

    /// The canonical all-`2` expected window with ONE token corrupted, which is how every
    /// base-case correctness-failure test builds its golden. The window is `required_steps`
    /// long, so `.steps(n).corrupt_expected_at(..)` corrupts a fixture that is still the right
    /// arity for the mode under test.
    pub fn corrupt_expected_at(self, index: usize, token: i64) -> Self {
        let mut expected = vec![2i64; self.required_steps];
        expected[index] = token;
        self.expected_tokens(expected)
    }

    /// Drop `model_type`. The loader only requires it when the caller asks for it, so this
    /// pairs with [`TestGolden::fixture_any_model_type`].
    pub fn without_model_type(mut self) -> Self {
        self.doc
            .as_object_mut()
            .expect("golden doc is an object")
            .remove("model_type");
        self
    }

    /// Attach a `correctness_gates` block.
    pub fn gates(mut self, gates: serde_json::Value) -> Self {
        self.doc["correctness_gates"] = gates;
        self
    }

    /// Replace the benchmark oracle block (see [`benchmark_oracle`]).
    pub fn benchmark(mut self, benchmark: serde_json::Value) -> Self {
        self.doc["benchmark"] = benchmark;
        self
    }

    /// Drop the benchmark oracle entirely — a structurally valid but benchmark-less golden.
    pub fn without_benchmark(mut self) -> Self {
        self.doc
            .as_object_mut()
            .expect("golden doc is an object")
            .remove("benchmark");
        self
    }

    /// Add the paired baselines to the benchmark oracle (§F2's requirement).
    pub fn baselines(mut self, prefill_spt: f64, decode_spt: f64) -> Self {
        self.doc["benchmark"]["baseline_prefill_seconds_per_token"] = json!(prefill_spt);
        self.doc["benchmark"]["baseline_decode_seconds_per_token"] = json!(decode_spt);
        self
    }

    /// The document's raw bytes — what the loader hashes into `GoldenFixture.sha256`.
    pub fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.doc).expect("golden doc serializes")
    }

    /// Load through the real loader, requiring the Qwen `model_type`, at this fixture's arity.
    pub fn fixture(&self) -> GoldenFixture {
        load_golden_fixture(
            &self.bytes(),
            self.required_steps,
            CORRECTNESS_PROMPT_TOKENS,
            Some(REQUIRED_GOLDEN_MODEL_TYPE),
            None,
            None,
        )
        .expect("test golden must load")
    }

    /// Load without requiring any `model_type` — for the fixtures that omit it.
    pub fn fixture_any_model_type(&self) -> GoldenFixture {
        load_golden_fixture(
            &self.bytes(),
            self.required_steps,
            CORRECTNESS_PROMPT_TOKENS,
            None,
            None,
            None,
        )
        .expect("test golden must load")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default must be the shape the tests that share it expect: a loadable Qwen golden
    /// with one primary case and a benchmark oracle. If this drifts, every test built on it
    /// drifts at once — so it is pinned here rather than implied by its users.
    #[test]
    fn canonical_golden_has_the_shared_shape() {
        let fx = TestGolden::new().fixture();
        assert_eq!(fx.model_type.as_deref(), Some(REQUIRED_GOLDEN_MODEL_TYPE));
        assert_eq!(fx.cases.len(), 1);
        assert_eq!(fx.cases[0].name, "p1");
        assert_eq!(fx.cases[0].prompt_tokens.len(), CORRECTNESS_PROMPT_TOKENS);
        assert_eq!(fx.cases[0].expected_tokens, vec![2i64; CORRECTNESS_STEPS]);
        assert!(fx.correctness_gates.is_none());
        assert!(fx.benchmark.is_some());
        assert_eq!(fx.sha256, bench_core::hash::sha256_hex(&fx_bytes()));
    }

    fn fx_bytes() -> Vec<u8> {
        TestGolden::new().bytes()
    }

    #[test]
    fn corrupt_expected_at_changes_exactly_one_token() {
        let fx = TestGolden::new().corrupt_expected_at(3, 999).fixture();
        let mut want = vec![2i64; CORRECTNESS_STEPS];
        want[3] = 999;
        assert_eq!(fx.cases[0].expected_tokens, want);
    }

    /// #109 window-4 E2: `steps` must move the DOCUMENT and the LOADER together. A fixture
    /// built at the local-iterate arity has to be one the reference's own load call accepts —
    /// if `steps` only widened the document, every iterate test would still be validated at the
    /// `correctnessSteps` default and the window drift this pins would stay invisible.
    #[test]
    fn steps_moves_the_document_and_the_loader_arity_together() {
        let li = crate::iterate::Mode::LocalIterate.golden_required_steps();
        assert!(li > CORRECTNESS_STEPS, "the local window is the wider one");
        let g = TestGolden::new().steps(li);
        assert_eq!(g.required_steps, li);
        let fx = g.fixture();
        assert_eq!(fx.cases[0].expected_tokens, vec![2i64; li]);

        // Built at the default arity, the same document is one the local-iterate loader refuses.
        let short = TestGolden::new();
        assert!(load_golden_fixture(
            &short.bytes(),
            li,
            CORRECTNESS_PROMPT_TOKENS,
            Some(REQUIRED_GOLDEN_MODEL_TYPE),
            None,
            None,
        )
        .is_err());
    }

    /// The two arity knobs compose: `required_steps` re-points the LOADER only, so a fixture
    /// supplying its own long window (local-submit) is still validated against the mode's
    /// arity; `expected_fill` takes its length FROM that arity rather than the call site.
    #[test]
    fn required_steps_and_expected_fill_track_the_builder_arity() {
        // Mode-derived, never a literal — a hard-coded window is what hid the #109 drift.
        let li = crate::iterate::Mode::LocalIterate.golden_required_steps();
        let submit = crate::iterate::Mode::LocalSubmit.golden_required_steps();

        // `required_steps` re-points the LOADER only; the caller's own long window survives.
        let fx = TestGolden::new()
            .required_steps(li)
            .expected_tokens(vec![2i64; submit])
            .fixture();
        assert_eq!(fx.cases[0].expected_tokens.len(), submit);

        let fx = TestGolden::new().steps(li).expected_fill(3).fixture();
        assert_eq!(fx.cases[0].expected_tokens, vec![3i64; li]);

        let fx = TestGolden::new()
            .steps(li)
            .corrupt_expected_at(3, 999)
            .fixture();
        let mut want = vec![2i64; li];
        want[3] = 999;
        assert_eq!(fx.cases[0].expected_tokens, want);
    }

    #[test]
    fn without_benchmark_drops_the_oracle_and_without_model_type_drops_the_id() {
        assert!(TestGolden::new()
            .without_benchmark()
            .fixture()
            .benchmark
            .is_none());
        assert!(TestGolden::new()
            .without_model_type()
            .fixture_any_model_type()
            .model_type
            .is_none());
    }

    #[test]
    fn baselines_land_inside_the_benchmark_block() {
        let fx = TestGolden::new().baselines(0.01, 0.1).fixture();
        let bm = fx.benchmark.expect("benchmark present");
        assert_eq!(bm.baseline_prefill_seconds_per_token, Some(0.01));
        assert_eq!(bm.baseline_decode_seconds_per_token, Some(0.1));
    }
}
