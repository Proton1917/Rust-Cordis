//! 项目规则 11：测试现场由作用域拥有，成功迁移证据后清理，失败保留诊断材料。

use std::{
    ffi::OsStr,
    fs, io,
    ops::Deref,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

struct Outcome {
    success: AtomicBool,
    preserve: AtomicBool,
}

/// 在测试入口先创建，确保后声明的进程、文件和夹具租约先释放。
pub struct TestGuard {
    workspace: PathBuf,
    id: String,
    evidence: PathBuf,
    paths: Mutex<Vec<PathBuf>>,
    outcome: Arc<Outcome>,
}

impl TestGuard {
    /// 本仓库 crate 位于工作区的 crates 或 plugins 目录中。
    pub fn for_package(package: &Path, label: &str) -> Self {
        Self::new(
            package
                .parent()
                .and_then(Path::parent)
                .expect("工作区 crate 布局"),
            label,
        )
    }

    /// 项目规则 11：标识使用一个完整名称段，所有现场位于当前工作区 target。
    pub fn new(workspace: &Path, label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        assert!(workspace.is_absolute(), "测试工作区需要绝对路径");
        assert!(
            matches!(
                (
                    Path::new(label).components().next(),
                    Path::new(label).components().nth(1)
                ),
                (Some(Component::Normal(_)), None)
            ),
            "测试标识需要一个完整名称段"
        );
        let id = format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            workspace: workspace.to_owned(),
            evidence: workspace.join("target/test-evidence").join(&id),
            id,
            paths: Mutex::new(Vec::new()),
            outcome: Arc::new(Outcome {
                success: AtomicBool::new(false),
                preserve: AtomicBool::new(false),
            }),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn evidence(&self) -> &Path {
        &self.evidence
    }

    /// 登记本次独占的精确目录；父目录已登记时由同一所有者统一清理。
    pub fn track(&self, path: PathBuf) -> PathBuf {
        let target = self.workspace.join("target");
        assert!(
            path.starts_with(&target)
                && path != target
                && !path.components().any(|part| part == Component::ParentDir)
                && !path.starts_with(target.join("test-evidence")),
            "测试现场需要位于本工作区 target 的独立目录：{}",
            path.display()
        );
        let mut paths = self.paths.lock().unwrap();
        if !paths.iter().any(|existing| path.starts_with(existing)) {
            paths.retain(|existing| !existing.starts_with(&path));
            paths.push(path.clone());
        }
        path
    }

    /// 创建本次唯一的现场路径；实际目录由测试业务按其启动要求建立。
    pub fn directory(&self, family: &str) -> PathBuf {
        self.track(self.workspace.join("target").join(family).join(&self.id))
    }

    pub fn evidence_for(&self, original: &Path) -> PathBuf {
        self.evidence.join(
            original
                .strip_prefix(self.workspace.join("target"))
                .expect("已登记的 target 现场"),
        )
    }

    /// 全部业务断言及进程退出完成后标记成功，清理发生在守卫析构时。
    pub fn success(&self) {
        self.outcome.success.store(true, Ordering::Release);
    }

    fn preserve(&self) {
        self.outcome.preserve.store(true, Ordering::Release);
    }

    fn clean(&self) -> io::Result<()> {
        for path in self.paths.lock().unwrap().iter() {
            if !path.try_exists()? {
                continue;
            }
            let evidence = self.evidence_for(path);
            let count = archive(path, &evidence)?;
            fs::remove_dir_all(path)?;
            if count > 0 {
                eprintln!(
                    "测试证据：{} → {}（{count} 个文件）",
                    path.display(),
                    evidence.display()
                );
            }
        }
        Ok(())
    }
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        if std::thread::panicking()
            || !self.outcome.success.load(Ordering::Acquire)
            || self.outcome.preserve.load(Ordering::Acquire)
        {
            return;
        }
        if let Err(problem) = self.clean() {
            panic!(
                "测试现场清理失败，保留路径与证据 {}：{problem}",
                self.evidence.display()
            );
        }
    }
}

/// 证据与编译产物按目录角色区分，目录遍历使用文件类型而非跟随符号链接。
fn archive(source: &Path, destination: &Path) -> io::Result<usize> {
    let mut pruned = Vec::new();
    let count = archive_entries(source, destination, source, &mut pruned)?;
    if !pruned.is_empty() {
        fs::create_dir_all(destination)?;
        fs::write(
            destination.join("pruned.json"),
            serde_json::to_vec_pretty(&serde_json::json!({ "removed_files": pruned }))?,
        )?;
    }
    Ok(count)
}

/// 项目规则 11：成功现场保留日志、结果、图片和小型数据，移除项写入清单。
fn archive_entries(
    source: &Path,
    destination: &Path,
    root: &Path,
    pruned: &mut Vec<serde_json::Value>,
) -> io::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        if kind.is_dir() {
            if artifact_directory(&entry.path()) {
                record_removed(&entry.path(), root, pruned)?;
            } else {
                count += archive_entries(&entry.path(), &destination.join(name), root, pruned)?;
            }
        } else if kind.is_file() && retain_file(&entry.path(), entry.metadata()?.len()) {
            fs::create_dir_all(destination)?;
            fs::rename(entry.path(), destination.join(name))?;
            count += 1;
        } else {
            record_removed(&entry.path(), root, pruned)?;
        }
    }
    Ok(count)
}

