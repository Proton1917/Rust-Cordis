//! ADR-0039、DOC-12 §7：共享真实服务实现，记录可观察的状态变化与最终析构。

use cordis_sdk::{Context, Result, Service};
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

#[cordis_sdk::interface(name = "test.resource-counter", version = "1.0")]
pub trait ResourceCounter: Send + Sync {
    fn add(&self, amount: u64) -> Result<u64>;
}

struct Counter {
    value: AtomicU64,
    released: Arc<AtomicUsize>,
}

impl ResourceCounter for Counter {
    fn add(&self, amount: u64) -> Result<u64> {
        Ok(self.value.fetch_add(amount, Ordering::SeqCst) + amount)
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

/// ADR-0039：服务对象通过实际 Context 和接口方法表取得独立引用与库租约。
pub fn tracked(
    context: &Context,
    value: u64,
    released: Arc<AtomicUsize>,
) -> Result<Service<dyn ResourceCounter>> {
    context.service::<dyn ResourceCounter>(Arc::new(Counter {
        value: AtomicU64::new(value),
        released,
    }))
}
