#!/bin/bash
# scripts/run-official-window.sh — the ONE human-triggered driver for the B-3 OFFICIAL-parity
# GPU window. Proves Track-B official mode at parity in a SINGLE GPU window and flips matrix §8
# (official) + §9 (#47). Runs benchctl official vs DIRECT `mlxfast-swift benchmark` (the trusted
# binary, official is ENV-driven — NOT the protected benchmark.sh workflow), JUDGE-LESS (the
# pre-judge sealed score is the parity unit, per the GPQA option-(b) ruling; semantic_gpqa_*/
# gpqa_ttft_* are 0/"" on both sides and match trivially).
#
# Mirrors run-variant-window.sh / run-manual-test.sh hardening EXACTLY: takes + HOLDS the real
# gpu-exclusive lock (fd 9) for the whole run (inner calls run DIRECTLY, never re-wrapped),
# box-quiet precheck, GPU-FREE differ self-test (incl. the S4 timing-waiver) BEFORE qwen is
# touched, golden pin+load gate, sources qwen-service.sh + verifies its functions, unloads qwen,
# runs the battery, and RELOADS qwen ALWAYS via a reentrant trap. Startup wipe, manifest/row
# anchoring in the legs, differ-version pin, DECLARED(#nn) rendering, ONE REPORT.md.
# Anti-fabrication: a missing score → TOOL-ERR (never a silent pass); qwen ALWAYS reloads.
#
# Legs in the window:
#   1+3. official-parity.sh    — 3-pair official parity (pre-judge score + det-fields; Leg 1) AND
#                                artifact byte-rows (score.json/.sha256/9-field integrity/exit; Leg 3)
#   2.   official-failure-map.sh — official failure map incl. the oracle class local couldn't test
#                                (asserts oracle-corruption fails BOTH sides) + the submit-1024
#                                band-failure FIXTURE (RULING 2: both sides FAIL the band identically)
#   4.   official-env-probe.sh  — #47 closing evidence via a LOCAL-ITERATE spawn (sanitized
#                                allowlist-only child env; GPU-free; official deny-write is orthogonal)
#
# NO scheduler, NO CI — a human runs this and reads REPORT.md. Paths default to the box layout.
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
G="${MLXFAST_PARITY_GIT:-$HOME/mlxfast-parity-git}"
PAR="${MLXFAST_PARITY_HOME:-$HOME/mlxfast-parity}"
ENGINE="${ENGINE:-$G/mlxfast-engine/.build/release/mlxfast-engine}"
SWIFT="${SWIFT:-$G/mlxfast-challenge-dev/.build/release/mlxfast-swift}"
BENCHCTL="${BENCHCTL:-$G/mlxfast-bench/target/release/benchctl}"
WEIGHTS="${WEIGHTS:-$PAR/weights}"
DIFF_CMD="${DIFF_CMD:-$BENCHCTL parity-diff}"
GEN="${GEN:-$HERE/gen-failure-corpus.py}"
# submit-1024.json (STALE baselines, pin a482f223…, 20993 B): the SOURCE for the calibrated golden
# AND the PINNED band-failure FIXTURE (RULING 2). Its baselines are stale for this box, so the
# box-measured speeds BUST the acceptance band ("improvement too large") → both sides FAIL the band
# identically. Leg 2 runs it as a declared, expected fixture (both-fail-identically), NOT an error.
SUBMIT1024_GOLDEN="${SUBMIT1024_GOLDEN:-$G/golden/submit-1024.json}"
SUBMIT1024_PIN_SHA="${SUBMIT1024_PIN_SHA:-a482f223edaa5b0b58e6ef0d1d276122f1a4b43f81ca6af33184cc0a64e726c9}"
SUBMIT1024_PIN_BYTES="${SUBMIT1024_PIN_BYTES:-20993}"
# OFFICIAL golden (RULING 1): the BOX-CALIBRATED golden, assembled deterministically from
# submit-1024 by scripts/assemble-official-golden.sh — only the two benchmark baselines are changed
# so the box-measured speeds PASS the official gates (0.95 floors + prefill ±5% / decode +2%/−5%
# bands); everything else (1024-step cases[0], anchors, free_run, 128-step oracle) is byte-identical
# to submit-1024, so it stays loader-valid + official-shaped. PARITY-TEST-ONLY, NEVER an organizer/
# ranking golden — the label lives in the .provenance.txt sidecar + .manifest.json + matrix §8 (the
# golden top level is closed, `deny_unknown_fields`, so it cannot carry an in-band `_provenance`).
OFFICIAL_GOLDEN="${OFFICIAL_GOLDEN:-$G/golden/official-calibrated-1024.json}"
OFFICIAL_PIN_SHA="${OFFICIAL_PIN_SHA:-5ac88f059f97627826951dc411e5c346ccf283509a2a50f9cbc4015119c4a936}"
OFFICIAL_PIN_BYTES="${OFFICIAL_PIN_BYTES:-20975}"
ASSEMBLE="${ASSEMBLE:-$HERE/assemble-official-golden.sh}"
# The challenge-dev repo root (direct-swift runs from here so a relative metallib/tools path
# resolves; MLXFAST_MLX_METALLIB is also set absolutely so CWD is not load-bearing).
SWIFT_REPO_ROOT="${SWIFT_REPO_ROOT:-$G/mlxfast-challenge-dev}"
PAIRS="${PAIRS:-3}"
LOCK="${MLXFAST_GPU_LOCK:-/tmp/mtplx-gpu-exclusive.lock}"
OUT="${OUT:-$G/golden/official-window}"
PARITY_OUT="$OUT/parity"; FMAP_OUT="$OUT/failure-map"; PROBE_OUT="$OUT/env-probe"
REPORT="$OUT/REPORT.md"
LOCK_POLICY="outer-hold / inner-direct — driver holds the gpu-exclusive lock (fd 9) for the whole run; all legs' inner calls are unwrapped → whole-run exclusivity"

