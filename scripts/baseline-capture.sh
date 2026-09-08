#!/usr/bin/env bash
#
# baseline-capture.sh -- capture prep for the Qwen 3.8 125B-A6B official baselines.
#
# Runs on the ranked box only (docs/qwen38-125b-a6b-baseline-capture.md). One
# subcommand per step of the box procedure. The GPU-lock steps are placeholders
# the box agent fills in; every placeholder exits 2 until it is filled in, so a
# half-filled script can never run a capture on an unlocked GPU.
#
#   baseline-capture.sh lock | unload | reload | release
#   baseline-capture.sh calibrate <A|B> <mtp|serial>
#   baseline-capture.sh compare <dir-A> <dir-B>
#   baseline-capture.sh verify
#
# Env (all required by `calibrate`):
#   MLXFAST_QWEN_MTP_TRACK_ID  the track id (its -{platform}-v{N} suffix keys the platform)
#   CAPTURE_DIR                where runs and calibration files are written
#   CANDIDATE_WS, BASELINE_WS  the two engine workspaces measure-job spawns from
#   CONTRACT                   the track fixture (benchmark.json) path
#   POOL_DIR                   directory holding every pinned pool prompt (*.json)
#   CORRECTNESS_GOLDEN         the official correctness golden (--correctness-golden)
#   BENCHD_BIN                 benchd binary (default: target/release/benchd)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BENCHD_BIN="${BENCHD_BIN:-${REPO_ROOT}/target/release/benchd}"

placeholder() {
  echo "baseline-capture.sh: step '$1' is a placeholder; the box agent fills in the concrete command before any capture runs" >&2
  exit 2
}

require_env() {
  for name in "$@"; do
    if [[ -z "${!name:-}" ]]; then
      echo "baseline-capture.sh: $name is required" >&2
      exit 2
    fi
  done
}

# -- GPU lock protocol (order: lock, unload, capture, reload, release) --------
cmd_lock()    { placeholder lock; }
cmd_unload()  { placeholder unload; }
cmd_reload()  { placeholder reload; }
cmd_release() { placeholder release; }

# -- One bootstrap run of the paired serial band for one series ---------------
cmd_calibrate() {
  local run="${1:-}" candidate="${2:-}"
  case "$run" in A|B) ;; *) echo "calibrate: run must be A or B" >&2; exit 2;; esac
  case "$candidate" in mtp|serial) ;; *) echo "calibrate: candidate must be mtp or serial" >&2; exit 2;; esac
  require_env MLXFAST_QWEN_MTP_TRACK_ID CAPTURE_DIR CANDIDATE_WS BASELINE_WS CONTRACT POOL_DIR \
    CORRECTNESS_GOLDEN
  local series
  case "$candidate" in mtp) series=free_run_v1_1;; serial) series=teacher_forced_v1;; esac
  # ONE measure-job run per pool prompt: the calibration bootstrap authors ONE targets[] entry
  # per run, keyed by the --target-id/--prompt/--prompt-sha256 trio (all-or-none), and merges
  # into the shared BASELINE_CALIBRATION file across the loop. --local-dev: the track is not
  # armed until the baselines exist, and calibration authors no ranked artifact (doc, section 4).
  local cal="${CAPTURE_DIR}/calibration.${series}.${run}.json"
  local found=0
  local prompt id sha out
  for prompt in "${POOL_DIR}"/*.json; do
    [[ -f "$prompt" ]] || continue
    found=1
    id="$(basename "$prompt" .json)"
    sha="$(shasum -a 256 "$prompt" | awk '{print $1}')"
    out="${CAPTURE_DIR}/${series}.${run}.${id}"
    mkdir -p "$out"
    BASELINE_CALIBRATION="$cal" \
    "$BENCHD_BIN" measure-job \
      --candidate "$CANDIDATE_WS" --baseline "$BASELINE_WS" \
      --golden "$prompt" \
      --correctness-golden "$CORRECTNESS_GOLDEN" \
      --contract "$CONTRACT" --min-pairs 4 --target-pairs 4 \
      --candidate-spec "{\"mode\":\"${candidate}\"}" --baseline-spec '{"mode":"serial"}' \
      --prompt "$prompt" --prompt-sha256 "$sha" --target-id "$id" \
      --tag "calibration-${run}-${id}" --out "$out" \
      --calibration-bootstrap --local-dev
  done
  if [[ "$found" -eq 0 ]]; then
    echo "calibrate: no pool prompts under ${POOL_DIR}" >&2
    exit 2
  fi
  echo "calibrate: wrote $cal (one targets[] entry per pool prompt)" >&2
}

# -- Double generation: A must equal B within the declared band ---------------
cmd_compare() {
  local a="${1:-}" b="${2:-}"
  if [[ ! -f "$a" || ! -f "$b" ]]; then
    echo "compare: two calibration files are required" >&2
    exit 2
  fi
  python3 - "$a" "$b" <<'EOF'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
bad = 0
for key in ("timed_mode", "track_id"):
    if a.get(key) != b.get(key):
        print(f"MISMATCH {key}: {a.get(key)!r} != {b.get(key)!r}"); bad += 1
lo, hi = a.get("serial_band_low", 0.95), a.get("serial_band_high", 1.05)
targets = sorted(set(a.get("targets", {})) | set(b.get("targets", {})))
for t in targets:
    ta, tb = a["targets"].get(t), b["targets"].get(t)
    if not ta or not tb:
        print(f"MISMATCH target {t}: present in only one run"); bad += 1; continue
    ma, mb = ta["serial_decode_seconds_per_token_mean"], tb["serial_decode_seconds_per_token_mean"]
    ratio = mb / ma
    ok = lo <= ratio <= hi
    print(f"{'ok  ' if ok else 'OUT '} {t} A={ma:.6g} B={mb:.6g} B/A={ratio:.4f} band=[{lo},{hi}]")
    bad += 0 if ok else 1
print("A == B within band" if bad == 0 else f"{bad} mismatch(es): discard both runs and capture again")
sys.exit(0 if bad == 0 else 1)
EOF
}

# -- After the sentinel is replaced: the mirror and the controls --------------
cmd_verify() {
  cd "$REPO_ROOT"
  cargo test -p benchd official_baseline_mirrors_the_reference_constants_capture_per_platform
  cargo test -p bench-core --test loader_parity negative_control_gemma_golden_refuses_by_name
  cargo test -p bench-core --test loader_parity positive_control_pinned_track_golden_is_accepted
  cargo test -p bench-core --test loader_parity cuda_platform_pin_controls
  cargo test -p benchd run_baselines_refuses_by_name_while_the_official_baseline_is_pending
}

case "${1:-}" in
  lock|unload|reload|release|calibrate|compare|verify)
    cmd="$1"; shift
    "cmd_${cmd}" "$@"
    ;;
  *)
    sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
    ;;
esac
