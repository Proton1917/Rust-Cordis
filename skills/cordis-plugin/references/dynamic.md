# 按名称调用服务

Rust SDK 0.2.0 提供 `Context::get_dynamic(interface)`。普通服务在 `Component::inject` 中用 `with::<dyn Interface>()` 或 `with_name("接口@版本")` 声明；可选项用 optional/optional_name。名称与版本共用类型化声明的装载校验。官方插件的 http、secrets、byte-stream 属于普通服务，核心内置服务沿既有取得规则处理。

| Rust 方法 | 参数与结果 | 执行入口 |
|---|---|---|
| invoke | CborValue 位置数组与普通 CBOR 结果 | 同步 V-2 |
| invoke_async | 同上 | 异步 V-2 |
| invoke_as | Serialize 参数、DeserializeOwned 结果 | 同步 V-2 |
| invoke_as_async | 同上，支持 Service 资源字段 | 异步 V-2 |

例如 `service.invoke_as::<_, Option<Bytes>>("get", &("key",))` 读取 KV 字节值。单参数 tuple 保留末尾逗号；零参数用空 `Vec<CborValue>`，unit 的 Serde 表示为 null。异步入口同样可以调用同步方法。

原始 `invoke` 只处理不含资源的纯数据：参数或结果中出现资源引用时返回参数错误（结果中的资源会先被归还）。传递文件流等资源时用 `invoke_as` 系列，把返回类型写成 `Service<dyn ByteStream>` 或含 `Service` 字段的结构；最后一个 `Service` 释放时归还资源。

Python：`remote = await context.get(interface, bindings)`，再 `await remote.invoke(interface, method, args)`。TypeScript：`await context.getDynamic(interface, binding)` 取得 Remote，再调用同名 `invoke`。binding 为 WIT 生成模块。一次性调用用 try/finally 关闭 Remote，长期持有时在效应中登记 defer 关闭。

动态代理与类型化代理在 Pending、替换、回滚和 Scoped 元数据（包括直接调用方身份）上行为相同。只在方法名确实来自运行时输入时用动态入口；常规调用用类型化代理，保留编译期检查和同组直接调用的速度。
