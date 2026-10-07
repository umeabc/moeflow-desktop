//! Tauri IPC surface, used by the frontend overlay and the settings window.
pub use crate::config::AppState;
use crate::exporter::{self, ExportReport, ExportRequest};
use crate::media::CacheStats;
use crate::network::ProxySettings;
use crate::profiles::{self, ProbeResult, Profile};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize)]
pub struct BootPayload {
    pub profiles: Vec<Profile>,
    pub active: String,
    pub active_port: u16,
    pub skip_launcher: bool,
    pub cache: CacheStats,
    pub proxy: ProxySettings,
    pub version: &'static str,
}

pub fn payload(state: &AppState) -> BootPayload {
    let store = state.store_snapshot();
    let active_port = store.active_profile().map(|p| p.port).unwrap_or(0);
    BootPayload {
        profiles: store.profiles,
        active: store.active,
        active_port,
        skip_launcher: store.skip_launcher,
        cache: state.cache.stats(),
        proxy: store.proxy,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
pub fn boot_payload(state: tauri::State<'_, AppState>) -> BootPayload {
    payload(&state)
}

#[tauri::command]
pub async fn set_skip_launcher(
    state: tauri::State<'_, AppState>,
    skip: bool,
) -> Result<BootPayload, String> {
    state.set_skip(skip).await?;
    Ok(payload(&state))
}

#[tauri::command]
pub async fn set_proxy_settings(
    state: tauri::State<'_, AppState>,
    settings: ProxySettings,
) -> Result<BootPayload, String> {
    state.set_proxy(settings).await?;
    Ok(payload(&state))
}

#[tauri::command]
pub fn open_launcher(app: AppHandle) {
    crate::open_launcher(&app);
}

#[tauri::command]
pub async fn probe_server(
    state: tauri::State<'_, AppState>,
    input: String,
) -> Result<ProbeResult, String> {
    if let Some(preset) = profiles::find_preset(&input) {
        return Ok(ProbeResult {
            ok: true,
            api_base: Some(preset.api_base),
            matched: Some(preset.host),
            message: format!("已匹配预设：{}", preset.name),
            tried: vec![],
        });
    }
    let candidates = profiles::api_base_candidates(&input);
    let client = state
        .clients
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .probe
        .clone();
    for (index, candidate) in candidates.iter().enumerate() {
        if profiles::probe_ping_network(&client, candidate).await {
            return Ok(ProbeResult {
                ok: true,
                api_base: Some(candidate.clone()),
                matched: Some(candidate.clone()),
                message: format!("连接成功：{candidate}"),
                tried: candidates[..=index].to_vec(),
            });
        }
    }
    let fallback = profiles::default_api_base(&input);
    Ok(ProbeResult {
        ok: true,
        api_base: Some(fallback.clone()),
        matched: None,
        message: format!("未找到可用后端，已设为默认：{fallback}"),
        tried: candidates,
    })
}

#[tauri::command]
pub async fn upsert_profile(
    state: tauri::State<'_, AppState>,
    profile: Profile,
) -> Result<BootPayload, String> {
    state.upsert(profile).await?;
    Ok(payload(&state))
}

#[tauri::command]
pub async fn delete_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<BootPayload, String> {
    state.delete(id).await?;
    Ok(payload(&state))
}

#[tauri::command]
pub async fn set_active_profile(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<u16, String> {
    let port = state.set_active(id).await?;
    let deferred = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::navigate_main(&deferred, port);
        if let Some(main) = deferred.get_webview_window("main") {
            let _ = main.show();
            let _ = main.unminimize();
            let _ = main.set_focus();
        }
        if let Some(launcher) = deferred.get_webview_window("launcher") {
            let _ = launcher.hide();
        }
    });
    Ok(port)
}

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
pub async fn set_cache_limit(
    state: tauri::State<'_, AppState>,
    bytes: u64,
) -> Result<CacheStats, String> {
    state.set_limit(bytes).await?;
    Ok(state.cache.stats())
}

#[tauri::command]
pub async fn export_local(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: ExportRequest,
) -> Result<ExportReport, String> {
    // Take the active listener's coherent profile/client snapshot before doing any IO.
    let snapshot = {
        let active = state.active().ok_or("没有已启用的服务器配置")?;
        let servers = state.servers.lock().unwrap_or_else(|p| p.into_inner());
        servers
            .get(&active.id)
            .ok_or("本地服务尚未启动")?
            .ctx
            .snapshot()
    };
    let emitter = app.clone();
    let report = exporter::run(
        &snapshot.client,
        &snapshot.profile,
        state.cache.clone(),
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

#[tauri::command]
pub fn choose_save_path(app: AppHandle, suggested: String) -> Option<String> {
    crate::download::ask_save_path(&app, &suggested).map(|path| path.to_string_lossy().to_string())
}

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
pub fn open_settings(app: AppHandle) {
    crate::open_shell(&app, "settings.html");
}
