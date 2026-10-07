use super::*;
use crate::network::ProxyMode;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    state: AppState,
    root: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "moeflow-config-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut store = ProfileStore::with_presets();
        store.proxy.mode = ProxyMode::Direct;
        store.profiles[0].port = 0;
        let state = AppState::new(
            root.join("config"),
            store,
            Arc::new(MediaCache::new(root.join("cache"), 1024 * 1024)),
            0,
        )
        .unwrap();
        state.start_servers(None, root.join("web")).await.unwrap();
        Self { state, root }
    }
    fn ctx(&self, id: &str) -> Arc<Ctx> {
        self.state
            .servers
            .lock()
            .unwrap()
            .get(id)
            .unwrap()
            .ctx
            .clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn upstream(text: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/ping",
            axum::routing::get(
                move || async move { axum::Json(serde_json::json!({"value": text})) },
            ),
        );
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

async fn response(port: u16) -> serde_json::Value {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{port}/api/ping"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn async_save_updates_real_listener_and_keeps_ports_and_contexts() {
    let fixture = Fixture::new().await;
    let (first, first_task) = upstream("first").await;
    let (second, second_task) = upstream("second").await;
    let ctx = fixture.ctx("moetran");
    let mut profile = fixture.state.active().unwrap();
    let port = profile.port;
    profile.api_base = first;
    profile.port = 1; // renderer cannot change an existing origin.
    fixture.state.upsert(profile.clone()).await.unwrap();
    assert_eq!(response(port).await["value"], "first");
    profile.api_base = second;
    for _ in 0..4 {
        fixture.state.upsert(profile.clone()).await.unwrap();
    }
    assert_eq!(response(port).await["value"], "second");
    assert!(Arc::ptr_eq(&ctx, &fixture.ctx("moetran")));
    assert_eq!(fixture.state.active().unwrap().port, port);
    let mut new_profile = profile;
    new_profile.id = "second".into();
    fixture.state.upsert(new_profile).await.unwrap();
    let second_port = fixture.state.store_snapshot().get("second").unwrap().port;
    assert_ne!(second_port, port);
    assert_eq!(response(port).await["value"], "second");
    assert_eq!(response(second_port).await["value"], "second");
    fixture.state.delete("second".into()).await.unwrap();
    assert!(Arc::ptr_eq(&ctx, &fixture.ctx("moetran")));
    assert_eq!(response(port).await["value"], "second");
    let persisted = ProfileStore::load(&fixture.state.config_dir);
    assert_eq!(persisted.active_profile().unwrap().port, port);
    first_task.abort();
    second_task.abort();
}

#[tokio::test]
async fn failed_persistence_retains_store_listeners_clients_and_cache_limit() {
    let fixture = Fixture::new().await;
    let before = serde_json::to_value(fixture.state.store_snapshot()).unwrap();
    let ctx = fixture.ctx("moetran");
    let clients = fixture.state.clients.read().unwrap().clone();
    // A directory at the destination forces the atomic replace to fail on Windows and Unix.
    std::fs::remove_file(ProfileStore::path(&fixture.state.config_dir)).unwrap();
    std::fs::create_dir(ProfileStore::path(&fixture.state.config_dir)).unwrap();
    let mut profile = fixture.state.active().unwrap();
    profile.id = "must-not-publish".into();
    assert!(fixture.state.upsert(profile).await.is_err());
    assert!(fixture.state.set_limit(10).await.is_err());
    assert!(fixture
        .state
        .set_proxy(ProxySettings {
            mode: ProxyMode::Manual,
            url: "http://127.0.0.1:1".into()
        })
        .await
        .is_err());
    assert_eq!(
        serde_json::to_value(fixture.state.store_snapshot()).unwrap(),
        before
    );
    assert_eq!(fixture.state.servers.lock().unwrap().len(), 1);
    assert!(Arc::ptr_eq(&ctx, &fixture.ctx("moetran")));
    assert!(Arc::ptr_eq(
        &clients,
        &fixture.state.clients.read().unwrap()
    ));
    assert!(
        fixture
            .state
            .config_dir
            .join(format!("profiles.{}.tmp", std::process::id()))
            .exists()
            == false
    );
}

#[tokio::test]
async fn failed_bind_never_publishes_and_concurrent_mutations_do_not_lose_updates() {
    let fixture = Fixture::new().await;
    let held = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut candidate = fixture.state.store_snapshot();
    let mut blocked = candidate.profiles[0].clone();
    blocked.id = "blocked".into();
    blocked.port = held.local_addr().unwrap().port();
    candidate.profiles.push(blocked);
    let runtime = fixture.state.runtime.lock().unwrap().clone().unwrap();
    assert!(fixture.state.publish(candidate, &runtime).await.is_err());
    assert!(fixture.state.store_snapshot().get("blocked").is_none());
    let mut one = fixture.state.active().unwrap();
    one.id = "one".into();
    let mut two = one.clone();
    two.id = "two".into();
    let (a, b, c, d) = tokio::join!(
        fixture.state.upsert(one),
        fixture.state.upsert(two),
        fixture.state.set_skip(true),
        fixture.state.set_limit(123456)
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    d.unwrap();
    let store = fixture.state.store_snapshot();
    assert!(store.get("one").is_some() && store.get("two").is_some());
    assert!(store.skip_launcher);
    assert_eq!(store.cache_limit_bytes, 123456);
    assert_eq!(
        serde_json::to_value(&store).unwrap(),
        serde_json::to_value(ProfileStore::load(&fixture.state.config_dir)).unwrap()
    );
    assert_eq!(fixture.state.servers.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn obsolete_responses_cannot_restore_learned_origins_after_upstream_edit() {
    let fixture = Fixture::new().await;
    let ctx = fixture.ctx("moetran");
    let old = ctx.snapshot();
    ctx.learn_origins(&old, &["https://old.test".into()]);
    let mut changed = fixture.state.active().unwrap();
    changed.api_base = "https://new.test/api".into();
    fixture.state.upsert(changed).await.unwrap();
    ctx.learn_origins(&old, &["https://late-old.test".into()]);
    let fresh = ctx.snapshot();
    ctx.learn_origins(&fresh, &["https://fresh.test".into()]);
    let snapshot = ctx.snapshot();
    assert!(!snapshot.known_origins.contains(&"https://old.test".into()));
    assert!(!snapshot
        .known_origins
        .contains(&"https://late-old.test".into()));
    assert!(snapshot
        .known_origins
        .contains(&"https://fresh.test".into()));
    assert_eq!(snapshot.profile.api_base, "https://new.test/api");
    assert!(Arc::ptr_eq(&ctx, &fixture.ctx("moetran")));
}

async fn held_proxy(
    body: &'static str,
    hold: bool,
) -> (
    String,
    tokio::sync::oneshot::Receiver<String>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).await.unwrap() == 0 {
                return;
            }
            bytes.push(byte[0]);
        }
        started_tx.send(String::from_utf8(bytes).unwrap()).unwrap();
        if hold {
            let _ = finish_rx.await;
        }
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    (url, started_rx, finish_tx, task)
}

#[tokio::test]
async fn proxy_changes_apply_live_while_existing_requests_finish_with_old_bundle() {
    let fixture = Fixture::new().await;
    let ctx = fixture.ctx("moetran");
    let mut profile = fixture.state.active().unwrap();
    let port = profile.port;
    profile.api_base = "http://remote.invalid".into();
    fixture.state.upsert(profile).await.unwrap();
    let (first, started, finish, first_task) = held_proxy(r#"{"value":"old"}"#, true).await;
    fixture
        .state
        .set_proxy(ProxySettings {
            mode: ProxyMode::Manual,
            url: first,
        })
        .await
        .unwrap();
    let old_bundle = fixture.state.clients.read().unwrap().clone();
    let request = tokio::spawn(async move { response(port).await });
    assert!(started
        .await
        .unwrap()
        .starts_with("GET http://remote.invalid/ping HTTP/1.1"));
    let (second, started, _, second_task) = held_proxy(r#"{"value":"new"}"#, false).await;
    fixture
        .state
        .set_proxy(ProxySettings {
            mode: ProxyMode::Manual,
            url: second,
        })
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(
        &old_bundle,
        &fixture.state.clients.read().unwrap()
    ));
    assert_eq!(response(port).await["value"], "new");
    assert!(started
        .await
        .unwrap()
        .starts_with("GET http://remote.invalid/ping HTTP/1.1"));
    finish.send(()).unwrap();
    assert_eq!(request.await.unwrap()["value"], "old");
    assert!(Arc::ptr_eq(&ctx, &fixture.ctx("moetran")));
    assert_eq!(fixture.state.active().unwrap().port, port);
    fixture
        .state
        .set_proxy(ProxySettings {
            mode: ProxyMode::Direct,
            url: String::new(),
        })
        .await
        .unwrap();
    assert_eq!(fixture.state.store_snapshot().proxy.mode, ProxyMode::Direct);
    first_task.await.unwrap();
    second_task.await.unwrap();
}
