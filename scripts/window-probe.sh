#!/bin/bash
# scripts/window-probe.sh — the BOX-SIDE half of the window-preflight gate.
#
# This script OBSERVES and never JUDGES. It runs on the box (piped over ssh as `bash -s`, or
# locally under DRIVER=local), gathers raw facts about the trees / binaries / weights /
# goldens / host, and prints them as a flat `key=<base64>` record on stdout. Every comparison
# against the expected pins happens on the LAPTOP side, in window-preflight.sh, so:
#
#   * the assertion logic is testable without a box (it is pure local shell over the record), and
#   * the box is never told what the right answer is, so it cannot launder a mismatch into a pass.
#
# WHY key=<base64> AND NOT JSON. The probe must run on a box that may not have `jq`, and it
# carries values (porcelain dirty-file lists, worker stderr, `uname -a`) containing newlines,
# quotes and arbitrary bytes. base64 makes the transport total; the laptop side (which does
# require jq) decodes and builds the JSON attestation.
#
# INPUT  : $1 = base64 of a newline-separated `key=<base64-value>` request.
# OUTPUT : stdout = `key=<base64-value>` observation lines, ending with `probe.ok`.
#          stderr = free-form progress (never parsed).
# EXIT   : 0 when the probe RAN to completion, even if everything it observed is wrong.
#          Non-zero only when the probe itself could not run. "Observed a bad thing" is a zero
#          exit with the bad thing in the record — judging is not the probe's job.
#
# PORTABILITY: bash 3.2, no `mapfile`, no associative arrays, no `${var,,}`, no `timeout(1)`,
# no `realpath(1)` — the same floor scripts/parity-lib.sh:12-15 sets for the box.
set -uo pipefail

# ---------------------------------------------------------------- record I/O --
# macOS base64 spells decode `-D` on older releases and `-d` on newer/GNU; probe once.
if printf 'eA==' | base64 -d >/dev/null 2>&1; then _B64D="-d"; else _B64D="-D"; fi
_b64()   { base64 | tr -d '\n'; }
_unb64() { printf '%s' "$1" | base64 "$_B64D" 2>/dev/null; }
# emit <key> <value> — value may contain anything at all.
emit() { printf '%s=%s\n' "$1" "$(printf '%s' "$2" | _b64)"; }
# emit <key> from a FILE's bytes (avoids a multi-megabyte argv).
emit_file() { printf '%s=%s\n' "$1" "$(_b64 < "$2")"; }

REQ=""
[ $# -ge 1 ] && REQ="$(_unb64 "$1")"

# req <key> [fallback] — read one request field.
#
# B1: a key that is PRESENT but EMPTY must fall through to the fallback. The laptop side emits
# every request key unconditionally, so an unset pin arrives as `key=` (empty base64) — and the
# original `return 0` on first match meant the fallback was DEAD for every key the gate emits,
# which is all of them. With `WP_QWEN_UNLOAD_TRIES` left at its documented default, QTRIES came
# back "", the unload poll `[ 0 -lt "" ]` never ran, `gone` stayed 0, and the gate refused every
# real box with exit 6. Empty is indistinguishable from absent here on purpose: no pin this
# probe reads has a meaningful empty value.
req() {
  local k="$1" d="${2-}" line v="" out
  while IFS= read -r line; do
    case "$line" in
      "$k="*) v="${line#"$k"=}"; break ;;
    esac
  done <<EOF
$REQ
EOF
  if [ -n "$v" ]; then
    out="$(_unb64 "$v")"
    if [ -n "$out" ]; then printf '%s' "$out"; return 0; fi
  fi
  printf '%s' "$d"
}

# req_int <key> <fallback> — as req, but guarantees a non-negative integer comes back. Every
# `req`-with-default call in this file feeds an arithmetic comparison, and `[ 0 -lt "abc" ]`
# is a shell error that silently skips the loop it guards. Belt to B1's braces.
req_int() {
  local v; v="$(req "$1" "$2")"
  case "$v" in
    ''|*[!0-9]*) printf '%s' "$2" ;;
    *)           printf '%s' "$v" ;;
  esac
}

sha_of() { shasum -a 256 "$1" 2>/dev/null | awk '{print $1}'; }

# ------------------------------------------------------------------- host ----
# B-1 (secret tier): the attestation is a shareable artifact, so it carries the box ALIAS the
# gate was invoked with — never a raw hostname. `uname -a` embeds the hostname; `uname -srm`
# gives the kernel/arch facts we actually want without it.
emit box.uname         "$(uname -srm 2>/dev/null)"
# The timestamp is the BOX's, not the laptop's: the attestation must date the observation where
# it happened, so a record cannot be silently re-dated by re-running the gate somewhere else.
emit box.timestamp_utc "$(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null)"
emit box.user          "$(id -un 2>/dev/null)"
emit box.probe_pid     "$$"

# ------------------------------------------------------------ git checkouts --
# probe_git <role> <path> — emits, for role in {bench, engine}:
#   .pinned/.path_exists/.is_repo   1/0
#   .head            40-hex resolved HEAD
#   .dirty           1/0 — `status --porcelain` non-empty (untracked counts)
#   .dirty_files     the porcelain output itself, so the gate's diagnostic can NAME what is
#                    dirty rather than merely assert that something is
#   .origin_url      "" when there is no origin remote — one of the two bundle tells
#   .bundle_marker   contents of `.window-bundle-provenance`, "" when absent
probe_git() {
  local role="$1" path="$2" porc marker origin hidden
  if [ -z "$path" ]; then emit "$role.pinned" "0"; return 0; fi
  emit "$role.pinned" "1"
  emit "$role.path" "$path"
  if [ ! -d "$path" ]; then
    emit "$role.path_exists" "0"; emit "$role.is_repo" "0"
    emit "$role.head" ""; emit "$role.dirty" ""; emit "$role.dirty_files" ""; emit "$role.hidden_flags" ""
    emit "$role.origin_url" ""; emit "$role.origin_kind" "none"; emit "$role.bundle_marker" ""
    return 0
  fi
  emit "$role.path_exists" "1"
  if ! git -C "$path" rev-parse --git-dir >/dev/null 2>&1; then
    emit "$role.is_repo" "0"
    emit "$role.head" ""; emit "$role.dirty" ""; emit "$role.dirty_files" ""; emit "$role.hidden_flags" ""
    emit "$role.origin_url" ""; emit "$role.origin_kind" "none"; emit "$role.bundle_marker" ""
    return 0
  fi
  emit "$role.is_repo" "1"
  emit "$role.head" "$(git -C "$path" rev-parse HEAD 2>/dev/null)"
  porc="$(git -C "$path" status --porcelain 2>/dev/null)"
  if [ -n "$porc" ]; then emit "$role.dirty" "1"; else emit "$role.dirty" "0"; fi
  emit "$role.dirty_files" "$porc"
  # M6: `assume-unchanged` (lowercase status letter) and `skip-worktree` (S) make git REPORT a
  # modified file as clean, so `status --porcelain` alone is not proof that HEAD describes the
  # working tree. `ls-files -v` shows the real flags.
  hidden="$(git -C "$path" ls-files -v 2>/dev/null | grep -E '^[a-zS] ' | head -50)"
  if [ -n "$hidden" ]; then emit "$role.hidden_flags" "$hidden"; else emit "$role.hidden_flags" ""; fi
  origin="$(git -C "$path" config --get remote.origin.url 2>/dev/null)"
  emit "$role.origin_url" "$origin"
  # B3: `git clone <file>.bundle` sets origin to the BUNDLE'S FILE PATH — non-empty, so an
  # emptiness test reads an unprovenanced bundle tree as "cloned from a remote". This is the
  # Proof A path verbatim. Classify the origin instead: only a real remote URL counts as
  # provenance; a filesystem path, a file:// URL or a .bundle target does not.
  case "$origin" in
    https://*|http://*|ssh://*|git://*) emit "$role.origin_kind" "remote-url" ;;
    *@*:*)                              emit "$role.origin_kind" "scp-like-remote" ;;
    '')                                 emit "$role.origin_kind" "none" ;;
    *.bundle)                           emit "$role.origin_kind" "bundle-path" ;;
    file://*|/*|./*|../*)               emit "$role.origin_kind" "local-path" ;;
    *)                                  emit "$role.origin_kind" "unrecognised" ;;
  esac
  # The bundle record lives in the GIT DIR, not the working tree. Written into the worktree it
  # would show up as an untracked file, so every bundle-provisioned tree would fail the
  # clean-tree assertion — the provenance record would defeat the provenance check.
  marker="$(git -C "$path" rev-parse --absolute-git-dir 2>/dev/null)/window-bundle-provenance"
  if [ -f "$marker" ]; then
    emit "$role.bundle_marker" "$(cat "$marker" 2>/dev/null)"
    # M4: the marker is a writable file this gate's own tooling wrote — self-attestation, not
    # proof. When the bundle is STILL ON THE BOX we can re-derive the claim from the bundle's
    # own bytes: re-digest it, and confirm the pinned commit is actually one of its heads.
    # That upgrades the row from CLAIMED to VERIFIED; without the bundle it stays CLAIMED.
    local bpath bsha
    bpath="$(sed -n 's/^bundle_path=//p' "$marker" | head -1)"
    emit "$role.bundle_file" "$bpath"
    if [ -n "$bpath" ] && [ -f "$bpath" ]; then
      bsha="$(sha_of "$bpath")"
      emit "$role.bundle_file_sha256" "$bsha"
      emit "$role.bundle_file_present" "1"
      emit "$role.bundle_heads" "$(git -C "$path" bundle list-heads "$bpath" 2>/dev/null)"
    else
      emit "$role.bundle_file_present" "0"
      emit "$role.bundle_file_sha256" ""; emit "$role.bundle_heads" ""
    fi
  else
    emit "$role.bundle_marker" ""; emit "$role.bundle_file" ""
    emit "$role.bundle_file_present" "0"; emit "$role.bundle_file_sha256" ""; emit "$role.bundle_heads" ""
  fi
}

