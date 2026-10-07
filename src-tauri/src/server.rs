//! The loopback HTTP server that makes the web frontend work as a desktop app.
//!
//! Everything is served from `http://127.0.0.1:<port>/`, which means the bundled frontend
//! behaves exactly as it does behind nginx:
//!
//!   * its root-absolute asset paths (`/assets/...`, `/static/...`) resolve,
//!   * `BrowserRouter` gets the SPA fallback it needs for deep links and full page loads,
//!   * the `token` cookie and `Authorization` header are same-origin, so nothing about
//!     authentication has to change,
//!   * `/api/*` is reverse-proxied to the configured server with streaming uploads and
//!     downloads, so large multipart imports still work.
//!
//! It additionally serves `/__media/...` (cache-backed images) and `/__download/...`
//! (native save-as), which is how the desktop-only features are injected without
//! patching the upstream frontend.

use std::path::PathBuf;
use std::sync::Arc;

use crate::network::{NetworkClient, SharedClients};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use futures_util::{StreamExt, TryStreamExt};
use std::sync::RwLock;
use tokio_util::io::ReaderStream;

use crate::media::MediaCache;
use crate::profiles::Profile;
use crate::rewrite::{decode_origin, rewrite_api_json};

/// Hop-by-hop headers must not be forwarded (RFC 9110 §7.6.1).
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Headers we let reqwest negotiate itself, plus any we must recompute after rewriting.
const REQUEST_HEADERS_TO_DROP: &[&str] = &["host", "content-length", "accept-encoding"];

struct LiveProfile {
    profile: Profile,
    known_origins: Vec<String>,
    generation: u64,
}

pub struct RequestSnapshot {
    pub profile: Profile,
    pub client: NetworkClient,
    pub(crate) known_origins: Vec<String>,
    generation: u64,
}

pub struct Ctx {
    live: RwLock<LiveProfile>,
    pub web_root: PathBuf,
    pub cache: Arc<MediaCache>,
    pub app: Option<tauri::AppHandle>,
    clients: SharedClients,
    publication: Arc<RwLock<()>>,
}

impl Ctx {
    pub fn new(
        profile: Profile,
        web_root: PathBuf,
        cache: Arc<MediaCache>,
        app: Option<tauri::AppHandle>,
        clients: SharedClients,
        publication: Arc<RwLock<()>>,
    ) -> Self {
        Self {
            live: RwLock::new(LiveProfile {
                known_origins: profile.media_origins.clone(),
                profile,
                generation: 0,
            }),
            web_root,
            cache,
            app,
            clients,
            publication,
        }
    }

    /// All locks are short and synchronous; none survives a network await. Publishing a
    /// proxy change swaps one shared Arc, so requests already started keep their pools.
    pub fn snapshot(&self) -> RequestSnapshot {
        let _publication = self.publication.read().unwrap_or_else(|p| p.into_inner());
        let live = self.live.read().unwrap_or_else(|p| p.into_inner());
        let bundle = self.clients.read().unwrap_or_else(|p| p.into_inner());
        RequestSnapshot {
            profile: live.profile.clone(),
            client: bundle.client(live.profile.allow_invalid_certs),
            known_origins: live.known_origins.clone(),
            generation: live.generation,
        }
    }

    pub fn update_profile(&self, profile: Profile) {
        let mut live = self.live.write().unwrap_or_else(|p| p.into_inner());
        if live.profile.api_base != profile.api_base
            || live.profile.site_url != profile.site_url
            || live.profile.allow_invalid_certs != profile.allow_invalid_certs
            || live.profile.media_origins != profile.media_origins
        {
            live.generation += 1;
            live.known_origins = profile.media_origins.clone();
        }
        live.profile = profile;
    }

    pub fn learn_origins(&self, snapshot: &RequestSnapshot, origins: &[String]) {
        let mut live = self.live.write().unwrap_or_else(|p| p.into_inner());
        if live.generation != snapshot.generation {
            return;
        }
        for origin in origins {
            if !live.known_origins.contains(origin) {
                live.known_origins.push(origin.clone());
            }
        }
    }
}

