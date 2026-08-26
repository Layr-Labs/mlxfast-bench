#!/bin/bash
# scripts/test-variant-offline.sh — OFFLINE self-test for the §12 + facade window scripts.
#
# Proves the NON-GPU control flow of gen-variant-corpus / variant-parity / facade-leg
# fail-loud correctly, WITHOUT a GPU and WITHOUT faking a GPU run: it uses canned goldens and a
# stub benchctl / stub swift / stub reference (the compat-matrix.sh stub pattern). What it
# verifies:
#   0. `bash -n` every new script + `py_compile` the assembler.
#   1. gen-variant-corpus.py assembles 4 variants + a manifest whose pins re-verify (sha256+bytes).
#   2. variant-lib: pin_check (ok/mismatch/missing), render_verdict (PASS/FAIL/DECLARED passthrough),
#      manifest_rows (parse + fail-loud on empty), dual_loader (accept / reject / harness-err).
#   3. variant-parity end-to-end with stubs: all-PASS → RESULT PASS + one row per manifest variant;
#      forced-FAIL → RESULT FAIL AND the declared `behavior-bearing` cell renders DECLARED (never FAIL).
#   4. facade-leg end-to-end with a stub reference + stub benchctl → GREEN; exit-code skew → FAIL.
#
# The REAL GPU steps (generate-golden; live benchctl iterate + swift benchmark; live facade vs
# reference) are NOT run here — those are the window's job. Exit 0 = all green.
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/variant-offline.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  PASS  %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; }

# shellcheck source=scripts/variant-lib.sh
. "$HERE/variant-lib.sh"

echo "== 0. syntax checks =="
for s in variant-lib.sh gen-variant-corpus.sh variant-parity.sh facade-leg.sh run-variant-window.sh test-variant-offline.sh; do
  if bash -n "$HERE/$s" 2>"$WORK/synerr"; then ok "bash -n $s"; else bad "bash -n $s"; sed 's/^/        /' "$WORK/synerr"; fi
done
if python3 -m py_compile "$HERE/gen-variant-corpus.py" 2>"$WORK/pyerr"; then ok "py_compile gen-variant-corpus.py"; else bad "py_compile gen-variant-corpus.py"; sed 's/^/        /' "$WORK/pyerr"; fi
echo ""

# ---- canned goldens (valid shape per crates/bench-core/src/golden.rs) -------------------
mk_goldens() {
  python3 - "$WORK" <<'PY'
import json, sys, os
w = sys.argv[1]
def dump(doc, path): open(path,"w").write(json.dumps(doc, separators=(",",":"), ensure_ascii=False))
# 129 = the local-iterate loader arity (benchmarkDecodeSteps 128 + 1 SEED). A shorter base is
# refused by BOTH loaders, so the canned base must carry it too (#124) — the sub-arity case is
# exercised as an explicit NEGATIVE below rather than as the default fixture.
base = {"version":1,"model_type":"qwen3_5_text",
        "cases":[{"name":"c1","prompt_tokens":[1]*512,"expected_tokens":[2]*129}]}
short_base = {"version":1,"model_type":"qwen3_5_text",
              "cases":[{"name":"c1","prompt_tokens":[1]*512,"expected_tokens":[2]*64}]}
bench = {"prefill_prompt_tokens":[1]*512,"expected_prefill_token":5,
         "decode_seed_tokens":[1]*512,"expected_decode_seed_token":6,
         "expected_decode_tokens":[7]*128,
         "baseline_prefill_seconds_per_token":0.0106,"baseline_decode_seconds_per_token":0.1336}
donor = dict(base)
donor["correctness_gates"]={"anchors":[{"name":"a1","context_tokens":[1]*8,"expected_token":100,"accepted_tokens":[100]}],
                            "free_run":[{"name":"fr1","prompt_tokens":[1]*512,"expected_tokens":[9,9,9]}]}
donor["benchmark"]=bench
dump(base, os.path.join(w,"base.json"))
dump(short_base, os.path.join(w,"short-base.json"))
dump(donor, os.path.join(w,"donor.json"))
# submit golden carries a 1024-token primary case → applicable to BOTH local modes (submit's loader
# arity is 1023+1), unlike the 129-token base (iterate-only). Exercises applicable-modes both ways.
submit_cases=[{"name":"c1","prompt_tokens":[1]*512,"expected_tokens":[2]*1024}]
dump({"version":1,"model_type":"qwen3_5_text","cases":submit_cases,"benchmark":bench}, os.path.join(w,"submit.json"))
# #124 F1 BOUNDARY: 1023 tokens is one BELOW local-submit's loader arity (1023 decode steps + 1
# SEED = 1024). It must be iterate-only. The 1024-token fixture above passes under BOTH the old
# (>=1023) and fixed (>=1024) gate, so without this fixture the submit-side off-by-one has no
# coverage at all.
submit_short=[{"name":"c1","prompt_tokens":[1]*512,"expected_tokens":[2]*1023}]
dump({"version":1,"model_type":"qwen3_5_text","cases":submit_short,"benchmark":bench}, os.path.join(w,"submit-1023.json"))
PY
}
mk_goldens
SUBMIT_SHA_C="$(shasum -a 256 "$WORK/submit.json" | awk '{print $1}')"
SUBMIT_BYTES_C="$(wc -c < "$WORK/submit.json" | tr -d ' ')"

