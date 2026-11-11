//! 绑定存储、访问中介与依赖通知。

use crate::{
    Context, Error, Event, FiberState, Provision, Realm, Result, Scoped, ServiceKey, SyncKind,
    Task, WaterfallKind,
    context::{Layer, Scope},
    kernel::{FiberId, ImplId, Kernel, State},
};
use futures_util::future::poll_fn;
use std::{
    any::{Any, TypeId},
    collections::HashSet,
    marker::PhantomData,
    panic::{AssertUnwindSafe, Location, catch_unwind},
    sync::Arc,
    task::{Poll, Waker},
};

pub(crate) type AnyValue = Arc<dyn Any + Send + Sync>;
type Check = Arc<dyn Fn() -> bool + Send + Sync>;

/// S-SVC-5：服务通知携带的名称与通知时的实时绑定。
#[derive(Clone)]
pub struct ServiceChange {
    /// 发生通知的服务名称，与 `ServiceKey::NAME` 相同。
    pub name: &'static str,
    /// 当前键和隔离域的实时值；绑定存在时为 `Some`，缺失时为 `None`。
    ///
    /// 类型擦除层保存 `Arc<K::Value>`，可用 `downcast_ref::<Arc<K::Value>>()` 读取。
    pub value: Option<Arc<dyn Any + Send + Sync>>,
}
/// S-SVC-5：提供、撤回及提供方可用状态变化时派发的同步服务通知。
///
/// 参数为 `ServiceChange`。派发按服务隔离域过滤监听器，
/// `global` 监听器遵循 `EventOptions` 的全局接收规则。
pub struct InternalService;
impl Event for InternalService {
    /// 服务变化事件的名称。
    const NAME: &'static str = "internal/service";
    /// 通知时的名称与实时绑定值。
    type Args = ServiceChange;
    /// 同步通知的返回值为单位值。
    type Output = ();
    /// 该事件采用同步派发。
    type Kind = SyncKind;
}

/// 服务读取或修改瀑布的访问上下文与目标名称。
#[derive(Clone)]
pub struct ServiceAccess {
    /// 读取时为请求上下文，修改时为提供该绑定的上下文。
    pub context: Context,
    /// 目标服务的 `ServiceKey::NAME`。
    pub name: &'static str,
}
/// `InternalGet` 返回的类型擦除服务值及其提供点。
#[derive(Clone)]
pub struct ServiceValue {
    /// 保存 `Arc<K::Value>` 的类型擦除层，读取端按键 `K` 恢复内部 `Arc`。
    pub value: Arc<dyn Any + Send + Sync>,
    /// 提供该值的上下文，用于返回句柄的 `definition` 和后续依赖解析。
    pub definition: Context,
}
/// 服务读取瀑布；监听器接收 `ServiceAccess`，返回 `ServiceValue`。
///
/// 默认终点执行服务访问授权和绑定定位，中间件可经由 `Next` 调用该终点。
pub struct InternalGet;
impl Event for InternalGet {
    /// 服务读取事件的名称。
    const NAME: &'static str = "internal/get";
    /// 请求上下文和目标服务名称。
    type Args = ServiceAccess;
    /// 读取成功时的值及其提供上下文。
    type Output = ServiceValue;
    /// 该事件采用同步瀑布派发。
    type Kind = WaterfallKind;
}
/// 服务修改瀑布；参数为访问描述与类型擦除后的 `Arc<K::Value>`。
///
/// 默认终点替换当前绑定的值，成功返回 `()`；状态错误通过瀑布的 `Result` 传播。
pub struct InternalSet;
impl Event for InternalSet {
    /// 服务修改事件的名称。
    const NAME: &'static str = "internal/set";
    /// 提供方访问上下文及传入的新值。
    type Args = (ServiceAccess, Arc<dyn Any + Send + Sync>);
    /// 修改成功的返回值。
    type Output = ();
    /// 该事件采用同步瀑布派发。
    type Kind = WaterfallKind;
}

