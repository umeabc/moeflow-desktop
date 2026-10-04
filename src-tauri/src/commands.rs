//! Tauri IPC surface, used by the frontend overlay and the settings window.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::exporter::{self, ExportReport, ExportRequest};
use crate::media::CacheStats;
use crate::profiles::{self, ProbeResult, Profile, ProfileStore};

/// Everything the command handlers need, managed as Tauri state.
pub struct AppState {
    pub config_dir: PathBuf,
    pub store: Mutex<ProfileStore>,
    pub cache: Arc<crate::media::MediaCache>,
    /// One loopback server per profile, keyed by profile id.
    pub servers: Mutex<HashMap<String, crate::ServerHandle>>,
    /// Where the shell pages are served; 0 when binding failed.
    pub shell_port: u16,
}

impl AppState {
    pub fn save(&self) {
        let store = self.store.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(err) = store.save(&self.config_dir) {
            eprintln!("[commands] failed to save profiles: {err}");
        }
    }

    pub fn active(&self) -> Option<Profile> {
        let store = self.store.lock().unwrap_or_else(|p| p.into_inner());
        store.active_profile().cloned()
    }
}

#[derive(Serialize)]
pub struct BootPayload {
    pub profiles: Vec<Profile>,
    pub active: String,
    pub active_port: u16,
    pub skip_launcher: bool,
    pub cache: CacheStats,
}

#[tauri::command]
pub fn boot_payload(state: tauri::State<'_, AppState>) -> BootPayload {
    let store = state.store.lock().unwrap_or_else(|p| p.into_inner());
    let active_port = store.active_profile().map(|p| p.port).unwrap_or(0);
    BootPayload {
        profiles: store.profiles.clone(),
        active: store.active.clone(),
        active_port,
        skip_launcher: store.skip_launcher,
        cache: state.cache.stats(),
    }
}

/// Persist the "always start in the remembered instance" preference.
#[tauri::command]
pub fn set_skip_launcher(state: tauri::State<'_, AppState>, skip: bool) -> BootPayload {
    {
        let mut store = state.store.lock().unwrap_or_else(|p| p.into_inner());
        store.skip_launcher = skip;
    }
    state.save();
    boot_payload(state)
}

#[tauri::command]
pub fn open_launcher(app: AppHandle) {
    crate::open_launcher(&app);
}

/// Probe a user-entered server URL and resolve which API base actually answers.
#[tauri::command]
pub async fn probe_server(input: String) -> Result<ProbeResult, String> {
    // Try preset first — zero network cost.
    if let Some(preset) = profiles::find_preset(&input) {
        return Ok(ProbeResult {
            ok: true,
            api_base: Some(preset.api_base.clone()),
            matched: Some(preset.host.clone()),
            message: format!("已匹配预设：{}", preset.name),
            tried: vec![],
        });
    }

    // No preset; probe the candidates.
    let candidates = profiles::api_base_candidates(&input);
    let client = crate::probe_client();

    for (index, candidate) in candidates.iter().enumerate() {
        if profiles::probe_ping(&client, candidate).await {
            return Ok(ProbeResult {
                ok: true,
                api_base: Some(candidate.clone()),
                matched: Some(candidate.clone()),
                message: format!("连接成功：{candidate}"),
                tried: candidates[..=index].to_vec(),
            });
        }
    }

    // Everything failed; fall back to `<input>/api`.
    let fallback = profiles::default_api_base(&input);
    Ok(ProbeResult {
        ok: true,
        api_base: Some(fallback.clone()),
        matched: None,
        message: format!("未找到可用后端，已设为默认：{fallback}"),
        tried: candidates,
    })
}

/// Resolve an address to an API base, or explain which candidates were tried.
async fn resolve_api_base(input: &str) -> Result<String, String> {
    // Try preset first.
    if let Some(preset) = profiles::find_preset(input) {
        return Ok(preset.api_base);
    }

    // No preset; probe the candidates.
    let candidates = profiles::api_base_candidates(input);
    let client = crate::probe_client();
    for candidate in &candidates {
        if profiles::probe_ping(&client, candidate).await {
            return Ok(candidate.clone());
        }
    }

    // Everything failed; fall back to `<input>/api`.
    Ok(profiles::default_api_base(input))
}

