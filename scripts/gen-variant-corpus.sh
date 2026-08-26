#!/bin/bash
# scripts/gen-variant-corpus.sh — §12 (P4) score-variant golden generator (deliverable A).
#
# Builds the §12 variant golden set the SAME way submit-1024.json was assembled:
#   1. [GPU]     mlxfast-swift generate-golden → a RAW teacher-forced base (cases[] computed BY
#                the Swift engine; "correct" == what Swift computes). This is the ONLY
#                GPU-dependent step — it runs INSIDE the window.
#   2. [OFFLINE] gen-variant-corpus.py grafts the benchmark oracle + gate sections from the donor
#                (beefed.json) onto that base, emits minimal / anchors-heavy / free-run-only /
#                behavior-bearing, PINS each (sha256 + bytes), and writes the manifest. Records
#                the reused submit-1024.json by path + its known pin (NOT regenerated).
#   3. [OFFLINE] For every variant, verify the pin (variant_pin_check) AND dual-loader acceptance
#                (benchctl validate-golden + swift preflight — both engine-free/model-free).
#
# Steps 2+3 are offline-validatable (no GPU): the offline self-test (test-variant-offline.sh)
# exercises them with a stub benchctl/swift + a canned base/donor. Only step 1 needs the box.
#
# Fails LOUD, never silently passes: a missing binary/donor, a generate-golden failure, a pin
# mismatch, or a dual-loader REJECT/HARNESS-ERR aborts with a non-zero exit and a named reason.
#
# Env contract:
#   SWIFT      mlxfast-swift binary (generate-golden + preflight)      [required]
#   BENCHCTL   benchctl binary (validate-golden)                        [required]
#   WEIGHTS    transformed weights dir (generate-golden + preflight)    [required]
#   OUT        corpus output dir                                        [required]
#   DONOR_GOLDEN   graft donor (benchmark oracle + anchors + free_run)  [default $MLXFAST_PARITY_GIT/golden/beefed.json]
#   SUBMIT_GOLDEN  reused 1024-step submit golden (NOT regenerated)     [default $MLXFAST_PARITY_GIT/golden/submit-1024.json]
#   SUBMIT_SHA / SUBMIT_BYTES  submit golden pin                        [default M-6 pin a482f22… / 20993]
#   STEPS      generate-golden reference continuation tokens            [default 129]
#              generate-golden emits EXACTLY --steps expected_tokens (main.swift:816,
#              `generateGreedyTokens(steps:)`), of which [0] is the SEED and [k+1] is decode
#              step k. local-iterate's loader demands benchmarkDecodeSteps+1 = 129, so STEPS
#              must be >= 129 or the corpus is iterate-unrunnable (#124). Enforced below.
#   PROMPT_FILE  generate-golden prompt (REQUIRED flag)                  [default $G/golden/prompts/primary.txt]
#   BASE_GOLDEN  SKIP generate-golden and use THIS raw base instead     [optional — offline / reuse]
#   GENERATE_GOLDEN_CMD  full command to produce the raw base at $RAW   [optional override]
#   GEN_PY     path to gen-variant-corpus.py                            [default alongside this script]
#   SANITY_GOLDEN  known-good golden for the preflight sanity gate      [default $DONOR_GOLDEN]
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
G="${MLXFAST_PARITY_GIT:-$HOME/mlxfast-parity-git}"
: "${SWIFT:?set SWIFT to the mlxfast-swift binary}"
: "${BENCHCTL:?set BENCHCTL to the benchctl binary}"
: "${WEIGHTS:?set WEIGHTS to a transformed weights dir}"
: "${OUT:?set OUT to the corpus output dir}"
DONOR_GOLDEN="${DONOR_GOLDEN:-$G/golden/beefed.json}"
SUBMIT_GOLDEN="${SUBMIT_GOLDEN:-$G/golden/submit-1024.json}"
SUBMIT_SHA="${SUBMIT_SHA:-a482f223edaa5b0b58e6ef0d1d276122f1a4b43f81ca6af33184cc0a64e726c9}"
SUBMIT_BYTES="${SUBMIT_BYTES:-20993}"
# #124: derived from the local-iterate decode window, NOT the flat CORRECTNESS_STEPS 64. The
# loader requires benchmarkDecodeSteps+1 = 129 expected_tokens and generate-golden emits EXACTLY
# --steps of them, so STEPS is the ARITY (129), not the window (128). A 64-step base produced a
# corpus benchctl AND Swift both refuse at load.
LOCAL_ITERATE_DECODE_STEPS="${LOCAL_ITERATE_DECODE_STEPS:-128}"
ITERATE_REQUIRED_TOKENS=$((LOCAL_ITERATE_DECODE_STEPS + 1))
STEPS="${STEPS:-$ITERATE_REQUIRED_TOKENS}"
GEN_PY="${GEN_PY:-$HERE/gen-variant-corpus.py}"
SANITY_GOLDEN="${SANITY_GOLDEN:-$DONOR_GOLDEN}"
# generate-golden REQUIRES --prompt-file (verified: `generate-golden requires --prompt-file PATH`).
# Mirror the proven author-golden.sh invocation (--prompt-file/--weights/--output/--name/--steps).
PROMPT_FILE="${PROMPT_FILE:-$G/golden/prompts/primary.txt}"
GG_NAME="${GG_NAME:-variant-base}"

