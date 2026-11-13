use cordis_supervisor::{Connection, Handler, Outgoing, Payload, Peer, PeerOptions};
use cordis_up::{Value, WireError, rpc::HandlerFuture};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::{sleep, timeout};

struct Host {
    active: AtomicUsize,
    peak: AtomicUsize,
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Handler for Host {
    fn request(
        &self,
        _: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "host_add" => {
                    let args = params.value().as_array().unwrap();
                    Ok(number(integer(&args[0]) + integer(&args[1])).into())
                }
                "invoke" => {
                    let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                    self.peak.fetch_max(current, Ordering::SeqCst);
                    let _active = Active(&self.active);
                    sleep(Duration::from_secs(2)).await;
                    Ok(Value::Null.into())
                }
                _ => Err(WireError::new(3, "unknown method")),
            }
        })
    }
    fn notify(
        &self,
        _: Connection,
        method: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async move { Err(WireError::new(3, format!("unknown notification {method}"))) })
    }
}

fn plugin() -> &'static Path {
    static PLUGIN: OnceLock<PathBuf> = OnceLock::new();
    PLUGIN.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = root.parent().and_then(Path::parent).unwrap();
        let _build = cordis_testkit::build::shared_build_lock(workspace).unwrap();
        let destination = cordis_testkit::build::shared_target_dir(workspace);
        let output = std::process::Command::new(env!("CARGO"))
            .args(["build", "--manifest-path"])
            .arg(root.join("fixtures/process-plugin/Cargo.toml"))
            .arg("--target-dir")
            .arg(&destination)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        cordis_testkit::build::snapshot_artifacts(
            workspace,
            "process-fixture",
            "debug",
            &["cordis-process-fixture"],
        )
        .unwrap()
        .join("cordis-process-fixture")
    })
}