echo "== 1. assembler: 4 variants + manifest, pins re-verify =="
if python3 "$HERE/gen-variant-corpus.py" --base "$WORK/base.json" --donor "$WORK/donor.json" \
     --out "$WORK/corpus" --submit "$WORK/submit.json" --submit-sha "$SUBMIT_SHA_C" \
     --submit-bytes "$SUBMIT_BYTES_C" >"$WORK/asm.out" 2>&1; then ok "assembler ran"; else bad "assembler failed"; sed 's/^/        /' "$WORK/asm.out"; fi
MANIFEST="$WORK/corpus/manifest.json"
for v in minimal anchors-heavy free-run-only behavior-bearing; do
  [ -f "$WORK/corpus/$v.json" ] && ok "emitted $v.json" || bad "missing $v.json"
done
# re-verify every manifest pin against the emitted bytes
PIN_BAD=0
while IFS=$'\t' read -r cls path sha bytes _d; do
  [ -n "$cls" ] || continue
  variant_pin_check "$path" "$sha" "$bytes" >/dev/null 2>&1 || { PIN_BAD=$((PIN_BAD+1)); echo "        pin re-verify FAILED for $cls ($path)"; }
done < <(variant_manifest_rows "$MANIFEST")
[ "$PIN_BAD" -eq 0 ] && ok "all manifest pins re-verify (sha256+bytes)" || bad "$PIN_BAD manifest pin(s) do not re-verify"
# behavior-bearing must be declared; the others must not
DEC="$(python3 -c "import json;print(next(v.get('declared') or '' for v in json.load(open('$MANIFEST'))['variants'] if v['class']=='behavior-bearing'))")"
[ -n "$DEC" ] && ok "behavior-bearing carries a declared ref ($DEC)" || bad "behavior-bearing not declared"
UND="$(python3 -c "import json;print(sum(1 for v in json.load(open('$MANIFEST'))['variants'] if v['class'] in ('minimal','anchors-heavy','free-run-only','submit-1024') and v.get('declared')))")"
[ "$UND" = "0" ] && ok "ordinary variants carry no declared ref" || bad "an ordinary variant is spuriously declared"
# applicable_modes: 129-token shape variants → iterate-only; 1024-token submit-1024 → both modes.
SHAPE_AM="$(python3 -c "import json;print(','.join('/'.join(v['applicable_modes']) for v in json.load(open('$MANIFEST'))['variants'] if v['class'] in ('minimal','anchors-heavy','free-run-only','behavior-bearing')))")"
[ "$SHAPE_AM" = "local-iterate,local-iterate,local-iterate,local-iterate" ] && ok "shape variants: applicable_modes = [local-iterate] only (iterate-scale)" || bad "shape-variant applicable_modes wrong ($SHAPE_AM)"
SUB_AM="$(python3 -c "import json;print('/'.join(next(v['applicable_modes'] for v in json.load(open('$MANIFEST'))['variants'] if v['class']=='submit-1024')))")"
[ "$SUB_AM" = "local-iterate/local-submit" ] && ok "submit-1024: applicable_modes = [local-iterate, local-submit]" || bad "submit-1024 applicable_modes wrong ($SUB_AM)"
# #124 F1 BOUNDARY (submit side): a 1023-token golden is ONE token below local-submit's loader
# arity (1023 decode steps + 1 SEED = 1024) → iterate-ONLY. This is the assertion that actually
# pins the submit gate: the 1024-token fixture above is satisfied by the old `>= 1023` gate too,
# so reverting REQUIRED_TOKENS["local-submit"] to 1023 leaves the whole suite green without it.
SUBMIT_SHA_S="$(shasum -a 256 "$WORK/submit-1023.json" | awk '{print $1}')"
SUBMIT_BYTES_S="$(wc -c < "$WORK/submit-1023.json" | tr -d ' ')"
if python3 "$HERE/gen-variant-corpus.py" --base "$WORK/base.json" --donor "$WORK/donor.json" \
     --out "$WORK/corpus-b1023" --submit "$WORK/submit-1023.json" --submit-sha "$SUBMIT_SHA_S" \
     --submit-bytes "$SUBMIT_BYTES_S" >"$WORK/asm-b1023.out" 2>&1; then
  B_AM="$(python3 -c "import json;print('/'.join(next(v['applicable_modes'] for v in json.load(open('$WORK/corpus-b1023/manifest.json'))['variants'] if v['class']=='submit-1024')))")"
  [ "$B_AM" = "local-iterate" ] \
    && ok "#124 F1: 1023-token golden → applicable_modes = [local-iterate] ONLY (submit needs 1024)" \
    || bad "#124 F1: 1023-token golden claimed submit-applicable ($B_AM) — submit gate is off by one"
else
  bad "#124 F1: assembler failed on the 1023-token boundary fixture"; sed 's/^/        /' "$WORK/asm-b1023.out"
fi
# #124 NEGATIVE: a base below the local-iterate loader arity (129) is unrunnable in EVERY local
# mode. The assembler must fail LOUD rather than emit variants that can only land TOOL-ERR at load.
if python3 "$HERE/gen-variant-corpus.py" --base "$WORK/short-base.json" --donor "$WORK/donor.json" \
     --out "$WORK/corpus-short" --submit "$WORK/submit.json" --submit-sha "$SUBMIT_SHA_C" \
     --submit-bytes "$SUBMIT_BYTES_C" >"$WORK/asm-short.out" 2>&1; then
  bad "#124: assembler ACCEPTED a 64-token base (must refuse: no local mode can load it)"
else
  ok "#124: assembler refuses a sub-129-token base (no applicable local mode)"
  grep -q "need at least 129" "$WORK/asm-short.out" \
    && ok "#124: refusal names the 129-token local-iterate loader arity" \
    || { bad "#124: refusal message does not name the required arity"; sed 's/^/        /' "$WORK/asm-short.out"; }
