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
        self.settle(name).await?;
        if mounted.fiber.state() != cordis::FiberState::Active {
            return Err(WireError::new(
                4,
                format!("验证实例 {name} 状态 {:?}", mounted.fiber.state()),
            ));
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
        let settings = self.mounted[suite].settings.clone();
        self.remove(suite).await?;
        let subject = &self.mounted[subject];
        let plan = catalog::plan(
            &subject.group,
            &self.groups[&subject.group],
            &self.project,
            &self.configuration.runtime,
            &self.locations,
        )?;
        self.manager.replace_group(plan).await?;
        next_stage();
        self.mount(suite, settings, None).await?;
        self.contract_settle(suite).await
    }
}
