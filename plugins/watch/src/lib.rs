//! DOC-15 §3.3、DOC-11 §7：文件变化稳定后请求管理接口，拒绝内容与保留副本逐字节比较。

use cordis_sdk::{
    builtin::{Manage, Runtime, RuntimeSnapshot},
    prelude::*,
};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub paths: Vec<PathBuf>,
    pub configuration: PathBuf,
    pub debounce_ms: u64,
    /// 受理结果缺席时的重提次数，旧配置名称继续映射至该项。
    #[serde(alias = "max_interrupted_retries")]
    pub max_submission_retries: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            paths: vec!["target/debug".into()],
            configuration: "cordis.toml".into(),
            debounce_ms: 300,
            max_submission_retries: 1,
        }
    }
}

#[derive(Default)]
struct Pending {
    paths: BTreeSet<PathBuf>,
    changed: Option<Instant>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize, PartialEq)]
struct Image(BTreeMap<PathBuf, Vec<u8>>);

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    rejected: BTreeMap<PathBuf, Rejected>,
    #[serde(default)]
    pending: BTreeSet<PathBuf>,
    #[serde(default)]
    awaiting: Option<PendingBatch>,
}

#[derive(Serialize, Deserialize)]
struct PendingBatch {
    id: String,
    replacements: Vec<(String, String)>,
    reload: bool,
    candidates: BTreeMap<PathBuf, Rejected>,
}

struct BatchPlan {
    replacements: Vec<(String, String)>,
    reload: bool,
    sources: BTreeMap<PathBuf, PathBuf>,
}

#[derive(Serialize, Deserialize)]
enum Rejected {
    Copy(PathBuf),
    Content(Image),
}

impl Rejected {
    fn matches(&self, candidate: &Image) -> Result<bool> {
        match self {
            Self::Copy(path) => Ok(optional_image(path)?.is_some_and(|saved| saved == *candidate)),
            Self::Content(saved) => Ok(saved == candidate),
        }
    }
}

