//! 实例授权沿用内核效应组合；最后一个部署所有者负责撤回。

use super::*;

pub(super) struct MountScope {
    pub(super) view: Context,
    manage: Option<Effect>,
    logs: Logs,
}

impl MountScope {
    pub(super) fn new(view: Context, manage: Option<Effect>, logs: Logs) -> Self {
        Self { view, manage, logs }
    }

    /// 正常提交或回退等待同一内核清理结果；等待被取消时 Drop 继续观察。
    pub(super) async fn release(mut self) -> Result<()> {
        let result = match &self.manage {
            Some(effect) => effect.dispose().await.map_err(kernel),
            None => Ok(()),
        };
        self.manage = None;
        result
    }
}

impl Drop for MountScope {
    fn drop(&mut self) {
        let Some(effect) = self.manage.take() else {
            return;
        };
        // dispose 同步撤销守卫，清理任务由内核持有，观察任务只报告结果。
        let cleanup = effect.dispose();
        let logs = self.logs.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    if let Err(problem) = cleanup.await {
                        logs.emit(0, "runtime", &format!("部署授权清理失败：{problem}"));
                    }
                });
            }
            Err(problem) => logs.emit(
                0,
                "runtime",
                &format!("部署授权已开始清理，运行时结束后无法等待结果：{problem}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{executor::Executor, status::Diagnostics};
    use cordis_supervisor::manager::ManagerOptions;
    use cordis_testkit::cleanup::TestGuard;
    use std::{path::Path, sync::Weak};
    use tokio::sync::{Notify, oneshot};

    async fn controller(case: &TestGuard) -> (Context, Controller, InstanceSettings) {
        let root = Context::new(Arc::new(Executor::new()));
        let manager = Arc::new(
            Manager::new(root.clone(), ManagerOptions::default())
                .await
                .unwrap(),
        );
        let configuration = config::parse(
            br#"
            [instance.subject]
            plugin = "subject"
            forwarder = true
            [instance.subject.grants]
            "cordis.manage@1" = ["*"]
        "#,
        )
        .unwrap();
        let settings = configuration.instance["subject"].clone();
        let project = case.directory("permission-lifecycle");
        let locations = Locations::create(&project, &configuration).unwrap();
        let store = Store::create(project.join("state")).unwrap();
        let diagnostics = Arc::new(Mutex::new(Diagnostics::default()));
        let (handle, _requests) = Handle::channel(Duration::from_secs(1));
        let logs = Logs(Arc::downgrade(&manager));
        let controller = Controller::new(
            manager,
            configuration,
            project.join("cordis.toml"),
            locations,
            store,
            Overlay::default(),
            ControlServices {
                diagnostics,
                handle,
                logs,
            },
        )
        .unwrap();
        (root, controller, settings)
    }

    async fn absent(view: &Context, keys: &[&str]) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while keys.iter().any(|key| view.lookup_named(*key).is_some()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("permission effects released by their owner");
    }

    #[tokio::test]
    async fn registration_failure_releases_prefix_and_preserves_existing_service() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "permissions-prefix");
        let (root, controller, settings) = controller(&case).await;
        let view = controller.mount_view(&settings).unwrap();
        let existing = view
            .provide_named(config::MANAGE_V2, Arc::new(17_u32))
            .unwrap();
        let problem = controller
            .provide_management(&settings, &view)
            .err()
            .unwrap();
        assert_eq!(problem.code, 1);
        assert!(problem.message.contains("has been registered"));
        absent(&view, &["cordis.forward@1", config::MANAGE]).await;
        assert_eq!(
            *view
                .get_named(config::MANAGE_V2)
                .unwrap()
                .downcast::<u32>()
                .unwrap(),
            17
        );
        existing.dispose().await.unwrap();
        controller.manager.shutdown().await.unwrap();
        root.fiber().dispose().await.unwrap();
        case.success();
    }

    #[tokio::test]
    async fn last_shared_permission_owner_releases_on_candidate_cancellation() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "permissions-cancel");
        let (root, controller, settings) = controller(&case).await;
        let view = controller.mount_view(&settings).unwrap();
        let manage = controller.provide_management(&settings, &view).unwrap();
        let scope = Arc::new(MountScope::new(
            view.clone(),
            manage,
            controller.logs.clone(),
        ));
        let candidate = scope.clone();
        drop(scope);
        assert!(view.lookup_named(config::MANAGE).is_some());
        assert!(view.lookup_named(config::MANAGE_V2).is_some());
        let (started, running) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _owned = candidate;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        running.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        absent(
            &view,
            &["cordis.forward@1", config::MANAGE, config::MANAGE_V2],
        )
        .await;
        controller.manager.shutdown().await.unwrap();
        root.fiber().dispose().await.unwrap();
        case.success();
    }

    #[tokio::test]
    async fn release_error_keeps_cleanup_complete_and_returns_first_error() {
        let root = Context::new(Arc::new(Executor::new()));
        let released = Arc::new(Mutex::new(Vec::new()));
        let effect = root
            .effect("instance-permissions", |scope| {
                scope.adopt(
                    root.provide_named("permission-first", Arc::new(1_u32))?
                        .effect(),
                )?;
                let sequence = released.clone();
                scope.adopt(root.effect("failed cleanup", |child| {
                    child.defer(move || {
                        sequence.lock().push("failure");
                        Err(cordis::Error::new("permission cleanup failed"))
                    })
                })?)?;
                scope.adopt(
                    root.provide_named("permission-last", Arc::new(2_u32))?
                        .effect(),
                )?;
                Ok(())
            })
            .unwrap();
        let scope = MountScope::new(root.clone(), Some(effect), Logs(Weak::new()));
        let problem = scope.release().await.unwrap_err();
        assert_eq!(problem.message, "permission cleanup failed");
        assert_eq!(*released.lock(), ["failure"]);
        assert!(root.lookup_named("permission-first").is_none());
        assert!(root.lookup_named("permission-last").is_none());
        assert!(root.fiber().effects().is_empty());
        root.fiber().dispose().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_release_keeps_kernel_cleanup_running_until_completion() {
        let root = Context::new(Arc::new(Executor::new()));
        let started = Arc::new(Notify::new());
        let proceed = Arc::new(Notify::new());
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let effect = root
            .effect("instance-permissions", |scope| {
                scope.adopt(
                    root.provide_named("permission-before-wait", Arc::new(1_u32))?
                        .effect(),
                )?;
                let (started, proceed, done) = (started.clone(), proceed.clone(), done.clone());
                scope.defer_async(move || async move {
                    started.notify_one();
                    proceed.notified().await;
                    done.store(true, std::sync::atomic::Ordering::Release);
                    Ok(())
                })
            })
            .unwrap();
        let owner = MountScope::new(root.clone(), Some(effect), Logs(Weak::new()));
        let task = tokio::spawn(owner.release());
        started.notified().await;
        assert!(!task.is_finished());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!done.load(std::sync::atomic::Ordering::Acquire));
        proceed.notify_one();
        absent(&root, &["permission-before-wait"]).await;
        assert!(done.load(std::sync::atomic::Ordering::Acquire));
        root.fiber().dispose().await.unwrap();
    }
}
