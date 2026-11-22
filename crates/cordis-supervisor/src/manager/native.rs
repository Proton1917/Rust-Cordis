//! DOC-11 SW-1–SW-9、DOC-15 §8.7：原生库的候选准备、组内切换与映像阈值重启。
//!
//! 库状态和租约由宿主报告；核心在完整预检后驱动内核更新，失败恢复原定义和原状态。

use super::*;
use cordis_types::{LibraryInfo, SwapReport};
use futures_util::future::join_all;
use std::path::{Path, PathBuf};

impl Manager {
    /// DOC-15 §11.3、ADR-0041：在既有页面连接登记新的逻辑插件，保留原组件定义身份。
    pub async fn add_library(&self, group: String, path: PathBuf) -> Result<payload::Plugin> {
        self.submit(
            move |manager| async move {
                manager.running()?;
                let group = manager.group(&group)?;
                if group.specification.lock().is_some() {
                    return Err(error(3, "动态页面模块登记需要逻辑连接分组"));
                }
                let previous = group.current.lock().clone();
                let prepared = manager.prepare_library(&previous.connection, &path).await?;
                let result = async {
                    if previous
                        .descriptor
                        .plugins
                        .iter()
                        .any(|plugin| plugin.name == prepared.plugin.name)
                    {
                        return Err(error(2, "页面插件名称已经登记"));
                    }
                    let candidate = manager.native_generation(
                        &group.name,
                        &prepared.plugin.name,
                        &previous,
                        &prepared,
                    )?;
                    manager
                        .library_command::<()>(
                            &previous.connection,
                            "library_activate",
                            prepared.library,
                        )
                        .await?;
                    manager.activate(&group, candidate)?;
                    Ok(prepared.plugin.clone())
                }
                .await;
                if result.is_err() {
                    manager
                        .library_command::<payload::LibraryRetired>(
                            &previous.connection,
                            "library_retire",
                            prepared.library,
                        )
                        .await?;
                }
                result
            },
            true,
        )
        .await
    }

    /// 取出当前可达宿主报告的库。
    pub async fn library_snapshots(&self) -> Result<Vec<LibraryInfo>> {
        Ok(self.sample_libraries().await.0)
    }

    /// 每个错误归本次采样，连接事件由监管流程处理。
    pub async fn sample_libraries(&self) -> (Vec<LibraryInfo>, BTreeMap<String, String>) {
        let groups: Vec<_> = self.0.catalog.lock().groups.values().cloned().collect();
        let queried = join_all(groups.into_iter().map(|group| async move {
            let version = group.current.lock().clone();
            if !version.connection.is_alive()
                || !version
                    .descriptor
                    .plugins
                    .iter()
                    .any(|plugin| plugin.format == 1)
            {
                return Ok(Vec::new());
            }
            match self.library_status(&version.connection).await {
                Ok(mut status) => {
                    for library in &mut status.libraries {
                        library.group = group.name.clone();
                        library.connection = Some(version.connection.id());
                    }
                    Ok(status.libraries)
                }
                Err(problem) => Err((group.name.clone(), saved_error(&problem).to_string())),
            }
        }))
        .await;
        let mut libraries = Vec::new();
        let mut errors = BTreeMap::new();
        for sample in queried {
            match sample {
                Ok(group) => libraries.extend(group),
                Err((group, problem)) => {
                    errors.insert(group, problem);
                }
            }
        }
        (libraries, errors)
    }

    /// DOC-11 SW-9：返回各逻辑插件最近的实际换库报告及来源连接。
    pub fn library_swap_reports(&self) -> Vec<(u64, SwapReport)> {
        self.0
            .catalog
            .lock()
            .native_reports
            .values()
            .cloned()
            .collect()
    }

    /// 页面与原生库使用同一批次切换，报告归该连接和插件。
    pub async fn replace_library(
        &self,
        group: String,
        plugin: String,
        path: PathBuf,
    ) -> Result<SwapReport> {
        let page = self.group(&group)?.specification.lock().is_none();
        self.submit(
            move |manager| async move {
                let change = BatchReplacement::Native {
                    group: group.clone(),
                    plugin: plugin.clone(),
                    path,
                };
                manager
                    .change_running_group(&group, Some(change), None)
                    .await?;
                manager
                    .0
                    .catalog
                    .lock()
                    .native_reports
                    .get(&(group, plugin))
                    .map(|(_, report)| report.clone())
                    .ok_or_else(|| error(4, "换库报告缺席"))
            },
            page,
        )
        .await
    }

    /// DOC-15 §8.7：读取宿主的库、租约与映像状态，协议错误交给调用方处理。
    async fn library_status(&self, connection: &Connection) -> Result<payload::LibraryStatus> {
        let response = connection
            .request("library_status", Value::Null, self.0.options.invoke_timeout)
            .await?;
        decode(response.value())
    }

    /// DOC-15 §8.7：用宿主已复制的库路径准备重启命令，保留完整活动库集合。
    pub(super) async fn restart_specification(
        &self,
        group: &Group,
        candidate: &GroupGeneration,
    ) -> Result<Option<LaunchPlan>> {
        let Some(plan) = group.specification.lock().clone() else {
            return Ok(None);
        };
        self.launch_plan(plan.specification.clone(), candidate)
            .await
            .map(Some)
    }