/// The LabelPlus Photoshop script the frontend links to.
///
/// Upstream hard-codes this third-party URL. Routing it through the cache means it is
/// fetched once and still available when the host is slow or unreachable — without us
/// redistributing it inside the installer (it is GPLv2, which is why upstream only links
/// to it in the first place).
const PS_SCRIPT_URL: &str = "https://files.kozzzx.com/labelplus/LabelPlus_PS-Script_latest.zip";
const PS_SCRIPT_CACHE_KEY: &str = "app://labelplus-ps-script";
const PS_SCRIPT_FILENAME: &str = "LabelPlus_PS-Script_latest.zip";

pub fn router(ctx: Arc<Ctx>) -> Router {
    Router::new()
        .route("/moeflow-runtime-config.json", any(runtime_config))
        .route("/api/{*rest}", any(api_proxy))
        .route("/__media/{token}/{*rest}", any(media))
        .route("/__download/{token}/{*rest}", any(download))
        .route("/__ps-script", any(ps_script))
        .fallback(any(static_file))
        .with_state(ctx)
}

/// Serve the Photoshop script through the native save dialog, caching the download.
async fn ps_script(State(ctx): State<Arc<Ctx>>) -> Response {
    let cached = match fetch_through_cache(&ctx, PS_SCRIPT_URL, PS_SCRIPT_CACHE_KEY).await {
        Ok(path) => path,
        Err(err) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("无法获取 PS 脚本（首次使用需要联网）：{err}"),
            )
        }
    };

    let Some(app) = ctx.app.as_ref() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "保存对话框不可用");
    };
    let Some(destination) = crate::download::ask_save_path(app, PS_SCRIPT_FILENAME) else {
        return (StatusCode::NO_CONTENT, "").into_response();
    };

    match tokio::fs::copy(&cached, &destination).await {
        Ok(bytes) => {
            crate::download::announce(app, &destination, bytes);
            (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                "<!doctype html><meta charset=\"utf-8\"><title>已保存</title>\
                 <body style=\"font-family:system-ui;padding:2rem\"><h3>已保存</h3>\
                 <script>setTimeout(()=>window.close(),1200)</script>"
                    .to_string(),
            )
                .into_response()
        }
        Err(err) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("写入失败：{err}"),
        ),
    }
}

/// Return a cached file, downloading it first if needed.
async fn fetch_through_cache(
    ctx: &Ctx,
    url: &str,
    cache_key: &str,
) -> Result<std::path::PathBuf, String> {
    if let Some(path) = ctx.cache.get(cache_key) {
        return Ok(path);
    }

    let snapshot = ctx.snapshot();
    let client = &snapshot.client;
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let temp = ctx.cache.temp_path();
    let mut file = tokio::fs::File::create(&temp)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
    }
    drop(file);

    ctx.cache
        .commit(cache_key, &temp, None)
        .map_err(|e| e.to_string())
}

/// Bind a loopback listener and serve until the returned sender fires.
///
/// Passing port 0 lets the OS choose; the actual port is returned so the caller can point
/// the window at it.
pub async fn bind(
    ctx: Arc<Ctx>,
    port: u16,
) -> std::io::Result<(u16, tokio::sync::oneshot::Sender<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let bound = listener.local_addr()?.port();
    let tx = serve_listener(ctx, listener);
    Ok((bound, tx))
}

pub fn serve_listener(
    ctx: Arc<Ctx>,
    listener: tokio::net::TcpListener,
) -> tokio::sync::oneshot::Sender<()> {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let app = router(ctx);

    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });

    tx
}

/// The frontend fetches this at startup and lets it override the build-time API base.
/// Serving it here is what points the unmodified bundle at our proxy.
async fn runtime_config() -> Response {
    let body = serde_json::json!({ "baseURL": "/api/" });
    (
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&body).unwrap_or_else(|_| "{}".into()),
    )
        .into_response()
}

