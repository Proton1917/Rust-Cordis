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
    pub max_interrupted_retries: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            paths: vec!["target/debug".into()],
            configuration: "cordis.toml".into(),
            debounce_ms: 300,
            max_interrupted_retries: 1,
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
    retries: u32,
}

struct BatchPlan {
    replacements: Vec<(String, String)>,
    reload: bool,
    snapshot: RuntimeSnapshot,
    sources: BTreeSet<PathBuf>,
}

#[derive(Serialize, Deserialize)]
enum Rejected {
    Copy(PathBuf),
    Content(Image),
}

impl Rejected {
    fn matches(&self, candidate: &Image) -> Result<bool> {
        match self {
            Self::Copy(path) => Ok(image(path)? == *candidate),
            Self::Content(saved) => Ok(saved == candidate),
        }
    }
}

fn problem(error: impl std::fmt::Display) -> Error {
    Error::new(1, error.to_string())
}

fn image(path: &Path) -> Result<Image> {
    fn collect(root: &Path, relative: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        let path = if relative.as_os_str().is_empty() {
            root.to_owned()
        } else {
            root.join(relative)
        };
        let kind = fs::symlink_metadata(&path).map_err(problem)?.file_type();
        if kind.is_file() {
            files.insert(relative.to_owned(), fs::read(path).map_err(problem)?);
        } else if kind.is_dir() {
            for entry in fs::read_dir(path).map_err(problem)? {
                let entry = entry.map_err(problem)?;
                collect(root, &relative.join(entry.file_name()), files)?;
            }
        } else {
            return Err(Error::new(2, "监视来源需要普通文件或目录"));
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    collect(path, Path::new(""), &mut files)?;
    Ok(Image(files))
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

/// DOC-11 §7：优先引用宿主保留的被拒库，配置与描述符前失败保存实际候选内容。
fn rejected(snapshot: &RuntimeSnapshot, candidate: Image) -> Result<Rejected> {
    for library in snapshot
        .libraries
        .iter()
        .rev()
        .filter(|library| library.retired)
    {
        if image(&library.path)? == candidate {
            return Ok(Rejected::Copy(library.path.clone()));
        }
    }
    for plugin in &snapshot.available_plugins {
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
    max_interrupted_retries: u32,
    submitted: Option<String>,
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
            let mut candidates = match self.read_candidates(&paths) {
                Ok(candidates) => candidates,
                Err(error) => {
                    self.saved.pending = paths;
                    save(&self.state, &self.saved)?;
                    return Err(error);
                }
            };
            if candidates.is_empty() || self.unchanged(&candidates)? {
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
            let collected = candidates.len();
            candidates.retain(|path, _| path == &self.configuration || plan.sources.contains(path));
            if candidates.len() != collected && self.unchanged(&candidates)? {
                return Ok(());
            }
            let candidates = candidates
                .into_iter()
                .map(|(path, candidate)| Ok((path, rejected(&plan.snapshot, candidate)?)))
                .collect::<Result<_>>()?;
            self.saved.awaiting = Some(PendingBatch {
                id: uuid::Uuid::new_v4().to_string(),
                replacements: plan.replacements,
                reload: plan.reload,
                candidates,
                retries: 0,
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
        let pending = self.saved.awaiting.as_ref().expect("pending window");
        let id = pending.id.clone();
        let replacements = pending.replacements.clone();
        let reload = pending.reload;
        self.submitted = Some(id.clone());
        match self
            .manage
            .replace_batch(replacements, reload, id.clone())
            .await
        {
            Ok(ticket) if ticket.id == id => Ok(()),
            Ok(_) => Err(Error::new(2, "批次受理票据与请求标识不符")),
            Err(error) => {
                let snapshot = self.runtime.snapshot()?;
                if snapshot.last_batch.iter().any(|outcome| outcome.id == id)
                    || matches!(error.code(), 4 | 6 | 7)
                {
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
        let outcome = self
            .runtime
            .snapshot()?
            .last_batch
            .into_iter()
            .find(|outcome| outcome.id == id);
        let Some(outcome) = outcome else {
            if self.submitted.as_deref() != Some(&id) {
                self.send_pending().await?;
            }
            return Ok(());
        };
        match outcome.state.as_str() {
            "pending" => Ok(()),
            "succeeded" => {
                self.completed()?;
                self.context.log(2, "watch批次已完成")
            }
            "rejected" => {
                self.reject_pending()?;
                self.context.log(
                    0,
                    &format!("watch批次被拒绝：{}", outcome.error.unwrap_or_default()),
                )
            }
            "interrupted" => self.retry_interrupted().await,
            state => Err(Error::new(3, format!("批次状态 {state} 超出接口约定"))),
        }
    }

    /// ADR-0031、CP-6：中断后的自动重提按配置限次，到限后等待新的文件变化。
    async fn retry_interrupted(&mut self) -> Result<()> {
        let pending = self.saved.awaiting.as_mut().expect("pending window");
        if pending.retries >= self.max_interrupted_retries {
            let pending = self.saved.awaiting.take().expect("pending window");
            self.saved.pending.extend(pending.candidates.into_keys());
            self.submitted = None;
            save(&self.state, &self.saved)?;
            return self
                .context
                .log(1, "watch中断重提次数已到上限，等待文件变化");
        }
        pending.retries += 1;
        save(&self.state, &self.saved)?;
        self.send_pending().await
    }

    fn reject_pending(&mut self) -> Result<()> {
        let pending = self.saved.awaiting.take().expect("pending window");
        self.saved.rejected = pending.candidates;
        self.saved.pending.clear();
        self.submitted = None;
        save(&self.state, &self.saved)
    }

    fn completed(&mut self) -> Result<()> {
        if self.saved.awaiting.is_some()
            || !self.saved.rejected.is_empty()
            || !self.saved.pending.is_empty()
        {
            self.saved = Saved::default();
            self.submitted = None;
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

    /// DOC-11 §7、ADR-0031：比较整批来源集合和完整内容，重复通知保持当前部署。
    fn unchanged(&self, candidates: &BTreeMap<PathBuf, Image>) -> Result<bool> {
        if candidates.len() != self.saved.rejected.len() {
            return Ok(false);
        }
        for (path, candidate) in candidates {
            let Some(previous) = self.saved.rejected.get(path) else {
                return Ok(false);
            };
            if !previous.matches(candidate)? {
                return Ok(false);
            }
        }
        Ok(true)
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
        let mut sources = BTreeSet::new();
        for (source, reference) in references {
            let plugin = snapshot
                .available_plugins
                .iter()
                .find(|plugin| plugin.reference == reference)
                .ok_or_else(|| Error::new(4, "安装引用在运行时目录中缺席"))?;
            for instance in &snapshot.instances {
                if instance.plugin != plugin.name
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
                sources.insert(source.clone());
            }
        }
        let reload = candidates.contains_key(&self.configuration);
        if targets.is_empty() && !reload {
            return Ok(None);
        }
        Ok(Some(BatchPlan {
            replacements: targets.into_values().collect(),
            reload,
            snapshot,
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
            .with::<dyn Runtime>()
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
                max_interrupted_retries: config.max_interrupted_retries,
                submitted: None,
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
