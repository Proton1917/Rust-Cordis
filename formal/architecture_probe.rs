//! Exact supervisor provenance methods over std Arc/Weak/Any/Mutex primitives.
#![allow(dead_code)]
use std::{
    any::Any,
    collections::HashMap,
    ops::Deref,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Text(String),
    Integer(u64),
    Null,
    Map(Vec<(Value, Value)>),
}
#[derive(Debug)]
struct Failure {
    code: u32,
    message: String,
}
type Result<T> = std::result::Result<T, Failure>;
fn error(code: u32, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}
struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}
type Dynamic = Arc<dyn Any + Send + Sync>;
#[derive(Clone)]
struct Context {
    metadata: HashMap<String, Dynamic>,
    trusted: Option<Dynamic>,
    current: bool,
}
impl Context {
    fn named_meta(&self, interface: &str) -> Option<Dynamic> {
        self.metadata.get(interface).cloned()
    }
    fn extension<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.trusted
            .clone()
            .and_then(|value| value.downcast::<T>().ok())
    }
    fn logger(&self) -> Logger {
        Logger
    }
    fn is_current(&self) -> bool {
        self.current
    }
}
struct Logger;
impl Logger {
    fn error(&self, _message: String) {}
}
struct TrustedCall {
    fields: Vec<(Value, Value)>,
}
#[derive(Clone)]
struct Connection {
    id: u64,
    alive: bool,
}
impl Connection {
    fn id(&self) -> u64 {
        self.id
    }
    fn is_alive(&self) -> bool {
        self.alive
    }
}
struct Component {
    connection: Connection,
}
struct Instance {
    name: String,
    group: String,
    plugin: String,
}
struct Core {
    context: Context,
    contexts: Mutex<HashMap<u64, Weak<ViewRecord>>>,
    stopping: AtomicBool,
    exported: AtomicBool,
}
struct Manager(Arc<Core>);
struct Generation {
    context: Context,
    component: Arc<Component>,
    instance: Arc<Instance>,
    core: Weak<Core>,
}
struct ViewRecord {
    generation: Weak<Generation>,
    context: Mutex<Option<Context>>,
}
struct ViewLease {
    id: u64,
    context: Context,
    generation: Arc<Generation>,
    record: Arc<ViewRecord>,
}
impl Deref for ViewLease {
    type Target = Generation;
    fn deref(&self) -> &Generation {
        &self.generation
    }
}
struct CallContext {
    connection: u64,
    context: u64,
    instance: String,
    group: String,
    plugin: String,
}
mod implementation {
    use super::*;
    impl Manager {
        // @SOURCE:SCOPED_METADATA@
        // @SOURCE:GENERATION@
        // @SOURCE:REQUEST_ORIGIN@
    }
    impl Generation {
        // @SOURCE:IS_CURRENT@
        // @SOURCE:ACTIVE@
    }
    // @SOURCE:CALL_ORIGIN@
}
fn text(value: &str) -> Value {
    Value::Text(value.into())
}
fn context(metadata: Option<Dynamic>, trusted: bool, current: bool, identity: u64) -> Context {
    let fields = vec![
        (text("cordis.caller"), Value::Integer(identity + 900)),
        (text("cordis.caller"), Value::Integer(identity + 901)),
        (text("origin"), Value::Integer(identity + 902)),
    ];
    let mut indexed = HashMap::new();
    if let Some(value) = metadata {
        indexed.insert("service@1".into(), value);
    }
    indexed.insert(
        "alternate@2".into(),
        Arc::new(Value::Map(vec![
            (text("other.user"), Value::Integer(identity + 700)),
            (text("cordis.caller"), Value::Integer(identity + 701)),
        ])) as Dynamic,
    );
    Context {
        metadata: indexed,
        trusted: trusted.then(|| Arc::new(TrustedCall { fields }) as Dynamic),
        current,
    }
}
fn enabled(mask: usize, bit: usize) -> bool {
    mask & (1 << bit) != 0
}
fn core(context: Context, stopping: bool, exported: bool) -> Arc<Core> {
    Arc::new(Core {
        context,
        contexts: Mutex::new(HashMap::new()),
        stopping: AtomicBool::new(stopping),
        exported: AtomicBool::new(exported),
    })
}
fn render(value: Value) -> String {
    let Value::Map(fields) = value else {
        panic!("metadata result is not a map")
    };
    fields
        .into_iter()
        .map(|(key, value)| {
            let key = match key {
                Value::Text(key) => format!("s:{key}"),
                Value::Integer(key) => format!("n:{key}"),
                _ => panic!("key domain"),
            };
            let Value::Integer(value) = value else {
                panic!("value domain")
            };
            format!("{key}={value}")
        })
        .collect::<Vec<_>>()
        .join(";")
}
fn main() {
    let keys = vec![
        text("cordis."),
        text("cordis.caller"),
        text("cordis.caller.extra"),
        text("cordis"),
        text("cordisExtra"),
        text(""),
        text("用户"),
        Value::Integer(7),
    ];
    for kind in 0..5 {
        for mask in 0..256 {
            for trusted in [false, true] {
                let fields = keys
                    .iter()
                    .enumerate()
                    .filter(|(id, _)| enabled(mask, *id))
                    .map(|(id, key)| (key.clone(), Value::Integer(id as u64 + 10)))
                    .collect();
                let metadata: Option<Dynamic> = match kind {
                    0 => None,
                    1 => Some(Arc::new(Value::Null)),
                    2 => Some(Arc::new(Value::Map(fields))),
                    3 => Some(Arc::new(Value::Integer(1))),
                    _ => Some(Arc::new(123u32)),
                };
                let view_context = context(metadata, trusted, true, 0);
                let manager_context = context(
                    Some(Arc::new(Value::Map(vec![(
                        text("root.user"),
                        Value::Integer(800),
                    )]))),
                    true,
                    false,
                    1000,
                );
                let generation_context = context(
                    Some(Arc::new(Value::Map(vec![(
                        text("generation.user"),
                        Value::Integer(1800),
                    )]))),
                    true,
                    false,
                    2000,
                );
                let core = core(manager_context, false, false);
                let generation = Arc::new(Generation {
                    context: generation_context,
                    component: Arc::new(Component {
                        connection: Connection {
                            id: 11,
                            alive: true,
                        },
                    }),
                    instance: Arc::new(Instance {
                        name: "31".into(),
                        group: "41".into(),
                        plugin: "51".into(),
                    }),
                    core: Arc::downgrade(&core),
                });
                let record = Arc::new(ViewRecord {
                    generation: Arc::downgrade(&generation),
                    context: Mutex::new(Some(view_context.clone())),
                });
                let view = ViewLease {
                    id: 23,
                    context: view_context,
                    generation,
                    record,
                };
                let manager = Manager(core);
                for (query, interface) in
                    ["service@1", "alternate@2", "missing@3"].iter().enumerate()
                {
                    let result = match manager.scoped_metadata(&view, interface) {
                        Ok(value) => render(value),
                        Err(failure) => format!("error:{}", failure.code),
                    };
                    let id = ((kind * 256 + mask) * 2 + usize::from(trusted)) * 3 + query;
                    println!("metadata\t{id}\t{result}");
                }
            }
        }
    }
    for (stored_index, stored) in [1u64, 23, u64::MAX].into_iter().enumerate() {
        for (query_index, query) in [1u64, 23, u64::MAX, 0].into_iter().enumerate() {
            for mask in 0..512 {
                let generation_context = context(None, true, enabled(mask, 3), 2000);
                let view_context = context(None, true, !enabled(mask, 3), 0);
                let manager_context = context(None, true, !enabled(mask, 3), 1000);
                let core = core(manager_context, enabled(mask, 6), enabled(mask, 7));
                let generation = Arc::new(Generation {
                    context: generation_context,
                    component: Arc::new(Component {
                        connection: Connection {
                            id: 11,
                            alive: enabled(mask, 4),
                        },
                    }),
                    instance: Arc::new(Instance {
                        name: "31".into(),
                        group: "41".into(),
                        plugin: "51".into(),
                    }),
                    core: if enabled(mask, 5) {
                        Arc::downgrade(&core)
                    } else {
                        Weak::new()
                    },
                });
                let record = Arc::new(ViewRecord {
                    generation: if enabled(mask, 1) {
                        Arc::downgrade(&generation)
                    } else {
                        Weak::new()
                    },
                    context: Mutex::new(enabled(mask, 2).then_some(view_context)),
                });
                if enabled(mask, 0) {
                    core.contexts.lock().insert(stored, Arc::downgrade(&record));
                }
                let connection = Connection {
                    id: if enabled(mask, 8) { 11 } else { 12 },
                    alive: true,
                };
                let result = match Manager(core).request_origin(&connection, query) {
                    Ok(origin) => format!(
                        "{},{},{},{},{}",
                        origin.connection,
                        origin.context,
                        origin.instance,
                        origin.group,
                        origin.plugin
                    ),
                    Err(failure) => format!("error:{}", failure.code),
                };
                let id = (stored_index * 4 + query_index) * 512 + mask;
                println!("origin\t{id}\t{result}");
            }
        }
    }
}
