#!/usr/bin/env bash
#
# test-dist-manifest.sh -- the dist channel manifest, offline.
#
# The channel now carries more than one binary, and the manifest that describes
# them is read on ranked boxes by scripts nobody redeploys in step with this
# repository. So the two things this test holds down are:
#
#   1. THE MANIFEST DESCRIBES EVERY PUBLISHED BINARY. One entry per name in
#      DIST_BINARIES, each with the sha256 and the byte count of the file.
#   2. AN OLD CONSUMER STILL READS AN OLD MANIFEST OUT OF THE NEW ONE. The
#      LEGACY sed expression is written out verbatim below -- not taken from
#      scripts/dist-lib.sh -- so that changing the library cannot quietly
#      change what "the old parser" means. It must return exactly ONE value for
#      each of the six original fields, and that value must still describe
#      `benchd`.
#
# The negative control is the point of (2): the same check is run against a
# PRETTY-PRINTED manifest, where the per-binary objects are spread over several
# lines, and it must FAIL. That is what makes the one-line rule in
# scripts/dist-lib.sh load-bearing rather than a style preference.
#
# No cargo build, no network, no binaries are published: the fixtures are two
# small files with known digests.
#
# A PASS IS COUNTED, NOT ASSUMED. This runs on macOS, whose /bin/bash is 3.2,
# and a construct a newer bash accepts can abort 3.2 mid-script -- which under
# some callers still leaves exit 0, so the checks after the abort are skipped
# and nothing says so. Two things make that impossible to mistake for a pass:
# the script counts every check it runs and asserts the total at the end, and an
# EXIT trap refuses any exit that did not reach that assertion. Nothing here may
# use bash-4 syntax (no negative array subscripts, no associative arrays).
#
#   ./scripts/test-dist-manifest.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/dist-lib.sh
. "${REPO_ROOT}/scripts/dist-lib.sh"

TMP_DIR="$(mktemp -d)"
completed=0
trap 'rc=$?; rm -rf "${TMP_DIR}"; \
      if [[ "${completed}" -ne 1 ]]; then \
        echo "test-dist-manifest.sh: ABORTED before the final assertion (exit ${rc}); this is NOT a pass." >&2; \
        exit 1; \
      fi; \
      exit "${rc}"' EXIT

fails=0
ran=0
pass() { ran=$((ran + 1)); echo "  ok    $*"; }
fail() { ran=$((ran + 1)); echo "  FAIL  $*" >&2; fails=$((fails + 1)); }
check() { # check <label> <expected> <actual>
  if [[ "$2" == "$3" ]]; then pass "$1"; else fail "$1: expected '$2', got '$3'"; fi
}

N_BINARIES="${#DIST_BINARIES[@]}"
LAST_BINARY="${DIST_BINARIES[$((N_BINARIES - 1))]}"

# The number of checks each section runs, as a claim to be checked at the end.
# A section that stops early takes the total with it.
EXPECT_ROSTER=2                                  # + one per binary if cargo is present
EXPECT_MANIFEST=$((1 + 3 * N_BINARIES))          # valid JSON, then sha/bytes/one-line each
EXPECT_COMPAT=7                                  # six legacy fields + the negative control
EXPECT_VERIFY=3

# THE LEGACY READER, verbatim as the consumers spell it (the engine repo's
# tools/fetch-benchd.sh `manifest_field`, and this repo's own scripts before the
# roster existed). Anchored, one key per line.
legacy_field() {
  sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",]*\)\"\{0,1\}[[:space:]]*,\{0,1\}[[:space:]]*\$/\1/p" "$1"
}

echo "roster"
# The channel must carry the golden author. An engine re-authors its track
# goldens with it, and the box must not build its own harness.
case " ${DIST_BINARIES[*]} " in
  *" record-correctness-golden "*) pass "DIST_BINARIES carries record-correctness-golden" ;;
  *) fail "DIST_BINARIES does not carry record-correctness-golden: ${DIST_BINARIES[*]}" ;;
esac
check "benchd is first (the six top-level fields describe it)" "benchd" "${DIST_BINARIES[0]}"

# Every roster name must be a real cargo bin target, or the build fails only at
# publish time, on the release machine.
cargo_checked=0
if command -v cargo >/dev/null 2>&1; then
  cargo_checked=1
  cargo_bins="$(cargo metadata --no-deps --format-version 1 --manifest-path "${REPO_ROOT}/Cargo.toml" \
    | python3 -c 'import json,sys
print(" ".join(t["name"] for p in json.load(sys.stdin)["packages"] for t in p["targets"] if "bin" in t["kind"]))')"
  for name in "${DIST_BINARIES[@]}"; do
    case " ${cargo_bins} " in
      *" ${name} "*) pass "cargo has a bin target named ${name}" ;;
      *) fail "no cargo bin target named ${name} (cargo build --bin ${name} would fail)" ;;
    esac
  done
else
  echo "  skip  cargo is not on PATH; bin-target names unchecked"
fi

echo "manifest"
BIN_DIR="${TMP_DIR}/dist"
mkdir -p "${BIN_DIR}"
declare -a SHAS BYTES_
for i in "${!DIST_BINARIES[@]}"; do
  name="${DIST_BINARIES[i]}"
  # Distinct contents, so a writer that mixed two entries up is visible.
  printf 'fixture bytes for %s %s\n' "${name}" "$(printf '%*s' "$((i + 1))" '' | tr ' ' 'x')" \
    > "${BIN_DIR}/${name}"
  SHAS[i]="$(dist_sha256 "${BIN_DIR}/${name}")"
  BYTES_[i]="$(dist_bytes "${BIN_DIR}/${name}")"
