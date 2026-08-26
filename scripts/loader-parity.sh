#!/usr/bin/env bash
# WS1-10 loader-parity (cross-language). Run the SAME fixture corpus through BOTH loaders —
# `benchctl validate-golden` (Rust bench-core) and `mlxfast-swift preflight` (live Swift
# Golden.swift) — and assert identical accept/reject decisions, flagging manifest-declared
# intentional divergences (e.g. Rust's stricter per-case deny_unknown_fields). preflight is
# model-free: NO GPU, NO qwen unload required. Writes a diff report artifact.
#
# Fails LOUD (never silently PASS) on: missing/unreadable/empty manifest, a fixture file
# that does not exist, or a loader exit code that is neither 0 (ACCEPT) nor 1 (REJECT)
# (anything else is a HARNESS ERROR, not a decision). Exit: 0 = clean, 2 = usage, 3 = fatal
# setup error (manifest), 1 = one or more mismatches / missing fixtures / harness errors.
#
# Usage:
#   BENCHCTL=<benchctl> SWIFT=<mlxfast-swift> WEIGHTS=<transformed-weights-dir> \
#     scripts/loader-parity.sh <corpus-dir> <report-out>
set -uo pipefail
CORPUS="${1:?corpus dir}"
REPORT="${2:?report output path}"
: "${BENCHCTL:?set BENCHCTL to the benchctl binary}"
: "${SWIFT:?set SWIFT to the mlxfast-swift binary}"
: "${WEIGHTS:?set WEIGHTS to a transformed weights dir}"
MANIFEST="${CORPUS}/manifest.json"
command -v jq >/dev/null || { echo "loader-parity: FATAL jq required" >&2; exit 2; }

# (i) manifest must exist, be readable, be valid JSON, and carry >=1 fixture — else FATAL.
[ -r "$MANIFEST" ] || { echo "loader-parity: FATAL manifest missing/unreadable: $MANIFEST" >&2; exit 3; }
# (v) one-pass jq: file<TAB>swift_diverges<TAB>gates_only<TAB>note per fixture. `gates_only`
# (default false) marks a fixture that LEGITIMATELY lacks the benchmark oracle for a purely
# STRUCTURAL check — validate-golden then gets `--gates-only` so it is not rejected on the
# oracle requirement (#77). `mapfile` is bash 4+, and macOS ships bash 3.2, so capture then
# read into an array portably.
ROWS_RAW="$(jq -er '.fixtures[] | [.file, (.swift_diverges|tostring), (.gates_only // false | tostring), .note] | @tsv' "$MANIFEST" 2>/dev/null)" || {
  echo "loader-parity: FATAL manifest is not valid JSON or has no non-empty .fixtures array: $MANIFEST" >&2
  exit 3
}
ROWS=()
while IFS= read -r line; do [ -n "$line" ] && ROWS+=("$line"); done <<< "$ROWS_RAW"
[ "${#ROWS[@]}" -ge 1 ] || { echo "loader-parity: FATAL zero fixtures in manifest (a zero-fixture run must never PASS)" >&2; exit 3; }

# #114 — the corpus's TRACK CONTRACT fixture, which declares the track's reference model. The
# Swift leg pins that identity from its own constants and ALWAYS applies it, so the Rust leg must
# be given the contract or the two loaders are being compared in different configurations and the
# `model_provenance` rows would read as divergences that are really just a missing argument.
# A manifest that declares one and does not ship it is FATAL, never silently skipped.
CONTRACT_FILE="$(jq -r '.reference_model_contract // empty' "$MANIFEST")"
CONTRACT_ARGS=()
if [ -n "$CONTRACT_FILE" ]; then
  [ -f "${CORPUS}/${CONTRACT_FILE}" ] || {
    echo "loader-parity: FATAL manifest declares reference_model_contract=${CONTRACT_FILE} but ${CORPUS}/${CONTRACT_FILE} is missing" >&2
    exit 3
  }
  CONTRACT_ARGS=(--contract "${CORPUS}/${CONTRACT_FILE}")
  echo "loader-parity: reference-model pin from ${CONTRACT_FILE} (#114)"
else
  echo "loader-parity: WARNING no reference_model_contract in the manifest — the Rust leg runs" >&2
  echo "  SHAPE-ONLY on model_provenance while the Swift leg still pins its values (#114)." >&2
fi

