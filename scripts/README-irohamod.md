# irohamod frontend

Build both resources before packaging:

```sh
npm run build:frontend
npm run build:frontend:irohamod
```

The second command refreshes `frontend-irohamod/` from `IROHAMOD_FRONTEND`
(default: `../moeflow-irohamod/irohamod/frontend`, relative to the client root).
This fork includes unpublished local changes, so a public GitHub SHA is not used.
For reproducible releases, keep that checkout at the reviewed revision and record
its revision and any local changes alongside the release.

The refresh excludes `node_modules`, `build`, and `.git`. The build reuses the
source checkout's dependencies through a Windows junction (a symlink elsewhere),
or installs dependencies if none exist. Only the index mount is injected; the
fork already supplies the PS scripts locally, so upstream OutputList patches
must never be applied to it. Overlay files are copied for both variants.

If the locale generator cannot run in a restricted environment, `SKIP_LOCALE=1`
uses the locale JSON already shipped by the frontend checkout. This skips only
the locale freshness step; Vite and the bundle checks still run.

On this Windows machine the fork's esbuild executable failed while removing its
temporary input file. The client checkout's binary has the same version (0.25.4)
and completed the build when selected explicitly:

```sh
SKIP_LOCALE=1 npm run build:frontend
SKIP_LOCALE=1 ESBUILD_BINARY_PATH="$PWD/frontend/node_modules/@esbuild/win32-x64/esbuild.exe" npm run build:frontend:irohamod
```

This override is machine-specific; it is not forced on other platforms.
