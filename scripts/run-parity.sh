#!/usr/bin/env bash
# WS1-10 — M1 parity: benchctl (Rust) vs benchmark.sh (Swift) on identical
# weights+golden, then a field-by-field score.json diff → one PASS/FAIL line.
#
# SAFETY: refuses to run unless the box is quiet (no Time Machine backup in
# flight, low load, no model process). It NEVER touches the backup. Run it only
# when the box has naturally quiesced.
#
# GOLDEN INPUT (REQUIRED): the golden you point this at MUST be an
# organizer/private benchmark-ORACLE golden — one that carries a `benchmark`
# block (prefill/decode oracle tokens). Swift `benchmark.sh --local-iterate`
# (QwenRuntime.localIterate) HARD-REQUIRES `golden.benchmark`.
# `generate-golden` emits `benchmark: nil`, so this script does NOT synthesize a
# golden and there is no public CLI that can: provide a real oracle golden at
# $GOLDEN (default below) or via MLXFAST_PARITY_GOLDEN.
#
# #127 (RULED David 2026-08-20): the golden's DECLARED baseline pair is INERT on
# this leg — the reference's localIterate never reads it (it uses the compile-time
# MLXFAST_CONSTANTS officialBaseline* pair) and benchd now mirrors that. The old
# text here said the reverse ("sources its baselines FROM that block"), which was
# the retired fork's behavior. So the golden's pair no longer needs to match
# anything for the diff to be meaningful; nothing reads it.
#
# Usage: run-parity.sh [--force]
#   --force   skip the box-quiet precheck (you accept noisy timings)
set -euo pipefail

# V5: derive ROOT from the script location (run-parity.sh lives at
# $ROOT/mlxfast-bench/scripts/), so the harness is portable across the laptop and the box
# instead of hardcoding one home. Overridable via MLXFAST_PARITY_ROOT. `pwd -P` resolves
# symlinks so a symlinked checkout still yields the real tree.
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${MLXFAST_PARITY_ROOT:-$(cd -P "$HERE/../.." && pwd)}"
CHAL="$ROOT/mlxfast-challenge-dev"
ENGINE_BIN="$ROOT/mlxfast-engine/.build/release/mlxfast-engine"
SWIFT_BIN="$CHAL/.build/release/mlxfast-swift"
BENCHCTL="$ROOT/mlxfast-bench/target/release/benchctl"
# EXPORT so the parity-diff.py shim (§T4) resolves this benchctl via $BENCHCTL instead of
# silently falling back to its own repo/PATH lookup (#66 review must-fix 4).
export BENCHCTL
# WEIGHTS env knob (shared shape with the driver): default to the transformed weights beside
# the challenge checkout, override via MLXFAST_PARITY_WEIGHTS.
WEIGHTS="${MLXFAST_PARITY_WEIGHTS:-$CHAL/weights}"

# Shared precheck/lock/golden primitives (composed, not forked — see parity-lib.sh).
# shellcheck source=scripts/parity-lib.sh
. "$HERE/parity-lib.sh"
WORK="${MLXFAST_PARITY_WORKDIR:-$ROOT/mlxfast-bench/.parity}"
# Must be an organizer/private benchmark-oracle golden (see GOLDEN INPUT header).
GOLDEN="${MLXFAST_PARITY_GOLDEN:-$WORK/parity-golden.json}"
SCORE_SWIFT="$WORK/score.swift.json"
SCORE_RUST="$WORK/score.benchctl.json"
# #127 (RULED David 2026-08-20) — NO baseline plumbing on the local leg. Both runners take the
# pair from their own compile-time official-runner constants and neither offers a local override:
# the reference's localIterate reads MLXFastConstants.officialBaseline* directly, and benchd now
# mirrors that (bench_core::constants::OFFICIAL_BASELINE_*, pinned against the reference source by
# iterate::tests::local_mode_baselines_match_the_reference_constants_capture).
#
# What used to be here was a pair of literals passed BOTH ways: MLXFAST_PAIRED_BASELINE_* to the
# Swift leg and --baseline-*-spt to the benchd leg, "so speedups are comparable". Both were
# already inert on the Swift side (PairedBaselineOverride.fromEnvironment is read only by
# QwenRuntimeBenchmark, never by localIterate) and the literals were the RETIRED
# mlxfast-challenge-dev fork's constants — so the driver was quietly feeding benchd a denominator
# 28.9x/9.64x off what the reference used, which is the split #127 records. Removed rather than
# corrected: matching constants is now the two runners' own job, not this script's.

