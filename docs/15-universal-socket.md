# 15 核心隔离与两种插口：修订计划

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-15` |
| 状态 | 已确认（2026-11-12）。`D-1`–`D-13` 均由项目所有者确认；同日按项目所有者要求增补前端插件（`D-9`）、“核心之外一切皆插件”（`D-10`–`D-12`）与隔离域、拦截、组合跨进程（`D-13`） |
| 读者 | 项目所有者、实现者（Codex）、评审者 |
| 输入 | `DOC-10`–`DOC-14`；`ADR-0009`–`ADR-0017`；项目所有者的要求（§1） |
| 输出 | 新的首要公理 `Q0`；进程架构；核心与开发工具分离；管理接口 `cordis.manage@1`；两种插口与插件分组；核心保护措施 `CP-1`–`CP-11`；核心自身崩溃的恢复；跨进程调用与 ABI 修订 `V-1`–`V-4`；隔离域、拦截与组合跨进程；接口描述语言；通用插口协议 `UP/1` 的设计要点；SDK；前端插件与全栈插件；项目脚手架与发布打包；对第二阶段文档的修订清单；重排后的里程碑；验收场景 `AU-1`–`AU-18` |
| 不包含 | `UP/1` 的逐字段规范（实现中产出 `docs/16-up1-protocol.md`）；WebAssembly 插口（§13） |

## 1 要求

项目所有者的要求：

1. 核心是系统中最稳定的部分，长期不动。任何插件的错误都不能使核心崩溃、卡住或损坏。
2. 核心是纯 Rust 的单一可执行文件，不内嵌任何其他语言的运行时。
3. 两种插口：
   - 高速插口：Rust（以及其他能产出 C 接口动态库的语言）编写的原生插件，速度优先。
   - 通用插口：任何语言编写的插件，首先支持 Python 与 TypeScript。
4. 速度让位于核心的稳定。隔离运行的 Rust 插件仍应明显快于 Python 与 TypeScript 插件。
5. 除核心以外一切都是插件：前端、数据库、网页服务、桌面窗口都是插件。前端与后端之间不存在插件接口以外的约定，不做前后端联调；一方只依赖另一方提供的接口（WIT），任何一方都可以单独替换。
6. 两种插口上的插件属于同一个系统：同样的接口命名、依赖、生命周期、热替换，可以互相调用。

## 2 公理 `Q0`：核心进程不运行插件代码

第二阶段的设计把原生插件装进核心进程（`ADR-0009`），并以 `Q8`“原生插件是可信代码”接受了由此带来的风险：插件中的段错误、栈溢出、`abort`、内存破坏都会终止或损坏整个核心进程。操作系统以进程为单位处理这些故障，同一进程内无法隔离。这与要求 1 冲突。

新增公理 `Q0`，排在 `DOC-10` 全部公理之前：

> 核心进程只运行核心自己的代码。所有插件，无论用什么语言编写，都在核心以外的进程中运行。核心与插件之间只交换消息。

`Q0` 的直接推论：

1. 核心进程不调用 `dlopen`，不链接 `cordis-abi` 与 `cordis-sdk`，不执行任何插件提供的函数指针。核心提供的内置接口（`cordis.log@1`、`cordis.data-dir@1`、`cordis.runtime@1`、`cordis.contract@1`）经 `UP/1` 提供。
2. 核心的全部 crate 都不使用 `unsafe`。`unsafe` 只存在于插件宿主程序 `cordis-host` 与 `cordis-abi` 中。
3. 插件进程崩溃时，核心回收它的全部效应与服务，依赖方转为 `Pending`，然后按策略重启该进程。
4. `DOC-10` 的 `Q8` 删除，`R7`（调用已失效库的代码）只存在于插件宿主进程内，不再威胁核心。
5. 系统中只有一个内核，位于核心进程。实例、依赖、纪元、加载与卸载、替换、回滚只由它决定。`cordis-host` 不依赖内核 crate：它按核心经 `UP/1` 下发的指令执行插件函数，用普通数据结构管理库、租约与对象编号对照表。插件登记的效应、服务、监听器与导出器都经 `UP/1` 记录在核心内核中，宿主进程崩溃时核心能完整回收。

## 3 进程架构

```
cordis（核心进程，纯 Rust，无 unsafe，不含插件代码）
├── 内核                 生命周期、依赖、效应回退、纪元
├── 进程监管             启动、监视、重启插件进程；超时；资源上限
├── 路由                 跨进程的服务调用、事件、回调
└── 配置与状态           cordis.toml、目录监视、状态目录
      │  UP/1（标准输入输出上的 CBOR 消息帧）
      ├──────────────── cordis-host 进程（分组 "math"）
      │                   ├── libmath_core.dylib   ┐ 同组插件之间
      │                   └── libmath_solver.dylib ┘ 直接函数调用
      ├──────────────── cordis-host 进程（分组 "storage"）
      │                   └── libkv_file.dylib
      ├──────────────── python -m kv_py           （通用插口）
      ├──────────────── node dist/main.js         （通用插口）
      └──────────────── cordis-host 进程（分组 "http"）
                          └── libhttp.dylib       网页服务；转发页面的 UP/1（§11.3）
                                │  WebSocket（UP/1）
                                ├── 浏览器页面 A   （网页宿主，通用插口）
                                └── 浏览器页面 B   （网页宿主，通用插口）
