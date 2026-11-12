use cordis_sdk::prelude::*;
use loader_fixture_api::{
    AsyncPulse, Counter, CounterV2, Metadata, NewCounter, Pulse, ScopedCounter,
};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

struct Store {
    value: Mutex<u64>,
    frozen: Frozen,
    bindings: AtomicU64,
    fault: Option<String>,
}

#[cordis_sdk::interface]
impl ScopedCounter for Store {
    fn value(&self) -> Result<u64> {
        self.read()
    }
    fn add(&self, amount: u64) -> Result<u64> {
        self.write(amount)
    }
    fn bindings(&self) -> Result<u64> {
        Ok(self.bindings.load(Ordering::SeqCst))
    }
    fn label(&self) -> Result<String> {
        Ok(String::new())
    }
}

struct Bound {
    store: Arc<Store>,
    metadata: Metadata,
}

#[cordis_sdk::interface]
impl ScopedCounter for Bound {
    fn value(&self) -> Result<u64> {
        Ok(self.store.read()? + self.metadata.offset)
    }
    fn add(&self, amount: u64) -> Result<u64> {
        Ok(self.store.write(amount)? + self.metadata.offset)
    }
    fn bindings(&self) -> Result<u64> {
        Ok(self.store.bindings.load(Ordering::SeqCst))
    }
    fn label(&self) -> Result<String> {
        Ok(self.metadata.label.clone())
    }
}

#[derive(serde::Deserialize)]
struct StoreConfig {
    #[serde(default = "initial")]
    initial: u64,
    #[serde(default)]
    realm: Option<String>,
    #[serde(default)]
    fault: Option<String>,
    #[serde(default)]
    panic_apply: bool,
}

fn initial() -> u64 {
    100
}

#[inline(never)]
fn overflow(depth: usize) {
    let frame = [0u8; 4096];
    std::hint::black_box(&frame);
    if depth != 0 {
        overflow(depth - 1);
    }
    std::hint::black_box(frame);
}

fn inaccessible_memory() -> Result<()> {
    // SAFETY: 验收程序在独立宿主中映射无访问权限的页面，读取触发真实内存访问故障。
    let page = unsafe {
        nix::libc::mmap(
            std::ptr::null_mut(),
            4096,
            nix::libc::PROT_NONE,
            nix::libc::MAP_ANON | nix::libc::MAP_PRIVATE,
            -1,
            0,
        )
    };
    if page == nix::libc::MAP_FAILED {
        return Err(Error::new(1, std::io::Error::last_os_error().to_string()));
    }
    // SAFETY: 本场景明确验证宿主对原生内存访问故障的进程隔离。
    unsafe {
        std::ptr::read_volatile(page.cast::<u8>());
    }
    // SAFETY: 页面由本函数映射；运行环境允许读取时回收该映射并报告失败。
    unsafe {
        nix::libc::munmap(page, 4096);
    }
    Err(Error::new(1019, "内存保护页读取意外成功"))
}

impl Store {
    fn read(&self) -> Result<u64> {
        match self.fault.as_deref() {
            Some("panic") => panic!("native service panic"),
            Some("abort") => std::process::abort(),
            Some("segfault") => inaccessible_memory()?,
            Some("stack") => overflow(usize::MAX),
            Some("exit") => std::process::exit(17),
            Some("spin") => loop {
                std::hint::spin_loop();
            },
            _ => {}
        }
        if cfg!(feature = "method-panics") {
            panic!("fixture method panic");
        }
        Ok(*self.value.lock().unwrap())
    }
    fn write(&self, amount: u64) -> Result<u64> {
        let mut value = self.value.lock().unwrap();
        self.frozen.check()?;
        *value += amount;
        Ok(*value)
    }
    fn implementation(&self) -> Result<u64> {
        Ok(
            if cfg!(any(feature = "version2", feature = "implementation2")) {
                2
            } else {
                1
            },
        )
    }
}

#[cordis_sdk::interface]
impl Counter for Store {
    fn value(&self) -> Result<u64> {
        self.read()
    }
    fn add(&self, amount: u64) -> Result<u64> {
        self.write(amount)
    }
    fn implementation(&self) -> Result<u64> {
        Store::implementation(self)
    }
}
#[cordis_sdk::interface]
impl NewCounter for Store {
    fn value(&self) -> Result<u64> {
        self.read()
    }
    fn add(&self, amount: u64) -> Result<u64> {
        self.write(amount)
    }
    fn implementation(&self) -> Result<u64> {
        Store::implementation(self)
    }
}
#[cordis_sdk::interface]
impl CounterV2 for Store {
    fn value(&self) -> Result<u64> {
        self.read()
    }
    fn add(&self, amount: u64) -> Result<u64> {
        self.write(amount)
    }
    fn implementation(&self) -> Result<u64> {
        Store::implementation(self)
    }
}

