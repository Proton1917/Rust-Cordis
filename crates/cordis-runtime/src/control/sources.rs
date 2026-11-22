//! DOC-11 §10、ADR-0031：代码来源确认记录与实例配置分开持久化。

use super::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Deserialize, Serialize, PartialEq)]
struct Source {
    declared_source: String,
    installed_reference: String,
    plugin_directory: PathBuf,
}

#[derive(Serialize)]
pub(crate) struct OfflineSources {
    schema_version: u32,
    instances: BTreeMap<String, Source>,
}

/// ADR-0054：读取已确认的程序来源，按项目位置展开安装目录。
pub(crate) fn offline_sources(path: &Path, project: &Path) -> Result<OfflineSources> {
    let bytes = crate::storage::read_committed_sources(path)?;
    let mut instances: BTreeMap<String, Source> =
        serde_json::from_slice(&bytes).map_err(parameter)?;
    for source in instances.values_mut() {
        source.plugin_directory = config::resolve_path(project, &source.plugin_directory);
    }
    Ok(OfflineSources {
        schema_version: 1,
        instances,
    })
}

/// DOC-11 §10、ADR-0054：恢复和查询共用受限读取，原子发布保证整份记录一致。
fn read_confirmed(path: &Path) -> Result<BTreeMap<String, Source>> {
    serde_json::from_slice(&crate::storage::read(path)?).map_err(parameter)
}

#[derive(Clone)]
pub(crate) struct SourceBindings {
    path: PathBuf,
    directory: PathBuf,
    confirmed: BTreeMap<String, Source>,
    pending: BTreeMap<String, Source>,
    dirty: bool,
}

impl SourceBindings {
    /// DOC-11 §10：每个状态目录保存实例代码来源，插件目录作为声明身份的一部分。
    pub(super) fn new(path: PathBuf, directory: PathBuf) -> Self {
        Self {
            path,
            directory,
            confirmed: BTreeMap::new(),
            pending: BTreeMap::new(),
            dirty: false,
        }
    }

    /// DOC-11 §10：在发现可变基础来源前读取已确认副本，首次部署使用空记录。
    pub(super) fn load(&mut self) -> Result<()> {
        match std::fs::metadata(&self.path) {
            Ok(_) => self.confirmed = read_confirmed(&self.path)?,
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => self.dirty = true,
            Err(problem) => return Err(crate::storage::io_error(problem)),
        }
        Ok(())
    }

    /// DOC-11 §10：人工改变代码声明时重选该来源，其余覆盖配置字段沿原规则保留。
    pub(super) fn apply_declarations(
        &self,
        configuration: &Configuration,
        overlay: &mut Overlay,
    ) -> bool {
        let mut changed = false;
        for (name, base) in &configuration.instance {
            if self.confirmed.get(name).is_some_and(|source| {
                source.declared_source != base.plugin || source.plugin_directory != self.directory
            }) && let Some(entry) = overlay.instance.get_mut(name)
                && entry.plugin != base.plugin
            {
                entry.plugin = base.plugin.clone();
                changed = true;
            }
        }
        changed
    }

    /// DOC-11 §10：显式管理引用优先；相同基础声明恢复该实例确认过的不可变代码。
    pub(super) fn select(&self, name: &str, declared: &str, configured: &str) -> String {
        match self.confirmed.get(name) {
            Some(source)
                if source.declared_source == declared
                    && source.plugin_directory == self.directory
                    && declared == configured =>
            {
                source.installed_reference.clone()
            }
            _ => configured.into(),
        }
    }

    /// ADR-0031：跟踪目录只在部署确认后更新，候选确认期继续保留窗口前的代码来源。
    pub(super) fn track<'a>(
        &mut self,
        configuration: &Configuration,
        overlay: &Overlay,
        mounted: impl Iterator<Item = (&'a String, &'a InstanceSettings, &'a Package)>,
    ) {
        let entries = configuration.entries(overlay);
        self.pending = mounted
            .map(|(name, settings, package)| {
                (
                    name.clone(),
                    Source {
                        declared_source: configuration
                            .instance
                            .get(name)
                            .map(|base| &base.plugin)
                            .unwrap_or(&settings.plugin)
                            .clone(),
                        installed_reference: package.reference.clone(),
                        plugin_directory: self.directory.clone(),
                    },
                )
            })
            .filter(|(name, source)| self.confirmed.get(name) != Some(source))
            .collect();
        let previous = self.confirmed.len();
        self.confirmed.retain(|name, _| entries.contains_key(name));
        self.dirty |= self.confirmed.len() != previous;
    }

    /// DOC-11 §10、CP-11：真实 Active 状态确认代码引用，多项变化共用一次原子写入。
    pub(super) fn observe(&mut self, manager: &Manager) {
        self.pending.retain(|name, source| {
            if manager
                .instance_fiber(name)
                .is_ok_and(|fiber| fiber.state() == cordis::FiberState::Active)
            {
                self.confirmed.insert(name.clone(), source.clone());
                self.dirty = true;
                return false;
            }
            true
        });
    }

    fn encoded(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec_pretty(&self.confirmed).map_err(parameter)?;
        if bytes.len() > cordis_up::Limits::DEFAULT.max_frame {
            return Err(WireError::new(8, "代码来源记录超过状态文件容量"));
        }
        Ok(bytes)
    }

    pub(super) fn commit(
        &mut self,
        store: &Store,
        overlay: &Overlay,
        states: &HashMap<String, Option<payload::State>>,
    ) -> Result<()> {
        store.commit_deployment(overlay, states, &self.encoded()?)?;
        self.dirty = false;
        Ok(())
    }

    pub(super) fn confirm(&mut self, manager: &Manager) -> Result<()> {
        self.observe(manager);
        if self.dirty {
            crate::storage::write(&self.path, &self.encoded()?)?;
            self.dirty = false;
        }
        Ok(())
    }
}
