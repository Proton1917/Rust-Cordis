# ADR-0004 服务键：以类型为键，保留名称

| 项 | 内容 |
|---|---|
| 状态 | 已接受（2026-11-11） |
| 影响 | `DOC-00` `P5`、`R4`；`DOC-02` §3.1、§6、§7 |

## 问题

`PAPER` 定义 12 的余效应上下文是 `(k : K) ⇀ V_k`：每个键 `k` 关联一个值类型 `V_k`。`UPSTREAM` 以字符串为键，值类型通过 TypeScript 的模块扩充声明在 `Context` 接口上，运行时不检查。Rust 需要决定键是什么，以及 `V_k` 如何表达。

## 选项

| 选项 | 内容 | 代价 |
|---|---|---|
| A | 字符串键，值为 `Arc<dyn Any>`，使用点向下转型 | 类型错误推迟到运行期；`PAPER` §5.5 的键冲突问题原样存在 |
| B | 以值的具体类型为键（`TypeId::of::<V>()`） | 一个接口只能有一个实现类型；无法以 trait 对象为服务 |
| C | 独立的键类型 `K: ServiceKey`，关联类型 `Value` 给出 `V_k`，关联类型 `Meta` 给出 `M_k`，常量 `NAME` 给出名称 | 每个服务多写一个键类型 |

## 判据

1. `PAPER` §5.3：语言应当让依赖访问具有良好类型，Rust 的 trait 可以承担这一点。
2. 服务常以接口形式存在，提供方可以更换（`PAPER` §5.1 的独占绑定）。
3. 后续的加载器按名称引用服务（配置文件中的 `isolate`、`intercept` 字段）。

## 决定

选 C。

```rust
pub trait ServiceKey: 'static {
    const NAME: &'static str;
    type Value: ?Sized + Send + Sync + 'static;   // V_k；可以是 dyn Trait
    type Meta: Merge + Clone + Send + Sync + 'static;   // M_k；无元数据时取 ()
}
```

- 存储表以 `(KeyId, RealmId)` 为键，`KeyId` 由 `TypeId::of::<K>()` 得到。
- `ctx.get::<K>()` 的返回类型由 `K::Value` 决定，使用点不做向下转型。
- 内核维护名称登记表 `NAME → KeyId`。两个不同的键类型声明同一个 `NAME` 时，后登记者得到错误。这把 `PAPER` §5.5 的键冲突从静默故障变为登记时的错误。

## 后果

- 错误消息、`internal/service` 事件、日志使用 `NAME`，与 `UPSTREAM` 的消息文本一致（`DOC-01` `S-SVC-7`）。
- 按名称的动态查找接口不在本次交付内；名称登记表为它保留了数据。
- `K::Value` 为 `dyn Trait` 时，值以 `Arc<dyn Trait>` 存放；存储表内部把它装入 `Box<dyn Any + Send + Sync>`。
