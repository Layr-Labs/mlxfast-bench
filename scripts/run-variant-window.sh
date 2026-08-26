#!/bin/bash
# scripts/run-variant-window.sh — the ONE human-triggered driver for the COMBINED §12 + facade
# GPU window (deliverable D). Closes two matrix items in a single window:
#   §12  — score-variant corpus deterministic parity (variant-parity.sh)
#   R-3  — facade confirmatory leg, LIVE vs the real Swift reference (facade-leg.sh)
#
# Mirrors run-manual-test.sh's hardening EXACTLY: takes + HOLDS the real gpu-exclusive lock (fd 9)
# for the whole run (inner calls run DIRECTLY, never wrapped in gpu_run.py — no LOCK_NB
# self-conflict, whole-run exclusivity), box-quiet precheck, GPU-free differ self-test BEFORE
# qwen is touched, sources qwen-service.sh + verifies its functions, unloads qwen, runs the
# battery, and reloads qwen ALWAYS via a reentrant trap. Startup wipe, manifest-anchored parse,
# differ-version pin. Emits ONE combined REPORT.md.
#
# Composition IN the window:
#   A. gen-variant-corpus.sh   [GPU generate-golden + offline graft/pin/dual-loader]
#   B. variant-parity.sh        [§12 truth table — benchctl iterate + swift benchmark, both modes]
#   C. facade-leg.sh            [R-3 — facade vs reference, both modes, byte-green artifacts]
#
# NO scheduler, NO CI — a human runs this and reads REPORT.md. Paths default to the box layout;
# override via env.
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
G="${MLXFAST_PARITY_GIT:-$HOME/mlxfast-parity-git}"
PAR="${MLXFAST_PARITY_HOME:-$HOME/mlxfast-parity}"
ENGINE="${ENGINE:-$G/mlxfast-engine/.build/release/mlxfast-engine}"
SWIFT="${SWIFT:-$G/mlxfast-challenge-dev/.build/release/mlxfast-swift}"
BENCHCTL="${BENCHCTL:-$G/mlxfast-bench/target/release/benchctl}"
WEIGHTS="${WEIGHTS:-$PAR/weights}"
DIFF_CMD="${DIFF_CMD:-$BENCHCTL parity-diff}"
DONOR_GOLDEN="${DONOR_GOLDEN:-$G/golden/beefed.json}"
SUBMIT_GOLDEN="${SUBMIT_GOLDEN:-$G/golden/submit-1024.json}"
SUBMIT_SHA="${SUBMIT_SHA:-a482f223edaa5b0b58e6ef0d1d276122f1a4b43f81ca6af33184cc0a64e726c9}"
SUBMIT_BYTES="${SUBMIT_BYTES:-20993}"
# #124: the generated corpus is iterate-destined, so the base must carry the local-iterate loader
# arity (benchmarkDecodeSteps+1 = 129 tokens). generate-golden emits EXACTLY --steps tokens, so
# STEPS is the ARITY, not the window. gen-variant-corpus.sh re-checks and aborts on a short STEPS.
LOCAL_ITERATE_DECODE_STEPS="${LOCAL_ITERATE_DECODE_STEPS:-128}"
STEPS="${STEPS:-$((LOCAL_ITERATE_DECODE_STEPS + 1))}"
MODES="${MODES:-local-iterate local-submit}"
# The real Swift reference benchmark.sh (facade leg C compares against it).
REFERENCE_BENCHMARK_SH="${REFERENCE_BENCHMARK_SH:-$G/mlxfast-challenge-dev/benchmark.sh}"
FACADE="${FACADE:-$HERE/benchmark.sh}"
LOCK="${MLXFAST_GPU_LOCK:-/tmp/mtplx-gpu-exclusive.lock}"
OUT="${OUT:-$G/golden/variant-window}"
CORPUS="$OUT/corpus"
PARITY_OUT="$OUT/parity"
FACADE_OUT="$OUT/facade"
REPORT="$OUT/REPORT.md"
LOCK_POLICY="outer-hold / inner-direct — driver holds the gpu-exclusive lock (fd 9) for the whole run; A/B/C inner calls are unwrapped (no gpu_run.py) → whole-run exclusivity"