fi
# #124: the orchestrator must refuse a short STEPS up front (before burning a GPU window).
STEPS_RC=0
SWIFT=/bin/echo BENCHCTL=/bin/echo WEIGHTS="$WORK" OUT="$WORK/corpus-steps" STEPS=64 \
  DONOR_GOLDEN="$WORK/donor.json" SUBMIT_GOLDEN="$WORK/submit.json" \
  bash "$HERE/gen-variant-corpus.sh" >"$WORK/steps-guard.out" 2>&1 || STEPS_RC=$?
[ "$STEPS_RC" -ne 0 ] && ok "#124: gen-variant-corpus.sh aborts on STEPS below the iterate arity (rc=$STEPS_RC)" \
  || bad "#124: gen-variant-corpus.sh accepted STEPS=64"
grep -q "iterate-unrunnable" "$WORK/steps-guard.out" \
  && ok "#124: STEPS guard names the iterate-unrunnable cause" \
  || { bad "#124: STEPS guard message missing"; sed 's/^/        /' "$WORK/steps-guard.out"; }
# #124 off-by-one trap: STEPS=128 is the decode WINDOW, one short of the 129-token ARITY.
# generate-golden emits EXACTLY --steps tokens, so 128 still produces an unrunnable base.
STEPS128_RC=0
SWIFT=/bin/echo BENCHCTL=/bin/echo WEIGHTS="$WORK" OUT="$WORK/corpus-steps128" STEPS=128 \
  DONOR_GOLDEN="$WORK/donor.json" SUBMIT_GOLDEN="$WORK/submit.json" \
  bash "$HERE/gen-variant-corpus.sh" >"$WORK/steps128.out" 2>&1 || STEPS128_RC=$?
[ "$STEPS128_RC" -ne 0 ] && ok "#124: STEPS=128 (the window, not the 129 arity) is ALSO refused" \
  || bad "#124: STEPS=128 accepted — off-by-one: generate-golden emits exactly --steps tokens"
# #124 F2 ACCEPT side: STEPS=129 must CLEAR the guard. Without this, an `-le`/off-by-one typo in
# the comparison would reject the only correct value and every negative above would still pass.
# The GPU step is dried out via BASE_GOLDEN (the guard runs BEFORE that branch), so this is offline.
STEPS129_RC=0
SWIFT=/bin/echo BENCHCTL=/bin/echo WEIGHTS="$WORK" OUT="$WORK/corpus-steps129" STEPS=129 \
  BASE_GOLDEN="$WORK/base.json" DONOR_GOLDEN="$WORK/donor.json" SUBMIT_GOLDEN="$WORK/submit.json" \
  SUBMIT_SHA="$SUBMIT_SHA_C" SUBMIT_BYTES="$SUBMIT_BYTES_C" \
  bash "$HERE/gen-variant-corpus.sh" >"$WORK/steps129.out" 2>&1 || STEPS129_RC=$?
grep -q "below the local-iterate loader arity" "$WORK/steps129.out" \
  && bad "#124: STEPS=129 was REJECTED by the guard (off-by-one in the comparison)" \
  || ok "#124: STEPS=129 clears the guard (accept side of the boundary)"
# #124 F3: a non-numeric STEPS must fail CLOSED, not fall through `[ abc -lt 129 ]` (rc=2 → false).
STEPSBAD_RC=0
SWIFT=/bin/echo BENCHCTL=/bin/echo WEIGHTS="$WORK" OUT="$WORK/corpus-stepsbad" STEPS=abc \
  BASE_GOLDEN="$WORK/base.json" DONOR_GOLDEN="$WORK/donor.json" SUBMIT_GOLDEN="$WORK/submit.json" \
  bash "$HERE/gen-variant-corpus.sh" >"$WORK/stepsbad.out" 2>&1 || STEPSBAD_RC=$?
{ [ "$STEPSBAD_RC" -ne 0 ] && grep -q "not a non-negative integer" "$WORK/stepsbad.out"; } \
  && ok "#124: non-numeric STEPS fails CLOSED (rc=$STEPSBAD_RC)" \
  || { bad "#124: non-numeric STEPS fell through the guard"; sed 's/^/        /' "$WORK/stepsbad.out"; }
echo ""

echo "== 2. variant-lib unit tests =="
variant_pin_check "$WORK/submit.json" "$SUBMIT_SHA_C" "$SUBMIT_BYTES_C" >/dev/null 2>&1 && ok "pin_check accepts a matching pin" || bad "pin_check rejected a matching pin"
variant_pin_check "$WORK/submit.json" "deadbeef" "$SUBMIT_BYTES_C" >/dev/null 2>&1 && bad "pin_check accepted a wrong sha" || ok "pin_check rejects a wrong sha"
variant_pin_check "$WORK/submit.json" "$SUBMIT_SHA_C" "999" >/dev/null 2>&1 && bad "pin_check accepted a wrong byte count" || ok "pin_check rejects a wrong byte count"
variant_pin_check "$WORK/nope.json" "$SUBMIT_SHA_C" "1" >/dev/null 2>&1; [ $? -eq 2 ] && ok "pin_check returns 2 for a missing file" || bad "pin_check wrong rc for missing file"
[ "$(variant_render_verdict PASS '#7')" = "PASS" ] && ok "render: PASS passes through even when declared" || bad "render: PASS mangled"
[ "$(variant_render_verdict FAIL '')" = "FAIL" ] && ok "render: undeclared FAIL stays FAIL" || bad "render: undeclared FAIL changed"
[ "$(variant_render_verdict FAIL '#74')" = "DECLARED(#74)" ] && ok "render: declared FAIL → DECLARED(#74)" || bad "render: declared FAIL not rewritten"
[ "$(variant_render_verdict 'TOOL-ERR (x)' '#74')" = "TOOL-ERR (x)" ] && ok "render: TOOL-ERR passes through (not a divergence)" || bad "render: TOOL-ERR wrongly rewritten"
# manifest_rows fail-loud on empty variants
echo '{"variants":[]}' > "$WORK/empty.json"
variant_manifest_rows "$WORK/empty.json" >/dev/null 2>&1 && bad "manifest_rows accepted an empty manifest" || ok "manifest_rows fails loud on empty variants"
ROWN="$(variant_manifest_rows "$MANIFEST" | grep -c .)"
[ "$ROWN" = "5" ] && ok "manifest_rows emits 5 rows (4 generated + reused submit)" || bad "manifest_rows emitted $ROWN rows (want 5)"

