//! Disk-backed LRU cache for storage images.
//!
//! Images are the bulk of the traffic in this app, and the same covers/page images get
//! re-fetched constantly while paging through the workbench. Caching them locally makes
//! paging effectively instant and lets already-viewed pages survive a network drop.
//!
//! Entries are keyed by the absolute upstream URL. The index is held in memory and flushed
//! lazily — bumping `atime` on every read would otherwise mean a disk write per image view.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
pub struct CacheStats {
    pub entries: u64,
    pub bytes: u64,
    pub limit_bytes: u64,
}

/// Bumped when a bug could have written wrong bytes under an otherwise valid key.
///
/// Entries from an older version are discarded instead of served: a poisoned cache is
/// worse than an empty one, because an empty cache self-heals on the next fetch while a
/// poisoned entry keeps serving the wrong image forever.
const CACHE_VERSION: u32 = 2;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Entry {
    size: u64,
    atime: u64,
    #[serde(default)]
    etag: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct IndexFile {
    version: u32,
    entries: HashMap<String, Entry>,
}

/// Per-process counter making scratch paths unique. See [`MediaCache::temp_path`].
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    total: u64,
    limit: u64,
    dirty: bool,
}

pub struct MediaCache {
    dir: PathBuf,
    index_path: PathBuf,
    inner: Mutex<Inner>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stable, filesystem-safe key for an upstream URL.
pub fn key_for(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

impl MediaCache {
    pub fn new(dir: PathBuf, limit: u64) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let index_path = dir.join("index.json");

        let stored: Option<IndexFile> = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());

        let mut reset = false;
        let mut entries = match stored {
            Some(index) if index.version == CACHE_VERSION => index.entries,
            // Either an older/unversioned index — whose entries we no longer trust to be the
            // right bytes for their key — or no index at all, which happens after a crash
            // before the first flush.
            _ => {
                reset = true;
                HashMap::new()
            }
        };

        // Drop index rows whose payload vanished (manual cleanup, AV quarantine, partial
        // copy) so the reported size stays honest.
        entries.retain(|key, _| dir.join(key).is_file());

        // …and drop payloads no row refers to, which is the same repair in the other
        // direction. On a reset this empties the directory outright.
        let keep: std::collections::HashSet<String> = entries.keys().cloned().collect();
        gc_payloads(&dir, &keep);

        // Scratch files are named uniquely per request, so any left behind belong to a run
        // that died mid-download. Nothing will ever claim them again.
        discard_scratch(&dir);

        let total = entries.values().map(|e| e.size).sum();
        Self {
            dir,
            index_path,
            inner: Mutex::new(Inner {
                entries,
                total,
                limit,
                // Mark the reset dirty so the emptied index reaches disk even if this run
                // never caches anything, instead of being re-discarded on every start.
                dirty: reset,
            }),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn stats(&self) -> CacheStats {
        let inner = self.lock();
        CacheStats {
            entries: inner.entries.len() as u64,
            bytes: inner.total,
            limit_bytes: inner.limit,
        }
    }

    pub fn set_limit(&self, limit: u64) {
        let mut inner = self.lock();
        inner.limit = limit;
        evict(&mut inner, &self.dir);
        inner.dirty = true;
    }

    /// Cached payload for `url`, if present. Refreshes its recency.
    pub fn get(&self, url: &str) -> Option<PathBuf> {
        let key = key_for(url);
        let mut inner = self.lock();
        let entry = inner.entries.get_mut(&key)?;
        let path = self.dir.join(&key);
        if !path.is_file() {
            let size = entry.size;
            inner.entries.remove(&key);
            inner.total = inner.total.saturating_sub(size);
            inner.dirty = true;
            return None;
        }
        entry.atime = now_secs();
        inner.dirty = true;
        Some(path)
    }

    pub fn etag_for(&self, url: &str) -> Option<String> {
        self.lock()
            .entries
            .get(&key_for(url))
            .and_then(|e| e.etag.clone())
    }

    /// A scratch path to stream a download into. Same filesystem as the final location so
    /// the commit is an atomic rename.
    ///
    /// Unique **per call**, not merely per second. A workbench page fetches a whole grid of
    /// images concurrently; if two downloads share a scratch file their bytes interleave,
    /// and whichever commits last caches a spliced payload under its own key — one image
    /// served in place of another. It also makes the losing `rename` fail on Windows, so
    /// the request that should have succeeded returns an error instead.
    pub fn temp_path(&self) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        self.dir
            .join(format!(".incoming-{}-{}", std::process::id(), seq))
    }

    /// Move a completed download into the cache.
    pub fn commit(&self, url: &str, temp: &Path, etag: Option<String>) -> io::Result<PathBuf> {
        let key = key_for(url);
        let final_path = self.dir.join(&key);
        std::fs::rename(temp, &final_path)?;
        let size = std::fs::metadata(&final_path)?.len();

        let mut inner = self.lock();
        if let Some(previous) = inner.entries.insert(
            key,
            Entry {
                size,
                atime: now_secs(),
                etag,
            },
        ) {
            inner.total = inner.total.saturating_sub(previous.size);
        }
        inner.total += size;
        evict(&mut inner, &self.dir);
        inner.dirty = true;
        Ok(final_path)
    }

    pub fn discard_temp(&self, temp: &Path) {
        let _ = std::fs::remove_file(temp);
    }

    pub fn clear(&self) -> io::Result<()> {
        let mut inner = self.lock();
        for key in inner.entries.keys() {
            let _ = std::fs::remove_file(self.dir.join(key));
        }
        inner.entries.clear();
        inner.total = 0;
        inner.dirty = true;
        drop(inner);
        self.flush()
    }

    /// Persist the index. Cheap enough to call on a timer and at shutdown.
    pub fn flush(&self) -> io::Result<()> {
        let snapshot = {
            let mut inner = self.lock();
            if !inner.dirty {
                return Ok(());
            }
            inner.dirty = false;
            inner.entries.clone()
        };
        let text = serde_json::to_string(&IndexFile {
            version: CACHE_VERSION,
            entries: snapshot,
        })?;
        let temp = self.index_path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, &self.index_path)
    }
}

