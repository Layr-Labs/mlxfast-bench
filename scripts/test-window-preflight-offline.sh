#!/bin/bash
# scripts/test-window-preflight-offline.sh — OFFLINE self-test for the window-preflight gate.
#
# Proves the gate's ASSERTION LOGIC without a box and without a GPU, by building a fake "box"
# directory tree and driving the gate against it with DRIVER=local. What is exercised:
#
#   1. The shell port of benchctl's `dir_digest` against the EXACT vector the Rust unit test
#      hand-computes (iterate.rs:2228-2265) — the highest-risk piece in the whole gate, because
#      a weights digest that disagrees with benchd's would either clear a tree the run rejects
#      or, far worse, reject a tree the run would have accepted.
#   2. All-match → PASS, exit 0, and a well-formed attestation.
#   3. Every single-pin mismatch → the RIGHT exit code and a NAMED diagnostic. One pin is
#      broken at a time, so a test that goes green cannot be riding on some other failure.
#   4. Dirty tree, absent repo, absent binary, non-executable binary, identity mismatch.
#   5. The bundle rule: unpinned bundle REFUSED (exit 7), pinned bundle accepted AND its hash
#      recorded into the attestation.
#   6. The basics checklist: lock held, lock stale, disk floor, box quiet, serving state.
#   7. The SMOKE LEG against fake workers driven by the REAL benchctl over the REAL transport:
#      healthy / dies-at-hello / garbage-hello / hangs. These are the #134 verdicts.
#   8. window-provision: refuses to switch a checkout it does not own, refuses an unpinned
#      bundle, accepts a pinned one, and deletes nothing.
#
# RUN ONE AT A TIME. This suite asserts over machine-GLOBAL namespaces — `pgrep` for stray
# engine and serving-model processes, and box-side temp paths — so a second copy running
# concurrently is visible to the first and both report each other's fixtures as failures. It is
# not safe to parallelise against itself.
#
# PREREQUISITE — BUILD benchctl FIRST:
#
#     cargo build --release -p benchctl
#
# Sections 7 and 11 drive the REAL `benchctl` against scripted workers over the REAL transport;
# that is the only thing in this file that proves the spawn seam. Without the binary those
# checks SKIP, and a SKIP FAILS this suite by design (a skipped check is an untested claim, not
# a pass) — so an unbuilt tree reports failure rather than a hollow green.
#
# (Historical note, for anyone reading OLD logs: before this enforcement the suite reported
# shapes like "142 passed, 0 failed, 2 skipped" and still exited 0 — those two skips were exactly
# these real-transport sections standing down quietly on an unbuilt tree. A green run with skips
# in it is not a posture this suite can produce any more.)
#
# The REAL box steps (ssh, real weights, real engine, real GPU) are NOT run here — that is the
# window's job, and the gate is exercised for real in the Proof A retry. Exit 0 = all green.
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -P "$HERE/.." && pwd)"
GATE="$HERE/window-preflight.sh"
PROBE="$HERE/window-probe.sh"
PROV="$HERE/window-provision.sh"
BENCHCTL="$ROOT/target/release/benchctl"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/wp-offline.XXXXXX")"
SUITE_SINGLETON="${TMPDIR:-/tmp}/window-preflight-offline-suite.lock.d"
SINGLETON_MINE=0
# ONE EXIT trap for the whole suite. bash keeps a single handler per signal, so a second
# `trap … EXIT` REPLACES this one rather than adding to it — which is precisely what happened
# when the singleton guard installed its own: cleanup_boxes and the $WORK wipe stopped running,
# and 28 fixture trees (1.1 GB) plus 59 stand-in processes accumulated machine-wide. Every
# cleanup action belongs in this one handler.
# The trap is installed AFTER cleanup_boxes is defined, further down — a handler that fires
# before its own helper exists silently does nothing, which is the failure mode this whole
# section is about.
_suite_cleanup() {
  cleanup_boxes
  rm -rf "$WORK"
  [ "$SINGLETON_MINE" = "1" ] && rm -rf "$SUITE_SINGLETON" 2>/dev/null
  return 0
}
# TALLIES LIVE IN FILES, not shell variables. A variable incremented inside `( … )` — or inside
# any command substitution, pipeline stage or `$( )` — belongs to a subshell and dies with it,
# while the printf of the same result still reaches shared stdout. That combination produced a
# suite that PRINTED failures and exited 0, which is the worst of both: a human reading the log
# sees FAIL, a harness reading the exit code sees success, and the two disagree forever. Three
# env-seam cases were wrapped in subshells and were the only witnesses to the branches they
# covered. Unwrapping them fixes today; counting through the filesystem makes the whole carrier
# subshell-safe BY CONSTRUCTION, so the next case written inside a subshell still counts.
TALLY_DIR="$WORK/.tally"; mkdir -p "$TALLY_DIR"
: > "$TALLY_DIR/pass"; : > "$TALLY_DIR/fail"; : > "$TALLY_DIR/skip"
BOX_QWEN_PIDS=""
BOX_QWEN_TAGS=""
# N-G: killing the pids mkbox started is not enough. The PROBE also spawns stand-ins — every
# qwen_reload it runs backgrounds `exec -a <tag> sleep 900`, and those pids were never recorded
# here, so each window that reloaded left a 900-second orphan behind. The tags are what identify
# them, and they are stable per box, so cleanup sweeps by TAG as well as by pid.
cleanup_boxes() {
  [ -n "$BOX_QWEN_PIDS" ] && kill $BOX_QWEN_PIDS 2>/dev/null
  for _t in $BOX_QWEN_TAGS; do pkill -f "$_t" 2>/dev/null; done
  return 0
}
# N-H: EXIT alone leaves fixtures behind on Ctrl-C or a harness TERM, which is exactly how an
# interrupted run seeds the next one with contamination.
trap _suite_cleanup EXIT
trap '_suite_cleanup; exit 130' INT
trap '_suite_cleanup; exit 143' TERM
ok()   { printf '%s\n' "$1" >> "$TALLY_DIR/pass"; printf '  PASS  %s\n' "$1"; }
bad()  { printf '%s\n' "$1" >> "$TALLY_DIR/fail"; printf '  FAIL  %s\n' "$1"; }
skip() { printf '%s\n' "$1" >> "$TALLY_DIR/skip"; printf '  SKIP  %s\n' "$1"; }
tally() { wc -l < "$TALLY_DIR/$1" 2>/dev/null | tr -d ' '; }

command -v jq >/dev/null 2>&1 || { echo "jq is required"; exit 3; }
# F2: ENFORCE the single-instance rule above, do not merely document it. Two concurrent runs
# contaminated real review batteries twice in one day: the machine-global assertions (pgrep for
# stray engines and the serving model, box-side temp paths) see the other run's fixtures and both
# report each other's processes as failures. mkdir is atomic, so it is the whole guard.
# The lock records its holder's pid, so a corpse can be told from a live peer. Without this a
# single SIGKILLed run wedges every later one for good — the same permanent-brick shape as the
# stranded reap mutex, and no more acceptable here than there.
_singleton_pid=""; _singleton_age=""
if [ -d "$SUITE_SINGLETON" ]; then
  [ -f "$SUITE_SINGLETON/pid" ] && _singleton_pid="$(tr -d '[:space:]' < "$SUITE_SINGLETON/pid" 2>/dev/null)"
  _smt="$(stat -f %m "$SUITE_SINGLETON" 2>/dev/null || stat -c %Y "$SUITE_SINGLETON" 2>/dev/null)"
  [ -n "$_smt" ] && _singleton_age="$(( $(date -u +%s) - _smt ))"
  # Reclaim ONLY on evidence: the holder is provably gone. No age threshold is needed to prove
  # absence — a pid that does not exist cannot be running this suite — but a pid-less lock is
  # given a grace window, because it may be a peer between mkdir and its pid write.
  _sg_heal=0
  if [ -n "$_singleton_pid" ] && ! ps -p "$_singleton_pid" >/dev/null 2>&1; then
    _sg_why="its holder (pid $_singleton_pid) is gone"; _sg_heal=1
  elif [ -z "$_singleton_pid" ] && [ -n "$_singleton_age" ] && [ "$_singleton_age" -ge 60 ]; then
    _sg_why="it never recorded a holder (${_singleton_age}s old)"; _sg_heal=1
  fi
  if [ "$_sg_heal" = "1" ]; then
    # RENAME + IDENTITY CHECK, not rm-then-mkdir. Deleting here and creating below is the same
    # check-then-act hole as the reap mutex, one file over — and it is reachable ONLY because
    # healing was added: two starters that both sampled the lock as stale would both delete and
    # both create, recreating exactly the concurrent-run contamination this guard exists to
    # prevent. Rename the directory we judged, then prove it is the one we judged before
    # discarding it; if a live peer retook the path meanwhile, put it back and stand down.
    _sg_aside="$SUITE_SINGLETON.stale.$$.$(date -u +%s)"
    if mv "$SUITE_SINGLETON" "$_sg_aside" 2>/dev/null; then
      _sg_apid=""; [ -f "$_sg_aside/pid" ] && _sg_apid="$(tr -d '[:space:]' < "$_sg_aside/pid" 2>/dev/null)"
      _sg_amt="$(stat -f %m "$_sg_aside" 2>/dev/null || stat -c %Y "$_sg_aside" 2>/dev/null)"
      if [ "$_sg_apid" = "$_singleton_pid" ] && [ "$_sg_amt" = "$_smt" ]; then
        echo "note: reclaimed a stale suite lock — $_sg_why" >&2
        rm -rf "$_sg_aside" 2>/dev/null
      elif [ ! -e "$SUITE_SINGLETON" ] && mv "$_sg_aside" "$SUITE_SINGLETON" 2>/dev/null; then
        echo "note: a peer retook the suite lock while it was being reclaimed — restored, standing down" >&2
      else
        echo "FATAL: displaced another run's suite lock and could not restore it: $_sg_aside" >&2
        exit 3
      fi
    fi
  fi
fi
if mkdir "$SUITE_SINGLETON" 2>/dev/null; then
  SINGLETON_MINE=1
  printf '%s\n' "$$" > "$SUITE_SINGLETON/pid" 2>/dev/null || true
else
  _age="$_singleton_age"
  echo "FATAL: another copy of this suite is already running." >&2
  echo "       $SUITE_SINGLETON exists${_age:+ (${_age}s old)}." >&2
  echo "       This suite asserts over machine-GLOBAL state (pgrep for stray engine and serving" >&2
  echo "       processes, box-side temp paths), so two runs corrupt each other's results in BOTH" >&2
  echo "       directions. Wait for the other run, or remove that directory if it is stale." >&2
  exit 3
fi

if [ ! -x "$BENCHCTL" ]; then
  echo "FATAL: $BENCHCTL is not built." >&2
  echo "       The real-transport sections need it, a SKIP fails this suite, and finding that" >&2
  echo "       out at the end costs ~20 minutes. Build it first:" >&2
  echo "           cargo build --release -p benchctl" >&2
  exit 3
fi
HAVE_PY=1; command -v python3 >/dev/null 2>&1 || HAVE_PY=0
if printf 'eA==' | base64 -d >/dev/null 2>&1; then B64D="-d"; else B64D="-D"; fi
sha_of() { shasum -a 256 "$1" | awk '{print $1}'; }

echo "== 0. syntax checks =="
for s in window-preflight.sh window-probe.sh window-provision.sh test-window-preflight-offline.sh; do
  if bash -n "$HERE/$s" 2>"$WORK/syn"; then ok "bash -n $s"; else bad "bash -n $s"; sed 's/^/        /' "$WORK/syn"; fi
done
if command -v shellcheck >/dev/null 2>&1; then
  if shellcheck -S error "$GATE" "$PROBE" "$PROV" "$HERE/test-window-preflight-offline.sh" 2>"$WORK/sc"; then
    ok "shellcheck -S error (all four scripts)"
  else bad "shellcheck -S error"; sed 's/^/        /' "$WORK/sc"; fi
else skip "shellcheck not installed"; fi
echo ""

# ---------------------------------------------------------------------------
echo "== 1. dir_digest: the shell port vs benchctl's Rust formula =="
# The exact tree from `dir_digest_matches_swift_tree_formula` (crates/benchctl/src/iterate.rs:
# 2228-2265): a.bin="hello", sub/b.bin="world", plus the two exact-relative-path ignores. The
# expected digest is that test's own hand-computed value, so this pins the shell port to the
# same formula the Rust test pins the Rust port to.
DD="$WORK/dd"; mkdir -p "$DD/sub"
printf 'hello'   > "$DD/a.bin"
printf 'world'   > "$DD/sub/b.bin"
printf 'ignored' > "$DD/.gitkeep"
printf 'ignored' > "$DD/.benchmark-source.sha256"
DD_EXPECT="b7b67b4190bfbe19dbfbc95668c9f21bc78c6947cbc246d734b3b20a1de694be"
probe_obs() { # <record-file> <key> — decode one observation (base64 padding safe)
  local line v=""
  while IFS= read -r line; do
    case "$line" in "$2="*) v="${line#"$2"=}" ;; esac
  done < "$1"
  [ -n "$v" ] && printf '%s' "$v" | base64 "$B64D" 2>/dev/null
}
mkreq() { # key=value... -> base64 request envelope
  local out=""
  for kv in "$@"; do
    out="$out${kv%%=*}=$(printf '%s' "${kv#*=}" | base64 | tr -d '\n')
"
  done
  printf '%s' "$out" | base64 | tr -d '\n'
}
bash "$PROBE" "$(mkreq "weights_path=$DD")" > "$WORK/dd.rec" 2>/dev/null
[ "$(probe_obs "$WORK/dd.rec" weights.sha256)" = "$DD_EXPECT" ] \
  && ok "dir_digest sha256 matches the Rust/Swift tree formula vector" \
  || bad "dir_digest sha256 = $(probe_obs "$WORK/dd.rec" weights.sha256), want $DD_EXPECT"
[ "$(probe_obs "$WORK/dd.rec" weights.file_count)" = "2" ] \
  && ok "dir_digest ignores .gitkeep + .benchmark-source.sha256 by EXACT relative path" \
  || bad "dir_digest file_count = $(probe_obs "$WORK/dd.rec" weights.file_count), want 2"
[ "$(probe_obs "$WORK/dd.rec" weights.byte_count)" = "10" ] \
  && ok "dir_digest byte_count counts only the non-ignored files" \
  || bad "dir_digest byte_count = $(probe_obs "$WORK/dd.rec" weights.byte_count), want 10"
# A nested `.gitkeep` is NOT ignored — Swift ignores by exact rel path, not basename.
printf 'x' > "$DD/sub/.gitkeep"
bash "$PROBE" "$(mkreq "weights_path=$DD")" > "$WORK/dd2.rec" 2>/dev/null
[ "$(probe_obs "$WORK/dd2.rec" weights.file_count)" = "3" ] \
  && ok "a NESTED .gitkeep is counted (basename matches are not ignored)" \
  || bad "nested .gitkeep was wrongly ignored"
rm -f "$DD/sub/.gitkeep"
echo ""

# ---------------------------------------------------------------------------
# The fake box. Everything the gate looks at, and nothing it does not.
mkbox() { # <dir>
  local B="$1"
  rm -rf "$B"; mkdir -p "$B/weights" "$B/bin" "$B/out"
  git init -q "$B/bench"
  git -C "$B/bench" -c user.email=t@t -c user.name=t commit -q --allow-empty -m bench
  git -C "$B/bench" remote add origin https://example.invalid/bench.git
  git init -q "$B/engine"
  git -C "$B/engine" -c user.email=t@t -c user.name=t commit -q --allow-empty -m engine
  git -C "$B/engine" remote add origin https://example.invalid/engine.git
  printf 'hello' > "$B/weights/a.safetensors"
  printf 'world' > "$B/weights/b.safetensors"
  # A REAL golden: generated from the loader-parity corpus's own `valid()`, so the
  # `benchctl validate-golden` leg is a genuine loader verdict and not a stub's opinion.
  if [ "$HAVE_PY" = 1 ]; then
    python3 -c "
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location('g', '$HERE/gen-loader-parity-corpus.py')
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
json.dump(m.valid(), open('$B/golden1.json', 'w'))
" 2>/dev/null || printf '{"version":1}' > "$B/golden1.json"
  else printf '{"version":1}' > "$B/golden1.json"; fi
  printf '{"track_id":"fake-track"}' > "$B/contract.json"
  # A POOL TAPE: the required-key signature measure-job routes on. Deliberately NOT a golden —
  # the golden loader rejects this shape outright, which is why it has its own pin family.
  python3 - "$B/tape1.json" <<'TEOF' 2>/dev/null || printf '{"seed_tokens":[1],"reference_seed_token":5,"rows":[]}' > "$B/tape1.json"
import json, sys
json.dump({"seed_tokens": [1] * 8, "reference_seed_token": 5,
           "rows": [{"sequential_argmax": 7}], "reference_self_consistent": True,
           "emitted_tokens": [7]}, open(sys.argv[1], "w"))
TEOF
  # The worker: speaks NDJSON, answers the hello, echoes ids. Also answers an identity flag,
  # so the "run it, do not stat it" check has something real to interrogate.
  cat > "$B/bin/worker" <<'WEOF'
#!/bin/bash
case "${1-}" in
  --identity) printf 'mlxfast-runtime-worker engine-sha FAKEENGINESHA\n'; exit 0 ;;
esac
printf '{"id":0,"nonce":"n1","ok":true,"protocol_version":1,"backend":"mlx","device":"fake"}\n'
while IFS= read -r line; do
  id="$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')"
  printf '{"id":%s,"nonce":"n1","ok":true}\n' "${id:-1}"
done
WEOF
  chmod +x "$B/bin/worker"
  # The workers that break, one seam each.
  cat > "$B/bin/worker-dies" <<'WEOF'
#!/bin/bash
echo "mlxfast-runtime-worker: failed to load weights: metal device unavailable" >&2
exit 1
WEOF
  cat > "$B/bin/worker-garbage" <<'WEOF'
#!/bin/bash
printf '{"id":0,"nonce":"n1","ok":true,"protocol_version":99}\n'
while IFS= read -r _; do printf '{"id":1,"nonce":"n1","ok":true}\n'; done
WEOF
  cat > "$B/bin/worker-hang" <<'WEOF'
#!/bin/bash
sleep 3600
WEOF
  chmod +x "$B/bin/worker-dies" "$B/bin/worker-garbage" "$B/bin/worker-hang"
  mkdir -p "$B/heads"; printf 'head\n' > "$B/heads/head.safetensors"
  # A REAL qwen-service.sh: sourced, defines qwen_unload/qwen_reload as functions, and actually
  # starts and stops a stand-in process that MATCHES the pinned pattern — so unload's
  # poll-until-gone and release's poll-until-back both have something real to observe. The tag
  # is per-box so concurrent boxes cannot kill each other's stand-in.
  QTAG="wp-fake-qwen-$(basename "$B")-$$"
  printf '%s' "$QTAG" > "$B/.qwen-tag"
  cat > "$B/qwen-service.sh" <<QEOF
qwen_unload() { pkill -f '$QTAG' >/dev/null 2>&1; sleep 0.3; return 0; }
qwen_reload() { ( exec -a '$QTAG' sleep 900 ) >/dev/null 2>&1 </dev/null & sleep 0.3; return 0; }
QEOF
  # The box starts in the steady state a real box is in before a window: SERVING.
  ( exec -a "$QTAG" sleep 900 ) >/dev/null 2>&1 </dev/null &
  BOX_QWEN_PIDS="$BOX_QWEN_PIDS $!"
  BOX_QWEN_TAGS="$BOX_QWEN_TAGS $QTAG"
  disown "$!" 2>/dev/null || true
  # benchd: the real thing when it is built (so validate-golden and the smoke leg are real),
  # otherwise a stub that answers the two verbs the gate uses.
  if [ -x "$BENCHCTL" ]; then cp "$BENCHCTL" "$B/bin/benchd"
  else
    # Recorded as a SKIP-causing condition: the suite now FAILS on any skip, so this stub can
    # never quietly stand in for the real transport.
    printf 'WARNING: target/release/benchctl absent — real-transport checks will SKIP (and fail the suite)\n' >&2
    cat > "$B/bin/benchd" <<'WEOF'
#!/bin/bash
case "${1-}" in
  validate-golden) echo "stub validate-golden: ACCEPT" >&2; exit 0 ;;
  prefill-decompose) echo "stub prefill-decompose ok"; exit 0 ;;
  --help) echo "benchctl (stub)"; exit 0 ;;
esac
exit 2
WEOF
    chmod +x "$B/bin/benchd"
  fi
}

genpins() { # <boxdir> <pinsfile> <smoke-recipe> [worker-basename]
  local B="$1" P="$2" recipe="$3" wk="${4:-worker}" wd
  bash "$PROBE" "$(mkreq "weights_path=$B/weights")" > "$WORK/gp.rec" 2>/dev/null
  wd="$(probe_obs "$WORK/gp.rec" weights.sha256)"
  cat > "$P" <<EOF
# generated all-match pins for the fake box at $B
WP_DRIVER=local
WP_OUT=$B/att
WP_BOX_OUT=$B/out
WP_BENCH_PATH=$B/bench
WP_BENCH_SHA=$(git -C "$B/bench" rev-parse HEAD)
WP_BENCH_BUILD_CMD=none
WP_ENGINE_PATH=$B/engine
WP_ENGINE_SHA=$(git -C "$B/engine" rev-parse HEAD)
WP_ENGINE_BUILD_CMD=none
WP_ENGINE_BIN=$B/bin/$wk
WP_ENGINE_BIN_SHA256=$(sha_of "$B/bin/$wk")
WP_ENGINE_IDENTITY_ARGV=none
WP_BENCHD_BIN=$B/bin/benchd
WP_BENCHD_BIN_SHA256=$(sha_of "$B/bin/benchd")
WP_BENCHD_IDENTITY_ARGV=none
WP_WEIGHTS_PATH=$B/weights
WP_WEIGHTS_SHA256=$wd
WP_WEIGHTS_FILE_COUNT=$(probe_obs "$WORK/gp.rec" weights.file_count)
WP_WEIGHTS_BYTE_COUNT=$(probe_obs "$WORK/gp.rec" weights.byte_count)
WP_GOLDEN_1=$B/golden1.json $(sha_of "$B/golden1.json") $(wc -c < "$B/golden1.json" | tr -d ' ')
WP_POOL_TAPE_1=$B/tape1.json $(sha_of "$B/tape1.json") $(wc -c < "$B/tape1.json" | tr -d ' ')
WP_CONTRACT_PATH=$B/contract.json
WP_CONTRACT_SHA256=$(sha_of "$B/contract.json")
WP_MIN_FREE_GB=0
WP_MAX_LOADAVG=9999
WP_QWEN_PROC_PATTERN=$(cat "$B/.qwen-tag" 2>/dev/null)
WP_QWEN_EXPECT=loaded
WP_BOX_LOCK=$B/box.lock.d
WP_GPU_LOCK=$B/gpu.lock
WP_WINDOW_TAG=offline-test-$$
WP_LOCK_REAP_AGE_S=900
WP_EXPECT_SMOKE_FAIL=none
WP_QWEN_RELOAD_TRIES=2
WP_QWEN_SERVICE=$B/qwen-service.sh
WP_ENV_MLXFAST_RUNTIME_WORKER_EXECUTABLE=unset
WP_ENV_MLXFAST_MEASURE_WORKER_BIN=unset
WP_ENV_MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE=unset
WP_ENV_MLXFAST_NO_SANDBOX=unset
WP_ENV_MLXFAST_USE_RUNTIME_WORKER=unset
WP_ENV_QMTP_HEAD_DIR=unset
WP_REQUIRE_TIMEMACHINE_IDLE=0
WP_REQUIRE_NO_STRAY=0
WP_SMOKE_RECIPE=$recipe
WP_SMOKE_TIMEOUT_S=20
EOF
}

