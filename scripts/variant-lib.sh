#!/bin/bash
# scripts/variant-lib.sh — shared §12 variant-corpus primitives, sourced (never executed).
#
# ONE definition each of the offline-validatable pieces the §12 driver leg needs, so the
# generator (gen-variant-corpus.sh), the parity leg (variant-parity.sh), and the offline
# self-test (test-variant-offline.sh) all share the SAME logic instead of forking drifting
# copies (mirrors parity-lib.sh's intent):
#
#   variant_pin_check      — sha256 + byte-count integrity pin of a file (no benchctl needed)
#   variant_dual_loader    — dual-loader acceptance (benchctl validate-golden + swift preflight)
#   variant_render_verdict — the no-undeclared-cells rule: FAIL + manifest `declared` → DECLARED(#nn)
#   variant_manifest_rows  — manifest-anchored parse: emit `class<TAB>path<TAB>sha256<TAB>bytes<TAB>declared`
#
# Portable to bash 3.2 (stock macOS): no mapfile, no associative arrays, no ${var,,}.
# Every function fails LOUD (prints a self-identifying line + non-zero) — never a silent pass.

if [ -n "${_VARIANT_LIB_SOURCED:-}" ]; then return 0 2>/dev/null || true; fi
_VARIANT_LIB_SOURCED=1

# --- sha256 + byte-count integrity pin (offline; no benchctl) --------------------------
# variant_pin_check <file> <want_sha256> <want_bytes>. Recomputes both over the RAW bytes
# and compares. 0 = pin matches; 1 = mismatch (prints expected/actual); 2 = file missing.
# Byte count first (cheap), then sha256 — same fail-closed order as bench-core's
# verify_golden_integrity. shasum(1) is on stock macOS + the box; wc -c for the byte count.
variant_pin_check() {
  local f="$1" want_sha="$2" want_bytes="$3" got_bytes got_sha
  if [ ! -f "$f" ]; then echo "  pin FAIL: file missing: $f"; return 2; fi
  got_bytes="$(wc -c < "$f" | tr -d ' ')"
  if [ "$got_bytes" != "$want_bytes" ]; then
    echo "  pin FAIL: byte count mismatch for $f: expected $want_bytes, actual $got_bytes"; return 1
  fi
  got_sha="$(shasum -a 256 "$f" | awk '{print $1}')"
  if [ "$got_sha" != "$want_sha" ]; then
    echo "  pin FAIL: sha256 mismatch for $f: expected $want_sha, actual $got_sha"; return 1
  fi
  echo "  pin ok: $f ($want_sha, $want_bytes B)"
  return 0
}