# --- P-3 artifact replica target (resolved up-front so the REPORT can name it) ----------
# On run completion (success OR fail, after qwen reloads) the driver replicates $OUT to a replica.
# REPLICA_TARGET set → offsite rsync; unset → a box-local second dir + an offsite-pending note.
RUN_TS="$(date +%Y%m%dT%H%M%S)"
REPLICA_LOCAL="${REPLICA_LOCAL:-$HOME/parity-artifact-replica/official-window-$RUN_TS}"
if [ -n "${REPLICA_TARGET:-}" ]; then REPLICA_DESC="$REPLICA_TARGET (offsite, rsync -az)"; else REPLICA_DESC="$REPLICA_LOCAL (box-local; offsite pull pending — P-3)"; fi

mkdir -p "$OUT"
: > "$OUT/run.log"
log() { echo "$@" | tee -a "$OUT/run.log"; }

# Anti-stale wipe: any file present at REPORT time was written THIS run; a failed leg leaves its
# table ABSENT → the MISSING marker fires, never a fabricated pass.
rm -rf "$PARITY_OUT" "$FMAP_OUT" "$PROBE_OUT" "$OUT/selftest" 2>/dev/null || true
rm -f "$OUT"/*.table.txt "$REPORT" 2>/dev/null || true

# shellcheck source=scripts/parity-lib.sh
. "$HERE/parity-lib.sh"
# shellcheck source=scripts/official-lib.sh
. "$HERE/official-lib.sh"

OFFICIAL_COMMIT="$(official_commit_sha40 "$G/mlxfast-bench")"

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

# --- assemble the BOX-CALIBRATED official golden (RULING 1), deterministically, if absent ------
# submit-1024 (the stale-baseline SOURCE + the Leg-2 band-failure fixture) must be present. The
# assembler verifies its pin, then replaces ONLY the two baselines → the calibrated golden whose
# sha256/bytes deterministically match OFFICIAL_PIN_SHA/OFFICIAL_PIN_BYTES (checked by the gate).
[ -r "$SUBMIT1024_GOLDEN" ] || { log "submit-1024 golden not readable: $SUBMIT1024_GOLDEN — aborting."; exit 5; }
if [ ! -f "$OFFICIAL_GOLDEN" ]; then
  [ -r "$ASSEMBLE" ] || { log "calibrated-golden assembler not found: $ASSEMBLE — aborting."; exit 5; }
  log "=== assemble calibrated official golden @ $(date) — $OFFICIAL_GOLDEN ==="
  if ! env SRC_PIN_SHA="$SUBMIT1024_PIN_SHA" SRC_PIN_BYTES="$SUBMIT1024_PIN_BYTES" \
       bash "$ASSEMBLE" "$SUBMIT1024_GOLDEN" "$OFFICIAL_GOLDEN" 2>&1 | tee -a "$OUT/run.log"; then
    log "calibrated golden assembly failed — aborting."; exit 4
  fi
fi
[ -r "$OFFICIAL_GOLDEN" ] || { log "official golden not readable: $OFFICIAL_GOLDEN — aborting."; exit 5; }
[ -r "$GEN" ] || { log "failure-corpus generator not readable: $GEN — aborting."; exit 5; }
command -v jq >/dev/null || { log "jq required — aborting."; exit 5; }
command -v sandbox-exec >/dev/null 2>&1 || { log "sandbox-exec not found — official runs need Seatbelt; aborting."; exit 5; }

# --- golden pin+load gate (delegates to benchctl validate-golden; official needs the oracle) ---
log "=== golden gate @ $(date) — $OFFICIAL_GOLDEN ==="
if ! parity_validate_golden "$BENCHCTL" "$OFFICIAL_GOLDEN" "$OFFICIAL_PIN_SHA" "$OFFICIAL_PIN_BYTES" 2>&1 | tee -a "$OUT/run.log"; then
  log "official golden rejected — aborting."; exit 4
fi

# --- differ self-test (GPU-FREE), BEFORE we touch qwen ---------------------------------
DIFFER_VERSION="$("$BENCHCTL" parity-diff --version 2>/dev/null || echo unknown)"
differ_selftest() {
  local d="$OUT/selftest"; mkdir -p "$d"
  "$BENCHCTL" parity-diff --emit-sample > "$d/a.json" 2>/dev/null || { log "S0: --emit-sample failed"; return 1; }
  # Pin the sample fields this self-test mutates: a DETERMINISTIC one (case_count) and a TIMING one
  # (benchmark_wall_seconds). If a differ/schema drift dropped either, abort with a clear message
  # rather than silently mutating an absent field and mis-reading the waiver behaviour.
  python3 -c "import json,sys;m=json.load(open('$d/a.json')).get('metrics',{});sys.exit(0 if ('case_count' in m and 'benchmark_wall_seconds' in m) else 1)" \
    || { log "S0: --emit-sample missing case_count/benchmark_wall_seconds — differ schema drift; aborting."; return 1; }
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

# --- source qwen-service.sh + verify functions ----------------------------------------
QWEN_SVC="$PAR/qwen-service.sh"
[ -f "$QWEN_SVC" ] || { log "qwen-service.sh not found at $QWEN_SVC — aborting."; exit 6; }
# shellcheck source=/dev/null
. "$QWEN_SVC"
for fn in qwen_unload qwen_reload; do
  command -v "$fn" >/dev/null 2>&1 || { log "qwen-service.sh did not define $fn() — aborting."; exit 6; }
done

# --- reentrant cleanup trap: reload qwen exactly once, THEN replicate the artifacts (P-3) ------
# Replication runs AFTER qwen reloads and on EVERY exit path (success OR fail), and NEVER fails the
# run (official_replicate_artifacts always returns 0; a replica error is logged only).
_CLEANED=0
cleanup() {
  [ "$_CLEANED" = "1" ] && return 0; _CLEANED=1
  log "=== RELOAD qwen (cleanup) @ $(date) ==="; qwen_reload
  log "=== replicate artifacts (P-3) @ $(date) — $REPLICA_DESC ==="
  official_replicate_artifacts "$OUT" "$REPLICA_LOCAL" 2>&1 | tee -a "$OUT/run.log"
}
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

log "=== UNLOAD qwen @ $(date) ==="
qwen_unload

COMMON_ENV=(BENCHCTL="$BENCHCTL" ENGINE="$ENGINE" SWIFT="$SWIFT" WEIGHTS="$WEIGHTS" \
  OFFICIAL_GOLDEN="$OFFICIAL_GOLDEN" OFFICIAL_COMMIT="$OFFICIAL_COMMIT" \
  SWIFT_REPO_ROOT="$SWIFT_REPO_ROOT" DIFF_CMD="$DIFF_CMD")

# ======================================================================================
# Leg 1+3 — official parity (pre-judge score + det-fields) + artifact byte-rows
# ======================================================================================
log "=== LEG 1+3: official-parity @ $(date) — $PAIRS pairs ==="
mkdir -p "$PARITY_OUT"
OP_TABLE="$OUT/official-parity.table.txt"; OP_STDERR="$OUT/official-parity.stderr.txt"
env "${COMMON_ENV[@]}" OUT="$PARITY_OUT" PAIRS="$PAIRS" \
  OFFICIAL_PIN_SHA="$OFFICIAL_PIN_SHA" OFFICIAL_PIN_BYTES="$OFFICIAL_PIN_BYTES" \
  bash "$HERE/official-parity.sh" > "$OP_TABLE" 2> "$OP_STDERR"
OP_RC=$?
if [ "$OP_RC" -ne 0 ]; then log "LEG 1+3: official-parity rc=$OP_RC (non-PASS — see below); continuing to render."; sed 's/^/  /' "$OP_STDERR" | tee -a "$OUT/run.log"; fi
OP_ROWS="$(awk -F' *\\| *' 'NF>=9 && $1!="pair" && $1 !~ /^-+$/ {print}' "$OP_TABLE")"
[ -n "$OP_ROWS" ] || { log "LEG 1+3: official-parity produced no table rows — aborting before REPORT."; exit 9; }

# ======================================================================================
# Leg 2 — official failure map (+ oracle-both-fail assertion + submit-1024 band-failure FIXTURE)
# ======================================================================================
# BAND_FIXTURE_GOLDEN (the STALE submit-1024) is handed to the failure map as a declared, expected
# fixture (RULING 2): both sides must FAIL the acceptance band identically and their blanked failed
# surfaces must byte-match (the differ agrees). It runs on the ORIGINAL stale baselines, NOT the
# calibrated golden, so it is passed explicitly.
log "=== LEG 2: official-failure-map @ $(date) ==="
mkdir -p "$FMAP_OUT"
FM_TABLE="$OUT/official-failure-map.table.txt"; FM_STDERR="$OUT/official-failure-map.stderr.txt"
env "${COMMON_ENV[@]}" GEN="$GEN" OUT="$FMAP_OUT" \
  BAND_FIXTURE_GOLDEN="$SUBMIT1024_GOLDEN" \
  BAND_FIXTURE_PIN_SHA="$SUBMIT1024_PIN_SHA" BAND_FIXTURE_PIN_BYTES="$SUBMIT1024_PIN_BYTES" \
  bash "$HERE/official-failure-map.sh" > "$FM_TABLE" 2> "$FM_STDERR"
FM_RC=$?
if [ "$FM_RC" -ne 0 ]; then log "LEG 2: official-failure-map rc=$FM_RC (non-PASS/assertion — see below); continuing to render."; sed 's/^/  /' "$FM_STDERR" | tee -a "$OUT/run.log"; fi
FM_ROWS="$(awk -F' *\\| *' 'NF>=4 && $1!="class" && $1 !~ /^-+$/ {print}' "$FM_TABLE")"
[ -n "$FM_ROWS" ] || { log "LEG 2: official-failure-map produced no table rows — aborting before REPORT."; exit 10; }
FM_ORACLE_ASSERT="$(grep -h 'oracle-corruption assertion\|ORACLE ASSERTION' "$FM_STDERR" | head -1)"
FM_BAND_ASSERT="$(grep -h 'band-failure fixture assertion\|BAND FIXTURE ASSERTION' "$FM_STDERR" | head -1)"

# ======================================================================================
# Leg 4 — #47 env-dump probe (GPU-free; runs in-window for convenience)
# ======================================================================================
log "=== LEG 4: official-env-probe (#47) @ $(date) ==="
mkdir -p "$PROBE_OUT"
EP_TABLE="$OUT/official-env-probe.table.txt"; EP_STDERR="$OUT/official-env-probe.stderr.txt"
env BENCHCTL="$BENCHCTL" WEIGHTS="$WEIGHTS" OFFICIAL_GOLDEN="$OFFICIAL_GOLDEN" \
  OFFICIAL_COMMIT="$OFFICIAL_COMMIT" OUT="$PROBE_OUT" \
  bash "$HERE/official-env-probe.sh" > "$EP_TABLE" 2> "$EP_STDERR"
EP_RC=$?
if [ "$EP_RC" -ne 0 ]; then log "LEG 4: official-env-probe rc=$EP_RC (non-PASS — see below); continuing to render."; sed 's/^/  /' "$EP_STDERR" | tee -a "$OUT/run.log"; fi
EP_ROWS="$(awk -F' *\\| *' 'NF>=3 && $1!="var (bucket)" && $1 !~ /^-+$/ {print}' "$EP_TABLE")"

log "=== battery done; qwen reloads on exit @ $(date) ==="

# --- ONE combined REPORT.md ------------------------------------------------------------
COMMIT="$(cd "$G/mlxfast-bench" 2>/dev/null && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
{
  echo "# B-3 official-parity window — REPORT"
  echo
  echo "Run \`$(date)\` · benchctl \`$COMMIT\` · differ \`$DIFF_CMD\` · differ-version \`$DIFFER_VERSION\` · pairs \`$PAIRS\`"
  echo "Golden \`$(basename "$OFFICIAL_GOLDEN")\` pin \`${OFFICIAL_PIN_SHA:0:12}…\` (${OFFICIAL_PIN_BYTES} B) · official commit \`${OFFICIAL_COMMIT:0:12}…\`"
  echo "**Golden provenance — PARITY-TEST-ONLY (RULING 1).** BOX-CALIBRATED official golden, assembled"
  echo "deterministically from \`submit-1024.json\` (pin \`${SUBMIT1024_PIN_SHA:0:12}…\`, ${SUBMIT1024_PIN_BYTES} B) by"
  echo "\`scripts/assemble-official-golden.sh\` — ONLY the two \`benchmark\` baselines changed so the box-measured"
  echo "speeds PASS the official gates (0.95 floors; prefill ±5% / decode +2%/−5% bands); every other byte is"
  echo "identical to submit-1024. **NEVER an organizer/ranking golden** — label lives in \`$(basename "$OFFICIAL_GOLDEN").provenance.txt\`"
  echo "+ \`.manifest.json\` + matrix §8 (the golden top level is \`deny_unknown_fields\`, so no in-band \`_provenance\`)."
  echo "**Replica (P-3).** $REPLICA_DESC."
  echo
  echo "**Mode.** benchctl \`iterate --mode official\` vs DIRECT \`mlxfast-swift benchmark\` (official is"
  echo "ENV-driven — no \`--official\` flag on the trusted binary; the STDOUT-sealed payload is the"
  echo "pre-judge parity unit). NOT the protected benchmark.sh workflow. **JUDGE-LESS**: benchd is"
  echo "judge-free, so \`semantic_gpqa_*\`/\`gpqa_ttft_*\` are 0/\"\" on both sides and match trivially"
  echo "(GPQA option-(b) ruling, §15)."
  echo
  echo "**Lock policy (measurement condition).** $LOCK_POLICY."
  echo "**Sandbox.** Both sides run their worker under a Seatbelt profile built by the SAME builder"
  echo "(\`official_seatbelt_profile\` == \`bench_runner::build_seatbelt_profile\`); benchctl"
  echo "self-generates its (engine-pinned) profile, the direct-swift side is handed an identically"
  echo "constructed (swift-worker-pinned) profile — same policy, per-side exec literal."
  echo
  echo "## Leg 1 — official parity (pre-judge score + deterministic fields; TIMING within band, JUDGE-LESS)"
  echo "| pair | bc.passed | sw.passed | det-fields |"
  echo "|---|---|---|---|"
  printf '%s\n' "$OP_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s | %s |\n",$1,$2,$3,$4}'
  echo
  echo "Deterministic surface via \`$DIFF_CMD\` (timing/environmental fields waived by the bucket"
  echo "policy → within-band; the sealed pre-judge score matches). A missing sealed score renders"
  echo "TOOL-ERR, never a silent pass."
  echo
  echo "## Leg 3 — artifact byte-rows (score.json / .sha256 / 9-field integrity / exit)"
  echo "| pair | score-name | .sha256 | integrity | exit | overall |"
  echo "|---|---|---|---|---|---|"
  printf '%s\n' "$OP_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s | %s | %s | %s |\n",$1,$5,$6,$7,$8,$9}'
  echo
  echo "Score naming (\`score.json\` both), \`.sha256\` sidecar (true hash of its own score), 9-field"
  echo "\`benchmark-integrity.json\` (key-set + deterministic VALUES; \`score_sha256\` +"
  echo "\`transform_source_sha256\` EXCEPTED as in facade-leg — timing-bearing payload + Swift-fresh vs"
  echo "marker source hash), and exact exit codes. benchd is the sole writer of all three on its side;"
  echo "the direct-swift side is sealed in the trusted shell (\`official-lib.sh\`) exactly as benchmark.sh."
  echo
  echo "## Leg 2 — official failure map (incl. the oracle class local couldn't test + submit-1024 band-fail fixture)"
  echo "| class | bc.passed | sw.passed | shared-surface diff |"
  echo "|---|---|---|---|"
  printf '%s\n' "$FM_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s | %s |\n",$1,$2,$3,$4}'
  echo
  echo "**Oracle assertion.** ${FM_ORACLE_ASSERT:-（assertion line missing — see official-failure-map.stderr.txt）}"
  echo "benchmark-oracle corruption MUST fail BOTH sides (a both-PASS is a harness failure — the run"
  echo "aborts). primary/anchor/free-run exercise official's FULL correctness scope. A"
  echo "manifest-declared divergence renders DECLARED(#nn), never FAIL."
  echo
  echo "**Band-failure fixture (RULING 2).** ${FM_BAND_ASSERT:-（assertion line missing — see official-failure-map.stderr.txt）}"
  echo "The \`submit-1024-band\` row runs the STALE-baseline \`submit-1024.json\` official on both sides — a"
  echo "declared, EXPECTED truth-table cell: both sides FAIL the acceptance band identically (both"
  echo "\`.passed=False\`, both carry the band-failure signature, and the RULING-2-aligned blanked failed"
  echo "surfaces byte-match via the differ). A both-PASS / divergence there aborts the leg LOUD."
  echo
  echo "## Leg 4 — #47 closing evidence (sanitized allowlist-only child env; GPU-free)"
  if [ -n "$EP_ROWS" ]; then
    echo "| var (bucket) | verdict | detail |"
    echo "|---|---|---|"
    printf '%s\n' "$EP_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s |\n",$1,$2,$3}'
  else
    echo "_(no probe rows — see official-env-probe.stderr.txt; a missing child-env capture is TOOL-ERR)_"
  fi
  echo
  echo "The benchctl \`iterate --mode local-iterate\` spawn delivered the child env via an env-dump worker"
  echo "shim (LOCAL mode applies the SAME B-1 env sanitizer as official but is NOT sandboxed deny-write, so"
  echo "the shim can record child-env.txt — the earlier \`--mode official\` probe TOOL-ERR'd precisely"
  echo "because the official sandbox correctly denies writes). Prefix-allowlist proven via \`LC_ALL\` (DYLD_"
  echo "not asserted). **Orthogonal + separately verified:** the official sandbox's deny-write is a distinct"
  echo "surface — the shim's inability to write under \`--mode official\` IS that sandbox working."
  echo
  echo "## Verdict"
  echo "- Leg 1+3 official parity/artifacts: $( [ "$OP_RC" -eq 0 ] && echo 'PASS — all pairs GREEN' || echo "NON-PASS (rc=$OP_RC) — see official-parity.stderr.txt" )"
  echo "- Leg 2 failure map + oracle + band-fail fixture: $( [ "$FM_RC" -eq 0 ] && echo 'PASS — oracle fails both sides; submit-1024 band-fails both sides identically; no undeclared FAIL' || echo "NON-PASS (rc=$FM_RC) — see official-failure-map.stderr.txt" )"
  echo "- Leg 4 #47 env probe (local-iterate): $( [ "$EP_RC" -eq 0 ] && echo 'PASS — allowlist-only child env' || echo "NON-PASS (rc=$EP_RC) — see official-env-probe.stderr.txt" )"
  echo
  echo "On green (all legs PASS), flips: §8 official row (CODE-VERIFIED → GPU-VERIFIED at parity) + §9 #47 row (PARTIAL → VERIFIED) — human-gated via the WINDOW-PENDING markers."
  echo "Artifacts: parity \`$PARITY_OUT/\`; failure-map \`$FMAP_OUT/\`; env-probe \`$PROBE_OUT/\`; logs \`$OUT/run.log\`."
} > "$REPORT"

log "=== REPORT written: $REPORT ==="
echo "----- REPORT.md -----"
cat "$REPORT"

# Overall exit: non-zero if any leg was non-PASS (qwen still reloads via the trap).
[ "$OP_RC" -eq 0 ] && [ "$FM_RC" -eq 0 ] && [ "$EP_RC" -eq 0 ]
