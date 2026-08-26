#!/usr/bin/env python3
# §12 (P4) — score-variant corpus ASSEMBLER + PINNER + MANIFEST writer.
#
# OFFLINE by construction: given a RAW teacher-forced base golden (the GPU-produced
# `mlxfast-swift generate-golden` output — cases[] computed BY the Swift engine, so "correct"
# = what Swift computes) and a DONOR golden that already carries a validated benchmark oracle +
# anchors + free_run (beefed.json), this grafts the §12 score-variant set the SAME way
# submit-1024.json was built (generate-golden cases[] + a benchmark-oracle block grafted from a
# beefed golden, then pinned sha256+bytes, dual-loader accepted). This script does the graft +
# pin + manifest; the GPU generate-golden step and the dual-loader acceptance run in the
# orchestrator (gen-variant-corpus.sh). Producing the corpus is decoupled from running it.
#
# Variants emitted (each = base cases[] + grafted benchmark oracle, differing ONLY in the
# correctness_gates section — so deterministic score parity is exercised on a widening shape):
#   minimal          cases[] + benchmark oracle, NO correctness_gates (base teacher-forced shape)
#   anchors-heavy    + correctness_gates.anchors (grafted from the donor)
#   free-run-only    + correctness_gates.free_run (grafted from the donor), NO anchors
#   behavior-bearing + correctness_gates.behavior — SYNTHETIC (see the DECLARED note below)
#
# behavior-bearing is marked `declared` in the manifest. The Qwen driver golden carries NO
# behavior gate (per the challenge-repo CLAUDE.md), and neither generate-golden nor the donor
# emits one, so a real teacher-forced behavior gate cannot be produced for this model offline.
# The gate here is SYNTHETIC (in-vocab filler) — it is LOADER-VALID (both loaders accept it) and
# under LOCAL modes `CorrectnessScope::BaseCasesOnly` does NOT evaluate correctness_gates at all,
# so deterministic score parity is still meaningful (loader-accept + score-invariance to the
# section's presence). But behavior-GATE EVALUATION is a Full/`--strict` concern, NOT exercised
# here; a divergence attributable to the synthetic gate is signed, not a real FAIL → DECLARED.
#
# The reused submit-1024.json (a real 1024-step submit golden already in hand) is NOT
# regenerated — it is recorded in the manifest by path + its known pin.
#
# Deterministic bytes: json.dumps(separators=(",",":"), ensure_ascii=False).encode() — the SAME
# serialization gen-failure-corpus.py uses, so the pin is reproducible.
import argparse, copy, hashlib, json, os, sys

# Per-mode decode windows (crates/bench-core/src/constants.rs
# LOCAL_ITERATE_BENCHMARK_DECODE_STEPS / LOCAL_SUBMIT_BENCHMARK_DECODE_STEPS).
LOCAL_ITERATE_DECODE_STEPS = 128
LOCAL_SUBMIT_DECODE_STEPS = 1023

# A golden can PHYSICALLY run a local mode only if its primary cases[0].expected_tokens carries the
# loader arity that mode's reference load call demands: `decode_steps + 1`, the `+ 1` being the SEED
# token (`expected_tokens[0]` = prefill/decode-seed argmax, `expected_tokens[k + 1]` = decode step
# `k`). Mirrors `Mode::golden_required_steps` (crates/benchctl/src/iterate.rs:76-81) and Swift
# `QwenRuntime.localIterate`'s `requiredSteps: options.benchmarkDecodeSteps + 1`. Carried per-variant
# as `applicable_modes` so variant-parity.sh renders an inapplicable mode as N/A (declared,
# non-FAIL), never TOOL-ERR.
#
# #124: local-iterate used to be returned UNCONDITIONALLY and submit gated at `>= 1023` — both
# looser than the loader. A sub-129-token golden was therefore mislabeled iterate-applicable and
# landed TOOL-ERR at load ("expected_tokens has N tokens; need at least 129") instead of N/A.
REQUIRED_TOKENS = {
    "local-iterate": LOCAL_ITERATE_DECODE_STEPS + 1,
    "local-submit": LOCAL_SUBMIT_DECODE_STEPS + 1,
}