# gate <pinsfile> [extra...] — run, capture rc, stash the attestation. Never aborts the suite.
#
# A PASSING gate now exits STILL HOLDING the box lock (that is the handoff), so every case
# here releases afterwards — otherwise case N+1 would fail on a lock case N legitimately took.
# The release is a no-op when nothing is held or the holder tag is not ours, which is itself
# the behaviour the lock-lifecycle section asserts directly.
G_RC=0; G_ATT=""
# release_after <pinsfile> [extra --pin ...] — put a box back after a case that PASSED.
# A gate run that succeeds exits STILL HOLDING the lock with the serving model unloaded: that is
# the handoff, and it is correct. But a CASE that leaves it that way hands the mess to whatever
# runs next in the same box — which is exactly how the X-2 fixtures kept failing on a box that
# earlier sub-cases had left un-serving. Cases that pass and are not themselves about the handoff
# should hand the box back.
release_after() {
  local P="$1"; shift
  bash "$GATE" --pins "$P" "$@" --release >/dev/null 2>&1 || true
}

gate() {
  local P="$1"; shift
  local outdir; outdir="$(grep '^WP_OUT=' "$P" | tail -1 | sed 's/^WP_OUT=//')"
  rm -rf "$outdir"
  bash "$GATE" --pins "$P" "$@" > "$WORK/gate.out" 2>"$WORK/gate.err"; G_RC=$?
  G_ATT="$outdir/window-provenance.json"
  bash "$GATE" --pins "$P" --release >/dev/null 2>&1
  return 0
}
av()  { [ -f "$G_ATT" ] && jq -r --arg i "$1" '.items[]|select(.id==$i)|.verdict' "$G_ATT" 2>/dev/null | head -1; }
ad()  { [ -f "$G_ATT" ] && jq -r --arg i "$1" '.items[]|select(.id==$i)|.diagnostic' "$G_ATT" 2>/dev/null | head -1; }
ao()  { [ -f "$G_ATT" ] && jq -r --arg i "$1" '.items[]|select(.id==$i)|.observed' "$G_ATT" 2>/dev/null | head -1; }
# expect <label> <want-rc> <item-id> <want-verdict> <diag-substring>
expect() {
  local label="$1" wrc="$2" iid="$3" wv="$4" sub="$5" gotv gotd
  if [ "$G_RC" != "$wrc" ]; then
    bad "$label — exit $G_RC, want $wrc"; sed 's/^/        /' "$WORK/gate.err" | tail -4; return 0
  fi
  if [ -n "$iid" ]; then
    gotv="$(av "$iid")"; gotd="$(ad "$iid")"
    [ "$gotv" = "$wv" ] || { bad "$label — item $iid verdict '$gotv', want '$wv'"; return 0; }
    if [ -n "$sub" ] && ! printf '%s' "$gotd" | grep -qF -- "$sub"; then
      bad "$label — item $iid diagnostic lacks '$sub': $gotd"; return 0
    fi
  fi
  ok "$label"
}

B="$WORK/box"; PINS="$WORK/pins"
mkbox "$B"; genpins "$B" "$PINS" handshake

# ---------------------------------------------------------------------------
echo "== 2. all-match => PASS =="
gate "$PINS"
if [ "$G_RC" = "0" ]; then ok "all-match gate exits 0"
else bad "all-match gate exits $G_RC"; sed 's/^/        /' "$WORK/gate.out" | tail -20; fi
[ -f "$G_ATT" ] && ok "attestation written" || bad "no attestation at $G_ATT"
[ "$(jq -r .schema "$G_ATT" 2>/dev/null)" = "window-provenance/v1" ] \
  && ok "attestation schema = window-provenance/v1" || bad "wrong attestation schema"
[ "$(jq -r .verdict "$G_ATT" 2>/dev/null)" = "PASS" ] && ok "attestation verdict PASS" || bad "attestation verdict not PASS"
[ "$(jq -r .lock_taken "$G_ATT" 2>/dev/null)" = "true" ] \
  && ok "attestation records lock_taken=true" || bad "lock_taken not true"
[ "$(jq -r .lock.state "$G_ATT" 2>/dev/null)" = "held" ] \
  && ok "the gate ACQUIRED and HELD THE box lock (single-flight enforced, not asserted)" \
  || bad "lock.state = $(jq -r .lock.state "$G_ATT" 2>/dev/null), want held"
[ -n "$(jq -r .lock.acquired_utc "$G_ATT" 2>/dev/null)" ] \
  && ok "attestation records the box-clock lock acquisition timestamp" || bad "no lock.acquired_utc"
jq -r .lock.holder "$G_ATT" 2>/dev/null | grep -q "^tag=" \
  && ok "attestation records the lock holder identity" || bad "no lock holder identity"
