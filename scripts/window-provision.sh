#!/bin/bash
# scripts/window-provision.sh — SYNC the box to the expected pins, so the gate has something
# to pass. The companion to scripts/window-preflight.sh, and deliberately a SEPARATE motion:
# the gate never assumes provisioning ran, and provisioning never reports a verdict.
#
# THREE RULES, IN ORDER OF IMPORTANCE:
#
#   1. NEVER SWITCH A CHECKOUT SOMEONE ELSE OWNS. If the pinned path already exists at the
#      wrong commit, this script REFUSES and says so. It does not `git checkout`, it does not
#      `git reset`, it does not `git pull`. The Proof A precedent is the pattern: build in a
#      separate worktree (`~/wt-proofA-bench`, `~/wt-proofA-engine`) and leave the box's own
#      checkout alone. Someone else may be mid-window in that tree.
#   2. NEVER DELETE ANYTHING. No `rm`, no `git worktree remove`, no `git worktree prune`, no
#      `git clean`. If something is in the way, that is a fact for a human, not a thing to
#      tidy. This holds even when the obstruction looks obviously stale.
#   3. A BUNDLE IS ONLY LEGAL WITH A PINNED HASH. The box has no GitHub credentials, so
#      shipping a tree as a git bundle is a supported path — but the bundle's sha256 must be
#      pinned in advance, is verified on BOTH sides of the wire, and is recorded into
#      `.window-bundle-provenance` inside the provisioned tree so the gate can read it back.
#      Without the pin this script refuses to ship anything.
#
# USAGE
#   scripts/window-provision.sh --pins <FILE> [--box <ALIAS>] [--driver ssh|local] [--dry-run]
#
# PINS CONSUMED (beyond the gate's own)
#   WP_BENCH_SOURCE_CLONE  / WP_ENGINE_SOURCE_CLONE   an existing clone ON THE BOX to add a
#                                                     detached worktree from (preferred path)
#   WP_BENCH_BUNDLE        / WP_ENGINE_BUNDLE         a git bundle ON THE LAPTOP to ship
#   WP_BENCH_BUNDLE_SHA256 / WP_ENGINE_BUNDLE_SHA256  REQUIRED whenever a bundle is used
#   WP_BENCH_BUILD_CMD     / WP_ENGINE_BUILD_CMD      build command run inside the tree, or the
#                                                     literal `none` to declare "no build"
#
# EXIT CODES  0 provisioned (or already correct) · 2 usage/missing pin · 5 could not provision
#             7 REFUSED (would have to touch a tree someone else owns, or an unpinned bundle)
#             9 transport error
set -uo pipefail

E_OK=0; E_USAGE=2; E_MISSING=5; E_REFUSED=7; E_TRANSPORT=9
die_usage() { printf 'window-provision: %s\n' "$1" >&2; exit "$E_USAGE"; }

OVERRIDES=""
PINS_FILE=""; DRIVER=""; BOX=""; DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --pins)    PINS_FILE="${2-}"; shift 2 ;;
    --driver)  DRIVER="${2-}"; shift 2 ;;
    --box)     BOX="${2-}"; shift 2 ;;
    # N-e: the gate forwards its CLI overrides here. Without this, `--provision` provisioned from
    # the FILE while the gate then verified against the OVERRIDDEN pins, and the disagreement
    # surfaced as a baffling digest failure instead of the real cause.
    --pin)     OVERRIDES="$OVERRIDES
${2-}"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) sed -n '2,40p' "$0"; exit 0 ;;
    *)         die_usage "unknown argument: $1" ;;
  esac
done
[ -n "$PINS_FILE" ] || die_usage "--pins <FILE> is required"
[ -r "$PINS_FILE" ] || die_usage "pins file not readable: $PINS_FILE"

