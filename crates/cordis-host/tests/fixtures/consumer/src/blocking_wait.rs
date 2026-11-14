//! DOC-15 CP-8：两个真实同步服务方法同时等待另一宿主中的服务。

use cordis_sdk::prelude::*;
use loader_fixture_api::{Counter, WaitControl, WaitProbe};
use std::sync::{
    Barrier,
    atomic::{AtomicU64, Ordering},
};

pub struct BlockingWait;

#[derive(serde::Deserialize)]
pub struct WaitConfig {
    raw: bool,
}

struct WaitingService {
    context: Context,
    provider: DynamicService,
    rendezvous: Barrier,
    raw: bool,
    polls: AtomicU64,
}

#[cordis_sdk::interface]
impl WaitProbe for WaitingService {
    fn call(&self) -> Result<u64> {
        let future = async {
            self.rendezvous.wait();
            self.provider.invoke_as_async("value", &[] as &[u64]).await
        };
        if self.raw {
            futures::executor::block_on(future)
        } else {
            self.context.wait(future)
        }
    }

    fn nested(&self) -> Result<u64> {
        self.context.wait(async {
            self.context
                .wait(self.provider.invoke_as_async("value", &[] as &[u64]))
        })
    }

    fn system_thread(&self) -> Result<u64> {
        let provider = self.provider.clone();
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.context.spawn_thread("sdk-wait", move |context| {
            let result = context.wait(provider.invoke_as_async::<_, u64>("value", &[] as &[u64]));
            sender
                .send(result)
                .map_err(|_| Error::new(4, "系统线程结果接收方已结束"))
        })?;
        self.context.wait(async {
            receiver
                .await
                .map_err(|error| Error::new(4, error.to_string()))?
        })
    }

    fn task(&self) -> Result<u64> {
        let provider = self.provider.clone();
        let context = self.context.clone();
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.context.spawn(async move {
            let result = context.wait(provider.invoke_as_async::<_, u64>("value", &[] as &[u64]));
            sender
                .send(result)
                .map_err(|_| Error::new(4, "后台任务结果接收方已结束"))
        })?;
        self.context.wait(async {
            receiver
                .await
                .map_err(|error| Error::new(4, error.to_string()))?
        })
    }

    fn failure(&self) -> Result<u64> {
        self.context
            .wait(async { Err(Error::new(1021, "等待体业务错误")) })
    }

    fn panics(&self) -> Result<u64> {
        self.context.wait(async { panic!("等待体 panic") })
    }

    fn legacy(&self) -> Result<u64> {
        self.context.wait(async {
            self.polls.fetch_add(1, Ordering::SeqCst);
            Ok(100)
        })
    }

    fn polls(&self) -> Result<u64> {
        Ok(self.polls.load(Ordering::SeqCst))
    }
}

impl Component for BlockingWait {
    const NAME: &'static str = "blocking-wait";
    type Config = WaitConfig;

    fn inject() -> Inject {
        Inject::new()
            .with::<dyn Counter>()
            .with::<dyn WaitControl>()
    }

    async fn apply(ctx: Context, config: WaitConfig) -> Result<()> {
        let probe = ctx.service::<dyn WaitProbe>(Arc::new(WaitingService {
            context: ctx.clone(),
            provider: ctx.get_dynamic("loader.counter@1.0")?,
            rendezvous: Barrier::new(if config.raw { 2 } else { 4 }),
            raw: config.raw,
            polls: AtomicU64::new(0),
        }))?;
        ctx.get::<dyn WaitControl>()?.offer(probe)
    }
}
