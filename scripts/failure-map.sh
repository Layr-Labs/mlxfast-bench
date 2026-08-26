#!/bin/bash
# P5 — failure-map harness. For each failure-class variant (gen-failure-corpus.py), run
# benchctl iterate AND swift benchmark --local-iterate, record each side's pass/fail, and
# field-diff the two FAILING score.jsons on the SHARED surface (the differ already gates the
# failing-run fields: first_failing_case/step, expected/actual_token, passed, error by
# semantics). Exit behavior alone is NOT parity — this diffs the fields.
#
# Verdict column: PASS / FAIL / DECLARED(#nn) / TOOL-ERR. A class the corpus manifest marks
# `declared` (an issue ref) renders DECLARED(#nn) instead of FAIL — the divergence is signed and
# audited. FAIL is reserved for UNDECLARED cells only, so a FAIL
# in this table always means "act on this".
#
# COMPOSED by run-manual-test.sh (leg 2 parses this truth table) and runnable standalone.
#
# GPU: needs the engine on both sides (correctness needs actual tokens), so run inside a GPU
# window (caller unloads/reloads qwen). Parameterized by env; no box paths hard-coded here.
#
# Required env: ENGINE SWIFT BENCHCTL WEIGHTS GEN(gen-failure-corpus.py) GOLDEN OUT
#   Differ knob (ONE shape, shared with run-manual-test.sh): DIFF_CMD = a full command,
#   e.g. "python3 .../parity-diff.py" or (post-#66) "$BENCHCTL parity-diff". Legacy DIFF (a
#   bare parity-diff.py path) is still accepted and wrapped as `python3 $DIFF`.
#   Optional: GPU(gpu_run.py wrapper) · FM_MIN_CLASSES(corpus floor, default 5).
# Thermal: MODE-derived + SYMMETRIC. local-iterate is measured cool-gate OFF on BOTH sides —
#   benchctl by RULING-A3 default, Swift via MLXFAST_LOCAL_COOL_GATE=0 set here. local-submit
#   gates ON by RULING but both native gates no-op w/o a temp reader → both legs un-gated.
set -uo pipefail
: "${ENGINE:?}" "${SWIFT:?}" "${BENCHCTL:?}" "${WEIGHTS:?}" "${GEN:?}" "${GOLDEN:?}" "${OUT:?}"
# Unify the differ knob: prefer DIFF_CMD; fall back to legacy DIFF (a parity-diff.py path).
if [ -z "${DIFF_CMD:-}" ]; then
  if [ -n "${DIFF:-}" ]; then DIFF_CMD="python3 $DIFF"; else
    echo "failure-map: need DIFF_CMD (full differ command) or legacy DIFF (parity-diff.py path)" >&2; exit 2
  fi
fi
RUN="${GPU:+python3 $GPU}"   # optional gpu_run.py flock wrapper; empty = run directly
# MODE (M-6): local-iterate | local-submit. leg-2 runs `benchctl iterate --mode $MODE` and
# `mlxfast-swift benchmark --$MODE` on the (corrupted) mode golden. The Swift cool-gate env is
# MODE-derived: local-iterate forces the gate OFF (MLXFAST_LOCAL_COOL_GATE=0 iterate override);
# local-submit passes NOTHING so the Swift submit leg keeps its helper-based gate as a no-op
# (no helper set) — measured un-gated, symmetric with benchctl's native gate skipping w/o a reader.
MODE="${MODE:-local-iterate}"
case "$MODE" in
  local-iterate) SWIFT_COOL_KV="MLXFAST_LOCAL_COOL_GATE=0";;
  local-submit)  SWIFT_COOL_KV="";;
  *) echo "failure-map: unknown MODE '$MODE' (want local-iterate|local-submit)" >&2; exit 2;;
esac
FM_MIN_CLASSES="${FM_MIN_CLASSES:-5}"
CORPUS="$OUT/corpus"; mkdir -p "$CORPUS"

# Anti-fabrication for STANDALONE use (Fable checkpoint Finding 5d): when composed by the
# driver, OUT is a freshly-wiped dir; run standalone with a reused OUT, a prior run's
# score/diff for a class whose binary fails THIS run would be read as a stale pass. Wipe the
# per-run artifacts at startup so a failed binary leaves its score absent (→ missing marker).
rm -f "$OUT"/score.bc.*.json "$OUT"/score.swift.*.json "$OUT"/diff.*.txt \
      "$OUT"/bc.*.log "$OUT"/sw.*.log 2>/dev/null || true