# Last assignment wins, exactly as in window-preflight.sh, so an override appended here beats the
# file. The two must agree about precedence or `--provision` builds a tree the gate then rejects.
PINS_RAW="$(grep -v '^[[:space:]]*#' "$PINS_FILE" | grep -v '^[[:space:]]*$')
$OVERRIDES"
pin() {
  local k="$1" line v=""
  while IFS= read -r line; do
    case "$line" in "$k="*) v="${line#"$k"=}" ;; esac
  done <<EOF
$PINS_RAW
EOF
  printf '%s' "$v"
}
# must <VALUE> <KEY> — validate AFTER assignment. An `exit` inside a `$(...)` only kills the
# subshell, so a `V="$(require KEY)"` helper would leave V empty and carry on regardless.
must() { [ -n "$1" ] || die_usage "required pin $2 is not set"; }

[ -n "$DRIVER" ] || DRIVER="$(pin WP_DRIVER)"; [ -n "$DRIVER" ] || DRIVER="ssh"
[ -n "$BOX" ]    || BOX="$(pin WP_BOX)"
[ "$DRIVER" = "ssh" ] && [ -z "$BOX" ] && die_usage "--box <ALIAS> is required under DRIVER=ssh"

b64() { base64 | tr -d '\n'; }
sha_of() { shasum -a 256 "$1" | awk '{print $1}'; }

# rexec <script-file> <arg-b64>... — run a script on the box (or locally), streaming it in on
# stdin so nothing is left behind.
#
# A-3: base64-wrapping each argument is NOT sufficient over ssh. ssh joins argv with spaces into
# ONE string that the remote login shell re-splits, so an EMPTY positional simply VANISHES and
# every argument after it shifts down one. This script passes empties routinely (no source
# clone, no bundle, no bundle sha), and the consequences were silent and severe: `--dry-run`
# landed in $6 and was read as the BUILD COMMAND while $8 (dry) read empty, so a dry run became
# a REAL run; the pinned build command was skipped but reported DECLARED-NONE; and the bundle
# branch was unreachable, so the box-side re-verification never ran at all — all at exit 0.
#
# The fix is to send ONE argument: a base64 blob of the newline-joined, individually-base64'd
# fields. Nothing the remote shell can split, and an empty field survives as an empty line.
rexec() {
  local script="$1"; shift
  local packed=""
  for a in "$@"; do packed="$packed$a
"; done
  packed="$(printf '%s' "$packed" | base64 | tr -d '\n')"
  if [ "$DRIVER" = "local" ]; then bash "$script" "$packed"
  else ssh "$BOX" bash -s -- "$packed" <"$script"; fi
}

# Remove a box-side temp THIS script allocated. Safe to delete: freshly created by us under an
# unpredictable name, so it is not someone else's state — and leaking one per failed attempt is
# how a shared box fills up with orphans.
_prov_rm_temp() { [ -n "${1-}" ] && ssh "$BOX" "rm -f -- \"$1\"" >/dev/null 2>&1; return 0; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/window-provision.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# The box-side payload. It is written once and reused for both roles.
PAYLOAD="$WORK/provision-remote.sh"
cat > "$PAYLOAD" <<'REMOTE'
#!/bin/bash
# Box-side provisioning payload. ONE argument: base64 of a newline-joined list of individually
# base64'd fields, in order: path sha source_clone bundle_path bundle_sha build_cmd role dry.
# One argument, because ssh flattens argv and empty positionals vanish (A-3).
set -uo pipefail
if printf 'eA==' | base64 -d >/dev/null 2>&1; then D="-d"; else D="-D"; fi
u() { printf '%s' "$1" | base64 "$D" 2>/dev/null; }
_unpacked="$(u "${1-}")"
_i=0
while IFS= read -r _line; do
  _i=$((_i + 1))
  case "$_i" in
    1) PATH_="$(u "$_line")" ;; 2) SHA="$(u "$_line")" ;;
    3) SRC="$(u "$_line")"   ;; 4) BUNDLE="$(u "$_line")" ;;
    5) BSHA="$(u "$_line")"  ;; 6) BUILD="$(u "$_line")" ;;
    7) ROLE="$(u "$_line")"  ;; 8) DRY="$(u "$_line")" ;;
  esac