# (1c) `mlxfast-swift preflight` is weights/baseline-COUPLED, not loader-only: a REJECT can
# mean a broken Swift setup (weights/baseline) rather than a golden decision. Sanity-gate on
# a known-good canonical golden through the Swift side FIRST; if IT is rejected, every Swift
# decision below would be unattributable, so abort (exit 4) instead of emitting a misleading
# report. Override the known-good golden with SANITY_GOLDEN (default: the corpus valid.json).
SANITY_GOLDEN="${SANITY_GOLDEN:-${CORPUS}/valid.json}"
[ -f "$SANITY_GOLDEN" ] || { echo "loader-parity: FATAL sanity golden not found: $SANITY_GOLDEN" >&2; exit 4; }
if ! "$SWIFT" preflight --golden "$SANITY_GOLDEN" --weights "$WEIGHTS" >/dev/null 2>&1; then
  echo "loader-parity: FATAL the Swift leg REJECTED the known-good golden ($SANITY_GOLDEN)." >&2
  echo "  mlxfast-swift preflight is weights/baseline-coupled; this means the Swift setup" >&2
  echo "  (weights dir / baseline) is broken, not that the golden is invalid. Fix WEIGHTS" >&2
  echo "  (or SANITY_GOLDEN) — loader decisions would otherwise be unattributable. Aborting." >&2
  exit 4
fi
echo "loader-parity: Swift-leg sanity OK (known-good golden accepted by preflight)"

decide() { case "$1" in 0) echo ACCEPT ;; 1) echo REJECT ;; *) echo "HARNESS-ERR($1)" ;; esac; }

match=0; known=0; mismatch=0; harness=0; missing=0
{
  echo "# loader-parity: Rust (benchctl validate-golden) vs Swift (mlxfast-swift preflight)"
  echo "corpus: ${CORPUS}   fixtures: ${#ROWS[@]}"
  echo "swift:  ${SWIFT}"
  echo "rust:   ${BENCHCTL}"
  echo ""
  printf "%-28s %-9s %-9s %-11s %s\n" FIXTURE RUST SWIFT VERDICT NOTE
} > "$REPORT"

for row in "${ROWS[@]}"; do
  IFS=$'\t' read -r f diverges gates_only note <<<"$row"
  fx="${CORPUS}/${f}"
  # (ii) existence-check the fixture before invoking either loader.
  if [ ! -f "$fx" ]; then
    printf "%-28s %-9s %-9s %-11s %s\n" "$f" "-" "-" "NO-FIXTURE" "$note" >> "$REPORT"
    missing=$((missing + 1)); continue
  fi
  # A manifest-declared `gates_only:true` fixture legitimately lacks the oracle (#77) — validate
  # its STRUCTURE only. Default (empty/false): full validate-golden, oracle required. GO is a
  # bare flag (no spaces), so unquoted word-split is safe + bash-3.2/set-u clean.
  GO=""; [ "$gates_only" = "true" ] && GO="--gates-only"
  "$BENCHCTL" validate-golden --golden "$fx" "${CONTRACT_ARGS[@]+"${CONTRACT_ARGS[@]}"}" $GO >/dev/null 2>&1; rust="$(decide $?)"
  "$SWIFT" preflight --golden "$fx" --weights "$WEIGHTS" >/dev/null 2>&1; swift="$(decide $?)"
  # (iii) any non-0/1 exit is a HARNESS ERROR, not a REJECT.
  if [[ "$rust" == HARNESS-ERR* || "$swift" == HARNESS-ERR* ]]; then
    verdict=HARNESS-ERR; harness=$((harness + 1))
  elif [ "$rust" = "$swift" ]; then
    verdict=MATCH; match=$((match + 1))
  elif [ "$diverges" = "true" ]; then
    verdict=KNOWN-DIV; known=$((known + 1))
  else
    verdict=MISMATCH; mismatch=$((mismatch + 1))
  fi
  printf "%-28s %-9s %-9s %-11s %s\n" "$f" "$rust" "$swift" "$verdict" "$note" >> "$REPORT"
done

{
  echo ""
  echo "summary: match=${match} known-divergence=${known} MISMATCH=${mismatch} MISSING-FIXTURE=${missing} HARNESS-ERR=${harness}"
  if [ "$mismatch" -eq 0 ] && [ "$missing" -eq 0 ] && [ "$harness" -eq 0 ]; then
    echo "RESULT: PASS — decisions identical except manifest-declared intentional divergences"
  else
    echo "RESULT: FAIL — ${mismatch} undeclared divergence(s), ${missing} missing fixture(s), ${harness} harness error(s)"
  fi
} >> "$REPORT"
cat "$REPORT"
[ "$mismatch" -eq 0 ] && [ "$missing" -eq 0 ] && [ "$harness" -eq 0 ]
