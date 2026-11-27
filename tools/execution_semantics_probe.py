#!/usr/bin/env python3
"""Execute production native lease bodies and callback destruction order.

The lease probe uses real std atomics, HashMap, Arc and library/image locks.
Library and cache objects record destructor invocation; OS image loading and the
foreign ABI are exercised by the separate real-native integration suite. This is
finite source execution evidence, alongside the arbitrary-history Lean proof.
"""
from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / 'formal'
LEASE = ROOT / 'crates/cordis-host/src/lease.rs'
CALLBACK = ROOT / 'crates/cordis-host/src/callback.rs'
INPUTS = [LEASE, CALLBACK, Path(__file__).resolve()] + [FORMAL / p for p in
    ['NativeOwnership.lean', 'Concurrency.lean', 'AsyncSemantics.lean',
     'NativeVectors.lean', 'Execution.lean', 'Effects.lean', 'lakefile.toml', 'lean-toolchain']]


def run(command, *, cwd=ROOT, input=None):
    result = subprocess.run(command, cwd=cwd, input=input, text=True,
                            capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f'{command}:\n{result.stdout}\n{result.stderr}')
    return result.stdout


def block(source, marker):
    if source.count(marker) != 1:
        raise ValueError(f'expected one source block: {marker}')
    start = source.index(marker)
    opening = source.index('{', start)
    depth, end = 1, opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


def hashes():
    return {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in INPUTS}


def receipt(path, data):
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile('w', dir=path.parent, delete=False) as f:
        temporary = Path(f.name)
        json.dump(data, f, indent=2)
        f.write('\n')
        f.flush()
        os.fsync(f.fileno())
    os.replace(temporary, path)


PRELUDE = r'''
use std::sync::{Arc, OnceLock, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}};
use std::collections::HashMap;
use std::task::{Context, Poll, Waker};
use std::future::Future;
use std::pin::Pin;
#[derive(Clone, Debug)] struct Error;
impl Error { fn new(_: u32, _: impl Into<String>) -> Self { Self } }
type Result<T = ()> = std::result::Result<T, Error>;
struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}
impl<T: Default> Default for Mutex<T> { fn default() -> Self { Self::new(T::default()) } }
static LIBRARY_DROPS: AtomicUsize = AtomicUsize::new(0);
static COPY_DROPS: AtomicUsize = AtomicUsize::new(0);
static CLOSED: AtomicU64 = AtomicU64::new(0);
struct Library;
impl Drop for Library { fn drop(&mut self) { LIBRARY_DROPS.fetch_add(1, Ordering::SeqCst); } }
struct CacheCopy;
impl Drop for CacheCopy { fn drop(&mut self) { COPY_DROPS.fetch_add(1, Ordering::SeqCst); } }
#[derive(Clone, Copy, PartialEq, Eq)] enum UnloadPolicy { Never, WhenUnleased }
struct Image {
    library: Option<Library>, copy: Option<CacheCopy>, waiters: Vec<Waker>,
    descriptor_result: Option<Result<()>>,
}
struct Lease { id: u64, count: AtomicUsize, retired: AtomicBool, policy: UnloadPolicy, image: Mutex<Image> }
static LIBRARIES: OnceLock<Mutex<HashMap<u64, Arc<Lease>>>> = OnceLock::new();
static READY: Mutex<Vec<Arc<Lease>>> = Mutex::new(Vec::new());
fn libraries() -> &'static Mutex<HashMap<u64, Arc<Lease>>> { LIBRARIES.get_or_init(Mutex::default) }
struct Mapping { closed: bool }
static MAPPINGS: OnceLock<Mutex<HashMap<u64, Mapping>>> = OnceLock::new();
fn retired_mappings() -> &'static Mutex<HashMap<u64, Mapping>> { MAPPINGS.get_or_init(Mutex::default) }
fn periodic_mapping_cleanup(_: u64, cleanup: impl FnOnce()) { cleanup(); }
fn mapped_retired_images(_: &[Arc<Lease>]) -> Option<u64> { Some(0) }
'''