# ---------------------------------------------------------------- the modes --
# The probe has three motions, because a GPU window has three:
#   observe — read-only facts about the trees, binaries, weights, goldens and box. Takes no
#             lock, touches no serving state, changes nothing.
#   window  — ACQUIRE the box lock, unload the serving model, run the smoke leg, and HOLD
#             the lock on the way out so the window that follows inherits single-flight.
#   release — reload the serving model, verify it is back, and release the lock this session
#             took (and only that one).
MODE="$(req mode observe)"

if [ "$MODE" = "observe" ]; then
probe_git bench  "$(req bench_path)"
probe_git engine "$(req engine_path)"

# ---------------------------------------------------------------- binaries ---
# probe_bin <role> <path> <identity_argv>
# The identity RUN is the point. A binary that merely EXISTS at the pinned path proves nothing
# about what it does when spawned — Proof A had the right binary at the right path and still
# died at the handshake. We execute it and capture what it says it is.
# `identity_argv` = the literal string "none" is an explicit DECLARATION that this binary
# exposes no identity flag; recorded as `declared-none`, never silently skipped.
probe_bin() {
  local role="$1" path="$2" idargv="$3" out rc
  if [ -z "$path" ]; then emit "$role.pinned" "0"; return 0; fi
  emit "$role.pinned" "1"
  emit "$role.path" "$path"
  if [ ! -f "$path" ]; then
    emit "$role.exists" "0"; emit "$role.executable" "0"; emit "$role.sha256" ""
    emit "$role.identity_rc" ""; emit "$role.identity_out" ""
    return 0
  fi
  emit "$role.exists" "1"
  if [ -x "$path" ]; then emit "$role.executable" "1"; else emit "$role.executable" "0"; fi
  emit "$role.sha256" "$(sha_of "$path")"
  if [ "$idargv" = "none" ] || [ -z "$idargv" ]; then
    emit "$role.identity_rc" "declared-none"; emit "$role.identity_out" ""
    return 0
  fi
  # shellcheck disable=SC2086  # idargv is a pinned argv template; word-splitting is intended.
  out="$( "$path" $idargv 2>&1 )"; rc=$?
  emit "$role.identity_rc" "$rc"
  emit "$role.identity_out" "$out"
}

probe_bin enginebin "$(req engine_bin)" "$(req engine_identity_argv none)"
probe_bin benchdbin "$(req benchd_bin)" "$(req benchd_identity_argv none)"
# LANE 2b (#148) — optional swift-worker seal (empty path ⇒ probe_bin emits pinned=0 and returns).
probe_bin swiftworkerbin "$(req swift_worker_bin)" "$(req swift_worker_identity_argv none)"

# ----------------------------------------------------------------- weights ---
# Port of benchctl's `dir_digest` (crates/benchctl/src/iterate.rs:180-212), itself a port of the
# Swift tree formula. It MUST agree byte-for-byte, or the gate would clear a weights tree the
# run then rejects — or, far worse, the reverse.
#   files:  every regular file under root, relative path, `\` -> `/`
#   ignore: the EXACT root-relative paths `.benchmark-source.sha256` and `.gitkeep`. Matches on
#           those BASENAMES deeper in the tree are NOT ignored (iterate.rs:184-186 calls this
#           out: Swift ignores by exact relative path).
#   sort:   by relative path, bytewise (LC_ALL=C)
#   fold:   sha256 over  rel || 0x00 || <32 RAW digest bytes> || 0x00  per file, in order
probe_weights() {
  local root="$1" list stream rel fc=0 bc=0 h sz broken
  if [ -z "$root" ]; then emit weights.pinned "0"; return 0; fi
  emit weights.pinned "1"; emit weights.path "$root"
  if [ ! -d "$root" ]; then
    emit weights.exists "0"; emit weights.sha256 ""
    emit weights.file_count ""; emit weights.byte_count ""
    return 0
  fi
  emit weights.exists "1"
  list="$(mktemp "${TMPDIR:-/tmp}/wp-wl.XXXXXX")"
  stream="$(mktemp "${TMPDIR:-/tmp}/wp-ws.XXXXXX")"
  # A BROKEN symlink still reports as type `l` under `find -L` (there is nothing to resolve to).
  # benchd errors on it — `File::open` fails inside `sha256_file_streaming` — so silently
  # skipping it here would let the gate clear a weights tree the run then refuses.
  broken="$( ( cd "$root" && find -L . -type l -print ) 2>/dev/null | sed 's|^\./||' | head -5 )"
  if [ -n "$broken" ]; then
    emit weights.error "broken-symlink"
    emit weights.error_detail "$broken"
    emit weights.sha256 ""; emit weights.file_count ""; emit weights.byte_count ""
    rm -f "$list" "$stream"; return 0
  fi
  emit weights.error ""
  # NEW-2: NUL-delimited, because a newline in a filename split the old `find -print | read`
  # pipeline into two bogus paths. `sha_of` then returned empty, and the empty-hash branch below
  # blanked the digest and returned WITHOUT setting weights.error — so the guard stayed quiet and
  # the gate reported drift or tamper for a tree the Rust dir_digest walks without complaint. A
  # silent WRONG verdict is worse than a loud refusal. find -print0 / sort -z / read -d '' all
  # behave on BSD and GNU, so the tree is simply handled correctly instead of being rejected.
  # Exclusions moved into the loop: `grep -z` is not portable, and a `case` needs no subprocess.
  ( cd "$root" && find -L . -type f -print0 ) 2>/dev/null | LC_ALL=C sort -z > "$list"
  : > "$stream"
  while IFS= read -r -d '' rel; do
    rel="${rel#./}"
    [ -n "$rel" ] || continue
    case "$rel" in .benchmark-source.sha256|.gitkeep) continue ;; esac
    h="$(sha_of "$root/$rel")"
    if [ -z "$h" ]; then
      # Fail CLOSED, and say which file. Reaching here means a file the walk listed could not be
      # digested at all; emitting empty counts with no error is what produced the silent
      # misverdict this guard exists to prevent.
      emit weights.error "undigestible-file"
      emit weights.error_detail "$rel"
      emit weights.sha256 ""; emit weights.file_count ""; emit weights.byte_count ""
      rm -f "$list" "$stream"; return 0
    fi
    sz="$(wc -c < "$root/$rel" | tr -d ' ')"
    fc=$((fc + 1)); bc=$((bc + sz))
    { printf '%s' "$rel"; printf '\000'
      printf '%s' "$h" | xxd -r -p
      printf '\000'; } >> "$stream"
  done < "$list"
  emit weights.sha256     "$(sha_of "$stream")"
  emit weights.file_count "$fc"
  emit weights.byte_count "$bc"
  rm -f "$list" "$stream"
}
probe_weights "$(req weights_path)"

