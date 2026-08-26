#!/bin/bash
# scripts/window-preflight.sh — THE WINDOW-PREFLIGHT GATE.
#
# Run this from the laptop BEFORE every GPU window, before the lock is taken. It fails closed
# on anything that would make the window's evidence unclaimable, and it seals what it observed
# into a window-provenance attestation next to the run artifacts.
#
# WHY THIS EXISTS. The program pins and fails closed at every CODE seam — golden sha256+bytes,
# spawn argv, protocol version, roster supersets — and never extended that rigor to the
# ENVIRONMENT seam. The Proof A window (2026-08-20) discovered, under the lock and on the
# clock, that the box had NEITHER required pin provisioned: the bench checkout was on the wrong
# branch and the parity engine repo was absent entirely, shipped ad hoc by bundle. Then every
# live leg died at `engine hello handshake failed` — a seam no GPU-free preflight can see,
# because `measure-job --preflight-only` returns before the first spawn (main.rs:1205-1215).
#
# PHASE ORDER — each phase fails closed before the next, and the lock is taken exactly once,
# after everything that can be checked without it has already passed:
#
#   1. PINS    (lock-free) — the tree seam. Every SHA, digest and byte count the window depends
#              on is asserted against an EXPLICIT expected pin. No pin has a default: a value
#              that is not supplied is a usage error, never a guess, because a default is
#              exactly the thing that drifts silently.
#   2. BASICS  (lock-free) — the box seam. Binaries present, executable, and interrogated for
#              their own identity; goldens accepted by the very loader the run will use; disk
#              above an explicit floor; box quiet; serving model in the expected state; and the
#              box lock ACQUIRABLE (or reapable) with no ambiguous holder.
#   3. ACQUIRE THE SESSION LOCK — atomically, recording holder identity and the box's own
#              timestamp. From here the box is single-flighted BY THE GATE, not merely asserted
#              to be free.
#   4. QWEN UNLOAD (+ verify the process is actually gone; rc=0 is not proof).
#   5. SMOKE   — the spawn seam, under the lock, with the serving model unloaded: the actual
#              benchd binary spawning the actual worker over the actual transport, through the
#              env scrub and the hello handshake and at least one real round trip. ~60 s of GPU
#              that would have caught #134 before a whole window was spent on it.
#   6. HOLD    — on success the gate EXITS STILL HOLDING THE LOCK, and the window proper runs
#              inside it. On any failure the trap reloads qwen and releases, so a failed gate
#              never leaves the box locked and unloaded.
#   7. RELEASE — `--release` reloads the serving model, verifies it is back, and releases the
#              lock this session took (and only that one: ownership is proved by the holder
#              tag, so a release can never clear someone else's lock).
#
# BARE PROBES ARE PROHIBITED. Every GPU-touching motion — full windows, smoke legs, one-off
# diagnostics, hypothesis probes — runs inside this lock and this environment class. A control
# that does not share the lock and residency conditions of the legs it controls for is not a
# control. See docs/window-preflight.md.
#
# The gate NEVER deletes anything and NEVER reaps a lock it did not take. Provisioning is a
# separate, explicit motion — see scripts/window-provision.sh, or pass --provision.
#
# USAGE
#   scripts/window-preflight.sh --pins <FILE> [--box <ALIAS>] [--driver ssh|local]
#                               [--out <DIR>] [--box-out <DIR>] [--provision]
#                               [--pin KEY=VALUE]... [--no-smoke]
#   scripts/window-preflight.sh --pins <FILE> --release      # after the window: reload + unlock
#
# EXIT CODES (aligned with the window drivers' de-facto convention)
#   0  PASS — every phase green; the window may take the lock
#   1  FAIL — a pinned assertion mismatched (SHA, digest, byte count, dirty tree)
#   2  usage / a required pin was not supplied
#   3  box unavailable — a lock is held, or the box is not quiet
#   4  a pinned comparison input was rejected — a golden (pin or loader) or a pool tape
#      (pin or required-key signature)
#   5  a required tree, binary or prerequisite is missing
#   6  serving-model state is not what the window expects
#   7  REFUSED — a bundle-shipped tree whose bundle hash is not pinned
#   8  SMOKE LEG FAILED — spawn/handshake/decode broke
#   9  transport error — the probe could not be run at all
set -uo pipefail

HERE="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROBE="$HERE/window-probe.sh"
PROVISION="$HERE/window-provision.sh"

E_PASS=0; E_FAIL=1; E_USAGE=2; E_BOX=3; E_GOLDEN=4; E_MISSING=5
E_QWEN=6; E_REFUSED=7; E_SMOKE=8; E_TRANSPORT=9

die_usage() { printf 'window-preflight: %s\n' "$1" >&2; exit "$E_USAGE"; }

# ------------------------------------------------------------------- pins ----
# Pins live in a flat KEY=VALUE file. It is PARSED, never sourced: a pins file is data the
# operator edits under time pressure, and `source` would make a stray backtick arbitrary code.
PINS_FILE=""; DRIVER=""; BOX=""; OUT=""; BOX_OUT=""; DO_PROVISION=0; NO_SMOKE=0; DO_RELEASE=0
OVERRIDES=""

while [ $# -gt 0 ]; do
  case "$1" in
    --pins)          PINS_FILE="${2-}"; shift 2 ;;
    --driver)        DRIVER="${2-}"; shift 2 ;;
    --box)           BOX="${2-}"; shift 2 ;;
    --out)           OUT="${2-}"; shift 2 ;;
    --box-out)       BOX_OUT="${2-}"; shift 2 ;;
    --provision)     DO_PROVISION=1; shift ;;
    --no-smoke)      NO_SMOKE=1; shift ;;
    --release)       DO_RELEASE=1; shift ;;
    --pin)           OVERRIDES="$OVERRIDES
${2-}"; shift 2 ;;
    -h|--help)       sed -n '2,60p' "$0"; exit 0 ;;
    *)               die_usage "unknown argument: $1" ;;
  esac
done

[ -n "$PINS_FILE" ] || die_usage "--pins <FILE> is required (there are no default pins, by design)"
[ -r "$PINS_FILE" ] || die_usage "pins file not readable: $PINS_FILE"

PINS_RAW="$(grep -v '^[[:space:]]*#' "$PINS_FILE" | grep -v '^[[:space:]]*$')
$OVERRIDES"

# C-1: `pin` deliberately takes the LAST assignment so `--pin` can override the file. That same
# rule makes a DUPLICATE key inside the file silently win, which is a fine way to edit a pins
# file under time pressure and not notice. Warn — loudly, once, naming the keys.
DUPES="$(grep -v '^[[:space:]]*#' "$PINS_FILE" | grep -v '^[[:space:]]*$' \
  | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p' | LC_ALL=C sort | uniq -d)"
if [ -n "$DUPES" ]; then
  printf 'window-preflight: WARNING — %s declares these keys more than once; the LAST wins:\n' "$PINS_FILE" >&2
  printf '%s\n' "$DUPES" | sed 's/^/    /' >&2
fi

# pin <KEY> — print the LAST assignment of KEY (so --pin overrides the file), or "".
pin() {
  local k="$1" line v=""
  while IFS= read -r line; do
    case "$line" in
      "$k="*) v="${line#"$k"=}" ;;
    esac
  done <<EOF
$PINS_RAW
EOF
  printf '%s' "$v"
}
# must <VALUE> <KEY> — a pin with no value is a usage error, never a default.
#
# NOTE the shape: this validates AFTER assignment rather than exiting from inside a
# `$(...)`. An `exit` in a command substitution only kills the subshell, so a
# `V="$(require KEY)"` helper would silently leave V empty and let the gate run on with an
# unset pin — which is precisely the class of silent default this gate exists to abolish.
must() {
  [ -n "$1" ] || die_usage "required pin $2 is not set (explicit pins only — no defaults that could silently drift)"
}

[ -n "$DRIVER" ] || DRIVER="$(pin WP_DRIVER)"
[ -n "$DRIVER" ] || DRIVER="ssh"
[ -n "$BOX" ]    || BOX="$(pin WP_BOX)"
[ -n "$OUT" ]    || OUT="$(pin WP_OUT)"
[ -n "$BOX_OUT" ]|| BOX_OUT="$(pin WP_BOX_OUT)"
[ -n "$OUT" ] || die_usage "--out <DIR> (or pin WP_OUT) is required — the attestation must have a home"
case "$DRIVER" in
  ssh)   [ -n "$BOX" ] || die_usage "--box <ALIAS> (or pin WP_BOX) is required under DRIVER=ssh" ;;
  local) ;;
  *)     die_usage "--driver must be ssh or local (got: $DRIVER)" ;;
esac

BENCH_PATH="$(pin WP_BENCH_PATH)";              must "$BENCH_PATH" WP_BENCH_PATH
BENCH_SHA="$(pin WP_BENCH_SHA)";                must "$BENCH_SHA" WP_BENCH_SHA
ENGINE_PATH="$(pin WP_ENGINE_PATH)";            must "$ENGINE_PATH" WP_ENGINE_PATH
ENGINE_SHA="$(pin WP_ENGINE_SHA)";              must "$ENGINE_SHA" WP_ENGINE_SHA
ENGINE_BIN="$(pin WP_ENGINE_BIN)";              must "$ENGINE_BIN" WP_ENGINE_BIN
ENGINE_BIN_SHA="$(pin WP_ENGINE_BIN_SHA256)";   must "$ENGINE_BIN_SHA" WP_ENGINE_BIN_SHA256
BENCHD_BIN="$(pin WP_BENCHD_BIN)";              must "$BENCHD_BIN" WP_BENCHD_BIN
BENCHD_BIN_SHA="$(pin WP_BENCHD_BIN_SHA256)";   must "$BENCHD_BIN_SHA" WP_BENCHD_BIN_SHA256
WEIGHTS_PATH="$(pin WP_WEIGHTS_PATH)";          must "$WEIGHTS_PATH" WP_WEIGHTS_PATH
WEIGHTS_SHA="$(pin WP_WEIGHTS_SHA256)";         must "$WEIGHTS_SHA" WP_WEIGHTS_SHA256
WEIGHTS_FC="$(pin WP_WEIGHTS_FILE_COUNT)";      must "$WEIGHTS_FC" WP_WEIGHTS_FILE_COUNT
WEIGHTS_BC="$(pin WP_WEIGHTS_BYTE_COUNT)";      must "$WEIGHTS_BC" WP_WEIGHTS_BYTE_COUNT
MIN_FREE_GB="$(pin WP_MIN_FREE_GB)";            must "$MIN_FREE_GB" WP_MIN_FREE_GB
MAX_LOADAVG="$(pin WP_MAX_LOADAVG)";            must "$MAX_LOADAVG" WP_MAX_LOADAVG
# The two other box-quiet conditions are PINNED rather than hard-wired, for the same reason
# every other threshold here is: a check with no pin is a policy nobody declared. A real window
# pins both to 1; a harness that must run on a contended machine declares 0, and the waiver is
# then recorded in the attestation instead of being invisible.
REQ_TM_IDLE="$(pin WP_REQUIRE_TIMEMACHINE_IDLE)"; must "$REQ_TM_IDLE" WP_REQUIRE_TIMEMACHINE_IDLE
REQ_NO_STRAY="$(pin WP_REQUIRE_NO_STRAY)";        must "$REQ_NO_STRAY" WP_REQUIRE_NO_STRAY
QWEN_PATTERN="$(pin WP_QWEN_PROC_PATTERN)";     must "$QWEN_PATTERN" WP_QWEN_PROC_PATTERN
QWEN_EXPECT="$(pin WP_QWEN_EXPECT)";            must "$QWEN_EXPECT" WP_QWEN_EXPECT
# ONE LOCK (RULED, David 2026-08-20). The gate acquires and holds THE box lock — the same
# `/tmp/mtplx-box-exclusive.lock.d` every actor already respects — rather than a private session
# lock beside it. run-paired-window.sh gained holder-tag inheritance so it proceeds under a lock
# the gate is holding on its behalf, and never releases one it inherited.
BOX_LOCK="$(pin WP_BOX_LOCK)";                  must "$BOX_LOCK" WP_BOX_LOCK
GPU_LOCK="$(pin WP_GPU_LOCK)";                  must "$GPU_LOCK" WP_GPU_LOCK
WINDOW_TAG="$(pin WP_WINDOW_TAG)";              must "$WINDOW_TAG" WP_WINDOW_TAG
LOCK_REAP_AGE_S="$(pin WP_LOCK_REAP_AGE_S)";    must "$LOCK_REAP_AGE_S" WP_LOCK_REAP_AGE_S

