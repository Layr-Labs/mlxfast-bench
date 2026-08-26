#!/bin/bash
# scripts/run-paired-window.sh — the ONE human-triggered driver for the PAIRED-decode-only GPU
# window. Proves the Option-A THREE-SEAM paired ranked flow (PROOF A) AND re-runs the existing B-3
# official battery UNCHANGED (PROOF B), in a SINGLE GPU window under a SINGLE lock + SINGLE qwen
# unload/reload.
#
# Mirrors run-official-window.sh hardening VERBATIM: takes + HOLDS the real gpu-exclusive lock
# (fd 9) for the whole run (inner calls run DIRECTLY, never re-wrapped), box-quiet precheck,
# binary-existence checks, deterministic calibrated-golden assembly, golden pin+load gate, a
# GPU-FREE differ self-test (PROOF B legs use the differ) BEFORE qwen is touched, sources
# qwen-service.sh + verifies its functions, unloads qwen, runs the battery, and RELOADS qwen
# ALWAYS via a reentrant trap that then replicates the artifacts (P-3). Startup wipe, ONE REPORT.md.
# Anti-fabrication: a missing artifact at ANY seam → TOOL-ERR (never a silent pass); qwen ALWAYS reloads.
#
# PROOF A — the THREE-SEAM paired IDENTITY chain (scripts/official-paired.sh, finding 15):
#   seam 1  gates ($GATES_PRODUCER)  <gate-cmd> --official, CHECK_GATES=1 SKIP_TIMED=1 → gates-score.json
#   seam 2  benchctl measure-job  --candidate WS == --baseline WS (IDENTITY) → results.json
#   seam 3  benchctl overlay-timing  merge gates+results → ranked score.json (+ neg-control floor-fail)
# PROOF B — the EXISTING B-3 battery, UNCHANGED (same env wiring run-official-window.sh uses):
#   1+3. official-parity.sh      — 3-pair official parity + artifact byte-rows
#   2.   official-failure-map.sh — official failure map + oracle-both-fail + submit-1024 band fixture
#   4.   official-env-probe.sh   — #47 closing evidence (local-iterate spawn; GPU-free)
#
# NO scheduler, NO CI — a human runs this and reads REPORT.md. Paths default to the box layout.
# BENCHCTL MUST point at the paired-branch (`paired-official-flow`) build of benchctl (native
# measure-job + overlay-timing subcommands).
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
G="${MLXFAST_PARITY_GIT:-$HOME/mlxfast-parity-git}"
PAR="${MLXFAST_PARITY_HOME:-$HOME/mlxfast-parity}"
ENGINE="${ENGINE:-$G/mlxfast-engine/.build/release/mlxfast-engine}"
SWIFT="${SWIFT:-$G/mlxfast-challenge-dev/.build/release/mlxfast-swift}"
# LANE 2b — the gate-attested engine-binary sha (WP_ENGINE_BIN_SHA256, sealed by
# window-preflight.sh) forwarded to official-paired.sh so it re-verifies the engine binary identity
# at EACH leg (per-phase re-verification). Sourced from the gate seal when present; a window that
# does not pin it leaves the per-leg check as an opt-in NOTE (official-paired.sh's own default).
OFFICIAL_ENGINE_BIN_SHA256="${OFFICIAL_ENGINE_BIN_SHA256:-${WP_ENGINE_BIN_SHA256:-}}"
# LANE 2b (#148) — the gate seal of the WORKER the direct-swift producer's mlxfast-swift HARNESS
# spawns. official_swift_run hands it to the harness as MLXFAST_RUNTIME_WORKER_EXECUTABLE_SHA256 +
# REQUIRED=1, arming the #31 belt to verify that worker at spawn. Sourced from the swift-worker's
# OWN gate seal (WP_SWIFT_WORKER_BIN_SHA256, sealed by window-preflight.sh's swiftworkerbin
# check_bin) — NOT the engine seal: the direct-swift worker (mlxfast-swift) and the measure-job
# engine (mlxfast-engine) are DIFFERENT binaries, so the engine seal would false-refuse an honest
# worker. A window that does not pin the swift worker leaves the belt opt-in (unarmed).
OFFICIAL_WORKER_BIN_SHA256="${OFFICIAL_WORKER_BIN_SHA256:-${WP_SWIFT_WORKER_BIN_SHA256:-}}"
# The paired flow is native to benchd — BENCHCTL must be the paired-official-flow branch build.
BENCHCTL="${BENCHCTL:-$G/mlxfast-bench/target/release/benchctl}"
PAIRED_BRANCH="${PAIRED_BRANCH:-paired-official-flow}"
WEIGHTS="${WEIGHTS:-$PAR/weights}"
DIFF_CMD="${DIFF_CMD:-$BENCHCTL parity-diff}"
GEN="${GEN:-$HERE/gen-failure-corpus.py}"
SWIFT_REPO_ROOT="${SWIFT_REPO_ROOT:-$G/mlxfast-challenge-dev}"
# ---- PROOF A (three-seam) wiring -----------------------------------------------------------------
# Seam 1 producer: the DEFAULT is the reference benchmark.sh (challenge-dev) that honors
# `--official` + MLXFAST_BENCHMARK_CHECK_GATES/SKIP_TIMED — the organizer's own ranked chain
# (ruling Q1a). benchd's OWN facade scripts/benchmark.sh --official (a full-parity `--official`
# impl → `benchctl iterate --mode official`, loading --weights DIRECTLY — no cached transformed
# weights/ needed) is the explicit GATES_PRODUCER=facade opt-in, for parity testing. Both emit a
# partial_result=true gates-score.
FACADE_CMD="${FACADE_CMD:-$HERE/benchmark.sh}"
FACADE_CMD_SHA="${FACADE_CMD_SHA:-}"
GATE_CMD="${GATE_CMD:-$SWIFT_REPO_ROOT/benchmark.sh}"
# Seam 2 identity: the on-box BUILT WORKSPACE measure-job spawns each leg's sandboxed worker from.
# candidate==baseline==PAIRED_WS ⇒ identity (raw ratio ≈ 1.00). measure-job resolves each leg's
# engine as <PAIRED_WS>/.build/release/mlxfast-engine (override the bin via MLXFAST_MEASURE_WORKER_BIN)
# and spawns `<engine> runtime-worker --weights <WEIGHTS>` — the WORKSPACE and the WEIGHTS are
# DIFFERENT paths. Default WS is the engine workspace ($G/mlxfast-engine), whose release build is
# $G/mlxfast-engine/.build/release/mlxfast-engine (mirrors run-official-window.sh's ENGINE layout).
# R6: WEIGHTS is an OPTIONAL OVERRIDE of measure-job's --weights (the approved draft CLI has none);
# this window carries a concrete WEIGHTS (below) because PROOF B's official legs also load it, and
# passes it through as the override. When unset, measure-job derives the weights DIR from the env
# QMTP_TARGET_DIR (the draft's on-box source) and fails closed if neither is set.
# UNVERIFIED(measure-job): the live workspace→sandboxed-worker spawn — Proof A is the FIRST to
# exercise it; the operator MAY point PAIRED_WS at a distinct on-box built workspace.
PAIRED_WS="${PAIRED_WS:-$G/mlxfast-engine}"
# Seam 2 contract: the track fixture (timed_prompt_pool + calibration). DRAFT-WF @148-153; the
# on-box path + field reads are UNVERIFIED(B-4).
CONTRACT="${CONTRACT:-$G/fixtures/qwen3_6_27b_mtp_track.json}"
TOKENS="${TOKENS:-512}"          # R13: default --tokens is 512 (was 128); depth-0 decode window both legs time
DEPTH="${DEPTH:-2}"              # R13: passed to measure-job as --mtp-depth (>= 2; serial control is depth-0)
MIN_PAIRS="${MIN_PAIRS:-3}"; TARGET_PAIRS="${TARGET_PAIRS:-4}"
TAG="${TAG:-qwen-mtp-paired-identity}"
# 0.10 = 1 − paired floor(0.90): the widest identity band whose lower edge still clears the floor
# read from score.json, so the band stays ⊆ [floor,ceiling] (finding-15 coherence). See official-paired.sh.
IDENTITY_BAND="${IDENTITY_BAND:-0.10}"
NEG_CONTROL="${NEG_CONTROL:-1}"
# --- seam-1 producer selector + R10 sha-pins -----------------------------------------------------
# RULED (David 2026-08-20, completion-gate sign-off Q1a): the default is `benchmark-sh`, the
# sha-pinned REFERENCE ranked chain — a scoring-bearing window mirrors the organizer's trust
# boundary by default. This driver passes GATES_PRODUCER through explicitly (below), so its default
# has to track official-paired.sh's or it would silently override the ruling. `facade` (benchd's own
# scripts/benchmark.sh --official; loads --weights directly, so NO cached transformed weights/
# provisioning) is the EXPLICIT OPT-IN for parity testing; direct-swift is the WEIGHTLESS fallback.
# NOTE: benchmark-sh REQUIRES a provisioned on-box `weights/` — an ops step this driver does not do.
# GATE_CMD_SHA / FACADE_CMD_SHA (optional) sha-pin the respective producer before it runs.
GATES_PRODUCER="${GATES_PRODUCER:-benchmark-sh}"
GATE_CMD_SHA="${GATE_CMD_SHA:-}"
# The command the report names for seam 1. Resolved from the SELECTOR, never hardcoded:
# both hardcodings this replaced were wrong in opposite directions (one named the facade
# unconditionally, one named the reference and claimed the facade refuses --official,
# which R22 retracted).
case "$GATES_PRODUCER" in
  facade)       SEAM1_PRODUCER_CMD="$FACADE_CMD" ;;
  direct-swift) SEAM1_PRODUCER_CMD="${SWIFT:-mlxfast-swift} benchmark" ;;
  *)            SEAM1_PRODUCER_CMD="$GATE_CMD" ;;
