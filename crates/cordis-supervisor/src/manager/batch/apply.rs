//! DOC-11 SW-6–SW-9、ADR-0031：批次切换应用、确认、提交、回滚、退役与换代报告。
use super::*;

impl Manager {
    /// DOC-11 §7：恢复窗口前全部定义和状态后释放候选，并保留实际回滚报告。
    pub(super) async fn rollback_batch(
        &self,
        prepared: BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &[BatchInstance],
        transition: lifecycle::Transition,
        elapsed: Duration,
        problem: WireError,
    ) -> BatchFailure {
        let discarded = self.confirmation_discard(&transition);
        let restored = match self
            .apply_batch(
                &prepared,
                target,
                before,
                &BatchIndex::new(before),
                &transition.inherited,
                true,
            )
            .await
        {
            Ok(()) => self
                .stabilize_with(
                    &transition.before,
                    &transition.names,
                    transition.permit_pending,
                )
                .await
                .map(|_| ()),
            Err(problem) => Err(problem),
        };
        let restored = match restored {
            Ok(()) => self.thaw_current(&transition.frozen).await,
            Err(problem) => Err(problem),
        };
        self.record_batch_reports(
            &prepared,
            transition.names.len(),
            elapsed,
            Some(restored.is_ok()),
            Some(&problem),
            discarded.as_ref(),
        );
        for group in prepared.values() {
            *group.group.health.lock() = Health {
                state: if restored.is_ok() {
                    "running"
                } else {
                    "failed"
                }
                .into(),
                error: Some(saved_error(&problem)),
            };
        }
        if let Err(recovery) = restored {
            *self.0.recovery_batch.lock() = Some(HeldDeployment {
                groups: prepared,
                _transition: transition,
            });
            return BatchFailure::RecoveryRequired(error(
                1,
                format!("批次失败：{problem}；原状态恢复失败：{recovery}"),
            ));
        }
        for group in prepared.values().filter(|group| group.previous.is_none()) {
            self.0.catalog.lock().groups.remove(&group.group.name);
        }
        BatchFailure::Restored {
            problem: self.discard_batch(&prepared, problem).await,
            discarded,
        }
    }

