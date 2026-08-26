#!/bin/bash
# scripts/variant-parity.sh — §12 (P4) score-variant deterministic parity leg (deliverable B).
#
# For EACH variant in the corpus manifest, run `benchctl iterate` AND `mlxfast-swift benchmark`
# in BOTH local modes (local-iterate + local-submit) and field-diff the two score.jsons on the
# DETERMINISTIC surface via `$DIFF_CMD` (default `benchctl parity-diff`, whose bucket policy
# already EXCLUDES timing/environmental fields — timing is not compared). §12 runs
# deterministic-field parity, which is native-local Swift-exact by default
# (`CorrectnessScope::BaseCasesOnly` — anchors/free_run/behavior are NOT evaluated in local
# modes), so the widening gate sections are a LOADER-accept + score-invariance probe, not a
# gate-evaluation probe.
#
# Deterministic fields don't need timing repeats, so each variant runs ONCE per mode (the 3-pair
# primary leg in run-manual-test.sh covers GPU timing on the primary golden — variants are NOT
# re-timed). Acceptance: every variant PARITY: PASS on the deterministic surface in BOTH modes,
# OR a DECLARED(<ref>) cell (manifest `declared` → the no-undeclared-cells rule; never FAIL).
#
# COMPOSED by run-variant-window.sh (which parses this truth table) and runnable standalone.
# GPU: needs the engine on both sides → run inside a GPU window (caller unloads/reloads qwen).
#
# Required env: ENGINE SWIFT BENCHCTL WEIGHTS MANIFEST(corpus manifest.json) OUT
#   DIFF_CMD  full differ command (default `$BENCHCTL parity-diff`).
#   Optional: GPU(gpu_run.py wrapper, empty = direct) · MODES(default "local-iterate local-submit").
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
: "${ENGINE:?}" "${SWIFT:?}" "${BENCHCTL:?}" "${WEIGHTS:?}" "${MANIFEST:?}" "${OUT:?}"
DIFF_CMD="${DIFF_CMD:-$BENCHCTL parity-diff}"
RUN="${GPU:+python3 $GPU}"   # optional gpu_run.py flock wrapper; empty = run directly
MODES="${MODES:-local-iterate local-submit}"

# shellcheck source=scripts/variant-lib.sh
. "$HERE/variant-lib.sh"

mkdir -p "$OUT"
[ -f "$MANIFEST" ] || { echo "variant-parity: manifest not found: $MANIFEST" >&2; exit 2; }

# Anti-fabrication (failure-map.sh Finding 5d): wipe per-run artifacts so a variant whose binary
# FAILS this run leaves its score ABSENT (→ missing marker), never a stale prior pass.
rm -f "$OUT"/score.bc.*.json "$OUT"/score.swift.*.json "$OUT"/diff.*.txt \
      "$OUT"/bc.*.log "$OUT"/sw.*.log "$OUT"/verdict.*.txt 2>/dev/null || true

score_of() { python3 -c "import json,sys
d=json.load(open(sys.argv[1]));s=d.get('score')
print('NULL' if s is None else format(float(s),'.4f'))" "$1" 2>/dev/null || echo MISSING; }

cool_kv_for() { case "$1" in local-iterate) echo "MLXFAST_LOCAL_COOL_GATE=0";; *) echo "";; esac; }

