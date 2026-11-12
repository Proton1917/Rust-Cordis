# ADR-0024 核心之外一切皆插件：开发工具分离、管理接口、`watch` 与窗口插件

| 项 | 内容 |
|---|---|
| 状态 | 已接受（2026-11-12） |
| 影响 | `DOC-15` §1 第 5 条、§3、§3.1–§3.3、§7、§11.5、§11.6、§12、§13、§14、§15（`M12`、`M13`、`M14`、`M16`、`M17`）、§16（`AU-8`、`AU-16`、`AU-17`）；`DOC-11` §6、§10；`DOC-13` §1；`ADR-0017` |

## 问题

项目所有者确定的原则：除核心以外一切都是插件，前后端之间没有插件接口以外的约定（`DOC-15` §1 第 5 条）。按此原则核对 `DOC-15`，有三处没有贯彻：

1. `cordis idl gen`、`cordis pack`、`cordis new`、`cordis verify` 规划在核心可执行文件中。这些工具变化频繁，核心要求长期不动；`wit-parser` 等工具依赖也进入核心。
2. `cordis.runtime@1` 只能查询。装载、替换、回滚只能由核心自身的目录监视或改写 `cordis.toml` 触发，管理后台、插件更新、远程推送都无法写成插件。
3. 桌面窗口插件没有归入任何里程碑。macOS 的 AppKit 与 WebView 只能在进程主线程上运行，而 `cordis-host` 的主线程运行 `UP/1` 消息循环。

## 选项

### 开发工具放在哪里

| 选项 | 内容 | 代价 |
|---|---|---|
| A1 | 留在 `cordis` 中 | 工具每次修改都要升级核心；核心依赖树变大 |
| A2 | 独立程序 `cordis-dev`；`cordis` 把不认识的子命令转交给它 | 机器上多一个程序 |

### 运行中的管理操作由谁发起

| 选项 | 内容 | 代价 |
|---|---|---|
| B1 | 只由核心自身（目录监视、配置文件）发起 | 管理功能只能写进核心 |
| B2 | 插件经 `cordis.manage@1` 请求，内核执行；授权只写在 `cordis.toml` | 需要授权检查与变更持久化 |

### 窗口插件的形态

| 选项 | 内容 | 代价 |
|---|---|---|
| C1 | 原生库插件，装入 `cordis-host` | 取不到主线程，macOS 上无法运行 |
| C2 | `cordis-host` 增加独占主线程的分组模式 | `cordis-host` 的结构随平台特例变复杂 |
| C3 | 独立可执行文件，作为进程插件接入 | 窗口与后端之间的调用跨进程 |

## 判据

1. 核心长期不动，依赖与输入来源尽量少（要求 1）。
2. 除核心以外的功能都能以插件形式提供、替换、回滚。
3. 授权不能经插件自身扩大。
4. 桌面窗口在 macOS 与 Linux 上都能运行，崩溃不影响核心。

## 决定

选 A2、B2、C3。

- `cordis` 只含 `run`、`status`、`service install`、`version`。`cordis-dev` 提供 `new`、`new-plugin`、`idl gen`、`verify`、`pack`，不含内核，不调用 `dlopen`；读取原生插件描述符经 `cordis-host --describe`。发布包不含 `cordis-dev`。
- `cordis.manage@1` 提供 `install`、`add`、`remove`、`replace`、`rollback`、`configure`、`reload`、`shutdown`。授权写在 `cordis.toml` 实例条目的 `grants` 中，可以细化到方法；该接口不能授予或扩大授权。变更写入叠加层 `state/instances.toml`，`cordis.toml` 不被改写。每个操作记入日志。`cordis.runtime@1` 保持只读。
- 核心不监视目录。`plugins/watch` 持有 `install`、`replace`、`reload` 授权，在文件变化后触发热替换。
- `plugins/window` 为进程插件，主线程运行界面事件循环，提供 `window@1`；在 `.app` 中代表整个应用，Cmd-Q 经 `shutdown()` 请求核心有序退出。

## 后果

- `M12` 交付子命令转交、`cordis-dev` 的 `new` 与 `new-plugin`、`cordis.manage@1`、`grants`、叠加层与 `plugins/watch`；`M13` 的验证门、`M16` 的打包、`M17` 的代码生成都在 `cordis-dev` 中。
- `M16` 交付 `plugins/window`，窗口库的选择另写 ADR。
- 核心的依赖树中不出现 `wit-parser` 与文件监视库（`AU-8`）。
- 管理后台、插件市场、远程更新今后都可以作为持有授权的插件编写。
