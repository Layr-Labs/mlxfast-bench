#!/bin/bash
# scripts/official-failure-map.sh — B-3 Leg 2: the OFFICIAL failure map.
#
# For each corruption class (gen-failure-corpus.py over the clean official golden) run BOTH sides
# at OFFICIAL semantics — `benchctl iterate --mode official` and DIRECT `mlxfast-swift benchmark`
# in official env — record each side's `.passed`, and field-diff the two FAILING sealed scores on
# the shared surface via the differ. Verdict column: PASS / FAIL / DECLARED(#nn) / TOOL-ERR (a
# manifest-declared class renders DECLARED(#nn), never FAIL).
#
# THE class local could not test is `oracle` (benchmark decode-oracle token flip — the 128-step
# timed path). At official semantics it MUST FAIL BOTH SIDES; a both-PASS there is a HARNESS
# FAILURE (the oracle gate is not wired), asserted after the table and aborts LOUD. primary /
# anchor / free-run exercise the full correctness scope official runs (base + anchors + free_run).
#
# GPU: needs a real sandboxed worker on both sides → run inside a GPU window (caller unloads/
# reloads qwen). Fails LOUD; a missing sealed score is a TOOL-ERR cell, never a blank pass.
#
# RULING 2 — submit-1024 band-failure FIXTURE: if BAND_FIXTURE_GOLDEN is set (the STALE-baseline
# submit-1024), the leg ALSO runs it official on BOTH sides and asserts a declared, EXPECTED cell:
# both sides FAIL the acceptance band identically (both `.passed=False`, both carry the band-failure
# signature, and the two blanked failed surfaces byte-match via the differ — the RULING-2-aligned
# timed-band blanking). This is a runnable truth-table cell, not an error; a both-PASS / divergence
# there is a real problem and aborts LOUD.
#
# Env: BENCHCTL ENGINE SWIFT WEIGHTS · GEN(gen-failure-corpus.py) · OFFICIAL_GOLDEN · OUT · DIFF_CMD
#      optional: OFFICIAL_COMMIT · FM_MIN_CLASSES(default 4) · SWIFT_REPO_ROOT · *_EXTRA_ENV ·
#                BAND_FIXTURE_GOLDEN + BAND_FIXTURE_PIN_SHA/BAND_FIXTURE_PIN_BYTES (band fixture)
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/official-lib.sh"

: "${BENCHCTL:?set BENCHCTL}" "${ENGINE:?set ENGINE}" "${SWIFT:?set SWIFT}" "${WEIGHTS:?set WEIGHTS}"
: "${GEN:?set GEN to gen-failure-corpus.py}" "${OFFICIAL_GOLDEN:?set OFFICIAL_GOLDEN}" "${OUT:?set OUT}"
DIFF_CMD="${DIFF_CMD:-$BENCHCTL parity-diff}"
FM_MIN_CLASSES="${FM_MIN_CLASSES:-4}"
export WEIGHTS SWIFT BENCHCTL ENGINE
export OFFICIAL_COMMIT="$(official_commit_sha40)"

CORPUS="$OUT/corpus"; mkdir -p "$CORPUS"
# Anti-stale (standalone reuse): wipe per-run artifacts so a class whose binary fails THIS run
# leaves its score ABSENT (→ TOOL-ERR marker), never a prior run's stale pass.
rm -rf "$OUT"/cls.* 2>/dev/null || true
rm -f "$OUT"/official-failure-map.table.txt 2>/dev/null || true

# Generate the corpus from the CLEAN official golden (stdout → stderr so the table stays clean).
python3 "$GEN" "$OFFICIAL_GOLDEN" "$CORPUS" 1>&2
MANIFEST="$CORPUS/manifest.json"
[ -f "$MANIFEST" ] || { echo "official-failure-map: corpus manifest missing — aborting" >&2; exit 6; }
N="$(python3 -c "import json;print(len(json.load(open('$MANIFEST'))['variants']))" 2>/dev/null)"
case "$N" in ''|*[!0-9]*) echo "official-failure-map: manifest variant count not a number ('$N') — refusing" >&2; exit 6;; esac
if [ "$N" -lt "$FM_MIN_CLASSES" ]; then
  echo "official-failure-map: corpus too small ($N < floor $FM_MIN_CLASSES) — refusing" >&2; exit 6
fi
# The oracle class is the whole point of the official failure map — refuse to run without it.
if ! python3 -c "import json,sys; sys.exit(0 if any(v['class']=='oracle' for v in json.load(open('$MANIFEST'))['variants']) else 1)"; then
  echo "official-failure-map: corpus has NO 'oracle' class — the golden lacks a benchmark decode-oracle; refusing" >&2; exit 6
