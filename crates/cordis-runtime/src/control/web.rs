//! DOC-15 §11、ADR-0041：网页状态与后端部署共用事务，资源和页面由唯一内核管理。

use super::*;

pub(super) enum WebChange {
    Update(String, cordis_web_deployment::Change),
    Add(cordis_web_deployment::Spec),
    Remove(String, cordis_web_deployment::Change),
}

impl Controller {
    /// DOC-15 §11.4：先解析全部目标资源，再冻结当前页面；准备失败恢复已冻结的前缀。
    pub(super) async fn prepare_web_changes(
        &self,
        replacement: &Replacement,
    ) -> Result<Vec<WebChange>> {
        let candidates = replacement
            .affected
            .iter()
            .map(|name| {
                let mounted = &self.mounted[name];
                let next = replacement
                    .candidate
                    .web
                    .as_ref()
                    .map(|_| {
                        Self::web_spec(
                            name,
                            &mounted.settings,
                            &replacement.candidate,
                            &mounted._view,
                        )
                    })
                    .transpose()?;
                Ok((name.clone(), next, false))
            })
            .collect::<Result<Vec<_>>>()?;
        self.prepare_web_targets(candidates).await
    }

    pub(super) async fn prepare_web_targets(
        &self,
        candidates: Vec<(String, Option<cordis_web_deployment::Spec>, bool)>,
    ) -> Result<Vec<WebChange>> {
        let mut changes = Vec::new();
        for (name, next, remount) in candidates {
            let result = self
                .prepare_web_target(&name, next, remount, &mut changes)
                .await;
            if let Err(problem) = result {
                return match self.rollback_web_changes(&changes).await {
                    Ok(()) => Err(problem),
                    Err(recovery) => Err(recovery_error("网页准备", problem, recovery)),
                };
            }
        }
        Ok(changes)
    }

    /// ADR-0025、DOC-15 §11：视图变更重新挂载 Owner，页面原状态随移除句柄保存。
    async fn prepare_web_target(
        &self,
        name: &str,
        next: Option<cordis_web_deployment::Spec>,
        remount: bool,
        changes: &mut Vec<WebChange>,
    ) -> Result<()> {
        match (self.web.specification(name), next) {
            (Some(_), Some(candidate)) if !remount => {
                changes.push(WebChange::Update(
                    name.into(),
                    self.web.prepare_change(name, candidate).await?,
                ));
            }
            (previous, candidate) => {
                if previous.is_some() {
                    changes.push(WebChange::Remove(
                        name.into(),
                        self.web.prepare_removal(name).await?,
                    ));
                }
                if let Some(candidate) = candidate {
                    changes.push(WebChange::Add(candidate));
                }
            }
        }
        Ok(())
    }

    /// DOC-15 §11.4：后端稳定后依次完成每项页面变化，首项失败进入全事务恢复。
    pub(super) async fn finish_web_changes(&self, changes: &[WebChange]) -> Result<()> {
        for change in changes {
            match change {
                WebChange::Update(name, change) => self.web.finish_change(name, change).await?,
                WebChange::Add(spec) => {
                    self.web.mount(spec.clone())?;
                    self.web.wait_mounted(&spec.instance).await?;
                }
                WebChange::Remove(name, _) => self.web.remove(name).await?,
            }
        }
        Ok(())
    }

    pub(super) fn restore_web_configurations(&self, changes: &[WebChange]) -> Result<()> {
        for change in changes {
            if let WebChange::Update(name, change) = change {
                self.web.restore_configuration(name, change)?;
            }
        }
        Ok(())
    }

    /// DOC-11 §7：每项恢复均执行，保存第一个失败供管理调用报告。
    pub(super) async fn rollback_web_changes(&self, changes: &[WebChange]) -> Result<()> {
        let mut failure = None;
        for change in changes.iter().rev() {
            let result = match change {
                WebChange::Update(name, change) | WebChange::Remove(name, change) => {
                    self.web.rollback_change(name, change).await
                }
                WebChange::Add(spec) => self.web.remove(&spec.instance).await,
            };
            if let Err(problem) = result {
                failure.get_or_insert(problem);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// DOC-15 §11.1：全栈组件名在实际父实例激活时确定；纯网页组件采用部署名称。
    pub(super) fn web_spec(
        name: &str,
        settings: &InstanceSettings,
        package: &Package,
        view: &Context,
    ) -> Result<cordis_web_deployment::Spec> {
        let web = package
            .web
            .as_ref()
            .ok_or_else(|| parameter("网页插件需要 web 清单"))?;
        let pure = matches!(package.entry, Entry::Web);
        Ok(cordis_web_deployment::Spec {
            instance: name.into(),
            name: package.name.clone(),
            version: package.version.clone(),
            assets: package
                .directory
                .join(&web.assets)
                .canonicalize()
                .map_err(crate::storage::io_error)?
                .to_string_lossy()
                .into_owned(),
            mount: web.mount.clone(),
            entry: web.entry.to_string_lossy().into_owned(),
            component: if pure {
                settings
                    .component
                    .clone()
                    .unwrap_or_else(|| package.name.clone())
            } else {
                String::new()
            },
            parent: (!pure).then(|| name.into()),
            config: Value::serialized(&settings.config).map_err(parameter)?,
            view: view.clone(),
        })
    }

    /// DOC-15 §11.1：纯网页父对象在部署视图中挂载，其句柄由网页部署持有。
    pub(super) fn mount_web(
        &mut self,
        name: &str,
        settings: InstanceSettings,
        package: Package,
    ) -> Result<()> {
        let view = self.mount_view(&settings)?;
        let fiber = self
            .web
            .mount(Self::web_spec(name, &settings, &package, &view)?)?;
        let group = settings
            .group
            .clone()
            .unwrap_or_else(|| package.name.clone());
        self.diagnostics
            .lock()
            .active_references
            .insert(name.into(), package.reference.clone());
        self.mounted.insert(
            name.into(),
            Mounted {
                settings,
                package,
                group,
                fiber,
                _view: view,
                manage: None,
            },
        );
        Ok(())
    }
}

pub(super) fn recovery_error(
    operation: &str,
    problem: WireError,
    recovery: WireError,
) -> WireError {
    WireError::new(
        1,
        format!("{operation}失败：{problem}；恢复失败：{recovery}"),
    )
}
