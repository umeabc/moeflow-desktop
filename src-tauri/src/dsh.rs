//! Dsh receives text through argv, so neither shell shims nor unbounded prompts are safe.
use crate::codex::{CodexCandidate, CodexReply, CodexRequest, CodexStatus};
use crate::commands::AppState;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("DSH_BIN") { paths.push(PathBuf::from(path)); }
    if let Some(path) = std::env::var_os("APPDATA") {
        paths.push(PathBuf::from(path).join("npm/node_modules/@deepseek-ai/dsh/lib/bin.js"));
    }
    paths.retain(|p| p.is_file());
    paths.dedup();
    paths
}

fn resolve(configured: &str) -> PathBuf {
    if !configured.trim().is_empty() { return PathBuf::from(configured.trim()); }
    if let Some(path) = std::env::var_os("DSH_BIN") { return PathBuf::from(path); }
    candidates().into_iter().next().unwrap_or_else(|| PathBuf::from("bin.js"))
}

/// dsh is an npm package, so it always needs a Node runtime — and a GUI process launched
/// from Explorer inherits the machine PATH, not the one a developer test opened from.
fn node_exe() -> PathBuf {
    if let Some(path) = std::env::var_os("NODE_BIN") {
        return PathBuf::from(path);
    }
    let mut candidates = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(pf).join("nodejs/node.exe"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs/nodejs/node.exe"));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "node.exe" } else { "node" }))
}

fn command(bin: &Path) -> Result<tokio::process::Command, String> {
    let ext = bin.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    if matches!(ext.as_str(), "cmd" | "bat" | "ps1") {
        return Err("dsh 不能使用 .cmd/.ps1 包装脚本；请选择 @deepseek-ai/dsh/lib/bin.js".into());
    }
    if !bin.is_file() { return Err(format!("找不到 {}；请在「设置 → 本地 dsh」指定 bin.js", bin.display())); }
    let mut cmd = std::process::Command::new(node_exe());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.arg(bin).stdin(Stdio::null());
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    Ok(cmd)
}

async fn probe_at(bin: &Path) -> Result<String, String> {
    let output = tokio::time::timeout(Duration::from_secs(10), command(bin)?.arg("--version").output())
        .await.map_err(|_| "dsh 检测超时".to_string())?.map_err(|e| format!("无法启动 dsh/node：{e}"))?;
    if !output.status.success() { return Err(format!("dsh 检测失败：{}", String::from_utf8_lossy(&output.stderr))); }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn status(configured: &str) -> CodexStatus {
    let mut probed = Vec::new();
    for path in candidates() {
        let result = probe_at(&path).await;
        probed.push(CodexCandidate { path: path.to_string_lossy().into_owned(),
            version: result.as_ref().ok().cloned(), error: result.err() });
    }
    let target = resolve(configured);
    let result = probe_at(&target).await;
    CodexStatus { configured: configured.trim().into(), resolved: target.is_file().then(|| target.to_string_lossy().into_owned()),
        version: result.as_ref().ok().cloned(), error: result.err(), candidates: probed }
}

#[tauri::command]
pub async fn dsh_status(state: tauri::State<'_, AppState>) -> Result<CodexStatus, String> {
    Ok(status(&state.store_snapshot().dsh_path).await)
}
#[tauri::command]
pub async fn dsh_detect(state: tauri::State<'_, AppState>, save: bool) -> Result<CodexStatus, String> {
    let report = status("").await;
    if save {
        if let Some(winner) = report.candidates.iter().find(|c| c.version.is_some()) {
            state.set_dsh_path(winner.path.clone()).await?;
            return Ok(status(&winner.path).await);
        }
    }
    Ok(report)
}
#[tauri::command]
pub async fn dsh_probe(state: tauri::State<'_, AppState>) -> Result<String, String> {
    probe_at(&resolve(&state.store_snapshot().dsh_path)).await
}
#[tauri::command]
pub async fn dsh_run(state: tauri::State<'_, AppState>, req: CodexRequest) -> Result<CodexReply, String> {
    run_with(&resolve(&state.store_snapshot().dsh_path), req).await
}

fn build_prompt(req: &CodexRequest, retry: bool) -> Result<String, String> {
    if !req.images.is_empty() { return Err("dsh 无视觉，仅支持已有标签的文字翻译，不能接收图片或创建标签".into()); }
    let instruction = if retry { "ONLY the JSON object: " } else { "Return ONLY this JSON object, no prose, no fences: " };
    let prompt = format!("{}\n\n{instruction}{}", req.prompt, req.schema);
    validate_argv(&prompt)?;
    Ok(prompt)
}

/// Count Windows UTF-16 units and quoting overhead, leaving room for node and bin.js.
fn validate_argv(prompt: &str) -> Result<(), String> {
    let units = prompt.encode_utf16().count() + prompt.chars().filter(|c| matches!(c, '"' | '\\')).count();
    if prompt.contains('\0') || units > 30_000 {
        return Err("dsh 提示词过长（含 JSON schema 和参数转义最多约 30000 字符）或包含空字符，请减少标签内容".into());
    }
    Ok(())
}

/// Scan balanced objects while respecting JSON strings, tolerating prose and fences.
fn extract_json(text: &str) -> Option<String> {
    for (start, ch) in text.char_indices() {
        if ch != '{' { continue; }
        let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
        for (offset, ch) in text[start..].char_indices() {
            if quoted {
                if escaped { escaped = false; }
                else if ch == '\\' { escaped = true; }
                else if ch == '"' { quoted = false; }
            } else {
                match ch {
                    '"' => quoted = true,
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let json = &text[start..=start + offset];
                            if serde_json::from_str::<serde_json::Value>(json).is_ok() { return Some(json.into()); }
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

async fn run_with(bin: &Path, req: CodexRequest) -> Result<CodexReply, String> {
    let _lock = RUNNING.try_lock().map_err(|_| "已有任务在跑".to_string())?;
    let prompt = build_prompt(&req, false)?;
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(req.timeout_ms.unwrap_or(120_000).clamp(1, 120_000));
    let dir = loop {
        let path = std::env::temp_dir().join(format!("moeflow-dsh-{}-{}", std::process::id(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        match std::fs::create_dir(&path) {
            Ok(()) => break Scratch(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    };
    for attempt in 0..2 {
        let prompt = if attempt == 0 { prompt.clone() } else { build_prompt(&req, true)? };
        let stdout = dir.0.join("stdout.txt");
        let stderr = dir.0.join("stderr.txt");
        // Files let a chatty CLI finish without filling an unread pipe.
        let mut child = command(bin)?.args(["--profile", "headless"]).arg(prompt)
            .stdout(std::fs::File::create(&stdout).map_err(|e| e.to_string())?)
            .stderr(std::fs::File::create(&stderr).map_err(|e| e.to_string())?)
            .spawn().map_err(|e| format!("无法启动 dsh/node：{e}"))?;
        let result = tokio::time::timeout_at(deadline, child.wait()).await;
        let status = match result {
            Ok(Ok(status)) => status,
            other => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(match other { Ok(Err(e)) => e.to_string(), _ => "dsh 超时，已终止任务".into() });
            }
        };
        if !status.success() {
            let bytes = tokio::fs::read(&stderr).await.unwrap_or_default();
            return Err(format!("dsh 退出 {status}：{}", String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4000)..])));
        }
        let output = tokio::fs::read_to_string(&stdout).await.map_err(|e| e.to_string())?;
        if let Some(json) = extract_json(&output) {
            return Ok(CodexReply { json, ms: started.elapsed().as_millis() as u64 });
        }
    }
    Err("dsh 未返回有效 JSON 对象（已重试一次）".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> CodexRequest {
        CodexRequest { prompt: "Return answer ok".into(), schema: r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#.into(), images: vec![], model: None, timeout_ms: None }
    }
    #[test]
    fn prompt_contract_and_vision() {
        let mut req = request();
        let prompt = build_prompt(&req, false).unwrap();
        assert!(prompt.contains(&req.schema));
        assert!(prompt.contains("Return ONLY this JSON object, no prose, no fences:"));
        assert!(build_prompt(&req, true).unwrap().len() < prompt.len());
        req.images.push("image".into());
        assert!(build_prompt(&req, false).unwrap_err().contains("无视觉"));
    }
    #[test]
    fn extracts_objects_with_escaped_strings() {
        let json = r#"{"texts":[{"translated":"brace } and \"quote\" and \\"}]}"#;
        assert_eq!(extract_json(&format!("prose {{bad}}\n```json\n{json}\n```")), Some(json.into()));
        assert_eq!(extract_json("no JSON {broken"), None);
        assert_eq!(extract_json("{}"), Some("{}".into()));
    }
    #[test]
    fn limits_windows_argv() {
        assert!(validate_argv(&"a".repeat(30_000)).is_ok());
        assert!(validate_argv(&"a".repeat(30_001)).is_err());
        assert!(validate_argv(&"😀".repeat(15_001)).is_err());
        assert!(validate_argv(&"\"".repeat(15_001)).is_err());
        assert!(validate_argv("a\0b").is_err());
        let mut req = request(); req.prompt = "a".repeat(30_000);
        assert!(build_prompt(&req, false).is_err());
    }
    #[test]
    fn rejects_shell_shims() {
        for path in ["dsh.cmd", "dsh.PS1", "dsh.bat"] { assert!(command(Path::new(path)).is_err()); }
    }
    #[tokio::test]
    async fn rejects_concurrent_run() {
        let _guard = RUNNING.lock().await;
        assert!(run_with(Path::new("missing"), request()).await.unwrap_err().contains("已有任务在跑"));
    }
    #[ignore = "needs a logged-in dsh CLI and spends quota"]
    #[tokio::test]
    async fn runs_dsh_and_returns_json() {
        use tauri::Manager;
        let dir = Scratch(std::env::temp_dir().join(format!("moeflow-dsh-test-{}", std::process::id())));
        std::fs::create_dir(&dir.0).unwrap();
        let state = AppState::new(dir.0.clone(), crate::profiles::ProfileStore::with_presets(),
            std::sync::Arc::new(crate::media::MediaCache::new(dir.0.join("cache"), 1024)), 0).unwrap();
        let app = tauri::test::mock_builder().build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
        app.manage(state);
        let reply = dsh_run(app.state(), request()).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&reply.json).unwrap();
        assert_eq!(value["answer"], "ok");
    }
}