for f in .gate.script_sha256 .gate.probe_sha256 .gate.provision_sha256 .gate.pins_file_sha256; do
  v="$(jq -r "$f" "$G_ATT" 2>/dev/null)"
  [ ${#v} -eq 64 ] && ok "attestation carries $f (64 hex)" || bad "$f is '$v'"
done
[ -n "$(jq -r .box.timestamp_utc "$G_ATT" 2>/dev/null)" ] \
  && ok "attestation carries the BOX's own timestamp" || bad "no box timestamp"
[ "$(jq '.items|length' "$G_ATT" 2>/dev/null)" -ge 20 ] \
  && ok "attestation itemises every check ($(jq '.items|length' "$G_ATT") items)" || bad "too few attestation items"
[ "$(jq -r '.items[]|select(.verdict=="FAIL")|.id' "$G_ATT" 2>/dev/null | wc -l | tr -d ' ')" = "0" ] \
  && ok "no FAIL items on the all-match run" || bad "unexpected FAIL items"
[ -f "$B/out/window-provenance.json" ] \
  && ok "attestation placed next to the run artifacts (WP_BOX_OUT)" || bad "attestation not copied to WP_BOX_OUT"
echo ""

# ---------------------------------------------------------------------------
echo "== 3. single-pin mismatches => right code, named diagnostic =="
BOGUS40="0000000000000000000000000000000000000000"
BOGUS64="0000000000000000000000000000000000000000000000000000000000000000"

gate "$PINS" --pin "WP_BENCH_SHA=$BOGUS40"
expect "bench HEAD mismatch => 1" 1 BENCH.head FAIL "wrong commit"

gate "$PINS" --pin "WP_ENGINE_SHA=$BOGUS40"
expect "engine HEAD mismatch => 1" 1 ENGINE.head FAIL "wrong commit"

gate "$PINS" --pin "WP_ENGINE_BIN_SHA256=$BOGUS64"
expect "engine BINARY sha mismatch => 1" 1 ENGINEBIN.sha256 FAIL "a path is not a seal"

gate "$PINS" --pin "WP_BENCHD_BIN_SHA256=$BOGUS64"
expect "benchd BINARY sha mismatch => 1" 1 BENCHDBIN.sha256 FAIL "a path is not a seal"

gate "$PINS" --pin "WP_WEIGHTS_SHA256=$BOGUS64"
expect "weights digest mismatch => 1" 1 weights.sha256 FAIL "weights tree digest mismatch"

gate "$PINS" --pin "WP_WEIGHTS_FILE_COUNT=999"
expect "weights file-count mismatch => 1" 1 weights.file_count FAIL "file count mismatch"

gate "$PINS" --pin "WP_WEIGHTS_BYTE_COUNT=999"
expect "weights byte-count mismatch => 1" 1 weights.byte_count FAIL "byte count mismatch"

GP="$(grep '^WP_GOLDEN_1=' "$PINS" | sed 's/^WP_GOLDEN_1=//')"
GPATH="$(printf '%s' "$GP" | awk '{print $1}')"; GSHA="$(printf '%s' "$GP" | awk '{print $2}')"
GBYTES="$(printf '%s' "$GP" | awk '{print $3}')"
gate "$PINS" --pin "WP_GOLDEN_1=$GPATH $BOGUS64 $GBYTES"
expect "golden sha256 mismatch => 4" 4 golden1.sha256 FAIL "sha256 mismatch"

gate "$PINS" --pin "WP_GOLDEN_1=$GPATH $GSHA 999999"
expect "golden BYTE COUNT mismatch => 4 (the sha256+bytes pin is two numbers)" 4 golden1.bytes FAIL "byte count mismatch"

gate "$PINS" --pin "WP_CONTRACT_SHA256=$BOGUS64"
expect "contract sha mismatch => 1" 1 contract.sha256 FAIL "contract sha256 mismatch"

# ---------------------------------------------------------------------------
echo "== 3b. LANE 2a: the correctness golden's pin is SOURCED FROM THE FIXTURE =="
# NEW-PATH coverage. The hidden correctness golden's identity is the review-gated
# `hidden_correctness_golden` SIBLING pin IN THE TRACK CONTRACT — not an operator WP_GOLDEN line
# (machine-state) and not a hardcoded box path. Build a contract that pins golden1's real
# sha256+bytes and prove the gate SOURCES the pin from it and PIN-VERIFIES the staged golden against
# it. (The WP_GOLDEN_* family stays for its loader/signature coverage above; this adds the fixture
# authority on top of it.)
G1_SHA="$(sha_of "$B/golden1.json")"; G1_BYTES="$(wc -c < "$B/golden1.json" | tr -d ' ')"
cat > "$B/contract-hcg.json" <<JEOF
{"track_id":"fake-track","hidden_correctness_golden":{"sha256":"$G1_SHA","bytes":$G1_BYTES},"hidden_correctness_golden_note":"a SIBLING of timed_prompt_pool, never a ninth pool entry"}
JEOF
gate "$PINS" \
  --pin "WP_CONTRACT_PATH=$B/contract-hcg.json" \
  --pin "WP_CONTRACT_SHA256=$(sha_of "$B/contract-hcg.json")"
expect "the fixture-pinned correctness golden is sourced from --contract => PASS" 0 \
  contract.hidden_correctness_golden PASS "review-gated fixture pin"
[ "$(av hidden_correctness_golden.sha256)" = "PASS" ] \
  && ok "the staged golden is PIN-VERIFIED against the FIXTURE sha256 (not an operator pin)" \
  || bad "hidden_correctness_golden.sha256 verdict = '$(av hidden_correctness_golden.sha256)'"
[ "$(av hidden_correctness_golden.bytes)" = "PASS" ] \
  && ok "and against the FIXTURE byte count" \
  || bad "hidden_correctness_golden.bytes verdict = '$(av hidden_correctness_golden.bytes)'"

# Wrong-digest: the fixture pins a DIFFERENT sha than the staged golden hashes to => REFUSE (exit 4).
# This is the gate-side mirror of benchd's wrong-digest refusal: the FIXTURE is the authority, so a
# staged golden that does not cite it is rejected even when the operator's own WP_GOLDEN pin matches.
cat > "$B/contract-hcg-bad.json" <<JEOF
{"track_id":"fake-track","hidden_correctness_golden":{"sha256":"$BOGUS64","bytes":$G1_BYTES}}
JEOF
gate "$PINS" \
  --pin "WP_CONTRACT_PATH=$B/contract-hcg-bad.json" \
  --pin "WP_CONTRACT_SHA256=$(sha_of "$B/contract-hcg-bad.json")"
expect "a staged golden that does not cite the fixture pin => 4 (fixture is the authority)" 4 \
  hidden_correctness_golden.sha256 FAIL "does not cite the fixture pin"

# A contract that pins NO correctness golden (the offline default) leaves the gate inert — the
# fixture-sourcing rows are simply absent, and the window still passes on its other pins.
gate "$PINS"
[ -z "$(av contract.hidden_correctness_golden)" ] \
  && ok "a fixture without hidden_correctness_golden leaves the LANE 2a rows absent (inert)" \
  || bad "unexpected LANE 2a row on a fixture that pins no correctness golden"
release_after "$PINS"

# POOL TAPES — same sha256+bytes pin, but a signature check instead of the golden loader, which
# rejects every tape by construction. A separate KIND, not a separate spelling.
# NEW-3: a correctly-pinned artifact under a path CONTAINING A SPACE must verify. Parsing the
# path as awk field 1 truncated it at the first space and reported ABSENT (exit 5) — the gate
# blaming the operator for a path it mangled itself.
SPACED="$B/pool tapes/with space"; mkdir -p "$SPACED"
cp "$B/tape1.json" "$SPACED/tape s.json"
cp "$B/golden1.json" "$SPACED/golden s.json"
gate "$PINS" \
  --pin "WP_POOL_TAPE_1=$SPACED/tape s.json $(sha_of "$SPACED/tape s.json") $(wc -c < "$SPACED/tape s.json" | tr -d ' ')" \
  --pin "WP_GOLDEN_1=$SPACED/golden s.json $(sha_of "$SPACED/golden s.json") $(wc -c < "$SPACED/golden s.json" | tr -d ' ')"
[ "$G_RC" = "0" ] \
  && ok "NEW-3: a golden AND a tape under a path with SPACES both verify (was ABSENT, exit 5)" \
  || { bad "NEW-3: spaced pin path rc=$G_RC"; grep -E "tape1|golden1" "$WORK/gate.out" | head -4; }
# And the pin values are still read from the right end of the line.
gate "$PINS" --pin "WP_POOL_TAPE_1=$SPACED/tape s.json $BOGUS64 $(wc -c < "$SPACED/tape s.json" | tr -d ' ')"
expect "NEW-3: and a bad sha under a spaced path still fails on the SHA" 4 tape1.sha256 FAIL "sha256 mismatch"
# A pin missing its byte count is a usage error, not a silent half-check.
gate "$PINS" --pin "WP_POOL_TAPE_1=/some/path deadbeef"
[ "$G_RC" = "2" ] && ok "NEW-3: a malformed (short) tape pin is a usage error" \
  || bad "NEW-3: short tape pin rc=$G_RC"

TP="$(grep '^WP_POOL_TAPE_1=' "$PINS" | sed 's/^WP_POOL_TAPE_1=//')"
TPATH="$(printf '%s' "$TP" | awk '{print $1}')"; TSHA="$(printf '%s' "$TP" | awk '{print $2}')"
TBYTES="$(printf '%s' "$TP" | awk '{print $3}')"
gate "$PINS" --pin "WP_POOL_TAPE_1=$TPATH $BOGUS64 $TBYTES"
expect "pool TAPE sha256 mismatch => 4" 4 tape1.sha256 FAIL "pool tape 1 sha256 mismatch"
gate "$PINS" --pin "WP_POOL_TAPE_1=$TPATH $TSHA 999999"
expect "pool TAPE byte count mismatch => 4" 4 tape1.bytes FAIL "pool tape 1 byte count mismatch"
# A file that is pinned as a tape but is not one must fail on the SIGNATURE, not the loader.
gate "$PINS" --pin "WP_POOL_TAPE_1=$GPATH $(sha_of "$GPATH") $(wc -c < "$GPATH" | tr -d ' ')"
expect "a GOLDEN pinned as a pool tape fails the required-key signature => 4" \
  4 tape1.signature FAIL "does not carry the required-key signature"
# And the healthy tape passes its signature check without ever touching validate-golden.
gate "$PINS"
[ "$(av tape1.signature)" = "PASS" ] \
  && ok "a real pool tape passes on its SIGNATURE (the golden loader would reject it)" \
  || bad "healthy tape signature verdict = '$(av tape1.signature)'"
# The two families must be independently required-shaped.
grep -v '^WP_GOLDEN_1=\|^WP_POOL_TAPE_1=' "$PINS" > "$WORK/pins-noinput"
gate "$WORK/pins-noinput"
[ "$G_RC" = "2" ] && grep -q "WP_POOL_TAPE_1" "$WORK/gate.err" \
  && ok "with neither a golden nor a tape pinned, the usage error names BOTH families" \
  || bad "missing-comparison-input usage error rc=$G_RC"
# A tape alone is a complete window input — goldens are not mandatory.
grep -v '^WP_GOLDEN_1=' "$PINS" > "$WORK/pins-tapeonly"
gate "$WORK/pins-tapeonly"
[ "$G_RC" = "0" ] && ok "a window pinned with ONLY pool tapes is complete (no golden required)" \
  || { bad "tape-only window rc=$G_RC"; tail -6 "$WORK/gate.out"; }

# A pin that is simply absent must be a USAGE error, never a silent default.
grep -v '^WP_WEIGHTS_SHA256=' "$PINS" > "$WORK/pins-nopin"
gate "$WORK/pins-nopin"
[ "$G_RC" = "2" ] && grep -q 'WP_WEIGHTS_SHA256' "$WORK/gate.err" \
  && ok "a MISSING pin is a usage error naming the pin (=> 2), not a default" \
  || bad "missing pin gave rc=$G_RC"
echo ""

# ---------------------------------------------------------------------------
echo "== 4. dirty tree / absent tree / absent + unusable binaries =="
printf 'uncommitted' > "$B/bench/scratch.txt"
gate "$PINS"
expect "dirty bench tree => 1, and the diagnostic NAMES the file" 1 BENCH.clean FAIL "scratch.txt"
rm -f "$B/bench/scratch.txt"

gate "$PINS" --pin "WP_ENGINE_PATH=$B/not-there"
expect "absent engine checkout => 5 (the Proof A failure verbatim)" 5 ENGINE.present FAIL "not on the box at all"

gate "$PINS" --pin "WP_ENGINE_BIN=$B/bin/not-a-binary"
expect "absent engine binary => 5" 5 ENGINEBIN.present FAIL "not at its pinned path"

cp "$B/bin/worker" "$B/bin/worker-noexec"; chmod -x "$B/bin/worker-noexec"
gate "$PINS" --pin "WP_ENGINE_BIN=$B/bin/worker-noexec" \
             --pin "WP_ENGINE_BIN_SHA256=$(sha_of "$B/bin/worker-noexec")"
expect "engine binary present but not executable => 5" 5 ENGINEBIN.executable FAIL "execute bit"

# Identity is a RUN, not a stat: right path, right digest, wrong self-report.
gate "$PINS" --pin "WP_ENGINE_IDENTITY_ARGV=--identity" \
             --pin "WP_ENGINE_IDENTITY_EXPECT=SOMEOTHERSHA"
expect "binary runs but reports the wrong identity => 1" 1 ENGINEBIN.identity FAIL "does not report the pinned identity"

gate "$PINS" --pin "WP_ENGINE_IDENTITY_ARGV=--identity" \
             --pin "WP_ENGINE_IDENTITY_EXPECT=FAKEENGINESHA"
expect "binary runs and reports the pinned identity => 0" 0 ENGINEBIN.identity PASS ""
echo ""

# ---------------------------------------------------------------------------
echo "== 5. the bundle rule =="
# A tree with no origin remote and no bundle record cannot have its provenance claimed at all.
BB="$WORK/box-bundle"; mkbox "$BB"; genpins "$BB" "$WORK/pins-b" handshake
git -C "$BB/engine" remote remove origin
gate "$WORK/pins-b"
expect "no origin remote + no bundle record => REFUSED (7)" 7 ENGINE.bundle REFUSED "provenance cannot be claimed from evidence"

# Bundle-shipped, hash NOT pinned => refuse. This is the path Proof A actually used.
printf 'bundle_sha256=%s\ncommit=%s\n' "$BOGUS64" "$(git -C "$BB/engine" rev-parse HEAD)" \
  > "$(git -C "$BB/engine" rev-parse --absolute-git-dir)/window-bundle-provenance"
gate "$WORK/pins-b"
expect "bundle-shipped tree with NO pinned bundle hash => REFUSED (7)" 7 ENGINE.bundle REFUSED "refusing an unprovenanced tree"

# Bundle-shipped, hash pinned but WRONG => refuse.
gate "$WORK/pins-b" --pin "WP_ENGINE_BUNDLE_SHA256=deadbeef"
expect "bundle hash does not match its pin => REFUSED (7)" 7 ENGINE.bundle REFUSED "does not match its pin"

# Bundle-shipped, hash pinned and correct => accepted AND recorded.
# Hash pinned and matching, but the bundle FILE is not on the box to re-verify against — so the
# row is accepted and graded CLAIMED, never PASS. (Section 11 covers the VERIFIED case, where
# the bundle is still present and the claim is re-derived from its own bytes.)
gate "$WORK/pins-b" --pin "WP_ENGINE_BUNDLE_SHA256=$BOGUS64"
[ "$G_RC" = "0" ] && ok "bundle-shipped tree WITH a matching pinned hash is accepted" \
  || bad "matching-hash bundle gave rc=$G_RC"
[ "$(av ENGINE.bundle)" = "CLAIMED" ] \
  && ok "and it is graded CLAIMED, not PASS — a self-attested marker is not a verification" \
  || bad "expected CLAIMED, got '$(av ENGINE.bundle)'"
jq -r '.items[]|select(.id=="ENGINE.bundle")|.observed' "$G_ATT" 2>/dev/null | grep -qF "$BOGUS64" \
  && ok "the accepted bundle's hash is RECORDED in the attestation" \
  || bad "bundle hash not recorded in the attestation"
echo ""

# ---------------------------------------------------------------------------
echo "== 6. basics: locks, disk, box-quiet, serving state =="
# RULED: reap the provably dead, refuse everything ambiguous. Four cases, one axis each.
LOCKD="$B/box.lock.d"
mklock() { # <pid-file-contents-or-EMPTY> <mtime-stamp-or-now> [holder-text]
  rm -rf "$LOCKD"; mkdir -p "$LOCKD"
  [ "$1" != "EMPTY" ] && printf '%s\n' "$1" > "$LOCKD/pid"
  printf 'tag=prior-session\npid=%s\nuser=someone\nacquired_utc=2026-08-01T00:00:00Z\n' "$1" > "$LOCKD/holder"
  [ "$2" != "now" ] && touch -t "$2" "$LOCKD"
  return 0
}

# (a) LIVE pid — refuse, whatever the age.
mklock "$$" 202001010000
gate "$PINS"
expect "live holder pid => refused (3), never reaped" 3 boxlock FAIL "held by a running process"
[ -d "$LOCKD" ] && ok "the live lock was left untouched" || bad "the gate reaped a LIVE holder"

# (b) UNVERIFIABLE holder — no pid file at all. Refuse: cannot prove death.
mklock EMPTY 202001010000
gate "$PINS"
expect "unverifiable holder (no numeric pid) => refused (3)" 3 boxlock FAIL "cannot be proved dead"
[ -d "$LOCKD" ] && ok "the unverifiable lock was left untouched" || bad "the gate reaped an unverifiable holder"

# (c) DEAD but FRESH — under the age threshold. Refuse: may be mid-restart.
mklock "999999" now
gate "$PINS" --pin "WP_LOCK_REAP_AGE_S=3600"
expect "dead holder but lock younger than the threshold => refused (3)" 3 boxlock FAIL "younger than the reap threshold"
[ -d "$LOCKD" ] && ok "the too-fresh lock was left untouched" || bad "the gate reaped a fresh lock"

# (d) PROVABLY DEAD and OLD ENOUGH — reap, and seal the evidence.
mklock "999999" 202001010000
gate "$PINS" --pin "WP_LOCK_REAP_AGE_S=60"
[ "$G_RC" = "0" ] && ok "provably-dead + old-enough lock => reaped, gate proceeds (0)" \
  || { bad "reapable lock gave rc=$G_RC"; tail -12 "$WORK/gate.out"; }
R="$(jq -r '.lock.reaped' "$G_ATT" 2>/dev/null)"
[ "$R" != "null" ] && ok "the attestation carries a reaped block" || bad "no reaped block in the attestation"
[ "$(jq -r '.lock.reaped.prior_tag' "$G_ATT" 2>/dev/null)" = "prior-session" ] \
  && ok "reaped block seals the PRIOR holder's tag" || bad "prior tag not sealed"
[ "$(jq -r '.lock.reaped.prior_pid' "$G_ATT" 2>/dev/null)" = "999999" ] \
  && ok "reaped block seals the prior holder's pid" || bad "prior pid not sealed"
[ "$(jq -r '.lock.reaped.prior_user' "$G_ATT" 2>/dev/null)" = "someone" ] \
  && ok "reaped block seals the prior holder's user" || bad "prior user not sealed"
[ "$(jq -r '.lock.reaped.prior_acquired_utc' "$G_ATT" 2>/dev/null)" = "2026-08-01T00:00:00Z" ] \
  && ok "reaped block seals the prior holder's acquired_utc" || bad "prior acquired_utc not sealed"
jq -r '.lock.reaped.verified_dead_how' "$G_ATT" 2>/dev/null | grep -q "ps -p 999999" \
  && ok "reaped block states HOW death was verified" || bad "verified_dead_how missing"
[ -n "$(jq -r '.lock.reaped.reaped_utc' "$G_ATT" 2>/dev/null)" ] \
  && ok "reaped block carries the reap timestamp" || bad "no reap timestamp"
[ "$(jq -r '.lock.reap_age_threshold_s' "$G_ATT" 2>/dev/null)" = "60" ] \
  && ok "the attestation records the reap age threshold that was in force" || bad "no reap threshold recorded"
# And the refusals are attested too, not merely printed.
mklock "$$" 202001010000
gate "$PINS"
[ "$(jq -r '.lock.reap_refused.reason' "$G_ATT" 2>/dev/null)" = "holder-alive" ] \
  && ok "a REFUSED reap is attested with its reason" || bad "reap_refused not attested"
rm -rf "$LOCKD"

gate "$PINS" --pin "WP_MIN_FREE_GB=999999999"
expect "free disk below the pinned floor => 3" 3 disk.free FAIL "not enough free space"

gate "$PINS" --pin "WP_MAX_LOADAVG=0"
expect "box load above the pinned ceiling => 3" 3 quiet.loadavg FAIL "not idle enough"

# Serving state: the probe pattern is made to match this very test process, so `pgrep -f` sees
# something and the state reads `loaded` while the pins expect `unloaded`.
# The box is serving (mkbox left it that way), which is what WP_QWEN_EXPECT=loaded pins. A
# window that expects the model DOWN before it starts must refuse when it is up, and vice versa.
gate "$PINS" --pin "WP_QWEN_EXPECT=unloaded"
expect "serving model resident when the window expects it DOWN => 6" 6 qwen.state FAIL "not in the state this window assumes"
gate "$PINS" --pin "WP_QWEN_PROC_PATTERN=a-name-that-is-never-resident-zzz"
expect "serving model ABSENT when the window expects it up => 6" 6 qwen.state FAIL "not in the state this window assumes"

gate "$PINS" --pin "WP_QWEN_EXPECT=none"
expect "WP_QWEN_EXPECT=none is a DECLARED waiver, not a silent skip" 0 qwen.state PASS ""
echo ""

# ---------------------------------------------------------------------------
echo "== 7. SMOKE LEG (real benchd, fake workers) =="
if [ ! -x "$BENCHCTL" ]; then
  skip "smoke matrix needs target/release/benchctl (cargo build --release -p benchctl)"
else
  gate "$PINS"
  expect "healthy worker: spawn + hello + round trip => PASS" 0 smoke.handshake PASS ""
  [ "$(jq -r .smoke.verdict "$G_ATT")" = "PASS" ] && ok "attestation smoke.verdict = PASS" || bad "smoke.verdict wrong"
  [ "$(jq -r .smoke.recipe  "$G_ATT")" = "handshake" ] && ok "attestation records the smoke recipe" || bad "smoke recipe not recorded"
  [ -n "$(jq -r .smoke.argv "$G_ATT")" ] && ok "attestation records the exact smoke argv" || bad "smoke argv not recorded"

  # #134's signature: the worker dies before writing the hello.
  gate "$PINS" --pin "WP_ENGINE_BIN=$B/bin/worker-dies" \
               --pin "WP_ENGINE_BIN_SHA256=$(sha_of "$B/bin/worker-dies")"
  expect "worker dies at hello => SMOKE FAIL (8), #134 signature named" 8 smoke.handshake FAIL "spawn seam broken"
  if printf '%s' "$(ad smoke.handshake)" | grep -qF "metal device unavailable"; then
    ok "the WORKER's own stderr survives into the diagnostic (the diagnosability win)"
  else bad "worker stderr not captured into the diagnostic: $(ad smoke.handshake)"; fi
  if jq -r .smoke.benchd_stderr "$G_ATT" | grep -q 'mlxfast-worker:'; then
    ok "attestation seals the forwarded worker stderr" ; else bad "worker stderr not sealed"; fi

  # The declared-dependency posture: the SAME failure, labelled EXPECTED-FAIL(<ref>), still
  # failing closed. A known-open issue must not read as a gate defect, and must not be waived.
  gate "$PINS" --pin "WP_ENGINE_BIN=$B/bin/worker-dies" \
               --pin "WP_ENGINE_BIN_SHA256=$(sha_of "$B/bin/worker-dies")" \
               --pin "WP_EXPECT_SMOKE_FAIL=#134"
  expect "declared dependency => EXPECTED-FAIL(#134) verdict, still exit 8" \
    8 smoke.handshake EXPECTED-FAIL "the declared known failure #134 reproduced"
  [ "$(jq -r .smoke.verdict "$G_ATT")" = "EXPECTED-FAIL(#134)" ] \
    && ok "the attestation records smoke.verdict = EXPECTED-FAIL(#134)" \
    || bad "EXPECTED-FAIL verdict not recorded ($(jq -r .smoke.verdict "$G_ATT"))"

  gate "$PINS" --pin "WP_ENGINE_BIN=$B/bin/worker-garbage" \
               --pin "WP_ENGINE_BIN_SHA256=$(sha_of "$B/bin/worker-garbage")"
  expect "worker sends a garbage hello => SMOKE FAIL (8), version named" 8 smoke.handshake FAIL "different protocol version"

  gate "$PINS" --pin "WP_ENGINE_BIN=$B/bin/worker-hang" \
               --pin "WP_ENGINE_BIN_SHA256=$(sha_of "$B/bin/worker-hang")" \
               --pin "WP_SMOKE_TIMEOUT_S=3"
  expect "worker hangs forever => SMOKE FAIL (8) on the timeout, not a hung gate" 8 smoke.handshake FAIL "smoke leg hung"

  # Fail-closed ordering: a smoke leg must not run against a tree that failed the pin phase.
  gate "$PINS" --pin "WP_BENCH_SHA=$BOGUS40"
  [ "$(jq -r .smoke.verdict "$G_ATT")" = "NOT-RUN" ] \
    && ok "smoke leg is NOT RUN when phase 1 failed (a PASS there would describe the wrong tree)" \
    || bad "smoke leg ran despite a failed pin phase"

  gate "$PINS" --no-smoke
  [ "$G_RC" = "0" ] && [ "$(jq -r .smoke.verdict "$G_ATT")" = "DECLARED-WAIVED" ] \
    && ok "--no-smoke records a DECLARED waiver in the attestation" || bad "--no-smoke not recorded as a declared waiver"

  # The recipe argv is EVAL'd on the box, so it must survive exactly one round of shell
  # parsing. The `decode` recipe carries JSON: bare {"mode":"serial"} comes back out of eval
  # as {mode:serial}, which benchd rejects — so the emitted form is asserted directly rather
  # than assumed. (The decode leg itself needs a real contract + QMTP_HEAD_DIR, so only its
  # argv is exercised offline.)
  gate "$PINS" --pin "WP_SMOKE_RECIPE=decode"
  DARGV="$(jq -r .smoke.argv "$G_ATT" 2>/dev/null)"
  if [ -n "$DARGV" ]; then
    EVALED="$(eval "printf '%s\n' $DARGV" 2>/dev/null | tr '\n' ' ')"
    printf '%s' "$EVALED" | grep -qF -- '{"mode":"mtp","mtp":{"depth":2}}' \
      && ok "the decode recipe's spec JSON survives eval on the box intact" \
      || bad "decode spec JSON mangled by eval: $EVALED"
    # M2: the recipe must carry the REAL leg's shape, not a teacher-forced prefix.
    printf '%s' "$DARGV" | grep -qF -- '"mode":"mtp"' \
      && ok "the decode recipe SPECULATES, so the real leg's --speculative-protocol argv is exercised" \
      || bad "decode recipe is teacher-forced and cannot reproduce an argv-rejection"
    printf '%s' "$DARGV" | grep -qv -- '--tokens' \
      && ok "the decode recipe omits --tokens (a hard usage error on the free-run branch)" \
      || bad "decode recipe still passes --tokens on a speculating spec"
  else bad "decode recipe produced no argv"; fi

  # A path with a space must not split into two arguments.
  mkdir -p "$B/we ird"; cp "$B/weights/"* "$B/we ird/" 2>/dev/null
  bash "$GATE" --pins "$PINS" --pin "WP_WEIGHTS_PATH=$B/we ird" \
       --pin "WP_OUT=$B/att-sp" >/dev/null 2>&1
  SP_ARGV="$(jq -r .smoke.argv "$B/att-sp/window-provenance.json" 2>/dev/null)"
  if [ -n "$SP_ARGV" ]; then
    # Split exactly as the box's `eval` would, then look for the spaced path as ONE element.
    if eval "printf '%s\n' $SP_ARGV" 2>/dev/null | grep -qxF "$B/we ird"; then
      ok "a weights path containing a space stays ONE argv element through eval"
    else
      bad "spaced path was split by eval: $(eval "printf '%s\n' $SP_ARGV" 2>/dev/null | tr '\n' '|')"
    fi
  else skip "spaced-path argv check (no argv emitted)"; fi
  bash "$GATE" --pins "$PINS" --release >/dev/null 2>&1
fi
echo ""

# ---------------------------------------------------------------------------
echo "== 8. window-provision =="
PB="$WORK/box-prov"; mkbox "$PB"; genpins "$PB" "$WORK/pins-p" none
# Rule 1: an existing checkout at another commit is REFUSED, never switched.
git -C "$PB/engine" -c user.email=t@t -c user.name=t commit -q --allow-empty -m drift
DRIFTED="$(git -C "$PB/engine" rev-parse HEAD)"
bash "$PROV" --pins "$WORK/pins-p" --driver local > "$WORK/prov.out" 2>&1; PRC=$?
[ "$PRC" = "7" ] && grep -q "will NOT switch a checkout" "$WORK/prov.out" \
  && ok "provision REFUSES to switch a checkout at another commit (=> 7)" \
  || { bad "provision rc=$PRC on a drifted checkout"; sed 's/^/        /' "$WORK/prov.out" | tail -5; }
[ "$(git -C "$PB/engine" rev-parse HEAD)" = "$DRIFTED" ] \
  && ok "the drifted checkout was left EXACTLY as it was" || bad "provision moved someone else's HEAD"

# Already correct => no-op, exit 0.
sed "s|^WP_ENGINE_SHA=.*|WP_ENGINE_SHA=$DRIFTED|" "$WORK/pins-p" > "$WORK/pins-p2"
bash "$PROV" --pins "$WORK/pins-p2" --driver local > "$WORK/prov2.out" 2>&1; PRC=$?
[ "$PRC" = "0" ] && grep -q "already at the pinned commit" "$WORK/prov2.out" \
  && ok "provision is a no-op when the tree is already at the pin" || bad "provision rc=$PRC on an already-correct tree"

# Rule 3: a bundle with no pinned hash never reaches the wire.
git -C "$PB/bench" bundle create "$WORK/bench.bundle" --all >/dev/null 2>&1
BSHA="$(sha_of "$WORK/bench.bundle")"
BENCH_SHA_P="$(git -C "$PB/bench" rev-parse HEAD)"
mk_bundle_pins() { # <destpath> <bundle-sha-pin-or-empty>
  { grep -v '^WP_BENCH_PATH=\|^WP_BENCH_BUNDLE\|^WP_BENCH_SOURCE_CLONE' "$WORK/pins-p2"
    printf 'WP_BENCH_PATH=%s\n' "$1"
    printf 'WP_BENCH_BUNDLE=%s\n' "$WORK/bench.bundle"
    [ -n "$2" ] && printf 'WP_BENCH_BUNDLE_SHA256=%s\n' "$2"
  } > "$WORK/pins-bundle"
}
mk_bundle_pins "$PB/from-bundle" ""
bash "$PROV" --pins "$WORK/pins-bundle" --driver local > "$WORK/prov3.out" 2>&1; PRC=$?
[ "$PRC" = "7" ] && grep -q "unprovenanced tree" "$WORK/prov3.out" \
  && ok "provision REFUSES a bundle with no pinned sha256 (=> 7)" \
  || { bad "provision rc=$PRC on an unpinned bundle"; sed 's/^/        /' "$WORK/prov3.out" | tail -5; }
[ ! -e "$PB/from-bundle" ] && ok "nothing was created for the refused bundle" || bad "a tree was created despite the refusal"

mk_bundle_pins "$PB/from-bundle" "$BOGUS64"
bash "$PROV" --pins "$WORK/pins-bundle" --driver local > "$WORK/prov4.out" 2>&1; PRC=$?
[ "$PRC" = "7" ] && ok "provision REFUSES a bundle whose sha256 differs from the pin" || bad "wrong-hash bundle rc=$PRC"

mk_bundle_pins "$PB/from-bundle" "$BSHA"
sed -i.bak "s|^WP_BENCH_SHA=.*|WP_BENCH_SHA=$BENCH_SHA_P|" "$WORK/pins-bundle"
bash "$PROV" --pins "$WORK/pins-bundle" --driver local > "$WORK/prov5.out" 2>&1; PRC=$?
if [ "$PRC" = "0" ] && [ -d "$PB/from-bundle" ]; then
  ok "provision accepts a bundle whose sha256 matches the pin"
else bad "pinned-bundle provision rc=$PRC"; sed 's/^/        /' "$WORK/prov5.out" | tail -8; fi
if grep -q "bundle_sha256=$BSHA" \
     "$(git -C "$PB/from-bundle" rev-parse --absolute-git-dir 2>/dev/null)/window-bundle-provenance" 2>/dev/null; then
  ok "the bundle hash is RECORDED into the git dir's window-bundle-provenance"
else bad "window-bundle-provenance missing or wrong"; fi
[ -z "$(git -C "$PB/from-bundle" status --porcelain 2>/dev/null)" ] \
  && ok "the bundle record does NOT dirty the working tree (it lives in the git dir)" \
  || bad "the provenance record made the tree dirty, which would fail the clean-tree check"
[ "$(git -C "$PB/from-bundle" rev-parse HEAD 2>/dev/null)" = "$BENCH_SHA_P" ] \
  && ok "the bundle-provisioned tree is at the pinned commit" || bad "bundle tree is at the wrong commit"

# And the gate then accepts that tree, because its bundle hash is pinned AND recorded.
genpins "$PB" "$WORK/pins-bg" none
{ grep -v '^WP_BENCH_PATH=\|^WP_BENCH_SHA=' "$WORK/pins-bg"
  printf 'WP_BENCH_PATH=%s\n' "$PB/from-bundle"
  printf 'WP_BENCH_SHA=%s\n' "$BENCH_SHA_P"
  printf 'WP_BENCH_BUNDLE_SHA256=%s\n' "$BSHA"
} > "$WORK/pins-bg2"
gate "$WORK/pins-bg2"
expect "the GATE accepts the bundle-provisioned tree (pinned + recorded)" 0 BENCH.bundle PASS ""
echo ""

# ---------------------------------------------------------------------------
echo "== 9. lock lifecycle: acquire, hold, hand off, release =="
LB="$WORK/box-lock"; mkbox "$LB"; genpins "$LB" "$WORK/pins-l" handshake
LTAG="lifecycle-$$"
sed -i.bak "s|^WP_WINDOW_TAG=.*|WP_WINDOW_TAG=$LTAG|" "$WORK/pins-l"
LLOCKD="$LB/box.lock.d"

# A passing gate must LEAVE the lock held — that is the handoff, and the whole point of
# acquire-and-hold over check-and-hope.
rm -rf "$LB/att"
bash "$GATE" --pins "$WORK/pins-l" > "$WORK/l1.out" 2>&1; LRC=$?
[ "$LRC" = "0" ] && ok "gate passes and exits 0" || { bad "lock-lifecycle gate rc=$LRC"; tail -20 "$WORK/l1.out"; }
[ -d "$LLOCKD" ] && ok "the lock is STILL HELD after a passing gate (handoff)" || bad "lock not held after a pass"
grep -q "^tag=$LTAG$" "$LLOCKD/holder" 2>/dev/null \
  && ok "the holder record carries this session's tag" || bad "holder tag missing/wrong"
grep -q "BOX LOCK IS HELD" "$WORK/l1.out" \
  && ok "the gate tells the operator the lock is held and how to release it" || bad "no handoff message"

# A SECOND gate against a held lock must be refused — single-flight, enforced.
bash "$GATE" --pins "$WORK/pins-l" --pin "WP_OUT=$LB/att2" > "$WORK/l2.out" 2>&1; LRC=$?
[ "$LRC" = "3" ] && ok "a second gate against a held lock is refused (=> 3): single-flight ENFORCED" \
  || { bad "second gate rc=$LRC, want 3"; tail -10 "$WORK/l2.out"; }
[ -d "$LLOCKD" ] && ok "the refused second gate did not disturb the first session's lock" || bad "second gate removed the lock"

# A release with the WRONG tag must refuse and leave the lock alone.
bash "$GATE" --pins "$WORK/pins-l" --pin "WP_WINDOW_TAG=someone-else" --release > "$WORK/l3.out" 2>&1; LRC=$?
[ "$LRC" != "0" ] && [ -d "$LLOCKD" ] \
  && ok "a release with a foreign tag REFUSES and leaves the lock held" \
  || { bad "foreign-tag release rc=$LRC, lock present=$([ -d "$LLOCKD" ] && echo yes || echo no)"; }
grep -q "not .*$LTAG\|REFUSED" "$WORK/l3.out" && ok "the foreign-tag refusal names the real holder" || bad "no refusal diagnostic"

# The rightful release reloads and unlocks, and records it.
bash "$GATE" --pins "$WORK/pins-l" --release > "$WORK/l4.out" 2>&1; LRC=$?
[ "$LRC" = "0" ] && ok "the rightful --release exits 0" || { bad "release rc=$LRC"; tail -10 "$WORK/l4.out"; }
[ ! -e "$LLOCKD" ] && ok "--release actually released the lock" || bad "lock still present after release"
[ "$(jq -r .verdict "$LB/att/window-release.json" 2>/dev/null)" = "released" ] \
  && ok "window-release.json records verdict=released" || bad "release record missing/wrong"
[ -n "$(jq -r .released_utc "$LB/att/window-release.json" 2>/dev/null)" ] \
  && ok "window-release.json records the box-clock release timestamp" || bad "no released_utc"

# Releasing when nothing is held is a benign no-op, not an error.
bash "$GATE" --pins "$WORK/pins-l" --release > "$WORK/l5.out" 2>&1; LRC=$?
[ "$LRC" = "0" ] && grep -q "not held" "$WORK/l5.out" \
  && ok "releasing an unheld lock is a no-op, not an error" || bad "unheld release rc=$LRC"

# A gate that FAILS after acquiring must unwind: no lock left behind.
bash "$GATE" --pins "$WORK/pins-l" --pin "WP_OUT=$LB/att3" \
     --pin "WP_ENGINE_BIN=$LB/bin/worker-dies" \
     --pin "WP_ENGINE_BIN_SHA256=$(sha_of "$LB/bin/worker-dies")" > "$WORK/l6.out" 2>&1; LRC=$?
if [ -x "$BENCHCTL" ]; then
  [ "$LRC" = "8" ] && ok "a smoke failure under the lock exits 8" || { bad "smoke-fail rc=$LRC, want 8"; tail -12 "$WORK/l6.out"; }
  [ ! -e "$LLOCKD" ] \
    && ok "the trap UNWOUND: a failed gate leaves no lock behind (never locked-and-unloaded)" \
    || bad "a failed gate left the lock held"
else skip "unwind test needs a built benchctl"; fi
echo ""

# ---------------------------------------------------------------------------
echo "== 10. driver holder-tag inheritance (run-paired-window.sh) =="
# The gate holds THE box lock through the window, so the driver must run UNDER a lock the gate took
# on its behalf. INHERIT-ONLY (bench#143 MEDIUM): the driver NEVER self-acquires — it inherits a
# live same-tag holder, aborts on a foreign holder, and REFUSES when the lock is FREE (a stale
# attestation replayed after the gate's window ended). These cases eval the REAL lock block
# extracted from the driver — not a copy — so the test cannot drift from the shipped lines.
DRV="$HERE/run-paired-window.sh"
DBLK="$WORK/drv-lock-block.sh"
sed -n '/^if \[ -d "\$BOX_LOCK" \]/,/^fi$/p' "$DRV" > "$DBLK"
[ -s "$DBLK" ] && grep -q 'WP_WINDOW_TAG' "$DBLK" \
  && ok "extracted the driver's real BOX_LOCK block ($(wc -l < "$DBLK" | tr -d ' ') lines)" \
  || bad "could not extract the driver's lock block"

# drv <lockdir> <tag-env-or-empty> -> prints "INHERITED" / "ABORT:<rc>" / "UNKNOWN:<out>"
drv() {
  local out rc
  out="$(BOX_LOCK="$1" WP_WINDOW_TAG="$2" bash -c '
    log() { printf "%s\n" "$*"; }
    . /dev/stdin
  ' < "$DBLK" 2>&1)"; rc=$?
  if [ "$rc" != "0" ]; then printf 'ABORT:%s' "$rc"; return 0; fi
  if printf '%s' "$out" | grep -q 'INHERITED'; then printf 'INHERITED'
  else printf 'UNKNOWN:%s' "$out"; fi
}

DL="$WORK/drvlock.d"

# (1) No gate-held lock to inherit (standalone / stale replay): REFUSES, never self-acquires.
rm -rf "$DL"
[ "$(drv "$DL" "")" = "ABORT:3" ] \
  && ok "driver with NO gate-held lock REFUSES (inherit-only; never self-acquires)" || bad "driver did not refuse a free lock (self-acquired?)"
[ ! -d "$DL" ] \
  && ok "driver left NO BOX_LOCK behind when it refused (no self-acquire)" || bad "driver created a BOX_LOCK despite having none to inherit"

# (2) A foreign lock, no matching tag: aborts 3.
rm -rf "$DL"; mkdir -p "$DL"; printf 'tag=someone-else\n' > "$DL/holder"; printf '4242\n' > "$DL/pid"
[ "$(drv "$DL" "")" = "ABORT:3" ] \
  && ok "driver against a FOREIGN existing lock ABORTS (3)" || bad "foreign-lock abort changed"

# (3) Gated, MATCHING tag: INHERITS without acquiring.
rm -rf "$DL"; mkdir -p "$DL"; printf 'tag=my-window\npid=1\n' > "$DL/holder"
[ "$(drv "$DL" "my-window")" = "INHERITED" ] \
  && ok "driver under a MATCHING holder tag INHERITS and does not acquire" || bad "matching tag did not inherit"

# (4) Gated, FOREIGN tag: still aborts.
rm -rf "$DL"; mkdir -p "$DL"; printf 'tag=someone-else\npid=1\n' > "$DL/holder"
[ "$(drv "$DL" "my-window")" = "ABORT:3" ] \
  && ok "driver under a FOREIGN holder tag still ABORTS (3)" || bad "foreign tag did not abort"

# (5) Tag set but lock has NO holder file: aborts (an untagged lock is not ours to inherit).
rm -rf "$DL"; mkdir -p "$DL"; printf '4242\n' > "$DL/pid"
[ "$(drv "$DL" "my-window")" = "ABORT:3" ] \
  && ok "driver aborts on a lock with no holder tag (an untagged lock is never inherited)" \
  || bad "untagged lock was wrongly inherited"

# (6) INHERIT-ONLY: the driver never self-acquires and never releases the inherited lock.
grep -q 'mkdir "\$BOX_LOCK"' "$DRV" \
  && bad "driver still SELF-ACQUIRES BOX_LOCK via mkdir (inherit-only violated)" \
  || ok "driver never self-acquires BOX_LOCK (mkdir path removed)"
grep -q 'rmdir "\$BOX_LOCK"' "$DRV" \
  && bad "driver still rmdir's the inherited BOX_LOCK (release is the gate --release's job)" \
  || ok "driver never releases the inherited BOX_LOCK (that remains --release's job)"

# (7) The flock is still this driver's own, taken unconditionally.
grep -q 'parity_take_gpu_lock' "$DRV" && ok "driver still takes the flock (its own dialect, unchanged)" || bad "flock acquisition lost"

# (8) No sibling driver has an abort-if-lock-exists check to update.
SIB=0
for d in run-official-window.sh run-manual-test.sh run-variant-window.sh; do
  grep -q 'mkdir "\$BOX_LOCK"' "$HERE/$d" 2>/dev/null && SIB=$((SIB+1))
done
[ "$SIB" = "0" ] \
  && ok "no sibling driver takes BOX_LOCK (they use the fd-scoped flock only) — nothing else to update" \
  || bad "$SIB sibling driver(s) take BOX_LOCK and need the same inheritance edit"
echo ""

# ---------------------------------------------------------------------------
echo "== 11. red-team regressions (each of these SHIPPED broken) =="

# --- B1: a present-but-empty request key must reach its default --------------
# The laptop emits every key unconditionally, so an unset pin arrives as `key=`. The old req()
# returned on first match, so the fallback was dead for every key — WP_QWEN_UNLOAD_TRIES came
# back "", the poll never ran, and the gate refused EVERY real box with exit 6.
RQ="$(printf 'qwen_unload_tries=\nmode=%s\n' "$(printf observe | base64)" | base64 | tr -d '\n')"
REQOUT="$(REQ="$(printf '%s' "$RQ" | base64 "$B64D")" WPPROBE="$PROBE" bash -c '
  _B64D="'"$B64D"'"
  _unb64() { printf "%s" "$1" | base64 "$_B64D" 2>/dev/null; }
  eval "$(sed -n "/^req() {/,/^}/p;/^req_int() {/,/^}/p" "$WPPROBE")"
  printf "tries=%s" "$(req_int qwen_unload_tries 12)"
')"
[ "$REQOUT" = "tries=12" ] \
  && ok "B1: a PRESENT-but-EMPTY request key falls through to its default (was: empty => rc 6 on every real box)" \
  || bad "B1: req_int returned '$REQOUT', want tries=12"
# And the default path is now actually exercised end-to-end: a REAL qwen-service fixture with
# WP_QWEN_UNLOAD_TRIES left unpinned. Every earlier fixture pinned WP_QWEN_SERVICE=none, which
# is exactly why the suite never caught this.
RB="$WORK/box-b1"; mkbox "$RB"; genpins "$RB" "$WORK/pins-b1" handshake
grep -q '^WP_QWEN_UNLOAD_TRIES=' "$WORK/pins-b1" && bad "B1 fixture pins the tries (it must not)" \
  || ok "B1 fixture leaves WP_QWEN_UNLOAD_TRIES at its documented default"
gate "$WORK/pins-b1"
[ "$G_RC" = "0" ] && ok "B1: a real qwen-service with the DEFAULT unload-tries passes (was: always exit 6)" \
  || { bad "B1 end-to-end rc=$G_RC"; tail -8 "$WORK/gate.out"; }

# --- B3: a bundle clone's origin is the BUNDLE PATH, not a remote -------------
B3B="$WORK/box-b3"; mkbox "$B3B"
git -C "$B3B/engine" bundle create "$WORK/e.bundle" --all >/dev/null 2>&1
E3SHA="$(git -C "$B3B/engine" rev-parse HEAD)"
rm -rf "$B3B/engine-fromb"; git clone -q "$WORK/e.bundle" "$B3B/engine-fromb"
git -C "$B3B/engine-fromb" checkout -q --detach "$E3SHA"
[ "$(git -C "$B3B/engine-fromb" config --get remote.origin.url)" = "$WORK/e.bundle" ] \
  && ok "B3 fixture: a REAL bundle clone sets origin to the bundle FILE PATH (non-empty)" \
  || bad "B3 fixture did not reproduce the bundle-origin shape"
genpins "$B3B" "$WORK/pins-b3" handshake
{ grep -v '^WP_ENGINE_PATH=\|^WP_ENGINE_SHA=' "$WORK/pins-b3"
  printf 'WP_ENGINE_PATH=%s\n' "$B3B/engine-fromb"
  printf 'WP_ENGINE_SHA=%s\n' "$E3SHA"
} > "$WORK/pins-b3b"
gate "$WORK/pins-b3b"
expect "B3: a REAL bundle-clone tree with no record is REFUSED (was: PASSED as cloned-from-a-remote)" \
  7 ENGINE.bundle REFUSED "not a remote URL"

# --- M4: the bundle marker is self-attested; grading must say so --------------
GD="$(git -C "$B3B/engine-fromb" rev-parse --absolute-git-dir)"
printf 'bundle_sha256=%s\nbundle_path=%s\ncommit=%s\n' \
  "$(sha_of "$WORK/e.bundle")" "$WORK/e.bundle" "$E3SHA" > "$GD/window-bundle-provenance"
gate "$WORK/pins-b3b" --pin "WP_ENGINE_BUNDLE_SHA256=$(sha_of "$WORK/e.bundle")"
expect "M4: bundle STILL on the box => re-derived from its own bytes, graded PASS" \
  0 ENGINE.bundle PASS ""
jq -r '.items[]|select(.id=="ENGINE.bundle")|.observed' "$G_ATT" | grep -q "one of its heads" \
  && ok "M4: the VERIFIED grading states the commit was found among the bundle's heads" \
  || bad "M4: verified grading does not name the head check"
mv "$WORK/e.bundle" "$WORK/e.bundle.moved"
gate "$WORK/pins-b3b" --pin "WP_ENGINE_BUNDLE_SHA256=$(sha_of "$WORK/e.bundle.moved")"
[ "$(av ENGINE.bundle)" = "CLAIMED" ] \
  && ok "M4: bundle GONE => graded CLAIMED, not PASS (self-attested marker is not proof)" \
  || bad "M4: expected CLAIMED, got '$(av ENGINE.bundle)'"
mv "$WORK/e.bundle.moved" "$WORK/e.bundle"
# A marker naming a different commit than HEAD is incoherent whatever its hash says.
printf 'bundle_sha256=%s\nbundle_path=%s\ncommit=%s\n' \
  "$(sha_of "$WORK/e.bundle")" "$WORK/e.bundle" "$BOGUS40" > "$GD/window-bundle-provenance"
gate "$WORK/pins-b3b" --pin "WP_ENGINE_BUNDLE_SHA256=$(sha_of "$WORK/e.bundle")"
expect "M4: a marker naming a commit != HEAD is REFUSED" 7 ENGINE.bundle REFUSED "different commit"

# --- M6: assume-unchanged hides a modification from --porcelain ---------------
M6B="$WORK/box-m6"; mkbox "$M6B"; genpins "$M6B" "$WORK/pins-m6" handshake
printf 'tracked\n' > "$M6B/bench/f.txt"
git -C "$M6B/bench" add f.txt
git -C "$M6B/bench" -c user.email=t@t -c user.name=t commit -q -m addf
sed -i.bak "s|^WP_BENCH_SHA=.*|WP_BENCH_SHA=$(git -C "$M6B/bench" rev-parse HEAD)|" "$WORK/pins-m6"
git -C "$M6B/bench" update-index --assume-unchanged f.txt
printf 'SECRETLY MODIFIED\n' > "$M6B/bench/f.txt"
[ -z "$(git -C "$M6B/bench" status --porcelain)" ] \
  && ok "M6 fixture: assume-unchanged really does hide the edit from status --porcelain" \
  || bad "M6 fixture did not reproduce the hidden modification"
gate "$WORK/pins-m6"
expect "M6: a tree with assume-unchanged/skip-worktree files FAILS the clean check" \
  1 BENCH.clean FAIL "HIDE modifications"

# --- B4: the gate must refuse when the REAL box lock is held by a live run ----
B4B="$WORK/box-b4"; mkbox "$B4B"; genpins "$B4B" "$WORK/pins-b4" handshake
BOXLOCK4="$(grep '^WP_BOX_LOCK=' "$WORK/pins-b4" | sed 's/^WP_BOX_LOCK=//')"
mkdir -p "$BOXLOCK4"; printf '%s\n' "$$" > "$BOXLOCK4/pid"
printf 'tag=a-live-window\npid=%s\n' "$$" > "$BOXLOCK4/holder"
gate "$WORK/pins-b4"
expect "B4: a LIVE holder of the REAL box lock refuses the gate (old head observed the wrong path entirely)" \
  3 boxlock FAIL "held by a running process"
[ -d "$BOXLOCK4" ] && ok "B4: the live run's lock was left untouched" || bad "B4: the gate disturbed a live lock"
rm -rf "$BOXLOCK4"
# And the pin the gate observes IS the pin it acquires — one lock, end to end.
gate "$WORK/pins-b4"
[ "$(jq -r '.lock.path' "$G_ATT")" = "$BOXLOCK4" ] \
  && ok "B4: the lock the gate ACQUIRES is the same WP_BOX_LOCK path it OBSERVED" \
  || bad "B4: acquired path $(jq -r '.lock.path' "$G_ATT") != observed $BOXLOCK4"

# --- B-5: a create-failure is not contention ---------------------------------
B5B="$WORK/box-b5"; mkbox "$B5B"; genpins "$B5B" "$WORK/pins-b5" handshake
gate "$WORK/pins-b5" --pin "WP_BOX_LOCK=$B5B/no-such-parent/deep/lock.d"
expect "B-5: an unwritable/missing-parent lock path reports CREATE FAILURE, not 'another session'" \
  5 lock.acquired FAIL "unrelated to contention"

# --- A-2: --release must not claim success when the model did not come back ---
A2B="$WORK/box-a2"; mkbox "$A2B"; genpins "$A2B" "$WORK/pins-a2" handshake
A2TAG="$(cat "$A2B/.qwen-tag")"
cat > "$A2B/qwen-service.sh" <<QF
qwen_unload() { pkill -f '$A2TAG' >/dev/null 2>&1; sleep 0.3; return 0; }
qwen_reload() { echo "launchctl: throttled" >&2; return 1; }
QF
sed -i.bak "s|^WP_QWEN_RELOAD_TRIES=.*||" "$WORK/pins-a2"
printf 'WP_QWEN_RELOAD_TRIES=2\n' >> "$WORK/pins-a2"
bash "$GATE" --pins "$WORK/pins-a2" >/dev/null 2>&1
bash "$GATE" --pins "$WORK/pins-a2" --release > "$WORK/a2.out" 2>&1; A2RC=$?
[ "$A2RC" != "0" ] \
  && ok "A-2: --release EXITS NONZERO when the serving model did not come back (was: exit 0, 'OK')" \
  || { bad "A-2: release exited 0 with qwen down"; tail -6 "$WORK/a2.out"; }
grep -q "NOT SERVING" "$WORK/a2.out" \
  && ok "A-2: and it SAYS the box is free but not serving" || bad "A-2: no serving-down warning"

# --- A-3: ssh flattens argv; empty positionals vanish -------------------------
# Emulate the flattening ssh performs: join argv with spaces, re-split through a shell. The old
# per-arg base64 scheme lost every empty field and shifted the rest — turning --dry-run into a
# REAL run and making the bundle branch unreachable, all at exit 0.
cat > "$WORK/flat.sh" <<'FL'
#!/bin/bash
# stand-in for `ssh host bash -s -- "$@"`: ssh joins argv into ONE string the remote re-splits.
joined="$*"
# shellcheck disable=SC2086
bash -s -- $joined
FL
chmod +x "$WORK/flat.sh"
NARGS_FLAT="$(printf 'a\n' | bash -c 'j="A  C"; set -- $j; echo $#')"
[ "$NARGS_FLAT" = "2" ] \
  && ok "A-3 fixture: ssh-style flattening really does collapse an empty positional (3 args -> 2)" \
  || bad "A-3 fixture did not reproduce argv flattening"
# The payload must now survive that: ONE packed argument, empties preserved as empty lines.
PACKED="$(printf '%s\n' "$(printf 'p1' | base64)" "" "$(printf 'p3' | base64)" | base64 | tr -d '\n')"
UNPACK_OUT="$(printf '%s' "$PACKED" | base64 "$B64D" | wc -l | tr -d ' ')"
[ "$UNPACK_OUT" = "3" ] \
  && ok "A-3: the packed envelope preserves an EMPTY middle field (3 lines survive)" \
  || bad "A-3: packed envelope lost the empty field ($UNPACK_OUT lines)"
# A-3's packing is proved BEHAVIOURALLY in section 15, where both the bundle fixture and the
# flattening ssh emulator exist — an EMPTY `remote` field crossing the real transport.

# --- NEW-2: a newline in a weights filename ----------------------------------
# `find -print | while read` split such a name into two bogus paths; sha_of returned empty and
# the empty-hash branch blanked the digest WITHOUT setting weights.error, so the gate reported
# drift/tamper for a tree the Rust dir_digest walks without complaint — a silent WRONG verdict.
NLB="$WORK/box-nl"; mkbox "$NLB"
NLNAME="$(printf 'shard\nb.bin')"
printf 'weightdata' > "$NLB/weights/$NLNAME" 2>/dev/null && NL_MADE=1 || NL_MADE=0
if [ "$NL_MADE" = "1" ]; then
  genpins "$NLB" "$WORK/pins-nl" handshake
  gate "$WORK/pins-nl"
  if [ "$G_RC" = "0" ]; then
    ok "NEW-2: a weights tree with a NEWLINE in a filename digests correctly (no false drift)"
  else
    WV="$(av weights.sha256) $(av weights.readable)"
    case "$WV" in
      *FAIL*) bad "NEW-2: newline filename still produces a drift/tamper verdict ($WV)" ;;
      *)      bad "NEW-2: newline filename gate rc=$G_RC" ;;
    esac
  fi
  # The digest must be the real one, not an empty string that happened to match.
  NLSHA="$(ao weights.sha256)"
  case "$NLSHA" in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f]*)
      ok "NEW-2: and the observed weights digest is a real hash, not the empty string" ;;
    *)  bad "NEW-2: observed weights digest for the newline tree is '$NLSHA'" ;;
  esac
  # The newline file must actually be IN the digest — a walk that silently dropped it would also
  # produce a clean-looking hash.
  NLFC="$(ao weights.file_count)"
  # Count NUL-safely — `find | wc -l` would itself miscount the newline-named file, which is the
  # exact confusion this section exists to rule out.
  NLREAL="$(find -L "$NLB/weights" -type f -print0 | tr -dc '\000' | wc -c | tr -d ' ')"
  [ "$NLFC" = "$NLREAL" ] \
    && ok "NEW-2: and the newline-named file is COUNTED, not silently skipped ($NLFC files)" \
    || bad "NEW-2: file_count $NLFC != $NLREAL actual files (newline file dropped from the walk)"