# --- dual-loader acceptance (benchctl validate-golden + swift preflight) ----------------
# variant_dual_loader <benchctl> <swift> <weights> <golden> <sha256> <bytes> [gates_only] [contract]
# Runs BOTH loaders on the same golden and asserts BOTH ACCEPT (exit 0). Neither loader
# spawns the engine (validate-golden is engine-free; preflight is model-free but
# weights/baseline-coupled) → this is offline-validatable, NO GPU / NO qwen unload.
#   benchctl validate-golden: also enforces the integrity pin (sha256+bytes) when given.
#     A `minimal` golden (cases + benchmark oracle, NO correctness_gates) is still a FULL
#     golden here (it HAS the oracle), so NO --gates-only is needed; pass gates_only=1 only
#     for a deliberately oracle-less fixture.
#   swift preflight: weights/baseline-coupled — a REJECT can mean a broken Swift setup, so
#     the caller SHOULD sanity-gate a known-good golden first (loader-parity.sh pattern).
# Exit contract: only 0 (ACCEPT) / 1 (REJECT) are decisions; anything else is a HARNESS
# error, surfaced as such (never silently treated as accept). Returns 0 iff BOTH accept.
#   contract (optional, or the VARIANT_CONTRACT env): the TRACK CONTRACT fixture declaring the
#     track's reference model (#114). The Swift leg ALWAYS applies its constant-driven
#     reference-model pin, so a variant golden carrying a `model_provenance` block is compared in
#     two different configurations unless the Rust leg is handed the contract too. Variant goldens
#     carry no provenance block today — this parameter exists so that stops being load-bearing.
variant_dual_loader() {
  local bc="$1" swift="$2" weights="$3" golden="$4" sha="$5" bytes="$6" gates_only="${7:-0}"
  local contract="${8:-${VARIANT_CONTRACT:-}}"
  local go="" rrc srrc
  [ "$gates_only" = "1" ] && go="--gates-only"
  local pin=()
  if [ -n "$sha" ] && [ -n "$bytes" ]; then pin=(--golden-sha256 "$sha" --golden-bytes "$bytes"); fi
  local ct=()
  if [ -n "$contract" ]; then
    [ -r "$contract" ] || { echo "  dual-loader HARNESS-ERR: contract not readable: $contract"; return 3; }
    ct=(--contract "$contract")
  fi
  # shellcheck disable=SC2086
  "$bc" validate-golden --golden "$golden" ${pin[@]+"${pin[@]}"} ${ct[@]+"${ct[@]}"} $go >/dev/null 2>&1; rrc=$?
  "$swift" preflight --golden "$golden" --weights "$weights" >/dev/null 2>&1; srrc=$?
  case "$rrc" in
    0) : ;;
    1) echo "  dual-loader FAIL: benchctl validate-golden REJECTED $golden (exit 1)"; return 1 ;;
    *) echo "  dual-loader HARNESS-ERR: benchctl validate-golden exited $rrc (not a decision) on $golden"; return 3 ;;
  esac
  case "$srrc" in
    0) : ;;
    1) echo "  dual-loader FAIL: swift preflight REJECTED $golden (exit 1)"; return 1 ;;
    *) echo "  dual-loader HARNESS-ERR: swift preflight exited $srrc (not a decision) on $golden"; return 3 ;;
  esac
  echo "  dual-loader ok: benchctl validate-golden + swift preflight both ACCEPT $golden"
  return 0
}

# --- the no-undeclared-cells rule (shared with failure-map.sh / run-manual-test.sh) -----
# variant_render_verdict <raw_verdict> <declared>. A manifest-DECLARED variant renders
# DECLARED(<ref>) instead of FAIL — the divergence is signed/audited, stop acting on it.
# This ONLY rewrites a FAIL; PASS (parity holds) and TOOL-ERR (harness broken) pass through
# unchanged. FAIL survives ONLY for UNDECLARED variants (act on this). Prints the rendered cell.
variant_render_verdict() {
  local vd="$1" declared="$2"
  if [ "$vd" = "FAIL" ] && [ -n "$declared" ]; then printf 'DECLARED(%s)' "$declared"; return 0; fi
  printf '%s' "${vd:-（no verdict）}"
}

# --- manifest-anchored parse ------------------------------------------------------------
# variant_manifest_rows <manifest.json>. Emits ONE `class<TAB>path<TAB>sha256<TAB>bytes<TAB>declared`
# line per manifest variant (declared empty for ordinary variants). The CONSUMER iterates the
# manifest, so a variant can neither be dropped (a missing row aborts the consumer) nor
# fabricated (a stray file that is not a manifest variant can never appear). Fails LOUD if the
# manifest is unreadable / not JSON / has zero variants.
variant_manifest_rows() {
  local manifest="$1"
  python3 - "$manifest" <<'PY'
import json, sys
try:
    m = json.load(open(sys.argv[1]))
except Exception as e:
    sys.stderr.write(f"variant-lib: manifest unreadable/not JSON ({sys.argv[1]}): {e}\n"); sys.exit(3)
vs = m.get("variants")
if not isinstance(vs, list) or not vs:
    sys.stderr.write("variant-lib: manifest has no non-empty .variants array — refusing\n"); sys.exit(3)
for v in vs:
    for k in ("class", "path", "sha256", "bytes"):
        if k not in v:
            sys.stderr.write(f"variant-lib: manifest variant missing {k!r}: {v}\n"); sys.exit(3)
    print("\t".join([v["class"], v["path"], v["sha256"], str(v["bytes"]), (v.get("declared") or "")]))
PY
}
