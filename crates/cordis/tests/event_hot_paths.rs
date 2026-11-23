use cordis::{
    Context, Event, EventOptions, InternalDispatch, InternalGet, ServiceKey, SyncKind,
    WaterfallKind,
};
use cordis_testkit::TestExecutor;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    hint::black_box,
    sync::Arc,
    time::Instant,
};

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

impl CountingAllocator {
    fn record(pointer: *mut u8) -> *mut u8 {
        if !pointer.is_null() {
            let _ = ALLOCATIONS.try_with(|count| {
                if let Some(value) = count.get() {
                    count.set(Some(value + 1));
                }
            });
        }
        pointer
    }
}

// SAFETY: System receives the original allocation layouts and pointers. The
// counter uses allocation-free thread-local storage and counts this thread only.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::record(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::record(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        Self::record(unsafe { System.realloc(pointer, layout, size) })
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn count_allocations<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ALLOCATIONS.set(None);
        }
    }
    assert!(
        ALLOCATIONS.get().is_none(),
        "allocation counting cannot nest"
    );
    ALLOCATIONS.set(Some(0));
    let _reset = Reset;
    let result = operation();
    (result, ALLOCATIONS.get().unwrap())
}

struct Ping;
impl Event for Ping {
    const NAME: &'static str = "hot-path/ping";
    type Args = usize;
    type Output = ();
    type Kind = SyncKind;
}

struct Wrap;
impl Event for Wrap {
    const NAME: &'static str = "hot-path/wrap";
    type Args = usize;
    type Output = usize;
    type Kind = WaterfallKind;
}

struct Number;
impl ServiceKey for Number {
    const NAME: &'static str = "hot-path/number";
    type Value = usize;
    type Meta = ();
}

#[test]
fn unobserved_empty_typed_dispatch_has_no_allocations() {
    let host = TestExecutor::new();
    let context = Context::new(host.clone());
    let (_, allocations) = count_allocations(|| context.emit::<Ping>(&black_box(7)));
    assert_eq!(allocations, 0);
    host.finish(context.fiber().dispose()).unwrap();
}

#[test]
fn empty_waterfall_has_no_allocations() {
    let host = TestExecutor::new();
    let context = Context::new(host.clone());
    let value = black_box(7);
    // Capture a nonzero-sized value so boxing the endpoint would allocate.
    let (result, allocations) =
        count_allocations(|| context.waterfall::<Wrap>(value, move || Ok(value)));
    assert_eq!(result.unwrap(), value);
    assert_eq!(allocations, 0);
    host.finish(context.fiber().dispose()).unwrap();
}

#[test]
fn unintercepted_service_read_allocation_budget() {
    let host = TestExecutor::new();
    let context = Context::new(host.clone());
    context.provide::<Number>(Arc::new(7)).unwrap();
    let (value, allocations) = count_allocations(|| context.get::<Number>().unwrap());
    assert_eq!(*value, 7);
    // The service access may own its name; an empty event/middleware path adds
    // no further allocation. A future allocation-free lookup also meets this budget.
    assert!(
        allocations <= 1,
        "service read allocated {allocations} times"
    );
    host.finish(context.fiber().dispose()).unwrap();
}

fn measure(name: &str, mut operation: impl FnMut() -> usize) {
    let setting = |name, default| {
        std::env::var(name)
            .map(|value| value.parse::<usize>().expect("positive integer setting"))
            .unwrap_or(default)
    };
    let iterations = setting("CORDIS_BENCH_ITERATIONS", 100_000);
    let rounds = setting("CORDIS_BENCH_ROUNDS", 9);
    assert!(iterations > 0 && rounds > 0);
    for _ in 0..10_000 {
        black_box(operation());
    }
    let allocation_iterations = 1_000;
    let (_, allocations) = count_allocations(|| {
        for _ in 0..allocation_iterations {
            black_box(operation());
        }
    });
    for round in 1..=rounds {
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(operation());
        }
        let elapsed_ns = started.elapsed().as_nanos();
        println!(
            "measurement,{name},{round},{iterations},{elapsed_ns},{allocation_iterations},{allocations}"
        );
    }
}

#[test]
#[ignore = "run explicitly in release mode for timing"]
fn measure_event_hot_paths() -> Result<(), &'static str> {
    if cfg!(debug_assertions) {
        return Err("timing requires --release");
    }
    let host = TestExecutor::new();
    let context = Context::new(host.clone());
    context.provide::<Number>(Arc::new(7)).unwrap();
    measure("emit_empty", || {
        context.emit::<Ping>(&black_box(7));
        7
    });
    measure("waterfall_empty", || {
        let value = black_box(7);
        context.waterfall::<Wrap>(value, move || Ok(value)).unwrap()
    });
    measure("get_unintercepted", || *context.get::<Number>().unwrap());
    context
        .on::<Ping>(|value| {
            black_box(value);
            None
        })
        .unwrap();
    measure("emit_one_listener", || {
        context.emit::<Ping>(&black_box(7));
        7
    });
    context
        .on_waterfall::<InternalGet>(|_, next| next.run(), EventOptions::default())
        .unwrap();
    measure("get_one_middleware", || *context.get::<Number>().unwrap());
    context
        .on::<InternalDispatch>(|dispatch| {
            black_box(dispatch);
            None
        })
        .unwrap();
    measure("emit_observed", || {
        context.emit::<Ping>(&black_box(7));
        7
    });
    host.finish(context.fiber().dispose()).unwrap();
    Ok(())
}