FORCE=0
while [ $# -gt 0 ]; do case "$1" in
  --force) FORCE=1;; *) echo "unknown arg $1" >&2; exit 2;; esac; shift; done

say() { printf '\n== %s ==\n' "$*"; }

# Box-quiet precheck is now the shared parity_precheck (parity-lib.sh) — one definition,
# so this harness and the manual driver can never drift apart on what "quiet" means.
precheck() { parity_precheck; }

need() { [ -x "$1" ] || { echo "missing/one-command-not-ready: $1" >&2; exit 1; }; }

# Fail-fast unless $GOLDEN exists AND carries a `benchmark` oracle block. Swift
# benchmark.sh --local-iterate sources its baselines from that block (not the
# MLXFAST_PAIRED_BASELINE_* env), so a golden without it cannot drive parity.
require_benchmark_oracle_golden() {
  say "golden benchmark-oracle precheck"
  if [ ! -f "$GOLDEN" ]; then
    echo "  FAIL: golden not found: $GOLDEN" >&2
    echo "  Provide an organizer/private benchmark-oracle golden (a 'benchmark' block +" >&2
    echo "  external baseline pair) at that path, or set MLXFAST_PARITY_GOLDEN. This script" >&2
    echo "  does NOT synthesize one (generate-golden emits benchmark: nil)." >&2
    echo; echo "PARITY: FAIL (missing benchmark-oracle golden)."; exit 1
  fi
  if ! python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if isinstance(d.get("benchmark"), dict) else 1)' "$GOLDEN"; then
    echo "  FAIL: golden $GOLDEN has no 'benchmark' oracle block." >&2
    echo "  Swift benchmark.sh --local-iterate (QwenRuntime.localIterate) throws when" >&2
    echo "  golden.benchmark == nil and reads its baselines from that block. A" >&2
    echo "  generate-golden output (benchmark: nil) cannot drive parity." >&2
    echo; echo "PARITY: FAIL (golden lacks benchmark oracle)."; exit 1
  fi
  echo "  ok: $GOLDEN carries a benchmark oracle block"
}

main() {
  need "$ENGINE_BIN"; need "$SWIFT_BIN"; need "$BENCHCTL"
  [ -d "$WEIGHTS" ] || { echo "missing transformed weights: $WEIGHTS (run: mlxfast-swift transform --reference <hf> --output weights)"; exit 1; }
  # engine needs mlx.metallib next to it (WS2-2 gap)
  [ -f "$ROOT/mlxfast-engine/.build/release/mlx.metallib" ] || echo "WARN: mlx.metallib not next to engine binary — engine may 255 (see mlxfast-engine/docs/RELEASE.md)"

  # Fail-fast on a golden that cannot drive --local-iterate BEFORE the box-quiet
  # wait: this script does NOT auto-generate one (`generate-golden` emits
  # `benchmark: nil`, and Swift `QwenRuntime.localIterate` throws when
  # `golden.benchmark == nil`). The golden MUST be an organizer/private
  # benchmark-oracle golden (see GOLDEN INPUT header).
  require_benchmark_oracle_golden

  if [ "$FORCE" -ne 1 ]; then precheck || { echo; echo "PARITY: SKIPPED (box not quiet). Re-run when quiet, or --force to override."; exit 3; }; fi

  mkdir -p "$WORK"

  say "Swift reference: benchmark.sh --local-iterate"
  ( cd "$CHAL" && MLXFAST_LOCAL_COOL_GATE=0 MLXFAST_WEIGHTS_PATH="$WEIGHTS" \
      MLXFAST_CORRECTNESS_GOLDEN_PATH="$GOLDEN" \
      MLXFAST_SCORE_PATH="$SCORE_SWIFT" ./benchmark.sh --local-iterate )

  say "Rust candidate: benchctl iterate (drives mlxfast-engine)"
  "$BENCHCTL" iterate --engine "$ENGINE_BIN" --weights "$WEIGHTS" --golden "$GOLDEN" \
    --mode local-iterate --score-path "$SCORE_RUST"

  say "field-by-field parity diff (WS1-10)"
  python3 "$HERE/parity-diff.py" "$SCORE_RUST" "$SCORE_SWIFT"
}
main "$@"