else
  bad "NEW-2 fixture: could not create a newline filename on this filesystem"
fi

# --- B-1 (secret tier): no raw hostname anywhere in the attestation -----------
gate "$PINS"
if jq -r '.. | strings' "$G_ATT" 2>/dev/null | grep -qiF "$(hostname -s 2>/dev/null || hostname)"; then
  bad "B-1: the attestation contains the raw hostname"
else ok "B-1: the attestation carries NO raw hostname (alias only)"; fi
jq -r '.box.uname' "$G_ATT" | grep -qiF "$(hostname -s 2>/dev/null || hostname)" \
  && bad "B-1: box.uname still embeds the hostname" \
  || ok "B-1: box.uname is uname -srm (no hostname)"

# --- B-6: --pin overrides must be sealed --------------------------------------
gate "$PINS" --pin "WP_MAX_LOADAVG=8888"
jq -r '.gate.cli_overrides[]?' "$G_ATT" 2>/dev/null | grep -q 'WP_MAX_LOADAVG=8888' \
  && ok "B-6: --pin overrides are sealed into gate.cli_overrides" || bad "B-6: overrides not sealed"
EFF_OVR="$(jq -r '.gate.effective_pins_sha256' "$G_ATT")"
FILE_OVR="$(jq -r '.gate.pins_file_sha256' "$G_ATT")"
# N-a: comparing effective vs FILE hash proves nothing — the effective hash is taken over the
# comment-stripped text, so the two differ even with zero overrides and the assertion passed
# vacuously. The real property is that the effective hash MOVES when argv changes and the file
# hash does NOT, so compare the same gate run twice.
gate "$PINS"
EFF_NONE="$(jq -r '.gate.effective_pins_sha256' "$G_ATT")"
FILE_NONE="$(jq -r '.gate.pins_file_sha256' "$G_ATT")"
[ "$EFF_OVR" != "$EFF_NONE" ] \
  && ok "B-6: effective_pins_sha256 CHANGES when argv overrides a pin (and not otherwise)" \
  || bad "B-6: effective pins hash is identical with and without the override ($EFF_NONE)"
[ "$FILE_OVR" = "$FILE_NONE" ] && [ -n "$FILE_NONE" ] \
  && ok "B-6: and pins_file_sha256 stays fixed — it hashes the FILE, not the argv" \
  || bad "B-6: pins_file_sha256 moved with argv ($FILE_OVR vs $FILE_NONE)"

# --- M7: an unwind must leave its own record ----------------------------------
if [ -x "$BENCHCTL" ]; then
  M7B="$WORK/box-m7"; mkbox "$M7B"; genpins "$M7B" "$WORK/pins-m7" handshake
  bash "$GATE" --pins "$WORK/pins-m7" \
    --pin "WP_ENGINE_BIN=$M7B/bin/worker-dies" \
    --pin "WP_ENGINE_BIN_SHA256=$(sha_of "$M7B/bin/worker-dies")" >/dev/null 2>&1
  M7ATT="$M7B/att/window-provenance.json"
  [ -f "$M7B/att/window-unwind.json" ] \
    && ok "M7: a failed-under-lock run writes window-unwind.json" || bad "M7: no unwind record"
  [ "$(jq -r '.release_verdict' "$M7B/att/window-unwind.json" 2>/dev/null)" = "released" ] \
    && ok "M7: the unwind record states the release outcome" || bad "M7: unwind verdict missing"
  [ "$(jq -r '.lock.state' "$M7ATT" 2>/dev/null)" != "held" ] \
    && ok "M7: the attestation does NOT seal state=held on an unwound run (was: always held/null)" \
    || bad "M7: attestation still claims held after unwinding"
  [ -n "$(jq -r '.lock.released_utc // empty' "$M7ATT" 2>/dev/null)" ] \
    && ok "M7: and it carries the release timestamp" || bad "M7: released_utc still null after unwind"
fi

# --- M8: a waived smoke leg STILL acquires and holds ---------------------------
M8B="$WORK/box-m8"; mkbox "$M8B"; genpins "$M8B" "$WORK/pins-m8" handshake
bash "$GATE" --pins "$WORK/pins-m8" --no-smoke > "$WORK/m8.out" 2>&1; M8RC=$?
M8ATT="$M8B/att/window-provenance.json"
[ "$M8RC" = "0" ] && ok "M8: --no-smoke passes" || { bad "M8 rc=$M8RC"; tail -6 "$WORK/m8.out"; }
[ "$(jq -r '.lock.state' "$M8ATT" 2>/dev/null)" = "held" ] \
  && ok "M8: --no-smoke STILL ACQUIRES AND HOLDS the lock (acquisition is not waivable)" \
  || bad "M8: --no-smoke left the box unlocked ($(jq -r '.lock.state' "$M8ATT" 2>/dev/null))"
[ "$(jq -r '.smoke.verdict' "$M8ATT" 2>/dev/null)" = "DECLARED-WAIVED" ] \
  && ok "M8: and the smoke waiver is recorded" || bad "M8: waiver not recorded"
bash "$GATE" --pins "$WORK/pins-m8" --release >/dev/null 2>&1

# --- B-2: dir_digest divergences ------------------------------------------------
DD2="$WORK/dd2"; mkdir -p "$DD2"
printf 'x' > "$DD2/Xgitkeep"              # BRE '.gitkeep' would match this
printf 'y' > "$DD2/0benchmark-source.sha256"
bash "$PROBE" "$(mkreq "weights_path=$DD2")" > "$WORK/dd2.rec" 2>/dev/null
[ "$(probe_obs "$WORK/dd2.rec" weights.file_count)" = "2" ] \
  && ok "B-2: the ignore rule is FIXED-STRING — 'Xgitkeep' and '0benchmark-source.sha256' are counted" \
  || bad "B-2: BRE dot still swallows near-miss filenames (count=$(probe_obs "$WORK/dd2.rec" weights.file_count))"
DD3="$WORK/dd3"; mkdir -p "$DD3"; printf 'a' > "$DD3/real"
ln -s "$DD3/nowhere" "$DD3/broken"
bash "$PROBE" "$(mkreq "weights_path=$DD3")" > "$WORK/dd3.rec" 2>/dev/null
[ "$(probe_obs "$WORK/dd3.rec" weights.error)" = "broken-symlink" ] \
  && ok "B-2: a BROKEN SYMLINK is an error (benchd errors too), not a silently skipped file" \
  || bad "B-2: broken symlink was skipped rather than raised"
