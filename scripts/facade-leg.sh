#!/bin/bash
# scripts/facade-leg.sh — R-3 facade confirmatory leg (deliverable C).
#
# Runs the benchctl FACADE (scripts/benchmark.sh) vs the REAL Swift reference benchmark.sh
# LIVE, in BOTH local modes, on a valid golden, and byte-compares the ARTIFACTS they produce:
#   - score file NAMING          (score.local-iterate.json / score.json per mode — must match)
#   - <score>.sha256 sidecar     (present in both; each is the true sha256 of its own score file)
#   - benchmark-integrity JSON    (present in both; same top-level key SET — a structural compare)
#   - sealed score payload        (deterministic surface via `benchctl parity-diff` — TIMING EXCEPTED)
#   - exit code                   (must be identical)
#
# TIMING VALUES ARE EXCEPTED ON PURPOSE: the facade runs benchctl (MLX engine) and the reference
# runs the Swift engine, so the scored timing legitimately differs. `benchctl parity-diff`'s bucket
# policy already waives timing/environmental fields, so it compares exactly the deterministic
# artifact fields — never the timing numbers. This leg CONFIRMS §4 (exit codes) and §5 (stdout /
# integrity / naming) rows on LIVE evidence; the offline compat-matrix.sh proved the shell contract.
#
# GPU: BOTH sides spawn a real engine → run inside a GPU window (caller unloads/reloads qwen).
# Fails LOUD, never silently passes.
#
# Env contract:
#   FACADE                  facade under test            [default scripts/benchmark.sh here]
#   REFERENCE_BENCHMARK_SH  the REAL Swift benchmark.sh  [required]
#   MLXFAST_ENGINE_BIN      MLX engine the facade/benchctl spawns  [required — facade side]
#   BENCHCTL                benchctl (facade dispatch + parity-diff)  [required]
#   WEIGHTS                 transformed weights dir      [required — both sides]
#   GOLDEN_ITERATE          valid golden for local-iterate  [default $MLXFAST_PARITY_GIT/golden/beefed.json]
#   GOLDEN_SUBMIT           valid golden for local-submit   [default $MLXFAST_PARITY_GIT/golden/submit-1024.json]
#   OUT                     artifact + report dir        [required]
#   MODES                   [default "local-iterate local-submit"]
#   REF_EXTRA_ENV           extra KEY=VAL (space-sep) the reference needs (e.g. MLXFAST_IN_SANDBOX=1) [optional]
#   COMPARE_ONLY            1 = SKIP the runs; re-compare the EXISTING $OUT/{ref,fac}.<mode>/ artifacts
#                           from a prior window (no GPU, no qwen downtime). Needs only BENCHCTL + jq +
#                           OUT + MODES; the per-mode dirs (with score/.sha256/integrity/exit_code)
#                           must already exist or the row fails LOUD. [default 0]
set -uo pipefail

HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# #123 (RULED David 2026-08-20, EXTEND THE SIDECAR) — benchd's sidecar is a strict SUPERSET of the
# reference's object: the reference's fields, unchanged, plus a RUNNER-IDENTITY roster. So the
# integrity key check is asymmetric — benchd's keys must CONTAIN the reference's, and the surplus
# must be EXACTLY that roster: every key present, none extra. An empty surplus is a FAIL, not a
# pass; a benchd that dropped the runner block is the precise regression #123 exists to prevent.
#
# The roster is read from ONE file (scripts/fixtures/integrity-runner-keys.json), which the Rust
# superset test reads too — see that file's header for why it is not four literals any more.
# The retired 9-field byte-match row is re-graded VERIFIED (superset).
INTEGRITY_RUNNER_KEYS="$(jq -S -c '.keys' "$HERE/fixtures/integrity-runner-keys.json")"
G="${MLXFAST_PARITY_GIT:-$HOME/mlxfast-parity-git}"
FACADE="${FACADE:-$HERE/benchmark.sh}"
COMPARE_ONLY="${COMPARE_ONLY:-0}"
: "${BENCHCTL:?set BENCHCTL to the benchctl binary}"
: "${OUT:?set OUT to the artifact/report dir}"
# The RUN requireds are needed only when we actually run (not in COMPARE_ONLY re-compare).
if [ "$COMPARE_ONLY" != "1" ]; then
  : "${REFERENCE_BENCHMARK_SH:?set REFERENCE_BENCHMARK_SH to the real Swift benchmark.sh}"
  : "${MLXFAST_ENGINE_BIN:?set MLXFAST_ENGINE_BIN to the MLX engine binary the facade spawns}"
  : "${WEIGHTS:?set WEIGHTS to a transformed weights dir}"
  [ -r "$FACADE" ] || { echo "facade-leg: FATAL facade not readable: $FACADE" >&2; exit 2; }
  [ -r "$REFERENCE_BENCHMARK_SH" ] || { echo "facade-leg: FATAL reference not readable: $REFERENCE_BENCHMARK_SH" >&2; exit 2; }