# M1/M3 — THE ENVIRONMENT IS PART OF THE ENVIRONMENT SEAM. Pinning the engine binary's sha256
# is worth nothing if `MLXFAST_RUNTIME_WORKER_EXECUTABLE` can redirect the spawn to a different
# file; `MLXFAST_NO_SANDBOX` silently changes the spawn CLASS; and an unset `QMTP_HEAD_DIR`
# kills the decode recipe before the spawn, which reads as a post-handshake failure to anyone
# not looking for it. Each `WP_ENV_<NAME>` pin is an expected VALUE, or the literal `unset`.
# These three are REQUIRED — declare `unset` rather than omitting, so the expectation is on the
# record either way. Extra vars may be pinned by adding more WP_ENV_* lines.
ENV_WATCH=""
ENV_PINNED_NAMES=""
# The spawn-critical set, from the code:
#   *_EXECUTABLE          on iterate --mode official the sandbox takes it VERBATIM in preference
#                         to the resolved path (sandbox.rs:229-236) — defeats WP_ENGINE_BIN_SHA256
#   *_MEASURE_WORKER_BIN  changes WHICH file inside the workspace is spawned (main.rs:1035-1038)
#   *_SANDBOX_PROFILE     short-circuits profile generation AND the sandbox-exec probe
#                         (sandbox.rs:257-259) — nominally sandboxed, effectively not
#   *_NO_SANDBOX          "1" turns the whole window into an instant refusal
#   *_USE_RUNTIME_WORKER  "0"/"false" likewise
#   QMTP_HEAD_DIR         the --mtp-head argv value; checked AFTER --preflight-only returns
#                         (main.rs:1219-1225), so a GPU-free preflight cannot catch it unset
for _req in MLXFAST_RUNTIME_WORKER_EXECUTABLE MLXFAST_MEASURE_WORKER_BIN \
            MLXFAST_RUNTIME_WORKER_SANDBOX_PROFILE MLXFAST_NO_SANDBOX \
            MLXFAST_USE_RUNTIME_WORKER QMTP_HEAD_DIR; do
  _v="$(pin "WP_ENV_$_req")"
  must "$_v" "WP_ENV_$_req"
done
# Collect every WP_ENV_* pin the file declares (the three above plus any extras).
ENV_PINNED_NAMES="$(printf '%s\n' "$PINS_RAW" | sed -n 's/^WP_ENV_\([A-Za-z0-9_]*\)=.*/\1/p' | LC_ALL=C sort -u)"
ENV_WATCH="$(printf '%s' "$ENV_PINNED_NAMES" | tr '\n' ' ')"
QWEN_SERVICE="$(pin WP_QWEN_SERVICE)";          must "$QWEN_SERVICE" WP_QWEN_SERVICE
SMOKE_RECIPE="$(pin WP_SMOKE_RECIPE)";          must "$SMOKE_RECIPE" WP_SMOKE_RECIPE
# The known-dependency posture, as a VERDICT rather than prose. Until #134's engine-seam fix
# lands (PR #136), the handshake recipe is expected to fail against a real box — that is the
# gate working. Naming the dependency turns a red run into a legible EXPECTED-FAIL(<ref>)
# instead of something a reader has to interpret. It does NOT waive the failure: the exit code
# and the gate verdict are unchanged, because the window still must not proceed.
EXPECT_SMOKE_FAIL="$(pin WP_EXPECT_SMOKE_FAIL)"; must "$EXPECT_SMOKE_FAIL" WP_EXPECT_SMOKE_FAIL
ENGINE_ID_ARGV="$(pin WP_ENGINE_IDENTITY_ARGV)"; must "$ENGINE_ID_ARGV" WP_ENGINE_IDENTITY_ARGV
ENGINE_ID_EXPECT="$(pin WP_ENGINE_IDENTITY_EXPECT)"
BENCHD_ID_ARGV="$(pin WP_BENCHD_IDENTITY_ARGV)"; must "$BENCHD_ID_ARGV" WP_BENCHD_IDENTITY_ARGV
BENCHD_ID_EXPECT="$(pin WP_BENCHD_IDENTITY_EXPECT)"
# LANE 2b (#148) — OPTIONAL seal of the mlxfast-swift WORKER the direct-swift producer's harness
# spawns (official_swift_run sets MLXFAST_RUNTIME_WORKER_EXECUTABLE to it). This is a DIFFERENT
# binary from WP_ENGINE_BIN (the measure-job engine), so it carries its OWN seal; the engine belt
# verifies THIS file, and run-paired-window.sh sources OFFICIAL_WORKER_BIN_SHA256 from
# WP_SWIFT_WORKER_BIN_SHA256 (never from the engine seal). Pinned only for a window that runs that
# producer; unset ⇒ the seal is skipped and the belt stays opt-in for that window. Sealing
# discipline mirrors enginebin (byte sha + identity RUN). A pinned binary MUST carry a pinned sha —
# a path is not a seal — so a half-pin fails closed.
SWIFT_WORKER_BIN="$(pin WP_SWIFT_WORKER_BIN)"
SWIFT_WORKER_BIN_SHA="$(pin WP_SWIFT_WORKER_BIN_SHA256)"
SWIFT_WORKER_ID_ARGV="$(pin WP_SWIFT_WORKER_IDENTITY_ARGV)"; [ -n "$SWIFT_WORKER_ID_ARGV" ] || SWIFT_WORKER_ID_ARGV="none"
SWIFT_WORKER_ID_EXPECT="$(pin WP_SWIFT_WORKER_IDENTITY_EXPECT)"
[ -z "$SWIFT_WORKER_BIN" ] || must "$SWIFT_WORKER_BIN_SHA" WP_SWIFT_WORKER_BIN_SHA256
CONTRACT_PATH="$(pin WP_CONTRACT_PATH)";    CONTRACT_SHA="$(pin WP_CONTRACT_SHA256)"
BENCH_BUNDLE_SHA="$(pin WP_BENCH_BUNDLE_SHA256)"
ENGINE_BUNDLE_SHA="$(pin WP_ENGINE_BUNDLE_SHA256)"
SMOKE_TIMEOUT="$(pin WP_SMOKE_TIMEOUT_S)"; [ -n "$SMOKE_TIMEOUT" ] || SMOKE_TIMEOUT=300
[ "$NO_SMOKE" = "1" ] && SMOKE_RECIPE="none"

if [ -n "$CONTRACT_PATH" ] && [ -z "$CONTRACT_SHA" ]; then
  die_usage "WP_CONTRACT_PATH is pinned but WP_CONTRACT_SHA256 is not — a path without a digest is not a pin"
fi

# Goldens: WP_GOLDEN_1..N, each "<path> <sha256> <bytes>". At least one is required.
# NEW-3: a pin is "<path> <sha256> <bytes>" and THE PATH MAY CONTAIN SPACES. Taking the path as
# field 1 truncated it at the first space, so a correctly-pinned artifact under, say,
# `/Volumes/Pool Tapes/t.json` was looked up as `/Volumes/Pool` and reported ABSENT (exit 5) —
# the gate blaming the operator for a path it had mangled itself. The two pin values are fixed
# width and always LAST, so parse from the right: bytes = NF, sha256 = NF-1, path = everything
# before them, spaces and all.
# Pure parameter expansion on the SPACE separator. awk splits on any whitespace and rebuilds the
# record with single spaces, so a path containing a literal TAB came back normalized — the same
# class of self-inflicted mangling as the field-1 truncation, just narrower. These forms touch
# only the last two space-separated fields and return the rest of the line byte for byte.
pin_bytes() { printf '%s' "${1##* }"; }
pin_sha()   { local _r="${1% *}"; printf '%s' "${_r##* }"; }
pin_path()  { local _r="${1% *}"; printf '%s' "${_r% *}"; }
# Well-formed means "at least two spaces", which is what the two trailing fields require.
pin_wellformed() { case "$1" in *' '*' '*) return 0 ;; *) return 1 ;; esac; }

GOLDEN_PATHS=""; GOLDEN_PINS=""; GN=0
while :; do
  g="$(pin "WP_GOLDEN_$((GN + 1))")"
  [ -n "$g" ] || break
  GN=$((GN + 1))
  pin_wellformed "$g" \
    || die_usage "WP_GOLDEN_$GN must be '<path> <sha256> <bytes>' (the sha256+bytes pin is TWO numbers, both required)"
  gp="$(pin_path "$g")"
  gs="$(pin_sha "$g")"
  gb="$(pin_bytes "$g")"
  [ -n "$gp" ] && [ -n "$gs" ] && [ -n "$gb" ] \
    || die_usage "WP_GOLDEN_$GN must be '<path> <sha256> <bytes>' (the sha256+bytes pin is TWO numbers, both required)"
  GOLDEN_PATHS="$GOLDEN_PATHS$gp
"
  GOLDEN_PINS="$GOLDEN_PINS$gs $gb
"
done

# POOL TAPES — WP_POOL_TAPE_1..N, same "<path> <sha256> <bytes>" shape as a golden.
#
# A separate family because they are a separate KIND, not a naming preference. "Golden" is
# reserved for an artifact carrying the weights-hash + prompts + prompt-SHAs binding; the R2
# track-pool objects are TAPES — contract-pinned comparison inputs. The distinction is load
# bearing here: `benchctl validate-golden` REJECTS a tape outright ("unknown field
# `seed_tokens`"), so tapes are checked against the required-key SIGNATURE measure-job routes on
# instead. Filing them under WP_GOLDEN_* would have meant either a false loader failure on every
# tape, or dropping the shape check entirely.
TAPE_PATHS=""; TAPE_PINS=""; TN=0
while :; do
  t="$(pin "WP_POOL_TAPE_$((TN + 1))")"
  [ -n "$t" ] || break
  TN=$((TN + 1))
  pin_wellformed "$t" \
    || die_usage "WP_POOL_TAPE_$TN must be '<path> <sha256> <bytes>' (the sha256+bytes pin is TWO numbers, both required)"
  tp="$(pin_path "$t")"
  ts="$(pin_sha "$t")"
  tb="$(pin_bytes "$t")"
  [ -n "$tp" ] && [ -n "$ts" ] && [ -n "$tb" ] \
    || die_usage "WP_POOL_TAPE_$TN must be '<path> <sha256> <bytes>' (the sha256+bytes pin is TWO numbers, both required)"
  TAPE_PATHS="$TAPE_PATHS$tp
"
  TAPE_PINS="$TAPE_PINS$ts $tb
"
done

[ "$((GN + TN))" -gt 0 ] \
  || die_usage "at least one comparison input is required: WP_GOLDEN_1 or WP_POOL_TAPE_1, each '<path> <sha256> <bytes>'"

# ------------------------------------------------------- optional provision --
if [ "$DO_PROVISION" = "1" ]; then
  printf '== provisioning (window-provision.sh) ==\n'
  # N-e: forward the CLI pin overrides. Provisioning that reads only the file builds a different
  # tree than the gate then verifies, and the mismatch surfaces as a confusing digest failure
  # rather than as "you overrode a pin and provisioning never heard about it".
  #
  # Built as an ARRAY: an override value may contain spaces (a pin path legitimately can), and a
  # word-split string would tear it apart — the same defect NEW-3 fixed on the parsing side.
  # `${BOX:+--box "$BOX"}` is quoted for the same reason.
  PROV_ARGS=(--pins "$PINS_FILE" --driver "$DRIVER")
  [ -n "$BOX" ] && PROV_ARGS+=(--box "$BOX")
  while IFS= read -r _ov; do
    [ -n "$_ov" ] && PROV_ARGS+=(--pin "$_ov")
  done <<EOF
