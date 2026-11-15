//! ADR-0053：真实宿主的描述、换库、故障恢复与并发回收共用文件租约。

use super::*;
use cordis_process::{
    Peer,
    cache::{CacheDirectory, CachePin},
};
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) async fn manager() -> Result<Manager> {
    Ok(Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            confirm: Duration::from_millis(10),
            retired_image_limit: 100,
            ..Default::default()
        },
    )
    .await?)
}

pub(super) fn copies(directory: &Path) -> Result<Vec<PathBuf>> {
    Ok(fs::read_dir(directory)?
        .map(|item| item.map(|item| item.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("dylib" | "so")
            )
        })
        .collect())
}

async fn mount(manager: &Manager, group: &str) -> Result<Fiber> {
    let fiber = manager
        .mount(
            InstanceSpec {
                name: "store".into(),
                group: group.into(),
                plugin: "loader-fixture".into(),
                component: "provider".into(),
                config: Value::Map(Vec::new()),
            },
            None,
        )
        .await?;
    fiber.settled().await?;
    Ok(fiber)
}

pub(super) async fn bounded(artifacts: &Artifacts) -> Result<()> {
    let directory = artifacts.work.join("describe-cache");
    for _ in 0..8 {
        cordis_supervisor::run_capture(
            &[
                artifacts.host.display().to_string(),
                "--describe".into(),
                artifacts.provider.display().to_string(),
                "--cache-dir".into(),
                directory.display().to_string(),
            ],
            &artifacts.work,
            Duration::from_secs(5),
            1024 * 1024,
        )
        .await?;
        check(copies(&directory)?.is_empty(), "描述结束应回收本次副本")?;
    }
    let work = artifacts.work.join("bounded-cache");
    let specification = group("bounded", &work, artifacts, &[&artifacts.provider])?;
    for _ in 0..4 {
        let manager = manager().await?;
        let result = async {
            manager.attach_group(specification.clone()).await?;
            mount(&manager, "bounded").await?;
            manager
                .invoke_service("loader.counter@1.0", "add", vec![37u64.into()])
                .await?;
            for _ in 0..4 {
                manager
                    .replace_library(
                        "bounded".into(),
                        "loader-fixture".into(),
                        artifacts.version2.clone(),
                    )
                    .await?;
                let value = manager
                    .invoke_service("loader.counter@1.0", "value", Vec::new())
                    .await?;
                check(value == Value::Integer(137u64.into()), "换库保持137")?;
                check(
                    copies(&work.join("lib"))?.len() == 1,
                    "静止的旧副本已释放，活动版本保留一份",
                )?;
            }
            Ok::<_, Box<dyn Error>>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result?;
        cleanup?;
        check(
            copies(&work.join("lib"))?.is_empty(),
            "核心和宿主退出后副本归零",
        )?;
    }
    eprintln!("副本有界：8次描述均归零，4次启动各4次换库保持1份，关闭后归零");
    Ok(())
}

pub(super) async fn recovery(artifacts: &Artifacts) -> Result<()> {
    let work = artifacts.work.join("cache-recovery");
    let source = artifacts.work.join("removable-source.dylib");
    fs::copy(&artifacts.provider, &source)?;
    let mut specification = group("recovery", &work, artifacts, &[&source])?;
    specification.restart = RestartPolicy {
        max: 1,
        backoff: Duration::from_millis(500),
    };
    let manager = manager().await?;
    let result = recover_and_replace(&manager, artifacts, &work, specification, &source).await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup?;
    check(
        copies(&work.join("lib"))?.is_empty(),
        "重启来源在全部计划释放后回收",
    )?;
    Ok(())
}

async fn recover_and_replace(
    manager: &Manager,
    artifacts: &Artifacts,
    work: &Path,
    specification: GroupSpec,
    source: &Path,
) -> Result<()> {
    let snapshot = manager.attach_group(specification).await?;
    let fiber = mount(manager, "recovery").await?;
    manager
        .invoke_service("loader.counter@1.0", "add", vec![37u64.into()])
        .await?;
    manager.checkpoint().await?;
    let old = manager.library_snapshots().await?[0].path.clone();
    fs::remove_file(source)?;
    let previous = fiber.epoch();
    check(
        Command::new("kill")
            .args(["-KILL", &snapshot.pid.unwrap().to_string()])
            .status()?
            .success(),
        "终止宿主",
    )?;
    let scanner = CacheDirectory::new(&work.join("lib"))?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            scanner.prune()?;
            check(old.is_file(), "退避及恢复期间核心计划保护旧来源")?;
            if fiber.epoch() != previous
                && fiber.state() == FiberState::Active
                && fiber.is_settled()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await??;
    check(
        manager
            .invoke_service("loader.counter@1.0", "value", Vec::new())
            .await?
            == Value::Integer(137u64.into()),
        "CP-6 从缓存来源与检查点恢复137",
    )?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let scan = tokio::task::spawn_blocking(move || -> std::io::Result<usize> {
        let mut scans = 0;
        while !stop.load(Ordering::Acquire) {
            scanner.prune()?;
            scans += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(scans)
    });
    let replaced = manager
        .replace_library(
            "recovery".into(),
            "loader-fixture".into(),
            artifacts.version2.clone(),
        )
        .await;
    stopped.store(true, Ordering::Release);
    check(scan.await?? > 0, "回收扫描与原生换库实际重叠")?;
    replaced?;
    check(
        manager
            .invoke_service("loader.counter@1.0", "implementation", Vec::new())
            .await?
            == Value::Integer(2u64.into()),
        "扫描期间候选成功激活",
    )?;
    check(
        manager
            .invoke_service("loader.counter@1.0", "value", Vec::new())
            .await?
            == Value::Integer(137u64.into()),
        "扫描期间状态保持137",
    )?;
    eprintln!("缓存恢复：删除原文件后SIGKILL，扫描保留重启来源并恢复137；并发扫描与换库成功");
    Ok(())
}

pub(super) async fn orphan(artifacts: &Artifacts) -> Result<()> {
    let work = artifacts.work.join("orphan-cache");
    let spec = group("orphan", &work, artifacts, &[&artifacts.provider])?;
    let manager = manager().await?;
    let peer = Peer::launch(
        1,
        &spec.command,
        &spec.directory,
        spec.peer,
        Arc::new(manager.clone()),
        Arc::new(|_, line| eprintln!("{line}")),
    )
    .await?;
    let result = orphan_files(&peer, artifacts, &work).await;
    let stopped = peer.stop("缓存验收结束").await;
    let cleanup = manager.shutdown().await;
    result?;
    stopped?;
    cleanup?;
    Ok(())
}

/// 独立进程持锁三秒，父测试的三百毫秒截止应先完成。
pub(super) fn hold_lock(directory: &Path) -> Result<()> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".sequence.lock"))?;
    file.lock()?;
    fs::write(directory.join("lock-ready"), b"locked")?;
    std::thread::sleep(Duration::from_secs(3));
    Ok(())
}