async fn launch(mode: &str, mut options: PeerOptions) -> (Peer, Arc<Host>) {
    options.rpc.request_timeout = Duration::from_secs(3);
    let host = Arc::new(Host {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    let peer = Peer::launch(
        1,
        &[plugin().display().to_string(), mode.into()],
        Path::new(env!("CARGO_MANIFEST_DIR")),
        options,
        host.clone(),
        Arc::new(|_, _| {}),
    )
    .await
    .unwrap();
    (peer, host)
}

fn number(value: u64) -> Value {
    Value::Integer(value.into())
}
fn integer(value: &Value) -> u64 {
    u64::try_from(value.as_integer().unwrap()).unwrap()
}
fn operation(method: &str, args: Vec<Value>) -> Value {
    Value::Map(vec![
        (Value::Text("method".into()), Value::Text(method.into())),
        (Value::Text("args".into()), Value::Array(args)),
    ])
}
async fn invoke(peer: &Peer, method: &str, args: Vec<Value>) -> Payload {
    peer.request("invoke", operation(method, args), Duration::from_secs(3))
        .await
        .unwrap()
}
async fn stats(peer: &Peer) -> Vec<u64> {
    peer.request("stats", Value::Null, Duration::from_secs(3))
        .await
        .unwrap()
        .value()
        .as_array()
        .unwrap()
        .iter()
        .map(integer)
        .collect()
}

// CP-3、CP-4、CP-10：双向调用、取消与引用释放都穿过真实标准输入输出。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_notifications_resources_and_orderly_exit() {
    let (peer, _) = launch("normal", PeerOptions::default()).await;
    assert_eq!(
        integer(
            invoke(&peer, "add", vec![number(3), number(8), number(13)])
                .await
                .value()
        ),
        24
    );
    assert_eq!(integer(invoke(&peer, "nested", vec![]).await.value()), 36);
    peer.notify("set", number(27)).await.unwrap();
    assert_eq!(integer(invoke(&peer, "value", vec![]).await.value()), 27);

    let result = invoke(&peer, "resource", vec![]).await;
    let id = result
        .value()
        .as_map()
        .unwrap()
        .iter()
        .find_map(|(key, value)| (key.as_text() == Some("object")).then(|| integer(value)))
        .unwrap();
    let object = result.import(id).unwrap();
    drop(result);
    assert_eq!(stats(&peer).await[2], 1);
    drop(object);
    assert_eq!(stats(&peer).await[2], 0);
    drop(invoke(&peer, "resource", vec![]).await);
    assert_eq!(stats(&peer).await[2], 0);

    let waiting = tokio::spawn({
        let peer = peer.clone();
        async move {
            peer.request("invoke", operation("wait", vec![]), Duration::from_secs(3))
                .await
        }
    });
    timeout(Duration::from_secs(3), async {
        while stats(&peer).await[0] == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiting.abort();
    let _cancelled = waiting.await;
    timeout(Duration::from_secs(3), async {
        while stats(&peer).await[0] != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(stats(&peer).await[..2], [0, 1]);
    peer.shutdown(Duration::from_secs(3)).await.unwrap();
    assert!(!peer.is_alive());
}

// CP-2、CP-3、CP-4、CP-7：坏输入、洪泛及超时限制在各自连接内。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_frames_timeouts_and_flood_leave_other_processes_responsive() {
    let (healthy, _) = launch("normal", PeerOptions::default()).await;
    for mode in ["oversize", "malformed"] {
        let (peer, _) = launch(mode, PeerOptions::default()).await;
        timeout(Duration::from_secs(3), peer.wait_exit())
            .await
            .unwrap()
            .unwrap();
        assert!(!peer.is_alive());
        assert_eq!(
            integer(
                invoke(&healthy, "add", vec![number(2), number(5)])
                    .await
                    .value()
            ),
            7
        );
    }
    let (killed, _) = launch("normal", PeerOptions::default()).await;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(killed.pid() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    timeout(Duration::from_secs(3), killed.wait_exit())
        .await
        .unwrap()
        .unwrap();
    assert!(!killed.is_alive());
    assert_eq!(
        integer(
            invoke(&healthy, "add", vec![number(3), number(6)])
                .await
                .value()
        ),
        9
    );
    let mut limits = PeerOptions::default();
    limits.rpc.max_inflight = 4;
    limits.rpc.queue_capacity = 8;
    let (flood, host) = launch("flood", limits).await;
    timeout(Duration::from_secs(3), async {
        while host.active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        integer(
            invoke(&healthy, "add", vec![number(19), number(4)])
                .await
                .value()
        ),
        23
    );
    assert!(host.peak.load(Ordering::SeqCst) <= 4);
    flood.stop("flood test completed").await.unwrap();

    let (stalled, _) = launch("normal", PeerOptions::default()).await;
    for _ in 0..3 {
        let error = stalled
            .request(
                "invoke",
                operation("wait", vec![]),
                Duration::from_millis(30),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, 6);
    }
    timeout(Duration::from_secs(3), stalled.wait_exit())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        integer(
            invoke(&healthy, "add", vec![number(1), number(1)])
                .await
                .value()
        ),
        2
    );
    healthy.shutdown(Duration::from_secs(3)).await.unwrap();
}

// ADR-0027：自然退出的僵尸组判定为空，存活后代在回收组长前收到终止信号。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn natural_exit_reaps_the_group_and_its_descendants() {
    for mode in ["exit", "descendant"] {
        let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = logs.clone();
        let peer = Peer::launch(
            9,
            &[plugin().display().to_string(), mode.into()],
            Path::new(env!("CARGO_MANIFEST_DIR")),
            PeerOptions::default(),
            Arc::new(Host {
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
            }),
            Arc::new(move |_, line| captured.lock().unwrap().push(line)),
        )
        .await
        .unwrap();
        timeout(Duration::from_secs(3), peer.wait_exit())
            .await
            .unwrap()
            .unwrap();
        if mode == "descendant" {
            let pid = timeout(Duration::from_secs(3), async {
                loop {
                    if let Some(pid) = logs.lock().unwrap().iter().find_map(|line| {
                        line.strip_prefix("descendant:")
                            .and_then(|pid| pid.parse::<i32>().ok())
                    }) {
                        break pid;
                    }
                    sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            timeout(Duration::from_secs(3), async {
                while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
                    != Err(nix::errno::Errno::ESRCH)
                {
                    sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
        assert!(
            logs.lock()
                .unwrap()
                .iter()
                .all(|line| line.starts_with("descendant:")),
            "{:?}",
            logs.lock().unwrap()
        );
    }
}

// 探测命令读取完整输出，并在输出超额或超时时完成组清理后报告错误。
#[tokio::test]
async fn capture_reports_output_limits_and_deadlines() {
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = cordis_supervisor::run_capture(
        &[
            "/usr/bin/printf".into(),
            "%s".into(),
            "cordis descriptor".into(),
        ],
        cwd,
        Duration::from_secs(3),
        64,
    )
    .await
    .unwrap();
    assert_eq!(output, b"cordis descriptor");
    let error =
        cordis_supervisor::run_capture(&["/usr/bin/yes".into()], cwd, Duration::from_secs(3), 32)
            .await
            .unwrap_err();
    assert_eq!(error.code, 8, "{error}");
    let error = cordis_supervisor::run_capture(
        &["/bin/sleep".into(), "60".into()],
        cwd,
        Duration::from_millis(30),
        32,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, 6);
}
