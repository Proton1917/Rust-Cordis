//! DOC-15 §11.3、K-4：网页 Owner 撤回后释放其部署视图与动态组件定义。

use cordis::{Clock, Context, Spawn, Task};
use cordis_supervisor::manager::{Manager, ManagerOptions};
use cordis_up::Value;
use cordis_web_deployment::{Deployment, Spec};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

struct Executor(Instant);
impl Spawn for Executor {
    fn spawn(&self, task: Task) {
        tokio::spawn(task);
    }
}
impl Clock for Executor {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) -> Task {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// PR-6：尚待 HTTP 服务的真实 Owner 同样拥有视图，移除即归还该视图资源。
#[tokio::test]
async fn removing_owner_releases_its_context_resources() {
    let root = Context::new(Arc::new(Executor(Instant::now())));
    let manager = Arc::new(
        Manager::new(root.clone(), ManagerOptions::default())
            .await
            .unwrap(),
    );
    let deployment = Deployment::new(manager.clone(), Arc::new(|_| false)).unwrap();
    let resource = Arc::new(());
    let observed = Arc::downgrade(&resource);
    deployment
        .mount(Spec {
            view: manager.context().extend(resource),
            instance: "panel".into(),
            name: "panel".into(),
            version: "0.1.0".into(),
            assets: ".".into(),
            mount: "/panel".into(),
            entry: "index.js".into(),
            component: "panel".into(),
            parent: None,
            config: Value::Null,
        })
        .unwrap();
    deployment.remove("panel").await.unwrap();
    let released = observed.upgrade().is_none();
    manager.shutdown().await.unwrap();
    root.fiber().dispose().await.unwrap();
    assert!(released, "移除网页 Owner 后，其部署视图资源应已释放");
}
