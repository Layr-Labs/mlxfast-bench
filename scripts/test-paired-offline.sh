#!/bin/bash
# scripts/test-paired-offline.sh — OFFLINE self-test for the PAIRED THREE-SEAM window (NO GPU).
#
# Proves the NON-GPU control flow of the rewritten official-paired.sh (PROOF A, three seams) fails
# LOUD correctly WITHOUT a GPU and WITHOUT faking a GPU run. Seam 1 runs the REAL official_swift_run
# seal path driven by a MOCK $SWIFT binary (prints a canned gates-only payload to STDOUT → sealed to
# $PA/gates/score.json); seam 2 (measure-job) is STUBBED; seam 3 ALWAYS runs the REAL `benchctl
# overlay-timing` on the mock inputs (REAL_BENCHCTL, default target/release/benchctl — BUILT on
# demand if absent, #110; the script aborts rather than substituting a stub). What it verifies:
#   0. `bash -n` every rewritten script.
#   1. HAPPY three-seam chain: gates (partial_result=true) → identity results (raw≈1.0, valid
#      superset) → overlay merged score (partial_result=false, scoring_mode set, score in band) →
#      official-paired RESULT PASS, one row per assertion, all PASS, incl. the floor-fail neg-control.
#   2. NEGATIVE (floor-fail results): identity results with raw ratios 0.5 (<0.90) → overlay nulls
#      the score + exits nonzero → official-paired RESULT FAIL (cannot fabricate a green).
#   3. ANTI-FABRICATION, seam by seam: a stub that writes NO gates-score / NO results / NO score at a
#      chosen seam → TOOL-ERR rows → RESULT FAIL (a missing artifact is never a silent pass).
#
# The REAL GPU steps (the live gate producer; the live measure-job workspace→worker spawn) are NOT
# run here — those are the window's job. Exit 0 = all green.
set -uo pipefail
HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/paired-offline.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  PASS  %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; }
# jq is REQUIRED, not optional (benchctl-fork precedent: facade-leg.sh, official-parity.sh and
# official-paired.sh all abort on a missing jq). It used to be a soft flag whose else-branches
# printed "SKIP (jq absent)" — so on a box without jq this suite printed SKIP twice and exited
# 0 having asserted NOTHING. A self-test that goes green with zero rows is worse than one that
# fails: it reports the absence of evidence as evidence.
command -v jq >/dev/null 2>&1 || { echo "test-paired-offline: FATAL jq required (this suite asserts nothing without it)" >&2; exit 2; }

# A paired-branch benchctl (measure-job + overlay-timing). Seam 3 runs the REAL merge — this test
# exists to exercise the real overlay's fences, and a STUBBED seam 3 exercises this file's own stub
# instead. So a missing binary is BUILT, not tolerated (#110).
#
# The stub-vs-real split used to change the assertion COUNT (66/1 with no binary, 67/0 with one),
# which made every reported figure ambiguous: a 66/1 could be a real regression or just an unbuilt
# target dir, and the two are indistinguishable in the output. Building removes the fork — the script
# either runs with the real overlay and reports a stable count, or ABORTS loudly with a build error.
# It never silently reports a different, quieter number.
REAL_BENCHCTL="${REAL_BENCHCTL:-$HERE/../target/release/benchctl}"
usable_benchctl() { [ -x "$REAL_BENCHCTL" ] && "$REAL_BENCHCTL" overlay-timing --help >/dev/null 2>&1; }
if ! usable_benchctl; then
  echo "note: no usable benchctl at $REAL_BENCHCTL — building it (cargo build --release -p benchctl)" >&2
  if ! (cd "$HERE/.." && cargo build --release -p benchctl) >&2; then
    echo "FATAL: cargo build --release -p benchctl failed; seam 3 needs the REAL overlay." >&2
    echo "       (set REAL_BENCHCTL=<path> to point at a prebuilt paired-branch binary instead)" >&2
    exit 2
  fi
fi
if ! usable_benchctl; then
  echo "FATAL: $REAL_BENCHCTL still does not support 'overlay-timing' after a release build." >&2
  echo "       This is a paired-branch binary requirement, not an optional enhancement." >&2
  exit 2
fi

echo "== 0. syntax checks =="
for s in official-paired.sh run-paired-window.sh test-paired-offline.sh; do
  if bash -n "$HERE/$s" 2>"$WORK/synerr"; then ok "bash -n $s"; else bad "bash -n $s"; sed 's/^/        /' "$WORK/synerr"; fi
done
echo "  note: seam-3 overlay = REAL benchctl ($REAL_BENCHCTL)"
echo ""

# ---- mock SWIFT: the PROVEN direct-binary seam-1 producer (official_swift_run drives it) ---------
# Seam 1 is NO LONGER `<gate> --official` (benchmark.sh). official-paired.sh now runs the trusted
# `mlxfast-swift benchmark` binary via official_swift_run, which SEALS the binary's STDOUT payload
# → $PA/gates/score.json (partial_result=true). We provide a MOCK $SWIFT that prints that canned
# gates-only payload to STDOUT (ignoring the sandbox-profile/metallib env, which is a no-op offline —
# the real sandbox is applied INSIDE the swift binary, not by official_swift_run). The payload
# carries the weights_hash/file_count/byte_count metrics official_write_sidecars requires so the
# seal + sidecars complete cleanly. Modes (env-driven):
#   default            → passed=true, partial_result=true, passed_correctness=true, error=""
#   MOCK_SWIFT_NONE=1  → print NOTHING + exit 1 (anti-fabrication probe: seal fails → no score.json)
MOCK_SWIFT="$WORK/mlxfast-swift"; cat > "$MOCK_SWIFT" <<'EOF'
#!/bin/bash
# invoked as: mlxfast-swift benchmark --weights <W> --golden <G> --score-path <U>
# The trusted seal is the STDOUT bytes (official_seal_stdout), so we ignore --score-path entirely.
[ "${MOCK_SWIFT_NONE:-}" = 1 ] && exit 1
cat <<JSON
{ "score": null, "passed": true,
  "metrics": { "partial_result": true, "passed_correctness": true, "error": "",
    "decode_speedup_floor": 0.95, "passed_decode_speedup_floor": false,
    "prefill_speedup_floor": 0.95, "passed_prefill_speedup_floor": false,
    "case_count": 12, "checked_steps": 34, "benchmark_wall_seconds": 1.5, "peak_ram_gb": 20.25,
    "weights_hash": "stubw", "weights_file_count": 1, "weights_byte_count": 1,
    "harness_hash": "${MOCK_HARNESS_HASH:?mock mlxfast-swift needs MOCK_HARNESS_HASH}",
    "commit": "deadbeefcafe" } }
JSON
exit 0
EOF
chmod +x "$MOCK_SWIFT"

# ---- mock benchmark.sh: the DEFAULT GATES_PRODUCER=benchmark-sh seam-1 producer -------------------
# The reference benchmark.sh (the trusted workflow) SEALS its own gates-score to MLXFAST_SCORE_PATH
# (it does NOT rely on official_swift_run's STDOUT seal). This mock honors --official and writes the
# canned partial_result=true gates payload to $MLXFAST_SCORE_PATH. Modes (env-driven):
#   default              → write the gates-score to MLXFAST_SCORE_PATH, exit 0
#   MOCK_BENCH_NONE=1     → write NOTHING + exit 1 (anti-fabrication: no gates-score → seam-1 TOOL-ERR)
MOCK_BENCH="$WORK/benchmark.sh"; cat > "$MOCK_BENCH" <<'EOF'
#!/bin/bash
# invoked as: benchmark.sh --official (env: MLXFAST_BENCHMARK_CHECK_GATES/SKIP_TIMED/SCORE_PATH/…)
[ "${MOCK_BENCH_NONE:-}" = 1 ] && exit 1
: "${MLXFAST_SCORE_PATH:?mock benchmark.sh needs MLXFAST_SCORE_PATH}"
mkdir -p "$(dirname "$MLXFAST_SCORE_PATH")"
# Record an injected allowlist var so the R10 "applied via env" assertion is observable.
printf '%s' "${FOO:-<unset>}" > "$MLXFAST_SCORE_PATH.foo"
cat > "$MLXFAST_SCORE_PATH" <<JSON
{ "score": null, "passed": true,
  "metrics": { "partial_result": true, "passed_correctness": true, "error": "",
    "decode_speedup_floor": 0.95, "passed_decode_speedup_floor": false,
    "case_count": 12, "checked_steps": 34, "benchmark_wall_seconds": 1.5, "peak_ram_gb": 20.25,
    "weights_hash": "stubw", "weights_file_count": 1, "weights_byte_count": 1,
    "harness_hash": "${MOCK_HARNESS_HASH:?mock benchmark.sh needs MOCK_HARNESS_HASH}",
    "commit": "deadbeefcafe" } }
JSON
exit 0
EOF
chmod +x "$MOCK_BENCH"
MOCK_BENCH_SHA="$(shasum -a 256 "$MOCK_BENCH" | awk '{print $1}')"

# ---- mock HARNESS WORKSPACE: the tree BOTH harness-identity legs resolve --------------------------
# David ruling 2026-08-26 — seam 3 now RE-RESOLVES the 9-root harness identity at the seal and
# refuses a merge whose gates leg disagrees. Offline, that gate must be exercised for real, not
# bypassed: so $WORK is made a genuine 9-root harness workspace (the mock benchmark.sh written above
# IS root index 4, exactly as the reference's own benchmark.sh is), the identity is computed by the
# REAL benchctl over that tree — no shell reimplementation of the algorithm — and the mocks stamp
# THAT digest as their gates leg via $MOCK_HARNESS_HASH.
#
# The driver then cds to this same root before seam 3 (official-paired.sh's GATES_WS), so the two
# legs agree and the happy chains below pass THROUGH the gate rather than around it. The tamper twin
# in section 3d overrides MOCK_HARNESS_HASH to a different well-formed digest and asserts the
# refusal, so a deleted or defanged gate turns this suite red.
mkdir -p "$WORK/Sources" "$WORK/Tests" "$WORK/tools"
printf '// mock package\n'      > "$WORK/Package.swift"
printf '// mock source\n'       > "$WORK/Sources/Mock.swift"
printf '// mock test\n'         > "$WORK/Tests/MockTests.swift"
printf '{"mock":true}\n'        > "$WORK/benchmark.json"
printf '#!/bin/sh\nexit 0\n'    > "$WORK/setup.sh"
printf '#!/bin/sh\nexit 0\n'    > "$WORK/tools/mock.sh"
printf '# mock README\n'        > "$WORK/README.md"
printf '# mock TASK\n'          > "$WORK/TASK.md"
HARNESS_HASH="$( (cd "$WORK" && "$REAL_BENCHCTL" harness-hash) )" || {
  echo "FATAL: benchctl harness-hash failed over the mock workspace $WORK; the cross-leg seam-3 gate cannot be exercised." >&2
  exit 2
}
# A DIFFERENT well-formed digest for the tamper twin: same shape, so it clears the single-leg
# well-formedness gate and the refusal under test is unambiguously the CROSS-LEG one.
HARNESS_HASH_TAMPERED="$(printf '%064d' 0 | tr '0' 'b')"
# Carried by every three-seam case below. MOCK_HARNESS_HASH is the gates leg the mock producers
# stamp; SWIFT_REPO_ROOT is what official-paired.sh derives GATES_WS from on the direct-swift
# producer (and what official_swift_run itself cds into), so both legs land on $WORK. On the
# benchmark-sh producer GATES_WS is derived from GATE_CMD's directory, which is $WORK already.
HARNESS_ENV="MOCK_HARNESS_HASH=$HARNESS_HASH SWIFT_REPO_ROOT=$WORK"

