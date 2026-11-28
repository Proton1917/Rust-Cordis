//! DOC-11 SW-7–SW-9、CP-6/CP-7：稳定等待、确认期、断连监管、退避重启与有序关闭。
use super::*;

impl Manager {
    /// ADR-0044：配置调和接受内核计算的 Pending，版本替换继续要求 Active。
    pub(in crate::manager) async fn stabilize_with(
        &self,
        before: &HashMap<String, cordis::Epoch>,
        explicit: &[String],
        permit_pending: bool,
    ) -> Result<Vec<Fiber>> {
        let future = async {
            loop {
                let instances = self.instances();
                let mut changed: Vec<_> = instances
                    .into_iter()
                    .filter(|instance| {
                        explicit.contains(&instance.data.name)
                            || before
                                .get(&instance.data.name)
                                .is_some_and(|epoch| *epoch != instance.fiber.epoch())
                    })
                    .collect();
                for (instance, result) in changed
                    .iter()
                    .zip(join_all(changed.iter().map(|instance| instance.fiber.settled())).await)
                {
                    result.map_err(|problem| {
                        instance
                            .data
                            .error
                            .lock()
                            .as_ref()
                            .map(|problem| problem.value.clone())
                            .unwrap_or_else(|| kernel_error(problem))
                    })?;
                }
                // ADR-0041：等待期间父级可完成子实例退役，按当前目录身份判断存活部署。
                changed.retain(|instance| {
                    let catalog = self.0.catalog.lock();
                    catalog
                        .instances
                        .get(&instance.data.name)
                        .is_some_and(|current| Arc::ptr_eq(current, instance))
                });
                if changed.iter().any(|instance| !instance.fiber.is_settled()) {
                    continue;
                }
                if changed.iter().any(|instance| {
                    let state = instance.fiber.state();
                    state != cordis::FiberState::Active
                        && !(permit_pending && state == cordis::FiberState::Pending)
                }) {
                    return Err(error(4, "受影响实例未进入Active"));
                }
                return Ok(changed
                    .into_iter()
                    .map(|instance| instance.fiber.clone())
                    .collect());
            }
        };
        tokio::time::timeout(self.0.options.swap_timeout, future)
            .await
            .map_err(|_| error(6, "实例稳定等待超时"))?
    }

    /// SW-8：在确认期观察受影响纤程及连接，故障交回替换事务执行回滚。
    pub(in crate::manager) async fn confirm(
        &self,
        fibers: &[Fiber],
        connections: &[Connection],
    ) -> Result<()> {
        if connections.is_empty() || fibers.is_empty() || self.0.options.confirm.is_zero() {
            return Ok(());
        }
        let failed = Arc::new(tokio::sync::Notify::new());
        let observed = failed.clone();
        let fibers = fibers.to_vec();
        let listener = self
            .0
            .context
            .on::<cordis::InternalStatus>(move |(fiber, _)| {
                if fibers.contains(fiber)
                    && fiber.state() != cordis::FiberState::Active
                    && (fiber.state() == cordis::FiberState::Failed
                        || !fiber.is_alive()
                        || fiber.epoch() != cordis::Epoch::Inactive)
                {
                    observed.notify_one();
                }
                None
            })
            .map_err(kernel_error)?;
        let mut closed: FuturesUnordered<_> = connections.iter().map(Connection::closed).collect();
        let result = tokio::select! {
            _ = tokio::time::sleep(self.0.options.confirm) => Ok(()),
            _ = failed.notified() => Err(error(4, "确认期内实例离开Active")),
            Some(reason) = closed.next() => Err(error(4, reason)),
        };
        listener.dispose().await.map_err(kernel_error)?;
        result
    }

    /// CP-6：连接断开只提交恢复请求，恢复动作由唯一事务执行者安排。
    pub(in crate::manager) fn monitor(
        &self,
        group: Arc<Group>,
        version: Arc<GroupGeneration>,
    ) -> Result<()> {
        let connection = version.connection.clone();
        let weak = Arc::downgrade(&self.0);
        let mut stopped = self.0.stop.subscribe();
        self.0
            .context
            .effect_async(
                format!("connection-monitor:{}", connection.id()),
                move |_| async move {
                    tokio::select! {
                        reason = connection.closed() => {
                            if let Some(core) = weak.upgrade() {
                                let manager = Manager(core);
                                manager.reclaim_connection(connection.id());
                                let current = group.current.lock().clone();
                                if current.connection.id() == connection.id() {
                                    manager.queue_recovery(group, current, reason).await;
                                }
                            }
                        }
                        _ = stopped.changed() => {}
                    }
                    Ok(())
                },
            )
            .map_err(kernel_error)?;
        Ok(())
    }

