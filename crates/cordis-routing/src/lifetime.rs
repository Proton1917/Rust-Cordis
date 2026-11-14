//! ADR-0052 R-3：路由调用与其提供方纪元的退役通知竞争。

use crate::{Result, error};
use std::{future::Future, sync::Arc};
use tokio::sync::watch;

#[derive(Clone)]
pub struct Retirement(Arc<watch::Sender<bool>>);

impl Default for Retirement {
    fn default() -> Self {
        Self(Arc::new(watch::channel(false).0))
    }
}

impl Retirement {
    pub(crate) fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    pub fn finish(&self) {
        self.0.send_replace(true);
    }

    pub fn ended(&self) -> bool {
        *self.0.borrow()
    }

    /// 提供方退役优先于同轮到达的回复，丢弃 RPC Future 沿 UP/1 发送取消。
    pub async fn during<T>(&self, operation: impl Future<Output = Result<T>>) -> Result<T> {
        let mut changed = self.0.subscribe();
        tokio::select! {
            biased;
            _ = changed.wait_for(|ended| *ended) => Err(error(4, "服务提供方加载纪元已结束")),
            result = operation => result,
        }
    }
}

/// 路由器只读取调用租约；加载代与业务对象由其事实拥有者管理。
pub trait RouteLifetime {
    fn retirement(&self) -> Option<Retirement>;
}