# ------------------------------------------------- goldens + contract files ---
# The sha256+bytes pin convention (bench-core/src/golden.rs:549-587): a golden is identified by
# BOTH its digest and its exact byte count, byte count checked FIRST. Two numbers, because the
# length is the cheap tripwire that catches a truncated or re-wrapped file before the digest has
# to be trusted at all.
probe_file_pin() {
  local key="$1" path="$2"
  emit "$key.path" "$path"
  if [ ! -f "$path" ]; then
    emit "$key.exists" "0"; emit "$key.sha256" ""; emit "$key.bytes" ""
    return 0
  fi
  emit "$key.exists" "1"
  emit "$key.sha256" "$(sha_of "$path")"
  emit "$key.bytes"  "$(wc -c < "$path" | tr -d ' ')"
}

GOLDEN_PATHS="$(req golden_paths)"
GOLDEN_PINS="$(req golden_pins)"   # parallel "sha bytes" lines, for the validate-golden leg
BENCHD_BIN="$(req benchd_bin)"
gi=0
while IFS= read -r gp; do
  [ -n "$gp" ] || continue
  gi=$((gi + 1))
  probe_file_pin "golden.$gi" "$gp"
  # Second, INDEPENDENT check: hand the file to the very loader the run will use, with the
  # pinned pair. Our own shasum agreeing with the pin is necessary but not sufficient — the
  # run is gated by benchctl's loader, so the preflight asks the loader directly.
  gpin="$(printf '%s\n' "$GOLDEN_PINS" | sed -n "${gi}p")"
  gsha="${gpin%% *}"; gbytes="${gpin##* }"
  if [ -n "$BENCHD_BIN" ] && [ -x "$BENCHD_BIN" ] && [ -n "$gsha" ] && [ -n "$gbytes" ]; then
    vg_out="$("$BENCHD_BIN" validate-golden --golden "$gp" \
                --golden-sha256 "$gsha" --golden-bytes "$gbytes" 2>&1)"; vg_rc=$?
    emit "golden.$gi.validate_rc"  "$vg_rc"
    emit "golden.$gi.validate_out" "$vg_out"
  else
    emit "golden.$gi.validate_rc" "skipped"; emit "golden.$gi.validate_out" ""
  fi
done <<EOF
$GOLDEN_PATHS
EOF
emit golden.count "$gi"

# POOL TAPES — the R2 track-pool objects the CONTRACT pins as comparison inputs. They are NOT
# goldens: a golden carries the weights-hash + prompts + prompt-SHAs binding, and the bench-core
# golden loader REJECTS a tape outright ("unknown field `seed_tokens`"). So a tape gets the same
# sha256+bytes pin, but its shape check is the REQUIRED-KEY SIGNATURE measure-job actually routes
# on (seed_tokens / reference_seed_token / rows) — never `validate-golden`, which would fail
# every tape and teach operators to ignore the check.
TAPE_PATHS="$(req pool_tape_paths)"
ti=0
while IFS= read -r tp; do
  [ -n "$tp" ] || continue
  ti=$((ti + 1))
  probe_file_pin "tape.$ti" "$tp"
  if [ -f "$tp" ]; then
    for k in seed_tokens reference_seed_token rows; do
      if grep -q "\"$k\"" "$tp" 2>/dev/null; then emit "tape.$ti.sig_$k" "1"
      else emit "tape.$ti.sig_$k" "0"; fi
    done
  else
    for k in seed_tokens reference_seed_token rows; do emit "tape.$ti.sig_$k" ""; done
  fi
done <<EOF
$TAPE_PATHS
EOF
emit tape.count "$ti"

CONTRACT_PATH="$(req contract_path)"
if [ -n "$CONTRACT_PATH" ]; then
  emit contract.pinned "1"; probe_file_pin contract "$CONTRACT_PATH"
  # LANE 2a — the track contract is the REVIEW-GATED AUTHORITY for the hidden correctness golden's
  # identity (its `hidden_correctness_golden` sha256+bytes SIBLING pin, a sibling of timed_prompt_pool
  # that never perturbs the pinned pool count). Emit that pin (empty when the field is absent) so the
  # gate can SOURCE it FROM THE FIXTURE and verify the staged golden against it, rather than an
  # operator-supplied WP_GOLDEN pin (machine-state). Read with jq when the box has it, else a
  # jq-free fallback (this probe may run on a box without jq): the pin object carries no nested
  # braces, so `[^}]*` isolates it, and the `_note` sibling is a different key left untouched.
  if [ -f "$CONTRACT_PATH" ]; then
    if command -v jq >/dev/null 2>&1; then
      emit contract.hcg_sha256 "$(jq -r '.hidden_correctness_golden.sha256 // empty' "$CONTRACT_PATH" 2>/dev/null)"
      emit contract.hcg_bytes  "$(jq -r '.hidden_correctness_golden.bytes // empty'  "$CONTRACT_PATH" 2>/dev/null)"
    else
      hcg_blk="$(tr -d '\n' < "$CONTRACT_PATH" | sed -n 's/.*"hidden_correctness_golden"[[:space:]]*:[[:space:]]*{\([^}]*\)}.*/\1/p')"
      emit contract.hcg_sha256 "$(printf '%s' "$hcg_blk" | sed -n 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([0-9a-f]\{64\}\)".*/\1/p')"
      emit contract.hcg_bytes  "$(printf '%s' "$hcg_blk" | sed -n 's/.*"bytes"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')"
    fi
  else
    emit contract.hcg_sha256 ""; emit contract.hcg_bytes ""
  fi
else
  emit contract.pinned "0"
fi

# --------------------------------------------------------- basic box health --
# Free bytes on the volume that will hold the run's artifacts. `df -k` is the portable common
# denominator (macOS `df` has no `--output`); column 4 is available 1K-blocks.
OUT_VOL="$(req out_dir)"; [ -n "$OUT_VOL" ] || OUT_VOL="$HOME"
emit disk.path "$OUT_VOL"
DF_AVAIL_K="$(df -k "$OUT_VOL" 2>/dev/null | awk 'NR==2 {print $4}')"
if [ -n "$DF_AVAIL_K" ]; then emit disk.free_bytes "$((DF_AVAIL_K * 1024))"
else emit disk.free_bytes ""; fi

