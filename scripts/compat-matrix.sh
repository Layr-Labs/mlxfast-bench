#!/usr/bin/env bash
# compat-matrix.sh — M-7 acceptance gate: prove the benchctl FACADE
# (scripts/benchmark.sh) is byte-green against the Swift reference benchmark.sh on
# the observable shell contract. OFFLINE — needs NO GPU and NO real engine.
#
# Three parts (all must PASS):
#   1. ARG-ERROR PARITY — for each shared early-exit case, run BOTH the real
#      reference benchmark.sh and the facade with identical argv/env; assert
#      byte-identical STDERR and identical exit code. These cases exit before any
#      GPU/transform work in BOTH.
#   2. SUMMARY BYTE-PARITY — feed a canned score.local-iterate.json and assert the
#      facade's end-to-end STDERR summary block is byte-identical to what the
#      reference's own jq program produces on the same file. Plus a DRIFT GUARD that
#      diffs the facade's report_local_score_summary against the reference's — so a
#      reference update can't silently break parity.
#   3. EXIT-CODE MAPPING — a stub benchctl exiting 2 must map to facade exit 1;
#      rc=1 -> 1; rc=0 -> 0 (with the summary path exercised).
#
# Fails LOUD, never silently passes. Exit 0 = all green; non-zero = any mismatch or
# harness/setup error.
#
# Config (env):
#   REFERENCE_BENCHMARK_SH  path to the Swift reference benchmark.sh
#                           (REQUIRED — no default; the gate aborts if unset)
#   FACADE                  path to the facade (default: scripts/benchmark.sh here)
set -uo pipefail

SELF_DIR="$(cd -P "$(dirname "$0")" && pwd -P)"
FACADE="${FACADE:-${SELF_DIR}/benchmark.sh}"
REFERENCE_BENCHMARK_SH="${REFERENCE_BENCHMARK_SH:-}"
[ -n "$REFERENCE_BENCHMARK_SH" ] || { echo "compat-matrix: FATAL set REFERENCE_BENCHMARK_SH=<path to the Swift reference benchmark.sh>" >&2; exit 2; }
FIXTURE="${SELF_DIR}/fixtures/facade/score.local-iterate.json"
BASELINE_FIXTURE="${SELF_DIR}/fixtures/facade/score.local-iterate.baseline.json"

command -v jq >/dev/null || { echo "compat-matrix: FATAL jq required" >&2; exit 2; }
[ -r "$FACADE" ] || { echo "compat-matrix: FATAL facade not readable: $FACADE" >&2; exit 2; }
[ -r "$REFERENCE_BENCHMARK_SH" ] || { echo "compat-matrix: FATAL reference not readable: $REFERENCE_BENCHMARK_SH" >&2; exit 2; }
[ -r "$FIXTURE" ] || { echo "compat-matrix: FATAL fixture not readable: $FIXTURE" >&2; exit 2; }
[ -r "$BASELINE_FIXTURE" ] || { echo "compat-matrix: FATAL baseline fixture not readable: $BASELINE_FIXTURE" >&2; exit 2; }

# Scrub any inherited MLXFAST_* so both scripts see an identical clean baseline; each
# case sets exactly what it needs inline.
while IFS= read -r v; do [ -n "$v" ] && unset "$v"; done < <(env | sed -n 's/^\(MLXFAST_[A-Za-z0-9_]*\)=.*/\1/p')

WORK="$(mktemp -d "${TMPDIR:-/tmp}/compat-matrix.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  PASS  %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; }

echo "compat-matrix: facade    = $FACADE"
echo "compat-matrix: reference = $REFERENCE_BENCHMARK_SH"
echo ""

# ---- Part 1: ARG-ERROR PARITY ---------------------------------------------------
# Each case is: label | env-prefix (KEY=VAL, space-sep, or '-') | argv...
# All five exit BEFORE any GPU/transform work in BOTH scripts, so their STDERR and
# exit code are directly comparable byte-for-byte.
echo "Part 1: arg-error parity (reference vs facade, identical argv/env)"