    /// ADR-0053：仅原生描述符进入缓存来源计划，普通进程沿原命令启动。
    pub(super) async fn launch_plan(
        &self,
        mut specification: GroupSpec,
        candidate: &GroupGeneration,
    ) -> Result<LaunchPlan> {
        let mut sources = Vec::new();
        if !candidate
            .descriptor
            .plugins
            .iter()
            .any(|plugin| plugin.format == 1)
        {
            return Ok(LaunchPlan {
                specification,
                _pins: Arc::default(),
            });
        }
        let status = self.library_status(&candidate.connection).await?;
        let mut command = Vec::new();
        let mut arguments = specification.command.into_iter();
        while let Some(argument) = arguments.next() {
            if argument == "--library" {
                arguments.next();
            } else if !argument.starts_with("--library=") {
                command.push(argument);
            }
        }
        for plugin in &candidate.descriptor.plugins {
            let id = plugin
                .library
                .ok_or_else(|| error(3, "原生分组描述缺少库编号"))?;
            let library = status
                .libraries
                .iter()
                .find(|library| library.id == id)
                .ok_or_else(|| error(4, "宿主缺少活动库状态"))?;
            sources.push(library.path.clone());
            command.extend([
                "--library".into(),
                library.path.to_string_lossy().into_owned(),
            ]);
        }
        specification.command = command;
        let pins = cordis_process::cache::pin_sources(sources, self.0.options.handshake_timeout)
            .await
            .map_err(|problem| {
                error(
                    if problem.kind() == std::io::ErrorKind::TimedOut {
                        6
                    } else {
                        1
                    },
                    problem.to_string(),
                )
            })?;
        Ok(LaunchPlan {
            specification,
            _pins: Arc::new(pins),
        })
    }

    /// DOC-15 CP-4：库激活和退役共用清理截止时间，超时触发连接回收。
    pub(super) async fn library_command<T: DeserializeOwned>(
        &self,
        connection: &Connection,
        command: &str,
        library: u64,
    ) -> Result<T> {
        let response = self
            .lifecycle_request(
                connection,
                command,
                value(&payload::LibraryHandle { library })?,
                self.0.options.cleanup_timeout,
            )
            .await?;
        decode(response.value())
    }

    /// DOC-11 SW-1–SW-3：宿主复制与握手完成后返回候选编号，保持当前实例。
    pub(super) async fn prepare_library(
        &self,
        connection: &Connection,
        path: &Path,
    ) -> Result<payload::LibraryPrepared> {
        let response = self
            .lifecycle_request(
                connection,
                "library_prepare",
                value(&payload::LibraryPrepare {
                    path: path.to_string_lossy().into_owned(),
                })?,
                self.0.options.handshake_timeout,
            )
            .await?;
        decode(response.value())
    }

    /// DOC-11 SW-3–SW-4：替换逻辑插件的定义，保持同组其他定义身份。
    pub(super) fn native_generation(
        &self,
        group: &str,
        plugin: &str,
        previous: &GroupGeneration,
        prepared: &payload::LibraryPrepared,
    ) -> Result<Arc<GroupGeneration>> {
        if prepared.plugin.name != plugin || prepared.plugin.library != Some(prepared.library) {
            return Err(error(3, "候选库的逻辑名称或库编号与替换目标不符"));
        }
        let part = self.generation_from_description(
            group,
            previous.connection.clone(),
            previous.process.clone(),
            payload::Describe {
                protocol: 1,
                plugins: vec![prepared.plugin.clone()],
            },
        )?;
        let mut descriptor = previous.descriptor.as_ref().clone();
        match descriptor
            .plugins
            .iter_mut()
            .find(|entry| entry.name == plugin)
        {
            Some(entry) => *entry = prepared.plugin.clone(),
            None => descriptor.plugins.push(prepared.plugin.clone()),
        }
        let retained = self.retain_metadata(
            &previous.connection,
            metadata::descriptor_bytes(&descriptor)?,
        )?;
        let mut components = previous.components.clone();
        let mut definitions = previous.definitions.clone();
        components.retain(|(name, _), _| name != plugin);
        definitions.retain(|(name, _), _| name != plugin);
        components.extend(part.components.clone());
        definitions.extend(part.definitions.clone());
        Ok(Arc::new(GroupGeneration {
            connection: previous.connection.clone(),
            process: previous.process.clone(),
            descriptor: Arc::new(descriptor),
            components,
            definitions,
            _metadata: retained,
        }))
    }

    /// DOC-15 §8.7：退役映像超过配置阈值时重启该组，实例继承重启前导出的状态。
    pub(super) async fn enforce_image_limit(
        &self,
        specification: Option<LaunchPlan>,
        retired_images: u64,
    ) -> Result<Option<ReplaceReport>> {
        if retired_images > self.0.options.retired_image_limit
            && let Some(specification) = specification
        {
            return self
                .replace_process_group(specification.specification.clone())
                .await
                .map(Some);
        }
        Ok(None)
    }

    /// 退役返回错误时从存活宿主读取累计数，按当前程序绑定重启。
    pub(super) async fn after_failed_libraries(
        &self,
        mut problem: WireError,
        groups: impl IntoIterator<Item = String>,
    ) -> WireError {
        for name in groups {
            let Ok(group) = self.group(&name) else {
                continue;
            };
            let Some(plan) = group.specification.lock().clone() else {
                continue;
            };
            let current = group.current.lock().clone();
            if !current.connection.is_alive()
                || !current
                    .descriptor
                    .plugins
                    .iter()
                    .any(|plugin| plugin.format == 1)
            {
                continue;
            }
            let recovery = async {
                let status = self.library_status(&current.connection).await?;
                self.enforce_image_limit(Some(plan), status.retired_images)
                    .await
            }
            .await;
            if let Err(error) = recovery {
                problem.message = format!(
                    "{}；分组 {name} 的退役映像回收失败：{error}",
                    problem.message
                );
            }
        }
        problem
    }
}
