#!/bin/bash
# scripts/official-paired.sh — PROOF A: the Option-A THREE-SEAM paired IDENTITY chain.
#
# REWRITTEN for finding 15 (the `--paired` monolith is gone). The paired ranked flow is now three
# trusted seams the driver runs in order, asserting each seam's artifact against the next
# (docs/paired-flow-design-note.md "The three seams" + §A-1/A-2/A-3):
#
#   Seam 1 — GATES ($GATES_PRODUCER):  <gate-cmd> --official, CHECK_GATES=1 SKIP_TIMED=1 → gates-score.json
#            (partial_result=true; DRAFT-WF @1423-1424/@1439/@1493-1506).
#   Seam 2 — MEASURE-JOB:     benchctl measure-job --candidate <WS> --baseline <WS> (IDENTITY: the
#            SAME on-box built workspace both legs) --golden --contract --tokens --mtp-depth --min-pairs 3
#            --target-pairs 4 --tag --out [--weights <DIR>] → results.json (+ benchmark-integrity.
#            results.json). Each leg's engine is resolved as <WS>/.build/release/mlxfast-engine and
#            spawned as `<engine> runtime-worker --weights <WEIGHTS>` (WS and WEIGHTS are DIFFERENT
#            paths). --weights is an OPTIONAL OVERRIDE (R6): when unset, measure-job derives the
#            weights DIR from the env QMTP_TARGET_DIR. Because both legs are the same workspace
#            the raw ratio ≈ 1.00.
#   Seam 3 — OVERLAY (LOCAL): benchctl overlay-timing --gates-score --results --score-path
#            [--integrity] → merged score.json (+ .sha256), partial_result flipped false,
#            scoring_mode discriminator, integrity re-anchored over the merged bytes.
#            RUN WITH CWD == $GATES_WS (David ruling 2026-08-26) — the workspace root the seam-1
#            producer hashed. benchd re-resolves the 9-root harness identity there and REFUSES the
#            merge if it differs from the gates leg's metrics.harness_hash (the between-phase
#            TOCTOU gate). See the GATES_WS capture block below for the per-producer derivation.
#
# One row per assertion (PASS/FAIL accumulator like official-failure-map.sh); RESULT PASS/FAIL +
# exit 0/1. Anti-fabrication: a MISSING/empty artifact at ANY seam renders a TOOL-ERR FAIL row,
# NEVER a silent pass — the driver cannot fabricate a green.
#
# IDENTITY TOLERANCE BAND (IDENTITY_BAND, default 0.10): both legs are the SAME workspace, so the
# only source of a decode-speedup deviation from 1.00 is thermal / measurement noise. Finding-15
# discipline: the band is asserted as a SUBSET of the [floor, ceiling] READ FROM the emitted
# score.json (never restated literals) — a mis-wired denominator (e.g. a 2x on an identity run)
# lands outside |raw−1.0|≤band and FAILS loud, while the band itself is proven to sit inside the
# artifact's own floor/ceiling so a green identity score is coherent with the paired gate.
# WHY 0.10 (not the 0.15 sketched in the task): the paired FLOOR read from the artifact is 0.90, so
# the widest band whose LOWER edge (1−band) still clears the floor is 1−0.90 = 0.10. A 0.15 band
# reaches 0.85 < 0.90 → NOT ⊆ [floor,ceiling] (an identity score of 0.87 would be "in band" yet the
# overlay would null it at the floor). The band is therefore DERIVED to be coherent with the
# artifact's own floor rather than a restated literal that contradicts the gate; override if desired.
#
# NEGATIVE CONTROL (NEG_CONTROL=1, default on; CHEAP — no GPU): seam 3 is re-run on a synthesized
# FLOOR-FAIL results.json (the real identity results with per-prompt/per-pair raw ratios rewritten
# to ~0.5, below the 0.90 floor). The overlay must null the score + exit nonzero — proving the
# driver cannot fabricate a green past a floor breach. This reuses the REAL seam-1 gates-score.json
# and needs no engine, so it always runs in-window.
#
# CONTRACT-DERIVED ASSUMPTIONS (stated so Proof A is honest about what it is first to exercise):
#   * PAIRED_WS is the on-box BUILT WORKSPACE measure-job spawns each leg's sandboxed worker from
#     (candidate==baseline==PAIRED_WS ⇒ identity). measure-job is WORKSPACE-based, not engine-binary
#     based; the live workspace→sandboxed-worker spawn is UNVERIFIED(measure-job) — Proof A is the
#     FIRST to exercise it end-to-end.
#   * CONTRACT is the track fixture (timed_prompt_pool + calibration); its on-box path + field reads
#     are UNVERIFIED(B-4).
#   * GATE_CMD (the reference challenge-dev benchmark.sh) is the DEFAULT seam-1 producer — the same
#     ranked chain the organizer runs, per RULING Q1a (David 2026-08-20, completion-gate sign-off);
#     see the GATES_PRODUCER selector block below for the reasoning. FACADE_CMD is benchd's OWN
#     scripts/benchmark.sh (the R22 full-parity facade): its `--official` honors
#     MLXFAST_BENCHMARK_CHECK_GATES/SKIP_TIMED and emits a partial_result=true gates-score via
#     `benchctl iterate --mode official` (gates-only), loading --weights DIRECTLY. It is the
#     explicit OPT-IN GATES_PRODUCER=facade, for PARITY TESTING — not the scoring default. Both
#     honor the same env contract.
#
# Required env: BENCHCTL · OFFICIAL_GOLDEN · OUT · PAIRED_WS · CONTRACT · WEIGHTS (seam 1's direct
#               mlxfast-swift gates producer loads --weights; the WORKSPACE and the WEIGHTS are
#               DIFFERENT paths). R6: for seam 2 the same WEIGHTS is passed to measure-job as an
#               OPTIONAL OVERRIDE of its (contract-derivable) --weights — measure-job otherwise
#               derives the weights DIR from the env QMTP_TARGET_DIR (the draft's on-box source).
# Seam-1 env:   GATES_PRODUCER(benchmark-sh DEFAULT | facade opt-in | direct-swift fallback) · GATE_CMD(default
#               $SWIFT_REPO_ROOT/benchmark.sh) · GATE_CMD_SHA(optional sha-pin of GATE_CMD, R10) ·
#               SWIFT · SWIFT_REPO_ROOT · optional GATE_EXTRA_ENV (a VALIDATED KEY=VAL allowlist, R10)
# Seam-2 env:   GOLDENS(optional whitespace/newline pool of DISTINCT golden files → repeatable
#               --golden; else single OFFICIAL_GOLDEN) · QMTP_TARGET_DIR / QMTP_HEAD_DIR /
#               QMTP_CANDIDATE_HEAD_DIR (passed through to measure-job, R14) · MEASURE_EXTRA_ENV
#               (a VALIDATED KEY=VAL allowlist, R10)
# Optional:     OFFICIAL_COMMIT · TOKENS(512) · DEPTH(2, sent as --mtp-depth) · MIN_PAIRS(3, per-prompt) ·
#               TARGET_PAIRS(4, per-prompt) · TAG(qwen-mtp-paired-identity) · IDENTITY_BAND(0.10) · NEG_CONTROL(1)
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/official-lib.sh
. "$HERE/official-lib.sh"

