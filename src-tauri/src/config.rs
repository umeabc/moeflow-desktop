//! Serialized fallible configuration mutations and incremental loopback listeners.
use crate::media::MediaCache;
use crate::network::{ClientBundle, ProxySettings, SharedClients};
use crate::profiles::{self, Profile, ProfileStore};
use crate::server::Ctx;
use crate::ServerHandle;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

pub struct AppState {
    pub config_dir: PathBuf,
    pub store: Mutex<ProfileStore>,
    pub cache: Arc<MediaCache>,
    pub servers: Mutex<HashMap<String, ServerHandle>>,
    pub shell_port: u16,
    mutation: tokio::sync::Mutex<()>,
    pub clients: SharedClients,
    publication: Arc<RwLock<()>>,
    runtime: Mutex<Option<(Option<tauri::AppHandle>, PathBuf)>>,
}

impl AppState {
    pub fn new(
        config_dir: PathBuf,
        mut store: ProfileStore,
        cache: Arc<MediaCache>,
        shell_port: u16,
    ) -> Result<Self, String> {
        store.proxy = store.proxy.validated()?;
        let clients = Arc::new(RwLock::new(Arc::new(ClientBundle::build(&store.proxy)?)));
        Ok(Self {
            config_dir,
            store: Mutex::new(store),
            cache,
            servers: Mutex::new(HashMap::new()),
            shell_port,
            mutation: tokio::sync::Mutex::new(()),
            clients,
            publication: Arc::new(RwLock::new(())),
            runtime: Mutex::new(None),
        })
    }

    pub fn active(&self) -> Option<Profile> {
        self.store
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active_profile()
            .cloned()
    }

    pub fn store_snapshot(&self) -> ProfileStore {
        self.store.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub async fn start_servers(
        &self,
        app: Option<tauri::AppHandle>,
        web_root: PathBuf,
    ) -> Result<(), String> {
        let _gate = self.mutation.lock().await;
        let runtime = (app, web_root);
        self.publish(self.store_snapshot(), &runtime).await?;
        *self.runtime.lock().unwrap_or_else(|p| p.into_inner()) = Some(runtime);
        Ok(())
    }

    async fn publish(
        &self,
        mut candidate: ProfileStore,
        runtime: &(Option<tauri::AppHandle>, PathBuf),
    ) -> Result<(), String> {
        let ids: HashSet<_> = candidate.profiles.iter().map(|p| p.id.clone()).collect();
        if ids.len() != candidate.profiles.len() {
            return Err("服务器配置 ID 重复".into());
        }
        candidate.proxy = candidate.proxy.validated()?;
        let old = self.store_snapshot();
        let replacement = if old.proxy != candidate.proxy {
            Some(Arc::new(ClientBundle::build(&candidate.proxy)?))
        } else {
            None
        };
        let existing: HashMap<String, u16> = self
            .servers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(id, h)| (id.clone(), h.port))
            .collect();
        let mut prepared = Vec::new();
        let mut used: HashSet<u16> = existing.values().copied().collect();
        for profile in &mut candidate.profiles {
            if let Some(port) = existing.get(&profile.id) {
                profile.port = *port;
                continue;
            }
            if profile.port != 0 && !used.insert(profile.port) {
                return Err("服务器配置端口重复".into());
            }
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", profile.port))
                .await
                .map_err(|e| {
                    format!("无法绑定 {} 的本地端口 {}：{e}", profile.name, profile.port)
                })?;
            profile.port = listener.local_addr().map_err(|e| e.to_string())?.port();
            used.insert(profile.port);
            let ctx = Arc::new(Ctx::new(
                profile.clone(),
                runtime.1.clone(),
                self.cache.clone(),
                runtime.0.clone(),
                self.clients.clone(),
                self.publication.clone(),
            ));
            prepared.push((profile.id.clone(), profile.port, ctx, listener));
        }
        // No runtime state has changed yet. Unserved listeners drop automatically on error.
        candidate
            .save(&self.config_dir)
            .map_err(|e| format!("保存配置失败：{e}"))?;
        {
            let mut servers = self.servers.lock().unwrap_or_else(|p| p.into_inner());
            let _publication = self.publication.write().unwrap_or_else(|p| p.into_inner());
            for profile in &candidate.profiles {
                if let Some(handle) = servers.get(&profile.id) {
                    handle.ctx.update_profile(profile.clone());
                }
            }
            if let Some(bundle) = replacement {
                *self.clients.write().unwrap_or_else(|p| p.into_inner()) = bundle;
            }
            for (id, port, ctx, listener) in prepared {
                let shutdown = crate::server::serve_listener(ctx.clone(), listener);
                servers.insert(
                    id,
                    ServerHandle {
                        port,
                        ctx,
                        shutdown: Some(shutdown),
                    },
                );
            }
            servers.retain(|id, handle| {
                if ids.contains(id) {
                    true
                } else {
                    if let Some(tx) = handle.shutdown.take() {
                        let _ = tx.send(());
                    }
                    false
                }
            });
            *self.store.lock().unwrap_or_else(|p| p.into_inner()) = candidate.clone();
        }
        if old.cache_limit_bytes != candidate.cache_limit_bytes {
            self.cache.set_limit(candidate.cache_limit_bytes);
        }
        Ok(())
    }