run_capture() {  # $1=script  $2=env-prefix  rest=argv ; writes stderr file path to REPLY_ERR, sets REPLY_RC
  local script="$1" envp="$2"; shift 2
  local errf="${WORK}/err.$$"
  if [ "$envp" = "-" ]; then
    ( cd "$WORK" && "$script" "$@" ) >/dev/null 2>"$errf"
  else
    # shellcheck disable=SC2086
    ( cd "$WORK" && env $envp "$script" "$@" ) >/dev/null 2>"$errf"
  fi
  REPLY_RC=$?
  REPLY_ERR="$errf"
}

compare_case() {  # $1=label  $2=env-prefix  rest=argv
  local label="$1" envp="$2"; shift 2
  run_capture "$REFERENCE_BENCHMARK_SH" "$envp" "$@"; local ref_rc=$REPLY_RC; local ref_err="${WORK}/ref.err"; cp "$REPLY_ERR" "$ref_err"
  run_capture "$FACADE"                 "$envp" "$@"; local fac_rc=$REPLY_RC; local fac_err="${WORK}/fac.err"; cp "$REPLY_ERR" "$fac_err"
  if [ "$ref_rc" != "$fac_rc" ]; then
    bad "$label — exit code differs (ref=$ref_rc facade=$fac_rc)"; return
  fi
  if ! diff -u "$ref_err" "$fac_err" >"${WORK}/diff.out" 2>&1; then
    bad "$label — STDERR differs (exit=$ref_rc):"; sed 's/^/        /' "${WORK}/diff.out"; return
  fi
  ok "$label (identical STDERR, exit=$ref_rc)"
}

compare_case "shell --weights rejected"                 "-" --weights
compare_case "--official + --local-iterate combo error" "-" --official --local-iterate
compare_case "--local-iterate + --local-submit error"   "-" --local-iterate --local-submit
compare_case "missing golden env heredoc"               "-" --local-iterate
compare_case "golden file not found"                    "MLXFAST_CORRECTNESS_GOLDEN_PATH=${WORK}/does-not-exist.json" --local-iterate
echo ""