mkdir -p "$OUT"
: > "$OUT/run.log"
log() { echo "$@" | tee -a "$OUT/run.log"; }

# Anti-fabrication: $OUT persists → wipe every prior per-run artifact at startup, so any file
# present at REPORT time was written THIS run; a failed sub-leg leaves its table ABSENT → the
# MISSING marker fires, never a fabricated pass.
rm -rf "$CORPUS" "$PARITY_OUT" "$FACADE_OUT" 2>/dev/null || true
rm -f "$OUT"/*.table.txt "$REPORT" 2>/dev/null || true

# shellcheck source=scripts/parity-lib.sh
. "$HERE/parity-lib.sh"

# --- take + HOLD the GPU lock first (fd 9 held for this script's lifetime) --------------
log "=== take GPU lock @ $(date) ==="
parity_take_gpu_lock "$LOCK"; LOCK_RC=$?
if [ "$LOCK_RC" -ne 0 ]; then log "GPU lock unavailable (rc=$LOCK_RC) — aborting; re-run when the box is free."; exit 3; fi
log "GPU lock held (fd 9) for the run."

# --- box-quiet precheck ----------------------------------------------------------------
if ! parity_precheck 2>&1 | tee -a "$OUT/run.log"; then
  log "box not quiet — aborting (re-run when quiet)."; exit 3
fi

for b in "$ENGINE" "$SWIFT" "$BENCHCTL"; do
  [ -x "$b" ] || { log "missing binary: $b — aborting."; exit 5; }
done
[ -x "$REFERENCE_BENCHMARK_SH" ] || [ -r "$REFERENCE_BENCHMARK_SH" ] || { log "reference benchmark.sh not found: $REFERENCE_BENCHMARK_SH — aborting."; exit 5; }

# --- differ self-test (GPU-FREE), BEFORE we touch qwen ---------------------------------
DIFFER_VERSION="$("$BENCHCTL" parity-diff --version 2>/dev/null || echo unknown)"
differ_selftest() {
  local d="$OUT/selftest"; mkdir -p "$d"
  "$BENCHCTL" parity-diff --emit-sample > "$d/a.json" 2>/dev/null || { log "S0: --emit-sample failed"; return 1; }
  # b: a DETERMINISTIC field changed (case_count) → must FAIL. c: a TIMING/env-only field changed
  # (benchmark_wall_seconds, waived by §13) → must PASS. S4 proves the bucket policy STILL waives
  # timing: if a differ regression ever stopped waiving it, the window aborts HERE (before qwen),
  # not after producing spurious FAILs across every variant + facade run.
  python3 -c "import json;x=json.load(open('$d/a.json'));x['metrics']['case_count']=x['metrics'].get('case_count',0)+1;json.dump(x,open('$d/b.json','w'))" || return 1
  python3 -c "import json;x=json.load(open('$d/a.json'));x['metrics']['benchmark_wall_seconds']=float(x['metrics'].get('benchmark_wall_seconds',0.0))+999.0;json.dump(x,open('$d/c.json','w'))" || return 1
  $DIFF_CMD "$d/a.json" "$d/a.json" >/dev/null 2>&1; local s1=$?
  $DIFF_CMD "$d/a.json" "$d/b.json" >/dev/null 2>&1; local s2=$?
  $DIFF_CMD "$d/a.json" "$d/nonexistent.json" >/dev/null 2>&1; local s3=$?
  $DIFF_CMD "$d/a.json" "$d/c.json" >/dev/null 2>&1; local s4=$?
  log "differ self-test: S1(identical)=$s1 expect 0 · S2(det-diff)=$s2 expect 1 · S3(io-err)=$s3 expect ∉{0,1} · S4(timing-only)=$s4 expect 0"
  [ "$s1" = 0 ] && [ "$s2" = 1 ] && [ "$s4" = 0 ] && { [ "$s3" != 0 ] && [ "$s3" != 1 ]; }
}
log "=== differ self-test (differ=$DIFF_CMD; $DIFFER_VERSION) @ $(date) ==="
if ! differ_selftest; then log "differ self-test FAILED — aborting before the GPU window."; exit 8; fi

# --- source qwen-service.sh + verify the functions we depend on exist ------------------
QWEN_SVC="$PAR/qwen-service.sh"
[ -f "$QWEN_SVC" ] || { log "qwen-service.sh not found at $QWEN_SVC — aborting."; exit 6; }
# shellcheck source=/dev/null
. "$QWEN_SVC"
for fn in qwen_unload qwen_reload; do
  command -v "$fn" >/dev/null 2>&1 || { log "qwen-service.sh did not define $fn() — aborting."; exit 6; }
done

# --- reentrant cleanup trap: reload qwen exactly once, on normal exit OR a signal ------
_CLEANED=0
cleanup() { [ "$_CLEANED" = "1" ] && return 0; _CLEANED=1; log "=== RELOAD qwen (cleanup) @ $(date) ==="; qwen_reload; }
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

log "=== UNLOAD qwen @ $(date) ==="
qwen_unload

# ======================================================================================
# A. generate + pin + dual-loader the §12 variant corpus (generate-golden is the GPU step)
# ======================================================================================
log "=== A: gen-variant-corpus @ $(date) — steps $STEPS, donor $(basename "$DONOR_GOLDEN") ==="
SWIFT="$SWIFT" BENCHCTL="$BENCHCTL" WEIGHTS="$WEIGHTS" OUT="$CORPUS" \
  DONOR_GOLDEN="$DONOR_GOLDEN" SUBMIT_GOLDEN="$SUBMIT_GOLDEN" \
  SUBMIT_SHA="$SUBMIT_SHA" SUBMIT_BYTES="$SUBMIT_BYTES" STEPS="$STEPS" \
  ${BASE_GOLDEN:+BASE_GOLDEN="$BASE_GOLDEN"} ${GENERATE_GOLDEN_CMD:+GENERATE_GOLDEN_CMD="$GENERATE_GOLDEN_CMD"} \
  bash "$HERE/gen-variant-corpus.sh" 2>&1 | tee -a "$OUT/run.log"
GEN_RC="${PIPESTATUS[0]}"
MANIFEST="$CORPUS/manifest.json"
if [ "$GEN_RC" -ne 0 ] || [ ! -f "$MANIFEST" ]; then log "A: gen-variant-corpus FAILED (rc=$GEN_RC) — aborting before REPORT."; exit 9; fi

# Manifest floor: refuse to publish a thin corpus. Validate N is a positive integer first.
N_MANIFEST="$(python3 -c "import json;print(len(json.load(open('$MANIFEST'))['variants']))" 2>/dev/null)"
case "$N_MANIFEST" in ''|*[!0-9]*) log "A: manifest variant count not a number ('$N_MANIFEST') — aborting."; exit 9;; esac
V_MIN="${V_MIN:-5}"
if [ "$N_MANIFEST" -lt "$V_MIN" ]; then log "A: corpus too small: $N_MANIFEST variants < floor $V_MIN — aborting."; exit 9; fi

# ======================================================================================
# B. §12 variant deterministic parity truth table
# ======================================================================================
log "=== B: variant-parity @ $(date) — modes [$MODES] ==="
mkdir -p "$PARITY_OUT"
VP_TABLE="$OUT/variant-parity.table.txt"; VP_STDERR="$OUT/variant-parity.stderr.txt"
ENGINE="$ENGINE" SWIFT="$SWIFT" BENCHCTL="$BENCHCTL" WEIGHTS="$WEIGHTS" \
  MANIFEST="$MANIFEST" OUT="$PARITY_OUT" DIFF_CMD="$DIFF_CMD" GPU="" MODES="$MODES" \
  bash "$HERE/variant-parity.sh" > "$VP_TABLE" 2> "$VP_STDERR"
VP_RC=$?
if [ "$VP_RC" -ne 0 ]; then log "B: variant-parity returned rc=$VP_RC (non-PASS — see below); continuing to render the truth table."; sed 's/^/  /' "$VP_STDERR" | tee -a "$OUT/run.log"; fi

# Build the §12 truth table by ANCHORING on the manifest: for each manifest class, extract EXACTLY
# its row from variant-parity's stdout (matched on the class in field 1). A missing row aborts.
VP_ROWS=""; VP_N=0
while IFS=$'\t' read -r cls _path _sha _bytes _declared; do
  [ -n "$cls" ] || continue
  row="$(awk -F' *\\| *' -v c="$cls" '$1==c {print; exit}' "$VP_TABLE")"
  if [ -z "$row" ]; then log "B: no truth-table row for manifest class '$cls' — aborting (variant-parity dropped a class)."; exit 9; fi
  it="$(printf '%s' "$row" | awk -F' *\\| *' '{print $2}')"
  su="$(printf '%s' "$row" | awk -F' *\\| *' '{print $3}')"
  se="$(printf '%s' "$row" | awk -F' *\\| *' '{print $4}')"
  VP_ROWS="$VP_ROWS| $cls | $it | $su | $se |
"
  VP_N=$((VP_N + 1))
  log "  §12 $cls: iterate=$it submit=$su"
done < <(python3 -c "import json;[print(x['class']+'\t'+x.get('path','')+'\t'+x['sha256']+'\t'+str(x['bytes'])+'\t'+(x.get('declared') or '')) for x in json.load(open('$MANIFEST'))['variants']]")
if [ "$VP_N" -ne "$N_MANIFEST" ]; then log "B: row-count mismatch: $VP_N rendered vs $N_MANIFEST manifest variants — aborting."; exit 9; fi

# ======================================================================================
# C. facade confirmatory leg (LIVE vs the real reference)
# ======================================================================================
log "=== C: facade-leg @ $(date) — modes [$MODES] ==="
mkdir -p "$FACADE_OUT"
FL_TABLE="$OUT/facade-leg.table.txt"; FL_STDERR="$OUT/facade-leg.stderr.txt"
FACADE="$FACADE" REFERENCE_BENCHMARK_SH="$REFERENCE_BENCHMARK_SH" \
  MLXFAST_ENGINE_BIN="$ENGINE" BENCHCTL="$BENCHCTL" WEIGHTS="$WEIGHTS" \
  GOLDEN_ITERATE="$DONOR_GOLDEN" GOLDEN_SUBMIT="$SUBMIT_GOLDEN" \
  OUT="$FACADE_OUT" MODES="$MODES" ${REF_EXTRA_ENV:+REF_EXTRA_ENV="$REF_EXTRA_ENV"} \
  bash "$HERE/facade-leg.sh" > "$FL_TABLE" 2> "$FL_STDERR"
FL_RC=$?
if [ "$FL_RC" -ne 0 ]; then log "C: facade-leg returned rc=$FL_RC (non-PASS — see below); continuing to render its table."; sed 's/^/  /' "$FL_STDERR" | tee -a "$OUT/run.log"; fi
# Extract the facade table's data rows (the `mode | … | overall` lines) verbatim for the report.
FL_ROWS="$(awk -F' *\\| *' 'NF>=7 && $1!="mode" && $1 !~ /^-+$/ {print}' "$FL_TABLE")"
[ -n "$FL_ROWS" ] || { log "C: facade-leg produced no table rows — aborting before REPORT."; exit 10; }

log "=== battery done; qwen reloads on exit @ $(date) ==="

# --- combined REPORT.md ----------------------------------------------------------------
COMMIT="$(cd "$G/mlxfast-bench" 2>/dev/null && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
{
  echo "# Combined §12 + facade window — REPORT"
  echo
  echo "Run \`$(date)\` · benchctl \`$COMMIT\` · differ \`$DIFF_CMD\` · differ-version \`$DIFFER_VERSION\` · steps \`$STEPS\` · modes \`$MODES\`"
  echo
  echo "**Lock policy (measurement condition).** $LOCK_POLICY."
  echo
  echo "**Thermal.** local-iterate cool-gate OFF both sides (benchctl RULING-A3 default; swift"
  echo "\`MLXFAST_LOCAL_COOL_GATE=0\`); local-submit ON by RULING but both native gates no-op w/o a"
  echo "box temp reader → measured un-gated, symmetric (declared)."
  echo
  echo "## A — variant corpus pins (dual-loader accepted; §12)"
  echo "| variant | sha256 | bytes | sections | declared |"
  echo "|---|---|---|---|---|"
  python3 -c "import json
for v in json.load(open('$MANIFEST'))['variants']:
    secs='+'.join(s.replace('correctness_gates.','cg.') for s in v.get('sections',[]))
    dec=v.get('declared') or '—'
    reused=' (reused)' if v.get('reused') else ''
    print('| %s%s | %s… | %s | %s | %s |' % (v['class'], reused, v['sha256'][:12], v['bytes'], secs, dec))"
  echo
  echo "## B — §12 variant deterministic parity ($VP_N/$N_MANIFEST variants; differ \`$DIFF_CMD\`)"
  echo "| variant | local-iterate | local-submit | sections |"
  echo "|---|---|---|---|"
  printf '%s' "$VP_ROWS"
  echo
  echo "Each of the $N_MANIFEST manifest variants matched exactly one truth-table row (a missing row"
  echo "aborts). Each variant runs only its APPLICABLE modes (manifest \`applicable_modes\`): the four"
  echo "generated shape variants are iterate-scale goldens (cases[0] < 1024 tokens) → local-iterate"
  echo "only, so their local-submit cell is **N/A** (a ${STEPS}-step golden physically cannot run submit's"
  echo "1023-step window, AND under \`BaseCasesOnly\` it would only reproduce submit-1024's cases score"
  echo "— inapplicable + redundant, NOT a skipped failure). \`submit-1024\` (cases[0] >= 1024) proves"
  echo "BOTH modes. So the corpus covers both local modes across the SET, not every variant in every"
  echo "mode. Deterministic surface only (timing waived by \`parity-diff\`); ONCE per applicable mode (no"
  echo "timing repeats — the 3-pair primary leg covers timing). \`behavior-bearing\` carries a SYNTHETIC"
  echo "gate (see A.declared) — DECLARED, not FAIL. N/A cells are declared skips, never counted as FAIL."
  echo
  echo "## C — facade confirmatory leg (LIVE vs reference; TIMING EXCEPTED)"
  echo "| mode | score-name | .sha256 | integrity | det-fields | exit | overall |"
  echo "|---|---|---|---|---|---|---|"
  printf '%s\n' "$FL_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s | %s | %s | %s | %s |\n",$1,$2,$3,$4,$5,$6,$7}'
  echo
  echo "Facade (\`$FACADE\`) vs reference (\`$REFERENCE_BENCHMARK_SH\`, run from its repo root with"
  echo "\`MLXFAST_SKIP_TRANSFORM=1\` + outputs redirected into the per-mode compare dir). Deterministic"
  echo "artifact surface byte-compared (score naming / .sha256 sidecar / integrity key-set AND"
  echo "deterministic integrity VALUES [score_path-by-basename, weights_sha256/file_count/byte_count,"
  echo "golden_sha256, golden_path, transform_source_sha256; score_sha256 excepted] / exact exit code)"
  echo "+ deterministic score fields via \`benchctl parity-diff\`; the scored TIMING legitimately differs"
  echo "(benchctl vs Swift engine) and is EXCEPTED. Confirms §4 (exit codes) + §5 (stdout/integrity/naming)."
  echo
  echo "## Verdict"
  echo "- §12 (B): $( [ "$VP_RC" -eq 0 ] && echo 'PASS — deterministic parity in every mode, zero undeclared FAIL/TOOL-ERR' || echo "NON-PASS (rc=$VP_RC) — see variant-parity.stderr.txt" )"
  echo "- facade (C): $( [ "$FL_RC" -eq 0 ] && echo 'GREEN — byte-green artifacts + exit codes, all modes' || echo "NON-GREEN (rc=$FL_RC) — see facade-leg.stderr.txt" )"
  echo
  echo "Artifacts: corpus \`$CORPUS/\`; parity \`$PARITY_OUT/\`; facade \`$FACADE_OUT/\`; logs \`$OUT/run.log\`."
} > "$REPORT"

log "=== REPORT written: $REPORT ==="
echo "----- REPORT.md -----"
cat "$REPORT"

# Overall exit: non-zero if either leg was non-PASS (qwen still reloads via the trap).
[ "$VP_RC" -eq 0 ] && [ "$FL_RC" -eq 0 ]
