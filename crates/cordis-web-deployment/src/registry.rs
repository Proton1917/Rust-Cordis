//! DOC-15 §11.3：核心保留HTTP服务引用，使旧路由清理作用于原登记对象。

use super::*;
use cordis_up::rpc::RemoteObject;

#[derive(Clone)]
pub(super) struct Registration {
    service: RemoteObject,
    pub id: u64,
    pub sequence: u64,
    pub path: String,
}

impl Registration {
    pub fn version_path(&self) -> &str {
        self.path.split('?').next().expect("registration URL path")
    }
    pub async fn create(manager: &Manager, spec: &Spec, sequence: u64) -> Result<Self> {
        let service = manager.lookup_remote_service("cordis.web@1.1")?;
        let fields = Value::Map(vec![
            ("name".into(), spec.name.clone().into()),
            ("version".into(), spec.version.clone().into()),
            ("sequence".into(), Value::Integer(sequence.into())),
            ("assets".into(), spec.assets.clone().into()),
            ("mount".into(), spec.mount.clone().into()),
            ("entry".into(), spec.entry.clone().into()),
            ("component".into(), spec.component.clone().into()),
        ]);
        let id: u64 = call(&service, "register", vec![fields])
            .await?
            .deserialized()
            .map_err(wire)?;
        let mut path = url::Url::parse(&format!(
            "http://cordis.invalid/_cordis/{}/{}/{}",
            spec.name, sequence, spec.entry
        ))
        .map_err(wire)?;
        path.query_pairs_mut()
            .append_pair("cordis_plugin", &spec.name)
            .append_pair("cordis_version", &spec.version)
            .append_pair("cordis_component", &spec.component)
            .append_pair("cordis_mount", &spec.mount);
        let path = format!("{}?{}", path.path(), path.query().unwrap_or_default());
        Ok(Self {
            service,
            id,
            sequence,
            path,
        })
    }

    pub async fn activate(&self) -> Result<()> {
        call(&self.service, "activate", vec![self.id.into()])
            .await
            .map(|_| ())
    }
    pub async fn close(&self) -> Result<()> {
        if !self.service.peer().is_alive() {
            return Ok(());
        }
        match call(&self.service, "unregister", vec![self.id.into()]).await {
            Err(_) if !self.service.peer().is_alive() => Ok(()),
            result => result.map(|_| ()),
        }
    }
}

/// DOC-16 §6：服务调用只承载资源路径和登记编号，远端对象引用由Registration持有。
async fn call(service: &RemoteObject, method: &str, args: Vec<Value>) -> Result<Value> {
    let response = service
        .peer()
        .request(
            "invoke",
            value(&payload::Invoke {
                object: service.id(),
                method: method.into(),
                args,
                refs: Vec::new(),
            })?,
            Duration::from_secs(30),
        )
        .await?;
    let result: payload::InvokeResult = response.value().deserialized().map_err(wire)?;
    payload::persistent_value(&result.value).map_err(wire)?;
    Ok(result.value)
}
