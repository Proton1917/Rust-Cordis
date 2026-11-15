//! DOC-11 SW-1：两个真实宿主共用副本目录，序号跨进程递增。

use super::*;

pub(super) async fn run(artifacts: &Artifacts) -> Result<()> {
    let directory = artifacts.work.join("shared-library-directory");
    std::fs::create_dir(&directory)?;
    let old = directory.join(format!(
        "loader-fixture-42-7.{}",
        std::env::consts::DLL_EXTENSION
    ));
    std::fs::copy(&artifacts.provider, old)?;
    let first = native_cache::manager().await?;
    let second = native_cache::manager().await?;
    let specification = |name: &str| GroupSpec {
        name: name.into(),
        command: vec![
            artifacts.host.display().to_string(),
            "--library".into(),
            artifacts.provider.display().to_string(),
            "--cache-dir".into(),
            directory.display().to_string(),
        ],
        directory: artifacts.work.clone(),
        peer: PeerOptions::default(),
        restart: RestartPolicy {
            max: 0,
            backoff: Duration::ZERO,
        },
    };
    let (a, b) = tokio::join!(
        first.attach_group(specification("first")),
        second.attach_group(specification("second"))
    );
    let result = async {
        a?;
        b?;
        let mut libraries = first.library_snapshots().await?;
        libraries.extend(second.library_snapshots().await?);
        libraries.sort_by_key(|library| library.sequence);
        check(
            libraries
                .iter()
                .map(|library| library.sequence)
                .collect::<Vec<_>>()
                == [8, 9],
            "共享目录分配实际序号8、9",
        )?;
        cordis_process::cache::CacheDirectory::new(&directory)?.prune()?;
        check(
            libraries.iter().all(|library| library.path.is_file()),
            "并行宿主及核心计划保护活动副本",
        )
    }
    .await;
    let (a, b) = tokio::join!(first.shutdown(), second.shutdown());
    result?;
    a?;
    b?;
    check(
        native_cache::copies(&directory)?.len() == 1,
        "活动租约归还后保留历史副本",
    )?;
    eprintln!("目录序号通过：两个宿主取得8、9，扫描保护活动副本，退出后归还；历史副本7保留");
    Ok(())
}