# dual_loader with stubs (validate-golden + preflight exit codes are configurable)
STUB_BC="$WORK/bc.sh"; cat > "$STUB_BC" <<'EOF'
#!/bin/bash
case "$1" in
  validate-golden) exit "${STUB_VG_RC:-0}";;
  parity-diff)
    case "$2" in
      --version) echo "parity-diff vSTUB roster0/0000"; exit 0;;
      --emit-sample) echo '{"passed":true,"score":2.88,"metrics":{}}'; exit 0;;
    esac
    # compare two JSON score files on the "score" field; STUB_DIFF_FORCE=fail forces FAIL.
    a="$2"; b="$3"
    if [ "${STUB_DIFF_FORCE:-}" = "fail" ]; then echo "PARITY: FAIL"; exit 1; fi
    va="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('score'))" "$a" 2>/dev/null)"
    vb="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('score'))" "$b" 2>/dev/null)"
    if [ "$va" = "$vb" ]; then echo "PARITY: PASS"; exit 0; else echo "PARITY: FAIL"; exit 1; fi
    ;;
  iterate)
    sp=""; while [ $# -gt 0 ]; do case "$1" in --score-path) sp="$2"; shift 2;; *) shift;; esac; done
    [ -n "$sp" ] && printf '{"passed":true,"score":%s,"metrics":{"runtime":"rust-stub"}}' "${STUB_BC_SCORE:-2.88}" > "$sp"
    printf '{"passed":true,"score":%s,"metrics":{}}' "${STUB_BC_SCORE:-2.88}"
    exit "${STUB_ITER_RC:-0}";;
esac
exit 0
EOF
chmod +x "$STUB_BC"
STUB_SW="$WORK/sw.sh"; cat > "$STUB_SW" <<'EOF'
#!/bin/bash
case "$1" in
  preflight) exit "${STUB_PF_RC:-0}";;
  benchmark)
    sp=""; while [ $# -gt 0 ]; do case "$1" in --score-path) sp="$2"; shift 2;; *) shift;; esac; done
    [ -n "$sp" ] && printf '{"passed":true,"score":%s,"metrics":{"runtime":"swift-stub"}}' "${STUB_SW_SCORE:-2.88}" > "$sp"
    exit "${STUB_BENCH_RC:-0}";;
esac
exit 0
EOF
chmod +x "$STUB_SW"
STUB_VG_RC=0 STUB_PF_RC=0 variant_dual_loader "$STUB_BC" "$STUB_SW" "$WORK/weights" "$WORK/submit.json" "$SUBMIT_SHA_C" "$SUBMIT_BYTES_C" 0 >/dev/null 2>&1 && ok "dual_loader: both ACCEPT → 0" || bad "dual_loader: accept case failed"
STUB_VG_RC=1 variant_dual_loader "$STUB_BC" "$STUB_SW" "$WORK/weights" "$WORK/submit.json" "$SUBMIT_SHA_C" "$SUBMIT_BYTES_C" 0 >/dev/null 2>&1; [ $? -eq 1 ] && ok "dual_loader: benchctl REJECT → 1" || bad "dual_loader: reject not surfaced"
STUB_VG_RC=2 variant_dual_loader "$STUB_BC" "$STUB_SW" "$WORK/weights" "$WORK/submit.json" "$SUBMIT_SHA_C" "$SUBMIT_BYTES_C" 0 >/dev/null 2>&1; [ $? -eq 3 ] && ok "dual_loader: benchctl exit 2 → HARNESS-ERR (3)" || bad "dual_loader: harness-err not surfaced"
echo ""

echo "== 3. variant-parity end-to-end (stubs) =="
# all-PASS: identical stub scores → parity PASS every cell → RESULT PASS, one row per variant.
if ENGINE=/usr/bin/true SWIFT="$STUB_SW" BENCHCTL="$STUB_BC" WEIGHTS="$WORK/weights" \
     MANIFEST="$MANIFEST" OUT="$WORK/vp" DIFF_CMD="$STUB_BC parity-diff" GPU="" \
     bash "$HERE/variant-parity.sh" >"$WORK/vp.out" 2>"$WORK/vp.err"; then
  ok "variant-parity RESULT PASS with all-PASS stubs"
else bad "variant-parity did not PASS with all-PASS stubs"; sed 's/^/        /' "$WORK/vp.err"; fi
RN="$(grep -cE '^\S+ +\| ' "$WORK/vp.out" | tr -d ' ')"
grep -q "5 variants" "$WORK/vp.out" && ok "variant-parity reports 5 variants" || bad "variant-parity variant count wrong"
# Applicable-modes (FIX 1): with MODES defaulting to both, the 4 iterate-scale shape variants must
# render local-submit = N/A (declared skip, NOT FAIL/TOOL-ERR) while still RESULT PASS; submit-1024
# (1024-step) runs BOTH modes. 4 shape variants × 1 submit N/A each = 4 N/A cells.
if awk -F' *\\| *' '$1=="minimal"{print $3}' "$WORK/vp.out" | grep -q "N/A"; then
  ok "FIX1: iterate-scale shape variant (minimal) → local-submit N/A (not TOOL-ERR/FAIL)"
else bad "FIX1: shape variant did not render local-submit N/A"; sed 's/^/        /' "$WORK/vp.out"; fi
if awk -F' *\\| *' '$1=="submit-1024"{print $3}' "$WORK/vp.out" | grep -qv "N/A"; then
  ok "FIX1: submit-1024 (1024-step) runs local-submit (applicable, not N/A)"
else bad "FIX1: submit-1024 wrongly rendered submit N/A"; fi
grep -q "N/A cells (inapplicable mode; declared, non-FAIL) = 4" "$WORK/vp.out" && ok "FIX1: exactly 4 N/A cells across the shape variants" || { bad "FIX1: N/A cell count wrong"; grep "N/A cells" "$WORK/vp.out" | sed 's/^/        /'; }
# forced-FAIL: every cell FAILs → RESULT FAIL, but behavior-bearing must render DECLARED, not FAIL.
ENGINE=/usr/bin/true SWIFT="$STUB_SW" BENCHCTL="$STUB_BC" WEIGHTS="$WORK/weights" \
  MANIFEST="$MANIFEST" OUT="$WORK/vpf" DIFF_CMD="$STUB_BC parity-diff" GPU="" STUB_DIFF_FORCE=fail \
  bash "$HERE/variant-parity.sh" >"$WORK/vpf.out" 2>"$WORK/vpf.err"; VPF_RC=$?
[ "$VPF_RC" -ne 0 ] && ok "variant-parity RESULT FAIL under forced divergence" || bad "variant-parity wrongly PASSed under forced divergence"
if awk -F' *\\| *' '$1=="behavior-bearing"{print $2 $3}' "$WORK/vpf.out" | grep -q "DECLARED"; then
  ok "behavior-bearing renders DECLARED under forced FAIL (never bare FAIL)"
else bad "behavior-bearing did not render DECLARED under forced FAIL"; fi
if awk -F' *\\| *' '$1=="minimal"{print $2}' "$WORK/vpf.out" | grep -q "FAIL"; then
  ok "undeclared variant (minimal) renders bare FAIL (act-on-this)"
else bad "minimal did not render FAIL under forced divergence"; fi
echo ""

echo "== 4. facade-leg end-to-end (stub reference + stub benchctl) =="
# The stub reference emulates the REAL benchmark.sh's new invocation contract (FIX 2): it is run
# from $REF_ROOT (dirname of the reference path) and HONORS the absolute MLXFAST_SCORE_PATH /
# MLXFAST_INTEGRITY_PATH facade-leg hands it (writing outputs into $refd), and it CAPTURES its
# invocation (cwd + key env) so the test can assert facade-leg invokes it correctly. Both sides
# write a RICH integrity JSON with the deterministic fields facade-leg value-compares (MINOR-2),
# golden_path "[private]" as the real benchctl + reference both do. Negative-control env knobs:
#   REF_GSHA / BC_GSHA   golden_sha256 written by each side (default equal → GREEN)
#   BC_DIFF_MODE=silent  parity-diff exits 0 with NO PARITY line (MINOR-1 negative control)
#   REF_CAPTURE          abs path; the stub writes cwd+env there (FIX 2 invocation assertion)
mkdir -p "$WORK/refroot/.build/release"; : > "$WORK/refroot/.build/release/mlx.metallib"
STUB_REF="$WORK/refroot/benchmark.sh"; cat > "$STUB_REF" <<'EOF'
#!/bin/bash
if [ -n "${REF_CAPTURE:-}" ]; then
  { echo "cwd=$(pwd)"; echo "skip=${MLXFAST_SKIP_TRANSFORM:-unset}"; echo "score=${MLXFAST_SCORE_PATH:-unset}"; \
    echo "integ=${MLXFAST_INTEGRITY_PATH:-unset}"; echo "metallib=${MLXFAST_MLX_METALLIB:-unset}"; \
    echo "weights=${MLXFAST_WEIGHTS_PATH:-unset}"; echo "cool=${MLXFAST_LOCAL_COOL_GATE:-unset}"; } > "$REF_CAPTURE"
fi
s="${MLXFAST_SCORE_PATH:?stub-ref needs MLXFAST_SCORE_PATH}"; i="${MLXFAST_INTEGRITY_PATH:?stub-ref needs MLXFAST_INTEGRITY_PATH}"
printf '{"passed":true,"score":%s,"metrics":{"runtime":"swift-ref-stub"}}' "${REF_SCORE:-2.88}" > "$s"
shasum -a 256 "$s" | awk '{print $1}' > "$s.sha256"
printf '{"score_path":"%s","golden_sha256":"%s","weights_sha256":"w1","weights_file_count":14,"weights_byte_count":100,"golden_path":"[private]","transform_source_sha256":"%s","score_sha256":"%s"}' \
  "$s" "${REF_GSHA:-abc}" "${REF_TSS:-t1}" "$(shasum -a 256 "$s" | awk '{print $1}')" > "$i"
exit "${REF_RC:-0}"
EOF
chmod +x "$STUB_REF"
STUB_BC2="$WORK/bc2.sh"; cat > "$STUB_BC2" <<'EOF'
#!/bin/bash
case "$1" in
  parity-diff)
    case "$2" in --version) echo "parity-diff vSTUB"; exit 0;; --emit-sample) echo '{"passed":true,"score":2.88,"metrics":{}}'; exit 0;; esac
    [ "${BC_DIFF_MODE:-}" = "silent" ] && exit 0             # exit 0, NO PARITY line (MINOR-1)
    # Emit the REAL benchctl verdict SHAPE — a descriptive suffix after PASS/FAIL — so the test
    # exercises facade-leg's prefix match (bug 1: `PARITY: PASS (no deterministic/ranking mismatch)`).
    [ "${BC_DIFF_FORCE:-}" = "fail" ] && { echo "PARITY: FAIL (deterministic mismatch)"; exit 1; }
    echo "PARITY: PASS (no deterministic/ranking mismatch)"; exit 0;;
  iterate)
    sp="score.json"; while [ $# -gt 0 ]; do case "$1" in --score-path) sp="$2"; shift 2;; *) shift;; esac; done
    printf '{"passed":true,"score":%s,"metrics":{"runtime":"rust-fac-stub"}}' "${BC_SCORE:-2.88}" > "$sp"
    shasum -a 256 "$sp" | awk '{print $1}' > "$sp.sha256"
    case "$(basename "$sp")" in score.local-iterate.json) i="benchmark-integrity.local-iterate.json";; *) i="benchmark-integrity.json";; esac
    # #123: the FACADE side emits the 9 reference fields PLUS the runner-identity roster, as real
    # benchd does — otherwise facade-leg.sh's surplus check only ever exercises its empty-surplus
    # path. BC_RUNNER_KEYS=drop reproduces a benchd that LOST the block (the regression #123 exists
    # to prevent), so the negative control can prove the check FAILS on it.
    if [ "${BC_RUNNER_KEYS:-}" = "drop" ]; then runner=""; else
      runner=',"candidate_executable":"/stub/engine","candidate_executable_sha256":"stubc","baseline_executable":"","baseline_executable_sha256":"","candidate_executable_resolution":"canonical","benchd_executable":"/stub/benchctl","benchd_executable_sha256":"stubb","candidate_workspace_sha256":""'
    fi
    printf '{"score_path":"%s","golden_sha256":"%s","weights_sha256":"w1","weights_file_count":14,"weights_byte_count":100,"golden_path":"[private]","transform_source_sha256":"%s","score_sha256":"%s"%s}' \
      "$(basename "$sp")" "${BC_GSHA:-abc}" "${BC_TSS:-t1}" "$(shasum -a 256 "$sp" | awk '{print $1}')" "$runner" > "$(dirname "$sp")/$i"
    printf '{"passed":true,"score":%s,"metrics":{}}' "${BC_SCORE:-2.88}"
    exit "${BC_ITER_RC:-0}";;
