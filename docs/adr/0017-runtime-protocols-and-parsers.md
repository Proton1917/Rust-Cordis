# ADR-0017 运行时接口与标准格式解析

| 项 | 内容 |
|---|---|
| 状态 | 已接受（2026-11-12） |
| 影响 | DOC-11 §8、DOC-13 §1、§5.3、§7；DOC-15 的 Q0 与跨进程事件；M9–M13 |

## 问题

运行时需要解析插件版本、命令行和 JSON 状态文件。console-log 需要接收运行时日志，契约套件需要协调一次被测实例的状态导出与继承。

## 选项与判据

采用标准格式对应的成熟解析库，使输入错误在格式边界报告。核心通过 UP/1 提供内置服务与事件，纳入普通服务与效应生命周期；原生插件宿主把协议对象映射为 SDK 接口。契约迁移由核心在套件交回当前阶段后推进，保持 apply 的惯性和依赖清理顺序。

## 决定

- 插件版本采用 semver 1.0.28，命令行采用 clap 4.6.7，JSON 印记与状态采用 serde_json 1.0.154。serde_json 用于 host/runtime/contract，semver 用于 host/supervisor/runtime，clap 用于 runtime/host。文件配置继续使用 toml，二进制载荷继续使用 ciborium。
- Q0 约束核心进程的内核、cordis-up、cordis-supervisor、cordis-runtime：源码使用安全 Rust，依赖树与 cordis-abi、cordis-sdk 分离。cordis-host 与原生插件依赖 SDK；接口 crate 的最低 Rust 版本为 1.85。
- `cordis.log@1` 的载荷包含 `level`、`message`、`instance`。核心向远端监听器发送异步通知，console-log 收到通知后写入终端。同组原生监听器继续遵守其本地事件族；跨进程的 bail、waterfall 返回值由服务调用表达（DOC-15 §6.3）。
- `cordis.contract@1` 包含 `report(name, passed, message)`、`finish()`、`stage() -> u32` 和 `request_state_roundtrip()`。套件在阶段 0 完成读写断言并请求状态往返，交回 apply；宿主停用套件、导出被测实例、重启并继承状态，再以阶段 1 挂载同一套件。阶段 1 检查迁移结果并调用 finish。宿主按阶段合并断言记录。
- `cordis.runtime@1` 经 UP/1 返回当前库、实例和替换报告。核心快照与管理操作分别持有必要状态，支持替换期间报告状态。cordis-host 负责 ABI 对象转换。
- `cordis.data-dir@1` 经 UP/1 返回实例所属逻辑插件的数据目录。使用该服务的示例组件显式声明依赖。
- `cordis.contract@1` 同样由核心经 UP/1 提供；契约辅助 crate 复用插件侧 SDK 接口，核心使用协议数据处理断言与阶段转换。
- ADR-0024 规定 cordis.runtime@1 保持只读；管理操作由 cordis.manage@1 提供，逐方法授权来自 cordis.toml 的 grants。持久变更写入 state/instances.toml，每次操作记录调用方、目标与结果。文件监视属于 watch 插件。
- 开发命令在 cordis-dev 中实现。该程序通过 cordis-host --describe 获取原生描述符，通过 cordis run 的独立部署执行验证；核心保留运行命令与参数转交。

## 后果

验证门复用进程监管、插件宿主与状态协议。命令行、版本、配置和印记分别由成熟库解释。内置能力以 UP/1 数据协议连接核心与插件宿主，原生插件通过普通接口虚表访问。
