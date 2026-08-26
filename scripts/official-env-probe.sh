#!/bin/bash
# scripts/official-env-probe.sh — B-3 Leg 4: #47 closing evidence (live env delivery).
#
# Proves the benchctl spawn delivers the SANITIZED, allowlist-only child environment to the live
# worker — the driver-evidence half of #47 (the code half is VERIFIED in B-1). B-1's env
# sanitization (`.env_clear()` + `sanitized_engine_env` allowlist FROM EMPTY + forced
# `MLXFAST_USE_RUNTIME_WORKER=0`) applies to ALL benchctl spawns, official AND local.
#
# WHY LOCAL-ITERATE (rework): the earlier revision spawned `--mode official`, whose Seatbelt profile
# correctly DENIES writes — so the env-dump shim could not record child-env.txt and the probe
# TOOL-ERR'd. The fix runs the probe against benchctl's `iterate --mode local-iterate` spawn: local
# mode applies the SAME env sanitization (that is what we are testing) but is NOT sandboxed
# deny-write, so the shim CAN write child-env.txt. The allowlist-only child env is thus proven on a
# writable spawn, with no Seatbelt relaxation hacks.
#
# ORTHOGONAL (separately verified, not this probe's job): the official sandbox's deny-write is a
# DIFFERENT surface and is itself verified — the shim's INABILITY to write child-env.txt under
# `--mode official` IS the deny-write sandbox working as intended. This probe isolates the env
# sanitization; the sandbox deny-write is confirmed by that expected write failure elsewhere.
#
# macOS DYLD_/SIP note: the PREFIX-allowlist path is asserted via a SURVIVING prefixed var —
# `LC_ALL` (LC_ prefix) — NOT `DYLD_*` (dyld may strip `DYLD_*` from children under SIP); the DYLD_
# marker is only reported, never asserted.
#
# GPU-FREE: this loads no model (the shim never handshakes), so it needs the REAL benchctl but NOT
# a GPU / qwen downtime. The driver runs it in-window for one-shot convenience; it is equally valid
# standalone. Fails LOUD: a MISSING capture is a TOOL-ERR abort, never a silent pass.
#
# Env: BENCHCTL WEIGHTS · OFFICIAL_GOLDEN · OUT   (optional OFFICIAL_COMMIT)
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/official-lib.sh"

: "${BENCHCTL:?set BENCHCTL}" "${WEIGHTS:?set WEIGHTS}" "${OFFICIAL_GOLDEN:?set OFFICIAL_GOLDEN}" "${OUT:?set OUT}"
mkdir -p "$OUT"
rm -f "$OUT/child-env.txt" "$OUT/official-env-probe.table.txt" 2>/dev/null || true

CHILD_ENV="$OUT/child-env.txt"
SHIM="$OUT/env-dump-worker.sh"
OUT_ABS="$(official_abs "$OUT")"
CHILD_ENV_ABS="$OUT_ABS/child-env.txt"
SHIM_ABS="$OUT_ABS/env-dump-worker.sh"

# --- env-dump worker shim ---------------------------------------------------------------
# Substituted as benchctl's `--engine`. Dumps its FULL received env (the sanitized child env) with
# NO exec (compgen/printf/${!n} are bash builtins) to a path baked in at generation time — the env
# cannot carry the path because MLXFAST_* names are stripped from the child. `compgen -e` lists the
# exported names, which for a spawned process are exactly its `environ`.
cat > "$SHIM" <<SHIM
#!/bin/bash
: > "$CHILD_ENV_ABS"
for n in \$(compgen -e); do printf '%s=%s\n' "\$n" "\${!n}" >> "$CHILD_ENV_ABS"; done
# Do NOT handshake — benchctl will error at the hello; that is expected and ignored. Under
# local-iterate the spawn is NOT sandboxed deny-write, so the write above SUCCEEDS.
exit 0
SHIM
chmod +x "$SHIM"

# --- sentinels planted in benchctl's PARENT env, one per allowlist bucket ---------------
#   LC_ALL                 : PREFIX-allowlisted (LC_) + surviving → MUST reach the child (PRIMARY)
#   TERM                   : EXACT-allowlisted                    → MUST reach the child
#   DYLD_PROBE_MARKER      : PREFIX-allowlisted (DYLD_) — REPORT only (SIP may strip; not asserted)
#   MLXFAST_PROBE_SECRET   : MLXFAST_ (harness) name              → MUST be ABSENT
#   PROBE_RANDOM_NAME      : non-allowlisted exact name           → MUST be ABSENT
#   MLXFAST_USE_RUNTIME_WORKER (forced 0 in child by the sanitizer)   → MUST be present == 0
LC_SENTINEL="en_US.UTF-8-probe"
TERM_SENTINEL="probe-term"
DYLD_SENTINEL="/probe/dyld/marker"