esac
exit 0
EOF
chmod +x "$STUB_BC2"
printf '{}' > "$WORK/vg.json"
run_facade_leg() {  # $1=OUT-subdir  rest=env prefix passed inline by caller
  FACADE="$HERE/benchmark.sh" REFERENCE_BENCHMARK_SH="$STUB_REF" MLXFAST_ENGINE_BIN=/usr/bin/true \
    BENCHCTL="$STUB_BC2" WEIGHTS="$WORK/weights" GOLDEN_ITERATE="$WORK/vg.json" GOLDEN_SUBMIT="$WORK/vg.json" \
    OUT="$WORK/$1" "${@:2}"
}
# GREEN: reference + facade agree on naming/sidecars/exit/integrity-values; det-fields PASS. The
# reference's integrity score_path is ABSOLUTE ($refd/…) and benchctl's is a relative basename, and
# parity-diff emits `PARITY: PASS (…suffix…)` — so this GREEN exercises bug-2 (score_path basename)
# and bug-1 (PASS-prefix) at once.
if MODES="local-iterate local-submit" run_facade_leg fl bash "$HERE/facade-leg.sh" >"$WORK/fl.out" 2>"$WORK/fl.err"; then
  ok "facade-leg GREEN with agreeing stubs (score_path abs-vs-rel + PASS suffix)"