// ---------------------------------------------------------------- API proxy

async fn api_proxy(
    State(ctx): State<Arc<Ctx>>,
    method: Method,
    Path(rest): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let snapshot = ctx.snapshot();
    let profile = &snapshot.profile;

    let mut target = format!("{}/{}", profile.api_base.trim_end_matches('/'), rest);
    if let Some(query) = uri.query() {
        target.push('?');
        target.push_str(query);
    }

    let client = &snapshot.client;
    let mut request = client.request(method.clone(), &target);

    for (name, value) in headers.iter() {
        let lower = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str()) || REQUEST_HEADERS_TO_DROP.contains(&lower.as_str())
        {
            continue;
        }
        request = request.header(name, value);
    }

    // Stream the request body: project imports can be hundreds of megabytes.
    let stream = body.into_data_stream();
    request = request.body(reqwest::Body::wrap_stream(stream));

    let upstream = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            return error_response(StatusCode::BAD_GATEWAY, &format!("无法连接服务器：{err}"))
        }
    };

    let status = upstream.status();
    let up_headers = upstream.headers().clone();
    let content_type = up_headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    // An API call that comes back as HTML did not reach the API — the backend only ever
    // speaks JSON. In practice this means `api_base` is wrong, and the usual cause is a
    // *site* address saved as the API base: `/api/v1/x` then goes to `<site>/v1/x`, which
    // nginx serves from the SPA fallback (200 for GET, 405 for POST). Say so plainly here
    // rather than letting every endpoint fail one at a time.
    if content_type.contains("text/html") {
        eprintln!(
            "[moeflow] API call returned HTML instead of JSON — check the API base for this \
             profile (is it missing the /api segment?): {target}"
        );
    }

    // JSON responses may carry storage URLs that need rewriting. Everything else streams.
    if content_type.contains("application/json") {
        let bytes = match upstream.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => {
                return error_response(StatusCode::BAD_GATEWAY, &format!("读取响应失败：{err}"))
            }
        };

        let known = &snapshot.known_origins;
        let site_origin = profile.site_origin();
        let payload = match rewrite_api_json(&bytes, &known, &site_origin) {
            Some((rewritten, outcome)) => {
                if media_debug_enabled() {
                    for url in &outcome.rewritten {
                        eprintln!("[moeflow] media rewritten: {url}");
                    }
                }
                ctx.learn_origins(&snapshot, &outcome.learned_origins);
                rewritten
            }
            None => bytes.to_vec(),
        };

        let mut response = Response::new(Body::from(payload));
        *response.status_mut() = status;
        copy_response_headers(&mut response, &up_headers);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        return response;
    }

    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    copy_response_headers(&mut response, &up_headers);
    response
}

// ---------------------------------------------------------------- media cache