# ---- stub benchctl: `measure-job` writes results.json (+ integrity + sidecar); `overlay-timing`
#      EXECs the REAL benchctl (#110 — no canned merge fallback; seam 3 is never stubbed). ---------
# measure-job modes (env-driven):
#   default          → identity results: raw ratios ≈ 1.0, valid superset, ACCEPTed (exit 0)
#   STUB_FLOOR_FAIL=1 → floor-fail results: raw ratios 0.5 (< 0.90 floor), still a valid superset
#   STUB_RESULTS_NONE=1 → write NOTHING, exit 5 (anti-fabrication probe for seam 2 / die-5)
STUB_BC="$WORK/bc.sh"; cat > "$STUB_BC" <<EOF
#!/bin/bash
REAL_BENCHCTL="$REAL_BENCHCTL"
EOF
cat >> "$STUB_BC" <<'EOF'
sub="$1"; shift
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
case "$sub" in
  measure-job)
    # Ruling Q1a: capture --gates-producer and SEAL it, exactly as the real measure-job does.
    # Absent → the same "undeclared" sentinel benchctl uses, so the stub cannot be greener than real.
    out=""; gp="undeclared"; contract=""; cg=""
    while [ $# -gt 0 ]; do case "$1" in
      --out) out="$2"; shift 2;;
      --gates-producer) gp="$2"; shift 2;;
      # LANE 2a (#157) — capture the track --contract (the fixture that PINS the hidden correctness
      # golden) and the run's --correctness-golden ATTESTATION, so this stub can model benchd's
      # fail-closed die-8 gate and CATCH a driver that stops passing the flag.
      --contract) contract="$2"; shift 2;;
      --correctness-golden) cg="$2"; shift 2;;
      # STUB_FORCE_GATES_PRODUCER makes the stub seal a producer that DISAGREES with the one the
      # driver resolved — the negative fixture for the Q1a conformance floor.
      *) shift;;
    esac; done
    gp="${STUB_FORCE_GATES_PRODUCER:-$gp}"
    [ -n "$out" ] || { echo "stub-bc measure-job: no --out" >&2; exit 2; }
    mkdir -p "$out"
    # LANE 2a (#157) — model benchd's correctness-golden ATTESTATION gate (die-8, pre-GPU,
    # fail-closed BOTH directions). The fixture's `hidden_correctness_golden.sha256` is the authority;
    # the run's --correctness-golden is HASHED and must CITE it. This stub is the OFF-BOX seam that
    # binds official-paired.sh's passing of the flag: revert the driver's `--correctness-golden` line
    # and a fixture that pins the golden dies-8 here, exactly as the real benchd would once #41 lands.
    fixture_hcg="$(jq -r '.hidden_correctness_golden.sha256 // empty' "$contract" 2>/dev/null)"
    attested_hcg=""; [ -n "$cg" ] && attested_hcg="$(sha "$cg")"
    if [ -n "$fixture_hcg" ] && [ -z "$attested_hcg" ]; then
      echo "stub-bc measure-job: --contract pins a hidden_correctness_golden but the run carries no --correctness-golden attestation (fail-closed)" >&2; exit 8
    fi
    if [ -z "$fixture_hcg" ] && [ -n "$attested_hcg" ]; then
      echo "stub-bc measure-job: --correctness-golden attested but the fixture pins no hidden_correctness_golden to authorize it (fail-closed)" >&2; exit 8
    fi
    if [ -n "$fixture_hcg" ] && [ "$fixture_hcg" != "$attested_hcg" ]; then
      echo "stub-bc measure-job: the correctness-golden attestation does not cite the fixture pin (wrong-digest)" >&2; exit 8
    fi
    [ "${STUB_RESULTS_NONE:-}" = 1 ] && exit 5      # write nothing → seam-2 anti-fabrication / die 5
    rp="$out/results.json"
    # A VALID multi-prompt POOL (R16/R17/R18): POOL_SIZE distinct prompts, one accepted pair each, so
    # the REAL overlay's full pool-shape predicate set holds. per-prompt means are chosen so each raw
    # ratio (serial/mtp) is EXACTLY 1.0 (identity) or 0.5 (STUB_FLOOR_FAIL), and the sealed
    # `aggregate.raw_decode_speedup_median` is the even-n median of the per-prompt RECOMPUTED ratios —
    # so the R18 sealed-median agreement holds BY CONSTRUCTION (the generator computes the median). The
    # pool_size + track_id are pinned to what the test seals in the seam-3 env (POOL_SIZE=3 / TRACK_ID).
    STUB_FLOOR_FAIL="${STUB_FLOOR_FAIL:-}" python3 - "$rp" <<'PY'
import json, sys, hashlib, os
rp = sys.argv[1]
floor_fail = os.environ.get("STUB_FLOOR_FAIL", "") == "1"
pool = 3
serial = 0.036
# ratio = serial / mtp. mtp = serial (→ 1.0, x/x is EXACT in IEEE754) or 2*serial (→ exactly 0.5).
mtp = 0.072 if floor_fail else 0.036
ratio = serial / mtp
per_prompt, pairs = [], []
for i in range(pool):
    # A distinct, valid 64-lowercase-hex prompt sha (the pool binds by bytes; the overlay requires
    # every per-prompt sha to match ^[0-9a-f]{64}$ AND be distinct across the pool).
    sha = hashlib.sha256(("qwen-mtp-offline-pool-prompt-%d" % i).encode()).hexdigest()
    per_prompt.append({
        "prompt_sha256": sha, "parity_ok": True, "accepted_pair_count": 1,
        "serial_seconds_per_token_mean": serial, "mtp_seconds_per_token_mean": mtp,
        "raw_ratio_of_means": ratio, "noop_reference_decode_speedup": 1.0,
    })
    pairs.append({
        "parity_ok": True, "serial_seconds_per_token": serial, "mtp_seconds_per_token": mtp,
        "order": "mtp-first" if i % 2 == 0 else "serial-first", "raw_ratio": ratio,
        "serial_gate_state": "fired", "candidate_gate_state": "fired",
        # W3 — the per-leg SERIES TAGS the overlay's §5 fence cross-checks against the sealed
        # descriptor. This stub models an ALL-TEACHER-FORCED run (the Model-2 identity shape).
        "serial_timed_mode": "teacher_forced_v1", "candidate_timed_mode": "teacher_forced_v1",
    })
# Even-n median of the per-prompt RECOMPUTED ratios (mirrors bench_core paired_decode_only_median):
# the sealed raw_decode_speedup_median MUST equal this (R18, within 1e-7) — here it is EXACT.
ratios = sorted(p["serial_seconds_per_token_mean"] / p["mtp_seconds_per_token_mean"] for p in per_prompt)
n = len(ratios)
median = ratios[n // 2] if n % 2 else (ratios[n // 2 - 1] + ratios[n // 2]) / 2.0
obj = {
    "track_id": "qwen-mtp-paired-identity",
    # W3 — the SERIES DESCRIPTOR (required by the overlay's §5 fence). Homogeneous teacher-forced:
    # both legs the same series, so the legs ARE comparable and the top-level tag is that series.
    "timed_mode": "teacher_forced_v1",
    "timed_series": {
        "serial_leg_timed_mode": "teacher_forced_v1",
        "candidate_leg_timed_mode": "teacher_forced_v1",
        # The SEALED key is `*_leg_timed_regime` (the value is a REGIME label, not an invocation
        # verb — benchd renamed the field with the value's meaning). The stub must mirror the shape
        # measure-job actually writes, or the fixture stops standing in for a real results.json.
        "serial_leg_timed_regime": "tf-serial-timed",
        "candidate_leg_timed_regime": "tf-serial-timed",
        "homogeneous": True,
        "legs_comparable": True,
    },
    "parity_all_ok": True,
    "accepted_pair_count": pool,     # one accepted pair per prompt → pairs.len == accepted == pool
    "candidate_accepted": True,
    "min_pairs": pool,               # >= this AND >= pool_size*min_per_prompt(=pool) both hold
    "prompt_count": pool,            # == pool_size == per_prompt.len (R17)
    "serial_depth": 0, "candidate_depth": 2,
    "pairs": pairs,
    "aggregate": {
        "baseline_serial_seconds_per_token_mean": serial,
        "candidate_mtp_seconds_per_token_mean": mtp,
        "mtp_decode_speedup_median": median,
        "mtp_decode_speedup_min": min(ratios),
        "raw_decode_speedup_median": median,   # R18: == even-n median of per-prompt recomputed ratios
    },
    "per_prompt": per_prompt,
    "provenance": {"candidate_executable": "ws", "baseline_executable": "ws",
        "thermal": {"cool_gate_c": 40.0, "clock_floor_mhz": 1000.0, "loaded_util": 0.85,
            "cool_gate_c_source": "box3_qmtp_defaults", "clock_floor_mhz_source": "box3_qmtp_defaults",
            "loaded_util_source": "box3_qmtp_defaults"}},
    "rejected_pairs": [],
    "commit": "deadbeefcafe",
    "weights_hash": "stubw",
}
json.dump(obj, open(rp, "w"), indent=2)
PY
    printf '%s  results.json\n' "$(sha "$rp")" > "$rp.sha256"
    printf '{\n  "results_path": "%s",\n  "results_sha256": "%s",\n  "candidate_workspace": "ws",\n  "baseline_workspace": "ws",\n  "candidate_executable": "ws",\n  "baseline_executable": "ws",\n  "weights_sha256": "stubw",\n  "weights_file_count": 1,\n  "weights_byte_count": 1,\n  "gates_producer": "%s"\n}\n' "$rp" "$(sha "$rp")" "$gp" > "$out/benchmark-integrity.results.json"
    exit 0     # measure-job ACCEPTs (candidate accepted); a FLOOR breach surfaces at seam 3, not here
    ;;
  overlay-timing)
    # #110 — seam 3 is ALWAYS the REAL overlay. The stub used to carry a canned merge for the
    # no-binary case; that made the pass/fail count depend on whether target/release was populated,
    # and it meant the assertions below could pass against this file's own arithmetic rather than
    # against the overlay's fences. The driver guarantees the binary exists (building it if needed),
    # so there is nothing left to fall back to.
    exec "$REAL_BENCHCTL" overlay-timing "$@"
    ;;
  *) echo "stub-bc: unsupported subcommand $sub" >&2; exit 2;;
esac
EOF
chmod +x "$STUB_BC"

