//! Central outbound policy. Loopback targets always use a direct connection.
//!
//! System clients retain reqwest's environment and Windows static-proxy/bypass support.
//! PAC is not evaluated. Redirects crossing the loopback boundary are returned to callers
//! rather than followed with the wrong client policy; other redirects follow normally.
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, Url};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    System,
    Direct,
    Manual,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySettings {
    #[serde(default)]
    pub mode: ProxyMode,
    #[serde(default)]
    pub url: String,
}

impl ProxySettings {
    pub fn validated(mut self) -> Result<Self, String> {
        self.url = self.url.trim().to_string();
        if self.mode == ProxyMode::Manual {
            let url =
                Url::parse(&self.url).map_err(|_| "代理地址必须是完整的 HTTP URL".to_string())?;
            if url.scheme() != "http"
                || url.host_str().is_none()
                || url.port_or_known_default().is_none()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "手动代理必须使用 http://host:port，可包含用户名和密码，不支持路径".into(),
                );
            }
            reqwest::Proxy::all(&self.url).map_err(|e| format!("代理地址无效：{e}"))?;
        }
        Ok(self)
    }
}

pub fn is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("localhost.")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[derive(Clone)]
pub struct NetworkClient {
    policy: Client,
    direct: Client,
}

impl NetworkClient {
    pub fn build(
        settings: &ProxySettings,
        invalid_certs: bool,
        probe: bool,
    ) -> Result<Self, String> {
        let settings = settings.clone().validated()?;
        let build = |direct: bool| {
            let mut builder = Client::builder()
                .danger_accept_invalid_certs(invalid_certs)
                .connect_timeout(Duration::from_secs(if probe { 6 } else { 15 }))
                .timeout(Duration::from_secs(if probe { 10 } else { 600 }))
                .pool_max_idle_per_host(8)
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 10 {
                        return attempt.error("too many redirects");
                    }
                    if attempt
                        .previous()
                        .first()
                        .is_some_and(|first| is_loopback(first) != is_loopback(attempt.url()))
                    {
                        return attempt.stop();
                    }
                    attempt.follow()
                }));
            if direct || settings.mode == ProxyMode::Direct {
                builder = builder.no_proxy();
            } else if settings.mode == ProxyMode::Manual {
                builder = builder
                    .no_proxy()
                    .proxy(reqwest::Proxy::all(&settings.url).map_err(|e| e.to_string())?);
            }
            builder
                .build()
                .map_err(|e| format!("无法创建网络客户端：{e}"))
        };
        Ok(Self {
            policy: build(false)?,
            direct: build(true)?,
        })
    }

    pub fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let direct = Url::parse(url).ok().is_some_and(|url| is_loopback(&url));
        if direct {
            self.direct.request(method, url)
        } else {
            self.policy.request(method, url)
        }
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        self.request(Method::GET, url)
    }
}

pub struct ClientBundle {
    pub strict: NetworkClient,
    pub permissive: NetworkClient,
    pub probe: NetworkClient,
}

impl ClientBundle {
    pub fn build(settings: &ProxySettings) -> Result<Self, String> {
        Ok(Self {
            strict: NetworkClient::build(settings, false, false)?,
            permissive: NetworkClient::build(settings, true, false)?,
            probe: NetworkClient::build(settings, true, true)?,
        })
    }

    pub fn client(&self, invalid_certs: bool) -> NetworkClient {
        if invalid_certs {
            self.permissive.clone()
        } else {
            self.strict.clone()
        }
    }
}

pub type SharedClients = Arc<RwLock<Arc<ClientBundle>>>;

#[cfg(test)]
mod tests;