HARNESS = r'''
fn main() {
    use std::io::{self, BufRead};
    assert_eq!(usize::BITS, 64, "the finite source domain is the audited x86_64 ABI");
    for line in io::stdin().lock().lines() {
        let fields: Vec<u64> = line.unwrap().split_whitespace().map(|x| x.parse().unwrap()).collect();
        assert_eq!(fields.len(), 9);
        libraries().lock().clear(); READY.lock().clear();
        LIBRARY_DROPS.store(0, Ordering::SeqCst); COPY_DROPS.store(0, Ordering::SeqCst);
        let lease = Arc::new(Lease {
            id: 1, count: AtomicUsize::new(fields[0] as usize), retired: AtomicBool::new(fields[1] != 0),
            policy: if fields[2] == 0 { UnloadPolicy::Never } else { UnloadPolicy::WhenUnleased },
            image: Mutex::new(Image { library: Some(Library), copy: Some(CacheCopy), waiters: Vec::new(),
                descriptor_result: match fields[3] { 0 => None, 1 => Some(Err(Error)), _ => Some(Ok(())) } }),
        });
        libraries().lock().insert(1, lease.clone());
        match fields[4] { 0 => { let _ = acquire(1); }, 1 => { let _ = release(1); }, _ => lease.reap() }
        let image = lease.image.lock();
        assert_eq!([lease.count() as u64, u64::from(image.library.is_some()),
            u64::from(image.copy.is_some()), LIBRARY_DROPS.load(Ordering::SeqCst) as u64],
            fields[5..9], "production native lease differs from Lean: {:?}", fields);
        assert_eq!(COPY_DROPS.load(Ordering::SeqCst), usize::from(image.copy.is_none()));
        drop(image);
        if fields[4] == 2 {
            lease.reap();
            assert_eq!(LIBRARY_DROPS.load(Ordering::SeqCst) as u64, fields[8], "second reap repeats image destructor");
        }
    }
    callback_schedules();
}
struct Notice(AtomicUsize);
impl std::task::Wake for Notice { fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); } }
struct CheckedFuture { admission: Arc<Admission>, remaining: usize, dropped: Arc<AtomicBool> }
impl Future for CheckedFuture {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let usage = self.admission.usage.lock();
        assert_eq!((usage.active, usage.bytes), (1, 7), "poll lost its permit");
        drop(usage);
        if self.remaining == 0 { Poll::Ready(()) } else {
            self.remaining -= 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
impl Drop for CheckedFuture {
    fn drop(&mut self) {
        let usage = self.admission.usage.lock();
        assert_eq!((usage.active, usage.bytes), (1, 7), "permit returned before native destructor");
        assert!(!self.dropped.swap(true, Ordering::SeqCst), "native destructor repeated");
    }
}
fn callback_schedules() {
    for remaining in 0..4 {
        for polls in 0..5 {
            let admission = Admission::new(1, 7).unwrap();
            let dropped = Arc::new(AtomicBool::new(false));
            let mut future = Box::pin(admission.acquire(7).unwrap().run(CheckedFuture {
                admission: admission.clone(), remaining, dropped: dropped.clone(),
            }));
            let notice = Arc::new(Notice(AtomicUsize::new(0)));
            let waker = Waker::from(notice);
            let mut context = Context::from_waker(&waker);
            for _ in 0..polls {
                if future.as_mut().poll(&mut context).is_ready() { break; }
            }
            drop(future);
            assert!(dropped.load(Ordering::SeqCst));
            let usage = admission.usage.lock();
            assert_eq!((usage.active, usage.bytes), (0, 0));
        }
    }
}
'''


def source_program():
    lease = LEASE.read_text()
    callback = CALLBACK.read_text()
    lease_markers = ['pub fn count(&self)', 'pub fn retired(&self)', 'pub fn reap(&self)',
                     'pub fn acquire(id: u64)', 'pub fn release(id: u64)']
    callback_markers = ['struct Usage', 'pub(crate) struct Admission', 'impl Admission',
                        'pub(crate) struct Permit', 'impl Permit', 'impl Drop for Permit',
                        'pub(crate) struct Admitted<F>', 'impl<F: Future> Future for Admitted<F>']
    ls = [block(lease, m) for m in lease_markers]
    cs = [block(callback, m) for m in callback_markers]
    parts = PRELUDE + '\nimpl Lease {\n' + '\n'.join(ls[:3]) + '\n}\n' + '\n'.join(ls[3:])
    parts += '\n#[derive(Default)]\n' + '\n'.join(cs) + HARNESS
    records = [{ 'source': str(path.relative_to(ROOT)), 'marker': marker,
                 'sha256': hashlib.sha256(body.encode()).hexdigest() }
               for path, markers, bodies in [(LEASE, lease_markers, ls), (CALLBACK, callback_markers, cs)]
               for marker, body in zip(markers, bodies)]
    return parts, records


