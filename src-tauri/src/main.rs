// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use moeflow_desktop_lib::commands::AppState;
use moeflow_desktop_lib::media::MediaCache;
use moeflow_desktop_lib::profiles::ProfileStore;
use moeflow_desktop_lib::{
    active_port, navigate_main, open_launcher, open_shell, restart_servers, skip_launcher,
};

/// How often the media cache index is written out.
///
/// Cache hits are served by looking the URL up in that index, so an index that never
/// reaches disk means the on-disk payloads are unreachable on the next launch — the cache
/// silently starts empty every time.
const CACHE_FLUSH_INTERVAL: Duration = Duration::from_secs(15);

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A second launch just focuses the existing window.
            show_main(app);
        }))
        .invoke_handler(tauri::generate_handler![
            moeflow_desktop_lib::commands::boot_payload,
            moeflow_desktop_lib::commands::probe_server,
            moeflow_desktop_lib::commands::upsert_profile,
            moeflow_desktop_lib::commands::delete_profile,
            moeflow_desktop_lib::commands::set_active_profile,
            moeflow_desktop_lib::commands::cache_stats,
            moeflow_desktop_lib::commands::clear_cache,
            moeflow_desktop_lib::commands::set_cache_limit,
            moeflow_desktop_lib::commands::export_local,
            moeflow_desktop_lib::commands::choose_save_path,
            moeflow_desktop_lib::commands::open_path,
            moeflow_desktop_lib::commands::open_external,
            moeflow_desktop_lib::commands::open_settings,
            moeflow_desktop_lib::commands::open_launcher,
            moeflow_desktop_lib::commands::set_skip_launcher,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            let config_dir = handle
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let cache_dir = handle
                .path()
                .app_cache_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("media");

            let mut store = ProfileStore::load(&config_dir);
            // Startup is the only moment our own servers are not yet holding these ports,
            // so it is the only moment a "is this port free?" test means anything.
            store.reassign_ports_held_elsewhere();
            let _ = store.save(&config_dir);
            let cache_limit = store.cache_limit_bytes;

            // Serve the instance picker and settings pages ourselves rather than through
            // Tauri's `tauri://` asset protocol — see `shell.rs` for why. Port 0 lets the OS
            // pick; the windows are pointed at whatever we get back.
            let shell_port = match tauri::async_runtime::block_on(moeflow_desktop_lib::shell::bind(0))
            {
                Ok(port) => port,
                Err(err) => {
                    // Not fatal: `shell_url` falls back to the asset protocol.
                    eprintln!("[moeflow] could not bind the shell server: {err}");
                    0
                }
            };

            app.manage(AppState {
                config_dir,
                store: Mutex::new(store),
                cache: Arc::new(MediaCache::new(cache_dir, cache_limit)),
                servers: Mutex::new(HashMap::new()),
                shell_port,
            });

            restart_servers(&handle);

            // Persist the media cache index on a timer. `flush` is a no-op unless something
            // changed, so this is a cheap tick rather than a periodic rewrite.
            let cache = app.state::<AppState>().cache.clone();
            tauri::async_runtime::spawn(async move {
                let mut ticker = tokio::time::interval(CACHE_FLUSH_INTERVAL);
                loop {
                    ticker.tick().await;
                    if let Err(err) = cache.flush() {
                        eprintln!("[moeflow] could not write the media cache index: {err}");
                    }
                }
            });

            // First run shows the instance picker; only an explicit opt-in skips it.
            let skip = skip_launcher(&handle);
            build_main_window(&handle, skip)?;
            if !skip {
                open_launcher(&handle);
            }
            build_tray(&handle)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window hides it; the app lives on in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building MoeFlow desktop");

    app.run(|handle, event| {
        // Last chance to persist the cache index. Without this the last up-to-15 s of
        // downloads are unreachable on the next launch.
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<AppState>() {
                if let Err(err) = state.cache.flush() {
                    eprintln!("[moeflow] could not write the media cache index on exit: {err}");
                }
            }
        }
    });
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Create the main window pointing at the remembered instance.
///
/// It starts hidden: on a first run the instance picker is shown instead, so the user
/// chooses where to connect rather than being dropped into whichever server happened to
/// be the default. `set_active_profile` reveals it once a choice is made.
fn build_main_window(app: &AppHandle, visible: bool) -> tauri::Result<()> {
    let port = active_port(app);
    // See `MAIN_ENTRY_PATH`: a protected route lands on /login when signed out and in the
    // app when signed in, instead of showing the site's public homepage.
    let url = format!("http://127.0.0.1:{port}{}", moeflow_desktop_lib::MAIN_ENTRY_PATH)
        .parse()
        .unwrap_or_else(|_| "about:blank".parse().unwrap());

    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("MoeFlow")
        .inner_size(1400.0, 900.0)
        .min_inner_size(960.0, 640.0)
        .visible(visible)
        .build()?;

    // Opt-in devtools. Debug builds have them available via right-click, but auto-opening
    // on every launch is intrusive, and the frontend runs from a remote origin whose
    // console is otherwise invisible.
    #[cfg(debug_assertions)]
    if std::env::var_os("MOEFLOW_DEVTOOLS").is_some() {
        window.open_devtools();
    }

    // `visible(false)` on the builder is not reliably honoured at creation time on
    // Windows, and the picker must not have the app showing through behind it.
    if !visible {
        let _ = window.hide();
    }
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开主窗口", true, None::<&str>)?;
    let instances = MenuItem::with_id(app, "instances", "切换实例…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "重新加载", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&show, &instances, &settings, &reload, &separator, &quit],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("MoeFlow")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "instances" => open_launcher(app),
            "settings" => open_shell(app, "settings.html"),
            "reload" => navigate_main(app, active_port(app)),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left click opens the instance picker, not the app window. The picker is the
            // app's landing page — it is where you decide *which* server you are looking at —
            // and reaching it from the tray has to work even when the current instance is
            // unreachable and the main window has nothing useful to show.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_launcher(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}
