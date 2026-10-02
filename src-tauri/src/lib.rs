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
pub mod download;
pub mod exporter;
pub mod labelplus;
pub mod media;
pub mod profiles;
pub mod pyfloat;
pub mod rewrite;
pub mod server;
pub mod shell;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use commands::AppState;
use server::Ctx;

/// A bound loopback listener and the handle that shuts it down.
pub struct ServerHandle {
    pub port: u16,
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Client used for API calls, uploads and image fetches.
///
/// The generous timeout is deliberate: project imports are large multipart bodies that can
/// legitimately take minutes on a slow link.
pub fn http_client(allow_invalid_certs: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(allow_invalid_certs)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .pool_max_idle_per_host(8)
        .build()
        .expect("failed to build HTTP client")
}

/// Short-timeout client for connection probes, where a hung server must not stall the UI.
pub fn probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .build()
        .expect("failed to build probe client")
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

/// Rebind a loopback server for every profile.
///
/// Each profile gets its own port so the origin-scoped `token` cookie cannot leak between
/// servers when the user switches.
pub fn restart_servers(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    {
        let mut servers = state.servers.lock().unwrap_or_else(|p| p.into_inner());
        for (_, mut handle) in servers.drain() {
            if let Some(shutdown) = handle.shutdown.take() {
                let _ = shutdown.send(());
            }
        }
    }

    let profiles = {
        let store = state.store.lock().unwrap_or_else(|p| p.into_inner());
        store.profiles.clone()
    };

    let root = web_root(app);
    let mut servers = state.servers.lock().unwrap_or_else(|p| p.into_inner());

    for profile in profiles {
        let ctx = Arc::new(Ctx::new(
            profile.clone(),
            root.clone(),
            state.cache.clone(),
            app.clone(),
        ));
        match tauri::async_runtime::block_on(server::bind(ctx, profile.port)) {
            Ok((port, shutdown)) => {
                servers.insert(
                    profile.id.clone(),
                    ServerHandle {
                        port,
                        shutdown: Some(shutdown),
                    },
                );
            }
            Err(err) => {
                eprintln!(
                    "[moeflow] could not bind loopback server for {} on port {}: {err}",
                    profile.id, profile.port
                );
            }
        }
    }
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
/// The picker is its own window rather than a page inside the main window. That keeps it
/// reachable regardless of which instance is loaded — including when the current instance
/// is unreachable and the main window is showing nothing useful — and keeps it independent
/// of whatever the main window happens to be rendering.
pub fn open_launcher(app: &AppHandle) {
    if let Some(existing) = app.get_webview_window("launcher") {
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }

    let app_for_events = app.clone();
    let built = WebviewWindowBuilder::new(app, "launcher", shell_url(app, "launcher.html"))
        .title("选择 MoeFlow 实例")
        .inner_size(760.0, 620.0)
        .min_inner_size(620.0, 520.0)
        .resizable(true)
        .build()
        .map(|window| {
            // Closing the picker without choosing should not leave the app in limbo: if the
            // main window has never been shown, reveal it on the remembered instance.
            let app = app_for_events;
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    let app = app.clone();
                    // Deferred on purpose — see `reveal_main_if_hidden`. Window getters and
                    // setters dispatch to the event loop and wait for it to answer, and this
                    // callback *is* the event loop.
                    tauri::async_runtime::spawn(async move {
                        reveal_main_if_hidden(&app);
                    });
                }
            });
        });

    // Never swallow this: if the picker cannot be created the app would otherwise start
    // with no visible window at all. Fall back to showing the main window.
    if let Err(err) = built {
        eprintln!("[moeflow] could not open the instance picker: {err}");
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
        }
    }
}

/// A small native window for server profiles and cache management.
pub fn open_settings_window(app: &AppHandle) {
    if let Some(existing) = app.get_webview_window("settings") {
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", shell_url(app, "settings.html"))
        .title("MoeFlow 设置")
        .inner_size(820.0, 660.0)
        .min_inner_size(640.0, 480.0)
        .build();
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
