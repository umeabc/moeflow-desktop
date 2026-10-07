//! MoeFlow desktop client.
//!
//! Bundles the upstream web frontend and serves it from a loopback HTTP server that also
//! reverse-proxies the API, caches storage images on disk, and performs LabelPlus / zip
//! exports locally.
//!
//! The loopback hop is the load-bearing idea: because the bundled bundle is served at
//! `http://127.0.0.1:<port>/` exactly as nginx would serve it, the *unmodified* upstream
//! frontend works — root-absolute asset paths, SPA deep links, the `token` cookie and the
//! `Authorization` header all behave normally.

pub mod commands;
pub mod config;
pub mod download;
pub mod exporter;
pub mod labelplus;
pub mod media;
pub mod network;
pub mod profiles;
pub mod pyfloat;
pub mod rewrite;
pub mod server;
pub mod shell;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use commands::AppState;
use server::Ctx;

/// A bound loopback listener and the handle that shuts it down.
pub struct ServerHandle {
    pub port: u16,
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub ctx: Arc<Ctx>,
}

/// Where the bundled frontend lives.
///
/// Packaged builds ship the upstream `build/` tree as a bundle resource; `tauri dev` reads
/// it from the sibling checkout instead.
pub fn web_root(app: &AppHandle) -> PathBuf {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join("web");
        if bundled.join("index.html").is_file() {
            return bundled;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../frontend/build")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("../frontend/build"))
}

/// Whether the user asked to skip the instance picker on startup.
pub fn skip_launcher(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<AppState>() else {
        return false;
    };
    let store = state.store.lock().unwrap_or_else(|p| p.into_inner());
    store.skip_launcher
}

/// Port of the currently active profile, or 0 when none is bound.
pub fn active_port(app: &AppHandle) -> u16 {
    let Some(state) = app.try_state::<AppState>() else {
        return 0;
    };
    let store = state.store.lock().unwrap_or_else(|p| p.into_inner());
    store.active_profile().map(|p| p.port).unwrap_or(0)
}

/// Where the shell pages (instance picker, settings) are served, or 0 if binding failed.
pub fn shell_port(app: &AppHandle) -> u16 {
    app.try_state::<AppState>()
        .map(|state| state.shell_port)
        .unwrap_or(0)
}

/// URL for a shell page.
///
/// Falls back to Tauri's own asset protocol when the shell server could not bind, so a
/// failure there costs us the safety net rather than the window.
fn shell_url(app: &AppHandle, page: &str) -> WebviewUrl {
    let port = shell_port(app);
    if port != 0 {
        if let Ok(url) = format!("http://127.0.0.1:{port}/{page}").parse() {
            return WebviewUrl::External(url);
        }
    }
    WebviewUrl::App(page.into())
}

/// Where the app opens once an instance is chosen.
///
/// Deliberately *not* `/`: the frontend treats `/` as a public page (`App.tsx` lists
/// `routes.index` in `publicPaths`), so it renders the site's homepage — complete with
/// whatever announcement banner the server has configured — even when nobody is signed in.
/// A desktop client should land on authentication instead.
///
/// A protected route gives both behaviours for free. `App.tsx` redirects to `/login` when
/// there is no token:
///
///   * not signed in  → the router bounces to `/login`
///   * signed in      → straight into the workbench, no login form to click past
///
/// A stale token also self-heals: the first 401 clears it, and the guard then redirects.
pub const MAIN_ENTRY_PATH: &str = "/dashboard/projects";

/// Navigate the main window to a profile's loopback origin.
pub fn navigate_main(app: &AppHandle, port: u16) {
    if let Some(window) = app.get_webview_window("main") {
        let url = format!("http://127.0.0.1:{port}{MAIN_ENTRY_PATH}");
        if let Ok(parsed) = url.parse() {
            let _ = window.navigate(parsed);
        }
    }
}

/// Show the main window if it is not on screen yet.
///
/// **Never call this from inside a window event handler on the event-loop thread.** In Tauri
/// v2 `WebviewWindow::is_visible` and `show` both dispatch to the event loop and block until
/// it answers; doing that from a handler that is itself running on the event loop deadlocks
/// the UI thread. It only bites when the other window happens to be busy — a webview
/// mid-navigation, for instance — which makes it look intermittent rather than broken.
/// Callers hand this to `async_runtime::spawn` instead.
pub fn reveal_main_if_hidden(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        if !main.is_visible().unwrap_or(false) {
            let _ = main.show();
        }
    }
}