$OVERRIDES
EOF
  "$PROVISION" "${PROV_ARGS[@]}" || {
    printf 'window-preflight: provisioning failed — gate NOT run (the gate never assumes provisioning succeeded)\n' >&2
    exit $E_MISSING
  }
fi

# ------------------------------------------------------------- smoke recipe --
# The smoke leg is built from PINS ALREADY ASSERTED ABOVE, so it cannot drift from the tree the
# gate just cleared. Recipes are named and explicit; `none` is a DECLARATION that this window
# waives the spawn seam, recorded as such in the attestation, never a silent skip.
#
#   handshake — `benchctl prefill-decompose --sizes 1,2 --reps 1`. The cheapest REAL spawn in
#               the tree: 2 spawns, 2 hellos, 2 round trips, no contract, no golden, no
#               sandbox, seconds of GPU. Crucially it runs the LOCAL spawn path, where
#               `forward_worker_stderr` is TRUE (transport.rs:476), so a dying worker's own
#               stderr reaches us prefixed `mlxfast-worker: `. This is the recipe that would
#               have caught #134.
#   decode    — the measure-job miniature: one serial-spec leg with a 1-token window, which is
#               the smallest existing invocation that issues a real `decode_begin` +
#               `decode_step` over the SANDBOXED transport. Needs a contract whose
#               timed_prompt_pool pins golden 1, and QMTP_HEAD_DIR in the environment. Note
#               that on this path benchd forwards NO worker stderr (sandbox.rs:277) and
#               `join_stderr_drain()` has no production caller, so a failure here yields only
#               the RunnerError string — run `handshake` first, always.
#   both      — handshake, then decode. The recommended setting for a measure-job window.
#   none      — declared waiver.
SMOKE_ARGV=""
# The argv is EVAL'd on the box, so every field it contains must survive one round of shell
# parsing. Paths are single-quoted (a box path with a space would otherwise split), and the
# spec JSON is single-quoted too — bare `{"mode":"serial"}` comes out of `eval` as
# `{mode:serial}`, which benchd rejects as malformed. `sq` wraps a value in single quotes and
# escapes any single quote inside it, which is the only form that is total.
SQ="'"
sq() { printf "%s%s%s" "$SQ" "$(printf '%s' "$1" | sed "s/$SQ/$SQ\\\\$SQ$SQ/g")" "$SQ"; }
ENGINE_WS="$ENGINE_PATH"
smoke_argv_for() {
  case "$1" in
    handshake)
      printf '%s prefill-decompose --engine %s --weights %s --sizes 1,2 --reps 1' \
        "$(sq "$BENCHD_BIN")" "$(sq "$ENGINE_BIN")" "$(sq "$WEIGHTS_PATH")" ;;
    decode)
      [ -n "$CONTRACT_PATH" ] || return 1
      # M2: the smoke leg must match the REAL leg's ARGV SHAPE, not a prefix of it. A real
      # candidate leg spawns, via sandbox-exec:
      #   <worker> runtime-worker --weights W --mtp-head H --speculative-protocol v1.1
      # `--mtp-head` is emitted on every leg; `--speculative-protocol v1.1` ONLY when the
      # candidate regime is free-run (spec.mode != "serial", measure_job.rs:179-185,699-706).
      # A `{"mode":"serial"}` spec is teacher-forced, so its argv is a strict PREFIX that omits
      # `--speculative-protocol` — and an engine that rejects that flag would sail through the
      # smoke leg and then kill every real leg pre-GPU with the same
      # "closed the stream before returning a response" that #134 reported. So the recipe
      # speculates.
      #
      # `--tokens` is deliberately ABSENT: on the free-run branch benchd makes it a hard usage
      # error unless it equals the ruled FREE_RUN_DECODE_TOKENS (main.rs:838-849), so omitting
      # it takes the ruled N. This is also why measure-job is the SANDBOXED recipe — every
      # measure-job leg goes through sandbox_exec_command, with no unsandboxed fallback.
      printf '%s measure-job --candidate %s --baseline %s --golden %s --contract %s --weights %s --candidate-spec %s --min-pairs 1 --target-pairs 1 --tag window-preflight-smoke --out %s' \
        "$(sq "$BENCHD_BIN")" "$(sq "$ENGINE_WS")" "$(sq "$ENGINE_WS")" \
        "$(sq "$(if [ -n "$TAPE_PATHS" ]; then printf '%s' "$TAPE_PATHS" | sed -n 1p
                 else printf '%s' "$GOLDEN_PATHS" | sed -n 1p; fi)")" \
        "$(sq "$CONTRACT_PATH")" "$(sq "$WEIGHTS_PATH")" \
        "$(sq '{"mode":"mtp","mtp":{"depth":2}}')" \
        "$(sq "${BOX_OUT:-/tmp}/smoke-decode")" ;;
    *) return 1 ;;
  esac
}
case "$SMOKE_RECIPE" in
  none) SMOKE_ARGV="" ;;
  handshake|decode)
    SMOKE_ARGV="$(smoke_argv_for "$SMOKE_RECIPE")" \
      || die_usage "smoke recipe '$SMOKE_RECIPE' needs WP_CONTRACT_PATH pinned" ;;
  both)
    a="$(smoke_argv_for handshake)"
    b="$(smoke_argv_for decode)" || die_usage "smoke recipe 'both' needs WP_CONTRACT_PATH pinned"
    SMOKE_ARGV="$a && $b" ;;
  custom)
    SMOKE_ARGV="$(pin WP_SMOKE_ARGV)"; must "$SMOKE_ARGV" WP_SMOKE_ARGV ;;
  *) die_usage "WP_SMOKE_RECIPE must be one of: handshake decode both custom none (got: $SMOKE_RECIPE)" ;;
esac

# ------------------------------------------------------------ run the probe --
b64() { base64 | tr -d '\n'; }
if printf 'eA==' | base64 -d >/dev/null 2>&1; then B64D="-d"; else B64D="-D"; fi
unb64() { printf '%s' "$1" | base64 "$B64D" 2>/dev/null; }
rq() { printf '%s=%s\n' "$1" "$(printf '%s' "$2" | b64)"; }

REQ="$( {
  rq bench_path            "$BENCH_PATH"
  rq engine_path           "$ENGINE_PATH"
  rq engine_bin            "$ENGINE_BIN"
  rq benchd_bin            "$BENCHD_BIN"
  rq engine_identity_argv  "$ENGINE_ID_ARGV"
  rq benchd_identity_argv  "$BENCHD_ID_ARGV"
  rq swift_worker_bin           "$SWIFT_WORKER_BIN"
  rq swift_worker_identity_argv "$SWIFT_WORKER_ID_ARGV"
  rq weights_path          "$WEIGHTS_PATH"
  rq golden_paths          "$GOLDEN_PATHS"
  rq golden_pins           "$GOLDEN_PINS"
  rq pool_tape_paths       "$TAPE_PATHS"
  rq contract_path         "$CONTRACT_PATH"
  rq out_dir               "${BOX_OUT:-}"
  rq box_lock              "$BOX_LOCK"
  rq gpu_lock              "$GPU_LOCK"
  rq qwen_proc_pattern     "$QWEN_PATTERN"
  rq mode                  "observe"
  rq env_watch             "$ENV_WATCH"
} )"
REQ_B64="$(printf '%s' "$REQ" | b64)"

mkdir -p "$OUT" || die_usage "cannot create --out dir: $OUT"
OBS="$OUT/window-probe.record"
PROBE_ERR="$OUT/window-probe.stderr"

# run_probe <req-b64> <record-out> — one transport round trip, either motion.
# ssh flattens argv into ONE string the remote login shell re-parses, so a positional arg is not
# injection-safe on its own — hence the base64 envelope (the trigger-manual-test.sh:20-29
# convention). The probe script itself is piped in on stdin, so nothing is left on the box.
run_probe() {
  local reqb64="$1" out="$2" rc
  if [ "$DRIVER" = "local" ]; then
    bash "$PROBE" "$reqb64" >"$out" 2>"$PROBE_ERR"; rc=$?
  else
    ssh "$BOX" bash -s -- "$reqb64" <"$PROBE" >"$out" 2>"$PROBE_ERR"; rc=$?
  fi
  if [ "$rc" -ne 0 ] || ! grep -q '^probe\.ok=' "$out"; then return 1; fi
  return 0
}
# obs_from <record> <key> — decode one observation. An absent key yields "".
obs_from() {
  local f="$1" k="$2" line v=""
  while IFS= read -r line; do
    case "$line" in
      "$k="*) v="${line#"$k"=}" ;;
    esac
  done < "$f"
  [ -n "$v" ] && unb64 "$v"
}
obs() { obs_from "$OBS" "$1"; }
# C-2: a record that was never written is not an error worth 7 ENOENT lines on a legitimate
# FAIL — the smoke phase is skipped whenever phase 1/2 failed, so $WIN legitimately may not exist.
obs_win() { [ -f "$WIN" ] && obs_from "$WIN" "$1"; }

# release_request — the envelope for the release motion, built from the same pins so the
# release can never target a different lock than the one the gate took.
release_request_b64() {
  printf '%s' "$( {
    rq mode              "release"
    rq session_lock      "$BOX_LOCK"
    rq window_tag        "$WINDOW_TAG"
    rq qwen_service      "$QWEN_SERVICE"
    rq qwen_proc_pattern "$QWEN_PATTERN"
    rq qwen_reload_tries "$(pin WP_QWEN_RELOAD_TRIES)"
    rq qwen_health_url   "$(pin WP_QWEN_HEALTH_URL)"
  } )" | b64
}

