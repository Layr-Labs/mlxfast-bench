#!/bin/bash
# scripts/run-manual-test.sh — the ONE human-triggered parity driver (grade-A item 8).
#
# Takes the box GPU lock (HELD for the run), checks the box quiet, gates the golden pin,
# unloads qwen, runs the acceptance battery, reloads qwen (ALWAYS, via a reentrant trap),
# and writes REPORT.md:
#   leg 1 — 3-pair local-iterate parity + weights_hash gate on the REAL tree (item 7 hold);
#   leg 2 — the failure-map truth table, produced by COMPOSING scripts/failure-map.sh
#           (every corruption class, shared-surface field-diff), whose table we parse.
#
# NO scheduler, NO CI — a human runs this and reads the report (David: manual driver, full
# stop). GPU-touching: run ON the box. Paths default to the box layout; override via env.
# The differ is scripts/parity-diff.py today; §T4 flips DIFF_CMD to `benchctl parity-diff`
# once PR #66 lands (one knob, shared with failure-map.sh).
#
# Shared primitives (precheck / held GPU lock / golden gate) come from parity-lib.sh so
# this driver composes run-parity's checks instead of forking a second, drifting copy.
set -uo pipefail

HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
G="${MLXFAST_PARITY_GIT:-$HOME/mlxfast-parity-git}"
PAR="${MLXFAST_PARITY_HOME:-$HOME/mlxfast-parity}"
ENGINE="${ENGINE:-$G/mlxfast-engine/.build/release/mlxfast-engine}"
SWIFT="${SWIFT:-$G/mlxfast-challenge-dev/.build/release/mlxfast-swift}"
BENCHCTL="${BENCHCTL:-$G/mlxfast-bench/target/release/benchctl}"
GEN="${GEN:-$G/mlxfast-bench/scripts/gen-failure-corpus.py}"
FAILURE_MAP="${FAILURE_MAP:-$HERE/failure-map.sh}"
# ONE differ knob shape shared with failure-map.sh: a full command. §T4 CLOSER: the default is
# now `benchctl parity-diff` directly (the Rust verdict tool on main), skipping the parity-diff.py
# shim hop. The shim stays for one release for any external caller that hardcodes it.
DIFF_CMD="${DIFF_CMD:-$BENCHCTL parity-diff}"
WEIGHTS="${WEIGHTS:-$PAR/weights}"
# MODE knob (M-6): local-iterate | local-submit. MODE only changes the DEFAULTS for the
# golden/pin/thermal below — the existing GOLDEN/PIN_SHA/PIN_BYTES env overrides still win.
# The benchctl leg passes `--mode "$MODE"` and the Swift leg passes `--$MODE`.
MODE="${MODE:-local-iterate}"
case "$MODE" in
  local-iterate)
    DEF_GOLDEN="$G/golden/beefed.json"
    DEF_PIN_SHA="32045f7e97f9c1c5bbabf0333a1b28b16b27f8240bc8e14647292b4b33005ac4"
    DEF_PIN_BYTES="16940"
    # Cool-gate OFF on BOTH sides so the timed residual is a same-conditions number, not a
    # thermal asymmetry. benchctl OFF by RULING-A3 default; Swift forced OFF via the env override.
    THERMAL="cool-gate OFF both sides (benchctl RULING-A3 default; swift MLXFAST_LOCAL_COOL_GATE=0)"
    ;;
  local-submit)
    DEF_GOLDEN="$G/golden/submit-1024.json"
    DEF_PIN_SHA="a482f223edaa5b0b58e6ef0d1d276122f1a4b43f81ca6af33184cc0a64e726c9"
    DEF_PIN_BYTES="20993"
    # Cool-gate ON by RULING for submit, but the box has NO temp reader → both native gates
    # no-op, so BOTH legs are measured UN-GATED and SYMMETRIC (declared, not accidental).
    THERMAL="cool-gate ON by RULING (submit), but the box has NO macmon/helper temp reader → benchctl's native gate SKIPS (CoolGateOutcome::Skipped, never a failure) and Swift's helper-gate no-ops → BOTH legs measured UN-GATED, SYMMETRIC (declared; not accidental ungated-vs-gated)"
    ;;
  *) echo "unknown MODE '$MODE' (want local-iterate|local-submit) — aborting." >&2; exit 2 ;;
