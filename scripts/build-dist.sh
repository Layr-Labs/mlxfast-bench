#!/usr/bin/env bash
#
# build-dist.sh -- produce the publishable prebuilt `benchctl` for THIS branch.
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
#
# Output (both under dist/, committed to the dist branch by .github/workflows/dist.yml):
#   dist/benchctl                  the release binary (aarch64-apple-darwin)
#   dist/benchctl.manifest.json    {version, branch, source_commit, target_triple, sha256, bytes}
#
# Standalone use (hand-publishing while org Actions are disabled):
#   ./scripts/build-dist.sh
#   git add -f dist && git commit
# The workflow runs this exact script; nothing about the build lives only in CI.
#
# Env:
#   BENCHD_DIST_BRANCH   branch name recorded in the manifest. Required when
#                        HEAD is detached (CI checkouts are). Default: the
#                        current branch.
#   BENCHD_DIST_TARGET   target triple. Default: aarch64-apple-darwin.
#   BENCHD_DIST_OUT      output directory. Default: <repo>/dist.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

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
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="benchctl"))')"
if [[ -z "${VERSION}" ]]; then
  echo "build-dist.sh: could not resolve the benchctl package version from cargo metadata" >&2
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
echo "build-dist.sh: building benchctl ${VERSION} for ${TARGET_TRIPLE} at ${SOURCE_COMMIT}" >&2

CARGO_INCREMENTAL=0 \
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" \
RUSTFLAGS="--remap-path-prefix=${REPO_ROOT}=/build --remap-path-prefix=${CARGO_HOME_DIR}=/cargo --remap-path-prefix=${RUST_SYSROOT}=/rust" \
  cargo build --release --bin benchctl --target "${TARGET_TRIPLE}"

BUILT="${REPO_ROOT}/target/${TARGET_TRIPLE}/release/benchctl"
if [[ ! -x "${BUILT}" ]]; then
  echo "build-dist.sh: cargo reported success but ${BUILT} is missing or not executable" >&2
  exit 1
fi

# -- 3. Stage + describe ------------------------------------------------------
mkdir -p "${OUT_DIR}"
cp "${BUILT}" "${OUT_DIR}/benchctl"
chmod 755 "${OUT_DIR}/benchctl"

SHA256="$(shasum -a 256 "${OUT_DIR}/benchctl" | awk '{print $1}')"
BYTES="$(wc -c < "${OUT_DIR}/benchctl" | tr -d '[:space:]')"

# Values only -- no prose keys. Consumers pin {branch, commit, sha256, bytes}
# from exactly these fields.
cat > "${OUT_DIR}/benchctl.manifest.json" <<EOF
{
  "version": "${VERSION}",
  "branch": "${BRANCH}",
  "source_commit": "${SOURCE_COMMIT}",
  "target_triple": "${TARGET_TRIPLE}",
  "sha256": "${SHA256}",
  "bytes": ${BYTES}
}
EOF

echo "build-dist.sh: wrote ${OUT_DIR}/benchctl" >&2
echo "  branch        ${BRANCH}" >&2
echo "  source_commit ${SOURCE_COMMIT}" >&2
echo "  target_triple ${TARGET_TRIPLE}" >&2
echo "  sha256        ${SHA256}" >&2
echo "  bytes         ${BYTES}" >&2