# ------------------------------------------------------------ --release mode --
# Run after the window: reload the serving model, verify it is back, release the lock.
if [ "$DO_RELEASE" = "1" ]; then
  printf '== window-preflight --release: reloading qwen and releasing the box lock ==\n'
  REL="$OUT/window-release.record"
  if ! run_probe "$(release_request_b64)" "$REL"; then
    printf 'window-preflight: TRANSPORT ERROR during release — the box may still be LOCKED and UNLOADED.\n' >&2
    printf '  Recover with: %s --pins %s --release\n' "$0" "$PINS_FILE" >&2
    sed 's/^/    /' "$PROBE_ERR" >&2
    exit $E_TRANSPORT
  fi
  RV="$(obs_from "$REL" lock.release_verdict)"
  command -v jq >/dev/null 2>&1 && jq -n \
    --arg schema "window-release/v1" --arg verdict "$RV" --arg tag "$WINDOW_TAG" \
    --arg lock "$BOX_LOCK" --arg held_tag "$(obs_from "$REL" lock.held_tag)" \
    --arg released_utc "$(obs_from "$REL" lock.released_utc)" \
    --arg box_utc "$(obs_from "$REL" box.timestamp_utc)" \
    --arg reload_rc "$(obs_from "$REL" qwen.reload_rc)" \
    --arg reloaded "$(obs_from "$REL" qwen.reloaded)" \
    --arg health "$(obs_from "$REL" qwen.health_out)" \
    '{schema:$schema, verdict:$verdict, window_tag:$tag, session_lock:$lock,
      held_tag:$held_tag, released_utc:$released_utc, box_timestamp_utc:$box_utc,
      qwen:{reload_rc:$reload_rc, reloaded:$reloaded, health:$health}}' \
    > "$OUT/window-release.json"
  RELOADED="$(obs_from "$REL" qwen.reloaded)"; RELOAD_RC="$(obs_from "$REL" qwen.reload_rc)"
  case "$RV" in
    released)
      # A-2: the lock coming off is NOT the whole job. If the serving model did not come back,
      # the box has just been handed to the next session quietly not serving — and the evidence
      # for that is sealed in the very record this branch used to ignore.
      if [ "$RELOADED" = "declared-none" ]; then
        printf '  lock released at %s (no qwen service pinned)\n' "$(obs_from "$REL" lock.released_utc)"
        printf '== window-preflight --release: OK ==\n'; exit $E_PASS
      fi
      case "$RELOADED" in
        no-service-file)
          printf '  BROKEN PIN: the pinned qwen service file is NOT on the box, so nothing was\n' >&2
          printf '  reloaded. The lock was released, so the box is FREE but NOT SERVING.\n' >&2
          printf '  Pinned path: %s\n' "$(obs_from "$REL" qwen.service_path)" >&2
          printf '  This is a broken pin, NOT the declared `none` waiver — fix WP_QWEN_SERVICE\n' >&2
          printf '  or declare `none` deliberately, then reload the box.\n' >&2
          printf '== window-preflight --release: FAILED (broken qwen-service pin, serving DOWN) ==\n' >&2
          exit $E_QWEN ;;
        service-missing-functions)
          printf '  BROKEN PIN: the pinned qwen service file exists but defines no qwen_reload,\n' >&2
          printf '  so nothing was reloaded. The box is FREE but NOT SERVING.\n' >&2
          printf '== window-preflight --release: FAILED (qwen service defines no reload, serving DOWN) ==\n' >&2
          exit $E_QWEN ;;
      esac
      if [ "$RELOADED" != "1" ]; then
        printf '  SERVING MODEL DID NOT COME BACK (reload_rc=%s, reloaded=%s) — the lock was\n' \
          "$RELOAD_RC" "$RELOADED" >&2
        printf '  released, so the box is FREE but NOT SERVING. Reload it before anyone relies on it.\n' >&2
        printf '  qwen_reload output: %s\n' "$(obs_from "$REL" qwen.reload_out | tr '\n' ';')" >&2
        printf '== window-preflight --release: FAILED (lock released, serving DOWN) ==\n' >&2
        exit $E_QWEN
      fi
      printf '  serving model reloaded and verified resident; lock released at %s\n' \
        "$(obs_from "$REL" lock.released_utc)"
      printf '== window-preflight --release: OK ==\n'; exit $E_PASS ;;
    not-held)
      printf '  the box lock was not held — nothing to release.\n'; exit $E_PASS ;;
    not-ours)
      printf '  REFUSED: the lock is held by tag %s, not %s. Left untouched.\n' \
        "$(obs_from "$REL" lock.held_tag)" "$WINDOW_TAG" >&2; exit $E_BOX ;;
    *)
      printf '  release FAILED (%s) — the box may still be locked.\n' "$RV" >&2; exit $E_BOX ;;
  esac
fi

printf '== window-preflight: probing (driver=%s%s) ==\n' "$DRIVER" "${BOX:+, box=$BOX}"
if ! run_probe "$REQ_B64" "$OBS"; then
  printf 'window-preflight: TRANSPORT ERROR — the probe did not run to completion.\n' >&2
  printf '  probe stderr:\n' >&2; sed 's/^/    /' "$PROBE_ERR" >&2
  exit $E_TRANSPORT
fi

# ------------------------------------------------------------- assertions ----
# Every check appends one row to the verdict table; the table IS the attestation's item list,
# so nothing can be asserted without being recorded, and nothing recorded without being judged.
# Rows are written straight out as NDJSON via jq --arg, which keeps newlines, tabs and quotes
# in a dirty-file list or a stderr tail intact without a hand-rolled escaping scheme.
command -v jq >/dev/null 2>&1 || { printf 'window-preflight: jq is required\n' >&2; exit "$E_TRANSPORT"; }
ROWS="$OUT/window-provenance.items.ndjson"; : > "$ROWS"
N_FAIL=0; WORST=0
row() { # <phase> <id> <expected> <observed> <verdict> <diagnostic>
  jq -nc --arg p "$1" --arg i "$2" --arg e "$3" --arg o "$4" --arg v "$5" --arg d "$6" \
    '{phase:$p, id:$i, expected:$e, observed:$o, verdict:$v, diagnostic:$d}' >> "$ROWS"
  if [ "$5" = "PASS" ] || [ "$5" = "NOTE" ]; then printf '  %-7s %-34s %s\n' "$5" "$2" "$4"
  else printf '  %-7s %-34s %s\n' "$5" "$2" "$6"; fi
}
fail_with() { # <code> — remember the FIRST (most specific) failure class
  N_FAIL=$((N_FAIL + 1)); [ "$WORST" -eq 0 ] && WORST="$1"; return 0
}
# check <phase> <id> <expected> <observed> <exit-class> <diag-prefix>
check() {
  if [ "$3" = "$4" ]; then row "$1" "$2" "$3" "$4" PASS ""
  else row "$1" "$2" "$3" "$4" FAIL "$6: expected '$3', observed '$4'"; fail_with "$5"; fi
}

printf '\n== phase 1: PINS (the tree seam) ==\n'

# --- git checkouts: present, a repo, at the pinned SHA, and CLEAN -------------
# A dirty tree is a hard fail, not a warning: `git rev-parse HEAD` is a claim about committed
# bytes, and an uncommitted edit means the SHA in the attestation does not describe what ran.
check_tree() { # <role> <label> <expected_sha> <bundle_pin>
  local role="$1" label="$2" exp="$3" bpin="$4" ph="pins"
  if [ "$(obs "$role.path_exists")" != "1" ]; then
    row "$ph" "$label.present" "a checkout at $(obs "$role.path")" "ABSENT" FAIL \
      "the $label checkout is not on the box at all — this is the Proof A failure verbatim; run --provision"
    fail_with $E_MISSING; return 0
  fi
  row "$ph" "$label.present" "present" "present" PASS ""
  if [ "$(obs "$role.is_repo")" != "1" ]; then
    row "$ph" "$label.is_repo" "a git repository" "not a git repository" FAIL \
      "$(obs "$role.path") exists but git does not recognise it"
    fail_with $E_MISSING; return 0
  fi
  check "$ph" "$label.head" "$exp" "$(obs "$role.head")" $E_FAIL \
    "$label checkout is at the wrong commit"
  # M6: `assume-unchanged` / `skip-worktree` make git REPORT a modified file as clean, so
  # porcelain alone is not proof that HEAD describes the working tree.
  if [ -n "$(obs "$role.hidden_flags")" ]; then
    row "$ph" "$label.clean" "clean, with no suppressed files" \
      "files flagged assume-unchanged/skip-worktree" FAIL \
      "$label has index flags that HIDE modifications from status --porcelain, so a clean report proves nothing: $(obs "$role.hidden_flags" | tr '\n' ';')"
    fail_with $E_FAIL
  elif [ "$(obs "$role.dirty")" = "0" ]; then
    row "$ph" "$label.clean" "clean" "clean" PASS ""
  else
    row "$ph" "$label.clean" "clean" "dirty" FAIL \
      "$label working tree is dirty, so its HEAD sha does not describe what will run: $(obs "$role.dirty_files" | tr '\n' ';')"
    fail_with $E_FAIL
  fi
  # --- the bundle rule ---
  # The box has no GitHub credentials, so shipping a tree as a git bundle is a LEGAL path — but
  # only when the bundle's own content hash is pinned and recorded. An unpinned bundle is an
  # unprovenanced tree wearing a commit sha, and the gate refuses it outright.
  local marker origin okind mb mcommit bpresent bsha bheads
  marker="$(obs "$role.bundle_marker")"; origin="$(obs "$role.origin_url")"
  okind="$(obs "$role.origin_kind")"
  if [ -n "$marker" ]; then
    mb="$(printf '%s\n' "$marker" | sed -n 's/^bundle_sha256=//p' | head -1)"
    mcommit="$(printf '%s\n' "$marker" | sed -n 's/^commit=//p' | head -1)"
    if [ -z "$bpin" ]; then
      row "$ph" "$label.bundle" "a pinned bundle sha256" "bundle-shipped, UNPINNED" REFUSED \
        "$label was shipped by bundle (sha256=$mb) but no bundle-sha256 pin was supplied — refusing an unprovenanced tree"
      fail_with $E_REFUSED
    elif [ "$mb" != "$bpin" ]; then
      row "$ph" "$label.bundle" "$bpin" "$mb" REFUSED \
        "$label bundle hash does not match its pin: expected '$bpin', observed '$mb'"
      fail_with $E_REFUSED
    elif [ -n "$mcommit" ] && [ "$mcommit" != "$exp" ]; then
      # The marker is a writable file. Even taken at face value it must be SELF-CONSISTENT with
      # the checkout it sits in; a marker claiming a different commit than HEAD is incoherent.
      row "$ph" "$label.bundle" "commit=$exp" "commit=$mcommit" REFUSED \
        "$label bundle record names a different commit than the checkout's HEAD"
      fail_with $E_REFUSED
    else
      # M4: how strong is this? The marker is a file our own tooling wrote and anyone can edit
      # — self-attestation, not proof. When the BUNDLE ITSELF is still on the box we re-derive
      # the claim from its bytes (re-digest + `bundle list-heads` must carry the pinned commit)
      # and grade VERIFIED. Otherwise the row is graded CLAIMED — recorded, visibly weaker than
      # PASS, and never dressed up as a verification we did not perform.
      bpresent="$(obs "$role.bundle_file_present")"
      bsha="$(obs "$role.bundle_file_sha256")"
      bheads="$(obs "$role.bundle_heads")"
      if [ "$bpresent" = "1" ] && [ "$bsha" = "$bpin" ] \
         && printf '%s' "$bheads" | grep -qF -- "$exp"; then
        row "$ph" "$label.bundle" "$bpin (verified from the bundle)" "$bpin — bundle re-digested, commit $exp is one of its heads" PASS ""
      elif [ "$bpresent" = "1" ] && [ "$bsha" != "$bpin" ]; then
        row "$ph" "$label.bundle" "$bpin" "bundle file on box digests to $bsha" REFUSED \
          "$label bundle file is still on the box and does NOT match the pin"
        fail_with $E_REFUSED
      elif [ "$bpresent" = "1" ]; then
        # M-b: the bundle IS here, its bytes DO match the pin, and the pinned commit is NOT among
        # its heads. That is the marker being contradicted by the very bytes it claims to
        # describe — a strictly stronger signal than "could not check", and it was falling into
        # the CLAIMED branch wearing the text "bundle file no longer on the box", which is
        # simply false when the file is sitting right there. Flattening a contradiction into a
        # benign cannot-re-verify is how a swapped-content bundle passes review.
        row "$ph" "$label.bundle" "$bpin" \
          "bundle re-digested and MATCHES the pin, but commit $exp is NOT among its heads" REFUSED \
          "$label bundle's own bytes contradict its provenance marker: the file matches its pinned sha256, so this is the bundle that was pinned, and it does not contain the commit the tree claims to have come from"
        fail_with $E_REFUSED
      else
        row "$ph" "$label.bundle" "$bpin" "$mb (CLAIMED — bundle file no longer on the box)" CLAIMED \
          "provenance rests on the recorded marker alone; the bundle was not available to re-verify from its own bytes"
      fi
    fi
  else
    # B3: `git clone <file>.bundle` sets origin to the BUNDLE'S PATH, which is non-empty — so an
    # emptiness test reads an unprovenanced bundle tree as "cloned from a remote". That is the
    # Proof A path verbatim. Only a real remote URL counts as provenance.
    case "$okind" in
      remote-url|scp-like-remote)
        row "$ph" "$label.bundle" "cloned from a remote" "origin=$origin" PASS "" ;;
      none)
        row "$ph" "$label.bundle" "an origin remote, or a recorded bundle" "no origin remote, no bundle record" REFUSED \
          "$label has no origin remote and no bundle record — its provenance cannot be claimed from evidence"
        fail_with $E_REFUSED ;;
      *)
        row "$ph" "$label.bundle" "an origin remote, or a recorded bundle" "origin is a $okind ($origin), with no bundle record" REFUSED \
          "$label's origin is not a remote URL — a bundle path or local clone is not provenance, and no bundle record accompanies it"
        fail_with $E_REFUSED ;;
    esac
  fi
}
check_tree bench  BENCH  "$BENCH_SHA"  "$BENCH_BUNDLE_SHA"
check_tree engine ENGINE "$ENGINE_SHA" "$ENGINE_BUNDLE_SHA"