esac
GOLDEN="${GOLDEN:-$DEF_GOLDEN}"
PIN_SHA="${PIN_SHA:-$DEF_PIN_SHA}"
PIN_BYTES="${PIN_BYTES:-$DEF_PIN_BYTES}"
# The driver HOLDS the real GPU-exclusive lock (the one gpu_run.py takes per-call) for its
# whole lifetime — so inner battery calls run the binaries DIRECTLY, never wrapped in
# gpu_run.py, whose per-call LOCK_NB would self-conflict on the lock we already hold (and
# is redundant: gpu_run.py is a pure lock+exec mutex with no env/GPU setup). Holding it for
# the run also fail-fast-blocks any other gpu_run.py-wrapped job → true whole-run
# exclusivity, no interleave perturbing thermal/timing between calls.
LOCK="${MLXFAST_GPU_LOCK:-/tmp/mtplx-gpu-exclusive.lock}"
OUT="${OUT:-$G/golden/manual-test}"
REPORT="$OUT/REPORT.md"
# Thermal policy is EXPLICIT + SYMMETRIC per mode ($THERMAL, set from MODE above) and printed
# into REPORT.md (#67 review). Both legs are always measured under the SAME policy so the timed
# residual is a same-conditions number, never a cool-gate asymmetry.
# Lock policy is a MEASUREMENT CONDITION now (#67 watch-list): the driver holds the real
# gpu-exclusive lock (fd 9) for the whole run and calls benchctl/swift DIRECTLY — inner calls
# are unwrapped (no per-call gpu_run.py), which both avoids gpu_run.py's LOCK_NB self-conflict
# and gives whole-run GPU exclusivity (nothing interleaves between calls). Both legs run this way.
LOCK_POLICY="outer-hold / inner-direct — driver holds the gpu-exclusive lock (fd 9) for the whole run; both legs' inner calls are unwrapped (no gpu_run.py) → whole-run exclusivity"

mkdir -p "$OUT"
: > "$OUT/run.log"
log() { echo "$@" | tee -a "$OUT/run.log"; }

# Anti-fabrication (#67 red-team Finding 1): $OUT is persistent, and binaries write their score
# via --score-path (not a shell redirect), so a prior run's score.*.json would survive a binary
# that FAILS this run before rewriting it — rendering a stale passing number. Wipe every prior
# per-run artifact at startup, so any file present at REPORT time was written THIS run; a failed
# binary leaves its score ABSENT → the MISSING marker fires, never a fabricated pass.
rm -f "$OUT"/score.bc.*.json "$OUT"/score.swift.*.json "$OUT"/diff.*.txt "$OUT"/verdict.*.txt \
      "$OUT"/bc.*.log "$OUT"/sw.*.log "$OUT"/failure-map.table.txt "$OUT"/failure-map.stderr.txt 2>/dev/null || true
rm -rf "$OUT"/failure-map "$OUT"/selftest 2>/dev/null || true

# shellcheck source=scripts/parity-lib.sh
. "$HERE/parity-lib.sh"

