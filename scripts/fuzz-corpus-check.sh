#!/usr/bin/env bash
# M-4 fuzz-corpus checker. Two jobs, both fail LOUD (never silently PASS):
#
#   (1) FREEZE + PIN: every fixture's sha256 + byte count must match manifest.json (the corpus
#       is frozen; regenerating with the same seed yields identical bytes). A drifted or missing
#       fixture is a hard failure.
#   (2) BENCHCTL-SIDE loader verdicts: run `benchctl validate-golden` on every fixture and assert
#       its accept/reject matches the manifest's expected_rust. This is the LOCAL (Rust) half of
#       the dual-loader parity — the Swift half needs the box (see below).
#
# When SWIFT and WEIGHTS are ALSO set, this delegates the full dual-loader run to the shared
# hardened harness scripts/loader-parity.sh (benchctl validate-golden vs mlxfast-swift preflight,
# agree-or-declared), the SAME harness the §6 loader-parity corpus uses. On the box:
#
#   BENCHCTL=target/release/benchctl SWIFT=/path/to/mlxfast-swift WEIGHTS=/path/to/weights \
#     scripts/fuzz-corpus-check.sh
#
# Exit: 0 = clean, 2 = usage/setup, 1 = freeze drift / verdict mismatch / dual-loader mismatch.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORPUS="${CORPUS:-$ROOT/crates/bench-core/tests/fixtures/golden_fuzz}"
MANIFEST="$CORPUS/manifest.json"
BENCHCTL="${BENCHCTL:-$ROOT/target/debug/benchctl}"
REPORT="${REPORT:-$ROOT/docs/fuzz-corpus-report.txt}"

# #114 (F5) — the TRACK CONTRACT fixture whose `target` declares the reference model. The Swift leg
# ALWAYS applies its constant-driven reference-model pin, so the Rust leg must be handed a contract
# or the two are compared in different configurations. The fuzz corpus ships no contract of its
# own; set CONTRACT to supply one. Safe to leave unset TODAY because no fuzz fixture carries a
# WELL-FORMED `model_provenance` (a malformed one rejects on shape in both loaders, pin or no pin)
# — and that is not left to prose: `loader_fuzz.rs` FAILS if such a fixture is ever added.
CONTRACT="${CONTRACT:-}"
CONTRACT_ARGS=()
if [ -n "$CONTRACT" ]; then
  [ -r "$CONTRACT" ] || { echo "fuzz-check: FATAL CONTRACT not readable: $CONTRACT" >&2; exit 2; }
  CONTRACT_ARGS=(--contract "$CONTRACT")
fi

command -v jq >/dev/null || { echo "fuzz-check: FATAL jq required" >&2; exit 2; }
[ -r "$MANIFEST" ] || { echo "fuzz-check: FATAL manifest missing/unreadable: $MANIFEST" >&2; exit 2; }
[ -x "$BENCHCTL" ] || { echo "fuzz-check: FATAL benchctl not executable: $BENCHCTL (cargo build -p benchctl)" >&2; exit 2; }

# sha256 of a file, portably (macOS shasum / linux sha256sum).
sha256() {
  if command -v shasum >/dev/null; then shasum -a 256 "$1" | awk '{print $1}';
  else sha256sum "$1" | awk '{print $1}'; fi
}
bytecount() { wc -c < "$1" | tr -d ' '; }
decide() { case "$1" in 0) echo ACCEPT ;; 1) echo REJECT ;; *) echo "HARNESS-ERR($1)" ;; esac; }

ROWS_RAW="$(jq -er '.fixtures[] | [.file, .expected_rust, (.swift_diverges|tostring), .sha256, (.bytes|tostring)] | @tsv' "$MANIFEST" 2>/dev/null)" || {
  echo "fuzz-check: FATAL manifest is not valid JSON or has no .fixtures array" >&2; exit 2; }
ROWS=(); while IFS= read -r line; do [ -n "$line" ] && ROWS+=("$line"); done <<< "$ROWS_RAW"
COUNT="${#ROWS[@]}"
[ "$COUNT" -ge 100 ] || { echo "fuzz-check: FATAL corpus has $COUNT fixtures; M-4 requires N>=100" >&2; exit 1; }

