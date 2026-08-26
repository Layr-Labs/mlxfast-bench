#!/usr/bin/env python3
# P5 — failing-run corpus generator. Takes a VALID benchmark golden and emits one corrupted
# variant per failure class, each still valid JSON (so the loader accepts it and the run fails
# at the correctness/benchmark stage, not at parse). Each variant is deterministic and pinned
# (sha256 + byte count) in a manifest, so the failure-map harness runs a fixed corpus.
#
# Classes emitted (when the source golden has the needed section):
#   primary          cases[0].expected_tokens[k] flipped -> primary teacher-forced mismatch
#   anchor           correctness_gates.anchors[0] expected/accepted flipped -> anchor mismatch
#   free-run         correctness_gates.free_run[0].expected_tokens[k] flipped -> free-run mismatch
#   behavior         correctness_gates.behavior[0] flipped (only if a behavior gate exists)
#   oracle           benchmark.expected_decode_tokens[k] flipped -> timed-oracle mismatch
#   baseline-missing benchmark baselines removed -> see the note below (#74 / #127)
#
# Each variant carries an optional `declared` issue ref in the manifest. A declared class renders
# as DECLARED(#nn) in the truth table (never FAIL); FAIL is reserved for UNDECLARED cells. The
# link is mandatory — a class is declared only WITH an issue ref (validated in add()).
#
# NOTE: this only PRODUCES the corpus; the harness (failure-map.sh) runs benchctl + swift on
# each and the differ field-diffs the failing score.jsons on the shared surface. What each
# side actually does is discovered empirically by the window, not predicted here.
#
# Usage: gen-failure-corpus.py <golden.json> <out_dir>
import json, sys, os, hashlib, copy

if len(sys.argv) != 3:
    sys.stderr.write("usage: gen-failure-corpus.py <golden.json> <out_dir>\n")
    sys.exit(2)

src_path, out_dir = sys.argv[1], sys.argv[2]
os.makedirs(out_dir, exist_ok=True)
with open(src_path, "rb") as f:
    raw = f.read()
src = json.loads(raw)


def flip(tok):
    """Deterministically move a token to a different in-vocab value."""
    return (tok + 1) if tok != 1 else 2


def dump(doc):
    # Match how goldens are serialized elsewhere: compact-ish but stable. json.dumps with
    # sort_keys=False preserves structure; the bytes only need to be VALID + deterministic.
    return json.dumps(doc, separators=(",", ":"), ensure_ascii=False).encode()


variants = []


def add(name, doc, note, declared=None):
    # `declared`: an issue ref (e.g. "#74") for a class whose bc-vs-swift divergence is SIGNED
    # and audited. The truth-table emitters render such a class as DECLARED(#nn) instead of FAIL
    # — FAIL is reserved for UNDECLARED cells (act on this). The link is MANDATORY: a class may
    # be marked declared only WITH an issue ref, so a declared cell always carries where it was
    # audited. `None` (default) = an ordinary class whose divergence, if any, is a real FAIL.
    if declared is not None and not (isinstance(declared, str) and declared.startswith("#")):
        raise ValueError(f"declared for {name!r} must be an issue ref like '#74', got {declared!r}")
    b = dump(doc)
    path = os.path.join(out_dir, f"{name}.json")
    with open(path, "wb") as f:
        f.write(b)
    variants.append({
        "class": name,
        "file": f"{name}.json",
        "sha256": hashlib.sha256(b).hexdigest(),
        "bytes": len(b),
        "note": note,
        "declared": declared,
    })


# primary — flip a mid-window token of the first case's expected_tokens.
if src.get("cases"):
    d = copy.deepcopy(src)
    et = d["cases"][0]["expected_tokens"]
    k = min(3, len(et) - 1)
    et[k] = flip(et[k])
    add("primary", d, f"cases[0].expected_tokens[{k}] flipped")

gates = src.get("correctness_gates", {})

# anchor — flip the first anchor's expected/accepted token.
if gates.get("anchors"):
    d = copy.deepcopy(src)
    a = d["correctness_gates"]["anchors"][0]
    if "expected_token" in a:
        a["expected_token"] = flip(a["expected_token"])
    if "accepted_tokens" in a and a["accepted_tokens"]:
        a["accepted_tokens"] = [flip(t) for t in a["accepted_tokens"]]
    add("anchor", d, "anchors[0] expected_token + accepted_tokens flipped")