: "${BENCHCTL:?set BENCHCTL}" "${OFFICIAL_GOLDEN:?set OFFICIAL_GOLDEN}" "${OUT:?set OUT}"
: "${PAIRED_WS:?set PAIRED_WS (the on-box built workspace; identity uses it for BOTH legs)}"
: "${CONTRACT:?set CONTRACT (the track fixture: timed_prompt_pool + calibration)}"
# WEIGHTS is required by seam 1 (the direct mlxfast-swift gates producer loads --weights). R6: for
# seam 2 it is passed to measure-job as an OPTIONAL OVERRIDE — measure-job's --weights is derivable
# from QMTP_TARGET_DIR, so run_measure only passes `--weights` when WEIGHTS is non-empty.
: "${WEIGHTS:?set WEIGHTS -- seam-1 gates producer loads it; also passed to measure-job as the R6 override}"
SWIFT_REPO_ROOT="${SWIFT_REPO_ROOT:-}"
GATE_CMD="${GATE_CMD:-${SWIFT_REPO_ROOT:+$SWIFT_REPO_ROOT/benchmark.sh}}"
: "${GATE_CMD:?set GATE_CMD (the reference benchmark.sh that honors --official + gates) or SWIFT_REPO_ROOT}"
TOKENS="${TOKENS:-512}"          # R13: default --tokens is 512 (was 128)
DEPTH="${DEPTH:-2}"              # R13: passed as --mtp-depth (>= 2; serial control is the depth-0 constant)
MIN_PAIRS="${MIN_PAIRS:-3}"
TARGET_PAIRS="${TARGET_PAIRS:-4}"
TAG="${TAG:-qwen-mtp-paired-identity}"
IDENTITY_BAND="${IDENTITY_BAND:-0.10}"   # 1 − floor(0.90); see header for the ⊆-coherence derivation
NEG_CONTROL="${NEG_CONTROL:-1}"
# seam-1 GATES_PRODUCER selector (KEEP all three producers):
#
# RULED (David 2026-08-20, completion-gate sign-off Q1a): the DEFAULT is `benchmark-sh`, the
# SHA-PINNED REFERENCE producer. A scoring-bearing run must mirror the ORGANIZER'S TRUST BOUNDARY
# by default — the gates that decide a score are produced by the same ranked chain the organizer
# runs, not by our own implementation of it. benchd-as-producer is the PARITY-TEST configuration,
# not the scoring default: using it by default would mean benchd both produces and checks the
# gates, and a parity bug in our `--official` would agree with itself. `facade` therefore stays a
# fully supported producer, but it is now an EXPLICIT OPT-IN (`GATES_PRODUCER=facade`) so that
# choosing it is a recorded, deliberate act rather than something a run inherits silently.
#
#   benchmark-sh  (DEFAULT) — the REFERENCE `benchmark.sh --official` (GATE_CMD), the SAME ranked
#                  chain the organizer runs; seals its own partial_result=true gates-score to
#                  MLXFAST_SCORE_PATH. REQUIRES the cached transformed `weights/` provisioned on-box
#                  (an ops step, NOT this script). GATE_CMD is sha-pinned (GATE_CMD_SHA) before it
#                  runs (R10).
#   facade        (OPT-IN, parity testing) — benchd's OWN `scripts/benchmark.sh --official`
#                  (FACADE_CMD) with CHECK_GATES=1 SKIP_TIMED=1. R22 retracted the facade's
#                  --official refusal, so the facade is a FULL-PARITY --official implementation.
#                  Its benchctl official backend loads --weights DIRECTLY (no reference-checkpoint
#                  regeneration) — which DISSOLVES the 3ffbd6b weights problem: no cached
#                  transformed weights/ need be provisioned on-box. That convenience is exactly why
#                  it must be asked for explicitly on a scoring run. Requires the benchctl MLX
#                  engine (ENGINE) the facade spawns; sha-pinnable via FACADE_CMD_SHA. Seals its
#                  partial_result=true gates-score to MLXFAST_SCORE_PATH (benchctl writes it).
#   direct-swift  (ALTERNATE) — the official_swift_run path (the trusted `mlxfast-swift` binary
#                  loading --weights directly), the WEIGHTLESS fallback (commit 3ffbd6b). Seals the
#                  binary's STDOUT.
GATES_PRODUCER="${GATES_PRODUCER:-benchmark-sh}"
case "$GATES_PRODUCER" in facade|benchmark-sh|direct-swift) : ;; *) echo "official-paired: FATAL unknown GATES_PRODUCER='$GATES_PRODUCER' (want facade|benchmark-sh|direct-swift)" >&2; exit 9 ;; esac
# R22 facade producer env: benchd's own scripts/benchmark.sh (this script lives beside it in
# scripts/), an optional sha-pin (like GATE_CMD_SHA), and the benchctl MLX engine the facade spawns.
FACADE_CMD="${FACADE_CMD:-$HERE/benchmark.sh}"
FACADE_CMD_SHA="${FACADE_CMD_SHA:-}"
ENGINE="${ENGINE:-}"

# ---------------------------------------------------------------------------------------------
# LANE 2b — per-phase (per-leg) engine-binary IDENTITY re-verification ------------------------
# ---------------------------------------------------------------------------------------------
# The gate (scripts/window-preflight.sh) attests the engine/worker binary's sha256 ONCE, at
# preflight: its `check_bin enginebin` seals the on-box bytes against WP_ENGINE_BIN_SHA256. That
# single seal cannot see a binary that is swapped AFTER the gate passed but BETWEEN this driver's
# legs — the calibration/gates leg (seam 1) and the timed execution leg (seam 2). 2b re-verifies
# the SAME engine binary the execution leg will spawn against that gate-attested sha at the TOP of
# EACH leg, so a mid-run swap is caught fail-closed BEFORE any measurement runs.
#
# HONEST SOURCE (not a fresh self-hash): OFFICIAL_ENGINE_BIN_SHA256 is the value the GATE sealed
# (WP_ENGINE_BIN_SHA256), threaded in by run-paired-window.sh. A fresh self-hash of whatever binary
# is present would verify a swapped binary against itself and protect nothing; this compares the
# spawned binary against the gate's seal. Opt-in like the golden's GATE_CMD_SHA: unset ⇒ NOTE and
# skip (so non-window offline/dev runs are unaffected); a real scoring window always pins it, and a
# set-but-mismatched or unreadable binary is a distinct die-class (12), never a silent pass.
#
# ENGINE_BIN is the binary measure-job resolves + spawns for each leg
# (<PAIRED_WS>/.build/release/mlxfast-engine), honoring the MLXFAST_MEASURE_WORKER_BIN override the
# window seam pins — so this driver hashes exactly the file the timed legs run.
ENGINE_BIN="${MLXFAST_MEASURE_WORKER_BIN:-$PAIRED_WS/.build/release/mlxfast-engine}"
OFFICIAL_ENGINE_BIN_SHA256="${OFFICIAL_ENGINE_BIN_SHA256:-}"
E_ENGINE_BIN_SWAP=12
verify_worker_sha_or_die() { # <leg-label>
  local leg="$1" got
  if [ -z "$OFFICIAL_ENGINE_BIN_SHA256" ]; then
    echo "official-paired: NOTE OFFICIAL_ENGINE_BIN_SHA256 unset — $leg leg runs WITHOUT re-verifying the engine binary identity (set it to the gate-attested WP_ENGINE_BIN_SHA256 to pin every leg, like the golden)" >&2
    return 0
  fi
  if [ ! -r "$ENGINE_BIN" ]; then
    echo "official-paired: FATAL $leg leg — engine binary unreadable at $ENGINE_BIN; refusing to run a measured leg against an unverifiable binary" >&2
    exit $E_ENGINE_BIN_SWAP
  fi
  got="$(official_sha_of "$ENGINE_BIN")"
  if [ "$got" != "$OFFICIAL_ENGINE_BIN_SHA256" ]; then
    echo "official-paired: FATAL $leg leg — engine binary sha mismatch: gate-attested=$OFFICIAL_ENGINE_BIN_SHA256 actual=$got ($ENGINE_BIN). The binary that will run this leg is not the one the gate sealed; refusing before any measurement (2b per-leg re-verification)." >&2
    exit $E_ENGINE_BIN_SWAP
  fi
}

# ---------------------------------------------------------------------------------------------
# R10 — GATE_EXTRA_ENV / MEASURE_EXTRA_ENV: parse as a VALIDATED KEY=VAL allowlist ------------
# ---------------------------------------------------------------------------------------------
# The reproduced hijack: `GATE_EXTRA_ENV="FOO=1 /bin/echo X"` word-splits into the producer command
# position and SWAPS the command. Fix: split the value on whitespace/newlines and require EVERY
# token to be a shell env assignment `NAME=VALUE` (NAME = [A-Za-z_][A-Za-z0-9_]*, VALUE whitespace-
# free). ANY token without a `=`, or whose name is not an identifier (a path like `/bin/echo` or a
# bare word like `X`), is a FATAL reject of the WHOLE list — never word-split into a command. The
# validated tokens are applied via `env NAME=VALUE …` (an array), never spliced into the command.
# Empty input → empty allowlist. Prints the validated tokens (one per line); rc!=0 on any bad token.
validate_kv_env() { # <label> <raw>
  local label="$1" raw="$2" tok name
  case "$raw" in *[![:space:]]*) : ;; *) return 0 ;; esac   # all-whitespace/empty → nothing
  for tok in $raw; do
    case "$tok" in
      [A-Za-z_]*=*) name="${tok%%=*}"
        case "$name" in
          *[!A-Za-z0-9_]*) echo "official-paired: FATAL $label token '$tok' has a non-identifier name '$name' — rejecting the allowlist" >&2; return 1 ;;
          *) printf '%s\n' "$tok" ;;
        esac ;;
      *) echo "official-paired: FATAL $label token '$tok' is not KEY=VAL (looks like a path/command) — refusing to word-split it into the producer command position" >&2; return 1 ;;
    esac
  done
}
# Materialize the two allowlists into arrays (a bad token aborts the whole run — fail LOUD/closed).
GATE_ENV_TOKENS=(); MEASURE_ENV_TOKENS=()
_kv_out="$(validate_kv_env GATE_EXTRA_ENV "${GATE_EXTRA_ENV:-}")" || exit 9
while IFS= read -r _l; do [ -n "$_l" ] && GATE_ENV_TOKENS+=("$_l"); done <<< "$_kv_out"
_kv_out="$(validate_kv_env MEASURE_EXTRA_ENV "${MEASURE_EXTRA_ENV:-}")" || exit 9
while IFS= read -r _l; do [ -n "$_l" ] && MEASURE_ENV_TOKENS+=("$_l"); done <<< "$_kv_out"