freeze_bad=0 verdict_bad=0 accept=0 reject=0 div=0
{
  echo "# M-4 fuzz-corpus check — FREEZE pin + benchctl (Rust) loader verdicts"
  echo "corpus:   ${CORPUS#$ROOT/}"
  echo "manifest: ${MANIFEST#$ROOT/}"
  echo "benchctl: ${BENCHCTL#$ROOT/}"
  echo "fixtures: $COUNT"
  echo ""
  printf "%-40s %-8s %-8s %-9s %s\n" FIXTURE EXPECT BENCHCTL FREEZE SWIFT
} > "$REPORT"

for row in "${ROWS[@]}"; do
  IFS=$'\t' read -r f exp diverges want_sha want_bytes <<<"$row"
  fx="$CORPUS/$f"
  if [ ! -f "$fx" ]; then
    printf "%-40s %-8s %-8s %-9s %s\n" "$f" "$exp" "-" "NO-FIXTURE" "-" >> "$REPORT"
    freeze_bad=$((freeze_bad+1)); continue
  fi
  got_sha="$(sha256 "$fx")"; got_bytes="$(bytecount "$fx")"
  if [ "$got_sha" != "$want_sha" ] || [ "$got_bytes" != "$want_bytes" ]; then
    frz=DRIFT; freeze_bad=$((freeze_bad+1))
  else
    frz=OK
  fi
  # shellcheck disable=SC2086
  "$BENCHCTL" validate-golden --golden "$fx" ${CONTRACT_ARGS[@]+"${CONTRACT_ARGS[@]}"} >/dev/null 2>&1; act="$(decide $?)"
  if [ "$act" != "$exp" ]; then verdict_bad=$((verdict_bad+1)); fi
  [ "$exp" = ACCEPT ] && accept=$((accept+1)); [ "$exp" = REJECT ] && reject=$((reject+1))
  [ "$diverges" = true ] && div=$((div+1))
  sw="agree"; [ "$diverges" = true ] && sw="DECLARED-DIV"
  printf "%-40s %-8s %-8s %-9s %s\n" "$f" "$exp" "$act" "$frz" "$sw" >> "$REPORT"
done

{
  echo ""
  echo "summary: fixtures=$COUNT accept=$accept reject=$reject predicted-swift-divergences=$div"
  echo "         freeze-drift=$freeze_bad  benchctl-verdict-mismatch=$verdict_bad"
  if [ "$freeze_bad" -eq 0 ] && [ "$verdict_bad" -eq 0 ]; then
    echo "RESULT: PASS — corpus frozen (all sha256+bytes pinned) and benchctl verdicts stable"
  else
    echo "RESULT: FAIL — $freeze_bad freeze drift(s), $verdict_bad verdict mismatch(es)"
  fi
} >> "$REPORT"
cat "$REPORT"

status=0
[ "$freeze_bad" -eq 0 ] && [ "$verdict_bad" -eq 0 ] || status=1

# Dual-loader leg: only when BOTH binaries are available (the box). Delegates to the shared
# hardened harness so the Swift verdicts are compared agree-or-declared against benchctl.
if [ -n "${SWIFT:-}" ] && [ -n "${WEIGHTS:-}" ]; then
  echo ""
  echo "fuzz-check: SWIFT + WEIGHTS set -> running full dual-loader parity via loader-parity.sh"
  BENCHCTL="$BENCHCTL" SWIFT="$SWIFT" WEIGHTS="$WEIGHTS" \
    "$ROOT/scripts/loader-parity.sh" "$CORPUS" "${DUAL_REPORT:-$ROOT/docs/fuzz-dual-loader-report.txt}" || status=1
else
  echo ""
  echo "fuzz-check: Swift leg SKIPPED (SWIFT/WEIGHTS unset). Rust side VERIFIED locally."
  echo "  Full dual-loader run on the box (both binaries present):"
  echo "    BENCHCTL=target/release/benchctl SWIFT=<mlxfast-swift> WEIGHTS=<weights-dir> \\"
  echo "      scripts/loader-parity.sh $CORPUS docs/fuzz-dual-loader-report.txt"
fi
exit "$status"