pub(crate) struct ImplData {
    pub uid: u64,
    pub key: TypeId,
    pub name: &'static str,
    pub realm: Realm,
    pub scope: Arc<Scope>,
    pub value: AnyValue,
    pub check: Option<Check>,
    pub dependents: HashSet<FiberId>,
    pub waiters: Vec<Waker>,
}

impl Context {
    /// T-COE-2、S-SVC-1：在当前隔离域提供键 `K` 的值，返回管理该绑定的句柄。
    ///
    /// `value` 通过 `Arc` 共享，登记效应归属于当前上下文纤程。
    /// 同键同域已有绑定、名称冲突或上下文已经释放时返回错误。
    /// 提供方处于 `Active` 时立即通知消费者，加载中的提供方在激活后通知。
    #[track_caller]
    pub fn provide<K: ServiceKey>(&self, value: Arc<K::Value>) -> Result<Provision<K>> {
        self.provide_checked::<K>(value, None)
    }

    /// S-SVC-4：提供带可用性谓词 `check` 的值，返回该绑定的管理句柄。
    ///
    /// 登记规则与 `provide` 相同；依赖解析在锁外调用 `check`。
    /// `true` 允许消费者使用该绑定，`false` 使依赖处于待满足状态；
    /// 谓词发生 panic 时记录提供方错误，并采用待满足状态。
    /// 谓词捕获的状态变化后，可调用返回句柄的 `notify` 触发重新解析。
    #[track_caller]
    pub fn provide_with_check<K: ServiceKey>(
        &self,
        value: Arc<K::Value>,
        check: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Provision<K>> {
        self.provide_checked::<K>(value, Some(Arc::new(check)))
    }

    #[track_caller]
    fn provide_checked<K: ServiceKey>(
        &self,
        value: Arc<K::Value>,
        check: Option<Check>,
    ) -> Result<Provision<K>> {
        let key = TypeId::of::<K>();
        let realm = self.resolve_realm(key);
        let owner = self.scope.fiber;
        let scope = self.scope.clone();
        let kernel = self.kernel.clone();
        let location = Location::caller();
        let slot = Arc::new(std::sync::OnceLock::new());
        let result = slot.clone();
        let effect = self.effect(format!("ctx.provide(\"{}\")", K::NAME), move |fx| {
            let (id, active) = {
                let mut state = kernel.state.lock();
                if state.names.get(K::NAME).is_some_and(|old| *old != key) {
                    return Err(Error::new(format!(
                        "service name \"{}\" belongs to another key",
                        K::NAME
                    ))
                    .at(location));
                }
                if let Some(old) = state.store.get(&(key, realm)) {
                    let provider = state.impls[*old].scope.fiber;
                    let name = crate::fiber::name_locked(&state, provider);
                    return Err(Error::new(format!(
                        "service \"{}\" has been registered at <{}>",
                        K::NAME,
                        name
                    ))
                    .at(location));
                }
                state.names.insert(K::NAME, key);
                state.impl_counter += 1;
                let uid = state.impl_counter;
                let id = state.impls.insert(ImplData {
                    uid,
                    key,
                    name: K::NAME,
                    realm,
                    scope,
                    value: Arc::new(value),
                    check,
                    dependents: HashSet::new(),
                    waiters: Vec::new(),
                });
                state.store.insert((key, realm), id);
                state.fibers[owner].provisions.push(id);
                (id, state.fibers[owner].state == FiberState::Active)
            };
            result.set(id).expect("provision effect runs once");
            let cleanup = kernel.clone();
            fx.defer_async(move || {
                let task = withdraw(&cleanup, id);
                async move {
                    task.await;
                    Ok(())
                }
            })?;
            if active {
                notify(&kernel, key, realm);
            }
            Ok(())
        })?;
        Ok(Provision {
            effect,
            binding: *slot.get().expect("provision initialized"),
            marker: PhantomData,
        })
    }

    /// T-CTX-2、T-CTX-3、S-SVC-6：按当前上下文的授权路径读取键 `K`。
    ///
    /// 组件上下文沿纤程链逐层定位，每层先查同域自供，再查该层的已提交绑定；
    /// 该键的显式隔离层会截断祖先声明继承。根上下文直接读取当前域的实时绑定。
    /// 成功返回 `Scoped<K>`；授权路径缺失、声明绑定为空或中间件返回类型冲突时返回错误。
    /// 调用会同步派发 `InternalGet`，用户中间件在内核锁外执行。
    #[track_caller]
    pub fn get<K: ServiceKey>(&self) -> Result<Scoped<K>> {
        let context = self.clone();
        let location = Location::caller();
        let resolved = self.waterfall::<InternalGet>(
            ServiceAccess {
                context: self.clone(),
                name: K::NAME,
            },
            move || context.get_value::<K>().map_err(|error| error.at(location)),
        )?;
        let value = resolved
            .value
            .downcast_ref::<Arc<K::Value>>()
            .ok_or_else(|| Error::new("service interceptor returned a different value type"))?
            .clone();
        Ok(Scoped {
            value,
            caller: self.clone(),
            definition: resolved.definition,
            requester: self.clone(),
        })
    }

    fn get_value<K: ServiceKey>(&self) -> Result<ServiceValue> {
        let state = self.kernel.state.lock();
        let key = TypeId::of::<K>();
        let located = if state.fibers[self.scope.fiber].parent_scope.is_none() {
            state
                .store
                .get(&(key, self.resolve_realm(key)))
                .copied()
                .map(|id| (None, Some(id)))
        } else {
            locate(&state, &self.scope, key)
        };
        let id = match located {
            Some((_, Some(id))) => id,
            Some((Some(_), None)) => {
                return Err(Error::new(format!(
                    "cannot get required service \"{}\" in inactive context",
                    K::NAME
                ))
                .at(Location::caller()));
            }
            _ => {
                return Err(Error::new(format!(
                    "cannot get property \"{}\" without inject",
                    K::NAME
                ))
                .at(Location::caller()));
            }
        };
        Ok(ServiceValue {
            value: state.impls[id].value.clone(),
            definition: Context {
                kernel: self.kernel.clone(),
                scope: state.impls[id].scope.clone(),
            },
        })
    }

    /// S-SVC-3：从任意上下文反射查询键 `K` 在当前隔离域的实时绑定。
    ///
    /// 提供方处于 `Active` 时返回 `Some(Scoped<K>)`，绑定缺失或其他状态返回 `None`。
    /// 查询条件为实时绑定和提供方状态；依赖声明授权、已提交视图由 `get` 处理，
    /// 可用性谓词由生命周期依赖解析执行。
    pub fn lookup<K: ServiceKey>(&self) -> Option<Scoped<K>> {
        let state = self.kernel.state.lock();
        let key = TypeId::of::<K>();
        let id = *state.store.get(&(key, self.resolve_realm(key)))?;
        (state.fibers[state.impls[id].scope.fiber].state == FiberState::Active)
            .then(|| scoped::<K>(&state, id, self.clone()))
    }

    /// 返回共享内核中全部隔离域的当前绑定总数。
    ///
    /// 加载中提供方已经登记的绑定计入总数，撤回时绑定随实时表移除。
    pub fn service_count(&self) -> usize {
        self.kernel.state.lock().store.len()
    }
}

fn scoped<K: ServiceKey>(state: &State, id: ImplId, caller: Context) -> Scoped<K> {
    let value = state.impls[id]
        .value
        .downcast_ref::<Arc<K::Value>>()
        .expect("service value follows key")
        .clone();
    Scoped {
        value,
        definition: Context {
            kernel: caller.kernel.clone(),
            scope: state.impls[id].scope.clone(),
        },
        requester: caller.clone(),
        caller,
    }
}

pub(crate) fn injected_meta<K: ServiceKey>(context: &Context) -> K::Meta {
    let metadata = {
        let state = context.kernel.state.lock();
        locate(&state, &context.scope, TypeId::of::<K>())
            .and_then(|(fiber, _)| fiber)
            .and_then(|id| {
                state.fibers[id]
                    .inject
                    .entries
                    .iter()
                    .find(|entry| entry.key == TypeId::of::<K>())
                    .map(|entry| entry.meta.clone())
            })
    };
    metadata
        .and_then(|value| value.downcast_ref::<K::Meta>().cloned())
        .unwrap_or_default()
}

/// S-SVC-6、S-SVC-9：同域自供优先，隔离层截断祖先规格查找。
pub(crate) fn locate(
    state: &State,
    start: &Arc<Scope>,
    key: TypeId,
) -> Option<(Option<FiberId>, Option<ImplId>)> {
    let realm = scope_realm(start, key);
    let mut cursor = start.clone();
    let mut fiber_id = start.fiber;
    loop {
        let fiber = &state.fibers[fiber_id];
        if let Some(id) = fiber.provisions.iter().rev().find(|id| {
            state
                .impls
                .get(**id)
                .is_some_and(|binding| binding.key == key && binding.realm == realm)
        }) {
            return Some((None, Some(*id)));
        }
        while !matches!(cursor.layer, Layer::Fiber | Layer::Root) || cursor.fiber != fiber_id {
            if matches!(cursor.layer, Layer::Isolate(k, _) if k == key) {
                return None;
            }
            cursor = cursor.parent.clone()?;
        }
        if let Some((_, binding)) = fiber.committed.iter().find(|(name, _)| *name == key) {
            return Some((Some(fiber_id), Some(*binding)));
        }
        if fiber.inject.entries.iter().any(|entry| entry.key == key) {
            return Some((Some(fiber_id), None));
        }
        let parent = fiber.parent_scope.as_ref()?;
        fiber_id = parent.fiber;
        cursor = parent.clone();
    }
}

fn scope_realm(start: &Arc<Scope>, key: TypeId) -> Realm {
    let mut scope = Some(start);
    while let Some(node) = scope {
        if let Layer::Isolate(k, realm) = node.layer
            && k == key
        {
            return realm;
        }
        scope = node.parent.as_ref();
    }
    Realm::DEFAULT
}

#[track_caller]
pub(crate) fn set<K: ServiceKey>(kernel: &Arc<Kernel>, id: ImplId, value: Arc<K::Value>) -> Result {
    let scope = kernel
        .state
        .lock()
        .impls
        .get(id)
        .map(|binding| binding.scope.clone());
    let Some(scope) = scope else {
        return Err(Error::new(format!(
            "cannot set property \"{}\" without provide",
            K::NAME
        ))
        .at(Location::caller()));
    };
    let context = Context {
        kernel: kernel.clone(),
        scope,
    };
    let kernel = kernel.clone();
    let value: AnyValue = Arc::new(value);
    context.waterfall::<InternalSet>(
        (
            ServiceAccess {
                context: context.clone(),
                name: K::NAME,
            },
            value.clone(),
        ),
        move || replace_value(&kernel, id, value.clone(), K::NAME),
    )
}

fn replace_value(kernel: &Arc<Kernel>, id: ImplId, value: AnyValue, name: &'static str) -> Result {
    let old = {
        let mut state = kernel.state.lock();
        let Some(binding) = state.impls.get(id) else {
            return Err(Error::new(format!(
                "cannot set property \"{}\" without provide",
                name
            )));
        };
        if state.store.get(&(binding.key, binding.realm)) != Some(&id) {
            return Err(Error::new(format!(
                "cannot set property \"{}\" without provide",
                name
            )));
        }
        std::mem::replace(&mut state.impls[id].value, value)
    };
    drop(old);
    Ok(())
}

/// T-COE-3、S-SVC-2：先撤回实时绑定，依赖清理结束后释放自供视图。
fn withdraw(kernel: &Arc<Kernel>, id: ImplId) -> Task {
    let (key, realm) = {
        let mut state = kernel.state.lock();
        let binding = &state.impls[id];
        let pair = (binding.key, binding.realm);
        if state.store.get(&pair) == Some(&id) {
            state.store.remove(&pair);
        }
        pair
    };
    notify(kernel, key, realm);
    let kernel = kernel.clone();
    let mut yielded = false;
    Box::pin(poll_fn(move |cx| {
        if !yielded {
            yielded = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let removed = {
            let mut state = kernel.state.lock();
            let Some(binding) = state.impls.get_mut(id) else {
                return Poll::Ready(());
            };
            if !binding.dependents.is_empty() {
                if !binding.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                    binding.waiters.push(cx.waker().clone());
                }
                return Poll::Pending;
            }
            let provider = binding.scope.fiber;
            state.fibers[provider].provisions.retain(|old| *old != id);
            state.impls.remove(id)
        };
        drop(removed);
        Poll::Ready(())
    }))
}

pub(crate) fn notify_binding(kernel: &Arc<Kernel>, id: ImplId) {
    let pair = kernel
        .state
        .lock()
        .impls
        .get(id)
        .map(|value| (value.key, value.realm));
    if let Some((key, realm)) = pair {
        notify(kernel, key, realm);
    }
}

/// T-COE-4、T-COE-5、S-SVC-5：只刷新同域内声明了变化键的纤程。
fn notify(kernel: &Arc<Kernel>, key: TypeId, realm: Realm) {
    let fibers = {
        let state = kernel.state.lock();
        state
            .fibers
            .iter()
            .filter_map(|(id, fiber)| {
                let scope = fiber.scope.as_ref()?;
                (fiber.inject.entries.iter().any(|entry| entry.key == key)
                    && scope_realm(scope, key) == realm)
                    .then(|| (id, scope.clone()))
            })
            .collect::<Vec<_>>()
    };
    for (id, _scope) in fibers {
        refresh_keys(kernel, id, Some(key));
    }
    let event = {
        let state = kernel.state.lock();
        let root = state
            .fibers
            .values()
            .find(|fiber| fiber.parent_scope.is_none())
            .expect("root fiber exists");
        let name = state
            .impls
            .values()
            .find(|binding| binding.key == key)
            .expect("notified binding retained")
            .name;
        let value = state
            .store
            .get(&(key, realm))
            .map(|id| state.impls[*id].value.clone());
        (
            root.scope
                .as_ref()
                .expect("root context initialized")
                .clone(),
            ServiceChange { name, value },
        )
    };
    let context = Context {
        kernel: kernel.clone(),
        scope: event.0,
    };
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
        context.emit_filtered::<InternalService>(
            &event.1,
            Some(&|listener| listener.resolve_realm(key) == realm),
        );
    })) {
        kernel.report(context.scope.fiber, Error::panic(payload));
    }
}

