//! Read local catalogs only when Settings requests them; catalogs do not prove reachability.
use crate::{commands::{AppState, BootPayload}, codex::CodexRequest, profiles::ProfileStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::{Path, PathBuf}, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalModel {
    pub id: String,
    pub label: Option<String>,
    pub source: String,
    pub vision: Option<bool>,
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}
fn json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
fn add(out: &mut Vec<LocalModel>, id: &str, row: &Value, source: &str) {
    let id = id.trim();
    if id.starts_with("ccs-") || id.is_empty() || id.contains("YOUR_MODEL_HERE") { return; }
    let modalities = ["input_modalities", "inputModalities", "input"].iter()
        .find_map(|k| row.get(k).and_then(Value::as_array))
        .or_else(|| row.pointer("/modalities/input").and_then(Value::as_array));
    let vision = modalities.filter(|m| !m.is_empty() && m.iter().all(Value::is_string)).map(|m| m.iter().any(|v| v.as_str() == Some("image")))
        .or_else(|| row.get("vision").and_then(Value::as_bool));
    let label = ["display_name", "displayName", "name", "label"].iter()
        .find_map(|k| row.get(k).and_then(Value::as_str)).map(str::to_owned);
    out.push(LocalModel { id: id.into(), label, source: source.into(), vision });
}
fn catalog(out: &mut Vec<LocalModel>, value: &Value, source: &str, prefix: &str) {
    if let Some(models) = value.get("models") {
        match models {
            Value::Array(rows) => for row in rows {
                if let Some(id) = ["slug", "model", "id"].iter().find_map(|k| row.get(k).and_then(Value::as_str)) {
                    add(out, &format!("{prefix}{id}"), row, source);
                }
            },
            Value::Object(rows) => for (id, row) in rows { add(out, &format!("{prefix}{id}"), row, source); },
            _ => {}
        }
    }
    if let Some(v) = value.get("modelCatalog") { catalog(out, v, source, prefix); }
}
fn toml_model(out: &mut Vec<LocalModel>, text: &str, source: &str) {
    if let Ok(v) = text.parse::<toml::Value>() {
        if let Some(id) = v.get("model").and_then(toml::Value::as_str) { add(out, id, &Value::Null, source); }
    }
}
fn dsh_patch(out: &mut Vec<LocalModel>, text: &str, source: &str) {
    // Profile patches are data; never evaluate Cordis JavaScript tags from composed dumps.
    if let Ok(Value::Array(entries)) = serde_yaml_ng::from_str::<Value>(text) {
        for entry in entries {
            if let Some(providers) = entry.pointer("/config/providers").and_then(Value::as_object) {
                for (provider, value) in providers {
                    catalog(out, value, &format!("{source} provider:{provider}"), "");
                }
            }
        }
    }
}

/// SQLite is necessary for provider-specific catalogs absent from the JSON exports.
/// Read-only mode and a short busy timeout keep discovery from changing or blocking CC Switch.
fn providers(out: &mut Vec<LocalModel>, root: &Path, backend: &str) {
    let Ok(db) = rusqlite::Connection::open_with_flags(root.join("cc-switch.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX) else { return; };
    let _ = db.busy_timeout(Duration::from_millis(100));
    let Ok(mut query) = db.prepare("SELECT id, settings_config, meta FROM providers WHERE app_type = ?1") else { return; };
    let Ok(rows) = query.query_map([backend], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))) else { return; };
    let live = json(&root.join("live-state.json")).unwrap_or(Value::Null);
    let route = &live["apps"][backend];
    for (id, settings, meta) in rows.flatten() {
        let source = format!("cc-switch.db provider:{id} (local catalog)");
        for text in [settings, meta].into_iter().flatten() {
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue; };
            catalog(out, &v, &source, "");
            if backend == "codex" {
                if let Some(config) = v.get("config").and_then(Value::as_str) { toml_model(out, config, &source); }
                if let Some(keys) = route.pointer("/stack/keys").and_then(Value::as_object) {
                    for (key, provider) in keys {
                        if provider.as_str() == Some(&id) { catalog(out, &v, &format!("{source} route:{key}"), ""); }
                    }
                }
            }
        }
    }
}