    /// SW-6：宿主候选全部激活后统一发布核心定义，再挂载最终实例和配置。
    pub(super) async fn apply_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &[BatchInstance],
        target_index: &BatchIndex<'_>,
        states: &SharedStates,
        rollback: bool,
    ) -> Result<()> {
        let mut recovered = Vec::new();
        for group in prepared.values() {
            if rollback && !group.candidate.connection.is_alive() {
                let transition = self
                    .prepare_transition(
                        self.group_instances(&group.group.name)
                            .into_iter()
                            .map(|i| i.data.name.clone()),
                        Vec::new(),
                        false,
                    )
                    .await?;
                self.inherit(&transition.inherited);
                recovered.push(transition);
                continue;
            }
            for library in &group.libraries {
                let id = if rollback {
                    library.previous
                } else {
                    library.candidate
                };
                self.library_command::<()>(&group.candidate.connection, "library_activate", id)
                    .await?;
            }
        }
        self.inherit(states);
        self.remove_batch_instances(before, target_index).await?;
        for group in prepared.values() {
            let version = if rollback {
                group.previous.clone()
            } else {
                Some(group.candidate.clone())
            };
            if let Some(version) = version {
                let version = if rollback {
                    self.rollback_version(&group.group, &version).await?
                } else {
                    version
                };
                self.0
                    .catalog
                    .lock()
                    .groups
                    .insert(group.group.name.clone(), group.group.clone());
                self.activate(&group.group, version)?;
            }
        }
        self.apply_batch_instances(prepared, target, states, rollback)
            .await?;
        for transition in recovered {
            self.stabilize_with(&transition.before, &transition.names, true)
                .await?;
            self.thaw_current(&transition.frozen).await?;
        }
        Ok(())
    }

    /// S-FBR-4、ADR-0031：所有版本已发布后调和实例配置，并等待全部配置更新任务。
    async fn apply_batch_instances(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        target: &[BatchInstance],
        states: &SharedStates,
        rollback: bool,
    ) -> Result<()> {
        let mut updated = Vec::new();
        for item in target {
            let specification = &item.specification;
            let existing = self.find_instance(&specification.name);
            if let Some(instance) = existing {
                if instance.configuration()?.config.as_ref() != &specification.config {
                    updated
                        .push(instance.set_configuration(Arc::new(specification.config.clone())));
                }
            } else if item.view.fiber().is_alive() {
                let generation = if rollback {
                    self.group(&specification.group)?.current.lock().clone()
                } else {
                    self.batch_generation(prepared, &specification.group, false)?
                };
                let definition = generation
                    .definitions
                    .get(&(
                        specification.plugin.clone(),
                        specification.component.clone(),
                    ))
                    .ok_or_else(|| error(3, "批次组件定义缺席"))?
                    .clone();
                self.install_instance(
                    item.view.clone(),
                    specification.clone(),
                    definition,
                    states.get(&specification.name).cloned().flatten(),
                )?;
            }
        }
        for outcome in join_all(updated).await {
            outcome?;
        }
        Ok(())
    }

    /// T-EFF-2：先释放移除及上下文变化的实例，再允许相同名称重新挂载。
    async fn remove_batch_instances(
        &self,
        before: &[BatchInstance],
        target: &BatchIndex<'_>,
    ) -> Result<()> {
        let removed = before
            .iter()
            .filter(|old| {
                target
                    .get(&old.specification.name)
                    .is_none_or(|next| prepare::remount(old, next))
            })
            .collect::<Vec<_>>();
        let mut tasks = Vec::new();
        for item in &removed {
            let instance = self.find_instance(&item.specification.name);
            if let Some(instance) = instance {
                tasks.push(instance.fiber.dispose());
            }
        }
        let results = join_all(tasks).await;
        {
            let mut catalog = self.0.catalog.lock();
            for item in removed {
                catalog.instances.remove(&item.specification.name);
            }
        }
        for result in results {
            result.map_err(kernel_error)?;
        }
        Ok(())
    }

    /// SW-7/SW-8：所有候选已生效后统一等待依赖稳定，并同时观察各连接的确认期。
    pub(super) async fn accept_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        transition: &mut lifecycle::Transition,
        names: &HashSet<String>,
        target: &[BatchInstance],
        started: std::time::Instant,
    ) -> Result<(Duration, usize)> {
        let mut selected: BTreeSet<_> = target
            .iter()
            .filter(|item| names.contains(&item.specification.name))
            .map(|item| item.specification.name.clone())
            .collect();
        selected.extend(transition.names.iter().cloned());
        let selected = selected.into_iter().collect::<Vec<_>>();
        let changed = self
            .stabilize_with(&transition.before, &selected, transition.permit_pending)
            .await?;
        let paused = started.elapsed();
        let connections = prepared
            .values()
            .map(|group| group.candidate.connection.clone())
            .collect::<Vec<_>>();
        self.confirm_transition(transition, &changed, &connections)
            .await?;
        Ok((paused, changed.len()))
    }

    /// SW-9：保存全批提交事实，进程监管继续指向本次活动版本。
    pub(super) async fn commit_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        recovering: bool,
    ) -> Result<()> {
        for group in prepared.values() {
            self.replace_plan(&group.group, group.specification.clone())
                .await;
            {
                let mut health = group.group.health.lock();
                health.state = "running".into();
                if !recovering {
                    health.error = None;
                    group.group.restarts.store(0, Ordering::Release);
                }
            }
            if group.libraries.is_empty() {
                self.monitor(group.group.clone(), group.candidate.clone())?;
            }
        }
        Ok(())
    }

    /// SW-9、CP-7：提交后释放旧库和退出的分组，全部独立清理都得到执行。
    pub(super) async fn retire_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &BatchIndex<'_>,
        recovering: bool,
    ) -> Result<()> {
        let mut failure = None;
        for group in prepared.values() {
            let retired = if group.libraries.is_empty() {
                match &group.previous {
                    Some(previous) => self.retire(previous).await,
                    None => Ok(()),
                }
            } else {
                self.retire_batch_libraries(group).await
            };
            if !recovering
                && let Some(report) = self.0.catalog.lock().reports.get_mut(&group.group.name)
            {
                report.succeeded = retired.is_ok();
                report.failure = retired.as_ref().err().map(Failure::from);
            }
            if let Err(problem) = retired {
                failure.get_or_insert(problem);
            }
        }
        let removed = before
            .iter()
            .map(|item| item.specification.group.clone())
            .filter(|group| !target.groups.contains(group.as_str()))
            .collect::<HashSet<_>>();
        for name in removed {
            if !self.group_instances(&name).is_empty() {
                continue;
            }
            let group = self.0.catalog.lock().groups.remove(&name);
            if let Some(group) = group {
                let previous = group.current.lock().clone();
                if let Err(problem) = self.retire(&previous).await {
                    failure.get_or_insert(problem);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// SW-9：每个原生候选保存真实退役结果，完成该组清理后执行映像阈值策略。
    async fn retire_batch_libraries(&self, group: &PreparedGroup) -> Result<()> {
        let mut failure = None;
        let mut retired_images = 0;
        let mut retirement = None;
        for library in &group.libraries {
            let result = self
                .library_command::<payload::LibraryRetired>(
                    &group.candidate.connection,
                    "library_retire",
                    library.previous,
                )
                .await
                .and_then(|retired| {
                    retired_images = retired_images.max(retired.retired_images);
                    retirement = retired.retirement;
                    if retired.leases == 0 {
                        Ok(retired.quiescence_ms)
                    } else {
                        Err(error(6, "批次旧库仍有活动租约"))
                    }
                });
            if let Some((_, report)) = self
                .0
                .catalog
                .lock()
                .native_reports
                .get_mut(&(group.group.name.clone(), library.plugin.clone()))
            {
                report.succeeded = result.is_ok();
                report.quiescence_ms = result.as_ref().ok().copied().flatten();
                report.failure = result.as_ref().err().map(|e| metadata::clipped(&e.message));
            }
            if let Err(problem) = result {
                failure.get_or_insert(problem);
            }
        }
        if let Some(problem) = failure {
            return Err(self
                .after_failed_libraries(problem, [group.group.name.clone()])
                .await);
        }
        let restarted = self
            .enforce_image_limit(
                group.specification.clone(),
                retired_images,
                retirement.as_ref(),
            )
            .await;
        self.record_image_restart(group, &restarted);
        restarted.map(|_| ())
    }

    /// RM-3/RM-4：阈值重启属于本次换库，换库报告计入重启的暂停与受影响实例。
    fn record_image_restart(
        &self,
        group: &PreparedGroup,
        restarted: &Result<Option<ReplaceReport>>,
    ) {
        let mut catalog = self.0.catalog.lock();
        for library in &group.libraries {
            let Some((_, report)) = catalog
                .native_reports
                .get_mut(&(group.group.name.clone(), library.plugin.clone()))
            else {
                continue;
            };
            match restarted {
                Ok(Some(restart)) => {
                    report.pause_ms = report.pause_ms.saturating_add(restart.pause_ms);
                    report.affected_instances =
                        report.affected_instances.max(restart.affected_instances);
                }
                Ok(None) => {}
                Err(problem) => {
                    report.succeeded = false;
                    report.failure = Some(metadata::clipped(&problem.message));
                }
            }
        }
    }

    /// DOC-11 SW-9：报告记录本批实际影响实例数、提交事实与恢复结果。
    pub(super) fn record_batch_reports(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        affected: usize,
        elapsed: Duration,
        rollback: Option<bool>,
        failure: Option<&WireError>,
        discarded: Option<&cordis_types::ConfirmationDiscard>,
    ) {
        let committed = rollback.is_none();
        let rolled_back = rollback.unwrap_or(false);
        let mut catalog = self.0.catalog.lock();
        for group in prepared.values() {
            let Some(previous) = &group.previous else {
                continue;
            };
            catalog.reports.insert(
                group.group.name.clone(),
                ReplaceReport {
                    group: group.group.name.clone(),
                    previous_connection: previous.connection.id(),
                    candidate_connection: group.candidate.connection.id(),
                    affected_instances: affected,
                    pause_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
                    committed,
                    succeeded: false,
                    rolled_back,
                    failure: failure.map(Failure::from),
                    discarded_confirmation: discarded.cloned(),
                },
            );
            for library in &group.libraries {
                catalog.native_reports.insert(
                    (group.group.name.clone(), library.plugin.clone()),
                    (
                        group.candidate.connection.id(),
                        cordis_types::SwapReport {
                            plugin: library.plugin.clone(),
                            previous_library: library.previous,
                            candidate_library: library.candidate,
                            affected_instances: affected,
                            pause_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
                            quiescence_ms: None,
                            committed,
                            succeeded: false,
                            rolled_back,
                            failure: failure.map(|failure| metadata::clipped(&failure.message)),
                            discarded_confirmation: discarded.cloned(),
                        },
                    ),
                );
            }
        }
    }

    /// SW-1–SW-4、CP-7：候选退出与库退役覆盖全部已准备来源，保留首个清理错误。
    pub(super) async fn discard_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        mut problem: WireError,
    ) -> WireError {
        for group in prepared.values() {
            let cleanup = if group.libraries.is_empty() {
                if group
                    .previous
                    .as_ref()
                    .is_some_and(|old| Arc::ptr_eq(old, &group.candidate))
                {
                    continue;
                }
                self.retire(&group.candidate).await
            } else {
                if !group.candidate.connection.is_alive() {
                    continue;
                }
                let mut failure = None;
                for library in &group.libraries {
                    if let Err(error) = self
                        .library_command::<payload::LibraryRetired>(
                            &group.candidate.connection,
                            "library_retire",
                            library.candidate,
                        )
                        .await
                    {
                        failure.get_or_insert(error);
                    }
                }
                failure.map_or(Ok(()), Err)
            };
            if let Err(cleanup) = cleanup {
                problem.message = format!("{}；候选清理失败：{cleanup}", problem.message);
            }
        }
        problem
    }
}
