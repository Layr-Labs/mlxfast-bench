#!/bin/bash
# scripts/official-lib.sh — shared B-3 official-parity primitives, sourced (never executed).
#
# ONE definition each of the offline-validatable pieces the official (Track-B) window legs need,
# so the parity leg (official-parity.sh), the failure map (official-failure-map.sh), the #47
# env probe (official-env-probe.sh), and the offline self-test (test-official-offline.sh) all
# share the SAME logic instead of forking drifting copies (mirrors parity-lib.sh / variant-lib.sh
# intent). Portable to bash 3.2 (stock macOS + the box): no mapfile, no associative arrays.
# Every function fails LOUD (self-identifying line + non-zero) — never a silent pass.
#
#   official_seatbelt_profile   — emit the exact Seatbelt profile text (port of
#                                 bench_runner::sandbox::build_seatbelt_profile AND
#                                 benchmark.sh write_runtime_worker_sandbox_profile). Both the
#                                 direct-swift side and (as an Override) the probe use it; benchctl
#                                 self-generates the identical text from the same builder.
#   official_swift_run          — run DIRECT `mlxfast-swift benchmark` in OFFICIAL env (NOT the
#                                 protected benchmark.sh workflow), capture stdout, SEAL it the way
#                                 benchmark.sh does (single valid payload → score.json), and write
#                                 the .sha256 + 9-field integrity sidecars. Fails loud; never seals
#                                 a non-payload as a pass.
#   official_passed_of          — read `.passed` from a sealed score (MISSING/ERR on trouble).
#   official_render_verdict     — no-undeclared-cells rule: FAIL + declared(#nn) → DECLARED(#nn).
#   official_commit_sha40       — a deterministic 40-hex commit id both sides stamp (metrics.commit
#                                 must MATCH, so it is handed to both sides identically).

if [ -n "${_OFFICIAL_LIB_SOURCED:-}" ]; then return 0 2>/dev/null || true; fi
_OFFICIAL_LIB_SOURCED=1

# sha256 of a file (stock macOS + box).
official_sha_of() { shasum -a 256 "$1" | awk '{print $1}'; }

