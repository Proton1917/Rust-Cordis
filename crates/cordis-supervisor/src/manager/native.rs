//! DOC-11 SW-1–SW-9、DOC-15 §8.7：原生库的候选准备、组内切换与映像阈值重启。
//!
//! 库状态和租约由宿主报告；核心在完整预检后驱动内核更新，失败恢复原定义和原状态。

use super::*;
use cordis_types::{LibraryInfo, SwapReport};
use futures_util::future::join_all;
use std::path::{Path, PathBuf};

struct NativeSwap {
    group: Arc<Group>,
    plugin: String,
    previous: Arc<GroupGeneration>,
    candidate: Arc<GroupGeneration>,
    specification: Option<GroupSpec>,
    old_library: u64,
    new_library: u64,
    transition: lifecycle::Transition,
}

impl Manager {
    /// DOC-15 CP-5：锁外查询活动宿主，查询错误写入对应分组状态。
    pub async fn library_snapshots(&self) -> Result<Vec<LibraryInfo>> {
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
                return Vec::new();
            }
            match self.library_status(&version.connection).await {
                Ok(mut status) => {
                    for library in &mut status.libraries {
                        library.group = group.name.clone();
                        library.connection = Some(version.connection.id());
                    }
                    status.libraries
                }
                Err(problem) => {
                    group.health.lock().error = Some(saved_error(&problem));
                    Vec::new()
                }
            }
        }))
        .await;
        Ok(queried.into_iter().flatten().collect())
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

    /// DOC-11 SW-1–SW-9：按当前导出状态替换单个逻辑插件，失败恢复原版本。
    pub async fn replace_library(
        &self,
        group: String,
        plugin: String,
        path: PathBuf,
    ) -> Result<SwapReport> {
        self.restore_library(group, plugin, path, HashMap::new())
            .await
    }

    /// DOC-11 §7：以保存的原快照恢复上一原生版本，失败恢复操作前版本及状态。
    pub async fn restore_library(
        &self,
        group: String,
        plugin: String,
        path: PathBuf,
        states: HashMap<String, Option<payload::State>>,
    ) -> Result<SwapReport> {
        let committed = Arc::new(AtomicBool::new(false));
        let marker = committed.clone();
        let reported_group = group.clone();
        let reported_plugin = plugin.clone();
        self.transaction(move |manager| async move {
            let started = std::time::Instant::now();
            manager.validate_restore_states(&group, Some(&plugin), &states)?;
            manager
                .swap_library(&group, &plugin, &path, states, marker, started)
                .await
        })
        .await
        .map_err(|problem| {
            replacement_failure(
                &reported_group,
                Some(&reported_plugin),
                committed.load(Ordering::Acquire),
                problem,
            )
        })
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
    ) -> Result<Option<GroupSpec>> {
        let Some(mut specification) = group.specification.lock().clone() else {
            return Ok(None);
        };
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
            command.extend([
                "--library".into(),
                library.path.to_string_lossy().into_owned(),
            ]);
        }
        specification.command = command;
        Ok(Some(specification))
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

    /// DOC-11 SW-1–SW-5：准备原生候选与原状态，前置失败退役候选。
    async fn prepare_native(
        &self,
        group_name: &str,
        plugin_name: &str,
        path: &Path,
    ) -> Result<NativeSwap> {
        self.running()?;
        let group = self.group(group_name)?;
        let previous = group.current.lock().clone();
        let old = previous
            .descriptor
            .plugins
            .iter()
            .find(|plugin| plugin.name == plugin_name)
            .ok_or_else(|| error(4, "待替换插件缺席"))?;
        let old_library = old
            .library
            .ok_or_else(|| error(3, "组内换库适用于原生插件"))?;
        let prepared = self.prepare_library(&previous.connection, path).await?;
        let result = async {
            let candidate =
                self.native_generation(group_name, plugin_name, &previous, &prepared)?;
            self.preflight(&group, &candidate).await?;
            let specification = self.restart_specification(&group, &candidate).await?;
            let instances: Vec<_> = self
                .group_instances(group_name)
                .into_iter()
                .filter(|instance| instance.data.plugin == plugin_name)
                .collect();
            let transition = self.prepare_transition(&instances).await?;
            Ok(NativeSwap {
                group,
                plugin: plugin_name.into(),
                previous: previous.clone(),
                candidate,
                specification,
                old_library,
                new_library: prepared.library,
                transition,
            })
        }
        .await;
        if result.is_err() {
            self.library_command::<payload::LibraryRetired>(
                &previous.connection,
                "library_retire",
                prepared.library,
            )
            .await?;
        }
        result
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
        *descriptor
            .plugins
            .iter_mut()
            .find(|entry| entry.name == plugin)
            .expect("selected plugin") = prepared.plugin.clone();
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

    /// DOC-11 SW-6–SW-8：切换候选后等待依赖重建，失败进入原状态回滚。
    async fn swap_library(
        &self,
        group_name: &str,
        plugin_name: &str,
        path: &Path,
        states: HashMap<String, Option<payload::State>>,
        committed: Arc<AtomicBool>,
        started: std::time::Instant,
    ) -> Result<SwapReport> {
        let swap = self.prepare_native(group_name, plugin_name, path).await?;
        self.activate_native(&swap, states).await?;
        let mut report = SwapReport {
            plugin: plugin_name.into(),
            previous_library: swap.old_library,
            candidate_library: swap.new_library,
            affected_instances: 0,
            pause_ms: 0,
            quiescence_ms: None,
            committed: false,
            succeeded: false,
            rolled_back: false,
            failure: None,
        };
        let mut confirmation = Duration::ZERO;
        let accepted = match self
            .stabilize(&swap.transition.before, &swap.transition.names)
            .await
        {
            Ok(changed) => {
                report.affected_instances = changed.len();
                report.pause_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let confirming = std::time::Instant::now();
                let result = self.confirm(&changed, &swap.previous.connection).await;
                confirmation = confirming.elapsed();
                result
            }
            Err(problem) => Err(problem),
        };
        if let Err(problem) = accepted {
            return self.rollback_native(swap, report, problem).await;
        }
        committed.store(true, Ordering::Release);
        self.commit_native(swap, report, started + confirmation)
            .await
    }

    /// DOC-11 SW-6：宿主切换成功后提交内核定义，宿主拒绝时解冻原版本。
    async fn activate_native(
        &self,
        swap: &NativeSwap,
        states: HashMap<String, Option<payload::State>>,
    ) -> Result<()> {
        swap.group.health.lock().state = "replacing".into();
        let activated = self
            .library_command::<()>(
                &swap.previous.connection,
                "library_activate",
                swap.new_library,
            )
            .await;
        if let Err(problem) = activated {
            self.thaw(&swap.transition.frozen).await?;
            swap.group.health.lock().state = "running".into();
            self.library_command::<payload::LibraryRetired>(
                &swap.previous.connection,
                "library_retire",
                swap.new_library,
            )
            .await?;
            return Err(problem);
        }
        let mut candidate_states = swap.transition.inherited.clone();
        candidate_states.extend(states);
        self.inherit(&candidate_states);
        self.activate(&swap.group, swap.candidate.clone(), Some(&swap.plugin))
    }

    /// DOC-11 §7：候选失败时恢复原定义及原状态，并记录实际回滚结果。
    async fn rollback_native(
        &self,
        swap: NativeSwap,
        mut report: SwapReport,
        problem: WireError,
    ) -> Result<SwapReport> {
        let rollback = async {
            self.library_command::<()>(
                &swap.previous.connection,
                "library_activate",
                swap.old_library,
            )
            .await?;
            self.inherit(&swap.transition.inherited);
            self.activate(&swap.group, swap.previous.clone(), Some(&swap.plugin))?;
            self.stabilize(&swap.transition.before, &swap.transition.names)
                .await
                .map(|_| ())
        }
        .await;
        report.rolled_back = rollback.is_ok();
        report.failure = Some(metadata::clipped(&problem.message));
        swap.group.health.lock().state = if rollback.is_ok() {
            "running"
        } else {
            "failed"
        }
        .into();
        swap.group.health.lock().error = Some(saved_error(&problem));
        self.record_native(&swap, &report);
        drop(swap.transition.frozen);
        self.library_command::<payload::LibraryRetired>(
            &swap.previous.connection,
            "library_retire",
            swap.new_library,
        )
        .await?;
        rollback?;
        Err(problem)
    }

    /// DOC-11 SW-9：释放冻结引用后退役原库，保存宿主报告及手动回滚所需的原状态。
    async fn commit_native(
        &self,
        mut swap: NativeSwap,
        mut report: SwapReport,
        started: std::time::Instant,
    ) -> Result<SwapReport> {
        report.committed = true;
        *swap.group.health.lock() = Health {
            state: "running".into(),
            error: None,
        };
        if let Some(specification) = &swap.specification {
            *swap.group.specification.lock() = Some(specification.clone());
        }
        self.retain_replacement_states(
            &swap.group.name,
            &swap.transition.names,
            &swap.transition.frozen,
        );
        self.record_native(&swap, &report);
        drop(std::mem::take(&mut swap.transition.frozen));
        let retired_images = self.retire_native(&swap, &mut report).await?;
        self.finish_native(swap, report, retired_images, started)
            .await
    }

    /// SW-9：释放原库的实际租约，退役错误保留已确认的版本提交事实。
    async fn retire_native(&self, swap: &NativeSwap, report: &mut SwapReport) -> Result<u64> {
        let retired = self
            .library_command::<payload::LibraryRetired>(
                &swap.previous.connection,
                "library_retire",
                swap.old_library,
            )
            .await;
        let retired = match retired {
            Ok(retired) => retired,
            Err(problem) => {
                report.failure = Some(metadata::clipped(&problem.message));
                swap.group.health.lock().error = Some(saved_error(&problem));
                self.record_native(swap, report);
                return Err(problem);
            }
        };
        report.quiescence_ms = retired.quiescence_ms;
        if retired.leases != 0 {
            report.failure = Some("旧库仍有活动租约".into());
            self.record_native(swap, report);
            return Err(error(6, "旧库仍有活动租约"));
        }
        Ok(retired.retired_images)
    }

    /// RM-3/RM-4：阈值重启属于原替换事务，统计最终稳定时刻与全部纪元变化。
    async fn finish_native(
        &self,
        mut swap: NativeSwap,
        mut report: SwapReport,
        retired_images: u64,
        started: std::time::Instant,
    ) -> Result<SwapReport> {
        let restarted = self
            .enforce_image_limit(
                &swap.group.name,
                swap.specification.take(),
                retired_images,
                started,
            )
            .await;
        match restarted {
            Ok(Some(restarted)) => {
                report.pause_ms = restarted.pause_ms;
                report.affected_instances = self
                    .epochs()
                    .iter()
                    .filter(|(name, epoch)| {
                        swap.transition
                            .before
                            .get(*name)
                            .is_some_and(|before| before != *epoch)
                    })
                    .count();
            }
            Ok(None) => {}
            Err(problem) => {
                report.succeeded = false;
                report.failure = Some(metadata::clipped(&problem.message));
                self.record_native(&swap, &report);
                return Err(problem);
            }
        }
        report.succeeded = true;
        self.record_native(&swap, &report);
        Ok(report)
    }

    /// DOC-15 §8.7：退役映像超过配置阈值时重启该组，并保留本次替换的原版本快照。
    pub(super) async fn enforce_image_limit(
        &self,
        group: &str,
        specification: Option<GroupSpec>,
        retired_images: u64,
        started: std::time::Instant,
    ) -> Result<Option<ReplaceReport>> {
        if retired_images > self.0.options.retired_image_limit
            && let Some(specification) = specification
        {
            let original = self.last_replacement_states(group);
            let restarted = self
                .replace_with_states(specification, HashMap::new(), None, started)
                .await;
            self.0
                .catalog
                .lock()
                .replacement_states
                .insert(group.into(), original);
            return restarted.map(Some);
        }
        Ok(None)
    }

    /// DOC-10 RM-3/RM-4：阈值重启完成后保存整次事务的实际影响数和 Active 时间。
    fn record_native(&self, swap: &NativeSwap, report: &SwapReport) {
        self.0.catalog.lock().native_reports.insert(
            (swap.group.name.clone(), swap.plugin.clone()),
            (swap.previous.connection.id(), report.clone()),
        );
    }
}