/// ADR-0031：比较整批来源集合和完整内容，重复通知保持当前部署。
fn unchanged(
    previous: &BTreeMap<PathBuf, Rejected>,
    candidates: &BTreeMap<PathBuf, Image>,
) -> Result<bool> {
    if candidates.len() != previous.len() {
        return Ok(false);
    }
    for (path, candidate) in candidates {
        let Some(previous) = previous.get(path) else {
            return Ok(false);
        };
        if !previous.matches(candidate)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 安装跨越异步管理调用；本批比较和拒绝记录绑定返回引用的不可变副本。
fn prepared_candidates(
    observed: BTreeMap<PathBuf, Image>,
    configuration: &Path,
    installed: &BTreeMap<PathBuf, PathBuf>,
    previous: &BTreeMap<PathBuf, Rejected>,
) -> Result<Option<BTreeMap<PathBuf, Rejected>>> {
    let mut candidates = BTreeMap::new();
    for (source, observed) in observed {
        if source == configuration {
            candidates.insert(source, observed);
        } else if let Some(copy) = installed.get(&source) {
            candidates.insert(source, image(copy)?);
        }
    }
    if unchanged(previous, &candidates)? {
        Ok(None)
    } else {
        // 已安装候选的内容已经核对，直接保留本次安装返回的不可变路径。
        // 配置仍保存观察到的字节；插件目录中的其他来源与本批判定无关。
        Ok(Some(
            candidates
                .into_iter()
                .map(|(source, candidate)| {
                    let rejected = if source == configuration {
                        Rejected::Content(candidate)
                    } else {
                        Rejected::Copy(installed[&source].clone())
                    };
                    (source, rejected)
                })
                .collect(),
        ))
    }
}

fn problem(error: impl std::fmt::Display) -> Error {
    Error::new(1, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_legacy_rejection_copy_allows_reconsideration() {
        let case = cordis_testkit::cleanup::TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "watch-legacy-copy",
        );
        let directory = case.directory("watch-legacy");
        fs::create_dir_all(&directory).unwrap();
        let source = std::env::current_exe().unwrap();
        let copy = directory.join("retired-copy");
        fs::copy(&source, &copy).unwrap();
        let rejected = Rejected::Copy(copy.clone());
        let candidate = image(&source).unwrap();
        assert!(rejected.matches(&candidate).unwrap());
        fs::remove_file(copy).unwrap();
        assert!(!rejected.matches(&candidate).unwrap());
        case.success();
    }

    #[test]
    fn persisted_candidate_tracks_the_installed_copy_across_source_changes() {
        let case = cordis_testkit::cleanup::TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "watch-installed-candidate",
        );
        let directory = case.directory("watch-installed-candidate");
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("libwatch.so");
        let installed = directory.join("installed-1.so");
        let state = directory.join("watch-rejected.cbor");
        let configuration = directory.join("cordis.toml");

        fs::write(&source, b"initial source").unwrap();
        let observed = BTreeMap::from([(source.clone(), image(&source).unwrap())]);
        // The author publishes while the management call is in flight. Install
        // captures this version, and the author can publish again before reply.
        fs::write(&source, b"installed candidate").unwrap();
        fs::copy(&source, &installed).unwrap();
        fs::write(&source, b"later source").unwrap();
        let selected = BTreeMap::from([(source.clone(), installed.clone())]);
        let candidates = prepared_candidates(observed, &configuration, &selected, &BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(matches!(&candidates[&source], Rejected::Copy(path) if path == &installed));
        assert!(
            candidates[&source]
                .matches(&image(&installed).unwrap())
                .unwrap()
        );
        assert!(
            !candidates[&source]
                .matches(&image(&source).unwrap())
                .unwrap()
        );

        let saved = Saved {
            awaiting: Some(PendingBatch {
                id: "installed-candidate".into(),
                replacements: vec![("watch".into(), "installed-1.so".into())],
                reload: false,
                candidates,
            }),
            ..Saved::default()
        };
        save(&state, &saved).unwrap();
        let mut resumed: Saved = cordis_sdk::decode(&fs::read(&state).unwrap()).unwrap();
        resumed.rejected = resumed.awaiting.take().unwrap().candidates;
        fs::copy(&installed, &source).unwrap();
        let current = image(&source).unwrap();
        let repeated = BTreeMap::from([(source, current)]);
        assert!(unchanged(&resumed.rejected, &repeated).unwrap());
        case.success();
    }

    #[test]
    fn installed_content_rechecks_rejection_with_the_same_source_count() {
        let case = cordis_testkit::cleanup::TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "watch-installed-rejection",
        );
        let directory = case.directory("watch-installed-rejection");
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("libwatch.so");
        let installed = directory.join("installed-1.so");
        fs::write(&source, b"observed before install").unwrap();
        fs::write(&installed, b"already rejected candidate").unwrap();
        let observed = BTreeMap::from([(source.clone(), image(&source).unwrap())]);
        let previous = BTreeMap::from([(source.clone(), Rejected::Copy(installed.clone()))]);
        assert!(!unchanged(&previous, &observed).unwrap());
        let selected = BTreeMap::from([(source, installed)]);
        assert!(
            prepared_candidates(
                observed,
                &directory.join("cordis.toml"),
                &selected,
                &previous,
            )
            .unwrap()
            .is_none()
        );
        case.success();
    }

    #[test]
    fn prepared_batch_reads_only_its_installed_plugins_and_preserves_config_content() {
        let case = cordis_testkit::cleanup::TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "watch-installed-selection",
        );
        let directory = case.directory("watch-installed-selection");
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("libwatch.so");
        let installed = directory.join("installed-1.so");
        let configuration = directory.join("cordis.toml");
        fs::write(&source, b"plugin content").unwrap();
        fs::copy(&source, &installed).unwrap();
        fs::write(&configuration, b"observed configuration").unwrap();
        let observed_config = image(&configuration).unwrap();
        let observed = BTreeMap::from([
            (source.clone(), image(&source).unwrap()),
            (configuration.clone(), observed_config),
        ]);
        fs::write(&configuration, b"next configuration").unwrap();
        let selected = BTreeMap::from([
            (source.clone(), installed.clone()),
            (
                directory.join("unrelated.so"),
                directory.join("missing-unrelated-installed.so"),
            ),
        ]);
        let candidates = prepared_candidates(observed, &configuration, &selected, &BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(matches!(&candidates[&source], Rejected::Copy(path) if path == &installed));
        assert!(matches!(&candidates[&configuration], Rejected::Content(_)));
        assert!(
            !candidates[&configuration]
                .matches(&image(&configuration).unwrap())
                .unwrap()
        );
        fs::write(&configuration, b"observed configuration").unwrap();
        assert!(
            candidates[&configuration]
                .matches(&image(&configuration).unwrap())
                .unwrap()
        );
        case.success();
    }

    #[test]
    fn missing_installed_candidate_preserves_the_preparation_error() {
        let case = cordis_testkit::cleanup::TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "watch-installed-missing",
        );
        let directory = case.directory("watch-installed-missing");
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("libwatch.so");
        fs::write(&source, b"author source remains available").unwrap();
        let observed = BTreeMap::from([(source.clone(), image(&source).unwrap())]);
        let selected = BTreeMap::from([(source, directory.join("missing-installed.so"))]);
        assert!(
            prepared_candidates(
                observed,
                &directory.join("cordis.toml"),
                &selected,
                &BTreeMap::new(),
            )
            .is_err()
        );
        case.success();
    }
}