gate "$PINS" --pin "WP_WEIGHTS_PATH=$DD3"
expect "B-2: and the gate FAILS on it rather than digesting a different tree than benchd would" \
  1 weights.readable FAIL "cannot be digested"
echo ""

# ---------------------------------------------------------------------------
# NOTE: the fixture box alias is deliberately NOT a real one from anybody's ssh config. Every
# driver invocation below prepends a stub `ssh`/`scp` to PATH, but a fixture that names a
# routable host is one PATH mistake away from connecting to a machine someone else is using.
# The property under test — that the attestation records the ALIAS and never the hostname — does
# not care which alias it is.
echo "== 12. the ssh DRIVER path (stub ssh/scp, no network) =="
# Every earlier case ran --driver local, so the ssh envelope — argv packing, stdin-piped probe,
# base64 attestation delivery, the release round trip — was never executed offline. A stub
# `ssh` that ignores the host and runs `bash -s` locally exercises exactly that code path.
SSHBIN="$WORK/sshbin"; mkdir -p "$SSHBIN"
cat > "$SSHBIN/ssh" <<'SEOF'
#!/bin/bash
# stand-in for ssh: drop the host, run the rest. `bash -s` reads the script from OUR stdin,
# exactly as the real thing does.
# NEW-5: real ssh does NOT preserve argv. It joins its command words with spaces and hands the
# string to the remote login shell, which re-splits on whitespace — which is exactly why an empty
# positional vanishes and every later argument shifts down one. The old stub had an
# `exec bash "$@"` fast path that preserved argv perfectly, so it could not have caught that, and
# the only thing testing the packing was a grep for a literal line of SOURCE TEXT — a phrase
# needle that passes whether or not the code works. Flatten the way ssh actually does; the
# packing is then proved by whether the box-side work comes out right.
shift                      # host
case "$*" in
  # `$*` is the flattening — argv is joined and re-split exactly as ssh does, so an empty
  # positional still disappears. The inner `exec` then makes the command REPLACE this shell
  # rather than run as its child, which is what keeps a signal sent to "ssh" landing on the
  # payload (real ssh tears the connection down and the remote takes a HUP). Only bash commands
  # are exec-prefixed: the box-side one-liners contain `;` and `&&`, which `exec` cannot take.
  bash\ *) exec bash -c "exec $*" ;;
  *)       exec bash -c "$*" ;;
esac
SEOF
chmod +x "$SSHBIN/ssh"
SB="$WORK/box-ssh"; mkbox "$SB"; genpins "$SB" "$WORK/pins-ssh" handshake
{ grep -v '^WP_DRIVER=\|^WP_BOX=\|^WP_BOX_OUT=' "$WORK/pins-ssh"
  printf 'WP_DRIVER=ssh\nWP_BOX=offline-fixture-box\nWP_BOX_OUT=%s\n' "$SB/out"
} > "$WORK/pins-ssh2"
SSH_RC=0
PATH="$SSHBIN:$PATH" bash "$GATE" --pins "$WORK/pins-ssh2" > "$WORK/ssh.out" 2>&1 || SSH_RC=$?
[ "$SSH_RC" = "0" ] \
  && ok "the ssh driver path runs end-to-end (probe piped over stdin, argv packed)" \
  || { bad "ssh-driver gate rc=$SSH_RC"; tail -12 "$WORK/ssh.out"; }
SATT="$SB/att/window-provenance.json"
[ "$(jq -r '.gate.driver' "$SATT" 2>/dev/null)" = "ssh" ] \
  && ok "the attestation records driver=ssh" || bad "driver not recorded as ssh"
[ "$(jq -r '.box.alias' "$SATT" 2>/dev/null)" = "offline-fixture-box" ] \
  && ok "the attestation records the box ALIAS (never a hostname)" || bad "box alias not recorded"
# M9: the attestation must actually LAND, and a failure to land must be reported, not swallowed.
[ -f "$SB/out/window-provenance.json" ] \
  && ok "M9: the attestation reached the box dir over the quote-safe base64 transfer" \
  || bad "M9: the attestation did not land on the box"
[ "$(jq -r '.schema' "$SB/out/window-provenance.json" 2>/dev/null)" = "window-provenance/v1" ] \
  && ok "M9: and it arrived intact (parses, right schema)" || bad "M9: delivered file is corrupt"
# A box dir containing shell metacharacters must survive the transfer unmangled.
WEIRD="$SB/out/dir with 'quotes' and \$dollar"
{ grep -v '^WP_BOX_OUT=' "$WORK/pins-ssh2"; printf 'WP_BOX_OUT=%s\n' "$WEIRD"; } > "$WORK/pins-ssh3"
PATH="$SSHBIN:$PATH" bash "$GATE" --pins "$WORK/pins-ssh3" >/dev/null 2>&1
[ -f "$WEIRD/window-provenance.json" ] \
  && ok "M9: a box path with quotes and \$dollar survives the transfer" \
  || bad "M9: quoted box path mangled the delivery"
PATH="$SSHBIN:$PATH" bash "$GATE" --pins "$WORK/pins-ssh2" --release >/dev/null 2>&1
[ ! -e "$SB/box.lock.d" ] && ok "the ssh-driver release path releases the lock" || bad "ssh release left the lock"

# C-1: a duplicate key in the pins file is silently last-wins — warn about it.
cp "$PINS" "$WORK/pins-dupe"; printf 'WP_MAX_LOADAVG=7777\n' >> "$WORK/pins-dupe"
bash "$GATE" --pins "$WORK/pins-dupe" --no-smoke >/dev/null 2>"$WORK/dupe.err"
grep -q 'WP_MAX_LOADAVG' "$WORK/dupe.err" && grep -q 'more than once' "$WORK/dupe.err" \
  && ok "C-1: a duplicated pins key is warned about by name" || bad "C-1: no duplicate-key warning"
bash "$GATE" --pins "$WORK/pins-dupe" --release >/dev/null 2>&1
echo ""

# ---------------------------------------------------------------------------
echo "== 13. CONCURRENCY: exactly one winner, reapable lock (N-1) =="
# The defect this exists to catch is invisible to every single-process case in this file: two
# probes deciding "reapable" from the same pre-acquisition snapshot BOTH proceeded, the loser
# deleting the pid/holder of a lock the winner had legitimately created in between. Multiple
# winners, each sealing verified_dead_how for a lock taken from a LIVE peer.
CONCLOCK="$WORK/conc.lock.d"
conc_reset_reapable() {   # a provably-dead, old-enough lock — the reap path
  rm -rf "$CONCLOCK" "$CONCLOCK".reaped.* "$CONCLOCK".reapmutex 2>/dev/null
  mkdir -p "$CONCLOCK"
  printf '999999\n' > "$CONCLOCK/pid"
  printf 'tag=prior-session\npid=999999\nuser=someone\nacquired_utc=2026-08-01T00:00:00Z\n' \
    > "$CONCLOCK/holder"
  touch -t 202001010000 "$CONCLOCK"
}
conc_reset_free() { rm -rf "$CONCLOCK" "$CONCLOCK".reaped.* "$CONCLOCK".reapmutex 2>/dev/null; }
conc_trial() {            # <n-probes> -> prints how many reported lock.acquired=1
  # NEVER a bare `wait` here. This function runs inside a command substitution, and a bare
  # `wait` there tries to reap every job entry the subshell inherited — including the long-lived
  # serving-model stand-ins the box fixtures start. Those are not children of this subshell, so
  # `wait` spun on one reporting "pid N is not a child of this shell" and wrote a 21 GB log
  # before filling the disk. Waiting on the pids we actually started is both correct and immune
  # to whatever else the parent has running.
  local n="$1" i=0 cnt=0 pids="" pid
  while [ "$i" -lt "$n" ]; do
    bash "$PROBE" "$(mkreq "mode=window" "session_lock=$CONCLOCK" "window_tag=conc-$i" \
      "qwen_service=none" "lock_reap_age_s=60")" > "$WORK/conc.$i.rec" 2>/dev/null &
    pids="$pids $!"
    i=$((i + 1))
  done
  for pid in $pids; do wait "$pid" 2>/dev/null; done
  i=0
  while [ "$i" -lt "$n" ]; do
    [ "$(probe_obs "$WORK/conc.$i.rec" lock.acquired)" = "1" ] && cnt=$((cnt + 1))
    i=$((i + 1))
  done
  printf '%s' "$cnt"
}

TRIALS=25; BAD=0; WORSTN=0
t=0
while [ "$t" -lt "$TRIALS" ]; do
  conc_reset_reapable
  c="$(conc_trial 3)"
  [ "$c" = "1" ] || { BAD=$((BAD + 1)); [ "$c" -gt "$WORSTN" ] && WORSTN="$c"; }
  t=$((t + 1))
done
[ "$BAD" -eq 0 ] \
  && ok "N-1: 3-way against a REAPABLE lock — exactly one winner in $TRIALS/$TRIALS trials" \
  || bad "N-1: $BAD/$TRIALS trials had != 1 winner (worst: $WORSTN simultaneous holders)"

# The control the reviewer ran: with no prior lock the pure acquire was always sound, and must
# stay that way — this pins that the reap rework did not regress the common path.
BAD2=0; t=0
while [ "$t" -lt "$TRIALS" ]; do
  conc_reset_free
  c="$(conc_trial 3)"
  [ "$c" = "1" ] || BAD2=$((BAD2 + 1))
  t=$((t + 1))
done
[ "$BAD2" -eq 0 ] \
  && ok "N-1 control: 3-way with NO prior lock — exactly one winner in $TRIALS/$TRIALS trials" \
  || bad "N-1 control regressed: $BAD2/$TRIALS trials had != 1 winner"

# The 20-way stress that produced 8 simultaneous holders on the broken head.
conc_reset_reapable
c20="$(conc_trial 20)"
[ "$c20" = "1" ] \
  && ok "N-1 stress: 20-way against a reapable lock — exactly one winner (was 8)" \
  || bad "N-1 stress: $c20 winners in a 20-way race"
# And a reap that DID happen must be attributable to the winner alone.
REAPERS=0; i=0
while [ "$i" -lt 20 ]; do
  [ "$(probe_obs "$WORK/conc.$i.rec" lock.reaped)" = "1" ] && REAPERS=$((REAPERS + 1))
  i=$((i + 1))
done
[ "$REAPERS" -le 1 ] \
  && ok "N-1: at most ONE probe sealed reaped=1 ($REAPERS) — no false proofs of death (was 20)" \
  || bad "N-1: $REAPERS probes each sealed a reaped=1 for the same lock"
# NEW-4: state the invariant in its own right — a probe that did NOT acquire must seal NO
# reaped-evidence field at all. Counting `reaped=1` alone would still pass if a loser wrote a
# verified_dead_how for a lock it never held, which is exactly the false attestation the
# original non-atomic reap produced.
# NEW-4, stated as the invariant that actually matters. Note what is NOT asserted: that only the
# lock's holder may carry a reap record. A prober can legitimately move a provably-dead lock
# aside and then lose the follow-up mkdir to a bystander — the reap still happened, and saying so
# is truthful. (The probe biases against that: a bystander that sees the reap mutex held stands
# down instead of racing.) The bug this guards is the FALSE one: evidence naming a holder that
# was never proved dead, or a reap claimed by a probe that never performed the rename.
PHANTOM=0; ALIVE_CLAIM=0; i=0
while [ "$i" -lt 20 ]; do
  _r="$(probe_obs "$WORK/conc.$i.rec" lock.reaped)"
  _pp="$(probe_obs "$WORK/conc.$i.rec" lock.reaped_prior_pid)"
  _mv="$(probe_obs "$WORK/conc.$i.rec" lock.reaped_moved_to)"
  _how="$(probe_obs "$WORK/conc.$i.rec" lock.reaped_verified_dead_how)"
  if [ "$_r" = "1" ]; then
    # A reap claim must be backed by the rename that IS the reap, and by a dead-holder proof.
    { [ -n "$_mv" ] && [ -n "$_how" ] && [ -n "$_pp" ]; } || PHANTOM=$((PHANTOM + 1))
    # ...and the holder it names must be the fixture's known-dead pid, never a live process.
    [ -n "$_pp" ] && kill -0 "$_pp" 2>/dev/null && ALIVE_CLAIM=$((ALIVE_CLAIM + 1))
  else
    # No claim => no evidence. This is the "sealed a verified_dead_how for a lock it never took"
    # shape the original non-atomic reap produced.
    { [ -z "$_mv" ] && [ -z "$_how" ] && [ -z "$_pp" ]; } || PHANTOM=$((PHANTOM + 1))
  fi
  i=$((i + 1))
done
[ "$PHANTOM" = "0" ] \
  && ok "N-1: every reap record is backed by the rename that performed it; no probe carries evidence without a claim" \
  || bad "N-1: $PHANTOM probe(s) carry reap evidence that does not match their claim"
[ "$ALIVE_CLAIM" = "0" ] \
  && ok "N-1: NO probe claimed to have reaped a lock whose holder was still ALIVE" \
  || bad "N-1: $ALIVE_CLAIM probe(s) claimed a reap against a LIVE holder"
# NEW-1: contention and create-failure are DIFFERENT verdicts and must not be confused in
# either direction. B-5 fixed create-failure-read-as-contention and introduced its inverse: a
# peer releasing between our mkdir and the follow-up `[ ! -d ]` test made ordinary contention
# report as "could not be created" (exit 5) instead of contention (exit 3).
CHURNLOCK="$WORK/churn.d"
rm -rf "$CHURNLOCK" "$CHURNLOCK".reap* 2>/dev/null
# A peer that takes and releases the lock as fast as it can, so most probes meet it mid-churn.
( i=0; while [ "$i" -lt 400 ]; do mkdir "$CHURNLOCK" 2>/dev/null && { printf '%s\n' "$$" > "$CHURNLOCK/pid"; rm -rf "$CHURNLOCK"; }; i=$((i + 1)); done ) &
CHURNER=$!
MISCLASS=0; CONTENDED=0; ACQUIRED=0; i=0
while [ "$i" -lt 60 ]; do
  bash "$PROBE" "$(mkreq "mode=window" "session_lock=$CHURNLOCK" "window_tag=churn" \
    "qwen_service=none" "lock_reap_age_s=60")" > "$WORK/churn.rec" 2>/dev/null
  if [ "$(probe_obs "$WORK/churn.rec" lock.acquired)" = "1" ]; then
    ACQUIRED=$((ACQUIRED + 1)); rm -rf "$CHURNLOCK"
  elif [ "$(probe_obs "$WORK/churn.rec" lock.create_failed)" = "1" ]; then
    MISCLASS=$((MISCLASS + 1))
  else CONTENDED=$((CONTENDED + 1)); fi
  i=$((i + 1))
done
wait "$CHURNER" 2>/dev/null
rm -rf "$CHURNLOCK" "$CHURNLOCK".reap* 2>/dev/null
[ "$MISCLASS" = "0" ] \
  && ok "NEW-1: 60 probes against a CHURNING peer — 0 misclassified as create-failure (acq=$ACQUIRED, contended=$CONTENDED)" \
  || bad "NEW-1: $MISCLASS/60 reported create-failure for plain contention (was ~13/600)"

# NEW-1 (DETERMINISTIC) — MN1's real witness. The churner case above PRODUCES the misclassifying
# interleaving by racing, which misses it ~27% of the time, so a genuine NEW-1 regression could
# slip through on a lucky run. That is the same probabilistic-witness class as the F1 subshell
# carriers and the carried-forward pass count: a test that only sometimes can fail is not a test.
#
# The errno classification and a post-hoc `[ ! -d ]` stat diverge in exactly one state: mkdir
# fails because a contender EXISTS, and that contender is GONE by the moment a follow-up stat
# would run. A `mkdir` SHIM forces that state every time instead of hoping for it — it fails with
# the real EEXIST error AND removes the directory in the same breath, which IS "a peer released
# between our mkdir and our test", deterministically.
N1SHIM="$WORK/n1shim"; mkdir -p "$N1SHIM"
N1LOCK="$WORK/n1-lock.d"
cat > "$N1SHIM/mkdir" <<SHIMEOF
#!/bin/bash
# Only the lock-dir mkdir is special; every other mkdir (reap mutex, -p trees) passes straight
# through, so the shim changes exactly the one call whose classification is under test.
if [ "\$#" = "1" ] && [ "\$1" = "$N1LOCK" ]; then
  err="\$(/bin/mkdir "\$1" 2>&1)"; rc=\$?
  if [ "\$rc" -ne 0 ] && printf '%s' "\$err" | grep -qi 'exists'; then
    rm -rf "$N1LOCK" 2>/dev/null   # the contender releases, right now, deterministically
  fi
  printf '%s' "\$err" >&2
  exit \$rc
fi
exec /bin/mkdir "\$@"
SHIMEOF
chmod +x "$N1SHIM/mkdir"
# Stage a GENUINE contender: a real lock directory with a holder. mkdir will fail EEXIST on it.
rm -rf "$N1LOCK"; mkdir -p "$N1LOCK"; printf '%s\n' "$$" > "$N1LOCK/pid"
printf 'tag=peer\npid=%s\nuser=x\nacquired_utc=2026-08-01T00:00:00Z\n' "$$" > "$N1LOCK/holder"
PATH="$N1SHIM:$PATH" bash "$PROBE" "$(mkreq "mode=window" "session_lock=$N1LOCK" \
  "window_tag=n1det" "qwen_service=none" "lock_reap_age_s=60")" > "$WORK/n1.rec" 2>/dev/null
# errno classification: EEXIST => contention, the dir is gone, so it retries and ACQUIRES.
# post-hoc stat: sees the vanished dir and wrongly reports create-failure. The two outcomes are
# mutually exclusive and require no timing at all.
[ "$(probe_obs "$WORK/n1.rec" lock.create_failed)" != "1" ] \
  && ok "NEW-1 (deterministic): a vanished contender is NOT misclassified as create-failure" \
  || bad "NEW-1 (deterministic): errno classification regressed to a post-hoc stat (create_failed=1 on EEXIST)"
[ "$(probe_obs "$WORK/n1.rec" lock.acquired)" = "1" ] \
  && ok "NEW-1 (deterministic): and the probe retries and ACQUIRES the released lock" \
  || bad "NEW-1 (deterministic): probe did not acquire after the contender released (acquired='$(probe_obs "$WORK/n1.rec" lock.acquired)')"
rm -rf "$N1LOCK" "$N1SHIM"
# A REAL create failure must still classify as one — the fix must not swallow it.
gate "$PINS" --pin "WP_BOX_LOCK=$WORK/no-such-parent/deeper/lock.d"
expect "NEW-1: a genuine create failure is still exit 5, named" 5 lock.acquired FAIL \
  "failed for a reason unrelated to contention"
# ...and a lock held by a LIVE holder is contention: exit 3, not 5.
LIVELOCK="$WORK/live.d"; rm -rf "$LIVELOCK"; mkdir -p "$LIVELOCK"
printf '%s\n' "$$" > "$LIVELOCK/pid"
printf 'tag=peer\npid=%s\nuser=x\nacquired_utc=2026-08-20T00:00:00Z\n' "$$" > "$LIVELOCK/holder"
gate "$PINS" --pin "WP_BOX_LOCK=$LIVELOCK"
# Caught in BASICS, before the smoke phase is reached at all — so the item is `boxlock`, and
# `lock.acquired` is correctly absent. What NEW-1 requires is the CLASS: contention is 3, never 5.
expect "NEW-1: a live holder is CONTENTION — exit 3, not 5" 3 boxlock FAIL "held by a running process"
rm -rf "$LIVELOCK"

# M8: the liveness guard itself. Every reap case above uses a DEAD holder (pid 999999), so
# disabling `ps -p` changes nothing in them — the check that a LIVE holder is never reaped was
# not actually asserted anywhere, and a mutation removing it survived the whole suite. The gate's
# own live-lock refusal happens earlier, in BASICS, so it does not cover this path either.
# Here the lock is old enough and stale enough to be reap-eligible on every other axis, and the
# ONLY thing standing between it and a reap is the holder being alive.
ALIVELOCK="$WORK/alive.d"; rm -rf "$ALIVELOCK" "$ALIVELOCK".reap* 2>/dev/null
sleep 600 & ALIVEPID=$!
mkdir -p "$ALIVELOCK"
printf '%s\n' "$ALIVEPID" > "$ALIVELOCK/pid"
printf 'tag=live-peer\npid=%s\nuser=x\nacquired_utc=2026-08-01T00:00:00Z\n' "$ALIVEPID" > "$ALIVELOCK/holder"
touch -t 202001010000 "$ALIVELOCK"          # ancient: age is NOT what should save it
bash "$PROBE" "$(mkreq "mode=window" "session_lock=$ALIVELOCK" "window_tag=alive" \
  "qwen_service=none" "lock_reap_age_s=1")" > "$WORK/alive.rec" 2>/dev/null
[ "$(probe_obs "$WORK/alive.rec" lock.reaped)" != "1" ] \
  && ok "battery-M8: an ancient, reap-age-eligible lock whose holder is ALIVE is NOT reaped" \
  || bad "battery-M8: reaped a lock from a LIVE holder (pid $ALIVEPID)"
# This also PINS THE ORDERING that keeps X-1 fixed: the lock here is ancient and reap-age
# eligible, so the only thing that can save it is liveness being consulted BEFORE age. A refusal
# reason of anything other than holder-alive (say `too-fresh`, or a reap) means the ladder was
# reordered and a live window's lock is reapable again purely by getting old.
[ "$(probe_obs "$WORK/alive.rec" lock.reap_refused)" = "holder-alive" ] \
  && ok "battery-M8: and the refusal names the reason — holder-alive (pins liveness-before-age)" \
  || bad "battery-M8: reap_refused = '$(probe_obs "$WORK/alive.rec" lock.reap_refused)', want holder-alive"
[ "$(probe_obs "$WORK/alive.rec" lock.acquired)" = "0" ] \
  && ok "battery-M8: and the probe did not take the lock" || bad "battery-M8: the probe took a live peer's lock"
[ -d "$ALIVELOCK" ] && [ -f "$ALIVELOCK/pid" ] \
  && ok "battery-M8: the live peer's lock is left exactly as it was" \
  || bad "battery-M8: the live peer's lock was disturbed"
kill "$ALIVEPID" 2>/dev/null; wait "$ALIVEPID" 2>/dev/null
rm -rf "$ALIVELOCK" "$ALIVELOCK".reap* 2>/dev/null

# M4: a smoke leg that spawns and handshakes but EXITS NONZERO. Every other smoke case in this
# suite either passes or dies with one of the recognised stderr signatures (timeout, "closed the
# stream", protocol_version), all of which are matched BEFORE the plain `rc != 0` branch — so
# that branch, the one that catches "the leg ran and simply failed", was never exercised, and a
# mutation reporting every smoke rc as 0 survived the whole suite.
M4B="$WORK/box-m4"; mkbox "$M4B"
M4REAL="$M4B/bin/benchd.real"; mv "$M4B/bin/benchd" "$M4REAL"
# Delegates everything the earlier phases interrogate to the real binary, and fails ONLY the
# recipe verb — with innocuous stderr, so none of the signature branches above can claim it.
cat > "$M4B/bin/benchd" <<M4EOF
#!/bin/bash
if [ "\${1-}" = "prefill-decompose" ]; then
  echo "stub: decompose declined for the M4 fixture" >&2
  exit 3
