//! DOC-11 §10、ADR-0033：解析部署配置并展开逻辑组，运行目标按实例身份增量调和。

mod scopes;

use cordis_up::{Limits, WireError};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, WireError>;
pub const MANAGE: &str = "cordis.manage@1";
pub const METHODS: &[&str] = &[
    "install",
    "add",
    "remove",
    "replace",
    "replace_batch",
    "rollback",
    "configure",
    "reload",
    "shutdown",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    #[serde(default = "any_version")]
    pub requires_core: semver::VersionReq,
    #[serde(default)]
    pub runtime: RuntimeSettings,
    #[serde(default)]
    pub instance: BTreeMap<String, InstanceSettings>,
    #[serde(default)]
    pub scope: BTreeMap<String, ScopeSettings>,
    #[serde(default)]
    pub pack: toml::Table,
}

fn any_version() -> semver::VersionReq {
    semver::VersionReq::STAR
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeSettings {
    pub plugin_dir: PathBuf,
    pub state_dir: PathBuf,
    pub require_verified: bool,
    pub swap_timeout_ms: u64,
    pub confirm_ms: u64,
    pub export_timeout_ms: u64,
    pub apply_timeout_ms: u64,
    pub cleanup_timeout_ms: u64,
    pub invoke_timeout_ms: u64,
    pub checkpoint_ms: u64,
    pub restart_max: u32,
    pub restart_backoff_ms: u64,
    pub retired_image_limit: usize,
    pub unload: Unload,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Unload {
    #[default]
    Never,
    WhenUnleased,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            plugin_dir: "plugins".into(),
            state_dir: "state".into(),
            require_verified: false,
            swap_timeout_ms: 30_000,
            confirm_ms: 10_000,
            export_timeout_ms: 5_000,
            apply_timeout_ms: 30_000,
            cleanup_timeout_ms: 30_000,
            invoke_timeout_ms: 30_000,
            checkpoint_ms: 60_000,
            restart_max: 5,
            restart_backoff_ms: 500,
            retired_image_limit: 20,
            unload: Unload::Never,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstanceSettings {
    pub plugin: String,
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub config: toml::Table,
    #[serde(default)]
    pub isolate: BTreeMap<String, Isolation>,
    #[serde(default)]
    pub intercept: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub grants: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub forwarder: bool,
}

/// UPSTREAM-EXT EntryGroup、ADR-0033：逻辑分组传递禁用状态、隔离域与拦截元数据。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ScopeSettings {
    pub parent: Option<String>,
    pub disabled: bool,
    pub isolate: BTreeMap<String, Isolation>,
    pub intercept: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    #[serde(default)]
    pub instance: BTreeMap<String, OverlayEntry>,
    #[serde(default)]
    pub removed: BTreeSet<String>,
    #[serde(default)]
    pub previous: BTreeMap<String, OverlayEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayEntry {
    pub plugin: String,
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub config: toml::Table,
    #[serde(default)]
    pub isolate: BTreeMap<String, Isolation>,
    #[serde(default)]
    pub intercept: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Isolation {
    Named(String),
    Fresh,
}

impl Serialize for Isolation {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Named(name) => serializer.serialize_str(name),
            Self::Fresh => serializer.serialize_bool(true),
        }
    }
}

impl<'de> Deserialize<'de> for Isolation {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Input {
            Named(String),
            Fresh(bool),
        }
        match Input::deserialize(deserializer)? {
            Input::Named(name) => Ok(Self::Named(name)),
            Input::Fresh(true) => Ok(Self::Fresh),
            Input::Fresh(false) => Err(serde::de::Error::custom("隔离域使用名称或 true")),
        }
    }
}

impl From<&InstanceSettings> for OverlayEntry {
    fn from(instance: &InstanceSettings) -> Self {
        Self {
            plugin: instance.plugin.clone(),
            component: instance.component.clone(),
            group: instance.group.clone(),
            scope: instance.scope.clone(),
            disabled: instance.disabled,
            config: instance.config.clone(),
            isolate: instance.isolate.clone(),
            intercept: instance.intercept.clone(),
        }
    }
}

/// CP-2、ADR-0033：来源配置在进入生命周期前完成名称、授权和组结构检查。
pub fn parse(bytes: &[u8]) -> Result<Configuration> {
    let configuration: Configuration = toml::from_str(text(bytes)?).map_err(parameter)?;
    for (name, instance) in &configuration.instance {
        instance_name(name)?;
        instance.validate()?;
        scopes::validate_names(&instance.isolate)?;
    }
    configuration.validate_scopes()?;
    Ok(configuration)
}

pub fn parse_overlay(bytes: &[u8]) -> Result<Overlay> {
    let overlay: Overlay = toml::from_str(text(bytes)?).map_err(parameter)?;
    for (name, instance) in &overlay.instance {
        instance_name(name)?;
        validate_instance(
            &instance.plugin,
            instance.component.as_deref(),
            instance.group.as_deref(),
        )?;
        if overlay.removed.contains(name) {
            return Err(WireError::new(2, "同一叠加实例同时存在与删除"));
        }
    }
    for name in &overlay.removed {
        instance_name(name)?;
    }
    for (name, instance) in &overlay.previous {
        instance_name(name)?;
        validate_instance(
            &instance.plugin,
            instance.component.as_deref(),
            instance.group.as_deref(),
        )?;
    }
    Ok(overlay)
}

pub fn text(bytes: &[u8]) -> Result<&str> {
    if bytes.len() > Limits::DEFAULT.max_frame {
        return Err(WireError::new(2, "配置文件超过 16 MiB"));
    }
    std::str::from_utf8(bytes).map_err(parameter)
}

pub fn instance_name(name: &str) -> Result<()> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\0']) {
        return Err(WireError::new(2, "名称需要为单个有效的文件名组成部分"));
    }
    Ok(())
}