# --- value extractors: print the value or an explicit marker, NEVER a fabricated 0.0000 ---
score_raw() { python3 -c "import json,sys
d=json.load(open(sys.argv[1]));s=d.get('score')
print('NULL' if s is None else s)" "$1" 2>/dev/null || echo "MISSING"; }
# 4-dp for the report table; passes MISSING/NULL/ERR through verbatim (no %.4f on a marker).
fmt_score() {
  local f="$1" v
  [ -s "$f" ] || { printf 'MISSING'; return; }
  v="$(python3 -c "import json,sys
d=json.load(open(sys.argv[1]));s=d.get('score')
print('NULL' if s is None else format(float(s), '.4f'))" "$f" 2>/dev/null)" || { printf 'ERR'; return; }
  printf '%s' "${v:-ERR}"
}
passed() { python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get('passed'))" "$1" 2>/dev/null || echo MISSING; }
whash() { python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['metrics']['weights_hash'])" "$1" 2>/dev/null || echo MISSING; }
verdict() { grep -h '^PARITY:' "$1" 2>/dev/null | sed 's/PARITY: //' | head -1; }

# --- take + HOLD the GPU lock first (fd 9 stays open for this script's lifetime) --------
# CRITICAL: call the lock helper DIRECTLY — never via `$(...)` or a `| tee` pipe, which
# would run `exec 9>` in a subshell and release the lock the instant that subshell exits
# (the very bug the #67 review flagged). Its diagnostics print to the terminal; the
# outcome + any abort reason go to run.log.
log "=== take GPU lock @ $(date) ==="
parity_take_gpu_lock "$LOCK"; LOCK_RC=$?
if [ "$LOCK_RC" -ne 0 ]; then log "GPU lock unavailable (rc=$LOCK_RC) — aborting; re-run when the box is free."; exit 3; fi
log "GPU lock held (fd 9) for the run."

# --- box-quiet precheck (composed from parity-lib) --------------------------------------
if ! parity_precheck 2>&1 | tee -a "$OUT/run.log"; then
  log "box not quiet — aborting (re-run when quiet)."; exit 3
fi

# --- golden pin+load gate: delegate to benchctl validate-golden -------------------------
log "=== golden gate @ $(date) ==="
if ! parity_validate_golden "$BENCHCTL" "$GOLDEN" "$PIN_SHA" "$PIN_BYTES" 2>&1 | tee -a "$OUT/run.log"; then
  log "golden rejected — aborting."; exit 4
fi

for b in "$ENGINE" "$SWIFT" "$BENCHCTL"; do
  [ -x "$b" ] || { log "missing binary: $b — aborting."; exit 5; }
done

# --- T3: differ self-tests (S1/S2/S3), GPU-FREE, BEFORE we touch qwen ---------------------
# Prove the wired differ ($DIFF_CMD) is correct + its exit contract holds before spending a GPU
# window on it. Uses `benchctl parity-diff --emit-sample` (a schema-current complete payload), so
# no fixtures to drift and no engine needed:
#   S1  identical pair            → PARITY PASS  (exit 0)
#   S2  one DET field changed     → PARITY FAIL  (exit 1)
#   S3  missing input file        → tool error   (exit ∉ {0,1})
# Any deviation aborts here — a broken/stale differ never reaches the battery.
differ_selftest() {
  local d="$OUT/selftest"; mkdir -p "$d"
  "$BENCHCTL" parity-diff --emit-sample > "$d/a.json" 2>/dev/null || { log "S0: --emit-sample failed"; return 1; }
  python3 -c "import json;x=json.load(open('$d/a.json'));x['metrics']['case_count']=x['metrics'].get('case_count',0)+1;json.dump(x,open('$d/b.json','w'))" || return 1
  $DIFF_CMD "$d/a.json" "$d/a.json" >/dev/null 2>&1; local s1=$?
  $DIFF_CMD "$d/a.json" "$d/b.json" >/dev/null 2>&1; local s2=$?
  $DIFF_CMD "$d/a.json" "$d/nonexistent.json" >/dev/null 2>&1; local s3=$?
  log "differ self-test: S1(identical)=$s1 expect 0 · S2(det-diff)=$s2 expect 1 · S3(io-err)=$s3 expect ∉{0,1}"
  [ "$s1" = 0 ] && [ "$s2" = 1 ] && { [ "$s3" != 0 ] && [ "$s3" != 1 ]; }
}
log "=== T3 differ self-test (differ=$DIFF_CMD; $("$BENCHCTL" parity-diff --version 2>/dev/null || echo unknown)) @ $(date) ==="
if ! differ_selftest; then
  log "differ self-test FAILED — aborting before the GPU window (differ is broken/stale)."; exit 8
fi

# --- source qwen-service.sh and VERIFY the functions we depend on exist -----------------
QWEN_SVC="$PAR/qwen-service.sh"
[ -f "$QWEN_SVC" ] || { log "qwen-service.sh not found at $QWEN_SVC — aborting (cannot manage qwen)."; exit 6; }
# shellcheck source=/dev/null
. "$QWEN_SVC"
for fn in qwen_unload qwen_reload; do
  command -v "$fn" >/dev/null 2>&1 || { log "qwen-service.sh did not define $fn() — aborting (unsafe to unload)."; exit 6; }
done

# --- reentrant cleanup trap: reload qwen exactly once, on normal exit OR a signal --------
# We hold fd 9 (the GPU lock) THROUGH qwen_reload deliberately: qwen_reload restarts qwen via
# `launchctl bootstrap`/`kickstart`, so the long-lived qwen daemon is spawned by LAUNCHD, NOT as
# a child of this shell — it does not inherit fd 9, so there is no lock leak (Fable checkpoint
# Finding 4, refuted). launchctl/curl are short-lived children that exit immediately. Holding the
# lock through reload keeps the whole window serialized (no concurrent driver races our reload).
_CLEANED=0
cleanup() { [ "$_CLEANED" = "1" ] && return 0; _CLEANED=1; log "=== RELOAD qwen (cleanup) @ $(date) ==="; qwen_reload; }
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

log "=== UNLOAD qwen @ $(date) ==="
qwen_unload

# --- leg 1: 3-pair parity + weights_hash gate (thermal policy explicit + symmetric) -----
log "=== LEG 1: 3-pair parity @ $(date) — mode $MODE — $THERMAL ==="
# Swift cool-gate env is MODE-derived: local-iterate forces the gate OFF (MLXFAST_LOCAL_COOL_GATE=0
# is the iterate override); local-submit passes NOTHING so the Swift submit leg keeps its
# helper-based gate as a no-op (no helper set) — measured un-gated, symmetric with benchctl's
# native gate skipping for want of a temp reader.
case "$MODE" in local-iterate) SWIFT_COOL_KV="MLXFAST_LOCAL_COOL_GATE=0";; *) SWIFT_COOL_KV="";; esac
for i in 1 2 3; do
  # Direct calls — the driver holds the GPU-exclusive lock (no gpu_run.py wrapper).
  # benchctl: gate per RULING for $MODE (native gate skips w/o a temp reader). Pin-gated golden.
  # On a non-zero exit, drop any partial/absent score so REPORT shows MISSING, not a stale/bogus
  # number (belt-and-suspenders with the startup wipe): the binary owns --score-path, so a crash
  # mid-write must not leave a half-file that parses.
  "$BENCHCTL" iterate --engine "$ENGINE" --weights "$WEIGHTS" --golden "$GOLDEN" \
    --golden-sha256 "$PIN_SHA" --golden-bytes "$PIN_BYTES" --mode "$MODE" \
    --score-path "$OUT/score.bc.$i.json" > "$OUT/bc.$i.log" 2>&1 \
    || { rc=$?; log "pair $i: benchctl FAILED (exit $rc; see bc.$i.log)"; rm -f "$OUT/score.bc.$i.json"; }
  # Swift: MODE-derived cool-gate env (iterate forces OFF; submit leaves the helper no-op — the
  # empty $SWIFT_COOL_KV word-splits away, so `env` just execs swift with no extra var).
  env $SWIFT_COOL_KV "$SWIFT" benchmark --weights "$WEIGHTS" --golden "$GOLDEN" \
    --score-path "$OUT/score.swift.$i.json" "--$MODE" > "$OUT/sw.$i.log" 2>&1 \
    || { rc=$?; log "pair $i: swift FAILED (exit $rc; see sw.$i.log)"; rm -f "$OUT/score.swift.$i.json"; }
  # Capture the differ's exit: only 0 (PASS) / 1 (FAIL) are real verdicts. Anything else — the
  # shim's SHIM_TOOL_ERR=9, or benchctl parity-diff's 2 (usage) / 3 (IO) — is a TOOL error, marked
  # as such in the verdict column rather than rendering a blank cell (Fable checkpoint Finding 5c).
  $DIFF_CMD "$OUT/score.bc.$i.json" "$OUT/score.swift.$i.json" > "$OUT/diff.$i.txt" 2>&1; drc=$?
  vd="$(verdict "$OUT/diff.$i.txt")"
  case "$drc" in 0|1) [ -n "$vd" ] || vd="TOOL-ERR (differ exit $drc, no PARITY line)";; *) vd="TOOL-ERR (differ exit $drc)";; esac
  printf '%s' "$vd" > "$OUT/verdict.$i.txt"
  log "pair $i: bc=$(score_raw "$OUT/score.bc.$i.json") sw=$(score_raw "$OUT/score.swift.$i.json") $vd"
