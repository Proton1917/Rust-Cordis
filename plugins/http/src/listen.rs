//! DOC-15 AU-15、PR-7：默认端口的实际绑定地址按实例保存在插件数据目录。

use cordis_sdk::{Context, Error, Result};
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, net::SocketAddr, path::PathBuf};

#[derive(Serialize, Deserialize)]
struct Address {
    configured: SocketAddr,
    bound: SocketAddr,
}
pub(super) struct Binding {
    path: PathBuf,
    configured: SocketAddr,
}

/// ADR-0041：显式端口沿配置使用；端口0按当前实例恢复已发布的实际地址。
pub(super) fn restore(
    context: &Context,
    configured: SocketAddr,
) -> Result<(SocketAddr, Option<Binding>)> {
    if configured.port() != 0 {
        return Ok((configured, None));
    }
    let snapshot = context
        .get::<dyn cordis_sdk::builtin::Runtime>()?
        .snapshot()?;
    let instance = snapshot
        .instances
        .iter()
        .find(|instance| instance.context == Some(context.context_id()))
        .ok_or_else(|| Error::new(4, "HTTP加载上下文尚待运行时登记"))?;
    let directory = context.data_dir()?.join("listeners");
    let path = directory.join(format!("{}.json", instance.name));
    match fs::read(&path) {
        Ok(bytes) => {
            let address: Address =
                serde_json::from_slice(&bytes).map_err(super::server::problem)?;
            if address.configured == configured {
                return Ok((address.bound, None));
            }
        }
        Err(problem) if problem.kind() == ErrorKind::NotFound => {}
        Err(problem) => return Err(super::server::problem(problem)),
    }
    fs::create_dir_all(directory).map_err(super::server::problem)?;
    Ok((configured, Some(Binding { path, configured })))
}

impl Binding {
    /// DOC-15 §11.3：地址在公布给页面之前持久化，后续激活直接使用该端口。
    pub fn save(self, bound: SocketAddr) -> Result<()> {
        let bytes = serde_json::to_vec(&Address {
            configured: self.configured,
            bound,
        })
        .map_err(super::server::problem)?;
        let pending = self.path.with_extension("next");
        fs::write(&pending, bytes).map_err(super::server::problem)?;
        fs::rename(pending, self.path).map_err(super::server::problem)
    }
}