async fn media(
    State(ctx): State<Arc<Ctx>>,
    Path((token, rest)): Path<(String, String)>,
    uri: Uri,
) -> Response {
    let upstream_url = match join_origin(&token, &rest, uri.query()) {
        Some(url) => url,
        None => return error_response(StatusCode::BAD_REQUEST, "无效的媒体地址"),
    };

    // Serve from cache when the payload is still on disk. `MediaCache::get` prunes entries
    // whose file has vanished, but eviction can still race us between the lookup and the
    // open — fall through to a fresh fetch instead of failing.
    if let Some(path) = ctx.cache.get(&upstream_url) {
        if let Ok(file) = tokio::fs::File::open(&path).await {
            return cached_response(file, &upstream_url, ctx.cache.etag_for(&upstream_url));
        }
    }

    let snapshot = ctx.snapshot();
    let profile = &snapshot.profile;
    let client = &snapshot.client;

    // An origin-relative URL was pinned to the site origin, but on a split deployment the
    // storage may actually live elsewhere. Try the configured origins before giving up.
    let origin = decode_origin(&token).unwrap_or_default();
    let mut candidates = vec![upstream_url.clone()];
    if origin == profile.site_origin() {
        for alternate in &profile.media_origins {
            candidates.push(format!(
                "{}{}",
                alternate.trim_end_matches('/'),
                format!("/{rest}")
            ));
        }
    }

    let referer = profile.media_referer();

    let mut last_error = String::from("上游返回错误");
    let mut upstream = None;
    for candidate in &candidates {
        let mut request = client.get(candidate);
        if let Some(referer) = &referer {
            request = request.header(header::REFERER, referer);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                // A 2xx is not proof of an image. Every one of these deployments sits behind
                // nginx with `try_files $uri /index.html`, so a storage path that does not
                // exist answers **200 with the SPA shell**. Accepting that would both hide
                // the failure and cache an HTML document under an image key, served with
                // `Content-Type: image/png` derived from the URL — a permanently broken
                // image, and one that also masks a correct alternate origin further down
                // the candidate list.
                if !looks_like_an_image(&response) {
                    let kind = response
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown");
                    last_error = format!("{candidate} 返回的不是图片（Content-Type: {kind}）");
                    eprintln!("[moeflow] media candidate rejected: {last_error}");
                    continue;
                }
                upstream = Some(response);
                break;
            }
            Ok(response) => {
                last_error = format!("HTTP {}", response.status());
                eprintln!("[moeflow] media candidate failed: {candidate} -> {last_error}");
            }
            Err(err) => {
                last_error = err.to_string();
                eprintln!("[moeflow] media candidate failed: {candidate} -> {last_error}");
            }
        }
    }

    let Some(upstream) = upstream else {
        eprintln!("[moeflow] media fetch gave up for {upstream_url}: {last_error}");
        return error_response(StatusCode::BAD_GATEWAY, &format!("取图失败：{last_error}"));
    };

    let etag = upstream
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    // Stream to a scratch file so a 30 MB page image never lands in memory.
    let temp = ctx.cache.temp_path();
    let mut file = match tokio::fs::File::create(&temp).await {
        Ok(file) => file,
        Err(err) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("建临时文件失败：{err}"),
            )
        }
    };

    let mut stream = upstream.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => {
                use tokio::io::AsyncWriteExt;
                if let Err(err) = file.write_all(&chunk).await {
                    ctx.cache.discard_temp(&temp);
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("写缓存失败：{err}"),
                    );
                }
            }
            Err(err) => {
                ctx.cache.discard_temp(&temp);
                return error_response(StatusCode::BAD_GATEWAY, &format!("传输中断：{err}"));
            }
        }
    }
    drop(file);

    match ctx.cache.commit(&upstream_url, &temp, etag.clone()) {
        Ok(path) => serve_cached(&path, &upstream_url, etag).await,
        Err(err) => {
            ctx.cache.discard_temp(&temp);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("缓存提交失败：{err}"),
            )
        }
    }
}

/// Set `MOEFLOW_MEDIA_DEBUG=1` to log every storage URL the proxy rewrites (and every media
/// candidate it rejects). Diagnosing "images do not load on server X" is otherwise guesswork:
/// the answer is always which URL form came back from the API.
fn media_debug_enabled() -> bool {
    std::env::var_os("MOEFLOW_MEDIA_DEBUG").is_some()
}

/// Whether an upstream response is plausibly the image we asked for.
///
/// Deliberately a *rejection* test rather than a whitelist: a storage server may legitimately
/// answer `application/octet-stream` for a `.png`, and refusing those would break working
/// deployments. What must never get through is the HTML that a SPA fallback returns for a
/// path that does not exist.
fn looks_like_an_image(response: &reqwest::Response) -> bool {
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    content_type_is_usable_as_media(content_type)
}

fn content_type_is_usable_as_media(content_type: &str) -> bool {
    let content_type = content_type.trim().to_ascii_lowercase();
    if content_type.starts_with("image/") {
        return true;
    }
    // A missing Content-Type is treated as usable: many object stores omit it, and the
    // caller still gets the bytes it asked for.
    !content_type.starts_with("text/html")
}

