//! ADR-0033、UPSTREAM-EXT EntryGroup：按祖先到成员的顺序展开配置视图。

use super::*;

const PRIVATE_REALM: &str = "\0cordis.scope/";

pub(super) fn validate_names(isolate: &BTreeMap<String, Isolation>) -> Result<()> {
    if isolate
        .values()
        .any(|realm| matches!(realm, Isolation::Named(name) if name.starts_with(PRIVATE_REALM)))
    {
        return Err(parameter("隔离域名称属于内部配置组名称空间"));
    }
    Ok(())
}

impl Configuration {
    /// CP-8：继承链长度有界，缺失组和环在来源边界产生明确错误。
    pub(super) fn validate_scopes(&self) -> Result<()> {
        if self.scope.len() > Limits::DEFAULT.max_collection {
            return Err(parameter("配置组数量超过集合上限"));
        }
        for (name, group) in &self.scope {
            instance_name(name)?;
            validate_names(&group.isolate)?;
            self.ancestors(Some(name))?;
        }
        for instance in self.instance.values() {
            self.ancestors(instance.scope.as_deref())?;
        }
        Ok(())
    }

    fn ancestors<'a>(
        &'a self,
        mut name: Option<&'a str>,
    ) -> Result<Vec<(&'a str, &'a ScopeSettings)>> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some(current) = name {
            if chain.len() >= Limits::DEFAULT.max_depth || !seen.insert(current) {
                return Err(parameter("配置组继承存在环或超过 64 层"));
            }
            let group = self
                .scope
                .get(current)
                .ok_or_else(|| parameter(format!("配置组 {current} 缺席")))?;
            chain.push((current, group));
            name = group.parent.as_deref();
        }
        chain.reverse();
        Ok(chain)
    }

    /// DOC-11 §10、CP-2：每个平铺目标按实际序列化大小计入总预算。
    pub(super) fn expand_scopes(
        &self,
        instances: BTreeMap<String, InstanceSettings>,
    ) -> Result<BTreeMap<String, InstanceSettings>> {
        let mut expanded = BTreeMap::new();
        let mut bytes = 0_usize;
        for (name, mut instance) in instances {
            let chain = self.ancestors(instance.scope.as_deref())?;
            if instance.disabled || chain.iter().any(|(_, scope)| scope.disabled) {
                continue;
            }
            let mut isolate = BTreeMap::new();
            let mut intercept = BTreeMap::new();
            let mut realm_bytes = 0_usize;
            for (scope_name, scope) in chain {
                for (key, realm) in &scope.isolate {
                    let length = match realm {
                        Isolation::Named(name) => name.len(),
                        Isolation::Fresh => PRIVATE_REALM.len() + scope_name.len() + 1 + key.len(),
                    };
                    realm_bytes = realm_bytes.saturating_add(key.len()).saturating_add(length);
                    if realm_bytes > Limits::DEFAULT.max_frame {
                        return Err(WireError::new(8, "配置组域名称展开超过 16 MiB"));
                    }
                    let realm = match realm {
                        Isolation::Named(name) => Isolation::Named(name.clone()),
                        Isolation::Fresh => {
                            Isolation::Named(format!("{PRIVATE_REALM}{scope_name}/{key}"))
                        }
                    };
                    isolate.insert(key.clone(), realm);
                }
                merge_metadata(&mut intercept, &scope.intercept);
            }
            isolate.extend(instance.isolate);
            merge_metadata(&mut intercept, &instance.intercept);
            instance.isolate = isolate;
            instance.intercept = intercept;
            bytes = bytes
                .saturating_add(name.len())
                .saturating_add(toml::to_string(&instance).map_err(parameter)?.len());
            if bytes > Limits::DEFAULT.max_frame {
                return Err(WireError::new(8, "配置组展开后的运行配置超过 16 MiB"));
            }
            expanded.insert(name, instance);
        }
        Ok(expanded)
    }
}

/// DOC-15 §6.4：接口元数据表按字段覆盖，其他值由最近一层替换。
fn merge_metadata(
    target: &mut BTreeMap<String, toml::Value>,
    next: &BTreeMap<String, toml::Value>,
) {
    for (key, value) in next {
        match (target.get_mut(key), value) {
            (Some(toml::Value::Table(old)), toml::Value::Table(next)) => old.extend(next.clone()),
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}