```

| 进程 | 程序 | 运行什么 | `unsafe` |
|---|---|---|---|
| 核心 | `cordis` | 内核、监管、路由、配置、管理接口（§3.2） | 无 |
| 插件宿主 | `cordis-host`（本项目交付的 Rust 程序，不含内核） | 一个分组内的原生插件库，经 ABI v1 装载；只执行核心的指令 | 有，集中在 `cordis-abi` 与库装载 |
| 通用插件 | 插件自己的程序（Python、Node 等） | 一个通用插口插件 | 与核心无关 |
| 网页宿主 | 浏览器页面中的 `cordis-web` 脚本（本项目交付，不含内核） | 该页面上全部前端插件的浏览器模块；只执行核心的指令 | 与核心无关 |
| 开发工具 | `cordis-dev`（开发机上按需运行，不是运行中系统的一部分） | 脚手架、代码生成、验证、打包（§3.1）；不含内核，不装载插件代码 | 无 |

### 3.1 核心可执行文件只含运行功能

核心长期不动，因此只包含运行系统必需的功能。经常变化的开发工具放在独立程序 `cordis-dev` 中，与核心分别发布、分别升级。

| 程序 | 命令 | 依赖 |
|---|---|---|
| `cordis`（核心） | `run`、`status`（读取 `state/status.json`）、`service install`、`version` | 核心 crate |
| `cordis-dev`（开发工具） | `new`、`new-plugin`、`idl gen`、`verify`、`pack` | `wit-parser` 等工具依赖，不进入核心的依赖树 |

1. `cordis` 收到自己不认识的子命令时，执行与它同目录的 `cordis-dev`（其次为 `PATH` 中的 `cordis-dev`），参数原样传递，退出码原样返回。使用者照常写 `cordis new`、`cordis pack`。核心中只有这段转交逻辑，不含任何工具的代码；找不到 `cordis-dev` 时报告“该命令由开发工具 cordis-dev 提供，当前未安装”。
2. `cordis-dev` 不含内核，不装载插件代码。`verify` 在 `target/` 下的临时目录生成部署配置，启动一个 `cordis run` 进程执行契约套件并读取结果；`pack` 读取原生插件描述符时调用 `cordis-host --describe <库>`，`cordis-dev` 自身不调用 `dlopen`。
3. 开发工具升级不要求升级核心。`cordis-dev` 声明它支持的核心版本范围，运行时与项目的 `requires_core` 一并检查。
4. 发布包（§12.2）只含 `cordis` 与 `cordis-host`，不含 `cordis-dev`。

### 3.2 管理接口 `cordis.manage@1`

实例的装载、卸下、替换、回滚由内核决定。核心在启动时按配置装载实例；运行中的变更由持有授权的插件经 `cordis.manage@1` 请求，管理后台、插件更新、开发时的自动热替换都是这样的插件。`cordis.runtime@1` 保持只读，任何实例都可使用。

| 方法 | 作用 |
|---|---|
| `install(source) -> 插件引用` | 把一个插件文件或目录复制进 `plugins/`，按序号命名；`source` 必须位于项目目录或调用方的数据目录（`cordis.data-dir@1`）之内；与已有插件完整字节相同时返回已有引用 |
| `add(name, plugin, group, config)` | 新增实例 |
| `remove(name)` | 卸下并删除实例 |
| `replace(name, plugin)` | 按 `DOC-11` §6 热替换 |
| `replace_batch(replacements, reload, request_id)` | `cordis.manage@1.1`：持久化受理后返回 BatchTicket；按最终候选集合执行整批替换，reload 为 true 时包含配置变化，Runtime@1.2 的 last_batch 发布实际结果，见 ADR-0031 |
| `rollback(name)` | 回到该实例的上一版本（`DOC-11` §7） |
| `configure(name, config)` | 更新实例配置，按内核的 `update` 语义处理 |
| `reload()` | 重读 `cordis.toml`，调和与当前实例的差异 |
| `shutdown()` | 核心有序退出（写出检查点，按依赖逆序卸下全部实例） |

1. **授权**只能写在 `cordis.toml` 的实例条目中，不能经 `cordis.manage@1` 授予或扩大：

   ```toml
   [instance.admin]
   plugin = "admin-console"
   grants = { "cordis.manage@1" = ["*"] }

   [instance.window]
   plugin = "window"
   grants = { "cordis.manage@1" = ["shutdown"] }
   ```

   未获授权的实例注入该接口时得不到服务，依赖保持 `Pending`，日志说明原因；调用授权以外的方法返回权限错误。
2. **核心是唯一决策者**：每个操作按 `DOC-11` 的步骤执行，失败即回滚；操作串行执行，带截止时间（`CP-4`）。
3. **持久化**：经该接口做出的变更写入叠加层 `state/instances.toml`，启动时叠加在 `cordis.toml` 之上。`cordis.toml` 由人维护，核心不改写。叠加层中的同名条目覆盖 `cordis.toml`，`cordis status` 标出被覆盖的条目。核心崩溃重启后（§5.1）变更仍然有效。
4. **审计**：每个操作经 `cordis.log@1` 记录调用方实例、方法、目标实例与结果。
5. 调用方对自身的 `remove`、`replace` 在调用返回之后执行。

### 3.3 文件变更触发热替换：`watch` 插件

`plugins/watch`（原生插件）监视配置的路径（插件构建产物、Python 与 TypeScript 插件目录、`cordis.toml`），变化稳定后（`debounce_ms`，默认 300）调用 `cordis.manage@1.1` 的 `install` 与 `replace_batch`，核心负责部署事务。

按 ADR-0031，防抖窗口中的插件来源先全部安装，再以一次 `replace_batch` 提交；同窗配置变化设置 `reload = true`。核心对全部候选预检并统一提交定义，失败恢复窗口开始前的配置、版本与原状态。原生批次保持同组其他实例的定义身份。watch 保存被拒批次的实际副本并逐字节比较，重复通知保持当前部署。

```toml
[instance.watch]
plugin = "watch"
grants = { "cordis.manage@1" = ["install", "replace_batch", "reload"] }
config = { debounce_ms = 300 }
```

`cordis new` 生成的项目配置包含这一条目；发布包是否包含 `watch` 由项目决定。

## 4 两种插口与分组

### 4.1 高速插口

原生插件按 `DOC-12` ABI v1 编译成动态库，不变。变化在于装载它的是 `cordis-host` 进程，不是核心。

`cordis.toml` 为每个原生插件指定分组：

```toml
[instance.solver]
plugin = "math-solver"
group = "math"          # 同组插件装进同一个 cordis-host 进程
```

| 调用关系 | 路径 | 速度 |
|---|---|---|
| 同组原生插件之间 | ABI v1 类型化函数表，直接调用 | 与第二阶段相同 |
| 不同分组、或原生与通用插件之间 | 经核心路由的 `UP/1` 消息 | 一次往返约为两次进程间消息，`RM-7` 实测 |

分组是部署时的选择，插件不需要为此重新编译：

- 调用频繁、彼此信任的插件放进同一组，获得直接调用的速度。代价是同组插件共担崩溃：一个出错，整组进程重启。
- 新写的、AI 生成的、依赖 C 库的插件单独成组，出错只影响自己。
- 默认每个插件库自成一组。

### 4.2 通用插口

任何语言编写的程序，读写标准输入输出上的 `UP/1` 消息即可成为插件。每个通用插件一个进程，清单 `cordis-plugin.toml` 声明启动命令（§9）。

通用插口有两种传输，传送相同的 `UP/1` 消息：

| 传输 | 对端 | 谁建立连接 |
|---|---|---|
| 子进程的标准输入输出 | 进程插件、`cordis-host` | 核心启动子进程 |
| 浏览器页面的 WebSocket，经 `http` 插件转发 | 网页宿主（§11.2） | 页面打开时连接 |

前端插件因此也是通用插口上的插件，与其他插件共用接口命名、依赖、生命周期与热替换。

### 4.3 两种插口在核心看来相同

核心只认 `UP/1`。`cordis-host` 对核心而言就是一个用 `UP/1` 通信的插件进程，它代表组内的全部原生插件说话；浏览器页面对核心而言是一条经转发的 `UP/1` 连接，它代表页面上的全部前端插件说话。核心中的进程监管、路由、热替换、崩溃处理对两种插口完全相同。

## 5 核心保护措施

核心把每个插件进程发来的消息都当作不可信输入处理。

| 编号 | 措施 | 防止的情况 |
|---|---|---|
| `CP-1` | 插件代码不在核心进程中运行（`Q0`） | 段错误、栈溢出、`abort`、内存破坏 |
| `CP-2` | 每条消息的帧长度上限（默认 16 MiB），超限即断开该进程 | 用超大消息耗尽核心内存 |
| `CP-3` | 每个进程的在途请求数上限与发送队列上限，满时对该进程背压，不阻塞核心的其他工作 | 消息洪泛 |
| `CP-4` | 核心发出的每个请求都有截止时间（`apply`、清理、导出、调用各有默认值，可配置）；超时按失败处理，记录在该实例上 | 插件卡死使核心等待 |
| `CP-5` | 核心从不在持有内核锁或路由锁时等待插件进程（延续 `A-3` 与 `B-8`） | 插件响应慢导致核心停顿 |
| `CP-6` | 进程退出、协议错误或连续超时时，核心终止该进程，回收它的全部编号、效应与服务，依赖方转 `Pending`；按 `restart` 策略重启（默认指数退避，最多 5 次，之后标记为失败并停止重启） | 坏插件反复拖累系统 |
| `CP-7` | 插件进程以独立进程组启动；核心退出时终止全部插件进程 | 残留进程 |
| `CP-8` | 核心解码 CBOR 时限制嵌套深度与集合长度 | 构造的畸形数据造成核心崩溃 |
| `CP-9` | `UP/1` 解码器、`cordis.toml` 解析器、插件清单解析器、状态块读取接受模糊测试（`cargo fuzz`，nightly 工具链）；每个目标连续运行 30 分钟无 panic、无超时 | 核心自身代码中未被常规测试覆盖的输入处理缺陷 |
| `CP-10` | 插件 SDK 与 `cordis-host` 在标准输入关闭时立即退出 | 核心退出或崩溃后残留孤儿插件进程 |
| `CP-11` | 核心按 `checkpoint_ms`（默认 60000，`0` 表示关闭）定期调用全部导出器，状态块写入 `state/`；有序退出时再写一次 | 核心崩溃后内存状态全部丢失 |

进程的内存与 CPU 配额（`setrlimit`、cgroup）列为后续工作（§13）。

### 5.1 核心自身崩溃的恢复

`Q0` 与 `CP-1`–`CP-9` 降低核心崩溃的概率，不能把它降为零：单机系统总有一个最底层的进程。核心崩溃时的恢复链如下。

| 环节 | 做法 |
|---|---|
| 拉起核心 | 由操作系统的服务管理器负责：macOS 的 launchd（`KeepAlive`），Linux 的 systemd（`Restart=always`）。`cordis` 随附两者的配置模板，由 `cordis service install` 生成 |
| 清理插件进程 | 核心的标准输入输出随进程消失，插件进程按 `CP-10` 自行退出；`CP-7` 的进程组保证没有遗漏 |
| 恢复插件 | 核心重启后按 `cordis.toml` 重新装载全部插件 |
| 恢复状态 | 持久化数据在插件的数据目录中，不受影响；内存状态从最近一次检查点（`CP-11`）继承，最多丢失一个检查点间隔内的变化 |

对外表现：核心崩溃造成一次短暂中断，系统随后回到最近检查点时的状态。验收场景 `AU-11` 验证这条链。

## 6 跨进程调用

### 6.1 通用调用

跨进程只能传递数据，不能传递函数指针。每个服务对象除了类型化方法表，还必须支持通用调用：按方法名调用，参数为 CBOR 数组，结果为 CBOR 值。

| 调用方 | 提供方 | 路径 |
|---|---|---|
| 原生 | 同组原生 | 类型化方法表，直接调用 |
| 原生 | 其他进程 | `cordis-host` 给调用方一个代理对象，代理没有类型化方法表；SDK 生成的调用方代码自动把参数编码为 CBOR，走通用调用，`cordis-host` 把它发给核心路由 |
| 其他进程 | 原生 | `cordis-host` 收到路由来的调用，执行提供方对象的通用调用入口（由 `#[interface]` 宏生成：解码参数、调用 trait 方法、编码结果） |
| 通用插件 | 通用插件 | 核心在两个进程之间转发 |