async fn serve_cached(path: &std::path::Path, url: &str, etag: Option<String>) -> Response {
    match tokio::fs::File::open(path).await {
        Ok(file) => cached_response(file, url, etag),
        Err(err) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("缓存文件无法读取：{err}"),
        ),
    }
}

fn cached_response(file: tokio::fs::File, url: &str, etag: Option<String>) -> Response {
    // Guess from the path alone — a query string would otherwise be read as part of the
    // extension and every `?v=2` image would go out as application/octet-stream.
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    if let Some(etag) = etag {
        if let Ok(value) = HeaderValue::from_str(&etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
    }
    response
}

// ---------------------------------------------------------------- downloads

/// Native "save as": the frontend's `<a href>` navigations for exports land here, we ask
/// the user where to put the file, and stream it there.
async fn download(
    State(ctx): State<Arc<Ctx>>,
    Path((token, rest)): Path<(String, String)>,
    uri: Uri,
) -> Response {
    let upstream_url = match join_origin(&token, &rest, uri.query()) {
        Some(url) => url,
        None => return error_response(StatusCode::BAD_REQUEST, "无效的下载地址"),
    };

    let suggested = filename_from_url(&upstream_url);

    let Some(app) = ctx.app.as_ref() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "保存对话框不可用");
    };
    let Some(destination) = crate::download::ask_save_path(app, &suggested) else {
        // User cancelled — report plainly rather than surfacing a transport error.
        return (StatusCode::NO_CONTENT, "").into_response();
    };

    match crate::download::fetch_to_file(&ctx, &upstream_url, &destination).await {
        Ok(bytes) => {
            crate::download::announce(app, &destination, bytes);
            let name = destination
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                format!(
                    "<!doctype html><meta charset=\"utf-8\"><title>已保存</title>\
                     <body style=\"font-family:system-ui;padding:2rem\">\
                     <h3>已保存</h3><p>{}</p>\
                     <script>setTimeout(()=>window.close(),1200)</script>",
                    html_escape(&name)
                ),
            )
                .into_response()
        }
        Err(err) => error_response(StatusCode::BAD_GATEWAY, &format!("下载失败：{err}")),
    }
}

fn filename_from_url(url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    // Drop `scheme://host` first: otherwise a URL with an empty path would offer the host
    // name (`cdn.test`) as the filename.
    let path = match without_query.split_once("://") {
        Some((_, after)) => match after.find('/') {
            Some(index) => &after[index..],
            None => "",
        },
        None => without_query,
    };
    let last = path.rsplit('/').find(|s| !s.is_empty());
    match last {
        Some(name) => percent_encoding::percent_decode_str(name)
            .decode_utf8_lossy()
            .to_string(),
        None => "download".to_string(),
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------- static files

/// Serve the bundled frontend, falling back to `index.html` for unknown paths so that
/// `BrowserRouter` deep links and the frontend's full-page `<a href>` navigations work.
async fn static_file(State(ctx): State<Arc<Ctx>>, uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    if raw.is_empty() {
        return serve_index(&ctx).await;
    }

    let Some(relative) = safe_relative_path(raw) else {
        return error_response(StatusCode::BAD_REQUEST, "非法路径");
    };

    let candidate = ctx.web_root.join(&relative);

    // Second line of defence: the resolved file must actually live under the web root.
    // This also catches anything a future change to `safe_relative_path` might let past.
    if let (Ok(root), Ok(resolved)) = (ctx.web_root.canonicalize(), candidate.canonicalize()) {
        if !resolved.starts_with(&root) {
            return error_response(StatusCode::BAD_REQUEST, "非法路径");
        }
    }

    if candidate.is_file() {
        return serve_path(&candidate, false).await;
    }

    // A missing *asset* must not be answered with the SPA shell. Returning HTML with a 200
    // for `/storage/x.png` renders as a silently broken image; a 404 makes the failure
    // legible. Client-side routes never carry a file extension, so they still fall through.
    if looks_like_asset(&relative) {
        return error_response(StatusCode::NOT_FOUND, "文件不存在");
    }

    serve_index(&ctx).await
}

/// Extensions that mean "this was a request for a file, not a client-side route".
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg", "ico", "css", "js", "mjs", "map",
    "ttf", "otf", "woff", "woff2", "mp4", "webm", "mp3", "txt", "zip", "pdf",
];

fn looks_like_asset(path: &str) -> bool {
    match path.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && !ext.is_empty()
                && !ext.contains('/')
                && ASSET_EXTENSIONS
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(ext))
        }
        None => false,
    }
}