pub fn discover_at(home: &Path, backend: &str) -> Vec<LocalModel> {
    let mut out = Vec::new();
    let cc = home.join(".cc-switch");
    match backend {
        "codex" => {
            for path in [home.join(".codex/cc-switch-model-catalog.json"), home.join(".codex/models_cache.json")] {
                if let Some(v) = json(&path) { catalog(&mut out, &v, path.file_name().unwrap().to_str().unwrap_or("codex catalog"), ""); }
            }
            if let Some(Value::Object(accounts)) = json(&cc.join("codex-official-models.json")) {
                for value in accounts.values() { catalog(&mut out, value, "codex-official-models.json (cached official catalog)", ""); }
            }
            if let Ok(text) = std::fs::read_to_string(home.join(".codex/config.toml")) { toml_model(&mut out, &text, ".codex/config.toml"); }
        }
        "dsh" => if let Ok(entries) = std::fs::read_dir(home.join(".dsh/profiles")) {
            for entry in entries.flatten() {
                let path = entry.path().join("cordis.patch.yml");
                if let Ok(text) = std::fs::read_to_string(&path) {
                    dsh_patch(&mut out, &text, &format!(".dsh/profiles/{}/cordis.patch.yml", entry.file_name().to_string_lossy()));
                }
            }
        },
        _ => return out,
    }
    providers(&mut out, &cc, backend);
    // The first explicit local metadata wins; conflicting declarations remain unknown.
    let mut unique: BTreeMap<String, LocalModel> = BTreeMap::new();
    let mut conflicts = std::collections::BTreeSet::new();
    for model in out {
        unique.entry(model.id.clone()).and_modify(|old| {
            if old.vision.is_none() { old.vision = model.vision; }
            else if model.vision.is_some() && old.vision != model.vision { old.vision = None; conflicts.insert(old.id.clone()); }
            if old.label.is_none() { old.label = model.label.clone(); }
            if !old.source.contains(&model.source) { old.source.push_str("; "); old.source.push_str(&model.source); }
        }).or_insert(model);
    }
    for id in conflicts { if let Some(model) = unique.get_mut(&id) { model.vision = None; } }
    unique.into_values().collect()
}