### 6.2 ABI v1 冻结前必须完成的修订

通用调用改变 `ObjectHeader` 的布局。`M8` 提交 `b792b2e` 已将 ABI 冻结为 v1.0，该冻结在 `M9` 中解除，`DOC-12` 状态改回“候选 v1.0”；ABI v1 改在 `M11` 的真实跨库测试通过后冻结。

| 编号 | 修订 | 所在文档 |
|---|---|---|
| `V-1` | `ObjectHeader` 增加 `flags: u32`（位 0：存在类型化方法表）、`methods: *const AbiStr` 与 `method_count: u64`（方法名表）、`invoke(data, method: AbiStr, args: AbiBytes, out: *mut AbiBuf, err) -> AbiStatus`、`invoke_async(data, method, args, out: *mut AbiFuture, err) -> AbiStatus`。位 0 为 0 时首部之后没有类型化方法表 | `DOC-12` §7 |
| `V-2` | `#[interface]` 宏为每个接口同时生成类型化方法表与通用调用入口；生成的调用方代理在类型化方法表缺失时走通用调用 | `DOC-13` §3.3 |
| `V-3` | 通用调用的参数为 CBOR 数组，按参数声明顺序；结果为单个 CBOR 值，`()` 编码为 `null`；未知方法名返回 `code = 3` | `DOC-12` §7.3 |
| `V-4` | 描述符 `PluginV1.format`：`1` 原生库，`2` 通用插口进程插件，`3` 保留给 WebAssembly | `DOC-12` §10 |

### 6.3 事件跨进程的语义

内核在核心进程中运行，插件的监听器在其他进程中。核心等待远端监听器同步返回，会违反 `CP-4`、`CP-5`。因此：

- 插件登记的监听器一律以异步方式被调用：同步事件对远端监听器是通知，不等待返回值。
- 需要收集返回值的派发（`bail`、`waterfall`）只用于核心内部的监听器。插件之间需要“请求—回答”时使用服务调用。

这一条写入 `UP/1` 规范与插件作者手册（`M15` skill）。

### 6.4 隔离域、拦截与组合跨进程

内核实现了论文的空间组合能力（`T-COE-7`、`T-COE-11`、`S-CTX-3`、`S-CTX-4`、`S-SVC-9`）：`isolate` 让一组实例拥有独立的服务集合，`intercept` 在一个范围内覆盖服务的元数据而不修改提供方，组件可以在派生的上下文中装入子组件。插件经 `UP/1` 与内核交互，因此这三项能力都要经 `UP/1` 提供，插件才能使用。

**部署层**：`cordis.toml` 的实例条目可以声明隔离域与拦截，核心在创建该实例的上下文时应用。

```toml
[instance.kv-a]
plugin = "kv-file"
isolate = { "kv@1" = "tenant-a" }       # 该实例提供与读取的 kv@1 都在域 tenant-a 中
config = { path = "a.db" }

[instance.app-a]
plugin = "app"
isolate = { "kv@1" = "tenant-a" }       # 读到 kv-a

[instance.app-b]
plugin = "app"                          # 未声明 isolate，读默认域中的 kv@1
intercept = { "kv@1" = { namespace = "app-b" } }   # 提供方在该实例的调用中读到这份元数据
```

`isolate` 的值为域名字符串（同名即同域，全核心共用一个命名空间）或 `true`（生成新域）。未声明的接口解析到默认域。

**插件层**：`UP/1` 增加两条宿主到核心的请求。

| 方法 | 参数 | 结果 | 内核操作 |
|---|---|---|---|
| `context_derive` | `context: u64, isolate: Vec<{interface: String, realm: Option<String>}>, intercept: Vec<{interface: String, meta: Value}>` | `{context: u64}` | 依次执行 `isolate_named`、`intercept_named`；`realm = None` 生成新域 |
| `plugin` | `context: u64, component: String, config: Value` | `{instance: u64}` | 在给定上下文中装入调用方插件自己的一个组件，作为子实例 |

1. 派生上下文与父上下文属于同一个实例，有效期相同；`provide`、`get`、`effect`、`on`、`emit` 都接受派生上下文的编号。
2. `plugin` 只能装入调用方所在插件的 `describe` 中列出的组件。子实例是调用方的一项效应：调用方卸下时子实例先被卸下；子实例的 `apply` 照常经 `apply` 消息发给同一连接。装入其他插件的组件经 `cordis.toml` 或 `cordis.manage@1` 完成。
3. 浏览器页面的独立域（§11.2 第 4 条）由核心经同一机制创建。

**拦截元数据送达提供方**：提供方读取调用方所在范围的拦截元数据，不能经额外的 ABI 参数传递，因为同组原生调用不经过核心，`invoke` 的签名也在 `M11` 冻结。做法是按句柄绑定：