#[tauri::command]
pub async fn upsert_profile(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    mut profile: Profile,
) -> Result<BootPayload, String> {
    // Try preset first — instant if it matches.
    if profiles::api_base_needs_resolution(&profile) {
        let input = if profile.api_base.trim().is_empty() {
            profile.site_url.clone()
        } else {
            profile.api_base.clone()
        };

        if let Some(preset) = profiles::find_preset(&input) {
            profile.api_base = preset.api_base;
            profile.media_origins = preset.media_origins;
            if profile.name.trim().is_empty() {
                profile.name = preset.name;
            }
            if profile.site_url.trim().is_empty() {
                profile.site_url = input;
            }
        } else {
            // No preset; resolve through probing or fall back to default.
            profile.api_base = resolve_api_base(&input).await?;
            if profile.site_url.trim().is_empty() {
                profile.site_url = profile.api_base.clone();
            }
        }
    }

    {
        let mut store = state.store.lock().unwrap_or_else(|p| p.into_inner());
        match store.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => {
                // Preserve the assigned port; the origin must stay stable across edits or
                // the stored login token would be orphaned.
                let port = existing.port;
                *existing = Profile { port, ..profile };
            }
            None => {
                let used: Vec<u16> = store.profiles.iter().map(|p| p.port).collect();
                store.profiles.push(Profile {
                    port: free_port(&used),
                    ..profile
                });
            }
        }
        store.ensure_ports();
    }
    state.save();
    crate::restart_servers(&app);
    Ok(boot_payload(state))
}

#[tauri::command]
pub fn delete_profile(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<BootPayload, String> {
    {
        let mut store = state.store.lock().unwrap_or_else(|p| p.into_inner());
        if store.profiles.len() <= 1 {
            return Err("至少要保留一个服务器配置".into());
        }
        store.profiles.retain(|p| p.id != id);
        if store.active == id {
            store.active = store.profiles[0].id.clone();
        }
    }
    state.save();
    crate::restart_servers(&app);
    Ok(boot_payload(state))
}

#[tauri::command]
pub fn set_active_profile(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<u16, String> {
    let port = {
        let mut store = state.store.lock().unwrap_or_else(|p| p.into_inner());
        let profile = store
            .get(&id)
            .ok_or_else(|| format!("未知的服务器配置：{id}"))?;
        let port = profile.port;
        store.active = id;
        port
    };
    state.save();
    crate::navigate_main(&app, port);

    // Reveal the app and get the picker out of the way.
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    if let Some(launcher) = app.get_webview_window("launcher") {
        let _ = launcher.hide();
    }
    // `set_active_profile` is also reached from the settings window's 「连接到此实例」 button.
    // That is an explicit request to go to that instance, so the window it was invoked from
    // steps aside rather than sitting in front of the app it just opened.
    Ok(port)
}

fn free_port(used: &[u16]) -> u16 {
    for port in 47100..47600u16 {
        if !used.contains(&port) && profiles::port_is_free(port) {
            return port;
        }
    }
    0
}

// ---------------------------------------------------------------- cache

#[tauri::command]
pub fn cache_stats(state: tauri::State<'_, AppState>) -> CacheStats {
    state.cache.stats()
}

#[tauri::command]
pub fn clear_cache(state: tauri::State<'_, AppState>) -> Result<CacheStats, String> {
    state.cache.clear().map_err(|err| err.to_string())?;
    Ok(state.cache.stats())
}

#[tauri::command]
pub fn set_cache_limit(
    state: tauri::State<'_, AppState>,
    bytes: u64,
) -> Result<CacheStats, String> {
    state.cache.set_limit(bytes);
    {
        let mut store = state.store.lock().unwrap_or_else(|p| p.into_inner());
        store.cache_limit_bytes = bytes;
    }
    state.save();
    Ok(state.cache.stats())
}

// ---------------------------------------------------------------- export

#[tauri::command]
pub async fn export_local(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: ExportRequest,
) -> Result<ExportReport, String> {
    let profile = state.active().ok_or("没有已启用的服务器配置")?;
    let cache = state.cache.clone();
    let client = crate::http_client(profile.allow_invalid_certs);

    let emitter = app.clone();
    let report = exporter::run(
        &client,
        &profile.api_base,
        cache,
        &request,
        move |stage, progress| {
            let _ = emitter.emit(
                "export://progress",
                serde_json::json!({ "stage": stage, "progress": progress }),
            );
        },
    )
    .await?;

    let _ = app.emit("export://finished", &report);
    Ok(report)
}

/// Native "save as" picker, for the overlay's export button.
#[tauri::command]
pub fn choose_save_path(app: AppHandle, suggested: String) -> Option<String> {
    crate::download::ask_save_path(&app, &suggested)
        .map(|path| path.to_string_lossy().to_string())
}

// ---------------------------------------------------------------- shell

#[tauri::command]
pub fn open_path(app: AppHandle, path: String) {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().reveal_item_in_dir(&path);
}

#[tauri::command]
pub fn open_external(app: AppHandle, url: String) {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().open_url(url, None::<&str>);
}

#[tauri::command]
/// Switch the shell window to its settings view.
///
/// This only navigates the window that already exists — it never creates one. Creating a
/// webview window from a synchronous command deadlocks the whole app; see `open_shell`.
pub fn open_settings(app: AppHandle) {
    crate::open_shell(&app, "settings.html");
}
