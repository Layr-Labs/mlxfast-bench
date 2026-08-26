#!/usr/bin/env python3
"""Generate the loader-parity fixture corpus + manifest.

One valid canonical-format golden plus every malformed/divergent variant. Each
fixture is annotated with the decision the Rust bench-core loader (required_model_type=
"gemma4_text", steps=64, prompt_tokens=1024) must make, and whether the Swift Golden.swift
loader is KNOWN to diverge (so the cross-language harness can flag intentional divergences
instead of failing on them).

#114 — `expected_rust` is the decision the Rust loader makes WITH the corpus's track-contract
fixture supplied (`reference_model_contract.json`, the manifest's `reference_model_contract`),
because that is the configuration the Swift reference is being compared against: Swift pins the
reference model from its own constants and always applies it. `expected_rust_unpinned` records
the decision with NO contract in hand and is emitted ONLY for the fixtures where the two differ,
so the corpus states exactly which decisions the contract pin is load-bearing for.

The corpus is consumed by:
  - crates/bench-core/tests/loader_parity.rs  (Rust side: asserts both decisions)
  - scripts/loader-parity.sh                  (box side: benchctl validate-golden vs
                                               mlxfast-swift preflight, both loaders live)

Regenerate + commit after editing:  python3 scripts/gen-loader-parity-corpus.py
"""
import json
import os

STEPS = 64
PROMPT = 1024
PREFILL = 1024
DECODE_SEED = 1024
DECODE_STEPS = 128
OUT = os.path.join(os.path.dirname(__file__), "..", "crates", "bench-core", "tests",
                   "fixtures", "golden_parity")

# The corpus's own TRACK CONTRACT fixture: the #114 pin authority in miniature. Only the keys
# benchd reads for the reference-model identity are present -- the shape is the challenger track
# fixture's `target` block (`upstream_model_id` + `upstream_revision`), which is the same pair the
# reference fork hard-codes as MLXFastConstants.referenceModel{Repository,Revision}. Keeping the
# identity HERE, in a fixture, is the whole point of the ruling: benchd names no model in code.
CONTRACT_FILE = "reference_model_contract.json"
# The gemma track's reference model — MLXFastConstants.referenceModel{Repository,Revision}
# (mlxfast-gemma4-26b-a4b-engine/Sources/MLXFastCore/Constants.swift@ade3b99a:93-94). The box-side
# Swift leg (scripts/loader-parity.sh) always applies ITS compiled-in pin, so the corpus's
# positive-control provenance fixture must name the SAME model or the Swift leg rejects it.
REFERENCE_REPOSITORY = "mlx-community/gemma-4-26B-A4B-it-qat-4bit"
REFERENCE_REVISION = "0e3cbab38ce568cf6e23543010d08d03b731910c"


def contract():
    return {
        "schema_version": 1,
        "track_id": "loader-parity-corpus-v1",
        "target": {
            "upstream_model_id": REFERENCE_REPOSITORY,
            "upstream_revision": REFERENCE_REVISION,
        },
    }


def base_case(name="c1"):
    return {"name": name, "prompt_tokens": [1] * PROMPT, "expected_tokens": [2] * STEPS}


# The paired baselines every emitted fixture carries. These are the REFERENCE's
# MLXFastConstants.officialBaseline{Prefill,Decode}SecondsPerToken
# (qwen-engine-verify/Sources/MLXFastCore/Constants.swift:255-256 @ b26f76f).
#
# #127 (F10) — this used to hard-code the RETIRED mlxfast-challenge-dev fork's Gemma-era pair
# (0.010605031949609375 / 0.1336139485703125, that fork's Constants.swift:124-125), so all 14
# oracle-bearing fixtures inherited it. INERT for what this corpus decides: these fixtures drive
# loader accept/reject, and the loader validates baseline SHAPE (paired, finite, positive) without
# ever comparing values -- so nothing here changes any expected_rust/expected_swift decision, only
# the fixture bytes (and therefore every sha256 in the manifest). Corrected anyway: the retired
# fork's constants are a stale-reference hazard wherever they survive, and a future check that DOES
# compare baseline values would have inherited them silently.
BASELINE_PREFILL_SECONDS_PER_TOKEN = 0.00036751938916015625
BASELINE_DECODE_SECONDS_PER_TOKEN = 0.01385621216015625