    async fn queue_recovery(
        &self,
        group: Arc<Group>,
        version: Arc<GroupGeneration>,
        reason: String,
    ) {
        let sender = {
            let queue = self.0.commands.lock();
            if !queue.accepting {
                return;
            }
            queue.sender.clone()
        };
        let (reply, _result) = oneshot::channel();
        let work: Command = Box::new(Action {
            callback: move |manager: Manager| async move {
                manager.recover(group, version, reason).await
            },
            reply,
        });
        if sender.send(work).await.is_err() && !self.0.stopping.load(Ordering::Acquire) {
            self.0.context.logger().error("进程回收操作队列已结束");
        }
    }

    /// CP-6：确认断开的仍为当前连接后撤回服务，按有限退避重启并继承最近检查点。
    /// 关闭信号中断退避，耗尽次数保留failed；候选完成重建后才报告running。
    async fn recover(
        &self,
        group: Arc<Group>,
        version: Arc<GroupGeneration>,
        reason: String,
    ) -> Result<()> {
        if !self.recovery_current(&group, version.connection.id()) {
            return Ok(());
        }
        let problem = error(4, metadata::clipped(&reason));
        let transition = self
            .prepare_transition(
                self.group_instances(&group.name)
                    .into_iter()
                    .map(|i| i.data.name.clone()),
                Vec::new(),
                false,
            )
            .await?;
        self.inherit(&transition.inherited);
        self.withdraw(&group, &version, problem).await;
        let cohort = transition
            .names
            .iter()
            .filter_map(|name| self.find_instance(name))
            .map(|instance| Arc::downgrade(&instance))
            .collect();
        self.thaw_current(&transition.frozen).await?;
        self.schedule_restart(group, cohort);
        Ok(())
    }

    fn recovery_current(&self, group: &Arc<Group>, connection: u64) -> bool {
        self.running().is_ok()
            && self
                .0
                .catalog
                .lock()
                .groups
                .get(&group.name)
                .is_some_and(|current| Arc::ptr_eq(current, group))
            && group.current.lock().connection.id() == connection
    }