esac
# --- new measure-job CLI: golden POOL + QMTP_* head/target dirs (R7/R14) -------------------------
# GOLDENS is a whitespace/newline pool of DISTINCT golden files → repeatable --golden (the 8-prompt
# pool; a real ranked pool needs DISTINCT digests — dup-digest is FATAL in measure-job). Unset ⇒ the
# single OFFICIAL_GOLDEN (cardinality-1 identity/parity framing). QMTP_TARGET_DIR (backbone/target
# cache measure-job derives weights from when --weights is unset), QMTP_HEAD_DIR (pinned serial MTP
# head), QMTP_CANDIDATE_HEAD_DIR (candidate BYO head; defaults to QMTP_HEAD_DIR) forward when set.
GOLDENS="${GOLDENS:-}"
QMTP_TARGET_DIR="${QMTP_TARGET_DIR:-}"
QMTP_HEAD_DIR="${QMTP_HEAD_DIR:-}"
QMTP_CANDIDATE_HEAD_DIR="${QMTP_CANDIDATE_HEAD_DIR:-}"
# submit-1024.json — the stale-baseline SOURCE for the calibrated golden AND the PINNED Leg-2
# band-failure fixture (RULING 2). Same pins run-official-window.sh uses.
SUBMIT1024_GOLDEN="${SUBMIT1024_GOLDEN:-$G/golden/submit-1024.json}"
SUBMIT1024_PIN_SHA="${SUBMIT1024_PIN_SHA:-a482f223edaa5b0b58e6ef0d1d276122f1a4b43f81ca6af33184cc0a64e726c9}"
SUBMIT1024_PIN_BYTES="${SUBMIT1024_PIN_BYTES:-20993}"
# OFFICIAL golden (RULING 1): the BOX-CALIBRATED golden. In the paired flow its baselines are unused
# for scoring; it is consumed for its prompts + benchmark oracle by BOTH the gates (seam 1) and the
# measure-job (seam 2). Same pin as B-3.
OFFICIAL_GOLDEN="${OFFICIAL_GOLDEN:-$G/golden/official-calibrated-1024.json}"
OFFICIAL_PIN_SHA="${OFFICIAL_PIN_SHA:-5ac88f059f97627826951dc411e5c346ccf283509a2a50f9cbc4015119c4a936}"
OFFICIAL_PIN_BYTES="${OFFICIAL_PIN_BYTES:-20975}"
ASSEMBLE="${ASSEMBLE:-$HERE/assemble-official-golden.sh}"
PAIRS="${PAIRS:-3}"
LOCK="${MLXFAST_GPU_LOCK:-/tmp/mtplx-gpu-exclusive.lock}"
# R11: an ATOMIC mkdir BOX_LOCK taken IN ADDITION to the flock(fd 9) — two dialects so the window
# mutually excludes a peer using EITHER mechanism (a flock-only holder and an mkdir-only holder are
# both fenced out). Both are released in cleanup.
BOX_LOCK="${MLXFAST_BOX_LOCK:-/tmp/mtplx-box-exclusive.lock.d}"
# R11 post-reload health probe: the served model id to grep in /v1/models + the bounded wait (>=180s;
# the 27B load is ~85s+ and can crash-loop the launchd throttle). QWEN_LAUNCHD_LABEL (if set) enables
# a `launchctl kickstart -k` retry inside the probe when the worker proc is down.
SERVED_MODEL_ID="${SERVED_MODEL_ID:-${QWEN_SERVED_MODEL_ID:-qwen}}"
QWEN_HEALTH_TIMEOUT="${QWEN_HEALTH_TIMEOUT:-180}"
OUT="${OUT:-$G/golden/paired-window}"
PROOFA_OUT="$OUT/proof-a"
PARITY_OUT="$OUT/parity"; FMAP_OUT="$OUT/failure-map"; PROBE_OUT="$OUT/env-probe"
REPORT="$OUT/REPORT.md"
LOCK_POLICY="outer-hold / inner-direct — driver takes its own flock(fd 9) AND INHERITS the gate-held mkdir BOX_LOCK under WP_WINDOW_TAG for the whole run (two dialects; a peer using either mechanism is fenced out; the inherited lock is released by the gate --release, not this driver); all legs' inner calls are unwrapped → whole-run exclusivity"

