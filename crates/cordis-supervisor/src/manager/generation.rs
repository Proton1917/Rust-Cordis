use super::metadata::{MetadataLease, RetainedError};
use super::*;
use cordis::{Effect, EffectScope};
use std::{ops::Deref, sync::atomic::AtomicUsize};

pub(super) struct InstanceData {
    pub name: String,
    pub group: String,
    pub plugin: String,
    pub component: String,
    pub current: Mutex<Weak<Generation>>,
    pub config: Mutex<Value>,
    pub inherited: Mutex<Option<payload::State>>,
    pub error: Mutex<Option<RetainedError>>,
}

#[derive(Clone)]
pub(super) struct Configuration {
    pub instance: Arc<InstanceData>,
    pub config: Value,
}

pub(super) struct RemoteComponent {
    pub group: String,
    pub plugin: String,
    pub library: Option<u64>,
    pub info: payload::Component,
    pub inject: Vec<(Interface, bool)>,
    pub provides: Vec<Interface>,
    pub connection: Connection,
    pub _metadata: Arc<MetadataLease>,
}

pub(super) struct ContextEntry {
    pub generation: Weak<Generation>,
    pub view: Context,
}
#[derive(Clone)]
pub(super) struct ViewLease {
    pub id: u64,
    pub context: Context,
    pub generation: Arc<Generation>,
}
impl Deref for ViewLease {
    type Target = Generation;
    fn deref(&self) -> &Self::Target {
        &self.generation
    }
}

pub(super) struct Exporter {
    pub schema: String,
    pub version: u32,
    pub context: u64,
    pub object: RemoteObject,
}

pub(super) struct Generation {
    pub id: u64,
    pub context: Context,
    pub instance: Arc<InstanceData>,
    pub component: Arc<RemoteComponent>,
    pub exporter: Mutex<Option<Arc<Exporter>>>,
    pub config: Value,
    pub inherited: Option<payload::State>,
    pub epoch: cordis::Epoch,
    pub effects: Mutex<HashMap<u64, Arc<RemoteEffect>>>,
    registrations: Arc<AtomicUsize>,
    max_registrations: usize,
    pub core: Weak<Core>,
    pub handles: Mutex<Vec<u64>>,
}

impl Generation {
    pub fn is_current(&self) -> bool {
        self.context.is_current() && self.context.fiber().epoch() == self.epoch
    }
    pub fn active(&self) -> Result<()> {
        if self.is_current()
            && self.component.connection.is_alive()
            && self
                .core
                .upgrade()
                .is_some_and(|core| !core.stopping.load(Ordering::Acquire))
        {
            Ok(())
        } else {
            Err(error(4, "上下文正在清理"))
        }
    }
    pub fn reserve(&self) -> Result<Arc<Registration>> {
        self.registrations
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                if count < self.max_registrations {
                    Some(count + 1)
                } else {
                    None
                }
            })
            .map_err(|_| error(8, "该上下文的保留登记已达上限"))?;
        Ok(Arc::new(Registration(self.registrations.clone())))
    }
}

pub(super) struct Registration(Arc<AtomicUsize>);
impl Drop for Registration {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        if let Some(core) = self.core.upgrade() {
            let contexts = {
                let mut contexts = core.contexts.lock();
                self.handles
                    .get_mut()
                    .iter()
                    .filter_map(|id| contexts.remove(id))
                    .collect::<Vec<_>>()
            };
            let views = {
                let mut views = core.scoped_views.lock();
                self.handles
                    .get_mut()
                    .iter()
                    .filter_map(|id| views.remove(id))
                    .collect::<Vec<_>>()
            };
            drop((contexts, views));
            let mut effects = core.effects.lock();
            for id in self.effects.get_mut().keys() {
                effects
                    .records
                    .remove(&(self.component.connection.id(), *id));
            }
        }
        for id in self.handles.get_mut().iter() {
            let notification = value(&payload::Context { context: *id });
            if let Err(problem) = notification.and_then(|params| {
                self.component
                    .connection
                    .try_notify("release_context", params)
            }) && let Some(core) = self.core.upgrade()
            {
                core.context
                    .logger()
                    .warn(saved_error(&problem).to_string());
            }
        }
    }
}

pub(super) struct Binding {
    pub interface: Interface,
    pub route: Arc<Route>,
    pub generation: Option<Arc<Generation>>,
    pub _registration: Option<Arc<Registration>>,
    pub _metadata: Option<Arc<MetadataLease>>,
}

pub(super) struct ScopeRecord {
    pub view: ViewLease,
    pub scope: EffectScope,
    pub failures: Arc<Mutex<Vec<RetainedError>>>,
}
struct ScopeGuard {
    id: u64,
    record: Arc<ScopeRecord>,
    core: Weak<Core>,
}

