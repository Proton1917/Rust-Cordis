//! 运行根持有状态目录写权，清理真实结束后归还；调用者取消不转移该写权。

use super::*;
use std::fs::File;

pub(super) struct RuntimeRoot {
    context: Context,
    executor: tokio::runtime::Handle,
    claim: Option<(File, File)>,
}

impl RuntimeRoot {
    pub(super) fn new(claim: (File, File)) -> Self {
        Self {
            context: Context::new(Arc::new(Executor::new())),
            executor: tokio::runtime::Handle::current(),
            claim: Some(claim),
        }
    }

    pub(super) fn context(&self) -> &Context {
        &self.context
    }

    pub(super) async fn finish(mut self) -> Result<()> {
        let result = self.context.fiber().dispose().await.map_err(kernel);
        self.claim = None;
        result
    }
}

impl Drop for RuntimeRoot {
    fn drop(&mut self) {
        let Some(claim) = self.claim.take() else {
            return;
        };
        // Fiber::dispose 立即发起同一次内核清理。目录锁与等待任务共同存活，
        // 因而顶层效应的并行清理、等待者取消都不会提前开放下一次部署。
        let cleanup = self.context.fiber().dispose();
        self.executor.spawn(async move {
            let result = cleanup.await;
            drop(claim);
            if let Err(problem) = result {
                eprintln!("运行时取消后的根清理失败：{problem}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicBool, Ordering},
    };
    use tokio::sync::Notify;

    fn root(case: &TestGuard) -> (RuntimeRoot, PathBuf) {
        let directory = case.directory("runtime-root-owner");
        let owner = RuntimeRoot::new(crate::storage::claim(&directory).unwrap());
        (owner, directory)
    }

    fn blocked_cleanup(root: &RuntimeRoot) -> (Arc<Notify>, Arc<Notify>, Arc<AtomicBool>) {
        let started = Arc::new(Notify::new());
        let proceed = Arc::new(Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        let (signal, wait, done) = (started.clone(), proceed.clone(), completed.clone());
        root.context()
            .effect("owned-resource", |scope| {
                scope.defer_async(move || async move {
                    signal.notify_one();
                    wait.notified().await;
                    done.store(true, Ordering::Release);
                    Ok(())
                })
            })
            .unwrap();
        (started, proceed, completed)
    }

    async fn claim_after_cleanup(directory: &Path) -> (File, File) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(claim) = crate::storage::claim(directory) {
                    return claim;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("state directory claim released after cleanup")
    }

    async fn cleanup_started(started: &Notify) {
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("root starts its owned cleanup");
    }

    #[tokio::test]
    async fn cancelled_run_retains_claim_until_root_cleanup_finishes() {
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "root-cancel");
        let (root, directory) = root(&case);
        let (started, proceed, completed) = blocked_cleanup(&root);
        let task = tokio::spawn(async move {
            let _root = root;
            std::future::pending::<()>().await;
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        cleanup_started(&started).await;
        assert!(crate::storage::claim(&directory).is_err());
        assert!(!completed.load(Ordering::Acquire));
        proceed.notify_one();
        let next = claim_after_cleanup(&directory).await;
        assert!(completed.load(Ordering::Acquire));
        drop(next);
        case.success();
    }

    #[tokio::test]
    async fn cancelling_explicit_finish_preserves_the_same_cleanup_and_claim() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "root-finish-cancel");
        let (root, directory) = root(&case);
        let (started, proceed, completed) = blocked_cleanup(&root);
        let task = tokio::spawn(root.finish());
        cleanup_started(&started).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(crate::storage::claim(&directory).is_err());
        proceed.notify_one();
        let next = claim_after_cleanup(&directory).await;
        assert!(completed.load(Ordering::Acquire));
        drop(next);
        case.success();
    }

    #[tokio::test]
    async fn cleanup_error_waits_for_other_resources_before_returning_claim() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "root-cleanup-error");
        let (root, directory) = root(&case);
        let (started, proceed, completed) = blocked_cleanup(&root);
        let logger = root.context().logger();
        root.context()
            .effect("failing-resource", |scope| {
                scope.defer(|| Err(cordis::Error::new("resource cleanup failed")))
            })
            .unwrap();
        let task = tokio::spawn(root.finish());
        cleanup_started(&started).await;
        assert!(!task.is_finished());
        assert!(crate::storage::claim(&directory).is_err());
        proceed.notify_one();
        task.await.unwrap().unwrap();
        assert!(logger.buffer().iter().flat_map(|message| &message.args).any(|value| {
            matches!(value, LogValue::Error(error) if error.message.as_ref() == "resource cleanup failed")
        }));
        assert!(completed.load(Ordering::Acquire));
        let next = crate::storage::claim(&directory).unwrap();
        drop(next);
        case.success();
    }
}