done

# --- leg 2: failure-map truth table — COMPOSE scripts/failure-map.sh, parse its table ----
log "=== LEG 2: failure map (via failure-map.sh) @ $(date) — mode $MODE — $THERMAL ==="
FM_OUT="$OUT/failure-map"; mkdir -p "$FM_OUT"
FM_TABLE="$OUT/failure-map.table.txt"
FM_STDERR="$OUT/failure-map.stderr.txt"
# GPU="" => failure-map runs its inner calls DIRECTLY: the driver already holds the
# GPU-exclusive lock for the whole run, so re-wrapping in gpu_run.py would self-conflict.
# stdout (the truth table) and stderr (gen output, diagnostics) go to SEPARATE files so a
# stderr line can never be mis-parsed as a truth-table row (#67 red-team Finding 2).
# MODE passes through: failure-map's leg-2 runs `benchctl iterate --mode $MODE` and
# `mlxfast-swift benchmark --$MODE` on the (corrupted) mode golden, and derives the Swift
# cool-gate env from MODE itself (iterate forces OFF, submit leaves the helper no-op).
ENGINE="$ENGINE" SWIFT="$SWIFT" BENCHCTL="$BENCHCTL" WEIGHTS="$WEIGHTS" GEN="$GEN" \
  DIFF_CMD="$DIFF_CMD" GOLDEN="$GOLDEN" OUT="$FM_OUT" GPU="" MODE="$MODE" \
  bash "$FAILURE_MAP" > "$FM_TABLE" 2> "$FM_STDERR" || {
    log "failure-map.sh FAILED — aborting before REPORT."; sed 's/^/  /' "$FM_STDERR" | tee -a "$OUT/run.log"; exit 7;
  }
