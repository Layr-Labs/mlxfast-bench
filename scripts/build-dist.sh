#!/usr/bin/env bash
#
# build-dist.sh -- produce the publishable prebuilt binaries for THIS branch.
#
# The channel carries the binaries listed in DIST_BINARIES (scripts/dist-lib.sh):
# `benchd`, the measurement harness, and `record-correctness-golden`, the
# golden author an engine repo runs to re-author its track goldens. Both are
# built from ONE source_commit and described by ONE manifest.
#
# Consumers (the engine repo's tools/fetch-benchd.sh) never build benchd: they
# resolve a {branch, commit, sha256, bytes} pin and download the binary this
# script produced. That only works if the sha256 in the manifest is the sha256
# of a build anyone can reproduce from the named source_commit, so this script
# is deliberately strict about the two things that otherwise make a Rust release
# build non-reproducible:
#
#   1. THE SOURCE MUST BE CLEAN. `source_commit` claims "these bytes are
#      mlxfast-bench@<commit> built for <triple>". A dirty tracked file makes
#      that claim false, so a dirty tree is refused (dist/ itself is exempt --
#      it is this script's own output).
#   2. ABSOLUTE PATHS MUST NOT LEAK IN. rustc embeds the workspace, the cargo
#      registry AND the TOOLCHAIN SYSROOT paths in panic messages and debug
#      records, so the same commit built in two different checkout directories --
#      or by two different users -- yields two different sha256s. All three are
#      remapped to fixed placeholders (/build, /cargo, /rust) so a rebuild
#      anywhere reproduces the pinned hash.
#
#      THE SYSROOT REMAP WAS MISSING until 2026-08-26, and the published binary
#      carried 32 std-library source paths under `$HOME/.rustup/toolchains/...`.
#      That is two defects in one:
#
#        * it publishes the BUILDER'S USERNAME in a public artifact; and
#        * it makes the pin reproducible only for a builder whose $HOME matches.
#          A double-build check that runs as one user CANNOT see this -- both
#          builds embed the same $HOME -- so the reproducibility claim the
#          manifest rests on was weaker than it read.
#
#      Remapping the sysroot makes reproducibility $HOME-INDEPENDENT BY
#      CONSTRUCTION rather than by observation. The sysroot is resolved from
#      `rustc --print sysroot` rather than assembled from $HOME/.rustup so a
#      non-default RUSTUP_HOME is handled too; its toolchain-name component is
#      already fixed for every builder by rust-toolchain.toml.
#   3. THE DEPENDENCY VERSIONS MUST BE THE LOCKED ONES. The build runs with
#      `--locked`, so cargo refuses to change Cargo.lock instead of quietly
#      resolving a dependency to a newer version than the one the commit
#      records. A dirty tree is already refused, so a lock update during the
#      build would otherwise pass unnoticed.
#
# Output (all under dist/, committed to the dist branch by .github/workflows/dist.yml):
#   dist/benchd                      the measurement harness (aarch64-apple-darwin)
#   dist/record-correctness-golden   the golden author, same source_commit
#   dist/benchd.manifest.json        {version, branch, source_commit, target_triple,
#                                     sha256, bytes, binaries}
#
# The six original top-level fields still describe `benchd` alone, so a
# consumer written before the second binary existed reads exactly what it read
# before. `binaries` adds one line per published binary, {sha256, bytes}.
# scripts/dist-lib.sh holds the shape, the writer and the readers, and states
# why every entry is one line.
#
# Standalone use (hand-publishing while org Actions are disabled):
#   ./scripts/build-dist.sh
#   git add -f dist && git commit
# The workflow runs this exact script; nothing about the build lives only in CI.
#
# Env:
#   BENCHD_DIST_BRANCH   branch name recorded in the manifest. Required when
#                        HEAD is detached (CI checkouts are). Default: the
#                        current branch. This is the PROJECT branch (for the
#                        Qwen 3.8 125B-A6B project: qwen3.8-125b-a6b-v1), never
#                        a track id -- track ids are platform namespaces
#                        (...-mlx-v1 / ...-cuda-v1) and the one channel serves
#                        both. This script publishes the Mac binary; the Linux
#                        binary is scripts/build-dist-linux.sh, which builds
#                        this same script in a container (README, "Publishing
#                        dist/").
#   BENCHD_DIST_TARGET   target triple. Default: aarch64-apple-darwin.
#   BENCHD_DIST_OUT      output directory. Default: <repo>/dist.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

# shellcheck source=scripts/dist-lib.sh
. "${REPO_ROOT}/scripts/dist-lib.sh"

TARGET_TRIPLE="${BENCHD_DIST_TARGET:-aarch64-apple-darwin}"
OUT_DIR="${BENCHD_DIST_OUT:-${REPO_ROOT}/dist}"

for tool in cargo git shasum; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "build-dist.sh: ${tool} is required but not on PATH" >&2
    exit 1
  fi
done

