//! DOC-11 §5、SW-5、CP-11：冻结快照、检查点、状态导出与解冻恢复。
use super::*;

/// SW-5：原状态与导出器持有关系；提交或解冻完成后统一释放。
#[derive(Default)]
pub(in crate::manager) struct Frozen {
    pub(in crate::manager) states: HashMap<String, Arc<payload::State>>,
    pub(in crate::manager) stateless: HashSet<String>,
    pub(in crate::manager) held: Vec<(Arc<Generation>, Arc<Exporter>)>,
    pub(in crate::manager) failure: Option<WireError>,
}

/// ADR-0041：状态与冻结许可共享寿命；最后一个接收者归还许可时由管理队列解冻。
#[derive(Clone)]
pub struct FrozenSnapshot(Arc<SnapshotLease>);

struct SnapshotLease {
    frozen: Arc<Frozen>,
    owner: Weak<Core>,
    returned: AtomicBool,
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        if self.returned.load(Ordering::Acquire) {
            return;
        }
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        // Shutdown consumes every outstanding freeze by retiring its instance.
        if owner.stopping.load(Ordering::Acquire) {
            return;
        }
        if owner.commands.lock().sender.is_closed() {
            // A failed worker cannot accept cleanup. The connection owner closes
            // the affected instances instead of leaving their writers paused.
            for (_, exporter) in &self.frozen.held {
                exporter.object.peer().terminate("冻结快照的管理任务已结束");
            }
            return;
        }
        let mut returned = owner.abandoned_snapshots.lock();
        // Synchronize with the final shutdown drain: a late return either joins
        // that drain or observes retirement already owning the freeze.
        if owner.stopping.load(Ordering::Acquire) {
            return;
        }
        returned.push(self.frozen.clone());
        drop(returned);
        owner.snapshot_returned.notify_one();
    }
}

impl FrozenSnapshot {
    pub fn states(&self) -> &HashMap<String, Arc<payload::State>> {
        &self.0.frozen.states
    }
}

/// SW-5–SW-8：一次替换的原快照、受影响实例名与变更前纪元。
#[derive(Default)]
pub(in crate::manager) struct Transition {
    pub frozen: Frozen,
    pub inherited: HashMap<String, Option<Arc<payload::State>>>,
    pub names: Vec<String>,
    pub before: HashMap<String, cordis::Epoch>,
    pub permit_pending: bool,
    pub confirming: Option<(std::time::Instant, Vec<String>)>,
}

impl Manager {
    /// DOC-11 SW-5、ADR-0041：取得指定页面的冻结快照，导出失败解冻已访问的对象。
    pub async fn freeze_instances(&self, names: &[String]) -> Result<FrozenSnapshot> {
        let names = names.to_vec();
        self.transaction(move |manager| async move {
            manager.running()?;
            for name in &names {
                manager.instance(name)?;
            }
            let frozen = manager.export(&names, true).await;
            if let Some(problem) = &frozen.failure {
                let problem = problem.clone();
                manager.thaw(&frozen).await?;
                return Err(problem);
            }
            Ok(FrozenSnapshot(Arc::new(SnapshotLease {
                frozen: Arc::new(frozen),
                owner: Arc::downgrade(&manager.0),
                returned: AtomicBool::new(false),
            })))
        })
        .await
    }

    /// DOC-11 SW-5：回退决议解冻原导出器，成功路径在旧页面清理后释放快照句柄。
    pub async fn thaw_snapshot(&self, snapshot: &FrozenSnapshot) -> Result<()> {
        if !Weak::ptr_eq(&snapshot.0.owner, &Arc::downgrade(&self.0)) {
            return Err(error(2, "冻结快照属于其他管理器"));
        }
        let lease = snapshot.0.clone();
        self.transaction(move |manager| async move {
            if lease.returned.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            manager.thaw_snapshot_contents(&lease.frozen).await
        })
        .await
    }

    async fn thaw_snapshot_contents(&self, frozen: &Frozen) -> Result<()> {
        // Successful replacement consumes the old generation's freeze. Only a
        // surviving generation needs its write permission returned.
        let live = Frozen {
            held: frozen
                .held
                .iter()
                .filter(|(generation, _)| generation.is_current())
                .cloned()
                .collect(),
            ..Frozen::default()
        };
        self.thaw(&live).await
    }

    pub(in crate::manager) async fn thaw_abandoned_snapshots(&self) {
        let returned = std::mem::take(&mut *self.0.abandoned_snapshots.lock());
        for frozen in returned {
            if let Err(problem) = self.thaw_snapshot_contents(&frozen).await {
                self.0
                    .context
                    .logger()
                    .error(saved_error(&problem).to_string());
            }
        }
    }

