//! ADR-0017、DOC-11 §8：验证会话的稳定等待与被测状态导出继承。

use super::*;

impl Controller {
    /// ADR-0017：指定实例交回 apply 并进入 Active 后，契约会话推进下一阶段。
    pub(crate) async fn contract_settle(&self, name: &str) -> Result<()> {
        let mounted = self.mounted.get(name).ok_or_else(|| {
            let problem = self
                .diagnostics
                .lock()
                .instance_errors
                .get(name)
                .cloned()
                .unwrap_or_else(|| format!("验证实例 {name} 缺席"));
            WireError::new(4, problem)
        })?;
        let fiber = if matches!(mounted.package.entry, Entry::Web) {
            self.web.instance_fiber(name)?
        } else {
            self.manager.instance_fiber(name)?
        };
        tokio::time::timeout(
            Duration::from_millis(self.runtime.lifecycle_timeout_ms),
            fiber.settled(),
        )
        .await
        .map_err(|_| WireError::new(6, "实例加载截止时间到达"))?
        .map_err(kernel)?;
        let state = fiber.state();
        if state != cordis::FiberState::Active {
            return Err(WireError::new(4, format!("验证实例 {name} 状态 {state:?}")));
        }
        Ok(())
    }

    /// ADR-0017、SW-5：停用套件后，由监管器导出并重启被测组，套件以阶段 1 重新挂载。
    pub(crate) async fn contract_roundtrip(
        &mut self,
        suite: &str,
        subject: &str,
        next_stage: impl FnOnce(),
    ) -> Result<()> {
        self.contract_settle(suite).await?;
        self.contract_settle(subject).await?;
        let mut paused = self.overlay.clone();
        paused.instance.remove(suite);
        paused.removed.insert(suite.into());
        self.apply_deployment_batch(&[], None, paused, false)
            .await?;
        let subject = &self.mounted[subject];
        let group = self
            .groups
            .get(&subject.group)
            .ok_or_else(|| parameter("被测实例缺少可重启的宿主分组"))?;
        let plan = self.group_plan(&subject.group, group)?;
        self.manager.replace_group(plan).await?;
        next_stage();
        self.resume_contract_suite(suite).await?;
        self.contract_settle(suite).await
    }
}