    async fn mutate<F>(&self, edit: F) -> Result<(), String>
    where
        F: FnOnce(&mut ProfileStore) -> Result<(), String>,
    {
        let _gate = self.mutation.lock().await;
        let mut candidate = self.store_snapshot();
        edit(&mut candidate)?;
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or("本地服务尚未启动")?;
        self.publish(candidate, &runtime).await
    }

    pub async fn upsert(&self, mut profile: Profile) -> Result<(), String> {
        let _gate = self.mutation.lock().await;
        let mut candidate = self.store_snapshot();
        if profile.id.trim().is_empty() {
            return Err("服务器配置 ID 不能为空".into());
        }
        if profiles::api_base_needs_resolution(&profile) {
            let input = if profile.api_base.trim().is_empty() {
                profile.site_url.clone()
            } else {
                profile.api_base.clone()
            };
            if let Some(preset) = profiles::find_preset(&input) {
                profile.api_base = preset.api_base;
                profile.media_origins = preset.media_origins;
                if profile.name.trim().is_empty() {
                    profile.name = preset.name;
                }
            } else {
                let client = self
                    .clients
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .probe
                    .clone();
                profile.api_base = profiles::default_api_base(&input);
                for base in profiles::api_base_candidates(&input) {
                    if profiles::probe_ping_network(&client, &base).await {
                        profile.api_base = base;
                        break;
                    }
                }
            }
            if profile.site_url.trim().is_empty() {
                profile.site_url = input;
            }
        }
        let url = reqwest::Url::parse(&profile.api_base).map_err(|_| "API 地址无效")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("API 地址必须使用 HTTP 或 HTTPS".into());
        }
        match candidate.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => {
                profile.port = existing.port;
                *existing = profile;
            }
            None => {
                profile.port = 0;
                candidate.profiles.push(profile);
            }
        }
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or("本地服务尚未启动")?;
        self.publish(candidate, &runtime).await
    }

    pub async fn delete(&self, id: String) -> Result<(), String> {
        self.mutate(move |store| {
            if store.get(&id).is_none() {
                return Err(format!("未知的服务器配置：{id}"));
            }
            if store.profiles.len() <= 1 {
                return Err("至少要保留一个服务器配置".into());
            }
            store.profiles.retain(|p| p.id != id);
            if store.active == id {
                store.active = store.profiles[0].id.clone();
            }
            Ok(())
        })
        .await
    }

    pub async fn set_proxy(&self, settings: ProxySettings) -> Result<(), String> {
        let settings = settings.validated()?;
        self.mutate(move |store| {
            store.proxy = settings;
            Ok(())
        })
        .await
    }

    pub async fn set_skip(&self, skip: bool) -> Result<(), String> {
        self.mutate(move |store| {
            store.skip_launcher = skip;
            Ok(())
        })
        .await
    }

    pub async fn set_limit(&self, bytes: u64) -> Result<(), String> {
        self.mutate(move |store| {
            store.cache_limit_bytes = bytes;
            Ok(())
        })
        .await
    }

    pub async fn set_active(&self, id: String) -> Result<u16, String> {
        let mut port = 0;
        self.mutate(|store| {
            port = store
                .get(&id)
                .ok_or_else(|| format!("未知的服务器配置：{id}"))?
                .port;
            store.active = id;
            Ok(())
        })
        .await?;
        Ok(port)
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        for handle in self
            .servers
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .values_mut()
        {
            if let Some(tx) = handle.shutdown.take() {
                let _ = tx.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests;