fi
exec "$M4REAL" "\$@"
M4EOF
chmod +x "$M4B/bin/benchd"
# Pins are generated AFTER the wrapper is in place: the gate verifies the benchd binary's own
# digest, so pinning the original and then swapping it fails at PINS (exit 1) and the smoke leg
# is never reached — the fixture would be testing the wrong seam.
genpins "$M4B" "$WORK/pins-m4" handshake
gate "$WORK/pins-m4"
expect "battery-M4: a smoke leg that runs and FAILS is exit 8, named" \
  8 smoke.handshake FAIL "did not complete its round trip"
# The REAL exit status must reach the record — that is the field the mutation blanked.
printf '%s' "$(ao smoke.handshake)" | grep -q "rc=3" \
  && ok "battery-M4: and the leg's true exit status (rc=3) is what got recorded" \
  || bad "battery-M4: observed '$(ao smoke.handshake)' does not carry rc=3"

# ---------------- M6: a GPU flock actually HELD ------------------------------
# The gate refuses when the gpu-exclusive flock is open in another process. Nothing held it in
# any existing case, so the "HELD" verdict was unreachable and a mutation reporting every flock
# as free survived. A real open fd from a live process is what lsof reports, so that is the
# fixture.
M6B="$WORK/box-m6"; mkbox "$M6B"; genpins "$M6B" "$WORK/pins-m6" handshake
GPUF="$M6B/gpu.lock"; : > "$GPUF"
( exec 9>"$GPUF"; sleep 300 ) & GPUPID=$!
n=0; while [ "$n" -lt 20 ]; do [ -n "$(lsof -t -- "$GPUF" 2>/dev/null)" ] && break; sleep 0.25; n=$((n + 1)); done
if [ -n "$(lsof -t -- "$GPUF" 2>/dev/null)" ]; then
  ok "battery-M6 fixture: a live process really does hold the flock open (lsof sees it)"
  gate "$WORK/pins-m6" --pin "WP_GPU_LOCK=$GPUF"
  expect "battery-M6: a HELD gpu flock refuses the gate — exit 3, named" 3 gpulock FAIL "is open in another process"
  # ...and the same lock with nothing holding it must NOT refuse, or the check is just noise.
  kill "$GPUPID" 2>/dev/null; wait "$GPUPID" 2>/dev/null; GPUPID=""
  n=0; while [ "$n" -lt 20 ]; do [ -z "$(lsof -t -- "$GPUF" 2>/dev/null)" ] && break; sleep 0.25; n=$((n + 1)); done
  gate "$WORK/pins-m6" --pin "WP_GPU_LOCK=$GPUF"
  [ "$G_RC" = "0" ] && [ "$(av gpulock)" = "PASS" ] \
    && ok "battery-M6: and the SAME file with no holder passes (the verdict tracks the holder, not the file)" \
    || bad "battery-M6: an unheld gpu flock still refused (rc=$G_RC, verdict $(av gpulock))"
else
  bad "battery-M6 fixture: could not get lsof to report a holder for the flock"
fi
[ -n "${GPUPID:-}" ] && { kill "$GPUPID" 2>/dev/null; wait "$GPUPID" 2>/dev/null; }

# ---------------- M7: req() must match the WHOLE key, not a prefix -----------
# The battery mutation `"$k="*` -> `"$k"*` is inert against today's request: the key set is
# closed and no key is a prefix of another. That is a property of the CURRENT key list, not of
# req(), and it stops holding the day someone adds `smoke_argv_extra` next to `smoke_argv`. Pin
# the invariant itself so the guarantee survives the next key.
M7B="$WORK/box-m7"; mkbox "$M7B"
M7WANT="$(bash "$PROBE" "$(mkreq "weights_path=$M7B/weights")" 2>/dev/null \
          | { while IFS= read -r l; do case "$l" in weights.sha256=*) printf '%s' "${l#weights.sha256=}";; esac; done; } \
          | base64 -d 2>/dev/null)"
# A longer key that SHARES the prefix, deliberately listed FIRST so a prefix match would take it.
M7REQ="$(printf '%s\n' \
  "weights_path_decoy=$(printf '%s' "$WORK/no-such-weights" | base64 | tr -d '\n')" \
  "weights_path=$(printf '%s' "$M7B/weights" | base64 | tr -d '\n')" | base64 | tr -d '\n')"
bash "$PROBE" "$M7REQ" > "$WORK/m7.rec" 2>/dev/null
M7GOT="$(probe_obs "$WORK/m7.rec" weights.sha256)"
{ [ -n "$M7WANT" ] && [ "$M7GOT" = "$M7WANT" ]; } \
  && ok "battery-M7: req() matches the WHOLE key — a longer key sharing its prefix, listed first, is not taken" \
  || bad "battery-M7: req() read the decoy — weights.sha256 '$M7GOT' != '$M7WANT'"

# ============ X-2: --release must not call a BROKEN PIN a waiver =============
# A-2's shape through a different door. With a qwen service PINNED but the file absent, release
# emitted qwen.reloaded=declared-none — the same token the deliberate `none` waiver uses — and
# declared-none sits in the OK list, so --release printed OK and exited 0 having reloaded
# nothing. The box was handed on FREE but NOT SERVING. Window mode already fails closed on this
# exact condition; two modes must not disagree about one pin.
X2B="$WORK/box-x2"; mkbox "$X2B"; genpins "$X2B" "$WORK/pins-x2" handshake
X2LOCK="$X2B/box.lock.d"; X2TAG="x2-window"
rm -rf "$X2LOCK"
gate "$WORK/pins-x2" --pin "WP_BOX_LOCK=$X2LOCK" --pin "WP_WINDOW_TAG=$X2TAG"
[ "$G_RC" = "0" ] && [ -d "$X2LOCK" ] \
  && ok "X-2 fixture: a window is held, ready to release" \
  || bad "X-2 fixture: gate rc=$G_RC"
# Now break the pin the way a real box breaks it: the service file is gone at release time.
X2SVC="$X2B/qwen-service.sh"; mv "$X2SVC" "$X2SVC.moved"
X2_RC=0
bash "$GATE" --pins "$WORK/pins-x2" --pin "WP_BOX_LOCK=$X2LOCK" --pin "WP_WINDOW_TAG=$X2TAG" \
  --release > "$WORK/x2.out" 2>&1 || X2_RC=$?
[ "$X2_RC" = "6" ] \
  && ok "X-2: --release with a PINNED-but-ABSENT qwen service fails closed (exit 6, E_QWEN)" \
  || { bad "X-2: release rc=$X2_RC, want 6 — a broken pin was read as the 'none' waiver"; tail -6 "$WORK/x2.out"; }
# A phrase needle alone would pass if the text moved to an unrelated branch, so it is paired
# with the exit code and the sealed value above, which are the load-bearing facts.
grep -q "BROKEN PIN" "$WORK/x2.out" \
  && ok "X-2: and it SAYS broken pin, distinguishing it from a declared waiver" \
  || bad "X-2: the output does not name this as a broken pin"
grep -q "NOT SERVING" "$WORK/x2.out" \
  && ok "X-2: and it says the box is NOT SERVING, which is the fact that matters next" \
  || bad "X-2: the output does not warn that the box is not serving"
# The gate writes window-release.json to $OUT, which is the att/ directory — NOT out/. Reading
# the wrong path made X2REL empty, and `"" != "declared-none"` is TRUE, so this assertion was
# green against a full revert of the X-2 fix: it proved nothing. Read the real path, and require
# the POSITIVE value rather than merely "not the bad one", so an empty file cannot pass either.
X2RELFILE="$X2B/att/window-release.json"
[ -f "$X2RELFILE" ] \
  && ok "X-2: the release record is written where the gate says it writes it (att/)" \
  || bad "X-2: no window-release.json at $X2RELFILE"
X2REL="$(jq -r '.qwen.reloaded // empty' "$X2RELFILE" 2>/dev/null)"
[ "$X2REL" = "no-service-file" ] \
  && ok "X-2: and it seals qwen.reloaded=no-service-file, distinct from the declared waiver" \
  || bad "X-2: sealed qwen.reloaded='$X2REL', want no-service-file"
mv "$X2SVC.moved" "$X2SVC"
# The OTHER broken-pin shape: the file is present but defines no qwen_reload. Same class, same
# fail-closed requirement, and it had no case at all.
rm -rf "$X2LOCK"
# PIN ROT is the only scenario that can reach the release-mode branch, and modelling it wrongly
# cost two fixture rewrites. window-probe.sh requires BOTH qwen_unload and qwen_reload to be
# defined before it will touch the serving model: given a file missing either, it refuses,
# releases the lock and exits (probe :764). So a window can never be ESTABLISHED with such a
# file — an earlier fixture tried and its setup gate failed with rc=6, which the case then read
# as the release-mode bug it was written to catch.
#
# The real shape is: the window is held with a WORKING service, and the pin rots before release —
# the file is edited, replaced, or repointed while the window runs. Hold with the real file, then
# point WP_QWEN_SERVICE at the broken one for the release only.
X2SVC2="$X2B/qwen-service-noreload.sh"
grep -v '^qwen_reload()' "$X2SVC" > "$X2SVC2"
{ grep -q '^qwen_unload()' "$X2SVC2" && ! grep -q '^qwen_reload()' "$X2SVC2"; } \
  && ok "X-2 fixture: the no-reload service still unloads for real; only qwen_reload is missing" \
  || bad "X-2 fixture: derived service file is not the intended shape"
# RESTORE THE BOX TO SERVING FIRST. The sub-cases above deliberately released without reloading
# (that is the fail-open they prove), so the stand-in is gone and any later window pinned to
# EXPECT=loaded fails on qwen.state before it can hold anything — which is what the previous
# fixture mistook for the release-mode bug. An operator would put the box back; so does this.
( . "$X2SVC"; qwen_reload; ) >/dev/null 2>&1
X2TAGP="$(cat "$X2B/.qwen-tag")"
n=0; while [ "$n" -lt 20 ]; do [ -n "$(pgrep -f "$X2TAGP" 2>/dev/null)" ] && break; sleep 0.25; n=$((n + 1)); done
[ -n "$(pgrep -f "$X2TAGP" 2>/dev/null)" ] \
  && ok "X-2 fixture: the box is serving again, so a fresh window can legitimately be held" \
  || bad "X-2 fixture: could not restore the serving stand-in"
rm -rf "$X2LOCK"
gate "$WORK/pins-x2" --pin "WP_BOX_LOCK=$X2LOCK" --pin "WP_WINDOW_TAG=$X2TAG"
{ [ "$G_RC" = "0" ] && [ -d "$X2LOCK" ]; } \
  && ok "X-2 fixture: a window is held using the WORKING service (the pin rots afterwards)" \
  || { bad "X-2 fixture: pin-rot setup gate rc=$G_RC, lock held=$([ -d "$X2LOCK" ] && echo yes || echo no)"
       jq -r '.items[]|select(.verdict=="FAIL")|"        \(.id): \(.diagnostic)"' "$G_ATT" 2>/dev/null | head -3; }
X2F_RC=0
bash "$GATE" --pins "$WORK/pins-x2" --pin "WP_BOX_LOCK=$X2LOCK" --pin "WP_WINDOW_TAG=$X2TAG" \
  --pin "WP_QWEN_SERVICE=$X2SVC2" --release > "$WORK/x2f.out" 2>&1 || X2F_RC=$?
[ "$X2F_RC" = "6" ] \
  && ok "X-2: a service file defining NO qwen_reload also fails closed at release (exit 6)" \
  || { bad "X-2: no-reload-function release rc=$X2F_RC, want 6"; tail -5 "$WORK/x2f.out"; }
[ "$(jq -r '.qwen.reloaded // empty' "$X2B/att/window-release.json" 2>/dev/null)" = "service-missing-functions" ] \
  && ok "X-2: and seals service-missing-functions, not an empty field" \
  || bad "X-2: no-reload case sealed '$(jq -r '.qwen.reloaded // empty' "$X2B/att/window-release.json" 2>/dev/null)'"

# ...and the WINDOW side of the same broken pin: it must refuse, and must SAY it is a broken pin
# rather than blaming the serving model. This arm used to report "STILL RESIDENT" for a box where
# nothing was resident and no unload had been attempted.
( . "$X2SVC"; qwen_reload; ) >/dev/null 2>&1
n=0; while [ "$n" -lt 20 ]; do [ -n "$(pgrep -f "$X2TAGP" 2>/dev/null)" ] && break; sleep 0.25; n=$((n + 1)); done
rm -rf "$X2LOCK"
gate "$WORK/pins-x2" --pin "WP_BOX_LOCK=$X2LOCK" --pin "WP_WINDOW_TAG=$X2TAG" \
  --pin "WP_QWEN_SERVICE=$X2SVC2"
expect "X-2: the WINDOW refuses a functions-missing service pin, exit 6" \
  6 qwen.unload FAIL "does not define both qwen_unload and qwen_reload"
printf '%s' "$(ao qwen.unload)" | grep -qF "NOT ATTEMPTED" \
  && ok "X-2: and says NOT ATTEMPTED rather than blaming a model that was never resident" \
  || bad "X-2: window-side observed reads '$(ao qwen.unload)'"
[ ! -d "$X2LOCK" ] \
  && ok "X-2: and the probe released the lock it had just taken (no brick on a broken pin)" \
  || bad "X-2: the lock survived a refused window"
# The genuine waiver must still pass, or the fix has simply broken the `none` path.
rm -rf "$X2LOCK"
gate "$WORK/pins-x2" --pin "WP_BOX_LOCK=$X2LOCK" --pin "WP_WINDOW_TAG=$X2TAG" \
  --pin "WP_QWEN_SERVICE=none" --pin "WP_QWEN_EXPECT=none"
X2N_RC=0
bash "$GATE" --pins "$WORK/pins-x2" --pin "WP_BOX_LOCK=$X2LOCK" --pin "WP_WINDOW_TAG=$X2TAG" \
  --pin "WP_QWEN_SERVICE=none" --pin "WP_QWEN_EXPECT=none" --release > "$WORK/x2n.out" 2>&1 || X2N_RC=$?
[ "$X2N_RC" = "0" ] \
  && ok "X-2: and a DECLARED 'none' still releases OK — the waiver is intact" \
  || { bad "X-2: declared-none release rc=$X2N_RC, want 0"; tail -4 "$WORK/x2n.out"; }

# ===================== X-1: the handoff must be ADOPTED ======================
# The single-flight break. The gate PASSes and exits STILL HOLDING the lock, with pid = the
# box-side PROBE, a process that exited by design. Nothing refreshed the pid or the mtime, so for
# the whole window the reap predicate read verifiable + provably-dead + old-enough: protection
# DECREASED with window length, and any window outlasting the reap age was reapable by a second
# gate, which would unload the serving model under a live run and seal a forged verified_dead_how.
X1B="$WORK/box-x1"; mkbox "$X1B"; genpins "$X1B" "$WORK/pins-x1" handshake
X1LOCK="$X1B/box.lock.d"; X1TAG="x1-window"
# These cases are about LOCK semantics only. The first gate legitimately leaves the serving model
# unloaded (it holds the window), so a follow-up gate asserting qwen state would fail on that
# instead — exit 6 before any lock verdict. Waive the serving assertion for the follow-ups only.
X1_ISOLATE=(--pin "WP_QWEN_SERVICE=none" --pin "WP_QWEN_EXPECT=none")
rm -rf "$X1LOCK" "$X1LOCK".reap* 2>/dev/null
gate "$WORK/pins-x1" --pin "WP_BOX_LOCK=$X1LOCK" --pin "WP_WINDOW_TAG=$X1TAG"
[ "$G_RC" = "0" ] && [ -d "$X1LOCK" ] \
  && ok "X-1 fixture: the gate PASSED and exited STILL HOLDING the lock (the handoff)" \
  || bad "X-1 fixture: gate rc=$G_RC, lock present=$([ -d "$X1LOCK" ] && echo yes || echo no)"
[ "$(cat "$X1LOCK/pid" 2>/dev/null)" != "$$" ] \
  && ok "X-1 fixture: and the pid it left is the box-side probe's, which has exited" \
  || bad "X-1 fixture: unexpected pid in the handed-off lock"

# (1) DRIVER ADOPTS: a live driver must make the lock un-reapable for the window's true duration.
( exec -a x1-driver-standin sleep 300 ) >/dev/null 2>&1 </dev/null & X1DRV=$!
disown "$X1DRV" 2>/dev/null || true
printf '%s\n' "$X1DRV" > "$X1LOCK/pid"
printf 'adopted_pid=%s\n' "$X1DRV" >> "$X1LOCK/holder"
touch -t 202001010000 "$X1LOCK"      # ancient: age alone must not be enough to reap it
gate "$WORK/pins-x1" --pin "WP_BOX_LOCK=$X1LOCK" --pin "WP_WINDOW_TAG=other-window" \
  --pin "WP_LOCK_REAP_AGE_S=1"
# NOTE ON WHAT DISCHARGES THIS: rc 3 here comes from the BASICS-phase boxlock check, which
# refuses a live-held lock before the probe's reap predicate is consulted at all. So this case
# proves the OUTCOME (a live adopted window is not disturbed) but not the predicate path. The
# predicate itself is covered by battery-M8, which drives the probe directly against a live
# holder and asserts reap_refused=holder-alive. Kept deliberately: the outcome is the property
# operators care about, and the two together cover both.
[ "$G_RC" = "3" ] \
  && ok "X-1: an ADOPTED lock older than the reap age is REFUSED (holder alive) — was: reaped" \
  || { bad "X-1: second gate rc=$G_RC against an adopted live lock, want 3"; tail -4 "$WORK/gate.out"; }
[ -d "$X1LOCK" ] && [ "$(cat "$X1LOCK/pid" 2>/dev/null)" = "$X1DRV" ] \
  && ok "X-1: and the live window's lock is left exactly as it was" \
  || bad "X-1: the live window's lock was disturbed"
[ -z "$(ls -d "$X1LOCK".reaped.* 2>/dev/null)" ] \
  && ok "X-1: and nothing was moved aside — no forged verified_dead_how" \
  || bad "X-1: a reaped-aside directory exists for a LIVE holder"

# (2) DRIVER CRASHED: the adopting process is gone, so the lock MAY be reaped — with true
#     evidence. This is the half that makes adoption safe rather than a permanent lock.
kill "$X1DRV" 2>/dev/null; wait "$X1DRV" 2>/dev/null
touch -t 202001010000 "$X1LOCK"
gate "$WORK/pins-x1" --pin "WP_BOX_LOCK=$X1LOCK" --pin "WP_WINDOW_TAG=other-window" \
  --pin "WP_LOCK_REAP_AGE_S=1" "${X1_ISOLATE[@]}"
[ "$G_RC" = "0" ] \
  && ok "X-1: once the adopting driver DIES the lock is reapable again (no permanent brick)" \
  || { bad "X-1: crashed-driver gate rc=$G_RC, want 0"; grep -E "FAIL" "$WORK/gate.out" | head -3; }
X1ATT_REAPED="$WORK/x1-reaped-att.json"; cp "$G_ATT" "$X1ATT_REAPED" 2>/dev/null || true
X1PRIOR="$(jq -r '.lock.reaped.prior_pid // empty' "$X1ATT_REAPED" 2>/dev/null)"
[ "$X1PRIOR" = "$X1DRV" ] \
  && ok "X-1: and the reap evidence names the DRIVER that died, not the long-gone probe" \
  || bad "X-1: reaped.prior_pid='$X1PRIOR', want the adopting driver's pid $X1DRV"

# ===== M-b: a bundle whose bytes CONTRADICT its marker is not "unavailable" ==
# bundle present + sha matching its pin + pinned commit NOT among its heads used to fall to the
# else branch and be graded CLAIMED with the text "bundle file no longer on the box" — false when
# the file is right there, and it flattens a contradiction into a benign cannot-re-verify. That
# is how a bundle with swapped contents passes review.
MBB="$WORK/box-mb"; mkbox "$MBB"
MBSRC="$WORK/mb-src"; rm -rf "$MBSRC"; mkdir -p "$MBSRC"
git -C "$MBSRC" init -q 2>/dev/null; printf 'a\n' > "$MBSRC/f"
git -C "$MBSRC" add -A 2>/dev/null
git -C "$MBSRC" -c user.email=t@t -c user.name=t commit -qm one 2>/dev/null
git -C "$MBSRC" bundle create -q "$MBB/mb.bundle" HEAD 2>/dev/null
# Clone FROM THE BUNDLE (so the tree legitimately carries a bundle marker), then commit PAST it:
# the pinned commit is then provably not among the bundle's heads.
rm -rf "$MBB/bench"; git clone -q "$MBB/mb.bundle" "$MBB/bench" 2>/dev/null
printf 'b\n' > "$MBB/bench/f"
git -C "$MBB/bench" add -A 2>/dev/null
git -C "$MBB/bench" -c user.email=t@t -c user.name=t commit -qm two 2>/dev/null
MBPINNED="$(git -C "$MBB/bench" rev-parse HEAD)"
MBGD="$(git -C "$MBB/bench" rev-parse --absolute-git-dir)"
printf 'bundle_sha256=%s\nbundle_path=%s\ncommit=%s\n' \
  "$(sha_of "$MBB/mb.bundle")" "$MBB/mb.bundle" "$MBPINNED" > "$MBGD/window-bundle-provenance"
genpins "$MBB" "$WORK/pins-mb" handshake
gate "$WORK/pins-mb" \
  --pin "WP_BENCH_PATH=$MBB/bench" --pin "WP_BENCH_SHA=$MBPINNED" \
  --pin "WP_BENCH_BUNDLE=$MBB/mb.bundle" \
  --pin "WP_BENCH_BUNDLE_SHA256=$(sha_of "$MBB/mb.bundle")"
[ "$G_RC" = "7" ] \
  && ok "M-b: a bundle matching its pin but NOT containing the claimed commit is REFUSED (exit 7)" \
  || { bad "M-b: contradicting bundle rc=$G_RC, want 7"; grep -iE "bundle|FAIL" "$WORK/gate.out" | head -4; }
# The gate's item ids use the UPPERCASE role label (`check_tree bench BENCH …`), so this is
# BENCH.bundle. Asserting on a lowercase id read back an empty verdict, and "" != "REFUSED"
# failed loudly here — but the same slip in a NEGATIVE assertion would have passed silently.
[ "$(av BENCH.bundle)" = "REFUSED" ] \
  && ok "M-b: and it is graded REFUSED, not CLAIMED" \
  || bad "M-b: BENCH.bundle verdict '$(av BENCH.bundle)', want REFUSED"
printf '%s' "$(ad BENCH.bundle)" | grep -qF "contradict" \
  && ok "M-b: and the diagnostic says the bytes contradict the marker" \
  || bad "M-b: diagnostic does not name the contradiction: $(ad BENCH.bundle)"