# The mode discriminator seam 3 seals (crates/benchctl/src/overlay.rs SCORING_MODE) — a NAME, not a
# numeric band, so it is asserted as an exact string (unlike floor/ceiling, read from the artifact).
EXP_MODE="qwen-native-mtp-paired-decode-only"
# AUTHOR-AT-SEAL (DECIDE-3) — when a dispatch context is present it is the AUTHORITY for the commit
# identity: the same sha `record_dispatch_sha` writes to candidate.sha, so the defence-in-depth
# MLXFAST_COMMIT_SHA agrees with the record instead of diverging to the box-checkout HEAD (which
# under the ranked sandbox is not the dispatched commit at all). No context ⇒ the pre-existing
# official_commit_sha40 resolution (OFFICIAL_COMMIT override / bench HEAD / sentinel). Written as an
# explicit if/else so the dispatch value is not silently dependent on official_commit_sha40's
# override-wins branch.
_dispatched="$(dispatch_context_sha)" || { echo "official-paired: FATAL malformed dispatched sha" >&2; exit 2; }
if [ -n "$_dispatched" ]; then
  export OFFICIAL_COMMIT="$_dispatched"
else
  export OFFICIAL_COMMIT="$(official_commit_sha40)"
fi
command -v jq >/dev/null || { echo "official-paired: FATAL jq required" >&2; exit 2; }
command -v python3 >/dev/null || { echo "official-paired: FATAL python3 required" >&2; exit 2; }

PA="$OUT/proof-a"
mkdir -p "$PA"
# Anti-stale: wipe prior per-run artifacts so any file present at assertion time was written THIS run.
# R9: do NOT `rm -f` the TABLE here. run-paired-window.sh redirects THIS script's stdout INTO
# $OUT/official-paired.table.txt (the same path as $TABLE); `rm -f`-ing it would UNLINK the parent's
# live redirect target, so the parent's echoed stdout (incl. the seam*-cmd lines the window greps)
# would be written to an orphaned inode and LOST. Truncate-in-place (`: > "$TABLE"`) instead — that
# clears the file for a standalone run yet preserves the inode the parent already holds open.
rm -f "$PA"/gates-score.json "$PA"/results.json "$PA"/results.json.sha256 \
      "$PA"/score.json "$PA"/score.json.sha256 "$PA"/benchmark-integrity.*.json \
      "$PA"/results.floor-fail.json "$PA"/score.floor-fail.json "$PA"/score.floor-fail.json.sha256 \
      2>/dev/null || true

TABLE="$OUT/official-paired.table.txt"; : > "$TABLE"
FAIL=0
row() { # <verdict> <check> <detail>
  printf '%-40s | %-8s | %s\n' "$2" "$1" "$3" | tee -a "$TABLE"
  case "$1" in PASS|SKIP) : ;; *) FAIL=$((FAIL+1)) ;; esac
}
hdr() {
  printf '%-40s | %-8s | %s\n' "check" "verdict" "detail" | tee -a "$TABLE"
  printf -- '-----------------------------------------|----------|----------------------------------------\n' | tee -a "$TABLE"
}
# Feed a python TSV evaluator's rows through row(); abort LOUD if the evaluator itself crashes.
feed() { # <label> <tsv-file> <err-file> <rc>
  if [ "$4" -ne 0 ] || [ ! -s "$2" ]; then
    echo "official-paired: FATAL $1 evaluator crashed (rc=$4) — see $3" >&2
    sed 's/^/    /' "$3" >&2
    exit 3
  fi
  while IFS=$'\t' read -r v c d; do [ -n "$c" ] && row "$v" "$c" "$d"; done < "$2"
}

echo "== PROOF A — THREE-SEAM paired IDENTITY chain =="
seam1_producer_cmd="$GATE_CMD"; [ "$GATES_PRODUCER" = facade ] && seam1_producer_cmd="$FACADE_CMD"
echo "   seam1=$GATES_PRODUCER ($seam1_producer_cmd) · benchctl=$BENCHCTL · ws=$PAIRED_WS (candidate==baseline) · contract=$(basename "$CONTRACT")"
echo "   golden=$(basename "$OFFICIAL_GOLDEN") · tokens=$TOKENS depth=$DEPTH min/target pairs=$MIN_PAIRS/$TARGET_PAIRS · identity band |raw-1.0|<=$IDENTITY_BAND"
echo ""
hdr