impl ScopeGuard {
    fn new(
        manager: &Manager,
        view: ViewLease,
        scope: EffectScope,
        failures: Arc<Mutex<Vec<RetainedError>>>,
    ) -> Self {
        let record = Arc::new(ScopeRecord {
            view,
            scope,
            failures,
        });
        let id = manager.id();
        manager.0.scopes.lock().insert(id, Arc::downgrade(&record));
        Self {
            id,
            record,
            core: Arc::downgrade(&manager.0),
        }
    }
}
impl Drop for ScopeGuard {
    fn drop(&mut self) {
        if let Some(core) = self.core.upgrade() {
            core.scopes.lock().remove(&self.id);
        }
    }
}

pub(super) struct RemoteEffect {
    pub generation: Weak<Generation>,
    pub effect: Effect,
    pub failures: Arc<Mutex<Vec<RetainedError>>>,
    _registration: Arc<Registration>,
    _metadata: Arc<MetadataLease>,
}

struct EffectDelivery {
    core: Weak<Core>,
    connection: Connection,
    id: u64,
    committed: bool,
}
impl EffectDelivery {
    fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for EffectDelivery {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(core) = self.core.upgrade() {
            let manager = Manager(core);
            if let Err(problem) = manager.effect_release(self.connection.id(), self.id)
                && self.connection.is_alive()
            {
                manager.0.context.logger().error(problem.to_string());
            }
        }
    }
}

pub(super) fn definition(core: Weak<Core>, component: Arc<RemoteComponent>) -> ComponentDef {
    let mut inject = Inject::new();
    for (interface, required) in &component.inject {
        let minor = interface.minor;
        inject = inject.with_named_options(
            interface.key.clone(),
            *required,
            Some(Arc::new(move |value| {
                value
                    .downcast_ref::<Binding>()
                    .is_some_and(|binding| binding.interface.minor >= minor)
            })),
        );
    }
    let name = metadata::clipped(&format!(
        "{}/{}/{}",
        component.group, component.plugin, component.info.name
    ));
    ComponentDef::new(
        name,
        inject,
        move |context, configuration: Configuration| {
            let core = core.clone();
            let component = component.clone();
            async move {
                let manager = Manager(
                    core.upgrade()
                        .ok_or_else(|| cordis::Error::new("核心生命周期已结束"))?,
                );
                let id = manager.id();
                let generation = Arc::new(Generation {
                    id,
                    epoch: context.fiber().epoch(),
                    context: context.clone(),
                    instance: configuration.instance.clone(),
                    component,
                    exporter: Mutex::new(None),
                    config: configuration.config,
                    inherited: configuration.instance.inherited.lock().take(),
                    effects: Mutex::new(HashMap::new()),
                    registrations: Arc::new(AtomicUsize::new(0)),
                    max_registrations: manager.0.options.max_registrations_per_context,
                    core: Arc::downgrade(&manager.0),
                    handles: Mutex::new(vec![id]),
                });
                *generation.instance.error.lock() = None;
                *generation.instance.current.lock() = Arc::downgrade(&generation);
                manager.0.contexts.lock().insert(
                    generation.id,
                    ContextEntry {
                        generation: Arc::downgrade(&generation),
                        view: context.clone(),
                    },
                );
                let keeper = generation.clone();
                context.effect("remote-context", move |scope| {
                    scope.defer(move || {
                        drop(keeper);
                        Ok(())
                    })
                })?;
                let request = payload::Apply {
                    plugin: generation.component.plugin.clone(),
                    component: generation.component.info.name.clone(),
                    library: generation.component.library,
                    context: generation.id,
                    config: generation.config.clone(),
                    inherited: generation.inherited.clone(),
                };
                let result = match value(&request) {
                    Ok(params) => manager
                        .lifecycle_request(
                            &generation.component.connection,
                            "apply",
                            params,
                            manager.0.options.apply_timeout,
                        )
                        .await
                        .and_then(|output| decode::<()>(output.value())),
                    Err(problem) => Err(problem),
                };
                result.map_err(|problem| {
                    let retained = manager.keep_error(&generation, problem.clone());
                    *generation.instance.error.lock() = Some(retained);
                    kernel(problem)
                })
            }
        },
    )
}

impl Manager {
    pub(super) async fn callback(
        &self,
        view: &ViewLease,
        callback: &RemoteObject,
        scope: u64,
    ) -> Result<()> {
        let params = value(&payload::Callback {
            object: callback.id(),
            context: view.id,
            scope,
            payload: Value::Null,
            refs: Vec::new(),
        })?;
        let timeout = if scope == 0 {
            self.0.options.cleanup_timeout
        } else {
            self.0.options.apply_timeout
        };
        let output = self
            .lifecycle_request(callback.peer(), "callback", params, timeout)
            .await?;
        decode(output.value())
    }