    /// 退避计时在管理队列外；轮到实际恢复时重新核对分组和连接。
    fn schedule_restart(&self, group: Arc<Group>, cohort: Vec<Weak<Instance>>) {
        let Some(specification) = group.specification.lock().clone() else {
            return;
        };
        let attempt = group.restarts.load(Ordering::Acquire);
        if attempt >= specification.restart.max {
            group.health.lock().state = "failed".into();
            return;
        }
        group.health.lock().state = "backoff".into();
        let delay = specification
            .restart
            .backoff
            .saturating_mul(1u32 << attempt);
        let connection = group.current.lock().connection.id();
        let mut shutdown = self.0.stop.subscribe();
        let sender = self.0.commands.lock().sender.clone();
        let (reply, _result) = oneshot::channel();
        let work: Command = Box::new(Action {
            callback: move |manager: Manager| async move {
                if manager.recovery_current(&group, connection) {
                    group.restarts.fetch_add(1, Ordering::AcqRel);
                    group.health.lock().state = "restarting".into();
                    if let Err(problem) = manager
                        .retry_deployment(specification.specification, &cohort)
                        .await
                    {
                        group.health.lock().error = Some(saved_error(&problem));
                        manager.schedule_restart(group, cohort);
                    }
                }
                Ok(())
            },
            reply,
        });
        tokio::spawn(async move {
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                _ = shutdown.changed() => {},
                _ = tokio::time::sleep(delay) => { let _ = sender.send(work).await; },
            }
        });
    }

    /// CP-6：以失败定义触发内核正常卸载，依赖方随绑定撤回进入Pending。
    async fn withdraw(&self, group: &Group, version: &GroupGeneration, problem: WireError) {
        *group.health.lock() = Health {
            state: "failed".into(),
            error: Some(problem.clone()),
        };
        let previous = group.definitions.lock().clone();
        let mut failed = HashMap::new();
        for (key, definition) in previous {
            let failure = problem.clone();
            let next = ComponentDef::new(
                format!("failed:{}:{}", version.connection.id(), failed.len()),
                Inject::new(),
                move |_, config: Configuration| {
                    let failure = failure.clone();
                    async move {
                        *config.instance.error.lock() = Some(metadata::RetainedError {
                            value: failure.clone(),
                            _budget: None,
                        });
                        Err(kernel(failure))
                    }
                },
            );
            self.0.context.registry().replace(&definition, &next);
            failed.insert(key, next);
        }
        *group.definitions.lock() = failed;
        for result in join_all(
            self.group_instances(&group.name)
                .iter()
                .map(|instance| instance.fiber.settled()),
        )
        .await
        {
            if let Err(problem) = result {
                self.0
                    .context
                    .logger()
                    .debug(metadata::clipped(&problem.to_string()));
            }
        }
    }

    /// DOC-11 §9：关闭新请求入口，取消排队操作，等待已开始事务和组清理完成。
    pub(in crate::manager) async fn shutdown_contents(&self) -> Result<()> {
        let sender = {
            let mut queue = self.0.commands.lock();
            queue.accepting = false;
            queue.sender.clone()
        };
        self.0.stop.send_replace(true);
        let mut finished = self.0.finished.subscribe();
        loop {
            if let Some(result) = finished.borrow_and_update().clone() {
                return result;
            }
            if sender.is_closed() {
                if let Some(result) = finished.borrow_and_update().clone() {
                    return result;
                }
                let mut failure = error(4, "核心管理操作任务已结束");
                if let Err(cleanup) = self.close_all().await {
                    failure.message = format!(
                        "{}；清理失败(code {})：{}",
                        failure.message,
                        cleanup.code,
                        metadata::clipped(&cleanup.message)
                    );
                    failure.detail = Value::Map(vec![
                        (
                            Value::Text("cleanup_code".into()),
                            Value::Integer(cleanup.code.into()),
                        ),
                        (
                            Value::Text("cleanup_message".into()),
                            Value::Text(cleanup.message),
                        ),
                        (Value::Text("cleanup_detail".into()), cleanup.detail),
                    ]);
                }
                self.0.finished.send_replace(Some(Err(failure)));
                continue;
            }
            tokio::select! {
                result = finished.changed() => result.map_err(|_| error(4, "核心管理器结束状态已关闭"))?,
                _ = sender.closed() => {},
            }
        }
    }

    /// DOC-11 §9、CP-7：冻结最终状态后释放实例与分组；清理覆盖全部已登记来源。
    pub(in crate::manager) async fn close_all(&self) -> Result<()> {
        self.0.stopping.store(true, Ordering::Release);
        self.0.stop.send_replace(true);
        let exported = self.0.shutdown_export.lock().take();
        let frozen = match exported {
            Some(frozen) => frozen,
            None => {
                let mut frozen = self.export(&self.instance_names(), false).await;
                self.0
                    .catalog
                    .lock()
                    .checkpoints
                    .extend(frozen.states.drain());
                frozen
            }
        };
        let (instances, groups) = {
            let catalog = self.0.catalog.lock();
            (
                catalog.instances.values().cloned().collect::<Vec<_>>(),
                catalog.groups.values().cloned().collect::<Vec<_>>(),
            )
        };
        let mut failure = frozen.failure.clone();
        for result in join_all(instances.iter().map(|instance| instance.fiber.dispose())).await {
            if let Err(problem) = result {
                failure.get_or_insert_with(|| kernel_error(problem));
            }
        }
        drop(frozen);
        failure = failure.or(self.close_held_deployment().await.err());
        for group in groups {
            self.teardown_group(&group, &mut failure).await;
            group.health.lock().state = "stopped".into();
        }
        let discarded = std::mem::take(&mut self.0.catalog.lock().instances);
        drop(discarded);
        self.0.abandoned_snapshots.lock().clear();
        failure.map_or(Ok(()), Err)
    }
}
