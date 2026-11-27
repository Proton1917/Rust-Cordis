// Support types and input/output adapters for extracted production functions.
// check_kernel_refinement.py replaces each SOURCE marker with the current,
// uniquely matched Rust source text before compilation. The containers below
// model valid indices; they do not verify SlotMap allocation or Rust memory.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::any::Any;
use std::future::{Future, poll_fn};
use std::io::{self, BufRead};
use std::ops::{Index, IndexMut};
use std::panic::Location;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, Weak, atomic::AtomicBool};
use std::task::{Context as TaskContext, Poll, Waker};

#[path = "@LIFECYCLE_PATH@"]
mod lifecycle;
use lifecycle::{Action, FiberState, TargetDecision};

// @SOURCE:EPOCH@

type FiberId = usize;
type ImplId = usize;
type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Realm(u64);
impl Realm {
    const DEFAULT: Self = Self(0);
}

struct Arena<T>(Vec<T>);
impl<T> Arena<T> {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn insert(&mut self, value: T) -> usize {
        let id = self.0.len();
        self.0.push(value);
        id
    }
    fn get(&self, id: usize) -> Option<&T> {
        self.0.get(id)
    }
}
impl<T> Index<usize> for Arena<T> {
    type Output = T;
    fn index(&self, id: usize) -> &T {
        &self.0[id]
    }
}
impl<T> IndexMut<usize> for Arena<T> {
    fn index_mut(&mut self, id: usize) -> &mut T {
        &mut self.0[id]
    }
}

#[derive(Clone)]
struct Effects(Vec<usize>);
impl Effects {
    fn insert(&mut self, id: usize, _secondary: bool) {
        self.0.push(id);
    }
    fn iter(&self) -> std::slice::Iter<'_, usize> {
        self.0.iter()
    }
}

#[derive(Clone)]
struct Dependency {
    key: Arc<str>,
    required: bool,
    predicate: Option<Arc<u8>>,
    meta: Option<AnyArc>,
}
#[derive(Clone)]
struct Inject {
    entries: Vec<Dependency>,
}

#[derive(Clone)]
struct FiberData {
    state: FiberState,
    alive: bool,
    error: Option<Error>,
    target: Epoch,
    running_epoch: Epoch,
    inertia: bool,
    suspended: Option<()>,
    queued: bool,
    notifications: usize,
    effects: Effects,
    revision: u64,
    bindings: Vec<Option<ImplId>>,
    inject: Inject,
    provisions: Vec<ImplId>,
    committed: Vec<(Arc<str>, ImplId)>,
    parent_scope: Option<Arc<Scope>>,
}
impl FiberData {
    // @SOURCE:FIBER_METHODS@
}

struct Scope {
    parent: Option<Arc<Scope>>,
    fiber: FiberId,
    layer: Layer,
    lease: Option<Weak<Kernel>>,
    has_epoch_guard: bool,
}
enum Layer {
    Root,
    Fiber,
    Plain,
    Isolate(Arc<str>, Realm),
    Epoch(Arc<Epoch>),
    Intercept(Arc<str>, AnyArc),
}
#[derive(Clone)]
struct Context {
    kernel: Arc<Kernel>,
    scope: Arc<Scope>,
}
impl Context {
    // @SOURCE:CONTEXT_METHODS@
}