    pub(super) async fn create_effect(
        &self,
        generation: ViewLease,
        label: String,
        callback: RemoteObject,
        asynchronous: bool,
    ) -> Result<Outgoing> {
        generation.active()?;
        let registration = generation.reserve()?;
        let metadata = self.retain_metadata(&generation.component.connection, label.len())?;
        let id = {
            let mut effects = self.0.effects.lock();
            let next = effects
                .next
                .entry(generation.component.connection.id())
                .or_insert(1);
            let id = *next;
            *next = next
                .checked_add(1)
                .ok_or_else(|| error(8, "效应编号空间已耗尽"))?;
            id
        };
        let actual = registration.clone();
        let actual_metadata = metadata.clone();
        let manager = self.clone();
        let current = generation.clone();
        let failures = Arc::new(Mutex::new(Vec::new()));
        let failed = failures.clone();
        let effect = generation
            .context
            .effect_async(label, move |scope| async move {
                let retained = current.clone();
                scope.defer(move || {
                    drop((retained, actual, actual_metadata));
                    Ok(())
                })?;
                let scope = ScopeGuard::new(&manager, current, scope, failed.clone());
                manager
                    .callback(&scope.record.view, &callback, scope.id)
                    .await
                    .map_err(|problem| {
                        failed
                            .lock()
                            .push(manager.keep_error(&scope.record.view, problem.clone()));
                        kernel(problem)
                    })
            })
            .map_err(kernel_error)?;
        let record = Arc::new(RemoteEffect {
            generation: Arc::downgrade(&generation.generation),
            effect,
            failures,
            _registration: registration,
            _metadata: metadata,
        });
        if !asynchronous {
            record.effect.settled().await.map_err(|problem| {
                record
                    .failures
                    .lock()
                    .first()
                    .map(|problem| problem.value.clone())
                    .unwrap_or_else(|| kernel_error(problem))
            })?;
        }
        self.0.effects.lock().records.insert(
            (generation.component.connection.id(), id),
            Arc::downgrade(&record),
        );
        generation.effects.lock().insert(id, record);
        let delivery = EffectDelivery {
            core: Arc::downgrade(&self.0),
            connection: generation.component.connection.clone(),
            id,
            committed: false,
        };
        Ok(Outgoing::with_commit(
            value(&payload::EffectHandle { effect: id })?,
            move || delivery.commit(),
        ))
    }

    pub(super) fn defer(&self, connection: u64, scope: u64, callback: RemoteObject) -> Result<()> {
        let scope = self
            .0
            .scopes
            .lock()
            .get(&scope)
            .and_then(Weak::upgrade)
            .ok_or_else(|| error(4, "效应作用域已结束"))?;
        if scope.view.component.connection.id() != connection {
            return Err(error(4, "效应作用域属于另一条连接"));
        }
        let registration = scope.view.reserve()?;
        let manager = self.clone();
        let current = scope.view.clone();
        let failures = scope.failures.clone();
        scope
            .scope
            .defer_async(move || async move {
                let _registration = registration;
                manager
                    .callback(&current, &callback, 0)
                    .await
                    .map_err(|problem| {
                        failures
                            .lock()
                            .push(manager.keep_error(&current, problem.clone()));
                        kernel(problem)
                    })
            })
            .map_err(kernel_error)
    }

    pub(super) async fn effect_wait(&self, connection: u64, id: u64, dispose: bool) -> Result<()> {
        let record = self
            .0
            .effects
            .lock()
            .records
            .get(&(connection, id))
            .and_then(Weak::upgrade)
            .ok_or_else(|| error(4, "效应句柄已退役"))?;
        let generation = record
            .generation
            .upgrade()
            .ok_or_else(|| error(4, "效应所属上下文已退役"))?;
        if generation.component.connection.id() != connection {
            return Err(error(4, "效应属于另一条连接"));
        }
        let result = if dispose {
            record.effect.dispose().await
        } else {
            record.effect.settled().await.map(|_| ())
        };
        result.map_err(|problem| {
            record
                .failures
                .lock()
                .first()
                .map(|problem| problem.value.clone())
                .unwrap_or_else(|| kernel_error(problem))
        })
    }

    pub(super) fn effect_release(&self, connection: u64, id: u64) -> Result<()> {
        let record = {
            let mut effects = self.0.effects.lock();
            let next = effects.next.get(&connection).copied().unwrap_or(1);
            if id == 0 || id >= next {
                return Err(error(4, "效应编号属于该连接已分配范围之外"));
            }
            effects
                .records
                .remove(&(connection, id))
                .and_then(|record| record.upgrade())
        };
        if let Some(record) = record
            && let Some(generation) = record.generation.upgrade()
        {
            let removed = generation.effects.lock().remove(&id);
            drop(removed);
        }
        Ok(())
    }

    fn keep_error(&self, generation: &Generation, problem: WireError) -> RetainedError {
        match self.retain_error(&generation.component.connection, &problem) {
            Ok(retained) => retained,
            Err(exhausted) => RetainedError {
                value: saved_error(&exhausted),
                _budget: None,
            },
        }
    }
}
