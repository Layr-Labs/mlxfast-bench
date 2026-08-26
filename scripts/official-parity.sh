#!/bin/bash
# scripts/official-parity.sh — B-3 Leg 1 (official parity) + Leg 3 (artifact byte-rows).
#
# Runs N (default 3) PAIRS of {`benchctl iterate --mode official`  vs  DIRECT `mlxfast-swift
# benchmark` in official env} on the SAME pinned clean golden, and per pair compares:
#   Leg 1 — the pre-judge SEALED score + deterministic fields via `benchctl parity-diff`
#           (TIMING waived by the differ's bucket policy → within-band; JUDGE-LESS: the
#           semantic_gpqa_*/gpqa_ttft_* fields are 0/"" on both sides and match trivially).
#   Leg 3 — the artifact byte surface: score naming (score.json both), <score>.sha256 sidecar
#           (present + true hash of its own score), 9-field benchmark-integrity.json (key-set +
#           deterministic VALUES; score_sha256 + transform_source_sha256 EXCEPTED as in facade-leg
#           since the timing-bearing payload + the Swift-fresh vs marker source hash differ by
#           design), and EXACT exit codes.
#
# The swift side is the TRUSTED BINARY invoked DIRECTLY (official is ENV-driven; no --official
# flag), with the STDOUT-sealed score as the pre-judge unit — NOT the protected benchmark.sh
# workflow. Both sides run their worker under a Seatbelt profile built by the SAME builder
# (official_seatbelt_profile == bench_runner build_seatbelt_profile), each pinned to that side's
# worker executable. GPU: BOTH sides spawn a real sandboxed worker → run inside a GPU window
# (caller unloads/reloads qwen). Fails LOUD, never silently passes.
#
# Env: BENCHCTL ENGINE SWIFT WEIGHTS · OFFICIAL_GOLDEN · OFFICIAL_COMMIT(40hex) · OUT · DIFF_CMD
#      optional: PAIRS(default 3) · OFFICIAL_PIN_SHA/OFFICIAL_PIN_BYTES(clean-golden pin) ·
#      SWIFT_REPO_ROOT · OFFICIAL_SWIFT_EXTRA_ENV/OFFICIAL_BC_EXTRA_ENV
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
. "$HERE/official-lib.sh"

: "${BENCHCTL:?set BENCHCTL}" "${ENGINE:?set ENGINE}" "${SWIFT:?set SWIFT}" "${WEIGHTS:?set WEIGHTS}"
: "${OFFICIAL_GOLDEN:?set OFFICIAL_GOLDEN}" "${OUT:?set OUT}"
DIFF_CMD="${DIFF_CMD:-$BENCHCTL parity-diff}"
PAIRS="${PAIRS:-3}"
export OFFICIAL_GOLDEN WEIGHTS SWIFT BENCHCTL ENGINE
export OFFICIAL_COMMIT="$(official_commit_sha40)"
command -v jq >/dev/null || { echo "official-parity: FATAL jq required" >&2; exit 2; }

mkdir -p "$OUT"
# Anti-stale: wipe prior per-run artifacts so any file at report time was written THIS run.
rm -rf "$OUT"/pair.* 2>/dev/null || true
rm -f "$OUT"/official-parity.table.txt 2>/dev/null || true

TABLE="$OUT/official-parity.table.txt"; : > "$TABLE"
FAIL=0

printf '%-6s | %-9s | %-9s | %-12s | %-10s | %-8s | %-10s | %-13s | %s\n' \
  "pair" "bc.pass" "sw.pass" "det-fields" "score-name" ".sha256" "integrity" "exit" "overall" | tee -a "$TABLE"
printf -- '-------|-----------|-----------|--------------|------------|----------|------------|---------------|--------\n' | tee -a "$TABLE"

sname="score.json"; iname="benchmark-integrity.json"

