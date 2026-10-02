//! Rewrite absolute storage URLs in API responses so they flow through the loopback server.
//!
//! The backend hands out fully-qualified URLs built from its `STORAGE_DOMAIN` (e.g.
//! `https://moeflow.basmc.org/storage/abc/001.png`, or a completely different host on
//! deployments that use object storage). The frontend drops those straight into
//! `<img src>` and `<a href>`, so unless we rewrite them the media cache never sees them
//! and downloads never reach the native save dialog.
//!
//! The rewrite is deliberately best-effort: an unrecognised URL is left untouched and
//! still loads directly from upstream. A miss costs caching, never correctness.

use base64::Engine as _;
use serde_json::Value;

/// Keys whose string values may hold a fetchable storage URL.
const MEDIA_KEYS: &[&str] = &[
    "url",
    "cover_url",
    "avatar",
    "safe_check_url",
    "file_url",
    "image_url",
    "preview_url",
    "thumb_url",
    "src",
];

/// Keys that are downloadable artefacts rather than inline media.
const DOWNLOAD_KEYS: &[&str] = &["link", "download_url"];

pub const MEDIA_PREFIX: &str = "/__media/";
pub const DOWNLOAD_PREFIX: &str = "/__download/";

fn b64_encode(text: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.as_bytes())
}

/// Decode the origin segment of a `/__media/<token>/...` path.
pub fn decode_origin(token: &str) -> Option<String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Split an absolute URL into `(origin, rest)`, where rest begins with `/`.
fn split_origin(url: &str) -> Option<(String, String)> {
    let (scheme, after) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    match after.find('/') {
        Some(idx) => {
            let host = &after[..idx];
            Some((format!("{scheme}://{host}"), after[idx..].to_string()))
        }
        // No path at all — not something we can usefully route.
        None => None,
    }
}

/// True when the value is origin-relative (`/storage/x.png`) rather than absolute.
fn is_origin_relative(url: &str) -> bool {
    // Requires something after the leading slash: a bare "/" is a link to the site root,
    // not a file, and rewriting it would produce a nonsense media URL.
    // `//host/path` is protocol-relative, not origin-relative — leave that alone too.
    url.len() > 1 && url.starts_with('/') && !url.starts_with("//")
}

/// Encode a media URL as a loopback path under `prefix`.
///
/// Absolute URLs keep their own origin. Origin-relative ones are pinned to `base_origin`
/// (the configured site), because in the desktop shell a bare `/storage/...` would resolve
/// against the loopback server and hit the SPA fallback instead of the real file.
pub fn to_loopback(prefix: &str, url: &str, base_origin: &str) -> Option<String> {
    if let Some((origin, rest)) = split_origin(url) {
        return Some(format!("{prefix}{}{}", b64_encode(&origin), rest));
    }
    if is_origin_relative(url) && !base_origin.is_empty() {
        return Some(format!("{prefix}{}{}", b64_encode(base_origin), url));
    }
    None
}

#[derive(Debug, Default)]
pub struct RewriteOutcome {
    /// Origins discovered in this response that were not previously known.
    pub learned_origins: Vec<String>,
    /// The upstream URL behind each rewritten value, for diagnostics.
    pub rewritten: Vec<String>,
    /// True when at least one value was rewritten.
    pub changed: bool,
}

/// Rewrite media/download URLs in an API JSON body.
///
/// `known_origins` seeds the set of origins we will rewrite; origins seen for the first
/// time are collected into `learned_origins` so the caller can cache them.
/// `base_origin` is the site origin used to resolve origin-relative URLs.
/// Returns `None` when the body is not JSON (HTML error pages, binary, …).
pub fn rewrite_api_json(
    body: &[u8],
    known_origins: &[String],
    base_origin: &str,
) -> Option<(Vec<u8>, RewriteOutcome)> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    let mut origins: Vec<String> = known_origins.to_vec();
    let mut outcome = RewriteOutcome::default();

    walk(&mut value, &mut origins, &mut outcome, base_origin);

    if !outcome.changed {
        // Still re-serialize so the caller gets a consistent representation.
        return serde_json::to_vec(&value).ok().map(|b| (b, outcome));
    }
    serde_json::to_vec(&value).ok().map(|b| (b, outcome))
}