# ---------------------------------------------------------------------------------------------
# SEAM 1 — gates → gates-score.json (partial_result=true), producer per $GATES_PRODUCER
# ---------------------------------------------------------------------------------------------
# RULING Q1a (David 2026-08-20): seam-1 gates come from the DEFAULT `benchmark-sh` producer — the
# REFERENCE `benchmark.sh --official`, the same ranked chain the organizer runs. SKIP_TIMED=1 makes
# CHECK_GATES a GATES-ONLY run → partial_result=true.
#
# OPERATIONAL CONSEQUENCE, and it is a provisioning step not a code one: the reference chain
# REGENERATES weights/ from a checkpoint that is absent on-box, so benchmark-sh REQUIRES a
# provisioned cached transformed `weights/`. The facade did NOT — its benchctl backend loads
# --weights directly — so the default flip re-introduces a staging requirement the facade had
# dissolved. R11's SEAM1_ONLY precheck catches a missing weights/ BEFORE the window unloads qwen,
# so this fails cheap rather than mid-window; the retry-window staging list must carry it.
#
# The OPT-IN and the fallback stay: facade (benchd's own scripts/benchmark.sh --official, for
# PARITY TESTING, needs no cached weights/) and direct-swift (the weightless mlxfast-swift
# STDOUT-seal fallback, commit 3ffbd6b). UNVERIFIED(B-4): the on-box gates env.
GATES_SCORE="$PA/gates/score.json"
# --- direct-swift FALLBACK producer (weightless; commit 3ffbd6b) ---------------------------------
# The trusted `mlxfast-swift` binary via official_swift_run; SKIP_TIMED=1 → gates-only run
# (partial_result=true). Seals the binary's STDOUT (the on-disk --score-path is untrusted).
run_gates_direct_swift() {
  mkdir -p "$PA/gates"
  # R10: the operator GATE_EXTRA_ENV allowlist (validated KEY=VAL tokens) is appended to the fixed
  # SKIP_TIMED flag — space-joined here is SAFE because every token was validated whitespace-free.
  local extra="MLXFAST_BENCHMARK_SKIP_TIMED=1" t
  for t in ${GATE_ENV_TOKENS[@]+"${GATE_ENV_TOKENS[@]}"}; do extra="$extra $t"; done
  OFFICIAL_SWIFT_EXTRA_ENV="$extra" official_swift_run "$PA/gates"
  # official_swift_run seals STDOUT → $PA/gates/score.json and records $PA/gates/exit_code.
  cp -f "$PA/gates/exit_code" "$PA/gates.exit" 2>/dev/null || printf '%s' "${OFFICIAL_SWIFT_RC:-1}" > "$PA/gates.exit"
  cp -f "$PA/gates/stderr" "$PA/gates.stderr" 2>/dev/null || true
}
# --- benchmark-sh DEFAULT producer (reference ranked chain; needs cached weights/ on-box) ---------
# Runs `bash $GATE_CMD --official` with CHECK_GATES/SKIP_TIMED so benchmark.sh (the trusted
# workflow) seals its OWN partial_result=true gates-score to MLXFAST_SCORE_PATH ($GATES_SCORE) —
# so, unlike direct-swift, the on-disk score is the trusted seal (benchmark.sh is the workflow).
# R10: GATE_CMD is sha-pinned (GATE_CMD_SHA) before it is executed; the GATE_EXTRA_ENV allowlist is
# applied via `env KEY=VAL …` (never bare word-splitting into the command position).
run_gates_benchmark_sh() {
  mkdir -p "$PA/gates"
  # R10 GATE_CMD sha-pin (like the golden): a set GATE_CMD_SHA MUST match shasum -a256 of the file.
  if [ -n "${GATE_CMD_SHA:-}" ]; then
    local got; got="$(official_sha_of "$GATE_CMD")"
    if [ "$got" != "$GATE_CMD_SHA" ]; then
      echo "official-paired: FATAL GATE_CMD sha mismatch — pin=$GATE_CMD_SHA actual=$got ($GATE_CMD); refusing to run an unpinned seam-1 producer" >&2
      exit 8
    fi
  else
    echo "official-paired: NOTE GATE_CMD_SHA unset — running $GATE_CMD without a sha-pin (set GATE_CMD_SHA to pin it, like the golden)" >&2
  fi
  env ${GATE_ENV_TOKENS[@]+"${GATE_ENV_TOKENS[@]}"} \
    MLXFAST_BENCHMARK_CHECK_GATES=1 \
    MLXFAST_BENCHMARK_SKIP_TIMED=1 \
    MLXFAST_SCORE_PATH="$GATES_SCORE" \
    MLXFAST_CORRECTNESS_GOLDEN_PATH="$OFFICIAL_GOLDEN" \
    MLXFAST_WEIGHTS_PATH="$WEIGHTS" \
    bash "$GATE_CMD" --official > "$PA/gates/stdout" 2> "$PA/gates/stderr"
  printf '%s' "$?" > "$PA/gates.exit"
  cp -f "$PA/gates/stderr" "$PA/gates.stderr" 2>/dev/null || true
}
# --- facade OPT-IN producer (R22; benchd's own scripts/benchmark.sh --official) -------------------
# Runs `bash $FACADE_CMD --official` with CHECK_GATES/SKIP_TIMED so the FACADE (whose --official is
# now a full-parity implementation) routes to `benchctl iterate --mode official` in gates-only mode
# and seals its OWN partial_result=true gates-score to MLXFAST_SCORE_PATH ($GATES_SCORE). Unlike
# benchmark-sh this needs NO cached transformed weights/: the benchctl official backend loads
# --weights DIRECTLY (dissolves the 3ffbd6b weights problem). FACADE_CMD is sha-pinnable (R10).
run_gates_facade() {
  mkdir -p "$PA/gates"
  if [ -n "${FACADE_CMD_SHA:-}" ]; then
    local got; got="$(official_sha_of "$FACADE_CMD")"
    if [ "$got" != "$FACADE_CMD_SHA" ]; then
      echo "official-paired: FATAL FACADE_CMD sha mismatch — pin=$FACADE_CMD_SHA actual=$got ($FACADE_CMD); refusing to run an unpinned seam-1 producer" >&2
      exit 8
    fi
  else
    echo "official-paired: NOTE FACADE_CMD_SHA unset — running $FACADE_CMD without a sha-pin (set FACADE_CMD_SHA to pin it, like the golden)" >&2
  fi
  : "${ENGINE:?set ENGINE (the benchctl MLX engine binary the facade spawns) for GATES_PRODUCER=facade}"
  env ${GATE_ENV_TOKENS[@]+"${GATE_ENV_TOKENS[@]}"} \
    BENCHCTL="$BENCHCTL" \
    MLXFAST_ENGINE_BIN="$ENGINE" \
    MLXFAST_USE_RUNTIME_WORKER=1 \
    MLXFAST_BENCHMARK_CHECK_GATES=1 \
    MLXFAST_BENCHMARK_SKIP_TIMED=1 \
    MLXFAST_SCORE_PATH="$GATES_SCORE" \
    MLXFAST_CORRECTNESS_GOLDEN_PATH="$OFFICIAL_GOLDEN" \
    MLXFAST_WEIGHTS_PATH="$WEIGHTS" \
    bash "$FACADE_CMD" --official > "$PA/gates/stdout" 2> "$PA/gates/stderr"
  printf '%s' "$?" > "$PA/gates.exit"
  cp -f "$PA/gates/stderr" "$PA/gates.stderr" 2>/dev/null || true
}
run_gates() {
  # 2b — re-verify the execution engine binary against the gate seal at the calibration/gates leg.
  verify_worker_sha_or_die "seam1-calibration"
  case "$GATES_PRODUCER" in
    facade)       run_gates_facade ;;
    benchmark-sh) run_gates_benchmark_sh ;;
    direct-swift) run_gates_direct_swift ;;
  esac
}
# --- David ruling 2026-08-26: CAPTURE THE GATES LEG'S WORKSPACE ROOT -----------------------------
# The gates score carries `metrics.harness_hash` — the 9-root harness identity of the workspace the
# GATES PRODUCER ran in. benchd's seam-3 overlay now RE-RESOLVES that identity at the seal and
# refuses a merge when the two disagree (the between-phase TOCTOU gate; see
# crates/benchctl/src/overlay.rs `validate_harness_identity_cross_leg`).
#
# A harness hash covers the workspace's ABSOLUTE LOCATION as well as its bytes (bench-core
# `harness_hash` module doc, item 1), so the two legs are only comparable when they resolved the
# SAME ROOT. Nothing made that true before: every producer below pins its own CWD, and seam 3
# inherited whatever CWD the operator invoked this driver from. So capture the root the producer
# actually used, ONCE, here — and `cd` to exactly it before seam 3 (below).
#
# Per producer, from each producer's own CWD discipline:
#   * benchmark-sh  — `bash $GATE_CMD --official`; the reference benchmark.sh resolves its own
#                     SCRIPT_DIR and `cd`s there, so the root is the directory GATE_CMD lives in.
#   * direct-swift  — official_swift_run runs the binary from $SWIFT_REPO_ROOT when set (see
#                     official-lib.sh `[ -n "${SWIFT_REPO_ROOT:-}" ] && cd "$SWIFT_REPO_ROOT"`),
#                     and does NOT cd when it is unset — mirrored exactly here.
#   * facade        — benchd's own scripts/benchmark.sh NEVER cd's, so the producer inherits THIS
#                     driver's CWD and that is the root.
# Fail-closed: an empty or non-directory root is a FATAL before any GPU work, not a seam-3 surprise.
gates_workspace_root() {
  case "$GATES_PRODUCER" in
    benchmark-sh) official_abs "$(dirname "$GATE_CMD")" ;;
    direct-swift) if [ -n "${SWIFT_REPO_ROOT:-}" ]; then official_abs "$SWIFT_REPO_ROOT"; else pwd -P; fi ;;
    facade)       pwd -P ;;
  esac
}
GATES_WS="$(gates_workspace_root)"
if [ -z "$GATES_WS" ] || [ ! -d "$GATES_WS" ]; then
  echo "official-paired: FATAL could not resolve the seam-1 gates WORKSPACE ROOT for GATES_PRODUCER=$GATES_PRODUCER (got '${GATES_WS}'); seam 3 must run with CWD == the workspace the gates producer hashed, or the cross-leg harness-identity gate cannot be satisfied" >&2
  exit 10
fi
echo "official-paired: seam-1 gates workspace root (seam-3 seal CWD) = $GATES_WS" >&2

echo "-- seam 1: gates via GATES_PRODUCER=$GATES_PRODUCER (CHECK_GATES=1 SKIP_TIMED=1 → partial_result gates-score) --" >&2
run_gates
python3 - "$GATES_SCORE" "$PA/gates.exit" > "$PA/s1.tsv" 2> "$PA/s1.err" <<'PY'
import json, sys
gp, ep = sys.argv[1], sys.argv[2]
def emit(v,c,d): print("%s\t%s\t%s" % (v,c,d))
try: ec = open(ep).read().strip()
except Exception: ec = "MISSING"
try:
    with open(gp) as f: g = json.load(f); gerr = None
except Exception as e: g, gerr = None, str(e)
if g is None:
    emit("FAIL","seam1.gates-score.json","TOOL-ERR unreadable/missing: %s" % gerr); raise SystemExit