1. 需要读取元数据的提供方在 `provide` 中声明 `scoped: true`，SDK 为该对象实现保留方法 `cordis.scope(meta) -> object`，返回一个绑定了该元数据的视图对象。
2. 使用方 `get` 时，若其上下文对该接口合并后的元数据（`S-SVC-9`）非空且提供方为 `scoped`，核心经通用调用 `invoke(provider, "cordis.scope", [meta])` 取得视图对象，返回给使用方；元数据为空或提供方未声明 `scoped` 时返回原对象。视图对象按（使用方上下文，绑定）缓存。
3. 视图对象与原对象在同一宿主中，同组原生使用方仍可直接调用。ABI 不需要新增对象级入口；`cordis.scope` 以 `V-1` 的通用调用实现。
4. 原生 SDK 为插件作者提供 `ctx.isolate`、`ctx.intercept`、`ctx.plugin` 与 `Scoped` 提供方辅助，宿主函数表 `HostV1` 在 `M11` 冻结前追加 `context_derive` 与 `plugin` 两个槽位；Python、TypeScript SDK 提供同名接口。

`bail`、`waterfall` 等需要收集返回值的事件派发仍只用于核心内部（§6.3）。

## 7 接口描述语言

接口以 WIT（WebAssembly Interface Type）文件为唯一来源，各语言的代码由 `cordis idl gen` 生成。

| 判据 | WIT 的情况 |
|---|---|
| 公开规范，独立于本项目 | WebAssembly 组件模型的标准接口语言 |
| 有 Rust 解析库 | `wit-parser`，只用在开发工具 `cordis-dev` 的 `idl gen` 中，不进入核心可执行文件（§3.1） |
| 能表达异步方法 | WASI 0.3 已发布，WIT 支持 `async func`、`future<T>`、`stream<T>` |
| 今后接入 WebAssembly 插口 | 同一份 WIT 直接用于 wasm 组件 |

| WIT | Rust | Python | TypeScript | CBOR |
|---|---|---|---|---|
| `interface kv`，包版本 `1.2.0` | `#[interface(name = "kv", version = "1.2")] trait Kv` | `class Kv(Protocol)` | `interface Kv` | — |
| `func` / `async func` | 方法 / `async fn` | 方法 / `async def` | 方法 / 返回 `Promise` | — |
| `record` | `struct`（serde） | `@dataclass` | `interface` | 映射，键为字段名 |
| `variant` / `enum` | `enum` | 标记联合 | 判别联合 | 单键映射 / 字符串 |
| `option<T>` | `Option<T>` | `T \| None` | `T \| null` | 值或 `null` |
| `result<T, E>` | `Result<T>` | 返回值或抛出 `CordisError` | 返回值或抛出 `CordisError` | — |
| `resource` | `Service<dyn I>` | 服务代理 | 服务代理 | 对象编号 |

接口版本取 WIT 包版本：主版本与次版本对应 `DOC-11` §3，修订号不参与匹配。

备选方案：自定义接口格式（需自行维护语法与文档）；以 Rust trait 为来源（Python 与 TypeScript 作者仍要读写 Rust）。两者都不满足“各语言作者只读一份中立定义”。

## 8 通用插口协议 `UP/1`（设计要点）

1. **传输**：子进程标准输入（核心到插件）与标准输出（插件到核心）；每帧为 4 字节大端长度加一个 CBOR 消息。标准错误输出由核心逐行转入日志。浏览器页面经 WebSocket 接入，每条二进制 WebSocket 消息承载一个 CBOR 消息，由 `http` 插件经第 9 条的转发通道交给核心。核心侧的连接抽象不依赖具体传输：内核、路由与编号表只看到“一条 `UP/1` 连接”。
2. **消息种类**：请求（带编号）、响应、通知。双向都可发请求，可同时在途多个。
3. **消息集**，与 `DOC-12` 的宿主函数表对应：
   - 核心到插件：`hello`、`apply`、`invoke`、`callback`（清理、事件监听器）、`export`、`thaw`、`release`、`cancel`、`shutdown`。
   - 插件到核心：`describe` 的响应、`log`、`provide`、`get`、`invoke`、`effect`、`on`、`emit`、`export_state`、`inherited_state`、`config`、`release`、`context_derive`、`plugin`（§6.4）。
4. **效应**：效应体在插件进程内执行；插件把效应及其清理回调的编号登记到核心。内核按后进先出顺序回退时，向插件发送 `callback` 执行清理。
5. **对象与回调**：跨进程只传编号。编号由创建方分配，持有方用完发送 `release`。进程退出时核心整表回收。
6. **替换**：新进程完成握手、新实例进入 `Active` 后，核心向旧进程发送 `shutdown`，超时后终止。旧进程退出即旧代码完全卸载，内存全部归还操作系统。
7. **分组内替换**：`cordis-host` 内替换一个原生库时沿用 `ADR-0012`（保证旧代码静止，映像不保证移除）。`cordis-host` 报告累积的旧映像数，超过阈值（默认 20）时核心对该组做一次有状态重启：导出组内状态、启动新的 `cordis-host`、继承状态、终止旧进程。
8. **版本**：`hello` 交换协议版本，主版本不一致时拒绝装载。
9. **转发通道**：`channel_open`（转发方到核心，带转发方分配的通道编号与连接元数据）、`channel_frame`（双向，带通道编号与一个完整的内层 `UP/1` 消息）、`channel_close`（双向）。核心把每个通道当作一条独立连接：独立握手、独立编号表、独立执行 `CP-2`、`CP-3`、`CP-8`；内层消息不得再包含转发通道消息。只有 `cordis.toml` 中标为转发方的插件（`http`）可以发送 `channel_open`。转发方进程退出时，它的全部通道按连接断开处理。`M10` 的规范定义这三条消息并在连接抽象中预留通道，实现与测试在 `M20`。

## 9 插件形态

每个插件可以带一份清单 `cordis-plugin.toml`。清单的 `socket` 决定插件的后端部分如何接入，`[web]` 段（§11）声明浏览器部分，两者可以任意组合。

| `socket` | 后端部分 | 清单 | 可带 `[web]` |
|---|---|---|---|
| `"native"` | 动态库，由 `cordis-host` 按分组装载（§4.1） | 不带 `[web]` 时可省略，`cordis.toml` 直接引用动态库 | 是 |
| `"process"` | 一个进程（§4.2） | 必需 | 是 |
| `"web"` | 无，插件只有浏览器部分 | 必需 | 必需 |

原生插件：一个动态库文件，放入插件目录，由 `cordis.toml` 的实例条目引用并指定分组（§4.1）。需要附带前端时，插件为一个目录：

```
plugins/kv-file/
├── cordis-plugin.toml
├── libkv_file.dylib        # 构建产物，由 cargo 构建或 cordis pack 放入
└── web/
    └── index.js
```

```toml
name = "kv"
version = "0.1.0"
socket = "native"
library = "kv_file"         # 按平台补全为 libkv_file.dylib、libkv_file.so

[web]
assets = "web"
mount = "/kv"
entry = "web/index.js"
```

清单的 `name`、`version` 必须与动态库描述符 `PluginV1` 中的一致，不一致时拒绝装载。分组仍由 `cordis.toml` 指定，不写在清单中。

通用插件：一个目录。

```
plugins/kv-py/
├── cordis-plugin.toml
├── kv.wit
└── kv_py/__main__.py
```

```toml
name = "kv-py"
version = "0.1.0"
socket = "process"
protocol = 1
command = ["python", "-m", "kv_py"]     # 解释器由部署环境提供，核心不包含
restart = { max = 5, backoff_ms = 500 }

[web]                                    # 可选，见 §11
assets = "web"
mount = "/kv"
entry = "web/index.js"
```

```python
from cordis_sdk import Component, Context, run
from kv_api import Kv                    # 由 kv.wit 生成

class MemoryKv(Kv):
    def __init__(self): self.data = {}
    def get(self, key: str) -> bytes | None: return self.data.get(key)
    def put(self, key: str, value: bytes) -> None: self.data[key] = value

class KvStore(Component):
    name = "kv-store"
    provides = [Kv]
    async def apply(self, ctx: Context, config: dict) -> None:
        store = MemoryKv()
        if (state := ctx.inherited_state("kv-py.snapshot")) is not None:
            store.data = state.decode()
        ctx.export_state("kv-py.snapshot", 1, lambda: store.data)
        ctx.provide(Kv, store)

run(components=[KvStore])
```

