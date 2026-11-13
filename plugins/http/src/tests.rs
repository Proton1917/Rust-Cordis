//! ADR-0038：真实HTTP请求、激活令牌变化及监听连接释放。

use super::*;
use std::{fs, io::Read, net::TcpStream, path::Path, process::Command, time::Duration};

struct Running {
    entry: WebEntry,
    stop: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
}

impl Running {
    fn start(config: Config) -> Self {
        let (stop, stopping) = watch::channel(false);
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let (listener, router, entry) = server::prepare(
                        &config,
                        Arc::new(registry::Registry::default()),
                        None,
                        stopping.clone(),
                        None,
                    )
                    .await?;
                    ready.send(entry).unwrap();
                    server::serve(listener, router, stopping, config.shutdown_ms).await
                })
        });
        Self {
            entry: started.recv().unwrap(),
            stop,
            thread: Some(thread),
        }
    }

    fn close(&mut self) -> Result<()> {
        let _closed = self.stop.send(true);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| Error::new(5, "HTTP测试线程发生panic"))??;
        }
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("HTTP测试资源清理失败：{error}");
        }
    }
}

/// ADR-0038：HTTP协议由curl解释，测试读取其明确的状态与响应头字段。
fn request(url: &str, output: &Path, format: &str) -> String {
    let result = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--max-time",
            "5",
            "--path-as-is",
            "--output",
        ])
        .arg(output)
        .arg("--write-out")
        .arg(format)
        .arg(url)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn static_pages_bootstrap_tokens_and_connections_follow_server_lifetime() {
    let guard = cordis_testkit::cleanup::TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "http-pages",
    );
    let directory = guard.directory("http-tests");
    fs::create_dir_all(directory.join("assets")).unwrap();
    let mut first = Running::start(Config::default());
    let url = url::Url::parse(&first.entry.url).unwrap();
    assert_eq!(
        url.fragment(),
        Some(format!("cordis_token={}", first.entry.token).as_str())
    );
    let body = directory.join("default.html");
    assert_eq!(request(url.as_str(), &body, "%{http_code}"), "200");
    assert!(
        fs::read_to_string(&body)
            .unwrap()
            .contains("id=\"cordis-components\"")
    );
    assert_eq!(
        request(
            url.join("/favicon.ico").unwrap().as_str(),
            &directory.join("favicon"),
            "%{http_code}"
        ),
        "204"
    );
    let bootstrap = directory.join("session.js");
    assert_eq!(
        request(
            url.join("/_cordis/session.js").unwrap().as_str(),
            &bootstrap,
            "%{http_code}:%header{cache-control}"
        ),
        "200:no-store"
    );
    assert!(
        fs::read_to_string(bootstrap)
            .unwrap()
            .contains(&first.entry.token)
    );
    let address = url.socket_addrs(|| None).unwrap()[0];
    let mut connection = TcpStream::connect(address).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    first.close().unwrap();
    let mut received = Vec::new();
    match connection.read_to_end(&mut received) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(error) => panic!("HTTP连接清理失败：{error}"),
    }
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
    custom_assets(&directory, &first.entry.token);
    guard.success();
}

/// ADR-0038：自定义入口和资源使用同一服务，下一次激活取得新的令牌。
fn custom_assets(directory: &Path, prior_token: &str) {
    fs::write(
        directory.join("assets/start.html"),
        "<h1>实际自定义页面</h1>",
    )
    .unwrap();
    fs::write(directory.join("assets/message.txt"), "实际静态资源").unwrap();
    fs::write(directory.join("outside.txt"), "资源目录之外").unwrap();
    let mut server = Running::start(Config {
        assets: Some(directory.join("assets")),
        index: "start.html".into(),
        ..Config::default()
    });
    assert_ne!(server.entry.token, prior_token);
    let url = url::Url::parse(&server.entry.url).unwrap();
    let output = directory.join("custom.html");
    assert_eq!(request(url.as_str(), &output, "%{http_code}"), "200");
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "<h1>实际自定义页面</h1>"
    );
    assert_eq!(
        request(
            url.join("message.txt").unwrap().as_str(),
            &output,
            "%{http_code}"
        ),
        "200"
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), "实际静态资源");
    let outside = format!("{}/../outside.txt", url.origin().ascii_serialization());
    assert_eq!(request(&outside, &output, "%{http_code}"), "404");
    server.close().unwrap();
}