# canned inputs. The mock WORKSPACE carries a `.build/release/<bin>` executable (the layout the real
# measure-job resolves + spawns from) and WEIGHTS is a SEPARATE dir (the transformed weights both
# legs load) — WS and WEIGHTS are DIFFERENT paths. The stub benchctl ignores both, but the fixture
# mirrors the real box layout so official-paired.sh's --weights wiring is exercised end-to-end.
mkdir -p "$WORK/ws/.build/release"; printf '#!/bin/sh\nexit 0\n' > "$WORK/ws/.build/release/mlxfast-engine"; chmod +x "$WORK/ws/.build/release/mlxfast-engine"
mkdir -p "$WORK/weights"; printf '{}' > "$WORK/weights/config.json"
printf '{"version":1}' > "$WORK/golden.json"; printf '{"timed_prompt_pool":[]}' > "$WORK/contract.json"
# SWIFT drives the new seam-1 direct-binary path (official_swift_run). GATE_CMD is still a REQUIRED
# env of official-paired.sh (the `:?` guard) but is NO LONGER invoked in seam 1 (retained only as the
# alternate benchmark.sh producer for a cached-weights box) — point it at the mock so the guard is
# satisfied; it is never executed here.
# COMMON drives the existing rich seam-1 assertions through the direct-swift FALLBACK producer (the
# mock swift + official_swift_run STDOUT seal); the benchmark-sh DEFAULT producer is covered by its
# own section (1d) with the mock benchmark.sh. GATE_CMD is set to the mock benchmark.sh so the guard
# is satisfied for both producers.
# R17/R12 seam-3 pool + track env: the REAL `benchctl overlay-timing` resolves the expected pool
# SHAPE fail-closed from MLXFAST_QWEN_MTP_POOL_SIZE (the stub seals a POOL_SIZE-prompt pool) and the
# expected track from MLXFAST_QWEN_MTP_TRACK_ID (the stub seals the SAME track_id). All are inherited
# by official-paired.sh → the stub bc.sh → the real benchctl. Kept identical to the stub's sealed pool.
#
# W3 §5 — MLXFAST_QWEN_MTP_TIMED_SERIES pins the expected TIMED SERIES: a results.json sealed for a
# different series is refused (overlay `validate_series` check 6), because §5 makes baselines, floors
# and bands per-series. Unset, that check is skipped and only the file's internal series coherence is
# enforced. The stub seals an all-teacher-forced run, so the driver states that series — which gives
# check 6 a PRODUCTION caller offline instead of unit-test coverage only.
QMTP_POOL="MLXFAST_QWEN_MTP_POOL_SIZE=3 MLXFAST_QWEN_MTP_TRACK_ID=qwen-mtp-paired-identity MLXFAST_QWEN_MTP_TIMED_SERIES=teacher_forced_v1"
COMMON="$QMTP_POOL $HARNESS_ENV GATES_PRODUCER=direct-swift BENCHCTL=$STUB_BC SWIFT=$MOCK_SWIFT GATE_CMD=$MOCK_BENCH OFFICIAL_GOLDEN=$WORK/golden.json CONTRACT=$WORK/contract.json PAIRED_WS=$WORK/ws WEIGHTS=$WORK/weights OFFICIAL_COMMIT=deadbeefcafe"
# benchmark-sh DEFAULT producer wiring (section 1d + R10 GATE_CMD sha-pin tests).
COMMON_BENCHSH="$QMTP_POOL $HARNESS_ENV GATES_PRODUCER=benchmark-sh BENCHCTL=$STUB_BC SWIFT=$MOCK_SWIFT GATE_CMD=$MOCK_BENCH OFFICIAL_GOLDEN=$WORK/golden.json CONTRACT=$WORK/contract.json PAIRED_WS=$WORK/ws WEIGHTS=$WORK/weights OFFICIAL_COMMIT=deadbeefcafe"
# R22 facade OPT-IN producer wiring (section 1h): benchd's own benchmark.sh --official as the
# seam-1 producer, selected explicitly per ruling Q1a. The mock benchmark.sh honors --official and writes the gates-score to
# MLXFAST_SCORE_PATH exactly as the real facade+benchctl backend do, so it stands in for FACADE_CMD;
# ENGINE is a stub (the mock ignores it). GATE_CMD is still set (its `:?` guard) but unused here.
COMMON_FACADE="$QMTP_POOL $HARNESS_ENV GATES_PRODUCER=facade BENCHCTL=$STUB_BC SWIFT=$MOCK_SWIFT GATE_CMD=$MOCK_BENCH FACADE_CMD=$MOCK_BENCH ENGINE=/usr/bin/true OFFICIAL_GOLDEN=$WORK/golden.json CONTRACT=$WORK/contract.json PAIRED_WS=$WORK/ws WEIGHTS=$WORK/weights OFFICIAL_COMMIT=deadbeefcafe"

  echo "== 1. HAPPY three-seam chain (gates→identity results→overlay merged; +neg-control) =="
  if env $COMMON OUT="$WORK/ph" bash "$HERE/official-paired.sh" > "$WORK/ph.out" 2> "$WORK/ph.err"; then
    grep -q 'RESULT PASS' "$WORK/ph.out" && ok "official-paired RESULT PASS on the happy three-seam chain" || bad "official-paired missing RESULT PASS"
    if awk -F' *\\| *' 'NF>=3 && $1!="check" && $1 !~ /^-+$/ {print $2}' "$WORK/ph/official-paired.table.txt" | grep -qv '^\(PASS\|SKIP\)$'; then
      bad "happy path has a non-PASS assertion row"; sed 's/^/        /' "$WORK/ph/official-paired.table.txt"
    else ok "happy path: every assertion row is PASS (or neg-control SKIP)"; fi
    # per-seam key rows present + PASS
    for k in 'seam1.partial_result' 'seam2.parity_all_ok' 'seam2.per_prompt raw~1.0' 'seam3.partial_result' 'seam3.scoring_mode' 'seam3.identity-band'; do
      awk -F' *\\| *' -v K="$k" '$1 ~ K {print $2}' "$WORK/ph/official-paired.table.txt" | grep -q '^PASS$' \
        && ok "row '$k' PASS" || bad "row '$k' not PASS"
    done
    # seam 3 read floor/ceiling FROM the artifact (finding 15): score.json carries them
    [ -f "$WORK/ph/proof-a/score.json" ] && jq -e '.metrics.decode_speedup_floor and .decode_speedup_ceiling' "$WORK/ph/proof-a/score.json" >/dev/null \
      && ok "score.json carries floor/ceiling (read from artifact, not restated)" || bad "score.json missing floor/ceiling"
    # neg-control ran and PASSED (floor-fail nulled the score)
    awk -F' *\\| *' '$1 ~ /neg.score-null/ {print $2}' "$WORK/ph/official-paired.table.txt" | grep -q '^PASS$' \
      && ok "neg-control: floor-fail results null the score → PASS" || bad "neg-control score-null row not PASS"
  else bad "official-paired did not PASS on the happy chain"; sed 's/^/        /' "$WORK/ph.out" "$WORK/ph.err"; fi
  echo ""

  echo "== 1b. R6 --weights OPTIONAL OVERRIDE wiring (measure-job seam 2) =="
  # R6: measure-job's --weights is an OPTIONAL OVERRIDE (the approved draft CLI has none; it derives
  # from QMTP_TARGET_DIR). run_measure passes `--weights` ONLY when WEIGHTS is non-empty and emits
  # no empty `--weights ''`. The rendered seam2-cmd is the driver's own record of the exact measure-job
  # invocation. WEIGHTS is required by seam 1 (the gates producer), so the happy chain always sets it ⇒
  # seam2-cmd carries the override. The empty-guard branch is covered directly below.
  grep -h '^seam2-cmd:' "$WORK/ph.out" | grep -q -- '--weights' \
    && ok "R6: WEIGHTS set ⇒ seam2-cmd passes --weights (override present)" || bad "R6: WEIGHTS set but seam2-cmd lacks --weights"
  # Empty-guard: source the run_measure array logic in isolation (WEIGHTS='') and assert NO `--weights ''`
  # token is built (the real CLI rejects an empty weights dir). Proves the conditional, without seam 1.
  wa_none="$(WEIGHTS='' bash -c 'weights_arg=(); [ -n "$WEIGHTS" ] && weights_arg=(--weights "$WEIGHTS"); echo "${#weights_arg[@]}"')"
  [ "$wa_none" = 0 ] && ok "R6: WEIGHTS empty ⇒ no --weights token emitted (derive-on-box path)" || bad "R6: WEIGHTS empty built $wa_none tokens"
  wa_set="$(WEIGHTS='/w' bash -c 'weights_arg=(); [ -n "$WEIGHTS" ] && weights_arg=(--weights "$WEIGHTS"); printf "%s|" "${weights_arg[@]}"')"
  [ "$wa_set" = "--weights|/w|" ] && ok "R6: WEIGHTS set ⇒ exactly '--weights <DIR>' token pair" || bad "R6: WEIGHTS set built '$wa_set'"
  echo ""

  echo "== 1c. R9 redirect-target survival: run the driver the way run-paired-window.sh redirects it =="
  # run-paired-window.sh redirects official-paired.sh STDOUT into $OUT/official-paired.table.txt — the
  # SAME path the driver lists in its anti-stale wipe. The R9 bug: `rm -f`-ing that path UNLINKS the
  # parent's live redirect inode, so the parent-redirected stdout (headers, the seam*-cmd lines the
  # window greps, RESULT) is written to an orphan and LOST. The fix truncates in place instead. Repro
  # the exact redirect and assert the echoed stdout (NOT just the tee'd rows) SURVIVES in the file.
  mkdir -p "$WORK/r9"
  R9_TABLE="$WORK/r9/official-paired.table.txt"
  env $COMMON NEG_CONTROL=0 OUT="$WORK/r9" bash "$HERE/official-paired.sh" > "$R9_TABLE" 2> "$WORK/r9.err"; R9_RC=$?
  [ -s "$R9_TABLE" ] && ok "R9: redirect target survives (non-empty after the driver's wipe)" || bad "R9: redirect target empty/destroyed after run (rc=$R9_RC)"
  grep -q '^seam2-cmd:' "$R9_TABLE" && ok "R9: parent-redirected seam2-cmd echo survived (not lost to an unlinked inode)" || { bad "R9: seam2-cmd echo LOST — redirect target was unlinked"; sed 's/^/        /' "$R9_TABLE"; }
  grep -q 'RESULT PASS' "$R9_TABLE" && ok "R9: RESULT line (echoed to stdout) survived in the table" || bad "R9: RESULT line lost from the redirected table"
  grep -q 'seam2.parity_all_ok' "$R9_TABLE" && ok "R9: tee'd assertion rows also present in the table" || bad "R9: assertion rows missing from the table"
  echo ""

  echo "== 1d. GATES_PRODUCER=benchmark-sh (DEFAULT reference producer; seals gates-score to MLXFAST_SCORE_PATH) =="
  # Q1a: benchmark-sh is the DEFAULT (the organizer's ranked chain), facade the explicit opt-in and
  # direct-swift the weightless fallback. The mock benchmark.sh writes the gates-score to
  # MLXFAST_SCORE_PATH (as the real trusted workflow does), which official-paired.sh trusts directly.
  if env $COMMON_BENCHSH GATE_CMD_SHA="$MOCK_BENCH_SHA" NEG_CONTROL=0 OUT="$WORK/bsh" bash "$HERE/official-paired.sh" > "$WORK/bsh.out" 2> "$WORK/bsh.err"; then
    grep -q 'RESULT PASS' "$WORK/bsh.out" && ok "benchmark-sh producer: RESULT PASS (default selector, sha-pinned GATE_CMD)" || bad "benchmark-sh producer missing RESULT PASS"
    awk -F' *\\| *' '$1 ~ /seam1.partial_result/ {print $2}' "$WORK/bsh/official-paired.table.txt" | grep -q '^PASS$' \
      && ok "benchmark-sh producer: seam1.partial_result PASS (gates-score sealed to MLXFAST_SCORE_PATH)" || bad "benchmark-sh seam1.partial_result not PASS"
    grep -q 'GATES_PRODUCER=benchmark-sh' "$WORK/bsh.out" && ok "benchmark-sh producer: seam1-cmd records benchmark-sh" || bad "benchmark-sh seam1-cmd not recorded"
  else bad "benchmark-sh producer did not PASS the happy chain"; sed 's/^/        /' "$WORK/bsh.out" "$WORK/bsh.err"; fi
  # anti-fabrication: mock benchmark.sh writes NO gates-score → seam-1 TOOL-ERR → RESULT FAIL
  if env $COMMON_BENCHSH GATE_CMD_SHA="$MOCK_BENCH_SHA" MOCK_BENCH_NONE=1 NEG_CONTROL=0 OUT="$WORK/bshn" bash "$HERE/official-paired.sh" > "$WORK/bshn.out" 2> "$WORK/bshn.err"; then
    bad "benchmark-sh producer PASSED despite writing NO gates-score"
  else
    grep -qi 'TOOL-ERR' "$WORK/bshn/official-paired.table.txt" && ok "benchmark-sh producer: no gates-score → TOOL-ERR (anti-fabrication)" || bad "benchmark-sh no-gates-score missing TOOL-ERR"
  fi
  echo ""

  echo "== 1e. R10 GATE_CMD sha-pin (benchmark-sh): a mismatched GATE_CMD_SHA ABORTS before running the producer =="
  if env $COMMON_BENCHSH GATE_CMD_SHA="deadbeefdeadbeef" NEG_CONTROL=0 OUT="$WORK/shamis" bash "$HERE/official-paired.sh" > "$WORK/shamis.out" 2> "$WORK/shamis.err"; then
    bad "GATE_CMD sha mismatch did NOT abort (producer ran with an unpinned command)"
  else
    grep -q 'GATE_CMD sha mismatch' "$WORK/shamis.err" && ok "R10: GATE_CMD sha mismatch is FATAL (abort, producer never runs)" || { bad "R10: sha mismatch did not emit the FATAL line"; sed 's/^/        /' "$WORK/shamis.err"; }
  fi
  echo ""

  echo "== 1f. R10 KEY=VAL allowlist: producer-hijack token REJECTED, valid token APPLIED via env =="
  # The reproduced hijack: a value that word-splits into the command position. Assert official-paired
  # ABORTS (nonzero) with the FATAL reject AND that /bin/echo never ran (no stray 'HIJACKED' on stdout).
  if env $COMMON_BENCHSH GATE_CMD_SHA="$MOCK_BENCH_SHA" GATE_EXTRA_ENV='FOO=1 /bin/echo HIJACKED' NEG_CONTROL=0 OUT="$WORK/hj" bash "$HERE/official-paired.sh" > "$WORK/hj.out" 2> "$WORK/hj.err"; then
    bad "R10: GATE_EXTRA_ENV word-split hijack did NOT abort"
  else
    grep -q 'is not KEY=VAL' "$WORK/hj.err" && ok "R10: GATE_EXTRA_ENV hijack token REJECTED (FATAL, not word-split into the command)" || { bad "R10: hijack not rejected with the FATAL line"; sed 's/^/        /' "$WORK/hj.err"; }
    grep -q 'HIJACKED' "$WORK/hj.out" "$WORK/hj.err" && bad "R10: /bin/echo executed — hijack succeeded" || ok "R10: hijacked command never executed (no 'HIJACKED' output)"
  fi
  # MEASURE_EXTRA_ENV takes the SAME validator.
  if env $COMMON_BENCHSH GATE_CMD_SHA="$MOCK_BENCH_SHA" MEASURE_EXTRA_ENV='/usr/bin/touch pwned' NEG_CONTROL=0 OUT="$WORK/hj2" bash "$HERE/official-paired.sh" > "$WORK/hj2.out" 2> "$WORK/hj2.err"; then
    bad "R10: MEASURE_EXTRA_ENV word-split hijack did NOT abort"
  else
    grep -q 'MEASURE_EXTRA_ENV token' "$WORK/hj2.err" && ok "R10: MEASURE_EXTRA_ENV hijack token REJECTED (same validator)" || { bad "R10: MEASURE_EXTRA_ENV not rejected"; sed 's/^/        /' "$WORK/hj2.err"; }
  fi
  # A VALID KEY=VAL allowlist is ACCEPTED and APPLIED via `env` (mock benchmark.sh records FOO).
  if env $COMMON_BENCHSH GATE_CMD_SHA="$MOCK_BENCH_SHA" GATE_EXTRA_ENV='FOO=1 BAR=2' NEG_CONTROL=0 OUT="$WORK/kv" bash "$HERE/official-paired.sh" > "$WORK/kv.out" 2> "$WORK/kv.err"; then
    grep -q 'RESULT PASS' "$WORK/kv.out" && ok "R10: valid KEY=VAL allowlist ACCEPTED (happy chain still PASS)" || bad "R10: valid allowlist did not PASS"
    [ "$(cat "$WORK/kv/proof-a/gates/score.json.foo" 2>/dev/null)" = 1 ] && ok "R10: valid token APPLIED via env (producer saw FOO=1)" || bad "R10: valid token not applied (FOO not seen by producer)"
  else bad "R10: valid KEY=VAL allowlist did not PASS the chain"; sed 's/^/        /' "$WORK/kv.out" "$WORK/kv.err"; fi
  echo ""

  echo "== 1g. new-CLI seam-2: repeatable --golden pool (GOLDENS) + QMTP_* passthrough (rendered seam2-cmd) =="
  # R7: GOLDENS is a whitespace pool → one repeatable --golden per entry. R14: QMTP_* dirs forwarded.
  # The stub measure-job ignores them, but the driver's OWN record (seam2-cmd) must reflect the exact
  # invocation, so assert the rendered command carries every pool golden + each QMTP_* var.
  printf '{"version":1}' > "$WORK/g1.json"; printf '{"version":2}' > "$WORK/g2.json"; printf '{"version":3}' > "$WORK/g3.json"
  if env $COMMON GOLDENS="$WORK/g1.json $WORK/g2.json $WORK/g3.json" \
       QMTP_TARGET_DIR="$WORK/tgt" QMTP_HEAD_DIR="$WORK/head" QMTP_CANDIDATE_HEAD_DIR="$WORK/chead" \
       NEG_CONTROL=0 OUT="$WORK/pool" bash "$HERE/official-paired.sh" > "$WORK/pool.out" 2> "$WORK/pool.err"; then
    n="$(grep -h '^seam2-cmd:' "$WORK/pool.out" | head -1 | grep -o -- '--golden' | wc -l | tr -d ' ')"
    [ "$n" = 3 ] && ok "R7: GOLDENS pool → 3 repeatable --golden in seam2-cmd" || bad "R7: expected 3 --golden, got $n"
    grep -h '^seam2-cmd:' "$WORK/pool.out" | grep -q "QMTP_TARGET_DIR='$WORK/tgt'" && ok "R14: QMTP_TARGET_DIR forwarded in seam2-cmd" || bad "R14: QMTP_TARGET_DIR missing from seam2-cmd"
    grep -h '^seam2-cmd:' "$WORK/pool.out" | grep -q "QMTP_HEAD_DIR='$WORK/head'" && ok "R14: QMTP_HEAD_DIR forwarded" || bad "R14: QMTP_HEAD_DIR missing"
    grep -h '^seam2-cmd:' "$WORK/pool.out" | grep -q "QMTP_CANDIDATE_HEAD_DIR='$WORK/chead'" && ok "R14: QMTP_CANDIDATE_HEAD_DIR forwarded" || bad "R14: QMTP_CANDIDATE_HEAD_DIR missing"
  else bad "new-CLI pool chain did not PASS"; sed 's/^/        /' "$WORK/pool.out" "$WORK/pool.err"; fi
  # default (no GOLDENS) → single --golden (cardinality-1 identity framing)
  n1="$(grep -h '^seam2-cmd:' "$WORK/ph.out" | head -1 | grep -o -- '--golden' | wc -l | tr -d ' ')"
  [ "$n1" = 1 ] && ok "R7: no GOLDENS → single --golden (cardinality-1 identity)" || bad "R7: default golden count $n1 != 1"
  echo ""

  echo "== 1h. GATES_PRODUCER=facade (OPT-IN: benchd's own benchmark.sh --official seam-1) =="
  # Q1a: the facade is the explicit opt-in seam-1 producer. It routes --official to the benchctl official
  # backend (loads --weights directly — no reference-checkpoint regeneration) and seals its own
  # partial_result=true gates-score to MLXFAST_SCORE_PATH. Same happy three-seam chain.
  # facade derives GATES_WS from the DRIVER's CWD (benchd's facade never cd's), so this case runs from
  # the mock harness workspace — the same root the seal must resolve. See official-paired.sh gates_workspace_root().
  if (cd "$WORK" && env $COMMON_FACADE FACADE_CMD_SHA="$MOCK_BENCH_SHA" NEG_CONTROL=0 OUT="$WORK/fac" bash "$HERE/official-paired.sh" > "$WORK/fac.out" 2> "$WORK/fac.err"); then
    grep -q 'RESULT PASS' "$WORK/fac.out" && ok "facade producer: RESULT PASS (opt-in selector)" || bad "facade producer missing RESULT PASS"
    awk -F' *\\| *' '$1 ~ /seam1.partial_result/ {print $2}' "$WORK/fac/official-paired.table.txt" | grep -q '^PASS$' \
      && ok "facade producer: seam1.partial_result PASS (gates-score sealed to MLXFAST_SCORE_PATH)" || bad "facade seam1.partial_result not PASS"
    grep -q 'seam1=facade' "$WORK/fac.out" && ok "facade producer: PROOF A header records seam1=facade" || bad "facade producer header not recorded"
    grep -h '^seam1-cmd:' "$WORK/fac.out" | grep -q 'GATES_PRODUCER=facade' && ok "facade producer: seam1-cmd records facade + --mode official note" || bad "facade seam1-cmd not recorded"
  else bad "facade producer did not PASS the happy chain"; sed 's/^/        /' "$WORK/fac.out" "$WORK/fac.err"; fi
  # TRUST BOUNDARY (ruling Q1a, David 2026-08-20): the DEFAULT must be the sha-pinned REFERENCE
  # producer, and the facade must be reachable ONLY by asking for it. Both directions are asserted
  # from the SAME env, differing only in whether GATES_PRODUCER=facade is set — so a regression that
  # flipped the default back could not hide behind the opt-in case still passing.
  PRODUCER_ENV="$QMTP_POOL $HARNESS_ENV BENCHCTL=$STUB_BC SWIFT=$MOCK_SWIFT GATE_CMD=$MOCK_BENCH FACADE_CMD=$MOCK_BENCH ENGINE=/usr/bin/true OFFICIAL_GOLDEN=$WORK/golden.json CONTRACT=$WORK/contract.json PAIRED_WS=$WORK/ws WEIGHTS=$WORK/weights OFFICIAL_COMMIT=deadbeefcafe"
  # (a) no opt-in → benchmark-sh (the organizer's ranked chain), NOT the facade.
  if env $PRODUCER_ENV NEG_CONTROL=0 OUT="$WORK/prodef" bash "$HERE/official-paired.sh" > "$WORK/prodef.out" 2> "$WORK/prodef.err"; then
    grep -q 'seam1=benchmark-sh' "$WORK/prodef.out" && ok "Q1a: unset GATES_PRODUCER defaults to benchmark-sh (reference producer)" || bad "Q1a: default producer is not benchmark-sh"
    grep -q 'seam1=facade' "$WORK/prodef.out" && bad "Q1a: facade was selected WITHOUT the explicit opt-in" || ok "Q1a: facade is not reachable without the opt-in"
  else bad "Q1a: default (benchmark-sh) producer did not PASS"; sed 's/^/        /' "$WORK/prodef.out" "$WORK/prodef.err"; fi
  # (b) explicit opt-in → facade, still fully supported for parity testing.
  # facade derives GATES_WS from the DRIVER's CWD (benchd's facade never cd's), so this case runs from
  # the mock harness workspace — the same root the seal must resolve. See official-paired.sh gates_workspace_root().
  if (cd "$WORK" && env $PRODUCER_ENV GATES_PRODUCER=facade NEG_CONTROL=0 OUT="$WORK/proopt" bash "$HERE/official-paired.sh" > "$WORK/proopt.out" 2> "$WORK/proopt.err"); then
    grep -q 'seam1=facade' "$WORK/proopt.out" && ok "Q1a: explicit GATES_PRODUCER=facade opts in to the parity-test producer" || bad "Q1a: explicit facade opt-in did not select the facade"
  else bad "Q1a: explicit facade opt-in did not PASS"; sed 's/^/        /' "$WORK/proopt.out" "$WORK/proopt.err"; fi
  echo ""

  echo "== 1i. window driver run-paired-window.sh: seam-1 selector (Q1a REVERT-PROOF) =="
  # The driver defaults GATES_PRODUCER *independently* and passes it DOWN explicitly, so a flip in
  # official-paired.sh alone is silently overridden here. Before this section the suite only
  # `bash -n`'d this script — which is why reverting run-paired-window.sh's default to `facade`
  # still passed 69/0. These rows EVALUATE the real selector lines out of the real file, so a
  # revert flips them red.
  DRV="$HERE/run-paired-window.sh"
  sel_line="$(grep -m1 '^GATES_PRODUCER="${GATES_PRODUCER:-' "$DRV")"
  if [ -n "$sel_line" ]; then
    ok "driver: seam-1 selector line located in run-paired-window.sh"
    drv_got="$(env -u GATES_PRODUCER bash -c "$sel_line"'; printf %s "$GATES_PRODUCER"')"
    [ "$drv_got" = benchmark-sh ] \
      && ok "Q1a: run-paired-window.sh DEFAULTS to benchmark-sh (revert-proof)" \
      || bad "Q1a: driver default resolved to '$drv_got', want benchmark-sh"
    drv_opt="$(GATES_PRODUCER=facade bash -c "$sel_line"'; printf %s "$GATES_PRODUCER"')"
    [ "$drv_opt" = facade ] \
      && ok "Q1a: driver honours the explicit facade opt-in" \
      || bad "Q1a: driver opt-in resolved to '$drv_opt', want facade"
  else
    bad "driver: no GATES_PRODUCER selector line found in run-paired-window.sh"
  fi
  # The window REPORT must RENDER the producer, not hardcode it. Both hardcodings this replaced were
  # wrong in opposite directions — one named the facade unconditionally, one named the reference and
  # claimed the facade REFUSES --official (retracted by R22).
  case_blk="$(sed -n '/^case "\$GATES_PRODUCER" in$/,/^esac$/p' "$DRV")"
  if [ -n "$case_blk" ]; then
    for pair in "benchmark-sh:GATE" "facade:FAC" "direct-swift:SW benchmark"; do
      pr="${pair%%:*}"; want="${pair#*:}"
      rendered="$(GATES_PRODUCER="$pr" FACADE_CMD=FAC GATE_CMD=GATE SWIFT=SW \
        bash -c "$case_blk"'; printf %s "$SEAM1_PRODUCER_CMD"')"
      [ "$rendered" = "$want" ] \
        && ok "driver report: $pr renders '$want'" \
        || bad "driver report: $pr rendered '$rendered', want '$want'"
    done
  else bad "driver: seam-1 producer-command case block not found"; fi
  grep -q 'gates (facade)' "$DRV" \
    && bad "driver report still HARDCODES 'gates (facade)'" \
    || ok "driver report: no hardcoded 'gates (facade)' cell"
  # NB: the target text is `REFUSES \`--official\`` — backslash AND backtick sit between the two
  # words, so a single-char wildcard does NOT match it. An earlier version of this pin used
  # `REFUSES .--official` and was DEAD: it could not fire even against the file that carried the
  # defect. Litmus-tested both directions against 2bb88c0 (fires) and this tree (silent).
  grep -q 'REFUSES.*--official' "$DRV" \
    && bad "driver report still carries the stale post-R22 'facade REFUSES --official' claim" \
    || ok "driver report: stale 'facade REFUSES --official' parenthetical is gone"
  echo ""

  echo "== 1j. Q1a CONFORMANCE FLOOR: the producer actually used is SEALED into the artifact =="
  # The opt-in is an ENVIRONMENT variable, so `GATES_PRODUCER=facade` can select the parity-test
  # producer for a scoring run without appearing in any command line. That is allowed to stand ONLY
  # because the run now seals which producer it used. These rows are that floor.
  #
  # (a) AMBIENT ENV — the peer's exact case: exported, not passed as an argument.
  # facade derives GATES_WS from the DRIVER's CWD (benchd's facade never cd's), so this case runs from
  # the mock harness workspace — the same root the seal must resolve. See official-paired.sh gates_workspace_root().
  if (cd "$WORK" && env $PRODUCER_ENV GATES_PRODUCER=facade NEG_CONTROL=0 OUT="$WORK/seal_f" \
       bash "$HERE/official-paired.sh" > "$WORK/seal_f.out" 2> "$WORK/seal_f.err"); then
    sealed="$(jq -r '.gates_producer // "ABSENT"' "$WORK/seal_f/proof-a/benchmark-integrity.results.json" 2>/dev/null)"
    [ "$sealed" = facade ] \
      && ok "Q1a floor: ambient GATES_PRODUCER=facade leaves a SEALED facade record (auditable)" \
      || bad "Q1a floor: ambient facade run sealed '$sealed', want facade"
    awk -F' *\\| *' '$1 ~ /seam2.integrity gates_producer/ {print $2}' "$WORK/seal_f/official-paired.table.txt" | grep -q '^PASS$' \
      && ok "Q1a floor: seam-2 gates_producer row PASSes when sealed==resolved" \
      || bad "Q1a floor: seam-2 gates_producer row did not PASS on the facade run"
  else bad "Q1a floor: ambient facade run did not PASS"; sed 's/^/        /' "$WORK/seal_f.out" "$WORK/seal_f.err"; fi
  # (b) the DEFAULT path seals the reference producer.
  if env $PRODUCER_ENV NEG_CONTROL=0 OUT="$WORK/seal_d" \
       bash "$HERE/official-paired.sh" > "$WORK/seal_d.out" 2> "$WORK/seal_d.err"; then
    sealed="$(jq -r '.gates_producer // "ABSENT"' "$WORK/seal_d/proof-a/benchmark-integrity.results.json" 2>/dev/null)"
    [ "$sealed" = benchmark-sh ] \
      && ok "Q1a floor: the DEFAULT path seals benchmark-sh" \
      || bad "Q1a floor: default run sealed '$sealed', want benchmark-sh"
  else bad "Q1a floor: default run did not PASS"; sed 's/^/        /' "$WORK/seal_d.out" "$WORK/seal_d.err"; fi
  # (c) THE FLOOR MUST BITE. A sidecar naming a DIFFERENT producer than the run resolved has to
  #     fail the run, not merely be reported. The stub seals "facade" while the driver resolves
  #     "benchmark-sh" (the default), so this exercises the real seam-2 assertion end to end —
  #     asserting the row is FAIL *and* that official-paired.sh exits nonzero because of it.
  # facade derives GATES_WS from the DRIVER's CWD (benchd's facade never cd's), so this case runs from
  # the mock harness workspace — the same root the seal must resolve. See official-paired.sh gates_workspace_root().
  if (cd "$WORK" && env $PRODUCER_ENV STUB_FORCE_GATES_PRODUCER=facade NEG_CONTROL=0 OUT="$WORK/seal_x" \
       bash "$HERE/official-paired.sh" > "$WORK/seal_x.out" 2> "$WORK/seal_x.err"); then
    bad "Q1a floor: run PASSED despite the sidecar naming a producer the run did not use"
  else
    ok "Q1a floor: a disagreeing sealed producer makes official-paired.sh exit nonzero"
    awk -F' *\\| *' '$1 ~ /seam2.integrity gates_producer/ {print $2}' "$WORK/seal_x/official-paired.table.txt" | grep -q '^FAIL$' \
      && ok "Q1a floor: seam-2 gates_producer row is FAIL on disagreement (the floor bites)" \
      || bad "Q1a floor: disagreement did not produce a FAIL row"
    grep -q "sealed='facade' resolved='benchmark-sh'" "$WORK/seal_x/official-paired.table.txt" \
      && ok "Q1a floor: the FAIL row NAMES both producers (diagnosable)" \
      || bad "Q1a floor: FAIL row does not name sealed vs resolved"
  fi
  echo ""

  # ENGINE is REQUIRED for the facade producer (the benchctl MLX engine the facade spawns).
  # facade derives GATES_WS from the DRIVER's CWD (benchd's facade never cd's), so this case runs from
  # the mock harness workspace — the same root the seal must resolve. See official-paired.sh gates_workspace_root().
  if (cd "$WORK" && env $COMMON_FACADE ENGINE='' NEG_CONTROL=0 OUT="$WORK/facnoeng" bash "$HERE/official-paired.sh" > "$WORK/facnoeng.out" 2> "$WORK/facnoeng.err"); then
    bad "facade producer ran without ENGINE set"
  else
    grep -q 'set ENGINE' "$WORK/facnoeng.err" && ok "facade producer: missing ENGINE aborts loud (fail-closed)" || { bad "facade producer no-ENGINE not loud"; sed 's/^/        /' "$WORK/facnoeng.err"; }
  fi
  echo ""

  echo "== 1k. LANE 2a REVERT-PROOF: driver STAGES + ATTESTS the hidden correctness golden (#41/#157) =="
  # THE GAP THIS CLOSES: bench #157 makes benchd REFUSE (die-8, pre-GPU) any scoring run whose
  # --contract pins a `hidden_correctness_golden` SIBLING but that carries no --correctness-golden
  # attestation. Engine #41 deploys that fixture pin. Before this driver wiring, official-paired.sh's
  # run_measure never staged the golden nor passed --correctness-golden, so EVERY driver-invoked
  # window would die-8 once #41 is the contract. No suite caught the gap (offline stubs pinned
  # nothing). This section pins the golden IN THE CONTRACT and drives the REAL run_measure path, so
  # the stub bc.sh (which now models benchd's fail-closed die-8 gate) greens WITH the wiring and
  # dies-8 WITHOUT it — the load-bearing RED/GREEN.
  #
  # COEXIST (ruled): OFFICIAL_GOLDEN stays the STAGED correctness-golden source; the flag merely
  # carries that same staged path. So the fixture pins sha256(OFFICIAL_GOLDEN) and the driver attests
  # the identical bytes — the attestation CITES the pin by construction.
  HCG_SHA="$(shasum -a 256 "$WORK/golden.json" | awk '{print $1}')"; HCG_BYTES="$(wc -c < "$WORK/golden.json" | tr -d ' ')"
  printf '{"track_id":"qwen-mtp-paired-identity","timed_prompt_pool":[],"hidden_correctness_golden":{"sha256":"%s","bytes":%s},"hidden_correctness_golden_note":"a SIBLING of timed_prompt_pool, never a ninth pool entry"}\n' \
    "$HCG_SHA" "$HCG_BYTES" > "$WORK/contract-hcg.json"
  # GREEN — with the wiring, the driver passes --correctness-golden OFFICIAL_GOLDEN (== the pinned
  # bytes), so the stub's fail-closed gate CLEARS and the three-seam chain reaches RESULT PASS.
  if env $COMMON CONTRACT="$WORK/contract-hcg.json" OUT="$WORK/hcg" bash "$HERE/official-paired.sh" > "$WORK/hcg.out" 2> "$WORK/hcg.err"; then
    grep -q 'RESULT PASS' "$WORK/hcg.out" \
      && ok "LANE 2a: a contract that PINS the hidden correctness golden reaches RESULT PASS (driver attests it)" \
      || bad "LANE 2a: pinned-golden run did not RESULT PASS"
    # The rendered seam2-cmd MUST carry --correctness-golden pointing at the staged OFFICIAL_GOLDEN,
    # so the reproduced call matches the run that actually executed.
    grep -q -- "--correctness-golden '$WORK/golden.json'" "$WORK/hcg.out" \
      && ok "LANE 2a: rendered seam2-cmd carries --correctness-golden with the staged path" \
      || { bad "LANE 2a: seam2-cmd missing --correctness-golden"; grep 'seam2-cmd' "$WORK/hcg.out" | sed 's/^/        /'; }
  else bad "LANE 2a: driver FAILED on a pinned-golden contract (the wiring should attest it)"; sed 's/^/        /' "$WORK/hcg.out" "$WORK/hcg.err"; fi
  # RED (the revert-proof, bound at the OFF-BOX seam): call the stub measure-job DIRECTLY with the
  # pinning contract and NO --correctness-golden — exactly what a reverted run_measure would emit —
  # and prove it dies-8. This is the failure the suite now catches; the GREEN row above only passes
  # because the driver actually passes the flag.
  "$STUB_BC" measure-job --out "$WORK/hcg_red" --contract "$WORK/contract-hcg.json" >/dev/null 2>"$WORK/hcg_red.err"
  red_rc=$?
  if [ "$red_rc" = 8 ] && grep -q 'no --correctness-golden attestation' "$WORK/hcg_red.err"; then
    ok "LANE 2a revert-proof: a pinned golden with NO --correctness-golden dies-8 (the die-8 gate bites)"
  else
    bad "LANE 2a revert-proof: no-attestation case did not die-8 (rc=$red_rc)"; sed 's/^/        /' "$WORK/hcg_red.err"
  fi
  # And with the flag it CLEARS (non-vacuous: the same stub accepts when the attestation cites the pin).
  if "$STUB_BC" measure-job --out "$WORK/hcg_grn" --contract "$WORK/contract-hcg.json" --correctness-golden "$WORK/golden.json" >/dev/null 2>"$WORK/hcg_grn.err"; then
    ok "LANE 2a revert-proof: the SAME stub ACCEPTS once --correctness-golden cites the fixture pin"
  else bad "LANE 2a revert-proof: stub refused a correctly-attested pinned golden"; sed 's/^/        /' "$WORK/hcg_grn.err"; fi
  echo ""

  echo "== 2. NEGATIVE: floor-fail measure results → overlay cannot fabricate a green =="
  if env $COMMON STUB_FLOOR_FAIL=1 NEG_CONTROL=0 OUT="$WORK/pf" bash "$HERE/official-paired.sh" > "$WORK/pf.out" 2> "$WORK/pf.err"; then
    bad "official-paired PASSED despite floor-fail measure results"
  else
    grep -q 'RESULT FAIL' "$WORK/pf.err" && ok "official-paired RESULT FAIL on floor-fail results" || bad "floor-fail missing RESULT FAIL"
    awk -F' *\\| *' '$1 ~ /seam3.exit-code/ {print $2}' "$WORK/pf/official-paired.table.txt" | grep -q '^FAIL$' \
      && ok "floor-fail: seam3 exit-code row FAIL (overlay exits nonzero)" || bad "floor-fail seam3 exit row not FAIL"
    # R20: a floored score is AUTHORED as 0.0 (finite, but out of [floor,ceil]) with passed=false — so
    # the load-bearing "not green" row is the score-in-band check (0.0 ∉ [0.90,5.0]), not score-finite.
    awk -F' *\\| *' '$1 ~ /seam3.score in/ {print $2}' "$WORK/pf/official-paired.table.txt" | grep -q '^FAIL$' \
      && ok "floor-fail: seam3 score-in-band row FAIL (0.0 out of [floor,ceil]; R20 refusal sentinel)" || bad "floor-fail score-in-band row not FAIL"
    awk -F' *\\| *' '$1 ~ /seam3.passed/ {print $2}' "$WORK/pf/official-paired.table.txt" | grep -q '^FAIL$' \
      && ok "floor-fail: seam3 passed row FAIL (cannot fabricate green)" || bad "floor-fail passed row not FAIL"
  fi
  echo ""

  echo "== 3a. ANTI-FABRICATION seam 1 (mock swift emits no payload → seal fails → no gates-score → TOOL-ERR) =="
  if env $COMMON MOCK_SWIFT_NONE=1 NEG_CONTROL=0 OUT="$WORK/n1" bash "$HERE/official-paired.sh" > "$WORK/n1.out" 2> "$WORK/n1.err"; then
    bad "official-paired PASSED despite the swift binary emitting NO sealable payload"
  else
    grep -q 'RESULT FAIL' "$WORK/n1.err" && ok "seam1 no-artifact → RESULT FAIL" || bad "seam1 no-artifact missing RESULT FAIL"
    grep -qi 'TOOL-ERR' "$WORK/n1/official-paired.table.txt" && ok "seam1 no-artifact: TOOL-ERR rendered" || { bad "seam1 no TOOL-ERR cell"; sed 's/^/        /' "$WORK/n1/official-paired.table.txt"; }
  fi
  echo ""

  echo "== 3b. ANTI-FABRICATION seam 2 (no results.json → TOOL-ERR / die 5) =="
  if env $COMMON STUB_RESULTS_NONE=1 NEG_CONTROL=0 OUT="$WORK/n2" bash "$HERE/official-paired.sh" > "$WORK/n2.out" 2> "$WORK/n2.err"; then
    bad "official-paired PASSED despite measure-job writing NO results.json"
  else
    grep -q 'RESULT FAIL' "$WORK/n2.err" && ok "seam2 no-artifact → RESULT FAIL" || bad "seam2 no-artifact missing RESULT FAIL"
    awk -F' *\\| *' '$1 ~ /seam2.results.json/ {print $2}' "$WORK/n2/official-paired.table.txt" | grep -q '^FAIL$' \
      && ok "seam2 no-artifact: results.json TOOL-ERR row FAIL" || { bad "seam2 results TOOL-ERR row not FAIL"; sed 's/^/        /' "$WORK/n2/official-paired.table.txt"; }
    # seam 3 depends on seam 2 → its score.json is absent too → TOOL-ERR downstream
    grep -qi 'TOOL-ERR' "$WORK/n2/official-paired.table.txt" && ok "seam2 no-artifact: downstream TOOL-ERR rendered" || bad "seam2 no downstream TOOL-ERR"
  fi
echo ""

# =====================================================================================================
# 2b. LANE 2b — per-leg engine-binary IDENTITY re-verification (gate-attested sha; per-phase)
# =====================================================================================================
# official-paired.sh re-hashes the engine binary measure-job will spawn
# (<PAIRED_WS>/.build/release/mlxfast-engine) against the gate-attested OFFICIAL_ENGINE_BIN_SHA256
# at the TOP of EACH leg — the calibration/gates leg (seam 1) and the timed execution leg (seam 2).
# A binary swapped BETWEEN the legs is caught at the second check, fail-closed (die-class 12), BEFORE
# measure-job runs. The gate-attested sha is the value window-preflight.sh sealed (WP_ENGINE_BIN_SHA256),
# NOT a fresh self-hash — a self-hash would verify a swapped binary against itself.
#
# REVERT-PROOF (both call sites pinned): remove the run_measure verify → the "swap between legs" case
# completes instead of dying → its rc-12 row flips RED. Remove the run_gates verify → a bad-from-start
# binary reaches seam 2 first and dies naming seam2-execution → the "bad from start" leg-label row
# (expects seam1-calibration) flips RED.
echo "== 2b. per-leg engine-binary re-verification (gate-attested sha) =="
L2B="$WORK/l2b"; mkdir -p "$L2B/ws/.build/release"
L2B_ENGINE="$L2B/ws/.build/release/mlxfast-engine"
mk_engine() { printf '#!/bin/sh\nexit 0\n' > "$L2B_ENGINE"; chmod +x "$L2B_ENGINE"; }
mk_engine
L2B_GOOD_SHA="$(shasum -a 256 "$L2B_ENGINE" | awk '{print $1}')"
L2B_COMMON="$QMTP_POOL $HARNESS_ENV GATES_PRODUCER=direct-swift BENCHCTL=$STUB_BC SWIFT=$MOCK_SWIFT GATE_CMD=$MOCK_BENCH OFFICIAL_GOLDEN=$WORK/golden.json CONTRACT=$WORK/contract.json PAIRED_WS=$L2B/ws WEIGHTS=$WORK/weights OFFICIAL_COMMIT=deadbeefcafe"

# (positive control) matching gate sha → both legs verify, the driver runs the whole chain.
mk_engine
if env $L2B_COMMON OFFICIAL_ENGINE_BIN_SHA256="$L2B_GOOD_SHA" NEG_CONTROL=0 OUT="$L2B/match" bash "$HERE/official-paired.sh" > "$L2B/match.out" 2> "$L2B/match.err"; then
  grep -q 'RESULT PASS' "$L2B/match.out" && ok "2b: engine-bin sha matches gate seal → both legs verify, chain runs (no false trip)" || bad "2b: matching sha did not reach RESULT PASS"
else
  bad "2b: matching engine-bin sha must NOT trip the guard"; sed 's/^/        /' "$L2B/match.out" "$L2B/match.err"
fi

# (swap between legs) a seam-1 producer that swaps the engine binary DURING gates → seam-2 verify dies.
L2B_SWAP_SWIFT="$L2B/swap-swift.sh"
cat > "$L2B_SWAP_SWIFT" <<EOF
#!/bin/bash
# stand in for the seam-1 direct producer: emit the canned gates payload (sealed from STDOUT), THEN
# swap the engine binary on disk — a competitor replacing the measured binary AFTER the calibration
# leg's identity check has passed, before the timed execution leg.
cat <<'JSON'
{ "score": null, "passed": true,
  "metrics": { "partial_result": true, "passed_correctness": true, "error": "",
    "decode_speedup_floor": 0.95, "passed_decode_speedup_floor": false,
    "case_count": 12, "checked_steps": 34, "benchmark_wall_seconds": 1.5, "peak_ram_gb": 20.25,
    "weights_hash": "stubw", "weights_file_count": 1, "weights_byte_count": 1,
    "harness_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "commit": "deadbeefcafe" } }
JSON
printf '#!/bin/sh\nexit 7\n' > "$L2B_ENGINE"
exit 0
EOF
chmod +x "$L2B_SWAP_SWIFT"
mk_engine   # start from the GOOD binary the gate sealed
if env $L2B_COMMON SWIFT="$L2B_SWAP_SWIFT" OFFICIAL_ENGINE_BIN_SHA256="$L2B_GOOD_SHA" NEG_CONTROL=0 OUT="$L2B/swap" bash "$HERE/official-paired.sh" > "$L2B/swap.out" 2> "$L2B/swap.err"; then
  bad "2b: a binary swapped between legs must DIE, not complete the chain"; sed 's/^/        /' "$L2B/swap.out"
else
  rc=$?
  [ "$rc" = 12 ] && ok "2b: swap between legs → die-class 12 at the execution leg" || bad "2b: swap died rc=$rc want 12"
  { grep -q 'seam2-execution' "$L2B/swap.err" && grep -q 'engine binary sha mismatch' "$L2B/swap.err"; } \
    && ok "2b: mismatch caught at seam2-execution (measure leg), naming the gate seal" || { bad "2b: swap diagnostic missing/mislabelled"; sed 's/^/        /' "$L2B/swap.err"; }
fi

# (bad from start) the engine binary already differs from the gate seal BEFORE seam 1 → seam-1 verify
# dies, naming the calibration leg (pins the run_gates call site specifically).
printf '#!/bin/sh\nexit 9\n' > "$L2B_ENGINE"; chmod +x "$L2B_ENGINE"   # sha != L2B_GOOD_SHA
if env $L2B_COMMON OFFICIAL_ENGINE_BIN_SHA256="$L2B_GOOD_SHA" NEG_CONTROL=0 OUT="$L2B/pre" bash "$HERE/official-paired.sh" > "$L2B/pre.out" 2> "$L2B/pre.err"; then
  bad "2b: a binary that never matched the gate seal must DIE at the calibration leg"; sed 's/^/        /' "$L2B/pre.out"
else
  rc=$?
  [ "$rc" = 12 ] && ok "2b: bad-from-start engine binary → die-class 12" || bad "2b: bad-from-start died rc=$rc want 12"
  grep -q 'seam1-calibration' "$L2B/pre.err" \
    && ok "2b: caught at seam1-calibration (gates leg) — pins the calibration-leg check" || { bad "2b: bad-from-start not caught at the calibration leg"; sed 's/^/        /' "$L2B/pre.err"; }
fi

# (opt-in) unset gate sha → NOTE + skip (dev/offline runs unaffected; the guard is fail-closed ONLY
# when the window pins the seal). The bad binary is still on disk but must NOT trip the guard.
if env $L2B_COMMON NEG_CONTROL=0 OUT="$L2B/optin" bash "$HERE/official-paired.sh" > "$L2B/optin.out" 2> "$L2B/optin.err"; then
  grep -q 'OFFICIAL_ENGINE_BIN_SHA256 unset' "$L2B/optin.err" && ok "2b: unset gate sha → NOTE + skip (opt-in, per-golden idiom)" || bad "2b: unset gate sha did not emit the opt-in NOTE"
else
  bad "2b: unset gate sha must not fail the run (opt-in)"; sed 's/^/        /' "$L2B/optin.out" "$L2B/optin.err"
fi
echo ""

# =====================================================================================================
# 2c. #148 ACTIVATION end-to-end — official_swift_run ARMS the engine belt; a swapped worker refused
# =====================================================================================================
# official_swift_run spawns the mlxfast-swift HARNESS, whose RuntimeWorkerExecutablePin (#31 belt)
# re-verifies the worker it is about to spawn against MLXFAST_RUNTIME_WORKER_EXECUTABLE_SHA256, read
# from the HARNESS's OWN environment — a shell env that never passes through benchd's
# sanitized_engine_env (that sanitizer builds only the WORKER-CHILD env, downstream of the belt).
# This models the belt with a faithful stand-in $SWIFT and proves the WIRING: the gate-attested pin
# + REQUIRED=1 REACH the belt (not stripped on this path) and a worker whose bytes != the gate seal
# is REFUSED. Revert-proof: dropping `belt_pin_env` from official_swift_run makes the swap slip
# through (the belt sees no pin) → the refuse-row and the wiring-proof row both flip RED. The real
# mlxfast-swift belt's arm/verify/refuse behaviour is unit-proven in the engine repo; the true
# real-binary spawn is the box leg.
echo "== 2c. #148 activation end-to-end (belt armed by official_swift_run; swap refused) =="
BELT_SWIFT="$WORK/belt-swift.sh"
cat > "$BELT_SWIFT" <<'EOF'
#!/bin/bash
# Faithful stand-in for the mlxfast-swift harness: run the belt contract against MY OWN env (what
# enforceBeforeSpawn reads via ProcessInfo) BEFORE emitting the gates payload. Refuse on a mismatch,
# or on an absent pin when the gate marked it MANDATORY; otherwise proceed.
bsha() { shasum -a 256 "$1" | awk '{print $1}'; }
pin="${MLXFAST_RUNTIME_WORKER_EXECUTABLE_SHA256:-}"
req="${MLXFAST_RUNTIME_WORKER_EXECUTABLE_SHA256_REQUIRED:-}"
exe="${MLXFAST_RUNTIME_WORKER_EXECUTABLE:-}"
# Record exactly what the belt SAW so the test can prove the wiring delivered pin+required unstripped.
[ -n "${BELT_ENV_SIDECAR:-}" ] && printf 'pin=%s req=%s exe=%s\n' "$pin" "$req" "$exe" > "$BELT_ENV_SIDECAR"
if [ -z "$pin" ]; then
  if [ "$req" = 1 ]; then echo "belt: refusing to spawn — pin MANDATORY (REQUIRED=1) but none declared" >&2; exit 3; fi
else
  actual="$(bsha "$exe")"
  if [ "$actual" != "$pin" ]; then echo "belt: refusing to spawn the participant runtime worker: its sha256 $actual does not match the pinned $pin" >&2; exit 3; fi
fi
cat <<JSON
{ "score": null, "passed": true,
  "metrics": { "partial_result": true, "passed_correctness": true, "error": "",
    "decode_speedup_floor": 0.95, "passed_decode_speedup_floor": false,
    "prefill_speedup_floor": 0.95, "passed_prefill_speedup_floor": false,
    "case_count": 12, "checked_steps": 34, "benchmark_wall_seconds": 1.5, "peak_ram_gb": 20.25,
    "weights_hash": "stubw", "weights_file_count": 1, "weights_byte_count": 1,
    "harness_hash": "${MOCK_HARNESS_HASH:?belt mock swift needs MOCK_HARNESS_HASH}",
    "commit": "deadbeefcafe" } }
JSON
exit 0
EOF
chmod +x "$BELT_SWIFT"
# The harness hashes MLXFAST_RUNTIME_WORKER_EXECUTABLE, which official_swift_run sets to $swift_abs
# (= the SWIFT binary itself). So the "worker" the belt verifies is $BELT_SWIFT.
BELT_GOOD_SHA="$(shasum -a 256 "$BELT_SWIFT" | awk '{print $1}')"
BELT_WRONG_SHA="$([ "${BELT_GOOD_SHA:0:1}" = "0" ] && echo "1${BELT_GOOD_SHA:1}" || echo "0${BELT_GOOD_SHA:1}")"
BELT_SIDE="$WORK/belt.env"

# (armed + matching seal) → belt verifies, run proceeds; and the belt SAW pin + REQUIRED=1.
if env $COMMON SWIFT="$BELT_SWIFT" OFFICIAL_WORKER_BIN_SHA256="$BELT_GOOD_SHA" BELT_ENV_SIDECAR="$BELT_SIDE" NEG_CONTROL=0 OUT="$WORK/belt-ok" bash "$HERE/official-paired.sh" > "$WORK/belt-ok.out" 2> "$WORK/belt-ok.err"; then
  grep -q 'RESULT PASS' "$WORK/belt-ok.out" && ok "2c: official_swift_run armed the belt with a matching gate seal → run proceeds" || bad "2c: matching seal did not reach RESULT PASS"
else
  bad "2c: a matching gate seal must not trip the belt"; sed 's/^/        /' "$WORK/belt-ok.out" "$WORK/belt-ok.err"
fi
[ -f "$BELT_SIDE" ] && grep -q "pin=$BELT_GOOD_SHA req=1 " "$BELT_SIDE" \
  && ok "2c: wiring delivered pin + REQUIRED=1 to the harness env (belt saw them; NOT stripped on this path)" \
  || { bad "2c: belt did not receive the pin/REQUIRED (wiring or sanitizer stripped them)"; cat "$BELT_SIDE" 2>/dev/null; }

# (armed + swapped worker) gate seal != the worker's on-disk bytes → belt REFUSES at spawn.
if env $COMMON SWIFT="$BELT_SWIFT" OFFICIAL_WORKER_BIN_SHA256="$BELT_WRONG_SHA" NEG_CONTROL=0 OUT="$WORK/belt-swap" bash "$HERE/official-paired.sh" > "$WORK/belt-swap.out" 2> "$WORK/belt-swap.err"; then
  bad "2c: a worker whose bytes != the gate seal must be REFUSED at spawn (belt did not fire)"; sed 's/^/        /' "$WORK/belt-swap.out"
else
  { grep -rq 'does not match the pinned' "$WORK/belt-swap" 2>/dev/null || grep -q 'does not match the pinned' "$WORK/belt-swap.out" "$WORK/belt-swap.err" 2>/dev/null; } \
    && ok "2c: swapped worker (bytes != gate seal) REFUSED by the armed belt → seam-1 fails" \
    || { bad "2c: swap not refused with the belt error"; sed 's/^/        /' "$WORK/belt-swap.err"; }
fi
echo ""

# =====================================================================================================
# 2d. #148 REGRESSION — the REAL OFFICIAL_WORKER_BIN_SHA256 default sources the SWIFT-WORKER seal
# =====================================================================================================
# The item-4 bug: the belt fires on the direct-swift path and verifies the mlxfast-swift WORKER
# ($swift_abs), but OFFICIAL_WORKER_BIN_SHA256 defaulted to the ENGINE seal (WP_ENGINE_BIN_SHA256 =
# sha(mlxfast-engine), a DIFFERENT binary) — so a gate-armed run FALSE-REFUSED an honest worker.
# §2c missed it by injecting OFFICIAL_WORKER_BIN_SHA256 directly. This drives the REAL sourcing line
# from run-paired-window.sh (do NOT inject the worker sha), with the gate sealing BOTH binaries.
# Fixed: sources WP_SWIFT_WORKER_BIN_SHA256 → honest worker PASSES, swapped worker REFUSED. Against
# the old engine-seal default it goes RED (the honest worker is false-refused).
echo "== 2d. #148 regression: real OFFICIAL_WORKER_BIN_SHA256 default → swift-worker seal =="
D2_WS_ENGINE="$WORK/ws/.build/release/mlxfast-engine"      # the seam-2 engine (a DIFFERENT binary)
D2_WS_ENGINE_SHA="$(shasum -a 256 "$D2_WS_ENGINE" | awk '{print $1}')"
D2_SWIFT_SHA="$(shasum -a 256 "$BELT_SWIFT" | awk '{print $1}')"   # the direct-swift worker (§2c stand-in)
# Exercise the ACTUAL sourcing line from run-paired-window.sh (revert-proof against that line): with
# the gate sealing BOTH binaries in env, derive OFFICIAL_WORKER_BIN_SHA256 the way the window does.
D2_SRC="$(grep -E '^OFFICIAL_WORKER_BIN_SHA256=' "$HERE/run-paired-window.sh" | head -1)"
d2_derive() { # <wp_engine_sha> <wp_swift_sha> -> the derived OFFICIAL_WORKER_BIN_SHA256
  env -u OFFICIAL_WORKER_BIN_SHA256 WP_ENGINE_BIN_SHA256="$1" WP_SWIFT_WORKER_BIN_SHA256="$2" \
    bash -c "set -u; $D2_SRC"'; printf "%s" "${OFFICIAL_WORKER_BIN_SHA256:-}"'
}
D2_DERIVED="$(d2_derive "$D2_WS_ENGINE_SHA" "$D2_SWIFT_SHA")"
# (regression assertion) the real default must pick the SWIFT-worker seal, never the engine seal.
{ [ "$D2_DERIVED" = "$D2_SWIFT_SHA" ] && [ "$D2_DERIVED" != "$D2_WS_ENGINE_SHA" ]; } \
  && ok "2d: run-paired-window sources OFFICIAL_WORKER_BIN_SHA256 from the swift-worker seal (not the engine seal)" \
  || bad "2d: OFFICIAL_WORKER_BIN_SHA256 sourced WRONGLY — derived=${D2_DERIVED:0:12}… swift=${D2_SWIFT_SHA:0:12}… engine=${D2_WS_ENGINE_SHA:0:12}…"

# (honest end-to-end via the REAL derived pin) HALF A armed on the engine seal (passes); the belt
# armed on the DERIVED worker pin must NOT false-refuse the honest, un-swapped swift worker.
if env $COMMON SWIFT="$BELT_SWIFT" OFFICIAL_ENGINE_BIN_SHA256="$D2_WS_ENGINE_SHA" OFFICIAL_WORKER_BIN_SHA256="$D2_DERIVED" NEG_CONTROL=0 OUT="$WORK/d2-ok" bash "$HERE/official-paired.sh" > "$WORK/d2-ok.out" 2> "$WORK/d2-ok.err"; then
  grep -q 'RESULT PASS' "$WORK/d2-ok.out" && ok "2d: real-default arming → honest swift worker PASSES (no false refusal — item-4 fixed)" || bad "2d: honest worker did not reach RESULT PASS"
else
  bad "2d: real-default arming FALSE-REFUSED an honest swift worker (item-4 regression)"; sed 's/^/        /' "$WORK/d2-ok.err"
fi

# (swapped worker via the REAL derived pin) change the worker's bytes after sealing → belt REFUSES.
printf '# swapped-%s\n' "$RANDOM" >> "$BELT_SWIFT"   # sha now != the sealed WP_SWIFT_WORKER_BIN_SHA256
if env $COMMON SWIFT="$BELT_SWIFT" OFFICIAL_ENGINE_BIN_SHA256="$D2_WS_ENGINE_SHA" OFFICIAL_WORKER_BIN_SHA256="$D2_DERIVED" NEG_CONTROL=0 OUT="$WORK/d2-swap" bash "$HERE/official-paired.sh" > "$WORK/d2-swap.out" 2> "$WORK/d2-swap.err"; then
  bad "2d: a swapped swift worker must be REFUSED (belt did not fire)"; sed 's/^/        /' "$WORK/d2-swap.out"
else
  { grep -rq 'does not match the pinned' "$WORK/d2-swap" 2>/dev/null || grep -q 'does not match the pinned' "$WORK/d2-swap.out" "$WORK/d2-swap.err" 2>/dev/null; } \
    && ok "2d: real-default arming → swapped swift worker REFUSED by the belt" \
    || { bad "2d: swap not refused"; sed 's/^/        /' "$WORK/d2-swap.err"; }
fi
echo ""

# =====================================================================================================
# 3. CROSS-LEG HARNESS-IDENTITY GATE at the seal (David ruling 2026-08-26)
# =====================================================================================================
echo "== 3c. cross-leg harness identity: driver-pinned seal CWD + the real equality =="
# EVERY happy three-seam chain above already runs THROUGH this gate: the mock producers stamp the
# REAL identity of $WORK (computed by `benchctl harness-hash` over that tree, not reimplemented in
# shell), the driver cds to the same root for seam 3, and benchd re-resolves it there. So the rows
# in sections 1/2 are the ACCEPTANCE side of this gate — they cannot pass if it is mis-wired.
# The rows below are the refusal side, plus the proof the driver's cd is load-bearing.

# The driver must ANNOUNCE the root it pinned, and it must be the mock harness workspace.
grep -q "seam-1 gates workspace root (seam-3 seal CWD) = " "$WORK/ph.err" \
  && ok "3c: driver announces the pinned seam-3 seal CWD" \
  || bad "3c: driver did not announce a pinned seam-3 seal CWD"
grep -q "^seam3-cmd: (cd '" "$WORK/ph.out" \
  && ok "3c: seam3-cmd reproduction line carries the cd (rerunnable as executed)" \
  || bad "3c: seam3-cmd does not record the cd"

echo ""
echo "== 3d. TAMPER TWIN: a gates leg from a DIFFERENT harness is REFUSED at the seal =="
# The between-phase TOCTOU, offline: everything is the honest chain except the gates score claims a
# harness identity that is not the tree the seal resolves. Same 64-lowercase-hex SHAPE, so it clears
# the single-leg well-formedness gate and the refusal under test is unambiguously the CROSS-LEG one.
# A DELETED or WARN-ONLY gate makes this row red.
if env $COMMON MOCK_HARNESS_HASH="$HARNESS_HASH_TAMPERED" NEG_CONTROL=0 OUT="$WORK/xleg" bash "$HERE/official-paired.sh" > "$WORK/xleg.out" 2> "$WORK/xleg.err"; then
  bad "3d: a MISMATCHED harness identity reached RESULT PASS (the cross-leg gate is not enforcing)"
  sed 's/^/        /' "$WORK/xleg.out"
else
  ok "3d: mismatched harness legs → chain FAILS (nonzero exit)"
  if grep -rq 'cross-leg harness-identity mismatch' "$WORK/xleg" 2>/dev/null; then
    ok "3d: refusal is the cross-leg harness-identity one"
  else
    bad "3d: chain failed but not with the cross-leg refusal"; sed 's/^/        /' "$WORK/xleg.out"
  fi
  grep -rq 'CHANGED BETWEEN PHASES' "$WORK/xleg" 2>/dev/null \
    && ok "3d: refusal states the between-phase-mutation implication" \
    || bad "3d: refusal does not state the between-phase implication"
  grep -rq "${HARNESS_HASH:0:12}" "$WORK/xleg" 2>/dev/null && grep -rq "${HARNESS_HASH_TAMPERED:0:12}" "$WORK/xleg" 2>/dev/null \
    && ok "3d: refusal names BOTH digests (12-char previews)" \
    || bad "3d: refusal does not name both digests"
  [ -f "$WORK/xleg/proof-a/score.json" ] \
    && bad "3d: a score.json was authored despite the refusal" \
    || ok "3d: NO score.json authored on the refusal (fail-closed, nothing published)"
fi

echo ""
echo "== 3e. the driver's cd is LOAD-BEARING: seal from a non-harness CWD fails closed =="
# Re-run the REAL overlay on the honest artifacts of the happy chain, but from a directory that is
# NOT a harness workspace — i.e. exactly what seam 3 did before the ruling, when it inherited the
# operator's CWD. It must REFUSE (resolution failure), never publish. This is the script-level proof
# that removing `cd "$GATES_WS"` from official-paired.sh turns the suite red.
NOWS="$WORK/not-a-harness"; mkdir -p "$NOWS"
if (cd "$NOWS" && env $QMTP_POOL "$REAL_BENCHCTL" overlay-timing \
      --gates-score "$WORK/ph/proof-a/gates/score.json" \
      --results "$WORK/ph/proof-a/results.json" \
      --score-path "$NOWS/score.json") > "$WORK/nows.out" 2> "$WORK/nows.err"; then
  bad "3e: the seal PUBLISHED from a non-harness CWD (fail-closed resolution missing)"
  sed 's/^/        /' "$WORK/nows.err"
else
  ok "3e: seal from a non-harness CWD → nonzero exit (fail-closed)"
  grep -q 'AT THE SEAL' "$WORK/nows.err" \
    && ok "3e: refusal says the identity could not be resolved AT THE SEAL" \
    || { bad "3e: refusal text is not the seal-time resolution one"; sed 's/^/        /' "$WORK/nows.err"; }
  grep -q 'harnessHash root missing from disk' "$WORK/nows.err" \
    && ok "3e: refusal names the missing harness root (F1 fail-closed cause, verbatim)" \
    || bad "3e: refusal does not name the missing root"
  [ -f "$NOWS/score.json" ] \
    && bad "3e: a score.json was authored from a non-harness CWD" \
    || ok "3e: NO score.json authored (nothing published unchecked)"
fi
echo ""

# =====================================================================================================
# 4. R11 — window robustness (run-paired-window.sh + official-lib.sh helpers; NO GPU)
# =====================================================================================================
echo "== 4a. R11 structural: HUP trap + dual lock (mkdir BOX_LOCK & flock) + seam-1 precheck BEFORE unload =="
WIN="$HERE/run-paired-window.sh"
grep -Eq "trap 'cleanup; exit 129' HUP" "$WIN" && ok "R11: HUP added to the cleanup trap (ssh-drop still reloads qwen)" || bad "R11: no HUP trap"
# bench#143 MEDIUM: the box lock is INHERIT-ONLY — the driver must NOT self-acquire via mkdir, must
# obtain it by holder-tag inheritance, and must NOT release it (the gate --release owns that).
grep -q 'mkdir "\$BOX_LOCK"' "$WIN" && bad "R11/MEDIUM: driver still SELF-ACQUIRES BOX_LOCK via mkdir" || ok "R11/MEDIUM: inherit-only — driver never mkdir-acquires BOX_LOCK"
grep -q 'INHERITED from the window-preflight gate' "$WIN" && ok "R11/MEDIUM: BOX_LOCK obtained by holder-tag INHERITANCE of the gate-held lock" || bad "R11/MEDIUM: no holder-tag inheritance path"
grep -q 'parity_take_gpu_lock' "$WIN" && ok "R11: flock(fd 9) still taken (this driver's own dialect)" || bad "R11: flock gone"
grep -q 'rmdir "\$BOX_LOCK"' "$WIN" && bad "R11/MEDIUM: driver still rmdir's the inherited BOX_LOCK (gate --release owns release)" || ok "R11/MEDIUM: inherit-only — driver never releases the inherited BOX_LOCK"
grep -q 'official_qwen_unload_verify' "$WIN" && ok "R11: unload goes through official_qwen_unload_verify (rc+proc check)" || bad "R11: no unload-verify"
grep -q 'official_qwen_health_probe' "$WIN" && ok "R11: post-reload health probe wired in cleanup" || bad "R11: no health probe"
# Ordering: the SEAM1_ONLY precheck must appear BEFORE the qwen unload line (hard-stop before spending it).
S1_LINE="$(grep -n 'SEAM1_ONLY=1' "$WIN" | head -1 | cut -d: -f1)"
UNLOAD_LINE="$(grep -n 'UNLOAD qwen (+ verify' "$WIN" | head -1 | cut -d: -f1)"
{ [ -n "$S1_LINE" ] && [ -n "$UNLOAD_LINE" ] && [ "$S1_LINE" -lt "$UNLOAD_LINE" ]; } \
  && ok "R11: seam-1 precheck (line $S1_LINE) precedes qwen unload (line $UNLOAD_LINE)" || bad "R11: seam-1 precheck not before unload ($S1_LINE vs $UNLOAD_LINE)"
echo ""

echo "== 4b. R11 unit: official_qwen_unload_verify — rc check + proc-gone verify (stub qwen_unload) =="
# rc!=0 → abort (nonzero); rc==0 + proc gone → 0; rc==0 + proc still resident → nonzero.
uv_fail_rc="$(bash -c '. "'"$HERE"'/official-lib.sh"; qwen_unload(){ return 3; }; official_qwen_unload_verify >/dev/null 2>&1; echo $?')"
[ "$uv_fail_rc" != 0 ] && ok "R11: qwen_unload rc!=0 → official_qwen_unload_verify aborts (rc=$uv_fail_rc)" || bad "R11: unload rc!=0 not aborted"
uv_ok_rc="$(bash -c '. "'"$HERE"'/official-lib.sh"; qwen_unload(){ return 0; }; QWEN_PROC_PATTERN="no_such_proc_zzz_'"$$"'" QWEN_UNLOAD_VERIFY_INTERVAL=0 official_qwen_unload_verify >/dev/null 2>&1; echo $?')"
[ "$uv_ok_rc" = 0 ] && ok "R11: qwen_unload rc==0 + proc gone → verify passes (rc=0)" || bad "R11: unload happy path rc=$uv_ok_rc"
# Deterministic lingering proc: a uniquely-named background script pgrep -f can match.
LING="$WORK/lingering_qwen_marker_proc.sh"; printf '#!/bin/bash\nsleep 30\n' > "$LING"; chmod +x "$LING"
"$LING" & LING_PID=$!
uv_ling_rc="$(bash -c '. "'"$HERE"'/official-lib.sh"; qwen_unload(){ return 0; }; QWEN_PROC_PATTERN="lingering_qwen_marker_proc" QWEN_UNLOAD_VERIFY_TRIES=2 QWEN_UNLOAD_VERIFY_INTERVAL=0 official_qwen_unload_verify >/dev/null 2>&1; echo $?')"
kill "$LING_PID" 2>/dev/null; wait "$LING_PID" 2>/dev/null
[ "$uv_ling_rc" != 0 ] && ok "R11: unload rc==0 but proc lingering → verify FAILS (rc=$uv_ling_rc)" || bad "R11: lingering proc not caught"
echo ""

echo "== 4c. R11 unit: official_qwen_health_probe — stub curl healthy path + loud timeout =="
CURL_OK="$WORK/curl-ok"; cat > "$CURL_OK" <<'EOF'
#!/bin/bash
url="${@: -1}"
case "$url" in
  */v1/models) echo '{"data":[{"id":"qwen-test"}]}' ;;
  */health)    echo '{"status":"ok"}' ;;
esac
EOF
chmod +x "$CURL_OK"
CURL_DOWN="$WORK/curl-down"; printf '#!/bin/bash\nexit 0\n' > "$CURL_DOWN"; chmod +x "$CURL_DOWN"
hp_ok="$(bash -c '. "'"$HERE"'/official-lib.sh"; QWEN_CURL="'"$CURL_OK"'" QWEN_HEALTH_FLOOR=0 QWEN_HEALTH_POLL_INTERVAL=0 official_qwen_health_probe "qwen-test" 5 >/dev/null 2>&1; echo $?')"
[ "$hp_ok" = 0 ] && ok "R11: health probe → HEALTHY when /v1/models has the model id AND /health up (rc=0)" || bad "R11: health probe healthy path rc=$hp_ok"
hp_down_out="$(bash -c '. "'"$HERE"'/official-lib.sh"; QWEN_CURL="'"$CURL_DOWN"'" QWEN_HEALTH_FLOOR=0 QWEN_HEALTH_POLL_INTERVAL=0 official_qwen_health_probe "qwen-test" 1 2>&1; echo "RC=$?"')"
printf '%s' "$hp_down_out" | grep -q 'NOT HEALTHY' && ok "R11: health probe LOGS LOUDLY on timeout (does not declare success)" || bad "R11: health probe timeout not loud"
printf '%s' "$hp_down_out" | grep -q 'RC=0' && bad "R11: health probe returned 0 despite being down" || ok "R11: health probe returns nonzero when never healthy"
echo ""

  echo "== 4d. R11 functional: SEAM1_ONLY hard-stop — failing gates producer never spends seam 2 =="
  # SEAM1_ONLY=1 + a gates producer that writes NO gates-score → official-paired.sh must exit nonzero
  # BEFORE seam 2, so measure-job is NEVER invoked (measure.exit absent). This is the exact property
  # the window relies on to abort WITHOUT unloading qwen.
  if env $COMMON_BENCHSH GATE_CMD_SHA="$MOCK_BENCH_SHA" MOCK_BENCH_NONE=1 SEAM1_ONLY=1 NEG_CONTROL=0 OUT="$WORK/s1f" bash "$HERE/official-paired.sh" > "$WORK/s1f.out" 2> "$WORK/s1f.err"; then
    bad "R11: SEAM1_ONLY with a failing gates producer did NOT abort"
  else
    grep -q 'SEAM1_ONLY FAIL' "$WORK/s1f.err" && ok "R11: SEAM1_ONLY FAIL emitted (hard-stop before seam 2)" || { bad "R11: no SEAM1_ONLY FAIL line"; sed 's/^/        /' "$WORK/s1f.err"; }
    [ ! -f "$WORK/s1f/proof-a/measure.exit" ] && ok "R11: seam 2 (measure-job) NEVER ran on seam-1 fail (measure.exit absent)" || bad "R11: measure-job ran despite seam-1 fail"
  fi
  # And SEAM1_ONLY on a GOOD gates producer → exit 0 (window would proceed to unload), still no seam 2.
  if env $COMMON_BENCHSH GATE_CMD_SHA="$MOCK_BENCH_SHA" SEAM1_ONLY=1 NEG_CONTROL=0 OUT="$WORK/s1p" bash "$HERE/official-paired.sh" > "$WORK/s1p.out" 2> "$WORK/s1p.err"; then
    grep -q 'SEAM1_ONLY OK' "$WORK/s1p.out" && ok "R11: SEAM1_ONLY OK on valid gates (window may unload)" || bad "R11: no SEAM1_ONLY OK line"
    [ ! -f "$WORK/s1p/proof-a/measure.exit" ] && ok "R11: SEAM1_ONLY exits before seam 2 even on success (qwen-light precheck)" || bad "R11: seam 2 ran in SEAM1_ONLY mode"
  else bad "R11: SEAM1_ONLY on a valid gates producer did not exit 0"; sed 's/^/        /' "$WORK/s1p.out" "$WORK/s1p.err"; fi
echo ""

echo "== 4e. bench#143 wire-a+MEDIUM: scoring-window PREFLIGHT GATE + INHERIT-ONLY lock (REVERT-PROOF) =="
# run-paired-window.sh has NO non-scoring mode — it always seals a ranked score.json — so every
# window it starts is a SCORING window. Two motions gate it: (wire a) refuse unless window-preflight
# ran and sealed a PASS attestation for THIS window; (MEDIUM) the box lock is INHERIT-ONLY — the
# driver never self-acquires it, so a scoring window can only run INSIDE the gate's live
# acquire-and-hold window. A stale attestation replayed against a FREE lock is refused, not run.
WIN="$HERE/run-paired-window.sh"

# (i) STRUCTURE: the enforcement call exists and runs BEFORE the lock is taken. Reverting the wire =
#     deleting this call; the ordering row then has nothing to find and flips red.
GATE_LINE="$(grep -n '^require_passed_preflight$' "$WIN" | head -1 | cut -d: -f1)"
LOCK_LINE="$(grep -n 'parity_take_gpu_lock' "$WIN" | head -1 | cut -d: -f1)"
{ [ -n "$GATE_LINE" ] && [ -n "$LOCK_LINE" ] && [ "$GATE_LINE" -lt "$LOCK_LINE" ]; } \
  && ok "wire-a: require_passed_preflight (line $GATE_LINE) runs BEFORE the GPU lock (line $LOCK_LINE)" \
  || bad "wire-a: preflight gate not present before the lock ($GATE_LINE vs $LOCK_LINE)"

# (ii) NON-VACUITY + refusal reasons: evaluate the REAL predicate out of the file (section-1i style).
#      A valid attestation → PASS (proves the gate is not always-refuse); every tamper → its reason.
pav="$(sed -n '/^preflight_attestation_verdict() {$/,/^}$/p' "$WIN")"
[ -n "$pav" ] && ok "wire-a: preflight_attestation_verdict predicate located" || bad "wire-a: predicate not found"
pav_eval() { bash -c "$pav"'; preflight_attestation_verdict "$1" "$2"' _ "$1" "$2"; }
GD="$WORK/wire-a-gate"; mkdir -p "$GD"
printf '%s\n' '{"schema":"window-provenance/v1","verdict":"PASS","lock":{"window_tag":"tagT"},"lock_taken":true}'  > "$GD/pass.json"
printf '%s\n' '{"schema":"window-provenance/v1","verdict":"FAIL","lock":{"window_tag":"tagT"},"lock_taken":true}'  > "$GD/fail.json"
printf '%s\n' '{"schema":"window-provenance/v1","verdict":"PASS","lock":{"window_tag":"OTHER"},"lock_taken":true}' > "$GD/othertag.json"
printf '%s\n' '{"schema":"window-provenance/v1","verdict":"PASS","lock":{"window_tag":"tagT"},"lock_taken":false}' > "$GD/nottaken.json"
printf '%s\n' '{"schema":"other/v1","verdict":"PASS"}'                                                            > "$GD/wrongschema.json"
[ "$(pav_eval tagT "$GD/pass.json")"       = "PASS" ]                 && ok "wire-a: valid PASS attestation + matching tag ACCEPTED (not vacuous)" || bad "wire-a: valid attestation not accepted"
[ "$(pav_eval tagT "$GD/fail.json")"       = "verdict-not-pass:FAIL" ] && ok "wire-a: FAILED gate verdict refused" || bad "wire-a: FAIL verdict not refused"
[ "$(pav_eval tagT "$GD/othertag.json")"   != "PASS" ]               && ok "wire-a: attestation for a DIFFERENT window (tag mismatch) refused" || bad "wire-a: stale-tag attestation not refused"
[ "$(pav_eval tagT "$GD/nottaken.json")"   != "PASS" ]               && ok "wire-a: attestation with lock_taken=false refused" || bad "wire-a: un-taken lock not refused"
[ "$(pav_eval tagT "$GD/wrongschema.json")" != "PASS" ]              && ok "wire-a: non-provenance schema refused" || bad "wire-a: wrong schema not refused"
[ "$(pav_eval '' "$GD/pass.json")"         = "no-window-tag" ]        && ok "wire-a: gate-skipped (no WP_WINDOW_TAG) refused" || bad "wire-a: missing tag not refused"
[ "$(pav_eval tagT "$GD/no-such-file.json")" = "attestation-unreadable" ] && ok "wire-a: UNREADABLE/absent attestation refused (bench#143 NIT)" || bad "wire-a: unreadable attestation not refused"

# (iii) END-TO-END on the REAL driver. Harmless temp paths for locks/out/replica so nothing on the
#       box is touched. The driver aborts later on missing binaries — we assert ONLY the gate/lock
#       decision, which is the property under test.
drv_run() { # <label> [prelock-tag|-] [env KEY=VAL ...]  -> writes $GD/<label>.out, echoes rc
  local out="$1" pretag="$2"; shift 2
  local box="$GD/$out.box.lock.d"; rm -rf "$box"
  # A gate-HELD lock, exactly as window-preflight.sh leaves one, when a prelock tag is given.
  if [ "$pretag" != "-" ]; then mkdir -p "$box"; printf 'tag=%s\npid=999990\n' "$pretag" > "$box/holder"; printf '999990\n' > "$box/pid"; fi
  env -u WP_WINDOW_TAG -u WP_ATTESTATION -u WP_OUT "$@" \
    MLXFAST_PARITY_GIT="$GD/pg" MLXFAST_PARITY_HOME="$GD/ph" OUT="$GD/$out.o" \
    REPLICA_LOCAL="$GD/$out.replica" \
    MLXFAST_GPU_LOCK="$GD/$out.gpu.lock" MLXFAST_BOX_LOCK="$box" \
    bash "$WIN" > "$GD/$out.out" 2>&1
  echo $?
}
# direction 1 — gate SKIPPED → REFUSE before any lock (revert-proof: remove require_passed_preflight
# and this fails, because the driver reaches "take GPU lock" instead of refusing).
rc_skip="$(drv_run skip -)"
{ [ "$rc_skip" != 0 ] \
  && grep -q 'SCORING WINDOW REFUSED' "$GD/skip.out" \
  && ! grep -q 'take GPU lock' "$GD/skip.out"; } \
  && ok "wire-a: gate-skipped scoring window REFUSED before the lock (rc=$rc_skip)" \
  || { bad "wire-a: gate-skipped window not refused before the lock (rc=$rc_skip)"; sed 's/^/        /' "$GD/skip.out"; }
# direction 2 — valid attestation + a currently gate-HELD lock under the same tag → INHERIT and
# proceed (the ONLY sanctioned way in; proves the gate is not always-refuse).
rc_inh="$(drv_run inh tagT WP_WINDOW_TAG=tagT WP_ATTESTATION="$GD/pass.json")"
{ grep -q 'scoring-window preflight gate: PASSED' "$GD/inh.out" \
  && grep -q 'INHERITED from the window-preflight gate' "$GD/inh.out" \
  && ! grep -q 'SCORING WINDOW REFUSED' "$GD/inh.out"; } \
  && ok "wire-a: valid attestation + gate-held lock → INHERITS and proceeds (sanctioned path)" \
  || { bad "wire-a: valid attestation did not inherit the gate-held lock"; sed 's/^/        /' "$GD/inh.out"; }
# direction 3 (bench#143 MEDIUM) — STALE REPLAY: valid attestation, SAME tag, but the gate-held lock
# is now FREE (its window ended) → REFUSE, and never self-acquire. Revert-proof: restore the mkdir
# self-acquire and this window would run behind a spent attestation; here it must refuse and leave
# no BOX_LOCK behind.
rc_stale="$(drv_run stale - WP_WINDOW_TAG=tagT WP_ATTESTATION="$GD/pass.json")"
{ [ "$rc_stale" != 0 ] \
  && grep -q 'SCORING WINDOW REFUSED: STALE attestation' "$GD/stale.out" \
  && ! grep -q 'INHERITED from the window-preflight gate' "$GD/stale.out" \
  && [ ! -d "$GD/stale.box.lock.d" ]; } \
  && ok "wire-a: STALE attestation + FREE lock REFUSED, no self-acquire (bench#143 MEDIUM)" \
  || { bad "wire-a: stale-replay not refused / self-acquired (rc=$rc_stale, lock-dir exists=$([ -d "$GD/stale.box.lock.d" ] && echo yes || echo no))"; sed 's/^/        /' "$GD/stale.out"; }
echo ""

echo "==================================================="
echo "paired three-seam offline self-test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