fn walk(
    value: &mut Value,
    origins: &mut Vec<String>,
    outcome: &mut RewriteOutcome,
    base_origin: &str,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let is_media = MEDIA_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key));
                let is_download = DOWNLOAD_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key));

                if (is_media || is_download) && child.is_string() {
                    let raw = child.as_str().unwrap_or_default().to_string();
                    // Only learn origins from absolute URLs; relative ones carry no origin
                    // of their own.
                    if let Some((origin, _)) = split_origin(&raw) {
                        if !origins.iter().any(|o| o == &origin) {
                            origins.push(origin.clone());
                            outcome.learned_origins.push(origin.clone());
                        }
                    }
                    let prefix = if is_download { DOWNLOAD_PREFIX } else { MEDIA_PREFIX };
                    if let Some(rewritten) = to_loopback(prefix, &raw, base_origin) {
                        *child = Value::String(rewritten);
                        outcome.rewritten.push(raw);
                        outcome.changed = true;
                        continue;
                    }
                }

                walk(child, origins, outcome, base_origin);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                walk(item, origins, outcome, base_origin);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SITE: &str = "https://moeflow.basmc.org";

    fn run(body: serde_json::Value) -> (Value, RewriteOutcome) {
        let bytes = serde_json::to_vec(&body).unwrap();
        let (out, outcome) = rewrite_api_json(&bytes, &[], SITE).unwrap();
        (serde_json::from_slice(&out).unwrap(), outcome)
    }

    #[test]
    fn rewrites_media_urls_and_keeps_path_and_query() {
        let (value, outcome) = run(json!({
            "id": "1",
            "cover_url": "https://moeflow.basmc.org/storage/2026/01/abc.png?x=1",
        }));
        let cover = value["cover_url"].as_str().unwrap();
        assert!(cover.starts_with(MEDIA_PREFIX), "got {cover}");
        assert!(cover.ends_with("/storage/2026/01/abc.png?x=1"), "got {cover}");
        assert_eq!(outcome.learned_origins, vec!["https://moeflow.basmc.org"]);
        assert!(outcome.changed);
    }

    /// Origin-relative storage paths must be pinned to the configured site.
    ///
    /// This is what `STORAGE_DOMAIN=/storage/` produces. In a browser such a path resolves
    /// against the site and works; in the desktop shell it would resolve against the
    /// loopback server, miss every local file, and be answered by the SPA fallback — an
    /// HTML page where an image was expected, i.e. a silently broken image.
    #[test]
    fn origin_relative_media_is_pinned_to_the_site() {
        let (value, outcome) = run(json!({"cover_url": "/storage/2026/01/abc.png"}));
        let cover = value["cover_url"].as_str().unwrap();
        assert!(cover.starts_with(MEDIA_PREFIX), "got {cover}");
        assert!(cover.ends_with("/storage/2026/01/abc.png"), "got {cover}");
        assert_eq!(
            decode_origin(cover.trim_start_matches(MEDIA_PREFIX).split('/').next().unwrap())
                .unwrap(),
            SITE
        );
        assert!(outcome.changed);
        // A relative URL carries no origin worth learning.
        assert!(outcome.learned_origins.is_empty());
    }

    #[test]
    fn origin_relative_downloads_use_the_download_route() {
        let (value, _) = run(json!({"link": "/storage/out/a.zip"}));
        assert!(value["link"].as_str().unwrap().starts_with(DOWNLOAD_PREFIX));
    }

    /// Degenerate inputs must be left exactly as they were.
    #[test]
    fn root_and_protocol_relative_paths_are_left_alone() {
        let (value, outcome) = run(json!({"url": "/", "cover_url": "//cdn.test/a.png"}));
        assert_eq!(value["url"], "/");
        assert_eq!(value["cover_url"], "//cdn.test/a.png");
        assert!(!outcome.changed);
    }

    #[test]
    fn relative_rewrite_is_skipped_without_a_site_origin() {
        let bytes = serde_json::to_vec(&json!({"url": "/storage/a.png"})).unwrap();
        let (_, outcome) = rewrite_api_json(&bytes, &[], "").unwrap();
        assert!(!outcome.changed);
    }

    #[test]
    fn download_links_use_the_download_route() {
        let (value, _) = run(json!({"link": "https://cdn.test/out/a.zip"}));
        assert!(value["link"].as_str().unwrap().starts_with(DOWNLOAD_PREFIX));
    }

    #[test]
    fn non_media_fields_are_left_alone() {
        let (value, outcome) = run(json!({
            "name": "https://example.test/not-media",
            "intro": "see https://example.test/docs",
        }));
        assert_eq!(value["name"], "https://example.test/not-media");
        assert_eq!(value["intro"], "see https://example.test/docs");
        assert!(!outcome.changed);
    }

    #[test]
    fn nested_arrays_are_walked() {
        let (value, _) = run(json!({
            "data": [{"cover_url": "https://cdn.test/a.png"}],
        }));
        assert!(value["data"][0]["cover_url"]
            .as_str()
            .unwrap()
            .starts_with(MEDIA_PREFIX));
    }

    #[test]
    fn non_json_bodies_are_rejected_so_callers_pass_them_through() {
        assert!(rewrite_api_json(b"<!DOCTYPE html>", &[], SITE).is_none());
    }

    #[test]
    fn each_distinct_origin_is_learned_once() {
        let (_, outcome) = run(json!({
            "a": {"cover_url": "https://cdn.test/1.png"},
            "b": {"cover_url": "https://cdn.test/2.png"},
        }));
        assert_eq!(outcome.learned_origins, vec!["https://cdn.test"]);
    }

    #[test]
    fn origin_round_trips_through_base64() {
        let token = b64_encode("http://172.29.133.26:8080");
        assert_eq!(decode_origin(&token).unwrap(), "http://172.29.133.26:8080");
    }

    #[test]
    fn urls_without_a_path_are_not_rewritable() {
        let (value, outcome) = run(json!({"url": "https://example.test"}));
        assert_eq!(value["url"], "https://example.test");
        assert!(!outcome.changed);
    }
}