    /// SW-5：取得可逆冻结与原快照；任一导出失败时解冻已经访问的导出器。
    pub(in crate::manager) async fn prepare_transition(
        &self,
        roots: impl IntoIterator<Item = String>,
        incoming: Vec<(Context, Arc<RemoteComponent>, String)>,
        stop_on_error: bool,
    ) -> Result<Transition> {
        let instances = self.select_instances(roots, incoming);
        let names: Vec<_> = instances
            .iter()
            .map(|instance| instance.data.name.clone())
            .collect();
        let mut frozen = self.export(&names, stop_on_error).await;
        if let Some(problem) = &frozen.failure
            && stop_on_error
        {
            let problem = problem.clone();
            self.thaw(&frozen).await?;
            return Err(problem);
        }
        let inherited = {
            let mut catalog = self.0.catalog.lock();
            for name in &frozen.stateless {
                catalog.checkpoints.remove(name);
            }
            catalog.checkpoints.extend(frozen.states.drain());
            instances
                .iter()
                .map(|instance| {
                    (
                        instance.data.name.clone(),
                        catalog
                            .checkpoints
                            .get(&instance.data.name)
                            .cloned()
                            .or_else(|| instance.data.inherited.lock().clone()),
                    )
                })
                .collect()
        };
        Ok(Transition {
            frozen,
            inherited,
            names,
            before: instances
                .iter()
                .map(|instance| (instance.data.name.clone(), instance.fiber.epoch()))
                .collect(),
            permit_pending: false,
            confirming: None,
        })
    }

