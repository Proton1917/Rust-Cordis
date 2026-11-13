# ADR-0041 对象路由、转发通道与网页部署职责

| 项 | 内容 |
|---|---|
| 状态 | M20 实施依据 |
| 影响 | DOC-13 §3.8；DOC-15 §11；DOC-16 §6、§11；cordis-routing、cordis-channels、cordis-web-deployment、supervisor、runtime、host、http、cordis-web |

## 问题

M20 把浏览器页面接入既有 UP/1 与唯一内核。提取前 supervisor 为 5500 代码行，runtime 为 5054 代码行。对象编号和转交协议已形成可独立维护的实现；网页连接又需要各自的握手、帧预算、页面域和资源版本。职责拆分使协议对象与生命周期决策分别由现有来源管理。

原生 http 插件通过冻结 HostV1 使用 SDK 服务接口，浏览器连接需要发送 channel_open、channel_frame、channel_close。适配通过普通接口对象建立，HostV1 的布局和槽位保持 v1.0。

## 决定与依赖

| crate 或模块 | 职责 | 产品依赖 | 目标 / 上限代码行 |
|---|---|---|---:|
| cordis-routing | 接收连接的单调出口目录、未提交 Batch、资源标签转交、普通服务调用转发、接口标识、元数据额度租约、内置服务调用参数与回复类型 | cordis-up、parking_lot、serde、semver | 500 / 800 |
| cordis-supervisor | 唯一内核的组件定义、加载纪元、上下文有效期、Scoped 缓存、进程与连接生命周期 | 既有依赖加 cordis-routing | 4500 / 5500 |
| cordis-channels | 转发连接授权、页面通道编号、帧队列、独立连接创建与物理连接结束后的关闭 | cordis-up、tokio、parking_lot | 700 / 1200 |
| cordis-web-deployment | 网页清单装配、资源版本登记、页面会话、页面子实例及全栈事务的前端步骤 | cordis、cordis-supervisor、cordis-channels、cordis-up、parking_lot、serde、tokio、url | 1000 / 1500 |
| cordis-runtime 的 web 装配入口 | 把配置、管理授权、安装与替换事务接入网页部署服务 | cordis-web-deployment | runtime 合计上限 5500 |
| cordis-host 的 forward 模块 | 把授权 Forward 接口调用转换成 channel 消息，保留接收回调并执行回程通知 | 既有 ABI 与 UP/1 | host 合计上限 3500 |
| http、cordis-web | HTTP/WebSocket 终结；浏览器模块装载、函数执行与效应清理 | 插件侧接口、成熟网络/CBOR 库 | 依各自实现记录 |

所有核心侧 crate 使用安全 Rust。网络解析留在 http，浏览器 JavaScript 留在 cordis-web。网页部署库是 runtime 的产品依赖，主管配置与部署事务；supervisor 继续通过现有挂载和释放入口与内核交互。

## 路由提取

`Router<R>` 保存按接收连接区分的出口目录。表项同时记录来源连接编号和 `Arc<R>`；R 是 supervisor 的 Route，保留其 Generation 与 ViewRecord。Router 对资源编号、发送批次、引用转交和协议调用负责。supervisor 在路由进入调用前取得使用方和提供方的纪元持有，随后调用 Router 完成消息转发。

Router 提供 `route`、`export`、`batch`、`release`、`reclaim`、`transfer`、`invoke_remote` 和 `invoke_local`。`Batch<R>` 在失败或丢弃时归还本轮分配的出口，成功进入 Outgoing 发送提交点后由接收端的 release 负责归还。目录锁覆盖编号和表项变动，函数调用与资源析构均在锁外执行。

Arguments、CallContext、LocalReply、LocalService 移入 cordis-routing，通过 `cordis_supervisor::manager` 重导出。调用方保持既有路径和签名。对象元数据的保留预算、纯字节计量、Interface 标识解析和有界诊断值构造归入 routing；诊断上报、错误回收、版本匹配和 Scoped 服务建立继续由 supervisor 处理。

ADR-0046 的受信调用来源沿此分层实现：协议描述符的名称、接口解析与字节计量，以及对象信息查询和诊断额度归入 routing。supervisor 继续保留公开入口、内核定义构造、纪元持有、Scoped 缓存和诊断上报。描述符检查只产生解析结果，组件定义的安装与退出仍由 supervisor 决定。

## Forward 接口与冻结 ABI

插件侧增加 forward-api，定义 `cordis.forward@1.0`：

| 方法 | 参数 | 结果 |
|---|---|---|
| open | metadata: CborValue，receiver: Service<ForwardReceiver> | 异步 u64 通道编号 |
| frame | channel: u64，frame: Vec<u8> | 异步单位值 |
| close | channel: u64，reason: String | 异步单位值 |

ForwardReceiver 使用 `cordis.forward-receiver@1.0`，提供异步 frame(bytes) 和 close(reason)。HTTP 插件持有本次 WebSocket 的 Receiver，Host 的适配表独立保留 ABI 对象份额。Host 从 1 开始单调分配通道编号。