# The product emits this text in the OBSERVED column (window-preflight.sh:656), not the
# diagnostic. Grepping the diagnostic made this pass whichever way the product behaved — a
# negative assertion pointed at a field that never carries the string is green by construction.
printf '%s' "$(ao BENCH.bundle)" | grep -qF "no longer on the box" \
  && bad "M-b: still claims the bundle is not on the box when it is" \
  || ok "M-b: and it no longer claims the file is absent when it is present"

# ============ M-a: the attestation must carry the moved-aside pointer ========
# The docs promised lock.reaped.moved_to; the probe emitted lock.reaped_moved_to; the gate's jq
# dropped it. Reaping MOVES the corpse aside precisely so it stays inspectable, and the sealed
# artifact — the only thing that outlives the run — had no pointer to it. Read it from the
# ATTESTATION, not the raw probe record, because the attestation is what anyone will actually have.
# Anchored to the crashed-driver run above — the one that actually reaped. (The unadopted-handoff
# case runs after it and reaps a different lock, so the attestation must be captured deliberately
# rather than whichever gate happened to run last.)
MOVED="$(jq -r '.lock.reaped.moved_to // empty' "$X1ATT_REAPED" 2>/dev/null)"
{ [ -n "$MOVED" ] && [ -d "$MOVED" ]; } \
  && ok "M-a: the attestation names the moved-aside lock, and the directory is really there" \
  || bad "M-a: attestation lock.reaped.moved_to='$MOVED' (dir present: $([ -d "$MOVED" ] && echo yes || echo no))"
[ -f "$MOVED/pid" ] \
  && ok "M-a: and the reaped lock's own remains are readable at that path" \
  || bad "M-a: no pid file under the moved-aside path"

# (3) HANDOFF, THEN NO DRIVER — ruled deliberately, not left to accident. The gate hands off and
#     nothing ever adopts. The pid stays the exited probe's, so once the lock ages past the
#     threshold it IS reapable. That is intended: nothing is alive to protect, so an abandoned
#     handoff self-heals rather than bricking the box until someone SSHes in. A window that means
#     to hold the box must run a driver that adopts — which is what the case below proves it does.
X1LOCK3="$X1B/box.lock3.d"; rm -rf "$X1LOCK3" "$X1LOCK3".reap* 2>/dev/null
gate "$WORK/pins-x1" --pin "WP_BOX_LOCK=$X1LOCK3" --pin "WP_WINDOW_TAG=$X1TAG" "${X1_ISOLATE[@]}"
{ [ "$G_RC" = "0" ] && [ -f "$X1LOCK3/pid" ]; } \
  && ok "X-1 fixture: a second handoff is held, with a readable pid, and nothing adopts it" \
  || bad "X-1 fixture: handoff-no-driver setup rc=$G_RC"
touch -t 202001010000 "$X1LOCK3"
gate "$WORK/pins-x1" --pin "WP_BOX_LOCK=$X1LOCK3" --pin "WP_WINDOW_TAG=other-window" \
  --pin "WP_LOCK_REAP_AGE_S=1" "${X1_ISOLATE[@]}"
[ "$G_RC" = "0" ] \
  && ok "X-1: an UNADOPTED handoff self-heals once aged out (RULED: no driver, nothing to protect)" \
  || { bad "X-1: unadopted aged handoff rc=$G_RC, want 0"; grep -E "FAIL" "$WORK/gate.out" | head -3; }
[ "$(jq -r '.lock.reaped.prior_pid // empty' "$G_ATT" 2>/dev/null)" != "" ] \
  && ok "X-1: and the reap is recorded with evidence, not done silently" \
  || bad "X-1: an unadopted handoff was reaped without a sealed reap record"

# ========== X-1 (driver half): the inherit branch must ADOPT the lock ========
# The three cases above prove the PROBE's reap semantics given an adopted lock. This one proves
# the other half — that run-paired-window.sh actually adopts on its inheritance branch — by
# running THE REAL SCRIPT. It is copied beside stub libs so `$HERE` resolves to them; the copy is
# byte-identical to the shipped file, so this tests the code rather than a description of it.
# parity_precheck is stubbed to FAIL, which exits the driver immediately after the lock section:
# far enough to observe adoption, nowhere near a benchmark.
DRVDIR="$WORK/drv"; mkdir -p "$DRVDIR"
# $HERE, never a cwd-relative path. Every other script under test is resolved through $HERE
# ($GATE/$PROBE/$PROV); this one was not, so it copied whichever run-paired-window.sh happened to
# sit under the CURRENT DIRECTORY. Under the mutation battery — which runs each mutant tree's
# suite with the cwd set to the real repo — that meant this case exercised the UNMUTATED driver
# every time, and passed no matter what the driver did. It is the same defect as asserting on
# source text: the test was not bound to the thing under test.
cp "$HERE/run-paired-window.sh" "$DRVDIR/run-paired-window.sh"
cat > "$DRVDIR/parity-lib.sh" <<'DEOF'
parity_take_gpu_lock() { return 0; }
parity_precheck() { echo "stub precheck: stopping here"; return 1; }
DEOF
cat > "$DRVDIR/official-lib.sh" <<'DEOF'
official_commit_sha40() { echo "0000000000000000000000000000000000000000"; }
DEOF
DRVG="$WORK/drv-g"; mkdir -p "$DRVG/mlxfast-bench"
DRVLOCK="$WORK/drv-box.lock.d"; rm -rf "$DRVLOCK"
DRVTAG="drv-window-tag"
# A handoff exactly as the gate leaves one: our tag, and a pid that is NOT alive.
mkdir -p "$DRVLOCK"; printf '999997\n' > "$DRVLOCK/pid"
printf 'tag=%s\npid=999997\nuser=x\nacquired_utc=2026-08-01T00:00:00Z\n' "$DRVTAG" > "$DRVLOCK/holder"
# The scoring-window gate (bench#143 wire a) refuses without a PASSED attestation for THIS window,
# so hand it the one the gate would have sealed alongside the lock it is still holding.
DRVATT="$WORK/drv-window-provenance.json"
printf '{"schema":"window-provenance/v1","verdict":"PASS","lock":{"window_tag":"%s"},"lock_taken":true}\n' "$DRVTAG" > "$DRVATT"
DRV_RC=0
env MLXFAST_PARITY_GIT="$DRVG" OUT="$WORK/drv-out" \
    MLXFAST_BOX_LOCK="$DRVLOCK" MLXFAST_GPU_LOCK="$WORK/drv-gpu.lock" \
    WP_WINDOW_TAG="$DRVTAG" WP_ATTESTATION="$DRVATT" REPLICA_LOCAL="$WORK/drv-replica" \
    bash "$DRVDIR/run-paired-window.sh" > "$WORK/drv.out" 2>&1 || DRV_RC=$?
grep -q "INHERITED" "$WORK/drv.out" \
  && ok "X-1 fixture: the real driver reached its INHERITANCE branch" \
  || { bad "X-1 fixture: driver did not reach the inherit branch (rc=$DRV_RC)"; tail -8 "$WORK/drv.out"; }
DRVPID="$(cat "$DRVLOCK/pid" 2>/dev/null)"
[ -n "$DRVPID" ] && [ "$DRVPID" != "999997" ] \
  && ok "X-1: the driver ADOPTS the handed-off lock — it writes its OWN pid (was: the exited probe's)" \
  || bad "X-1: the lock still carries pid '$DRVPID' after inheritance — a live window looks DEAD to the reaper"
grep -q "adopted_pid=" "$DRVLOCK/holder" 2>/dev/null \
  && ok "X-1: and records the adoption in the holder record" \
  || bad "X-1: no adopted_pid in the holder record"
# It must still NOT release an inherited lock — adoption is about who vouches, not who releases.
[ -d "$DRVLOCK" ] \
  && ok "X-1: and still does not RELEASE the inherited lock (that remains --release's job)" \
  || bad "X-1: the driver released a lock it inherited"
rm -rf "$DRVLOCK" "$WORK/drv-out" 2>/dev/null

# ============ F3: a stranded reap mutex must not brick reaping ==============
# A trap cannot cover SIGKILL, and a stranded mutex is quietly catastrophic: every later probe
# stands down with `reap-in-progress`, so ONE hard-killed run disables reaping on the box forever.
F3LOCK="$WORK/f3.d"; rm -rf "$F3LOCK" "$F3LOCK".reap* 2>/dev/null
mkdir -p "$F3LOCK"; printf '999999\n' > "$F3LOCK/pid"
printf 'tag=dead\npid=999999\nuser=x\nacquired_utc=2026-08-01T00:00:00Z\n' > "$F3LOCK/holder"
touch -t 202001010000 "$F3LOCK"
# A mutex left by a process that no longer exists, older than the staleness threshold.
mkdir -p "$F3LOCK.reapmutex"; printf '999998\n' > "$F3LOCK.reapmutex/pid"
touch -t 202001010000 "$F3LOCK.reapmutex"
bash "$PROBE" "$(mkreq "mode=window" "session_lock=$F3LOCK" "window_tag=f3" \
  "qwen_service=none" "lock_reap_age_s=1")" > "$WORK/f3.rec" 2>/dev/null
[ "$(probe_obs "$WORK/f3.rec" lock.reap_mutex_reclaimed)" = "1" ] \
  && ok "F3: a STALE reap mutex whose holder is gone is reclaimed, not obeyed forever" \
  || bad "F3: stale mutex not reclaimed (reaping would be bricked permanently)"
[ "$(probe_obs "$WORK/f3.rec" lock.acquired)" = "1" ] \
  && ok "F3: and the probe goes on to reap and acquire normally" \
  || bad "F3: probe did not acquire after reclaiming the stale mutex"
# A mutex held by a LIVE process must NEVER be reclaimed, however old it looks.
rm -rf "$F3LOCK" "$F3LOCK".reap* 2>/dev/null
mkdir -p "$F3LOCK"; printf '999999\n' > "$F3LOCK/pid"
printf 'tag=dead\npid=999999\nuser=x\nacquired_utc=2026-08-01T00:00:00Z\n' > "$F3LOCK/holder"
touch -t 202001010000 "$F3LOCK"
sleep 300 & F3PID=$!
mkdir -p "$F3LOCK.reapmutex"; printf '%s\n' "$F3PID" > "$F3LOCK.reapmutex/pid"
touch -t 202001010000 "$F3LOCK.reapmutex"
bash "$PROBE" "$(mkreq "mode=window" "session_lock=$F3LOCK" "window_tag=f3b" \
  "qwen_service=none" "lock_reap_age_s=1")" > "$WORK/f3b.rec" 2>/dev/null
[ "$(probe_obs "$WORK/f3b.rec" lock.reap_mutex_reclaimed)" != "1" ] \
  && ok "F3: but a mutex whose holder is ALIVE is never reclaimed, however old it looks" \
  || bad "F3: reclaimed a reap mutex from a LIVE holder — mutual exclusion broken"
[ "$(probe_obs "$WORK/f3b.rec" lock.reap_refused)" = "reap-in-progress" ] \
  && ok "F3: and the probe stands down with reap-in-progress, as it should" \
  || bad "F3: refusal was '$(probe_obs "$WORK/f3b.rec" lock.reap_refused)'"
kill "$F3PID" 2>/dev/null; wait "$F3PID" 2>/dev/null
rm -rf "$F3LOCK" "$F3LOCK".reap* 2>/dev/null

# ================= C-c / C-d: --provision and the `both` recipe =============
# Both were entirely unexercised. Smoke-level, but they now RUN rather than being argued about.
PRB="$WORK/box-prov"; mkbox "$PRB"
PRSRC="$WORK/prov-src"; rm -rf "$PRSRC"; mkdir -p "$PRSRC"
git -C "$PRSRC" init -q 2>/dev/null; printf 'x\n' > "$PRSRC/f"
git -C "$PRSRC" add -A 2>/dev/null
git -C "$PRSRC" -c user.email=t@t -c user.name=t commit -qm one 2>/dev/null
PRSHA="$(git -C "$PRSRC" rev-parse HEAD)"
git -C "$PRSRC" bundle create -q "$PRB/prov.bundle" HEAD 2>/dev/null
genpins "$PRB" "$WORK/pins-prov" handshake
rm -rf "$PRB/bench-prov"
# C-c: --provision must (a) run, and (b) hear the SAME --pin overrides the gate then verifies
# against. Before N-e it read only the file, so an overridden path was provisioned in one place
# and verified in another, surfacing as a baffling digest failure rather than the real cause.
# Provisioned from a BUNDLE with its sha pinned, which is the shape the bundle rule accepts.
PR_RC=0
bash "$GATE" --pins "$WORK/pins-prov" --provision --no-smoke \
  --pin "WP_BENCH_PATH=$PRB/bench-prov" \
  --pin "WP_BENCH_SHA=$PRSHA" \
  --pin "WP_BENCH_BUNDLE=$PRB/prov.bundle" \
  --pin "WP_BENCH_BUNDLE_SHA256=$(sha_of "$PRB/prov.bundle")" > "$WORK/prov.out" 2>&1 || PR_RC=$?
[ "$PR_RC" = "0" ] \
  && ok "C-c: --provision exits 0 (the tree existing is not on its own proof it succeeded)" \
  || { bad "C-c: --provision rc=$PR_RC"; tail -6 "$WORK/prov.out"; }
[ -d "$PRB/bench-prov/.git" ] \
  && ok "C-c: --provision provisions the tree named by a --pin OVERRIDE, not just the one in the file" \
  || { bad "C-c: --provision did not create the overridden tree (rc=$PR_RC)"; tail -8 "$WORK/prov.out"; }
[ "$(git -C "$PRB/bench-prov" rev-parse HEAD 2>/dev/null)" = "$PRSHA" ] \
  && ok "C-c: and at the overridden commit" || bad "C-c: provisioned tree is at the wrong commit"
release_after "$WORK/pins-prov" --pin "WP_BENCH_PATH=$PRB/bench-prov" --pin "WP_BENCH_SHA=$PRSHA" \
  --pin "WP_BENCH_BUNDLE=$PRB/prov.bundle" \
  --pin "WP_BENCH_BUNDLE_SHA256=$(sha_of "$PRB/prov.bundle")"

# C-d: the `both` recipe composes handshake AND decode. Assert the COMPOSITION behaviourally —
# a benchd that RECORDS the argv it was called with — rather than re-testing either leg, which
# sections 7 and 11 already cover.
BOTHB="$WORK/box-both"; mkbox "$BOTHB"
BOTHLOG="$BOTHB/benchd-argv.log"; : > "$BOTHLOG"
BOTHREAL="$BOTHB/bin/benchd.real"; mv "$BOTHB/bin/benchd" "$BOTHREAL"
cat > "$BOTHB/bin/benchd" <<BEOF
#!/bin/bash
printf '%s\n' "\$*" >> "$BOTHLOG"
case "\${1-}" in
  prefill-decompose|measure-job) exit 0 ;;
  *) exec "$BOTHREAL" "\$@" ;;
esac
BEOF
chmod +x "$BOTHB/bin/benchd"
genpins "$BOTHB" "$WORK/pins-both" handshake
gate "$WORK/pins-both" --pin "WP_SMOKE_RECIPE=both"
grep -q "prefill-decompose" "$BOTHLOG" && grep -q "measure-job" "$BOTHLOG" \
  && ok "C-d: the 'both' recipe really invokes BOTH verbs (recorded from the calls themselves)" \
  || { bad "C-d: 'both' did not invoke both verbs"; cat "$BOTHLOG"; }
[ "$(grep -n 'prefill-decompose' "$BOTHLOG" | head -1 | cut -d: -f1)" -lt \
  "$(grep -n 'measure-job' "$BOTHLOG" | head -1 | cut -d: -f1)" ] 2>/dev/null \
  && ok "C-d: and handshake runs FIRST, so a spawn failure is not misread as a decode bug" \
  || bad "C-d: 'both' did not run handshake before decode"
# `both` without a contract is a usage error, not a silently degraded recipe.
grep -v '^WP_CONTRACT_PATH=\|^WP_CONTRACT_SHA256=' "$WORK/pins-both" > "$WORK/pins-both-nc"
BOTHN_RC=0
bash "$GATE" --pins "$WORK/pins-both-nc" --pin "WP_SMOKE_RECIPE=both" \
  > "$WORK/bothn.out" 2>&1 || BOTHN_RC=$?
[ "$BOTHN_RC" = "2" ] \
  && ok "C-d: and 'both' without WP_CONTRACT_PATH is a usage error, not a quiet downgrade" \
  || { bad "C-d: 'both' with no contract rc=$BOTHN_RC, want 2"; tail -4 "$WORK/bothn.out"; }

# ============================ C-a: the ENV SEAM ==============================
# The headline assertion of this gate, and until now exercised only with all six WP_ENV_* pinned
# `unset` and nothing exported — so every iteration took the same branch and an always-PASS
# mutation of the whole loop survived the suite. Drive both directions for real.
ENVB="$WORK/box-env"; mkbox "$ENVB"; genpins "$ENVB" "$WORK/pins-env" handshake
# (i) a var pinned `unset` that IS exported => named FAIL, exit 1.
# NOT in a subshell. `bad()` increments a counter, and a counter incremented inside `( … )` dies
# with the subshell — the parent tally never sees it, so these three cases could REPORT a failure
# on stdout and still leave the suite green. Two of them were the only witnesses to the
# fail-open branches they cover, which is exactly the shape that lets a surgical mutation of the
# env loop survive. Export and restore around the parent shell instead.
_env_save() { _SV_SET=0; if [ -n "${!1+x}" ]; then _SV_SET=1; _SV_VAL="${!1}"; fi; }
_env_restore() { if [ "$_SV_SET" = "1" ]; then export "$2=$_SV_VAL"; else unset "$2"; fi; }

_env_save MLXFAST_NO_SANDBOX
export MLXFAST_NO_SANDBOX=1
gate "$WORK/pins-env"
expect "C-a: a var pinned 'unset' that is EXPORTED fails, named" \
  1 env.MLXFAST_NO_SANDBOX FAIL "is exported on the box"
_env_restore _ MLXFAST_NO_SANDBOX
# (ii) a var pinned to a VALUE, exported with that value => PASS row.
_env_save QMTP_HEAD_DIR
export QMTP_HEAD_DIR=/pinned/head/dir
gate "$WORK/pins-env" --pin "WP_ENV_QMTP_HEAD_DIR=/pinned/head/dir"
{ [ "$G_RC" = "0" ] && [ "$(av env.QMTP_HEAD_DIR)" = "PASS" ]; } \
  && ok "C-a: a var pinned to a VALUE and exported with it passes" \
  || bad "C-a: declared-value env var rc=$G_RC verdict '$(av env.QMTP_HEAD_DIR)'"
# (iii) same var, exported with the WRONG value => FAIL naming the mismatch.
export QMTP_HEAD_DIR=/some/other/dir
gate "$WORK/pins-env" --pin "WP_ENV_QMTP_HEAD_DIR=/pinned/head/dir"
expect "C-a: a var pinned to a VALUE but exported with another fails, named" \
  1 env.QMTP_HEAD_DIR FAIL "does not hold its pinned value"
_env_restore _ QMTP_HEAD_DIR
# (iv) a var pinned to a VALUE and NOT exported => FAIL (absence is not satisfaction).
gate "$WORK/pins-env" --pin "WP_ENV_QMTP_HEAD_DIR=/pinned/head/dir"
expect "C-a: a var pinned to a VALUE but NOT exported fails, named" \
  1 env.QMTP_HEAD_DIR FAIL "is not exported on the box"

# (v) a REQUIRED WP_ENV_* pin missing from the file is a usage error, not a silent skip. The
#     required six are the vars that change what gets spawned; a window that forgets to declare
#     one must not simply stop checking it.
grep -v '^WP_ENV_MLXFAST_NO_SANDBOX=' "$WORK/pins-env" > "$WORK/pins-env-missing"
gate "$WORK/pins-env-missing"
[ "$G_RC" = "2" ] && grep -q "WP_ENV_MLXFAST_NO_SANDBOX" "$WORK/gate.err" \
  && ok "C-a: a REQUIRED WP_ENV_* pin missing from the file is a usage error, named" \
  || bad "C-a: missing required env pin rc=$G_RC (want 2, naming the pin)"
# (vi) and the seam is not silently empty: the watch list must actually carry the six.
gate "$WORK/pins-env"
ENVCOUNT="$(jq -r '[.items[]|select(.id|startswith("env."))|select(.id!="env.namespace")]|length' "$G_ATT" 2>/dev/null)"
[ "${ENVCOUNT:-0}" -ge 6 ] \
  && ok "C-a: and every declared WP_ENV_* produces its own sealed row ($ENVCOUNT rows)" \
  || bad "C-a: only ${ENVCOUNT:-0} env rows sealed — the watch list is not being walked"

# ======================= C-b: the two REFUSAL gates ==========================
# WP_REQUIRE_NO_STRAY and WP_REQUIRE_TIMEMACHINE_IDLE both had only their WAIVED branch executed.
STRAYB="$WORK/box-stray"; mkbox "$STRAYB"; genpins "$STRAYB" "$WORK/pins-stray" handshake
( exec -a mlxfast-engine sleep 120 ) >/dev/null 2>&1 </dev/null & STRAYPID=$!
disown "$STRAYPID" 2>/dev/null || true
n=0; while [ "$n" -lt 20 ]; do [ -n "$(pgrep -x mlxfast-engine 2>/dev/null)" ] && break; sleep 0.25; n=$((n + 1)); done
if [ -n "$(pgrep -x mlxfast-engine 2>/dev/null)" ]; then
  ok "C-b fixture: a stray process really is named mlxfast-engine (pgrep -x sees it)"
  gate "$WORK/pins-stray" --pin "WP_REQUIRE_NO_STRAY=1"
  expect "C-b: a stray model process REFUSES the gate — exit 3, named" \
    3 quiet.stray FAIL "will contend for the GPU"
  # ...and the SAME box with the requirement waived must not refuse, or the pin does nothing.
  gate "$WORK/pins-stray" --pin "WP_REQUIRE_NO_STRAY=0"
  [ "$(av quiet.stray)" = "NOTE" ] \
    && ok "C-b: and WP_REQUIRE_NO_STRAY=0 waives it explicitly (recorded, not silent)" \
    || bad "C-b: waived stray check verdict '$(av quiet.stray)'"
  release_after "$WORK/pins-stray" --pin "WP_REQUIRE_NO_STRAY=0"
else
  bad "C-b fixture: could not start a process named mlxfast-engine"
fi
kill "$STRAYPID" 2>/dev/null; wait "$STRAYPID" 2>/dev/null

# Time Machine: the probe reads `tmutil status`, so a stub tmutil on PATH is the fixture.
TMBIN="$WORK/tmbin"; mkdir -p "$TMBIN"
printf '#!/bin/bash\necho "Backup session status: { Running = 1; }"\n' > "$TMBIN/tmutil"
chmod +x "$TMBIN/tmutil"
TMB="$WORK/box-tm"; mkbox "$TMB"; genpins "$TMB" "$WORK/pins-tm" handshake
G_RC=0; PATH="$TMBIN:$PATH" bash "$GATE" --pins "$WORK/pins-tm" \
  --pin "WP_REQUIRE_TIMEMACHINE_IDLE=1" > "$WORK/gate.out" 2>"$WORK/gate.err" || G_RC=$?
