use cordis::{
    Clock, Component, Config, Context, Event, FiberState, Inject, Result, ServiceKey, Spawn,
    SyncKind, Task, ValidationError,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct TokioHost {
    handle: tokio::runtime::Handle,
    started: Instant,
}

impl Spawn for TokioHost {
    fn spawn(&self, task: Task) {
        self.handle.spawn(task);
    }
}

impl Clock for TokioHost {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
    fn sleep(&self, duration: Duration) -> Task {
        Box::pin(async move { tokio::time::sleep(duration).await })
    }
}

type History = Arc<Mutex<Vec<String>>>;

#[derive(Clone)]
struct Settings {
    generation: u64,
    history: History,
}

impl Config for Settings {
    type Raw = Self;
    fn resolve(raw: &Self::Raw) -> std::result::Result<Self, ValidationError> {
        Ok(raw.clone())
    }
}

struct DatabaseValue {
    generation: u64,
}
struct Database;
impl ServiceKey for Database {
    const NAME: &'static str = "database";
    type Value = DatabaseValue;
    type Meta = ();
}

struct Message;
impl Event for Message {
    const NAME: &'static str = "message";
    type Args = String;
    type Output = ();
    type Kind = SyncKind;
}

struct Provider;
impl Component for Provider {
    const NAME: &'static str = "provider";
    type Config = Settings;
    async fn apply(ctx: Context, config: Settings) -> Result {
        ctx.provide::<Database>(Arc::new(DatabaseValue {
            generation: config.generation,
        }))?;
        ctx.effect("provider-resource", move |fx| {
            fx.defer(move || {
                config
                    .history
                    .lock()
                    .unwrap()
                    .push(format!("Provider {} 清理", config.generation));
                Ok(())
            })
        })?;
        Ok(())
    }
}

struct Consumer;
impl Component for Consumer {
    const NAME: &'static str = "consumer";
    type Config = Settings;
    fn inject() -> Inject {
        Inject::new().with::<Database>()
    }
    async fn apply(ctx: Context, config: Settings) -> Result {
        let database = ctx.get::<Database>()?;
        let generation = database.generation;
        let history = config.history.clone();
        ctx.on::<Message>(move |message| {
            history
                .lock()
                .unwrap()
                .push(format!("Consumer {generation} 收到 {message}"));
            None
        })?;
        ctx.effect("consumer-resource", move |fx| {
            fx.defer(move || {
                config
                    .history
                    .lock()
                    .unwrap()
                    .push(format!("Consumer {} 清理", database.generation));
                Ok(())
            })
        })?;
        Ok(())
    }
}

fn snapshot(ctx: &Context) -> (usize, usize, usize, usize) {
    (
        ctx.event_count(),
        ctx.service_count(),
        ctx.registry().len(),
        ctx.fiber().effects().len(),
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result {
    let host = Arc::new(TokioHost {
        handle: tokio::runtime::Handle::current(),
        started: Instant::now(),
    });
    let root = Context::new(host);
    let before = snapshot(&root);
    let history = History::default();
    let settings = Settings {
        generation: 1,
        history: history.clone(),
    };

    let consumer = root.mount::<Consumer>(settings.clone())?;
    consumer.settled().await?;
    assert_eq!(consumer.state(), FiberState::Pending);
    println!("1. 挂载 Consumer：{:?}", consumer.state());

    let provider = root.mount::<Provider>(settings.clone())?;
    provider.settled().await?;
    consumer.settled().await?;
    assert_eq!(consumer.state(), FiberState::Active);
    let first_epoch = consumer.epoch();
    root.emit::<Message>(&"第一条消息".to_owned());
    println!(
        "2. 挂载 Provider：Consumer {:?}，纪元 {:?}",
        consumer.state(),
        first_epoch
    );

    provider.dispose().await?;
    consumer.settled().await?;
    assert_eq!(consumer.state(), FiberState::Pending);
    assert_eq!(
        history.lock().unwrap().as_slice(),
        [
            "Consumer 1 收到 第一条消息",
            "Consumer 1 清理",
            "Provider 1 清理"
        ]
    );
    println!(
        "3. 释放 Provider：Consumer 1 清理 → Provider 1 清理；Consumer {:?}",
        consumer.state()
    );

    let provider = root.mount::<Provider>(Settings {
        generation: 2,
        ..settings
    })?;
    provider.settled().await?;
    consumer.settled().await?;
    assert_eq!(consumer.state(), FiberState::Active);
    assert_ne!(consumer.epoch(), first_epoch);
    println!(
        "4. 再次挂载 Provider：Consumer {:?}，纪元 {:?}",
        consumer.state(),
        consumer.epoch()
    );

    root.fiber().dispose().await?;
    assert_eq!(consumer.state(), FiberState::Disposed);
    assert_eq!(provider.state(), FiberState::Disposed);
    let after = snapshot(&root);
    assert_eq!(after, before);
    println!(
        "5. 释放根组件：事件、服务、组件、效应快照 {:?} = {:?}",
        after, before
    );
    Ok(())
}