# ---- Shared stub + golden for the end-to-end facade runs below -------------------
# Stub benchctl: copy the canned fixture to whatever --score-path it's handed, echo
# the sealed payload on stdout (as real benchctl does), then exit STUB_EXIT.
STUB="${WORK}/benchctl-stub.sh"
cat > "$STUB" <<'STUBEOF'
#!/usr/bin/env bash
sp=""
while [ $# -gt 0 ]; do case "$1" in --score-path) sp="$2"; shift 2;; *) shift;; esac; done
if [ -n "${STUB_SCORE_SRC:-}" ]; then
  [ -n "$sp" ] && cp "$STUB_SCORE_SRC" "$sp"
  cat "$STUB_SCORE_SRC"
fi
exit "${STUB_EXIT:-0}"
STUBEOF
chmod +x "$STUB"
# A dummy golden file so the facade's -f golden check passes (benchctl is stubbed, so
# the golden's contents are irrelevant here).
GOLDEN="${WORK}/golden.json"; printf '{}' > "$GOLDEN"

# run_facade — drive the facade end-to-end with the stub benchctl. $1 = run subdir;
# $2 = mode arg ("--local-iterate"/"--local-submit", or "" for the no-mode default
# path); $3.. = extra files copied into the run dir first (e.g. a baseline snapshot).
# Writes STDERR to ${WORK}/<subdir>.err and STDOUT to ${WORK}/<subdir>.out. Honors
# STUB_EXIT (default 0) from the caller's environment.
run_facade() {  # $1=subdir  $2=mode-arg  $3..=files to seed into the run dir
  local sub="$1" mode="$2"; shift 2
  local runf="${WORK}/${sub}"; rm -rf "$runf"; mkdir -p "$runf"
  local f; for f in "$@"; do cp "$f" "${runf}/"; done
  local args=(); [ -n "$mode" ] && args=("$mode")
  ( cd "$runf" && \
    env BENCHCTL="$STUB" STUB_SCORE_SRC="$FIXTURE" STUB_EXIT="${STUB_EXIT:-0}" \
        MLXFAST_ENGINE_BIN="/usr/bin/true" \
        MLXFAST_CORRECTNESS_GOLDEN_PATH="$GOLDEN" \
        MLXFAST_WEIGHTS_PATH="${runf}/weights" \
        "$FACADE" ${args[@]+"${args[@]}"} ) >"${WORK}/${sub}.out" 2>"${WORK}/${sub}.err"
}

# ---- Part 1b: no-mode default STDOUT line parity --------------------------------
# The reference prints `benchmark.sh: no mode given; defaulting to --local-iterate …`
# on STDOUT (not stderr). A full no-mode run then diverges (the reference proceeds to
# the real Swift run; the facade dispatches to benchctl), so compare ONLY that first
# STDOUT line byte-for-byte. Reference: MLXFAST_IN_SANDBOX=1 skips the build/sandbox
# machinery and a missing Swift binary makes it exit right after the default line.
echo "Part 1b: no-mode default STDOUT line parity"
refd="${WORK}/nomode-ref"; mkdir -p "$refd"
( cd "$refd" && env -u MLXFAST_OFFICIAL_BENCHMARK_RUN \
    MLXFAST_IN_SANDBOX=1 \
    MLXFAST_CORRECTNESS_GOLDEN_PATH="$GOLDEN" \
    MLXFAST_SWIFT_BIN="${refd}/no-such-swift-binary" \
    "$REFERENCE_BENCHMARK_SH" ) >"${WORK}/nomode-ref.out" 2>/dev/null
run_facade "nomode-fac" ""
head -n1 "${WORK}/nomode-ref.out" > "${WORK}/nomode-ref.line"
head -n1 "${WORK}/nomode-fac.out" > "${WORK}/nomode-fac.line"
if [ ! -s "${WORK}/nomode-ref.line" ]; then
  bad "no-mode default STDOUT line — reference produced no STDOUT (harness/setup issue)"
elif diff -u "${WORK}/nomode-ref.line" "${WORK}/nomode-fac.line" >"${WORK}/nomode.diff" 2>&1; then
  ok "no-mode default STDOUT line (reference vs facade byte-identical)"
else
  bad "no-mode default STDOUT line — reference vs facade differ:"; sed 's/^/        /' "${WORK}/nomode.diff"
fi
echo ""

# ---- Part 1c: facade --official full-parity surface (R22) -----------------------
# R22 retracted the facade's --official refusal: --official now routes to the benchctl
# official backend, exactly as the reference runs its own official path. These rows exercise
# that surface. The arg-exclusivity message and the two enforce_official_sandbox refusals run
# BEFORE any engine work in BOTH scripts, so they are byte-vs-reference. The gates-only SHAPE
# needs a real engine in the reference (GPU) and cannot run offline, so it is a facade-only
# route+passthrough assertion (with a stub benchctl), not a byte-vs-reference row.
echo "Part 1c: facade --official surface (R22)"
OFFICIAL_GOLDEN_FIX="${WORK}/official-golden.json"; printf '{}' > "$OFFICIAL_GOLDEN_FIX"

# arg-exclusivity: --official cannot combine with --local-submit (byte-matched existing message).
compare_case "--official + --local-submit combo error" "-" --official --local-submit

# enforce_official_sandbox refusals — both scripts fire them in official mode BEFORE any engine
# work (ref 671/675; the facade fires them for OFFICIAL=1). Identical argv/env → identical
# stderr + exit. MLXFAST_ENGINE_BIN is read only by the facade (harmless to the reference).
compare_case "official NO_SANDBOX refusal (byte-vs-reference)" \
  "MLXFAST_OFFICIAL_BENCHMARK_RUN=1 MLXFAST_NO_SANDBOX=1 MLXFAST_CORRECTNESS_GOLDEN_PATH=${OFFICIAL_GOLDEN_FIX} MLXFAST_ENGINE_BIN=/usr/bin/true" \
  --official
compare_case "official runtime-worker refusal (byte-vs-reference)" \
  "MLXFAST_OFFICIAL_BENCHMARK_RUN=1 MLXFAST_USE_RUNTIME_WORKER=0 MLXFAST_CORRECTNESS_GOLDEN_PATH=${OFFICIAL_GOLDEN_FIX} MLXFAST_ENGINE_BIN=/usr/bin/true" \
  --official

# Facade-only (the reference's real --official engine run cannot run offline): assert the facade
# ROUTES --official to the benchctl official backend (`--mode official`) and passes the sealed
# gates-only payload (partial_result=true, under MLXFAST_BENCHMARK_SKIP_TIMED=1) straight through.
OFFICIAL_STUB="${WORK}/benchctl-official-stub.sh"
cat > "$OFFICIAL_STUB" <<'STUBEOF'
#!/usr/bin/env bash
echo "OFFICIAL-STUB-ARGS: $*" >&2
sp=""; while [ $# -gt 0 ]; do case "$1" in --score-path) sp="$2"; shift 2;; *) shift;; esac; done
payload='{"score":null,"passed":true,"metrics":{"partial_result":true,"passed_correctness":true,"error":""}}'
[ -n "$sp" ] && printf '%s' "$payload" > "$sp"
printf '%s' "$payload"
STUBEOF
chmod +x "$OFFICIAL_STUB"
offdir="${WORK}/official-route"; mkdir -p "$offdir"
( cd "$offdir" && env BENCHCTL="$OFFICIAL_STUB" MLXFAST_ENGINE_BIN=/usr/bin/true \
    MLXFAST_CORRECTNESS_GOLDEN_PATH="$OFFICIAL_GOLDEN_FIX" MLXFAST_WEIGHTS_PATH="${offdir}/weights" \
    MLXFAST_BENCHMARK_CHECK_GATES=1 MLXFAST_BENCHMARK_SKIP_TIMED=1 MLXFAST_SCORE_PATH="${offdir}/gates-score.json" \
    "$FACADE" --official ) >"${offdir}.out" 2>"${offdir}.err"; off_rc=$?
if grep -Eq 'OFFICIAL-STUB-ARGS: iterate .*--mode official' "${offdir}.err"; then
  ok "facade --official routes to 'benchctl iterate --mode official' (no --cool-gate)"
else
  bad "facade --official did NOT route to --mode official"; sed 's/^/        /' "${offdir}.err"
fi
if [ "$off_rc" = 0 ] && jq -e '.metrics.partial_result == true and .passed == true' "${offdir}.out" >/dev/null 2>&1; then
  ok "facade --official gates-only: partial_result=true payload passes through untouched"
else
  bad "facade --official gates-only shape not passed through (rc=$off_rc)"; sed 's/^/        /' "${offdir}.out"
fi
# The facade must NOT pass the LOCAL --cool-gate to the official backend (official has no local gate).
if grep -q -- '--cool-gate' "${offdir}.err"; then
  bad "facade --official forwarded --cool-gate to the official backend (must not)"
else
  ok "facade --official does not forward the local --cool-gate"
fi
echo ""

# ---- Part 2: SUMMARY BYTE-PARITY ------------------------------------------------
echo "Part 2: summary byte-parity + drift guard"

# 2a. DRIFT GUARDS: the facade's summary functions must be byte-identical to the
# reference's (function open-brace to close-brace). Catches any reference drift in the
# jq programs OR the echo/printf framing that the end-to-end diffs might otherwise
# only see for exercised branches.
extract_fn() {  # $1=file $2=function-name  -> prints the function body inclusive
  awk -v fn="$2" '
    $0 ~ ("^" fn "\\(\\) \\{") {f=1}
    f {print}
    f && $0 == "}" {exit}
  ' "$1"
}
drift_guard_fn() {  # $1=function-name
  local fn="$1"
  extract_fn "$REFERENCE_BENCHMARK_SH" "$fn" > "${WORK}/ref.${fn}"
  extract_fn "$FACADE"                 "$fn" > "${WORK}/fac.${fn}"
  if [ ! -s "${WORK}/ref.${fn}" ] || [ ! -s "${WORK}/fac.${fn}" ]; then
    bad "drift guard — could not extract ${fn} from both files"
  elif diff -u "${WORK}/ref.${fn}" "${WORK}/fac.${fn}" >"${WORK}/${fn}.diff" 2>&1; then
    ok "drift guard — facade ${fn} == reference (byte-identical)"
  else
    bad "drift guard — facade ${fn} DIVERGED from reference:"; sed 's/^/        /' "${WORK}/${fn}.diff"
  fi
}
drift_guard_fn report_local_score_summary
drift_guard_fn report_local_baseline_context

# Extract the reference jq PROGRAM bodies standalone (between each `jq …'` opener and
# its `' …' closer) so we can reproduce the reference's exact output on our fixtures.
awk '
  index($0,"summary=\"$(jq -r \x27"){f=1; next}
  f && index($0,"\x27 \"${SCORE_PATH}\" 2>/dev/null || true)\""){exit}
  f{print}
' "$REFERENCE_BENCHMARK_SH" > "${WORK}/summary.jq"
awk '
  index($0,"context=\"$(jq -r \x27"){f=1; next}
  f && index($0,"\x27 \"${baseline_path}\" 2>/dev/null || true)\""){exit}
  f{print}
' "$REFERENCE_BENCHMARK_SH" > "${WORK}/baseline_context.jq"
awk '
  index($0,"compare=\"$(jq -r -n"){f=1; next}
  f && index($0,"2>/dev/null || true)\""){exit}
  f{print}
' "$REFERENCE_BENCHMARK_SH" > "${WORK}/compare.jq"

# 2b. END-TO-END (no baseline): facade STDERR must equal the reference summary jq's
# output on the canned score, framed as report_local_score_summary frames it, ending
# with the no-baseline note (no sibling baseline file present).
if [ ! -s "${WORK}/summary.jq" ]; then
  bad "summary byte-parity (no baseline) — could not extract reference summary jq program"
else
  sum_body="$(jq -r -f "${WORK}/summary.jq" "$FIXTURE")"
  expected="$(printf 'benchmark.sh: local-iterate summary\n%s\nbenchmark.sh: no local baseline at score.local-iterate.baseline.json; run '\''cp score.local-iterate.json score.local-iterate.baseline.json'\'' to compare future runs\n' "$sum_body")"
  run_facade "facade-nobase" "--local-iterate"
  if diff -u <(printf '%s\n' "$expected") "${WORK}/facade-nobase.err" >"${WORK}/nobase.diff" 2>&1; then
    ok "summary byte-parity (no baseline) — facade stderr == reference jq output"
  else
    bad "summary byte-parity (no baseline) — facade stderr DIVERGED:"; sed 's/^/        /' "${WORK}/nobase.diff"
  fi
fi

# 2c. END-TO-END (baseline present): seed a sibling score.local-iterate.baseline.json
# so BOTH report_local_baseline_context (the `local baseline to beat` line, printed
# BEFORE the run) AND the vs-baseline compare block (printed AFTER, in place of the
# no-baseline note) execute. Facade STDERR must equal the three reference jq programs'
# output framed in the reference's order.
if [ ! -s "${WORK}/summary.jq" ] || [ ! -s "${WORK}/baseline_context.jq" ] || [ ! -s "${WORK}/compare.jq" ]; then
  bad "summary byte-parity (baseline) — could not extract all three reference jq programs"
else
  ctx_body="$(jq -r -f "${WORK}/baseline_context.jq" "$BASELINE_FIXTURE")"
  sum_body="$(jq -r -f "${WORK}/summary.jq" "$FIXTURE")"
  cmp_body="$(jq -r -n --slurpfile cur "$FIXTURE" --slurpfile base "$BASELINE_FIXTURE" -f "${WORK}/compare.jq")"
  expected_bp="$(printf 'benchmark.sh: local baseline to beat (score.local-iterate.baseline.json): %s\nbenchmark.sh: local-iterate summary\n%s\nbenchmark.sh: vs score.local-iterate.baseline.json (negative s/token deltas = faster)\n%s\n' "$ctx_body" "$sum_body" "$cmp_body")"
  run_facade "facade-base" "--local-iterate" "$BASELINE_FIXTURE"
  if diff -u <(printf '%s\n' "$expected_bp") "${WORK}/facade-base.err" >"${WORK}/base.diff" 2>&1; then
    ok "summary byte-parity (baseline present) — facade stderr == reference jq output (context + compare)"
  else
    bad "summary byte-parity (baseline present) — facade stderr DIVERGED:"; sed 's/^/        /' "${WORK}/base.diff"
  fi
fi
echo ""

# ---- Part 3: EXIT-CODE MAPPING --------------------------------------------------
echo "Part 3: exit-code mapping (benchctl rc -> facade exit)"

STUB="${WORK}/benchctl-stub.sh"  # reuse the stub from Part 2 if present
if [ ! -x "$STUB" ]; then
  cat > "$STUB" <<'STUBEOF'
#!/usr/bin/env bash
sp=""
while [ $# -gt 0 ]; do case "$1" in --score-path) sp="$2"; shift 2;; *) shift;; esac; done
if [ -n "${STUB_SCORE_SRC:-}" ]; then
  [ -n "$sp" ] && cp "$STUB_SCORE_SRC" "$sp"
  cat "$STUB_SCORE_SRC"
fi
exit "${STUB_EXIT:-0}"
STUBEOF
  chmod +x "$STUB"
fi
GOLDEN="${WORK}/golden.json"; [ -f "$GOLDEN" ] || printf '{}' > "$GOLDEN"

run_exit_case() {  # $1=label $2=stub-exit $3=expected-facade-exit
  local label="$1" stub_exit="$2" want="$3"
  local runf="${WORK}/exit.$stub_exit"; rm -rf "$runf"; mkdir -p "$runf"
  ( cd "$runf" && \
    env BENCHCTL="$STUB" \
        STUB_SCORE_SRC="$FIXTURE" \
        STUB_EXIT="$stub_exit" \
        MLXFAST_ENGINE_BIN="/usr/bin/true" \
        MLXFAST_CORRECTNESS_GOLDEN_PATH="$GOLDEN" \
        MLXFAST_WEIGHTS_PATH="${runf}/weights" \
        "$FACADE" --local-iterate ) >/dev/null 2>/dev/null
  local got=$?
  if [ "$got" = "$want" ]; then
    ok "$label (benchctl rc=$stub_exit -> facade exit $got)"
  else
    bad "$label (benchctl rc=$stub_exit -> facade exit $got, expected $want)"
  fi
}

run_exit_case "pass passthrough"            0 0
run_exit_case "fail passthrough"            1 1
run_exit_case "usage 2 mapped to 1"         2 1
echo ""

# ---- Summary --------------------------------------------------------------------
echo "compat-matrix: PASS=${PASS} FAIL=${FAIL}"
if [ "$FAIL" -eq 0 ]; then
  echo "compat-matrix: RESULT PASS — facade is byte-green against the Swift reference"
  exit 0
else
  echo "compat-matrix: RESULT FAIL — ${FAIL} mismatch(es)"
  exit 1
fi