def check(lake, rustc):
    before = hashes()
    run([lake, 'build', 'NativeOwnership', 'Execution'], cwd=FORMAL)
    for module in ['Concurrency', 'AsyncSemantics']:
        run([lake, 'env', 'lean', '-o', f'.lake/build/lib/lean/{module}.olean', f'{module}.lean'], cwd=FORMAL)
    vectors = run([lake, 'env', 'lean', '--run', 'NativeVectors.lean'], cwd=FORMAL)
    rows = [tuple(map(int, line.split())) for line in vectors.splitlines()]
    expected = set(itertools.product([0, 1, 2, 4, (1 << 64)-2, (1 << 64)-1], [0, 1], [0, 1], [0, 1, 2], [0, 1, 2]))
    keys = [row[:5] for row in rows]
    if any(len(row) != 9 for row in rows) or len(keys) != len(set(keys)) or set(keys) != expected:
        raise ValueError('native vector domain must be complete, unique and exactly match its independent contract')
    program, extracts = source_program()
    mutants = {
        'lease_increment': ('count.checked_add(1)', 'count.checked_add(2)'),
        'lease_decrement': ('count.checked_sub(1)', 'count.checked_sub(2)'),
        'early_image_take': ('self.retired() && self.count() == 0', 'self.retired() && self.count() <= 1'),
        'permit_before_future': ('future: Pin<Box<F>>,\n    _permit: Permit,', '_permit: Permit,\n    future: Pin<Box<F>>,')
    }
    rejected = {}
    with tempfile.TemporaryDirectory(prefix='cordis-native-semantics-') as name:
        directory = Path(name)
        for label in ['production', *mutants]:
            source = program
            if label != 'production':
                old, new = mutants[label]
                if source.count(old) != 1:
                    raise ValueError(f'mutation target is not unique: {label}')
                source = source.replace(old, new)
            path, binary = directory / f'{label}.rs', directory / label
            path.write_text(source)
            run([rustc, '--edition=2024', '-Awarnings', str(path), '-o', str(binary)])
            completed = subprocess.run([str(binary)], input=vectors, text=True, capture_output=True, timeout=60)
            if label == 'production':
                if completed.returncode:
                    raise RuntimeError(completed.stderr)
            else:
                if not completed.returncode:
                    raise RuntimeError(f'negative control survived: {label}')
                rejected[label] = {'status': 'rejected', 'exit_code': completed.returncode}
    if before != hashes():
        raise RuntimeError('source/proof inputs changed during native source verification')
    return {'status': 'passed', 'lease_vectors': len(rows), 'callback_schedules': 20,
            'cases': len(rows) + 20, 'compiled_input_cases': len(rows) + 20,
            'source_sha256': before, 'source_extracts': extracts,
            'negative_controls': rejected,
            'adapters': ['std Mutex with parking_lot lock-return shape', 'Error values omit text',
                         'library and cache destructors count drops', 'mapping statistics are inert',
                         'controlled Future uses real Waker, Pin and Future polling'],
            'proof_scope': 'Concurrency proves arbitrary atomic histories for valid held-lease clones; AsyncSemantics proves microstep safety and conditional infinite-trace cleanup liveness.',
            'environment_contract': 'Atomic RMW follows per-object modification order; loads return preceding modifications; valid FFI clone retains a counted source owner; Rust/Tokio/OS implementation contracts remain explicit.'}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--lake', default='lake')
    parser.add_argument('--rustc', default='rustc')
    parser.add_argument('--evidence', type=Path)
    args = parser.parse_args()
    receipt(args.evidence, {'status': 'running'})
    try:
        result = check(shutil.which(args.lake) or args.lake, shutil.which(args.rustc) or args.rustc)
    except BaseException as error:
        receipt(args.evidence, {'status': 'failed', 'error': str(error)})
        raise
    receipt(args.evidence, result)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