# --- P-3 artifact replica target (resolved up-front so the REPORT can name it) ----------
RUN_TS="$(date +%Y%m%dT%H%M%S)"
REPLICA_LOCAL="${REPLICA_LOCAL:-$HOME/parity-artifact-replica/paired-window-$RUN_TS}"
if [ -n "${REPLICA_TARGET:-}" ]; then REPLICA_DESC="$REPLICA_TARGET (offsite, rsync -az)"; else REPLICA_DESC="$REPLICA_LOCAL (box-local; offsite pull pending — P-3)"; fi

mkdir -p "$OUT"
log() { echo "$@" | tee -a "$OUT/run.log"; }
# bench#143 LOW: the anti-stale run.log RESET + artifact wipe used to run HERE, before the gate — so
# a REFUSED window deleted the previous run's artifacts. Both are DEFERRED to the commit point below
# (after the gate PASSED and the gate-held lock was inherited). Until then log() only APPENDS, so a
# refused/aborted window leaves the prior run's run.log and artifacts intact.

# shellcheck source=scripts/parity-lib.sh
. "$HERE/parity-lib.sh"
# shellcheck source=scripts/official-lib.sh
. "$HERE/official-lib.sh"

OFFICIAL_COMMIT="$(official_commit_sha40 "$G/mlxfast-bench")"

# --- SCORING-WINDOW PREFLIGHT GATE (bench#143 wire a) -----------------------------------
# WHAT IS A SCORING WINDOW HERE: this driver has NO smoke-only / dry-run / release mode. It
# parses no argv and always runs PROOF A — the three-seam paired chain (official-paired.sh) that
# SEALS a ranked score.json (seam 3). Every invocation therefore starts a scoring window, so the
# gate below is UNCONDITIONAL. (If a non-scoring mode is ever added, key this off it; until then
# the honest reading of the script's own control flow is "always scoring".)
#
# THE GAP THIS CLOSES: a scoring window that runs WITHOUT the gate skips the ONLY place the env-seam
# pins are verified: the worker-executable sha (WP_ENGINE_BIN_SHA256), the pinned
# MLXFAST_RUNTIME_WORKER_EXECUTABLE / MLXFAST_NO_SANDBOX env, the contract/bundle shas, the smoke
# leg — and spawns an UNVERIFIED worker binary. Two motions close it: this gate REFUSES unless the
# gate ran and PASSED, and the box-lock block below is INHERIT-ONLY (bench#143 MEDIUM) — the driver
# never self-acquires the lock, so it can only run inside the gate's live acquire-and-hold window.
#
# HOW "ran and PASSED" IS PROVED: window-preflight.sh seals a window-provenance/v1 attestation next
# to its run artifacts and, on PASS, HOLDS the box lock under WP_WINDOW_TAG and hands that tag off
# (the holder-tag inheritance below). We require, from the attestation the operator points us at:
#   - schema == "window-provenance/v1"      (this is a preflight attestation, not some other json)
#   - .verdict == "PASS"                     (a FAILED gate unwinds and releases; it must not score)
#   - .lock.window_tag == WP_WINDOW_TAG      (THIS window's attestation, not a stale PASS reused)
#   - .lock_taken == true                    (the gate actually took/handed off the lock)
# WP_ATTESTATION names the file; it defaults to $WP_OUT/window-provenance.json (the gate's own
# --out convention). Missing tag, missing/stale attestation, or any check ❌ ⇒ REFUSE (exit 3).
#
# DECIDE (policy, NOT settled here — see the ruling question in the PR): the preflight pins this
# gate leans on are SELF-DECLARED OPERATOR values today, not organizer-signed baselines. This wire
# binds the mechanism to the pin AS IT EXISTS; whether the pin must be organizer-signed is David's.
WP_ATTESTATION="${WP_ATTESTATION:-${WP_OUT:+$WP_OUT/window-provenance.json}}"