# -- 1. Identify the source ---------------------------------------------------
# Refuse a dirty tree: the manifest's source_commit is a claim about the bytes,
# and an uncommitted edit silently falsifies it. dist/ is excluded because it is
# this script's own output.
dirty="$(git status --porcelain -- . ':(exclude)dist' | head -20)"
if [[ -n "${dirty}" ]]; then
  echo "build-dist.sh: refusing to build from a dirty tree -- source_commit would not describe the produced bytes." >&2
  printf '%s\n' "${dirty}" >&2
  exit 1
fi

SOURCE_COMMIT="$(git rev-parse HEAD)"

BRANCH="${BENCHD_DIST_BRANCH:-}"
if [[ -z "${BRANCH}" ]]; then
  BRANCH="$(git rev-parse --abbrev-ref HEAD)"
fi
if [[ -z "${BRANCH}" || "${BRANCH}" == "HEAD" ]]; then
  echo "build-dist.sh: HEAD is detached; set BENCHD_DIST_BRANCH to the branch this build publishes for." >&2
  exit 1
fi

# The crate version, read from cargo rather than re-parsed out of Cargo.toml, so
# a move off workspace-inherited versions cannot silently mis-stamp the manifest.
VERSION="$(cargo metadata --no-deps --format-version 1 --manifest-path Cargo.toml \
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="benchd"))')"
if [[ -z "${VERSION}" ]]; then
  echo "build-dist.sh: could not resolve the benchd package version from cargo metadata" >&2
  exit 1
fi

# -- 2. Build -----------------------------------------------------------------
CARGO_HOME_DIR="${CARGO_HOME:-${HOME}/.cargo}"
# The toolchain sysroot, asked of rustc itself: the std sources rustc embeds live
# under <sysroot>/lib/rustlib/src/rust/library/. Fail closed rather than build a
# binary that would silently keep leaking them.
RUST_SYSROOT="$(rustc --print sysroot)"
if [[ -z "${RUST_SYSROOT}" || ! -d "${RUST_SYSROOT}" ]]; then
  echo "build-dist.sh: could not resolve the rust toolchain sysroot (rustc --print sysroot)" >&2
  exit 1
fi
echo "build-dist.sh: building ${DIST_BINARIES[*]} ${VERSION} for ${TARGET_TRIPLE} at ${SOURCE_COMMIT}" >&2

# One cargo invocation for the whole roster: the binaries share a source_commit,
# a dependency graph and a RUSTFLAGS remap, and building them separately would
# only add a way for them to disagree.
bin_args=()
for name in "${DIST_BINARIES[@]}"; do
  bin_args+=(--bin "${name}")
done

CARGO_INCREMENTAL=0 \
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" \
RUSTFLAGS="--remap-path-prefix=${REPO_ROOT}=/build --remap-path-prefix=${CARGO_HOME_DIR}=/cargo --remap-path-prefix=${RUST_SYSROOT}=/rust" \
  cargo build --locked --release "${bin_args[@]}" --target "${TARGET_TRIPLE}"

# -- 3. Stage + describe ------------------------------------------------------
mkdir -p "${OUT_DIR}"
for name in "${DIST_BINARIES[@]}"; do
  BUILT="${REPO_ROOT}/target/${TARGET_TRIPLE}/release/${name}"
  if [[ ! -x "${BUILT}" ]]; then
    echo "build-dist.sh: cargo reported success but ${BUILT} is missing or not executable" >&2
    exit 1
  fi
  cp "${BUILT}" "${OUT_DIR}/${name}"
  chmod 755 "${OUT_DIR}/${name}"
done

# Values only -- no prose keys. Consumers pin {branch, commit, sha256, bytes}
# from exactly these fields; the per-binary entries carry {sha256, bytes} for
# every binary the channel publishes, `benchd` included.
dist_manifest_write "${OUT_DIR}/benchd.manifest.json" "${OUT_DIR}" \
  "${VERSION}" "${BRANCH}" "${SOURCE_COMMIT}" "${TARGET_TRIPLE}"

# The manifest is a claim about the staged bytes. Read it back and check it,
# so a writer defect fails here rather than on a box.
dist_manifest_verify "${OUT_DIR}/benchd.manifest.json" "${OUT_DIR}" \
  || { echo "build-dist.sh: the manifest just written does not describe the staged binaries" >&2; exit 1; }

echo "build-dist.sh: wrote ${OUT_DIR}/benchd.manifest.json" >&2
echo "  branch        ${BRANCH}" >&2
echo "  source_commit ${SOURCE_COMMIT}" >&2
echo "  target_triple ${TARGET_TRIPLE}" >&2
for name in "${DIST_BINARIES[@]}"; do
  echo "  ${name}" >&2
  echo "    sha256      $(dist_sha256 "${OUT_DIR}/${name}")" >&2
  echo "    bytes       $(dist_bytes "${OUT_DIR}/${name}")" >&2
done
