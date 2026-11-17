#!/usr/bin/env bash
# Produce the static frontend bundles that get embedded in the installer.
#
# Two bundles ship: the pinned upstream frontend and the irohamod fork, because the client
# picks one per instance at runtime (see docs/local-models.md and the flavor probe). With no
# argument both are built, which is what CI and packaging need; --variant builds one.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

VARIANTS=(upstream irohamod)
if [ "${1:-}" = "--variant" ] && [ "$#" = 2 ]; then
  case "$2" in
    upstream|irohamod) VARIANTS=("$2") ;;
    *) echo "Usage: $0 [--variant upstream|irohamod]" >&2; exit 1 ;;
  esac
elif [ "$#" != 0 ]; then
  echo "Usage: $0 [--variant upstream|irohamod]" >&2
  exit 1
fi

build_one() {
  local variant="$1"
  local frontend="$ROOT/frontend"
  if [ "$variant" = "irohamod" ]; then
    bash "$ROOT/scripts/fetch-frontend-irohamod.sh"
    frontend="$ROOT/frontend-irohamod"
  fi

  if [ ! -d "$frontend" ]; then
    echo "frontend/ is missing — run scripts/fetch-frontend.sh first" >&2
    exit 1
  fi

  echo "==> [$variant] building $frontend"

  # The overlay is all new files, so it is a plain copy rather than a patch. Keeping it out
  # of the patch means only one line of upstream source is ever modified.
  if [ -d "$ROOT/overlay/src" ]; then
    cp -r "$ROOT/overlay/src/." "$frontend/src/"
  fi

  if [ "$variant" = "irohamod" ]; then
    # The fork already serves PS scripts locally; only the desktop mount belongs here.
    (cd "$frontend" && node --input-type=module <<'JS'
import fs from 'node:fs';
const file = 'src/index.tsx';
const mount = "import './desktop/mount';";
const anchor = "const debugLogger = createDebugLogger('app');";
let source = fs.readFileSync(file, 'utf8');
if (!source.includes(mount)) {
  if (!source.includes(anchor)) throw new Error('irohamod mount anchor moved');
  fs.writeFileSync(file, source.replace(anchor, mount + '\n\n' + anchor));
}
JS
    )
  elif [ "${SKIP_PATCHES:-0}" != "1" ]; then
    # Apply the desktop integration patches. They touch two files, one line each, so a
    # failure here means upstream moved one of them — see patches/README.md and rebase.
    (cd "$frontend" && {
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
    })
  fi

  if [ ! -d "$frontend/node_modules" ]; then
    # The vendored lock is not ours to keep in sync, and at the pinned commit upstream's
    # package-lock.json disagrees with package.json — `babel-plugin-macros` is ^3.1.0 in
    # the manifest but only 2.8.0 in the lock. `npm ci` refuses to run against a lock that
    # is out of step, so this step used to work only on a machine that already had
    # node_modules (the usual local case) and failed on every clean checkout.
    #
    # Resync the lock from the manifest, then install reproducibly from the resynced lock.
    (cd "$frontend" && {
      echo "==> [$variant] syncing package-lock.json with package.json"
      npm install --package-lock-only --no-audit --no-fund
      echo "==> [$variant] npm ci"
      npm ci --no-audit --no-fund
    })
  fi

  (cd "$frontend" && {
    # Keeps src/locales/{en,zh-cn}.json in step with messages.yaml. The JSON files are
    # committed upstream, so this is a freshness step rather than a hard prerequisite.
    if [ "${SKIP_LOCALE:-0}" != "1" ]; then
      echo "==> [$variant] npm run build:locale"
      npm run build:locale
    else
      echo "==> [$variant] using the vendored locale JSON (SKIP_LOCALE=1)"
    fi

    echo "==> [$variant] npm run build"
    npm run build
  })

  # The build must be root-served: vite's `base` is unset, so assets are emitted as
  # "/assets/...". Verify rather than assume — an unexpected base would ship a broken app.
  if [ ! -f "$frontend/build/index.html" ]; then
    echo "$variant: build/index.html is missing" >&2
    exit 1
  fi
  if grep -q "PUBLIC_URL" "$frontend/build/index.html"; then
    echo "$variant: build/index.html is the stale CRA template, not the Vite output" >&2
    exit 1
  fi
  if ! grep -q 'src="/assets/' "$frontend/build/index.html"; then
    echo "$variant: build/index.html does not reference root-absolute /assets/ paths" >&2
    exit 1
  fi
  compgen -G "$frontend/build/assets/index-*.js" >/dev/null || {
    echo "$variant: missing index JS bundle in $frontend/build/assets" >&2
    exit 1
  }
  echo "==> $variant frontend bundle ready at $frontend/build"
}

for variant in "${VARIANTS[@]}"; do
  build_one "$variant"
done
