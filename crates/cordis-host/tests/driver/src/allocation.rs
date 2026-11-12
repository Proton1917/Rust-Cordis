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
    let command = vec![
        artifacts.host.to_string_lossy().into_owned(),
        "--describe".into(),
        artifacts.provider.to_string_lossy().into_owned(),
        "--cache-dir".into(),
        directory.to_string_lossy().into_owned(),
        "--unload".into(),
        "when-unleased".into(),
    ];
    let first = cordis_supervisor::run_capture(
        &command,
        &artifacts.work,
        Duration::from_secs(5),
        1024 * 1024,
    );
    let second = cordis_supervisor::run_capture(
        &command,
        &artifacts.work,
        Duration::from_secs(5),
        1024 * 1024,
    );
    let (first, second) = tokio::join!(first, second);
    first?;
    second?;
    for sequence in [8, 9] {
        let library = directory.join(format!(
            "loader-fixture-{sequence}.{}",
            std::env::consts::DLL_EXTENSION
        ));
        check(
            library.is_file(),
            format!("共享目录的实际库副本应取得序号{sequence}"),
        )?;
    }
    eprintln!("目录序号通过：两个宿主并行读取既有序号7，实际新副本依次分配8、9");
    Ok(())
}