# Generate the corpus (stdout → stderr so the parsed truth table below stays clean).
python3 "$GEN" "$GOLDEN" "$CORPUS" 1>&2
# Corpus floor: refuse to emit a thin/degenerate table (anti-fabrication). Validate N is a
# positive integer first — a non-numeric value would make `[ "" -lt N ]` return non-zero and
# silently skip the floor (#67 red-team, twin of the driver's N_MANIFEST guard).
N="$(python3 -c "import json;print(len(json.load(open('$CORPUS/manifest.json'))['variants']))" 2>/dev/null)"
case "$N" in ''|*[!0-9]*) echo "failure-map: corpus manifest variant count not a number ('$N') — refusing to run" >&2; exit 6;; esac
if [ "$N" -lt "$FM_MIN_CLASSES" ]; then
  echo "failure-map: corpus too small ($N variants < floor $FM_MIN_CLASSES) — refusing to run" >&2; exit 6
fi
echo >&2

printf '%-16s | %-9s | %-9s | %s\n' "class" "bc passed" "sw passed" "shared-surface diff"
printf -- '-----------------|-----------|-----------|--------------------\n'

passed_of() { python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get('passed'))" "$1" 2>/dev/null || echo "ERR"; }

# Emit `class<TAB>declared` per variant so the renderer can bind a manifest-declared divergence
# to DECLARED(#nn) in the verdict column. `declared` is empty for
# ordinary classes. (`or ""` collapses JSON null.)
while IFS=$'\t' read -r v declared; do
  g="$CORPUS/$v.json"
  bc="$OUT/score.bc.$v.json"; sw="$OUT/score.swift.$v.json"
  # benchctl: gate per RULING for $MODE (native gate skips w/o a temp reader). Swift: MODE-derived
  # cool-gate env ($SWIFT_COOL_KV — iterate forces OFF, submit leaves the helper no-op; the empty
  # word splits away so `env` just execs swift).
  $RUN "$BENCHCTL" iterate --engine "$ENGINE" --weights "$WEIGHTS" --golden "$g" \
    --mode "$MODE" --score-path "$bc" > "$OUT/bc.$v.log" 2>&1
  env $SWIFT_COOL_KV $RUN "$SWIFT" benchmark --weights "$WEIGHTS" --golden "$g" \
    --score-path "$sw" "--$MODE" > "$OUT/sw.$v.log" 2>&1
  bcp="$(passed_of "$bc")"; swp="$(passed_of "$sw")"
  if [ -s "$bc" ] && [ -s "$sw" ]; then
    # Only differ exit 0/1 is a verdict; anything else (shim 9, benchctl 2/3) is a TOOL error,
    # marked as such rather than a blank cell (Fable checkpoint Finding 5c, leg-2 side).
    $DIFF_CMD "$bc" "$sw" > "$OUT/diff.$v.txt" 2>&1; drc=$?
    verdict="$(grep '^PARITY:' "$OUT/diff.$v.txt" | sed 's/PARITY: //' | head -1)"
    case "$drc" in 0|1) [ -n "$verdict" ] || verdict="TOOL-ERR (differ exit $drc, no PARITY line)";; *) verdict="TOOL-ERR (differ exit $drc)";; esac
  else
    verdict="(missing score.json — bc:$([ -s "$bc" ] && echo y || echo n) sw:$([ -s "$sw" ] && echo y || echo n))"
  fi
  # No-undeclared-cells rule: a manifest-DECLARED class renders DECLARED(#nn), never FAIL — the
  # divergence is signed/audited, stop acting on it. This ONLY rewrites a real FAIL verdict; a
  # PASS (divergence resolved) or a TOOL-ERR (harness broken, not a divergence) passes through
  # unchanged. FAIL survives ONLY for UNDECLARED classes (act on this). The field-diff itself
  # stays in diff.<class>.txt — only the rendered cell changes.
  if [ "$verdict" = "FAIL" ] && [ -n "$declared" ]; then verdict="DECLARED($declared)"; fi
  printf '%-16s | %-9s | %-9s | %s\n' "$v" "$bcp" "$swp" "${verdict:-（no verdict）}"
done < <(python3 -c "import json;[print(x['class']+'\t'+(x.get('declared') or '')) for x in json.load(open('$CORPUS/manifest.json'))['variants']]")
echo
echo "per-class field diffs in $OUT/diff.<class>.txt; logs in $OUT/{bc,sw}.<class>.log"
