use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn proxy() -> (
    String,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 32768 {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    bytes.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&bytes).to_string();
                let connect = request.starts_with("CONNECT ");
                let _ = tx.send(request);
                if connect {
                    // Demonstrate CONNECT negotiation without an external TLS server:
                    // close after acknowledgement and assert the requested tunnel target.
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await;
                } else {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nproxied").await;
                }
            });
        }
    });
    (url, rx, task)
}

#[test]
fn legacy_stores_default_to_system_and_invalid_manual_urls_are_rejected() {
    let store = crate::profiles::ProfileStore::with_presets();
    let mut value = serde_json::to_value(store).unwrap();
    value.as_object_mut().unwrap().remove("proxy");
    let loaded: crate::profiles::ProfileStore = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.proxy, ProxySettings::default());
    for url in [
        "",
        "127.0.0.1:7890",
        "socks5://127.0.0.1:7890",
        "https://proxy.test",
        "http://proxy.test/path",
        "http://proxy.test?x=1",
    ] {
        assert!(
            ProxySettings {
                mode: ProxyMode::Manual,
                url: url.into()
            }
            .validated()
            .is_err(),
            "{url}"
        );
    }
    assert!(ProxySettings {
        mode: ProxyMode::Manual,
        url: " http://user:password@127.0.0.1:7890 ".into()
    }
    .validated()
    .is_ok());
    for url in [
        "http://localhost/x",
        "http://LOCALHOST./",
        "http://127.0.0.2/",
        "http://[::1]/",
    ] {
        assert!(is_loopback(&Url::parse(url).unwrap()), "{url}");
    }
    assert!(!is_loopback(
        &Url::parse("http://localhost.example/").unwrap()
    ));
}

#[tokio::test]
async fn manual_proxy_routes_http_and_https_connect_and_bypasses_loopback() {
    let (url, mut seen, task) = proxy().await;
    let client = NetworkClient::build(
        &ProxySettings {
            mode: ProxyMode::Manual,
            url,
        },
        false,
        true,
    )
    .unwrap();
    let body = client
        .get("http://upstream.invalid/resource?q=1")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "proxied");
    assert!(seen
        .recv()
        .await
        .unwrap()
        .starts_with("GET http://upstream.invalid/resource?q=1 HTTP/1.1"));
    assert!(client
        .get("https://upstream.invalid/resource")
        .send()
        .await
        .is_err());
    assert!(seen
        .recv()
        .await
        .unwrap()
        .starts_with("CONNECT upstream.invalid:443 HTTP/1.1"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = format!("http://{}/", listener.local_addr().unwrap());
    let direct_task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route("/", axum::routing::get(|| async { "direct" })),
        )
        .await
        .unwrap();
    });
    assert_eq!(
        client
            .get(&local)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "direct"
    );
    let localhost = local.replace("127.0.0.1", "localhost");
    assert_eq!(
        client
            .get(&localhost)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "direct"
    );
    assert!(seen.try_recv().is_err());
    direct_task.abort();
    task.abort();
}

#[tokio::test]
async fn cross_boundary_redirects_never_send_loopback_to_manual_proxy() {
    let (url, mut seen, task) = proxy().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}/", listener.local_addr().unwrap());
    let app_task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(|| async {
                    axum::response::Redirect::temporary("http://remote.invalid/")
                }),
            ),
        )
        .await
        .unwrap();
    });
    let client = NetworkClient::build(
        &ProxySettings {
            mode: ProxyMode::Manual,
            url,
        },
        false,
        true,
    )
    .unwrap();
    assert_eq!(
        client.get(&target).send().await.unwrap().status(),
        reqwest::StatusCode::TEMPORARY_REDIRECT
    );
    assert!(seen.try_recv().is_err());
    app_task.abort();
    task.abort();
}

// Process isolation prevents environment mutation from contaminating parallel tests or
// reqwest's process-global system proxy cache. The child performs only localhost mock IO.
#[tokio::test(flavor = "multi_thread")]
async fn system_honors_environment_direct_ignores_it_and_manual_is_explicit() {
    let (url, mut seen, task) = proxy().await;
    let exe = std::env::current_exe().unwrap();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new(exe)
            .args([
                "--exact",
                "network::tests::environment_child",
                "--nocapture",
            ])
            .env("MOEFLOW_PROXY_TEST_CHILD", "1")
            .env("HTTP_PROXY", &url)
            .env("http_proxy", &url)
            .env("HTTPS_PROXY", &url)
            .env("https_proxy", &url)
            .env("ALL_PROXY", &url)
            .env("all_proxy", &url)
            .env("NO_PROXY", "bypass.invalid")
            .env("no_proxy", "bypass.invalid")
            .status()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(status.success());
    let first = seen.recv().await.unwrap();
    let second = seen.recv().await.unwrap();
    assert!(first.starts_with("GET http://system.invalid/"));
    assert!(second.starts_with("GET http://bypass.invalid/")); // manual ignores env bypass
    assert!(seen.try_recv().is_err());
    task.abort();
}

#[tokio::test]
async fn environment_child() {
    if std::env::var_os("MOEFLOW_PROXY_TEST_CHILD").is_none() {
        return;
    }
    let system = NetworkClient::build(&ProxySettings::default(), false, true).unwrap();
    assert_eq!(
        system
            .get("http://system.invalid/")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "proxied"
    );
    assert!(system.get("http://bypass.invalid/").send().await.is_err());
    let direct = NetworkClient::build(
        &ProxySettings {
            mode: ProxyMode::Direct,
            url: String::new(),
        },
        false,
        true,
    )
    .unwrap();
    assert!(direct.get("http://system.invalid/").send().await.is_err());
    let manual = NetworkClient::build(
        &ProxySettings {
            mode: ProxyMode::Manual,
            url: std::env::var("HTTP_PROXY").unwrap(),
        },
        false,
        true,
    )
    .unwrap();
    assert_eq!(
        manual
            .get("http://bypass.invalid/")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "proxied"
    );
}