# shellcheck source=scripts/variant-lib.sh
. "$HERE/variant-lib.sh"

mkdir -p "$OUT"
RAW="$OUT/base.generated.json"

for b in "$SWIFT" "$BENCHCTL"; do
  [ -x "$b" ] || { echo "gen-variant-corpus: missing binary: $b — aborting." >&2; exit 5; }
done
[ -f "$DONOR_GOLDEN" ] || { echo "gen-variant-corpus: donor golden not found: $DONOR_GOLDEN — aborting." >&2; exit 5; }
[ -f "$SUBMIT_GOLDEN" ] || { echo "gen-variant-corpus: reused submit golden not found: $SUBMIT_GOLDEN — aborting." >&2; exit 5; }
[ -f "$GEN_PY" ] || { echo "gen-variant-corpus: assembler not found: $GEN_PY — aborting." >&2; exit 5; }
# #124 guard: a base below the local-iterate loader arity yields a corpus BOTH loaders refuse
# ("expected_tokens has N tokens; need at least 129"). Refuse up front rather than burn a GPU
# window producing unrunnable goldens.
# #124 F3: validate NUMERACY first — `[ abc -lt 129 ]` exits 2 ("integer expression expected"),
# which `if` reads as FALSE, so a typo'd/empty STEPS would fall straight through the guard into the
# GPU step. Fail CLOSED on anything that is not a plain non-negative integer.
case "$STEPS" in
  ''|*[!0-9]*)
    echo "gen-variant-corpus: STEPS='$STEPS' is not a non-negative integer — aborting (#124)." >&2
    exit 5
    ;;
esac
if [ "$STEPS" -lt "$ITERATE_REQUIRED_TOKENS" ]; then
  echo "gen-variant-corpus: STEPS=$STEPS is below the local-iterate loader arity" >&2
  echo "  ($ITERATE_REQUIRED_TOKENS = benchmarkDecodeSteps $LOCAL_ITERATE_DECODE_STEPS + 1 SEED): generate-golden emits EXACTLY" >&2
  echo "  --steps expected_tokens, so the corpus would be iterate-unrunnable (#124). Aborting." >&2
  exit 5
fi

# --- step 1: [GPU] produce the raw teacher-forced base --------------------------------
# GPU-DEPENDENT: generate-golden spawns the Swift engine to teacher-force cases[]. Skipped
# when BASE_GOLDEN is supplied (an already-generated base, or an offline reuse of an existing
# golden's cases[]). The exact generate-golden flags are confirmed in the window; override via
# GENERATE_GOLDEN_CMD if they differ from the default below.
if [ -n "${BASE_GOLDEN:-}" ]; then
  [ -f "$BASE_GOLDEN" ] || { echo "gen-variant-corpus: BASE_GOLDEN not found: $BASE_GOLDEN — aborting." >&2; exit 5; }
  echo "gen-variant-corpus: using supplied BASE_GOLDEN ($BASE_GOLDEN) — skipping generate-golden (no GPU)"
  cp "$BASE_GOLDEN" "$RAW"