emit("PASS" if ec=="0" else "FAIL","seam1.exit-code","exit=%s want 0%s"%(ec,"" if ec=="0" else " (TOOL-ERR)"))
def g_(root,*ks):
    cur=root
    for k in ks:
        if not isinstance(cur,dict) or k not in cur: return ("__MISSING__",False)
        cur=cur[k]
    return (cur,True)
p,ok=g_(g,"passed");            emit("PASS" if ok and p is True else "FAIL","seam1.passed","got=%r want=true"%(p,))
pr,ok=g_(g,"metrics","partial_result"); emit("PASS" if ok and pr is True else "FAIL","seam1.partial_result","got=%r want=true (gates awaiting overlay)"%(pr,))
pc,ok=g_(g,"metrics","passed_correctness"); emit("PASS" if ok and pc is True else "FAIL","seam1.passed_correctness","got=%r want=true"%(pc,))
er,ok=g_(g,"metrics","error"); emit("PASS" if ok and er=="" else "FAIL","seam1.error-empty","got=%r want empty"%(er,))
PY
feed seam1 "$PA/s1.tsv" "$PA/s1.err" $?

# R11 hard-stop: SEAM1_ONLY=1 validates seam 1 (gates, GPU-light) and EXITS BEFORE seam 2 — so the
# window can gate on a valid gates-score.json WITHOUT unloading qwen / spending the (85s-reload)
# measure-job window on a producer that never produced. A seam-1 FAIL exits 1; seam-1 clean exits 0.
if [ "${SEAM1_ONLY:-0}" = "1" ]; then
  echo ""
  if [ "$FAIL" -eq 0 ]; then
    echo "official-paired: SEAM1_ONLY OK — seam 1 produced a valid gates-score.json ($GATES_SCORE); seam 2 NOT run (qwen untouched)"
    exit 0
  else
    echo "official-paired: SEAM1_ONLY FAIL — $FAIL seam-1 assertion(s) failed; ABORTING before seam 2 (qwen untouched, window not spent)" >&2
    exit 1
  fi
fi

# ---------------------------------------------------------------------------------------------
# SEAM 2 — measure-job IDENTITY → results.json (+ benchmark-integrity.results.json)
# ---------------------------------------------------------------------------------------------
RESULTS="$PA/results.json"
RESULTS_INTEGRITY="$PA/benchmark-integrity.results.json"
# R7 golden pool: GOLDENS is a whitespace/newline-separated list of DISTINCT golden files → one
# repeatable `--golden` per entry (the 8-prompt pool). A real ranked pool needs distinct digests —
# dup-digest is FATAL in measure-job (validate_golden_set), so identical files would abort. When
# GOLDENS is unset the single OFFICIAL_GOLDEN is used (the identity/parity framing: cardinality-1).
measure_golden_args() {
  local g
  if [ -n "${GOLDENS:-}" ]; then
    for g in $GOLDENS; do printf '%s\n%s\n' "--golden" "$g"; done
  else
    printf '%s\n%s\n' "--golden" "$OFFICIAL_GOLDEN"
  fi
}
# LANE 2a — SOURCE the track contract's hidden correctness-golden pin (its `hidden_correctness_golden`
# sha256, a SIBLING of `timed_prompt_pool`; engine PR #41). Read the SHA with jq when the box has it,
# else a jq-free fallback (this driver may run on a box without jq): the pin object carries no nested
# braces, so `[^}]*` isolates it, and the prose `_note` sibling is a different key left untouched. This
# is the SAME sourcing model window-probe.sh uses, so the gate and the run agree on ONE authority. A
# non-empty result means the fixture PINS the correctness golden and seam-2 MUST attest it.
contract_hidden_correctness_golden_sha() {
  [ -f "$CONTRACT" ] || return 0
  if command -v jq >/dev/null 2>&1; then
    jq -r '.hidden_correctness_golden.sha256 // empty' "$CONTRACT" 2>/dev/null
  else
    tr -d '\n' < "$CONTRACT" \
      | sed -n 's/.*"hidden_correctness_golden"[[:space:]]*:[[:space:]]*{\([^}]*\)}.*/\1/p' \
      | sed -n 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([0-9a-f]\{64\}\)".*/\1/p'
  fi
}
run_measure() {
  # 2b — re-verify the SAME engine binary again at the timed execution leg. A binary swapped since
  # the calibration leg's check (above) is caught HERE, fail-closed, before measure-job spawns it.
  verify_worker_sha_or_die "seam2-execution"
  # R6: --weights is an OPTIONAL OVERRIDE — pass it only when WEIGHTS is set; otherwise measure-job
  # derives the weights DIR from QMTP_TARGET_DIR (and fails closed if that too is unset).
  local weights_arg=(); [ -n "$WEIGHTS" ] && weights_arg=(--weights "$WEIGHTS")
  # LANE 2a — ATTEST the hidden correctness golden when the fixture PINS it (#157). benchd refuses
  # (die-8, pre-GPU) any scoring run whose --contract pins a `hidden_correctness_golden` but that
  # carries no `--correctness-golden` attestation — so once #41 deploys the fixture pin, EVERY
  # driver-invoked window would die-8 without this wiring. COEXIST (ruled): OFFICIAL_GOLDEN stays the
  # STAGED correctness-golden source (seam-1 passes it as MLXFAST_CORRECTNESS_GOLDEN_PATH); the flag
  # merely carries that same staged path to measure-job, which HASHES it (sha256+bytes) and refuses
  # any run whose identity does not CITE the fixture pin. Fail-closed BOTH ways: pass the flag ONLY
  # when the fixture pins the golden — passing it against a fixture that pins none is itself a die-8.
  local cg_arg=(); local _hcg_sha
  _hcg_sha="$(contract_hidden_correctness_golden_sha)"
  [ -n "$_hcg_sha" ] && cg_arg=(--correctness-golden "$OFFICIAL_GOLDEN")
  # R7: repeatable --golden built from the GOLDENS pool (or the single OFFICIAL_GOLDEN).
  local golden_args=(); local _g
  while IFS= read -r _g; do [ -n "$_g" ] && golden_args+=("$_g"); done < <(measure_golden_args)
  # R14: pass the QMTP_* head/target dirs through to measure-job when set (it reads them from env:
  # QMTP_TARGET_DIR → backbone/target cache; QMTP_HEAD_DIR → pinned serial head; QMTP_CANDIDATE_HEAD_DIR
  # → candidate-leg BYO head, defaulting to QMTP_HEAD_DIR when unset). Only forwarded when non-empty.
  local qmtp_env=()
  [ -n "${QMTP_TARGET_DIR:-}" ]         && qmtp_env+=("QMTP_TARGET_DIR=$QMTP_TARGET_DIR")
  [ -n "${QMTP_HEAD_DIR:-}" ]           && qmtp_env+=("QMTP_HEAD_DIR=$QMTP_HEAD_DIR")
  [ -n "${QMTP_CANDIDATE_HEAD_DIR:-}" ] && qmtp_env+=("QMTP_CANDIDATE_HEAD_DIR=$QMTP_CANDIDATE_HEAD_DIR")
  # AUTHOR-AT-SEAL (DECIDE-3): record the dispatched sha to $PA/candidate.sha and point measure-job
  # at it so the seal AUTHORS metrics.commit from the dispatch record (not the git identity / a
  # competitor proposal). With no dispatch context nothing is recorded, so on this scoring/ranked
  # path measure-job's seal FAILS CLOSED (die-8) rather than falling back to the box git identity;
  # the unbound commit_identifier fallback survives only under --local-dev.
  local record_env=() _rec
  _rec="$(record_dispatch_sha "$PA")" || { echo "official-paired: FATAL could not record dispatched sha" >&2; exit 2; }
  [ -n "$_rec" ] && record_env+=("MLXFAST_CANDIDATE_SHA_FILE=$_rec")
  # R10: MEASURE_EXTRA_ENV is applied as the VALIDATED KEY=VAL array (never a bare word-split into
  # the command position). ${MEASURE_ENV_TOKENS[@]+…} is empty-safe under `set -u`.
  # --min-pairs/--target-pairs are the PER-PROMPT budget flags (R13: die-5 if a prompt accepts < MIN).
  # shellcheck disable=SC2086
  env \
    MLXFAST_USE_RUNTIME_WORKER=1 \
    MLXFAST_COMMIT_SHA="$(official_commit_sha40)" \
    ${record_env[@]+"${record_env[@]}"} \
    ${qmtp_env[@]+"${qmtp_env[@]}"} \
    ${MEASURE_ENV_TOKENS[@]+"${MEASURE_ENV_TOKENS[@]}"} \
    "$BENCHCTL" measure-job \
      --candidate "$PAIRED_WS" \
      --baseline "$PAIRED_WS" \
      ${weights_arg[@]+"${weights_arg[@]}"} \
      "${golden_args[@]}" \
      --contract "$CONTRACT" \
      ${cg_arg[@]+"${cg_arg[@]}"} \
      --tokens "$TOKENS" \
      --mtp-depth "$DEPTH" \
      --min-pairs "$MIN_PAIRS" \
      --target-pairs "$TARGET_PAIRS" \
      --tag "$TAG" \
      --gates-producer "$GATES_PRODUCER" \
      --out "$PA" \
    > "$PA/measure.stdout" 2> "$PA/measure.stderr"
  printf '%s' "$?" > "$PA/measure.exit"
}
echo "-- seam 2: measure-job identity (candidate==baseline==$PAIRED_WS) --" >&2
run_measure
python3 - "$RESULTS" "$PA/measure.exit" "$RESULTS_INTEGRITY" "$MIN_PAIRS" "$IDENTITY_BAND" "$GATES_PRODUCER" > "$PA/s2.tsv" 2> "$PA/s2.err" <<'PY'
import json, sys, hashlib
rp, ep, ip, minp, band = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), float(sys.argv[5])
# The producer THIS run resolved (ruling Q1a). Compared against the sealed field below.
RESOLVED_PRODUCER = sys.argv[6]
def emit(v,c,d): print("%s\t%s\t%s" % (v,c,d))
def isnum(x): return isinstance(x,(int,float)) and not isinstance(x,bool)
try: ec = open(ep).read().strip()
except Exception: ec = "MISSING"
try:
    raw = open(rp,'rb').read(); r = json.loads(raw); rerr = None