fi
echo >&2

TABLE="$OUT/official-failure-map.table.txt"; : > "$TABLE"
printf '%-16s | %-9s | %-9s | %s\n' "class" "bc.pass" "sw.pass" "shared-surface diff" | tee -a "$TABLE"
printf -- '-----------------|-----------|-----------|--------------------\n' | tee -a "$TABLE"

# per-class captured pass state for the post-table assertions.
ORACLE_BC=""; ORACLE_SW=""
# Verdict accumulator (BLOCKER-1): an undeclared FAIL / TOOL-ERR / missing-score cell must break
# the leg (mirrors official-parity's per-check accumulator). DECLARED(#nn) and PASS do NOT.
FAIL=0

while IFS=$'\t' read -r cls declared; do
  [ -n "$cls" ] || continue
  g="$CORPUS/$cls.json"
  d="$OUT/cls.$cls"; bcd="$d/bc"; swd="$d/sw"
  mkdir -p "$bcd" "$swd"
  OFFICIAL_GOLDEN="$g" official_benchctl_run "$bcd"
  OFFICIAL_GOLDEN="$g" official_swift_run "$swd"
  bcp="$(official_passed_of "$bcd/score.json")"; swp="$(official_passed_of "$swd/score.json")"
  if [ -s "$bcd/score.json" ] && [ -s "$swd/score.json" ]; then
    $DIFF_CMD "$bcd/score.json" "$swd/score.json" > "$d/diff.txt" 2>&1; drc=$?
    vd="$(official_diff_cell "$drc" "$d/diff.txt")"
  else
    vd="(missing score — bc:$([ -s "$bcd/score.json" ] && echo y || echo n) sw:$([ -s "$swd/score.json" ] && echo y || echo n))"
  fi
  # No-undeclared-cells rule: a declared class's FAIL renders DECLARED(#nn).
  vd="$(official_render_verdict "$vd" "$declared")"
  printf '%-16s | %-9s | %-9s | %s\n' "$cls" "$bcp" "$swp" "$vd" | tee -a "$TABLE"
  # Accumulate: anything that is not PASS and not DECLARED(...) breaks the leg (undeclared FAIL,
  # TOOL-ERR, or a missing-score cell). A declared divergence and a clean PASS do not.
  case "$vd" in
    PASS|DECLARED\(*) : ;;
    *) FAIL=$((FAIL+1)); echo "official-failure-map: undeclared non-PASS cell for '$cls': $vd" >&2 ;;
  esac
  if [ "$cls" = "oracle" ]; then ORACLE_BC="$bcp"; ORACLE_SW="$swp"; fi
done < <(python3 -c "import json;[print(x['class']+'\t'+(x.get('declared') or '')) for x in json.load(open('$MANIFEST'))['variants']]")

echo ""
# --- oracle-corruption assertion: MUST fail BOTH sides (a both-PASS is a harness failure) ---
# `.passed` must be the literal False on both sides. Anything else — True, MISSING, ERR — means the
# benchmark-oracle gate did not fire on one side; abort LOUD (never let it read as parity).
# MAJOR-1: route the assertion-OK line to STDERR too (the driver greps the leg's stderr for it),
# so the oracle-fails-both proof shows in the REPORT on a PASSING run, not only on failure.
if [ "$ORACLE_BC" = "False" ] && [ "$ORACLE_SW" = "False" ]; then
  ORACLE_OK="official-failure-map: oracle-corruption assertion OK — benchctl=$ORACLE_BC swift=$ORACLE_SW (both FAIL)"
  echo "$ORACLE_OK"; echo "$ORACLE_OK" >&2   # stdout for the table/log; stderr for the driver's grep
else
  echo "official-failure-map: ORACLE ASSERTION FAILED — benchctl=$ORACLE_BC swift=$ORACLE_SW (expected both False); a both-PASS/missing here is a HARNESS FAILURE (oracle gate not wired) — aborting" >&2
  exit 5