i=1
while [ "$i" -le "$PAIRS" ]; do
  bcd="$OUT/pair.$i/bc"; swd="$OUT/pair.$i/sw"
  mkdir -p "$bcd" "$swd"

  # --- run both sides (official semantics) ---
  official_benchctl_run "$bcd"
  bc_rc="$OFFICIAL_BC_RC"
  official_swift_run "$swd"
  sw_rc="$OFFICIAL_SWIFT_RC"; seal_rc="$OFFICIAL_SWIFT_SEAL_RC"

  bcp="$(official_passed_of "$bcd/$sname")"; swp="$(official_passed_of "$swd/$sname")"

  # --- Leg 1: deterministic + pre-judge score parity (TIMING waived; JUDGE-LESS) ---
  if [ -s "$bcd/$sname" ] && [ -s "$swd/$sname" ]; then
    $DIFF_CMD "$bcd/$sname" "$swd/$sname" > "$OUT/pair.$i/det.txt" 2>&1; drc=$?
    c_det="$(official_diff_cell "$drc" "$OUT/pair.$i/det.txt")"
  else
    c_det="TOOL-ERR(missing score)"
  fi
  case "$c_det" in PASS) : ;; *) FAIL=$((FAIL+1)) ;; esac

  # --- Leg 3a: score naming — both wrote score.json ---
  if [ -f "$bcd/$sname" ] && [ -f "$swd/$sname" ]; then c_name="ok"; else c_name="FAIL"; FAIL=$((FAIL+1)); fi

  # --- Leg 3b: <score>.sha256 sidecar present AND == true sha256 of its own score ---
  c_sha="ok"
  for side in "$bcd" "$swd"; do
    if [ ! -f "$side/$sname.sha256" ]; then c_sha="FAIL"; break; fi
    want="$(awk '{print $1}' "$side/$sname.sha256" 2>/dev/null)"
    [ -f "$side/$sname" ] && got="$(official_sha_of "$side/$sname")" || got="MISSING"
    if [ "$want" != "$got" ]; then c_sha="FAIL"; break; fi
  done
  [ "$c_sha" = "ok" ] || { FAIL=$((FAIL+1)); echo "official-parity[pair $i]: .sha256 sidecar missing or not matching" >&2; }

  # --- Leg 3c: 9-field integrity JSON — same key SET + deterministic VALUES ---
  # EXCEPTED: score_sha256 (hashes the timing-bearing payload) + transform_source_sha256 (Swift
  # fresh source_hash vs marker/"" — §3 caveat, facade-leg). Others must byte-match: same golden,
  # golden_path "[private]" both. The weights IDENTITY (weights_sha256/file_count/byte_count) stays a
  # STRICT full-value compare — that is the security-bearing pin. The provenance PATHS (score_path,
  # weights_path) are RULING-C compared: (a) a STRUCTURAL-RELATIVE guard — each side must have sealed
  # a workspace-RELATIVE path (a leading `/` is a home-leaking / raw-path regression on either impl,
  # bc-Rust or sw-shell, and FAILS), then (b) a BASENAME compare (the dirs differ legitimately). The
  # two relativisers need not be byte-identical — the structural guard makes basename-equality
  # divergence-proof without pinning a fragile Rust↔shell string equivalence.
  if [ -f "$bcd/$iname" ] && [ -f "$swd/$iname" ]; then
    bk="$(jq -S 'keys' "$bcd/$iname" 2>/dev/null)"; sk="$(jq -S 'keys' "$swd/$iname" 2>/dev/null)"
    # #123: SUPERSET, not equality — see INTEGRITY_RUNNER_KEYS at the top of this file.
    surplus="$(jq -S -n --argjson b "${bk:-[]}" --argjson s "${sk:-[]}" '$b - $s' -c 2>/dev/null)"
    missing="$(jq -S -n --argjson b "${bk:-[]}" --argjson s "${sk:-[]}" '$s - $b' -c 2>/dev/null)"
    c_int="ok"
    if [ -z "$bk" ] || [ "$missing" != "[]" ]; then c_int="FAIL"; echo "official-parity[pair $i]: integrity is MISSING reference keys $missing" >&2; fi
    if [ "$surplus" != "$INTEGRITY_RUNNER_KEYS" ]; then c_int="FAIL"; echo "official-parity[pair $i]: integrity surplus keys are not EXACTLY the declared #123 runner roster (got=$surplus want=$INTEGRITY_RUNNER_KEYS)" >&2; fi
    for fld in score_path weights_path weights_sha256 weights_file_count weights_byte_count golden_sha256 golden_path; do
      bv="$(jq -r --arg k "$fld" '.[$k] // "ABSENT"' "$bcd/$iname" 2>/dev/null)"
      sv="$(jq -r --arg k "$fld" '.[$k] // "ABSENT"' "$swd/$iname" 2>/dev/null)"
      # RULING C — the provenance PATHS: structural-relative guard, then basename compare. An
      # ABSOLUTE value on either side is a raw/home-leak regression and fails the leg by itself.
      if [ "$fld" = "score_path" ] || [ "$fld" = "weights_path" ]; then
        case "$bv" in /*) c_int="FAIL"; echo "official-parity[pair $i]: integrity.$fld is ABSOLUTE on bc ($bv) — must be workspace-relative (raw-path / home-leak regression)" >&2 ;; esac
        case "$sv" in /*) c_int="FAIL"; echo "official-parity[pair $i]: integrity.$fld is ABSOLUTE on sw ($sv) — must be workspace-relative (raw-path / home-leak regression)" >&2 ;; esac
        bv="$(basename "$bv")"; sv="$(basename "$sv")"
      fi
      if [ "$bv" != "$sv" ]; then c_int="FAIL"; echo "official-parity[pair $i]: integrity.$fld differs (bc=$bv sw=$sv)" >&2; fi
    done
    bts="$(jq -r '.transform_source_sha256 // "ABSENT"' "$bcd/$iname" 2>/dev/null)"
    sts="$(jq -r '.transform_source_sha256 // "ABSENT"' "$swd/$iname" 2>/dev/null)"
    [ "$bts" = "$sts" ] || echo "official-parity[pair $i]: integrity.transform_source_sha256 differs (bc=$bts sw=$sts) — DECLARED §3 caveat, excepted" >&2
    [ "$c_int" = "ok" ] || FAIL=$((FAIL+1))
  else
    c_int="FAIL"; FAIL=$((FAIL+1))
    echo "official-parity[pair $i]: integrity JSON missing (bc:$([ -f "$bcd/$iname" ] && echo y || echo n) sw:$([ -f "$swd/$iname" ] && echo y || echo n))" >&2
  fi

  # --- Leg 3d: exact exit codes (a missing sealed swift payload is itself a TOOL-ERR) ---
  if [ "$seal_rc" -ne 0 ]; then
    c_exit="FAIL(sw seal $seal_rc)"; FAIL=$((FAIL+1))
  elif [ "$bc_rc" = "$sw_rc" ]; then c_exit="ok($bc_rc)"; else c_exit="FAIL(b=$bc_rc/s=$sw_rc)"; FAIL=$((FAIL+1)); fi

  if [ "$c_det" = "PASS" ] && [ "$c_name" = "ok" ] && [ "$c_sha" = "ok" ] && [ "$c_int" = "ok" ] && [ "${c_exit#ok}" != "$c_exit" ]; then
    overall="GREEN"
  else
    overall="FAIL"
  fi
  printf '%-6s | %-9s | %-9s | %-12s | %-10s | %-8s | %-10s | %-13s | %s\n' \
    "$i" "$bcp" "$swp" "$c_det" "$c_name" "$c_sha" "$c_int" "$c_exit" "$overall" | tee -a "$TABLE"
  i=$((i+1))
done

echo ""
if [ "$FAIL" -eq 0 ]; then
  echo "official-parity: RESULT PASS — $PAIRS/$PAIRS pairs GREEN (pre-judge score + det-fields parity, byte-green artifacts, exit codes; timing + score_sha256 + transform_source_sha256 excepted)"
  exit 0
else
  echo "official-parity: RESULT FAIL — $FAIL check(s) failed across $PAIRS pairs; see $OUT/pair.*/ for per-side artifacts" >&2
  exit 1
fi
