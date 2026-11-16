//! One bounded ACP conversation per request; no prompt is placed in argv.
use crate::{codex::{CodexCandidate, CodexReply, CodexRequest, CodexStatus}, commands::AppState};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, process::Stdio, time::{Duration, Instant}};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Listing models is a second ACP handshake; it must not collide with an inference run.
static LISTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static NEXT_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Local stand-in for the real Antigravity CLI. Resolved from the profile so the shipped
/// binary contains no machine-specific path, and simply absent on machines that never
/// installed the bridge.
fn fallback() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| {
        PathBuf::from(home).join("Documents/deepseek-harness/default-workspace/agy-bridge/dist-tunnel/agy.exe")
    })
}
const LIMIT: usize = 16 * 1024 * 1024;
fn is_shim(p: &Path) -> bool {
    matches!(p.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase().as_str(), "cmd" | "bat" | "ps1")
}
fn candidates(configured: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(p) = std::env::var_os("AGY_BIN") { paths.push(PathBuf::from(p)); }
    if let Some(p) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&p) { for name in ["agy.exe", "agy"] { paths.push(dir.join(name)); } }
    }
    if let Some(path) = fallback() { paths.push(path); }
    if !configured.trim().is_empty() { paths.push(PathBuf::from(configured.trim())); }
    let mut found: Vec<PathBuf> = vec![];
    for p in paths {
        if p.is_file() && !is_shim(&p) && !found.iter().any(|old| old.to_string_lossy().eq_ignore_ascii_case(&p.to_string_lossy())) { found.push(p); }
    }
    found
}
pub(crate) fn validate_path(configured: &str) -> Result<(), String> {
    if !configured.trim().is_empty() && is_shim(Path::new(configured.trim())) { return Err("agy 不支持 .cmd/.bat/.ps1，请选择原生可执行文件".into()); }
    Ok(())
}
fn resolve(configured: &str) -> Result<PathBuf, String> {
    validate_path(configured)?;
    if let Some(p) = std::env::var_os("AGY_BIN") {
        let p = PathBuf::from(p);
        if is_shim(&p) { return Err("AGY_BIN 不支持包装脚本".into()); }
        return Ok(p);
    }
    candidates(configured).into_iter().next().ok_or_else(|| "未找到 agy，请配置原生可执行文件路径".into())
}
fn command(bin: &Path) -> Result<tokio::process::Command, String> {
    if is_shim(bin) { return Err("agy 不支持包装脚本".into()); }
    let mut cmd = std::process::Command::new(bin);
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.arg("acp").kill_on_drop(true).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    Ok(cmd)
}
struct Acp {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    bytes: usize,
    /// ACP failures (missing login, tunnel down) surface on stderr, not in JSON-RPC.
    stderr_path: PathBuf,
}
impl Drop for Acp {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.stderr_path); }
}
impl Acp {
    fn spawn(bin: &Path) -> Result<Self, String> {
        let stderr_path = std::env::temp_dir().join(format!(
            "agy-acp-{}-{}.log", std::process::id(),
            NEXT_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let stderr = std::fs::File::create(&stderr_path).map_err(|e| format!("无法创建 agy 日志：{e}"))?;
        let mut child = command(bin)?.stderr(Stdio::from(stderr)).spawn()
            .map_err(|e| format!("无法启动 agy：{e}"))?;
        Ok(Self { input: child.stdin.take().unwrap(), output: BufReader::new(child.stdout.take().unwrap()), child, bytes: 0, stderr_path })
    }
    fn stderr_tail(&self) -> String {
        std::fs::read(&self.stderr_path).ok()
            .map(|b| String::from_utf8_lossy(&b[b.len().saturating_sub(2000)..]).trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "agy 没有输出 stderr".into())
    }
    async fn call(&mut self, id: u64, method: &str, params: Value, text: &mut String) -> Result<Value, String> {
        let mut bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).map_err(|e| e.to_string())?;
        if bytes.len() > LIMIT { return Err("agy 请求超过 16 MiB 限制".into()); }
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.map_err(|e| e.to_string())?;
        self.input.flush().await.map_err(|e| e.to_string())?;
        loop {
            // fill_buf bounds each frame before allocating, including malformed output.
            let mut line = Vec::new();
            loop {
                let buf = self.output.fill_buf().await.map_err(|e| e.to_string())?;
                if buf.is_empty() { let tail = self.stderr_tail(); return Err(format!("agy ACP 在返回结果前关闭：{tail}")); }
                let n = buf.iter().position(|b| *b == b'\n').map_or(buf.len(), |n| n+1);
                self.bytes += n;
                if self.bytes > LIMIT { return Err("agy 输出超过 16 MiB 限制".into()); }
                line.extend_from_slice(&buf[..n]);
                self.output.consume(n);
                if line.last() == Some(&b'\n') { break; }
            }
            let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue; };
            if v["method"] == "session/update" && v["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
                if let Some(chunk) = v["params"]["update"]["content"]["text"].as_str() { text.push_str(chunk); }
            }
            if v["id"].as_u64() == Some(id) {
                if let Some(e) = v.get("error") { return Err(format!("agy ACP {method}：{e}")); }
                return v.get("result").cloned().ok_or_else(|| "agy ACP 缺少 result".into());
            }
            // Deny agent-initiated operations; translation never grants workspace tools.
            if let Some(request_id) = v.get("id") {
                if v["method"] == "session/request_permission" {
                    let reply = json!({"jsonrpc":"2.0","id":request_id,"result":{"outcome":{"outcome":"cancelled"}}}).to_string()+"\n";
                    self.input.write_all(reply.as_bytes()).await.map_err(|e| e.to_string())?;
                    continue;
                }
                let reply = json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"Client operation unavailable"}}).to_string()+"\n";
                self.input.write_all(reply.as_bytes()).await.map_err(|e| e.to_string())?;
            }
        }
    }
    async fn open(&mut self) -> Result<(Value, Value), String> {
        let init = self.call(1, "initialize", json!({"protocolVersion":1}), &mut String::new()).await?;
        if init["protocolVersion"] != 1 { return Err("agy ACP 协议版本不兼容".into()); }
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let session = self.call(2, "session/new", json!({"cwd":cwd,"mcpServers":[]}), &mut String::new()).await?;
        Ok((init, session))
    }
}
async fn probe_at(bin: &Path) -> Result<String, String> {
    let mut acp = Acp::spawn(bin)?;
    let result = tokio::time::timeout(Duration::from_secs(20), acp.open()).await;
    let _ = acp.child.kill().await;
    let _ = acp.child.wait().await;
    let (init, session) = result.map_err(|_| "agy 检测超时".to_string())??;
    Ok(format!("{} {} · ACP 1 · 默认模型 {}", init["agentInfo"]["name"].as_str().unwrap_or("agy"), init["agentInfo"]["version"].as_str().unwrap_or("?"), session["models"]["currentModelId"].as_str().unwrap_or("未知")))
}
async fn status(configured: &str) -> CodexStatus {
    let mut probed = vec![];
    for p in candidates(configured) {
        let r = probe_at(&p).await;
        probed.push(CodexCandidate { path:p.to_string_lossy().into_owned(), version:r.as_ref().ok().cloned(), error:r.err() });
    }
    let target = resolve(configured);
    let result = match &target { Ok(p) => probe_at(p).await, Err(e) => Err(e.clone()) };
    CodexStatus { configured:configured.into(), resolved:target.ok().map(|p| p.to_string_lossy().into_owned()), version:result.as_ref().ok().cloned(), error:result.err(), candidates:probed }
}
#[tauri::command]
pub async fn agy_status(state: tauri::State<'_, AppState>) -> Result<CodexStatus, String> { Ok(status(&state.store_snapshot().agy_path).await) }
#[tauri::command]
pub async fn agy_probe(state: tauri::State<'_, AppState>) -> Result<String, String> { probe_at(&resolve(&state.store_snapshot().agy_path)?).await }
#[tauri::command]
pub async fn agy_detect(state: tauri::State<'_, AppState>, save: bool) -> Result<CodexStatus, String> {
    let mut report = status(&state.store_snapshot().agy_path).await;
    if save && report.version.is_some() {
        if let Some(path) = report.resolved.clone() {
            state.set_agy_path(path.clone()).await?;
            report.configured = path;
        }
    }
    Ok(report)
}
#[tauri::command]
pub async fn agy_run(state: tauri::State<'_, AppState>, mut req: CodexRequest) -> Result<CodexReply, String> {
    let store = state.store_snapshot();
    req.model = crate::models::selected_model(req.model, &store.agy_model);
    run_with(&resolve(&store.agy_path)?, req).await
}
async fn run_with(bin: &Path, req: CodexRequest) -> Result<CodexReply, String> {
    let _lock = RUNNING.try_lock().map_err(|_| "已有任务在运行".to_string())?;
    if req.prompt.len()+req.schema.len()+req.images.iter().map(String::len).sum::<usize>() > LIMIT/2 { return Err("agy 输入过长".into()); }
    let started = Instant::now();
    let mut acp = Acp::spawn(bin)?;
    let result = tokio::time::timeout(Duration::from_millis(req.timeout_ms.unwrap_or(180_000).clamp(1,180_000)), async {
        let (init, session) = acp.open().await?;
        #[cfg(test)]
        println!("initialize={} session/new.models={}",init,session["models"]);
        let sid = session["sessionId"].as_str().ok_or("agy 缺少 sessionId")?;
        if let Some(model) = &req.model {
            let available = session["models"]["availableModels"].as_array().ok_or("agy 未提供模型选择能力")?;
            if !available.iter().any(|v| v["modelId"].as_str() == Some(model)) { return Err("agy 不支持此模型 ID".into()); }
            let selected = acp.call(4,"session/set_model",json!({"sessionId":sid,"modelId":model}),&mut String::new()).await?;
            #[cfg(test)]
            println!("session/set_model modelId={model} result={selected}");
            #[cfg(not(test))]
            let _ = selected;
        }
        if !req.images.is_empty() && (!verified_vision(&init, req.model.as_deref()) || init["agentCapabilities"]["promptCapabilities"]["image"] != true) {
            return Err("agy 当前模型视觉未验证，仅支持已有标签翻译；可选择已验证的 gemini-3.8-flash-high".into());
        }
        let mut prompt = vec![json!({"type":"text","text":format!("{}\nDo not use tools, execute commands, or modify files. Return ONLY a JSON object matching this schema, no prose or fences:\n{}",req.prompt,req.schema)})];
        for image in &req.images {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(image).map_err(|_| "agy 图片不是有效 base64")?;
            prompt.push(json!({"type":"image","mimeType":"image/png","data":image}));
        }
        let mut text = String::new();
        acp.call(3,"session/prompt",json!({"sessionId":sid,"prompt":prompt}),&mut text).await?;
        let json = extract_json(&text).ok_or("agy 未返回有效 JSON 对象")?;
        Ok(CodexReply { json, ms:started.elapsed().as_millis() as u64 })
    }).await.map_err(|_| "agy 超时，已终止任务".to_string()).and_then(|r| r);
    let _ = acp.child.kill().await;
    let _ = acp.child.wait().await;
    result
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


// Capability flags alone do not prove that a model receives image pixels.
/// Models whose image path was actually exercised against this ACP server.
///
/// The handshake advertises `promptCapabilities.image`, which only proves the transport
/// accepts an image block — not that a given model reads it. Pinning the *server version*
/// was worse still: a routine bridge update silently turned vision off. So: capability
/// must be advertised, and the model must be one that was really tested.
const VERIFIED_VISION: &[&str] = &["gemini-3.8-flash-high"];

fn verified_vision(init: &Value, model: Option<&str>) -> bool {
    init["agentCapabilities"]["promptCapabilities"]["image"] == true
        && model.is_some_and(|m| VERIFIED_VISION.contains(&m))
}
#[tauri::command]
pub async fn agy_models(state: tauri::State<'_, AppState>) -> Result<Vec<crate::models::LocalModel>, String> {
    let _lock = LISTING.try_lock().map_err(|_| "正在读取 agy 模型目录".to_string())?;
    let mut acp = Acp::spawn(&resolve(&state.store_snapshot().agy_path)?)?;
    let result = tokio::time::timeout(Duration::from_secs(20), acp.open()).await;
    let _ = acp.child.kill().await;
    let _ = acp.child.wait().await;
    let (init, session) = result.map_err(|_| "agy 模型检测超时".to_string())??;
    let models: Vec<_> = session["models"]["availableModels"].as_array().into_iter().flatten().filter_map(|v| {
        let id = v["modelId"].as_str()?;
        Some(crate::models::LocalModel { id:id.into(), label:v["name"].as_str().map(str::to_owned), source:"agy ACP session/new · 本机实时目录；非推理可达性验证".into(), vision:verified_vision(&init, Some(id)).then_some(true) })
    }).collect();
    state.save_model_catalog("agy".into(), models.clone()).await?;
    Ok(models)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> CodexRequest { CodexRequest { prompt:"Return JSON with answer equal to ok. Do not use tools.".into(), schema:r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#.into(), images:vec![], model:Some("gemini-3.8-flash-high".into()), timeout_ms:None } }
    #[test]
    fn rejects_shims_and_extracts_json() {
        for p in ["agy.cmd","agy.PS1","agy.bat"] { assert!(command(Path::new(p)).is_err()); }
        assert_eq!(extract_json("prose ```json\n{\"answer\":\"ok\"}\n```"),Some(r#"{"answer":"ok"}"#.into()));
        assert!(extract_json("{broken}").is_none());
    }
    #[tokio::test]
    async fn rejects_concurrent_run() {
        let _guard = RUNNING.lock().await;
        assert!(run_with(Path::new("missing"),request()).await.unwrap_err().contains("已有任务"));
    }
    #[tokio::test]
    #[ignore = "Real agy ACP inference spends quota; opt in explicitly"]
    async fn real_agy() {
        let bin = resolve("").unwrap();
        println!("ACP status: {}",probe_at(&bin).await.unwrap());
        let reply = run_with(&bin, request()).await.unwrap();
        let parsed: Value = serde_json::from_str(&reply.json).unwrap();
        println!("text parsed answer={parsed} ms={}",reply.ms);
        assert_eq!(parsed["answer"],"ok");
        use base64::Engine;
        let mut req = request();
        req.prompt = "Do not use tools or inspect files. Look directly at the attached image. What is its dominant color? Return JSON with answer equal to the color.".into();
        req.images = vec![base64::engine::general_purpose::STANDARD.encode(include_bytes!("testdata/agy-green.png"))];
        let reply = run_with(&bin, req).await.unwrap();
        let parsed: Value = serde_json::from_str(&reply.json).unwrap();
        println!("image parsed answer={parsed} ms={}",reply.ms);
        assert_eq!(parsed["answer"].as_str().unwrap().to_ascii_lowercase(), "green");
    }
}