except Exception as e: r, raw, rerr = None, None, str(e)
if r is None:
    emit("FAIL","seam2.results.json","TOOL-ERR unreadable/missing: %s" % rerr); raise SystemExit
emit("PASS" if ec=="0" else "FAIL","seam2.exit-code","exit=%s want 0 (candidate accepted)%s"%(ec,"" if ec=="0" else " (TOOL-ERR/die5)"))
def g_(root,*ks):
    cur=root
    for k in ks:
        if not isinstance(cur,dict) or k not in cur: return ("__MISSING__",False)
        cur=cur[k]
    return (cur,True)
pa,ok=g_(r,"parity_all_ok"); emit("PASS" if ok and pa is True else "FAIL","seam2.parity_all_ok","got=%r want=true"%(pa,))
apc,ok=g_(r,"accepted_pair_count")
emit("PASS" if ok and isnum(apc) and apc>=minp else "FAIL","seam2.accepted_pair_count>=min","got=%r want>=%d"%(apc,minp))
prs=r.get("pairs")
plen = len(prs) if isinstance(prs,list) else None
emit("PASS" if plen is not None and plen==apc else "FAIL","seam2.pairs-len==accepted","pairs=%r accepted=%r (must match)"%(plen,apc))
pp=r.get("per_prompt")
emit("PASS" if isinstance(pp,list) and len(pp)>0 else "FAIL","seam2.per_prompt-nonempty","len=%r want>0"%(len(pp) if isinstance(pp,list) else None,))
bm,ok=g_(r,"aggregate","baseline_serial_seconds_per_token_mean"); emit("PASS" if ok and isnum(bm) and bm>0 else "FAIL","seam2.baseline_mean>0","got=%r"%(bm,))
cm,ok=g_(r,"aggregate","candidate_mtp_seconds_per_token_mean"); emit("PASS" if ok and isnum(cm) and cm>0 else "FAIL","seam2.candidate_mean>0","got=%r"%(cm,))
# Identity band: every per-prompt raw ratio-of-means AND every per-pair raw ratio ≈ 1.0.
bad=[]
for i,e in enumerate(pp or []):
    x=e.get("raw_ratio_of_means")
    if not (isnum(x) and abs(x-1.0)<=band): bad.append("per_prompt[%d]=%r"%(i,x))
emit("PASS" if not bad else "FAIL","seam2.per_prompt raw~1.0","|raw-1.0|<=%.2f identity band; offenders=%s"%(band, bad or "none"))
badp=[]
for i,e in enumerate(prs or []):
    x=e.get("raw_ratio")
    if not (isnum(x) and abs(x-1.0)<=band): badp.append("pairs[%d]=%r"%(i,x))
emit("PASS" if not badp else "FAIL","seam2.pairs raw_ratio~1.0","|raw-1.0|<=%.2f identity band; offenders=%s"%(band, badp or "none"))
# Integrity sidecar: results.json digest lives INSIDE benchmark-integrity.results.json (finding 10).
try:
    integ = json.load(open(ip)); ierr=None
except Exception as e: integ, ierr = None, str(e)
if integ is None:
    emit("FAIL","seam2.integrity-sidecar","TOOL-ERR missing %s: %s"%(ip,ierr))
else:
    want = integ.get("results_sha256")
    got = hashlib.sha256(raw).hexdigest()
    emit("PASS" if want==got else "FAIL","seam2.integrity results_sha256","anchor=%s actual=%s"%((want or "")[:12]+"…", got[:12]+"…"))
    # RULING Q1a conformance floor — the sidecar must NAME the seam-1 producer this run actually
    # used. The opt-in is an ENV VAR, so `GATES_PRODUCER=facade` can select the parity-test producer
    # for a scoring run without ever appearing in a command line; this row is what makes that
    # choice auditable in the artifact instead of invisible. A missing or disagreeing field FAILS
    # the run rather than being tolerated: an artifact that cannot say which producer made its
    # gates cannot support a scoring claim.
    sealed = integ.get("gates_producer")
    emit("PASS" if sealed==RESOLVED_PRODUCER else "FAIL","seam2.integrity gates_producer",
         "sealed=%r resolved=%r%s"%(sealed, RESOLVED_PRODUCER,
          "" if sealed is not None else " (field ABSENT — pre-Q1a benchctl?)"))
PY
feed seam2 "$PA/s2.tsv" "$PA/s2.err" $?
# Bash-side results.json.sha256 sidecar (bare-basename FIX-4 form) — a missing sidecar is a FAIL.
if [ -f "$RESULTS.sha256" ] && [ -f "$RESULTS" ]; then
  rwant="$(awk '{print $1}' "$RESULTS.sha256")"; rbody="$(awk '{print $2}' "$RESULTS.sha256")"; rgot="$(official_sha_of "$RESULTS")"
  [ "$rwant" = "$rgot" ] && row PASS "seam2.results.json.sha256 hash" "true hash (${rgot:0:12}…)" || row FAIL "seam2.results.json.sha256 hash" "sidecar $rwant != actual $rgot"
  [ "$rbody" = "results.json" ] && row PASS "seam2.results.json.sha256 form" "bare basename (FIX-4)" || row FAIL "seam2.results.json.sha256 form" "body=$rbody want 'results.json'"
else
  row FAIL "seam2.results.json.sha256" "TOOL-ERR sidecar or results.json missing"
fi