else
  echo "gen-variant-corpus: [GPU] generate-golden --steps $STEPS --prompt-file $PROMPT_FILE → $RAW"
  if [ -n "${GENERATE_GOLDEN_CMD:-}" ]; then
    # shellcheck disable=SC2086
    eval "$GENERATE_GOLDEN_CMD" || { echo "gen-variant-corpus: GENERATE_GOLDEN_CMD failed — aborting." >&2; exit 6; }
  else
    [ -f "$PROMPT_FILE" ] || { echo "gen-variant-corpus: prompt file not found: $PROMPT_FILE (set PROMPT_FILE) — aborting." >&2; exit 5; }
    # Proven flags (author-golden.sh:40): --prompt-file / --weights / --output / --name / --steps.
    "$SWIFT" generate-golden --prompt-file "$PROMPT_FILE" --weights "$WEIGHTS" \
      --output "$RAW" --name "$GG_NAME" --steps "$STEPS" \
      || { echo "gen-variant-corpus: generate-golden failed (check flags; override via GENERATE_GOLDEN_CMD) — aborting." >&2; exit 6; }
  fi
  [ -s "$RAW" ] || { echo "gen-variant-corpus: generate-golden produced no output at $RAW — aborting." >&2; exit 6; }
fi

# --- step 2: [OFFLINE] graft + pin + manifest -----------------------------------------
echo "gen-variant-corpus: [offline] assembling variants (graft benchmark oracle + gates)…"
python3 "$GEN_PY" --base "$RAW" --donor "$DONOR_GOLDEN" --out "$OUT" \
  --submit "$SUBMIT_GOLDEN" --submit-sha "$SUBMIT_SHA" --submit-bytes "$SUBMIT_BYTES" \
  || { echo "gen-variant-corpus: assembler failed — aborting." >&2; exit 7; }

MANIFEST="$OUT/manifest.json"
[ -f "$MANIFEST" ] || { echo "gen-variant-corpus: manifest not written ($MANIFEST) — aborting." >&2; exit 7; }

# --- step 3: [OFFLINE] pin + dual-loader every variant --------------------------------
# preflight is weights/baseline-coupled: sanity-gate a known-good golden FIRST so a Swift-setup
# breakage cannot masquerade as a per-variant REJECT (loader-parity.sh pattern).
if ! "$SWIFT" preflight --golden "$SANITY_GOLDEN" --weights "$WEIGHTS" >/dev/null 2>&1; then
  echo "gen-variant-corpus: FATAL — swift preflight REJECTED the known-good sanity golden ($SANITY_GOLDEN)." >&2
  echo "  The Swift setup (weights/baseline) is broken; per-variant decisions would be unattributable. Aborting." >&2
  exit 4
fi
echo "gen-variant-corpus: swift-leg sanity OK (known-good golden accepted by preflight)"

FAIL=0
# Iterate the manifest (anchored parse): class \t path \t sha \t bytes \t declared.
while IFS=$'\t' read -r cls path sha bytes declared; do
  [ -n "$cls" ] || continue
  echo "gen-variant-corpus: verifying $cls ($path)"
  if ! variant_pin_check "$path" "$sha" "$bytes"; then FAIL=$((FAIL+1)); continue; fi
  # All variants here carry the benchmark oracle → full validate-golden (no --gates-only).
  if ! variant_dual_loader "$BENCHCTL" "$SWIFT" "$WEIGHTS" "$path" "$sha" "$bytes" 0; then FAIL=$((FAIL+1)); fi
done < <(variant_manifest_rows "$MANIFEST") || { echo "gen-variant-corpus: manifest parse failed — aborting." >&2; exit 7; }

echo ""
if [ "$FAIL" -eq 0 ]; then
  echo "gen-variant-corpus: RESULT PASS — every variant pinned + dual-loader accepted; manifest at $MANIFEST"
  exit 0
else
  echo "gen-variant-corpus: RESULT FAIL — $FAIL variant(s) failed pin/dual-loader (see lines above)" >&2
  exit 1
fi