else bad "facade-leg not GREEN with agreeing stubs"; sed 's/^/        /' "$WORK/fl.out" "$WORK/fl.err"; fi
grep -qE '\| +GREEN' "$WORK/fl.out" && ok "facade-leg table shows GREEN rows" || bad "facade-leg table missing GREEN"
# BUG 1: the det-fields cell must render PASS (parity-diff emitted `PASS (no deterministic/ranking
# mismatch)`), NOT a false TOOL-ERR from an exact `= "PASS"` test on the suffixed verdict.
if awk -F' *\\| *' '$1=="local-iterate"{print $5}' "$WORK/fl.out" | grep -qx "PASS"; then
  ok "BUG1: suffixed 'PARITY: PASS (…)' → det-fields PASS (prefix match, not TOOL-ERR)"
else bad "BUG1: suffixed PASS mis-parsed"; awk -F' *\\| *' '$1=="local-iterate"' "$WORK/fl.out" | sed 's/^/        /'; fi
# BUG 3: transform_source_sha256 diverges (reference has a hash, benchctl marker-less → "") but the
# row must stay GREEN — it is the DECLARED §3 caveat, excepted like score_sha256.
if MODES="local-iterate" REF_TSS=25d111f9deadbeef BC_TSS="" run_facade_leg fltss bash "$HERE/facade-leg.sh" >"$WORK/fltss.out" 2>"$WORK/fltss.err"; then
  ok "BUG3: transform_source_sha256 divergence is EXCEPTED (§3 caveat) → still GREEN"
