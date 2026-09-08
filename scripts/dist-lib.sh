#!/usr/bin/env bash
#
# dist-lib.sh -- the channel roster, the manifest writer and the manifest
# readers, in one place.
#
# The dist channel used to carry one binary, so "the manifest" and "the
# benchd pin" were the same six fields and every consumer could hardcode
# that. The channel now carries MORE THAN ONE binary from the same
# source_commit (see DIST_BINARIES), and that has one hard constraint:
#
#   AN OLD CONSUMER MUST STILL READ AN OLD MANIFEST OUT OF A NEW ONE.
#
# Consumers parse this file with sed, not jq -- the offline path on a ranked
# box has a shell and shasum and nothing else -- and the sed they use is
# ANCHORED, one key per line:
#
#   sed -n 's/^[[:space:]]*"KEY"[[:space:]]*:[[:space:]]*"\{0,1\}\([^\",]*\)...
#
# So the manifest keeps BOTH of these true:
#
#   * THE SIX TOP-LEVEL FIELDS ARE UNCHANGED, and they still describe
#     `benchd`. A consumer written before the second binary existed reads
#     the same sha256 and the same bytes it always read, for the same file.
#   * EVERY PER-BINARY ENTRY IS ONE LINE. The nested `sha256`/`bytes` keys
#     therefore never start a line, so the anchored sed above CANNOT see them.
#     Pretty-printing the nested objects across lines would make an old
#     consumer's `manifest_field sha256` return THREE values and refuse -- the
#     one-line rule is what stops that, and scripts/test-dist-manifest.sh
#     proves it against the exact legacy expression.
#
# The shape:
#
#   {
#     "version": "0.0.0",
#     "branch": "qwen3.8-125b-a6b-v1",
#     "source_commit": "<40 hex>",
#     "target_triple": "aarch64-apple-darwin",
#     "sha256": "<benchd sha256>",
#     "bytes": <benchd bytes>,
#     "binaries": {
#       "benchd": {"sha256": "<64 hex>", "bytes": <int>},
#       "record-correctness-golden": {"sha256": "<64 hex>", "bytes": <int>}
#     }
#   }
#
# Source it; it defines functions and one array and runs nothing.

# The binaries the channel publishes, in manifest order. `benchd` MUST stay
# first: it is the binary the six top-level fields describe.
#
#   benchd                     the measurement harness every run drives.
#   record-correctness-golden  the golden AUTHOR. An engine repo's golden
#                              re-author tooling drives it, and a golden must be
#                              authored by the same benchd build that later
#                              validates it. Building it on the box from source
#                              is exactly the "the box builds its own harness"
#                              hole the channel exists to close, so it ships
#                              here instead.
DIST_BINARIES=(benchd record-correctness-golden)

# Read one top-level manifest field. This is the LEGACY expression, unchanged.
dist_manifest_field() {
  sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",]*\)\"\{0,1\}[[:space:]]*,\{0,1\}[[:space:]]*\$/\1/p" "$1"
}

# Read `sha256` or `bytes` out of one per-binary entry:
#   dist_manifest_binary_field <manifest> <binary name> <sha256|bytes>
# The entry is one line, so the binary name anchors the match and the field is
# taken from inside the braces.
dist_manifest_binary_field() {
  sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*{.*\"$3\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",}]*\)\"\{0,1\}.*}[[:space:]]*,\{0,1\}[[:space:]]*\$/\1/p" "$1"
}

# sha256 of a file, as the manifest spells it.
dist_sha256() {
  shasum -a 256 "$1" | awk '{print $1}'
}

dist_bytes() {
  wc -c < "$1" | tr -d '[:space:]'
}

# Write the manifest describing the binaries staged in a directory.
#   dist_manifest_write <out manifest> <bin dir> <version> <branch> <commit> <triple>
# Every binary in DIST_BINARIES must be present; a missing one is a publish
# defect, not a smaller channel, so it fails rather than emitting a short
# manifest that consumers would read as "the channel does not carry it".
dist_manifest_write() {
  local out="$1" bin_dir="$2" version="$3" branch="$4" commit="$5" triple="$6"
  local name path entries=() head_sha head_bytes

  for name in "${DIST_BINARIES[@]}"; do
    path="${bin_dir}/${name}"
    if [[ ! -f "${path}" ]]; then
      echo "dist_manifest_write: ${path} is missing; the channel publishes ${DIST_BINARIES[*]}" >&2
      return 1
    fi
    entries+=("$(printf '    "%s": {"sha256": "%s", "bytes": %s}' \
      "${name}" "$(dist_sha256 "${path}")" "$(dist_bytes "${path}")")")
  done

  # The six top-level fields describe DIST_BINARIES[0] -- `benchd` -- exactly
  # as they did before the channel carried a second binary.
  head_sha="$(dist_sha256 "${bin_dir}/${DIST_BINARIES[0]}")"
  head_bytes="$(dist_bytes "${bin_dir}/${DIST_BINARIES[0]}")"

  {
    printf '{\n'
    printf '  "version": "%s",\n' "${version}"
    printf '  "branch": "%s",\n' "${branch}"
    printf '  "source_commit": "%s",\n' "${commit}"
    printf '  "target_triple": "%s",\n' "${triple}"
    printf '  "sha256": "%s",\n' "${head_sha}"
    printf '  "bytes": %s,\n' "${head_bytes}"
    printf '  "binaries": {\n'
    local i
    for i in "${!entries[@]}"; do
      if (( i + 1 < ${#entries[@]} )); then
        printf '%s,\n' "${entries[i]}"
      else
        printf '%s\n' "${entries[i]}"
      fi
    done
    printf '  }\n'
    printf '}\n'
  } > "${out}"
}

# Check every published binary in a directory against the manifest beside it.
#   dist_manifest_verify <manifest> <bin dir>
# Reports which binary and which half failed; silent on success.
dist_manifest_verify() {
  local manifest="$1" bin_dir="$2" name path want_sha want_bytes have_sha have_bytes rc=0
  for name in "${DIST_BINARIES[@]}"; do
    path="${bin_dir}/${name}"
    if [[ ! -f "${path}" ]]; then
      echo "dist_manifest_verify: ${path} is missing" >&2
      rc=1
      continue
    fi
    want_sha="$(dist_manifest_binary_field "${manifest}" "${name}" sha256)"
    want_bytes="$(dist_manifest_binary_field "${manifest}" "${name}" bytes)"
    have_sha="$(dist_sha256 "${path}")"
    have_bytes="$(dist_bytes "${path}")"
    if [[ "${#want_sha}" -ne 64 || -n "${want_sha//[0-9a-f]/}" ]]; then
      echo "dist_manifest_verify: the manifest has no usable sha256 for ${name}: '${want_sha}'" >&2
      rc=1
      continue
    fi
    if [[ "${want_sha}" != "${have_sha}" ]]; then
      echo "dist_manifest_verify: ${name} sha256 mismatch: manifest=${want_sha} actual=${have_sha}" >&2
      rc=1
    fi
    if [[ "${want_bytes}" != "${have_bytes}" ]]; then
      echo "dist_manifest_verify: ${name} byte count mismatch: manifest=${want_bytes} actual=${have_bytes}" >&2
      rc=1
    fi
  done
  return "${rc}"
}