/// Decode a request path and reject anything that could escape the web root.
///
/// Decoding matters for correctness, not just safety: asset names with spaces or non-ASCII
/// characters arrive percent-encoded, and looking for the encoded name on disk would 404.
/// The traversal check then has to run on the *decoded* string, since `%2e%2e` would
/// otherwise slip past a raw comparison.
fn safe_relative_path(raw: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .ok()?
        .to_string();

    // Backslash is a separator on Windows even though it is not on the URL's own terms.
    if decoded.contains('\\') {
        return None;
    }
    if decoded
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return None;
    }
    // Absolute paths and drive-relative paths must not survive either.
    if decoded.starts_with('/') || decoded.contains(':') {
        return None;
    }

    Some(decoded)
}

async fn serve_index(ctx: &Ctx) -> Response {
    serve_path(&ctx.web_root.join("index.html"), true).await
}

async fn serve_path(path: &std::path::Path, no_cache: bool) -> Response {
    let Ok(file) = tokio::fs::File::open(path).await else {
        return error_response(StatusCode::NOT_FOUND, "文件不存在");
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    // Hashed assets are immutable; the index must never be cached or a rebuild would be
    // invisible. Mirrors the upstream nginx configuration.
    let cache_control = if no_cache {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

// ---------------------------------------------------------------- helpers

fn join_origin(token: &str, rest: &str, query: Option<&str>) -> Option<String> {
    let origin = decode_origin(token)?;
    let mut url = format!("{origin}/{rest}");
    if let Some(query) = query {
        url.push('?');
        url.push_str(query);
    }
    Some(url)
}

fn copy_response_headers(response: &mut Response, upstream: &HeaderMap) {
    for (name, value) in upstream.iter() {
        let lower = name.as_str().to_ascii_lowercase();
        // Content-Length/Encoding are recomputed: bodies may be rewritten or decompressed.
        if HOP_BY_HOP.contains(&lower.as_str())
            || lower == "content-length"
            || lower == "content-encoding"
        {
            continue;
        }
        response.headers_mut().insert(name.clone(), value.clone());
    }
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "message": message });
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        serde_json::to_string(&body).unwrap_or_else(|_| "{}".into()),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_is_taken_from_the_url_path() {
        assert_eq!(
            filename_from_url("https://cdn.test/out/My%20Project.zip?token=abc"),
            "My Project.zip"
        );
    }

    #[test]
    fn filename_falls_back_when_the_url_has_no_path() {
        assert_eq!(filename_from_url("https://cdn.test/"), "download");
    }

    #[test]
    fn join_origin_rebuilds_the_upstream_url() {
        let token = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            "https://cdn.test",
        );
        assert_eq!(
            join_origin(&token, "a/b.png", Some("v=2")).unwrap(),
            "https://cdn.test/a/b.png?v=2"
        );
    }

    #[test]
    fn join_origin_rejects_a_corrupt_token() {
        assert!(join_origin("!!!not-base64!!!", "a.png", None).is_none());
    }

    #[test]
    fn html_escape_covers_the_dangerous_characters() {
        assert_eq!(html_escape("a<b>&c"), "a&lt;b&gt;&amp;c");
    }

    #[test]
    fn asset_paths_are_recognised() {
        assert!(looks_like_asset("storage/2026/01/abc.png"));
        assert!(looks_like_asset("assets/index-abc.js"));
        assert!(
            looks_like_asset("a.PNG"),
            "extension match is case-insensitive"
        );
    }

    /// Client-side routes must keep falling through to the SPA shell.
    /// Regression: a SPA fallback answers 200 + text/html for any unknown path, so an
    /// unreachable storage path used to be accepted as a successful image fetch, cached,
    /// and served as `image/png` — a permanently broken image, with the correct alternate
    /// origin never tried.
    #[test]
    fn html_from_a_spa_fallback_is_not_accepted_as_an_image() {
        assert!(!content_type_is_usable_as_media("text/html"));
        assert!(!content_type_is_usable_as_media("text/html; charset=utf-8"));
        assert!(!content_type_is_usable_as_media("TEXT/HTML; charset=UTF-8"));
    }

    #[test]
    fn real_image_responses_are_accepted() {
        assert!(content_type_is_usable_as_media("image/png"));
        assert!(content_type_is_usable_as_media(
            "image/jpeg; charset=binary"
        ));
        assert!(content_type_is_usable_as_media("image/webp"));
        // Object stores commonly hand out images without a useful type. Refusing these
        // would break deployments that work today, so only HTML is rejected outright.
        assert!(content_type_is_usable_as_media("application/octet-stream"));
        assert!(content_type_is_usable_as_media(""));
    }

    #[test]
    fn routes_are_not_mistaken_for_assets() {
        assert!(!looks_like_asset("dashboard/projects"));
        assert!(!looks_like_asset(
            "projects/6a0a1e2c-0000-0000-0000-000000000000"
        ));
        assert!(!looks_like_asset("teams/abc/workbench"));
        // A dotted directory must not make the last segment look like a file.
        assert!(!looks_like_asset("v1.2/dashboard"));
    }

    #[test]
    fn plain_paths_pass_through() {
        assert_eq!(
            safe_relative_path("assets/index-abc.js").unwrap(),
            "assets/index-abc.js"
        );
        assert_eq!(
            safe_relative_path("static/favicon.png").unwrap(),
            "static/favicon.png"
        );
    }

    /// Percent-encoded names must be decoded, or non-ASCII assets would 404.
    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(safe_relative_path("a%20b.png").unwrap(), "a b.png");
        assert_eq!(
            safe_relative_path("%E5%9B%BE%E6%A0%87.png").unwrap(),
            "图标.png"
        );
    }

    #[test]
    fn traversal_is_rejected() {
        assert!(safe_relative_path("../package.json").is_none());
        assert!(safe_relative_path("assets/../../secret").is_none());
        assert!(safe_relative_path("a/./b").is_none());
    }

    /// The encoded form is the one that actually arrives on the wire.
    #[test]
    fn encoded_traversal_is_rejected() {
        assert!(safe_relative_path("..%2f..%2fpackage.json").is_none());
        assert!(safe_relative_path("%2e%2e/secret").is_none());
        assert!(safe_relative_path("assets/%2e%2e/%2e%2e/secret").is_none());
    }

    #[test]
    fn windows_and_absolute_paths_are_rejected() {
        assert!(safe_relative_path("..\\..\\secret").is_none());
        assert!(safe_relative_path("C:/Windows/win.ini").is_none());
        assert!(safe_relative_path("/etc/passwd").is_none());
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        assert!(safe_relative_path("%FF%FE").is_none());
    }

    #[test]
    fn rewrite_prefixes_are_distinct() {
        use crate::rewrite::{DOWNLOAD_PREFIX, MEDIA_PREFIX};
        assert_ne!(MEDIA_PREFIX, DOWNLOAD_PREFIX);
        // Both must stay under a reserved, non-API path so they can never shadow a real route.
        assert!(MEDIA_PREFIX.starts_with("/__"));
        assert!(DOWNLOAD_PREFIX.starts_with("/__"));
    }
}