# --- weights: the sha256+bytes+count triple ----------------------------------
if [ "$(obs weights.exists)" != "1" ]; then
  row pins weights.present "a weights dir at $WEIGHTS_PATH" "ABSENT" FAIL "weights directory is not on the box"
  fail_with $E_MISSING
elif [ -n "$(obs weights.error)" ]; then
  # benchd errors on a broken symlink (File::open fails inside the streaming digest), so a
  # gate that silently skipped it would clear a tree the run then refuses.
  row pins weights.readable "every file readable" "$(obs weights.error): $(obs weights.error_detail | tr '\n' ' ')" FAIL \
    "the weights tree cannot be digested the way benchd digests it"
  fail_with $E_FAIL
else
  check pins weights.sha256     "$WEIGHTS_SHA" "$(obs weights.sha256)"     $E_FAIL "weights tree digest mismatch"
  check pins weights.file_count "$WEIGHTS_FC"  "$(obs weights.file_count)" $E_FAIL "weights file count mismatch"
  check pins weights.byte_count "$WEIGHTS_BC"  "$(obs weights.byte_count)" $E_FAIL "weights byte count mismatch"
fi

# --- goldens: sha256 AND bytes, then the loader's own verdict ----------------
gi=0
while IFS= read -r gpin; do
  [ -n "$gpin" ] || continue
  gi=$((gi + 1))
  gs="${gpin%% *}"; gb="${gpin##* }"
  if [ "$(obs "golden.$gi.exists")" != "1" ]; then
    row pins "golden$gi.present" "$(obs "golden.$gi.path")" "ABSENT" FAIL "golden $gi is not on the box"
    fail_with $E_MISSING; continue
  fi
  check pins "golden$gi.bytes"  "$gb" "$(obs "golden.$gi.bytes")"  $E_GOLDEN "golden $gi byte count mismatch"
  check pins "golden$gi.sha256" "$gs" "$(obs "golden.$gi.sha256")" $E_GOLDEN "golden $gi sha256 mismatch"
  vrc="$(obs "golden.$gi.validate_rc")"
  if [ "$vrc" = "0" ]; then
    row basics "golden$gi.loader" "accepted by benchctl validate-golden" "accepted" PASS ""
  elif [ "$vrc" = "skipped" ]; then
    row basics "golden$gi.loader" "accepted by benchctl validate-golden" "SKIPPED (benchd binary unusable)" FAIL \
      "could not ask the loader — benchd is missing or not executable, so the golden is only shasum-verified"
    fail_with $E_MISSING
  else
    row basics "golden$gi.loader" "accepted by benchctl validate-golden" "rejected (rc=$vrc)" FAIL \
      "the loader the run will use REJECTS golden $gi: $(obs "golden.$gi.validate_out" | tr '\n' ';')"
    fail_with $E_GOLDEN
  fi
done <<EOF
$GOLDEN_PINS
EOF

# Pool tapes: the same sha256+bytes pin, then the required-key signature rather than the golden
# loader (which rejects every tape by construction).
ti=0
while IFS= read -r tpin; do
  [ -n "$tpin" ] || continue
  ti=$((ti + 1))
  ts="${tpin%% *}"; tb="${tpin##* }"
  if [ "$(obs "tape.$ti.exists")" != "1" ]; then
    row pins "tape$ti.present" "$(obs "tape.$ti.path")" "ABSENT" FAIL "pool tape $ti is not on the box"
    fail_with $E_MISSING; continue
  fi
  check pins "tape$ti.bytes"  "$tb" "$(obs "tape.$ti.bytes")"  $E_GOLDEN "pool tape $ti byte count mismatch"
  check pins "tape$ti.sha256" "$ts" "$(obs "tape.$ti.sha256")" $E_GOLDEN "pool tape $ti sha256 mismatch"
  _missing=""
  for _k in seed_tokens reference_seed_token rows; do
    [ "$(obs "tape.$ti.sig_$_k")" = "1" ] || _missing="$_missing $_k"
  done
  if [ -z "$_missing" ]; then
    row pins "tape$ti.signature" "a timed-prompt tape (seed_tokens, reference_seed_token, rows)" "signature present" PASS ""
  else
    row pins "tape$ti.signature" "a timed-prompt tape (seed_tokens, reference_seed_token, rows)" \
      "missing:$_missing" FAIL \
      "pool tape $ti does not carry the required-key signature measure-job routes on, so the legs would not recognise it as a tape"
    fail_with $E_GOLDEN
  fi
done <<EOF
$TAPE_PINS
EOF

if [ -n "$CONTRACT_PATH" ]; then
  if [ "$(obs contract.exists)" != "1" ]; then
    row pins contract.present "$CONTRACT_PATH" "ABSENT" FAIL "track contract is not on the box"
    fail_with $E_MISSING
  else
    check pins contract.sha256 "$CONTRACT_SHA" "$(obs contract.sha256)" $E_FAIL "track contract sha256 mismatch"
    # LANE 2a — SOURCE the hidden correctness golden's pin FROM THE FIXTURE. The contract is now
    # sha256-verified above, so its declared `hidden_correctness_golden` sha256+bytes is the
    # review-gated authority for the correctness oracle's identity — NOT an operator WP_GOLDEN line
    # (machine-state), and NOT a hardcoded box path. When the fixture declares it, the staged
    # correctness golden (WP_GOLDEN_1, the correctness oracle per the terminology ruling — tapes are
    # WP_POOL_TAPE_*) is PIN-VERIFIED against the FIXTURE sha256+bytes. benchd re-enforces the
    # identical pin on the run via --correctness-golden (fail-closed both directions), so this gate
    # and the run agree on one authority. This SIBLING pin never touches the anti-lottery pool count.
    HCG_SHA="$(obs contract.hcg_sha256)"; HCG_BYTES="$(obs contract.hcg_bytes)"
    if [ -n "$HCG_SHA" ]; then
      row basics contract.hidden_correctness_golden \
        "fixture-pinned sha256+bytes (LANE 2a)" "sourced from --contract" PASS \
        "the correctness golden's identity is the review-gated fixture pin, not an operator pin"
      if [ "$(obs golden.1.exists)" = "1" ]; then
        check pins hidden_correctness_golden.bytes  "$HCG_BYTES" "$(obs golden.1.bytes)"  $E_GOLDEN \
          "the staged correctness golden's byte count does not cite the fixture pin"
        check pins hidden_correctness_golden.sha256 "$HCG_SHA"   "$(obs golden.1.sha256)" $E_GOLDEN \
          "the staged correctness golden's sha256 does not cite the fixture pin"
      else
        row pins hidden_correctness_golden.staged \
          "a correctness golden staged for the fixture pin" "not staged in this window" PASS \
          "no correctness golden staged here; benchd requires --correctness-golden at run time (fail-closed)"
      fi
    fi
  fi
fi

printf '\n== phase 2: BASICS (the box seam) ==\n'

# --- binaries: present, EXECUTABLE, byte-pinned, and asked what they are ------
check_bin() { # <role> <label> <path> <sha_pin> <id_argv> <id_expect>
  local role="$1" label="$2" path="$3" shapin="$4" idargv="$5" idexp="$6" idrc idout
  if [ "$(obs "$role.exists")" != "1" ]; then
    row basics "$label.present" "$path" "ABSENT" FAIL "$label binary is not at its pinned path — nothing will spawn"
    fail_with $E_MISSING; return 0
  fi
  if [ "$(obs "$role.executable")" != "1" ]; then
    row basics "$label.executable" "executable" "not executable" FAIL "$label exists but the execute bit is not set"
    fail_with $E_MISSING; return 0
  fi
  row basics "$label.executable" "executable" "executable" PASS ""
  check basics "$label.sha256" "$shapin" "$(obs "$role.sha256")" $E_FAIL \
    "$label binary bytes do not match the pin — a path is not a seal, and this is the binary the run will spawn"
  # Identity RUN, not a stat. A binary at the right path with the right digest is still only a
  # file until it has been executed and asked what it is.
  idrc="$(obs "$role.identity_rc")"; idout="$(obs "$role.identity_out")"
  if [ "$idargv" = "none" ]; then
    row basics "$label.identity" "DECLARED-NONE" "DECLARED-NONE (no identity flag)" PASS ""
  elif [ "$idrc" != "0" ]; then
    row basics "$label.identity" "exit 0 from '$idargv'" "exit $idrc" FAIL \
      "$label did not survive being run with its identity flag: $(printf '%s' "$idout" | tr '\n' ';')"
    fail_with $E_MISSING
  elif [ -n "$idexp" ] && ! printf '%s' "$idout" | grep -qF -- "$idexp"; then
    row basics "$label.identity" "output containing '$idexp'" "$(printf '%s' "$idout" | tr '\n' ';')" FAIL \
      "$label ran but does not report the pinned identity"
    fail_with $E_FAIL
  else
    row basics "$label.identity" "${idexp:-exit 0}" "ok" PASS ""
  fi
}
check_bin benchdbin BENCHDBIN "$BENCHD_BIN" "$BENCHD_BIN_SHA" "$BENCHD_ID_ARGV" "$BENCHD_ID_EXPECT"
check_bin enginebin ENGINEBIN "$ENGINE_BIN" "$ENGINE_BIN_SHA" "$ENGINE_ID_ARGV" "$ENGINE_ID_EXPECT"
# LANE 2b (#148) — seal the swift worker ONLY when the window pins it (direct-swift producer). An
# unpinned worker means that window does not use it; sealing an absent binary would fail closed.
[ -z "$SWIFT_WORKER_BIN" ] || check_bin swiftworkerbin SWIFTWORKERBIN "$SWIFT_WORKER_BIN" "$SWIFT_WORKER_BIN_SHA" "$SWIFT_WORKER_ID_ARGV" "$SWIFT_WORKER_ID_EXPECT"

# --- THE box lock: acquirable, or reapable with proof ------------------------
# This is a pre-check, not the acquisition. The gate takes the lock in phase 3, atomically, so
# the answer here can go stale between the two — which is exactly why the acquisition is an
# atomic `mkdir` and not a check-then-act on this observation.
# Classification only — the reap itself happens at acquire time, in phase 3, where it can be
# sealed. An answer here can go stale between the two, which is why the acquisition is an atomic
# `mkdir` and not a check-then-act on this observation.
REAP_REFUSED=""; REAP_REFUSED_DETAIL=""
if [ "$(obs boxlock.present)" != "1" ]; then
  row basics boxlock "acquirable" "acquirable (no holder)" PASS ""
elif [ "$(obs boxlock.holder_alive)" = "1" ]; then
  REAP_REFUSED="holder-alive"; REAP_REFUSED_DETAIL="pid $(obs boxlock.pid) is still running"
  row basics boxlock "acquirable" "HELD by live pid $(obs boxlock.pid)" FAIL \
    "the box lock is held by a running process — another window is in flight"
  fail_with $E_BOX
elif [ "$(obs boxlock.pid_numeric)" != "1" ]; then
  REAP_REFUSED="unverifiable-holder"; REAP_REFUSED_DETAIL="pid file absent or non-numeric (pid='$(obs boxlock.pid)')"
  row basics boxlock "acquirable" "HELD by an UNVERIFIABLE holder (pid='$(obs boxlock.pid)')" FAIL \
    "the box lock at $BOX_LOCK has no readable numeric pid, so its holder cannot be proved dead. An unverifiable holder is never reaped — clear it deliberately, then re-run."
  fail_with $E_BOX