done

MANIFEST="${TMP_DIR}/benchd.manifest.json"
COMMIT="0123456789abcdef0123456789abcdef01234567"
dist_manifest_write "${MANIFEST}" "${BIN_DIR}" "0.0.0" "qwen3.8-125b-a6b-v1" "${COMMIT}" "aarch64-apple-darwin"

if python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "${MANIFEST}"; then
  pass "the manifest is valid JSON"
else
  fail "the manifest is not valid JSON"
fi

for i in "${!DIST_BINARIES[@]}"; do
  name="${DIST_BINARIES[i]}"
  check "binaries.${name}.sha256" "${SHAS[i]}" "$(dist_manifest_binary_field "${MANIFEST}" "${name}" sha256)"
  check "binaries.${name}.bytes" "${BYTES_[i]}" "$(dist_manifest_binary_field "${MANIFEST}" "${name}" bytes)"
done

# One line per entry: this is what keeps the legacy reader correct.
for name in "${DIST_BINARIES[@]}"; do
  check "binaries.${name} is one line" "1" "$(grep -c "\"${name}\": {" "${MANIFEST}" | tr -d '[:space:]')"
done

echo "backward compatibility (the pre-roster reader)"
for key in version branch source_commit target_triple sha256 bytes; do
  value="$(legacy_field "${MANIFEST}" "${key}")"
  lines="$(printf '%s' "${value}" | grep -c '' || true)"
  if [[ "${lines}" != "1" ]]; then
    fail "legacy read of '${key}' returned ${lines} values, not 1: '${value}'"
    continue
  fi
  case "${key}" in
    version) check "legacy version" "0.0.0" "${value}" ;;
    branch) check "legacy branch" "qwen3.8-125b-a6b-v1" "${value}" ;;
    source_commit) check "legacy source_commit" "${COMMIT}" "${value}" ;;
    target_triple) check "legacy target_triple" "aarch64-apple-darwin" "${value}" ;;
    # The six top-level fields still describe benchd, and only benchd.
    sha256) check "legacy sha256 is benchd's" "${SHAS[0]}" "${value}" ;;
    bytes) check "legacy bytes is benchd's" "${BYTES_[0]}" "${value}" ;;
  esac
done

# NEGATIVE CONTROL. Pretty-print the same content and the legacy reader must
# break -- three sha256 lines instead of one. If this passes, the one-line rule
# is not what is protecting the old consumers and the test above proves nothing.
PRETTY="${TMP_DIR}/pretty.manifest.json"
python3 -c 'import json,sys; json.dump(json.load(open(sys.argv[1])), open(sys.argv[2],"w"), indent=2)' \
  "${MANIFEST}" "${PRETTY}"
pretty_sha_lines="$(legacy_field "${PRETTY}" sha256 | grep -c '' || true)"
if [[ "${pretty_sha_lines}" == "1" ]]; then
  fail "negative control: a pretty-printed manifest ALSO reads cleanly, so the one-line rule is not what protects the old reader"
else
  pass "negative control: pretty-printing breaks the legacy reader (${pretty_sha_lines} sha256 values), so the one-line rule is load-bearing"
fi

echo "verification"
if dist_manifest_verify "${MANIFEST}" "${BIN_DIR}" 2>/dev/null; then
  pass "dist_manifest_verify accepts the described binaries"
else
  fail "dist_manifest_verify rejected the binaries its own manifest describes"
fi

CORRUPT_DIR="${TMP_DIR}/corrupt"
cp -R "${BIN_DIR}" "${CORRUPT_DIR}"
# The LAST roster entry, spelled without a negative subscript: bash 3.2 aborts
# on ${array[-1]}. Tamper with the last one because it is the entry the six
# top-level fields do NOT describe -- the one a benchd-only check misses.
printf 'tampered\n' >> "${CORRUPT_DIR}/${LAST_BINARY}"
if dist_manifest_verify "${MANIFEST}" "${CORRUPT_DIR}" 2>/dev/null; then
  fail "dist_manifest_verify accepted a tampered ${LAST_BINARY}"
else
  pass "dist_manifest_verify refuses a tampered ${LAST_BINARY}"
fi

SHORT_DIR="${TMP_DIR}/short"
mkdir -p "${SHORT_DIR}"
cp "${BIN_DIR}/${DIST_BINARIES[0]}" "${SHORT_DIR}/"
if dist_manifest_write "${TMP_DIR}/short.manifest.json" "${SHORT_DIR}" \
     "0.0.0" "b" "${COMMIT}" "aarch64-apple-darwin" 2>/dev/null; then
  fail "dist_manifest_write wrote a manifest with a roster binary missing"
else
  pass "dist_manifest_write refuses to describe an incomplete channel"
fi

echo
expected=$((EXPECT_ROSTER + EXPECT_MANIFEST + EXPECT_COMPAT + EXPECT_VERIFY))
if [[ "${cargo_checked}" -eq 1 ]]; then
  expected=$((expected + N_BINARIES))
fi
if [[ "${ran}" -ne "${expected}" ]]; then
  echo "test-dist-manifest.sh: ran ${ran} checks, expected ${expected} -- a section did not run to the end, so a silent skip is being reported as a result" >&2
  exit 1
fi
if [[ "${fails}" -ne 0 ]]; then
  echo "test-dist-manifest.sh: ${fails} of ${ran} check(s) failed" >&2
  exit 1
fi
completed=1
echo "test-dist-manifest.sh: all ${ran} checks passed"
