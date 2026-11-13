use cordis_sdk::prelude::*;
use loader_fixture_api::{Control, Counter, Metadata, NewCounter, ScopedCounter};

struct Low;
impl Component for Low {
    const NAME: &'static str = "low";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }
    async fn apply(ctx: Context, _: EmptyConfig) -> Result<()> {
        let provider = ctx.get::<dyn Counter>()?;
        provider.value()?;
        let retained = ctx.clone();
        ctx.effect("consumer-cleanup", move |fx| {
            fx.defer(move || {
                let version = provider.implementation()?;
                retained.log(2, &format!("consumer-cleanup:{version}"))
            })
        })?;
        Ok(())
    }
}

struct Idle;
impl Component for Idle {
    const NAME: &'static str = "idle";
    type Config = EmptyConfig;
    async fn apply(_: Context, _: EmptyConfig) -> Result<()> {
        Ok(())
    }
}

struct Carrier;

struct Dependent;
impl Component for Dependent {
    const NAME: &'static str = "dependent";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }
    async fn apply(_: Context, _: EmptyConfig) -> Result<()> {
        Ok(())
    }
}

struct PrivateCounter(std::sync::atomic::AtomicU64);
#[cordis_sdk::interface]
impl Counter for PrivateCounter {
    fn value(&self) -> Result<u64> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn add(&self, amount: u64) -> Result<u64> {
        Ok(self
            .0
            .fetch_add(amount, std::sync::atomic::Ordering::SeqCst)
            + amount)
    }
    fn implementation(&self) -> Result<u64> {
        Ok(1)
    }
}

struct PrivateProvider;
impl Component for PrivateProvider {
    const NAME: &'static str = "private-provider";
    type Config = EmptyConfig;
    fn provides() -> Provides {
        Provides::new().with::<dyn Counter>()
    }
    async fn apply(ctx: Context, _: EmptyConfig) -> Result<()> {
        ctx.provide::<dyn Counter>(Arc::new(PrivateCounter(std::sync::atomic::AtomicU64::new(
            41,
        ))))?;
        let retained = ctx.clone();
        ctx.effect("private provider", move |scope| {
            scope.defer(move || retained.log(2, "private-provider-cleanup"))
        })?;
        Ok(())
    }
}

struct PrivateReader;
impl Component for PrivateReader {
    const NAME: &'static str = "private-reader";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }
    async fn apply(ctx: Context, _: EmptyConfig) -> Result<()> {
        if ctx.get::<dyn Counter>()?.add(1)? != 42 {
            return Err(Error::new(1020, "私域子组件应共享实际服务状态"));
        }
        let retained = ctx.clone();
        ctx.effect("private reader", move |scope| {
            scope.defer(move || retained.log(2, "private-reader-cleanup"))
        })?;
        Ok(())
    }
}

impl Component for Carrier {
    const NAME: &'static str = "carrier";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>().with::<dyn Control>()
    }
    async fn apply(ctx: Context, _: EmptyConfig) -> Result<()> {
        ctx.get::<dyn Control>()?.offer(ctx.get::<dyn Counter>()?)
    }
}

struct High;
impl Component for High {
    const NAME: &'static str = "high";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn NewCounter>()
    }
    async fn apply(ctx: Context, _: EmptyConfig) -> Result<()> {
        let provider = ctx.get::<dyn NewCounter>()?;
        provider.value()?;
        ctx.effect("consumer-cleanup", move |fx| {
            fx.defer(move || provider.implementation().map(|_| ()))
        })?;
        Ok(())
    }
}

struct Optional;
impl Component for Optional {
    const NAME: &'static str = "optional";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().optional::<dyn NewCounter>()
    }
    async fn apply(ctx: Context, _: EmptyConfig) -> Result<()> {
        match ctx.get::<dyn NewCounter>() {
            Ok(provider) => {
                provider.value()?;
                ctx.effect("optional-cleanup", move |fx| {
                    fx.defer(move || provider.implementation().map(|_| ()))
                })?;
                Ok(())
            }
            Err(error) if error.code() == 4 => Ok(()),
            Err(error) => Err(error),
        }
    }
}

struct Contract;

struct Callers;

#[derive(serde::Deserialize)]
struct CallersConfig {
    #[serde(default)]
    observation: Option<std::path::PathBuf>,
}

#[derive(serde::Serialize)]
struct CallerObservation {
    pid: u32,
    complete: bool,
    values: [u64; 8],
    versions: [u64; 8],
}

struct CallerObservations {
    path: std::path::PathBuf,
    seen: std::sync::atomic::AtomicU8,
    values: [std::sync::atomic::AtomicU64; 8],
    versions: [std::sync::atomic::AtomicU64; 8],
}

