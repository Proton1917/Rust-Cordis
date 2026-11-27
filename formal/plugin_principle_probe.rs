// Complete production methods are inserted at the marked locations. The
// adapters record constructor fields and outbound RPC arguments. Real kernel
// scheduling, registry ownership and cleanup run in tests/plugin_principle.rs.
#![allow(dead_code)]

use std::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context as PollContext, Poll, Wake, Waker},
};

#[derive(Debug)]
struct Error;
impl Error {
    fn new(_: &str) -> Self {
        Self
    }
}
#[derive(Debug)]
struct ValidationError;
impl From<ValidationError> for Error {
    fn from(_: ValidationError) -> Self {
        Self
    }
}
type Result<T = ()> = std::result::Result<T, Error>;
type Task<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type RawConfig = Arc<dyn Any + Send + Sync>;

mod identity {
    use super::*;
    // @SOURCE:ALLOCATE@
}

#[derive(Clone, Copy)]
struct Context {
    identity: u64,
}
#[derive(Clone)]
struct Inject(Vec<(String, bool)>);
impl Inject {
    fn new() -> Self {
        Self(Vec::new())
    }
}
enum ComponentId {
    Closure(u64),
}
type Resolve = dyn Fn(RawConfig) -> Result<RawConfig> + Send + Sync;
type Apply = dyn Fn(Context, RawConfig) -> Task<Result> + Send + Sync;
struct ComponentDef {
    id: ComponentId,
    name: Arc<str>,
    inject: Inject,
    resolve: Arc<Resolve>,
    apply: Arc<Apply>,
}
impl ComponentDef {
    // @SOURCE:NEW@
    // @SOURCE:VALIDATED@
}

#[derive(Clone)]
struct ComponentInfo {
    name: String,
}
#[derive(Clone)]
struct Connection(u64);
struct RemoteComponent {
    plugin: String,
    info: ComponentInfo,
    library: Option<u64>,
    connection: Connection,
}
struct Generation {
    component: Arc<RemoteComponent>,
    id: u64,
    config: Arc<u64>,
    inherited: Option<Arc<u64>>,
}
mod payload {
    pub struct Apply<'a> {
        pub plugin: String,
        pub component: String,
        pub library: Option<u64>,
        pub context: u64,
        pub config: &'a u64,
        pub inherited: Option<&'a u64>,
    }
}
#[derive(Clone)]
struct Request {
    plugin: String,
    component: String,
    library: Option<u64>,
    context: u64,
    config: u64,
    inherited: Option<u64>,
}
fn value(request: &payload::Apply<'_>) -> Result<Request> {
    Ok(Request {
        plugin: request.plugin.clone(),
        component: request.component.clone(),
        library: request.library,
        context: request.context,
        config: *request.config,
        inherited: request.inherited.copied(),
    })
}
fn decode(response: &bool) -> Result<()> {
    if *response { Ok(()) } else { Err(Error) }
}
struct Response(bool);
impl Response {
    fn value(&self) -> &bool {
        &self.0
    }
}
struct Options {
    apply_timeout: u64,
    dispose_timeout: u64,
}
struct ManagerCore {
    options: Options,
    connection: Connection,
    response: bool,
    seen: Mutex<Option<(u64, String, Request, u64)>>,
}
struct Manager(Arc<ManagerCore>);
impl Manager {
    async fn lifecycle_request(
        &self,
        connection: &Connection,
        method: &str,
        request: Request,
        timeout: u64,
    ) -> Result<Response> {
        let previous = self.0.seen.lock().unwrap().replace((
            connection.0,
            method.to_owned(),
            request,
            timeout,
        ));
        assert!(previous.is_none(), "one apply must produce one request");
        Ok(Response(self.0.response))
    }
    // @SOURCE:APPLY_GENERATION@
}

struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}
fn run<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Noop));
    let mut context = PollContext::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("probe adapter unexpectedly suspended"),
    }
}
fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "-".into(), |value| value.to_string())
}

fn main() {
    for case in 0..192usize {
        let implementation = case % 2;
        let raw = [0usize, 9, 35][case / 2 % 3];
        let context = [13, 29][case / 6 % 2];
        let dependencies = match case / 12 % 4 {
            0 => vec![],
            1 => vec![("alpha@1".to_owned(), true)],
            2 => vec![("beta@2".to_owned(), false)],
            _ => vec![("α@1".to_owned(), false), ("β@2".to_owned(), true)],
        };
        let name = [
            "",
            "fresh.extension/Ω",
            "cordis-supervisor",
            "web-owner-new",
        ][case / 48];
        let observed = Arc::new(Mutex::new(None));
        let callback = observed.clone();
        let definition =
            ComponentDef::new(name, Inject(dependencies), move |ctx, config: usize| {
                callback
                    .lock()
                    .unwrap()
                    .replace((ctx.identity, config, implementation));
                async { Ok(()) }
            });
        let bad = (definition.resolve)(Arc::new("different raw type".to_owned())).is_err();
        let config = (definition.resolve)(Arc::new(raw)).unwrap();
        run((definition.apply)(Context { identity: context }, config)).unwrap();
        let (seen_context, seen_config, seen_implementation) = observed.lock().unwrap().unwrap();
        let dependencies = definition
            .inject
            .0
            .iter()
            .map(|(key, required)| format!("{key}={required}"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "constructor\t{case}\t{}|{dependencies}|{seen_context}|{seen_config}|{seen_implementation}|{bad}",
            definition.name
        );
    }
    for case in 0..512usize {
        let plugin = ["plugin/α", "new.application"][case % 2].to_owned();
        let component = ["component/β", "worker"][case / 2 % 2].to_owned();
        let generation = Generation {
            component: Arc::new(RemoteComponent {
                plugin,
                info: ComponentInfo { name: component },
                library: [None, Some(37)][case / 4 % 2],
                connection: Connection([97, 101][case / 64 % 2]),
            }),
            id: [41, 53][case / 8 % 2],
            config: Arc::new([67, 71][case / 16 % 2]),
            inherited: [None, Some(83)][case / 32 % 2].map(Arc::new),
        };
        let manager = Manager(Arc::new(ManagerCore {
            options: Options {
                apply_timeout: [103, 107][case / 128 % 2],
                dispose_timeout: 109,
            },
            connection: Connection(113),
            response: case / 256 % 2 == 0,
            seen: Mutex::new(None),
        }));
        let response = run(manager.apply_generation(&generation)).is_ok();
        let (connection, method, request, timeout) = manager.0.seen.lock().unwrap().take().unwrap();
        println!(
            "remote\t{case}\t{connection}|{method}|{}|{}|{}|{}|{}|{}|{timeout}|{response}",
            request.plugin,
            request.component,
            optional(request.library),
            request.context,
            request.config,
            optional(request.inherited)
        );
    }
    for (start, initial) in [0, 1, u64::MAX - 2, u64::MAX - 1, u64::MAX]
        .into_iter()
        .enumerate()
    {
        let counter = AtomicU64::new(initial);
        for step in 0..3 {
            let result = identity::allocate(&counter);
            println!(
                "identity\t{}\t{}|{}",
                start * 3 + step,
                optional(result),
                counter.load(Ordering::Relaxed)
            );
        }
    }
}