elif [ -z "$(obs boxlock.age_seconds)" ]; then
  REAP_REFUSED="unverifiable-age"; REAP_REFUSED_DETAIL="the lock directory's mtime could not be read"
  row basics boxlock "acquirable" "HELD, age unreadable" FAIL \
    "the box lock's mtime could not be read, so its age cannot be proved past the reap threshold"
  fail_with $E_BOX
elif [ "$(obs boxlock.age_seconds)" -lt "$LOCK_REAP_AGE_S" ]; then
  REAP_REFUSED="too-fresh"; REAP_REFUSED_DETAIL="pid $(obs boxlock.pid) is gone but the lock is only $(obs boxlock.age_seconds)s old (threshold ${LOCK_REAP_AGE_S}s)"
  row basics boxlock "acquirable" "STALE but FRESH (pid $(obs boxlock.pid) gone, age $(obs boxlock.age_seconds)s < ${LOCK_REAP_AGE_S}s)" FAIL \
    "the holder is gone but the lock is younger than the reap threshold — a holder that died seconds ago may be mid-restart, so this is refused rather than reaped"
  fail_with $E_BOX
else
  # Provably dead and old enough. Phase 3 will reap it and seal the evidence.
  row basics boxlock "acquirable" "REAPABLE (pid $(obs boxlock.pid) provably gone, age $(obs boxlock.age_seconds)s >= ${LOCK_REAP_AGE_S}s)" NOTE ""
fi
# B4: the flock dialect used to be reported and never judged, which is how the old head could
# pass while a live run held the box. It cannot be probed by TAKING it — that is acquisition,
# and the fd would die with the probe's shell anyway — but a flock holder must hold the file
# OPEN, so open-fd inspection is sound in the direction that matters. It is deliberately
# CONSERVATIVE: an open fd without an actual flock reads as held and refuses, which is the
# fail-closed direction. Mere EXISTENCE of the file is never judged; it survives every window
# ever run on the box.
GPUHELD="$(obs gpulock.held)"
if [ "$(obs gpulock.present)" != "1" ]; then
  row basics gpulock "not held" "file absent" PASS ""
elif [ "$GPUHELD" = "1" ]; then
  row basics gpulock "not held" "HELD — open by pid(s) $(obs gpulock.holder_pids)" FAIL \
    "the gpu-exclusive flock at $GPU_LOCK is open in another process, so a window driver is running. The gate refuses rather than contend."
  fail_with $E_BOX
elif [ "$GPUHELD" = "0" ]; then
  row basics gpulock "not held" "not held (no process has it open)" PASS ""
else
  row basics gpulock "not held" "UNDETERMINED (lsof unavailable on the box)" FAIL \
    "the flock's holder could not be determined, so freedom from contention cannot be established. Install lsof on the box, or declare the risk explicitly."
  fail_with $E_BOX
fi

# --- disk, box quiet, serving state ------------------------------------------
FREE_B="$(obs disk.free_bytes)"
if [ -z "$FREE_B" ]; then
  row basics disk.free "> ${MIN_FREE_GB} GB" "unreadable" FAIL "could not read free space on $(obs disk.path)"
  fail_with $E_MISSING
else
  FREE_GB=$((FREE_B / 1024 / 1024 / 1024))
  if [ "$FREE_GB" -ge "$MIN_FREE_GB" ]; then
    row basics disk.free ">= ${MIN_FREE_GB} GB" "${FREE_GB} GB" PASS ""
  else
    row basics disk.free ">= ${MIN_FREE_GB} GB" "${FREE_GB} GB" FAIL \
      "not enough free space on $(obs disk.path) to seal this window's artifacts"
    fail_with $E_BOX
  fi
fi

TM="$(obs quiet.timemachine)"
if [ "$REQ_TM_IDLE" != "1" ]; then
  row basics quiet.timemachine "DECLARED-WAIVED" "not asserted (WP_REQUIRE_TIMEMACHINE_IDLE=0)" NOTE ""
elif printf '%s' "$TM" | grep -q 'Running = 1'; then
  row basics quiet.timemachine "not running" "RUNNING" FAIL "Time Machine is backing up — timings will be contaminated"
  fail_with $E_BOX
else
  row basics quiet.timemachine "not running" "not running" PASS ""
fi
LOAD="$(obs quiet.loadavg_1m)"
if [ -n "$LOAD" ] && awk -v l="$LOAD" -v m="$MAX_LOADAVG" 'BEGIN{exit !(l+0 < m+0)}'; then
  row basics quiet.loadavg "< $MAX_LOADAVG" "$LOAD" PASS ""
else
  row basics quiet.loadavg "< $MAX_LOADAVG" "${LOAD:-unreadable}" FAIL "the box is not idle enough to time anything"
  fail_with $E_BOX
fi
STRAY="$(obs quiet.stray_swift)$(obs quiet.stray_engine)"
if [ "$REQ_NO_STRAY" != "1" ]; then
  row basics quiet.stray "DECLARED-WAIVED" "not asserted (WP_REQUIRE_NO_STRAY=0)" NOTE ""
elif [ -n "$(printf '%s' "$STRAY" | tr -d ' ')" ]; then
  row basics quiet.stray "no stray model processes" "pids: $STRAY" FAIL "a model process is already resident and will contend for the GPU"
  fail_with $E_BOX
else
  row basics quiet.stray "no stray model processes" "none" PASS ""
fi

# M1/M3 — the ENV seam. Each WP_ENV_<NAME> pin is an expected VALUE, or the literal `unset`.
# `unset` and empty are DIFFERENT observations: `FOO=` and no FOO at all mean different things
# to a spawn, and the distinction is preserved here rather than flattened.
for _n in $ENV_WATCH; do
  _want="$(pin "WP_ENV_$_n")"
  _set="$(obs "env.$_n.set")"; _got="$(obs "env.$_n")"
  if [ "$_want" = "unset" ]; then
    if [ "$_set" = "0" ]; then row basics "env.$_n" "unset" "unset" PASS ""
    else
      row basics "env.$_n" "unset" "set to '$_got'" FAIL \
        "$_n is exported on the box; it changes what gets spawned or how, and the window did not declare it"
      fail_with $E_FAIL
    fi
  elif [ "$_set" != "1" ]; then
    row basics "env.$_n" "$_want" "unset" FAIL "$_n is required by this window but is not exported on the box"
    fail_with $E_FAIL
  else
    check basics "env.$_n" "$_want" "$_got" $E_FAIL "$_n does not hold its pinned value"
  fi
done
row basics env.namespace "sealed" "$(obs env.namespace_count) MLXFAST_*/QMTP_* var(s) observed and sealed" NOTE ""

if [ "$QWEN_EXPECT" = "none" ]; then
  row basics qwen.state "DECLARED-NONE" "not asserted" PASS ""
else
  check basics qwen.state "$QWEN_EXPECT" "$(obs qwen.state)" $E_QWEN \
    "the serving model is not in the state this window assumes (the gate reports it; changing serving state is the window driver's job, under its own trap discipline)"
fi

# ------------------------------- phase 3: LOCK -> UNLOAD -> SMOKE (and HOLD) --
# Everything above was lock-free. From here the box is single-flighted BY THIS GATE.
SMOKE_VERDICT="SKIPPED"; LOCK_STATE="not-taken"
LOCK_ACQ_UTC=""; LOCK_HOLDER=""; WIN="$OUT/window-phase.record"
UNWIND_VERDICT=""; LOCK_RELEASED_UTC=""

# Trap discipline (the run-paired-window.sh:226-249 pattern). If the gate holds — or MIGHT hold
# — the lock and is not handing off, it must put the box back: reload the serving model,
# release. HUP is trapped too, so an ssh drop unwinds instead of leaving the box locked and
# unloaded, which is the worst state to leave a shared box in.
#
# A-1: the state that arms this must be set BEFORE the window-phase probe is dispatched, not
# after it returns. bash defers a trap until the running foreground command completes, so a
# SIGTERM during acquire/unload/smoke fires unwind only afterwards — and with the old ordering
# LOCK_STATE was still "not-taken" at that point, so unwind no-opped and the lock was stranded
# with qwen down. `maybe-held` covers the whole dispatch window; the release is tag-scoped, so
# attempting it when we do NOT in fact hold the lock is a verified-safe no-op ("not-ours").
_HANDOFF=0; _UNWOUND=0
unwind() {
  [ "$_UNWOUND" = "1" ] && return 0
  case "$LOCK_STATE" in held|maybe-held) ;; *) return 0 ;; esac
  [ "$_HANDOFF" = "1" ] && return 0
  _UNWOUND=1
  printf '\n== unwinding: reloading qwen and releasing the box lock ==\n' >&2
  if run_probe "$(release_request_b64)" "$OUT/window-unwind.record"; then
    UNWIND_VERDICT="$(obs_from "$OUT/window-unwind.record" lock.release_verdict)"
    LOCK_RELEASED_UTC="$(obs_from "$OUT/window-unwind.record" lock.released_utc)"
    printf '  %s (qwen reloaded=%s)\n' "$UNWIND_VERDICT" \
      "$(obs_from "$OUT/window-unwind.record" qwen.reloaded)" >&2
    case "$UNWIND_VERDICT" in
      released)  LOCK_STATE="released-on-failure" ;;
      not-held)  LOCK_STATE="not-taken" ;;
      not-ours)  LOCK_STATE="not-ours" ;;
      *)         LOCK_STATE="unwind-failed" ;;
    esac
    _unwind_reloaded="$(obs_from "$OUT/window-unwind.record" qwen.reloaded)"
    # Anything that is not a positive confirmation is a warning. Testing for "0" alone missed
    # the EMPTY case — which is exactly the box-side-signal path, where the probe released the
    # lock itself and this release therefore reports `not-held` and never reaches its reload.
    case "$_unwind_reloaded" in
      1|declared-none) ;;
      *) printf '  WARNING: the serving model is NOT CONFIRMED BACK (qwen.reloaded=%s).\n' \
           "${_unwind_reloaded:-<absent>}" >&2
         printf '           If the box side released on a signal it reloads on its way out; verify.\n' >&2 ;;
    esac
  else
    UNWIND_VERDICT="transport-error"; LOCK_STATE="unwind-failed"
    printf '  UNWIND FAILED (transport) — the box may still be LOCKED and UNLOADED.\n' >&2
    printf '  Recover with: %s --pins %s --release\n' "$0" "$PINS_FILE" >&2
  fi
  # M7: an unwind record on EVERY trap path. The attestation's own lock block is written by the
  # main flow, which a signal may never reach — so the unwind writes its own evidence, or the
  # "two files prove single-flight" claim is false exactly when it matters most.
  if command -v jq >/dev/null 2>&1; then
    jq -n --arg v "${UNWIND_VERDICT:-unknown}" --arg tag "$WINDOW_TAG" --arg lock "$BOX_LOCK" \
          --arg state "$LOCK_STATE" --arg rel "$LOCK_RELEASED_UTC" \
          --arg reloaded "$(obs_from "$OUT/window-unwind.record" qwen.reloaded)" \
          --arg utc "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
      '{schema:"window-unwind/v1", trigger:"gate-failure-or-signal", release_verdict:$v,
        window_tag:$tag, box_lock:$lock, resulting_state:$state, released_utc:$rel,
        qwen_reloaded:$reloaded, unwound_at_utc:$utc}' > "$OUT/window-unwind.json" 2>/dev/null
  fi
}
trap 'unwind' EXIT
trap 'unwind; exit 130' INT
trap 'unwind; exit 143' TERM
trap 'unwind; exit 129' HUP

printf '\n== phase 3: LOCK -> UNLOAD -> SMOKE (the spawn seam) ==\n'
if [ "$N_FAIL" -gt 0 ]; then
  # Fail-closed ordering, and the reason the lock is taken HERE and not earlier: a smoke leg run
  # against a tree that failed the pin phase would prove something about the WRONG tree, and its
  # PASS would be worse than no result at all. It would also have spent the lock for nothing.
  row smoke smoke.recipe "$SMOKE_RECIPE" "NOT RUN (phase 1/2 failed; no lock taken)" NOTE ""
  SMOKE_VERDICT="NOT-RUN"
