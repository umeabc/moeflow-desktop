//! Claude uses file-backed stdin because long label lists exceed Windows argv limits.
use crate::codex::{CodexCandidate, CodexReply, CodexRequest, CodexStatus};
use crate::commands::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

/// Resolve npm shims to native targets rather than passing user data through a shell.
fn native(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd")) {
        if let Some(parent) = path.parent() {
            for suffix in ["node_modules/@anthropic-ai/claude-code/bin/claude.exe", "node_modules/@anthropic-ai/claude-code/cli.exe"] {
                let target = parent.join(suffix);
                if target.is_file() { return target; }
            }
        }
    }
    path
}
fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(p) = std::env::var_os("CLAUDE_BIN") { paths.push(native(p.into())); }
    if let Some(p) = std::env::var_os("APPDATA") { paths.push(native(PathBuf::from(p).join("npm/claude.cmd"))); }
    if let Some(p) = std::env::var_os("USERPROFILE") { paths.push(PathBuf::from(p).join("Downloads/claude-offline-packages-windows/node_modules/@anthropic-ai/claude-code/bin/claude.exe")); }
    if let Some(p) = std::env::var_os("LOCALAPPDATA") { paths.push(PathBuf::from(p).join("Programs/claude/claude.exe")); }
    if let Some(p) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&p) {
            paths.push(dir.join(if cfg!(windows) { "claude.exe" } else { "claude" }));
            paths.push(native(dir.join("claude.cmd")));
        }
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for path in paths {
        if path.is_file() && !is_shim(&path) && !found.iter().any(|p| p.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy())) { found.push(path); }
    }
    found
}
fn resolve(configured: &str) -> PathBuf {
    if !configured.trim().is_empty() { return native(PathBuf::from(configured.trim())); }
    if let Some(p) = std::env::var_os("CLAUDE_BIN") { return native(p.into()); }
    candidates().into_iter().next().unwrap_or_else(|| PathBuf::from("claude"))
}
fn is_shim(path: &Path) -> bool {
    matches!(path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase().as_str(), "cmd" | "bat" | "ps1")
}
fn command(bin: &Path) -> Result<tokio::process::Command, String> {
    if is_shim(bin) { return Err("Claude 不能使用 .cmd/.ps1 包装脚本；请选择 claude.exe".into()); }
    let mut cmd = std::process::Command::new(bin);
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    Ok(cmd)
}

