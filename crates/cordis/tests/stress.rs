use cordis::{
    Clock, Component, Config, Context, Event, Fiber, FiberState, Inject, Realm, Result, ServiceKey,
    Spawn, SyncKind, Task, ValidationError,
};
use futures_util::future::join_all;
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Active(usize),
    ConsumerCleanup(usize),
    ProviderCleanup(usize),
}
type Journal = Arc<Mutex<Vec<Action>>>;

#[derive(Clone)]
struct Settings {
    generation: usize,
    journal: Journal,
    applications: Arc<Mutex<Vec<usize>>>,
}

impl Config for Settings {
    type Raw = Self;
    fn resolve(raw: &Self::Raw) -> std::result::Result<Self, ValidationError> {
        Ok(raw.clone())
    }
}

struct DatabaseValue {
    generation: usize,
    journal: Journal,
}
struct Database;
impl ServiceKey for Database {
    const NAME: &'static str = "stress-database";
    type Value = DatabaseValue;
    type Meta = ();
}

struct Ping;
impl Event for Ping {
    const NAME: &'static str = "stress-ping";
    type Args = usize;
    type Output = ();
    type Kind = SyncKind;
}

struct Provider;
impl Component for Provider {
    const NAME: &'static str = "stress-provider";
    type Config = Settings;
    async fn apply(ctx: Context, config: Settings) -> Result {
        ctx.provide::<Database>(Arc::new(DatabaseValue {
            generation: config.generation,
            journal: config.journal.clone(),
        }))?;
        ctx.effect("provider-resource", move |fx| {
            fx.defer(move || {
                config
                    .journal
                    .lock()
                    .unwrap()
                    .push(Action::ProviderCleanup(config.generation));
                Ok(())
            })
        })?;
        Ok(())
    }
}

struct Consumer;
impl Consumer {
    fn observe_database(ctx: Context) -> Result {
        let database = ctx.get::<Database>()?;
        database
            .journal
            .lock()
            .unwrap()
            .push(Action::Active(database.generation));
        let value = database.value();
        ctx.on::<Ping>(move |expected| {
            assert_eq!(value.generation, *expected);
            None
        })?;
        ctx.effect("consumer-resource", move |fx| {
            fx.defer(move || {
                database
                    .journal
                    .lock()
                    .unwrap()
                    .push(Action::ConsumerCleanup(database.generation));
                Ok(())
            })
        })?;
        Ok(())
    }
}