/// Delete cached payloads that the index does not reference.
///
/// Lookups go through the index, so an unreferenced payload can never be served — it is
/// pure garbage, and it accumulates every time the process dies between a `commit` and the
/// next `flush`. Keeping the directory and the index in exact agreement is what stops the
/// cache from growing without bound. (Losing those payloads costs one re-download; they
/// were unreachable either way.)
fn gc_payloads(dir: &Path, keep: &std::collections::HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "index.json" || name.ends_with(".tmp") {
            continue;
        }
        if !keep.contains(name.as_ref()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Remove leftover `.incoming-*` scratch files.
fn discard_scratch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(".incoming-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Drop least-recently-used entries until the cache fits its limit.
fn evict(inner: &mut Inner, dir: &Path) {
    if inner.total <= inner.limit {
        return;
    }
    let mut by_age: Vec<(String, u64, u64)> = inner
        .entries
        .iter()
        .map(|(key, entry)| (key.clone(), entry.atime, entry.size))
        .collect();
    by_age.sort_by_key(|(_, atime, _)| *atime);

    for (key, _, size) in by_age {
        if inner.total <= inner.limit {
            break;
        }
        let _ = std::fs::remove_file(dir.join(&key));
        inner.entries.remove(&key);
        inner.total = inner.total.saturating_sub(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mfd-cache-test-{tag}-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(cache: &MediaCache, url: &str, bytes: &[u8]) {
        let temp = cache.temp_path();
        std::fs::write(&temp, bytes).unwrap();
        cache.commit(url, &temp, None).unwrap();
    }

    #[test]
    fn stores_and_returns_payloads() {
        let dir = temp_dir("roundtrip");
        let cache = MediaCache::new(dir.clone(), 1024 * 1024);
        put(&cache, "https://example.test/a.png", b"hello");

        let path = cache.get("https://example.test/a.png").expect("cache hit");
        assert_eq!(std::fs::read(path).unwrap(), b"hello");
        assert_eq!(cache.stats().bytes, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let dir = temp_dir("evict");
        let cache = MediaCache::new(dir.clone(), 10);

        put(&cache, "u1", b"aaaaa"); // 5 bytes
        put(&cache, "u2", b"bbbbb"); // 10 bytes total
        // Touch u1 so u2 becomes the least recently used.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(cache.get("u1").is_some());

        put(&cache, "u3", b"ccccc"); // would exceed 10 -> evict u2

        assert!(cache.get("u1").is_some(), "recently used entry must survive");
        assert!(cache.get("u2").is_none(), "least recently used entry must be evicted");
        assert!(cache.get("u3").is_some());
        assert!(cache.stats().bytes <= 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_survives_reopen() {
        let dir = temp_dir("reopen");
        {
            let cache = MediaCache::new(dir.clone(), 1024);
            put(&cache, "https://example.test/b.png", b"payload");
            cache.flush().unwrap();
        }
        let reopened = MediaCache::new(dir.clone(), 1024);
        assert!(reopened.get("https://example.test/b.png").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_index_rows_are_dropped() {
        let dir = temp_dir("stale");
        let cache = MediaCache::new(dir.clone(), 1024);
        put(&cache, "gone", b"data");
        cache.flush().unwrap();

        std::fs::remove_file(dir.join(key_for("gone"))).unwrap();

        let reopened = MediaCache::new(dir.clone(), 1024);
        assert_eq!(reopened.stats().entries, 0, "missing payload must not be counted");
        assert!(reopened.get("gone").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_empties_the_cache() {
        let dir = temp_dir("clear");
        let cache = MediaCache::new(dir.clone(), 1024);
        put(&cache, "x", b"1234");
        put(&cache, "y", b"5678");
        cache.clear().unwrap();
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().bytes, 0);
        assert!(cache.get("x").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: scratch paths used to be `<pid>-<seconds>`, so every concurrent download
    /// in the same second shared one file. Their bytes interleaved and one image was then
    /// cached under another's key — a corrupted payload that `serve_cached` happily handed
    /// out forever after.
    #[test]
    fn every_scratch_path_is_unique() {
        let dir = temp_dir("temp-unique");
        let cache = MediaCache::new(dir.clone(), 1024);

        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            assert!(
                seen.insert(cache.temp_path()),
                "temp_path returned a path already in use — concurrent downloads would collide"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Concurrent downloads must not be able to overwrite each other's payload.
    #[test]
    fn concurrent_commits_keep_their_own_bytes() {
        use std::sync::Arc;

        let dir = temp_dir("concurrent");
        let cache = Arc::new(MediaCache::new(dir.clone(), 1024 * 1024));

        let mut handles = Vec::new();
        for i in 0..24u8 {
            let cache = cache.clone();
            handles.push(std::thread::spawn(move || {
                let url = format!("https://example.test/{i}.png");
                let payload = vec![i; 4096];
                let temp = cache.temp_path();
                std::fs::write(&temp, &payload).unwrap();
                cache.commit(&url, &temp, None).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        for i in 0..24u8 {
            let url = format!("https://example.test/{i}.png");
            let path = cache.get(&url).expect("every url must be cached");
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(bytes.len(), 4096, "payload for {url} was truncated");
            assert!(
                bytes.iter().all(|b| *b == i),
                "payload for {url} contains bytes from another download"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cache written before the version marker is dropped rather than trusted.
    #[test]
    fn entries_from_an_older_version_are_discarded() {
        let dir = temp_dir("version");
        {
            let cache = MediaCache::new(dir.clone(), 1024);
            put(&cache, "https://example.test/a.png", b"poisoned");
            cache.flush().unwrap();
        }
        // Rewrite the index as an older, unversioned cache would have left it.
        std::fs::write(dir.join("index.json"), r#"{"https://example.test/a.png":{"size":8,"atime":1}}"#)
            .unwrap();

        let reopened = MediaCache::new(dir.clone(), 1024);
        assert_eq!(reopened.stats().entries, 0);
        assert!(reopened.get("https://example.test/a.png").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Scratch files from a run that died mid-download are never reclaimed, so clear them.
    #[test]
    fn stale_scratch_files_are_removed_on_open() {
        let dir = temp_dir("scratch");
        std::fs::create_dir_all(&dir).unwrap();
        let leftover = dir.join(".incoming-1234-7");
        std::fs::write(&leftover, b"half a download").unwrap();

        let _cache = MediaCache::new(dir.clone(), 1024);
        assert!(!leftover.exists(), "orphaned scratch file must be cleaned up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole point of the on-disk cache is surviving a restart — which requires the
    /// index to actually reach disk. It used to never be written, so every launch started
    /// from an empty cache.
    #[test]
    fn a_flushed_index_makes_the_cache_survive_a_restart() {
        let dir = temp_dir("persist");
        {
            let cache = MediaCache::new(dir.clone(), 1024 * 1024);
            put(&cache, "https://example.test/kept.png", b"payload");
            cache.flush().unwrap();
        }
        let reopened = MediaCache::new(dir.clone(), 1024 * 1024);
        assert_eq!(reopened.stats().entries, 1);
        assert!(reopened.get("https://example.test/kept.png").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Payloads the index does not reference can never be served, so they are collected —
    /// otherwise a crash between commit and flush leaks a file per download, forever.
    #[test]
    fn payloads_missing_from_the_index_are_collected() {
        let dir = temp_dir("orphan");
        {
            let cache = MediaCache::new(dir.clone(), 1024 * 1024);
            put(&cache, "https://example.test/known.png", b"payload");
            cache.flush().unwrap();
        }
        // A download that was committed but never flushed leaves a file with no index row.
        let orphan = dir.join(key_for("https://example.test/orphan.png"));
        std::fs::write(&orphan, b"never indexed").unwrap();

        let reopened = MediaCache::new(dir.clone(), 1024 * 1024);
        assert_eq!(reopened.stats().entries, 1);
        assert!(!orphan.exists(), "unreferenced payload must be collected");
        assert!(reopened.get("https://example.test/known.png").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
