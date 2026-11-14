//! 项目规则 11：通过真实文件观察成功、错误返回和共享租约的现场寿命。

use cordis_testkit::cleanup::{SharedFixture, TestGuard};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

fn outer(label: &str) -> (TestGuard, PathBuf) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let guard = TestGuard::new(workspace, label);
    let path = guard.directory("testkit-cleanup");
    fs::create_dir_all(&path).unwrap();
    (guard, path)
}

#[test]
fn successful_case_moves_evidence_and_removes_artifacts() {
    let (outer, path) = outer("cleanup-success");
    let guard = TestGuard::new(outer.workspace(), "successful-subject");
    let subject = guard.track(path.join("subject"));
    fs::create_dir(&subject).unwrap();
    fs::write(subject.join("result.json"), b"{\"passed\":true}\n").unwrap();
    fs::write(subject.join("program.bin"), [1, 2, 3]).unwrap();
    fs::File::create(subject.join("payload.state"))
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    let evidence = guard.evidence_for(&subject);
    guard.success();
    drop(guard);
    assert!(!subject.exists(), "成功现场应释放原目录");
    assert_eq!(
        fs::read(evidence.join("result.json")).unwrap(),
        b"{\"passed\":true}\n"
    );
    assert!(
        !evidence.join("program.bin").exists(),
        "证据目录保留验收数据"
    );
    assert!(
        !evidence.join("payload.state").exists(),
        "成功现场的可生成大载荷应释放"
    );
    let pruned: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("pruned.json")).unwrap()).unwrap();
    assert!(
        pruned["removed_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| {
                file["file"] == "payload.state" && file["size_bytes"] == 64 * 1024 * 1024
            })
    );
    outer.success();
}

fn failing_operation(workspace: &Path, subject: &Path) -> io::Result<()> {
    let guard = TestGuard::new(workspace, "failed-subject");
    guard.track(subject.to_owned());
    fs::create_dir(subject)?;
    fs::write(subject.join("result.json"), b"{\"phase\":\"prepared\"}\n")?;
    fs::File::open(subject.join("absent-input"))?;
    guard.success();
    Ok(())
}

#[test]
fn error_return_preserves_the_diagnostic_case() {
    let (outer, path) = outer("cleanup-error");
    let subject = path.join("subject");
    failing_operation(outer.workspace(), &subject).expect_err("缺失输入结束实际文件操作");
    assert!(
        subject.join("result.json").is_file(),
        "错误返回保留已产生的诊断文件"
    );
    outer.success();
}

#[test]
fn failed_shared_user_preserves_artifacts_after_the_last_successful_lease() {
    let (outer, path) = outer("cleanup-shared");
    let first = TestGuard::new(outer.workspace(), "first-user");
    let last = TestGuard::new(outer.workspace(), "last-user");
    let cache = SharedFixture::new();
    let build = |guard: &TestGuard| {
        let directory = guard.track(path.join(guard.id()));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("program.bin"), [1, 2, 3]).unwrap();
        directory
    };
    let one = cache
        .get_or_init(outer.workspace(), "fixture", build)
        .for_case(&first);
    let two = cache
        .get_or_init(outer.workspace(), "fixture", build)
        .for_case(&last);
    assert_eq!(*one, *two, "同时存活的使用者取得相同夹具");
    let directory = (*one).clone();
    drop(one);
    last.success();
    drop(two);
    assert!(
        directory.join("program.bin").is_file(),
        "先前失败的使用者保留共享现场"
    );
    outer.success();
}
