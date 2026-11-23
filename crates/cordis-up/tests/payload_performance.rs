//! Release-only DTO measurements. Encoding to bytes, transport, service execution
//! and input preparation are outside the measured roundtrip window.

use cordis_up::{
    Value,
    payload::{Invoke, InvokeResult},
};
use serde::Serialize;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    hint::black_box,
    time::Instant,
};

// Calls and requested bytes include successful alloc, alloc_zeroed and realloc
// requests; a realloc contributes its full new request size. Live bytes use the
// new-minus-old size, relative to the window entrance, including input frees.
// The peak measures requested live sizes, excluding allocator bookkeeping and
// temporary storage internal to System's realloc implementation.
#[derive(Clone, Copy, Default)]
struct Allocations {
    calls: usize,
    requested_bytes: usize,
    live_delta: i128,
    peak_additional_live_bytes: usize,
}

thread_local! {
    static ALLOCATIONS: Cell<Option<Allocations>> = const { Cell::new(None) };
}

struct CountingAllocator;

impl CountingAllocator {
    fn allocated(pointer: *mut u8, size: usize, old_size: usize) -> *mut u8 {
        if !pointer.is_null() {
            let _ = ALLOCATIONS.try_with(|current| {
                if let Some(mut count) = current.get() {
                    count.calls += 1;
                    count.requested_bytes += size;
                    count.live_delta += size as i128 - old_size as i128;
                    count.peak_additional_live_bytes = count
                        .peak_additional_live_bytes
                        .max(count.live_delta.max(0) as usize);
                    current.set(Some(count));
                }
            });
        }
        pointer
    }

    fn deallocated(size: usize) {
        let _ = ALLOCATIONS.try_with(|current| {
            if let Some(mut count) = current.get() {
                count.live_delta -= size as i128;
                current.set(Some(count));
            }
        });
    }
}

// SAFETY: System receives unchanged pointers and layouts. The counter accesses
// allocation-free thread-local state; another test thread cannot enter the window.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        CountingAllocator::allocated(unsafe { System.alloc(layout) }, layout.size(), 0)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        CountingAllocator::allocated(unsafe { System.alloc_zeroed(layout) }, layout.size(), 0)
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        CountingAllocator::allocated(
            unsafe { System.realloc(pointer, layout, size) },
            size,
            layout.size(),
        )
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        CountingAllocator::deallocated(layout.size());
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct CountingScope;

impl CountingScope {
    fn begin() -> Self {
        assert!(
            ALLOCATIONS.get().is_none(),
            "allocation windows cannot nest"
        );
        ALLOCATIONS.set(Some(Allocations::default()));
        Self
    }
}

impl Drop for CountingScope {
    fn drop(&mut self) {
        ALLOCATIONS.set(None);
    }
}

fn counted<T>(mut input: T, iterations: usize, transform: fn(T) -> T) -> (T, Allocations) {
    let scope = CountingScope::begin();
    for _ in 0..iterations {
        input = black_box(transform(black_box(input)));
    }
    let allocations = ALLOCATIONS.get().unwrap();
    drop(scope);
    (input, allocations)
}

fn timed<T>(mut input: T, iterations: usize, transform: fn(T) -> T) -> (T, u128) {
    let started = Instant::now();
    for _ in 0..iterations {
        input = black_box(transform(black_box(input)));
    }
    let elapsed_ns = started.elapsed().as_nanos();
    (input, elapsed_ns)
}

fn legacy_invoke(input: Invoke) -> Invoke {
    Value::serialized(&input).unwrap().deserialized().unwrap()
}

fn owned_invoke(input: Invoke) -> Invoke {
    Invoke::from_value_owned(input.into_value()).unwrap()
}

fn legacy_result(input: InvokeResult) -> InvokeResult {
    Value::serialized(&input).unwrap().deserialized().unwrap()
}

fn owned_result(input: InvokeResult) -> InvokeResult {
    InvokeResult::from_value_owned(input.into_value()).unwrap()
}

fn encoded<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).unwrap();
    bytes
}

struct Settings {
    rounds: usize,
    small_iterations: usize,
    large_iterations: usize,
}

struct Case<T> {
    label: &'static str,
    input: T,
    into_value: fn(T) -> Value,
    legacy: fn(T) -> T,
    owned: fn(T) -> T,
}

