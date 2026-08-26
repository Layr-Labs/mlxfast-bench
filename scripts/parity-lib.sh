#!/bin/bash
# scripts/parity-lib.sh — shared parity harness primitives, sourced (never executed).
#
# ONE definition each of: the box-quiet precheck, the lifetime-held GPU lock, and the
# golden pin/load gate. Both run-parity.sh and run-manual-test.sh source this file, but they
# use different subsets — accurately (Fable checkpoint Finding 5a): the DRIVER
# (run-manual-test.sh) composes ALL THREE — parity_precheck + parity_take_gpu_lock (held) +
# parity_validate_golden. run-parity.sh (the older dev harness) uses only parity_precheck; it
# has no GPU lock and its own benchmark-oracle golden check, not the pin gate. The shared
# definition means the ONE thing both use — the precheck — cannot drift between them.
#
# Portable to bash 3.2 (stock macOS) and to a box without flock(1): no mapfile, no
# associative arrays, no ${var,,}. GPU-lock fd is 9 (opened in the CALLER's shell so it
# is held for the caller's lifetime) — callers must invoke parity_take_gpu_lock in the
# current shell (via `||`/`if`), never in a $()/pipe/() subshell.

# Guard against double-source (functions are idempotent, but this keeps intent explicit).
if [ -n "${_PARITY_LIB_SOURCED:-}" ]; then return 0 2>/dev/null || true; fi
_PARITY_LIB_SOURCED=1

# --- box-quiet precheck: Time Machine idle, low load, no stray model process ----------
# Returns 0 if the box is quiet enough to measure, 1 otherwise. Prints ok/FAIL lines.
# tmutil is macOS-only; on a box without it the grep simply finds nothing (treated as
# "no backup"), which is correct. Stray-proc match is by exact process NAME (pgrep -x),
# never a `-f` command-line substring that would false-positive on scripts naming them.
parity_precheck() {
  local ok=1 l1 pct
  printf '\n== box-quiet precheck ==\n'
  if tmutil status 2>/dev/null | grep -q "Running = 1"; then
    pct=$(tmutil status 2>/dev/null | awk -F'"' '/Percent/{print $2; exit}')
    echo "  FAIL: Time Machine backup in flight (Percent=$pct). Wait for it to finish."; ok=0
  else
    echo "  ok: no Time Machine backup running"
  fi
  l1=$(uptime | sed -E 's/.*load average[s]?:[[:space:]]*([0-9.]+).*/\1/')
  if awk "BEGIN{exit !($l1 < 2.0)}"; then echo "  ok: 1-min load $l1 < 2.0"; else echo "  FAIL: 1-min load $l1 >= 2.0"; ok=0; fi
  if pgrep -x mlxfast-swift >/dev/null || pgrep -x mlxfast-engine >/dev/null; then
    echo "  FAIL: a model process (mlxfast-swift/mlxfast-engine) is already running"; ok=0
  else
    echo "  ok: no model process running"
  fi
  if command -v macmon >/dev/null 2>&1; then
    echo "  note: macmon present — check GPU util manually if desired"
  else
    echo "  note: macmon absent — GPU util not checked (load + no-model-proc proxy)"
  fi
  [ "$ok" -eq 1 ]
}

# --- lifetime-held GPU lock -----------------------------------------------------------
# parity_take_gpu_lock <lockpath>. Opens fd 9 on the lock IN THE CALLER'S SHELL and takes
# a non-blocking exclusive lock that is HELD until the caller exits (fd 9 stays open) —
# fixing the earlier bug where a short-lived `python3 -c fcntl.flock(...)` released the
# lock the instant it exited. Distinguishes open-failure from held so a foreign-owned
# lock file is NOT misreported as "held":
#   0  = lock acquired (held for the caller's lifetime)
#   10 = cannot OPEN the lock file (permission / foreign owner) — not a false "held"
#   11 = lock is HELD by another live process
parity_take_gpu_lock() {
  local lock="$1"
  if ! exec 9>"$lock" 2>/dev/null; then
    echo "  FAIL: cannot open GPU lock $lock (permission / foreign owner?) — refusing to assert a false 'held'"
    return 10
  fi
  if command -v flock >/dev/null 2>&1; then
    if flock -n 9; then return 0; fi
    echo "  FAIL: GPU lock $lock is held by another process"; return 11
  fi
  # No flock(1) (stock macOS): lock the INHERITED fd 9 via python fcntl. The lock lives on
  # the open file description shared with the caller's fd 9, so python exiting does NOT
  # release it — only closing fd 9 (caller exit) does. LOCK_NB so a live holder => held.
  if python3 - <<'PY'
import fcntl, sys
try:
    fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    sys.exit(1)
except OSError as e:
    sys.stderr.write("flock(9) error: %s\n" % e); sys.exit(2)
PY
  then return 0; fi
  echo "  FAIL: GPU lock $lock is held by another process"; return 11
}

# --- golden pin/load gate: delegate to `benchctl validate-golden` ---------------------
# parity_validate_golden <benchctl> <golden> <sha256> <bytes>. Delegates the integrity pin
# AND the schema/load check to the one authority (benchctl validate-golden), instead of a
# hand-rolled shasum+wc that only checks bytes and never load-validates. Maps its exit
# contract (0 accepted / 1 rejected / 2 usage / 3 IO) to one ok/FAIL line and re-returns.
parity_validate_golden() {
  local bc="$1" golden="$2" sha="$3" bytes="$4" rc out
  out="$("$bc" validate-golden --golden "$golden" --golden-sha256 "$sha" --golden-bytes "$bytes" 2>&1)"
  rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "  ok: golden pin+load accepted by benchctl validate-golden ($sha, $bytes B)"
  else
    echo "  FAIL: benchctl validate-golden rejected $golden (exit $rc):"
    echo "$out" | sed 's/^/    /'
  fi
  return "$rc"
}