# ---------------------------------------------------------------------------------------------
# SEAM 3 — overlay-timing merge → score.json (+ .sha256, integrity re-anchored)
# ---------------------------------------------------------------------------------------------
SCORE="$PA/score.json"
run_overlay() { # <gates-score> <results> <score-path> [integrity]
  local gs="$1" res="$2" sp="$3" integ="${4:-}"
  # David ruling 2026-08-26 — PIN THE SEAL'S CWD to $GATES_WS (the root the seam-1 producer hashed).
  # benchd re-resolves the 9-root harness identity CWD-relative at the seal and refuses a merge that
  # does not match the gates leg; a harness hash is an identity of a tree AT A PATH, so running the
  # seal from anywhere else would refuse every honest run. Every path handed to benchctl is
  # ABSOLUTISED FIRST, because the cd changes what a relative one means; the cd itself is scoped to a
  # SUBSHELL so the rest of this driver keeps its own CWD (OUT/PA and the assertion blocks below).
  local gs_a res_a sp_a integ_a
  gs_a="$(official_abs "$gs")"; res_a="$(official_abs "$res")"; sp_a="$(official_abs "$sp")"
  local integ_arg=(); [ -n "$integ" ] && { integ_a="$(official_abs "$integ")"; integ_arg=(--integrity "$integ_a"); }
  (
    cd "$GATES_WS" || exit 11
    "$BENCHCTL" overlay-timing \
        --gates-score "$gs_a" --results "$res_a" --score-path "$sp_a" \
        ${integ_arg[@]+"${integ_arg[@]}"} \
        > "$sp_a.stdout" 2> "$sp_a.stderr"
  )
  printf '%s' "$?" > "$sp.exit"
}
echo "-- seam 3: overlay-timing (merge gates + results → LOCAL/parity score.json; organizer owns the ranked seal) --" >&2
run_overlay "$GATES_SCORE" "$RESULTS" "$SCORE" "$RESULTS_INTEGRITY"
python3 - "$SCORE" "$SCORE.exit" "$RESULTS_INTEGRITY" "$IDENTITY_BAND" "$EXP_MODE" > "$PA/s3.tsv" 2> "$PA/s3.err" <<'PY'
import json, sys, math, hashlib
sp, ep, ip, band, exp_mode = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4]), sys.argv[5]
def emit(v,c,d): print("%s\t%s\t%s" % (v,c,d))
def isnum(x): return isinstance(x,(int,float)) and not isinstance(x,bool)
try: ec = open(ep).read().strip()
except Exception: ec = "MISSING"
try:
    raw = open(sp,'rb').read(); s = json.loads(raw); serr=None
except Exception as e: s, raw, serr = None, None, str(e)
if s is None:
    emit("FAIL","seam3.score.json","TOOL-ERR unreadable/missing: %s"%serr); raise SystemExit
emit("PASS" if ec=="0" else "FAIL","seam3.exit-code","exit=%s want 0 (merged score passes)%s"%(ec,"" if ec=="0" else " (TOOL-ERR)"))
def g_(root,*ks):
    cur=root
    for k in ks:
        if not isinstance(cur,dict) or k not in cur: return ("__MISSING__",False)
        cur=cur[k]
    return (cur,True)
pr,ok=g_(s,"metrics","partial_result"); emit("PASS" if ok and pr is False else "FAIL","seam3.partial_result","got=%r want=false (timed overlay applied)"%(pr,))
mode,ok=g_(s,"scoring_mode"); emit("PASS" if ok and mode==exp_mode else "FAIL","seam3.scoring_mode","got=%r want=%s"%(mode,exp_mode))
sc,ok=g_(s,"score"); sc_fin = ok and isnum(sc) and math.isfinite(sc)
emit("PASS" if sc_fin else "FAIL","seam3.score-finite","got=%r want finite"%(sc,))
pp,ok=g_(s,"passed"); emit("PASS" if ok and pp is True else "FAIL","seam3.passed","got=%r want=true"%(pp,))
# Finding-15 discipline: READ floor/ceiling FROM the artifact; assert the identity band ⊆ [floor,ceil].
fl,okf=g_(s,"metrics","decode_speedup_floor"); ce,okc=g_(s,"decode_speedup_ceiling")
coherent = okf and okc and isnum(fl) and isnum(ce) and fl<ce
emit("PASS" if coherent else "FAIL","seam3.floor<ceiling (from artifact)","floor=%r ceiling=%r"%(fl,ce))
if coherent:
    sub = (1.0-band) >= fl and (1.0+band) <= ce
    emit("PASS" if sub else "FAIL","seam3.identity-band ⊆ [floor,ceil]","[%.2f,%.2f] ⊆ [%s,%s] (band not restated)"%(1-band,1+band,fl,ce))
    if sc_fin:
        ingate = fl <= sc <= ce
        emit("PASS" if ingate else "FAIL","seam3.score in [floor,ceil]","score=%r in [%s,%s]"%(sc,fl,ce))
        emit("PASS" if abs(sc-1.0)<=band else "FAIL","seam3.score identity ~1.0","|%.5f-1.0|<=%.2f"%(sc,band))
# Integrity re-anchored over the MERGED bytes (GEMMA-OVL :177-181).
try:
    integ = json.load(open(ip)); ierr=None
except Exception as e: integ, ierr = None, str(e)
if integ is None:
    emit("FAIL","seam3.integrity re-anchor","TOOL-ERR missing %s: %s"%(ip,ierr))
else:
    want = integ.get("score_sha256"); got = hashlib.sha256(raw).hexdigest()
    emit("PASS" if want==got else "FAIL","seam3.integrity score_sha256","anchor=%s actual=%s (merged bytes)"%((want or "")[:12]+"…", got[:12]+"…"))
PY
feed seam3 "$PA/s3.tsv" "$PA/s3.err" $?
# Bash-side score.json.sha256 sidecar (bare-basename form) — a missing sidecar is a FAIL.
if [ -f "$SCORE.sha256" ] && [ -f "$SCORE" ]; then
  swant="$(awk '{print $1}' "$SCORE.sha256")"; sgot="$(official_sha_of "$SCORE")"
  [ "$swant" = "$sgot" ] && row PASS "seam3.score.json.sha256" "true hash of score.json (${sgot:0:12}…)" || row FAIL "seam3.score.json.sha256" "sidecar $swant != actual $sgot"
else
  row FAIL "seam3.score.json.sha256" "TOOL-ERR sidecar or score.json missing"
fi

# ---------------------------------------------------------------------------------------------
# NEGATIVE CONTROL — overlay a FLOOR-FAIL results.json → score null, nonzero exit (cheap, no GPU)
# ---------------------------------------------------------------------------------------------
if [ "$NEG_CONTROL" = "1" ]; then
  echo ""
  echo "== negative control (floor-fail results → overlay nulls score; cannot fabricate green) =="
  echo "-- neg-control: synth floor-fail results (raw ratios → ~0.5 < 0.90 floor), re-run seam 3 --" >&2
  NEG_RESULTS="$PA/results.floor-fail.json"; NEG_SCORE="$PA/score.floor-fail.json"
  # Synthesize a floor-fail results.json from the REAL identity results: rewrite every per-prompt
  # raw_ratio_of_means and per-pair raw_ratio to 0.5 (a candidate 2x SLOWER than serial). If the
  # real results.json is absent (seam 2 failed), fall back to a minimal valid-superset floor-fail.
  python3 - "$RESULTS" "$NEG_RESULTS" 2> "$PA/neg.synth.err" <<'PY'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
try:
    r = json.load(open(src))
except Exception:
    r = {"track_id":"neg","parity_all_ok":True,"accepted_pair_count":3,
         "pairs":[{"raw_ratio":1.0}]*3,
         "per_prompt":[{"raw_ratio_of_means":1.0}],
         "aggregate":{"baseline_serial_seconds_per_token_mean":0.036,
                      "candidate_mtp_seconds_per_token_mean":0.072,
                      "raw_decode_speedup_median":1.0}}
# Rewrite every per-prompt raw ratio-of-means to 0.5 (a candidate 2x SLOWER than serial) AND set the
# per-prompt means so the R18 recompute (serial_mean / mtp_mean) is EXACTLY 0.5 — the overlay does not
# trust the sealed median blindly; it recomputes from the means and rejects a disagreement as a wrapper
# tamper. Keeping the means coherent makes the run breach the FLOOR (not a shape/tamper rejection).
for e in r.get("per_prompt",[]) or []:
    e["raw_ratio_of_means"]=0.5
    base=e.get("serial_seconds_per_token_mean") or 0.036
    e["serial_seconds_per_token_mean"]=base
    e["mtp_seconds_per_token_mean"]=base/0.5   # serial/mtp = 0.5 → R18 recompute median = 0.5 exactly
for e in r.get("pairs",[]) or []: e["raw_ratio"]=0.5
agg=r.setdefault("aggregate",{}); agg["raw_decode_speedup_median"]=0.5  # == even-n median of per-prompt 0.5s
# keep means > 0 so only the FLOOR (not a validation) is what fails
agg.setdefault("baseline_serial_seconds_per_token_mean",0.036)
agg["candidate_mtp_seconds_per_token_mean"]=agg["baseline_serial_seconds_per_token_mean"]/0.5
json.dump(r, open(dst,"w"), indent=2)
PY
  run_overlay "$GATES_SCORE" "$NEG_RESULTS" "$NEG_SCORE" ""   # fresh integrity sidecar, no re-anchor
  python3 - "$NEG_SCORE" "$NEG_SCORE.exit" > "$PA/sneg.tsv" 2> "$PA/sneg.err" <<'PY'