TypeScript 插件形态相同，`command = ["node", "dist/main.js"]`；Node 26 也可以直接运行 `.ts` 文件。

前端插件：一个目录，只有浏览器部分。

```toml
name = "dashboard"
version = "0.1.0"
socket = "web"

[web]
assets = "web"
mount = "/dashboard"
entry = "web/index.js"
```

```ts
// web/index.ts，构建为 web/index.js
import { defineComponent } from "cordis-web";
import { Counter } from "./generated/counter";   // 由 counter.wit 生成

export default defineComponent({
  name: "dashboard",
  inject: [Counter],
  async apply(ctx, { counter }) {
    const el = document.createElement("div");
    document.querySelector("#app")!.append(el);
    ctx.effect(() => () => el.remove());           // 卸下时撤回界面
    const render = async () => { el.textContent = String(await counter.value()); };
    ctx.on(Counter.changed, render);               // 后端事件，按 §6.3 为通知
    await render();
  },
});
```

## 10 SDK

| SDK | 位置 | 语言要求 | 第三方依赖 | 内容 |
|---|---|---|---|---|
| Rust | `crates/cordis-sdk` | Rust 1.85 | `DOC-13` §7 | 第二阶段已有；`V-2` 增加通用调用 |
| Python | `sdks/python/cordis_sdk` | Python 3.11 及以上 | 无，CBOR 由 SDK 自行实现 | `Component`、`Context`、`run`；`asyncio`；效应与清理；状态导出；`UP/1` 客户端 |
| TypeScript | `sdks/typescript/cordis-sdk` | Node 22 及以上，或 Bun、Deno | 无，CBOR 由 SDK 自行实现 | 同上；`Promise` |
| 网页宿主 | `sdks/typescript/cordis-web` | 支持 ES 模块与 WebSocket 的浏览器、系统 WebView | 无 | TypeScript SDK 的浏览器构建：WebSocket 传输、`defineComponent`、§11.2 的宿主职责 |
| 代码生成 | `cordis idl gen --lang rust\|python\|ts <wit>` | — | `wit-parser` | 由 WIT 生成接口、数据类型、调用方代理；TypeScript 代码同时用于 Node 与浏览器 |

脚本语言 SDK 不依赖第三方库：插件作者把 SDK 目录复制进插件即可使用。发布到 PyPI 与 npm 不在范围内。本机已有 Python 3.12（conda base）与 Node 26，开发测试不需要安装新环境。

## 11 前端插件与全栈插件

前端代码是插件，由核心中的唯一内核管理：声明依赖、注入服务、登记效应、热替换、回滚，与其他插件相同。它的代码在浏览器中运行，接入方式是通用插口的 WebSocket 传输（§4.2）。

### 11.1 两种形态

| 形态 | 清单 | 内核中的组件 |
|---|---|---|
| 前端插件 | `socket = "web"`，带 `[web]` | 组件 `<名称>`，代码只在浏览器中运行 |
| 全栈插件 | `socket = "native"` 或 `"process"`，带 `[web]` | 后端组件 `<名称>` 与前端组件 `<名称>/web`；两者版本相同，作为一个插件替换与回滚 |

`[web]` 段的字段：`assets`（资源目录，相对插件目录）、`mount`（挂载路径）、`entry`（浏览器入口模块，导出一个组件定义）。

### 11.2 网页宿主

浏览器页面是通用插口上的一种宿主，地位与 `cordis-host` 相同。

1. 页面加载本项目交付的脚本 `cordis-web`（TypeScript SDK 的浏览器构建，§10）。`cordis-web` 不含内核，只执行核心经 `UP/1` 下发的指令：按入口地址 `import()` 插件模块、执行 `apply`、执行清理回调、导出与继承状态。
2. 浏览器中不运行任何内核。上游 JS 版 cordis 不用作前端内核：那会在浏览器中形成第二个内核，与 `Q0` 推论 5 冲突。
3. 每个打开的页面对应核心中的一条连接。核心为每个页面上的每个前端组件建立一个子实例，挂在该组件的实例之下；页面关闭时这些子实例被卸下，它们登记的效应、服务与监听器由核心回收。
4. 每个页面是一个独立的 realm（`isolate`）：页面上的前端组件提供的服务只在该页面内可见，不进入后端，也不进入其他页面。
5. 前端组件经 `UP/1` 注入任何插口上提供的服务（`get`、`invoke`），订阅后端事件（`on`，按 §6.3 为通知）。前端与后端之间不另设 HTTP 接口。
6. 前端组件的依赖不可用时，各页面上的子实例转为 `Pending`，清理回调撤下界面；依赖恢复后自动重建。

### 11.3 `http` 插件与转发

1. `http` 是原生插件，默认自成一组，提供内置接口 `cordis.web@1`。它负责静态资源、挂载路由、浏览器入口清单，以及 WebSocket 连接。
2. 核心装载带 `[web]` 段的插件时，经 `cordis.web@1` 登记它的资源目录、挂载路径与入口。登记是该插件实例的效应，插件卸下时路由撤回。浏览器模块的地址带替换序号：`/_cordis/<插件>/<序号>/<入口>`，新旧版本的模块地址不同，浏览器缓存不会混用新旧代码。
3. 页面经 WebSocket 连接 `http` 插件。`http` 插件不解释 `UP/1` 内容，只用 §8 第 9 条的转发通道把每个页面作为一条独立连接交给核心。核心对每个页面连接分别执行 `CP-2`、`CP-3`、`CP-8`，页面发来的内容按不可信输入处理。
4. 核心不监听网络端口。HTTP 与 WebSocket 的解析代码都在 `http` 插件进程中，它们的缺陷与崩溃只影响该插件：`http` 插件崩溃时全部页面连接关闭，各页面的子实例被回收；`http` 插件按 `CP-6` 重启后，页面自动重连，子实例重建。
5. `http` 插件默认只绑定 `127.0.0.1`。WebSocket 握手检查 `Origin`，并要求 `http` 插件每次启动生成的随机会话令牌，令牌写入它提供的页面。对外服务时的地址、TLS 与认证是 `http` 插件的配置项。

### 11.4 热替换与回滚

1. 替换前端插件时，在每个已连接的页面上：核心经旧子实例导出页面内状态，执行旧子实例的清理回调，令页面 `import()` 新序号地址的模块并 `apply`，新子实例继承状态。整页不刷新。
2. 替换全栈插件时，后端组件先完成替换并进入 `Active`，前端组件随后在各页面上替换。任一步失败时，前后端一起回到旧版本（`DOC-11` 回滚）。
3. 浏览器不卸载已导入的 ES 模块。开发中长时间反复替换时页面内的旧模块会累积，刷新页面即释放。刷新后页面装载当前版本；需要跨刷新保留的数据由前端组件存入后端服务。

### 11.5 桌面软件

`cordis pack` 输出的 `.app` 中，窗口插件（§11.6）打开系统 WebView，指向 `http` 插件的地址。WebView 中的页面与浏览器中的页面相同，同样是网页宿主。

### 11.6 窗口插件

