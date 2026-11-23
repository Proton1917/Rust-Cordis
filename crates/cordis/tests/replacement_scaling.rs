//! 组件替换的同步调用与全部实例重新稳定的独立测量窗口。
//!
//! `CORDIS_REPLACEMENT_ROUNDS=5 cargo test --release -p cordis
//! --test replacement_scaling -- --ignored --nocapture`
//!
//! 在固定基线和候选源码上交替运行同一入口，保留每轮 JSON 行。
//! 挂载、初次加载、结果核对和释放均在计时窗口之外。

use cordis::{Clock, ComponentDef, Context, Fiber, Inject, Spawn, Task};
use futures_util::future::try_join_all;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::runtime::{Builder, Handle, Runtime};

struct BenchmarkHost {
    handle: Handle,
    started: Instant,
}

impl Spawn for BenchmarkHost {
    fn spawn(&self, task: Task) {
        self.handle.spawn(task);
    }
}

impl Clock for BenchmarkHost {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&self, duration: Duration) -> Task {
        Box::pin(async move { tokio::time::sleep(duration).await })
    }
}

fn measure(
    runtime: &Runtime,
    source_instances: usize,
    target_instances: usize,
) -> cordis::Result<(u128, u128)> {
    let host = Arc::new(BenchmarkHost {
        handle: runtime.handle().clone(),
        started: Instant::now(),
    });
    let root = Context::new(host);
    let source = ComponentDef::new("source", Inject::new(), |_, ()| async { Ok(()) });
    let target = ComponentDef::new("target", Inject::new(), |_, ()| async { Ok(()) });
    let source_fibers = (0..source_instances)
        .map(|_| root.mount_with(&source, ()))
        .collect::<cordis::Result<Vec<_>>>()?;
    let target_fibers = (0..target_instances)
        .map(|_| root.mount_with(&target, ()))
        .collect::<cordis::Result<Vec<_>>>()?;
    runtime.block_on(try_join_all(
        source_fibers
            .iter()
            .chain(&target_fibers)
            .map(Fiber::settled),
    ))?;

    // 单线程运行时在 block_on 中推进任务，同步窗口只包含 replace 的调用。
    let started = Instant::now();
    let migrated = root.registry().replace(&source, &target);
    let synchronous_replace_ns = started.elapsed().as_nanos();
    runtime.block_on(try_join_all(migrated.iter().map(Fiber::settled)))?;
    let replace_to_settled_ns = started.elapsed().as_nanos();

    assert_eq!(migrated, source_fibers);
    let mut expected = target_fibers;
    expected.extend(source_fibers);
    assert_eq!(root.registry().fibers(&target), expected);
    for fiber in &expected {
        assert!(fiber.is_settled());
        assert_eq!(fiber.component_id(), Some(target.id()));
    }
    runtime.block_on(root.fiber().dispose())?;
    assert!(root.registry().is_empty());
    Ok((synchronous_replace_ns, replace_to_settled_ns))
}

#[test]
#[ignore = "release-mode component replacement measurement"]
fn replacement_scaling() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("run replacement_scaling with --release".into());
    }
    let rounds = std::env::var("CORDIS_REPLACEMENT_ROUNDS")
        .unwrap_or_else(|_| "5".to_owned())
        .parse::<usize>()?;
    if rounds == 0 {
        return Err("CORDIS_REPLACEMENT_ROUNDS must be positive".into());
    }
    // Tokio 避免把 TestExecutor 的逐任务 Vec::retain 清理计入生命周期窗口。
    let runtime = Builder::new_current_thread().enable_time().build()?;
    for source_instances in [100, 1_000, 10_000] {
        for target_instances in [0, source_instances] {
            measure(&runtime, source_instances, target_instances)?;
            for round in 0..rounds {
                let (synchronous_replace_ns, replace_to_settled_ns) =
                    measure(&runtime, source_instances, target_instances)?;
                println!(
                    "{{\"scenario\":\"replacement_scaling\",\"source_instances\":{source_instances},\"target_instances\":{target_instances},\"round\":{round},\"synchronous_replace_ns\":{synchronous_replace_ns},\"replace_to_settled_ns\":{replace_to_settled_ns}}}"
                );
            }
        }
    }
    Ok(())
}