fi
GOLDEN_ITERATE="${GOLDEN_ITERATE:-$G/golden/beefed.json}"
GOLDEN_SUBMIT="${GOLDEN_SUBMIT:-$G/golden/submit-1024.json}"
MODES="${MODES:-local-iterate local-submit}"
command -v jq >/dev/null || { echo "facade-leg: FATAL jq required" >&2; exit 2; }

mkdir -p "$OUT"
# In COMPARE_ONLY we MUST NOT wipe the prior window's captured artifacts; only clear the table.
[ "$COMPARE_ONLY" = "1" ] || rm -rf "$OUT"/ref.* "$OUT"/fac.* 2>/dev/null || true
rm -f "$OUT"/facade-leg.table.txt 2>/dev/null || true

golden_for() { case "$1" in local-iterate) echo "$GOLDEN_ITERATE";; *) echo "$GOLDEN_SUBMIT";; esac; }
score_name_for() { case "$1" in local-iterate) echo "score.local-iterate.json";; *) echo "score.json";; esac; }
integrity_name_for() { case "$1" in local-iterate) echo "benchmark-integrity.local-iterate.json";; *) echo "benchmark-integrity.json";; esac; }

# sha256 of a file (stock macOS + box).
sha_of() { shasum -a 256 "$1" | awk '{print $1}'; }

# run_leg_pair <mode> <golden> <sname> <iname> <refd> <facd> — run the reference + facade LIVE and
# capture their artifacts + exit codes. Sets globals ref_rc / fac_rc and writes each side's exit
# code to <dir>/exit_code so COMPARE_ONLY can re-check exit-code parity later without re-running.
run_leg_pair() {
  local mode="$1" golden="$2" sname="$3" iname="$4" refd="$5" facd="$6"
  # The reference benchmark.sh derives RELATIVE paths from its own repo root: SWIFT_BIN
  # `.build/release/mlxfast-swift`, MLX_METALLIB `$(dirname SWIFT_BIN)/mlx.metallib`, sandbox
  # `tools/deny-network.sb`, `reference_weights/`, `.git`. Running it from $refd broke ALL of them
  # ("MLX metallib is missing" → exit 1). So run it from $REF_ROOT (the challenge-dev repo root)
  # where those resolve, and redirect its OUTPUTS into $refd via ABSOLUTE env:
  #   MLXFAST_SCORE_PATH / MLXFAST_INTEGRITY_PATH → $refd/<per-mode basename> (basenames still equal
  #     the facade's per-mode names, so the score-name + integrity checks compare like-for-like).
  #   MLXFAST_SKIP_TRANSFORM=1  — $WEIGHTS is already transformed; do NOT re-transform.
  #   MLXFAST_MLX_METALLIB=$REF_ROOT/.build/release/mlx.metallib  — belt-and-suspenders.
  #   cool-gate MODE-derived + SYMMETRIC with the parity driver (run-manual-test SWIFT_COOL_KV):
  #     local-iterate forces the gate OFF (MLXFAST_LOCAL_COOL_GATE=0); local-submit leaves the
  #     helper no-op. Do NOT set MLXFAST_OFFICIAL_BENCHMARK_RUN.
  local REF_ROOT ref_cool
  REF_ROOT="$(cd -P "$(dirname "$REFERENCE_BENCHMARK_SH")" && pwd)"
  case "$mode" in local-iterate) ref_cool="MLXFAST_LOCAL_COOL_GATE=0";; *) ref_cool="";; esac
  # shellcheck disable=SC2086
  ( cd "$REF_ROOT" && env ${REF_EXTRA_ENV:-} $ref_cool \
      MLXFAST_SKIP_TRANSFORM=1 \
      MLXFAST_WEIGHTS_PATH="$WEIGHTS" \
      MLXFAST_CORRECTNESS_GOLDEN_PATH="$golden" \
      MLXFAST_SCORE_PATH="$refd/$sname" \
      MLXFAST_INTEGRITY_PATH="$refd/$iname" \
      MLXFAST_MLX_METALLIB="$REF_ROOT/.build/release/mlx.metallib" \
      "$REFERENCE_BENCHMARK_SH" "--$mode" ) > "$refd/stdout" 2> "$refd/stderr"
  ref_rc=$?
  printf '%s' "$ref_rc" > "$refd/exit_code"

  ( cd "$facd" && env \
      BENCHCTL="$BENCHCTL" \
      MLXFAST_ENGINE_BIN="$MLXFAST_ENGINE_BIN" \
      MLXFAST_CORRECTNESS_GOLDEN_PATH="$golden" \
      MLXFAST_WEIGHTS_PATH="$WEIGHTS" \
      "$FACADE" "--$mode" ) > "$facd/stdout" 2> "$facd/stderr"
  fac_rc=$?
  printf '%s' "$fac_rc" > "$facd/exit_code"
}