1. `plugins/window` 是进程插件：一个用 Rust 编写的独立可执行文件，经 `UP/1` 接入，不是原生库插件。macOS 的 AppKit 与 WebView 只能在进程主线程上运行；`cordis-host` 的主线程运行 `UP/1` 消息循环，同组还有其他插件，原生库形式取不到主线程。窗口进程的主线程运行界面事件循环，`UP/1` 在另一线程处理。
2. 它提供接口 `window@1`：`open(url, title, width, height) -> 窗口编号`、`close(编号)`、事件 `closed`。打开窗口是效应，插件卸下时窗口关闭。它依赖 `cordis.web@1` 取得页面地址与会话令牌。
3. 平台组件：macOS 使用 WKWebView，Linux 使用 WebKitGTK。所用的 Rust 库在 `M16` 以 ADR 确定。
4. 窗口进程崩溃时按 `CP-6` 重启，窗口重新打开；核心与后端插件不受影响。
5. 在 `.app` 中，窗口进程代表整个应用：Dock 中只出现一个应用图标，名称为应用名。Cmd-Q，以及关闭最后一个窗口（可配置），经 `cordis.manage@1` 的 `shutdown()` 请求核心有序退出。核心进程不显示界面。

## 12 项目脚手架与发布打包

### 12.1 开发时：核心全机一份，项目中只有插件

核心像语言运行时一样在机器上安装一份，所有项目共用。项目仓库中不放核心二进制：二进制不适合版本控制，各项目各自复制的副本也会停留在不同版本。ABI 的支持期（`ADR-0014`）保证新核心继续运行老项目的插件。

```
my-project/
├── cordis.toml          # 实例、分组、运行参数；requires_core = ">=1.0" 声明所需的最低核心版本
├── interfaces/          # *.wit
├── plugins/             # 插件源码：Rust crate、Python 包、TypeScript 包
└── contracts/           # 契约套件
```

| 命令 | 由谁执行 | 作用 |
|---|---|---|
| `cordis new <项目名>` | `cordis-dev` | 生成上述目录骨架与 `cordis.toml`（含 `watch` 插件条目，§3.3） |
| `cordis new-plugin <名称> [--rust\|--python\|--ts] [--web]` | `cordis-dev` | 在 `plugins/` 下生成插件模板。语言选项生成后端部分，`--web` 生成前端部分；两者都给时生成全栈插件，只给 `--web` 时生成前端插件 |
| `cordis run` | 核心 | 在项目目录中运行；插件重新编译后由 `watch` 插件触发热替换 |
| `cordis service install` | 核心 | 生成并安装 launchd 或 systemd 配置（§5.1） |
| `cordis idl gen`、`cordis verify`、`cordis pack` | `cordis-dev` | 代码生成（§7）、验证门、发布打包（§12.2） |

`cordis.toml` 的 `requires_core` 高于当前核心版本时，核心拒绝启动并报告所需版本。模板与 `M15` skill 中的模板为同一份文件。

### 12.2 交付时：发布包附带核心

```bash
cordis pack --target aarch64-apple-darwin --out dist/
```

```
mathbook-studio-1.0-macos-arm64/
├── cordis               # 核心
├── cordis-host          # 原生插件宿主
├── cordis.toml
├── plugins/             # 编译好的插件，不含源码
├── runtimes/            # 可选：独立版 Python、Node，供通用插件使用
└── service/             # launchd / systemd 配置
```

| 项 | 规定 |
|---|---|
| 目标平台 | 每个平台一个发布包：`aarch64-apple-darwin`、`x86_64-unknown-linux-gnu` |
| 插件 | Rust 插件按目标平台编译；Python、TypeScript 插件按目录复制；`cordis pack` 检查每个插件的 ABI 与协议版本受打包的核心支持 |
| 语言运行时 | 配置 `[pack] runtimes = ["python", "node"]` 时，把独立版解释器放入 `runtimes/`，通用插件的启动命令改写为指向它们。解释器属于发布包，不属于核心，`Q0` 与 `AU-8` 不受影响 |
| 服务器部署 | 输出压缩包；可选输出容器镜像的构建文件 |
| 桌面软件 | macOS 输出 `.app`：核心为主程序且不显示界面，窗口插件（§11.6）显示前端插件 |
| 开发工具 | 发布包不含 `cordis-dev`（§3.1） |
| macOS 签名 | 开启 hardened runtime 的程序默认只能加载同一开发者团队签名的动态库。`cordis pack --sign <身份>` 用同一身份为 `cordis`、`cordis-host` 与全部原生插件签名；需要加载第三方签名插件的部署，为 `cordis-host`（不为 `cordis`）配置 `com.apple.security.cs.disable-library-validation`。核心本身不加载任何动态库，不需要此项 |
| 发布后的更新 | 由持有 `cordis.manage@1` 授权的插件完成：`watch` 插件监视插件目录，或更新插件取得新版本后调用 `install` 与 `replace`；核心热替换，不需要重装发布包 |

## 13 范围外与后续

| 事项 | 说明 |
|---|---|
| WebAssembly 插口 | 在核心外运行（例如由一个 `cordis-wasm` 宿主进程内嵌 `wasmtime`），遵守 `Q0`；WIT 与 `format = 3` 已预留。另行立项 |
| 进程配额 | 内存、CPU、文件描述符上限，依赖操作系统机制 |
| 插件进程之间的直连通道 | 跨分组调用目前经核心路由；需要时可在路由建立后让两个进程直接通信，属于性能优化 |
| Python、TypeScript 之外的语言 SDK | `UP/1` 规范公开后按需编写 |
| 核心直接提供 HTTP 与 WebSocket | 不采用。网络协议解析会把大量不可信输入的处理放进核心进程，与要求 1 冲突；由 `http` 插件承担（§11.3） |
| 页面之间共享前端服务 | 不采用。页面关闭即回收，跨页面共享的数据由后端服务提供（§11.2 第 4 条） |
| 插件市场、远程更新服务 | 可作为持有 `cordis.manage@1` 授权的插件编写，不在本计划内 |
| 核心监视目录 | 不采用。由 `watch` 插件承担（§3.3），核心少一类依赖与输入来源 |

## 14 对第二阶段文档的修订清单

由实现者在对应里程碑中修改，修改内容以本文为准。