fn image(path: &Path) -> Result<Image> {
    optional_image(path)?.ok_or_else(|| Error::new(1, "监视来源缺席"))
}

fn optional_image(path: &Path) -> Result<Option<Image>> {
    let kind = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_type(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(problem(error)),
    };
    fn collect(
        root: &Path,
        relative: &Path,
        kind: fs::FileType,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> Result<()> {
        let path = if relative.as_os_str().is_empty() {
            root.to_owned()
        } else {
            root.join(relative)
        };
        if kind.is_file() {
            files.insert(relative.to_owned(), fs::read(path).map_err(problem)?);
        } else if kind.is_dir() {
            for entry in fs::read_dir(path).map_err(problem)? {
                let entry = entry.map_err(problem)?;
                collect(
                    root,
                    &relative.join(entry.file_name()),
                    entry.file_type().map_err(problem)?,
                    files,
                )?;
            }
        } else {
            return Err(Error::new(2, "监视来源需要普通文件或目录"));
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    collect(path, Path::new(""), kind, &mut files)?;
    Ok(Some(Image(files)))
}

fn native(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|part| part.to_str()),
        Some("dylib" | "so")
    )
}

fn source_for(path: &Path, targets: &[PathBuf], configuration: &Path) -> Option<PathBuf> {
    if path == configuration {
        return Some(configuration.to_owned());
    }
    if managed_source(path) {
        return None;
    }
    targets.iter().find_map(|target| {
        if managed_source(target) {
            return None;
        }
        if target.join("cordis-plugin.toml").is_file() && path.starts_with(target) {
            Some(target.clone())
        } else if (path == target || path.parent() == Some(target.as_path())) && native(path) {
            Some(path.to_owned())
        } else {
            None
        }
    })
}

fn sources(targets: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let mut sources = BTreeSet::new();
    for target in targets {
        if managed_source(target) {
            continue;
        }
        if target.is_file() || target.join("cordis-plugin.toml").is_file() {
            sources.insert(target.clone());
        } else {
            for entry in fs::read_dir(target).map_err(problem)? {
                let path = entry.map_err(problem)?.path();
                if native(&path) && !managed_source(&path) {
                    sources.insert(path);
                }
            }
        }
    }
    Ok(sources)
}

/// DOC-15 §3.2、ADR-0031：管理库存使用保留前缀，文件监视处理作者的构建来源。
fn managed_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("installed-") || name.starts_with(".install-"))
}

fn save(path: &Path, saved: &Saved) -> Result<()> {
    let staged = path.with_extension("pending");
    let mut file = fs::File::create(&staged).map_err(problem)?;
    file.write_all(&cordis_sdk::encode(saved)?)
        .map_err(problem)?;
    file.sync_all().map_err(problem)?;
    drop(file);
    fs::rename(staged, path).map_err(problem)
}