pub(crate) fn refresh(kernel: &Arc<Kernel>, id: FiberId) {
    refresh_keys(kernel, id, None);
}

fn refresh_keys(kernel: &Arc<Kernel>, id: FiberId, key: Option<TypeId>) {
    let candidates = {
        let state = kernel.state.lock();
        let fiber = &state.fibers[id];
        if fiber.state == FiberState::Disposed {
            return;
        }
        let scope = fiber.scope.as_ref().expect("fiber context initialized");
        fiber
            .inject
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| key.is_none_or(|key| entry.key == key))
            .map(|(index, entry)| {
                let pair = (entry.key, scope_realm(scope, entry.key));
                let binding = state.store.get(&pair).copied();
                let active = binding.is_some_and(|binding| {
                    state.fibers[state.impls[binding].scope.fiber].state == FiberState::Active
                });
                let check = binding.and_then(|binding| state.impls[binding].check.clone());
                let provider = binding.map(|binding| state.impls[binding].scope.clone());
                (index, pair, binding, active, check, provider)
            })
            .collect::<Vec<_>>()
    };
    let checked = candidates
        .into_iter()
        .map(|(index, pair, binding, active, check, provider)| {
            let available = active
                && check.is_none_or(|check| match catch_unwind(AssertUnwindSafe(|| check())) {
                    Ok(value) => value,
                    Err(payload) => {
                        kernel.report(
                            provider.as_ref().expect("checked provider exists").fiber,
                            Error::panic(payload),
                        );
                        false
                    }
                });
            (index, pair, binding, active, available)
        })
        .collect::<Vec<_>>();
    {
        let mut state = kernel.state.lock();
        if state.fibers[id].state == FiberState::Disposed {
            return;
        }
        let length = state.fibers[id].inject.entries.len();
        state.fibers[id].bindings.resize(length, None);
        for (index, pair, binding, active, available) in checked {
            if state.fibers[id]
                .inject
                .entries
                .get(index)
                .is_none_or(|entry| entry.key != pair.0)
            {
                continue;
            }
            let current = state.store.get(&pair).copied();
            let current_active = current.is_some_and(|binding| {
                state.fibers[state.impls[binding].scope.fiber].state == FiberState::Active
            });
            if current == binding && current_active == active {
                state.fibers[id].bindings[index] = binding.filter(|_| available);
            }
        }
        crate::fiber::enqueue_locked(&mut state, id);
    }
    crate::fiber::drain(kernel);
}