# Build the truth table by ANCHORING on the corpus manifest: for each manifest class, extract
# EXACTLY its row from failure-map's stdout (matched on the class name in field 1). A missing
# row aborts (dropped class) and a stray/injected line can't be counted (it isn't a manifest
# class), so the table can neither drop a real class nor fabricate a bogus one.
MANIFEST="$FM_OUT/corpus/manifest.json"
[ -f "$MANIFEST" ] || { log "corpus manifest missing ($MANIFEST) — aborting."; exit 7; }
N_MANIFEST="$(python3 -c "import json;print(len(json.load(open('$MANIFEST'))['variants']))" 2>/dev/null)"
# N_MANIFEST must be a positive integer, else the -lt/-ne guards below silently no-op (#67
# red-team: a non-numeric value makes `[ "" -lt 5 ]` return non-zero → the `if` reads false).
case "$N_MANIFEST" in ''|*[!0-9]*) log "corpus manifest variant count not a number ('$N_MANIFEST') — aborting."; exit 7;; esac
FM_MIN_CLASSES="${FM_MIN_CLASSES:-5}"
if [ "$N_MANIFEST" -lt "$FM_MIN_CLASSES" ]; then
  log "corpus too small: $N_MANIFEST variants < floor $FM_MIN_CLASSES — aborting (won't publish a thin table)."; exit 7