# relativize_for_seal <path> — the SHELL MIRROR of benchctl's `relativize_for_seal`
# (crates/benchctl/src/main.rs). A relative path is kept as-is; an absolute path is made relative to
# the current working directory (the run's workspace root) when it lies under it, else its $HOME
# prefix is stripped (dropping the username with it), else a leading /Users/<user>/ or /home/<user>/
# head is dropped. The result never begins with a user-home segment; a non-home absolute path
# (e.g. /opt/…) carries no home and is kept.
#
# WHY IT LIVES ON BOTH SIDES. The bc (Rust) and sw (this) integrity sidecars must seal `weights_path`
# with the SAME reduction — official-parity.sh Leg 3c value-compares it in FULL (only score_path is
# basename-normalized), so a raw sw side against a relativized bc side would DIVERGE whenever the
# weights dir is under the operator home (the fleet case). Sealing it identically here makes the
# parity hold by construction and closes the reference sidecar's own home-dir leak. Keep this
# SEMANTICALLY IDENTICAL to the Rust version. Bash 3.2 (stock macOS + box): no arrays, no mapfile.
relativize_for_seal() {
  local p="$1"
  # A relative path is already leak-free.
  case "$p" in /*) : ;; *) printf '%s' "$p"; return 0 ;; esac
  # Reduce an absolute path: (1) under $PWD, (2) under the operator's own $HOME, (3) drop a
  # /Users/<user>/ or /home/<user>/ head — INCLUDING a bare home ROOT (/Users/<user>, /home/<user>)
  # with NO trailing component, which a naive head-strip left absolute and SEALED (secret-tier leak).
  # After this, no absolute home path can survive. String-boundary tests (not case globs) for $PWD /
  # $HOME so a value containing glob metacharacters cannot mis-slice the path.
  if [ -n "$PWD" ] && [ "$p" = "$PWD" ]; then
    p="."
  elif [ -n "$PWD" ] && [ "${p#"$PWD"/}" != "$p" ]; then
    p="${p#"$PWD"/}"
  elif [ -n "${HOME:-}" ] && [ "$p" = "$HOME" ]; then
    p="."
  elif [ -n "${HOME:-}" ] && [ "${p#"$HOME"/}" != "$p" ]; then
    p="${p#"$HOME"/}"
  else
    case "$p" in
      /Users/*/*) p="${p#/Users/*/}" ;;   # /Users/<user>/<tail> → <tail>
      /home/*/*)  p="${p#/home/*/}" ;;     # /home/<user>/<tail>  → <tail>
      /Users/*|/home/*) p="." ;;           # bare home ROOT (own or foreign) → "."
    esac
  fi
  # Never seal an empty provenance string (a home dir can no longer be absolute at this point).
  case "$p" in "") p="." ;; esac
  printf '%s' "$p"
}

# --- Seatbelt profile text (byte-shape port) -------------------------------------------
# official_seatbelt_profile <executable_abs> <golden_abs> [private_dir_abs]. Emits the exact
# profile bench_runner builds (build_seatbelt_profile, sandbox.rs) and benchmark.sh writes
# (write_runtime_worker_sandbox_profile, :700-719): allow-default, deny net/fork/exec, allow the
# ONE worker executable, deny writes except /dev/null, deny-read the private golden; +the private
# dir subpath denials when given. The `(allow process-exec (literal <exe>))` line necessarily
# names THIS side's worker executable — the direct-swift side names the swift bin, benchctl names
# the engine — so the two profiles are IDENTICAL in construction/policy but differ in that one
# literal (the correct per-side exec allowance, NOT a policy asymmetry). Seatbelt string escape:
# backslash first, then double-quote (Swift sandbox_escape order).
official_seatbelt_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
official_seatbelt_profile() {
  local exe="$1" golden="$2" priv="${3:-}"
  printf '(version 1)\n'
  printf '(allow default)\n'
  printf '(deny network*)\n'
  printf '(deny process-fork)\n'
  printf '(deny process-exec*)\n'
  printf '(allow process-exec (literal "%s"))\n' "$(official_seatbelt_escape "$exe")"
  printf '(deny file-write*)\n'
  printf '(allow file-write* (literal "/dev/null"))\n'
  printf '(deny file-read* (literal "%s"))\n' "$(official_seatbelt_escape "$golden")"
  if [ -n "$priv" ]; then
    printf '(deny file-read* (subpath "%s"))\n' "$(official_seatbelt_escape "$priv")"
    printf '(deny file-write* (subpath "%s"))\n' "$(official_seatbelt_escape "$priv")"
  fi
}

# abs path of a file/dir that already exists, else a best-effort join (portable, no realpath(1)).
official_abs() {
  local p="$1"
  if [ -d "$p" ]; then (cd -P "$p" && pwd); return; fi
  local d b; d="$(dirname "$p")"; b="$(basename "$p")"
  if [ -d "$d" ]; then printf '%s/%s\n' "$(cd -P "$d" && pwd)" "$b"; else printf '%s\n' "$p"; fi
}

# --- deterministic 40-hex commit id (stamped on BOTH sides) -----------------------------
# metrics.commit must MATCH across sides (a deterministic field the differ gates), so both the
# benchctl official run and the direct-swift official run are handed the SAME MLXFAST_COMMIT_SHA.
# Prefer the caller's OFFICIAL_COMMIT (validated 7-40 lowercase hex); else the bench repo HEAD
# (full 40-hex); else a fixed 40-hex sentinel (valid `isCommitSHAHex`). Never empty — an
# empty/garbage value would make the two sides fall back to their own `git rev-parse` and DIVERGE.
official_commit_sha40() {
  local repo="${1:-}"
  # A caller OFFICIAL_COMMIT override wins ONLY if it is 7-40 lowercase hex (Swift `isCommitSHAHex`
  # / `commit_identifier`); a malformed value is ignored + warned, then we fall through — the SAME
  # validation both sides get, so a bad override can never stamp a divergent junk metrics.commit.
  if [ -n "${OFFICIAL_COMMIT:-}" ]; then
    case "$OFFICIAL_COMMIT" in
      *[!0-9a-f]*) echo "official_commit_sha40: OFFICIAL_COMMIT '$OFFICIAL_COMMIT' not lowercase hex — ignoring" >&2 ;;
      *) if [ "${#OFFICIAL_COMMIT}" -ge 7 ] && [ "${#OFFICIAL_COMMIT}" -le 40 ]; then printf '%s' "$OFFICIAL_COMMIT"; return
         else echo "official_commit_sha40: OFFICIAL_COMMIT length ${#OFFICIAL_COMMIT} not in 7..40 — ignoring" >&2; fi ;;
    esac
  fi
  local sha=""
  if [ -n "$repo" ] && [ -d "$repo/.git" ]; then
    sha="$(cd "$repo" && git rev-parse HEAD 2>/dev/null || true)"
  fi
  case "$sha" in
    [0-9a-f]*) [ "${#sha}" = 40 ] && { printf '%s' "$sha"; return; } ;;
  esac
  printf '%s' "0000000000000000000000000000000000000000"
}

# --- AUTHOR-AT-SEAL dispatch record (DECIDE-3) ------------------------------------------
# The DISPATCHED sha is the CI/yukon dispatch context (github.sha equivalent): MLXFAST_CANDIDATE_SHA
# if set, else GITHUB_SHA. Participant git state is deliberately NOT consulted here — `git rev-parse`
# is unusable under the ranked sandbox (dubious-ownership under `env -i`), so the dispatch context,
# not the checkout, is the authority for the commit that gets sealed. Echoes the validated 40-hex
# sha, or NOTHING (rc 0) when there is no dispatch context (local/offline). A SET-but-malformed
# dispatched sha is FATAL (rc 1) — a dispatch that named a commit must name a real full commit id,
# matching the trusted workflow's `^[0-9a-f]{40}$` gate before it records one.
dispatch_context_sha() {
  local s="${MLXFAST_CANDIDATE_SHA:-${GITHUB_SHA:-}}"
  [ -n "$s" ] || return 0
  case "$s" in
    *[!0-9a-f]*) echo "dispatch_context_sha: FATAL dispatched sha '$s' is not lowercase hex" >&2; return 1 ;;
  esac
  [ "${#s}" -eq 40 ] || { echo "dispatch_context_sha: FATAL dispatched sha length ${#s} != 40 (need a full commit id)" >&2; return 1; }
  printf '%s' "$s"
}

# Record the dispatched sha to <out_dir>/candidate.sha (benchd-readable) so `benchctl measure-job`
# AUTHORS metrics.commit from it at seal time (the challenger candidate.sha shape). Echoes the
# record PATH when a dispatch context is present; nothing (rc 0) when there is none. FATAL (rc 1)
# if the sha is malformed or the write fails. The caller threads the echoed path to measure-job as
# MLXFAST_CANDIDATE_SHA_FILE.
record_dispatch_sha() {
  local out_dir="$1" sha
  sha="$(dispatch_context_sha)" || return 1
  [ -n "$sha" ] || return 0
  mkdir -p "$out_dir" || { echo "record_dispatch_sha: FATAL cannot create $out_dir" >&2; return 1; }
  printf '%s\n' "$sha" > "$out_dir/candidate.sha" || {
    echo "record_dispatch_sha: FATAL cannot write $out_dir/candidate.sha" >&2; return 1;
  }
  printf '%s/candidate.sha' "$out_dir"
}

# --- SEAL a swift-official stdout payload the way benchmark.sh does ----------------------
# official_seal_stdout <raw_stdout_file> <out_score>. Requires EXACTLY one JSON object shaped
# like a score payload (benchmark.sh:1247-1252): `.passed` boolean, has `score`, `.metrics`
# object. Empty / non-JSON / multiple concatenated objects → return 1 (TOOL-ERR — never seal an
# attacker-controlled or malformed score as a pass). On success copies the raw stdout verbatim to
# <out_score> (the trusted seal is the STDOUT bytes, not the untrusted on-disk --score-path file).
official_seal_stdout() {
  local raw="$1" out="$2"
  command -v jq >/dev/null 2>&1 || { echo "  seal FAIL: jq required" >&2; return 3; }
  [ -s "$raw" ] || { echo "  seal FAIL: benchmark emitted empty stdout (no payload)" >&2; return 1; }
  if [ "$(jq -s 'length' "$raw" 2>/dev/null)" != "1" ] \
      || ! jq -e '(.passed | type == "boolean") and has("score") and (.metrics | type == "object")' \
          "$raw" >/dev/null 2>&1; then
    echo "  seal FAIL: benchmark did not emit a single valid score payload on stdout" >&2; return 1
  fi
  cp "$raw" "$out"
}

# --- write the swift-side .sha256 + 9-field integrity sidecars (benchmark.sh:1269-1308) --
# official_write_sidecars <score> <weights_path> <golden> <integrity_out>. Mirrors the trusted
# shell sealing benchmark.sh performs AFTER the swift process exits: the score sha256 sidecar
# (`<hex>  <path>` two-space form) and the 9-field integrity JSON (jq -n, golden_path "[private]").
# transform_source_sha256 is read from the weights `.benchmark-source.sha256` marker (or "" when
# absent) — identical to benchctl's rule; benchmark.sh instead computes source_hash() fresh, so
# this field is DECLARED-excepted in the Leg-3 byte-compare exactly as facade-leg does.
official_write_sidecars() {
  local score="$1" weights="$2" golden="$3" integrity_out="$4"
  command -v jq >/dev/null 2>&1 || { echo "  sidecar FAIL: jq required" >&2; return 3; }
  local score_hash; score_hash="$(official_sha_of "$score")"
  printf '%s  %s\n' "$score_hash" "$score" > "$score.sha256"
  local score_metrics
  if ! score_metrics="$(jq -er '
      .metrics
      | select((.weights_hash | type) == "string" and (.weights_hash | length) > 0)
      | select((.weights_file_count | type) == "number")
      | select((.weights_byte_count | type) == "number")
      | [.weights_hash, (.weights_file_count | tostring), (.weights_byte_count | tostring)]
      | @tsv
    ' "$score")"; then
    echo "  sidecar FAIL: score payload has invalid weights integrity metrics" >&2; return 1
  fi
  local weights_hash weights_file_count weights_byte_count
  IFS=$'\t' read -r weights_hash weights_file_count weights_byte_count <<< "$score_metrics"
  local golden_hash=""
  [ -f "$golden" ] && golden_hash="$(official_sha_of "$golden")"
  local tsrc=""
  [ -f "$weights/.benchmark-source.sha256" ] && tsrc="$(tr -d '[:space:]' < "$weights/.benchmark-source.sha256")"
  # F-5 — relativise the sealed paths so this reference sidecar carries no operator home directory
  # and its `weights_path` byte-matches the bc side (which relativises identically). The `.sha256`
  # body above keeps the trusted `<hex>  <path>` shape (Leg 3b compares the hash, not the path).
  local score_rel weights_rel
  score_rel="$(relativize_for_seal "$score")"
  weights_rel="$(relativize_for_seal "$weights")"
  jq -n \
    --arg score_path "$score_rel" \
    --arg score_sha256 "$score_hash" \
    --arg weights_path "$weights_rel" \
    --arg weights_sha256 "$weights_hash" \
    --argjson weights_file_count "$weights_file_count" \
    --argjson weights_byte_count "$weights_byte_count" \
    --arg golden_sha256 "$golden_hash" \
    --arg transform_source_sha256 "$tsrc" \
    '{
      score_path: $score_path,
      score_sha256: $score_sha256,
      weights_path: $weights_path,
      weights_sha256: $weights_sha256,
      weights_file_count: $weights_file_count,
      weights_byte_count: $weights_byte_count,
      golden_path: "[private]",
      golden_sha256: $golden_sha256,
      transform_source_sha256: $transform_source_sha256
    }' > "$integrity_out"
}

# --- run DIRECT `mlxfast-swift benchmark` in OFFICIAL env, capture + seal ----------------
# official_swift_run <dir> — the trusted binary, NOT benchmark.sh. Reads config from env:
#   SWIFT (abs mlxfast-swift), WEIGHTS, OFFICIAL_GOLDEN(this run's golden), OFFICIAL_COMMIT(40hex).
# Writes into <dir>: score.json (SEALED from stdout), score.json.sha256, benchmark-integrity.json,
# exit_code, stdout.raw, stderr, sandbox.sb. Sets global OFFICIAL_SWIFT_RC / OFFICIAL_SWIFT_SEAL_RC.
# Official is ENV-DRIVEN (the trusted binary has NO --official flag): MLXFAST_OFFICIAL_BENCHMARK_RUN=1
# + the runtime-worker sandbox env, then `mlxfast-swift benchmark --weights --golden --score-path`.
# The on-disk --score-path is UNTRUSTED (submitted code shares that process); the SEAL is the
# process STDOUT, re-materialized here in the trusted shell (benchmark.sh:1224-1254).
official_swift_run() {
  local dir="$1"
  : "${SWIFT:?official_swift_run needs SWIFT}" "${WEIGHTS:?needs WEIGHTS}" "${OFFICIAL_GOLDEN:?needs OFFICIAL_GOLDEN}"
  mkdir -p "$dir"
  local swift_abs golden_abs profile metallib
  swift_abs="$(official_abs "$SWIFT")"
  golden_abs="$(official_abs "$OFFICIAL_GOLDEN")"
  profile="$dir/sandbox.sb"
  # Profile pinned to the SWIFT worker executable + this run's golden (same builder benchctl uses).
  official_seatbelt_profile "$swift_abs" "$golden_abs" "${MLXFAST_PRIVATE_DIR:-}" > "$profile"
  metallib="$(dirname "$swift_abs")/mlx.metallib"
  # LANE 2b (#148) — ARM the engine belt. This path spawns the mlxfast-swift HARNESS, whose
  # RuntimeWorkerExecutablePin re-verifies the worker it is about to spawn
  # (MLXFAST_RUNTIME_WORKER_EXECUTABLE, set to "$swift_abs" below) against an env-seam sha pin,
  # reading the HARNESS's OWN environment (not benchd's sanitized child env — this shell env never
  # passes through sanitized_engine_env). Hand it the GATE-attested sha as that pin + mark it
  # MANDATORY, so a worker swapped after the gate sealed it is refused fail-closed at spawn. The
  # value is OFFICIAL_WORKER_BIN_SHA256 — the gate seal of the WORKER THIS HARNESS SPAWNS ($swift_abs)
  # — never a self-hash. It is kept DISTINCT from the measure-job engine's OFFICIAL_ENGINE_BIN_SHA256
  # (that pin binds a different binary, the seam-2 engine) so the belt verifies the right file. Armed
  # ONLY on the sealed/official path (a gate-attested worker sha is present); dev/offline runs leave
  # it unset and the belt stays opt-in.
  #
  # 2b SEAL-TARGETING NOTE — the seal must pin the ACTUALLY-SPAWNED worker, not a harness that only
  # launches it. On this fork `mlxfast-swift` is HARNESS-ONLY; the binary that actually runs the
  # decode window is `mlxfast-runtime-worker` (the worker designated via the R10 GATE_EXTRA_ENV
  # allowlist and set as MLXFAST_RUNTIME_WORKER_EXECUTABLE). So `OFFICIAL_WORKER_BIN_SHA256` — and
  # the `$SWIFT`/`$swift_abs` this path pins it to — must resolve to that spawned runtime worker, so
  # the belt re-verifies the file that actually executes rather than a wrapper the gate never spawns.
  local belt_pin_env=""
  if [ -n "${OFFICIAL_WORKER_BIN_SHA256:-}" ]; then
    belt_pin_env="MLXFAST_RUNTIME_WORKER_EXECUTABLE_SHA256=$OFFICIAL_WORKER_BIN_SHA256 MLXFAST_RUNTIME_WORKER_EXECUTABLE_SHA256_REQUIRED=1"
  fi
  # The trusted swift binary writes its (untrusted) score to MLXFAST_SCORE_PATH; we seal STDOUT.
  local untrusted="$dir/score.untrusted.json"
  # NOTE: run from SWIFT_REPO_ROOT when given (the challenge-dev repo root, where a relative
  # metallib/tools path would resolve) — a GPU-box concern; MLXFAST_MLX_METALLIB is set absolutely
  # as belt-and-suspenders so CWD is not load-bearing.
  (
    [ -n "${SWIFT_REPO_ROOT:-}" ] && cd "$SWIFT_REPO_ROOT"
    env \
      MLXFAST_OFFICIAL_BENCHMARK_RUN=1 \
      MLXFAST_USE_RUNTIME_WORKER=1 \
      MLXFAST_RUNTIME_WORKER_EXECUTABLE="$swift_abs" \
      ${belt_pin_env} \
      MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE="$profile" \
      MLXFAST_WEIGHTS_PATH="$WEIGHTS" \
      MLXFAST_CORRECTNESS_GOLDEN_PATH="$OFFICIAL_GOLDEN" \
      MLXFAST_SCORE_PATH="$untrusted" \
      MLXFAST_COMMIT_SHA="$(official_commit_sha40)" \
      MLXFAST_BENCHMARK_CHECK_GATES=1 \
      MLXFAST_MLX_METALLIB="$metallib" \
      ${OFFICIAL_SWIFT_EXTRA_ENV:-} \
      "$SWIFT" benchmark \
        --weights "$WEIGHTS" \
        --golden "$OFFICIAL_GOLDEN" \
        --score-path "$untrusted"
  ) > "$dir/stdout.raw" 2> "$dir/stderr"
  OFFICIAL_SWIFT_RC=$?
  printf '%s' "$OFFICIAL_SWIFT_RC" > "$dir/exit_code"
  # Seal the STDOUT payload (trusted); a failing OFFICIAL run still emits a passed:false payload
  # on stdout (makeFailedScore), so seal whenever stdout is a single valid payload.
  official_seal_stdout "$dir/stdout.raw" "$dir/score.json"
  OFFICIAL_SWIFT_SEAL_RC=$?
  if [ "$OFFICIAL_SWIFT_SEAL_RC" -eq 0 ]; then
    official_write_sidecars "$dir/score.json" "$WEIGHTS" "$OFFICIAL_GOLDEN" "$dir/benchmark-integrity.json" || return 1
  fi
  return 0
}

# --- run `benchctl iterate --mode official`, capture ------------------------------------
# official_benchctl_run <dir> — writes into <dir>: score.json (sealed by benchd), score.json.sha256,
# benchmark-integrity.json (benchd is the sole writer of all three), exit_code, stdout, stderr.
# Reads: BENCHCTL, ENGINE, WEIGHTS, OFFICIAL_GOLDEN, OFFICIAL_COMMIT; optional OFFICIAL_PIN_SHA /
# OFFICIAL_PIN_BYTES (integrity pin, clean golden only). benchctl SELF-GENERATES the Seatbelt
# profile from the SAME builder (build_seatbelt_profile), pinned to the ENGINE worker — do NOT set
# MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE here (the probe leg is the one deliberate Override). Sets
# global OFFICIAL_BC_RC.
official_benchctl_run() {
  local dir="$1"
  : "${BENCHCTL:?needs BENCHCTL}" "${ENGINE:?needs ENGINE}" "${WEIGHTS:?needs WEIGHTS}" "${OFFICIAL_GOLDEN:?needs OFFICIAL_GOLDEN}"
  mkdir -p "$dir"
  local pin=()
  if [ -n "${OFFICIAL_PIN_SHA:-}" ] && [ -n "${OFFICIAL_PIN_BYTES:-}" ]; then
    pin=(--golden-sha256 "$OFFICIAL_PIN_SHA" --golden-bytes "$OFFICIAL_PIN_BYTES")
  fi
  # shellcheck disable=SC2086
  env \
    MLXFAST_USE_RUNTIME_WORKER=1 \
    MLXFAST_COMMIT_SHA="$(official_commit_sha40)" \
    ${OFFICIAL_BC_EXTRA_ENV:-} \
    "$BENCHCTL" iterate \
      --engine "$ENGINE" \
      --weights "$WEIGHTS" \
      --golden "$OFFICIAL_GOLDEN" \
      ${pin[@]+"${pin[@]}"} \
      --mode official \
      --score-path "$dir/score.json" \
    > "$dir/stdout" 2> "$dir/stderr"
  OFFICIAL_BC_RC=$?
  printf '%s' "$OFFICIAL_BC_RC" > "$dir/exit_code"
  return 0
}

# --- `.passed` reader (MISSING/ERR, never a fabricated value) ---------------------------
official_passed_of() {
  [ -s "$1" ] || { printf 'MISSING'; return; }
  python3 -c "import json,sys
try: print(json.load(open(sys.argv[1])).get('passed'))
except Exception: print('ERR')" "$1" 2>/dev/null || printf 'ERR'
}

# --- no-undeclared-cells rule (shared with variant-lib / failure-map) -------------------
# official_render_verdict <raw_verdict> <declared>. FAIL + declared(#nn) → DECLARED(#nn); PASS and
# TOOL-ERR pass through unchanged. FAIL survives ONLY for UNDECLARED cells (act on this).
official_render_verdict() {
  local vd="$1" declared="$2"
  if [ "$vd" = "FAIL" ] && [ -n "$declared" ]; then printf 'DECLARED(%s)' "$declared"; return 0; fi
  printf '%s' "${vd:-（no verdict）}"
}

# --- P-3 artifact replication (run completion, success OR fail) --------------------------
# official_replicate_artifacts <out_dir> <box_local_dir>. Replicates the run's artifact dir to a
# replica so the evidence survives the box. If REPLICA_TARGET is set → `rsync -az "$out/"
# "$REPLICA_TARGET/"` (offsite). If unset → copy to the box-local <box_local_dir> AND log that the
# offsite pull is still pending (P-3). NEVER returns non-zero: a replica error is logged, never
# fatal to the run (the run's own verdict must not hinge on replication). Emits log lines on stdout
# (the driver tees them into run.log). Portable (rsync when targeted, cp -R for the box-local copy).
official_replicate_artifacts() {
  local out="$1" box_local="${2:-}"
  [ -d "$out" ] || { echo "replica: source artifact dir '$out' missing — nothing to replicate (non-fatal)"; return 0; }
  if [ -n "${REPLICA_TARGET:-}" ]; then
    if rsync -az "$out/" "$REPLICA_TARGET/" >/dev/null 2>&1; then
      echo "replica: rsynced run artifacts to REPLICA_TARGET $REPLICA_TARGET"
    else
      echo "replica: rsync to REPLICA_TARGET $REPLICA_TARGET FAILED (non-fatal; run verdict unaffected)"
    fi
  else
    if [ -z "$box_local" ]; then
      echo "replica: no REPLICA_TARGET and no box-local dir given — skipping (non-fatal)"
      return 0
    fi
    if mkdir -p "$box_local" 2>/dev/null && cp -R "$out/." "$box_local/" 2>/dev/null; then
      echo "replica: box-local copy at $box_local"
    else
      echo "replica: box-local copy to $box_local FAILED (non-fatal; run verdict unaffected)"
    fi
    echo "replica: offsite REPLICA_TARGET unset — box-local replica only; offsite pull pending (P-3)"
  fi
  return 0
}

# --- R11: qwen unload/health hardening (paired window; additive, official window unaffected) -----
# official_qwen_proc_gone [pattern]. Read-only probe (NEVER kills): returns 0 if NO live process
# matches the served-model worker pattern (default QWEN_PROC_PATTERN or mlx_lm.server), non-zero if
# one is still resident. pgrep -f when available, else a ps scan.
official_qwen_proc_gone() {
  local pat="${1:-${QWEN_PROC_PATTERN:-mlx_lm.server}}"
  if command -v pgrep >/dev/null 2>&1; then
    pgrep -f "$pat" >/dev/null 2>&1 && return 1 || return 0
  fi
  ps ax 2>/dev/null | grep -v grep | grep -q -- "$pat" && return 1 || return 0
}

# official_qwen_unload_verify — call qwen_unload, CHECK its rc, then VERIFY the served-model proc is
# actually gone (R11: an abrupt/failed unload once caused a GIL crash + launchd crash-loop). Returns
# 0 only if unload rc==0 AND the proc left within the bounded poll; non-zero otherwise so the caller
# ABORTS BEFORE spending the window. qwen_unload must be defined (sourced from qwen-service.sh).
# Bounds: QWEN_UNLOAD_VERIFY_TRIES (default 12) × QWEN_UNLOAD_VERIFY_INTERVAL (default 1s).
official_qwen_unload_verify() {
  qwen_unload; local urc=$?
  if [ "$urc" -ne 0 ]; then echo "qwen unload: FAILED (rc=$urc) — aborting before the window (not unloading further / not spending seam 2)"; return "$urc"; fi
  local i=0 n="${QWEN_UNLOAD_VERIFY_TRIES:-12}"
  while [ "$i" -lt "$n" ]; do
    if official_qwen_proc_gone; then echo "qwen unload: rc=0 and served-model proc verified gone"; return 0; fi
    i=$((i+1)); sleep "${QWEN_UNLOAD_VERIFY_INTERVAL:-1}"
  done
  echo "qwen unload: rc=0 but the served-model proc is STILL resident after ${n} checks — aborting (unsafe to run the window)"; return 1
}

# official_qwen_health_probe <served_model_id> [timeout_s] [base_url]. Post-reload probe: polls
# <base>/v1/models (grepping the served model id) AND <base>/health until BOTH are healthy, or the
# bounded wait elapses. The 27B load is ~85s+ and can crash-loop through the launchd throttle, so the
# floor is >=180s (QWEN_HEALTH_FLOOR, default 180; the effective wait = max(requested, floor)); on a
# DOWN worker it issues a best-effort `launchctl kickstart -k` between polls (when QWEN_LAUNCHD_LABEL
# is set). Returns 0 healthy; non-zero on timeout (the caller LOGS LOUDLY — never a silent success).
# Overridable for offline tests: QWEN_CURL (curl bin), QWEN_HEALTH_POLL_INTERVAL, QWEN_HEALTH_FLOOR.
official_qwen_health_probe() {
  local model="$1" timeout="${2:-180}" base="${3:-${QWEN_BASE_URL:-http://127.0.0.1:8080}}"
  local floor="${QWEN_HEALTH_FLOOR:-180}"
  [ "$timeout" -lt "$floor" ] 2>/dev/null && timeout="$floor"
  local curl_bin="${QWEN_CURL:-curl}" now deadline tries=0 models health
  now="$(date +%s)"; deadline=$(( now + timeout ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    tries=$((tries+1))
    models="$("$curl_bin" -s -m5 "$base/v1/models" 2>/dev/null || true)"
    health="$("$curl_bin" -s -m5 "$base/health" 2>/dev/null || true)"
    if printf '%s' "$models" | grep -q -- "$model" && printf '%s' "$health" | grep -qiE 'ok|healthy|"status"|serving'; then
      echo "qwen health: OK after ${tries} probe(s) — model '$model' served AND /health up"; return 0
    fi
    # Worker down? kick launchd through its throttle (best-effort; needs the label + launchctl).
    if official_qwen_proc_gone && [ -n "${QWEN_LAUNCHD_LABEL:-}" ] && command -v launchctl >/dev/null 2>&1; then
      launchctl kickstart -k "${QWEN_LAUNCHD_DOMAIN:-gui/$(id -u 2>/dev/null || echo 0)}/${QWEN_LAUNCHD_LABEL}" >/dev/null 2>&1 || true
    fi
    sleep "${QWEN_HEALTH_POLL_INTERVAL:-5}"
  done
  echo "qwen health: NOT HEALTHY after ${timeout}s (${tries} probes) — model '$model' not served / /health down; INVESTIGATE (do not declare success)"; return 1
}

# --- differ verdict cell from a parity-diff exit + output (shared) -----------------------
# official_diff_cell <differ_exit> <diff_output_file>. Only 0 (PASS)/1 (FAIL) are verdicts and
# each REQUIRES a matching `PARITY:` line (a bare 0 with no line is TOOL-ERR, never a silent pass);
# anything else is TOOL-ERR. Prints PASS|FAIL|TOOL-ERR(...).
official_diff_cell() {
  local drc="$1" out="$2" vd
  vd="$(grep '^PARITY:' "$out" 2>/dev/null | sed 's/PARITY: //' | head -1)"
  case "$drc" in
    0) case "$vd" in PASS*) printf 'PASS';; *) printf 'TOOL-ERR(exit 0, no PARITY line)';; esac ;;
    1) case "$vd" in FAIL*) printf 'FAIL';; *) printf 'FAIL(exit 1)';; esac ;;
    *) printf 'TOOL-ERR(%s)' "$drc" ;;
  esac
}