pub(crate) fn epoch_ids(state: &State, id: FiberId) -> Option<Vec<u64>> {
    let fiber = &state.fibers[id];
    if fiber.bindings.len() != fiber.inject.entries.len() {
        return None;
    }
    fiber
        .bindings
        .iter()
        .map(|id| id.and_then(|id| state.impls.get(id).map(|binding| binding.uid)))
        .collect()
}

/// S-FBR-9：加载开始时固定绑定，并登记具体绑定上的消费者。
pub(crate) fn commit_locked(state: &mut State, id: FiberId) {
    let fiber = &state.fibers[id];
    let bindings = fiber
        .inject
        .entries
        .iter()
        .zip(&fiber.bindings)
        .filter_map(|(entry, binding)| binding.map(|id| (entry.key, id)))
        .collect::<Vec<_>>();
    for (_, binding) in &bindings {
        let binding = &mut state.impls[*binding];
        if binding.scope.fiber != id {
            binding.dependents.insert(id);
        }
    }
    state.fibers[id].committed = bindings;
}

/// T-COE-6、S-FBR-12：提供方保留资源至全部已提交消费者清理结束。
pub(crate) fn wait_dependents(kernel: Arc<Kernel>, id: FiberId) -> Task {
    Box::pin(poll_fn(move |cx| {
        let mut state = kernel.state.lock();
        let bindings = state.fibers[id].provisions.clone();
        let mut ready = true;
        for id in bindings {
            let binding = &mut state.impls[id];
            if binding.dependents.is_empty() {
                continue;
            }
            ready = false;
            if !binding.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                binding.waiters.push(cx.waker().clone());
            }
        }
        if ready {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }))
}

pub(crate) fn release_committed(kernel: &Arc<Kernel>, id: FiberId) {
    let waiters = {
        let mut state = kernel.state.lock();
        let bindings = std::mem::take(&mut state.fibers[id].committed);
        let mut waiters = Vec::new();
        for (_, binding) in bindings {
            let binding = &mut state.impls[binding];
            binding.dependents.remove(&id);
            if binding.dependents.is_empty() {
                waiters.append(&mut binding.waiters);
            }
        }
        waiters
    };
    for waiter in waiters {
        waiter.wake();
    }
}

pub(crate) fn status_changed(kernel: &Arc<Kernel>, id: FiberId) {
    let pairs = {
        let state = kernel.state.lock();
        state.fibers[id]
            .provisions
            .iter()
            .filter_map(|id| {
                let binding = &state.impls[*id];
                let pair = (binding.key, binding.realm);
                (state.store.get(&pair) == Some(id)).then_some(pair)
            })
            .collect::<Vec<_>>()
    };
    for (key, realm) in pairs {
        notify(kernel, key, realm);
    }
}