fi
FM_ROWS=""; N_ROWS=0
# Bind each class to its manifest `declared` issue ref (empty for ordinary classes). The emitter
# RE-ENFORCES the no-undeclared-cells rule on top of failure-map's rendering (defense in depth):
# a declared class's FAIL is rewritten DECLARED(#nn) here too, so an older/uncooperative
# failure-map that still emitted FAIL cannot leak a declared FAIL into the REPORT.
while IFS=$'\t' read -r cls declared; do
  row="$(awk -F' *\\| *' -v c="$cls" '$1==c {print; exit}' "$FM_TABLE")"
  if [ -z "$row" ]; then
    log "leg-2: no truth-table row for manifest class '$cls' — aborting (failure-map dropped a class)."; exit 7
  fi
  bcp="$(printf '%s' "$row" | awk -F' *\\| *' '{print $2}')"
  swp="$(printf '%s' "$row" | awk -F' *\\| *' '{print $3}')"
  vd="$(printf '%s' "$row" | awk -F' *\\| *' '{print $4}')"
  # DECLARED(#nn), never FAIL, for a signed/audited class (link mandatory, from the manifest).
  if [ "$vd" = "FAIL" ] && [ -n "$declared" ]; then vd="DECLARED($declared)"; fi
  FM_ROWS="$FM_ROWS| $cls | $bcp | $swp | ${vd:-（no score）} |
"
  N_ROWS=$((N_ROWS + 1))
  log "failure $cls: bc=$bcp sw=$swp ${vd:-（no score）}"
done < <(python3 -c "import json;[print(x['class']+'\t'+(x.get('declared') or '')) for x in json.load(open('$MANIFEST'))['variants']]")
if [ "$N_ROWS" -ne "$N_MANIFEST" ]; then
  log "leg-2 row-count mismatch: emitted $N_ROWS rows vs $N_MANIFEST manifest variants — aborting."; exit 7
fi

log "=== battery done; qwen reloads on exit @ $(date) ==="

# --- REPORT.md --------------------------------------------------------------------------
COMMIT="$(cd "$G/mlxfast-bench" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
# #70 (T5): pin the DIFFER VERSION into every declaration, so a score is tied to the exact
# verdict logic + roster/tolerance surface that validated it (auto-bumps on a bucket change).
DIFFER_VERSION="$("$BENCHCTL" parity-diff --version 2>/dev/null || echo unknown)"
{
  echo "# Manual parity driver — REPORT"
  echo
  echo "Run \`$(date)\` · mode \`$MODE\` · benchctl \`$COMMIT\` · golden pin \`${PIN_SHA:0:12}…\` (${PIN_BYTES} B) · differ \`$DIFF_CMD\` · differ-version \`$DIFFER_VERSION\`"
  echo
  echo "**Thermal policy (explicit, per mode).** $MODE: $THERMAL. Both legs"
  echo "are measured under this symmetric policy — the timed residual is a"
  echo "same-conditions number, not a cool-gate asymmetry."
  echo
  echo "**Lock policy (measurement condition).** $LOCK_POLICY."
  echo
  echo "## Leg 1 — 3-pair parity ($MODE; score = ×baseline)"
  echo "| pair | benchctl | swift | verdict | weights_hash (real tree) |"
  echo "|---|---|---|---|---|"
  for i in 1 2 3; do
    printf '| %d | %s | %s | %s | %s… |\n' "$i" \
      "$(fmt_score "$OUT/score.bc.$i.json")" "$(fmt_score "$OUT/score.swift.$i.json")" \
      "$(cat "$OUT/verdict.$i.txt" 2>/dev/null || echo MISSING)" "$(whash "$OUT/score.bc.$i.json" | cut -c1-12)"
  done
  echo
  echo "weights_hash gated on the real 14-file tree every pair (item 7 hold)."
  echo
  echo "## Leg 2 — failure-map truth table ($N_ROWS/$N_MANIFEST classes; via failure-map.sh)"
  echo "| class | benchctl passed | swift passed | diff |"
  echo "|---|---|---|---|"
  printf '%s' "$FM_ROWS"
  echo
  echo "Each of the $N_MANIFEST manifest classes was matched to exactly one truth-table row (a missing row aborts the run); a stray/injected line is not a manifest class and cannot appear."
  echo "Per-pair field diffs + logs in \`$OUT/\`; failure-map artifacts in \`$FM_OUT/\`."
} > "$REPORT"

log "=== REPORT written: $REPORT ==="
echo "----- REPORT.md -----"
cat "$REPORT"