# Run one (variant,mode) pair; sets REPLY_VERDICT (raw PASS/FAIL/TOOL-ERR…) + prints a log line.
run_pair() {  # $1=class $2=path $3=sha $4=bytes $5=mode
  local cls="$1" path="$2" sha="$3" bytes="$4" mode="$5"
  local bc="$OUT/score.bc.$cls.$mode.json" sw="$OUT/score.swift.$cls.$mode.json"
  local ck; ck="$(cool_kv_for "$mode")"
  # benchctl: pin-gated golden, mode-selected decode window; drop a partial score on failure so a
  # crash mid-write can't leave a half-file that parses (belt-and-suspenders with the startup wipe).
  $RUN "$BENCHCTL" iterate --engine "$ENGINE" --weights "$WEIGHTS" --golden "$path" \
    --golden-sha256 "$sha" --golden-bytes "$bytes" --mode "$mode" --score-path "$bc" \
    > "$OUT/bc.$cls.$mode.log" 2>&1 || { rm -f "$bc"; }
  # swift: MODE-derived cool-gate env (empty word-splits away → plain exec).
  env $ck $RUN "$SWIFT" benchmark --weights "$WEIGHTS" --golden "$path" \
    --score-path "$sw" "--$mode" > "$OUT/sw.$cls.$mode.log" 2>&1 || { rm -f "$sw"; }
  if [ ! -s "$bc" ] || [ ! -s "$sw" ]; then
    REPLY_VERDICT="TOOL-ERR (missing score — bc:$([ -s "$bc" ] && echo y || echo n) sw:$([ -s "$sw" ] && echo y || echo n))"
    return
  fi
  # Only differ exit 0/1 is a verdict; anything else (shim 9, benchctl 2/3) is a TOOL error.
  $DIFF_CMD "$bc" "$sw" > "$OUT/diff.$cls.$mode.txt" 2>&1; local drc=$?
  local vd; vd="$(grep '^PARITY:' "$OUT/diff.$cls.$mode.txt" | sed 's/PARITY: //' | head -1)"
  case "$drc" in 0|1) [ -n "$vd" ] || vd="TOOL-ERR (differ exit $drc, no PARITY line)";; *) vd="TOOL-ERR (differ exit $drc)";; esac
  REPLY_VERDICT="$vd"
}

echo "variant-parity: differ=$DIFF_CMD ($("$BENCHCTL" parity-diff --version 2>/dev/null || echo unknown)); modes=[$MODES]"
echo ""
printf '%-16s | %-12s | %-12s | %s\n' "variant" "iterate" "submit" "sections"
printf -- '-----------------|--------------|--------------|--------------------\n'

TABLE_FILE="$OUT/variant-parity.table.txt"; : > "$TABLE_FILE"
UNDECLARED_FAIL=0; TOOL_ERR=0; N=0; APPLICABLE_RUNS=0; NA_CELLS=0

# Look up a manifest field for one variant (informational; manifest is the anchor).
field_of() {  # $1=class $2=jq-ish python expr producing a string
  python3 -c "import json,sys
m=json.load(open(sys.argv[1]))
for v in m['variants']:
    if v['class']==sys.argv[2]:
        $2; break" "$MANIFEST" "$1" 2>/dev/null
}