type AnyArc = Arc<dyn Any + Send + Sync>;
trait Merge: Clone + Default { fn merge(&self, next:&Self)->Self; }
impl Merge for Vec<u32> {
    fn merge(&self,next:&Self)->Self {let mut result=self.clone();result.extend(next);result}
}
trait ServiceKey {
    const NAME:&'static str;
    type Value: ?Sized + Send + Sync + 'static;
    type Meta: Merge + 'static;
}
struct Scoped<K: ServiceKey> {
    value: Arc<K::Value>, caller: Context, definition: Context, requester: Context,
}
impl<K: ServiceKey> Scoped<K> {
    // @SOURCE:FROM_BINDING@
    // @SOURCE:SCOPED_META@
}
trait Counter: Send + Sync { fn number(&self) -> u32; }
impl Counter for u32 { fn number(&self) -> u32 { *self } }
struct NumberKey;
impl ServiceKey for NumberKey {const NAME:&'static str="svc";type Value=u32;type Meta=Vec<u32>;}
struct TextKey;
impl ServiceKey for TextKey {const NAME:&'static str="svc";type Value=String;type Meta=Vec<u32>;}
struct DynamicKey;
impl ServiceKey for DynamicKey {const NAME:&'static str="svc";type Value=dyn Counter;type Meta=Vec<u32>;}

struct IdentityState {impl_counter:u64,names:HashMap<Arc<str>,Option<u32>>}
fn reserve_identity(state:&mut IdentityState)->Result<u64> {
    let key:Arc<str>="svc".into();
    let kind=Some(1);
    let location=Location::caller();
    // @SOURCE:RESERVE_IDENTITY@
    Ok(uid)
}
fn advance_notification(state:&mut State,key:&Arc<str>,realm:Realm) {
    // @SOURCE:ADVANCE_NOTIFICATION@
}

#[derive(Clone)]
struct Error(String);
impl Error {
    fn inactive() -> Self {
        Self("inactive".into())
    }
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
    fn at(self, _: &'static Location<'static>) -> Self {
        self
    }
}
type Result<T = ()> = std::result::Result<T, Error>;
struct Effect {
    kernel: Arc<Kernel>,
    id: usize,
    armed: Arc<AtomicBool>,
}
struct EffectData {
    owner: FiberId,
    refs: usize,
    armed: Arc<AtomicBool>,
    running: bool,
    cleaning: bool,
    steps: Vec<()>,
    label: String,
    children: Vec<usize>,
    parent: Option<usize>,
    outcome: Option<Result>,
    execution_error: Option<Error>,
    suspended: Option<()>,
    dispatch_error: Option<Error>,
    waiters: Vec<()>,
    location: &'static Location<'static>,
}
struct ImplData {
    key: Arc<str>,
    realm: Realm,
    uid: u64,
    scope: Arc<Scope>,
    dependents: HashSet<FiberId>,
    waiters: Vec<Waker>,
    notification_revision: u64,
    notification_exhausted: bool,
    active: bool,
    value: Arc<u8>,
}
impl ImplData {
    fn is_active(&self, _state: &State) -> bool {
        self.active
    }
    fn definition(&self, kernel: &Arc<Kernel>) -> Context {
        Context { kernel: kernel.clone(), scope: self.scope.clone() }
    }
}
struct ServiceValue {
    value: Arc<u8>,
    definition: Context,
}
struct State {
    fibers: Vec<FiberData>,
    impls: Arena<ImplData>,
    effects: Arena<EffectData>,
    store: HashMap<(Arc<str>, Realm), ImplId>,
}
struct Lock<T>(Mutex<T>);
impl<T> Lock<T> {
    fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}
struct Kernel {
    state: Lock<State>,
}
struct Fiber {
    id: FiberId,
}

mod reflect {
    use super::*;
    // @SOURCE:REFLECT_FUNCTIONS@
}
use reflect::locate;
// @SOURCE:CURRENT_CANDIDATE@

fn refresh_boundary(
    mut state: &mut State,
    id: FiberId,
) -> (FiberState, FiberState, Option<bool>, Action) {
    // @SOURCE:REFRESH_CORE@
    (old_state, new_state, transition, action)
}

fn finish_boundary(
    mut state: &mut State,
    id: FiberId,
    loading: bool,
) -> (FiberState, FiberState, Option<bool>, Action) {
    let fiber = Fiber { id };
    // @SOURCE:FINISH_CORE@
    let action = match next {
        Some(true) => Action::Load,
        Some(false) => Action::Unload,
        None => Action::Idle,
    };
    (old, new, next, action)
}

type Checked = (
    usize,
    (Arc<str>, Realm),
    Option<ImplId>,
    Option<u64>,
    bool,
    bool,
    Option<Arc<u8>>,
    Option<Arc<u8>>,
);
fn install_checked(state: &mut State, id: FiberId, checked: Vec<Checked>) {
    // @SOURCE:SNAPSHOT_LOOP@
}

fn bit(mask: usize, index: usize) -> bool {
    mask >> index & 1 == 1
}
fn phase(code: usize) -> FiberState {
    match code {
        0 => FiberState::Pending,
        1 => FiberState::Loading,
        2 => FiberState::Active,
        3 => FiberState::Failed,
        4 => FiberState::Disposed,
        _ => FiberState::Unloading,
    }
}
fn phase_code(value: FiberState) -> usize {
    match value {
        FiberState::Pending => 0,
        FiberState::Loading => 1,
        FiberState::Active => 2,
        FiberState::Failed => 3,
        FiberState::Disposed => 4,
        FiberState::Unloading => 5,
    }
}
fn epoch(code: usize) -> Epoch {
    match code {
        0 => Epoch::Inactive,
        1 => Epoch::Active {
            revision: 0,
            impls: vec![],
        },
        2 => Epoch::Active {
            revision: 1,
            impls: vec![7, 0],
        },
        _ => Epoch::Active {
            revision: u64::MAX,
            impls: vec![u64::MAX, 1],
        },
    }
}
fn epoch_code(value: &Epoch) -> usize {
    (0..4)
        .find(|&code| *value == epoch(code))
        .expect("unexpected epoch")
}
fn action_code(value: Action) -> usize {
    match value {
        Action::Keep => 0,
        Action::Idle => 1,
        Action::Load => 2,
        Action::Unload => 3,
    }
}
fn csv(values: &[usize]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
fn fiber(state: usize, mask: usize, target: usize, running: usize) -> FiberData {
    FiberData {
        state: phase(state),
        alive: bit(mask, 0),
        error: bit(mask, 1).then(|| Error::inactive()),
        target: epoch(target),
        running_epoch: epoch(running),
        inertia: bit(mask, 2),
        suspended: bit(mask, 3).then_some(()),
        queued: bit(mask, 4),
        notifications: 7,
        effects: Effects(if bit(mask, 5) { vec![0, 1, 2] } else { vec![] }),
        revision: 0,
        bindings: vec![],
        inject: Inject { entries: vec![] },
        provisions: vec![],
        committed: vec![],
        parent_scope: None,
    }
}
fn state(data: FiberData) -> State {
    State {
        fibers: vec![data.clone(), data],
        impls: Arena::new(),
        effects: Arena::new(),
        store: HashMap::new(),
    }
}
fn root_scope(owner: usize) -> Arc<Scope> {
    Arc::new(Scope {
        parent: None,
        fiber: owner,
        layer: Layer::Root,
        lease: None,
        has_epoch_guard: false,
    })
}
fn child_scope(owner: usize, parent: Arc<Scope>) -> Arc<Scope> {
    Arc::new(Scope {
        parent: Some(parent),
        fiber: owner,
        layer: Layer::Fiber,
        lease: None,
        has_epoch_guard: false,
    })
}
fn implementation(uid: u64, key: &str, realm: u64, owner: usize) -> ImplData {
    ImplData {
        key: key.into(),
        realm: Realm(realm),
        uid,
        scope: root_scope(owner),
        dependents: HashSet::new(),
        waiters: vec![],
        notification_revision: 1,
        notification_exhausted: false,
        active: true,
        value: Arc::new(0),
    }
}
fn resolved_state(mut data: FiberData, resolved: usize) -> State {
    let mut implementations = Arena::new();
    match epoch(resolved) {
        Epoch::Inactive => {
            data.inject.entries.push(Dependency {
                key: "missing".into(),
                required: true,
                predicate: None,                meta: None,
            });
            data.bindings.push(None);
        }
        Epoch::Active { revision, impls } => {
            data.revision = revision;
            for (index, uid) in impls.into_iter().enumerate() {
                let key = format!("k{index}");
                data.inject.entries.push(Dependency {
                    key: key.clone().into(),
                    required: uid != 0,
                    predicate: None,                    meta: None,
                });
                data.bindings.push(
                    (uid != 0).then(|| implementations.insert(implementation(uid, &key, 0, 1))),
                );
            }
        }
    }
    let mut result = state(data);
    result.impls = implementations;
    result
}
fn boundary_code(
    state: &State,
    boundary: (FiberState, FiberState, Option<bool>, Action),
) -> String {
    let data = &state.fibers[0];
    let (old, new, transition, action) = boundary;
    assert_eq!(new, data.state);
    csv(&[
        phase_code(data.state),
        epoch_code(&data.target),
        epoch_code(&data.running_epoch),
        data.alive as usize,
        data.error.is_some() as usize,
        data.inertia as usize,
        data.suspended.is_some() as usize,
        data.queued as usize,
        data.notifications,
        data.effects.0.len(),
        action_code(action),
        (old != new) as usize,
        (transition == Some(true)) as usize,
    ])
}
fn context(kernel: &Arc<Kernel>, kind: usize, depth: usize, captured: usize) -> Context {
    let mut ctx = Context {
        kernel: kernel.clone(),
        scope: root_scope(0),
    };
    let bind = |context: Context, epoch_code: usize| {
        context.derive(Layer::Epoch(Arc::new(epoch(epoch_code))))
    };
    ctx = match kind {
        0 => ctx,
        1 => bind(ctx, captured),
        2 => bind(bind(ctx, (captured + 1) % 4), captured),
        3 => {
            let ctx = bind(ctx, captured);
            Context {
                kernel: kernel.clone(),
                scope: child_scope(1, ctx.scope),
            }
        }
        4 => {
            let ctx = bind(ctx, (captured + 1) % 4);
            bind(
                Context {
                    kernel: kernel.clone(),
                    scope: child_scope(1, ctx.scope),
                },
                captured,
            )
        }
        _ => bind(bind(ctx, captured), captured),
    };
    for _ in 0..depth {
        ctx = ctx.derive(Layer::Plain);
    }
    ctx
}
fn dependencies(length: usize, mask: usize) -> Vec<Dependency> {
    (0..length)
        .map(|index| Dependency {
            key: format!("k{index}").into(),
            required: bit(mask, index),
            predicate: None,            meta: None,
        })
        .collect()
}
fn bindings(length: usize, base: usize, code: usize) -> Vec<Option<usize>> {
    (0..length)
        .map(|index| match code / base.pow(index as u32) % base {
            0 => None,
            1 => Some(0),
            2 => Some(1),
            _ => Some(9),
        })
        .collect()
}
fn pairs_code(pairs: &[(Arc<str>, usize)]) -> String {
    pairs
        .iter()
        .map(|(key, id)| format!("{key}:{id}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn execute(family: &str, input: &[usize]) -> String {
    match family {
        "refresh" | "finish" | "counter" => {
            let [phase, mask, target, running, last] = input[..5] else {
                panic!("bad boundary input")
            };
            let mut data = fiber(phase, mask, target, running);
            if family == "finish" {
                data.inertia = true;
            }
            if family == "counter" {
                data.notifications = if input[5] == 1 { usize::MAX - 1 } else { 0 };
            }
            let mut state = resolved_state(data, if family == "finish" { target } else { last });
            let boundary = if family != "finish" {
                refresh_boundary(&mut state, 0)
            } else {
                finish_boundary(&mut state, 0, last == 1)
            };
            boundary_code(&state, boundary)
        }
        "register" => {
            let [mask, target, running, kind, depth, captured] = *input else {
                panic!("bad admission input")
            };
            let mut data = fiber(2, mask, target, running);
            data.effects = Effects(if bit(mask, 1) { vec![0, 1, 2] } else { vec![] });
            let initial = data.effects.0.len();
            let kernel = Arc::new(Kernel {
                state: Lock(Mutex::new(state(data))),
            });
            let ctx = context(&kernel, kind, depth, captured);
            // Input records are well-owned by the view's selected fiber.
            {
                let mut state = kernel.state.lock();
                for _ in 0..initial {
                    state.effects.insert(EffectData {
                        owner: ctx.scope.fiber,
                        refs: 1,
                        armed: Arc::new(AtomicBool::new(true)),
                        running: true,
                        cleaning: false,
                        steps: vec![],
                        label: String::new(),
                        children: vec![],
                        parent: None,
                        outcome: None,
                        execution_error: None,
                        suspended: None,
                        dispatch_error: None,
                        waiters: vec![],
                        location: Location::caller(),
                    });
                }
            }
            let result = ctx.create_effect("probe".into(), Location::caller());
            let state = kernel.state.lock();
            if let Ok(effect) = &result {
                let record = &state.effects[effect.id];
                assert_eq!(record.owner, ctx.scope.fiber);
                assert_eq!(record.refs, 1);
                assert!(record.running && !record.cleaning && record.parent.is_none());
                assert!(state.fibers[ctx.scope.fiber].effects.0.contains(&effect.id));
            }
            csv(&[
                result.is_ok() as usize,
                state.fibers[ctx.scope.fiber].effects.0.len(),
                state.effects.0.len(),
            ])
        }
        "epoch" | "commit" => {
            let length = input[0];
            let code = input[1];
            let mut state = state(fiber(2, 1, 1, 1));
            let owner_mask = if family == "commit" { input[2] } else { 2 };
            state
                .impls
                .insert(implementation(1, "a", 0, bit(owner_mask, 0) as usize));
            state.impls.insert(implementation(
                u64::MAX,
                "b",
                0,
                bit(owner_mask, 1) as usize,
            ));
            state.fibers[0].bindings =
                bindings(length, if family == "epoch" { 4 } else { 3 }, code);
            state.fibers[0].inject.entries = dependencies(
                if family == "epoch" { input[3] } else { length },
                if family == "epoch" { input[2] } else { 0 },
            );
            if family == "epoch" {
                match reflect::epoch_ids(&state, 0) {
                    None => "none".into(),
                    Some(uids) => format!(
                        "some:{}",
                        uids.iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                }
            } else {
                reflect::commit_locked(&mut state, 0);
                format!(
                    "{}|{}",
                    pairs_code(&state.fibers[0].committed),
                    csv(&[
                        state.impls[0].dependents.contains(&0) as usize,
                        state.impls[1].dependents.contains(&0) as usize
                    ])
                )
            }
        }
        "locate" | "get_value" | "metadata" => {
            let mask = input[0];
            let realm = if bit(mask, 0) { 2 } else { 0 };
            let root = root_scope(0);
            let child = child_scope(1, root.clone());
            let mut state = state(fiber(2, 1, 1, 1));
            state.fibers[1].parent_scope = Some(root);
            for id in 0..6 {
                state.impls.insert(implementation(
                    (id + 1) as u64,
                    "svc",
                    if id == 3 { realm + 1 } else { realm },
                    if id == 2 || id == 5 { 0 } else { 1 },
                ));
            }
            if bit(mask, 2) {
                state.fibers[1].provisions.push(0);
            }
            if bit(mask, 3) {
                state.fibers[1].provisions.push(1);
            }
            if bit(mask, 9) {
                state.fibers[1].provisions.push(3);
            }
            // A removed arena record is skipped by the actual `get` in locate.
            state.fibers[1].provisions.push(99);
            if bit(mask, 4) {
                state.fibers[1].committed.push(("svc".into(), 4));
            }
            if bit(mask, 5) {
                state.fibers[1].inject.entries.push(Dependency {
                    key: "svc".into(),
                    required: bit(mask, 11),
                    predicate: None,                    meta: None,
                });
            }
            if bit(mask, 6) {
                state.fibers[0].committed.push(("svc".into(), 5));
            }
            if bit(mask, 7) {
                state.fibers[0].provisions.push(2);
            }
            if bit(mask, 8) {
                state.fibers[0].inject.entries.push(Dependency {
                    key: "svc".into(),
                    required: true,
                    predicate: None,                    meta: None,
                });
            }
            if family == "metadata" {
                for owner in 0..2 {
                    for entry in &mut state.fibers[owner].inject.entries {
                        entry.meta = Some(if bit(input[1], 1) {Arc::new(99_u32)}
                            else {Arc::new(vec![100_u32, owner as u32])});
                    }
                }
            }
            let kernel = Arc::new(Kernel {
                state: Lock(Mutex::new(state)),
            });
            let mut ctx = Context {
                kernel: kernel.clone(),
                scope: if family == "get_value" && bit(mask, 10) { kernel.state.lock().fibers[1].parent_scope.clone().unwrap() } else { child },
            };
            if bit(mask, 0) {
                ctx = ctx.derive(Layer::Isolate("svc".into(), Realm(2)));
            }
            if bit(mask, 1) {
                ctx = ctx.derive(Layer::Isolate("other".into(), Realm(3)));
            }
            if family == "metadata" {
                let flags=input[1];
                let mut caller=Context{kernel:kernel.clone(),scope:root_scope(0)};
                if bit(flags,0){caller=caller.derive(Layer::Intercept("svc".into(),Arc::new(vec![10_u32])));}
                if bit(flags,1){caller=caller.derive(Layer::Intercept("other".into(),Arc::new(99_u32)));}
                if bit(flags,2){caller=caller.derive(Layer::Intercept("svc".into(),Arc::new(vec![20_u32])));}
                if bit(flags,3){caller=caller.derive(Layer::Intercept("svc".into(),Arc::new(99_u32)));}
                let handle=Scoped::<NumberKey>{value:Arc::new(19),caller,definition:ctx.clone(),requester:ctx};
                let hook=std::panic::take_hook();
                std::panic::set_hook(Box::new(|_|{}));
                let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||handle.meta()));
                std::panic::set_hook(hook);
                return match result {Ok(values)=>format!("ok:{}",values.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")),Err(_)=>"panic".into()};
            }
            if family == "get_value" {
                if bit(mask, 2) {
                    kernel.state.lock().store.insert(("svc".into(), Realm(realm)), 0);
                }
                return match ctx.get_value(&"svc".into()) {
                    Ok(value) => format!("ok:{},{}", *value.value, value.definition.scope.fiber),
                    Err(error) if error.0.starts_with("optional service") => "optional".into(),
                    Err(error) if error.0.starts_with("cannot get required") => "required".into(),
                    Err(_) => "unauthorized".into(),
                };
            }
            match reflect::locate(&kernel.state.lock(), &ctx.scope, "svc") {
                None => "none".into(),
                Some((None, Some(id))) => format!("p:{id}"),
                Some((Some(owner), Some(id))) => format!("c:{owner}:{id}"),
                Some((Some(owner), None)) => format!("a:{owner}"),
                other => panic!("invalid location: {other:?}"),
            }
        }
        "release" => {
            let [length, code, before_mask, provision_mask] = *input else {
                panic!("bad release input")
            };
            let mut state = state(fiber(2, 1, 1, 1));
            for binding in 0..2 {
                let mut value = implementation((binding + 1) as u64, "svc", 0, 0);
                for consumer in 1..=2 {
                    if bit(before_mask, 2 * binding + consumer - 1) {
                        value.dependents.insert(consumer);
                    }
                }
                state.impls.insert(value);
                if bit(provision_mask, binding) {
                    state.fibers[0].provisions.push(binding);
                }
            }
            state.fibers[1].committed = dependencies(length, 0)
                .into_iter()
                .zip(bindings(length, 3, code))
                .filter_map(|(dependency, binding)| binding.map(|id| (dependency.key, id)))
                .collect();
            let kernel = Arc::new(Kernel {
                state: Lock(Mutex::new(state)),
            });
            let mut task = reflect::wait_dependents(kernel.clone(), 0);
            let mut context = TaskContext::from_waker(Waker::noop());
            let ready_before = matches!(task.as_mut().poll(&mut context), Poll::Ready(()));
            reflect::release_committed(&kernel, 1);
            let mut next_task = reflect::wait_dependents(kernel.clone(), 0);
            let ready_after = matches!(next_task.as_mut().poll(&mut context), Poll::Ready(()));
            let state = kernel.state.lock();
            assert!(state.fibers[1].committed.is_empty());
            csv(&[
                state.impls[0].dependents.contains(&1) as usize,
                state.impls[0].dependents.contains(&2) as usize,
                state.impls[1].dependents.contains(&1) as usize,
                state.impls[1].dependents.contains(&2) as usize,
                ready_before as usize,
                ready_after as usize,
            ])
        }
        "typed_handle" => {
            let [actual, expected, mask] = *input else {panic!("bad typed handle input")};
            let kernel=Arc::new(Kernel{state:Lock(Mutex::new(state(fiber(2,1,1,1))))});
            let caller=Context{kernel:kernel.clone(),scope:root_scope(bit(mask,0) as usize)};
            let definition=Context{kernel,scope:root_scope(bit(mask,1) as usize)};
            let value:AnyArc=match actual {
                0=>Arc::new(Arc::new(19_u32)),
                1=>Arc::new(Arc::new("19".to_owned())),
                2=>Arc::new(Arc::new(19_u32) as Arc<dyn Counter>),
                _=>Arc::new(19_u32),
            };
            match expected {
                0=>handle_observation::<NumberKey>(&caller,&definition,&value,|v|*v),
                1=>handle_observation::<TextKey>(&caller,&definition,&value,|v|v.parse().unwrap()),
                _=>handle_observation::<DynamicKey>(&caller,&definition,&value,|v|v.number()),
            }
        }
        "identity" => {
            let [index,existing]=*input else{panic!("bad identity input")};
            let mut state=IdentityState{impl_counter:boundary_counter(index),names:HashMap::from([("other".into(),Some(2))])};
            if existing==1{state.names.insert("svc".into(),Some(1));}
            let result=reserve_identity(&mut state);
            format!("{},{},{}",result.is_ok() as u8,state.impl_counter,state.names.len())
        }
        "notification" => {
            let [index,poisoned]=*input else{panic!("bad notification input")};
            let mut state=state(fiber(2,1,1,1));
            let mut value=implementation(1,"svc",0,0);
            value.notification_revision=boundary_counter(index);
            value.notification_exhausted=poisoned==1;
            state.impls.insert(value);
            state.store.insert(("svc".into(),Realm::DEFAULT),0);
            advance_notification(&mut state,&"svc".into(),Realm::DEFAULT);
            format!("{},{}",state.impls[0].notification_revision,state.impls[0].notification_exhausted as u8)
        }
        "snapshot" => {
            let mask = input[0];
            let mut state = state(fiber(2, 1, 1, 1));
            let predicate = Arc::new(0);
            let value = Arc::new(0);
            for _ in 0..2 {
                let mut implementation = implementation(1, "svc", 0, 1);
                implementation.notification_revision = if bit(mask, 1) { 2 } else { 1 };
                implementation.notification_exhausted = bit(mask, 8);
                implementation.active = !bit(mask, 2);
                implementation.value = if bit(mask, 4) {
                    Arc::new(1)
                } else {
                    value.clone()
                };
                state.impls.insert(implementation);
            }
            state.store.insert(
                ("svc".into(), Realm::DEFAULT),
                if bit(mask, 0) { 1 } else { 0 },
            );
            state.fibers[0].bindings = vec![Some(9)];
            state.fibers[0].inject.entries = vec![Dependency {
                key: if bit(mask, 5) { "other" } else { "svc" }.into(),
                required: true,
                predicate: Some(if bit(mask, 3) {
                    Arc::new(1)
                } else {
                    predicate.clone()
                }),
                meta: None,
            }];
            install_checked(
                &mut state,
                0,
                vec![(
                    0,
                    ("svc".into(), Realm::DEFAULT),
                    Some(0),
                    Some(1),
                    true,
                    bit(mask, 7),
                    Some(predicate),
                    (!bit(mask, 6)).then_some(value),
                )],
            );
            state.fibers[0].bindings[0]
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".into())
        }
        _ => panic!("unknown family: {family}"),
    }
}

fn boundary_counter(index:usize)->u64 {
    [0,1,u64::MAX-2,u64::MAX-1,u64::MAX,9_007_199_254_740_992][index]
}

fn handle_observation<K:ServiceKey>(caller:&Context, definition:&Context, value:&AnyArc,
    read:impl Fn(&K::Value)->u32)->String {
    let called=std::cell::Cell::new(0);
    let result=Scoped::<K>::from_binding(caller,value,||{called.set(called.get()+1);definition.clone()});
    match result {
        None=>format!("wrong:{}",called.get()),
        Some(result)=>format!("ok:{}",csv(&[read(&result.value) as usize,result.caller.scope.fiber,
            result.definition.scope.fiber,result.requester.scope.fiber,
            Arc::ptr_eq(&result.value,value.downcast_ref::<Arc<K::Value>>().unwrap()) as usize,called.get()])),
    }
}

fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("input read");
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 3);
        let input: Vec<usize> = fields[1]
            .split(',')
            .map(|value| value.parse().expect("numeric input"))
            .collect();
        println!(
            "{}\t{}\t{}",
            fields[0],
            fields[1],
            execute(fields[0], &input)
        );
    }
}