Host.get 先执行普通 get，由核心解析实例视图内的 Forward 授权。成功后 Host 构造 GenericService 适配并保留授权 RemoteObject。open 登记 Receiver，再发送真正的 channel_open；发送失败撤回对应 Receiver。frame 与 close 转为对应的 channel 通知。Host 收到回程 channel_frame、channel_close 后调用 Receiver，通道结束时移除表项并释放持有。HTTP 激活清理关闭其全部通道，Host 连接结束释放整表。

runtime 从实例设置 `forwarder = true` 构造 Forward 服务视图。Host 的 channel_open metadata 由绑定上下文与页面信息组成 `{context, page}`；Manager::request_origin 校验该上下文属于实际来源连接且仍处于活跃加载，网页部署再按 runtime 的现行 Permissions 判断该实例的 forwarder 设置。后续帧绑定 open 时记录的实例、来源连接和通道编号。转发方重新激活时以新上下文重新授权，实例停用时相应页面和权限随生命周期收束。

## 转发通道

每条通道用 `(physical_connection, channel)` 定位，并获得独立的核心连接世代。channels 将浏览器的一条二进制消息包装为四字节长度与同一 CBOR 消息体，交给既有 Connection::attach。回程拆除该四字节帧头，作为外层 channel_frame 发送。两次帧预算分别执行；外层封装所占字节计入物理连接上限。

内层请求表、资源表和背压队列独立。内层 channel_open、channel_frame、channel_close 被识别为协议错误并关闭该页面。页面关闭、帧错误或超时结束逻辑连接；物理转发连接结束时关闭它的全部页面连接。supervisor 按连接结束路径撤回页面实例与效应，其他物理连接继续运行。

## 网页部署与全栈事务

网页部署解析 socket=web 以及 native/process 清单中的 web 段，保存 assets、mount、entry 和部署版本序号。运行中的资源登记由插件实例效应拥有，版本地址使用 `/_cordis/<插件>/<序号>/<入口>`。HTTP 提供方重新激活后，网页部署服务重新登记当前有效资源。

assets 相对插件目录，entry 相对 assets。资源登记、版本地址、相对模块导入、发布打包与脚手架共用此规则；`assets="web"`、`entry="index.js"` 定位插件目录的 web/index.js。原生组合脚手架附带 build.sh，将 Cargo 产物复制到清单指定的插件目录，使动态库与网页资源作为同一个目录版本替换。

页面会话通过独立握手取得其浏览器宿主描述符。网页部署为每个页面创建独立的服务域，并在相应插件实例下挂载页面子实例。后端服务从页面的父视图注入，页面提供的服务在页面域内可见。页面结束时等待该页面全部子实例清理，回收其资源登记和连接对象。

页面命名域以结构化键 `(Some(页面连接编号), 域名)` 标识，后端的全局命名域使用 `(None, 域名)`。网页部署通过 Manager::page_context 在内核上下文中保存页面命名空间扩展，派生视图与子组件沿原上下文链继承；realm_lease 按此扩展选择命名域。同一页面中显式指定相同域名会共享服务，其他页面及后端各自保留独立身份。后端服务仍由部署的父视图注入页面，普通后端插件的命名域行为保持原有定义。

纯前端替换依次执行旧状态导出、旧效应清理、新版本模块装载和 apply。全栈替换先完成后端 Active，再切换所有现有页面；后端与页面步骤共享事务的旧版本、状态和提交结果。任一步失败时按 DOC-11 恢复旧后端及所有受影响页面。网页部署提供这些步骤与等待结果，runtime 管理事务统一发布部署叠加层。

网页宿主描述采用 format=2，并以 library 字段表示当前页面的模块句柄。普通进程 SDK 继续发送 library=null。浏览器实现 library_prepare、library_activate 和 library_retire，prepare 的路径为核心已安装清单指定的版本 URL，apply.library 精确选择旧模块或候选模块。这样复用 Manager::replace_library 的状态导出、清理、继承及失败恢复。页面组 specification=None，沿现有规则跳过原生进程旧库阈值重启；ES 模块退役返回 closed=false，模块缓存随页面刷新或关闭释放。

Manager::add_library 在已经打开的页面连接准备并激活新增插件，合并其描述符、组件和定义，既有定义保持身份。重复逻辑名称交回错误，准备失败退役候选。该入口接受逻辑连接分组；宿主进程的启动参数继续由既有原生部署流程管理。组件名称接受多个完整逻辑名称段，支持后端组件对应的 `<名称>/web`；组名、插件名与实例名仍按单段名称校验。

Manager::reserve_connection_id 统一分配进程与页面连接世代。Manager::mount_child_instance 接收父实例名、目标 InstanceSpec、继承状态和 ContextOptions，使用活跃父纪元的 Context 挂载，等待远端配置检查后再次检查父纪元。子级的回退登记为父效应，父级结束等待其清理并移除目录与检查点记录。

纯前端部署在运行时按实例配置派生的 Context 中挂载由核心代码实现的 Owner 组件，其职责是资源登记与页面生命周期。隔离域和拦截配置随该视图进入页面子实例。Manager::mount_owned_child 接收该 Owner 的 Context，复用安装、父效应等待与目录移除步骤。两种入口都保持同一内核中的实际父子关系；浏览器业务函数始终在页面中执行。