# Manifest-anchored iteration: one row per manifest variant; a stray file that is not a manifest
# variant can never appear, and the caller re-checks the row count against the manifest length.
while IFS=$'\t' read -r cls path sha bytes declared; do
  [ -n "$cls" ] || continue
  N=$((N+1))
  sections="$(field_of "$cls" "print('+'.join(s.replace('correctness_gates.','cg.') for s in v.get('sections',[])))")"
  # APPLICABLE modes for THIS variant (manifest). An iterate-scale golden (cases[0] < 1024 tokens)
  # PHYSICALLY cannot run local-submit and, under local BaseCasesOnly, would only reproduce
  # submit-1024's cases score anyway — so the inapplicable mode is N/A (declared, non-FAIL), NOT
  # a TOOL-ERR. submit coverage is submit-1024's job; the shape variants prove local-iterate.
  # #124 F5: NO fallback default. `applicable_modes` defaulting to ['local-iterate'] is exactly the
  # pre-#124 behavior — a legacy/hand-rolled manifest missing the key would silently reinstate the
  # defect (an iterate-unrunnable variant claimed as iterate-applicable → TOOL-ERR at load). A
  # manifest that does not state the key is a corpus defect: fail LOUD.
  amodes="$(field_of "$cls" "print(' '.join(v['applicable_modes']))")"
  if [ -z "$amodes" ]; then
    echo "variant-parity: FATAL manifest variant '$cls' declares no \`applicable_modes\` — regenerate" >&2
    echo "  the corpus with gen-variant-corpus.py (#124). Refusing to assume local-iterate." >&2
    exit 3
  fi
  cell_it="-"; cell_su="-"
  for mode in $MODES; do
    if printf ' %s ' $amodes | grep -q " $mode "; then
      run_pair "$cls" "$path" "$sha" "$bytes" "$mode"
      raw="$REPLY_VERDICT"
      rendered="$(variant_render_verdict "$raw" "$declared")"
      APPLICABLE_RUNS=$((APPLICABLE_RUNS+1))
      # POSITIVE acceptance (no silent pass): ONLY an explicit `PASS*` verdict is accepted; a
      # `FAIL*` is an undeclared FAIL unless declared; ANYTHING ELSE (TOOL-ERR*, empty, or an
      # unrecognized verdict shape — e.g. if benchctl ever SUFFIXED `FAIL` the way it suffixes
      # PASS) fails closed as a TOOL-ERR and breaks acceptance. DECLARED rewrites the CELL, not
      # the ledger's intent. Prefix matches so a suffixed verdict cannot slip past.
      case "$raw" in
        PASS*) : ;;
        FAIL*) [ -n "$declared" ] || UNDECLARED_FAIL=$((UNDECLARED_FAIL+1)) ;;
        *)     TOOL_ERR=$((TOOL_ERR+1)) ;;
      esac
      echo "  $cls/$mode: bc=$(score_of "$OUT/score.bc.$cls.$mode.json") sw=$(score_of "$OUT/score.swift.$cls.$mode.json") $rendered" >&2
    else
      # Inapplicable mode → N/A cell. Not run, not counted as FAIL/TOOL-ERR (declared skip).
      rendered="N/A"
      NA_CELLS=$((NA_CELLS+1))
      echo "  $cls/$mode: N/A (not applicable; applicable=[$amodes] — iterate-scale golden, submit needs >=1024 tokens)" >&2
    fi
    case "$mode" in
      local-iterate) cell_it="$rendered" ;;
      local-submit)  cell_su="$rendered" ;;
      *)             cell_it="$rendered" ;;  # single-mode override lands in the first column
    esac
  done
  printf '%-16s | %-12s | %-12s | %s\n' "$cls" "$cell_it" "$cell_su" "$sections" | tee -a "$TABLE_FILE"
done < <(variant_manifest_rows "$MANIFEST") || { echo "variant-parity: manifest parse failed" >&2; exit 3; }

echo ""
echo "variant-parity: N/A cells (inapplicable mode; declared, non-FAIL) = $NA_CELLS"
if [ "$N" -eq 0 ]; then echo "variant-parity: RESULT FAIL — zero variants (a zero-variant run must never PASS)" >&2; exit 3; fi
if [ "$APPLICABLE_RUNS" -eq 0 ]; then echo "variant-parity: RESULT FAIL — zero applicable (variant,mode) runs (all N/A) — refusing to PASS" >&2; exit 3; fi
if [ "$UNDECLARED_FAIL" -eq 0 ] && [ "$TOOL_ERR" -eq 0 ]; then
  echo "variant-parity: RESULT PASS — $N variants, $APPLICABLE_RUNS applicable (variant,mode) runs, deterministic parity across every APPLICABLE mode (undeclared-FAIL=0, TOOL-ERR=0; N/A=$NA_CELLS)"
  exit 0
else
  echo "variant-parity: RESULT FAIL — undeclared-FAIL=$UNDECLARED_FAIL TOOL-ERR=$TOOL_ERR across $APPLICABLE_RUNS applicable runs ($N variants)" >&2
  exit 1
fi