else
  # M8: ACQUISITION IS NOT WAIVABLE. The directive is "the gate acquires the lock before its
  # smoke leg and holds it through the window" — only the SMOKE LEG is waivable. A `none`
  # recipe still takes and holds the lock; it just runs no smoke argv, and the waiver is
  # recorded. Otherwise `--no-smoke` would hand the window an unlocked box.
  [ "$SMOKE_RECIPE" = "none" ] && SMOKE_ARGV=""
  LOCK_STATE="maybe-held"          # A-1: armed BEFORE the dispatch, not after it returns
  WIN_REQ="$( {
    rq mode                "window"
    rq session_lock        "$BOX_LOCK"
    rq window_tag          "$WINDOW_TAG"
    rq lock_reap_age_s     "$LOCK_REAP_AGE_S"
    rq qwen_service        "$QWEN_SERVICE"
    rq qwen_proc_pattern   "$QWEN_PATTERN"
    rq qwen_unload_tries   "$(pin WP_QWEN_UNLOAD_TRIES)"
    rq smoke_argv          "$SMOKE_ARGV"
    rq smoke_timeout_s     "$SMOKE_TIMEOUT"
    rq smoke_worker_stderr "$(pin WP_SMOKE_WORKER_STDERR)"
  } )"
  if ! run_probe "$(printf '%s' "$WIN_REQ" | b64)" "$WIN"; then
    # B2: the transport dropped somewhere between mkdir and readback. We may or may not hold
    # the lock, and we cannot tell from here — so attempt the tag-scoped release unconditionally
    # rather than leaving a box that is LOCKED, UNLOADED and un-attested.
    printf 'window-preflight: TRANSPORT ERROR during the lock/smoke phase.\n' >&2
    sed 's/^/    /' "$PROBE_ERR" >&2
    unwind
    exit $E_TRANSPORT
  fi

  # The reap, if one happened, is its own attested row: what was removed, whose it was, how
  # death was verified, and when. A lock that vanished with no record would be the same
  # unaccountable box-state change the refuse-and-report policy exists to prevent.
  if [ "$(obs_win lock.reaped)" = "1" ]; then
    row smoke lock.reaped "a provably-dead holder" \
      "REAPED prior holder tag='$(obs_win lock.reaped_prior_tag)' pid=$(obs_win lock.reaped_prior_pid) user='$(obs_win lock.reaped_prior_user)' acquired=$(obs_win lock.reaped_prior_acquired_utc) age=$(obs_win lock.reaped_age_seconds)s" NOTE ""
  elif [ -n "$(obs_win lock.reap_refused)" ]; then
    row smoke lock.reaped "a provably-dead holder" \
      "NOT REAPED ($(obs_win lock.reap_refused)): $(obs_win lock.reap_refused_detail)" NOTE ""
  fi

  if [ "$(obs_win lock.acquired)" = "1" ]; then
    LOCK_STATE="held"
    LOCK_ACQ_UTC="$(obs_win lock.acquired_utc)"
    LOCK_HOLDER="$(obs_win lock.holder)"
    row smoke lock.acquired "the box lock" "ACQUIRED at $LOCK_ACQ_UTC (tag $WINDOW_TAG)" PASS ""
  elif [ "$(obs_win lock.create_failed)" = "1" ]; then
    row smoke lock.acquired "the box lock" "COULD NOT BE CREATED" FAIL \
      "mkdir $BOX_LOCK failed for a reason unrelated to contention (missing parent, unwritable, or a plain file at the path): $(obs_win lock.create_error | tr '\n' ';')"
    fail_with $E_MISSING; SMOKE_VERDICT="NO-LOCK"; LOCK_STATE="not-taken"
  else
    row smoke lock.acquired "the box lock" "NOT ACQUIRED" FAIL \
      "another session holds $BOX_LOCK (pid $(obs_win lock.blocking_pid), alive=$(obs_win lock.blocking_alive)): $(obs_win lock.holder | tr '\n' ';')"
    fail_with $E_BOX; SMOKE_VERDICT="NO-LOCK"; LOCK_STATE="not-taken"
  fi

  # --- serving model unloaded, under the lock ---
  if [ "$LOCK_STATE" = "held" ]; then
    QU="$(obs_win qwen.unloaded)"
    if [ "$QU" = "declared-none" ]; then
      row smoke qwen.unload "DECLARED-NONE" "no qwen service pinned" PASS ""
    elif [ "$QU" = "1" ]; then
      row smoke qwen.unload "unloaded and gone" "unloaded (rc=$(obs_win qwen.unload_rc))" PASS ""
    elif [ "$(obs_win qwen.unload_rc)" = "no-service-file" ] \
      || [ "$(obs_win qwen.unload_rc)" = "service-missing-functions" ]; then
      # The mirror of X-2, on the window side. This arm used to fall through to STILL RESIDENT
      # below and report that the serving model had failed to unload — when in fact nothing was
      # resident and nothing was attempted: the probe refused the BROKEN PIN and released the
      # lock. Failing closed was right; saying the wrong thing about why is not. A diagnostic
      # that misnames the cause sends the next operator to hunt a process that was never there.
      _qwhy="the pinned qwen service file is not on the box"
      [ "$(obs_win qwen.unload_rc)" = "service-missing-functions" ] \
        && _qwhy="the pinned qwen service file does not define both qwen_unload and qwen_reload"
      row smoke qwen.unload "unloaded and gone" "NOT ATTEMPTED — broken qwen-service pin" FAIL \
        "$_qwhy, so the window refused before touching the serving model and released the lock it had just taken. This is a BROKEN PIN, not a serving-model fault and not the declared \`none\` waiver: fix WP_QWEN_SERVICE, or declare \`none\` deliberately."
      fail_with $E_QWEN; SMOKE_VERDICT="NO-UNLOAD"
      LOCK_STATE="released-by-probe"
    else
      # rc=0 is not proof; the process still being resident is (official-lib.sh:320-329).
      row smoke qwen.unload "unloaded and gone" "STILL RESIDENT (rc=$(obs_win qwen.unload_rc))" FAIL \
        "the serving model did not actually unload, so the smoke leg would have contended for the GPU. The probe released the lock it had just taken. $(obs_win qwen.unload_out | tr '\n' ';')"
      fail_with $E_QWEN; SMOKE_VERDICT="NO-UNLOAD"
      LOCK_STATE="released-by-probe"
    fi
  fi

  # --- the smoke leg itself ---
  if [ "$LOCK_STATE" = "held" ] && [ "$SMOKE_RECIPE" = "none" ]; then
    # M8: acquisition happened; only the smoke LEG is waived, and the waiver is on the record.
    row smoke smoke.recipe "a declared recipe" "none (DECLARED WAIVER — lock still acquired and held)" NOTE ""
    SMOKE_VERDICT="DECLARED-WAIVED"
  elif [ "$LOCK_STATE" = "held" ]; then
    SRC="$(obs_win smoke.rc)"; SERR="$(obs_win smoke.stderr)"
    SOUT="$(obs_win smoke.stdout)"
    row smoke smoke.recipe "$SMOKE_RECIPE" "$SMOKE_RECIPE" PASS ""
    if [ "$(obs_win smoke.timed_out)" = "1" ]; then
      row smoke smoke.handshake "a hello within ${SMOKE_TIMEOUT}s" "TIMED OUT" FAIL \
        "the smoke leg hung — a worker that never answers the hello is exactly the seam this phase exists to catch"
      fail_with $E_SMOKE; SMOKE_VERDICT="FAIL"
    elif printf '%s' "$SERR" | grep -qi 'closed the stream before returning a response\|hello handshake failed' \
         && [ "$EXPECT_SMOKE_FAIL" != "none" ]; then
      # The declared dependency reproduced. Still a FAIL — the window does not proceed — but
      # labelled, so nobody re-diagnoses a known-open issue as a gate defect.
      row smoke smoke.handshake "hello (id=0, ok, protocol_version=1)" \
        "engine closed the stream before the hello — EXPECTED-FAIL($EXPECT_SMOKE_FAIL)" EXPECTED-FAIL \
        "the declared known failure $EXPECT_SMOKE_FAIL reproduced: the spawn seam is broken exactly as recorded. This is the gate working, not a gate defect — but the window still must not proceed. Worker stderr: $(printf '%s' "$SERR" | grep 'mlxfast-worker:' | tail -20 | tr '\n' ';')"
      fail_with $E_SMOKE; SMOKE_VERDICT="EXPECTED-FAIL($EXPECT_SMOKE_FAIL)"
    elif printf '%s' "$SERR" | grep -qi 'closed the stream before returning a response\|hello handshake failed'; then
      # #134's signature. The engine also exits 1 on an unknown flag BEFORE the hello, which
      # surfaces identically (main.rs:1338-1348) — so the diagnostic names both possibilities
      # rather than guessing between them.
      row smoke smoke.handshake "hello (id=0, ok, protocol_version=1)" "engine closed the stream before the hello" FAIL \
        "spawn seam broken (the #134 signature): either the worker died during load, or it rejected an argv flag before ever writing the hello. Worker stderr: $(printf '%s' "$SERR" | grep 'mlxfast-worker:' | tail -20 | tr '\n' ';')"
      fail_with $E_SMOKE; SMOKE_VERDICT="FAIL"
    elif printf '%s' "$SERR" | grep -qi 'protocol_version'; then
      row smoke smoke.handshake "protocol_version=1" "version mismatch" FAIL \
        "the engine handshook but speaks a different protocol version: $(printf '%s' "$SERR" | tail -5 | tr '\n' ';')"
      fail_with $E_SMOKE; SMOKE_VERDICT="FAIL"
    elif [ "$SRC" != "0" ]; then
      row smoke smoke.handshake "hello (id=0, ok, protocol_version=1)" "reached, but the leg failed (rc=$SRC)" FAIL \
        "the smoke leg spawned and handshook but did not complete its round trip: $(printf '%s' "$SERR" | tail -10 | tr '\n' ';')"
      fail_with $E_SMOKE; SMOKE_VERDICT="FAIL"
    else
      row smoke smoke.handshake "hello (id=0, ok, protocol_version=1)" "ok" PASS ""
      row smoke smoke.roundtrip "at least one real round trip" "ok (rc=0, $(obs_win smoke.elapsed_s)s)" PASS ""
      SMOKE_VERDICT="PASS"
    fi
    # Worker stderr is CAPTURED either way — a green smoke leg that logged warnings is evidence
    # too, and on the sandboxed path benchd drops the drained tail on Drop, so this record may
    # be the only place a failing worker's own words survive.
    WSC="$(printf '%s' "$SERR" | grep -c 'mlxfast-worker:')"
    row smoke smoke.worker_stderr "captured" "${WSC} forwarded line(s), $(printf '%s' "$SOUT" | wc -c | tr -d ' ') stdout bytes" NOTE ""
    # B-7: a worker that outlived the leg holds GPU memory into the NEXT session's window.
    STRAY_AFTER="$(obs_win smoke.stray_after | tr -d ' ')"
    if [ -n "$STRAY_AFTER" ]; then
      row smoke smoke.no_stray "no engine left running" "STRAY pids: $(obs_win smoke.stray_after)" FAIL \
        "the smoke leg left an engine process running; it holds GPU memory and would contaminate the window. Kill it before proceeding."
      fail_with $E_SMOKE
    else
      row smoke smoke.no_stray "no engine left running" "none" PASS ""
    fi
  fi
fi

# M7: if the gate failed while holding the lock, unwind NOW — before the attestation is
# written — so the sealed `lock.state` and `released_utc` describe what actually happened. Left
# to the EXIT trap, the attestation would seal state="held"/released_utc=null on every unwound
# run, which is exactly when the "two files prove single-flight" claim needs to be true.
if [ "$N_FAIL" -gt 0 ] && [ "$LOCK_STATE" = "held" ]; then
  unwind