# GPU / box locks — OBSERVED, never taken. `parity_take_gpu_lock` acquires and HOLDS (fd 9 in
# the caller's shell, parity-lib.sh:14-15,59), so a preflight must not call it: it would either
# hold the lock it is reporting on, or violate the no-subshell contract. The non-destructive
# read is the mkdir-dialect BOX_LOCK dir plus its `pid` file, with liveness by `ps -p` and
# NEVER `kill -0` (cross-uid EPERM false-negative, docs/measure-job-contract.md:144-149).
probe_lock() {
  local key="$1" path="$2" pid mtime
  if [ -z "$path" ]; then emit "$key.pinned" "0"; return 0; fi
  emit "$key.pinned" "1"; emit "$key.path" "$path"
  if [ ! -e "$path" ]; then
    emit "$key.present" "0"; emit "$key.pid" ""; emit "$key.holder_alive" ""
    emit "$key.age_seconds" ""
    return 0
  fi
  emit "$key.present" "1"
  pid=""
  [ -f "$path/pid" ] && pid="$(tr -d '[:space:]' < "$path/pid" 2>/dev/null)"
  emit "$key.pid" "$pid"
  emit "$key.holder" "$(cat "$path/holder" 2>/dev/null)"
  # A holder is VERIFIABLE only when the pid file exists and is a plain integer. Anything else
  # — missing, empty, non-numeric — is unverifiable, and an unverifiable holder can never be
  # declared dead. `ps -p` on a non-numeric string fails, which would otherwise read as
  # "provably dead" and reap a lock nobody could account for.
  case "$pid" in
    ''|*[!0-9]*) emit "$key.pid_numeric" "0" ;;
    *)           emit "$key.pid_numeric" "1" ;;
  esac
  if [ -n "$pid" ] && [ -z "$(printf '%s' "$pid" | tr -d '0-9')" ]; then
    # `ps -p`, never `kill -0`: a cross-uid holder returns EPERM from kill(2), which would read
    # as dead (docs/measure-job-contract.md:144-149).
    if ps -p "$pid" >/dev/null 2>&1; then emit "$key.holder_alive" "1"
    else emit "$key.holder_alive" "0"; fi
  else
    emit "$key.holder_alive" "unknown"
  fi
  mtime="$(stat -f %m "$path" 2>/dev/null || stat -c %Y "$path" 2>/dev/null)"
  if [ -n "$mtime" ]; then emit "$key.age_seconds" "$(( $(date -u +%s) - mtime ))"
  else emit "$key.age_seconds" ""; fi
}
probe_lock boxlock "$(req box_lock)"
# The flock dialect (/tmp/mtplx-gpu-exclusive.lock) is kernel-enforced and cannot be observed
# without taking it, so we report only that the file exists and who owns it — never a verdict.
# B4: the flock dialect cannot be probed by TAKING it (that would be acquisition, and the fd
# would die with this shell anyway). But a flock holder must hold the file OPEN, so open-fd
# inspection is sound in the direction that matters: any process listing the lock file as open
# is evidence of a holder. It is deliberately CONSERVATIVE — an open fd without an actual flock
# would read as held and refuse, which is the fail-closed direction. Mere EXISTENCE of the file
# is not evidence of anything (it survives every window ever run), so it is never judged.
GPU_LOCK="$(req gpu_lock)"
if [ -n "$GPU_LOCK" ]; then
  emit gpulock.pinned "1"; emit gpulock.path "$GPU_LOCK"
  if [ -e "$GPU_LOCK" ]; then
    emit gpulock.present "1"; emit gpulock.stat "$(ls -ld "$GPU_LOCK" 2>/dev/null)"
    if command -v lsof >/dev/null 2>&1; then
      gl_holders="$(lsof -t -- "$GPU_LOCK" 2>/dev/null | tr '\n' ' ')"
      emit gpulock.holder_pids "$gl_holders"
      emit gpulock.probe "lsof"
      if [ -n "$(printf '%s' "$gl_holders" | tr -d ' ')" ]; then emit gpulock.held "1"
      else emit gpulock.held "0"; fi
    else
      emit gpulock.probe "unavailable"; emit gpulock.held "unknown"; emit gpulock.holder_pids ""
    fi
  else
    emit gpulock.present "0"; emit gpulock.stat ""
    emit gpulock.held "0"; emit gpulock.probe "file-absent"; emit gpulock.holder_pids ""
  fi
else
  emit gpulock.pinned "0"
fi

# Box-quiet observations — the same three facts `parity_precheck` (scripts/parity-lib.sh:26-48)
# asserts, gathered here and judged on the laptop. `pgrep -x` is EXACT-name on purpose: `-f`
# false-positives on the harness scripts that mention the binary in their own argv.
# M1/M3: the process ENVIRONMENT is part of the environment seam. `MLXFAST_RUNTIME_WORKER_EXECUTABLE`
# redirects which binary is spawned — which defeats WP_ENGINE_BIN_SHA256 outright — `MLXFAST_NO_SANDBOX`
# silently changes the spawn class, and an unset `QMTP_HEAD_DIR` makes the decode recipe die before
# the spawn (read as a post-handshake failure if you are not looking). Seal the whole namespace as
# observed HERE, in the same non-interactive shell the smoke leg will spawn from.
emit env.namespace "$(env 2>/dev/null | grep -E '^(MLXFAST_|QMTP_)' | LC_ALL=C sort)"
emit env.namespace_count "$(env 2>/dev/null | grep -cE '^(MLXFAST_|QMTP_)')"
# Individually, so an expected-vs-observed row can name the variable. `unset` is a DISTINCT
# observation from empty: `FOO=` and no FOO at all mean different things to the spawn.
for _v in $(req env_watch); do
  if env 2>/dev/null | grep -q "^${_v}="; then
    emit "env.$_v" "$(env 2>/dev/null | sed -n "s/^${_v}=//p" | head -1)"
    emit "env.$_v.set" "1"
  else
    emit "env.$_v" ""; emit "env.$_v.set" "0"
  fi
done

emit quiet.timemachine "$(tmutil status 2>/dev/null | tr -d '\n')"
emit quiet.loadavg_1m  "$(uptime 2>/dev/null | sed 's/.*load averages*: *//' | awk -F'[, ]+' '{print $1}')"
emit quiet.stray_swift  "$(pgrep -x mlxfast-swift  2>/dev/null | tr '\n' ' ')"
emit quiet.stray_engine "$(pgrep -x mlxfast-engine 2>/dev/null | tr '\n' ' ')"

# Serving state: the model must be in the state the window protocol assumes (UNLOADED for a
# window that is about to take the GPU). The probe REPORTS; it never unloads or reloads —
# changing box serving state is the window driver's job, under its own trap discipline.
QWEN_PATTERN="$(req qwen_proc_pattern)"
if [ -n "$QWEN_PATTERN" ]; then
  emit qwen.pinned "1"; emit qwen.pattern "$QWEN_PATTERN"
  qpids="$(pgrep -f "$QWEN_PATTERN" 2>/dev/null | tr '\n' ' ')"
  emit qwen.pids "$qpids"
  if [ -n "$qpids" ]; then emit qwen.state "loaded"; else emit qwen.state "unloaded"; fi
else
  emit qwen.pinned "0"
fi

fi

