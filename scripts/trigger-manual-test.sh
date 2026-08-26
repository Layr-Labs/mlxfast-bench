#!/bin/bash
# scripts/trigger-manual-test.sh — the LAPTOP trigger for the manual parity driver (item 8).
#
# Syncs the box's mlxfast-bench checkout to a git ref (default: current HEAD), rebuilds
# benchctl, runs scripts/run-manual-test.sh on the box (GPU window: unload → battery →
# reload), and fetches REPORT.md locally. Human-triggered — you run this and read the report.
#
# Usage: scripts/trigger-manual-test.sh [git-ref] [ssh-host]
set -euo pipefail
REF="${1:-$(git rev-parse --abbrev-ref HEAD)}"
HOST="${2:-ai-server}"
LOCAL_OUT="${MLXFAST_MANUAL_OUT:-$(git rev-parse --show-toplevel)/.parity/manual-test}"
mkdir -p "$LOCAL_OUT"
DRIVER_OUT="$LOCAL_OUT/driver.out"

echo "=== syncing box ($HOST) mlxfast-bench to '$REF' + rebuilding benchctl ==="
# Capture the driver's stdout locally (pipefail here surfaces an ssh/driver failure through the
# tee) so we can parse its own 'REPORT written:' line for the scp path below.
#
# INJECTION-SAFE REF: ssh flattens its argv into ONE string that the remote login shell
# RE-PARSES, so a positional arg alone does NOT stop injection (`main; rm -rf ~` would run on the
# box). We base64-encode REF locally — base64's alphabet ([A-Za-z0-9+/=], no newlines from
# python) contains no shell metacharacters, so it survives the remote re-parse verbatim — and
# decode it on the box. python3 is already a hard dependency of this harness (portable decode).
b64() { python3 -c 'import sys,base64; sys.stdout.write(base64.b64encode(sys.stdin.buffer.read()).decode())'; }
REF_B64="$(printf '%s' "$REF" | b64)"
ssh "$HOST" bash -s -- "$REF_B64" <<'REMOTE' 2>&1 | tee "$DRIVER_OUT"
set -eo pipefail
REF="$(printf '%s' "$1" | python3 -c 'import sys,base64; sys.stdout.write(base64.b64decode(sys.stdin.buffer.read()).decode())')"
export PATH="$HOME/.cargo/bin:$PATH"
cd ~/mlxfast-parity-git/mlxfast-bench
git fetch origin --quiet
# checkout MUST succeed (set -e); only the pull may fail (a detached ref has no upstream).
git checkout -q "$REF"
git pull --ff-only --quiet 2>/dev/null || true
git log --oneline -1
# pipefail (set above) makes a cargo build failure propagate through the '| tail' pipe.
cargo build --release -p benchctl 2>&1 | tail -1
echo '=== running run-manual-test.sh ==='
bash scripts/run-manual-test.sh
REMOTE

# Parse the driver's OWN 'REPORT written: <path>' line (honours an overridden OUT) instead
# of hardcoding a box path that drifts if OUT changes.
REMOTE_REPORT="$(grep -E '=== REPORT written: .* ===' "$DRIVER_OUT" | tail -1 | sed -E 's/^=== REPORT written: (.*) ===$/\1/')"
if [ -z "$REMOTE_REPORT" ]; then
  echo "ERROR: driver did not emit a 'REPORT written:' line — no REPORT to fetch (see $DRIVER_OUT)." >&2
  exit 1
fi

echo "=== fetching REPORT.md ($REMOTE_REPORT) ==="
# Single-quote the remote path so a box path containing spaces reaches scp's remote shell as
# ONE argument (scp subjects the remote path to remote-shell word-splitting otherwise).
scp "$HOST:'$REMOTE_REPORT'" "$LOCAL_OUT/REPORT.md"
echo "REPORT: $LOCAL_OUT/REPORT.md"
cat "$LOCAL_OUT/REPORT.md"