pub fn selected_model(request: Option<String>, saved: &str) -> Option<String> {
    request.filter(|s| !s.trim().is_empty()).or_else(|| (!saved.trim().is_empty()).then(|| saved.trim().to_owned()))
}
pub fn validate_images(store: &ProfileStore, backend: &str, req: &CodexRequest) -> Result<(), String> {
    if req.model.as_ref().is_some_and(|m| m.starts_with("ccs-")) { return Err("路由别名不是模型 ID".into()); }
    if !req.images.is_empty() {
        if backend != "codex" { return Err("此后端尚不支持图片输入".into()); }
        if let Some(id) = &req.model {
            if store.model_catalogs.get(backend).into_iter().flatten().any(|m| &m.id == id && m.vision == Some(false)) {
                return Err(format!("模型 {id} 已明确为仅文本，无法识别图片；请选择视觉模型"));
            }
        }
    }
    Ok(())
}
async fn discover(state: tauri::State<'_, AppState>, backend: &'static str) -> Result<Vec<LocalModel>, String> {
    let models = tokio::task::spawn_blocking(move || discover_at(&home(), backend)).await.map_err(|e| e.to_string())?;
    state.save_model_catalog(backend.into(), models.clone()).await?;
    Ok(models)
}
#[tauri::command]
pub async fn codex_models(state: tauri::State<'_, AppState>) -> Result<Vec<LocalModel>, String> { discover(state, "codex").await }
#[tauri::command]
pub async fn dsh_models(state: tauri::State<'_, AppState>) -> Result<Vec<LocalModel>, String> { discover(state, "dsh").await }
#[tauri::command]
pub async fn set_backend_model(state: tauri::State<'_, AppState>, backend: String, model: String) -> Result<BootPayload, String> {
    state.set_backend_model(backend, model).await?;
    Ok(crate::commands::payload(&state))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_is_explicit() {
        let mut out = vec![];
        catalog(&mut out, &serde_json::json!({"models":[{"slug":"image","input_modalities":["text","image"]},{"model":"text","inputModalities":["text"]},{"id":"unknown"}]}), "fixture", "");
        assert_eq!(out.iter().map(|m| m.vision).collect::<Vec<_>>(), vec![Some(true), Some(false), None]);
        assert_eq!(out.len(), 3);
    }
    #[test]
    fn persisted_selection_and_image_guard() {
        let mut store = ProfileStore::with_presets();
        store.model_catalogs.insert("codex".into(), vec![LocalModel { id:"text".into(), label:None, source:"fixture".into(), vision:Some(false) }]);
        let mut req = CodexRequest { prompt:"".into(), schema:"{}".into(), images:vec!["png".into()], model:Some("text".into()), timeout_ms:None };
        assert!(validate_images(&store, "codex", &req).is_err());
        req.model = Some("new".into());
        assert!(validate_images(&store, "codex", &req).is_ok());
        assert!(validate_images(&store, "agy", &req).is_err());
        assert_eq!(selected_model(None,"saved"),Some("saved".into()));
        assert_eq!(selected_model(Some("override".into()),"saved"),Some("override".into()));
        store.codex_model = "saved-codex".into();
        let roundtrip: ProfileStore = serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();
        assert_eq!(roundtrip.codex_model, "saved-codex");
        assert_eq!(roundtrip.model_catalogs["codex"][0].vision, Some(false));
        let mut v = serde_json::to_value(store).unwrap();
        for key in ["codex_model","dsh_model","agy_model","model_catalogs"] { v.as_object_mut().unwrap().remove(key); }
        let restored: ProfileStore = serde_json::from_value(v).unwrap();
        assert!(restored.codex_model.is_empty() && restored.agy_model.is_empty());
    }
    #[test]
    fn local_source_shapes_and_routes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("model-fixture-{}", std::process::id()));
        let cc = root.join(".cc-switch");
        std::fs::create_dir_all(&cc).unwrap();
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        std::fs::create_dir_all(root.join(".dsh/profiles/headless")).unwrap();
        std::fs::write(cc.join("codex-official-models.json"), r#"{"account-key":{"models":[{"slug":"official-image","display_name":"Official","input_modalities":["text","image"]}]}}"#).unwrap();
        std::fs::write(cc.join("live-state.json"), r#"{"apps":{"codex":{"stack":{"keys":{"route":"provider"}}}}}"#).unwrap();
        std::fs::write(cc.join("model-pricing.json"), r#"{"priced-only":{"input_cost":1}}"#).unwrap();
        std::fs::write(root.join(".codex/config.toml"), "model = 'configured'\n").unwrap();
        std::fs::write(root.join(".dsh/profiles/headless/cordis.patch.yml"), "- id: llm-pi-ai\n  script: !!js 'throw 42'\n  config:\n    providers:\n      local:\n        models:\n        - id: dsh-text\n          input: [text]\n        - id: dsh-unknown\n").unwrap();
        {
            let db = rusqlite::Connection::open(cc.join("cc-switch.db")).unwrap();
            db.execute_batch("CREATE TABLE providers (id TEXT, app_type TEXT, settings_config TEXT, meta TEXT);").unwrap();
            db.execute("INSERT INTO providers VALUES (?1, 'codex', ?2, ?3)", ["provider", r#"{"modelCatalog":{"models":[{"model":"provider-text","displayName":"Text","inputModalities":["text"]}]}}"#, r#"{"modelCatalog":{"models":[{"id":"meta-model"}]}}"#]).unwrap();
        }
        let db = rusqlite::Connection::open(cc.join("cc-switch.db")).unwrap();
        db.execute("INSERT INTO providers VALUES ('nullable', 'codex', NULL, ?1)", [r#"{"models":[{"id":"nullable-model"}]}"#]).unwrap();
        db.execute("INSERT INTO providers VALUES ('nullable2', 'codex', ?1, NULL)", [r#"{"models":[{"id":"nullable-settings"}]}"#]).unwrap();
        drop(db);
        let codex = discover_at(&root, "codex");
        assert!(codex.iter().any(|m| m.id == "nullable-model"));
        assert!(codex.iter().any(|m| m.id == "nullable-settings"));
        assert!(!codex.iter().any(|m| m.id.starts_with("ccs-")));
        for (id, vision) in [("official-image", Some(true)), ("configured", None), ("provider-text", Some(false)), ("meta-model", None)] {
            assert_eq!(codex.iter().find(|m| m.id == id).unwrap().vision, vision);
        }
        assert!(!codex.iter().any(|m| m.id == "priced-only"));
        assert!(codex.iter().find(|m| m.id == "provider-text").unwrap().source.contains("route:route"));
        let dsh = discover_at(&root, "dsh");
        assert_eq!(dsh.iter().find(|m| m.id == "dsh-text").unwrap().vision, Some(false));
        assert_eq!(dsh.iter().find(|m| m.id == "dsh-unknown").unwrap().vision, None);
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    #[ignore = "Read-only local catalog evidence, opt in explicitly"]
    fn real_local_discovery() {
        for backend in ["codex", "dsh", "agy"] {
            let models = discover_at(&home(), backend);
            println!("{backend}: {}", serde_json::to_string(&models).unwrap());
            if backend != "agy" { assert!(!models.is_empty()); }
            assert!(!models.iter().any(|m| m.id.starts_with("ccs-")));
            assert!(!models.iter().any(|m| m.id.contains("YOUR_MODEL_HERE")));
        }
    }
}