| 文档 | 修订 | 里程碑 |
|---|---|---|
| `DOC-10` | 增加 `Q0`（§2）；删除 `Q8`；`G12` 改为“插件的任何故障不影响核心进程”；`R7` 限定在插件宿主进程内；§2 系统形态换为本文 §3；§4 删除“进程隔离与沙箱”一行 | `M10` |
| `DOC-11` | §6 `SW-*` 的执行者改为核心监管加插件进程；增加进程崩溃与重启（`CP-6`）、检查点（`CP-11`）；§10 `cordis.toml` 增加 `group`、`checkpoint_ms`、`requires_core`；增加 §6.3 的事件语义 | `M10`–`M12` |
| `DOC-12` | `V-1`、`V-3`、`V-4`；状态改回“候选 v1.0”，`M11` 冻结；§1 说明 ABI v1 是原生插件与 `cordis-host` 之间的接口，核心不使用 | `M9`、`M11` |
| `DOC-13` | `cordis-loader` 改名为 `cordis-host` 并成为可执行程序；新增 `cordis-supervisor`（核心侧进程监管与路由）与 `cordis-up`（`UP/1` 编解码，纯安全 Rust）；`B-2` 改为“核心的全部 crate 无 `unsafe`，不依赖 `cordis-abi`、`cordis-sdk`”；§3.3 加入 `V-2`；§7 白名单为模糊测试增加开发依赖 | `M9`–`M12` |
| `DOC-14` | 里程碑按 §15 重排；`AC-7` 改为“插件段错误、栈溢出、`abort` 时核心继续运行”；增加 `AU-*` 的引用 | `M10` |
| `ADR-0009` | 增补：原生库只在 `cordis-host` 中装载 | `M10` |
| `ADR-0012` | 增补：卸载语义只适用于 `cordis-host` 内部；分组有状态重启（§8 第 7 条） | `M11` |
| `ADR-0017` | 核心侧 crate 不依赖 `cordis-sdk`；内置接口经 `UP/1` 提供；`cordis.log@1` 对远端监听器为通知 | `M10` 之前 |
| `ADR-0018` | 新增：核心隔离（`Q0`）及其推论 | 设计方已写 |
| `ADR-0019` | 新增：两种插口、分组与跨进程通用调用 | 设计方已写 |
| `ADR-0020` | 新增：以 WIT 为接口描述语言 | 设计方已写 |
| `ADR-0023` | 新增：前端插件经通用插口接入，浏览器中无内核 | 设计方已写 |
| `ADR-0024` | 新增：核心之外一切皆插件——开发工具分离、管理接口、`watch` 与窗口插件 | 设计方已写 |
| `ADR-0025` | 新增：隔离域、拦截与组合经 `UP/1` 提供给插件 | 设计方已写 |
| `docs/16-up1-protocol.md` | 增加 `context_derive`、`plugin`；`provide` 增加 `scoped`；保留方法 `cordis.scope`；`get` 返回视图对象的规则（§6.4） | `M11` |
| `DOC-12` | `HostV1` 追加 `context_derive`、`plugin` 槽位；说明 `cordis.scope` 为通用调用保留方法 | `M11` 冻结前 |
| `DOC-13` | SDK 草图增加 `ctx.isolate`、`ctx.intercept`、`ctx.plugin` 与 `Scoped` 提供方辅助 | `M11` |
| `DOC-13` | §1 新增 `crates/cordis-dev`、`plugins/watch`、`plugins/window`；`cordis-runtime` 去掉目录监视与脚手架，增加子命令转交与 `cordis.manage@1`；`wit-parser` 只进入 `cordis-dev`；内置接口表增加 `cordis.manage@1` | `M12`、`M16`、`M17` |
| `DOC-11` | §6 热替换的触发方改为核心启动与 `cordis.manage@1`；§10 `cordis.toml` 增加 `grants`、`isolate`、`intercept`（§6.4）；状态目录增加叠加层 `state/instances.toml` | `M12` |
| `ADR-0017` | `cordis.runtime@1` 保持只读；运行中的管理操作由 `cordis.manage@1` 提供 | `M12` |
| `DOC-11` | §10 增加插件清单 `cordis-plugin.toml` 的 `socket`（`native`、`process`、`web`）与 `[web]` 段；`cordis.toml` 增加转发方标记 | `M12`、`M20` |
| `docs/16-up1-protocol.md` | 定义 §8 第 9 条的转发通道消息；说明连接抽象与传输无关 | `M10` |

## 15 里程碑

`M7`（已完成）、`M8`（已提交 `b792b2e`）不变。`M9` 加入 `V-1`–`V-4` 并解除 ABI 冻结。从 `M10` 起重排。

| 里程碑 | 内容 | 依赖 | 交付与测试 |
|---|---|---|---|
| `M9` Rust SDK | 原计划；加 `V-1`–`V-4`；`DOC-12` 改回“候选 v1.0” | `M8` | 原计划测试；同一接口的类型化调用与通用调用结果一致；通用调用的往返与未知方法 |
| `M10` `UP/1` 与核心监管 | `docs/16-up1-protocol.md`；`cordis-up`；`cordis-supervisor`：进程启动、握手、路由、编号表、截止时间、背压、崩溃回收与重启；`CP-1`–`CP-10`；模糊测试目标（`CP-9`）；连接抽象与传输无关，规范定义转发通道消息（§8 第 9 条，实现在 `M20`） | `M9` | 以一个 Rust 编写的最小进程插件作测试替身：每种消息、超时、取消、协议错误、超大帧、畸形 CBOR、消息洪泛、进程被 `SIGKILL`；核心在每种情况下存活；每个模糊测试目标运行 30 分钟无 panic |
| `M11` 插件宿主 `cordis-host` | 装载器移入 `cordis-host`：`dlopen`、握手、租约、分组、组内直连、组内替换、分组有状态重启；§6.4 的 `context_derive`、`plugin`、`scoped` 与 `cordis.scope`（核心、`UP/1`、宿主函数、原生 SDK）；ABI v1 冻结 | `M10` | 真实动态库跨库调用；1.85.0 编译的插件；组内插件段错误、栈溢出、`abort` 时核心存活、依赖方转 `Pending`；`DOC-12` 改为“已冻结 v1.0”；`AU-18` 中原生插件的部分 |
| `M12` 核心可执行文件 | `cordis run`、`cordis.toml`（含 `group`、`checkpoint_ms`、`requires_core`）、插件清单 `socket = "native"`（§9）、状态目录、`status.json`、有序退出、定期检查点（`CP-11`）、`cordis service install`（launchd、systemd）；子命令转交与 `cordis-dev`（`new`、`new-plugin` 的 Rust 模板，§3.1）；`cordis.manage@1`、`grants` 与叠加层 `state/instances.toml`（§3.2）；`plugins/watch`（§3.3）；实例条目的 `isolate`、`intercept`（§6.4） | `M11` | 原 `M11` 测试；核心进程依赖树中无 `cordis-abi`、`cordis-sdk`、`wit-parser`、文件监视库，核心 crate 无 `unsafe`；检查点按间隔写出；未授权实例得不到 `cordis.manage@1`，授权外的方法返回权限错误；`watch` 触发的替换与回滚；叠加层在核心重启后生效；`cordis new` 经转交执行，生成的项目可直接 `cordis run`；`AU-18` 中部署层的部分 |
| `M13` 验证门 | 原 `M12`；`cordis-dev verify` 启动独立的 `cordis run` 进程运行契约套件与被测插件（§3.1） | `M12` | 原 `M12` 测试 |
| `M14` 第二阶段验收 | 原 `M13`；`AC-*` 按 §14 修订；`AU-1`、`AU-2`、`AU-8`–`AU-11`、`AU-16`、`AU-18`；`RM-10` | `M13` | `AC-1`–`AC-9` 与所列 `AU-*` |
| `M15` 插件编写 skill | Codex 编写；Rust 插件部分；模板与 `cordis new-plugin` 共用 | `M14` | `SK-1`–`SK-3` |
| `M16` 发布打包与窗口插件 | `cordis-dev pack`；目标平台检查；`.app` 与压缩包；`--sign`；`runtimes/` 机制（以 Rust 编写的进程插件测试替身验证命令改写）；`plugins/window`（§11.6），窗口库的选择写 ADR | `M14` | `AU-12`、`AU-17` |
| `M17` 接口描述语言 | `cordis-dev idl gen`；`kv-api` 改为由 `kv.wit` 生成 | `M14` | 生成的 Rust 代码与手写版本行为一致；§7 映射表每一行有用例 |
| `M18` Python SDK | `sdks/python`（含 §6.4 的 `isolate`、`intercept`、`plugin`、`scoped`）；`plugins/kv-py`、`plugins/counter-py`；`cordis new-plugin --python` | `M17` | 跨插口互调两个方向；热替换与状态迁移；`kv-contract` 验证 `kv-py`；`AU-18` 以 Python 提供方重做 |
| `M19` TypeScript SDK | `sdks/typescript`；`plugins/kv-ts`；`cordis new-plugin --ts` | `M17` | 同 `M18` |
| `M20` 前端插件与全栈插件 | `plugins/http`（静态资源、`cordis.web@1`、WebSocket 转发）；核心侧转发通道与页面子实例（§11.2、§11.3）；网页宿主 `cordis-web`；清单 `socket = "web"` 与 `[web]` 段；示例：一个前端插件、一个 Rust 后端的全栈插件、一个 Python 后端的全栈插件；`cordis new-plugin --web` | `M18`、`M19` | 资源登记与撤回；页面关闭后子实例与效应回收；前端注入 Rust、Python、TypeScript 提供的服务；无头浏览器验证局部更新与回滚；`AU-7`、`AU-14`、`AU-15` |
| `M21` 第三阶段验收 | §16 全部场景；`RM-7`–`RM-9`；`cordis pack` 附带 Python 与 Node 运行时的发布包；skill 增加 Python、TypeScript、全栈与打包章节 | `M20`、`M16` | §16 全部通过 |

