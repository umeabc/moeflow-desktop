#!/usr/bin/env bash
# Vendor the irohamod fork's frontend into frontend-irohamod/.
#
# Two sources, because the fork is developed in a local checkout but has to be reproducible
# in CI: IROHAMOD_FRONTEND selects a local tree (fast, used on the dev machine), otherwise
# the pinned commit is cloned from IROHAMOD_REPO. The pin moves only after reviewing the
# fork's frontend diff — the same rule scripts/fetch-frontend.sh uses for upstream.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="${IROHAMOD_REPO:-https://github.com/umeabc/moeflow-irohamod.git}"
# fix12 (dark-mode fix + release notes), the frontend the desktop build currently ships.
SHA="${IROHAMOD_SHA:-980255f587e0a8926569de686fa099fa0ff1c27a}"
TARGET="$ROOT/frontend-irohamod"

# Replace everything except node_modules: the dependency tree is either a junction to the
# local checkout (dev) or an installed tree from a previous run (CI), and re-copying it
# costs minutes for no benefit.
copy_tree() {
  local source="$1"
  [ -f "$source/package.json" ] || { echo "Missing frontend: $source" >&2; exit 1; }
  case "$source" in
    "$TARGET"|"$TARGET"/*) echo "Source must be outside frontend-irohamod" >&2; exit 1 ;;
  esac
  mkdir -p "$TARGET"
  find "$TARGET" -mindepth 1 -maxdepth 1 ! -name node_modules -exec rm -rf {} +
  tar -cf - --exclude=node_modules --exclude=build --exclude=.git -C "$source" . \
    | tar -xf - -C "$TARGET"
}

if [ -n "${IROHAMOD_FRONTEND:-}" ]; then
  echo "==> vendoring irohamod from $IROHAMOD_FRONTEND"
  copy_tree "$(cd "$IROHAMOD_FRONTEND" && pwd)"
  echo "$IROHAMOD_FRONTEND (local checkout)" > "$TARGET/.irohamod-source"
else
  echo "==> fetching $REPO @ ${SHA:0:7}"
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  # Sparse + blobless: only the frontend directory is needed, and the repository also
  # carries the backend and images.
  #
  # GitHub occasionally drops the TLS handshake from this network, and a fresh clone has no
  # cache to fall back on — retry rather than fail a whole release build on one blip.
  for attempt in 1 2 3; do
    rm -rf "$WORK/repo"
    if git clone -q -c core.symlinks=false --filter=blob:none --sparse "$REPO" "$WORK/repo" 2>/dev/null; then
      break
    fi
    [ "$attempt" = 3 ] && { echo "clone failed after 3 attempts" >&2; exit 1; }
    echo "==> clone attempt $attempt failed, retrying" >&2
    sleep $((attempt * 5))
  done
  git -C "$WORK/repo" sparse-checkout set frontend
  git -C "$WORK/repo" checkout -q "$SHA"
  copy_tree "$WORK/repo/frontend"
  echo "irohamod @ $SHA" > "$TARGET/.irohamod-source"
fi

echo "==> done. $(cat "$TARGET/.irohamod-source")"