# ============================================================================
# MODE: window — ACQUIRE THE LOCK, UNLOAD, SMOKE, AND HOLD
# ============================================================================
# Single-flight is ENFORCED here, not merely asserted. Every GPU-touching motion on the box —
# full windows, smoke legs, one-off diagnostics, hypothesis probes — runs under this lock, and
# the gate takes it before the smoke leg and HOLDS it through the window that follows. A
# control that does not share the lock and residency conditions of the legs it controls for is
# not a control; the Proof A D1-class standalone probes ran bare and produced exactly that
# invalid result.
#
# The mkdir dialect is the one that can be held ACROSS ssh sessions: a flock fd dies with the
# shell that opened it, but a lock directory persists until someone removes it. That is the
# whole reason THE box lock is this dialect and not the flock.
if [ "$MODE" = "window" ]; then
  LOCK_DIR="$(req session_lock)"
  TAG="$(req window_tag)"
  emit lock.path "$LOCK_DIR"
  emit lock.tag  "$TAG"
  REAP_AGE_S="$(req_int lock_reap_age_s 900)"
  emit lock.reap_age_threshold_s "$REAP_AGE_S"
  emit lock.reaped "0"

  # REAP THE PROVABLY DEAD, REFUSE THE AMBIGUOUS (RULED, David 2026-08-20).
  #
  # A lock is reaped only when ALL of these hold, and the evidence for each is sealed into the
  # attestation before anything is removed:
  #   * the holder pid is VERIFIABLE  — the pid file exists and is a plain integer;
  #   * the holder is PROVABLY DEAD   — `ps -p <pid>` says not running (never `kill -0`, which
  #                                     returns EPERM for a live cross-uid holder);
  #   * the lock is OLD ENOUGH        — age >= the explicit threshold argument.
  # Live pid, unverifiable holder, or a fresh lock: refuse and report. This matches the upstream
  # contract's auto-reap (docs/measure-job-contract.md:144-152) while keeping the
  # no-unprompted-cleanup spirit — nothing is removed without a recorded proof of death.
  # N-1: REAP MUST BE ATOMIC WITH RESPECT TO ACQUISITION.
  #
  # The first cut read the lock's state, decided "reapable", then rm'd and rmdir'd it BEFORE the
  # mkdir. Two probes deciding from the same pre-acquisition snapshot both proceeded: the loser
  # deleted the pid/holder files of a lock the winner had legitimately created in the interval,
  # rmdir'd the now-empty directory, and mkdir'd its own — MULTIPLE WINNERS, each sealing a
  # `verified_dead_how` for a lock it had taken from a LIVE peer. Measured 11/25 three-way
  # trials. The pure acquire was never the problem (25/25 exactly-one with no prior lock); the
  # reap was.
  #
  # The shape that fixes it:
  #   1. Try the plain `mkdir` FIRST. With no stale lock, no reap code runs at all — that is the
  #      common path and it was always sound.
  #   2. Only on contention, contend for a separate REAP MUTEX (`mkdir`, atomic). At most one
  #      process may reap at a time; everyone else reports and stops.
  #   3. Under the mutex, RE-READ the lock's state from disk. The pre-mutex snapshot is exactly
  #      the stale information that caused the bug. A peer that acquired in the meantime is now
  #      visible: its pid is alive (refuse), and its directory is seconds old (refuse again).
  #   4. Reap by `mv`, not `rm` — rename is atomic, it preserves the reaped lock as evidence,
  #      and it cannot clobber files a peer wrote afterwards.
  #   5. ACQUISITION IS STILL AND ONLY THE `mkdir`. Reaping never confers ownership; if a peer
  #      wins the mkdir in the gap after our `mv`, we report not-acquired and it holds. Exactly
  #      one winner falls out of mkdir's atomicity, which is the one guarantee worth resting on.
  REAP_MUTEX="${LOCK_DIR}.reapmutex"
  # Orders of magnitude longer than a real reap, so this can only ever catch a corpse.
  REAP_MUTEX_STALE_S="${WP_REAP_MUTEX_STALE_S:-120}"
  _WP_MUTEX_HELD=0
  _wp_release_mutex() {
    [ "$_WP_MUTEX_HELD" = "1" ] || return 0
    rm -f "$REAP_MUTEX/pid" 2>/dev/null; rmdir "$REAP_MUTEX" 2>/dev/null; _WP_MUTEX_HELD=0
  }

  # N-5: capture rc AND stderr from the SAME attempt. The previous code re-ran `mkdir` purely to
  # harvest its error text, and that second attempt could transiently SUCCEED — stranding a
  # pid-less lock that every later gate then (correctly) refuses to reap, forever.
  # NEW-1: classify a failed mkdir by ITS OWN errno, never by a follow-up stat. The B-5 fix
  # tested `[ ! -d "$LOCK_DIR" ]` after the fact, which introduced the inverse of the bug it
  # closed: under plain contention a peer that RELEASED between our mkdir and our test left the
  # directory gone, so ordinary contention was reported as "could not be created" and the gate
  # exited 5 (E_MISSING) instead of 3 (E_BOX). A peer measured 13/600 iterations against a
  # churning peer. EEXIST means contention no matter what the directory looks like a moment
  # later; anything else is a real create failure and reproduces on retry.
  _lock_class=""; _mk_attempt=0
  while :; do
    _mk_err="$( { mkdir "$LOCK_DIR"; } 2>&1 )"; _mk_rc=$?
    if [ "$_mk_rc" -eq 0 ]; then _lock_class="acquired"; break; fi
    case "$_mk_err" in
      *[Ee]xists*)      _lock_class="contended" ;;
      *) if [ -d "$LOCK_DIR" ]; then _lock_class="contended"; else _lock_class="create_failed"; fi ;;
    esac
    [ "$_lock_class" = "create_failed" ] && break
    # Contention that has ALREADY resolved: the holder released while we were classifying. Retry
    # rather than report contention with no contender — the lock is free and ours to take.
    [ -d "$LOCK_DIR" ] && break
    # ...but never race a reap in progress. The prober that moved a provably-dead lock aside
    # under the mutex must be the one that takes the lock it cleared; a bystander retrying into
    # that gap would strand a TRUE reap record on a probe that ends up holding nothing, splitting
    # one lock's story across two attestations.
    [ -d "$REAP_MUTEX" ] && { _lock_class="contended"; break; }
    _mk_attempt=$((_mk_attempt + 1))
    [ "$_mk_attempt" -ge 8 ] && break
  done

  if [ "$_lock_class" = "create_failed" ]; then
    # B-5: not contention — a missing parent, an unwritable directory, a plain file at the path.
    emit lock.acquired "0"; emit lock.create_failed "1"; emit lock.create_error "$_mk_err"
    trap - HUP INT TERM
    emit probe.ok "1"; exit 0
  fi

  if [ "$_mk_rc" -ne 0 ]; then
    # Contended. Only the reap-mutex winner may even consider reaping.
    # F3: a trap cannot cover SIGKILL, and a stranded mutex is catastrophic in a quiet way —
    # every later probe stands down with `reap-in-progress`, so ONE hard-killed run disables
    # reaping on the box permanently. Age it out, but only on evidence: the mutex records its
    # holder's pid, and it is reclaimed only when that holder is provably gone AND the mutex is
    # far older than any real reap (which is a stat, a ps and a rename — sub-second). A live
    # reaper is never disturbed.
    if [ -d "$REAP_MUTEX" ]; then
      _mx_pid=""; [ -f "$REAP_MUTEX/pid" ] && _mx_pid="$(tr -d '[:space:]' < "$REAP_MUTEX/pid" 2>/dev/null)"
      _mx_mt="$(stat -f %m "$REAP_MUTEX" 2>/dev/null || stat -c %Y "$REAP_MUTEX" 2>/dev/null)"
      _mx_age=""; [ -n "$_mx_mt" ] && _mx_age="$(( $(date -u +%s) - _mx_mt ))"
      # A PID-LESS mutex is not evidence of death: a peer that has just mkdir'd and not yet
      # written its pid looks exactly the same. Only an age floor separates the two, so a mutex
      # with no recorded holder must survive at least that floor before it can be touched.
      _mx_reclaimable=0
      if [ -n "$_mx_age" ] && [ "$_mx_age" -ge "$REAP_MUTEX_STALE_S" ]; then
        if [ -n "$_mx_pid" ]; then
          ps -p "$_mx_pid" >/dev/null 2>&1 || _mx_reclaimable=1
        else
          _mx_reclaimable=1
        fi
      fi
      emit lock.reap_mutex_sampled_pid "$_mx_pid"
      emit lock.reap_mutex_sampled_mtime "$_mx_mt"
      _mx_mvback=0
      if [ "$_mx_reclaimable" = "1" ]; then
        # RECLAIM BY RENAME — but rename alone is NOT enough, and the previous comment here was
        # wrong to claim it was. `mv` is atomic, yet it operates on the PATH, not on the inode we
        # sampled: between the staleness sample and the rename, the stale mutex can be cleared
        # and a LIVE peer can mkdir the same path. The rename then moves THAT peer's mutex aside
        # and we would proceed to reap while it believes it holds exclusion — two reapers, which
        # is the whole failure this mutex exists to prevent.
        #
        # So the rename is followed by an IDENTITY CHECK against the sample: the directory we now
        # hold must carry the same holder pid AND the same mtime we judged stale. `rename` leaves
        # the moved directory's own mtime untouched, so the pair identifies the inode well enough
        # to tell a corpse from a live peer's fresh mutex.
        _mx_aside="${REAP_MUTEX}.stale.$$.$(date -u +%s)"
        if mv "$REAP_MUTEX" "$_mx_aside" 2>/dev/null; then
          _mx_apid=""; [ -f "$_mx_aside/pid" ] && _mx_apid="$(tr -d '[:space:]' < "$_mx_aside/pid" 2>/dev/null)"
          _mx_amt="$(stat -f %m "$_mx_aside" 2>/dev/null || stat -c %Y "$_mx_aside" 2>/dev/null)"
          if [ "$_mx_apid" = "$_mx_pid" ] && [ "$_mx_amt" = "$_mx_mt" ]; then
            emit lock.reap_mutex_reclaimed "1"
            emit lock.reap_mutex_reclaimed_detail "the reap mutex was ${_mx_age}s old and its holder (pid '${_mx_pid:-unrecorded}') is gone — renamed aside to $_mx_aside"
            # KEEP THE CORPSE. Sealing a pointer and then deleting what it points at is worse
            # than not sealing it: the record claims evidence that no longer exists. The lock
            # itself is reaped by rename precisely so the remains stay inspectable, and this
            # path is held to the same rule.
            emit lock.reap_mutex_reclaimed_moved_to "$_mx_aside"
          else
            # NOT the directory we judged. Put it back and take nothing: exclusion is only sound
            # if a prober that cannot prove what it moved refuses to act on it.
            _mx_mvback=1
            if [ ! -e "$REAP_MUTEX" ] && mv "$_mx_aside" "$REAP_MUTEX" 2>/dev/null; then
              emit lock.reap_mutex_reclaim_aborted "identity-mismatch-restored"
            else
              # The path was retaken while we held the directory aside, so it cannot be restored
              # without clobbering whatever is there now. Say so loudly — a displaced live mutex
              # is a real fault, and silence would leave its owner reaping under a mutex nobody
              # else can see.
              emit lock.reap_mutex_reclaim_aborted "identity-mismatch-displaced"
              emit lock.reap_mutex_displaced_path "$_mx_aside"
            fi
            emit lock.reap_mutex_reclaim_aborted_detail "sampled pid '${_mx_pid:-unrecorded}' mtime '${_mx_mt:-?}' but the renamed directory carried pid '${_mx_apid:-unrecorded}' mtime '${_mx_amt:-?}' — a live peer had retaken the path, so no reclaim was performed"
          fi
        fi
      fi
      emit lock.reap_mutex_mvback "$_mx_mvback"
    fi
    if mkdir "$REAP_MUTEX" 2>/dev/null; then
      _WP_MUTEX_HELD=1
      printf '%s\n' "$$" > "$REAP_MUTEX/pid" 2>/dev/null || true
      # N-b: cover the mutex from the instant it is held. The window-mode trap is installed
      # further down, AFTER this block, so a probe killed mid-reap left .reapmutex behind
      # forever — and a stranded mutex makes every later probe stand down with
      # `reap-in-progress`, so one interrupted run disables reaping on the box permanently.
      trap '_wp_release_mutex; exit 129' HUP
      trap '_wp_release_mutex; exit 130' INT
      trap '_wp_release_mutex; exit 143' TERM
      # FRESH read, under the mutex. Never the pre-mutex snapshot.
      r_pid=""; [ -f "$LOCK_DIR/pid" ] && r_pid="$(tr -d '[:space:]' < "$LOCK_DIR/pid" 2>/dev/null)"
      r_holder="$(cat "$LOCK_DIR/holder" 2>/dev/null)"
      r_mtime="$(stat -f %m "$LOCK_DIR" 2>/dev/null || stat -c %Y "$LOCK_DIR" 2>/dev/null)"
      r_age=""; [ -n "$r_mtime" ] && r_age="$(( $(date -u +%s) - r_mtime ))"
      r_numeric=0
      case "$r_pid" in ''|*[!0-9]*) r_numeric=0 ;; *) r_numeric=1 ;; esac
      if [ ! -d "$LOCK_DIR" ]; then
        emit lock.reap_refused "vanished"
        emit lock.reap_refused_detail "the lock was released by its holder while we waited for the reap mutex"
      elif [ "$r_numeric" != "1" ]; then
        # Also the guard for a peer that has mkdir'd but not yet written its pid: an
        # unattributable lock is never reaped, which is the fail-closed direction.
        emit lock.reap_refused "unverifiable-holder"
        emit lock.reap_refused_detail "the lock has no readable numeric pid (pid='$r_pid'), so its holder cannot be proved dead"
      # ORDER IS LOAD-BEARING: liveness is checked BEFORE age, and must stay that way. Nothing
      # refreshes the lock's mtime during a window, so a long window's lock ages past the
      # threshold while it is still running; it survives only because a live holder is refused
      # here, before age is consulted. Reorder these two, or promote age into a short-circuit,
      # and X-1 returns — a live window's lock becomes reapable purely by getting old.
      elif ps -p "$r_pid" >/dev/null 2>&1; then
        emit lock.reap_refused "holder-alive"
        emit lock.reap_refused_detail "pid $r_pid is still running"
      elif [ -z "$r_age" ]; then
        emit lock.reap_refused "unverifiable-age"
        emit lock.reap_refused_detail "the lock directory's mtime could not be read"
      elif [ "$r_age" -lt "$REAP_AGE_S" ]; then
        emit lock.reap_refused "too-fresh"
        emit lock.reap_refused_detail "pid $r_pid is gone but the lock is only ${r_age}s old (threshold ${REAP_AGE_S}s) — a holder that died seconds ago may be mid-restart"
      else
        # Provably dead, old enough, and we hold the reap mutex. Move it aside ATOMICALLY.
        _reaped_to="${LOCK_DIR}.reaped.$$.$(date -u +%s)"
        if mv "$LOCK_DIR" "$_reaped_to" 2>/dev/null; then
          emit lock.reaped "1"
          emit lock.reaped_prior_holder "$r_holder"
          emit lock.reaped_prior_pid "$r_pid"
          emit lock.reaped_prior_tag "$(printf '%s' "$r_holder" | sed -n 's/^tag=//p' | head -1)"
          emit lock.reaped_prior_user "$(printf '%s' "$r_holder" | sed -n 's/^user=//p' | head -1)"
          emit lock.reaped_prior_acquired_utc "$(printf '%s' "$r_holder" | sed -n 's/^acquired_utc=//p' | head -1)"
          emit lock.reaped_age_seconds "$r_age"
          emit lock.reaped_verified_dead_how "ps -p $r_pid returned no process; lock age ${r_age}s >= threshold ${REAP_AGE_S}s; reaped under $REAP_MUTEX"
          emit lock.reaped_utc "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
          # Moved, never deleted: the reaped lock's remains stay inspectable, and a rename can
          # never clobber files a peer wrote after our snapshot.
          emit lock.reaped_moved_to "$_reaped_to"
          _mk_err="$( { mkdir "$LOCK_DIR"; } 2>&1 )"; _mk_rc=$?
        else
          emit lock.reap_refused "reap-race-lost"
          emit lock.reap_refused_detail "the lock changed under us between the check and the rename"
        fi
      fi
      _wp_release_mutex
    else
      emit lock.reap_refused "reap-in-progress"
      emit lock.reap_refused_detail "another process holds the reap mutex $REAP_MUTEX"
    fi
  fi

  # B2 (box half): once this process owns the lock, ANY abnormal end must put the box back. An
  # ssh drop delivers SIGHUP here; without a trap the box is left LOCKED and UNLOADED with no
  # attestation, and the next gate refuses — a brick. EXIT is deliberately NOT trapped: a normal
  # exit is the handoff, where holding the lock is the whole point.
  #
  # N-4: the handler must RELOAD BEFORE RELEASING. Releasing alone left the box unlocked and NOT
  # SERVING — and silently, because the gate's own unwind then saw `not-held` and short-circuited
  # before its reload block ever ran. The box side unloaded the model, so the box side owns
  # putting it back; nobody downstream is in a position to.
  _WP_UNLOADED=0
  _wp_release_own() {
    if [ -f "$LOCK_DIR/pid" ] && [ "$(tr -d '[:space:]' < "$LOCK_DIR/pid" 2>/dev/null)" = "$$" ]; then
      if [ "$_WP_UNLOADED" = "1" ] && command -v qwen_reload >/dev/null 2>&1; then
        printf 'window-probe: reloading the serving model before releasing (signal)\n' >&2
        qwen_reload >/dev/null 2>&1 </dev/null \
          && printf 'window-probe: qwen_reload returned 0\n' >&2 \
          || printf 'window-probe: qwen_reload FAILED — box is NOT SERVING\n' >&2
      fi
      rm -f "$LOCK_DIR/holder" "$LOCK_DIR/pid" 2>/dev/null
      rmdir "$LOCK_DIR" 2>/dev/null
      printf 'window-probe: released own lock %s on signal\n' "$LOCK_DIR" >&2
    fi
    _wp_release_mutex
  }
  trap '_wp_release_own; exit 129' HUP
  trap '_wp_release_own; exit 130' INT
  trap '_wp_release_own; exit 143' TERM

  if [ "$_mk_rc" -eq 0 ]; then
    LOCK_ACQ_UTC="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    # pid FIRST: it is what the signal handler and the reaper key off, so the window in which
    # the lock exists but is unattributable must be as close to zero as possible.
    printf '%s\n' "$$" > "$LOCK_DIR/pid" 2>/dev/null
    { printf 'tag=%s\n' "$TAG"
      printf 'pid=%s\n' "$$"
      printf 'user=%s\n' "$(id -un 2>/dev/null)"
      printf 'acquired_utc=%s\n' "$LOCK_ACQ_UTC"
    } > "$LOCK_DIR/holder" 2>/dev/null
    emit lock.acquired "1"
    emit lock.acquired_utc "$LOCK_ACQ_UTC"
    emit lock.holder "$(cat "$LOCK_DIR/holder" 2>/dev/null)"
  else
    # Someone else holds it — either they always did, or they won the mkdir in the gap after our
    # reap. Report who, and STOP; the gate decides, and it never reaps on our behalf.
    emit lock.acquired "0"
    emit lock.create_failed "0"
    emit lock.holder "$(cat "$LOCK_DIR/holder" 2>/dev/null)"
    hpid=""; [ -f "$LOCK_DIR/pid" ] && hpid="$(tr -d '[:space:]' < "$LOCK_DIR/pid" 2>/dev/null)"
    emit lock.blocking_pid "$hpid"
    if [ -n "$hpid" ] && ps -p "$hpid" >/dev/null 2>&1; then emit lock.blocking_alive "1"
    else emit lock.blocking_alive "0"; fi
    trap - HUP INT TERM
    emit probe.ok "1"; exit 0
  fi

  # ---- qwen unload, under the lock we now hold ----
  # `qwen-service.sh` lives on the box and is SOURCED, not invoked: it defines qwen_unload /
  # qwen_reload as shell functions (run-paired-window.sh:212-218).
  QSVC="$(req qwen_service)"
  QPAT="$(req qwen_proc_pattern)"
  QTRIES="$(req_int qwen_unload_tries 12)"
  release_own_lock() {
    # Releases ONLY the lock this process just took, identified by the pid it wrote. It is
    # never used to clear anyone else's lock.
    _wp_release_own
    [ -d "$LOCK_DIR" ] || emit lock.released_on_abort "1"
  }
  if [ -n "$QSVC" ] && [ "$QSVC" != "none" ]; then
    emit qwen.service "$QSVC"
    if [ ! -f "$QSVC" ]; then
      emit qwen.unload_rc "no-service-file"; release_own_lock; emit probe.ok "1"; exit 0
    fi
    # shellcheck disable=SC1090  # the service file is a pinned box-side path, not in this repo.
    . "$QSVC"
    if ! command -v qwen_unload >/dev/null 2>&1 || ! command -v qwen_reload >/dev/null 2>&1; then
      emit qwen.unload_rc "service-missing-functions"; release_own_lock; emit probe.ok "1"; exit 0
    fi
    # NEVER capture a service function with `$(...)`. qwen_reload/qwen_unload legitimately
    # BACKGROUND a long-lived server process, and a background child inherits the command
    # substitution's stdout PIPE — so `$(...)` blocks until that server exits, which is to say
    # forever. Redirect to a file and read it back; a file fd the child inherits is harmless.
    # Mark the debt BEFORE incurring it. Setting this after the poll left a window — up to one
    # poll interval — in which the model was already down but a signal handler would not have
    # reloaded it. Erring toward an unnecessary reload is harmless; erring the other way leaves
    # a shared box silently not serving.
    _WP_UNLOADED=1
    _qf="$(mktemp "${TMPDIR:-/tmp}/wp-qw.XXXXXX")"
    qwen_unload >"$_qf" 2>&1 </dev/null; qrc=$?
    qout="$(cat "$_qf" 2>/dev/null)"; rm -f "$_qf"
    emit qwen.unload_rc "$qrc"; emit qwen.unload_out "$qout"
    # rc=0 is not proof: poll until the process is actually gone (official-lib.sh:320-329).
    tries=0; gone=0
    while [ "$tries" -lt "$QTRIES" ]; do
      if [ -z "$(pgrep -f "$QPAT" 2>/dev/null | tr -d '[:space:]')" ]; then gone=1; break; fi
      sleep 1; tries=$((tries + 1))
    done
    emit qwen.unloaded "$gone"
    if [ "$gone" != "1" ]; then release_own_lock; emit probe.ok "1"; exit 0; fi
  else
    emit qwen.service "declared-none"; emit qwen.unloaded "declared-none"
  fi

  # ---- the smoke leg, under the lock, with qwen unloaded ----
  SMOKE_ARGV="$(req smoke_argv)"
  if [ -n "$SMOKE_ARGV" ]; then
    emit smoke.pinned "1"; emit smoke.argv "$SMOKE_ARGV"
    smoke_out="$(mktemp "${TMPDIR:-/tmp}/wp-so.XXXXXX")"
    smoke_err="$(mktemp "${TMPDIR:-/tmp}/wp-se.XXXXXX")"
    smoke_timeout="$(req_int smoke_timeout_s 300)"
    smoke_t0="$(date -u +%s)"
    emit smoke.started_utc "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    # Stock macOS has no `timeout(1)`: run in the background and reap with a poll loop, so a
    # worker that HANGS at the handshake fails the gate instead of hanging the window prep.
    #
    # B-7: `set -m` puts the child in its OWN PROCESS GROUP so the timeout can kill the whole
    # tree with `kill -- -PGID`. Killing only the eval subshell leaves benchd AND the worker
    # running, and a surviving engine holds GPU memory straight into the next session's window
    # — worse than the hang it was meant to cure.
    set -m
    ( eval "$SMOKE_ARGV" ) >"$smoke_out" 2>"$smoke_err" &
    smoke_pid=$!
    set +m
    waited=0; smoke_rc=""
    while [ "$waited" -lt "$smoke_timeout" ]; do
      if ! kill -0 "$smoke_pid" 2>/dev/null; then wait "$smoke_pid"; smoke_rc=$?; break; fi
      sleep 1; waited=$((waited + 1))
    done
    if [ -z "$smoke_rc" ]; then
      # TERM the GROUP first (benchd gets a chance to tear its worker down), then KILL it.
      kill -TERM -- "-$smoke_pid" 2>/dev/null || kill -TERM "$smoke_pid" 2>/dev/null
      sleep 2
      kill -9 -- "-$smoke_pid" 2>/dev/null || kill -9 "$smoke_pid" 2>/dev/null
      wait "$smoke_pid" 2>/dev/null
      emit smoke.timed_out "1"; emit smoke.rc "124"
    else
      emit smoke.timed_out "0"; emit smoke.rc "$smoke_rc"
    fi
    # Timed out or not, SAY whether anything survived. A stray engine holding GPU memory must
    # be discovered by THIS window, not by the next one.
    # N-d: EXACT-name, matching quiet.stray_* above. `pgrep -f` matches the whole command line,
    # so it reports any process that merely MENTIONS these names — an editor, a grep, this
    # suite's own fixtures — and a stray check that cries wolf gets ignored, which is worse than
    # not having one.
    emit smoke.stray_after "$( { pgrep -x mlxfast-runtime-worker 2>/dev/null
                                 pgrep -x mlxfast-engine 2>/dev/null; } | tr '\n' ' ')"
    emit smoke.elapsed_s "$(( $(date -u +%s) - smoke_t0 ))"
    emit_file smoke.stdout "$smoke_out"
    emit_file smoke.stderr "$smoke_err"
    SMOKE_WORKER_STDERR="$(req smoke_worker_stderr)"
    if [ -n "$SMOKE_WORKER_STDERR" ] && [ -f "$SMOKE_WORKER_STDERR" ]; then
      emit smoke.worker_stderr_path "$SMOKE_WORKER_STDERR"
      emit_file smoke.worker_stderr "$SMOKE_WORKER_STDERR"
    else
      emit smoke.worker_stderr_path ""; emit smoke.worker_stderr ""
    fi
    rm -f "$smoke_out" "$smoke_err"
  else
    emit smoke.pinned "0"
  fi
  # The lock is deliberately STILL HELD on the way out. Releasing it is `mode=release`'s job,
  # and the gate calls that either on failure (via its trap) or after the window is done.
  trap - HUP INT TERM
  emit probe.ok "1"; exit 0