struct Provider;
impl Component for Provider {
    const NAME: &'static str = "provider";
    type Config = StoreConfig;
    fn inject() -> Inject {
        if cfg!(feature = "resources") {
            Inject::new().with::<dyn cordis_sdk::builtin::DataDir>()
        } else {
            Inject::new()
        }
    }
    fn provides() -> Provides {
        if cfg!(feature = "scoped") {
            Provides::new().with::<dyn ScopedCounter>()
        } else if cfg!(feature = "incompatible") {
            Provides::new().with::<dyn CounterV2>()
        } else if cfg!(feature = "version2") {
            Provides::new().with::<dyn NewCounter>()
        } else {
            Provides::new().with::<dyn Counter>()
        }
    }
    fn state() -> StateSchema {
        StateSchema::new("loader.counter.state", 1, 1)
    }
    async fn apply(ctx: Context, config: StoreConfig) -> Result<()> {
        if cfg!(feature = "apply-panics") || config.panic_apply {
            panic!("fixture apply panic");
        }
        let value = match ctx.inherited_state()? {
            Some(state) => state.decode::<u64>()?,
            None => config.initial,
        };
        let frozen = Frozen::new();
        let store = Arc::new(Store {
            value: Mutex::new(value),
            frozen: frozen.clone(),
            bindings: AtomicU64::new(0),
            fault: config.fault,
        });
        if cfg!(feature = "resources") {
            resources(ctx.clone(), store.clone()).await?;
        }
        let exported = store.clone();
        ctx.export_state_with("loader.counter.state", 1, frozen, move || {
            if cfg!(feature = "export-fails") {
                return Err(Error::new(1001, "fixture export failure"));
            }
            Ok(*exported.value.lock().unwrap())
        })?;
        if cfg!(feature = "scoped") {
            let view = match config.realm.as_deref() {
                Some(realm) => ctx.isolate::<dyn ScopedCounter>(Some(realm))?,
                None => ctx.clone(),
            };
            let retained = store.clone();
            view.provide_scoped::<dyn ScopedCounter>(Scoped::new(
                store,
                move |metadata: Metadata| {
                    retained.bindings.fetch_add(1, Ordering::SeqCst);
                    Ok(Arc::new(Bound {
                        store: retained.clone(),
                        metadata,
                    }))
                },
            ))?;
        } else if cfg!(feature = "incompatible") {
            ctx.provide::<dyn CounterV2>(store)?;
        } else if cfg!(feature = "version2") {
            ctx.provide::<dyn NewCounter>(store)?;
        } else {
            ctx.provide::<dyn Counter>(store)?;
        }
        let retained = ctx.clone();
        ctx.effect("provider marker", move |scope| {
            scope.defer(move || {
                retained.log(
                    2,
                    if cfg!(any(feature = "version2", feature = "implementation2")) {
                        "provider-cleanup:2"
                    } else {
                        "provider-cleanup:1"
                    },
                )
            })
        })?;
        if cfg!(feature = "apply-fails") {
            return Err(Error::new(1002, "fixture apply failure"));
        }
        Ok(())
    }
}

async fn resources(ctx: Context, store: Arc<Store>) -> Result<()> {
    let directory = ctx.data_dir()?;
    let sync = store.clone();
    ctx.on::<Pulse, _>(move |event| sync.write(event.amount).map(|_| ()))?;
    ctx.on_async::<AsyncPulse, _, _>(move |event| {
        let store = store.clone();
        async move { store.write(event.amount).map(|_| ()) }
    })?;
    ctx.emit(Pulse { amount: 2 })?;
    ctx.emit_async(AsyncPulse { amount: 3 }).await?;
    let asynchronous = directory.join("async-effect");
    let effect = ctx.effect_async("async-file", move |fx| async move {
        std::fs::write(&asynchronous, b"active").map_err(|e| Error::new(1, e.to_string()))?;
        fx.defer_async(move || async move {
            std::fs::remove_file(asynchronous).map_err(|e| Error::new(1, e.to_string()))
        })
    })?;
    effect.settled().await?;
    effect.dispose().await?;
    let marker = directory.join("active-effect");
    let cleanup = ctx.clone();
    ctx.effect("active-file", move |fx| {
        std::fs::write(&marker, b"active").map_err(|e| Error::new(1, e.to_string()))?;
        fx.defer(move || {
            let _: EmptyConfig = cleanup.config()?;
            cleanup.now()?;
            cleanup.log(1, "native cleanup context remains available")?;
            std::fs::remove_file(marker).map_err(|e| Error::new(1, e.to_string()))
        })
    })?;
    let task_context = ctx.clone();
    ctx.spawn(async move {
        loop {
            task_context
                .sleep(std::time::Duration::from_millis(1))
                .await?;
        }
    })?;
    ctx.spawn_thread("native-resource-worker", move |thread_context| {
        while thread_context.is_current()? {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        std::fs::write(directory.join("thread-joined"), b"finished")
            .map_err(|e| Error::new(1, e.to_string()))
    })?;
    Ok(())
}

cordis_sdk::export_plugin!(
    name = "loader-fixture",
    version = env!("CARGO_PKG_VERSION"),
    components = [Provider]
);