import json, sys
sp, ep = sys.argv[1], sys.argv[2]
def emit(v,c,d): print("%s\t%s\t%s"%(v,c,d))
try: ec = open(ep).read().strip()
except Exception: ec = "MISSING"
# A floor breach makes the overlay exit NONZERO; a missing score file is itself a fail-closed signal.
emit("PASS" if ec not in ("0","MISSING") else "FAIL","neg.exit-nonzero","exit=%s want nonzero (floor breach)"%(ec,))
try:
    s = json.load(open(sp)); serr=None
except Exception as e: s, serr = None, str(e)
if s is None:
    # No score written at all is still fail-closed (never a fabricated green), but assert what we can.
    emit("PASS","neg.no-green","no score.json written (floor breach fail-closed: %s)"%serr); raise SystemExit
# R20: on a floor breach the overlay AUTHORS the refusal into score.json — score is 0 (NOT a passing
# value) with passed=false + the floor error; the CLI's OverlayOutcome carries no passing score. "No
# fabricated green" is therefore score ∈ {null, 0} (never a positive/finite score).
sc = s.get("score"); emit("PASS" if (sc is None or sc==0) else "FAIL","neg.score-null","got=%r want null-or-0 (no passing score, R20)"%(sc,))
pp = s.get("passed"); emit("PASS" if pp is False else "FAIL","neg.passed-false","got=%r want=false"%(pp,))
err = (s.get("metrics") or {}).get("error","")
emit("PASS" if isinstance(err,str) and err.startswith("performance floor failed") else "FAIL",
     "neg.error names floor","error=%r (want 'performance floor failed' prefix, from artifact)"%(err,))
PY
  feed neg "$PA/sneg.tsv" "$PA/sneg.err" $?
else
  echo ""
  row SKIP "neg-control" "NEG_CONTROL=0 — floor-fail reject path covered by test-paired-offline.sh"
fi

echo ""
RERUN="env GATES_PRODUCER='$GATES_PRODUCER' BENCHCTL='$BENCHCTL' FACADE_CMD='$FACADE_CMD' FACADE_CMD_SHA='${FACADE_CMD_SHA:-}' ENGINE='${ENGINE:-}' GATE_CMD='$GATE_CMD' GATE_CMD_SHA='${GATE_CMD_SHA:-}' SWIFT_REPO_ROOT='$SWIFT_REPO_ROOT' SWIFT='${SWIFT:-}' WEIGHTS='${WEIGHTS:-}' PAIRED_WS='$PAIRED_WS' CONTRACT='$CONTRACT' OFFICIAL_GOLDEN='$OFFICIAL_GOLDEN' GOLDENS='${GOLDENS:-}' TOKENS='$TOKENS' DEPTH='$DEPTH' MIN_PAIRS='$MIN_PAIRS' TARGET_PAIRS='$TARGET_PAIRS' TAG='$TAG' IDENTITY_BAND='$IDENTITY_BAND' OUT='$OUT' bash $HERE/official-paired.sh"
echo "re-run (PROOF A three-seam): $RERUN"
# Per-seam re-run commands (rendered so the REPORT can quote the EXACT command per seam).
if [ "$GATES_PRODUCER" = facade ]; then
  echo "seam1-cmd: env MLXFAST_ENGINE_BIN='$ENGINE' MLXFAST_USE_RUNTIME_WORKER=1 MLXFAST_BENCHMARK_CHECK_GATES=1 MLXFAST_BENCHMARK_SKIP_TIMED=1 MLXFAST_SCORE_PATH='$GATES_SCORE' MLXFAST_CORRECTNESS_GOLDEN_PATH='$OFFICIAL_GOLDEN' MLXFAST_WEIGHTS_PATH='$WEIGHTS' BENCHCTL='$BENCHCTL' bash '$FACADE_CMD' --official  # GATES_PRODUCER=facade (R22 full-parity; benchctl --mode official loads --weights directly)${FACADE_CMD_SHA:+ (FACADE_CMD sha-pinned)}"
elif [ "$GATES_PRODUCER" = benchmark-sh ]; then
  echo "seam1-cmd: env MLXFAST_BENCHMARK_CHECK_GATES=1 MLXFAST_BENCHMARK_SKIP_TIMED=1 MLXFAST_SCORE_PATH='$GATES_SCORE' MLXFAST_CORRECTNESS_GOLDEN_PATH='$OFFICIAL_GOLDEN' MLXFAST_WEIGHTS_PATH='$WEIGHTS' bash '$GATE_CMD' --official  # GATES_PRODUCER=benchmark-sh${GATE_CMD_SHA:+ (GATE_CMD sha-pinned)}"
else
  echo "seam1-cmd: env MLXFAST_OFFICIAL_BENCHMARK_RUN=1 MLXFAST_BENCHMARK_CHECK_GATES=1 MLXFAST_BENCHMARK_SKIP_TIMED=1 '$SWIFT' benchmark --weights '$WEIGHTS' --golden '$OFFICIAL_GOLDEN' --score-path '<untrusted>'  # GATES_PRODUCER=direct-swift (weightless fallback; seal=STDOUT)"
fi
SEAM2_WEIGHTS_CMD=""; [ -n "$WEIGHTS" ] && SEAM2_WEIGHTS_CMD="--weights '$WEIGHTS' "
# R7: render one --golden per pool entry (the exact repeatable form measure-job receives).
SEAM2_GOLDEN_CMD=""; for _g in ${GOLDENS:-$OFFICIAL_GOLDEN}; do SEAM2_GOLDEN_CMD="$SEAM2_GOLDEN_CMD--golden '$_g' "; done
# LANE 2a: render --correctness-golden EXACTLY as run_measure passes it (only when the fixture pins
# the hidden correctness golden), so the reproduced call matches the run that actually executed.
SEAM2_CG_CMD=""; [ -n "$(contract_hidden_correctness_golden_sha)" ] && SEAM2_CG_CMD="--correctness-golden '$OFFICIAL_GOLDEN' "
# R14: QMTP_* env forwarded to measure-job (rendered so the REPORT can reproduce the exact call).
SEAM2_QMTP_CMD=""
[ -n "${QMTP_TARGET_DIR:-}" ]         && SEAM2_QMTP_CMD="${SEAM2_QMTP_CMD}QMTP_TARGET_DIR='$QMTP_TARGET_DIR' "
[ -n "${QMTP_HEAD_DIR:-}" ]           && SEAM2_QMTP_CMD="${SEAM2_QMTP_CMD}QMTP_HEAD_DIR='$QMTP_HEAD_DIR' "
[ -n "${QMTP_CANDIDATE_HEAD_DIR:-}" ] && SEAM2_QMTP_CMD="${SEAM2_QMTP_CMD}QMTP_CANDIDATE_HEAD_DIR='$QMTP_CANDIDATE_HEAD_DIR' "
echo "seam2-cmd: env ${SEAM2_QMTP_CMD}$BENCHCTL measure-job --candidate '$PAIRED_WS' --baseline '$PAIRED_WS' ${SEAM2_WEIGHTS_CMD}${SEAM2_GOLDEN_CMD}--contract '$CONTRACT' ${SEAM2_CG_CMD}--tokens $TOKENS --mtp-depth $DEPTH --min-pairs $MIN_PAIRS --target-pairs $TARGET_PAIRS --tag '$TAG' --gates-producer '$GATES_PRODUCER' --out '$PA'"
echo "seam3-cmd: (cd '$GATES_WS' && $BENCHCTL overlay-timing --gates-score '$(official_abs "$GATES_SCORE")' --results '$(official_abs "$RESULTS")' --score-path '$(official_abs "$SCORE")' --integrity '$(official_abs "$RESULTS_INTEGRITY")')  # cd = the seam-1 gates workspace root; the cross-leg harness-identity gate resolves the identity from THIS cwd"

if [ "$FAIL" -eq 0 ]; then
  echo "official-paired: RESULT PASS — three-seam identity chain green (gates partial→measure identity→overlay merged, score within the identity band ⊆ artifact floor/ceiling$([ "$NEG_CONTROL" = 1 ] && echo '; neg-control floor-fail nulls the score'))"
  exit 0
else
  echo "official-paired: RESULT FAIL — $FAIL assertion(s) failed across the three seams; see $PA/ and the table above" >&2
  exit 1
fi
