#!/usr/bin/env python3
"""M-4 — structure-aware fuzz corpus for the golden LOADER (accept/reject decision).

This EXTENDS the 12-fixture loader-parity corpus (scripts/gen-loader-parity-corpus.py,
docs/parity-matrix.md §6) to a large (N >= 100) deterministic fuzz corpus that probes the
LOADER's structural accept/reject decision — NOT the engine/correctness surface (that is the
job of gen-failure-corpus.py, whose variants stay VALID and fail later at run time). Every
fixture here MUTATES THE STRUCTURE so the loader itself must decide accept-or-reject:

  wrong field types, out-of-bounds counts, missing required keys, extra unknown keys,
  malformed gate shapes, boundary token values (0, negative, huge, vocab edge), duplicate
  keys, truncated / non-JSON payloads, wrong model_type, explicit nulls, etc.

DETERMINISM: seeded (SEED below); no wall-clock, no unseeded randomness. Re-running with the
same seed emits byte-identical fixtures — the corpus is FROZEN and PINNED (sha256 + byte
count per fixture in manifest.json), exactly like gen-failure-corpus.py's manifest.

DUAL-LOADER PARITY: each fixture is annotated with
  * expected_rust  ACCEPT/REJECT — the decision the Rust bench-core loader
                   (load_golden_fixture, required_model_type="gemma4_text", steps=64,
                   prompt_tokens=1024) must make. VERIFIED locally by benchctl validate-golden.
  * swift_diverges true when the Swift Golden.swift loader (mlxfast-swift preflight) is
                   KNOWN or PREDICTED to reach a DIFFERENT decision than Rust. The only such
                   family is UNKNOWN KEYS INSIDE INNER OBJECTS: Rust's serde
                   deny_unknown_fields rejects them at every level, while Swift's JSONDecoder
                   silently DROPS unknown keys inside Codable structs (per-case objects, the
                   benchmark block, the gates sections). Swift only key-set-validates the TOP
                   level, so a top-level unknown key is a shared REJECT (no divergence). The
                   per-case unknown-key divergence is already VERIFIED on the box in §6; the
                   benchmark/gates inner-object unknown keys are PREDICTED the same way and
                   flagged for box confirmation. Marking a fixture swift_diverges keeps the
                   dual harness from FAILING on it: if the two loaders actually AGREE it is
                   recorded MATCH, if they differ it is recorded KNOWN-DIV — never an
                   undeclared MISMATCH.

The corpus is consumed by:
  - crates/bench-core/tests/loader_fuzz.rs   (Rust side: asserts expected_rust + FREEZE pin)
  - scripts/loader-parity.sh                 (box side: benchctl validate-golden vs
                                              mlxfast-swift preflight, BOTH loaders live —
                                              the SAME hardened harness §6 uses; this manifest
                                              is drop-in compatible with it)
  - scripts/fuzz-corpus-check.sh             (local: freeze verify + benchctl-only verdicts)

Regenerate + commit after editing:  python3 scripts/gen-fuzz-corpus.py
"""
import copy
import hashlib
import json
import os
import random

# --- loader constants (mirror crates/bench-core/src/constants.rs) ---
VOCAB_SIZE = 262_144
STEPS = 64                    # CORRECTNESS_STEPS
PROMPT = 1024                 # CORRECTNESS_PROMPT_TOKENS
TOP_LOGITS = 8               # CORRECTNESS_TOP_LOGITS
MAX_ANCHOR_CTX = 1_024       # CORRECTNESS_MAX_ANCHOR_CONTEXT_TOKENS
MAX_FREE_RUN_STEPS = 256     # CORRECTNESS_MAX_FREE_RUN_STEPS
MAX_BEHAVIOR_PROMPT = 2_048  # CORRECTNESS_MAX_BEHAVIOR_PROMPT_TOKENS
MAX_BEHAVIOR_STEPS = 64      # CORRECTNESS_MAX_BEHAVIOR_STEPS
BENCH_PREFILL = 1024         # BENCHMARK_PREFILL_PROMPT_TOKENS
BENCH_DECODE_SEED = 1024     # BENCHMARK_DECODE_SEED_TOKENS
BENCH_DECODE_STEPS = 128     # BENCHMARK_DECODE_STEPS

REQUIRED_MODEL_TYPE = "gemma4_text"
SEED = 20260817  # fixed seed; deterministic filler only, the mutation structure is enumerated
RNG = random.Random(SEED)

OUT = os.path.join(os.path.dirname(__file__), "..", "crates", "bench-core", "tests",
                   "fixtures", "golden_fuzz")

ACCEPT, REJECT = "ACCEPT", "REJECT"


def tok():
    """A deterministic in-vocab token (seeded, so the corpus is reproducible)."""
    return RNG.randint(1, 1000)


def toks(n):
    return [tok() for _ in range(n)]


# --- valid canonical golden (accepted by BOTH loaders) -----------------------------------
def base_case(name="c1"):
    return {"name": name, "prompt_tokens": toks(PROMPT), "expected_tokens": toks(STEPS)}


def anchor(name="a1", **over):
    a = {"name": name, "context_tokens": toks(8), "expected_token": tok(),
         "accepted_tokens": [tok()], "max_expected_rank": 1, "max_top_logit_delta": 0.5}
    a.update(over)
    return a


def free_run(name="fr1", **over):
    fr = {"name": name, "prompt_tokens": toks(PROMPT), "expected_tokens": toks(3)}
    fr.update(over)
    return fr