struct Sample {
    label: &'static str,
    payload_bytes: usize,
    round: usize,
    iterations: usize,
    allocation_iterations: usize,
}

fn record<T>(state: &mut Option<T>, sample: &Sample, path: &str, transform: fn(T) -> T) {
    let input = state.take().unwrap();
    let (input, allocations) = counted(input, sample.allocation_iterations, transform);
    let (input, elapsed_ns) = timed(input, sample.iterations, transform);
    *state = Some(input);
    println!(
        concat!(
            "{{\"benchmark\":\"{}\",\"path\":\"{}\",\"payload_bytes\":{},",
            "\"round\":{},\"iterations\":{},\"elapsed_ns\":{},",
            "\"allocation_iterations\":{},\"allocation_calls\":{},",
            "\"requested_bytes\":{},\"peak_additional_live_bytes\":{},",
            "\"final_live_delta_bytes\":{}}}"
        ),
        sample.label,
        path,
        sample.payload_bytes,
        sample.round,
        sample.iterations,
        elapsed_ns,
        sample.allocation_iterations,
        allocations.calls,
        allocations.requested_bytes,
        allocations.peak_additional_live_bytes,
        allocations.live_delta,
    );
}

fn run_case<T: Clone + Serialize>(case: Case<T>, payload_bytes: usize, settings: &Settings) {
    // All construction, clones and byte-level equivalence checks precede timing.
    let expected = encoded(&Value::serialized(&case.input).unwrap());
    assert_eq!(encoded(&(case.into_value)(case.input.clone())), expected);
    assert_eq!(encoded(&(case.legacy)(case.input.clone())), expected);
    assert_eq!(encoded(&(case.owned)(case.input.clone())), expected);
    let mut legacy = case.input.clone();
    let mut owned = case.input;
    for _ in 0..32 {
        legacy = black_box((case.legacy)(black_box(legacy)));
        owned = black_box((case.owned)(black_box(owned)));
    }
    let mut legacy = Some(legacy);
    let mut owned = Some(owned);
    for round in 1..=settings.rounds {
        let sample = Sample {
            label: case.label,
            payload_bytes,
            round,
            iterations: if payload_bytes == 64 {
                settings.small_iterations
            } else {
                settings.large_iterations
            },
            allocation_iterations: if payload_bytes == 64 { 1000 } else { 8 },
        };
        if round % 2 == 1 {
            record(&mut legacy, &sample, "legacy", case.legacy);
            record(&mut owned, &sample, "owned", case.owned);
        } else {
            record(&mut owned, &sample, "owned", case.owned);
            record(&mut legacy, &sample, "legacy", case.legacy);
        }
    }
    // Reused result buffers preserve the original request bytes after all rounds.
    assert_eq!(encoded(legacy.as_ref().unwrap()), expected);
    assert_eq!(encoded(owned.as_ref().unwrap()), expected);
}

fn setting(name: &str, default: usize) -> Result<usize, String> {
    let value = match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|error| format!("{name}: {error}"))?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(format!("{name}: {error}")),
    };
    if value == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(value)
}

#[test]
#[ignore = "run explicitly in release mode with --nocapture for DTO measurements"]
fn measure_payload_roundtrips() -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("DTO timing requires --release".into());
    }
    let settings = Settings {
        rounds: setting("CORDIS_PAYLOAD_BENCH_ROUNDS", 9)?,
        small_iterations: setting("CORDIS_PAYLOAD_BENCH_SMALL_ITERATIONS", 100_000)?,
        large_iterations: setting("CORDIS_PAYLOAD_BENCH_LARGE_ITERATIONS", 128)?,
    };
    for size in [64, 1024 * 1024, 4 * 1024 * 1024] {
        run_case(
            Case {
                label: "invoke_dto_roundtrip",
                input: Invoke {
                    object: 9,
                    method: "echo".into(),
                    args: vec![Value::Bytes(vec![73; size])],
                    refs: vec![],
                },
                into_value: Invoke::into_value,
                legacy: legacy_invoke,
                owned: owned_invoke,
            },
            size,
            &settings,
        );
        run_case(
            Case {
                label: "result_dto_roundtrip",
                input: InvokeResult {
                    value: Value::Bytes(vec![91; size]),
                    refs: vec![],
                },
                into_value: InvokeResult::into_value,
                legacy: legacy_result,
                owned: owned_result,
            },
            size,
            &settings,
        );
    }
    Ok(())
}
