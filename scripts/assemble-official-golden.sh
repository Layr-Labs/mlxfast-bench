#!/bin/bash
# scripts/assemble-official-golden.sh — deterministically assemble the BOX-CALIBRATED official
# golden from submit-1024.json (David's RULING 1).
#
# WHY: the B-3 window found submit-1024's baselines are STALE for this box. The box measured
# prefill ~0.001165 s/token and decode ~0.03668 s/token; against submit-1024's baselines
# (prefill 0.010605031949609375, decode 0.1336139485703125) those readings BUST the official
# acceptance band ("improvement too large for one submission") → BOTH sides FAIL. To compare
# PASSING-score parity we need a golden whose baselines make the box-measured speeds PASS the
# official gates, then compare parity on a passing run.
#
# WHAT: take submit-1024.json byte-for-byte and replace ONLY the two baseline float literals
# with box-calibrated values. Everything else (1024-step cases[0], anchors, free_run, the >=128
# decode-step benchmark oracle, gates) stays IDENTICAL → still loader-valid + official-shaped.
# The replacement is a targeted, assert-unique string substitution (the cases[] bytes are never
# reserialized), so the output is DETERMINISTIC and pinnable.
#
# CALIBRATED BASELINES + BAND MATH (official gates: 0.95 speedup floor; prefill band ±5%, i.e.
# [baseline*0.95, baseline*1.05]; decode band +2%/-5%, i.e. [baseline*0.95, baseline*1.02];
# check() value = measured, reference = baseline; speedup = baseline/measured):
#   baseline_prefill = 0.001182475  (= measured 0.001165 * 1.015)
#     floor : speedup = 0.001182475/0.001165 = 1.0150 >= 0.95            PASS
#     band  : [0.00112335125, 0.0012415988]; measured 0.001165 in-band   PASS  (+3.58% / +6.58% margin)
#   baseline_decode  = 0.0372302    (= measured 0.03668  * 1.015)
#     floor : speedup = 0.0372302/0.03668 = 1.0150 >= 0.95               PASS
#     band  : [0.03536869, 0.037974804]; measured 0.03668 in-band        PASS  (+3.58% / +3.53% margin)
# The +1.5% headroom centres the measured decode inside the TIGHTER (+2%/-5%) decode band so both
# sides retain ~3.5% run-to-run variance margin on each edge before touching a band edge.
#
# LABEL: the golden top level is CLOSED (`GoldenDocument` carries `#[serde(deny_unknown_fields)]`,
# golden.rs:127), so a `_provenance` key inside the golden would be REJECTED by both loaders. The
# parity-test-only provenance therefore lives OUTSIDE the golden bytes: a `.provenance.txt` sidecar
# and a `.manifest.json` (both emitted here), plus the matrix §8 note and the driver default. The
# golden itself is NEVER an organizer/ranking golden — box-calibrated baselines, parity-test only.
#
# Usage: assemble-official-golden.sh [SRC_submit-1024.json] [OUT_calibrated.json]
#   env: BASELINE_PREFILL / BASELINE_DECODE (defaults below); SRC_PIN_SHA / SRC_PIN_BYTES to
#        verify the source submit-1024 before transforming (recommended; skipped if unset).
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
G="${MLXFAST_PARITY_GIT:-$HOME/mlxfast-parity-git}"

SRC="${1:-${OFFICIAL_SRC_GOLDEN:-$G/golden/submit-1024.json}}"
OUT="${2:-${OFFICIAL_GOLDEN:-$G/golden/official-calibrated-1024.json}}"

# Box-calibrated baselines (see band math above). Fixed decimal literals (deterministic bytes).
BASELINE_PREFILL="${BASELINE_PREFILL:-0.001182475}"
BASELINE_DECODE="${BASELINE_DECODE:-0.0372302}"

# The stale submit-1024 literals we replace (must each appear EXACTLY once).
OLD_PREFILL_KV='"baseline_prefill_seconds_per_token":0.010605031949609375'
OLD_DECODE_KV='"baseline_decode_seconds_per_token":0.1336139485703125'
NEW_PREFILL_KV="\"baseline_prefill_seconds_per_token\":$BASELINE_PREFILL"
NEW_DECODE_KV="\"baseline_decode_seconds_per_token\":$BASELINE_DECODE"

[ -r "$SRC" ] || { echo "assemble-official-golden: source golden not readable: $SRC" >&2; exit 2; }
command -v shasum >/dev/null || { echo "assemble-official-golden: shasum required" >&2; exit 2; }

