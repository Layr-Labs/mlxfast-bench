#!/bin/bash
# scripts/test-official-offline.sh — OFFLINE self-test for the B-3 official-window scripts.
#
# Proves the NON-GPU control flow of official-lib / official-parity / official-failure-map /
# official-env-probe fails LOUD correctly, WITHOUT a GPU and WITHOUT faking a GPU run: canned
# goldens + a stub benchctl / stub swift / stub probe-benchctl (the compat-matrix / variant-offline
# stub pattern). What it verifies:
#   0. `bash -n` every new script.
#   1. official-lib: seatbelt-profile text shape, seal (valid/empty/multi/non-json), sidecars
#      (9-field + .sha256), render_verdict (DECLARED passthrough), diff_cell, commit_sha40.
#   2. official-parity end-to-end (stubs): agreeing → RESULT PASS + one row per pair; forced
#      det-diff → FAIL; a swift side that emits NO payload (seal fails) → FAIL (never silent pass).
#   3. official-failure-map (stubs): oracle fails BOTH → assertion OK; a stubbed oracle-PASS on one
#      side → ABORT (harness-failure guard, exit 5); a golden with no oracle → refuse (exit 6);
#      declared class renders DECLARED(#nn).
#   4. official-env-probe: good capture → PASS; leaked MLXFAST_*/random → FAIL; NO capture → TOOL-ERR
#      abort (exit 4). Uses a stub benchctl that runs the env-dump shim under a chosen env.
#
# The REAL GPU steps (live benchctl official + direct swift official; the live sandboxed spawn) are
# NOT run here — those are the window's job. Exit 0 = all green.
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/official-offline.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  PASS  %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; }
# RULING C — 0 iff $1 is a workspace-RELATIVE, leak-free provenance path: no leading `/`, no
# `/Users/`, no `/home/` (and not empty). This is the structural property official-parity Leg 3c
# enforces on score_path/weights_path; asserting it on BOTH impls proves basename-compare cannot
# diverge and neither side seals a home dir — without pinning a fragile Rust↔shell byte-equivalence.
struct_relative_leakfree() {
  case "$1" in ""|/*) return 1 ;; esac
  case "$1" in *"/Users/"*|*"/home/"*) return 1 ;; esac
  return 0
}

# shellcheck source=scripts/official-lib.sh
. "$HERE/official-lib.sh"
HAVE_JQ=1; command -v jq >/dev/null 2>&1 || HAVE_JQ=0

echo "== 0. syntax checks =="
for s in official-lib.sh official-parity.sh official-failure-map.sh official-env-probe.sh run-official-window.sh assemble-official-golden.sh test-official-offline.sh; do
  if bash -n "$HERE/$s" 2>"$WORK/synerr"; then ok "bash -n $s"; else bad "bash -n $s"; sed 's/^/        /' "$WORK/synerr"; fi
done
echo ""

echo "== 1. official-lib primitives =="
# 1a. seatbelt profile text shape (port of build_seatbelt_profile).
official_seatbelt_profile "/x/swift" "/priv/golden.json" > "$WORK/p.sb"
grep -q '^(version 1)$' "$WORK/p.sb" && grep -q '^(allow default)$' "$WORK/p.sb" \
  && grep -q '(allow process-exec (literal "/x/swift"))' "$WORK/p.sb" \
  && grep -q '(allow file-write\* (literal "/dev/null"))' "$WORK/p.sb" \
  && grep -q '(deny file-read\* (literal "/priv/golden.json"))' "$WORK/p.sb" \
  && ok "seatbelt profile has the exact rule shape" || { bad "seatbelt profile shape wrong"; sed 's/^/        /' "$WORK/p.sb"; }
official_seatbelt_profile "/x/e" "/g.json" "/private/dir" > "$WORK/p2.sb"
grep -q '(deny file-read\* (subpath "/private/dir"))' "$WORK/p2.sb" \
  && grep -q '(deny file-write\* (subpath "/private/dir"))' "$WORK/p2.sb" \
  && ok "seatbelt profile appends private-dir subpath denials" || bad "private-dir subpath denials missing"

# 1b. seal: valid / empty / multi-object / non-json.
if [ "$HAVE_JQ" = 1 ]; then
  printf '{"passed":true,"score":2.88,"metrics":{}}' > "$WORK/raw.ok"
  official_seal_stdout "$WORK/raw.ok" "$WORK/sealed.json" >/dev/null 2>&1 && [ -s "$WORK/sealed.json" ] && ok "seal: single valid payload → sealed" || bad "seal: valid payload rejected"
  : > "$WORK/raw.empty"
  official_seal_stdout "$WORK/raw.empty" "$WORK/s2.json" >/dev/null 2>&1 && bad "seal: empty stdout accepted (should fail)" || ok "seal: empty stdout → FAIL"
  printf '{"passed":true,"score":1,"metrics":{}}{"passed":true,"score":2,"metrics":{}}' > "$WORK/raw.multi"
  official_seal_stdout "$WORK/raw.multi" "$WORK/s3.json" >/dev/null 2>&1 && bad "seal: multi-object accepted (should fail)" || ok "seal: multiple concatenated objects → FAIL"
  printf 'not json at all' > "$WORK/raw.bad"
  official_seal_stdout "$WORK/raw.bad" "$WORK/s4.json" >/dev/null 2>&1 && bad "seal: non-json accepted (should fail)" || ok "seal: non-json → FAIL"

  # 1c. sidecars: 9-field integrity + two-space .sha256.
  printf '{"passed":true,"score":2.88,"metrics":{"weights_hash":"abc","weights_file_count":14,"weights_byte_count":123}}' > "$WORK/score.json"
  printf 'golden-bytes' > "$WORK/g.json"
  mkdir -p "$WORK/wdir"
  if official_write_sidecars "$WORK/score.json" "$WORK/wdir" "$WORK/g.json" "$WORK/integ.json" >/dev/null 2>&1; then
    nkeys="$(jq -S 'keys|length' "$WORK/integ.json" 2>/dev/null)"
    [ "$nkeys" = 9 ] && ok "sidecars: integrity has 9 fields" || bad "sidecars: integrity field count = $nkeys (want 9)"
    [ "$(jq -r '.golden_path' "$WORK/integ.json")" = "[private]" ] && ok "sidecars: golden_path is [private]" || bad "sidecars: golden_path wrong"
    grep -q "  $WORK/score.json\$" "$WORK/score.json.sha256" && ok "sidecars: .sha256 is the two-space '<hex>  <path>' form" || bad "sidecars: .sha256 form wrong"
  else bad "sidecars: official_write_sidecars failed on a valid score"; fi
else
  echo "  SKIP  seal/sidecar tests (jq absent)"
fi

# 1d. render_verdict (no-undeclared-cells rule).
[ "$(official_render_verdict FAIL '#74')" = "DECLARED(#74)" ] && ok "render_verdict: FAIL + declared → DECLARED(#74)" || bad "render_verdict: declared FAIL not rewritten"
[ "$(official_render_verdict PASS '#74')" = "PASS" ] && ok "render_verdict: PASS passes through" || bad "render_verdict: PASS rewritten"
[ "$(official_render_verdict FAIL '')" = "FAIL" ] && ok "render_verdict: undeclared FAIL survives" || bad "render_verdict: undeclared FAIL lost"

# 1e. diff_cell.
printf 'PARITY: PASS (no deterministic/ranking mismatch)\n' > "$WORK/d.pass"
printf 'PARITY: FAIL (case_count)\n' > "$WORK/d.fail"
printf 'some noise, no verdict line\n' > "$WORK/d.none"
[ "$(official_diff_cell 0 "$WORK/d.pass")" = "PASS" ] && ok "diff_cell: 0 + PASS line → PASS" || bad "diff_cell: PASS wrong"
[ "$(official_diff_cell 1 "$WORK/d.fail")" = "FAIL" ] && ok "diff_cell: 1 + FAIL line → FAIL" || bad "diff_cell: FAIL wrong"
case "$(official_diff_cell 0 "$WORK/d.none")" in TOOL-ERR*) ok "diff_cell: 0 w/o PARITY line → TOOL-ERR (no silent pass)";; *) bad "diff_cell: silent 0 accepted";; esac
case "$(official_diff_cell 3 "$WORK/d.none")" in TOOL-ERR*) ok "diff_cell: exit 3 → TOOL-ERR";; *) bad "diff_cell: exit 3 not TOOL-ERR";; esac

# 1f (F-5, RULING C). shell relativize_for_seal LEAK-SCAN over the exact divergence vectors the
# re-review named — the SAME set the Rust unit test feeds. EVERY home-shaped input must reduce to a
# relative, leak-free string; the bare home ROOT (/Users/<u>, /home/<u>, no trailing component) is
# the mandatory case a naive head-strip left absolute and SEALED. C requires both-relative-and-leak-
# free, NOT byte-equal. The home arm is forced with a non-matching $PWD.
leak_ok=1; leak_out=""
for v in "/Users/op/models/qwen/" "/Users/op/" "/Users/other/models/qwen/" "/Users/other" "/home/other"; do
  r="$( HOME=/Users/op; PWD=/nowhere; relativize_for_seal "$v" )"
  if ! struct_relative_leakfree "$r"; then leak_ok=0; leak_out="$leak_out [$v -> '$r']"; fi
done
# symlinked-CWD path: a path under a SYMLINKED working dir reduces via the $PWD arm to a relative
# tail (the CWD-arm counterpart of the home vectors above).
ln -s "$WORK" "$WORK/link" 2>/dev/null || true
r_sym="$( HOME=/nohome; PWD="$WORK/link"; relativize_for_seal "$WORK/link/models/qwen" )"
if ! struct_relative_leakfree "$r_sym"; then leak_ok=0; leak_out="$leak_out [symlinked-CWD -> '$r_sym']"; fi
[ "$leak_ok" = 1 ] && ok "F-5: shell relativize_for_seal reduces every home-shaped + symlinked-CWD input to a relative, leak-free string" \
  || bad "F-5: shell relativize_for_seal leaked an absolute/home path:$leak_out"

# 1f. commit_sha40.
[ "$(OFFICIAL_COMMIT=deadbeef official_commit_sha40)" = "deadbeef" ] && ok "commit_sha40: OFFICIAL_COMMIT override wins" || bad "commit_sha40: override ignored"
[ "$(OFFICIAL_COMMIT= official_commit_sha40 /no/such/repo)" = "0000000000000000000000000000000000000000" ] && ok "commit_sha40: no repo → 40-zero sentinel (never empty)" || bad "commit_sha40: sentinel wrong"

# 1f-bis. AUTHOR-AT-SEAL dispatch record (DECIDE-3): dispatch_context_sha + record_dispatch_sha.
DSHA40="0123456789abcdef0123456789abcdef01234567"
# No dispatch context ⇒ empty (rc 0): measure-job falls back to its un-bound resolution.
[ -z "$(MLXFAST_CANDIDATE_SHA= GITHUB_SHA= dispatch_context_sha)" ] && ok "dispatch_context_sha: no context → empty (local/offline)" || bad "dispatch_context_sha: non-empty without a context"
# MLXFAST_CANDIDATE_SHA is the authority (github.sha equivalent); it wins over GITHUB_SHA.
[ "$(MLXFAST_CANDIDATE_SHA=$DSHA40 GITHUB_SHA=ffffffffffffffffffffffffffffffffffffffff dispatch_context_sha)" = "$DSHA40" ] && ok "dispatch_context_sha: MLXFAST_CANDIDATE_SHA wins" || bad "dispatch_context_sha: candidate override ignored"
# GITHUB_SHA is the github.sha-equivalent fallback.
[ "$(MLXFAST_CANDIDATE_SHA= GITHUB_SHA=$DSHA40 dispatch_context_sha)" = "$DSHA40" ] && ok "dispatch_context_sha: GITHUB_SHA fallback" || bad "dispatch_context_sha: GITHUB_SHA fallback wrong"
# A malformed dispatched sha is FATAL (rc 1) — never a silent junk seal.
( MLXFAST_CANDIDATE_SHA=nothex40 dispatch_context_sha >/dev/null 2>&1 ); [ $? -eq 1 ] && ok "dispatch_context_sha: non-hex → FATAL rc1" || bad "dispatch_context_sha: non-hex not fatal"
( MLXFAST_CANDIDATE_SHA=deadbeef dispatch_context_sha >/dev/null 2>&1 ); [ $? -eq 1 ] && ok "dispatch_context_sha: short (not 40) → FATAL rc1" || bad "dispatch_context_sha: short not fatal"
# record_dispatch_sha writes candidate.sha at the pinned path and echoes it; content == the sha.
RECDIR="$WORK/rec"; rm -rf "$RECDIR"
REC="$(MLXFAST_CANDIDATE_SHA=$DSHA40 record_dispatch_sha "$RECDIR")"
{ [ "$REC" = "$RECDIR/candidate.sha" ] && [ -f "$RECDIR/candidate.sha" ] && [ "$(cat "$RECDIR/candidate.sha")" = "$DSHA40" ]; } \
  && ok "record_dispatch_sha: writes candidate.sha (benchd-readable) from the dispatched sha" || bad "record_dispatch_sha: candidate.sha wrong/missing"
# No context ⇒ no file written, empty echo (rc 0).
RECDIR2="$WORK/rec2"; rm -rf "$RECDIR2"
REC2="$(MLXFAST_CANDIDATE_SHA= GITHUB_SHA= record_dispatch_sha "$RECDIR2")"
{ [ -z "$REC2" ] && [ ! -f "$RECDIR2/candidate.sha" ]; } && ok "record_dispatch_sha: no context → no record file (fallback path)" || bad "record_dispatch_sha: wrote a record without a context"

# 1g. official_replicate_artifacts (P-3): box-local replica created; never fails the run.
mkdir -p "$WORK/repl-src"; printf 'REPORT\n' > "$WORK/repl-src/REPORT.md"; printf 'x\n' > "$WORK/repl-src/scores.json"
# unset REPLICA_TARGET → box-local copy + offsite-pending log; function returns 0.
out="$(REPLICA_TARGET= official_replicate_artifacts "$WORK/repl-src" "$WORK/repl-dst"; echo "rc=$?")"
{ [ -f "$WORK/repl-dst/REPORT.md" ] && [ -f "$WORK/repl-dst/scores.json" ]; } && ok "replicate: box-local replica dir created with artifacts" || bad "replicate: box-local replica not created"
printf '%s' "$out" | grep -q 'offsite pull pending (P-3)' && ok "replicate: unset REPLICA_TARGET logs offsite-pending (P-3)" || bad "replicate: offsite-pending line missing"
printf '%s' "$out" | grep -q 'rc=0' && ok "replicate: returns 0 on the box-local path" || bad "replicate: box-local path non-zero"
# a replica ERROR (REPLICA_TARGET rsync to an impossible dest / rsync absent) must NOT fail the run.
out2="$(REPLICA_TARGET="/nonexistent-root-$$/deep/dest" official_replicate_artifacts "$WORK/repl-src" "$WORK/repl-dst2"; echo "rc=$?")"
printf '%s' "$out2" | grep -q 'rc=0' && ok "replicate: rsync error is non-fatal (returns 0)" || bad "replicate: rsync error failed the run"
# a MISSING source dir must be non-fatal too (returns 0).
out3="$(REPLICA_TARGET= official_replicate_artifacts "$WORK/no-such-src" "$WORK/repl-dst3"; echo "rc=$?")"
printf '%s' "$out3" | grep -q 'rc=0' && ok "replicate: missing source dir is non-fatal (returns 0)" || bad "replicate: missing source failed the run"
echo ""

# ---- canned goldens + stubs ------------------------------------------------------------
mk_golden() {
  python3 - "$1" "$2" <<'PY'
import json, sys
kind = sys.argv[2]
base = {"version":1,"model_type":"qwen3_5_text",
        "cases":[{"name":"c1","prompt_tokens":[1]*512,"expected_tokens":[2]*1024}]}
bench = {"prefill_prompt_tokens":[1]*512,"expected_prefill_token":5,
         "decode_seed_tokens":[1]*512,"expected_decode_seed_token":6,
         "expected_decode_tokens":[7]*128,
         "baseline_prefill_seconds_per_token":0.0106,"baseline_decode_seconds_per_token":0.1336}
doc = dict(base)
doc["correctness_gates"]={"anchors":[{"name":"a1","context_tokens":[1]*8,"expected_token":100,"accepted_tokens":[100]}],
                          "free_run":[{"name":"fr1","prompt_tokens":[1]*512,"expected_tokens":[9,9,9]}]}
if kind != "no-oracle":
    doc["benchmark"]=bench
open(sys.argv[1],"w").write(json.dumps(doc, separators=(",",":"), ensure_ascii=False))
PY
}
mk_golden "$WORK/official.json" full
mk_golden "$WORK/no-oracle.json" no-oracle
mk_golden "$WORK/submit-1024-band.json" full   # STALE-baseline band-failure fixture (name carries "band")

# stub benchctl: iterate (writes sealed score + .sha256 + 9-field integrity), parity-diff, validate-golden.
STUB_BC="$WORK/bc.sh"; cat > "$STUB_BC" <<'EOF'
#!/bin/bash
sub="$1"; shift
case "$sub" in
  --version) echo "benchctl vSTUB"; exit 0;;
  validate-golden) exit 0;;
  parity-diff)
    case "${1:-}" in
      --version) echo "parity-diff vSTUB roster0/0000"; exit 0;;
      --emit-sample) echo '{"passed":true,"score":2.88,"metrics":{}}'; exit 0;;
    esac
    if [ "${STUB_DIFF_FORCE:-}" = fail ]; then echo "PARITY: FAIL (stub forced)"; exit 1; fi
    # Simulate the real baseline-missing divergence (fail-point differs) so the DECLARED(#74)
    # render path is exercised end-to-end; every other class agrees (PASS).
    case "$*" in *baseline-missing*) echo "PARITY: FAIL (baseline-missing divergence)"; exit 1;; esac
    # Negative-control knob: force an UNDECLARED-class divergence (e.g. primary) → the leg must break.
    if [ -n "${STUB_DIFF_FAIL_CLASS:-}" ]; then case "$*" in *"$STUB_DIFF_FAIL_CLASS"*) echo "PARITY: FAIL (undeclared divergence)"; exit 1;; esac; fi
    echo "PARITY: PASS (no deterministic/ranking mismatch)"; exit 0;;
  iterate)
    golden=""; weights=""; sp=""
    while [ $# -gt 0 ]; do case "$1" in
      --golden) golden="$2"; shift 2;;
      --weights) weights="$2"; shift 2;;
      --score-path) sp="$2"; shift 2;;
      --engine|--mode|--golden-sha256|--golden-bytes) shift 2;;
      *) shift;;
    esac; done
    passed=true
    case "$golden" in *oracle*|*primary*|*anchor*|*free-run*|*baseline-missing*|*band*) passed=false;; esac
    [ "${STUB_BC_ORACLE_PASSES:-}" = 1 ] && case "$golden" in *oracle*) passed=true;; esac
    # Band-failure fixture: a STALE-baseline golden fails the acceptance band with the band signature
    # (unless STUB_BC_BAND_PASSES forces a both-PASS divergence for the negative control).
    err=""; case "$golden" in *band*) err="acceptance band failed: prefill below -5% of reference improvement too large for one submission (chunk it)";; esac
    [ "${STUB_BC_BAND_PASSES:-}" = 1 ] && case "$golden" in *band*) passed=true; err="";; esac
    gsha="$(shasum -a 256 "$golden" | awk '{print $1}')"
    sdir="$(dirname "$sp")"; mkdir -p "$sdir"
    printf '{"passed":%s,"score":2.88,"error":"%s","metrics":{"weights_hash":"stubw","weights_file_count":1,"weights_byte_count":1,"commit":"%s","runtime":"rust-official-stub"}}' "$passed" "$err" "${MLXFAST_COMMIT_SHA:-x}" > "$sp"
    ssha="$(shasum -a 256 "$sp" | awk '{print $1}')"
    printf '%s  %s\n' "$ssha" "$sp" > "$sp.sha256"
    # #123: the stub emits the 9 reference fields PLUS the runner-identity roster, exactly as real
    # benchd does. Without them official-parity.sh's surplus check would only ever exercise its
    # empty-surplus path, and the leg would stay green against a benchd that dropped the block.
    if [ "${STUB_BC_DROP_RUNNER:-}" = 1 ]; then runner=""; else
      runner=',"candidate_executable":"/stub/engine","candidate_executable_sha256":"stubc","baseline_executable":"","baseline_executable_sha256":"","candidate_executable_resolution":"canonical","benchd_executable":"/stub/benchctl","benchd_executable_sha256":"stubb","candidate_workspace_sha256":""'
    fi
    # F-5 — model the REAL post-fix benchd: seal weights_path/score_path RELATIVIZED (not raw) via
    # the SAME shell helper the reference sidecar uses, sourced from official-lib.sh. Re-implementing
    # the old raw behaviour here is exactly the stub-agreeing-with-stub gap that hid the divergence.
    . "${OFFICIAL_LIB:?stub-bc needs OFFICIAL_LIB to source relativize_for_seal}"
    sp_rel="$(relativize_for_seal "$sp")"; weights_rel="$(relativize_for_seal "$weights")"
    printf '{"score_path":"%s","score_sha256":"%s","weights_path":"%s","weights_sha256":"stubw","weights_file_count":1,"weights_byte_count":1,"golden_path":"[private]","golden_sha256":"%s","transform_source_sha256":""%s}\n' "$sp_rel" "$ssha" "$weights_rel" "$gsha" "$runner" > "$sdir/benchmark-integrity.json"
    [ "$passed" = true ] && exit 0 || exit 1;;
  *) echo "stub-bc: unknown $sub" >&2; exit 2;;
esac
EOF
chmod +x "$STUB_BC"

# stub swift: `benchmark` emits the (sealed-on-stdout) payload; corruption → passed:false.
STUB_SW="$WORK/sw.sh"; cat > "$STUB_SW" <<'EOF'
#!/bin/bash
[ "$1" = benchmark ] || { echo "stub-sw: only benchmark" >&2; exit 2; }
shift; golden=""
while [ $# -gt 0 ]; do case "$1" in --golden) golden="$2"; shift 2;; --weights|--score-path) shift 2;; *) shift;; esac; done
passed=true
case "$golden" in *oracle*|*primary*|*anchor*|*free-run*|*baseline-missing*|*band*) passed=false;; esac
[ "${STUB_SW_ORACLE_PASSES:-}" = 1 ] && case "$golden" in *oracle*) passed=true;; esac
err=""; case "$golden" in *band*) err="acceptance band failed: prefill below -5% of reference improvement too large for one submission (chunk it)";; esac
[ "${STUB_SW_BAND_PASSES:-}" = 1 ] && case "$golden" in *band*) passed=true; err="";; esac
[ "${STUB_SW_NOPAYLOAD:-}" = 1 ] && { echo ""; exit 1; }   # emit NO payload → seal must fail
# Negative-control knob: emit no payload for ONE class → a missing-score cell in the failure map.
if [ -n "${STUB_SW_NOSCORE_CLASS:-}" ]; then case "$golden" in *"$STUB_SW_NOSCORE_CLASS"*) echo ""; exit 1;; esac; fi
printf '{"passed":%s,"score":2.88,"error":"%s","metrics":{"weights_hash":"stubw","weights_file_count":1,"weights_byte_count":1,"commit":"%s","runtime":"swift-official-stub"}}' "$passed" "$err" "${MLXFAST_COMMIT_SHA:-x}"
[ "$passed" = true ] && exit 0 || exit 1
EOF
chmod +x "$STUB_SW"

mkdir -p "$WORK/wdir"

if [ "$HAVE_JQ" = 1 ]; then
  echo "== 2. official-parity end-to-end (stubs) =="
  # 2z (F-5, RULING C). The STRUCTURAL property Leg 3c relies on, exercised end-to-end: for a
  # $HOME-based weights dir (the fleet case), the reference (sw) sidecar AND the REAL post-fix benchd
  # (modelled by STUB_BC, which sources the SAME relativize_for_seal) must EACH seal a RELATIVE,
  # leak-free weights_path — NOT byte-equal (C drops the fragile Rust↔shell equivalence; the weights
  # IDENTITY is carried strictly by weights_sha256/file_count/byte_count). RED if EITHER side reverts
  # to a raw absolute path (fails struct_relative_leakfree → basename-compare could diverge and a
  # home dir leaks); GREEN when both are relative + leak-free.
  agree_home="$WORK/f5home"; agree_cwd="$WORK/f5cwd"
  mkdir -p "$agree_home/models/qwen" "$agree_cwd" "$WORK/f5-bc"
  agree_w="$agree_home/models/qwen"
  printf '{"passed":true,"score":2.88,"metrics":{"weights_hash":"abc","weights_file_count":14,"weights_byte_count":123}}' > "$WORK/f5-score.json"
  printf 'gb' > "$WORK/f5-g.json"
  ( cd "$agree_cwd" && HOME="$agree_home" official_write_sidecars "$WORK/f5-score.json" "$agree_w" "$WORK/f5-g.json" "$WORK/f5-sw.json" ) >/dev/null 2>&1
  sw_wp="$(jq -r '.weights_path // "ABSENT"' "$WORK/f5-sw.json" 2>/dev/null)"
  ( cd "$agree_cwd" && HOME="$agree_home" OFFICIAL_LIB="$HERE/official-lib.sh" MLXFAST_COMMIT_SHA=deadbeef \
      "$STUB_BC" iterate --golden "$WORK/f5-g.json" --weights "$agree_w" --score-path "$WORK/f5-bc/score.json" ) >/dev/null 2>&1
  bc_wp="$(jq -r '.weights_path // "ABSENT"' "$WORK/f5-bc/benchmark-integrity.json" 2>/dev/null)"
  if struct_relative_leakfree "$sw_wp" && struct_relative_leakfree "$bc_wp"; then
    ok "F-5: bc and sw each seal a RELATIVE, leak-free weights_path (bc=$bc_wp sw=$sw_wp) for a \$HOME-based weights dir"
  else
    bad "F-5: weights_path not relative/leak-free on a side (bc=$bc_wp sw=$sw_wp) — a raw/absolute regression leaks \$HOME and breaks Leg 3c"
  fi
  # DIFF_CMD is intentionally NOT set — official-parity defaults it to "$BENCHCTL parity-diff"
  # (== the stub's parity-diff), which avoids a space-in-value that `env <string>` would word-split.
  # HOME=$WORK so the temp WEIGHTS ($WORK/wdir) relativizes to "wdir" on BOTH sides — faithfully
  # mirroring the on-box run where weights live under the operator home, and the exact condition
  # Leg 3c's RULING-C structural-relative guard requires.
  COMMON="HOME=$WORK BENCHCTL=$STUB_BC ENGINE=/bin/echo SWIFT=$STUB_SW WEIGHTS=$WORK/wdir OFFICIAL_GOLDEN=$WORK/official.json OFFICIAL_COMMIT=deadbeefcafe OFFICIAL_LIB=$HERE/official-lib.sh"
  # 2a. agreeing → RESULT PASS, one row per pair.
  if env $COMMON OUT="$WORK/op" PAIRS=3 bash "$HERE/official-parity.sh" > "$WORK/op.out" 2> "$WORK/op.err"; then
    rows="$(awk -F' *\\| *' 'NF>=9 && $1!="pair" && $1 !~ /^-+$/' "$WORK/op/official-parity.table.txt" | wc -l | tr -d ' ')"
    [ "$rows" = 3 ] && ok "official-parity PASS with agreeing stubs (3 rows)" || bad "official-parity row count = $rows (want 3)"
    grep -q 'RESULT PASS' "$WORK/op.out" && ok "official-parity prints RESULT PASS" || bad "official-parity missing RESULT PASS"
  else bad "official-parity did not PASS with agreeing stubs"; sed 's/^/        /' "$WORK/op.out" "$WORK/op.err"; fi
  # 2b. forced det-diff → FAIL.
  if env $COMMON STUB_DIFF_FORCE=fail OUT="$WORK/opf" PAIRS=2 bash "$HERE/official-parity.sh" > "$WORK/opf.out" 2>&1; then
    bad "official-parity did NOT fail on forced det-diff"
  else grep -q 'RESULT FAIL' "$WORK/opf.out" && ok "official-parity RESULT FAIL on forced det-diff" || bad "official-parity failed without RESULT FAIL line"; fi
  # 2c. swift emits no payload → seal fails → FAIL (never a silent pass).
  if env $COMMON STUB_SW_NOPAYLOAD=1 OUT="$WORK/opn" PAIRS=1 bash "$HERE/official-parity.sh" > "$WORK/opn.out" 2>&1; then
    bad "official-parity passed despite swift emitting no payload"
  else grep -q 'RESULT FAIL' "$WORK/opn.out" && ok "official-parity FAIL when swift emits no payload (seal fail, not silent pass)" || bad "official-parity no-payload not surfaced as FAIL"; fi
  # 2d (#123). benchd DROPS the runner-identity block → RESULT FAIL. 2a above is the positive half
  # (the stub emits all 7 keys), so the surplus check is exercised on BOTH branches here — without
  # this pair the check would be present but never enforcing, which is the regression #123 exists
  # to prevent.
  if env $COMMON STUB_BC_DROP_RUNNER=1 OUT="$WORK/opr" PAIRS=1 bash "$HERE/official-parity.sh" > "$WORK/opr.out" 2>&1; then
    bad "official-parity accepted a sidecar with NO runner-identity fields"
  else
    { grep -q 'RESULT FAIL' "$WORK/opr.out" && grep -q 'not EXACTLY the declared #123 runner roster' "$WORK/opr.out"; } \
      && ok "#123: missing runner-identity block → official-parity RESULT FAIL (surplus check enforces)" \
      || { bad "#123: runner-block drop not surfaced as a roster FAIL"; sed 's/^/        /' "$WORK/opr.out"; }
  fi
  echo ""

  echo "== 3. official-failure-map (stubs) =="
  FMCOMMON="HOME=$WORK BENCHCTL=$STUB_BC ENGINE=/bin/echo SWIFT=$STUB_SW WEIGHTS=$WORK/wdir GEN=$HERE/gen-failure-corpus.py OFFICIAL_COMMIT=deadbeefcafe OFFICIAL_LIB=$HERE/official-lib.sh"
  # 3a. oracle fails both → assertion OK, exit 0.
  if env $FMCOMMON OFFICIAL_GOLDEN="$WORK/official.json" OUT="$WORK/fm" bash "$HERE/official-failure-map.sh" > "$WORK/fm.out" 2> "$WORK/fm.err"; then
    grep -q 'oracle-corruption assertion OK' "$WORK/fm.out" && ok "failure-map: oracle fails BOTH sides → assertion OK" || bad "failure-map: assertion-OK line missing"
    awk -F' *\\| *' '$1=="oracle"' "$WORK/fm/official-failure-map.table.txt" | grep -q 'False' && ok "failure-map: oracle row shows both False" || bad "failure-map: oracle row not both-False"
    # #127 (RULED 2026-08-20): the class's declared ref moved #74 -> #127. #74's fail-POINT half is
    # closed; what survives on the OFFICIAL map is benchd refusing where the reference falls back to
    # the constants — deliberate ranked-path strictness, recorded on #127 (F8).
    awk -F' *\\| *' '$1=="baseline-missing"{print $4}' "$WORK/fm/official-failure-map.table.txt" | grep -q 'DECLARED(#127)' && ok "failure-map: baseline-missing renders DECLARED(#127)" || bad "failure-map: declared class not rendered DECLARED"
  else bad "failure-map: aborted unexpectedly on the normal path"; sed 's/^/        /' "$WORK/fm.out" "$WORK/fm.err"; fi
  # 3b. harness-failure guard: benchctl oracle passes → ABORT (exit 5).
  if env $FMCOMMON STUB_BC_ORACLE_PASSES=1 OFFICIAL_GOLDEN="$WORK/official.json" OUT="$WORK/fmg" bash "$HERE/official-failure-map.sh" > "$WORK/fmg.out" 2> "$WORK/fmg.err"; then
    bad "failure-map: did NOT abort when oracle passed on a side"
  else rc=$?; { [ "$rc" = 5 ] && grep -q 'ORACLE ASSERTION FAILED' "$WORK/fmg.err"; } && ok "failure-map: oracle-PASS on a side → ABORT exit 5 (harness-failure guard)" || bad "failure-map: guard exit/message wrong (rc=$rc)"; fi
  # 3c. golden with no oracle → refuse (exit 6). FM_MIN_CLASSES=3 so the corpus clears the size
  # floor (primary/anchor/free-run) and the refusal is specifically the missing-oracle guard.
  if env $FMCOMMON FM_MIN_CLASSES=3 OFFICIAL_GOLDEN="$WORK/no-oracle.json" OUT="$WORK/fmo" bash "$HERE/official-failure-map.sh" > "$WORK/fmo.out" 2> "$WORK/fmo.err"; then
    bad "failure-map: ran on a golden with no oracle class"
  else rc=$?; { [ "$rc" = 6 ] && grep -q "NO 'oracle' class" "$WORK/fmo.err"; } && ok "failure-map: no-oracle golden → refuse exit 6" || bad "failure-map: no-oracle refusal wrong (rc=$rc)"; fi
  # 3d. a MISSING golden file → abort LOUD (the corpus can't be generated, manifest missing).
  if env $FMCOMMON OFFICIAL_GOLDEN="$WORK/does-not-exist.json" OUT="$WORK/fmm" bash "$HERE/official-failure-map.sh" > "$WORK/fmm.out" 2> "$WORK/fmm.err"; then
    bad "failure-map: ran with a missing golden"
  else ok "failure-map: missing golden → abort (no fabricated table)"; fi
  # 3e. NON-VACUOUS undeclared divergence (BLOCKER-1 regression): the differ FAILs on `primary`
  # (undeclared) → the leg MUST exit non-zero (no unbacked GREEN); the DECLARED baseline-missing
  # class must NOT break it. This bites ONLY because the accumulator exists — without it the leg
  # would exit 0 on the trailing echo despite the FAIL cell.
  if env $FMCOMMON STUB_DIFF_FAIL_CLASS=primary OFFICIAL_GOLDEN="$WORK/official.json" OUT="$WORK/fmp" bash "$HERE/official-failure-map.sh" > "$WORK/fmp.out" 2> "$WORK/fmp.err"; then
    bad "failure-map: undeclared primary divergence did NOT fail the leg (unbacked GREEN)"
  else
    { grep -q 'RESULT FAIL' "$WORK/fmp.err" \
      && awk -F' *\\| *' '$1=="primary"{print $4}' "$WORK/fmp/official-failure-map.table.txt" | grep -q '^FAIL' \
      && awk -F' *\\| *' '$1=="baseline-missing"{print $4}' "$WORK/fmp/official-failure-map.table.txt" | grep -q 'DECLARED(#127)'; } \
      && ok "failure-map: undeclared primary FAIL → leg non-zero (declared class still passes)" \
      || { bad "failure-map: primary-fail negative control wrong"; sed 's/^/        /' "$WORK/fmp/official-failure-map.table.txt"; }
  fi
  # 3f. missing-score cell (swift emits no payload for `primary`) → leg MUST exit non-zero.
  if env $FMCOMMON STUB_SW_NOSCORE_CLASS=primary OFFICIAL_GOLDEN="$WORK/official.json" OUT="$WORK/fms" bash "$HERE/official-failure-map.sh" > "$WORK/fms.out" 2> "$WORK/fms.err"; then
    bad "failure-map: missing-score cell did NOT fail the leg"
  else grep -q 'RESULT FAIL' "$WORK/fms.err" && ok "failure-map: missing-score cell → leg non-zero" || bad "failure-map: missing-score not surfaced as leg FAIL"; fi

  # 3g. submit-1024 band-failure FIXTURE (RULING 2): both sides FAIL the band identically → the
  # fixture row is both-False, the shared blanked surface byte-matches (differ PASS), assertion OK.
  if env $FMCOMMON OFFICIAL_GOLDEN="$WORK/official.json" \
       BAND_FIXTURE_GOLDEN="$WORK/submit-1024-band.json" OUT="$WORK/fmb" \
       bash "$HERE/official-failure-map.sh" > "$WORK/fmb.out" 2> "$WORK/fmb.err"; then
    { grep -q 'band-failure fixture assertion OK' "$WORK/fmb.out" \
      && awk -F' *\\| *' '$1=="submit-1024-band"{print $2" "$3" "$4}' "$WORK/fmb/official-failure-map.table.txt" | grep -q 'False .*False .*PASS'; } \
      && ok "failure-map: band fixture → both False + differ PASS (both-fail-identically), assertion OK" \
      || { bad "failure-map: band fixture row/assertion wrong"; sed 's/^/        /' "$WORK/fmb/official-failure-map.table.txt"; }
  else bad "failure-map: band fixture aborted on the expected both-fail path"; sed 's/^/        /' "$WORK/fmb.out" "$WORK/fmb.err"; fi
  # 3h. band-fixture NEGATIVE control: a side that PASSES the band (divergence) → ABORT exit 8.
  if env $FMCOMMON STUB_BC_BAND_PASSES=1 OFFICIAL_GOLDEN="$WORK/official.json" \
       BAND_FIXTURE_GOLDEN="$WORK/submit-1024-band.json" OUT="$WORK/fmbg" \
       bash "$HERE/official-failure-map.sh" > "$WORK/fmbg.out" 2> "$WORK/fmbg.err"; then
    bad "failure-map: band fixture did NOT abort when a side passed the band (divergence)"
  else rc=$?; { [ "$rc" = 8 ] && grep -q 'BAND FIXTURE ASSERTION FAILED' "$WORK/fmbg.err"; } && ok "failure-map: band fixture both-pass/divergence → ABORT exit 8" || bad "failure-map: band fixture guard exit/message wrong (rc=$rc)"; fi
  echo ""
else
  echo "== 2/3. SKIP (jq absent) =="
  echo ""
fi

echo "== 4. official-env-probe (LOCAL-ITERATE rework; assertion control flow) =="
# stub benchctl for the probe: models the LOCAL-ITERATE spawn — it applies B-1's env sanitization
# (env_clear + allowlist + forced guard) then runs the env-dump shim (via --engine), which — because
# local mode is NOT sandboxed deny-write — records child-env.txt. `good` = sanitized child env;
# `leaked` = full env inherited (negative control); `nospawn` = shim never runs (missing capture).
# The probe passes `--mode local-iterate`; the stub ignores --mode (the sanitizer is what matters).
STUB_BCP="$WORK/bcp.sh"; cat > "$STUB_BCP" <<'EOF'
#!/bin/bash
shim=""
while [ $# -gt 0 ]; do case "$1" in --engine) shim="$2"; shift 2;; *) shift;; esac; done
case "${STUB_PROBE_MODE:-good}" in
  good)   env -i LC_ALL="${LC_ALL:-}" TERM="${TERM:-}" MLXFAST_USE_RUNTIME_WORKER=0 "$shim" runtime-worker --weights w ;;
  leaked) "$shim" runtime-worker --weights w ;;   # full env inherited incl the secret/random sentinels
  nospawn) : ;;                                    # never runs the shim → no capture
esac
exit 1   # benchctl "fails at hello" — the probe ignores this
EOF
chmod +x "$STUB_BCP"

# 4a. good (sanitized) capture → PASS.
if env BENCHCTL="$STUB_BCP" WEIGHTS="$WORK/wdir" OFFICIAL_GOLDEN="$WORK/official.json" STUB_PROBE_MODE=good \
     OUT="$WORK/ep" bash "$HERE/official-env-probe.sh" > "$WORK/ep.out" 2> "$WORK/ep.err"; then
  ok "env-probe: sanitized capture → RESULT PASS"
else bad "env-probe: sanitized capture did not PASS"; sed 's/^/        /' "$WORK/ep.out" "$WORK/ep.err"; fi
# 4b. leaked env → FAIL.
if env BENCHCTL="$STUB_BCP" WEIGHTS="$WORK/wdir" OFFICIAL_GOLDEN="$WORK/official.json" STUB_PROBE_MODE=leaked \
     OUT="$WORK/epl" bash "$HERE/official-env-probe.sh" > "$WORK/epl.out" 2> "$WORK/epl.err"; then
  bad "env-probe: leaked env did NOT fail"
else grep -q 'RESULT FAIL' "$WORK/epl.err" && grep -q 'LEAKED' "$WORK/epl.out" && ok "env-probe: leaked MLXFAST_*/random → RESULT FAIL" || bad "env-probe: leak not surfaced as FAIL"; fi
# 4c. no capture → TOOL-ERR abort (exit 4).
if env BENCHCTL="$STUB_BCP" WEIGHTS="$WORK/wdir" OFFICIAL_GOLDEN="$WORK/official.json" STUB_PROBE_MODE=nospawn \
     OUT="$WORK/epn" bash "$HERE/official-env-probe.sh" > "$WORK/epn.out" 2> "$WORK/epn.err"; then
  bad "env-probe: missing capture did NOT abort"
else rc=$?; { [ "$rc" = 4 ] && grep -q 'TOOL-ERR' "$WORK/epn.err"; } && ok "env-probe: missing capture → TOOL-ERR abort exit 4 (fail-loud)" || bad "env-probe: missing-capture abort wrong (rc=$rc)"; fi
echo ""

echo "== 5. assemble-official-golden (deterministic box-calibrated band-pass golden) =="
# A minimal submit-1024-SHAPED source carrying the EXACT stale baseline literals the assembler
# replaces (the transform is a byte-exact string substitution, not a JSON reserialization).
SRC1024="$WORK/submit-1024-src.json"
printf '%s' '{"version":1,"model_type":"qwen3_5_text","cases":[{"name":"c1","prompt_tokens":[1],"expected_tokens":[2]}],"benchmark":{"baseline_decode_seconds_per_token":0.1336139485703125,"baseline_prefill_seconds_per_token":0.010605031949609375,"expected_prefill_token":5}}' > "$SRC1024"
CAL="$WORK/official-calibrated-1024.json"
if bash "$HERE/assemble-official-golden.sh" "$SRC1024" "$CAL" > "$WORK/asm.out" 2>&1; then
  # 5a. calibrated baselines are present; the stale literals are gone.
  { grep -q '"baseline_prefill_seconds_per_token":0.001182475' "$CAL" \
    && grep -q '"baseline_decode_seconds_per_token":0.0372302' "$CAL" \
    && ! grep -q '0.010605031949609375' "$CAL" && ! grep -q '0.1336139485703125' "$CAL"; } \
    && ok "assemble: calibrated baselines substituted, stale literals gone" || bad "assemble: baseline substitution wrong"
  # 5b. ONLY the baselines changed — every other byte identical to the source (cases[] untouched).
  python3 - "$SRC1024" "$CAL" <<'PY' && ok "assemble: only the two baselines differ from source" || bad "assemble: unexpected byte changes"
import sys
s=open(sys.argv[1]).read()
exp=s.replace('"baseline_decode_seconds_per_token":0.1336139485703125','"baseline_decode_seconds_per_token":0.0372302').replace('"baseline_prefill_seconds_per_token":0.010605031949609375','"baseline_prefill_seconds_per_token":0.001182475')
sys.exit(0 if open(sys.argv[2]).read()==exp else 1)
PY
  # 5c. DETERMINISTIC: a second assembly yields byte-identical output (same sha256).
  bash "$HERE/assemble-official-golden.sh" "$SRC1024" "$WORK/cal2.json" >/dev/null 2>&1
  [ "$(shasum -a 256 "$CAL" | awk '{print $1}')" = "$(shasum -a 256 "$WORK/cal2.json" | awk '{print $1}')" ] \
    && ok "assemble: deterministic (two runs → identical sha256)" || bad "assemble: non-deterministic output"
  # 5d. provenance label is parity-test-only, in the sidecar + manifest (top level is closed).
  { [ -f "$CAL.provenance.txt" ] && grep -q 'parity-test-only' "$CAL.provenance.txt" \
    && grep -q 'NEVER organizer' "$CAL.provenance.txt"; } \
    && ok "assemble: .provenance.txt marks parity-test-only / never-organizer" || bad "assemble: provenance sidecar wrong"
  if [ "$HAVE_JQ" = 1 ]; then
    { [ -f "$CAL.manifest.json" ] && [ "$(jq -r '.provenance' "$CAL.manifest.json" 2>/dev/null | grep -c 'parity-test-only')" = 1 ]; } \
      && ok "assemble: .manifest.json carries the parity-test-only provenance" || bad "assemble: manifest provenance wrong"
  fi
  # 5e. the assembler refuses a SOURCE-pin mismatch (fail-closed before transforming).
  if SRC_PIN_SHA=deadbeef SRC_PIN_BYTES=999999 bash "$HERE/assemble-official-golden.sh" "$SRC1024" "$WORK/cal3.json" >/dev/null 2>&1; then
    bad "assemble: bad SOURCE pin was accepted"
  else ok "assemble: SOURCE pin mismatch → refuse (fail-closed)"; fi
else bad "assemble: assembler failed on a valid source"; sed 's/^/        /' "$WORK/asm.out"; fi

# 5f. BAND-PASS logic (port of score.rs check(): 0.95 floor; prefill ±5%, decode +2%/-5%): the
# box-measured speeds must PASS both gates against the CALIBRATED baselines (both sides PASS), and
# BUST them against the STALE submit-1024 baselines (both sides FAIL — the fixture's premise).
python3 - <<'PY' && ok "assemble: measured speeds PASS calibrated baselines AND FAIL stale baselines (band math)" || bad "assemble: band-pass/fail math wrong"
mp,md=0.001165,0.03668                       # box-measured prefill/decode s/tok
cal_p,cal_d=0.001182475,0.0372302            # calibrated baselines
stale_p,stale_d=0.010605031949609375,0.1336139485703125
def passes(measured,baseline,down,up):       # check(): lo<=measured<=hi ; floor: baseline/measured>=0.95
    lo,hi=baseline*(1-down),baseline*(1+up)
    return (lo<=measured<=hi) and (baseline/measured>=0.95)
cal_ok = passes(mp,cal_p,0.05,0.05) and passes(md,cal_d,0.05,0.02)
stale_ok = passes(mp,stale_p,0.05,0.05) and passes(md,stale_d,0.05,0.02)
import sys; sys.exit(0 if (cal_ok and not stale_ok) else 1)
PY
echo ""

echo "==================================================="
echo "official offline self-test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
