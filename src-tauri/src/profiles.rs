//! Server profiles and connection discovery.
//!
//! The frontend always requests `/api/<rest>` (its build-time default). What that maps to
//! depends on the deployment:
//!
//!   * nginx deployments (moeflow.basmc.org, demo.moeflow.org, the internal test box) use
//!     `location /api { proxy_pass http://moeflow-backend:5000/; }` — the trailing slash
//!     *strips* `/api`, so `/api/v1/x` reaches the backend as `/v1/x`. The API base is
//!     therefore `<site>/api` and we forward verbatim.
//!   * moetran.com serves the frontend and a backend on a *separate* origin
//!     (`api.moetran.com`) with no `/api` segment at all.
//!
//! So a profile stores a single fully-resolved `api_base`; the proxy just appends the path.
//!
//! Discovery cannot trust HTTP status codes: `location / { try_files $uri /index.html; }`
//! answers *every* unknown path with the SPA shell and a 200. The reliable probe is the
//! backend's public `/ping`, which returns the literal body `pong`.

use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const PRESETS_JSON: &str = include_str!("../presets/servers.json");

#[derive(Debug, Clone, Deserialize)]
pub struct ServerPreset {
    pub host: String,
    pub name: String,
    pub api_base: String,
    #[serde(default)]
    pub media_origins: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PresetsFile {
    presets: Vec<ServerPreset>,
}

/// Ports are allocated from here so they stay clear of common dev servers.
const PORT_RANGE_START: u16 = 47100;

pub fn default_cache_limit() -> u64 {
    2 * 1024 * 1024 * 1024 // 2 GiB
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Human-facing site URL — opened in the system browser, never loaded by the shell.
    pub site_url: String,
    /// Where `/api/<rest>` is forwarded. See the module docs.
    pub api_base: String,
    /// Loopback port for this profile. Distinct per profile so that the `token` cookie
    /// (which is origin-scoped) cannot leak between servers.
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub allow_invalid_certs: bool,
    /// Additional origins whose absolute URLs get routed through the media cache.
    /// Populated by discovery plus whatever the API hands us at runtime.
    #[serde(default)]
    pub media_origins: Vec<String>,
}

impl Profile {
    /// The `Referer` a browser would send when loading this instance's media.
    ///
    /// Object-storage buckets behind these deployments commonly set a Referer allowlist
    /// that **denies requests with no Referer at all**. The browser always sends one, so the
    /// images load there; our proxy stands in for the browser, so it has to send one too.
    /// Without it every image from such a bucket fails with 403 — and, because the bucket's
    /// edge may cache a success, the failure can look intermittent and cache-dependent.
    pub fn media_referer(&self) -> Option<String> {
        let origin = self.site_origin();
        if origin.is_empty() {
            return None;
        }
        Some(format!("{origin}/"))
    }