/// DOC-11 §7、ADR-0053：比较依据取自已安装引用或候选内容，寿命归 watch 持有。
fn rejected(snapshot: &RuntimeSnapshot, candidate: Image) -> Result<Rejected> {
    for plugin in snapshot
        .available_plugins
        .iter()
        .filter(|plugin| managed_source(&plugin.path))
    {
        if image(&plugin.path)? == candidate {
            return Ok(Rejected::Copy(plugin.path.clone()));
        }
    }
    Ok(Rejected::Content(candidate))
}

struct Session {
    context: Context,
    manage: Service<dyn Manage>,
    runtime: Service<dyn Runtime>,
    configuration: PathBuf,
    pending: Arc<Mutex<Pending>>,
    state: PathBuf,
    saved: Saved,
    debounce: Duration,
    submissions: u64,
    max_submission_retries: u32,
}

impl Session {
    /// ADR-0031、PR-1：防抖窗口串行提交，失败保留来源并继续接收后续文件变化。
    fn run(mut self) -> cordis_sdk::BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            loop {
                if self.saved.awaiting.is_some() {
                    if let Err(error) = self.advance().await {
                        self.context
                            .log(0, &format!("watch批次结果读取失败：{error}"))?;
                    }
                    self.context.sleep(Duration::from_millis(100)).await?;
                    continue;
                }
                let (paths, failure) = {
                    let mut pending = self.pending.lock().unwrap();
                    let paths = if pending
                        .changed
                        .is_some_and(|at| at.elapsed() >= self.debounce)
                    {
                        pending.changed = None;
                        std::mem::take(&mut pending.paths)
                    } else {
                        BTreeSet::new()
                    };
                    (paths, pending.error.take())
                };
                if let Some(failure) = failure {
                    self.context.log(0, &failure)?;
                }
                if !paths.is_empty() {
                    if let Err(error) = self.process(paths).await {
                        self.context.log(0, &format!("watch批次失败：{error}"))?;
                    }
                }
                self.context
                    .sleep(self.debounce.min(Duration::from_millis(50)))
                    .await?;
            }
        })
    }

    /// ADR-0031：将上次失败来源并入新窗口，整批内容变化后再次提交。
    fn process(&mut self, mut paths: BTreeSet<PathBuf>) -> cordis_sdk::BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            paths.extend(self.saved.rejected.keys().cloned());
            paths.extend(self.saved.pending.iter().cloned());
            let candidates = match self.read_candidates(&paths) {
                Ok(candidates) => candidates,
                Err(error) => {
                    self.saved.pending = paths;
                    save(&self.state, &self.saved)?;
                    return Err(error);
                }
            };
            if candidates.is_empty() || unchanged(&self.saved.rejected, &candidates)? {
                return Ok(());
            }
            let plan = match self.prepare_request(&candidates).await {
                Ok(Some(plan)) => plan,
                Ok(None) => return self.completed(),
                Err(error) => {
                    let snapshot = self.runtime.snapshot()?;
                    self.saved.rejected = candidates
                        .into_iter()
                        .map(|(path, candidate)| Ok((path, rejected(&snapshot, candidate)?)))
                        .collect::<Result<_>>()?;
                    self.saved.pending.clear();
                    save(&self.state, &self.saved)?;
                    return Err(error);
                }
            };
            let candidates = match prepared_candidates(
                candidates,
                &self.configuration,
                &plan.sources,
                &self.saved.rejected,
            ) {
                Ok(Some(candidates)) => candidates,
                Ok(None) => return Ok(()),
                Err(error) => {
                    self.saved.pending = paths;
                    save(&self.state, &self.saved)?;
                    return Err(error);
                }
            };
            self.saved.awaiting = Some(PendingBatch {
                id: uuid::Uuid::new_v4().to_string(),
                replacements: plan.replacements,
                reload: plan.reload,
                candidates,
            });
            if let Err(error) = save(&self.state, &self.saved) {
                self.saved.awaiting.take();
                return Err(error);
            }
            self.send_pending().await
        })
    }

    /// ADR-0031：受理票据对应已持久化窗口，传输不确定时保留记录供后续接续。
    async fn send_pending(&mut self) -> Result<()> {
        if self.submissions > u64::from(self.max_submission_retries) {
            self.reject_pending()?;
            return Err(Error::new(6, "批次受理确认重提次数已耗尽，等待来源变化"));
        }
        let pending = self.saved.awaiting.as_ref().expect("pending window");
        let id = pending.id.clone();
        let replacements = pending.replacements.clone();
        let reload = pending.reload;
        self.submissions += 1;
        match self
            .manage
            .replace_batch(replacements, reload, id.clone())
            .await
        {
            Ok(ticket) if ticket.id == id => Ok(()),
            Ok(_) => Err(Error::new(2, "批次受理票据与请求标识不符")),
            Err(error) => {
                if self.manage.result(&id)?.is_some() || matches!(error.code(), 4 | 6 | 7) {
                    return Err(error);
                }
                self.reject_pending()?;
                Err(error)
            }
        }
    }

    /// ADR-0031：实际结果决定清除、拒绝或有界重提，实例重建后沿同一请求标识接续。
    async fn advance(&mut self) -> Result<()> {
        let id = self
            .saved
            .awaiting
            .as_ref()
            .expect("pending window")
            .id
            .clone();
        let outcome = self.manage.result(&id)?;
        let Some(outcome) = outcome else {
            self.send_pending().await?;
            return Ok(());
        };
        match outcome.state.as_str() {
            "pending" => Ok(()),
            "succeeded" | "committed_with_cleanup" => {
                self.completed()?;
                match outcome.error {
                    Some(problem) => self
                        .context
                        .log(1, &format!("watch批次已提交，清理失败：{problem}")),
                    None => self.context.log(2, "watch批次已完成"),
                }
            }
            "rejected" | "restored" | "recovery_required" => {
                self.reject_pending()?;
                self.context.log(
                    0,
                    &format!("watch批次被拒绝：{}", outcome.error.unwrap_or_default()),
                )
            }
            state => Err(Error::new(3, format!("批次状态 {state} 超出接口约定"))),
        }
    }

    fn reject_pending(&mut self) -> Result<()> {
        let pending = self.saved.awaiting.take().expect("pending window");
        self.saved.rejected = pending.candidates;
        self.saved.pending.clear();
        self.submissions = 0;
        save(&self.state, &self.saved)
    }

    fn completed(&mut self) -> Result<()> {
        if self.saved.awaiting.is_some()
            || !self.saved.rejected.is_empty()
            || !self.saved.pending.is_empty()
        {
            self.saved = Saved::default();
            self.submissions = 0;
            save(&self.state, &self.saved)?;
        }
        Ok(())
    }

    /// ADR-0031：读取整批当前内容，配置缺席或来源读取失败在提交前返回。
    fn read_candidates(&self, paths: &BTreeSet<PathBuf>) -> Result<BTreeMap<PathBuf, Image>> {
        let mut candidates = BTreeMap::new();
        for path in paths {
            if path != &self.configuration && !path.exists() {
                continue;
            }
            candidates.insert(path.clone(), image(path)?);
        }
        Ok(candidates)
    }

    /// ADR-0031：安装完成后按逻辑插件与组去重，一次请求包含同窗配置变化。
    async fn prepare_request(
        &self,
        candidates: &BTreeMap<PathBuf, Image>,
    ) -> Result<Option<BatchPlan>> {
        let mut references = Vec::new();
        for source in candidates
            .keys()
            .filter(|path| *path != &self.configuration)
        {
            references.push((
                source.clone(),
                self.manage.install(&source.to_string_lossy()).await?,
            ));
        }
        let snapshot = self.runtime.snapshot()?;
        let mut targets = BTreeMap::<(String, String), (String, String)>::new();
        let mut sources = BTreeMap::new();
        for (source, reference) in references {
            let plugin = snapshot
                .available_plugins
                .iter()
                .find(|plugin| plugin.reference == reference)
                .ok_or_else(|| Error::new(4, "安装引用在运行时目录中缺席"))?;
            for instance in &snapshot.instances {
                if instance.page_of.is_some()
                    || instance.plugin != plugin.name
                    || instance.reference.as_deref() == Some(&reference)
                {
                    continue;
                }
                let key = (instance.group.clone(), instance.plugin.clone());
                if targets
                    .get(&key)
                    .is_some_and(|(_, previous)| previous != &reference)
                {
                    return Err(Error::new(2, "同一批次包含同组逻辑插件的多个不同候选"));
                }
                targets
                    .entry(key)
                    .or_insert_with(|| (instance.name.clone(), reference.clone()));
                sources.insert(source.clone(), plugin.path.clone());
            }
        }
        let reload = candidates.contains_key(&self.configuration);
        if targets.is_empty() && !reload {
            return Ok(None);
        }
        Ok(Some(BatchPlan {
            replacements: targets.into_values().collect(),
            reload,
            sources,
        }))
    }
}

