# Rust 接口

## 最小完整示例

使用方组件：注入一个接口，读取服务，在效应中持有文件，监听事件，运行后台轮询。接口 crate 由 `templates/interface.rs` 生成（见下文“共享模板”，此处服务名为 `demo`，crate 以 `service-api` 为依赖名引入）。

```rust
use cordis_sdk::prelude::*;
use service_api::{Changed, Counter};
use std::{io::Write, sync::Mutex, time::Duration};

pub struct Watcher;

impl Component for Watcher {
    const NAME: &'static str = "watcher";
    type Config = EmptyConfig;

    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }

    fn listens() -> Listens {
        Listens::new().with::<Changed>()
    }

    async fn apply(context: Context, _config: EmptyConfig) -> Result<()> {
        // 必需依赖缺席时核心不会调用 apply，这里的 get 一定能取到。
        let counter = context.get::<dyn Counter>()?;
        context.log(2, &format!("当前值 {}", counter.value()?))?;

        // 资源在效应中取得，清理登记在同一作用域；停用或替换时核心逆序执行清理。
        let path = context.data_dir()?.join("changes.log");
        let file = std::fs::File::create(&path).map_err(|error| Error::new(2, error.to_string()))?;
        let file = Arc::new(Mutex::new(file));
        let held = file.clone();
        context.effect("changes-log", move |scope| {
            scope.defer(move || {
                held.lock().unwrap().flush().map_err(|error| Error::new(2, error.to_string()))
            })
        })?;

        // 监听器随实例停用自动撤回，不需要手动注销。
        context.on::<Changed, _>(move |event| {
            writeln!(file.lock().unwrap(), "{}", event.value)
                .map_err(|error| Error::new(2, error.to_string()))
        })?;

        // 后台任务归属当前实例：停用时宿主终止并等待它，循环用 is_current 判断退出。
        let task = context.clone();
        context.spawn(async move {
            while task.is_current()? {
                task.sleep(Duration::from_secs(5)).await?;
                task.log(3, &format!("轮询 {}", counter.value()?))?;
            }
            Ok(())
        })
    }
}

cordis_sdk::export_plugin!(
    name = "watcher",
    version = env!("CARGO_PKG_VERSION"),
    components = [Watcher]
);
```

提供方的完整写法（共享状态锁、冻结、导出、事件）见 `templates/provider.rs`。

## 组件与声明

`Component::Config` 实现 `serde::de::DeserializeOwned + Send + Sync + 'static`；空配置用 `EmptyConfig`。`apply(ctx, config)` 是异步函数，返回 `Result<()>`；`resolve(&config)` 只做配置检查。

| 声明 | 写法 |
|---|---|
| 必需依赖 | `Inject::new().with::<dyn I>()`；只有接口名时 `with_name("名称@版本")` |
| 可选依赖 | `optional::<dyn I>()`、`optional_name(...)` |
| 提供的服务 | `fn provides() -> Provides { Provides::new().with::<dyn I>() }`，在 apply 中 `context.provide::<dyn I>(Arc::new(...))` |
| 监听的事件 | `Listens::new().with::<E>()` |
| 状态 | `fn state() -> StateSchema { StateSchema::new("名称", 最低版本, 最高版本) }` |

服务实现用 `Arc` 共享。`context.get::<dyn I>()` 返回 `Service<dyn I>`，同组与跨组调用用同一份源码，调用路径由宿主选择。

## 数据与版本

接口方法返回 `Result<T>`。拥有型字符串和字节用 `String` 与 `Vec<u8>`；结构中的字节字段用 `cordis_sdk::Bytes`。复杂类型派生 serde 序列化。资源参数用 `Service<dyn I>`，SDK 管理跨进程引用。

接口主版本是行为兼容边界，次版本表示追加能力。追加方法放在 trait 末尾；数据类型新增字段给出 serde 缺省值。使用方声明的次版本是它需要的最低版本。

## 隔离与拦截

`ctx.isolate::<dyn I>(Some("tenant-a"))` 进入命名域，同名域共享服务集合；`None` 创建独有域。`ctx.intercept::<dyn I, _>(&metadata)` 叠加接口元数据；需要读取元数据的提供方用 `Scoped` 与 `provide_scoped` 为每个使用方创建视图。

`ctx.plugin::<Child>(&config).await` 在当前视图挂载本插件 `export_plugin!` 中列出的子组件。父实例释放完成时，子实例与其服务已回收。

## 事件

事件类型用 `#[cordis_sdk::event(name = "名称", version = "1.0")]`，异步事件加 `kind = "async"`。同步事件用 `ctx.on`、`ctx.emit`，异步事件用 `ctx.on_async`、`ctx.emit_async`。跨进程监听器收到的是通知；需要返回值时用服务方法。

## 共享模板

`templates/` 下的文件可直接复制，把 `{{name}}` 换成服务名：

| 目标目录 | 清单 | 源文件 |
|---|---|---|
| `interfaces/<name>-api` | `interface.toml` | `interface.rs` → `src/lib.rs` |
| `plugins/<name>` | `provider.toml` | `provider.rs` → `src/lib.rs` |
| `contracts/<name>-contract` | `contract.toml` | `contract.rs` → `src/lib.rs` |

创建后把 `interfaces/*`、`contracts/*` 加入项目 workspace members，并为 `cordis-contract` 声明与 `cordis-sdk` 相同的来源。provider 模板展示共享状态锁、冻结与事件；contract 模板以真实读写检查跨阶段状态继承。