# Pure predicate: echoes "PASS" or a single refusal-reason token. Never exits, reads only its args
# + jq, so the offline suite can evaluate it out of this file directly (revert-proof rows).
preflight_attestation_verdict() {
  local tag="${1:-}" att="${2:-}"
  [ -n "$tag" ] || { printf 'no-window-tag'; return 0; }
  [ -n "$att" ] || { printf 'no-attestation-path'; return 0; }
  [ -r "$att" ] || { printf 'attestation-unreadable'; return 0; }
  command -v jq >/dev/null 2>&1 || { printf 'jq-absent'; return 0; }
  local schema verdict atag taken
  schema="$(jq -r '.schema // ""' "$att" 2>/dev/null)"
  [ "$schema" = "window-provenance/v1" ] || { printf 'wrong-schema:%s' "${schema:-none}"; return 0; }
  verdict="$(jq -r '.verdict // ""' "$att" 2>/dev/null)"
  [ "$verdict" = "PASS" ] || { printf 'verdict-not-pass:%s' "${verdict:-none}"; return 0; }
  atag="$(jq -r '.lock.window_tag // ""' "$att" 2>/dev/null)"
  [ "$atag" = "$tag" ] || { printf 'tag-mismatch:%s!=%s' "${atag:-none}" "$tag"; return 0; }
  taken="$(jq -r '.lock_taken // false' "$att" 2>/dev/null)"
  [ "$taken" = "true" ] || { printf 'lock-not-taken:%s' "${taken:-none}"; return 0; }
  printf 'PASS'; return 0
}

require_passed_preflight() {
  local v; v="$(preflight_attestation_verdict "${WP_WINDOW_TAG:-}" "${WP_ATTESTATION:-}")"
  if [ "$v" != "PASS" ]; then
    log "SCORING WINDOW REFUSED: window-preflight.sh did not run and PASS for this window ($v)."
    log "  A scoring window must be gated: run scripts/window-preflight.sh --pins <FILE> --out <DIR>"
    log "  first (it verifies the env-seam pins incl. the worker-executable sha and holds the box"
    log "  lock), then re-run this driver with WP_WINDOW_TAG=<tag> WP_ATTESTATION=<DIR>/window-provenance.json"
    log "  (or WP_OUT=<DIR>). Refusing before taking any lock or touching the serving model."
    exit 3
  fi
  log "scoring-window preflight gate: PASSED (tag '$WP_WINDOW_TAG', attestation '$WP_ATTESTATION')."
}
require_passed_preflight

# --- take + HOLD the GPU lock first (fd 9 held for this script's lifetime) --------------
log "=== take GPU lock @ $(date) ==="
parity_take_gpu_lock "$LOCK"; LOCK_RC=$?
if [ "$LOCK_RC" -ne 0 ]; then log "GPU lock unavailable (rc=$LOCK_RC) — aborting; re-run when the box is free."; exit 3; fi
log "GPU lock held (fd 9) for the run."

# --- R11: the mkdir BOX_LOCK — INHERIT-ONLY (bench#143 MEDIUM) ----------------------------------
# A scoring window may take the mkdir BOX_LOCK ONLY by INHERITING a lock the gate is CURRENTLY
# holding under this window's tag — it runs INSIDE the gate's acquire-and-hold window (#138). It
# must NEVER self-acquire. require_passed_preflight above already accepted an attestation, and an
# attestation with lock_taken=true beside a currently-FREE BOX_LOCK is proof that attestation's
# window ALREADY ENDED: the same operator-chosen WP_WINDOW_TAG + an OLD attestation, re-run after
# the lock was released or reaped, would otherwise self-acquire a scoring window whose LIVE env was
# never re-verified (STALE REPLAY). So: inherit a live same-tag holder, abort on a foreign holder,
# and REFUSE when the lock is FREE. The flock(fd 9) above is still this driver's own dialect.
if [ -d "$BOX_LOCK" ] \
  && [ "$(sed -n 's/^tag=//p' "$BOX_LOCK/holder" 2>/dev/null | head -1)" = "${WP_WINDOW_TAG:-}" ]; then
  # HOLDER-TAG INHERITANCE (RULED 2026-08-20). window-preflight.sh takes this same BOX_LOCK before
  # its smoke leg and HOLDS it through the window, so single-flight covers the gap between preflight
  # and run. The holder's tag matches the tag we were handed: the lock is already ours. This driver
  # never RELEASES it — releasing an INHERITED lock belongs to the gate (`--release`, which also
  # reloads the serving model).
  log "BOX_LOCK $BOX_LOCK INHERITED from the window-preflight gate (tag $WP_WINDOW_TAG) — not acquiring, not releasing."
  # X-1: INHERITING IS NOT ENOUGH — ADOPT IT. The pid in the lock is the gate's box-side PROBE,
  # which exited by design the moment the gate returned. Left as-is the reap predicate would read
  # verifiable + provably-dead + old-enough for the whole window, so a second gate would reap a LIVE
  # window's lock, unload the serving model under it, and seal a verified_dead_how that is a forgery.
  # The pid field has only ever meant "the process whose liveness vouches for this lock": during the
  # gate that is the probe; during the window it is THIS driver. The handoff transfers that duty, so
  # the driver signs for it. If this driver crashes its pid dies and the lock becomes reapable again
  # — the correct outcome; a marker outliving its process turns every abandoned window into a brick.
  printf '%s\n' "$$" > "$BOX_LOCK/pid" 2>/dev/null || true
  printf 'adopted_pid=%s\nadopted_utc=%s\n' "$$" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
    >> "$BOX_LOCK/holder" 2>/dev/null || true
  # Refresh the directory mtime too: age is the reaper's second axis, and an inherited lock that
  # keeps its acquisition mtime ages toward the threshold while a live driver holds it.
  touch "$BOX_LOCK" 2>/dev/null || true