else bad "BUG3: transform_source divergence wrongly FAILed the row"; sed 's/^/        /' "$WORK/fltss.out" "$WORK/fltss.err"; fi
grep -q "transform_source_sha256 differs" "$WORK/fltss.err" && ok "BUG3: the §3 divergence is REPORTED (for the record) though excepted" || bad "BUG3: §3 divergence not reported"
# BUG 1 negative: a real HARD-FAIL diff (`PARITY: FAIL (…)`, exit 1) → det-fields FAIL + row FAIL.
MODES="local-iterate" BC_DIFF_FORCE=fail run_facade_leg flfail bash "$HERE/facade-leg.sh" >"$WORK/flfail.out" 2>"$WORK/flfail.err"; FLF_RC=$?
if [ "$FLF_RC" -ne 0 ] && awk -F' *\\| *' '$1=="local-iterate"{print $5}' "$WORK/flfail.out" | grep -qx "FAIL"; then
  ok "BUG1-neg: real HARD-FAIL diff → det-fields FAIL (differ still catches divergence)"
else bad "BUG1-neg: hard-fail diff not surfaced as FAIL"; sed 's/^/        /' "$WORK/flfail.out"; fi
# NEGATIVE (exit skew): reference exits non-zero → facade-leg FAIL.
MODES="local-iterate" REF_RC=3 run_facade_leg fl2 bash "$HERE/facade-leg.sh" >"$WORK/fl2.out" 2>"$WORK/fl2.err"; FL2_RC=$?
[ "$FL2_RC" -ne 0 ] && ok "facade-leg FAILs on an exit-code skew" || bad "facade-leg missed an exit-code skew"
# NEGATIVE (MINOR-1): parity-diff exits 0 with NO PARITY line → det-field must be TOOL-ERR, not PASS.
MODES="local-iterate" BC_DIFF_MODE=silent run_facade_leg fl3 bash "$HERE/facade-leg.sh" >"$WORK/fl3.out" 2>"$WORK/fl3.err"; FL3_RC=$?
if [ "$FL3_RC" -ne 0 ] && awk -F' *\\| *' '$1=="local-iterate"{print $5}' "$WORK/fl3.out" | grep -q "TOOL-ERR"; then
  ok "MINOR-1: parity-diff exit 0 w/o PARITY line → det-field TOOL-ERR (not silent PASS)"
else bad "MINOR-1: silent parity-diff exit 0 was accepted as PASS"; sed 's/^/        /' "$WORK/fl3.out"; fi
# NEGATIVE (MINOR-2): integrity golden_sha256 diverges → integrity must FAIL (value-compare bites).
MODES="local-iterate" BC_GSHA=zzz run_facade_leg fl4 bash "$HERE/facade-leg.sh" >"$WORK/fl4.out" 2>"$WORK/fl4.err"; FL4_RC=$?
if [ "$FL4_RC" -ne 0 ] && awk -F' *\\| *' '$1=="local-iterate"{print $4}' "$WORK/fl4.out" | grep -q "FAIL"; then
  ok "MINOR-2: integrity golden_sha256 divergence → integrity FAIL (value-compare, not key-set-only)"
else bad "MINOR-2: integrity value divergence slipped through key-set-only check"; sed 's/^/        /' "$WORK/fl4.out"; fi
# NEGATIVE (#123): benchd DROPS the runner-identity block → integrity must FAIL. This is the
# regression #123 exists to prevent, and without it the surplus check would only ever run its
# empty-surplus path — the check would be present but never enforcing. Positive coverage is the
# default stub above, which emits all 7 keys, so both branches are exercised in this file.
MODES="local-iterate" BC_RUNNER_KEYS=drop run_facade_leg fl5 bash "$HERE/facade-leg.sh" >"$WORK/fl5.out" 2>"$WORK/fl5.err"; FL5_RC=$?
if [ "$FL5_RC" -ne 0 ] \
   && grep -q 'not EXACTLY the declared #123 runner roster' "$WORK/fl5.err" \
   && awk -F' *\\| *' '$1=="local-iterate"{print $4}' "$WORK/fl5.out" | grep -q "FAIL"; then
  ok "#123: missing runner-identity block → integrity FAIL (surplus check enforces, not just tolerates)"