fi

# ============================================================================
# MODE: release — RELOAD QWEN, VERIFY SERVING, RELEASE THE LOCK
# ============================================================================
if [ "$MODE" = "release" ]; then
  LOCK_DIR="$(req session_lock)"
  TAG="$(req window_tag)"
  QSVC="$(req qwen_service)"
  QPAT="$(req qwen_proc_pattern)"
  QTRIES="$(req_int qwen_reload_tries 60)"
  emit lock.path "$LOCK_DIR"; emit lock.tag "$TAG"

  # OWNERSHIP FIRST. A release must never remove a lock this session did not take: the holder
  # record's tag is the proof of ownership, and without a match we report and leave it alone.
  held_tag=""
  [ -f "$LOCK_DIR/holder" ] && held_tag="$(sed -n 's/^tag=//p' "$LOCK_DIR/holder" | head -1)"
  emit lock.held_tag "$held_tag"
  if [ ! -d "$LOCK_DIR" ]; then
    emit lock.release_verdict "not-held"; emit probe.ok "1"; exit 0
  fi
  if [ "$held_tag" != "$TAG" ]; then
    emit lock.release_verdict "not-ours"; emit probe.ok "1"; exit 0
  fi

  # Reload BEFORE releasing: the box must be back in its serving state before the next session
  # can take the lock, or the release hands over a box that is quietly not serving.
  if [ -n "$QSVC" ] && [ "$QSVC" != "none" ] && [ ! -f "$QSVC" ]; then
    # X-2: a PINNED qwen service file that is not on the box is a BROKEN PIN, not the declared
    # `none` waiver. Reporting it as `declared-none` put it in the gate's OK list, so --release
    # printed OK and exited 0 having reloaded nothing — the box handed on with serving DOWN, by
    # the same fail-open shape as A-2, through a different door. Window mode already fails closed
    # on exactly this condition; the two modes must not disagree about one pin.
    emit qwen.service_path "$QSVC"
    emit qwen.reload_rc "no-service-file"
    emit qwen.reloaded  "no-service-file"
  elif [ -n "$QSVC" ] && [ "$QSVC" != "none" ] && [ -f "$QSVC" ]; then
    # shellcheck disable=SC1090
    . "$QSVC"
    if command -v qwen_reload >/dev/null 2>&1; then
      _rf="$(mktemp "${TMPDIR:-/tmp}/wp-qr.XXXXXX")"
      qwen_reload >"$_rf" 2>&1 </dev/null; rrc=$?
      rout="$(cat "$_rf" 2>/dev/null)"; rm -f "$_rf"
      emit qwen.reload_rc "$rrc"; emit qwen.reload_out "$rout"
      tries=0; back=0
      while [ "$tries" -lt "$QTRIES" ]; do
        if [ -n "$(pgrep -f "$QPAT" 2>/dev/null | tr -d '[:space:]')" ]; then back=1; break; fi
        sleep 1; tries=$((tries + 1))
      done
      emit qwen.reloaded "$back"
      HURL="$(req qwen_health_url)"
      if [ -n "$HURL" ] && command -v curl >/dev/null 2>&1; then
        emit qwen.health_out "$(curl -s -m 10 "$HURL" 2>&1)"
      fi
    else
      # Emit the verdict field too. A branch that sets reload_rc but leaves qwen.reloaded EMPTY
      # forces every reader to infer failure from an absence, and an absence is exactly what the
      # OK path also looks like if anyone ever adds a default.
      emit qwen.reload_rc "service-missing-functions"
      emit qwen.reloaded  "service-missing-functions"
    fi
  else
    emit qwen.reload_rc "declared-none"; emit qwen.reloaded "declared-none"
  fi

  rm -f "$LOCK_DIR/holder" "$LOCK_DIR/pid" 2>/dev/null
  if rmdir "$LOCK_DIR" 2>/dev/null; then
    emit lock.release_verdict "released"
    emit lock.released_utc "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  else
    emit lock.release_verdict "release-failed"
  fi
  emit probe.ok "1"; exit 0
fi

emit probe.ok "1"
exit 0