elif [ -d "$BOX_LOCK" ]; then
  # A holder exists but its tag is not ours: a FOREIGN window. Never inherit it.
  log "BOX_LOCK $BOX_LOCK is held by a FOREIGN window (pid $(cat "$BOX_LOCK/pid" 2>/dev/null || echo '?'), tag '$(sed -n 's/^tag=//p' "$BOX_LOCK/holder" 2>/dev/null | head -1)') — not ours to inherit; aborting."; exit 3
else
  # The BOX_LOCK is FREE. The attestation said the gate TOOK it (lock_taken=true), so a free lock
  # proves the gate's window has ENDED and this attestation is STALE. A scoring window must run
  # INSIDE the gate's acquire-and-hold window; it must not self-acquire behind a spent attestation.
  log "SCORING WINDOW REFUSED: STALE attestation — the gate-held BOX_LOCK $BOX_LOCK is FREE, so the window it sealed (tag '${WP_WINDOW_TAG:-}') has already ended. Re-run scripts/window-preflight.sh to open a fresh window under a live gate-held lock. Refusing (no BOX_LOCK acquired)."; exit 3
fi

# --- COMMIT POINT: anti-stale run.log reset + artifact wipe (bench#143 LOW) ---------------------
# The gate PASSED and the gate-held lock was INHERITED, so this window is committed and owns the
# box. ONLY now do we reset run.log and wipe prior artifacts — every refusal/abort above left the
# previous run's run.log and artifacts intact. "Any file present at REPORT time was written THIS
# run" holds from here. (The gate/lock lines logged above survive on stdout via tee.)
: > "$OUT/run.log"
rm -rf "$PROOFA_OUT" "$PARITY_OUT" "$FMAP_OUT" "$PROBE_OUT" "$OUT/selftest" 2>/dev/null || true
rm -f "$OUT"/*.table.txt "$REPORT" 2>/dev/null || true

# --- box-quiet precheck ----------------------------------------------------------------
if ! parity_precheck 2>&1 | tee -a "$OUT/run.log"; then
  log "box not quiet — aborting (re-run when quiet)."; exit 3
fi

for b in "$ENGINE" "$SWIFT" "$BENCHCTL"; do
  [ -x "$b" ] || { log "missing binary: $b — aborting."; exit 5; }
done
# PROOF A seam-2 identity workspace + seam-1 gate producer + contract must be present (fail LOUD, not
# a silent pass) — but their INTERNAL wiring is what Proof A exercises for the first time.
[ -e "$PAIRED_WS" ] || { log "missing PAIRED_WS (identity workspace): $PAIRED_WS — set it to the on-box built workspace; aborting."; exit 5; }
[ -r "$CONTRACT" ]  || { log "missing CONTRACT (track fixture): $CONTRACT — aborting."; exit 5; }
[ -x "$GATE_CMD" ] || [ -r "$GATE_CMD" ] || { log "missing seam-1 GATE_CMD (reference benchmark.sh): $GATE_CMD — aborting."; exit 5; }

# --- assemble the BOX-CALIBRATED official golden (RULING 1), deterministically, if absent ------
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
command -v python3 >/dev/null || { log "python3 required — aborting."; exit 5; }
command -v sandbox-exec >/dev/null 2>&1 || { log "sandbox-exec not found — official runs need Seatbelt; aborting."; exit 5; }

# --- golden pin+load gate (delegates to benchctl validate-golden; official needs the oracle) ---
log "=== golden gate @ $(date) — $OFFICIAL_GOLDEN ==="
if ! parity_validate_golden "$BENCHCTL" "$OFFICIAL_GOLDEN" "$OFFICIAL_PIN_SHA" "$OFFICIAL_PIN_BYTES" 2>&1 | tee -a "$OUT/run.log"; then
  log "official golden rejected — aborting."; exit 4
fi

# --- differ self-test (GPU-FREE), BEFORE we touch qwen (PROOF B legs use the differ) -----------
DIFFER_VERSION="$("$BENCHCTL" parity-diff --version 2>/dev/null || echo unknown)"
differ_selftest() {
  local d="$OUT/selftest"; mkdir -p "$d"
  "$BENCHCTL" parity-diff --emit-sample > "$d/a.json" 2>/dev/null || { log "S0: --emit-sample failed"; return 1; }
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

# --- reentrant cleanup trap: reload qwen (only if we unloaded it) + health-probe, replicate, unlock ---
# R11: HUP is trapped too (an ssh drop still reloads qwen + releases the locks). qwen_reload runs ONLY
# when _QWEN_UNLOADED=1 (so an abort BEFORE the unload — e.g. the seam-1 precheck failing — never
# needlessly bounces qwen), and is followed by a POST-RELOAD HEALTH PROBE (bounded >=180s; the 27B
# load is ~85s+ and can crash-loop the launchd throttle). The mkdir BOX_LOCK is released here; the
# flock(fd 9) releases when the shell exits.
_CLEANED=0
_QWEN_UNLOADED=0
cleanup() {
  [ "$_CLEANED" = "1" ] && return 0; _CLEANED=1
  if [ "$_QWEN_UNLOADED" = "1" ]; then
    log "=== RELOAD qwen (cleanup) @ $(date) ==="; qwen_reload
    log "=== qwen post-reload HEALTH PROBE (bounded ${QWEN_HEALTH_TIMEOUT}s, floor 180s; model '$SERVED_MODEL_ID') @ $(date) ==="
    official_qwen_health_probe "$SERVED_MODEL_ID" "$QWEN_HEALTH_TIMEOUT" 2>&1 | tee -a "$OUT/run.log"
    if [ "${PIPESTATUS[0]}" -eq 0 ]; then
      log "qwen health: OK (model served + /health up)."
    else
      log "!!! qwen DID NOT come healthy after reload — INVESTIGATE (launchd may be crash-looping through its throttle); NOT declaring success."
    fi
  fi
  log "=== replicate artifacts (P-3) @ $(date) — $REPLICA_DESC ==="
  official_replicate_artifacts "$OUT" "$REPLICA_LOCAL" 2>&1 | tee -a "$OUT/run.log"
  # The BOX_LOCK is ALWAYS inherited now (inherit-only, bench#143 MEDIUM), never self-acquired, so
  # this driver never releases it: releasing an inherited lock belongs to the gate (`--release`,
  # which also reloads the serving model). The flock(fd 9) releases when this shell exits.
}
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM
trap 'cleanup; exit 129' HUP

# --- R11 HARD-STOP: validate seam 1 (gates, GPU-light) BEFORE unloading qwen --------------------
# The R11 headline: we once burned a whole seam-2 window (qwen unloaded, ~85s reload) on a seam-1
# producer that never produced a valid gates-score.json. So run seam 1 FIRST (SEAM1_ONLY=1) — if it
# does not produce a valid gates-score.json the driver ABORTS here, WITHOUT touching qwen (the
# cleanup trap sees _QWEN_UNLOADED=0 and skips the reload, releasing only the locks).
log "=== R11 seam-1 precheck (SEAM1_ONLY; qwen still loaded) @ $(date) ==="
S1_TABLE="$OUT/seam1-precheck.table.txt"; S1_STDERR="$OUT/seam1-precheck.stderr.txt"
env GATES_PRODUCER="$GATES_PRODUCER" GATE_CMD_SHA="$GATE_CMD_SHA" \
  FACADE_CMD="$FACADE_CMD" FACADE_CMD_SHA="$FACADE_CMD_SHA" ENGINE="$ENGINE" \
  BENCHCTL="$BENCHCTL" GATE_CMD="$GATE_CMD" SWIFT="$SWIFT" SWIFT_REPO_ROOT="$SWIFT_REPO_ROOT" \
  WEIGHTS="$WEIGHTS" PAIRED_WS="$PAIRED_WS" CONTRACT="$CONTRACT" \
  OFFICIAL_ENGINE_BIN_SHA256="$OFFICIAL_ENGINE_BIN_SHA256" \
  OFFICIAL_WORKER_BIN_SHA256="$OFFICIAL_WORKER_BIN_SHA256" \
  OFFICIAL_GOLDEN="$OFFICIAL_GOLDEN" GOLDENS="$GOLDENS" OFFICIAL_COMMIT="$OFFICIAL_COMMIT" \
  TOKENS="$TOKENS" DEPTH="$DEPTH" TAG="$TAG" NEG_CONTROL=0 SEAM1_ONLY=1 OUT="$OUT/seam1-precheck-out" \
  bash "$HERE/official-paired.sh" > "$S1_TABLE" 2> "$S1_STDERR"
S1_RC=$?
if [ "$S1_RC" -ne 0 ]; then
  log "R11 seam-1 precheck FAILED (rc=$S1_RC) — ABORTING before unloading qwen (window NOT spent)."; sed 's/^/  /' "$S1_STDERR" | tee -a "$OUT/run.log"; exit 1
fi
log "R11 seam-1 precheck OK — gates-score.json valid; safe to unload qwen and spend the seam-2 window."

# --- UNLOAD qwen: check the rc AND VERIFY the proc is gone (R11) --------------------------------
# Set _QWEN_UNLOADED=1 BEFORE the attempt so ANY subsequent abort (incl. a rc=0-but-proc-lingering
# verify fail) still reloads qwen via cleanup rather than leaving the box with qwen down.
log "=== UNLOAD qwen (+ verify proc gone) @ $(date) ==="
_QWEN_UNLOADED=1
official_qwen_unload_verify 2>&1 | tee -a "$OUT/run.log"
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
  log "qwen unload/verify FAILED — aborting (qwen was NOT confirmed unloaded; refusing to run the GPU battery). Cleanup will reload + health-probe qwen."; exit 6
fi

COMMON_ENV=(BENCHCTL="$BENCHCTL" ENGINE="$ENGINE" SWIFT="$SWIFT" WEIGHTS="$WEIGHTS" \
  OFFICIAL_GOLDEN="$OFFICIAL_GOLDEN" OFFICIAL_COMMIT="$OFFICIAL_COMMIT" \
  SWIFT_REPO_ROOT="$SWIFT_REPO_ROOT" DIFF_CMD="$DIFF_CMD")

# ======================================================================================
# PROOF A — the THREE-SEAM paired IDENTITY chain (+ floor-fail negative control)
# ======================================================================================
log "=== PROOF A: three-seam identity chain @ $(date) ==="
mkdir -p "$PROOFA_OUT"
PA_TABLE="$OUT/official-paired.table.txt"; PA_STDERR="$OUT/official-paired.stderr.txt"
env GATES_PRODUCER="$GATES_PRODUCER" GATE_CMD_SHA="$GATE_CMD_SHA" \
  FACADE_CMD="$FACADE_CMD" FACADE_CMD_SHA="$FACADE_CMD_SHA" ENGINE="$ENGINE" \
  BENCHCTL="$BENCHCTL" GATE_CMD="$GATE_CMD" SWIFT="$SWIFT" SWIFT_REPO_ROOT="$SWIFT_REPO_ROOT" \
  WEIGHTS="$WEIGHTS" PAIRED_WS="$PAIRED_WS" CONTRACT="$CONTRACT" \
  OFFICIAL_ENGINE_BIN_SHA256="$OFFICIAL_ENGINE_BIN_SHA256" \
  OFFICIAL_WORKER_BIN_SHA256="$OFFICIAL_WORKER_BIN_SHA256" \
  OFFICIAL_GOLDEN="$OFFICIAL_GOLDEN" GOLDENS="$GOLDENS" OFFICIAL_COMMIT="$OFFICIAL_COMMIT" \
  QMTP_TARGET_DIR="$QMTP_TARGET_DIR" QMTP_HEAD_DIR="$QMTP_HEAD_DIR" QMTP_CANDIDATE_HEAD_DIR="$QMTP_CANDIDATE_HEAD_DIR" \
  TOKENS="$TOKENS" DEPTH="$DEPTH" MIN_PAIRS="$MIN_PAIRS" TARGET_PAIRS="$TARGET_PAIRS" \
  TAG="$TAG" IDENTITY_BAND="$IDENTITY_BAND" NEG_CONTROL="$NEG_CONTROL" OUT="$OUT" \
  bash "$HERE/official-paired.sh" > "$PA_TABLE" 2> "$PA_STDERR"
PA_RC=$?
if [ "$PA_RC" -ne 0 ]; then log "PROOF A: three-seam rc=$PA_RC (non-PASS — see below); continuing to render."; sed 's/^/  /' "$PA_STDERR" | tee -a "$OUT/run.log"; fi
PA_ROWS="$(awk -F' *\\| *' 'NF>=3 && $1!="check" && $1 !~ /^-+$/ {print}' "$PA_TABLE")"
[ -n "$PA_ROWS" ] || { log "PROOF A: three-seam produced no table rows — aborting before REPORT."; exit 9; }
PA_S1_CMD="$(grep -h '^seam1-cmd:' "$PA_TABLE" | head -1 | sed 's/^seam1-cmd: //')"
PA_S2_CMD="$(grep -h '^seam2-cmd:' "$PA_TABLE" | head -1 | sed 's/^seam2-cmd: //')"
PA_S3_CMD="$(grep -h '^seam3-cmd:' "$PA_TABLE" | head -1 | sed 's/^seam3-cmd: //')"

# ======================================================================================
# PROOF B — Leg 1+3 : official parity (pre-judge score + det-fields) + artifact byte-rows
# ======================================================================================
log "=== PROOF B / LEG 1+3: official-parity @ $(date) — $PAIRS pairs ==="
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
# PROOF B — Leg 2 : official failure map (+ oracle-both-fail + submit-1024 band-failure FIXTURE)
# ======================================================================================
log "=== PROOF B / LEG 2: official-failure-map @ $(date) ==="
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
# PROOF B — Leg 4 : #47 env-dump probe (GPU-free; runs in-window for convenience)
# ======================================================================================
log "=== PROOF B / LEG 4: official-env-probe (#47) @ $(date) ==="
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
OP_RERUN="env ${COMMON_ENV[*]} OUT='$PARITY_OUT' PAIRS=1 OFFICIAL_PIN_SHA='$OFFICIAL_PIN_SHA' OFFICIAL_PIN_BYTES='$OFFICIAL_PIN_BYTES' bash $HERE/official-parity.sh"
{
  echo "# PAIRED-decode-only window — REPORT (PROOF A three-seam + PROOF B)"
  echo
  echo "Run \`$(date)\` · benchctl \`$COMMIT\` (branch \`$PAIRED_BRANCH\`) · differ \`$DIFF_CMD\` · differ-version \`$DIFFER_VERSION\`"
  echo "Golden \`$(basename "$OFFICIAL_GOLDEN")\` pin \`${OFFICIAL_PIN_SHA:0:12}…\` (${OFFICIAL_PIN_BYTES} B) · official commit \`${OFFICIAL_COMMIT:0:12}…\`"
  echo "**Golden provenance — PARITY-TEST-ONLY (RULING 1).** BOX-CALIBRATED official golden assembled"
  echo "deterministically from \`submit-1024.json\` (pin \`${SUBMIT1024_PIN_SHA:0:12}…\`, ${SUBMIT1024_PIN_BYTES} B). In"
  echo "the paired flow the golden's baselines are unused for scoring; it is consumed for its prompts +"
  echo "benchmark oracle by BOTH seam 1 (gates) and seam 2 (measure-job). **NEVER an organizer/ranking golden.**"
  echo "**Lock policy (measurement condition).** $LOCK_POLICY."
  echo "**Replica (P-3).** $REPLICA_DESC."
  echo "**R11 window hardening.** seam-1 gates validated FIRST (SEAM1_ONLY) while qwen is still loaded —"
  echo "a seam-1 miss aborts WITHOUT spending the seam-2 window; qwen unload is rc-checked + proc-verified;"
  echo "cleanup reloads qwen (HUP-trapped too) then HEALTH-PROBES it (bounded ${QWEN_HEALTH_TIMEOUT}s, floor 180s);"
  echo "the window holds BOTH an atomic mkdir BOX_LOCK and the flock(fd 9)."
  echo
  echo "## PROOF A — the THREE-SEAM paired IDENTITY chain (finding 15)"
  echo "The paired ranked flow is three trusted seams the driver runs in order, asserting each seam's"
  echo "artifact against the next (\`docs/paired-flow-design-note.md\`):"
  echo
  echo "| # | seam | command | artifact |"
  echo "|---|---|---|---|"
  echo "| 1 | gates ($GATES_PRODUCER) | \`<gate> --official\` CHECK_GATES=1 SKIP_TIMED=1 | \`gates-score.json\` (partial_result=true) |"
  echo "| 2 | measure-job | \`benchctl measure-job\` candidate==baseline==\`\$PAIRED_WS\` | \`results.json\` (+ \`benchmark-integrity.results.json\`) |"
  echo "| 3 | overlay (LOCAL) | \`benchctl overlay-timing\` gates+results | \`score.json\` (+ \`.sha256\`) |"
  echo
  echo "**Identity band.** Both legs are the SAME workspace, so any decode-speedup deviation from 1.00"
  echo "is thermal/measurement noise: \`|raw − 1.0| ≤ $IDENTITY_BAND\`. Finding-15 discipline — the band is"
  echo "asserted as a SUBSET of the \`[floor, ceiling]\` **read from the emitted \`score.json\`** (never"
  echo "restated literals), so a green identity score is coherent with the paired gate and a mis-wired"
  echo "denominator lands outside the band and FAILS loud."
  echo "**Negative control.** Seam 3 re-run on a synthesized FLOOR-FAIL \`results.json\` (raw ratios → ~0.5"
  echo "< 0.90 floor) must null the score + exit nonzero — proving the driver cannot fabricate a green"
  echo "past a floor breach (cheap; no GPU)."
  echo "**Contract-derived assumptions (UNVERIFIED until Proof A).** PAIRED_WS \`$PAIRED_WS\` is the on-box"
  echo "built workspace measure-job spawns each leg from (workspace→worker spawn UNVERIFIED(measure-job));"
  echo "CONTRACT \`$(basename "$CONTRACT")\` is the track fixture (field reads UNVERIFIED(B-4)); the seam-1"
  echo "producer is \`$GATES_PRODUCER\` (\`$SEAM1_PRODUCER_CMD\`)."
  echo
  echo "| check | verdict | detail |"
  echo "|---|---|---|"
  printf '%s\n' "$PA_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s |\n",$1,$2,$3}'
  echo
  echo "PROOF A verdict: $( [ "$PA_RC" -eq 0 ] && echo 'PASS — three-seam identity chain green; neg-control floor-fail nulls the score' || echo "NON-PASS (rc=$PA_RC) — see official-paired.stderr.txt" )"
  echo
  echo "**Re-run per seam (EXACT commands):**"
  echo "- seam 1 (gates): \`${PA_S1_CMD:-see official-paired.table.txt}\`"
  echo "- seam 2 (measure-job): \`${PA_S2_CMD:-see official-paired.table.txt}\`"
  echo "- seam 3 (overlay): \`${PA_S3_CMD:-see official-paired.table.txt}\`"
  echo
  echo "## PROOF B / Leg 1 — official parity (pre-judge score + deterministic fields; TIMING within band, JUDGE-LESS)"
  echo "| pair | bc.passed | sw.passed | det-fields |"
  echo "|---|---|---|---|"
  printf '%s\n' "$OP_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s | %s |\n",$1,$2,$3,$4}'
  echo
  echo "## PROOF B / Leg 3 — artifact byte-rows (score.json / .sha256 / 9-field integrity / exit)"
  echo "| pair | score-name | .sha256 | integrity | exit | overall |"
  echo "|---|---|---|---|---|---|"
  printf '%s\n' "$OP_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s | %s | %s | %s |\n",$1,$5,$6,$7,$8,$9}'
  echo
  echo "## PROOF B / Leg 2 — official failure map (incl. the oracle class + submit-1024 band-fail fixture)"
  echo "| class | bc.passed | sw.passed | shared-surface diff |"
  echo "|---|---|---|---|"
  printf '%s\n' "$FM_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s | %s |\n",$1,$2,$3,$4}'
  echo
  echo "**Oracle assertion.** ${FM_ORACLE_ASSERT:-（assertion line missing — see official-failure-map.stderr.txt）}"
  echo "**Band-failure fixture (RULING 2).** ${FM_BAND_ASSERT:-（assertion line missing — see official-failure-map.stderr.txt）}"
  echo
  echo "## PROOF B / Leg 4 — #47 closing evidence (sanitized allowlist-only child env; GPU-free)"
  if [ -n "$EP_ROWS" ]; then
    echo "| var (bucket) | verdict | detail |"
    echo "|---|---|---|"
    printf '%s\n' "$EP_ROWS" | awk -F' *\\| *' '{printf "| %s | %s | %s |\n",$1,$2,$3}'
  else
    echo "_(no probe rows — see official-env-probe.stderr.txt; a missing child-env capture is TOOL-ERR)_"
  fi
  echo
  echo "## Verdict"
  echo "- PROOF A three-seam identity chain + floor-fail neg-control: $( [ "$PA_RC" -eq 0 ] && echo 'PASS' || echo "NON-PASS (rc=$PA_RC) — see official-paired.stderr.txt" )"
  echo "- PROOF B Leg 1+3 official parity/artifacts: $( [ "$OP_RC" -eq 0 ] && echo 'PASS — all pairs GREEN' || echo "NON-PASS (rc=$OP_RC) — see official-parity.stderr.txt" )"
  echo "- PROOF B Leg 2 failure map + oracle + band-fail fixture: $( [ "$FM_RC" -eq 0 ] && echo 'PASS' || echo "NON-PASS (rc=$FM_RC) — see official-failure-map.stderr.txt" )"
  echo "- PROOF B Leg 4 #47 env probe (local-iterate): $( [ "$EP_RC" -eq 0 ] && echo 'PASS — allowlist-only child env' || echo "NON-PASS (rc=$EP_RC) — see official-env-probe.stderr.txt" )"
  echo
  echo "benchctl \`$COMMIT\` on branch \`$PAIRED_BRANCH\` · golden pin \`${OFFICIAL_PIN_SHA:0:12}…\` (${OFFICIAL_PIN_BYTES} B) · P-3 replica \`$REPLICA_DESC\`."
  echo "Artifacts: proof-a \`$PROOFA_OUT/\`; parity \`$PARITY_OUT/\`; failure-map \`$FMAP_OUT/\`; env-probe \`$PROBE_OUT/\`; logs \`$OUT/run.log\`."
  echo "**Re-run (PROOF B, one parity pair):** \`$OP_RERUN\`"
} > "$REPORT"

log "=== REPORT written: $REPORT ==="
echo "----- REPORT.md -----"
cat "$REPORT"

# Overall exit: non-zero if ANY proof/leg was non-PASS (qwen still reloads via the trap).
[ "$PA_RC" -eq 0 ] && [ "$OP_RC" -eq 0 ] && [ "$FM_RC" -eq 0 ] && [ "$EP_RC" -eq 0 ]