def die(msg):
    sys.stderr.write(f"gen-variant-corpus: {msg}\n")
    sys.exit(2)


def applicable_modes_for(cases, label):
    """Modes whose loader arity (`decode_steps + 1`) cases[0].expected_tokens actually satisfies.

    A variant that satisfies NO mode is unrunnable everywhere: emitting it would hand
    variant-parity.sh a row that can only land TOOL-ERR. Fail LOUD at generation instead (#124).
    """
    n = len(cases[0].get("expected_tokens", [])) if cases else 0
    modes = [mode for mode, need in REQUIRED_TOKENS.items() if n >= need]
    if not modes:
        need = min(REQUIRED_TOKENS.values())
        die(
            f"{label}: cases[0].expected_tokens has {n} tokens; no local mode can load it "
            f"(need at least {need} for local-iterate). Regenerate the base with "
            f"--steps >= {need} (STEPS env in gen-variant-corpus.sh) — generate-golden emits "
            f"EXACTLY --steps expected_tokens, so --steps IS the arity."
        )
    return modes


def dump(doc):
    return json.dumps(doc, separators=(",", ":"), ensure_ascii=False).encode()


def load(path):
    try:
        with open(path, "rb") as f:
            raw = f.read()
        return json.loads(raw), raw
    except Exception as e:
        die(f"could not read/parse {path}: {e}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True, help="raw generate-golden output (teacher-forced cases[])")
    ap.add_argument("--donor", required=True, help="donor golden with benchmark oracle + anchors + free_run (beefed.json)")
    ap.add_argument("--out", required=True, help="corpus output dir")
    ap.add_argument("--submit", required=True, help="reused submit-1024.json (absolute path; NOT regenerated)")
    ap.add_argument("--submit-sha", required=True)
    ap.add_argument("--submit-bytes", required=True, type=int)
    ap.add_argument("--behavior-declared", default="#behavior-synthetic-local-unevaluated",
                    help="declared ref for behavior-bearing (synthetic gate, not evaluated under local BaseCasesOnly)")
    a = ap.parse_args()

    base, base_raw = load(a.base)
    donor, _ = load(a.donor)
    out = os.path.abspath(a.out)
    os.makedirs(out, exist_ok=True)

    # The base must carry the teacher-forced cases[] (and version/model_type provenance).
    if not base.get("cases"):
        die(f"--base {a.base} has no cases[] — expected generate-golden teacher-forced output")
    # The donor must carry the graftable sections; fail LOUD rather than emit a thin corpus.
    if not donor.get("benchmark"):
        die(f"--donor {a.donor} has no benchmark oracle to graft")
    dg = donor.get("correctness_gates") or {}
    if not dg.get("anchors"):
        die(f"--donor {a.donor} has no correctness_gates.anchors to graft")
    if not dg.get("free_run"):
        die(f"--donor {a.donor} has no correctness_gates.free_run to graft")

    version = base.get("version", 1)
    model_type = base.get("model_type")
    cases = base["cases"]
    benchmark = donor["benchmark"]
    anchors = dg["anchors"]
    free_run = dg["free_run"]
    base_case_names = {c.get("name") for c in cases}

    variants = []

    def emit(name, gates, sections, note, declared=None):
        doc = {"version": version}
        if model_type is not None:
            doc["model_type"] = model_type
        doc["cases"] = copy.deepcopy(cases)
        if gates is not None:
            doc["correctness_gates"] = gates
        doc["benchmark"] = copy.deepcopy(benchmark)
        b = dump(doc)
        path = os.path.join(out, f"{name}.json")
        with open(path, "wb") as f:
            f.write(b)
        variants.append({
            "class": name,
            "file": f"{name}.json",
            "path": path,
            "sha256": hashlib.sha256(b).hexdigest(),
            "bytes": len(b),
            "sections": sections,
            "applicable_modes": applicable_modes_for(doc["cases"], name),
            "declared": declared,
            "reused": False,
            "note": note,
        })

    # minimal — cases[] + benchmark oracle only (base teacher-forced shape).
    emit("minimal", None, ["cases", "benchmark"],
         "cases[] (generate-golden teacher-forced) + grafted benchmark oracle; NO correctness_gates")

    # anchors-heavy — + correctness_gates.anchors (grafted).
    emit("anchors-heavy", {"anchors": copy.deepcopy(anchors)},
         ["cases", "correctness_gates.anchors", "benchmark"],
         "+ correctness_gates.anchors grafted from donor")

    # free-run-only — + correctness_gates.free_run (grafted), NO anchors.
    emit("free-run-only", {"free_run": copy.deepcopy(free_run)},
         ["cases", "correctness_gates.free_run", "benchmark"],
         "+ correctness_gates.free_run grafted from donor; NO anchors")

    # behavior-bearing — + a SYNTHETIC correctness_gates.behavior (loader-valid, in-vocab
    # filler). Name is chosen to not collide with base/donor case names (layered-name uniqueness).
    beh_name = "behavior_synth"
    if beh_name in base_case_names:
        beh_name = "behavior_synth_v"
    behavior = [{
        "name": beh_name,
        "prompt_tokens": [1] * 8,
        "accepted_token_sequences": [[1, 1]],
        "max_new_tokens": 4,
    }]
    emit("behavior-bearing", {"behavior": behavior},
         ["cases", "correctness_gates.behavior", "benchmark"],
         "+ SYNTHETIC correctness_gates.behavior (in-vocab filler; NOT teacher-forced; local "
         "BaseCasesOnly does not evaluate it — deterministic parity meaningful, behavior-gate "
         "evaluation out of local scope)",
         declared=a.behavior_declared)

    # reused submit-1024.json — recorded by path + known pin; NOT regenerated. Its applicable modes
    # are read from ITS OWN cases[0] length (a 1024-step golden → both local modes).
    submit_path = os.path.abspath(a.submit)
    submit_doc, _ = load(submit_path)
    variants.append({
        "class": "submit-1024",
        "file": os.path.basename(submit_path),
        "path": submit_path,
        "sha256": a.submit_sha,
        "bytes": a.submit_bytes,
        "sections": ["cases", "correctness_gates.anchors", "correctness_gates.free_run", "benchmark"],
        "applicable_modes": applicable_modes_for(submit_doc.get("cases", []), "submit-1024"),
        "declared": None,
        "reused": True,
        "note": "reused 1024-step submit golden (NOT regenerated); pin from M-6",
    })

    manifest = {
        "source": os.path.basename(a.base),
        "base_source": os.path.abspath(a.base),
        "base_source_sha256": hashlib.sha256(base_raw).hexdigest(),
        "donor_source": os.path.abspath(a.donor),
        "generator": "mlxfast-swift generate-golden (base cases[]) + beefed graft (benchmark oracle + gates)",
        "note": "GPU step = generate-golden (teacher-forced cases[]); graft+pin+manifest are offline; "
                "dual-loader acceptance is engine-free (validate-golden + swift preflight).",
        "variants": variants,
    }
    with open(os.path.join(out, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")

    print(f"wrote {len([v for v in variants if not v['reused']])} generated variants "
          f"(+1 reused submit) to {out}")
    for v in variants:
        tag = f"  DECLARED({v['declared']})" if v.get("declared") else ""
        reused = "  [reused]" if v.get("reused") else ""
        modes = "/".join(m.replace("local-", "") for m in v["applicable_modes"])
        print(f"  {v['class']:16} {v['sha256'][:12]}… {v['bytes']:>6}B  "
              f"[{'+'.join(s.replace('correctness_gates.', 'cg.') for s in v['sections'])}]  "
              f"modes={modes}{tag}{reused}")


if __name__ == "__main__":
    main()