def behavior(name="b1", **over):
    b = {"name": name, "prompt_tokens": toks(4), "accepted_token_sequences": [toks(3)],
         "max_new_tokens": 8}
    b.update(over)
    return b


def benchmark_block():
    return {
        "prefill_prompt_tokens": toks(BENCH_PREFILL),
        "expected_prefill_token": tok(),
        "decode_seed_tokens": toks(BENCH_DECODE_SEED),
        "expected_decode_seed_token": tok(),
        "expected_decode_tokens": toks(BENCH_DECODE_STEPS),
        "baseline_prefill_seconds_per_token": 0.010605031949609375,
        "baseline_decode_seconds_per_token": 0.1336139485703125,
    }


def valid():
    return {
        "version": 1,
        "model_type": REQUIRED_MODEL_TYPE,
        "cases": [base_case()],
        "correctness_gates": {
            "anchors": [anchor("a1"), anchor("a2", max_expected_rank=None,
                                             max_top_logit_delta=None)],
            "free_run": [free_run("fr1")],
        },
        "benchmark": benchmark_block(),
    }


def valid_with_behavior():
    v = valid()
    v["correctness_gates"]["behavior"] = [behavior("b1")]
    return v


HUGE = 1 << 40  # far above VOCAB_SIZE -> out-of-range token

# Each item: dict(file, doc-or-raw, expected_rust, swift_diverges, mutation, note)
ITEMS = []


def add(name, doc, expected_rust, mutation, note, swift_diverges=False):
    ITEMS.append({"file": f"{name}.json", "doc": doc, "expected_rust": expected_rust,
                  "swift_diverges": swift_diverges, "mutation": mutation, "note": note})


def add_raw(name, raw_bytes, expected_rust, mutation, note, swift_diverges=False):
    """For payloads that are not serialisable via json.dump (truncated / duplicate keys / non-JSON)."""
    ITEMS.append({"file": f"{name}.json", "raw": raw_bytes, "expected_rust": expected_rust,
                  "swift_diverges": swift_diverges, "mutation": mutation, "note": note})


def mut(fn):
    v = valid()
    fn(v)
    return v


# =========================================================================================
# Family Q — VALID variants (ACCEPT, both loaders)
# =========================================================================================
add("valid", valid(), ACCEPT, "none", "canonical golden — accepted by both loaders")
add("valid_with_behavior", valid_with_behavior(), ACCEPT, "add behavior gate",
    "canonical golden plus a well-formed behavior gate")


def _minimal(v):
    v.pop("correctness_gates", None)
    v.pop("benchmark", None)


# #77 (via #113): the ACCEPT/REJECT modelled here is the decision the PARITY HARNESS makes —
# `benchctl validate-golden` with no `--gates-only`, i.e. the structural load AND the benchmark
# oracle requirement — not the structural load alone. `loader_fuzz.rs` models exactly that
# (`load ok && fixture.benchmark.is_some()`). A structurally valid but benchmark-less golden is
# therefore REJECT on both sides, which is what closed the #77 divergence.
add("valid_cases_only", mut(_minimal), REJECT, "drop optional sections",
    "#77 FIXED: benchctl now requires the benchmark oracle by default → both loaders REJECT "
    "→ divergence closed.")


def _anchor_opt_null(v):
    # per-case OPTIONAL field explicitly null -> Rust None (no deny_explicit_null on per-case
    # fields), Swift nil. Both ACCEPT.
    v["correctness_gates"]["anchors"][0]["accepted_tokens"] = None
    v["correctness_gates"]["anchors"][0]["max_expected_rank"] = None
    v["correctness_gates"]["anchors"][0]["max_top_logit_delta"] = None


add("valid_anchor_optional_null", mut(_anchor_opt_null), ACCEPT, "per-case optional = null",
    "anchor optional fields explicitly null -> decoded as absent by both loaders")


def _extra_expected(v):
    v["cases"][0]["expected_tokens"] = toks(STEPS + 20)  # >= STEPS is fine


add("valid_extra_expected_tokens", mut(_extra_expected), ACCEPT, "expected_tokens length > steps",
    "expected_tokens longer than required steps is accepted (>= not ==)")


# =========================================================================================
# Family A — model_type (REJECT; both loaders require gemma4_text)
# =========================================================================================
def _del(key):
    return lambda v: v.pop(key, None)


add("model_type_missing", mut(_del("model_type")), REJECT, "drop model_type",
    "model_type absent; benchmark loader requires model_type==gemma4_text")

# Same near-miss structure as the qwen-era list, relative to the NEW identity: the retired
# identity itself (a real qwen-era golden MUST reject on the gemma track), truncations,
# unrelated families, a case variant, a suffix variant, the bare word, and the sibling id.
for i, mt in enumerate(["qwen3_5_text", "gemma4", "gemma4.text", "llama", "gemma",
                        "GEMMA4_TEXT", "gemma4_text_v2", "text", "gemma_text"]):
    add(f"model_type_wrong_{i:02d}", mut(lambda v, mt=mt: v.__setitem__("model_type", mt)),
        REJECT, f"model_type={mt!r}", "model_type != gemma4_text")

add("model_type_empty", mut(lambda v: v.__setitem__("model_type", "")), REJECT,
    "model_type=''", "empty model_type string")
add("model_type_leading_ws", mut(lambda v: v.__setitem__("model_type", " gemma4_text")),
    REJECT, "model_type=' gemma4_text'", "untrimmed model_type (leading whitespace)")