TABLE_FILE="$OUT/facade-leg.table.txt"; : > "$TABLE_FILE"
FAIL=0

printf '%-14s | %-10s | %-8s | %-10s | %-12s | %-9s | %s\n' \
  "mode" "score-name" ".sha256" "integrity" "det-fields" "exit" "overall" | tee -a "$TABLE_FILE"
printf -- '---------------|------------|----------|------------|--------------|-----------|--------\n' | tee -a "$TABLE_FILE"

for mode in $MODES; do
  golden="$(golden_for "$mode")"
  sname="$(score_name_for "$mode")"
  iname="$(integrity_name_for "$mode")"
  refd="$OUT/ref.$mode"; facd="$OUT/fac.$mode"

  if [ "$COMPARE_ONLY" = "1" ]; then
    # Re-compare a prior window's captured artifacts: never run, never wipe. Fail LOUD (never PASS)
    # if the per-mode dirs / stored exit codes are absent — a missing artifact is not a match.
    if [ ! -d "$refd" ] || [ ! -d "$facd" ]; then
      echo "facade-leg[$mode]: COMPARE_ONLY=1 but $refd or $facd is missing — cannot re-compare" >&2
      printf '%-14s | %-10s | %-8s | %-10s | %-12s | %-9s | %s\n' \
        "$mode" "NO-ARTIFACTS" "-" "-" "-" "-" "FAIL" | tee -a "$TABLE_FILE"
      FAIL=$((FAIL+1)); continue
    fi
    [ -f "$refd/exit_code" ] && ref_rc="$(cat "$refd/exit_code")" || ref_rc="MISSING"
    [ -f "$facd/exit_code" ] && fac_rc="$(cat "$facd/exit_code")" || fac_rc="MISSING"
  else
    rm -rf "$refd" "$facd"; mkdir -p "$refd" "$facd"
    if [ ! -f "$golden" ]; then
      echo "facade-leg: golden missing for $mode: $golden — recording FAIL row" >&2
      printf '%-14s | %-10s | %-8s | %-10s | %-12s | %-9s | %s\n' \
        "$mode" "NO-GOLDEN" "-" "-" "-" "-" "FAIL" | tee -a "$TABLE_FILE"
      FAIL=$((FAIL+1)); continue
    fi
    run_leg_pair "$mode" "$golden" "$sname" "$iname" "$refd" "$facd"
  fi

  # --- checks 1-5 below re-compare whatever is in $refd/$facd (freshly run or prior window) -----

  # --- 1. score-file naming: both wrote the SAME basename ---
  if [ -f "$refd/$sname" ] && [ -f "$facd/$sname" ]; then c_name="ok"; else
    c_name="FAIL"; FAIL=$((FAIL+1))
    echo "facade-leg[$mode]: score-name — ref:$([ -f "$refd/$sname" ] && echo y || echo n) fac:$([ -f "$facd/$sname" ] && echo y || echo n) (want $sname)" >&2
  fi

  # --- 2. <score>.sha256 sidecar: present in both AND each is the true sha256 of its score ---
  c_sha="ok"
  for side in "$refd" "$facd"; do
    if [ ! -f "$side/$sname.sha256" ]; then c_sha="FAIL"; break; fi
    want="$(cat "$side/$sname.sha256" 2>/dev/null | awk '{print $1}')"
    [ -f "$side/$sname" ] && got="$(sha_of "$side/$sname")" || got="MISSING"
    if [ "$want" != "$got" ]; then c_sha="FAIL"; break; fi
  done
  [ "$c_sha" = "ok" ] || { FAIL=$((FAIL+1)); echo "facade-leg[$mode]: .sha256 sidecar missing or not matching its score file" >&2; }

  # --- 3. benchmark-integrity JSON: present in both, same top-level key SET AND matching
  #        deterministic VALUES. Key-set alone is too weak — a same-shaped integrity JSON with a
  #        different golden/weights digest would slip through. Value-compare the fields that MUST
  #        byte-match (same weights, same golden, same per-mode naming). Two fields are EXCEPTED:
  #          - score_sha256          — hashes the timing-bearing score payload (differs by design).
  #          - transform_source_sha256 — a DECLARED §3 caveat: Swift computes it fresh via
  #            source_hash(); benchctl reads the `<weights>/.benchmark-source.sha256` marker and
  #            writes "" when that marker is ABSENT (as it is on the box). They diverge only for the
  #            marker-less weights dir — orthogonal to the facade's artifact contract, not a defect.
  #            (We do NOT write a marker into the weights dir — that is the box owner's state.)
  #        score_path is compared by BASENAME: the reference records the ABSOLUTE MLXFAST_SCORE_PATH
  #        we hand it ($refd/…) while benchctl records the relative per-mode basename — the directory
  #        legitimately differs (two run dirs), the per-mode NAME must match.
  if [ -f "$refd/$iname" ] && [ -f "$facd/$iname" ]; then
    rk="$(jq -S 'keys' "$refd/$iname" 2>/dev/null)"; fk="$(jq -S 'keys' "$facd/$iname" 2>/dev/null)"
    # #123: SUPERSET, not equality — see INTEGRITY_RUNNER_KEYS at the top of this file.
    surplus="$(jq -S -n --argjson r "${rk:-[]}" --argjson f "${fk:-[]}" '$f - $r' -c 2>/dev/null)"
    missing="$(jq -S -n --argjson r "${rk:-[]}" --argjson f "${fk:-[]}" '$r - $f' -c 2>/dev/null)"
    c_int="ok"
    if [ -z "$rk" ] || [ "$missing" != "[]" ]; then
      c_int="FAIL"; echo "facade-leg[$mode]: integrity is MISSING reference keys $missing (ref=$rk fac=$fk)" >&2
    fi
    if [ "$surplus" != "$INTEGRITY_RUNNER_KEYS" ]; then
      c_int="FAIL"; echo "facade-leg[$mode]: integrity surplus keys are not EXACTLY the declared #123 runner roster (got=$surplus want=$INTEGRITY_RUNNER_KEYS)" >&2
    fi
    for fld in score_path weights_sha256 weights_file_count weights_byte_count golden_sha256 golden_path; do
      rv="$(jq -r --arg k "$fld" '.[$k] // "ABSENT"' "$refd/$iname" 2>/dev/null)"
      fv="$(jq -r --arg k "$fld" '.[$k] // "ABSENT"' "$facd/$iname" 2>/dev/null)"
      if [ "$fld" = "score_path" ]; then rv="$(basename "$rv")"; fv="$(basename "$fv")"; fi
      if [ "$rv" != "$fv" ]; then c_int="FAIL"; echo "facade-leg[$mode]: integrity.$fld differs (ref=$rv fac=$fv)" >&2; fi
    done
    # transform_source_sha256: report the declared §3 divergence for the record, never FAIL on it.
    rts="$(jq -r '.transform_source_sha256 // "ABSENT"' "$refd/$iname" 2>/dev/null)"
    fts="$(jq -r '.transform_source_sha256 // "ABSENT"' "$facd/$iname" 2>/dev/null)"
    [ "$rts" = "$fts" ] || echo "facade-leg[$mode]: integrity.transform_source_sha256 differs (ref=$rts fac=$fts) — DECLARED §3 caveat (marker-less weights), excepted" >&2
    [ "$c_int" = "ok" ] || FAIL=$((FAIL+1))
  else
    c_int="FAIL"; FAIL=$((FAIL+1))
    echo "facade-leg[$mode]: integrity JSON missing — ref:$([ -f "$refd/$iname" ] && echo y || echo n) fac:$([ -f "$facd/$iname" ] && echo y || echo n) (want $iname)" >&2
  fi

  # --- 4. deterministic score surface via parity-diff (TIMING EXCEPTED) ---
  # SAME rule as leg B (variant-parity.sh): exit 0 REQUIRES a `PARITY:` line, and its verdict word
  # is matched by PREFIX — benchctl prints `PARITY: PASS (no deterministic/ranking mismatch)` /
  # `PARITY: FAIL (…)`, so an EXACT `= "PASS"` test wrongly reads the descriptive suffix as "no
  # verdict". A 0 with no PARITY line at all is still TOOL-ERR (never a silent PASS).
  if [ -f "$refd/$sname" ] && [ -f "$facd/$sname" ]; then
    "$BENCHCTL" parity-diff "$facd/$sname" "$refd/$sname" > "$OUT/det.$mode.txt" 2>&1; drc=$?
    vd="$(grep '^PARITY:' "$OUT/det.$mode.txt" | sed 's/PARITY: //' | head -1)"
    case "$drc" in
      0) case "$vd" in PASS*) c_det="PASS" ;; *) c_det="TOOL-ERR(exit 0, no PARITY line)"; FAIL=$((FAIL+1)) ;; esac ;;
      1) case "$vd" in FAIL*) c_det="FAIL" ;; *) c_det="FAIL(exit 1)" ;; esac; FAIL=$((FAIL+1)) ;;
      *) c_det="TOOL-ERR($drc)"; FAIL=$((FAIL+1)) ;;
    esac
  else
    c_det="FAIL"; FAIL=$((FAIL+1))
  fi

  # --- 5. exit codes identical (a MISSING stored exit code in COMPARE_ONLY fails LOUD) ---
  if [ "$ref_rc" = "MISSING" ] || [ "$fac_rc" = "MISSING" ]; then
    c_exit="FAIL(exit_code artifact missing)"; FAIL=$((FAIL+1))
    echo "facade-leg[$mode]: exit_code artifact missing (ref=$ref_rc fac=$fac_rc) — cannot verify exit parity" >&2
  elif [ "$ref_rc" = "$fac_rc" ]; then c_exit="ok($ref_rc)"; else
    c_exit="FAIL(r=$ref_rc/f=$fac_rc)"; FAIL=$((FAIL+1))
  fi

  if [ "$c_name" = "ok" ] && [ "$c_sha" = "ok" ] && [ "$c_int" = "ok" ] && [ "$c_det" = "PASS" ] && [ "${c_exit#ok}" != "$c_exit" ]; then
    overall="GREEN"
  else
    overall="FAIL"
  fi
  printf '%-14s | %-10s | %-8s | %-10s | %-12s | %-9s | %s\n' \
    "$mode" "$c_name" "$c_sha" "$c_int" "$c_det" "$c_exit" "$overall" | tee -a "$TABLE_FILE"
done

echo ""
srcnote="$([ "$COMPARE_ONLY" = "1" ] && echo ' (COMPARE_ONLY — re-compared prior-window artifacts, no runs)' || echo '')"
if [ "$FAIL" -eq 0 ]; then
  echo "facade-leg: RESULT PASS — facade byte-green vs reference on the deterministic artifact surface (score_sha256 + transform_source_sha256[§3 caveat] + timing excepted), all modes$srcnote"
  exit 0
else
  echo "facade-leg: RESULT FAIL — $FAIL artifact/exit mismatch(es); see $OUT/ for per-side artifacts$srcnote" >&2
  exit 1
fi