G_ATT="$TMB/att/window-provenance.json"
[ "$G_RC" = "3" ] && [ "$(av quiet.timemachine)" = "FAIL" ] \
  && ok "C-b: Time Machine RUNNING refuses the gate — exit 3, named" \
  || bad "C-b: timemachine-running rc=$G_RC verdict '$(av quiet.timemachine)'"
printf '%s' "$(ad quiet.timemachine)" | grep -qF "timings will be contaminated" \
  && ok "C-b: and the diagnostic says why a backup disqualifies the window" \
  || bad "C-b: timemachine diagnostic lacks its reason"
# ...and the waived counterpart, so the pin is shown to be what decides it — same as the stray check.
G_RC=0; PATH="$TMBIN:$PATH" bash "$GATE" --pins "$WORK/pins-tm" \
  --pin "WP_REQUIRE_TIMEMACHINE_IDLE=0" > "$WORK/gate.out" 2>"$WORK/gate.err" || G_RC=$?
G_ATT="$TMB/att/window-provenance.json"
{ [ "$G_RC" = "0" ] && [ "$(av quiet.timemachine)" = "NOTE" ]; } \
  && ok "C-b: and WP_REQUIRE_TIMEMACHINE_IDLE=0 waives it explicitly (recorded, not silent)" \
  || bad "C-b: waived timemachine rc=$G_RC verdict '$(av quiet.timemachine)'"
release_after "$WORK/pins-tm" --pin "WP_REQUIRE_TIMEMACHINE_IDLE=0"

# Reaping moves the corpse aside rather than deleting it, so the evidence survives.
ls -d "$CONCLOCK".reaped.* >/dev/null 2>&1 \
  && ok "N-1: the reaped lock is MOVED aside (rename is atomic; the remains stay inspectable)" \
  || bad "N-1: no reaped-aside directory found"
[ ! -e "$CONCLOCK.reapmutex" ] && ok "N-1: the reap mutex is released" || bad "N-1: reap mutex leaked"
rm -rf "$CONCLOCK" "$CONCLOCK".reaped.* 2>/dev/null
echo ""

echo "== 14. box-side signal trap: reload BEFORE release (N-4) =="
# An ssh drop delivers SIGHUP to the box-side probe. Releasing alone left the box unlocked and
# NOT SERVING — silently, because the gate's unwind then saw `not-held` and short-circuited
# before its own reload block.
# A process started under `nohup` (or any parent that ignored SIGHUP) inherits SIG_IGN, and a
# shell CANNOT trap a signal that was ignored at exec time — `trap ... HUP` silently does
# nothing. This case would then fail with the probe running to normal completion and an empty
# handler log, which reads exactly like a regression in the handler itself. Detect it and say so.
# Probed in a SEPARATE bash process: inside a `( )` subshell `$$` is still the PARENT's pid, so
# testing it that way signals the suite itself rather than the probe (it exits 129 mid-run).
if [ "$(bash -c 'trap "echo TRAPPED; exit 0" HUP; kill -HUP $$ 2>/dev/null; sleep 0.5; echo IGNORED' 2>/dev/null)" = "IGNORED" ]; then
  bad "N-4: SIGHUP is IGNORED in this environment (started under nohup?) — the trap cannot be tested here"
  echo "        Re-run WITHOUT nohup: the handler is untestable when SIGHUP arrives as SIG_IGN."
fi
SIGB="$WORK/box-sig"; mkbox "$SIGB"
SIGTAG="$(cat "$SIGB/.qwen-tag")"
SIGLOCK="$SIGB/box.lock.d"; rm -rf "$SIGLOCK"
bash "$PROBE" "$(mkreq "mode=window" "session_lock=$SIGLOCK" "window_tag=sigtest" \
  "qwen_service=$SIGB/qwen-service.sh" "qwen_proc_pattern=$SIGTAG" \
  "smoke_argv=sleep 30" "smoke_timeout_s=30" "lock_reap_age_s=60")" \
  > "$WORK/sig.rec" 2>"$WORK/sig.err" &
SIGPID=$!
# Wait until it has acquired and unloaded (the stand-in process is gone).
n=0; while [ "$n" -lt 40 ]; do
  [ -d "$SIGLOCK" ] && [ -z "$(pgrep -f "$SIGTAG" 2>/dev/null)" ] && break
  sleep 0.25; n=$((n + 1))
done
[ -d "$SIGLOCK" ] && ok "N-4 fixture: the probe acquired the lock" || bad "N-4 fixture: no lock taken"
[ -z "$(pgrep -f "$SIGTAG" 2>/dev/null)" ] \
  && ok "N-4 fixture: and unloaded the serving model" || bad "N-4 fixture: model still resident"
# Let the probe settle into the smoke phase before signalling. Without this the HUP can land
# in the narrow window between the model disappearing and the probe observing it, which makes
# the case race rather than test anything.
sleep 1
# PRECONDITION: the probe must still be running. A probe that has already exited keeps the lock
# on purpose (a normal exit is the handoff), so signalling a dead pid would fail all three
# assertions below for a reason that has nothing to do with the signal handler. Under a full
# suite this is the difference between a real regression and noise, so it is checked, not assumed.
if kill -0 "$SIGPID" 2>/dev/null; then
  ok "N-4 fixture: the probe is still running when the signal is sent"
else
  bad "N-4 fixture: the probe had ALREADY EXITED before the signal — the case below tests nothing"
  sed 's/^/        /' "$WORK/sig.err" | tail -6
fi
kill -HUP "$SIGPID" 2>/dev/null
wait "$SIGPID" 2>/dev/null
if [ ! -e "$SIGLOCK" ]; then
  ok "N-4: SIGHUP released the lock"
else
  bad "N-4: the lock survived the signal"
  # Dump everything needed to tell a handler regression from a fixture problem, because this
  # case only misbehaves under a full suite and $WORK is gone by the time anyone reads the log.
  echo "        --- probe stderr ---"; sed 's/^/        /' "$WORK/sig.err" | tail -12
  echo "        --- probe exit / lock state ---"
  echo "        lock pid file: [$(cat "$SIGLOCK/pid" 2>/dev/null)]  this suite: [$$]  probe: [$SIGPID]"
  echo "        probe still alive: $(kill -0 "$SIGPID" 2>/dev/null && echo yes || echo no)"
  for _k in lock.acquired lock.create_failed lock.reap_refused qwen.state smoke.rc probe.ok; do
    echo "        $_k = [$(probe_obs "$WORK/sig.rec" "$_k")]"
  done
fi
n=0; BACK=0
while [ "$n" -lt 20 ]; do
  [ -n "$(pgrep -f "$SIGTAG" 2>/dev/null)" ] && { BACK=1; break; }
  sleep 0.25; n=$((n + 1))
done
[ "$BACK" = "1" ] \
  && ok "N-4: and RELOADED the serving model before releasing (was: unlocked + NOT SERVING, silently)" \
  || bad "N-4: the box was left unlocked and NOT SERVING after the signal"
grep -q "reloading the serving model before releasing" "$WORK/sig.err" \
  && ok "N-4: the handler says so on stderr" || bad "N-4: no reload message from the signal handler"
pkill -f "$SIGTAG" 2>/dev/null
echo ""

echo "== 15. provisioning a bundle over the ssh DRIVER (N-2) =="
# Every bundle case above ran --driver local, which bypasses the box-side mktemp entirely —
# which is why an mktemp template that is INVALID on BSD/macOS survived review. On the target
# platform it returned empty and the whole bundle-over-ssh path (Proof A's own case) died at
# exit 9 having shipped nothing.
[ -x "$SSHBIN/ssh" ] || { mkdir -p "$SSHBIN"; }
cat > "$SSHBIN/scp" <<'PEOF'
#!/bin/bash
# stand-in for scp: strip the -q flag and the `host:` prefix, then copy.
args=(); for a in "$@"; do [ "$a" = "-q" ] && continue; args+=("$a"); done
src="${args[0]}"; dst="${args[1]}"; dst="${dst#*:}"
cp "$src" "$dst"
PEOF
chmod +x "$SSHBIN/scp"
NB="$WORK/box-nbundle"; mkbox "$NB"; genpins "$NB" "$WORK/pins-nb" none
git -C "$NB/bench" bundle create "$WORK/nb.bundle" --all >/dev/null 2>&1
NBSHA="$(sha_of "$WORK/nb.bundle")"; NBCOMMIT="$(git -C "$NB/bench" rev-parse HEAD)"
{ grep -v '^WP_BENCH_PATH=\|^WP_BENCH_SHA=\|^WP_DRIVER=\|^WP_BOX=' "$WORK/pins-nb"
  printf 'WP_DRIVER=ssh\nWP_BOX=offline-fixture-box\n'
  printf 'WP_BENCH_PATH=%s\n' "$NB/bench-from-bundle"
  printf 'WP_BENCH_SHA=%s\n' "$NBCOMMIT"
  printf 'WP_BENCH_BUNDLE=%s\n' "$WORK/nb.bundle"
  printf 'WP_BENCH_BUNDLE_SHA256=%s\n' "$NBSHA"
} > "$WORK/pins-nb2"
NBRC=0
PATH="$SSHBIN:$PATH" bash "$PROV" --pins "$WORK/pins-nb2" > "$WORK/nb.out" 2>&1 || NBRC=$?
[ "$NBRC" = "0" ] \
  && ok "N-2: the bundle-over-ssh path provisions (was: exit 9, nothing shipped, on BSD/macOS)" \
  || { bad "N-2: bundle-over-ssh rc=$NBRC"; tail -8 "$WORK/nb.out"; }
[ -d "$NB/bench-from-bundle" ] \
  && ok "N-2: the tree was actually created from the shipped bundle" || bad "N-2: no tree created"
[ "$(git -C "$NB/bench-from-bundle" rev-parse HEAD 2>/dev/null)" = "$NBCOMMIT" ] \
  && ok "N-2: at the pinned commit" || bad "N-2: wrong commit"
grep -q "bundle_sha256=$NBSHA" \
  "$(git -C "$NB/bench-from-bundle" rev-parse --absolute-git-dir 2>/dev/null)/window-bundle-provenance" 2>/dev/null \
  && ok "N-2: and the bundle hash was recorded box-side" || bad "N-2: no box-side bundle record"

# BEHAVIOURAL, through the flattening emulator, replacing that grep. Provision a bundle over the
# ssh driver: the `remote` field is EMPTY on the bundle path and sits BEFORE `role` and `DRY` in
# the envelope. If the payload passed N positionals, ssh's flattening would drop the empty and
# shift the rest down — so asserting the tree exists AT THE PINNED COMMIT tests both that the
# empty survived and that nothing after it moved.
rm -rf "$NB/bench-a3"
{ grep -v '^WP_BENCH_PATH=\|^WP_BENCH_SOURCE_CLONE=' "$WORK/pins-nb2"
  printf 'WP_BENCH_PATH=%s\n' "$NB/bench-a3"
} > "$WORK/pins-a3"
A3_RC=0
PATH="$SSHBIN:$PATH" bash "$PROV" --pins "$WORK/pins-a3" > "$WORK/a3.out" 2>&1 || A3_RC=$?
{ [ "$A3_RC" = "0" ] && [ -d "$NB/bench-a3/.git" ]; } \
  && ok "A-3: an EMPTY field survives the REAL ssh flattening (bundle provisioned, not skipped)" \
  || { bad "A-3: the empty-field envelope did not survive flattening (rc=$A3_RC)"; tail -8 "$WORK/a3.out"; }
[ "$(git -C "$NB/bench-a3" rev-parse HEAD 2>/dev/null)" = "$NBCOMMIT" ] \
  && ok "A-3: and the fields AFTER the empty one did not shift (tree at the pinned commit)" \
  || bad "A-3: post-empty fields shifted — HEAD is not the pinned commit"

# M5: the box-side bundle re-digest REFUSAL. The bundle rule is tested from several angles, but
# nothing ever shipped a bundle whose sha did not match its pin and watched the box reject it —
# so the refusal branch was unreachable in test, and a mutation downgrading it to a warning
# survived the whole suite. This is the check that stops a swapped bundle becoming a tree.
rm -rf "$NB/bench-m5"
{ grep -v '^WP_BENCH_PATH=\|^WP_BENCH_BUNDLE_SHA256=\|^WP_BENCH_SOURCE_CLONE=' "$WORK/pins-nb2"
  printf 'WP_BENCH_PATH=%s\n' "$NB/bench-m5"
  printf 'WP_BENCH_BUNDLE_SHA256=%s\n' "$BOGUS64"
} > "$WORK/pins-m5"
M5_RC=0
PATH="$SSHBIN:$PATH" bash "$PROV" --pins "$WORK/pins-m5" > "$WORK/m5.out" 2>&1 || M5_RC=$?
[ "$M5_RC" = "7" ] \
  && ok "battery-M5: a bundle whose sha256 does not match its pin is REFUSED on the box (exit 7)" \
  || { bad "battery-M5: mismatched bundle gave rc=$M5_RC, want 7"; tail -6 "$WORK/m5.out"; }
grep -q "REFUSE" "$WORK/m5.out" \
  && ok "battery-M5: and the refusal SAYS so, naming both the computed and the pinned digest" \
  || bad "battery-M5: no REFUSE in the provisioning output"
[ ! -d "$NB/bench-m5/.git" ] \
  && ok "battery-M5: and NO tree was created from the unverified bundle" \
  || bad "battery-M5: a tree was built from a bundle that failed its pin"
# The case above is caught on the LAPTOP, before anything ships — so it does not reach the
# box-side re-digest at all, and a mutation that downgrades the BOX-side refusal to a warning
# still survives it. The box-side check exists for the case the laptop cannot see: the bundle was
# correct when it left and is not what arrived. Emulate exactly that — a transfer that delivers
# different bytes than it was handed — so the laptop check passes and only the box can catch it.
CORRUPT="$WORK/corruptbin"; mkdir -p "$CORRUPT"; cp "$SSHBIN/ssh" "$CORRUPT/ssh"
cat > "$CORRUPT/scp" <<'CEOF'
#!/bin/bash
# scp that DELIVERS CORRUPTION: the source is intact (so the laptop-side digest matched), but
# what lands on the far side has an extra byte — a truncated or rewritten transfer.
args=(); for a in "$@"; do [ "$a" = "-q" ] && continue; args+=("$a"); done
src="${args[0]}"; dst="${args[1]}"; dst="${dst#*:}"
cp "$src" "$dst"; printf 'X' >> "$dst"
CEOF
chmod +x "$CORRUPT/scp"
rm -rf "$NB/bench-m5b"
{ grep -v '^WP_BENCH_PATH=\|^WP_BENCH_SOURCE_CLONE=' "$WORK/pins-nb2"
  printf 'WP_BENCH_PATH=%s\n' "$NB/bench-m5b"
} > "$WORK/pins-m5b"
M5B_RC=0
PATH="$CORRUPT:$PATH" bash "$PROV" --pins "$WORK/pins-m5b" > "$WORK/m5b.out" 2>&1 || M5B_RC=$?
grep -q "bundle sha256 verified on the laptop" "$WORK/m5b.out" \
  && ok "battery-M5: the laptop-side digest PASSES (the corruption happens in transit)" \
  || bad "battery-M5: the laptop check did not pass — this case is not reaching the box-side one"
[ "$M5B_RC" = "7" ] \
  && ok "battery-M5: and the BOX re-digests what ARRIVED and refuses it (exit 7)" \
  || { bad "battery-M5: corrupt-in-transit bundle gave rc=$M5B_RC, want 7"; tail -6 "$WORK/m5b.out"; }
grep -q "REFUSE: bundle sha256 on the box" "$WORK/m5b.out" \
  && ok "battery-M5: and the refusal is the BOX's own, naming the digest it computed" \
  || bad "battery-M5: no box-side REFUSE in the output"
[ ! -d "$NB/bench-m5b/.git" ] \
  && ok "battery-M5: and no tree was built from the bytes that actually landed" \
  || bad "battery-M5: a tree was built from a corrupted bundle"

# N-3: --dry-run must not digest a path it never shipped, nor invent a REFUSE from it.
rm -rf "$NB/bench-dry"
{ grep -v '^WP_BENCH_PATH=' "$WORK/pins-nb2"; printf 'WP_BENCH_PATH=%s\n' "$NB/bench-dry"; } \
  > "$WORK/pins-nb3"
NDRC=0
PATH="$SSHBIN:$PATH" bash "$PROV" --pins "$WORK/pins-nb3" --dry-run > "$WORK/nd.out" 2>&1 || NDRC=$?
[ "$NDRC" = "0" ] \
  && ok "N-3: --dry-run over ssh exits 0 (was: fabricated REFUSE, exit 7)" \
  || { bad "N-3: dry-run rc=$NDRC"; tail -6 "$WORK/nd.out"; }
grep -q "DRY-RUN: would verify the bundle sha256" "$WORK/nd.out" \
  && ok "N-3: and says what it WOULD have verified" || bad "N-3: no dry-run verification notice"
[ ! -e "$NB/bench-dry" ] && ok "N-3: --dry-run created nothing" || bad "N-3: dry-run created a tree"

# N-2 remainder (a): the box-side temp must not leak — not on success, not on failure.
# Scoped to a TMPDIR of THIS suite's own, never the shared one: globbing the real TMPDIR counted
# temps belonging to any other process on the machine, so the assertion reported another
# session's files as our leak.
BOXTMP="$WORK/boxtmp"; mkdir -p "$BOXTMP"
leaked_ct() { ls -d "$BOXTMP"/window-provision.?????? 2>/dev/null | wc -l | tr -d ' '; }
rm -rf "$NB/bench-nb3"
{ grep -v '^WP_BENCH_PATH=' "$WORK/pins-nb2"; printf 'WP_BENCH_PATH=%s\n' "$NB/bench-nb3"; } > "$WORK/pins-nb3b"
TMPDIR="$BOXTMP" PATH="$SSHBIN:$PATH" bash "$PROV" --pins "$WORK/pins-nb3b" > "$WORK/nl2.out" 2>&1
LEAKED="$(leaked_ct)"
[ "$LEAKED" = "0" ] \
  && ok "N-2: no box-side temp leaked by the successful or dry runs" \
  || bad "N-2: $LEAKED box-side temp(s) left behind"
# Failure path: an unwritable destination makes the box-side payload fail AFTER the temp exists.
rm -rf "$NB/bench-fail"
{ grep -v '^WP_BENCH_PATH=\|^WP_BENCH_SOURCE_CLONE=' "$WORK/pins-nb2"
  printf 'WP_BENCH_PATH=%s\n' "$NB/nonexistent-parent/deep/tree"
} > "$WORK/pins-nb4"
TMPDIR="$BOXTMP" PATH="$SSHBIN:$PATH" bash "$PROV" --pins "$WORK/pins-nb4" > "$WORK/nf.out" 2>&1
LEAKED2="$(leaked_ct)"
[ "$LEAKED2" = "0" ] \
  && ok "N-2: and none leaked when the box-side payload FAILED" \
  || bad "N-2: $LEAKED2 temp(s) leaked on the failure path"

# N-2 remainder (b): the pre-fix template left a LITERAL, world-guessable file. It is inert for
# the new template but must be surfaced, and must NOT be deleted blind (that is the very
# symlink-swap hazard the unpredictable name exists to avoid).
STALE="$BOXTMP/window-provision.XXXXXX.bundle"
: > "$STALE"
TMPDIR="$BOXTMP" PATH="$SSHBIN:$PATH" bash "$PROV" --pins "$WORK/pins-nb3" --dry-run > "$WORK/ns.out" 2>&1
grep -q "leftover from the pre-fix mktemp template" "$WORK/ns.out" \
  && ok "N-2: a pre-existing literal-XXXXXX.bundle leftover is REPORTED by path" \
  || bad "N-2: the pre-fix leftover was not surfaced"
[ -e "$STALE" ] \
  && ok "N-2: and is NOT deleted (a world-guessable path is never removed blind)" \
  || bad "N-2: the leftover was deleted despite the guessable-path hazard"
rm -f "$STALE"
echo ""

# ---------------------------------------------------------------------------
PASS="$(tally pass)"; FAIL="$(tally fail)"; SKIP="$(tally skip)"

# MANIFEST. Two whole case blocks (M-b, and X-1's third ruling) were silently DELETED by an
# editing slip and nobody noticed for a full review round: the suite still passed, the count
# still rose because other cases were added alongside, and reviewing only the FAIL lines could
# never have caught it. A count is not evidence that a specific thing was tested. Each entry
# below is a case that must have RUN — if one disappears again, this fails loudly and names it.
MANIFEST_MISSING=""
for _m in \
  "X-1: an ADOPTED lock older than the reap age is REFUSED" \
  "X-1: once the adopting driver DIES the lock is reapable again" \
  "X-1: an UNADOPTED handoff self-heals once aged out" \
  "X-1: the driver ADOPTS the handed-off lock" \
  "X-2: --release with a PINNED-but-ABSENT qwen service fails closed" \
  "M-a: the attestation names the moved-aside lock" \
  "M-b: a bundle matching its pin but NOT containing the claimed commit is REFUSED" \
  "C-a: a var pinned 'unset' that is EXPORTED fails" \
  "C-a: a var pinned to a VALUE but exported with another fails" \
  "C-b: a stray model process REFUSES the gate" \
  "C-b: Time Machine RUNNING refuses the gate" \
  "C-c: --provision provisions the tree named by a --pin OVERRIDE" \
  "C-d: the 'both' recipe really invokes BOTH verbs" \
  "F3: a STALE reap mutex whose holder is gone is reclaimed" \
  "battery-M8: an ancient, reap-age-eligible lock whose holder is ALIVE is NOT reaped" \
  "X-2: a service file defining NO qwen_reload also fails closed at release" \
  "X-2: the WINDOW refuses a functions-missing service pin" \
  "X-2: and says NOT ATTEMPTED rather than blaming a model that was never resident" \
  "C-b: and WP_REQUIRE_NO_STRAY=0 waives it explicitly" \
  "C-b: and WP_REQUIRE_TIMEMACHINE_IDLE=0 waives it explicitly" \
  "C-c: --provision exits 0" \
  "NEW-1 (deterministic): a vanished contender is NOT misclassified as create-failure" \
  "NEW-1 (deterministic): and the probe retries and ACQUIRES the released lock" \
; do
  grep -qF -- "$_m" "$TALLY_DIR/pass" "$TALLY_DIR/fail" 2>/dev/null || MANIFEST_MISSING="$MANIFEST_MISSING
  $_m"
done
printf '== window-preflight offline: %d passed, %d failed, %d skipped ==\n' "$PASS" "$FAIL" "$SKIP"
if [ -n "$MANIFEST_MISSING" ]; then
  printf 'FAIL: case(s) named in the manifest never ran — a block was deleted or renamed:%s\n' \
    "$MANIFEST_MISSING" >&2
  exit 1
fi
[ "$FAIL" -eq 0 ] || exit 1
# A SKIP is NOT a pass. The smoke matrix silently substituted an always-pass stub benchd when
# target/release/benchctl was absent, so the whole real-transport section could "succeed"
# without ever exercising the transport. Anything skipped is an untested claim.
if [ "$SKIP" -gt 0 ]; then
  printf 'FAIL: %d check(s) were SKIPPED — a skipped check is an untested claim, not a pass.\n' "$SKIP" >&2
  printf '      Build benchctl (cargo build --release -p benchctl) and re-run.\n' >&2
  exit 1
fi
exit 0