add("model_type_trailing_ws", mut(lambda v: v.__setitem__("model_type", "gemma4_text ")),
    REJECT, "model_type='gemma4_text '", "untrimmed model_type (trailing whitespace)")
add("model_type_null", mut(lambda v: v.__setitem__("model_type", None)), REJECT,
    "model_type=null", "explicit null model_type rejected by deny_explicit_null (Swift too)")
add("model_type_number", mut(lambda v: v.__setitem__("model_type", 7)), REJECT,
    "model_type=7", "model_type wrong type (number)")
add("model_type_bool", mut(lambda v: v.__setitem__("model_type", True)), REJECT,
    "model_type=true", "model_type wrong type (bool)")
add("model_type_array", mut(lambda v: v.__setitem__("model_type", [REQUIRED_MODEL_TYPE])),
    REJECT, "model_type=[...]", "model_type wrong type (array)")
add("model_type_object", mut(lambda v: v.__setitem__("model_type", {"v": REQUIRED_MODEL_TYPE})),
    REJECT, "model_type={...}", "model_type wrong type (object)")


# =========================================================================================
# Family B — version (REJECT unless exactly integer 1)
# =========================================================================================
for i, ver in enumerate([0, 2, -1, 100, 3]):
    add(f"version_wrong_{i:02d}", mut(lambda v, ver=ver: v.__setitem__("version", ver)),
        REJECT, f"version={ver}", "version must be exactly 1")
add("version_missing", mut(_del("version")), REJECT, "drop version", "version must be present and 1")
add("version_string", mut(lambda v: v.__setitem__("version", "1")), REJECT, "version='1'",
    "version wrong type (string)")
add("version_float", mut(lambda v: v.__setitem__("version", 1.5)), REJECT, "version=1.5",
    "version wrong type (non-integer float)")
add("version_bool", mut(lambda v: v.__setitem__("version", True)), REJECT, "version=true",
    "version wrong type (bool)")
add("version_null", mut(lambda v: v.__setitem__("version", None)), REJECT, "version=null",
    "version null (optional field, but must equal 1)")
add("version_array", mut(lambda v: v.__setitem__("version", [1])), REJECT, "version=[1]",
    "version wrong type (array)")


# =========================================================================================
# Family C — top-level structure (REJECT; unknown top key is a SHARED reject, no divergence)
# =========================================================================================
for i, key in enumerate(["bogus", "model_provenance", "extra", "notes", "signature", "_meta"]):
    val = {"model_id": "x", "revision": "y"} if key == "model_provenance" else 1
    # #113: the note is PER-FIXTURE, not per-family. `model_provenance` is no longer an
    # unknown TOP-level key (reference corrected: engine #11 -> PR #12) — that one fixture
    # rejects because its value carries an unknown INNER key (`model_id`) where the reference
    # allows exactly repository+revision. The other five really are unknown top-level keys.
    # Both reasons give the same shared-with-Swift REJECT, so the frozen bytes stay valid
    # corpus; only the reason differs, and each fixture must state its own.
    note = ("unknown top-level key (model_provenance: unknown INNER key model_id); "
            "Swift key-set-validates both levels -> shared REJECT"
            if key == "model_provenance" else
            "unknown top-level key; Swift key-set-validates the top level -> shared REJECT")
    add(f"unknown_top_key_{i:02d}",
        mut(lambda v, key=key, val=val: v.__setitem__(key, val)),
        REJECT, f"add top-level key {key!r}", note,
        swift_diverges=False)

add("cases_missing", mut(_del("cases")), REJECT, "drop cases", "cases is required")
add("cases_null", mut(lambda v: v.__setitem__("cases", None)), REJECT, "cases=null",
    "cases explicitly null (required Vec)")
add("cases_empty", mut(lambda v: v.__setitem__("cases", [])), REJECT, "cases=[]",
    "cases must be non-empty")
add("cases_object", mut(lambda v: v.__setitem__("cases", {"c1": base_case()})), REJECT,
    "cases={...}", "cases wrong type (object not array)")
add("cases_number", mut(lambda v: v.__setitem__("cases", 3)), REJECT, "cases=3",
    "cases wrong type (number)")
add("cases_string", mut(lambda v: v.__setitem__("cases", "c1")), REJECT, "cases='c1'",
    "cases wrong type (string)")


# =========================================================================================
# Family D — base-case field mutations (REJECT)
# =========================================================================================
def c0(v):
    return v["cases"][0]


add("case_missing_name", mut(lambda v: c0(v).pop("name")), REJECT, "drop case.name",
    "base case missing required name")
add("case_missing_prompt", mut(lambda v: c0(v).pop("prompt_tokens")), REJECT,
    "drop case.prompt_tokens", "base case missing required prompt_tokens")
add("case_missing_expected", mut(lambda v: c0(v).pop("expected_tokens")), REJECT,
    "drop case.expected_tokens", "base case missing required expected_tokens")
add("case_name_empty", mut(lambda v: c0(v).__setitem__("name", "")), REJECT, "case.name=''",
    "base case name empty")
add("case_name_leading_ws", mut(lambda v: c0(v).__setitem__("name", " c1")), REJECT,
    "case.name=' c1'", "base case name has leading whitespace")
add("case_name_trailing_ws", mut(lambda v: c0(v).__setitem__("name", "c1 ")), REJECT,
    "case.name='c1 '", "base case name has trailing whitespace")
add("case_name_control", mut(lambda v: c0(v).__setitem__("name", "c\t1")), REJECT,
    "case.name='c\\t1'", "base case name contains a control character")
