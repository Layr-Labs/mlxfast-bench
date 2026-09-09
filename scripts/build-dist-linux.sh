#!/usr/bin/env bash
#
# build-dist-linux.sh -- publish a Linux directory of the dist channel.
#
# scripts/build-dist.sh publishes the Mac half of the channel. This script
# publishes dist/linux-aarch64/ (the CUDA engine's box) or dist/linux-x86_64/
# (a participant's x86 workstation; BENCHD_DIST_TARGET=x86_64-unknown-linux-gnu),
# from the SAME source commit, using deploy/Dockerfile.dist-linux. Both halves carry the whole
# roster (scripts/dist-lib.sh, DIST_BINARIES): `benchd` and
# `record-correctness-golden`, described by one manifest. The container is what
# makes the bytes reproducible; see that file for what it pins and what it
# proves.
#
# The commit must already be pushed: the container fetches it by sha from the
# bench repository rather than copying the working tree. The commit must also
# stay reachable from the branch tip, so a pull request that carries dist/ is
# merged with a merge commit, never squashed.
#
# Typical use, publishing the commit the Mac binary already names:
#
#     ./scripts/build-dist-linux.sh "$(sed -n 's/.*"source_commit": "\(.*\)".*/\1/p' dist/benchd.manifest.json)"
#     git add -f dist/linux-aarch64 && git commit
#
# To prove reproducibility, run it twice with BENCHD_DIST_NO_CACHE=1 and compare
# the reported sha256.
#
# Usage:
#   ./scripts/build-dist-linux.sh [source_commit]
#
# Env:
#   BENCHD_DIST_BRANCH    branch name recorded in the manifest. Default: the
#                         current branch. This is the PROJECT branch, never a
#                         track id (README, "Branches, tracks and the dist
#                         channel").
#   BENCHD_DIST_OUT       output directory. Default: <repo>/dist/linux-aarch64.
#   BENCHD_DIST_TARGET    rust target. Default: aarch64-unknown-linux-gnu.
#   BENCHD_DIST_REPO      https clone URL the container fetches from.
#   BENCHD_DIST_TOKEN / GITHUB_TOKEN
#                         token for the private bench repository. If neither is
#                         set, `gh auth token` is tried.
#   BENCHD_DIST_NO_CACHE  set to 1 to build with no layer cache.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

# shellcheck source=scripts/dist-lib.sh
. "${REPO_ROOT}/scripts/dist-lib.sh"

TARGET_TRIPLE="${BENCHD_DIST_TARGET:-aarch64-unknown-linux-gnu}"

# One directory and one container platform per Linux target. The directory
# name is the machine token the engine's fetch derives from `uname -m`.
case "${TARGET_TRIPLE}" in
  aarch64-unknown-linux-gnu) DIST_SUBDIR="linux-aarch64"; DOCKER_PLATFORM="linux/arm64" ;;
  x86_64-unknown-linux-gnu) DIST_SUBDIR="linux-x86_64"; DOCKER_PLATFORM="linux/amd64" ;;
  *) echo "build-dist-linux.sh: unsupported BENCHD_DIST_TARGET '${TARGET_TRIPLE}' (aarch64-unknown-linux-gnu or x86_64-unknown-linux-gnu)" >&2; exit 1 ;;
esac
OUT_DIR="${BENCHD_DIST_OUT:-${REPO_ROOT}/dist/${DIST_SUBDIR}}"
BENCH_REPO="${BENCHD_DIST_REPO:-https://github.com/Layr-Labs/mlxfast-bench-dev.git}"

die() {
  echo "build-dist-linux.sh: $*" >&2
  exit 1
}

command -v docker >/dev/null 2>&1 || die "docker is required to build the Linux binary."
command -v shasum >/dev/null 2>&1 || die "shasum is required to check the produced binary."

SOURCE_COMMIT="${1:-$(git rev-parse HEAD)}"
if [[ "${#SOURCE_COMMIT}" -ne 40 || -n "${SOURCE_COMMIT//[0-9a-f]/}" ]]; then
  die "source_commit must be a full 40-character lowercase sha: '${SOURCE_COMMIT}'"
fi

BRANCH="${BENCHD_DIST_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}"
if [[ -z "${BRANCH}" || "${BRANCH}" == "HEAD" ]]; then
  die "HEAD is detached; set BENCHD_DIST_BRANCH to the branch this build publishes for."
fi