Codex 从 `M9` 连续推进到 `M21`，包括 `M15` 的 skill 与 `M21` 的 skill 章节；设计方只负责设计文档、ADR 与复核。

## 16 验收场景

| 编号 | 场景 | 通过的判据 |
|---|---|---|
| `AU-1` | 原生插件中故意触发段错误、栈溢出、`abort`、`std::process::exit`、死循环 | 每种情况下核心进程存活；该分组的实例 `Failed` 后按策略重启；其他分组与通用插件的实例纪元不变 |
| `AU-2` | 进程插件发送超大帧、畸形 CBOR、每秒数万条消息、长期不响应 | 核心存活，按 `CP-2`–`CP-6` 断开或终止该进程，其他插件不受影响 |
| `AU-3` | 同一个 `cordis` 中同时运行 Rust、Python、TypeScript 插件 | 全部实例 `Active`；`status.json` 标明每个插件的插口与分组 |
| `AU-4` | Rust、Python、TypeScript 插件两两互相注入 `kv@1` | 六个方向的调用结果一致 |
| `AU-5` | `counter`（Rust）持续写入时，`kv@1` 的提供方依次替换为 Rust、Python、TypeScript 实现 | 每次替换后计数不回退；依赖方自动重建 |
| `AU-6` | `kv-contract`（Rust）分别验证三种语言的 `kv` 实现 | 全部通过 |
| `AU-7` | 替换一个全栈插件；再替换为前端 `apply` 必定失败的版本 | 第一次替换后页面局部更新为新版本，不刷新整页；第二次替换后前后端一起回到上一版本 |
| `AU-8` | 核心进程的组成 | 依赖树中没有 `cordis-abi`、`cordis-sdk`、`wit-parser`、文件监视库，没有 Python 或 JavaScript 运行时；`cordis` 中没有开发工具的代码；`unsafe` 审计对核心 crate 输出为空 |
| `AU-9` | 同组原生插件之间的调用速度 | 与第二阶段直接调用相比无退化（`RM-9`） |
| `AU-10` | 在一个分组内连续替换同一插件 50 次 | 达到阈值时自动进行分组有状态重启，状态保留，内存回落 |
| `AU-11` | 在服务管理器托管下对核心进程发送 `SIGKILL` | 核心被重新拉起；没有残留的旧插件进程；全部插件恢复 `Active`；内存状态等于最近一次检查点 |
| `AU-12` | 在一个没有 Rust 工具链与项目源码的目录中解开 `cordis pack` 的产物并运行 | 全部插件 `Active`；macOS 上签名校验通过 |
| `AU-13` | 对 `UP/1` 解码器与各解析器进行模糊测试 | 每个目标 30 分钟无 panic、无超时；发现的问题均已修复并加入回归测试 |
| `AU-14` | 两个页面打开同一个前端插件，它注入 Rust 与 Python 插件提供的服务；拔出提供方再插回；关闭其中一个页面 | 核心中每个页面各有一个子实例；拔出后两个页面的子实例转 `Pending`、界面撤下，插回后自动恢复；关闭的页面的子实例与效应全部回收，另一页面不受影响 |
| `AU-15` | 一个页面经 WebSocket 发送超大帧、畸形 CBOR、消息洪泛、伪造的转发通道消息；对 `http` 插件发送 `SIGKILL` | 核心存活，只断开该页面的连接；`http` 插件重启后其余页面自动重连，子实例恢复 `Active` |
| `AU-16` | 一个未获授权的插件注入 `cordis.manage@1`；一个获授权的插件依次执行 `install`、`replace`、`rollback`、`replace`；随后对核心发送 `SIGKILL` | 未获授权的插件得不到服务，核心存活；授权插件的每个操作按 `DOC-11` 完成并记入日志；核心重启后实例保持最后一次 `replace` 之后的版本 |
| `AU-17` | 在打包的 `.app` 中强制终止窗口进程；之后按 Cmd-Q | 窗口自动重新打开，后端实例纪元不变；Cmd-Q 后核心有序退出并写出检查点，没有残留进程；Dock 中始终只有一个应用图标 |
| `AU-18` | 两个 `kv-file` 实例分别在域 `tenant-a`、`tenant-b`；两个 `app` 实例各读一个域；一个实例经 `intercept` 覆盖 `kv@1` 的元数据；一个插件经 `context_derive` 与 `plugin` 在私有域中装入自己的子组件；提供方依次放在同组原生、不同组原生、进程插件中 | 每个 `app` 只读写自己域中的数据；声明 `scoped` 的提供方在三种位置都收到正确的元数据，同组时调用仍为直接调用；卸下父实例时子实例先被卸下，私有域中的服务随之撤回 |

| 编号 | 指标 | 测量方法 |
|---|---|---|
| `RM-7` | 跨进程一次服务调用的往返时延 | Rust 调 Rust（不同分组）、Rust 调 Python、Rust 调 TypeScript 各 10000 次，记录中位数与 99 分位 |
| `RM-8` | 插件进程的替换停顿 | 同 `RM-4`，按插口种类分别记录 |
| `RM-9` | 同组原生插件之间的调用时延 | 与第二阶段的直接调用基准对比 |
| `RM-10` | 核心崩溃到全部插件恢复 `Active` 的时长 | `AU-11` 中计时 |

## 17 已确认的事项

| 编号 | 事项 | 取值 |
|---|---|---|
| `D-1` | 首要公理 `Q0`：核心进程不运行任何插件代码 | 是 |
| `D-2` | 原生插件由 `cordis-host` 进程按分组装载；默认每个插件库自成一组 | 是 |
| `D-3` | `V-1`–`V-4` 并入 `M9`；ABI v1 在 `M11` 冻结 | 是 |
| `D-4` | 接口描述语言为 WIT | 是 |
| `D-5` | 首批通用插口语言为 Python、TypeScript；SDK 自行实现 CBOR，不依赖第三方库 | 是 |
| `D-6` | 里程碑按 §15 重排 | 是 |
| `D-7` | WebAssembly 插口与进程配额列为后续 | 是 |
| `D-8` | 核心崩溃恢复（`CP-9`–`CP-11`、§5.1）、项目脚手架与发布打包（§12） | 是 |
| `D-9` | 前端插件经通用插口接入：浏览器页面为网页宿主，不含内核；`http` 插件转发，核心不监听网络；原生插件与进程插件都可带前端部分，也可以只有前端部分（`socket = "web"`）（§9、§11） | 是 |
| `D-10` | 核心可执行文件只含运行功能；开发工具为独立程序 `cordis-dev`，由 `cordis` 转交未知子命令（§3.1） | 是 |
| `D-11` | 运行中的管理操作由持有授权的插件经 `cordis.manage@1` 请求；授权只写在 `cordis.toml`；变更写入叠加层；目录监视由 `watch` 插件承担（§3.2、§3.3） | 是 |
| `D-12` | 桌面窗口为进程插件 `plugins/window`，在 `M16` 交付（§11.6） | 是 |
| `D-13` | 隔离域、拦截与组合经 `UP/1` 提供给插件：部署层写在 `cordis.toml`，插件层用 `context_derive` 与 `plugin`，拦截元数据按句柄绑定送达提供方；`bail`、`waterfall` 仍只用于核心内部（§6.4） | 是 |