add("case_name_number", mut(lambda v: c0(v).__setitem__("name", 5)), REJECT, "case.name=5",
    "base case name wrong type (number)")
add("case_name_null", mut(lambda v: c0(v).__setitem__("name", None)), REJECT, "case.name=null",
    "base case name null (required String)")

for i, n in enumerate([PROMPT - 1, PROMPT + 1, 0, 256, 512]):
    add(f"case_prompt_count_{i:02d}",
        mut(lambda v, n=n: c0(v).__setitem__("prompt_tokens", toks(n))),
        REJECT, f"prompt_tokens len={n}", f"prompt_tokens must be exactly {PROMPT}")
for i, n in enumerate([STEPS - 1, 0, 1, 32]):
    add(f"case_expected_count_{i:02d}",
        mut(lambda v, n=n: c0(v).__setitem__("expected_tokens", toks(n))),
        REJECT, f"expected_tokens len={n}", f"expected_tokens must be at least {STEPS}")

# boundary token values in prompt/expected
for i, (field, bad) in enumerate([
    ("prompt_tokens", -1), ("prompt_tokens", VOCAB_SIZE), ("prompt_tokens", VOCAB_SIZE + 1),
    ("prompt_tokens", HUGE), ("expected_tokens", -1), ("expected_tokens", VOCAB_SIZE),
    ("expected_tokens", -HUGE),
]):
    def _bad_tok(v, field=field, bad=bad):
        n = PROMPT if field == "prompt_tokens" else STEPS
        arr = toks(n)
        arr[n // 2] = bad
        c0(v)[field] = arr
    add(f"case_token_oob_{i:02d}", mut(_bad_tok), REJECT, f"{field} has {bad}",
        f"out-of-range token {bad} in {field} (valid range 0..{VOCAB_SIZE})")

# wrong element / container types
add("case_prompt_string_elem",
    mut(lambda v: c0(v).__setitem__("prompt_tokens", ["x"] * PROMPT)), REJECT,
    "prompt_tokens elem=string", "prompt_tokens element wrong type (string)")
add("case_prompt_float_elem",
    mut(lambda v: c0(v).__setitem__("prompt_tokens", [1.5] * PROMPT)), REJECT,
    "prompt_tokens elem=float", "prompt_tokens element wrong type (non-integer float)")
add("case_prompt_null_elem",
    mut(lambda v: c0(v).__setitem__("prompt_tokens", [None] * PROMPT)), REJECT,
    "prompt_tokens elem=null", "prompt_tokens element null")
add("case_prompt_not_array",
    mut(lambda v: c0(v).__setitem__("prompt_tokens", 5)), REJECT, "prompt_tokens=5",
    "prompt_tokens wrong type (number not array)")
add("case_expected_nested_array",
    mut(lambda v: c0(v).__setitem__("expected_tokens", [[1]] * STEPS)), REJECT,
    "expected_tokens elem=array", "expected_tokens element wrong type (nested array)")
add("case_not_object", mut(lambda v: v["cases"].__setitem__(0, [1, 2, 3])), REJECT,
    "cases[0]=array", "base case is not an object")


def _dup_case_name(v):
    v["cases"].append(base_case("c1"))  # same name as cases[0]


add("case_duplicate_name", mut(_dup_case_name), REJECT, "two cases named 'c1'",
    "duplicate base case name")


# =========================================================================================
# Family E — base-case UNKNOWN key (Rust REJECT / Swift DROPS -> DIVERGENCE)
# =========================================================================================
for i, key in enumerate(["acepted_tokens", "prompt_token", "expected_token", "weight", "id"]):
    add(f"case_unknown_key_{i:02d}",
        mut(lambda v, key=key: c0(v).__setitem__(key, [tok()])),
        REJECT, f"base case unknown key {key!r}",
        "unknown per-case key: Rust deny_unknown_fields REJECTS; Swift JSONDecoder DROPS -> "
        "ACCEPT (INTENTIONAL divergence, Rust stricter, anti-cheat)",
        swift_diverges=True)


# =========================================================================================
# Family F — correctness_gates structure (REJECT)
# =========================================================================================
def gates(v):
    return v["correctness_gates"]


add("gates_null", mut(lambda v: v.__setitem__("correctness_gates", None)), REJECT,
    "correctness_gates=null", "gates explicitly null (deny_explicit_null; Swift rejects too)")
add("gates_empty_object", mut(lambda v: v.__setitem__("correctness_gates", {})), REJECT,
    "correctness_gates={}", "gates object must declare at least one section")
add("gates_number", mut(lambda v: v.__setitem__("correctness_gates", 3)), REJECT,
    "correctness_gates=3", "gates wrong type (number)")
add("gates_array", mut(lambda v: v.__setitem__("correctness_gates", [])), REJECT,
    "correctness_gates=[]", "gates wrong type (array)")
add("gates_anchors_empty", mut(lambda v: gates(v).__setitem__("anchors", [])), REJECT,
    "anchors=[]", "declared anchors section must be non-empty")
add("gates_free_run_empty", mut(lambda v: gates(v).__setitem__("free_run", [])), REJECT,
    "free_run=[]", "declared free_run section must be non-empty")
add("gates_anchors_null", mut(lambda v: gates(v).__setitem__("anchors", None)), REJECT,
    "anchors=null", "anchors explicitly null (deny_explicit_null; Swift rejects too)")
add("gates_free_run_null", mut(lambda v: gates(v).__setitem__("free_run", None)), REJECT,
    "free_run=null", "free_run explicitly null (deny_explicit_null; Swift rejects too)")
add("gates_anchors_object", mut(lambda v: gates(v).__setitem__("anchors", {"a1": anchor()})),
    REJECT, "anchors={...}", "anchors wrong type (object not array)")

# unknown SECTION key inside the gates object. This was PREDICTED to diverge (Swift decodes
# gates via Codable, which MAY drop an unknown key inside an inner object) — the box run
# CONFIRMED Swift also REJECTS, so it is a shared reject, not a divergence. #113: that
# confirmation was recorded in the committed manifest but never carried back here, so
# regeneration kept re-asserting the stale prediction.
for i, key in enumerate(["anchorz", "free_runs", "behaviors"]):
    add(f"gates_unknown_section_{i:02d}",
        mut(lambda v, key=key: gates(v).__setitem__(key, [])),
        REJECT, f"gates unknown section {key!r}",
        "unknown gates-section key: Rust deny_unknown_fields REJECTS; Swift Codable may DROP "
        "(inner object) -> box-confirmed MATCH (Swift also REJECTS this inner-object unknown key)",
        swift_diverges=False)


# =========================================================================================
# Family G — anchor-case mutations (REJECT)
# =========================================================================================
def a0(v):
    return gates(v)["anchors"][0]


add("anchor_missing_name", mut(lambda v: a0(v).pop("name")), REJECT, "drop anchor.name",
    "anchor missing required name")
add("anchor_missing_context", mut(lambda v: a0(v).pop("context_tokens")), REJECT,
    "drop anchor.context_tokens", "anchor missing required context_tokens")
add("anchor_missing_expected", mut(lambda v: a0(v).pop("expected_token")), REJECT,
    "drop anchor.expected_token", "anchor missing required expected_token")
add("anchor_context_empty", mut(lambda v: a0(v).__setitem__("context_tokens", [])), REJECT,
    "anchor.context_tokens=[]", "anchor context_tokens must be non-empty")
add("anchor_context_too_long",
    mut(lambda v: a0(v).__setitem__("context_tokens", toks(MAX_ANCHOR_CTX + 1))), REJECT,
    f"anchor.context_tokens len={MAX_ANCHOR_CTX + 1}",
    f"anchor context_tokens exceeds max {MAX_ANCHOR_CTX}")
add("anchor_expected_oob", mut(lambda v: a0(v).__setitem__("expected_token", VOCAB_SIZE)),
    REJECT, "anchor.expected_token=VOCAB", "anchor expected_token out of vocab range")
add("anchor_expected_negative", mut(lambda v: a0(v).__setitem__("expected_token", -5)),
    REJECT, "anchor.expected_token=-5", "anchor expected_token negative")
add("anchor_accepted_empty", mut(lambda v: a0(v).__setitem__("accepted_tokens", [])), REJECT,
    "anchor.accepted_tokens=[]", "anchor accepted_tokens must be non-empty when present")
add("anchor_accepted_oob", mut(lambda v: a0(v).__setitem__("accepted_tokens", [HUGE])),
    REJECT, "anchor.accepted_tokens=[HUGE]", "anchor accepted_tokens out of vocab range")
for i, r in enumerate([0, TOP_LOGITS + 1, 999]):
    add(f"anchor_rank_bad_{i:02d}",
        mut(lambda v, r=r: a0(v).__setitem__("max_expected_rank", r)), REJECT,
        f"anchor.max_expected_rank={r}", f"max_expected_rank must be in 1..{TOP_LOGITS}")
add("anchor_rank_negative", mut(lambda v: a0(v).__setitem__("max_expected_rank", -1)),
    REJECT, "anchor.max_expected_rank=-1", "max_expected_rank negative (usize) -> decode reject")
add("anchor_delta_negative",
    mut(lambda v: a0(v).__setitem__("max_top_logit_delta", -0.5)), REJECT,
    "anchor.max_top_logit_delta=-0.5", "max_top_logit_delta must be non-negative")
add("anchor_delta_without_rank", mut(lambda v: (a0(v).__setitem__("max_top_logit_delta", 1.0),
                                                a0(v).__setitem__("max_expected_rank", None))),
    REJECT, "delta without rank", "max_top_logit_delta requires max_expected_rank")
add("anchor_context_string_elem",
    mut(lambda v: a0(v).__setitem__("context_tokens", ["x"])), REJECT,
    "anchor.context_tokens elem=string", "anchor context_tokens element wrong type")
add("anchor_expected_string", mut(lambda v: a0(v).__setitem__("expected_token", "9")),
    REJECT, "anchor.expected_token='9'", "anchor expected_token wrong type (string)")
add("anchor_not_object", mut(lambda v: gates(v)["anchors"].__setitem__(0, 5)), REJECT,
    "anchors[0]=5", "anchor case is not an object")


# =========================================================================================
# Family H — anchor UNKNOWN key (DIVERGENCE)
# =========================================================================================
for i, key in enumerate(["accepted_tokenss", "max_expected_ranke", "contextt_tokens", "note"]):
    add(f"anchor_unknown_key_{i:02d}",
        mut(lambda v, key=key: a0(v).__setitem__(key, 1)), REJECT,
        f"anchor unknown key {key!r}",
        "unknown anchor key: Rust REJECTS; Swift Codable DROPS -> ACCEPT (divergence)",
        swift_diverges=True)


# =========================================================================================
# Family I — free-run-case mutations (REJECT)
# =========================================================================================
def fr0(v):
    return gates(v)["free_run"][0]


add("free_run_missing_name", mut(lambda v: fr0(v).pop("name")), REJECT, "drop free_run.name",
    "free_run missing required name")
add("free_run_missing_prompt", mut(lambda v: fr0(v).pop("prompt_tokens")), REJECT,
    "drop free_run.prompt_tokens", "free_run missing required prompt_tokens")
add("free_run_missing_expected", mut(lambda v: fr0(v).pop("expected_tokens")), REJECT,
    "drop free_run.expected_tokens", "free_run missing required expected_tokens")
for i, n in enumerate([PROMPT - 1, PROMPT + 1, 0]):
    add(f"free_run_prompt_count_{i:02d}",
        mut(lambda v, n=n: fr0(v).__setitem__("prompt_tokens", toks(n))), REJECT,
        f"free_run prompt_tokens len={n}", f"free_run prompt_tokens must be exactly {PROMPT}")
add("free_run_expected_empty", mut(lambda v: fr0(v).__setitem__("expected_tokens", [])),
    REJECT, "free_run.expected_tokens=[]", "free_run expected_tokens must be non-empty")
add("free_run_expected_too_long",
    mut(lambda v: fr0(v).__setitem__("expected_tokens", toks(MAX_FREE_RUN_STEPS + 1))), REJECT,
    f"free_run expected_tokens len={MAX_FREE_RUN_STEPS + 1}",
    f"free_run expected_tokens exceeds max {MAX_FREE_RUN_STEPS}")
add("free_run_expected_oob",
    mut(lambda v: fr0(v).__setitem__("expected_tokens", [VOCAB_SIZE, 1, 2])), REJECT,
    "free_run expected_tokens has VOCAB", "free_run expected_tokens out of range")
for i, p in enumerate([0, 999]):
    add(f"free_run_prefix_bad_{i:02d}",
        mut(lambda v, p=p: fr0(v).__setitem__("exact_prefix_tokens", p)), REJECT,
        f"free_run.exact_prefix_tokens={p}", "exact_prefix_tokens must be in 1..len(expected)")
add("free_run_prefix_negative",
    mut(lambda v: fr0(v).__setitem__("exact_prefix_tokens", -1)), REJECT,
    "free_run.exact_prefix_tokens=-1", "exact_prefix_tokens negative (usize) -> decode reject")


# =========================================================================================
# Family J — free-run UNKNOWN key (DIVERGENCE)
# =========================================================================================
for i, key in enumerate(["exact_prefix_tokenss", "prompt_token", "extra"]):
    add(f"free_run_unknown_key_{i:02d}",
        mut(lambda v, key=key: fr0(v).__setitem__(key, 1)), REJECT,
        f"free_run unknown key {key!r}",
        "unknown free_run key: Rust REJECTS; Swift Codable DROPS -> ACCEPT (divergence)",
        swift_diverges=True)


# =========================================================================================
# Family K — behavior-case mutations (REJECT) — base has a behavior gate added
# =========================================================================================
def with_behavior_mut(fn):
    v = valid_with_behavior()
    fn(v)
    return v


def b0(v):
    return v["correctness_gates"]["behavior"][0]


add("behavior_missing_name", with_behavior_mut(lambda v: b0(v).pop("name")), REJECT,
    "drop behavior.name", "behavior missing required name")
add("behavior_missing_prompt", with_behavior_mut(lambda v: b0(v).pop("prompt_tokens")),
    REJECT, "drop behavior.prompt_tokens", "behavior missing required prompt_tokens")
add("behavior_missing_sequences",
    with_behavior_mut(lambda v: b0(v).pop("accepted_token_sequences")), REJECT,
    "drop behavior.accepted_token_sequences", "behavior missing required accepted_token_sequences")
add("behavior_missing_max_new",
    with_behavior_mut(lambda v: b0(v).pop("max_new_tokens")), REJECT,
    "drop behavior.max_new_tokens", "behavior missing required max_new_tokens")
add("behavior_prompt_empty",
    with_behavior_mut(lambda v: b0(v).__setitem__("prompt_tokens", [])), REJECT,
    "behavior.prompt_tokens=[]", "behavior prompt_tokens must be non-empty")
add("behavior_prompt_too_long",
    with_behavior_mut(lambda v: b0(v).__setitem__("prompt_tokens", toks(MAX_BEHAVIOR_PROMPT + 1))),
    REJECT, f"behavior.prompt_tokens len={MAX_BEHAVIOR_PROMPT + 1}",
    f"behavior prompt_tokens exceeds max {MAX_BEHAVIOR_PROMPT}")
for i, m in enumerate([0, MAX_BEHAVIOR_STEPS + 1]):
    add(f"behavior_max_new_bad_{i:02d}",
        with_behavior_mut(lambda v, m=m: b0(v).__setitem__("max_new_tokens", m)), REJECT,
        f"behavior.max_new_tokens={m}", f"max_new_tokens must be in 1..{MAX_BEHAVIOR_STEPS}")
add("behavior_sequences_empty",
    with_behavior_mut(lambda v: b0(v).__setitem__("accepted_token_sequences", [])), REJECT,
    "behavior.accepted_token_sequences=[]", "accepted_token_sequences must be non-empty")
add("behavior_sequence_elem_empty",
    with_behavior_mut(lambda v: b0(v).__setitem__("accepted_token_sequences", [[]])), REJECT,
    "behavior sequence=[]", "an accepted_token_sequence entry must be non-empty")
add("behavior_sequence_too_long",
    with_behavior_mut(lambda v: b0(v).__setitem__("accepted_token_sequences", [toks(20)])),
    REJECT, "behavior sequence len>max_new_tokens",
    "accepted_token_sequence longer than max_new_tokens")
add("behavior_sequence_oob",
    with_behavior_mut(lambda v: b0(v).__setitem__("accepted_token_sequences", [[VOCAB_SIZE]])),
    REJECT, "behavior sequence has VOCAB", "accepted_token_sequence token out of range")
add("behavior_prompt_oob",
    with_behavior_mut(lambda v: b0(v).__setitem__("prompt_tokens", [-1, 1])), REJECT,
    "behavior.prompt_tokens has -1", "behavior prompt_tokens out of range")


# =========================================================================================
# Family L — behavior UNKNOWN key (DIVERGENCE)
# =========================================================================================
for i, key in enumerate(["semantic_prompts", "max_new_token", "extra"]):
    add(f"behavior_unknown_key_{i:02d}",
        with_behavior_mut(lambda v, key=key: b0(v).__setitem__(key, "x")), REJECT,
        f"behavior unknown key {key!r}",
        "unknown behavior key: Rust REJECTS; Swift Codable DROPS -> ACCEPT (divergence)",
        swift_diverges=True)


# =========================================================================================
# Family M — benchmark-block mutations (REJECT)
# =========================================================================================
def bm(v):
    return v["benchmark"]


add("benchmark_null", mut(lambda v: v.__setitem__("benchmark", None)), REJECT,
    "benchmark=null", "benchmark explicitly null (deny_explicit_null; Swift rejects too)")
add("benchmark_number", mut(lambda v: v.__setitem__("benchmark", 3)), REJECT,
    "benchmark=3", "benchmark wrong type (number)")
add("benchmark_missing_prefill", mut(lambda v: bm(v).pop("prefill_prompt_tokens")), REJECT,
    "drop benchmark.prefill_prompt_tokens", "benchmark missing required prefill_prompt_tokens")
add("benchmark_missing_decode_seed", mut(lambda v: bm(v).pop("decode_seed_tokens")), REJECT,
    "drop benchmark.decode_seed_tokens", "benchmark missing required decode_seed_tokens")
add("benchmark_missing_expected_prefill", mut(lambda v: bm(v).pop("expected_prefill_token")),
    REJECT, "drop benchmark.expected_prefill_token", "benchmark missing expected_prefill_token")
for i, n in enumerate([BENCH_PREFILL - 1, BENCH_PREFILL + 1, 0]):
    add(f"benchmark_prefill_count_{i:02d}",
        mut(lambda v, n=n: bm(v).__setitem__("prefill_prompt_tokens", toks(n))), REJECT,
        f"benchmark prefill_prompt_tokens len={n}",
        f"prefill_prompt_tokens must be exactly {BENCH_PREFILL}")
for i, n in enumerate([BENCH_DECODE_SEED - 1, 0]):
    add(f"benchmark_decode_seed_count_{i:02d}",
        mut(lambda v, n=n: bm(v).__setitem__("decode_seed_tokens", toks(n))), REJECT,
        f"benchmark decode_seed_tokens len={n}",
        f"decode_seed_tokens must be exactly {BENCH_DECODE_SEED}")
add("benchmark_decode_steps_short",
    mut(lambda v: bm(v).__setitem__("expected_decode_tokens", toks(BENCH_DECODE_STEPS - 1))),
    REJECT, f"expected_decode_tokens len={BENCH_DECODE_STEPS - 1}",
    f"expected_decode_tokens must be at least {BENCH_DECODE_STEPS}")
add("benchmark_prefill_oob",
    mut(lambda v: bm(v).__setitem__("expected_prefill_token", VOCAB_SIZE)), REJECT,
    "benchmark.expected_prefill_token=VOCAB", "expected_prefill_token out of range")
add("benchmark_decode_token_oob",
    mut(lambda v: bm(v)["expected_decode_tokens"].__setitem__(0, -1)), REJECT,
    "benchmark.expected_decode_tokens[0]=-1", "expected_decode_tokens out of range")
add("benchmark_half_baseline_prefill",
    mut(lambda v: bm(v).pop("baseline_decode_seconds_per_token")), REJECT,
    "drop baseline_decode_seconds_per_token", "only one of the paired baselines present")
add("benchmark_half_baseline_decode",
    mut(lambda v: bm(v).pop("baseline_prefill_seconds_per_token")), REJECT,
    "drop baseline_prefill_seconds_per_token", "only one of the paired baselines present")
for i, (bad, lbl) in enumerate([(0.0, "0"), (-1.0, "-1"), ("x", "string")]):
    add(f"benchmark_baseline_bad_{i:02d}",
        mut(lambda v, bad=bad: bm(v).__setitem__("baseline_prefill_seconds_per_token", bad)),
        REJECT, f"baseline_prefill={lbl}", "baseline must be finite and positive")
add("benchmark_prefill_string_elem",
    mut(lambda v: bm(v).__setitem__("prefill_prompt_tokens", ["x"] * BENCH_PREFILL)), REJECT,
    "benchmark prefill_prompt_tokens elem=string", "benchmark token element wrong type")


# =========================================================================================
# Family N — benchmark UNKNOWN key (divergence PREDICTED, box-CONFIRMED as a match)
# =========================================================================================
for i, key in enumerate(["baseline_prefil_typo", "extra_baseline", "note"]):
    add(f"benchmark_unknown_key_{i:02d}",
        mut(lambda v, key=key: bm(v).__setitem__(key, 0.01)), REJECT,
        f"benchmark unknown key {key!r}",
        "unknown benchmark key: Rust deny_unknown_fields REJECTS; Swift Codable may DROP "
        "(inner object) -> box-confirmed MATCH (Swift also REJECTS this inner-object unknown key)",
        swift_diverges=False)


# =========================================================================================
# Family O — layered duplicate case names (REJECT)
# =========================================================================================
add("layered_dup_base_anchor",
    mut(lambda v: gates(v)["anchors"].append(anchor("c1"))), REJECT,
    "anchor name == base case name", "duplicate layered case name (base vs anchor)")
add("layered_dup_base_free_run",
    mut(lambda v: gates(v)["free_run"].append(free_run("c1"))), REJECT,
    "free_run name == base case name", "duplicate layered case name (base vs free_run)")
add("layered_dup_anchors",
    mut(lambda v: gates(v)["anchors"].append(anchor("a1"))), REJECT,
    "two anchors named 'a1'", "duplicate anchor case name")
add("layered_dup_free_run",
    mut(lambda v: gates(v)["free_run"].append(free_run("fr1"))), REJECT,
    "two free_run named 'fr1'", "duplicate free_run case name")


# =========================================================================================
# Family P — JSON-level malformations (REJECT at decode; both loaders parse-fail)
# =========================================================================================
_valid_bytes = json.dumps(valid()).encode()
add_raw("json_truncated", _valid_bytes[: len(_valid_bytes) // 2], REJECT,
        "truncate JSON at 50%", "truncated JSON payload; decode error")
add_raw("json_trailing_garbage", _valid_bytes + b"  garbage!!", REJECT,
        "append trailing garbage", "trailing non-JSON bytes; decode error")
add_raw("json_empty", b"", REJECT, "empty file", "empty payload; decode error")
add_raw("json_whitespace_only", b"   \n\t ", REJECT, "whitespace only", "no JSON value")
add_raw("json_top_level_array", b"[1,2,3]", REJECT, "top-level array", "root is an array, not an object")
add_raw("json_top_level_number", b"42", REJECT, "top-level number", "root is a number, not an object")
add_raw("json_top_level_string", b"\"golden\"", REJECT, "top-level string", "root is a string")
add_raw("json_not_json", b"this is not json", REJECT, "non-JSON text", "not JSON at all")

# duplicate JSON key: RFC 8259 leaves this implementation-defined. serde_json AND Swift
# JSONDecoder both take the LAST value, so a duplicate model_type whose last value is wrong
# makes BOTH loaders reject (wrong model_type) — deterministic shared REJECT.
_dup = ('{"version":1,"model_type":"' + REQUIRED_MODEL_TYPE + '","model_type":"qwen3_5_text",'
        '"cases":' + json.dumps(valid()["cases"]) + '}').encode()
add_raw("json_duplicate_model_type", _dup, REJECT, "duplicate model_type key (last=qwen3_5_text)",
        "duplicate JSON key; both loaders take the last value -> wrong model_type -> REJECT")


# =========================================================================================
# Emit corpus + manifest
# =========================================================================================
def main():
    os.makedirs(OUT, exist_ok=True)
    # Remove any stale fixtures so a rename/removal cannot leave an orphan in the frozen dir.
    for existing in os.listdir(OUT):
        if existing.endswith(".json"):
            os.remove(os.path.join(OUT, existing))

    seen = set()
    fixtures = []
    n_accept = n_reject = n_div = 0
    for it in ITEMS:
        fname = it["file"]
        if fname in seen:
            raise SystemExit(f"duplicate fixture filename {fname!r}")
        seen.add(fname)
        if "raw" in it:
            data = it["raw"]
        else:
            # Compact, stable serialisation; insertion order is deterministic in CPython.
            data = json.dumps(it["doc"], separators=(",", ":")).encode()
        with open(os.path.join(OUT, fname), "wb") as fh:
            fh.write(data)
        fixtures.append({
            "file": fname,
            "expected_rust": it["expected_rust"],
            "swift_diverges": it["swift_diverges"],
            "sha256": hashlib.sha256(data).hexdigest(),
            "bytes": len(data),
            "mutation": it["mutation"],
            "note": it["note"],
        })
        n_accept += it["expected_rust"] == ACCEPT
        n_reject += it["expected_rust"] == REJECT
        n_div += bool(it["swift_diverges"])

    manifest = {
        "corpus": "golden_fuzz",
        "purpose": "M-4 structure-aware fuzz corpus for the golden loader accept/reject decision",
        "seed": SEED,
        "required_model_type": REQUIRED_MODEL_TYPE,
        "steps": STEPS,
        "prompt_tokens": PROMPT,
        "vocab_size": VOCAB_SIZE,
        "count": len(fixtures),
        "accept": n_accept,
        "reject": n_reject,
        "predicted_swift_divergences": n_div,
        "fixtures": fixtures,
    }
    with open(os.path.join(OUT, "manifest.json"), "w") as fh:
        json.dump(manifest, fh, indent=2)
        fh.write("\n")

    print(f"wrote {len(fixtures)} fuzz fixtures + manifest.json to {os.path.normpath(OUT)}")
    print(f"  ACCEPT={n_accept}  REJECT={n_reject}  predicted swift-divergences={n_div}")


if __name__ == "__main__":
    main()
