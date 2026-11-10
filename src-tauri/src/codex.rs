use base64::Engine as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Deserialize)]
pub struct CodexRequest {
    prompt: String,
    schema: String,
    images: Vec<String>,
    model: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(serde::Serialize)]
pub struct CodexReply {
    json: String,
    ms: u64,
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command() -> tokio::process::Command {
    let mut cmd = std::process::Command::new(
        std::env::var_os("CODEX_BIN").unwrap_or_else(|| "codex".into()),
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    cmd
}

#[tauri::command]
pub async fn codex_probe() -> Result<String, String> {
    let output = tokio::time::timeout(Duration::from_secs(10), command().arg("--version").output())
        .await.map_err(|_| "Codex 检测超时".to_string())?
        .map_err(|e| format!("无法启动 Codex：{e}"))?;
    if !output.status.success() {
        return Err(format!("Codex 检测失败：{}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub async fn codex_run(req: CodexRequest) -> Result<CodexReply, String> {
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
        let mut cmd = command();
        cmd.args(["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-C"])
            .arg(&dir.0).arg("--output-schema").arg(&schema);
        if let Some(model) = req.model { cmd.arg("-m").arg(model); }
        for (i, image) in req.images.iter().enumerate() {
            let bytes = base64::engine::general_purpose::STANDARD.decode(image)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let path = dir.0.join(format!("image-{i}.png"));
            tokio::fs::write(&path, bytes).await?;
            cmd.arg("-i").arg(path);
        }
        cmd.arg("-o").arg(&last).arg("-").stdin(Stdio::piped())
            .stdout(std::fs::File::create(&stdout)?)
            .stderr(std::fs::File::create(&stderr)?);
        let mut child = cmd.spawn()?;
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