done <<UNPACK
$_unpacked
UNPACK
: "${PATH_:=}" "${SHA:=}" "${SRC:=}" "${BUNDLE:=}" "${BSHA:=}" "${BUILD:=}" "${ROLE:=}" "${DRY:=}"
say() { printf '  [%s] %s\n' "$ROLE" "$1"; }
run() { if [ "$DRY" = "1" ]; then say "DRY-RUN: $*"; return 0; fi; "$@"; }

if [ -e "$PATH_" ]; then
  if ! git -C "$PATH_" rev-parse --git-dir >/dev/null 2>&1; then
    say "REFUSE: $PATH_ exists but is not a git repository. Nothing is deleted here — clear it deliberately or pin a different path."
    exit 7
  fi
  head="$(git -C "$PATH_" rev-parse HEAD 2>/dev/null)"
  if [ "$head" = "$SHA" ]; then
    say "already at the pinned commit ($SHA) — nothing to do"
  else
    # Rule 1. This is the whole point of the script's existence as a separate motion.
    say "REFUSE: $PATH_ is checked out at $head, not the pinned $SHA."
    say "        This script will NOT switch a checkout that someone else may own — the box's"
    say "        main checkout stays exactly as it is. Pin a fresh worktree path instead."
    exit 7
  fi
else
  if [ -n "$SRC" ]; then
    # Preferred path: a detached worktree off an existing clone. Additive, reversible by the
    # operator, and it never moves the source clone's own HEAD.
    if [ ! -d "$SRC" ]; then say "REFUSE: source clone $SRC is not on the box"; exit 5; fi
    say "fetching in $SRC"
    run git -C "$SRC" fetch --all --tags --quiet || { say "fetch failed"; exit 5; }
    if ! git -C "$SRC" cat-file -e "${SHA}^{commit}" 2>/dev/null && [ "$DRY" != "1" ]; then
      say "REFUSE: commit $SHA is not reachable in $SRC after fetching"; exit 5
    fi
    say "adding detached worktree $PATH_ @ $SHA"
    run git -C "$SRC" worktree add --detach "$PATH_" "$SHA" || { say "worktree add failed"; exit 5; }
  elif [ -n "$BUNDLE" ]; then
    # Rule 3. The bundle has already been sha-verified on the laptop; verify again HERE, on the
    # bytes that actually landed, because the wire is part of the trust chain too.
    if [ -z "$BSHA" ]; then say "REFUSE: bundle with no pinned sha256"; exit 7; fi
    # N-3: under --dry-run nothing was shipped, so there is no file here to digest. Digesting
    # the absent path yielded an empty hash and a FABRICATED "REFUSE: sha256 mismatch" — a
    # dry run inventing a failure that the real run would not have had.
    if [ "$DRY" = "1" ]; then
      say "DRY-RUN: would verify the bundle sha256 on the box against $BSHA"
    else
      got="$(shasum -a 256 "$BUNDLE" | awk '{print $1}')"
      if [ "$got" != "$BSHA" ]; then
        say "REFUSE: bundle sha256 on the box is $got, pinned $BSHA"; exit 7
      fi
      say "bundle sha256 verified on the box ($got)"
    fi
    say "cloning from bundle -> $PATH_"
    run git clone --quiet "$BUNDLE" "$PATH_" || { say "clone from bundle failed"; exit 5; }
    run git -C "$PATH_" checkout --quiet --detach "$SHA" || { say "checkout $SHA failed"; exit 5; }
    # Recorded, not assumed: the gate reads this file back and refuses the tree without it.
    # It goes in the GIT DIR, never the working tree — a provenance record that made the tree
    # dirty would fail the very clean-tree assertion it exists to support.
    if [ "$DRY" != "1" ]; then
      gd="$(git -C "$PATH_" rev-parse --absolute-git-dir 2>/dev/null)"
      { printf 'bundle_sha256=%s\n' "$BSHA"
        printf 'bundle_path=%s\n' "$BUNDLE"
        printf 'commit=%s\n' "$SHA"
        printf 'provisioned_utc=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
      } > "$gd/window-bundle-provenance"
    fi
    say "recorded .window-bundle-provenance"
  else
    say "REFUSE: $PATH_ is absent and neither a source clone nor a pinned bundle was supplied"
    exit 5
  fi