def benchmark_block():
    return {
        "prefill_prompt_tokens": [1] * PREFILL,
        "expected_prefill_token": 5,
        "decode_seed_tokens": [1] * DECODE_SEED,
        "expected_decode_seed_token": 6,
        "expected_decode_tokens": [7] * DECODE_STEPS,
        "baseline_prefill_seconds_per_token": BASELINE_PREFILL_SECONDS_PER_TOKEN,
        "baseline_decode_seconds_per_token": BASELINE_DECODE_SECONDS_PER_TOKEN,
    }


def valid():
    return {
        "version": 1,
        "model_type": "gemma4_text",
        "cases": [base_case()],
        "correctness_gates": {
            "anchors": [
                {"name": "a1", "context_tokens": [1] * 8, "expected_token": 100,
                 "accepted_tokens": [100], "max_expected_rank": 1, "max_top_logit_delta": 0.5},
                {"name": "a2", "context_tokens": [1] * 8, "expected_token": 200,
                 "accepted_tokens": [200]},
            ],
            "free_run": [
                {"name": "fr1", "prompt_tokens": [1] * PROMPT, "expected_tokens": [9, 9, 9]},
            ],
        },
        "benchmark": benchmark_block(),
    }


# Each entry: (filename, mutate(dict)->dict, expected_rust ACCEPT/REJECT, note, swift_diverges)
# plus two optional trailing elements:
#   [5] expected_rust_unpinned      — when the contract-less decision differs;
#   [6] expected_rust_message_contains — a substring the REJECT diagnostic must contain, for rows
#       that pin an ORDER rather than a decision (both loaders reject, but on different grounds).
def corpus():
    def mut(f):
        v = valid()
        f(v)
        return v

    def drop(k):
        return lambda: (lambda v: (v.pop(k), v)[1])(valid())

    items = []
    items.append(("valid.json", valid(), "ACCEPT", "canonical golden", False))

    # `model_provenance` is an ACCEPTED optional key since the reference loader was corrected
    # (mlxfast-qwen-38-27b-mtp-engine #11 -> PR #12, 2be4e21): model_type is the schema key and
    # model_provenance rides alongside it. Both shapes are pinned here — a WELL-FORMED block
    # accepts, and a MALFORMED one still rejects, so the widening is bounded by a fixture.
    v = valid(); v["model_provenance"] = {"model_id": "x", "revision": "y"}
    items.append(("model_provenance.json", v, "REJECT",
                  "model_provenance carrying an unknown inner key (model_id): the reference allows "
                  "exactly repository+revision -> shared REJECT", False))

    v = valid(); v["model_provenance"] = {
        "repository": REFERENCE_REPOSITORY,
        "revision": REFERENCE_REVISION,
    }
    items.append(("model_provenance_valid.json", v, "ACCEPT",
                  "well-formed model_provenance NAMING the contract's declared reference model "
                  "(repository + 40-hex revision): an OPTIONAL key on the corrected reference "
                  "schema, and the positive control for the #114 value pin -- it must stay "
                  "ACCEPT with the contract supplied, or the pin is rejecting everything", False))

    # #114 — RULED (David 2026-08-20): the reference-model identity is declared by the TRACK
    # CONTRACT, not by a compiled-in benchd constant. This row was the declared divergence
    # (#112 M2: Rust ACCEPT / Swift REJECT, `swift_diverges: true`); with the corpus contract
    # supplied it is now a SHARED REJECT and the divergence is CLOSED for contract-bearing
    # callers. `expected_rust_unpinned: ACCEPT` keeps the contract-less decision on the record,
    # which is exactly the residual the parity matrix still declares for benchd's commands that
    # take no `--contract` (iterate / correctness).
    v = valid(); v["model_provenance"] = {
        "repository": "NotTheOrganizer/Some-Other-Model-4bit",
        "revision": "0123456789abcdef0123456789abcdef01234567",
    }
    items.append(("model_provenance_not_pinned.json", v, "REJECT",
                  "well-formed model_provenance whose VALUES name a DIFFERENT model (valid "
                  "shape: non-empty repository + 40-hex lowercase revision, but neither is the "
                  "contract's declared reference model): REJECTED against the track contract's "
                  "target pin, matching Swift's constant-driven pin -> SHARED REJECT (#114). "
                  "WITHOUT a contract the same bytes ACCEPT (shape-only): see "
                  "expected_rust_unpinned", False, "ACCEPT"))

    # #114 (F1) — a golden wrong in BOTH the model_type gate AND the provenance identity. Both
    # loaders REJECT, so a decision-only harness cannot see the ordering; this row exists so the
    # ORDER is pinned by a fixture. The reference interleaves requiredModelType
    # (Golden.swift:377-385) BEFORE the provenance identity (:386-393), so the model_type
    # diagnostic is the one that must come out of either loader. The Rust-side assertion on the
    # message lives in loader_parity.rs (`expected_rust_message_contains`).
    v = valid()
    v["model_type"] = "gemma_text"
    v["model_provenance"] = {
        "repository": "NotTheOrganizer/Some-Other-Model-4bit",
        "revision": "0123456789abcdef0123456789abcdef01234567",
    }
    items.append(("wrong_model_type_and_provenance.json", v, "REJECT",
                  "wrong model_type AND a provenance naming a different model: BOTH loaders "
                  "REJECT, and both must report the MODEL_TYPE defect -- the reference checks "
                  "requiredModelType BEFORE the provenance identity, so a loader that reported "
                  "the provenance mismatch here would be decision-identical but diagnostic-"
                  "divergent (#114 F1)", False, None,
                  "correctness golden file model_type="))

    v = valid(); del v["model_type"]
    items.append(("missing_model_type.json", v, "REJECT",
                  "model_type absent; the benchmark loader requires model_type==gemma4_text", False))

    v = valid(); v["model_type"] = "qwen3_5_text"
    items.append(("wrong_model_type.json", v, "REJECT", "model_type != gemma4_text "
                  "(the retired qwen-era identity must reject on the gemma track)", False))

    v = valid(); v["bogus"] = 1
    items.append(("unknown_top_key.json", v, "REJECT", "unknown top-level key", False))

    v = valid(); v["version"] = 2
    items.append(("bad_version.json", v, "REJECT", "version must be 1", False))

    v = valid(); v["cases"] = []
    items.append(("empty_cases.json", v, "REJECT", "cases must be non-empty", False))

    v = valid(); v["cases"][0]["expected_tokens"] = [2] * (STEPS - 1)
    items.append(("short_expected_tokens.json", v, "REJECT",
                  f"primary case expected_tokens < {STEPS}", False))

    v = valid(); v["cases"][0]["prompt_tokens"] = [1] * (PROMPT - 1)
    items.append(("wrong_prompt_tokens.json", v, "REJECT",
                  f"primary case prompt_tokens != {PROMPT}", False))

    v = valid(); v["benchmark"] = None
    items.append(("null_benchmark.json", v, "REJECT", "benchmark explicitly null", False))

    v = valid(); del v["benchmark"]["baseline_prefill_seconds_per_token"]
    items.append(("half_baseline.json", v, "REJECT",
                  "only one of the paired baselines present", False))

    v = valid(); v["cases"][0]["acepted_tokens"] = [2]  # deliberate typo'd per-case key
    items.append(("per_case_unknown_key.json", v, "REJECT",
                  "per-case unknown key: Rust deny_unknown_fields rejects it; Swift JSONDecoder "
                  "silently DROPS it and ACCEPTS -> INTENTIONAL divergence (Rust stricter, anti-cheat)",
                  True))

    return items


