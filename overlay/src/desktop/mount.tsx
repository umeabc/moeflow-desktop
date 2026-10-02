/**
 * Mounts the desktop dock at `document.body`.
 *
 * This is pulled in by a single added import line in `src/index.tsx`. Mounting into a
 * container we create ourselves — rather than into a page component — keeps the overlay
 * independent of upstream's component tree, so a frontend update cannot break it and a
 * failure here cannot break the app.
 */
import React from 'react';
import ReactDOM from 'react-dom';

import { describeError, shell } from './bridge';
import { DesktopDock } from './DesktopDock';

const CONTAINER_ID = 'moeflow-desktop-dock';

function mount() {
  if (document.getElementById(CONTAINER_ID)) return;

  const container = document.createElement('div');
  container.id = CONTAINER_ID;
  document.body.appendChild(container);

  // React 17 — the app pins react-dom 17.0.2, so this is `render`, not `createRoot`.
  ReactDOM.render(<DesktopDock />, container);

  void probeIpc();
}

/**
 * Confirm IPC actually works before painting buttons that depend on it.
 *
 * Being inside the desktop shell is not enough: Tauri only permits IPC from an origin a
 * capability has declared, and the main window is served from `http://127.0.0.1:<port>` —
 * a *remote* origin. Without `capabilities/default.json` listing it under `remote.urls`,
 * `window.__TAURI__` exists and every call is silently rejected, so the dock renders but
 * nothing it does has any effect.
 *
 * The verdict is left on `body[data-mf-dock-ipc]` rather than just logged: a rejected IPC
 * call is otherwise indistinguishable from a click that did nothing.
 */
async function probeIpc() {
  try {
    await shell.boot();
    document.body.setAttribute('data-mf-dock-ipc', 'ok');
  } catch (error) {
    document.body.setAttribute('data-mf-dock-ipc', 'failed');
    // eslint-disable-next-line no-console
    console.error('[moeflow-desktop] IPC unavailable:', describeError(error));
  }
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', mount);
} else {
  mount();
}