impl InstanceSettings {
    fn validate(&self) -> Result<()> {
        validate_instance(
            &self.plugin,
            self.component.as_deref(),
            self.group.as_deref(),
        )?;
        if let Some(methods) = self.grants.get(MANAGE) {
            for method in methods {
                if method != "*" && !METHODS.contains(&method.as_str()) {
                    return Err(WireError::new(2, format!("未知管理方法 {method}")));
                }
            }
        }
        Ok(())
    }

    pub fn allows(&self, method: &str) -> bool {
        self.grants.get(MANAGE).is_some_and(|methods| {
            methods
                .iter()
                .any(|allowed| allowed == "*" || allowed == method)
        })
    }
}

impl Configuration {
    pub fn check_version(&self, version: &semver::Version) -> Result<()> {
        if self.requires_core.matches(version) {
            Ok(())
        } else {
            Err(WireError::new(
                3,
                format!("项目需要核心 {}，当前为 {version}", self.requires_core),
            ))
        }
    }

    /// DOC-11 §10：覆盖条目沿基础授权展开组视图，禁用条目保留在配置来源。
    pub fn effective(&self, overlay: &Overlay) -> Result<BTreeMap<String, InstanceSettings>> {
        self.expand_scopes(self.entries(overlay))
    }

    /// ADR-0033：管理持久化保存成员自身的配置，组继承在生成运行目标时计算。
    pub fn entries(&self, overlay: &Overlay) -> BTreeMap<String, InstanceSettings> {
        let mut instances = self.instance.clone();
        for name in &overlay.removed {
            instances.remove(name);
        }
        for (name, entry) in &overlay.instance {
            instances.insert(
                name.clone(),
                InstanceSettings {
                    plugin: entry.plugin.clone(),
                    component: entry.component.clone(),
                    group: entry.group.clone(),
                    scope: entry.scope.clone(),
                    disabled: entry.disabled
                        || self.instance.get(name).is_some_and(|base| base.disabled),
                    config: entry.config.clone(),
                    isolate: entry.isolate.clone(),
                    intercept: entry.intercept.clone(),
                    grants: self
                        .instance
                        .get(name)
                        .map(|base| base.grants.clone())
                        .unwrap_or_default(),
                    forwarder: self.instance.get(name).is_some_and(|base| base.forwarder),
                },
            );
        }
        instances
    }
}

pub fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn validate_instance(plugin: &str, component: Option<&str>, group: Option<&str>) -> Result<()> {
    if plugin.is_empty() || component.is_some_and(str::is_empty) || group.is_some_and(str::is_empty)
    {
        return Err(WireError::new(2, "插件、组件与分组名称需要有效取值"));
    }
    Ok(())
}

pub fn parameter(problem: impl std::fmt::Display) -> WireError {
    WireError::new(2, problem.to_string())
}
