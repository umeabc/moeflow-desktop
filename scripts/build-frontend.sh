#!/usr/bin/env bash
# Produce the static frontend bundle that gets embedded in the installer.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND="$ROOT/frontend"

if [ ! -d "$FRONTEND" ]; then
  echo "frontend/ is missing — run scripts/fetch-frontend.sh first" >&2
  exit 1
fi

cd "$FRONTEND"

# The overlay is all new files, so it is a plain copy rather than a patch. Keeping it out
# of the patch means only one line of upstream source is ever modified.
if [ -d "$ROOT/overlay/src" ]; then
  echo "==> copying overlay"
  cp -r "$ROOT/overlay/src/." "$FRONTEND/src/"
fi

# Apply the desktop integration patches. They touch two files, one line each, so a failure
# here means upstream moved one of them — see patches/README.md and rebase.
if [ "${SKIP_PATCHES:-0}" != "1" ]; then
  for patch in "$ROOT"/patches/[0-9]*.patch; do
    [ -e "$patch" ] || continue
    if git apply --check "$patch" 2>/dev/null; then
      echo "==> applying $(basename "$patch")"
      git apply "$patch"
    elif git apply --reverse --check "$patch" 2>/dev/null; then
      echo "==> $(basename "$patch") already applied"
    else
      echo "!! $(basename "$patch") does not apply cleanly — upstream likely moved." >&2
      echo "   The two mount points are listed in patches/README.md." >&2
      exit 1
    fi
  done
fi

if [ ! -d node_modules ]; then
  echo "==> npm ci"
  npm ci --no-audit --no-fund
fi

# Keeps src/locales/{en,zh-cn}.json in step with messages.yaml. The JSON files are
# committed upstream, so this is a freshness step rather than a hard prerequisite.
echo "==> npm run build:locale"
npm run build:locale

echo "==> npm run build"
npm run build

# The build must be root-served: vite's `base` is unset, so assets are emitted as
# "/assets/...". Verify rather than assume — an unexpected base would ship a broken app.
if [ ! -f "$FRONTEND/build/index.html" ]; then
  echo "build/index.html is missing" >&2
  exit 1
fi
if grep -q "PUBLIC_URL" "$FRONTEND/build/index.html"; then
  echo "build/index.html is the stale CRA template from public/, not the Vite output" >&2
  exit 1
fi
if ! grep -q 'src="/assets/' "$FRONTEND/build/index.html"; then
  echo "build/index.html does not reference root-absolute /assets/ paths" >&2
  exit 1
fi

echo "==> frontend bundle ready at frontend/build"