    /// `scheme://host[:port]` of the site, used to resolve origin-relative media URLs.
    ///
    /// Note this is the *site*, not `api_base`: on a split deployment like moetran.com the
    /// API lives on `api.moetran.com` while storage paths belong to the site.
    pub fn site_origin(&self) -> String {
        let raw = if self.site_url.is_empty() {
            self.api_base.as_str()
        } else {
            self.site_url.as_str()
        };
        match raw.split_once("://") {
            Some((scheme, rest)) => {
                let host = rest.split('/').next().unwrap_or(rest);
                if host.is_empty() {
                    String::new()
                } else {
                    format!("{scheme}://{host}")
                }
            }
            None => String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStore {
    pub active: String,
    pub profiles: Vec<Profile>,
    #[serde(default = "default_cache_limit")]
    pub cache_limit_bytes: u64,
    /// Start straight in the remembered instance instead of showing the picker.
    ///
    /// Off by default: launching into whichever instance happens to be last used is
    /// exactly the surprise a multi-instance client should not spring on you.
    #[serde(default)]
    pub skip_launcher: bool,
}

/// Result of probing a candidate server.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub ok: bool,
    /// Resolved API base, when a probe succeeded.
    pub api_base: Option<String>,
    /// Which candidate matched, for display.
    pub matched: Option<String>,
    pub message: String,
    /// Candidates that were tried and rejected, for troubleshooting.
    pub tried: Vec<String>,
}

impl ProfileStore {
    /// Ship with the one instance this build is for.
    ///
    /// Deliberately a single preset: a client that launches with a list of somebody else's
    /// servers invites connecting to the wrong one. Anything else is added through
    /// 「添加实例」, which probes the address and fills in the real API base.
    pub fn with_presets() -> Self {
        let profiles = vec![Profile {
            id: "moetran".into(),
            name: "尨译 MoeTran".into(),
            site_url: "https://moetran.com".into(),
            // Frontend and backend live on different hosts; the API has no /api segment.
            api_base: "https://api.moetran.com".into(),
            port: 0,
            allow_invalid_certs: false,
            // Storage sits on an Aliyun OSS bucket (c01.m-t.pics) whose URLs are absolute,
            // so rewriting does not need this. It stays as the alternate origin to try when
            // a relative storage path is pinned to the site and that path 404s.
            media_origins: vec!["https://api.moetran.com".into()],
        }];
        Self {
            active: "moetran".into(),
            profiles,
            cache_limit_bytes: default_cache_limit(),
            skip_launcher: false,
        }
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join("profiles.json")
    }

    pub fn load(dir: &Path) -> Self {
        let path = Self::path(dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                eprintln!("[profiles] {} is unreadable ({err}); using presets", path.display());
                Self::with_presets()
            }),
            Err(_) => {
                let store = Self::with_presets();
                let _ = store.save(dir);
                store
            }
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::path(dir), text)
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn active_profile(&self) -> Option<&Profile> {
        self.get(&self.active).or_else(|| self.profiles.first())
    }

    /// Give every profile a stable loopback port, reassigning any that are unset or now taken.
    ///
    /// Keeping the port stable across restarts matters: the `token` cookie is scoped to the
    /// loopback origin, so a changing port would silently log the user out.
    pub fn ensure_ports(&mut self) {
        let mut claimed: Vec<u16> = Vec::new();
        for profile in self.profiles.iter_mut() {
            if profile.port != 0 && !claimed.contains(&profile.port) && port_is_free(profile.port) {
                claimed.push(profile.port);
            } else {
                let fresh = allocate_port(&claimed);
                claimed.push(fresh);
                profile.port = fresh;
            }
        }
    }
}

/// Is the port bindable right now? A profile whose port is held by our own already-running
/// server would report `false`, which is why `ensure_ports` is only called at startup.
pub fn port_is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn allocate_port(used: &[u16]) -> u16 {
    for port in PORT_RANGE_START..=PORT_RANGE_START + 500 {
        if !used.contains(&port) && port_is_free(port) {
            return port;
        }
    }
    // Fall back to an OS-assigned port rather than failing to start.
    0
}

fn load_presets() -> Vec<ServerPreset> {
    serde_json::from_str::<PresetsFile>(PRESETS_JSON)
        .map(|f| f.presets)
        .unwrap_or_default()
}

/// Extract the host from a URL or address string.
fn extract_host(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if let Some((_, rest)) = trimmed.split_once("://") {
        let host = rest.split('/').next().unwrap_or(rest);
        if !host.is_empty() {
            return Some(host.to_string());
        }
    }
    None
}

/// Find a preset matching the given input address.
pub fn find_preset(input: &str) -> Option<ServerPreset> {
    let host = extract_host(input)?;
    load_presets().into_iter().find(|p| p.host == host)
}

/// Build a fallback API base when no preset matches: `<input>/api`.
pub fn default_api_base(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    format!("{}/api", trimmed)
}

/// Build the candidate API bases to try for a user-entered URL, most likely first.
/// Whether a profile's API base still has to be discovered before it can be saved.
///
/// The address a person types is a *site* address; the API may sit under `/api` on the same
/// host, on a sibling subdomain, or at the root. Falling back to the site address is the
/// tempting shortcut and it is wrong: `/api/v1/x` then reaches `<site>/v1/x`, which nginx
/// serves from the SPA fallback — 200 HTML for GET, 405 for POST. The client looks connected
/// and then fails one endpoint at a time (the login captcha simply never appears).
///
/// A base that differs from the site is taken at face value: someone who typed a specific
/// API address may well know something the probe cannot discover.
pub fn api_base_needs_resolution(profile: &Profile) -> bool {
    let api = profile.api_base.trim().trim_end_matches('/');
    if api.is_empty() {
        return true;
    }
    let site = profile.site_url.trim().trim_end_matches('/');
    !site.is_empty() && api == site
}

pub fn api_base_candidates(input: &str) -> Vec<String> {
    let trimmed = input.trim().trim_end_matches('/').to_string();
    let mut out = vec![format!("{trimmed}/api"), trimmed.clone()];
    // moetran.com hosts its API on a sibling subdomain (`api.moetran.com`), with the API
    // served at the root rather than under `/api`.
    if let Some(rest) = trimmed.strip_prefix("https://") {
        let host = rest.split('/').next().unwrap_or(rest);
        if !host.starts_with("api.") && host.contains('.') {
            out.push(format!("https://api.{host}"));
        }
    }
    out.dedup();
    out
}

/// A candidate answers correctly only when `/ping` returns the literal body `pong`.
pub async fn probe_ping(client: &reqwest::Client, api_base: &str) -> bool {
    let url = format!("{}/ping", api_base.trim_end_matches('/'));
    let Ok(resp) = client.get(&url).send().await else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    match resp.text().await {
        Ok(body) => body.trim() == "pong",
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_try_api_prefix_then_root_then_api_subdomain() {
        let c = api_base_candidates("https://moetran.com");
        assert_eq!(c[0], "https://moetran.com/api");
        assert_eq!(c[1], "https://moetran.com");
        assert_eq!(c[2], "https://api.moetran.com");
    }

    #[test]
    fn candidates_do_not_double_wrap_an_api_subdomain() {
        let c = api_base_candidates("https://api.moetran.com");
        assert_eq!(c, vec!["https://api.moetran.com/api", "https://api.moetran.com"]);
    }

    /// End-to-end check of the discovery rule against a stub that behaves like the real
    /// deployments: `try_files $uri /index.html` answers *every* unknown path with the SPA
    /// shell, so only `/api/ping` ever reaches the backend.
    ///
    /// This is the bug in one test — the bare site address looks alive (200, HTML) and is
    /// the tempting thing to save, while the base that actually works is `<site>/api`.
    #[tokio::test]
    async fn probing_a_site_address_finds_the_api_under_the_api_prefix() {
        use axum::routing::get;

        let app = axum::Router::new()
            .route("/api/ping", get(|| async { "pong" }))
            .fallback(|| async {
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        "text/html; charset=utf-8",
                    )],
                    "<!DOCTYPE html><html lang=\"zh-CN\"><head></head><body>SPA</body></html>",
                )
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let site = format!("http://127.0.0.1:{}", addr.port());

        // The shortcut must be demonstrably wrong, not merely unverified.
        assert!(
            !probe_ping(&client, &site).await,
            "a SPA fallback's HTML must never be mistaken for an API"
        );

        // And the real resolution must land on the /api base.
        let mut resolved = None;
        for candidate in api_base_candidates(&site) {
            if probe_ping(&client, &candidate).await {
                resolved = Some(candidate);
                break;
            }
        }
        assert_eq!(resolved.as_deref(), Some(format!("{site}/api").as_str()));
    }

    /// The mistake this guards against: saving the *site* address as the API base. It looks
    /// fine in the picker and then breaks every call — `/api/v1/x` lands on `<site>/v1/x`,
    /// which nginx answers from the SPA fallback (200 for GET, 405 for POST, so the login
    /// captcha silently never renders).
    #[test]
    fn a_site_address_saved_as_the_api_base_needs_resolution() {
        let mut p = profile("https://moeflow.basmc.org", "https://moeflow.basmc.org");
        assert!(api_base_needs_resolution(&p));

        // Trailing slashes must not disguise it.
        p.api_base = "https://moeflow.basmc.org/".into();
        assert!(api_base_needs_resolution(&p));

        p.api_base = "https://moeflow.basmc.org/api".into();
        assert!(!api_base_needs_resolution(&p));
    }

    #[test]
    fn an_empty_api_base_needs_resolution() {
        let p = profile("https://moeflow.basmc.org", "");
        assert!(api_base_needs_resolution(&p));
    }

    /// A deliberately different API host — moetran's whole shape — is taken as given.
    #[test]
    fn a_distinct_api_base_is_left_alone() {
        let p = profile("https://moetran.com", "https://api.moetran.com");
        assert!(!api_base_needs_resolution(&p));
    }

    #[test]
    fn candidates_ignore_a_trailing_slash() {
        let c = api_base_candidates("https://demo.moeflow.org/");
        assert_eq!(c[0], "https://demo.moeflow.org/api");
    }

    fn profile(site: &str, api: &str) -> Profile {
        Profile {
            id: "t".into(),
            name: "t".into(),
            site_url: site.into(),
            api_base: api.into(),
            port: 0,
            allow_invalid_certs: false,
            media_origins: vec![],
        }
    }

    #[test]
    fn site_origin_drops_path_and_query() {
        assert_eq!(
            profile("https://moeflow.basmc.org/some/path", "x").site_origin(),
            "https://moeflow.basmc.org"
        );
    }

    #[test]
    fn site_origin_keeps_a_nonstandard_port() {
        assert_eq!(
            profile("http://172.29.133.24:8080", "x").site_origin(),
            "http://172.29.133.24:8080"
        );
    }

    /// On a split deployment the media belongs to the site, not the API host.
    /// The browser sends a Referer; object-storage buckets that deny empty Referers only
    /// serve images when we send one too.
    #[test]
    fn media_referer_matches_the_site_origin() {
        let p = profile("https://moetran.com", "https://api.moetran.com");
        assert_eq!(p.media_referer().as_deref(), Some("https://moetran.com/"));
    }

    #[test]
    fn media_referer_is_absent_without_a_usable_origin() {
        let p = profile("", "");
        assert_eq!(p.media_referer(), None);
    }

    #[test]
    fn site_origin_is_the_site_not_the_api() {
        let p = profile("https://moetran.com", "https://api.moetran.com");
        assert_eq!(p.site_origin(), "https://moetran.com");
    }

    #[test]
    fn site_origin_falls_back_to_the_api_base() {
        assert_eq!(profile("", "https://api.moetran.com").site_origin(), "https://api.moetran.com");
    }

    /// One preset only — the instance this build is for. Everything else is added by hand.
    #[test]
    fn presets_contain_only_this_builds_instance() {
        let store = ProfileStore::with_presets();
        assert_eq!(store.profiles.len(), 1);
        let only = store.get("moetran").expect("the moetran preset must exist");
        assert_eq!(only.api_base, "https://api.moetran.com");
        assert_eq!(only.site_url, "https://moetran.com");
        assert_eq!(store.active, "moetran", "the preset must also be the active one");
    }

    #[test]
    fn preset_matching_extracts_host_correctly() {
        assert_eq!(extract_host("https://moetran.com"), Some("moetran.com".into()));
        assert_eq!(extract_host("https://moetran.com/"), Some("moetran.com".into()));
        assert_eq!(extract_host("https://moeflow.basmc.org/some/path"), Some("moeflow.basmc.org".into()));
        assert_eq!(extract_host("http://172.29.133.24:8080"), Some("172.29.133.24:8080".into()));
        assert_eq!(extract_host("not-a-url"), None);
    }

    #[test]
    fn finds_preset_for_known_hosts() {
        let preset = find_preset("https://moetran.com");
        assert!(preset.is_some());
        let p = preset.unwrap();
        assert_eq!(p.host, "moetran.com");
        assert_eq!(p.api_base, "https://api.moetran.com");

        let preset = find_preset("https://moeflow.basmc.org/some/path");
        assert!(preset.is_some());
        let p = preset.unwrap();
        assert_eq!(p.host, "moeflow.basmc.org");
        assert_eq!(p.api_base, "https://moeflow.basmc.org/api");
    }

    #[test]
    fn no_preset_for_unknown_hosts() {
        assert!(find_preset("https://example.com").is_none());
        assert!(find_preset("https://192.168.1.1").is_none());
    }

    #[test]
    fn default_api_base_appends_api_segment() {
        assert_eq!(default_api_base("https://example.com"), "https://example.com/api");
        assert_eq!(default_api_base("https://example.com/"), "https://example.com/api");
        assert_eq!(default_api_base("http://192.168.1.1:8080"), "http://192.168.1.1:8080/api");
    }

    #[test]
    fn ports_are_assigned_uniquely() {
        let mut store = ProfileStore::with_presets();
        store.ensure_ports();
        let ports: Vec<u16> = store.profiles.iter().map(|p| p.port).collect();
        assert!(ports.iter().all(|p| *p != 0), "every profile needs a port: {ports:?}");
        let mut sorted = ports.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ports.len(), "ports must be distinct: {ports:?}");
    }
}
