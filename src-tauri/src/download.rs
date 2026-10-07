//! Native "save as" downloads.
//!
//! The frontend triggers every download as `<a href={output.link} target="_blank">`, where
//! the link is an absolute storage URL. `rewrite.rs` routes those through `/__download/...`,
//! so they arrive here instead of opening a browser window: we ask the user where to put
//! the file and stream it straight to disk, never buffering a whole export in memory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter};
#[cfg(not(test))]
use tauri_plugin_dialog::DialogExt;

use crate::server::Ctx;

/// Ask the user where to save. Returns `None` when they cancel the dialog.
///
/// Called from an axum worker thread, never the main thread — `blocking_save_file`
/// dispatches to the main thread internally and would deadlock if called from it.
#[cfg(not(test))]
pub fn ask_save_path(app: &AppHandle, suggested: &str) -> Option<PathBuf> {
    app.dialog()
        .file()
        .set_file_name(suggested)
        .blocking_save_file()
        .and_then(|path| path.into_path().ok())
}

// The headless library harness has no native event loop or common-controls manifest.
// Keep native dialogs out of its link graph; server tests pass an absent app handle.
#[cfg(test)]
pub fn ask_save_path(_app: &AppHandle, _suggested: &str) -> Option<PathBuf> {
    None
}

/// Stream `url` into `destination`, emitting progress events for the UI.
pub async fn fetch_to_file(ctx: &Ctx, url: &str, destination: &Path) -> Result<u64, String> {
    let snapshot = ctx.snapshot();
    let profile = &snapshot.profile;
    let client = &snapshot.client;

    // Downloads come from the same object storage as images, so they face the same Referer
    // allowlist. See `Profile::media_referer`.
    let mut request = client.get(url);
    if let Some(referer) = profile.media_referer() {
        request = request.header(reqwest::header::REFERER, referer);
    }

    let response = request.send().await.map_err(|err| format!("{err}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let total = response.content_length();
    let mut written: u64 = 0;
    let mut last_emit = std::time::Instant::now();

    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(|err| format!("无法写入目标文件：{err}"))?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| format!("传输中断：{err}"))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|err| format!("写入失败：{err}"))?;
        written += chunk.len() as u64;

        // Throttle UI updates; a large export would otherwise flood the event loop.
        if last_emit.elapsed() > Duration::from_millis(200) {
            last_emit = std::time::Instant::now();
            if let Some(app) = &ctx.app {
                let _ = app.emit(
                    "download://progress",
                    serde_json::json!({
                        "url": url,
                        "written": written,
                        "total": total,
                    }),
                );
            }
        }
    }
    drop(file);

    Ok(written)
}

/// Tell the renderer a download finished so it can offer "open folder".
pub fn announce(app: &AppHandle, path: &Path, bytes: u64) {
    let _ = app.emit(
        "download://finished",
        serde_json::json!({
            "path": path.to_string_lossy(),
            "bytes": bytes,
        }),
    );
}