fi

# ------------------------------------------------------------- attestation ---
# Provenance is CLAIMED FROM EVIDENCE, not assumption: every pin the window depends on appears
# here with what was expected, what was actually observed, and the verdict — plus the box's own
# timestamp and the sha256 of the three scripts that produced the record, so the gate cannot
# later be edited into having checked something it did not.
#
# This is a SEPARATE SEALED FILE. It does not touch benchmark-integrity.*.json or the #123
# runner-identity roster: that sidecar answers "what binary ran"; this answers "what TREE ran",
# and the two are pinned independently on purpose.
sha_of() { shasum -a 256 "$1" | awk '{print $1}'; }

ATT="$OUT/window-provenance.json"
ITEMS="$(jq -s '.' "$ROWS")"

if [ "$N_FAIL" -eq 0 ]; then OVERALL="PASS"; else OVERALL="FAIL"; fi

jq -n \
  --arg schema  "window-provenance/v1" \
  --arg overall "$OVERALL" \
  --arg driver  "$DRIVER" \
  --arg box     "${BOX:-local}" \
  --arg boxts   "$(obs box.timestamp_utc)" \
  --arg boxuname "$(obs box.uname)" \
  --arg boxuser "$(obs box.user)" \
  --arg laptopts "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
  --arg pinsfile "$PINS_FILE" \
  --arg pinssha  "$(sha_of "$PINS_FILE")" \
  --arg overrides "$OVERRIDES" \
  --arg effsha   "$(printf '%s' "$PINS_RAW" | shasum -a 256 | awk '{print $1}')" \
  --arg gatesha  "$(sha_of "$0")" \
  --arg libsha   "$(sha_of "$PROBE")" \
  --arg provsha  "$( [ -f "$PROVISION" ] && sha_of "$PROVISION" )" \
  --arg smokerec "$SMOKE_RECIPE" \
  --arg smokev   "$SMOKE_VERDICT" \
  --arg smokeargv "$SMOKE_ARGV" \
  --arg smokerc  "$(obs_win smoke.rc)" \
  --arg smokeerr "$(obs_win smoke.stderr)" \
  --arg smokeout "$(obs_win smoke.stdout)" \
  --arg smokewerr "$(obs_win smoke.worker_stderr)" \
  --arg smokestart "$(obs_win smoke.started_utc)" \
  --arg lockstate "$LOCK_STATE" \
  --arg lockpath  "$BOX_LOCK" \
  --arg locktag   "$WINDOW_TAG" \
  --arg lockacq   "$LOCK_ACQ_UTC" \
  --arg lockholder "$LOCK_HOLDER" \
  --arg lockrel   "$LOCK_RELEASED_UTC" \
  --arg unwindv   "$UNWIND_VERDICT" \
  --arg qunload   "$(obs_win qwen.unloaded)" \
  --arg qunloadrc "$(obs_win qwen.unload_rc)" \
  --arg reaped     "$(obs_win lock.reaped)" \
  --arg rpholder   "$(obs_win lock.reaped_prior_holder)" \
  --arg rptag      "$(obs_win lock.reaped_prior_tag)" \
  --arg rppid      "$(obs_win lock.reaped_prior_pid)" \
  --arg rpuser     "$(obs_win lock.reaped_prior_user)" \
  --arg rpacq      "$(obs_win lock.reaped_prior_acquired_utc)" \
  --arg rage       "$(obs_win lock.reaped_age_seconds)" \
  --arg rhow       "$(obs_win lock.reaped_verified_dead_how)" \
  --arg rmoved     "$(obs_win lock.reaped_moved_to)" \
  --arg rutc       "$(obs_win lock.reaped_utc)" \
  --arg rthresh    "$LOCK_REAP_AGE_S" \
  --arg rrefused   "$(w="$(obs_win lock.reap_refused)"; printf '%s' "${w:-$REAP_REFUSED}")" \
  --arg rrefdetail "$(w="$(obs_win lock.reap_refused_detail)"; printf '%s' "${w:-$REAP_REFUSED_DETAIL}")" \
  --argjson nfail "$N_FAIL" \
  --argjson items "$ITEMS" \
  '{
     schema: $schema,
     verdict: $overall,
     failed_items: $nfail,
     gate: {
       script: "scripts/window-preflight.sh", script_sha256: $gatesha,
       probe:  "scripts/window-probe.sh",     probe_sha256:  $libsha,
       provision: "scripts/window-provision.sh", provision_sha256: $provsha,
       pins_file: $pinsfile, pins_file_sha256: $pinssha,
       # B-6: the pins FILE hash alone is a half-truth — `--pin KEY=VALUE` overrides (waivers
       # included) never touched the file. `effective_pins_sha256` digests what the run really
       # used; `cli_overrides` lists them verbatim.
       effective_pins_sha256: $effsha,
       cli_overrides: ($overrides | split("\n") | map(select(length > 0))),
       driver: $driver, laptop_timestamp_utc: $laptopts
     },
     box: {
       alias: $box, timestamp_utc: $boxts, uname: $boxuname, user: $boxuser
     },
     phases: {
       pins:   ($items | map(select(.phase=="pins"))   | if any(.verdict|test("FAIL|REFUSED")) then "FAIL" else "PASS" end),
       basics: ($items | map(select(.phase=="basics")) | if any(.verdict|test("FAIL|REFUSED")) then "FAIL" else "PASS" end),
       smoke:  $smokev
     },
     smoke: {
       recipe: $smokerec, verdict: $smokev, argv: $smokeargv, exit_code: $smokerc,
       started_utc: $smokestart,
       benchd_stderr: $smokeerr, benchd_stdout: $smokeout, worker_stderr: $smokewerr
     },
     items: $items,
     # Single-flight is PROVED here, not asserted: the holder record, the box-clock acquisition
     # timestamp, and the tag that a later --release must match before it may remove anything.
     lock: {
       dialect: "mkdir-session-lock",
       path: $lockpath,
       window_tag: $locktag,
       state: $lockstate,
       acquired_utc: $lockacq,
       holder: $lockholder,
       # RULED: reap the provably dead, refuse the ambiguous. Either way the evidence is here.
       reap_age_threshold_s: $rthresh,
       reaped: (if $reaped == "1" then {
         prior_holder: $rpholder, prior_tag: $rptag, prior_pid: $rppid, prior_user: $rpuser,
         prior_acquired_utc: $rpacq, age_seconds: $rage,
         verified_dead_how: $rhow, reaped_utc: $rutc,
         # M-a: the pointer to the moved-aside directory. The probe has always emitted it and the
         # docs have always promised it, but the attestation dropped it — so the sealed artifact,
         # the one thing that outlives the run, had no way to find the evidence it was pointing
         # at. Reaping moves the corpse aside precisely so it stays inspectable; without this
         # field nobody can inspect it.
         moved_to: (if $rmoved == "" then null else $rmoved end)
       } else null end),
       reap_refused: (if $rrefused == "" then null else {reason: $rrefused, detail: $rrefdetail} end),
       released_utc: (if $lockrel == "" then null else $lockrel end),
       unwind_verdict: (if $unwindv == "" then null else $unwindv end),
       note: "state=held means the lock was HANDED OFF and is still held; released_utc is set when this run unwound, or by `window-preflight.sh --release`, which writes window-release.json alongside this file"
     },
     qwen: { unloaded: $qunload, unload_rc: $qunloadrc },
     lock_taken: ($lockstate == "held")
   }' > "$ATT"

# The attestation belongs NEXT TO THE RUN ARTIFACTS, on the box, or it is not sealing anything
# the window can later be read against.
if [ -n "$BOX_OUT" ]; then
  if [ "$DRIVER" = "local" ]; then
    if mkdir -p "$BOX_OUT" && cp "$ATT" "$BOX_OUT/window-provenance.json"; then
      printf '\nattestation -> %s and %s\n' "$ATT" "$BOX_OUT/window-provenance.json"
    else
      printf '\nWARNING: could not place the attestation in %s\n' "$BOX_OUT" >&2
    fi
  else
    # M9: `scp host:'path'` re-parses the remote side through a shell whose quoting rules
    # changed under OpenSSH 10's SFTP-backed transfer, so embedded quotes can silently fail —
    # and an `&&` chain swallows the failure, leaving the operator believing the evidence
    # landed. Instead: ONE base64 argument carrying both the destination and the bytes, with
    # the receiving script on stdin. Nothing the remote shell can word-split, and the exit
    # status is checked explicitly.
    #
    # The payload rides in argv, so it is bounded by ARG_MAX (~1 MiB on macOS). Attestations
    # run ~10-20 KB; refuse loudly rather than truncate if one ever grows past a safe fraction.
    ATT_PAYLOAD="$(printf '%s\n%s\n' "$(printf '%s' "$BOX_OUT" | b64)" "$(b64 < "$ATT")")"
    ATT_PAYLOAD="$(printf '%s' "$ATT_PAYLOAD" | b64)"
    if [ "${#ATT_PAYLOAD}" -gt 262144 ]; then
      printf '\nWARNING: the attestation is too large to deliver in one argument (%s bytes b64).\n' \
        "${#ATT_PAYLOAD}" >&2
      printf '         It exists at %s and was NOT copied to the box.\n' "$ATT" >&2
      false
    elif ssh "$BOX" bash -s -- "$ATT_PAYLOAD" <<'RECV'
set -u
if printf 'eA==' | base64 -d >/dev/null 2>&1; then D="-d"; else D="-D"; fi
unpacked="$(printf '%s' "${1-}" | base64 "$D")"
dir="$(printf '%s\n' "$unpacked" | sed -n 1p | base64 "$D")"
[ -n "$dir" ] || exit 1
mkdir -p "$dir" || exit 1
printf '%s\n' "$unpacked" | sed -n 2p | base64 "$D" > "$dir/window-provenance.json" || exit 1
[ -s "$dir/window-provenance.json" ] || exit 1
RECV
    then
      printf '\nattestation -> %s and %s:%s/window-provenance.json\n' "$ATT" "$BOX" "$BOX_OUT"
    else
      printf '\nWARNING: the attestation did NOT reach %s:%s — it exists only at %s\n' \
        "$BOX" "$BOX_OUT" "$ATT" >&2
    fi
  fi
else
  printf '\nattestation -> %s (no WP_BOX_OUT pinned, so it was not placed next to the run artifacts)\n' "$ATT"
fi

printf '\n== window-preflight: %s (%s failed item(s)) ==\n' "$OVERALL" "$N_FAIL"
if [ "$N_FAIL" -eq 0 ]; then
  if [ "$LOCK_STATE" = "held" ]; then
    # HAND OFF. The gate exits STILL HOLDING the lock, so there is no window between the smoke
    # leg and the window proper in which another session could take the box. The trap is
    # disarmed for exactly this reason — this is the one path on which not unwinding is right.
    _HANDOFF=1
    printf '\nBOX LOCK IS HELD — %s (tag %s, acquired %s).\n' "$BOX_LOCK" "$WINDOW_TAG" "$LOCK_ACQ_UTC"
    printf 'qwen is UNLOADED. Run the window now — pass WP_WINDOW_TAG=%s so the driver\n' "$WINDOW_TAG"
    printf 'inherits this lock instead of aborting on it.\n'
    printf 'When the window is done, ALWAYS run:\n'
    printf '  %s --pins %s --release\n' "$0" "$PINS_FILE"
    printf 'which reloads the serving model, verifies it is back, and releases the lock.\n'
  else
    printf 'All checks passed. The smoke leg was waived, so no lock is held.\n'
  fi
  exit $E_PASS
fi
if [ "$LOCK_STATE" = "held" ]; then
  printf 'The gate FAILED while holding the lock — unwinding (qwen reload + release) now.\n' >&2
else
  printf 'NO LOCK IS HELD. Fix the named items above and re-run.\n' >&2
fi
exit "$WORST"
