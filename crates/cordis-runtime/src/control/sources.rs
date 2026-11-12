//! DOC-11 §10、ADR-0031：代码来源确认记录与实例配置分开持久化。

use super::*;
use cordis_supervisor::manager::Snapshot;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize, PartialEq)]
struct Source {
    declared_source: String,
    installed_reference: String,
    plugin_directory: PathBuf,
}

pub(crate) struct SourceBindings {
    path: PathBuf,
    directory: PathBuf,
    confirmed: BTreeMap<String, Source>,
    tracked: BTreeMap<String, Source>,
    dirty: bool,
}

impl SourceBindings {
    /// DOC-11 §10：每个状态目录保存实例代码来源，插件目录作为声明身份的一部分。
    pub(super) fn new(path: PathBuf, directory: PathBuf) -> Self {
        Self {
            path,
            directory,
            confirmed: BTreeMap::new(),
            tracked: BTreeMap::new(),
            dirty: false,
        }
    }

    /// DOC-11 §10：在发现可变基础来源前读取已确认副本，首次部署使用空记录。
    pub(super) fn load(&mut self) -> Result<()> {
        match std::fs::metadata(&self.path) {
            Ok(_) => {
                self.confirmed =
                    serde_json::from_slice(&crate::storage::read(&self.path)?).map_err(parameter)?
            }
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {}
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
    pub(super) fn track(
        &mut self,
        configuration: &Configuration,
        overlay: &Overlay,
        mounted: &BTreeMap<String, Mounted>,
    ) {
        let entries = configuration.entries(overlay);
        self.tracked = mounted
            .iter()
            .map(|(name, mounted)| {
                (
                    name.clone(),
                    Source {
                        declared_source: configuration
                            .instance
                            .get(name)
                            .map(|base| &base.plugin)
                            .unwrap_or(&mounted.settings.plugin)
                            .clone(),
                        installed_reference: mounted.package.reference.clone(),
                        plugin_directory: self.directory.clone(),
                    },
                )
            })
            .collect();
        let previous = self.confirmed.len();
        self.confirmed.retain(|name, _| entries.contains_key(name));
        self.dirty |= self.confirmed.len() != previous;
    }

    /// DOC-11 §10、CP-11：真实 Active 状态确认代码引用，多项变化共用一次原子写入。
    pub(crate) fn confirm(&mut self, snapshot: &Snapshot) -> Result<()> {
        for instance in &snapshot.instances {
            if instance.state != "active" {
                continue;
            }
            let Some(source) = self.tracked.get(&instance.name) else {
                continue;
            };
            if self.confirmed.get(&instance.name) != Some(source) {
                self.confirmed.insert(instance.name.clone(), source.clone());
                self.dirty = true;
            }
        }
        if self.dirty {
            let bytes = serde_json::to_vec_pretty(&self.confirmed).map_err(parameter)?;
            if bytes.len() > cordis_up::Limits::DEFAULT.max_frame {
                return Err(WireError::new(8, "代码来源记录超过状态文件容量"));
            }
            crate::storage::write(&self.path, &bytes)?;
            self.dirty = false;
        }
        Ok(())
    }
}