页面关闭和 HTTP 清理通过 dispose_owned_child 直接等待已持有的 Fiber，适用于正在执行核心生命周期回调的路径。目录与检查点的撤回同时匹配名称和 Fiber 身份，迟到的父级清理保留已复用名称的新页面实例。该路径直接调用内核释放，管理事务可以等待它完成。

稳定性检查等待实例 settled 后，再按当前目录中的 Instance 身份判定需要确认的集合。等待期间随父级退出的页面子实例已结束其生命周期，确认阶段检查仍在目录中的实例状态。子实例登记期间父级结束时，挂载等待该子实例释放并按同一身份撤回目录。

远端组件每次加载由核心登记 `cordis.instance.<实例名>` 的内部存在信号，与该加载共用效应寿命。全栈资源 Owner 声明此信号以及 cordis.web@1 的依赖，随对应后端和 HTTP 服务的可用性由内核重建。该信号采用内核命名键和值 Arc<()>，为后端未提供业务服务时的网页资源生命周期保留统一依赖来源。

runtime 强持网页部署的 Handler 扩展，Manager::set_handler_extension 保存弱引用。Manager 完成原有生命周期方法分派后，把额外的 channel 方法交给该处理器；扩展只接受 DOC-16 §11 的转发消息。immediate_notification 同步委派通知顺序判定，保障 channel_frame 的帧顺序。弱引用使网页部署持有 Manager 时的所有权关系保持可释放。

Manager::invoke_service 供核心装配读取当前活跃的远端服务绑定，在保留提供方加载纪元期间调用其普通方法。参数和结果经过持久化值约束，网页资源登记通过该入口调用 cordis.web@1.1 的 register/unregister。服务声明与元数据仍来自真实提供方，网页部署沿既有 UP/1 连接调用 HTTP 插件。

RegistryClient 通过 Manager::lookup_remote_service 保存登记时的 HTTP 资源对象，撤回精确作用于同次激活的 registry。FrozenSnapshot 由网页部署事务持有，Manager::freeze_instances 复用既有导出过程并保持冻结，states() 提供保存的状态；失败决议调用 thaw_snapshot，成功清理旧页面后释放句柄。外部请求结束等待时，已受理的网页部署事务继续完成状态决议。

运行时的配置变更与 replace_batch 复用目标部署、候选预检、后端提交及恢复流程。纯网页 Owner 由网页部署管理，后端实例集合交给 supervisor；网页版本和配置的变化先保存页面状态，再随后端结果提交或恢复。资源声明撤回持有原配置与页面快照，持久化失败同样恢复这一组状态。

浏览器的入口导入地址和页面宿主命令由核心已安装的清单决定。HTTP 握手检查同次激活令牌与 Origin；WebSocket 页面通过 UP/1 获取服务，网络解析由插件进程执行。

HTTP 的缺省端口 0 在首次绑定后将实际地址保存到插件数据目录的 listeners/<实例名>.json。插件通过只读运行时快照和 Context::context_id 定位自身实例；后续激活恢复该实例的实际地址，并生成新会话令牌。浏览器在原 origin 读取当前令牌后重连。端口文件的解析、写入和监听操作都归 HTTP 插件执行。

TypeScript Runtime 通过构造参数接收连接与调用信号作用域，Context、BindingTransport、Payload 和值图预算由 Node 与浏览器复用。process 和 AsyncLocalStorage 移入 Node 入口，浏览器入口提供 WebSocket Connection 与页面信号作用域。两种入口共用 cbor-x 的 CBOR 实现，分别采用标准流帧和 WebSocket 二进制消息。

## 验证与结果记录

routing 提取复用现有 supervisor manager/process 测试，核验资源转交、连接回收、Scoped、清理、替换与失败路径。保留正式公开路径的重导出，并用既有源代码行工具记录提取前后各 crate 的代码行与总行。

M20 验收沿 AU-7、AU-14、AU-15：页面局部替换与全栈回退；两个页面的依赖失效与恢复、单页关闭；单页异常隔离和 HTTP 重启后的重连。浏览器验收保存实际查看的画面及运行证据。新增核心 crate 与 supervisor、runtime 合并统计，使拆分后的整体实现可比较。

ADR-0046 的 M22 提取与受信身份实现后，代码行与总行如下。supervisor 为页面及后端提供直接调用方事实，具体授权仍由插件决定；runtime 增量来自服务安装环境变量。

| crate | 目标 / 上限 | 实际代码行 / 总行 |
|---|---:|---:|
| cordis-routing | 500 / 800 | 736 / 828 |
| cordis-supervisor | 4500 / 5500；M22 结束至多 5450 | 5445 / 5896 |
| cordis-channels | 700 / 1200 | 250 / 277 |
| cordis-web-deployment | 1000 / 1500 | 870 / 939 |
| cordis-runtime | 4500 / 5500 | 5400 / 5933 |
| cordis-host | 2000 / 3500 | 2730 / 3020 |
