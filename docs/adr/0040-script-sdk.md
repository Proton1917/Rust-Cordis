# ADR-0040 脚本 SDK 与 CBOR 依赖

| 项 | 内容 |
|---|---|
| 状态 | M18 实施依据 |
| 影响 | DOC-15 §10、D-5；Python 与 TypeScript SDK、插件脚手架与发布依赖 |

## 问题

Python SDK 需要读取 RFC 8949 CBOR，保留 UP/1 的资源标签、嵌套可选值、字节串与请求包络。DOC-15 §10 原先选择由 SDK 自行实现 CBOR。当前工程规则要求成熟格式由第三方库解析，M17 的 Python 绑定验收已经使用 cbor2 5.6.5。

## 选项与判据

维护专用 CBOR 编解码器会增加格式实现与测试职责。采用 cbor2 由成熟库负责语法与基本类型，SDK 专注协议帧、编号所有权、上下文和组件生命周期。判据为 RFC 8949 兼容、M17 生成代码互通、依赖可部署及核心进程隔离。

## 决定

Python 采用 cbor2==5.6.5。sdks/python/requirements.txt 和 pyproject.toml 声明运行时依赖，Python 插件脚手架引用该依赖。CBOR 解析与编码调用 cbor2，四字节长度帧使用 Python 标准库。TypeScript 采用 cbor-x 1.6.0，package.json 声明运行时依赖；typescript 和 @types/node 归开发依赖。部署环境或发布包提供相应 CBOR 库，M21 的解释器打包包含其运行依赖。

SDK 分为 wire、runtime、context 和 bindings。wire 使用 asyncio 管理标准流、在途请求、截止时间与取消；runtime 执行核心请求并持有对象、回调、导出器和上下文；context 提供效应、服务、事件、隔离、拦截和子组件接口；bindings 实现 ADR-0039 生成 Transport 契约。资源引用由每次消息的出口事务及接收载荷的拥有关系控制。

同步和异步 Python 组件均由核心调度。插件的后台任务通过效应登记清理；export 冻结写入并产生持久化状态，thaw 解冻，apply 取得继承状态。清理与 apply 遵循 DOC-16 §10 的惯性语义，invoke 允许取消。服务代理经生成 Client 调用，scoped 提供方根据元数据建立共享状态的视图。

WIT 的 async 元数据保留原生入口选择；脚本提供方的 dispatch 等待实际返回的 awaitable/Promise，允许提供方在实现中调用远程服务。二次转交资源时按同一生成 schema 解码入参、调用代理并重新编码结果，使每个连接持有自己的出口引用。事件回调可以声明生成绑定，导入的资源句柄独立于原事件载荷存活。

Python 脚手架将当前 SDK 源码放入插件目录，requirements.txt 声明 cbor2；启动文件读取插件内 .dependencies，依赖由用户显式安装。插件目录位于项目 plugins/<名称>，配置的 plugin_dir 中建立指向该目录的链接；watch 通过该目录观察插件变化。Cargo workspace 的 exclude 登记该 Python 目录，Rust 成员继续正常构建。生成操作保留现有配置项，新的实例条目追加至 cordis.toml。

开发工具使用 toml_edit 0.25.19 在 Cargo workspace 中追加排除项，并保持原文件的注释与格式。该依赖位于 cordis-dev，运行核心继续使用既有配置解析器。

TypeScript 脚手架把 SDK TypeScript 源码放入插件的 src/cordis_sdk，与业务入口一次编译到 dist。npm 配置 bin-links=false，构建命令直接调用 TypeScript 编译器文件，安装目录由普通文件组成。kv-ts 示例使用 install-links=true 安装本地 SDK 的完整副本；先构建 SDK，再安装与构建插件。两种方式均满足运行时安装副本的普通文件约束。

## 后果与验收

DOC-15 的 Python SDK、TypeScript SDK 依赖栏与 D-5 按上述成熟库更新。核心继续执行 UP/1 路由与生命周期管理。M18 和 M19 分别通过真实核心验证 Rust 与脚本 SDK 双向调用、KV 契约、替换状态迁移和 AU-18 的脚本提供方行为，覆盖清理、资源释放与调用取消。
