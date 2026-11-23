//! Host-owned admission lasts until a native notification future is destroyed.

use cordis_abi::{Error, Result};
use parking_lot::Mutex;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

#[derive(Default)]
struct Usage {
    active: usize,
    bytes: usize,
}

pub(crate) struct Admission {
    max_active: usize,
    max_bytes: usize,
    usage: Mutex<Usage>,
}

impl Admission {
    pub fn new(max_active: usize, max_bytes: usize) -> Result<Arc<Self>> {
        if max_active == 0 || max_bytes == 0 {
            return Err(Error::new(2, "原生回调数量与字节额度需要为正值"));
        }
        Ok(Arc::new(Self {
            max_active,
            max_bytes,
            usage: Mutex::new(Usage::default()),
        }))
    }

    pub fn acquire(self: &Arc<Self>, bytes: usize) -> Result<Permit> {
        let mut usage = self.usage.lock();
        let total = usage.bytes.checked_add(bytes);
        if usage.active >= self.max_active || total.is_none_or(|total| total > self.max_bytes) {
            return Err(Error::new(8, "原生回调执行额度已满"));
        }
        usage.active += 1;
        usage.bytes = total.expect("checked byte sum");
        Ok(Permit {
            admission: self.clone(),
            bytes,
        })
    }
}

pub(crate) struct Permit {
    admission: Arc<Admission>,
    bytes: usize,
}

impl Permit {
    pub fn run<F: Future>(self, future: F) -> Admitted<F> {
        Admitted {
            future: Box::pin(future),
            _permit: self,
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut usage = self.admission.usage.lock();
        usage.active -= 1;
        usage.bytes -= self.bytes;
    }
}

// Struct fields are destroyed in declaration order. The native future's
// synchronous destructor runs before capacity is returned, including cancellation
// before first poll and cancellation while a synchronous native poll is running.
pub(crate) struct Admitted<F> {
    future: Pin<Box<F>>,
    _permit: Permit,
}

impl<F: Future> Future for Admitted<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().future.as_mut().poll(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{NativeExecutor, RuntimeOwner};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn count_and_wire_bytes_are_admitted_atomically_and_returned_once() {
        assert!(Admission::new(0, 1).is_err());
        assert!(Admission::new(1, 0).is_err());
        let admission = Admission::new(2, 100).unwrap();
        let first = admission.acquire(60).unwrap();
        assert_eq!(admission.acquire(41).err().unwrap().code(), 8);
        let second = admission.acquire(40).unwrap();
        assert_eq!(admission.acquire(0).err().unwrap().code(), 8);
        assert_eq!(admission.usage.lock().bytes, 100);
        drop(first);
        assert!(admission.acquire(61).is_err());
        drop(second);
        let usage = admission.usage.lock();
        assert_eq!((usage.active, usage.bytes), (0, 0));
        drop(usage);
        let large = Admission::new(2, usize::MAX).unwrap();
        let _held = large.acquire(usize::MAX).unwrap();
        assert!(large.acquire(1).is_err());
    }

    struct BlockingCallback {
        admission: Arc<Admission>,
        started: Option<tokio::sync::oneshot::Sender<()>>,
        finish: std::sync::mpsc::Receiver<()>,
        dropped: Arc<AtomicBool>,
    }

    impl Future for BlockingCallback {
        type Output = Result<()>;
        fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            self.started.take().unwrap().send(()).unwrap();
            self.finish.recv().unwrap();
            Poll::Ready(Ok(()))
        }
    }

    impl Drop for BlockingCallback {
        fn drop(&mut self) {
            let usage = self.admission.usage.lock();
            assert_eq!((usage.active, usage.bytes), (1, 80));
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn cancellation_holds_capacity_through_native_poll_and_destruction() {
        let native = RuntimeOwner::new(1).unwrap();
        let executor = NativeExecutor::new(native.handle(), tokio::runtime::Handle::current());
        let admission = Admission::new(1, 100).unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finish, gate) = std::sync::mpsc::channel();
        let dropped = Arc::new(AtomicBool::new(false));
        let future = BlockingCallback {
            admission: admission.clone(),
            started: Some(started),
            finish: gate,
            dropped: dropped.clone(),
        };
        let task = executor.start(admission.acquire(80).unwrap().run(future), true);
        ready.await.unwrap();
        task.cancellation().unwrap().cancel();
        assert_eq!(admission.acquire(1).err().unwrap().code(), 8);
        assert!(!dropped.load(Ordering::Acquire));
        finish.send(()).unwrap();
        let _result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap();
        assert!(dropped.load(Ordering::Acquire));
        let usage = admission.usage.lock();
        assert_eq!((usage.active, usage.bytes), (0, 0));
    }

    #[test]
    fn unpolled_callback_is_destroyed_before_capacity_returns() {
        let admission = Admission::new(1, 100).unwrap();
        let (started, _ready) = tokio::sync::oneshot::channel();
        let (_finish, gate) = std::sync::mpsc::channel();
        let dropped = Arc::new(AtomicBool::new(false));
        let future = BlockingCallback {
            admission: admission.clone(),
            started: Some(started),
            finish: gate,
            dropped: dropped.clone(),
        };
        drop(admission.acquire(80).unwrap().run(future));
        assert!(dropped.load(Ordering::Acquire));
        let usage = admission.usage.lock();
        assert_eq!((usage.active, usage.bytes), (0, 0));
    }
}