else bad "#123: a sidecar with NO runner-identity fields was accepted"; sed 's/^/        /' "$WORK/fl5.err"; fi
# FIX 2: assert facade-leg invokes the reference from $REF_ROOT with the right env (cwd == REF_ROOT,
# MLXFAST_SKIP_TRANSFORM=1, absolute MLXFAST_SCORE_PATH inside the run dir, MLX_METALLIB set, cool
# gate off for iterate). The stub reference records its invocation to REF_CAPTURE.
# Resolve REF_ROOT the SAME way facade-leg does (`cd -P … && pwd -P`) so a /var→/private/var
# symlink on macOS doesn't spuriously fail the cwd assertion.
REF_ROOT_EXP="$(cd -P "$WORK/refroot" && pwd -P)"; CAP="$WORK/refcap.txt"
MODES="local-iterate" REF_CAPTURE="$CAP" run_facade_leg flcap bash "$HERE/facade-leg.sh" >"$WORK/flcap.out" 2>"$WORK/flcap.err"
if [ -f "$CAP" ]; then
  cap_cwd="$(sed -n 's/^cwd=//p' "$CAP")"; cap_skip="$(sed -n 's/^skip=//p' "$CAP")"
  cap_score="$(sed -n 's/^score=//p' "$CAP")"; cap_metallib="$(sed -n 's/^metallib=//p' "$CAP")"; cap_cool="$(sed -n 's/^cool=//p' "$CAP")"
  [ "$cap_cwd" = "$REF_ROOT_EXP" ] && ok "FIX2: reference invoked from \$REF_ROOT ($cap_cwd)" || bad "FIX2: reference cwd wrong (got $cap_cwd, want $REF_ROOT_EXP)"
  [ "$cap_skip" = "1" ] && ok "FIX2: MLXFAST_SKIP_TRANSFORM=1 set for the reference" || bad "FIX2: SKIP_TRANSFORM not set (got '$cap_skip')"
  case "$cap_score" in "$WORK/flcap/"*) ok "FIX2: MLXFAST_SCORE_PATH is ABSOLUTE inside the run dir ($cap_score)";; *) bad "FIX2: score path not redirected into run dir (got $cap_score)";; esac
  [ -n "$cap_metallib" ] && [ "$cap_metallib" != "unset" ] && ok "FIX2: MLXFAST_MLX_METALLIB set ($cap_metallib)" || bad "FIX2: MLX_METALLIB not set"
  [ "$cap_cool" = "0" ] && ok "FIX2: cool-gate forced OFF for local-iterate (symmetric)" || bad "FIX2: cool-gate env wrong for iterate (got '$cap_cool')"
else bad "FIX2: reference capture file not written — reference not invoked?"; fi
# BUG 4 (COMPARE_ONLY): re-compare the GREEN run's captured artifacts WITHOUT re-running, with the
# minimal env (BENCHCTL + OUT + MODES; no ENGINE_BIN/WEIGHTS/reference). Must re-GREEN and NOT wipe.
CO_OUT="$WORK/fl"   # the GREEN run's artifact dir (fl.local-iterate etc. + exit_code files)
if COMPARE_ONLY=1 BENCHCTL="$STUB_BC2" OUT="$CO_OUT" MODES="local-iterate local-submit" \
     bash "$HERE/facade-leg.sh" >"$WORK/co.out" 2>"$WORK/co.err"; then
  ok "BUG4: COMPARE_ONLY re-compares prior artifacts → GREEN (no runs, minimal env)"
else bad "BUG4: COMPARE_ONLY re-compare did not PASS"; sed 's/^/        /' "$WORK/co.out" "$WORK/co.err"; fi
grep -q "COMPARE_ONLY — re-compared prior-window artifacts, no runs" "$WORK/co.out" && ok "BUG4: RESULT line marks COMPARE_ONLY" || bad "BUG4: COMPARE_ONLY not marked in result"
[ -f "$CO_OUT/ref.local-iterate/score.local-iterate.json" ] && [ -f "$CO_OUT/fac.local-iterate/score.local-iterate.json" ] && ok "BUG4: prior artifacts preserved (not wiped)" || bad "BUG4: prior artifacts were wiped by COMPARE_ONLY"
# BUG 4 fail-loud: COMPARE_ONLY on a directory with NO artifacts must FAIL (never PASS on missing).
COMPARE_ONLY=1 BENCHCTL="$STUB_BC2" OUT="$WORK/co-empty" MODES="local-iterate" \
  bash "$HERE/facade-leg.sh" >"$WORK/coe.out" 2>"$WORK/coe.err"; COE_RC=$?
[ "$COE_RC" -ne 0 ] && grep -q "NO-ARTIFACTS" "$WORK/coe.out" && ok "BUG4: COMPARE_ONLY fails LOUD on missing artifacts (never PASS)" || bad "BUG4: COMPARE_ONLY did not fail on missing artifacts"
echo ""

echo "== 5. MINOR-3 timing-waiver + det-diff via REAL benchctl parity-diff (engine-free) =="
REAL_BC="$HERE/../target/release/benchctl"
if [ -x "$REAL_BC" ]; then
  d="$WORK/waiver"; mkdir -p "$d"
  "$REAL_BC" parity-diff --emit-sample > "$d/a.json" 2>/dev/null || bad "emit-sample failed"
  python3 -c "import json;x=json.load(open('$d/a.json'));x['metrics']['case_count']=x['metrics'].get('case_count',0)+1;json.dump(x,open('$d/b.json','w'))"
  python3 -c "import json;x=json.load(open('$d/a.json'));x['metrics']['benchmark_wall_seconds']=float(x['metrics'].get('benchmark_wall_seconds',0.0))+999.0;json.dump(x,open('$d/c.json','w'))"
  "$REAL_BC" parity-diff "$d/a.json" "$d/c.json" >/dev/null 2>&1; s4=$?
  "$REAL_BC" parity-diff "$d/a.json" "$d/b.json" >/dev/null 2>&1; s2=$?
  [ "$s4" = 0 ] && ok "S4: timing-only diff (benchmark_wall_seconds) → PASS (bucket policy waives timing)" || bad "S4: timing-only diff did NOT PASS (exit $s4) — waiver regressed"
  [ "$s2" = 1 ] && ok "det-diff (case_count) → FAIL (control: differ still catches deterministic drift)" || bad "det-diff did not FAIL (exit $s2)"
else
  echo "  SKIP  real benchctl not built at $REAL_BC (window self-test S4 covers this on the box)"
fi
echo ""

echo "== summary =="
echo "test-variant-offline: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] && { echo "RESULT PASS — offline control flow proven fail-loud (GPU steps NOT run)"; exit 0; } || { echo "RESULT FAIL — $FAIL check(s) failed"; exit 1; }
