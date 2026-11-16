//! Runs the local `codex` CLI as the translator for one page.
//!
//! The desktop app cannot assume `codex` is on PATH: the Codex desktop install puts its CLI
//! under `%USERPROFILE%\Codex\_internal\app\resources\`, and a GUI process inherits neither
//! a shell profile nor a user-modified PATH. So the executable is resolved in four steps —
//! the path saved in Settings, `CODEX_BIN`, a list of known install locations, then bare
//! `codex` — and the settings page can "lock onto" whichever one answers `--version`.

use base64::Engine as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

use crate::commands::AppState;

static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Deserialize)]
pub struct CodexRequest {
    pub prompt: String,
    pub schema: String,
    pub images: Vec<String>,
    pub model: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, serde::Serialize)]
pub struct CodexReply {
    pub json: String,
    pub ms: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct CodexCandidate {
    pub path: String,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CodexStatus {
    /// What Settings holds; empty means "detect automatically".
    pub configured: String,
    /// The executable that would actually be spawned, when one was found.
    pub resolved: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    pub candidates: Vec<CodexCandidate>,
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// Paths worth trying when nothing has been configured.
///
/// "Just spawn `codex`" fails on the exact setup most users have — the Codex app installed
/// and running — because the app never touches PATH. Ordered cheapest first, deduplicated,
/// and only paths that actually exist are returned.
pub fn candidates() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut push = |path: PathBuf| {
        if path.is_file() && !found.iter().any(|p| same_path(p, &path)) {
            found.push(path);
        }
    };
    if let Some(env) = std::env::var_os("CODEX_BIN") {
        push(PathBuf::from(env));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        push(home.join("Codex/_internal/app/resources/codex.exe"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        push(local.join("Codex/_internal/app/resources/codex.exe"));
        push(local.join("Programs/Codex/codex.exe"));
        push(local.join("Programs/codex/codex.exe"));
        push(local.join("Codex/codex.exe"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        push(pf.join("Codex/codex.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            push(dir.join("codex.exe"));
        }
    }
    found
}

/// Settings first, then `CODEX_BIN`, then auto-detection, then bare `codex` so the error
/// message names something the user can act on instead of a silent failure.
fn resolve(configured: &str) -> PathBuf {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return path;
        }
    }
    candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("codex"))
}

fn command(bin: &Path) -> std::io::Result<tokio::process::Command> {
    let ext = bin
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(ext.as_str(), "cmd" | "bat" | "ps1") {
        // Running a .cmd needs a shell, and the prompt is user content — it must never be
        // handed to cmd.exe. The npm shim also cannot be followed to the real binary here.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{} 是脚本包装，不是可执行文件；请选择 codex.exe（通常在 %USERPROFILE%\\Codex\\_internal\\app\\resources\\）",
                bin.display()
            ),
        ));
    }
    let mut cmd = std::process::Command::new(bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    Ok(cmd)
}

async fn probe_at(bin: &Path) -> Result<String, String> {
    let mut spawned = command(bin).map_err(|e| e.to_string())?;
    let output = tokio::time::timeout(Duration::from_secs(10), spawned.arg("--version").output())
        .await
        .map_err(|_| "Codex 检测超时".to_string())?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "找不到 {}；请在「设置 → 本地 Codex」里指定 codex.exe 的完整路径",
                bin.display()
            ),
            _ => format!("无法启动 Codex（{}）：{e}", bin.display()),
        })?;
    if !output.status.success() {
        return Err(format!(
            "Codex 检测失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// What Settings and the dock both want: what would we run, and does it answer?
pub async fn status(configured: &str) -> CodexStatus {
    let mut probed: Vec<CodexCandidate> = Vec::new();
    for path in candidates() {
        let shown = path.to_string_lossy().to_string();
        match probe_at(&path).await {
            Ok(version) => probed.push(CodexCandidate { path: shown, version: Some(version), error: None }),
            Err(error) => probed.push(CodexCandidate { path: shown, version: None, error: Some(error) }),
        }
    }
    let target = resolve(configured);
    let configured = configured.trim().to_string();
    match probe_at(&target).await {
        Ok(version) => CodexStatus {
            configured,
            resolved: Some(target.to_string_lossy().to_string()),
            version: Some(version),
            error: None,
            candidates: probed,
        },
        Err(error) => CodexStatus {
            configured,
            resolved: target.is_file().then(|| target.to_string_lossy().to_string()),
            version: probed.iter().find_map(|c| c.version.clone()),
            error: Some(error),
            candidates: probed,
        },
    }
}

#[tauri::command]
pub async fn codex_status(state: tauri::State<'_, AppState>) -> Result<CodexStatus, String> {
    Ok(status(&state.store_snapshot().codex_path).await)
}

/// "一键锁定": probe the usual locations and remember the first one that answers.
#[tauri::command]
pub async fn codex_detect(
    state: tauri::State<'_, AppState>,
    save: bool,
) -> Result<CodexStatus, String> {
    let report = status("").await;
    if save {
        if let Some(winner) = report.candidates.iter().find(|c| c.version.is_some()) {
            let path = winner.path.clone();
            state.set_codex_path(path.clone()).await?;
            return Ok(status(&path).await);
        }
    }
    Ok(report)
}

#[tauri::command]
pub async fn codex_probe(state: tauri::State<'_, AppState>) -> Result<String, String> {
    probe_at(&resolve(&state.store_snapshot().codex_path)).await
}

#[tauri::command]
pub async fn codex_run(
    state: tauri::State<'_, AppState>,
    req: CodexRequest,
) -> Result<CodexReply, String> {
    let store = state.store_snapshot();
    let mut req = req;
    req.model = crate::models::selected_model(req.model, &store.codex_model);
    crate::models::validate_images(&store, "codex", &req)?;
    run_with(&resolve(&store.codex_path), req).await
}

async fn run_with(bin: &Path, req: CodexRequest) -> Result<CodexReply, String> {
    let _lock = RUNNING.try_lock().map_err(|_| "已有任务在跑".to_string())?;
    let started = Instant::now();
    let dir = loop {
        let path = std::env::temp_dir().join(format!("moeflow-codex-{}-{}",
            std::process::id(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        match std::fs::create_dir(&path) {
            Ok(()) => break Scratch(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    };
    let result: Result<CodexReply, std::io::Error> = async {
        let schema = dir.0.join("schema.json");
        tokio::fs::write(&schema, &req.schema).await?;
        let last = dir.0.join("last-message.json");
        // File-backed output avoids pipe deadlocks while the prompt is being written.
        let stdout = dir.0.join("stdout.txt");
        let stderr = dir.0.join("stderr.txt");
        let mut cmd = command(bin)?;
        cmd.args(["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-C"])
            .arg(&dir.0).arg("--output-schema").arg(&schema);
        if let Some(model) = req.model { cmd.arg("-m").arg(model); }
        for (i, image) in req.images.iter().enumerate() {
            let bytes = base64::engine::general_purpose::STANDARD.decode(image)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput,
                    format!("图片 base64 解码失败（{e}）：前端应发送不带前缀的标准 base64")))?;
            let path = dir.0.join(format!("image-{i}.png"));
            tokio::fs::write(&path, bytes).await?;
            cmd.arg("-i").arg(path);
        }
        cmd.arg("-o").arg(&last).arg("-").stdin(Stdio::piped())
            .stdout(std::fs::File::create(&stdout)?)
            .stderr(std::fs::File::create(&stderr)?);
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("找不到 {}；请在「设置 → 本地 Codex」里指定 codex.exe 的完整路径", bin.display()),
            ),
            _ => e,
        })?;
        let execution = tokio::time::timeout(
            Duration::from_millis(req.timeout_ms.unwrap_or(180_000).clamp(1, 180_000)),
            async {
                let mut stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("缺少 stdin"))?;
                stdin.write_all(req.prompt.as_bytes()).await?;
                drop(stdin);
                child.wait().await
            },
        ).await;
        let status = match execution {
            Ok(Ok(status)) => status,
            other => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let reason = match other {
                    Ok(Err(e)) => e.to_string(),
                    _ => "Codex 超时，已终止任务".to_string(),
                };
                let bytes = tokio::fs::read(&stderr).await.unwrap_or_default();
                let tail = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4000)..]);
                return Err(std::io::Error::other(format!("{reason}：{tail}")));
            }
        };
        if !status.success() {
            let bytes = tokio::fs::read(&stderr).await?;
            let tail = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4000)..]);
            return Err(std::io::Error::other(format!("Codex 退出 {status}：{tail}")));
        }
        let mut json = tokio::fs::read_to_string(&last).await.unwrap_or_default();
        if json.trim().is_empty() { json = tokio::fs::read_to_string(&stdout).await?; }
        if json.trim().is_empty() { return Err(std::io::Error::other("Codex 返回为空")); }
        Ok(CodexReply { json, ms: started.elapsed().as_millis() as u64 })
    }.await;
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 PNG: enough for Codex to accept the attachment without any real content.
    const PNG_2X2: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAE0lEQVR4nGP8//8/AwMDEwMYAAAkBgMBXaJOiAAAAABJRU5ErkJggg==";

    /// End-to-end check of the whole command path against the real CLI.
    ///
    /// Ignored by default: it needs a logged-in `codex` on this machine, costs ~40 s of
    /// quota and reaches the network. Everything else about this module is covered by
    /// reading the wrapper; only an actual spawn proves the argument vector, the `-o` file
    /// and the stdin handoff line up with the CLI we ship against.
    #[ignore = "needs a logged-in codex CLI and spends quota"]
    #[tokio::test]
    async fn runs_codex_and_returns_schema_shaped_json() {
        let bin = candidates().into_iter().next().expect("codex should be installed");
        let probe = probe_at(&bin).await.expect("codex --version should answer");
        assert!(probe.to_lowercase().contains("codex"), "unexpected version banner: {probe}");

        let schema = r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}"#;
        let reply = run_with(&bin, CodexRequest {
            prompt: "Return only JSON matching the schema. The answer is the two characters: ok".to_string(),
            schema: schema.to_string(),
            images: vec![PNG_2X2.to_string()],
            model: None,
            timeout_ms: Some(120_000),
        })
        .await
        .expect("run_with should succeed");

        let parsed: serde_json::Value = serde_json::from_str(reply.json.trim())
            .unwrap_or_else(|e| panic!("reply is not JSON ({e}): {}", reply.json));
        assert!(parsed.get("answer").and_then(|v| v.as_str()).is_some(), "missing answer: {parsed}");
        assert!(reply.ms > 0, "elapsed millis should be recorded");
    }

    /// Second concurrent call must be rejected instead of queueing behind a 40 s run.
    #[tokio::test]
    async fn rejects_a_second_concurrent_run() {
        let guard = RUNNING.try_lock().expect("first lock should be free");
        let err = run_with(Path::new("codex"), CodexRequest {
            prompt: "x".to_string(),
            schema: "{}".to_string(),
            images: vec![],
            model: None,
            timeout_ms: Some(1_000),
        })
        .await
        .expect_err("second run must be rejected");
        assert!(err.contains("已有任务在跑"), "unexpected error: {err}");
        drop(guard);
    }

    /// Detection must find the Codex desktop install without any configuration, and must
    /// refuse the npm `.cmd` shim instead of handing a prompt to cmd.exe.
    #[test]
    fn detection_prefers_real_executables() {
        let found = candidates();
        if found.is_empty() {
            return; // A machine without Codex: nothing to assert about ordering.
        }
        assert!(found.iter().all(|p| p.is_file()), "candidates must exist");
        assert!(
            !found.iter().any(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("cmd"))),
            "the npm shim must never be proposed as an executable"
        );
        assert_eq!(command(Path::new("C:/x/codex.cmd")).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(resolve(""), found[0], "auto-detection should pick the first existing path");
    }

    /// Proves the resolution order actually lands on the Codex desktop install on a real
    /// machine, which is the failure the first build shipped with (`program not found`).
    #[ignore = "spawns codex --version for every candidate"]
    #[tokio::test]
    async fn detects_and_probes_the_local_install() {
        let report = status("").await;
        println!("resolved={:?} version={:?} candidates={}", report.resolved, report.version, report.candidates.len());
        assert!(report.version.is_some(), "no working codex found: {:?}", report.error);
        assert!(report.resolved.as_deref().unwrap_or("").to_lowercase().ends_with("codex.exe"));
        assert!(report.candidates.iter().any(|c| c.version.is_some()));
    }
}