impl Component for Consumer {
    const NAME: &'static str = "stress-consumer";
    type Config = Settings;
    fn inject() -> Inject {
        Inject::new().with::<Database>()
    }
    async fn apply(ctx: Context, config: Settings) -> Result {
        config.applications.lock().unwrap().push(config.generation);
        Self::observe_database(ctx)
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

async fn exercise_realm(
    root: Context,
    generation: usize,
    barrier: Arc<tokio::sync::Barrier>,
) -> (Fiber, Fiber, Fiber, Journal) {
    let context = root.isolate::<Database>(Realm::new());
    let journal = Journal::default();
    let applications = Arc::new(Mutex::new(Vec::new()));
    let settings = Settings {
        generation,
        journal: journal.clone(),
        applications: applications.clone(),
    };
    let consumer = context.mount::<Consumer>(settings.clone()).unwrap();
    consumer.settled().await.unwrap();
    assert_eq!(consumer.state(), FiberState::Pending);

    let first = context.mount::<Provider>(settings.clone()).unwrap();
    first.settled().await.unwrap();
    consumer.settled().await.unwrap();
    assert_eq!(consumer.state(), FiberState::Active);
    let epoch = consumer.epoch();
    let realm = context.realm::<Database>();
    context.emit_filtered::<Ping>(
        &generation,
        Some(&|listener| listener.realm::<Database>() == realm),
    );

    barrier.wait().await;
    let updated_generation = generation + 100_000;
    consumer
        .update(Settings {
            generation: updated_generation,
            ..settings.clone()
        })
        .await
        .unwrap();
    assert_eq!(
        consumer.config::<Settings>().unwrap().generation,
        updated_generation
    );
    assert_eq!(
        applications.lock().unwrap().as_slice(),
        [generation, updated_generation]
    );
    assert_eq!(consumer.state(), FiberState::Active);
    assert!(consumer.is_settled());

    barrier.wait().await;
    consumer.restart().await.unwrap();
    assert_eq!(
        consumer.config::<Settings>().unwrap().generation,
        updated_generation
    );
    assert_eq!(
        applications.lock().unwrap().as_slice(),
        [generation, updated_generation, updated_generation]
    );
    assert_eq!(consumer.state(), FiberState::Active);
    assert!(consumer.is_settled());

    first.dispose().await.unwrap();
    consumer.settled().await.unwrap();
    assert_eq!(consumer.state(), FiberState::Pending);
    assert_eq!(
        journal.lock().unwrap().as_slice(),
        [
            Action::Active(generation),
            Action::ConsumerCleanup(generation),
            Action::Active(generation),
            Action::ConsumerCleanup(generation),
            Action::Active(generation),
            Action::ConsumerCleanup(generation),
            Action::ProviderCleanup(generation),
        ]
    );

    let second = context
        .mount::<Provider>(Settings {
            generation: generation + 1,
            ..settings
        })
        .unwrap();
    second.settled().await.unwrap();
    consumer.settled().await.unwrap();
    assert_eq!(consumer.state(), FiberState::Active);
    assert_ne!(consumer.epoch(), epoch);
    assert_eq!(
        applications.lock().unwrap().as_slice(),
        [
            generation,
            updated_generation,
            updated_generation,
            updated_generation
        ]
    );
    context.emit_filtered::<Ping>(
        &(generation + 1),
        Some(&|listener| listener.realm::<Database>() == realm),
    );
    (consumer, first, second, journal)
}

// DEC-01、T-CTX-4：声明依赖控制方法执行，方法通过显式上下文读取服务并交回效应。
#[tokio::test(flavor = "current_thread")]
async fn declared_dependency_controls_method_effect() {
    let host = Arc::new(TokioHost {
        handle: tokio::runtime::Handle::current(),
        started: Instant::now(),
    });
    let root = Context::new(host);
    let journal = Journal::default();
    let config = Settings {
        generation: 7,
        journal: journal.clone(),
        applications: Arc::new(Mutex::new(Vec::new())),
    };
    let consumer = root.mount::<Consumer>(config.clone()).unwrap();
    consumer.settled().await.unwrap();
    assert!(journal.lock().unwrap().is_empty());

    let provider = root.mount::<Provider>(config).unwrap();
    provider.settled().await.unwrap();
    consumer.settled().await.unwrap();
    assert_eq!(journal.lock().unwrap().as_slice(), [Action::Active(7)]);

    provider.dispose().await.unwrap();
    consumer.settled().await.unwrap();
    assert_eq!(
        journal.lock().unwrap().as_slice(),
        [
            Action::Active(7),
            Action::ConsumerCleanup(7),
            Action::ProviderCleanup(7)
        ]
    );
    root.fiber().dispose().await.unwrap();
}

// G4：8 个执行器线程运行固定 1000 轮。
// 每轮 8 个隔离域通过屏障并发更新、重启，并经历撤回、重供和父级回收。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn eight_workers_recover_one_thousand_rounds() {
    let host = Arc::new(TokioHost {
        handle: tokio::runtime::Handle::current(),
        started: Instant::now(),
    });
    let root = Context::new(host);
    let baseline = snapshot(&root);
    tokio::time::timeout(Duration::from_secs(120), async {
        for round in 0..1000 {
            let barrier = Arc::new(tokio::sync::Barrier::new(8));
            let tasks = (0..8).map(|lane| {
                tokio::spawn(exercise_realm(
                    root.clone(),
                    round * 16 + lane * 2,
                    barrier.clone(),
                ))
            });
            let realms = join_all(tasks)
                .await
                .into_iter()
                .map(|task| task.unwrap())
                .collect::<Vec<_>>();
            root.fiber().dispose().await.unwrap();
            for (lane, (consumer, first, second, journal)) in realms.into_iter().enumerate() {
                for fiber in [consumer, first, second] {
                    fiber.settled().await.unwrap();
                    assert!(fiber.is_settled());
                    assert_eq!(fiber.state(), FiberState::Disposed);
                }
                let generation = round * 16 + lane * 2;
                assert_eq!(
                    journal.lock().unwrap().as_slice(),
                    [
                        Action::Active(generation),
                        Action::ConsumerCleanup(generation),
                        Action::Active(generation),
                        Action::ConsumerCleanup(generation),
                        Action::Active(generation),
                        Action::ConsumerCleanup(generation),
                        Action::ProviderCleanup(generation),
                        Action::Active(generation + 1),
                        Action::ConsumerCleanup(generation + 1),
                        Action::ProviderCleanup(generation + 1),
                    ]
                );
            }
            assert_eq!(root.fiber().state(), FiberState::Active);
            assert_eq!(snapshot(&root), baseline, "round {round}");
        }
    })
    .await
    .expect("1000 rounds must finish within 120 seconds");
}
