//! ADR-0046：原生插件自己的执行线程通过真实宿主调用另一组，并观察取消事件。

use cordis_sdk::prelude::*;
use futures_channel::oneshot;
use futures_util::{
    FutureExt,
    future::{Either, select},
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[cordis_sdk::interface(name = "test.thread-call", version = "1.0")]
trait Calls: Send + Sync {
    async fn echo(&self, value: u64) -> Result<u64>;
    async fn wait(&self, token: String) -> Result<()>;
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[cordis_sdk::event(name = "test.thread-state", version = "1.0")]
struct Signal {
    token: String,
    stopped: bool,
}

struct Cancelled {
    context: Context,
    token: String,
}
impl Drop for Cancelled {
    fn drop(&mut self) {
        if let Err(problem) = self.context.emit(Signal {
            token: self.token.clone(),
            stopped: true,
        }) {
            eprintln!("取消通知：{problem}");
        }
    }
}

struct Implementation(Context);
#[cordis_sdk::interface]
impl Calls for Implementation {
    async fn echo(&self, value: u64) -> Result<u64> {
        Ok(value + 1)
    }
    async fn wait(&self, token: String) -> Result<()> {
        let _cancelled = Cancelled {
            context: self.0.clone(),
            token: token.clone(),
        };
        self.0.emit(Signal {
            token,
            stopped: false,
        })?;
        std::future::pending().await
    }
}

pub struct Provider;
impl Component for Provider {
    const NAME: &'static str = "thread-provider";
    type Config = EmptyConfig;
    fn provides() -> Provides {
        Provides::new().with::<dyn Calls>()
    }
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        context.provide::<dyn Calls>(Arc::new(Implementation(context.clone())))
    }
}

pub struct Idle;
impl Component for Idle {
    const NAME: &'static str = "thread-child";
    type Config = EmptyConfig;
    async fn apply(_: Context, _: EmptyConfig) -> Result<()> {
        Ok(())
    }
}

#[derive(serde::Deserialize)]
pub struct Config {
    mode: String,
    result: String,
}

pub struct Probe;
impl Component for Probe {
    const NAME: &'static str = "thread-probe";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Calls>()
    }
    fn listens() -> Listens {
        Listens::new().with::<Signal>()
    }

    async fn apply(context: Context, config: Config) -> Result<()> {
        let (started, first) = oneshot::channel();
        let (stopped, second) = oneshot::channel();
        let senders = Mutex::new((Some(started), Some(stopped)));
        let token = config.mode.clone();
        context.on::<Signal, _>(move |signal| {
            if signal.token == token {
                let mut pair = senders.lock().unwrap();
                let sender = if signal.stopped {
                    pair.1.take()
                } else {
                    pair.0.take()
                };
                if let Some(sender) = sender {
                    let _received = sender.send(());
                }
            }
            Ok(())
        })?;
        context.spawn_thread("own-runtime-regression", move |thread_context| {
            let operation = exercise(
                thread_context,
                config.mode.clone(),
                first,
                second,
                config.result,
            );
            if config.mode == "tokio" {
                tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .map_err(problem)?
                    .block_on(operation)
            } else {
                futures_executor::block_on(operation)
            }
        })
    }
}

async fn exercise(
    context: Context,
    token: String,
    started: oneshot::Receiver<()>,
    stopped: oneshot::Receiver<()>,
    output: String,
) -> Result<()> {
    let service = context.get::<dyn Calls>()?;
    let first = service.echo(41).await?;
    context.sleep(std::time::Duration::from_millis(1)).await?;
    let cleaned = Arc::new(AtomicBool::new(false));
    let marker = cleaned.clone();
    let effect = context.effect("thread-owned-effect", move |scope| {
        scope.defer(move || {
            marker.store(true, Ordering::Release);
            Ok(())
        })
    })?;
    effect.settled().await?;
    effect.dispose().await?;
    context.plugin::<Idle>(&EmptyConfig::default()).await?;
    match select(service.wait(token.clone()).boxed(), started).await {
        Either::Right((received, waiting)) => {
            received.map_err(problem)?;
            drop(waiting);
        }
        Either::Left(_) => return Err(Error::new(1, "待取消调用提前完成")),
    }
    stopped.await.map_err(problem)?;
    let second = service.echo(99).await?;
    if first != 42 || second != 100 || !cleaned.load(Ordering::Acquire) {
        return Err(Error::new(1, "自有线程调用或效应清理结果错误"));
    }
    let result = serde_json::json!({"mode": token, "before": first, "after": second, "remote_cancelled": true, "effect_cleaned": true});
    std::fs::write(output, serde_json::to_vec(&result).map_err(problem)?).map_err(problem)
}

fn problem(problem: impl std::fmt::Display) -> Error {
    Error::new(1, problem.to_string())
}
