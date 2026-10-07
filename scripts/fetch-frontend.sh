#!/usr/bin/env bash
# Vendor the upstream MoeFlow frontend at a pinned commit.
#
# The client wraps the *upstream* frontend (moeflow-com/moeflow, directory frontend-v1),
# not the internal iroha fork. We pin the commit so builds are reproducible and so the
# desktop patches have a stable base to apply against.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UPSTREAM_REPO="${UPSTREAM_REPO:-https://github.com/moeflow-com/moeflow.git}"
# Bump deliberately after reviewing upstream changes.
UPSTREAM_SHA="${UPSTREAM_SHA:-c292e452017b0e36223c724bf6dbb9d48fa9e8cb}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> fetching $UPSTREAM_REPO @ $UPSTREAM_SHA"
# `core.symlinks=false` is deliberate. Upstream's frontend-v1 carries exactly one
# symlink (`.eslintignore`), and a Windows `cp -r` cannot recreate symlinks at all — it
# dies with "cannot create symbolic link ... No such file or directory" and takes the
# whole build down with it. Materialising it as a regular file costs nothing (nothing in
# the build reads .eslintignore) and makes the checkout copyable on every platform.
git clone -c core.symlinks=false --filter=blob:none --sparse "$UPSTREAM_REPO" "$WORK/moeflow"
git -C "$WORK/moeflow" sparse-checkout set frontend-v1
git -C "$WORK/moeflow" checkout "$UPSTREAM_SHA"

echo "==> replacing $ROOT/frontend"
rm -rf "$ROOT/frontend"
cp -r "$WORK/moeflow/frontend-v1" "$ROOT/frontend"
rm -rf "$ROOT/frontend/.git"
echo "$UPSTREAM_SHA" > "$ROOT/frontend/.upstream-sha"

# A git repo of its own, so `patches/*.patch` can be applied and rebased with normal tools.
git -C "$ROOT/frontend" init -q
git -C "$ROOT/frontend" add -A
git -C "$ROOT/frontend" -c user.email=dev@local -c user.name=dev \
  commit -q -m "vendor: upstream moeflow frontend-v1 @ ${UPSTREAM_SHA:0:7}"

echo "==> done. baseline commit: $(git -C "$ROOT/frontend" rev-parse --short HEAD)"
