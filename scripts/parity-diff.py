#!/usr/bin/env python3
# §T4 — parity-diff.py is now a ONE-RELEASE SHIM over `benchctl parity-diff`.
#
# The verdict logic moved to Rust (crates/benchctl/src/parity.rs): it shares the ACTUAL
# ScoreMetrics type, so the bucket roster is checked against the serde field names by a cargo
# test (a new unbucketed field fails the build, not a live window), and the mutate-every-field
# property test proves no silently-ignorable field. Same buckets / 1e-9 float rule / peak_ram
# tolerance / failing-pair MODE / PASS-FAIL verdict as this differ had.
#
# This shim keeps existing callers (run-parity.sh, failure-map.sh, run-manual-test.sh) working
# for one release, then is deleted. benchctl is located via $BENCHCTL, else the repo's
# target/release build, else PATH.
#
# EXIT CONTRACT (#66 review must-fix 4): the shim NEVER lets a tool problem masquerade as a
# parity verdict. It passes through ONLY benchctl's 0 (PASS) / 1 (FAIL) — the runs that print a
# `PARITY:` line. Anything else (binary missing/not executable, or benchctl exiting 2/3/other:
# usage, IO, crash) prints a self-identifying error and exits SHIM_TOOL_ERR (9) — distinct from
# 0/1 (verdict), from benchctl's own 2/3, and from run-parity.sh's SKIPPED=3, so a stale binary
# can't render as an empty `PARITY:` cell under `|| true`.
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SHIM_TOOL_ERR = 9  # tool problem, NOT a parity verdict (avoids 0/1 verdict + 2/3 + SKIPPED=3)


def find_benchctl():
    """Return (path, source_label). Path may not exist — the caller checks."""
    env = os.environ.get("BENCHCTL")
    if env:
        return env, "$BENCHCTL"
    local = os.path.join(HERE, "..", "target", "release", "benchctl")
    if os.path.exists(local):
        return local, "repo target/release"
    w = shutil.which("benchctl")
    if w:
        return w, "PATH"
    return local, "repo target/release (absent)"


def die_tool(msg):
    sys.stderr.write(f"parity-diff.py shim: {msg}\n")
    sys.exit(SHIM_TOOL_ERR)


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--selftest":
        # The differ's self-test is the Rust suite. Run it for real; if the toolchain is absent,
        # die with the exact command to run (never a vacuous exit 0 while docs advertise cases).
        if shutil.which("cargo") is None:
            die_tool("--selftest needs the Rust toolchain; run `cargo test -p benchctl parity`.")
        sys.exit(subprocess.call(["cargo", "test", "-p", "benchctl", "parity"], cwd=os.path.join(HERE, "..")))

    bc, src = find_benchctl()
    if not (os.path.isfile(bc) and os.access(bc, os.X_OK)):
        die_tool(
            f"benchctl not found/executable at {bc} (via {src}). Set $BENCHCTL or build it. "
            f"This is a TOOL error, not a PARITY verdict."
        )
    rc = subprocess.call([bc, "parity-diff", *sys.argv[1:]])
    if rc in (0, 1):
        sys.exit(rc)  # genuine PASS/FAIL — benchctl printed the PARITY: line
    die_tool(
        f"`{bc} parity-diff` exited {rc} (usage/IO/crash), not 0/1 — no PARITY verdict was "
        f"produced; refusing to pass it off as one."
    )
