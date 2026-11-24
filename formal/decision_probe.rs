//! Compile the production decision module without a second implementation.
#[allow(dead_code)]
#[path = "../crates/cordis/src/lifecycle.rs"]
mod lifecycle;

fn bit(mask: usize, field: usize) -> bool {
    mask & (1 << field) != 0
}

fn main() {
    for m in 0..8 {
        println!(
            "idle\t{m}\t{:?}",
            lifecycle::idle(bit(m, 0), bit(m, 1), bit(m, 2))
        );
    }
    for m in 0..4 {
        println!("target\t{m}\t{:?}", lifecycle::target(bit(m, 0), bit(m, 1)));
    }
    for m in 0..128 {
        println!(
            "refresh\t{m}\t{:?}",
            lifecycle::refresh(
                bit(m, 0),
                bit(m, 1),
                || bit(m, 2),
                bit(m, 3),
                bit(m, 4),
                bit(m, 5),
                bit(m, 6)
            )
        );
    }
    for m in 0..32 {
        println!(
            "finish\t{m}\t{:?}",
            lifecycle::finish(bit(m, 0), || bit(m, 1), bit(m, 2), bit(m, 3), bit(m, 4))
        );
    }
    for m in 0..16 {
        println!(
            "settled\t{m}\t{}",
            lifecycle::settled(bit(m, 0), bit(m, 1), bit(m, 2), bit(m, 3))
        );
    }
    for m in 0..4 {
        println!(
            "current\t{m}\t{}",
            lifecycle::current(bit(m, 0), || bit(m, 1))
        );
    }
    for m in 0..8 {
        println!(
            "current_epoch\t{m}\t{}",
            lifecycle::current_epoch(bit(m, 0), || bit(m, 1), || bit(m, 2))
        );
    }
    for m in 0..4 {
        println!("cleanup\t{m}\t{}", lifecycle::cleanup(bit(m, 0), bit(m, 1)));
    }
    // Epoch equality walks an unbounded vector. Busy/dead/non-loading paths
    // must preserve the baseline's short circuit and never evaluate it.
    for m in 0..128 {
        let seen = std::cell::Cell::new(0);
        lifecycle::refresh(
            bit(m, 0),
            bit(m, 1),
            || {
                seen.set(seen.get() + 1);
                bit(m, 2)
            },
            bit(m, 3),
            bit(m, 4),
            bit(m, 5),
            bit(m, 6),
        );
        assert_eq!(seen.get(), usize::from(!bit(m, 0) && !bit(m, 1)));
    }
    for m in 0..32 {
        let seen = std::cell::Cell::new(0);
        lifecycle::finish(
            bit(m, 0),
            || {
                seen.set(seen.get() + 1);
                bit(m, 1)
            },
            bit(m, 2),
            bit(m, 3),
            bit(m, 4),
        );
        assert_eq!(seen.get(), usize::from(bit(m, 0)));
    }
    for m in 0..4 {
        let seen = std::cell::Cell::new(0);
        lifecycle::current(bit(m, 0), || {
            seen.set(seen.get() + 1);
            bit(m, 1)
        });
        assert_eq!(seen.get(), usize::from(bit(m, 0)));
    }
    for m in 0..8 {
        let same = std::cell::Cell::new(0);
        let expected = std::cell::Cell::new(0);
        lifecycle::current_epoch(
            bit(m, 0),
            || {
                same.set(same.get() + 1);
                bit(m, 1)
            },
            || {
                expected.set(expected.get() + 1);
                bit(m, 2)
            },
        );
        assert_eq!(same.get(), usize::from(bit(m, 0)));
        assert_eq!(expected.get(), usize::from(bit(m, 0) && bit(m, 1)));
    }
}