fi

if [ -n "$BUILD" ] && [ "$BUILD" != "none" ]; then
  say "building: $BUILD"
  if [ "$DRY" = "1" ]; then say "DRY-RUN: (build skipped)"
  else
    ( cd "$PATH_" && eval "$BUILD" ) || { say "build FAILED"; exit 5; }
    say "build ok"
  fi
else
  say "build: DECLARED-NONE"
fi
exit 0
REMOTE

provision_role() { # <ROLE> <path_pin> <sha_pin>
  local role="$1" path sha src bundle bsha build lb
  path="$(pin "WP_${role}_PATH")"; must "$path" "WP_${role}_PATH"
  sha="$(pin "WP_${role}_SHA")";   must "$sha"  "WP_${role}_SHA"
  src="$(pin "WP_${role}_SOURCE_CLONE")"
  bundle="$(pin "WP_${role}_BUNDLE")"
  bsha="$(pin "WP_${role}_BUNDLE_SHA256")"
  build="$(pin "WP_${role}_BUILD_CMD")"
  [ -n "$build" ] || die_usage "required pin WP_${role}_BUILD_CMD is not set (use the literal 'none' to declare that this tree needs no build)"


  printf '== provisioning %s ==\n' "$role"

  # If a bundle is named, it is verified and shipped BEFORE the box-side payload runs — an
  # unpinned or mismatched bundle never reaches the wire at all.
  if [ -n "$bundle" ] && [ ! -e "$bundle" ]; then
    printf '  [%s] bundle not found on the laptop: %s\n' "$role" "$bundle" >&2; return $E_MISSING
  fi
  lb=""
  if [ -n "$bundle" ]; then
    if [ -z "$bsha" ]; then
      printf '  [%s] REFUSE: WP_%s_BUNDLE is set but WP_%s_BUNDLE_SHA256 is not.\n' "$role" "$role" "$role" >&2
      printf '  [%s]         A bundle without a pinned content hash is an unprovenanced tree.\n' "$role" >&2
      return $E_REFUSED
    fi
    local got; got="$(sha_of "$bundle")"
    if [ "$got" != "$bsha" ]; then
      printf '  [%s] REFUSE: bundle sha256 is %s, pinned %s — not shipping it.\n' "$role" "$got" "$bsha" >&2
      return $E_REFUSED
    fi
    printf '  [%s] bundle sha256 verified on the laptop (%s)\n' "$role" "$got"
    if [ "$DRIVER" = "local" ]; then
      lb="$bundle"
    else
      # C-8: a fixed, world-guessable /tmp path invites a symlink swap between the scp and the
      # box-side re-digest. mktemp on the box gives an unpredictable name in a dir we own.
      #
      # N-2: BSD/macOS mktemp requires the template to END in X's. `.XXXXXX.bundle` is not
      # expanded at all — BSD takes it as a LITERAL filename and creates it exclusively. So the
      # old form did one of two bad things depending on the box:
      #   * first ever run: succeeded, returning the fully PREDICTABLE, world-guessable path
      #     `<tmp>/window-provision.XXXXXX.bundle` — defeating the exact unpredictability C-8
      #     added it for; or
      #   * every run after that: `mkstemp failed: File exists`, empty stdout, and the whole
      #     bundle-over-ssh path (Proof A's own case) died at exit 9 having shipped nothing.
      # GNU mktemp expands the suffix form happily, which is why this survived review on Linux.
      #
      # Allocate with TRAILING X's, then rename to add the suffix.
      #
      # First, surface the old bug's fingerprint if this box carries it. It is inert for the new
      # template, but it is cruft at a guessable path, and NOT ours to remove: deleting a
      # world-guessable path is precisely the symlink-swap hazard, and it may no longer be the
      # file our old code made. Report it and let a human decide.
      _stale="$(ssh "$BOX" 'p="${TMPDIR:-/tmp}/window-provision.XXXXXX.bundle"; [ -e "$p" ] && printf %s "$p"' 2>/dev/null)"
      if [ -n "$_stale" ]; then
        printf '  [%s] NOTE: a leftover from the pre-fix mktemp template is on the box:\n' "$role" >&2
        printf '  [%s]       %s\n' "$role" "$_stale" >&2
        printf '  [%s]       It is unused now and NOT removed here (a world-guessable path is not\n' "$role" >&2
        printf '  [%s]       safe to delete blind). Clear it deliberately.\n' "$role" >&2
      fi

      lb="$(ssh "$BOX" 'mktemp "${TMPDIR:-/tmp}/window-provision.XXXXXX"' 2>/dev/null)"
      [ -n "$lb" ] || { printf '  [%s] could not allocate a temp path on the box\n' "$role" >&2; return $E_TRANSPORT; }
      # From here the temp EXISTS on the box, so every failure path below must remove it. It is
      # ours — freshly created, unpredictable name — so cleaning it up is not someone else's
      # state, and leaking one per failed attempt is how a box fills with orphans.
      if ssh "$BOX" "mv -- \"$lb\" \"$lb.bundle\"" >/dev/null 2>&1; then
        lb="$lb.bundle"
      else
        printf '  [%s] could not name the box-side bundle path\n' "$role" >&2
        _prov_rm_temp "$lb"
        return $E_TRANSPORT
      fi
      if [ "$DRY" = "1" ]; then printf '  [%s] DRY-RUN: scp bundle -> %s:%s\n' "$role" "$BOX" "$lb"
      else
        if ! scp -q "$bundle" "$BOX:$lb"; then
          printf '  [%s] scp of the bundle failed\n' "$role" >&2
          _prov_rm_temp "$lb"
          return $E_TRANSPORT
        fi
        printf '  [%s] shipped bundle -> %s:%s\n' "$role" "$BOX" "$lb"
      fi
    fi
  fi

  local _rc=0
  rexec "$PAYLOAD" \
    "$(printf '%s' "$path"   | b64)" "$(printf '%s' "$sha"    | b64)" \
    "$(printf '%s' "$src"    | b64)" "$(printf '%s' "$lb"     | b64)" \
    "$(printf '%s' "$bsha"   | b64)" "$(printf '%s' "$build"  | b64)" \
    "$(printf '%s' "$role"   | b64)" "$(printf '%s' "$DRY"    | b64)" || _rc=$?
  # The shipped bundle is a temp we created; it has served its purpose either way.
  if [ -n "$lb" ] && [ "$DRIVER" != "local" ]; then _prov_rm_temp "$lb"; fi
  return $_rc
}

RC=0
provision_role BENCH  || RC=$?
[ "$RC" -eq 0 ] || { printf 'window-provision: FAILED on BENCH (rc=%s)\n' "$RC" >&2; exit "$RC"; }
provision_role ENGINE || RC=$?
[ "$RC" -eq 0 ] || { printf 'window-provision: FAILED on ENGINE (rc=%s)\n' "$RC" >&2; exit "$RC"; }

printf 'window-provision: done. Nothing was deleted and no existing checkout was switched.\n'
printf 'Run scripts/window-preflight.sh next — provisioning is not a verdict.\n'
exit $E_OK