# The container fetches the commit from the remote, so an unpushed commit fails
# there with a confusing git error. Say so here instead.
if ! git branch -r --contains "${SOURCE_COMMIT}" 2>/dev/null | grep -q .; then
  die "${SOURCE_COMMIT} is not on any remote branch; push it before building the dist binary."
fi

# The token is passed as a build secret, so it is never written to a layer.
TOKEN="${BENCHD_DIST_TOKEN:-${GITHUB_TOKEN:-}}"
if [[ -z "${TOKEN}" ]] && command -v gh >/dev/null 2>&1; then
  TOKEN="$(gh auth token 2>/dev/null || true)"
fi
if [[ -z "${TOKEN}" ]]; then
  echo "build-dist-linux.sh: no token found; the fetch will fail if ${BENCH_REPO} is private. Set BENCHD_DIST_TOKEN or GITHUB_TOKEN." >&2
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT
SECRET_FILE="${TMP_DIR}/gh_token"
printf '%s' "${TOKEN}" > "${SECRET_FILE}"
chmod 600 "${SECRET_FILE}"

build_args=(
  build
  --platform "${DOCKER_PLATFORM}"
  --file deploy/Dockerfile.dist-linux
  --target export
  --build-arg "SOURCE_COMMIT=${SOURCE_COMMIT}"
  --build-arg "DIST_BRANCH=${BRANCH}"
  --build-arg "BENCH_REPO=${BENCH_REPO}"
  --build-arg "TARGET_TRIPLE=${TARGET_TRIPLE}"
  --secret "id=gh_token,src=${SECRET_FILE}"
  --output "type=local,dest=${TMP_DIR}/out"
)
if [[ "${BENCHD_DIST_NO_CACHE:-0}" == "1" ]]; then
  build_args+=(--no-cache)
fi
build_args+=(.)

echo "build-dist-linux.sh: building ${DIST_BINARIES[*]} for ${TARGET_TRIPLE} at ${SOURCE_COMMIT}" >&2
docker "${build_args[@]}"

BUILT_DIR="${TMP_DIR}/out"
BUILT_MANIFEST="${BUILT_DIR}/benchd.manifest.json"
[[ -f "${BUILT_MANIFEST}" ]] || die "the container did not produce benchd.manifest.json"
for name in "${DIST_BINARIES[@]}"; do
  [[ -f "${BUILT_DIR}/${name}" ]] \
    || die "the container did not export ${name}; the channel publishes ${DIST_BINARIES[*]}"
done

# Re-check the whole roster outside the container. The manifest is a claim about
# these exact bytes; a mismatch here means the export step, not the build, is
# wrong. Every published binary is checked, not only the one the six top-level
# fields describe.
dist_manifest_verify "${BUILT_MANIFEST}" "${BUILT_DIR}" \
  || die "the exported manifest does not describe the exported binaries"
[[ "$(dist_manifest_field "${BUILT_MANIFEST}" sha256)" == "$(dist_sha256 "${BUILT_DIR}/benchd")" ]] \
  || die "manifest sha256 does not describe the exported benchd"
[[ "$(dist_manifest_field "${BUILT_MANIFEST}" bytes)" == "$(dist_bytes "${BUILT_DIR}/benchd")" ]] \
  || die "manifest bytes does not describe the exported benchd"
[[ "$(dist_manifest_field "${BUILT_MANIFEST}" source_commit)" == "${SOURCE_COMMIT}" ]] || die "manifest source_commit is not the requested commit"
[[ "$(dist_manifest_field "${BUILT_MANIFEST}" target_triple)" == "${TARGET_TRIPLE}" ]] || die "manifest target_triple is not the requested target"

mkdir -p "${OUT_DIR}"
cp "${BUILT_MANIFEST}" "${OUT_DIR}/benchd.manifest.json"
chmod 644 "${OUT_DIR}/benchd.manifest.json"
for name in "${DIST_BINARIES[@]}"; do
  cp "${BUILT_DIR}/${name}" "${OUT_DIR}/${name}"
  chmod 755 "${OUT_DIR}/${name}"
done

echo "build-dist-linux.sh: wrote ${OUT_DIR}/benchd.manifest.json" >&2
echo "  branch        ${BRANCH}" >&2
echo "  source_commit ${SOURCE_COMMIT}" >&2
echo "  target_triple ${TARGET_TRIPLE}" >&2
for name in "${DIST_BINARIES[@]}"; do
  echo "  ${name}" >&2
  echo "    sha256      $(dist_sha256 "${OUT_DIR}/${name}")" >&2
  echo "    bytes       $(dist_bytes "${OUT_DIR}/${name}")" >&2
done