/// Show the instance picker (“launcher”).
///
/// The shell is one window holding two views — the instance picker and the settings — rather
/// than two windows. See `SHELL_VIEWS` for why that is not merely tidier.
///
/// It is a window of its own rather than a page inside the main window, so it stays reachable
/// regardless of which instance is loaded — including when the current instance is
/// unreachable and the main window is showing nothing useful.
///
/// Exactly one shell exists at a time: the window outlives being closed, so a second call
/// raises the one that is already there instead of stacking another on top.
pub fn open_launcher(app: &AppHandle) {
    open_shell(app, "launcher.html");
}

/// The view a shell window is showing, keyed by the file name it was loaded from.
pub const SHELL_VIEWS: [&str; 2] = ["launcher.html", "settings.html"];

/// Create the shell during setup, before any renderer can invoke an IPC command.
///
/// This is intentionally the only function that calls `WebviewWindowBuilder::build`. A shell
/// must exist even when the picker is hidden on startup; otherwise the first later "设置"
/// click would try to create a WebView from a synchronous command and deadlock the event loop.
pub fn ensure_shell_window(app: &AppHandle, visible: bool) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window("launcher") {
        if visible {
            let _ = existing.show();
        } else {
            let _ = existing.hide();
        }
        return Ok(());
    }

    let app_for_events = app.clone();
    let mut builder = WebviewWindowBuilder::new(app, "launcher", shell_url(app, "launcher.html"));
    if let Some(root) = std::env::var_os("MOEFLOW_TEST_ROOT") {
        builder = builder.data_directory(PathBuf::from(root).join("webview"));
        if let Ok(port) = std::env::var("MOEFLOW_TEST_CDP_PORT")
            .unwrap_or_default()
            .parse::<u16>()
        {
            builder = builder.additional_browser_args(&format!("--remote-debugging-port={port}"));
        }
    }
    let window = builder
        .title("MoeFlow")
        .inner_size(840.0, 680.0)
        .min_inner_size(680.0, 520.0)
        .resizable(true)
        .visible(visible)
        .build()?;

    // Closing the shell hides it instead of destroying the one safe-to-create WebView.
    let window_for_events = window.clone();
    let window_for_close = window.clone();
    window_for_events.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let window = window_for_close.clone();
            let app = app_for_events.clone();
            tauri::async_runtime::spawn(async move {
                let _ = window.hide();
                reveal_main_if_hidden(&app);
            });
        }
    });
    if !visible {
        let _ = window.hide();
    }
    Ok(())
}

/// Show the shell window, switching it to `page` if it is on the other view.
///
/// The window is created by `ensure_shell_window` during setup. This function only operates on
/// that existing window, so it is safe for IPC and tray callers.
pub fn open_shell(app: &AppHandle, page: &str) {
    let app = app.clone();
    let page = page.to_string();
    tauri::async_runtime::spawn(async move {
        let Some(existing) = app.get_webview_window("launcher") else {
            eprintln!("[moeflow] shell window was not created during setup");
            return;
        };
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_focus();
        let on_page = existing
            .url()
            .ok()
            .and_then(|url| {
                url.path_segments()
                    .map(|s| s.last().unwrap_or("").to_string())
            })
            .map(|last| last == page)
            .unwrap_or(false);
        if !on_page {
            if let Ok(url) = shell_view_url(&app, &page).parse() {
                let _ = existing.navigate(url);
            }
        }
    });
}

/// Absolute URL of a shell view, for navigating the shell window between pages.
fn shell_view_url(app: &AppHandle, page: &str) -> String {
    let port = shell_port(app);
    if port != 0 {
        return format!("http://127.0.0.1:{port}/{page}");
    }
    format!("http://tauri.localhost/{page}")
}

/// Ports in use, for the settings window.
pub fn bound_ports(app: &AppHandle) -> HashMap<String, u16> {
    let Some(state) = app.try_state::<AppState>() else {
        return HashMap::new();
    };
    let servers = state.servers.lock().unwrap_or_else(|p| p.into_inner());
    servers
        .iter()
        .map(|(id, handle)| (id.clone(), handle.port))
        .collect()
}