impl CallerObservations {
    /// AC-9：每次加载先公布待观测状态，避免下一纪元复用上一纪元的线程结果。
    fn new(path: std::path::PathBuf) -> Result<Self> {
        let observation = Self {
            path,
            seen: std::sync::atomic::AtomicU8::new(0),
            values: std::array::from_fn(|_| std::sync::atomic::AtomicU64::new(0)),
            versions: std::array::from_fn(|_| std::sync::atomic::AtomicU64::new(0)),
        };
        observation.write(false)?;
        Ok(observation)
    }

    /// AC-9：各真实线程保存首次读取结果，八项齐备后一次发布该加载纪元的观察文件。
    fn record(&self, index: usize, value: u64, version: u64) -> Result<()> {
        use std::sync::atomic::Ordering;
        self.values[index].store(value, Ordering::Release);
        self.versions[index].store(version, Ordering::Release);
        let previous = self.seen.fetch_or(1 << index, Ordering::AcqRel);
        if previous != u8::MAX && previous | (1 << index) == u8::MAX {
            self.write(true)?;
        }
        Ok(())
    }

    /// AC-9：观察文件写入部署提供的插件数据目录，原子发布保护并发读取。
    fn write(&self, complete: bool) -> Result<()> {
        let observation = CallerObservation {
            pid: std::process::id(),
            complete,
            values: std::array::from_fn(|index| {
                self.values[index].load(std::sync::atomic::Ordering::Acquire)
            }),
            versions: std::array::from_fn(|index| {
                self.versions[index].load(std::sync::atomic::Ordering::Acquire)
            }),
        };
        let staged = self.path.with_extension("next");
        std::fs::write(&staged, cordis_sdk::encode(&observation)?)
            .map_err(|error| Error::new(1, error.to_string()))?;
        std::fs::rename(staged, &self.path).map_err(|error| Error::new(1, error.to_string()))
    }
}