fn artifact_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(OsStr::to_str),
        Some(
            "debug"
                | "release"
                | "incremental"
                | "build"
                | "deps"
                | ".fingerprint"
                | "node_modules"
                | "npm-cache"
                | ".cargo-home"
                | "cargo-home"
                | "browser-profile"
                | "__pycache__"
                | ".dependencies"
                | ".git"
                | ".cordis"
                | "plugins"
                | "runtimes"
                | "provided-runtime"
                | "dist"
                | "delivered"
                | "contracts"
                | "bin"
        )
    ) || path.extension() == Some(OsStr::new("app"))
}

/// 结果、日志与截图保留；其他配置和状态数据以 1 MiB 为归档上限。
fn retain_file(path: &Path, bytes: u64) -> bool {
    let extension = path.extension().and_then(OsStr::to_str);
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    match extension {
        Some("log" | "jsonl" | "png" | "svg") => true,
        Some("json") if name.ends_with("result.json") || name.ends_with("status.json") => true,
        Some("json" | "txt" | "cbor" | "state" | "toml") => bytes <= 1024 * 1024,
        _ => false,
    }
}

/// 目录按实际文件遍历，清单记录相对名称和大小，符号链接保持自身身份。
fn record_removed(path: &Path, root: &Path, pruned: &mut Vec<serde_json::Value>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            record_removed(&entry?.path(), root, pruned)?;
        }
    } else {
        pruned.push(serde_json::json!({
            "file": path.strip_prefix(root).expect("已登记现场内的条目"),
            "size_bytes": metadata.len(),
        }));
    }
    Ok(())
}

struct Fixture<T> {
    value: T,
    guard: TestGuard,
}

/// 静态缓存仅持 Weak，最后一个实际使用者决定共享程序副本的释放时点。
pub struct SharedFixture<T> {
    cached: Mutex<Weak<Fixture<T>>>,
}

impl<T> SharedFixture<T> {
    pub const fn new() -> Self {
        Self {
            cached: Mutex::new(Weak::new()),
        }
    }

    pub fn get_or_init(
        &self,
        workspace: &Path,
        label: &str,
        build: impl FnOnce(&TestGuard) -> T,
    ) -> FixtureLease<T> {
        let mut cached = self.cached.lock().expect("共享夹具初始化已失败");
        let fixture = match cached.upgrade() {
            Some(fixture) => fixture,
            None => {
                let guard = TestGuard::new(workspace, label);
                let value = build(&guard);
                guard.success();
                let fixture = Arc::new(Fixture { value, guard });
                *cached = Arc::downgrade(&fixture);
                fixture
            }
        };
        FixtureLease {
            fixture,
            outcome: None,
        }
    }
}

impl<T> Default for SharedFixture<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FixtureLease<T> {
    fixture: Arc<Fixture<T>>,
    outcome: Option<Arc<Outcome>>,
}

impl<T> FixtureLease<T> {
    /// 将测试结果绑定到本租约，任何失败使用者均保留共享程序的诊断现场。
    pub fn for_case(mut self, case: &TestGuard) -> Self {
        self.outcome = Some(case.outcome.clone());
        self
    }
}

impl<T> Deref for FixtureLease<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.fixture.value
    }
}
impl<T: AsRef<OsStr>> AsRef<OsStr> for FixtureLease<T> {
    fn as_ref(&self) -> &OsStr {
        self.fixture.value.as_ref()
    }
}
impl<T> Drop for FixtureLease<T> {
    fn drop(&mut self) {
        if std::thread::panicking()
            || self.outcome.as_ref().is_some_and(|outcome| {
                !outcome.success.load(Ordering::Acquire) || outcome.preserve.load(Ordering::Acquire)
            })
        {
            self.fixture.guard.preserve();
        }
    }
}