    pub async fn checkpoint_instances(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, payload::State>> {
        let names = names.to_vec();
        self.transaction(move |manager| async move {
            manager.running()?;
            for name in &names {
                manager.instance(name)?;
            }
            Ok(manager
                .checkpoint_selected(&names)
                .await
                .states
                .into_iter()
                .map(|(name, state)| (name, Arc::unwrap_or_clone(state)))
                .collect())
        })
        .await
    }

    /// DOC-11 §9、CP-11：停止结构变更后保存各实例成功导出的状态，冻结持有至 shutdown。
    pub async fn export_shutdown(&self) -> Result<HashMap<String, Arc<payload::State>>> {
        self.transaction(|manager| async move {
            if let Some(frozen) = manager.0.shutdown_export.lock().as_ref() {
                return Ok(frozen.states.clone());
            }
            manager.running()?;
            manager.0.exported.store(true, Ordering::Release);
            let names = manager.instance_names();
            let mut frozen = manager.export(&names, false).await;
            let durable: HashSet<_> = manager.deployment_instance_names().into_iter().collect();
            frozen.states.retain(|name, _| durable.contains(name));
            let states = frozen.states.clone();
            manager.0.catalog.lock().checkpoints.extend(states.clone());
            *manager.0.shutdown_export.lock() = Some(frozen);
            Ok(states)
        })
        .await
    }

    pub async fn checkpoint(&self) -> Result<HashMap<String, payload::State>> {
        self.transaction(|manager| async move {
            manager.running()?;
            let names = manager.instance_names();
            Ok(manager
                .checkpoint_selected(&names)
                .await
                .states
                .into_iter()
                .map(|(name, state)| (name, Arc::unwrap_or_clone(state)))
                .collect())
        })
        .await
    }

    /// 部署提交区分已导出的值、已确认无状态的实例和本次缺席的结果。
    pub async fn deployment_checkpoint(
        &self,
        names: Vec<String>,
    ) -> Result<HashMap<String, Option<payload::State>>> {
        self.transaction(move |manager| async move {
            manager.running()?;
            Ok(manager
                .capture_deployment(&names)
                .await
                .into_iter()
                .map(|(name, state)| (name, state.map(Arc::unwrap_or_clone)))
                .collect())
        })
        .await
    }

    pub(in crate::manager) async fn capture_deployment(
        &self,
        names: &[String],
    ) -> HashMap<String, Option<Arc<payload::State>>> {
        let frozen = self.checkpoint_selected(names).await;
        frozen
            .states
            .into_iter()
            .map(|(name, state)| (name, Some(state)))
            .chain(frozen.stateless.into_iter().map(|name| (name, None)))
            .collect()
    }

    /// 周期任务每次只预约一个实例。轮到该实例时，它可能已被管理事务移除。
    pub async fn checkpoint_instance(
        &self,
        name: String,
    ) -> Result<HashMap<String, Arc<payload::State>>> {
        self.transaction(move |manager| async move {
            manager.running()?;
            Ok(manager.checkpoint_selected(&[name]).await.states)
        })
        .await
    }

    async fn checkpoint_selected(&self, names: &[String]) -> Frozen {
        let mut selected = Frozen::default();
        for name in names {
            let frozen = self.export(std::slice::from_ref(name), false).await;
            if let Err(problem) = self.thaw(&frozen).await {
                self.0
                    .context
                    .logger()
                    .error(format!("检查点解冻失败：{problem}"));
            }
            selected.states.extend(frozen.states);
            selected.stateless.extend(frozen.stateless);
        }
        self.0.catalog.lock().checkpoints.extend(
            selected
                .states
                .iter()
                .map(|(name, state)| (name.clone(), state.clone())),
        );
        selected
    }

    /// DOC-11 §5、SW-5：对选定活跃实例执行有截止时间的导出，并保留源侧持有关系。
    pub(in crate::manager) async fn export(&self, names: &[String], stop_on_error: bool) -> Frozen {
        let instances = {
            let catalog = self.0.catalog.lock();
            names
                .iter()
                .filter_map(|name| catalog.instances.get(name).cloned())
                .collect::<Vec<_>>()
        };
        let mut frozen = Frozen::default();
        for instance in instances {
            let generation = instance.data.current.lock().upgrade();
            let Some(generation) = generation else {
                continue;
            };
            // CP-6：断连可先于排队的回收事务，导出资格同时取决于实际连接。
            if !generation.is_current()
                || instance.fiber.state() != cordis::FiberState::Active
                || !generation.component.connection.is_alive()
            {
                continue;
            }
            let exporter = generation.exporter.lock().clone();
            let Some(exporter) = exporter else {
                frozen.stateless.insert(instance.data.name.clone());
                continue;
            };
            frozen.held.push((generation.clone(), exporter.clone()));
            match self.export_state(&exporter).await {
                Ok(state) => {
                    frozen
                        .states
                        .insert(instance.data.name.clone(), Arc::new(state));
                }
                Err(problem) => {
                    self.0.context.logger().error(format!(
                        "实例 {} 导出失败：{}",
                        instance.data.name,
                        saved_error(&problem)
                    ));
                    frozen.failure.get_or_insert(problem);
                    if stop_on_error {
                        break;
                    }
                }
            }
        }
        frozen
    }

    /// SW-5、CP-4：调用活跃导出器并验证持久状态值，截止时间或编码错误交回冻结事务。
    async fn export_state(&self, exporter: &Exporter) -> Result<payload::State> {
        let response = self
            .lifecycle_request(
                exporter.object.peer(),
                "export",
                value(&payload::Export {
                    object: exporter.object.id(),
                    context: exporter.context,
                })?,
                self.0.options.export_timeout,
            )
            .await?;
        let result: payload::ValueResult = decode(response.value())?;
        payload::persistent_value(&result.value)
            .map_err(|problem| error(2, problem.to_string()))?;
        Ok(payload::State {
            schema: exporter.schema.clone(),
            version: exporter.version,
            data: result.value,
        })
    }

    /// SW-5、DOC-11 §7：解除已冻结写入；连接失效由其故障回收路径处理。
    pub(in crate::manager) async fn thaw(&self, frozen: &Frozen) -> Result<()> {
        let mut failure = None;
        for (_, exporter) in &frozen.held {
            if !exporter.object.peer().is_alive() {
                continue;
            }
            let result = async {
                let response = self
                    .lifecycle_request(
                        exporter.object.peer(),
                        "thaw",
                        value(&payload::Thaw {
                            object: exporter.object.id(),
                            context: exporter.context,
                        })?,
                        self.0.options.cleanup_timeout,
                    )
                    .await?;
                decode::<()>(response.value())
            }
            .await;
            if let Err(problem) = result {
                exporter.object.peer().terminate("状态解冻失败");
                failure.get_or_insert(problem);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub(in crate::manager) fn inherit(
        &self,
        states: &HashMap<String, Option<Arc<payload::State>>>,
    ) {
        let instances = {
            let mut catalog = self.0.catalog.lock();
            let mut instances = Vec::new();
            for (name, state) in states {
                match state {
                    Some(state) => {
                        catalog.checkpoints.insert(name.clone(), state.clone());
                    }
                    None => {
                        catalog.checkpoints.remove(name);
                    }
                }
                if let Some(instance) = catalog.instances.get(name) {
                    instances.push((instance.clone(), state.clone()));
                }
            }
            instances
        };
        for (instance, state) in instances {
            *instance.data.inherited.lock() = state;
        }
    }
}