# Optional: verify the SOURCE submit-1024 against its pin BEFORE transforming (fail-closed).
if [ -n "${SRC_PIN_SHA:-}" ] && [ -n "${SRC_PIN_BYTES:-}" ]; then
  sbytes="$(wc -c < "$SRC" | tr -d ' ')"
  ssha="$(shasum -a 256 "$SRC" | awk '{print $1}')"
  if [ "$sbytes" != "$SRC_PIN_BYTES" ] || [ "$ssha" != "$SRC_PIN_SHA" ]; then
    echo "assemble-official-golden: SOURCE pin mismatch (bytes $sbytes/$SRC_PIN_BYTES sha $ssha/$SRC_PIN_SHA) — refusing" >&2
    exit 3
  fi
fi

# Deterministic transform: exact, assert-unique string replacement of the two baseline literals.
# python is used ONLY for the count-and-replace (no JSON reserialization → cases[] bytes untouched).
if ! python3 - "$SRC" "$OUT" "$OLD_PREFILL_KV" "$NEW_PREFILL_KV" "$OLD_DECODE_KV" "$NEW_DECODE_KV" <<'PY'
import sys
src, out, op, npf, od, ndf = sys.argv[1:7]
data = open(src, "r", encoding="utf-8").read()
for label, old in (("prefill", op), ("decode", od)):
    n = data.count(old)
    if n != 1:
        sys.stderr.write("assemble-official-golden: %s baseline literal found %d times (want exactly 1) in %s\n" % (label, n, src))
        sys.exit(4)
data = data.replace(op, npf).replace(od, ndf)
# Belt-and-suspenders: the calibrated literals must now be present exactly once each.
for label, new in (("prefill", npf), ("decode", ndf)):
    if data.count(new) != 1:
        sys.stderr.write("assemble-official-golden: calibrated %s literal not uniquely present after replace\n" % label)
        sys.exit(4)
open(out, "w", encoding="utf-8").write(data)
PY
then
  echo "assemble-official-golden: transform failed — see above" >&2; exit 4
fi

OUT_BYTES="$(wc -c < "$OUT" | tr -d ' ')"
OUT_SHA="$(shasum -a 256 "$OUT" | awk '{print $1}')"

# --- provenance sidecar (parity-test-only label; the golden top level is closed) --------
cat > "$OUT.provenance.txt" <<EOF
parity-test-only — box-calibrated baselines, NEVER organizer scoring/ranking.
Derived deterministically from submit-1024.json by scripts/assemble-official-golden.sh.
Only benchmark.baseline_prefill_seconds_per_token / baseline_decode_seconds_per_token
were changed (to make box-measured speeds PASS the official gates); every other byte is
identical to submit-1024.json (1024-step cases[0], 2 anchors, 1 free_run, 128-step oracle).
baseline_prefill_seconds_per_token = $BASELINE_PREFILL   (measured ~0.001165 s/tok * 1.015)
baseline_decode_seconds_per_token  = $BASELINE_DECODE     (measured ~0.03668  s/tok * 1.015)
pin: sha256 $OUT_SHA  bytes $OUT_BYTES
source submit-1024: ${SRC_PIN_SHA:-$(shasum -a 256 "$SRC" | awk '{print $1}')} / ${SRC_PIN_BYTES:-$(wc -c < "$SRC" | tr -d ' ')} bytes
EOF

# --- manifest (pin + provenance, machine-readable) --------------------------------------
if command -v jq >/dev/null 2>&1; then
  jq -n \
    --arg provenance "parity-test-only — box-calibrated baselines, NEVER organizer scoring/ranking" \
    --arg golden_path "$OUT" \
    --arg golden_sha256 "$OUT_SHA" \
    --argjson golden_bytes "$OUT_BYTES" \
    --arg source_path "$SRC" \
    --arg baseline_prefill "$BASELINE_PREFILL" \
    --arg baseline_decode "$BASELINE_DECODE" \
    '{provenance:$provenance, golden_path:$golden_path, golden_sha256:$golden_sha256,
      golden_bytes:$golden_bytes, source:$source_path,
      baseline_prefill_seconds_per_token:($baseline_prefill|tonumber),
      baseline_decode_seconds_per_token:($baseline_decode|tonumber)}' > "$OUT.manifest.json"
fi

echo "assemble-official-golden: wrote $OUT"
echo "  pin: sha256 $OUT_SHA  bytes $OUT_BYTES"
echo "  provenance: $OUT.provenance.txt (parity-test-only)"
[ -f "$OUT.manifest.json" ] && echo "  manifest:   $OUT.manifest.json"
# Emit the pin on stdout in an eval-able form so callers can capture it.
echo "OFFICIAL_CALIBRATED_PIN_SHA=$OUT_SHA"
echo "OFFICIAL_CALIBRATED_PIN_BYTES=$OUT_BYTES"