/// DOC-15 §3.3、PR-6：监视器回调只登记变化，实例清理时释放系统监视资源。
fn register(
    context: &Context,
    targets: Vec<PathBuf>,
    configuration: PathBuf,
    pending: Arc<Mutex<Pending>>,
) -> Result<()> {
    let selected = targets.clone();
    let configured = configuration.clone();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |event: notify::Result<Event>| {
            let mut pending = pending.lock().unwrap();
            match event {
                Ok(event) => {
                    if matches!(event.kind, notify::EventKind::Access(_)) {
                        return;
                    }
                    for path in event.paths {
                        if let Some(source) = source_for(&path, &selected, &configured) {
                            pending.paths.insert(source);
                            pending.changed = Some(Instant::now());
                        }
                    }
                }
                Err(error) => pending.error = Some(error.to_string()),
            }
        })
        .map_err(problem)?;
    watcher
        .watch(
            configuration.parent().expect("configuration parent"),
            RecursiveMode::NonRecursive,
        )
        .map_err(problem)?;
    for target in &targets {
        let directory = if target.is_dir() {
            target.as_path()
        } else {
            target.parent().expect("source parent")
        };
        watcher
            .watch(directory, RecursiveMode::Recursive)
            .map_err(problem)?;
    }
    context.effect("file-watcher", move |scope| {
        scope.defer(move || {
            drop(watcher);
            Ok(())
        })
    })?;
    Ok(())
}

