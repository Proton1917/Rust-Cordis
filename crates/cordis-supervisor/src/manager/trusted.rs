//! ADR-0046 §3.11：受信调用来源存于核心上下文，业务拦截与该来源分别持有。

use super::*;

pub(super) struct TrustedCall {
    pub instance: String,
    fields: Vec<(Value, Value)>,
}

impl Manager {
    /// ADR-0046 §3.11：核心装配入口建立调用身份，派生视图和子组件沿上下文链继承。
    pub fn caller_context(
        &self,
        parent: &Context,
        instance: &str,
        fields: Vec<(String, Value)>,
    ) -> Context {
        let mut fields: BTreeMap<String, Value> = fields.into_iter().collect();
        fields
            .entry("cordis.caller".into())
            .or_insert_with(|| Value::Map(vec![("instance".into(), instance.into())]));
        parent.extend(TrustedCall {
            instance: instance.into(),
            fields: fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        })
    }

    /// ADR-0046 AU-25/26：权限诊断只记来源和键名，元数据值保持在调用载荷内。
    pub(super) fn check_interception(&self, context: &Context, meta: &Value) -> Result<()> {
        if let Value::Map(fields) = meta {
            for (key, _) in fields {
                if let Value::Text(key) = key
                    && key.starts_with("cordis.")
                {
                    let caller = context.extension::<TrustedCall>();
                    let instance = caller
                        .as_ref()
                        .map_or("deployment", |caller| caller.instance.as_str());
                    let failure = error(9, format!("实例 {instance} 尝试设置受信元数据键 {key}"));
                    self.0.context.logger().warn(failure.message.clone());
                    return Err(failure);
                }
            }
        }
        Ok(())
    }

    /// ADR-0046 §3.11：Scoped 绑定采用核心身份，保留业务元数据并覆盖受信来源字段。
    pub(super) fn scoped_metadata(&self, view: &ViewLease, interface: &str) -> Result<Value> {
        let metadata = view
            .context
            .named_meta(interface)
            .and_then(|meta| meta.downcast::<Value>().ok());
        let mut fields = match metadata.as_deref() {
            Some(Value::Map(fields)) => fields.clone(),
            Some(Value::Null) | None => Vec::new(),
            Some(_) => return Err(error(2, "Scoped 元数据需要映射")),
        };
        fields.retain(|(key, _)| !matches!(key, Value::Text(key) if key.starts_with("cordis.")));
        let caller = view
            .context
            .extension::<TrustedCall>()
            .expect("加载上下文持有核心调用身份");
        fields.extend(caller.fields.clone());
        Ok(Value::Map(fields))
    }
}