struct Locker(std::process::Child);
impl Drop for Locker {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|exit| exit.is_none()) {
            if let Err(error) = self.0.kill() {
                eprintln!("持锁测试进程终止失败：{error}");
            }
        }
        if let Err(error) = self.0.wait() {
            eprintln!("持锁测试进程回收失败：{error}");
        }
    }
}

pub(super) async fn bounded_lock(artifacts: &Artifacts) -> Result<()> {
    let work = artifacts.work.join("cache-lock");
    let manager = manager().await?;
    let result = async {
        manager
            .attach_group(group("locked", &work, artifacts, &[&artifacts.provider])?)
            .await?;
        mount(&manager, "locked").await?;
        let path = manager.library_snapshots().await?[0].path.clone();
        let _locker = Locker(
            Command::new(std::env::current_exe()?)
                .arg("--hold-cache-lock")
                .arg(work.join("lib"))
                .spawn()?,
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while !work.join("lib/lock-ready").is_file() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let started = Instant::now();
        let (first, second, healthy) = tokio::join!(
            cordis_process::cache::pin_sources(vec![path.clone()], Duration::from_millis(300)),
            cordis_process::cache::pin_sources(vec![path], Duration::from_millis(300)),
            manager.invoke_service("loader.counter@1.0", "value", Vec::new()),
        );
        for result in [first, second] {
            check(
                result.is_err_and(|error| error.kind() == std::io::ErrorKind::TimedOut),
                "文件租约等待按截止时间结束",
            )?;
        }
        check(
            started.elapsed() < Duration::from_secs(1),
            "两个阻塞文件任务沿三百毫秒截止返回",
        )?;
        check(
            healthy? == Value::Integer(100u64.into()),
            "文件锁等待期间独立服务仍可应答",
        )?;
        let replaced = manager
            .replace_library(
                "locked".into(),
                "loader-fixture".into(),
                artifacts.version2.clone(),
            )
            .await;
        check(
            matches!(replaced, Err(WireError { code: 6, .. })),
            "核心收到缓存锁截止错误code6",
        )?;
        eprintln!("文件锁截止：两个来源任务300ms返回超时，核心保持服务，换库返回code6");
        Ok::<_, Box<dyn Error>>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup?;
    Ok(())
}

struct Immutable(PathBuf);
impl Drop for Immutable {
    fn drop(&mut self) {
        match Command::new("chflags").arg("nouchg").arg(&self.0).status() {
            Ok(status) if status.success() => {}
            other => eprintln!("测试文件属性回收失败：{other:?}"),
        }
    }
}

pub(super) async fn cleanup_error(artifacts: &Artifacts) -> Result<()> {
    let directory = artifacts.work.join("cleanup-error");
    let cache = CacheDirectory::new(&directory)?;
    let (_, copy, mut output) = cache.reserve(std::env::consts::DLL_EXTENSION)?;
    std::io::copy(&mut fs::File::open(&artifacts.provider)?, &mut output)?;
    drop(output);
    let path = copy.path().to_owned();
    check(
        Command::new("chflags")
            .arg("uchg")
            .arg(&path)
            .status()?
            .success(),
        "设置真实删除限制",
    )?;
    let guard = Immutable(path.clone());
    drop(copy);
    check(cache.prune().is_err(), "旧副本删除因文件系统权限失败")?;
    let result = cordis_supervisor::run_capture(
        &[
            artifacts.host.display().to_string(),
            "--describe".into(),
            artifacts.provider.display().to_string(),
            "--cache-dir".into(),
            directory.display().to_string(),
        ],
        &artifacts.work,
        Duration::from_secs(5),
        1024 * 1024,
    )
    .await;
    drop(guard);
    result?;
    check(
        copies(&directory)? == [path],
        "新描述成功并回收，仅保留删除受限的旧副本",
    )?;
    cache.prune()?;
    check(copies(&directory)?.is_empty(), "限制解除后旧副本可回收")?;
    eprintln!("回收失败隔离：真实不可删除旧副本保留诊断，新宿主描述成功且新副本归还");
    Ok(())
}

async fn orphan_files(peer: &Peer, artifacts: &Artifacts, work: &Path) -> Result<()> {
    let hello = Value::serialized(&payload::Hello {
        protocol: 1,
        process: 1,
        group: "orphan".into(),
    })?;
    peer.request("hello", hello, Duration::from_secs(5)).await?;
    let status: payload::LibraryStatus = peer
        .request("library_status", Value::Null, Duration::from_secs(5))
        .await?
        .value()
        .deserialized()?;
    let path = &status.libraries[0].path;
    let pin = CachePin::open(path)?.ok_or_else(|| std::io::Error::other("核心来源文件租约缺席"))?;
    let prepare = Value::serialized(&payload::LibraryPrepare {
        path: artifacts.version2.display().to_string(),
    })?;
    peer.request("library_prepare", prepare, Duration::from_secs(5))
        .await?;
    check(
        copies(&work.join("lib"))?.len() == 2,
        "宿主持有活动和准备副本",
    )?;
    check(
        Command::new("kill")
            .args(["-KILL", &peer.pid().to_string()])
            .status()?
            .success(),
        "SIGKILL 实际宿主",
    )?;
    peer.wait_exit().await?;
    CacheDirectory::new(&work.join("lib"))?.prune()?;
    check(
        copies(&work.join("lib"))? == [path.clone()],
        "无核心引用的候选已回收，来源租约仍受保护",
    )?;
    drop(pin);
    CacheDirectory::new(&work.join("lib"))?.prune()?;
    check(
        copies(&work.join("lib"))?.is_empty(),
        "最终租约归还后来源释放",
    )?;
    eprintln!("SIGKILL 释放宿主共享锁：无引用候选回收，核心引用保留到显式归还");
    Ok(())
}