pub struct Watch;
impl Component for Watch {
    const NAME: &'static str = "watch";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new()
            .with::<dyn Manage>()
            .with_name("cordis.runtime@1.4")
            .with::<dyn cordis_sdk::builtin::DataDir>()
    }
    fn resolve(config: &Config) -> Result<()> {
        if config.debounce_ms == 0 {
            return Err(Error::new(2, "debounce_ms需要正整数"));
        }
        Ok(())
    }
    /// DOC-15 §3.3、PR-7：解析监视路径并恢复拒绝记录，资源失败沿 apply 返回。
    async fn apply(context: Context, config: Config) -> Result<()> {
        let project = std::env::current_dir()
            .map_err(problem)?
            .canonicalize()
            .map_err(problem)?;
        let data = context.data_dir()?.canonicalize().map_err(problem)?;
        let resolve = |path: &Path| -> Result<PathBuf> {
            let absolute = project.join(path).canonicalize().map_err(problem)?;
            if absolute.starts_with(&project) || absolute.starts_with(&data) {
                Ok(absolute)
            } else {
                Err(Error::new(9, "监视来源需要位于项目或实例数据目录"))
            }
        };
        let configuration = resolve(&config.configuration)?;
        let targets = config
            .paths
            .iter()
            .map(|path| resolve(path))
            .collect::<Result<Vec<_>>>()?;
        let pending = Arc::new(Mutex::new(Pending {
            paths: sources(&targets)?,
            changed: Some(Instant::now()),
            error: None,
        }));
        register(&context, targets, configuration.clone(), pending.clone())?;
        let manage = context.get::<dyn Manage>()?;
        let runtime = context.get::<dyn Runtime>()?;
        let state = data.join("watch-rejected.cbor");
        let saved = match fs::read(&state) {
            Ok(bytes) => cordis_sdk::decode(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Saved::default(),
            Err(error) => return Err(problem(error)),
        };
        context.spawn(
            Session {
                context: context.clone(),
                manage,
                runtime,
                configuration,
                pending,
                state,
                saved,
                debounce: Duration::from_millis(config.debounce_ms),
                submissions: 0,
                max_submission_retries: config.max_submission_retries,
            }
            .run(),
        )
    }
}

cordis_sdk::export_plugin!(
    name = "watch",
    version = env!("CARGO_PKG_VERSION"),
    components = [Watch]
);
