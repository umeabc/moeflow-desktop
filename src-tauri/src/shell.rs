//! Loopback server for the desktop shell pages: the instance picker and the settings window.
//!
//! These two pages are served over HTTP from `127.0.0.1` rather than through Tauri's custom
//! `tauri://` asset protocol. That protocol resolves `WebviewUrl::App` against an asset map
//! baked in at compile time, and cargo does **not** re-run `generate_context!` when only
//! files under `frontendDist` change. An edited page therefore keeps serving stale bytes,
//! and a miss surfaces as a plain white window with nothing in the log — which is exactly
//! how this went wrong once already.
//!
//! Serving the pages ourselves removes that failure mode: `include_str!` is a normal
//! compile-time dependency, so editing the HTML rebuilds the binary, and dev and packaged
//! builds behave identically. The pages sit on their own port, so their origin is distinct
//! from every profile's — no cookie or storage can leak between them.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

/// The pages live in `ui/` so there is exactly one copy of each, shared with any future
/// `tauri build` that still wants them on disk.
const LAUNCHER_HTML: &str = include_str!("../ui/launcher.html");
const SETTINGS_HTML: &str = include_str!("../ui/settings.html");

fn html(body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // The shell pages are part of the binary; a cached copy could only ever be
            // stale, and they are a few kilobytes.
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

async fn launcher() -> Response {
    html(LAUNCHER_HTML)
}

async fn settings() -> Response {
    html(SETTINGS_HTML)
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn router() -> Router {
    Router::new()
        .route("/", get(launcher))
        .route("/launcher.html", get(launcher))
        .route("/settings.html", get(settings))
        .fallback(not_found)
}

/// Bind the shell server. Passing port 0 lets the OS choose; the real port is returned so
/// the windows can be pointed at it.
///
/// The server runs for the life of the process — there is no shutdown handle, because there
/// is no point in the app's lifetime at which the shell pages should stop being reachable.
pub async fn bind(port: u16) -> std::io::Result<u16> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let bound = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router()).await;
    });
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blank shell window is the failure this module exists to prevent, so make the
    /// pages' presence a build-time and test-time guarantee rather than a runtime hope.
    #[test]
    fn shell_pages_are_embedded_and_non_trivial() {
        assert!(LAUNCHER_HTML.len() > 1000, "launcher.html looks empty");
        assert!(SETTINGS_HTML.len() > 1000, "settings.html looks empty");
        assert!(LAUNCHER_HTML.contains("boot_payload"));
        assert!(SETTINGS_HTML.contains("cache_stats") || SETTINGS_HTML.contains("boot_payload"));
    }
}