echo "official-env-probe: spawning benchctl LOCAL-ITERATE with an env-dump worker shim ($SHIM_ABS)"
# `--engine <shim>` becomes the spawned worker. LOCAL-iterate applies B-1's SAME env sanitization
# (the surface under test) but is NOT sandboxed deny-write, so the shim can record child-env.txt.
# No Seatbelt profile override — this spawn is unsandboxed by design (see the header's ORTHOGONAL
# note: the official deny-write sandbox is a separate, separately-verified surface).
env \
  LC_ALL="$LC_SENTINEL" \
  TERM="$TERM_SENTINEL" \
  DYLD_PROBE_MARKER="$DYLD_SENTINEL" \
  MLXFAST_PROBE_SECRET="topsecret-should-be-dropped" \
  PROBE_RANDOM_NAME="should-be-dropped" \
  MLXFAST_COMMIT_SHA="${OFFICIAL_COMMIT:-$(official_commit_sha40)}" \
  "$BENCHCTL" iterate \
    --engine "$SHIM_ABS" \
    --weights "$WEIGHTS" \
    --golden "$OFFICIAL_GOLDEN" \
    --mode local-iterate \
    --score-path "$OUT/probe-score.json" \
  > "$OUT/benchctl.stdout" 2> "$OUT/benchctl.stderr" || true   # benchctl exits non-zero at the hello — expected

# --- fail-loud: the capture MUST exist (a missing dump is TOOL-ERR, never a pass) -------
if [ ! -s "$CHILD_ENV" ]; then
  echo "official-env-probe: TOOL-ERR — the worker shim did not record a child env ($CHILD_ENV missing/empty)." >&2
  echo "  benchctl.stderr tail:" >&2; tail -5 "$OUT/benchctl.stderr" 2>/dev/null | sed 's/^/    /' >&2
  exit 4
fi

has() { grep -q "^$1=" "$CHILD_ENV"; }
val() { grep "^$1=" "$CHILD_ENV" | head -1 | sed "s/^$1=//"; }

FAIL=0
TABLE="$OUT/official-env-probe.table.txt"; : > "$TABLE"
row() { printf '%-28s | %-10s | %s\n' "$1" "$2" "$3" | tee -a "$TABLE"; }
row "var (bucket)" "verdict" "detail"
printf -- '-----------------------------|------------|--------------------\n' | tee -a "$TABLE"

# PRIMARY: prefix-allowlist path survives (LC_).
if has LC_ALL && [ "$(val LC_ALL)" = "$LC_SENTINEL" ]; then row "LC_ALL (prefix LC_)" "PASS" "present, value preserved"; else row "LC_ALL (prefix LC_)" "FAIL" "absent/altered — prefix allowlist path NOT delivered"; FAIL=$((FAIL+1)); fi
# EXACT allowlist passes through.
if has TERM && [ "$(val TERM)" = "$TERM_SENTINEL" ]; then row "TERM (exact)" "PASS" "present, value preserved"; else row "TERM (exact)" "FAIL" "absent/altered — exact allowlist path NOT delivered"; FAIL=$((FAIL+1)); fi
# MLXFAST_ harness names dropped (except the forced guard).
if has MLXFAST_PROBE_SECRET; then row "MLXFAST_PROBE_SECRET" "FAIL" "LEAKED — MLXFAST_* reached the child"; FAIL=$((FAIL+1)); else row "MLXFAST_PROBE_SECRET" "PASS" "absent (MLXFAST_* stripped)"; fi
# non-allowlisted exact dropped.
if has PROBE_RANDOM_NAME; then row "PROBE_RANDOM_NAME" "FAIL" "LEAKED — non-allowlisted name reached the child"; FAIL=$((FAIL+1)); else row "PROBE_RANDOM_NAME" "PASS" "absent (env_clear + allowlist)"; fi
# forced guard present == 0.
if has MLXFAST_USE_RUNTIME_WORKER && [ "$(val MLXFAST_USE_RUNTIME_WORKER)" = "0" ]; then row "MLXFAST_USE_RUNTIME_WORKER" "PASS" "forced to 0 (no recursive worker)"; else row "MLXFAST_USE_RUNTIME_WORKER" "FAIL" "not forced to 0 (got '$(val MLXFAST_USE_RUNTIME_WORKER)')"; FAIL=$((FAIL+1)); fi
# DYLD_ prefix — report only, never fail (LC_ covers the prefix-allowlist path).
if has DYLD_PROBE_MARKER; then row "DYLD_PROBE_MARKER (prefix)" "note" "present (local-iterate is unsandboxed, so no SIP strip) — LC_ is the asserted prefix var"; else row "DYLD_PROBE_MARKER (prefix)" "note" "absent — DYLD_ stripped by macOS SIP (not asserted; LC_ covers the prefix path)"; fi

echo ""
if [ "$FAIL" -eq 0 ]; then
  echo "official-env-probe: RESULT PASS — benchctl local-iterate spawn delivered an allowlist-only child env (LC_ prefix + exact survive; MLXFAST_*/non-allowlisted dropped; guard forced 0). Same B-1 sanitizer as the official spawn; official deny-write is orthogonal + separately verified. #47 §9 evidence."
  exit 0
else
  echo "official-env-probe: RESULT FAIL — $FAIL sanitization check(s) failed; see $CHILD_ENV" >&2
  exit 1
fi
