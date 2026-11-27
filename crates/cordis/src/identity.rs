//! Monotonic identities with a permanent exhaustion frontier.

use std::sync::atomic::{AtomicU64, Ordering};

/// Return the current frontier only when the next frontier is representable.
/// Callers reserve zero by initializing their counter to one. The maximum value
/// is the exhausted sentinel; a failed allocation leaves it unchanged.
pub(crate) fn allocate(counter: &AtomicU64) -> Option<u64> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_identity_stays_exhausted() {
        let counter = AtomicU64::new(u64::MAX - 2);
        assert_eq!(allocate(&counter), Some(u64::MAX - 2));
        assert_eq!(allocate(&counter), Some(u64::MAX - 1));
        for _ in 0..4 {
            assert_eq!(allocate(&counter), None);
            assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
        }
    }

    #[test]
    fn concurrent_allocations_are_unique_and_reserve_zero() {
        let counter = AtomicU64::new(1);
        let mut identities = std::thread::scope(|scope| {
            let workers = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..64)
                            .map(|_| allocate(&counter).expect("identity available"))
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().expect("allocator worker"))
                .collect::<Vec<_>>()
        });
        identities.sort_unstable();
        assert_eq!(identities, (1..=512).collect::<Vec<_>>());
        assert_eq!(counter.load(Ordering::Relaxed), 513);
    }
}