impl Component for Callers {
    const NAME: &'static str = "callers";
    type Config = CallersConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }
    async fn apply(ctx: Context, config: CallersConfig) -> Result<()> {
        let observation = config
            .observation
            .map(CallerObservations::new)
            .transpose()?
            .map(Arc::new);
        for index in 0..8 {
            let observation = observation.clone();
            ctx.spawn_thread(&format!("caller-{index}"), move |context| {
                let mut observed = 0;
                while context.is_current()? {
                    match context
                        .get::<dyn Counter>()
                        .and_then(|service| Ok((service.value()?, service.implementation()?)))
                    {
                        Ok((value, version)) if observed != version => {
                            observed = version;
                            context.log(2, &format!("caller-{index}-version-{version}"))?;
                            if let Some(observation) = &observation {
                                observation.record(index, value, version)?;
                            }
                        }
                        Ok(_) => {}
                        Err(error) if error.code() == 4 => {}
                        Err(error) => return Err(error),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(())
            })?;
        }
        ctx.spawn(async move {
            std::future::pending::<()>().await;
            Ok(())
        })?;
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct ContractConfig {
    expected_typed: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ChildConfig {
    expected: u64,
}

struct ScopedChild;
impl Component for ScopedChild {
    const NAME: &'static str = "scoped-child";
    type Config = ChildConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn ScopedCounter>()
    }
    async fn apply(ctx: Context, config: ChildConfig) -> Result<()> {
        let counter = ctx.get::<dyn ScopedCounter>()?;
        if counter.add(1)? != config.expected || counter.label()? != "outer" {
            return Err(Error::new(1010, "子组件应继承派生视图的元数据"));
        }
        let retained = ctx.clone();
        ctx.effect("child marker", move |scope| {
            scope.defer(move || retained.log(2, "child-cleanup"))
        })?;
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RealmConfig {
    expected: Option<u64>,
    add: u64,
}

struct RealmChild;
impl Component for RealmChild {
    const NAME: &'static str = "realm-child";
    type Config = RealmConfig;
    fn inject() -> Inject {
        Inject::new().optional::<dyn ScopedCounter>()
    }
    async fn apply(ctx: Context, config: RealmConfig) -> Result<()> {
        match (ctx.get::<dyn ScopedCounter>(), config.expected) {
            (Ok(counter), Some(expected)) if counter.add(config.add)? == expected => Ok(()),
            (Err(error), None) if error.code() == 4 => Ok(()),
            _ => Err(Error::new(1018, "隔离子组件的服务状态与命名域配置失配")),
        }
    }
}

#[derive(serde::Deserialize)]
struct CompositionConfig {
    expected_typed: bool,
}

struct Composition;
impl Component for Composition {
    const NAME: &'static str = "composition";
    type Config = CompositionConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn ScopedCounter>()
    }
    async fn apply(ctx: Context, config: CompositionConfig) -> Result<()> {
        let base = ctx.get::<dyn ScopedCounter>()?;
        // ADR-0046 §3.11：基础视图携带可信调用身份，形成一次 Scoped 绑定。
        if base.value()? != 100 || base.bindings()? != 1 {
            return Err(Error::new(1011, "基视图计数应为100"));
        }
        let view = ctx.intercept::<dyn ScopedCounter, _>(&Metadata {
            offset: 7,
            label: "outer".into(),
        })?;
        let bound = view.get::<dyn ScopedCounter>()?;
        let repeated = view.get::<dyn ScopedCounter>()?;
        if bound.value()? != 107 || repeated.value()? != 107 || base.bindings()? != 2 {
            return Err(Error::new(1012, "同一视图应复用一次绑定结果"));
        }
        if bound.clone_object()?.has_typed_methods() != config.expected_typed {
            return Err(Error::new(1013, "Scoped对象的调用插口与部署分组不一致"));
        }
        let retained = view.clone();
        view.effect("view keeper", move |scope| {
            let captured = scope.context().clone();
            scope.defer(move || {
                let _: CompositionConfig = captured.config()?;
                retained.log(2, "parent-view-cleanup")
            })
        })?;
        #[derive(serde::Serialize)]
        struct Offset {
            offset: u64,
        }
        let nested = view.intercept::<dyn ScopedCounter, _>(&Offset { offset: 10 })?;
        drop(view);
        let inherited = nested.get::<dyn ScopedCounter>()?;
        if inherited.value()? != 110 || inherited.label()? != "outer" || base.bindings()? != 3 {
            return Err(Error::new(1014, "内层元数据应覆盖offset并继承label"));
        }
        let a = ctx.isolate::<dyn ScopedCounter>(Some("native-a"))?;
        match a.get::<dyn ScopedCounter>() {
            Err(error) if error.code() == 4 => {}
            _ => return Err(Error::new(1015, "隔离层应截断祖先的注入视图")),
        }
        a.plugin::<RealmChild>(&RealmConfig {
            expected: Some(203),
            add: 3,
        })
        .await?;
        drop(a);
        let a = ctx.isolate::<dyn ScopedCounter>(Some("native-a"))?;
        let b = ctx.isolate::<dyn ScopedCounter>(Some("native-b"))?;
        a.plugin::<RealmChild>(&RealmConfig {
            expected: Some(203),
            add: 0,
        })
        .await?;
        b.plugin::<RealmChild>(&RealmConfig {
            expected: Some(300),
            add: 0,
        })
        .await?;
        if base.value()? != 100 {
            return Err(Error::new(1016, "命名域应共享自身状态并隔离其他域"));
        }
        let fresh = ctx.isolate::<dyn ScopedCounter>(None)?;
        match fresh.get::<dyn ScopedCounter>() {
            Err(error) if error.code() == 4 => {}
            _ => return Err(Error::new(1017, "新域应保持服务缺席")),
        }
        fresh
            .plugin::<RealmChild>(&RealmConfig {
                expected: None,
                add: 0,
            })
            .await?;
        nested
            .plugin::<ScopedChild>(&ChildConfig { expected: 111 })
            .await?;
        drop(nested);
        while base.value()? != 101 {
            ctx.sleep(std::time::Duration::from_millis(1)).await?;
        }
        let private = ctx.isolate::<dyn Counter>(None)?;
        private.plugin::<PrivateProvider>(&EmptyConfig {}).await?;
        private.plugin::<PrivateReader>(&EmptyConfig {}).await?;
        Ok(())
    }
}

impl Component for Contract {
    const NAME: &'static str = "contract";
    type Config = ContractConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }
    async fn apply(ctx: Context, config: ContractConfig) -> Result<()> {
        let provider = ctx.get::<dyn Counter>()?;
        let actual = provider.clone_object()?.has_typed_methods();
        if actual != config.expected_typed {
            return Err(Error::new(
                1003,
                format!(
                    "类型化方法表标志期望 {}，实际 {actual}",
                    config.expected_typed
                ),
            ));
        }
        if provider.value()? != 100 {
            return Err(Error::new(1003, "初始计数应为100"));
        }
        if provider.add(2)? != 102 {
            return Err(Error::new(1003, "增加2后应返回102"));
        }
        if provider.value()? != 102 {
            return Err(Error::new(1003, "后续读取应保持102"));
        }
        Ok(())
    }
}

cordis_sdk::export_plugin!(
    name = "loader-consumer",
    version = env!("CARGO_PKG_VERSION"),
    components = [
        Low,
        High,
        Optional,
        Contract,
        ScopedChild,
        RealmChild,
        Composition,
        Idle,
        Dependent,
        PrivateProvider,
        PrivateReader,
        Carrier,
        Callers
    ]
);