# free-run — flip a token of the first free-run case's expected_tokens.
if gates.get("free_run"):
    d = copy.deepcopy(src)
    et = d["correctness_gates"]["free_run"][0]["expected_tokens"]
    k = min(2, len(et) - 1)
    et[k] = flip(et[k])
    add("free-run", d, f"free_run[0].expected_tokens[{k}] flipped")

# behavior — only if the source golden actually has a behavior gate.
if gates.get("behavior"):
    d = copy.deepcopy(src)
    b0 = d["correctness_gates"]["behavior"][0]
    if isinstance(b0, dict) and b0.get("expected_tokens"):
        b0["expected_tokens"][0] = flip(b0["expected_tokens"][0])
        add("behavior", d, "behavior[0].expected_tokens[0] flipped")

# oracle — flip a benchmark decode-oracle token (timed path).
if src.get("benchmark", {}).get("expected_decode_tokens"):
    d = copy.deepcopy(src)
    et = d["benchmark"]["expected_decode_tokens"]
    k = min(5, len(et) - 1)
    et[k] = flip(et[k])
    add("oracle", d, f"benchmark.expected_decode_tokens[{k}] flipped")

# baseline-missing — remove BOTH benchmark paired baselines.
#
# HISTORY, because this variant's meaning changed under two rulings and the class name did not:
#
#   As measured 2026-08-17 (against the RETIRED mlxfast-challenge-dev fork) both sides rejected,
#   at different points: benchctl computed golden_hash and ran 4 cases before refusing, the fork
#   refused at case 0 (case_count 4-vs-0 / golden_hash present-vs-absent). That fail-POINT
#   divergence was signed as #74.
#
#   #74 RULED 2026-08-20 (MIRROR SWIFT EXACTLY): benchd's early-refuse record now seals what the
#   reference seals — golden_hash "", counts 0 — pinned byte-for-byte in
#   crates/benchctl/tests/fixtures/swift-early-refuse-failure-record.json.
#
#   #127 RULED 2026-08-20 (MIRROR REFERENCE): the local legs take their baselines from the
#   constants and ignore the golden's pair entirely. So on the CURRENT reference (b26f76f) this
#   variant is not a refusal on either side: (nil, nil) is the accepted case in
#   validateBenchmarkGoldenBaselines (Golden.swift@b26f76f:658-660) and localIterate never reads
#   the pair. Both runners now ACCEPT it and score against their constants.
#
# The variant is KEPT because that is exactly what makes it worth running: on the LOCAL maps it is
# the negative control for #127 — a golden with no declared pair must now score identically to one
# declaring any pair, on both sides. (The half-present pair, which DOES still refuse through the
# loader, is covered by the loader-parity corpus's half_baseline.json.)
#
# It stays `declared`, and the ref moves #74 -> #127, because on the OFFICIAL map one divergence
# survives both rulings: benchd refuses an official run whose golden declares no pair and whose
# caller passed no override, where the reference falls back to the constants
# (resolvedBaseline* = baseline* ?? officialBaseline*, Golden.swift@b26f76f:220-226). That is
# benchd being deliberately STRICTER on the ranked path — the ranked runner measures its baseline
# in the same session (#61), so an official run with no pair in sight is a missing measurement,
# not a cue to score against a cached constant. Recorded on #127 (F8) and explicitly NOT changed
# by that ruling, which scoped itself to the local leg. A declared class renders DECLARED(#nn)
# only in place of a FAIL, so the local maps still render this cell PASS when it passes.
if src.get("benchmark"):
    d = copy.deepcopy(src)
    d["benchmark"].pop("baseline_decode_seconds_per_token", None)
    d["benchmark"].pop("baseline_prefill_seconds_per_token", None)
    add("baseline-missing", d,
        "benchmark paired baselines removed (local: #127 negative control; official: benchd stricter)",
        declared="#127")

manifest = {"source": os.path.basename(src_path),
            "source_sha256": hashlib.sha256(raw).hexdigest(),
            "variants": variants}
with open(os.path.join(out_dir, "manifest.json"), "w") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")

print(f"wrote {len(variants)} failure-class variants to {out_dir}")
for v in variants:
    tag = f"  DECLARED({v['declared']})" if v.get("declared") else ""
    print(f"  {v['class']:16} {v['sha256'][:12]}… {v['bytes']:>6}B  {v['note']}{tag}")