def main():
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, CONTRACT_FILE), "w") as fh:
        json.dump(contract(), fh, indent=2)
    manifest = []
    for entry in corpus():
        fname, doc, expected_rust, note, swift_diverges = entry[:5]
        with open(os.path.join(OUT, fname), "w") as fh:
            json.dump(doc, fh)
        row = {"file": fname, "expected_rust": expected_rust,
               "swift_diverges": swift_diverges, "note": note}
        if len(entry) > 5 and entry[5] is not None:
            row["expected_rust_unpinned"] = entry[5]
        if len(entry) > 6 and entry[6] is not None:
            row["expected_rust_message_contains"] = entry[6]
        manifest.append(row)
    with open(os.path.join(OUT, "manifest.json"), "w") as fh:
        json.dump({"required_model_type": "gemma4_text", "steps": STEPS,
                   "prompt_tokens": PROMPT,
                   "reference_model_contract": CONTRACT_FILE, "fixtures": manifest}, fh, indent=2)
    print(f"wrote {len(manifest)} fixtures + {CONTRACT_FILE} + manifest.json "
          f"to {os.path.normpath(OUT)}")
    for m in manifest:
        unpinned = m.get("expected_rust_unpinned")
        print(f"  {m['expected_rust']:6} {m['file']:32} "
              f"{'(swift diverges)' if m['swift_diverges'] else ''}"
              f"{f'(unpinned: {unpinned})' if unpinned else ''}")


if __name__ == "__main__":
    main()