fi
# --- submit-1024 band-failure FIXTURE (RULING 2): both sides FAIL the band identically ------------
# A declared, EXPECTED truth-table cell — NOT an error. Runs the STALE-baseline submit-1024 official
# on both sides and asserts: both `.passed=False`, both carry the band-failure signature, and the two
# blanked failed surfaces byte-match (the differ agrees). A both-PASS / divergence aborts LOUD.
if [ -n "${BAND_FIXTURE_GOLDEN:-}" ]; then
  bf="$BAND_FIXTURE_GOLDEN"
  [ -r "$bf" ] || { echo "official-failure-map: BAND_FIXTURE_GOLDEN not readable: $bf — aborting" >&2; exit 7; }
  # Optional fixture pin (the stale submit-1024 is itself pinned).
  if [ -n "${BAND_FIXTURE_PIN_SHA:-}" ] && [ -n "${BAND_FIXTURE_PIN_BYTES:-}" ]; then
    fbytes="$(wc -c < "$bf" | tr -d ' ')"; fsha="$(official_sha_of "$bf")"
    if [ "$fbytes" != "$BAND_FIXTURE_PIN_BYTES" ] || [ "$fsha" != "$BAND_FIXTURE_PIN_SHA" ]; then
      echo "official-failure-map: band-fixture pin mismatch (bytes $fbytes/$BAND_FIXTURE_PIN_BYTES sha $fsha/$BAND_FIXTURE_PIN_SHA) — refusing" >&2; exit 7
    fi
  fi
  bfd="$OUT/band-fixture"; bfbc="$bfd/bc"; bfsw="$bfd/sw"; mkdir -p "$bfbc" "$bfsw"
  OFFICIAL_GOLDEN="$bf" official_benchctl_run "$bfbc"
  OFFICIAL_GOLDEN="$bf" official_swift_run "$bfsw"
  BAND_BC="$(official_passed_of "$bfbc/score.json")"; BAND_SW="$(official_passed_of "$bfsw/score.json")"
  # Band-failure signature (field-name-agnostic grep of the sealed score text): the check() reason
  # "improvement too large..." wrapped as "acceptance band failed: ...".
  BAND_SIG='improvement too large\|acceptance band failed\|below -5'
  bc_sig=no; sw_sig=no
  grep -qi "$BAND_SIG" "$bfbc/score.json" 2>/dev/null && bc_sig=yes
  grep -qi "$BAND_SIG" "$bfsw/score.json" 2>/dev/null && sw_sig=yes
  # Differ on the two FAILING scores — the RULING-2-aligned blanked surfaces must byte-match.
  if [ -s "$bfbc/score.json" ] && [ -s "$bfsw/score.json" ]; then
    $DIFF_CMD "$bfbc/score.json" "$bfsw/score.json" > "$bfd/diff.txt" 2>&1; bdrc=$?
    band_vd="$(official_diff_cell "$bdrc" "$bfd/diff.txt")"
  else
    band_vd="(missing score — bc:$([ -s "$bfbc/score.json" ] && echo y || echo n) sw:$([ -s "$bfsw/score.json" ] && echo y || echo n))"
  fi
  printf '%-16s | %-9s | %-9s | %s\n' "submit-1024-band" "$BAND_BC" "$BAND_SW" "$band_vd (declared band-fail fixture)" | tee -a "$TABLE"
  if [ "$BAND_BC" = "False" ] && [ "$BAND_SW" = "False" ] && [ "$bc_sig" = "yes" ] && [ "$sw_sig" = "yes" ] && [ "$band_vd" = "PASS" ]; then
    BAND_OK="official-failure-map: band-failure fixture assertion OK — submit-1024 STALE baselines FAIL the acceptance band on BOTH sides identically (bc=$BAND_BC sw=$BAND_SW; band-signature both; blanked failed surface byte-matches: $band_vd)"
    echo "$BAND_OK"; echo "$BAND_OK" >&2
  else
    echo "official-failure-map: BAND FIXTURE ASSERTION FAILED — expected both .passed=False + band-signature both + differ PASS (blanked surfaces byte-match), got bc=$BAND_BC sw=$BAND_SW sig(bc=$bc_sig sw=$sw_sig) differ=$band_vd — aborting" >&2
    exit 8
  fi
fi

echo "per-class field diffs in $OUT/cls.<class>/diff.txt; per-side artifacts under $OUT/cls.<class>/{bc,sw}/"

# Derived leg verdict (BLOCKER-1): the oracle gate fired (asserted above), but an undeclared
# non-PASS cell still fails the leg so the window cannot render an unbacked GREEN.
if [ "$FAIL" -eq 0 ]; then
  echo "official-failure-map: RESULT PASS — oracle fails BOTH sides; no undeclared FAIL/TOOL-ERR/missing-score"
  exit 0
else
  echo "official-failure-map: RESULT FAIL — $FAIL undeclared non-PASS cell(s); see the table + $OUT/cls.<class>/diff.txt" >&2
  exit 1
fi