async fn probe_at(bin: &Path) -> Result<String, String> {
    let output = tokio::time::timeout(Duration::from_secs(10), command(bin)?.arg("--version").output())
        .await.map_err(|_| "Claude 检测超时".to_string())?.map_err(|e| format!("无法启动 Claude：{e}"))?;
    if !output.status.success() { return Err(format!("claude 检测失败：{}", String::from_utf8_lossy(&output.stderr))); }
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
pub async fn claude_status(state: tauri::State<'_, AppState>) -> Result<CodexStatus, String> {
    Ok(status(&state.store_snapshot().claude_path).await)
}
#[tauri::command]
pub async fn claude_detect(state: tauri::State<'_, AppState>, save: bool) -> Result<CodexStatus, String> {
    let report = status("").await;
    if save {
        if let Some(winner) = report.candidates.iter().find(|c| c.version.is_some()) {
            state.set_claude_path(winner.path.clone()).await?;
            return Ok(status(&winner.path).await);
        }
    }
    Ok(report)
}
#[tauri::command]
pub async fn claude_probe(state: tauri::State<'_, AppState>) -> Result<String, String> {
    probe_at(&resolve(&state.store_snapshot().claude_path)).await
}
#[tauri::command]
pub async fn claude_run(state: tauri::State<'_, AppState>, req: CodexRequest) -> Result<CodexReply, String> {
    run_with(&resolve(&state.store_snapshot().claude_path), req).await
}

fn build_prompt(req: &CodexRequest, retry: bool) -> Result<String, String> {
    if !req.images.is_empty() { return Err("Claude 视觉尚未验证，仅支持已有标签的文字翻译".into()); }
    let instruction = if retry { "ONLY the JSON object: " } else { "Return ONLY this JSON object matching the schema, no prose, no fences: " };
    Ok(format!("{}\n\n{instruction}{}", req.prompt, req.schema))
}

/// The outer object describes CLI execution; only its result is assistant text.
fn assistant_json(output: &str) -> Result<Option<String>, String> {
    let envelope: serde_json::Value = serde_json::from_str(output.trim()).map_err(|e| format!("Claude 输出不是 JSON：{e}"))?;
    if envelope["is_error"].as_bool() == Some(true) {
        return Err(format!("Claude 返回错误：{}", envelope["result"].as_str().unwrap_or("未知错误")));
    }
    if envelope["type"] != "result" { return Err("Claude 输出缺少 result 信封".into()); }
    let text = envelope["result"].as_str().ok_or("Claude 输出缺少 result 字符串")?;
    Ok(extract_json(text))
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
    let deadline = tokio::time::Instant::now() + Duration::from_millis(req.timeout_ms.unwrap_or(180_000).clamp(1, 180_000));
    let dir = loop {
        let path = std::env::temp_dir().join(format!("moeflow-claude-{}-{}", std::process::id(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
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
        let prompt_file = dir.0.join("prompt.txt");
        std::fs::write(&prompt_file, prompt).map_err(|e| e.to_string())?;
        // File-backed stdin avoids Windows argv limits and pipe deadlocks for long labels.
        let mut cmd = command(bin)?;
        cmd.current_dir(&dir.0).args(["-p", "--output-format", "json", "--tools", "Read", "--allowedTools", "Read", "--permission-mode", "dontAsk", "--no-session-persistence"])
            .arg("--add-dir").arg(&dir.0);
        if let Some(model) = &req.model { cmd.arg("--model").arg(model); }
        let mut child = cmd.stdin(std::fs::File::open(&prompt_file).map_err(|e| e.to_string())?)
            .stdout(std::fs::File::create(&stdout).map_err(|e| e.to_string())?)
            .stderr(std::fs::File::create(&stderr).map_err(|e| e.to_string())?)
            .spawn().map_err(|e| format!("无法启动 Claude：{e}"))?;
        let result = tokio::time::timeout_at(deadline, child.wait()).await;
        let status = match result {
            Ok(Ok(status)) => status,
            other => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(match other { Ok(Err(e)) => e.to_string(), _ => "claude 超时，已终止任务".into() });
            }
        };
        if !status.success() {
            // API failures also use the JSON envelope, often with an empty stderr.
            let output = tokio::fs::read_to_string(&stdout).await.unwrap_or_default();
            if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&output) {
                if envelope["is_error"].as_bool() == Some(true) {
                    return Err(format!("Claude 返回错误：{}", envelope["result"].as_str().unwrap_or("未知错误")));
                }
            }
            let bytes = tokio::fs::read(&stderr).await.unwrap_or_default();
            return Err(format!("claude 退出 {status}：{}", String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4000)..])));
        }
        let output = tokio::fs::read_to_string(&stdout).await.map_err(|e| e.to_string())?;
        if let Some(json) = assistant_json(&output)? {
            return Ok(CodexReply { json, ms: started.elapsed().as_millis() as u64 });
        }
    }
    Err("claude 未返回有效 JSON 对象（已重试一次）".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> CodexRequest {
        CodexRequest { prompt: "Return only JSON with answer equal to ok".into(), schema: r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#.into(), images: vec![], model: std::env::var("MOEFLOW_CLAUDE_TEST_MODEL").ok(), timeout_ms: None }
    }
    #[test]
    fn parses_envelope_and_inner_json() {
        let output = serde_json::json!({"type":"result","is_error":false,"result":"prose {bad} ```json\n{\"answer\":\"brace } and \\\"quote\\\"\"}\n```"}).to_string();
        let json = assistant_json(&output).unwrap().unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
        assert!(assistant_json(r#"{"type":"result","is_error":true,"result":"API Error"}"#).is_err());
        assert!(assistant_json(r#"{"type":"result"}"#).is_err());
        assert!(assistant_json(r#"{"type":"result","result":"no object"}"#).unwrap().is_none());
        assert!(assistant_json("broken").is_err());
    }
    #[test]
    fn rejects_shims_and_unverified_vision() {
        for p in ["claude.cmd", "claude.PS1", "claude.bat"] { assert!(command(Path::new(p)).is_err()); }
        let mut req = request();
        assert!(build_prompt(&req, false).unwrap().contains(&req.schema));
        req.images.push("image".into());
        assert!(build_prompt(&req, false).is_err());
    }
    #[tokio::test]
    async fn rejects_concurrent_run() {
        let _guard = RUNNING.lock().await;
        assert!(run_with(Path::new("missing"), request()).await.unwrap_err().contains("已有任务在跑"));
    }
    #[ignore = "needs a configured Claude connection and spends quota"]
    #[tokio::test]
    async fn runs_claude_and_returns_json() {
        let bin = resolve("");
        println!("version={}", probe_at(&bin).await.unwrap());
        let reply = run_with(&bin, request()).await.expect("real Claude must return assistant JSON");
        println!("json={} ms={}", reply.json, reply.ms);
        let value: serde_json::Value = serde_json::from_str(&reply.json).unwrap();
        assert_eq!(value["answer"], "ok");
    }
}
