# ADR-0046 外部接入：HTTP 路由、字节流、页面调用来源与密钥

| 项 | 内容 |
|---|---|
| 状态 | 已决定，M22 实施依据（设计方 2026-11-13；同日按用户“能通用就通用”扩展 §3.8–§3.10，并按 §0 的职责原则修订） |
| 影响 | DOC-15 §11.2、§11.3、新增 §11.7、§15（M22）、§16（AU-19–AU-26、RM-11）；ADR-0041 的行数表；plugins/http；新增 plugins/http-api、plugins/stream-api、plugins/secrets、plugins/secrets-api；cordis-supervisor（§3.11 受信调用元数据，先按 ADR-0041 提取）；cordis-web-deployment（§3.9 的页面键）；cordis-runtime 的 `service install`（§3.10 第 5 条）；三种 SDK 的辅助函数；cordis-dev 脚手架；skills/cordis-plugin；docs/guide.md |
| 不影响 | cordis（内核）、cordis-up、cordis-routing、cordis-channels、cordis-types、cordis-process、cordis-abi 与 ABI v1 布局；cordis-runtime 的配置解析与进程启动 |

## 0 职责原则

本 ADR 的每项能力按以下原则放置，§2 的选项比较与 §3 的各节都以此为判据：

1. `Q0`：核心不运行插件代码。
2. 核心之外一切皆插件。一项职责能由插件承担，就不进入核心。
3. 核心只承担只有它掌握事实的职责：实例、依赖、生命周期、连接与调用来源。
4. 协议语义归终结该协议的插件。HTTP 的方法、请求头、Cookie 只出现在 http 插件与处理方中，核心不解析也不保存它们。
5. 机密由插件读取、解释和保存。跨分组与进程插件的调用经核心路由，核心把调用参数与结果作为不透明载荷转交，不解析，不写入状态、日志、检查点、诊断与错误信息；机密也不进入插件进程的环境变量。同组原生插件之间经 ABI 直接调用，不经过核心。
| 编号说明 | ADR-0045 已由 M21 的静态分派候选占用（target/phase3-acceptance/m21/rm9-analysis），本 ADR 取 0046 |

## 1 问题

M20 交付的 http 插件（ADR-0038、ADR-0041）只开放三类地址：

| 地址 | 用途 |
|---|---|
| `/_cordis/socket` | 浏览器页面的 WebSocket，经转发通道接入核心 |
| `/_cordis/manifest.json`、`/_cordis/session.js` | 页面清单与本次激活的入口信息 |
| `/_cordis/<插件>/<序号>/...` 与 `[web]` 段的挂载路径 | 网页插件的静态资源 |

DOC-15 §11.2 第 5 条规定前端组件经 UP/1 调用后端，前端与后端之间不另设 HTTP 接口。这条规则覆盖 Cordis 体系内的浏览器页面。体系外的调用方只会发送普通 HTTP 请求，不能使用 UP/1，现有设计没有接住它们的位置。

第一个实际使用方是 MathBook_Studio（`/Users/gordongauerk/Projects/MathBook_Studio`，分支 `dev/neutron`，提交 `ddf83ab`）。它的迁移计划（2026-11-13，消息 864075eb 的答复）列出以下需求：

| 编号 | 需求 | 来源 |
|---|---|---|
| `HR-1` | `/api` 服务外部调用方：带令牌的角色服务（controller、extractor、author、math_reviewer、reader_reviewer、layout_reviewer）与 worker | `backend/config/plugins.json` 的 identity、worker 配置 |
| `HR-2` | Cookie 登录：读取请求 Cookie，返回 `Set-Cookie`（含 `Secure`） | api 插件配置 `secure_cookie = true` |
| `HR-3` | PDF 与附件的上传和下载；现有上限 `max_body_bytes = 104857600`（100 MiB），超过 UP/1 单帧上限 16 MiB（DOC-16 `max_frame = 16777216`） | http 插件配置 |
| `HR-4` | 页面、WebSocket 与 `/api` 经反向代理使用同一外部域名；当前 WebSocket 握手把 `Origin` 与内部监听地址比较，外部域名会被拒绝 | MathBook 迁移计划的部署适配说明 |
| `HR-5` | 后端服务需要知道发起调用的页面属于哪个登录用户。登录 Cookie 为 HttpOnly，页面脚本读不到；当前 WebSocket 握手只检查 `Origin` 与会话令牌（`plugins/http/src/socket.rs`），请求头不进入核心 | MathBook 的 Cookie 登录与角色权限 |
| `HR-6` | 插件需要密钥（`MBS_SECRET`、各角色令牌、管理员密码）。插件进程继承核心进程的环境变量（supervisor 启动进程时不清空环境）；由 `cordis service install` 交给 launchd 或 systemd 托管时，交互终端中的变量不存在，`cordis.toml` 也没有提供密钥的字段 | `backend/config/plugins.json` 的 `*_env` 字段 |
| `HR-7` | 插件之间传递生成的 PDF 等大文件。跨分组或跨进程的单条消息受 16 MiB 帧上限约束 | MathBook 迁移计划“插件之间传递产物身份或分块资源” |

`HR-5`–`HR-7` 对任何 Web 应用都成立，按通用能力设计。用户 2026-11-13 决定：这些能力在 Rust-cordis 中补齐，并在首次 GitHub Release 之前完成。

## 2 HTTP 路由的选项与判据

本节比较 `HR-1`–`HR-4` 的承担方式；`HR-5`–`HR-7` 的放置理由见 §3.8–§3.10。判据：

1. 路由不进入核心：核心进程的 crate 与 ABI v1 不因路由而改动（§0 第 2、4 条，`AU-8`，supervisor 余量 20 行）。
2. 路由处理方是普通插件，享有依赖、效应、热替换与回滚，三种语言都能编写。
3. 外部调用方只看到普通 HTTP，同一端口提供页面、WebSocket 与 `/api`。
4. 支持 `HR-3` 的大文件，单条 UP/1 消息不超过帧上限。

| 选项 | 内容 | 判据 1 | 判据 2 | 判据 3 | 判据 4 |
|---|---|---|---|---|---|
| A | 处理方插件自己监听端口 | 满足 | 部分：端口与监听代码在每个插件中重复 | 不满足：需要反向代理拼接 | 由插件自行处理 |
| B1 | 插件清单增加 `[http]` 段，由核心的网页部署登记路由 | 不满足：改动 cordis-web-deployment 与 runtime | 满足 | 满足 | 需另行设计 |
| B2 | http 插件提供接口 `cordis.http@1`，处理方插件在 `apply` 中登记路由，处理对象作为参数传入 | 满足 | 满足 | 满足 | 流对象满足 |

选择 B2。B2 只使用已有机制：插件间接口调用、对象作为参数转交（`cordis.forward@1.0` 的 `receiver: Service<ForwardReceiver>` 已在 M20 使用；Python SDK 的 `export_resource`/`import_resource` 已存在）、效应清理。

## 3 决定

### 3.1 接口 crate `plugins/http-api`

新增 `plugins/http-api`，与 `plugins/web-api` 同样只依赖 cordis-sdk 与 serde。同时提供 WIT 文件 `plugins/http-api/wit/http.wit`，由 `cordis idl gen` 生成 Python 与 TypeScript 绑定。两个接口，另外使用 §3.8 的通用字节流：

| 接口 | 提供方 | 方法 |
|---|---|---|
| `cordis.http@1.0` | http 插件 | 异步 `route(spec: RouteSpec, handler: Service<HttpHandler>) -> u64`；异步 `unroute(id: u64)` |
| `cordis.http-handler@1.0` | 路由处理方插件 | 异步 `handle(request: HttpRequest) -> HttpResponse` |
| `cordis.byte-stream@1.0`（§3.8） | 发送字节的一方（上传时为 http，下载时为处理方） | 异步 `read(max_bytes: u32) -> Option<Vec<u8>>`；返回 `None` 表示结束 |

数据结构：

| 结构 | 字段 |
|---|---|
| `RouteSpec` | `prefix: String`；`methods: Vec<String>`（空表示全部方法）；`max_body_bytes: Option<u64>`；`timeout_ms: Option<u64>` |
| `HttpRequest` | `method: String`；`path: String`（完整路径，百分号编码保持原样）；`prefix: String`（命中的路由前缀）；`query: String`（不含 `?`）；`headers: Vec<(String, String)>`（保留重复项与顺序，名称小写）；`peer: String`（直接对端地址）；`body: Body` |
| `HttpResponse` | `status: u16`；`headers: Vec<(String, String)>`（允许重复，例如多条 `set-cookie`）；`body: Body` |
| `Body` | `Empty`；`Inline(Vec<u8>)`；`Stream { length: Option<u64>, source: Service<ByteStream> }` |

`Inline` 的字节与 `HttpRequest`、`HttpResponse` 中的其他 `list<u8>` 按 ADR-0039 的 M22 修订编码为 CBOR 字节串。

Rust 侧另提供辅助函数，把登记返回的编号包装为效应，插件卸下时自动 `unroute`。Python 与 TypeScript SDK 提供同等的辅助函数。SDK 若缺少把对象放进结构体字段传递的能力，Codex 在实施前写明缺口与改动范围，交设计方确认；ABI v1 布局不变。

### 3.2 路由匹配

1. 前缀按路径段匹配：`/api` 命中 `/api` 与 `/api/...`，不命中 `/apix`。多个前缀同时命中时取最长者。
2. 以下登记返回冲突错误，已有路由保持：
   - 前缀与已有路由相同；
   - 前缀以 `/_cordis` 开头；
   - 前缀与已登记的网页挂载路径（`[web]` 的 `mount`）相同，或位于其下，或包含它。网页资源后登记时遇到同样冲突，同样返回错误。
3. 方法不在 `methods` 中时返回 405，并带 `Allow` 头。
4. 未命中任何路由与网页资源时保持现有行为（404 或静态入口）。

### 3.3 请求与响应的转换

1. http 删除逐跳头（`connection`、`keep-alive`、`proxy-connection`、`te`、`trailer`、`transfer-encoding`、`upgrade`），其余请求头原样传入，包括 `cookie`、`authorization`、`x-forwarded-*`。http 不解释转发头；是否信任由处理方决定。
2. 请求头的值不是合法 UTF-8 时返回 400，不调用处理方。
3. 响应的 `content-length` 与传输编码由 http 设置；处理方给出的逐跳头被丢弃；其余响应头原样返回，重复项保持顺序。
4. 处理方返回的状态码不在 100–599 时，http 返回 502。

### 3.4 请求体与响应体

| 配置（http 实例） | 缺省值 | 含义 |
|---|---|---|
| `inline_body_bytes` | 1048576（1 MiB） | 已知长度不超过该值的请求体内联传输；不得超过 4 MiB |
| `max_body_bytes` | 16777216（16 MiB） | 全局请求体上限；`RouteSpec.max_body_bytes` 只能更小 |
| `stream_chunk_bytes` | 1048576（1 MiB） | 每次 `read` 的上限；不得超过 4 MiB |
| `route_timeout_ms` | 30000 | 处理方返回响应头的截止时间；`RouteSpec.timeout_ms` 可覆盖 |
| `body_idle_ms` | 30000 | 流式读取两次 `read` 之间的最长间隔 |

1. 请求体超过 `inline_body_bytes`，或长度未知（分块传输），http 以 `Body::Stream` 传给处理方。处理方逐块 `read`，http 从套接字按需读取，不把整个请求体放进内存。
2. 累计超过上限时，http 返回 413 并关闭该请求的流；处理方后续 `read` 得到错误。
3. 处理方返回 `Body::Stream` 时，http 逐块 `read` 并写回客户端；客户端断开时 http 释放流对象，处理方的流对象据此收到释放通知。
4. MathBook 的 100 MiB 上限通过把 http 实例的 `max_body_bytes` 设为 104857600 获得。

### 3.5 生命周期、替换与故障

1. 登记是处理方插件的效应。处理方卸下时效应执行 `unroute`。
2. 处理方的连接或宿主结束时，核心回收它导出的处理对象。http 调用失败且错误为对象已释放时，删除该路由。
3. http 插件重启后全部登记丢失。处理方依赖 `cordis.http@1`，按已有依赖规则转 `Pending` 后重建，在新的 `apply` 中重新登记。
4. 替换宽限：前缀被 `unroute` 后的 `replace_grace_ms`（缺省 2000）内，到达该前缀的请求等待重新登记，等到后交给新处理方；超时返回 503 与 `Retry-After: 1`。替换失败回滚时，旧版本重新登记，同样在宽限内接住请求。
5. 状态码约定：处理方返回错误时为 500，响应体为不含内部细节的固定文本，错误写入 http 日志事件；处理对象已释放且宽限期外为 503；超过 `route_timeout_ms` 为 504。
6. 一个处理方崩溃只影响它的路由。http、核心、其他路由与页面连接不受影响。

### 3.6 认证与公开来源

1. 路由请求不检查 http 的会话令牌。会话令牌继续只用于 WebSocket 握手与 `/_cordis/manifest.json`。认证由处理方负责（MathBook 的 identity）。
2. http 增加配置 `public_origins: Vec<String>`（缺省为空），每项为完整 origin，例如 `https://book.example.com`。WebSocket 握手接受的 `Origin` 为：实际监听地址的 origin，以及该列表中的各项。其他 origin 仍被拒绝。
3. 监听地址缺省仍为 `127.0.0.1`。对外服务的 TLS 由反向代理承担；guide 给出一份反向代理配置示例，说明 `public_origins` 与 `listen` 的对应关系。

### 3.7 与 DOC-15 §11.2 第 5 条的关系

浏览器页面继续经 UP/1 注入服务，不使用路由。路由只服务体系外的 HTTP 调用方。DOC-15 §11.2 第 5 条改写为：“前端组件与后端之间不另设 HTTP 接口；体系外调用方经 §11.7 的路由接入。”

### 3.8 通用字节流 `cordis.byte-stream@1.0`（`HR-3`、`HR-7`）

字节流对象供任意两个插件之间传递超过帧上限的数据，HTTP 的请求体与响应体是它的一个使用方。

1. 新增 `plugins/stream-api`，定义 `cordis.byte-stream@1.0`：异步 `read(max_bytes: u32) -> Option<Vec<u8>>`。`max_bytes` 不得超过 4 MiB，超过时返回参数错误。附 WIT。
2. 发送方导出流对象，作为参数或返回值交给接收方；接收方逐块拉取，读到 `None` 结束。接收方释放对象即取消，发送方经效应或对象释放回调清理底层资源（文件句柄、套接字）。
3. 三种 SDK 各提供两个辅助函数：从文件或字节迭代器构造流对象；把流对象读到文件或回调。Rust 辅助函数不把整个流读进内存。
4. 同组原生插件之间也走同一接口，调用仍为直接调用。

### 3.9 页面调用来源（`HR-5`）

后端服务需要可信地得知一次调用来自哪个页面，并取得该页面建立连接时的请求信息（Cookie 等）。按 §0 第 3、4 条拆成两部分：

| 事实 | 唯一掌握者 | 放置 |
|---|---|---|
| 一次调用来自哪个页面连接 | 核心：页面连接经核心的转发通道建立，调用经核心路由 | 核心的网页部署 |
| 该页面连接的 HTTP 请求头与对端地址 | http 插件：它终结了 WebSocket 升级请求 | http 插件 |

1. http 为每个通过检查的 WebSocket 连接生成一个随机的页面键（至少 128 位），保存该连接升级请求中配置 `page_headers` 列出的请求头（缺省 `cookie`、`user-agent`、`accept-language`）与对端地址，合计不超过 8 KiB，超过时拒绝连接。页面键随 `forward.open` 的 `metadata` 交给核心。
2. 核心的网页部署把页面键作为不透明字节保存为该页面的只读属性，不解释其内容。页面经 UP/1 发来的内容不能修改它。页面键只来自持有 `forwarder = true` 授权的 http 实例。
3. 提供方声明 `scoped`（ADR-0025）时，来自页面子实例的调用所得视图对象的元数据带有键 `cordis.page`，值为 `{ forwarder: String（http 实例名）, key: Vec<u8> }`。来自后端插件的调用没有这个键。
4. http 增加接口 `cordis.http-page@1.0`（定义在 `plugins/http-api`）：异步 `attributes(key: Vec<u8>) -> Option<PageAttributes>`，`PageAttributes { headers: Vec<(String, String)>, peer: String }`。页面断开后该键失效，返回 `None`。只有声明依赖该接口的插件能取得属性，依赖由 `cordis.toml` 控制。
5. 三种 SDK 提供辅助函数：从视图元数据取出 `cordis.page`，再向对应 http 实例查询属性。
6. 属性是页面建立连接时的快照。登录或退出改变 Cookie 后，页面经整页导航重新建立连接，新属性随新连接生效。guide 写明这条流程：登录与退出走 §3.1 的路由，成功后页面导航。
7. 核心的改动：网页部署保存页面键，并经 §3.11 的受信调用元数据交出 `cordis.page`。页面与插件经 `context_derive` 或 `intercept` 写入的同名键被拒绝，不能覆盖。

### 3.10 密钥插件与服务环境变量（`HR-6`）

按 §0 第 2、5 条，密钥由插件提供，核心不读取密钥。

1. 新增 `plugins/secrets-api`，定义 `cordis.secrets@1.0`：异步 `get(name: String) -> Option<String>`。附 WIT。
2. 新增原生插件 `plugins/secrets`，提供该接口并声明 `scoped`。实例配置 `file` 指定密钥文件路径。文件每行 `NAME=VALUE`，允许空行与 `#` 开头的注释，不做变量展开。
3. 插件激活时检查文件：属主为当前用户，权限不宽于 0600。检查不通过时激活失败，错误信息给出路径与实际权限，不输出内容。
4. 授权写在密钥插件自己的实例配置中：`grants = { "<使用方实例名>" = ["MBS_SECRET", ...] }`。插件按 §3.11 受信元数据 `cordis.caller.instance` 判定调用方，只返回该实例名下列出的名字；请求列表外的名字返回权限错误；`grants` 中没有的实例取不到任何密钥。使用方的 `intercept` 元数据与授权无关。页面调用方的 `cordis.caller` 不带 `instance`（§3.11），一律返回权限错误；`grants` 不提供授权给页面的写法，密钥不进入浏览器。
5. 非机密的环境变量（例如 launchd 下缺失的 `PATH`）属于服务管理器的配置。`cordis service install` 增加可重复的 `--env NAME=VALUE` 与 `--inherit-env NAME`，写入 launchd plist 的 `EnvironmentVariables` 或 systemd 单元的 `Environment`。命令拒绝名字中含 `SECRET`、`TOKEN`、`PASSWORD`、`KEY` 的变量，并提示改用密钥插件。
6. 密钥值不得出现在 `status.json`、日志、检查点与 `cordis pack` 产物中；`cordis pack` 不打包密钥文件。

密钥文件采用 `dotenv-parser 0.1.3` 的成熟 `.env` 解析器处理，值保持字面文本；带空白、井号或空值时使用引号。该依赖只进入 secrets 插件。解析错误返回文件位置和格式诊断，原始错误中的输入片段由插件边界隔离。

### 3.11 受信调用元数据（`HR-5`、`HR-6` 的共同前提）

问题：`scoped` 元数据在 supervisor 中构造（`crates/cordis-supervisor/src/manager/routing.rs` 的 `get_service` 读取 `Context.named_meta` 并构造 `cordis.scope`）。`intercept` 是插件与页面可调用的普通 Context 派生能力（`views.rs` 的 `derive_remote`→`derive_context`），部署配置中的 `intercept` 与插件自行派生的 `intercept` 走同一路径，无法区分来源。若以 `intercept` 承载身份或授权，会出现两个漏洞：

- 页面可以写入 `cordis.page`，冒充其他页面；
- 插件可以给自己写入密钥授权，取得全部密钥。

按 §0 拆分：

| 事实或策略 | 唯一掌握者 | 放置 |
|---|---|---|
| 一次调用来自哪个实例、哪个页面连接 | 核心 | supervisor 的受信调用元数据 |
| 某个实例能读哪些密钥 | 密钥插件的部署配置 | `plugins/secrets` 的实例配置 `grants`（§3.10） |
| 业务元数据（租户、域名等） | 使用方与部署配置 | 现有 `intercept`，用途不变 |

1. supervisor 为 Context 增加通用扩展“受信调用元数据”，名称与内容不涉及 HTTP，只由核心侧代码写入。派生上下文与子组件按原规则继承所属实例的扩展；插件与页面没有修改它的接口。
2. `get_service` 构造 `scoped` 元数据时，加入受信键：
   - `cordis.caller`：所有调用都带，按调用方所在的一侧取两种形式之一：
     - 后端调用方（原生或进程插件，以及它们自行装入的子组件）：`{ instance: String }`，值为所属实例名；子组件取其父实例名。
     - 页面调用方（页面子实例，以及页面中装入的子组件）：`{ page_of: String }`，值为前端插件实例名，不带 `instance` 键。
   - `cordis.page = { forwarder: String, key: Vec<u8> }`：仅来自页面子实例的调用带，见 §3.9。
   - 两个键只描述直接调用方，不沿调用链传递。后端插件在处理页面调用期间再调用其他服务，那次调用的 `cordis.caller` 是 `{ instance: 该后端实例名 }`，不带 `cordis.page`。

   页面调用方单独使用 `page_of` 的理由（M22 实施中发现，设计方 2026-11-13 修订）：全栈部署的页面子实例挂在与后端相同的部署实例名下；页面代码在浏览器中运行，其描述符的 `inject` 由客户端提交，不能作为授权界限。“调用来自浏览器一侧”这一事实只有核心掌握，所以由核心写进身份本身。按 `instance` 授权的任何插件因此自动拒绝页面调用方，无需各自识别 `cordis.page`。需要服务页面的插件按 `page_of` 显式授权。
3. 以 `cordis.` 开头的元数据键是保留键，只由第 2 条写入。任何来源经 `intercept` 或 `context_derive` 提交保留键，都返回权限错误，并写入核心诊断（只记键名与实例名，不记值）。部署配置不再需要保留键。
4. 行数：supervisor 当前为 5480 / 5500 代码行。实施顺序：
   - 先按 ADR-0041 把一个内聚单元从 supervisor 提取到 cordis-routing 或新模块，行为不变，调用路径与签名不变，原有测试全部通过；
   - 再加本节功能，完成后 supervisor 不超过 5450 代码行；
   - 注释行合计不少于提取前；
   - 更新 ADR-0041 行数表的实际值。
   提取与功能分开记录在 PROGRESS 中。
5. 任何插件都可以按同样方式以 `cordis.caller` 实现自己的授权，例如 http 路由处理方之外的内部管理服务。核心只提供身份，不解释授权策略。
6. 上游对应：上游 cordis 内核的服务经 `symbols.caller` 取得调用方上下文（`cordis/packages/core/src/utils.ts` 的 `createTraceable`），上游日志服务据此标出来源插件（`logger.ts`）。上游插件同在一个进程，内核直接交出上下文；本项目插件分布在多个进程，核心以 `cordis.caller` 把实例名带过进程边界。这是既有内核语义的跨进程形式，不是新增的核心职责。

## 4 交付

| 编号 | 交付物 | 位置 |
|---|---|---|
| `D22-1` | 接口 crate 与 WIT | `plugins/http-api`、`plugins/stream-api` |
| `D22-7` | 页面调用来源 | `plugins/http`（页面键与 `cordis.http-page@1.0`）、`crates/cordis-web-deployment`（页面键与 `cordis.page` 元数据）、三种 SDK 的读取辅助函数 |
| `D22-8` | 密钥插件与服务环境变量 | `plugins/secrets-api`、`plugins/secrets`；`crates/cordis-runtime` 的 `service install` 选项 |
| `D22-2` | http 插件的路由、流、宽限、公开来源 | `plugins/http` |
| `D22-3` | 三种语言的示例处理方插件，实现同一个回显与上传下载接口 | `plugins/http-echo`（Rust）、`plugins/http-echo-py`、`plugins/http-echo-ts` |
| `D22-4` | 脚手架：`cordis new-plugin <名称> --http` 与 `--rust`、`--python`、`--ts` 组合 | `crates/cordis-dev`，模板沿 skill |
| `D22-5` | 文档：guide 新增“HTTP 路由插件”一节（含反向代理示例）；skill 新增 HTTP 路由参考；README 能力表、组成表、路线图 | `docs/guide.md`、`skills/cordis-plugin`、`README.md` |
| `D22-6` | 验收记录与测量 | `docs/measurements/m22-*.json`、PROGRESS 的 M22 节 |

## 5 验收

测试均在真实进程中运行：真实 `cordis run`、真实 http 原生插件、真实 TCP 客户端。

| 编号 | 场景 | 通过的判据 |
|---|---|---|
| `AU-19` | Rust、Python、TypeScript 处理方各登记一个前缀；对每个前缀发送 GET、POST、PUT、DELETE，带查询串、自定义头与 Cookie | 处理方收到的方法、路径、前缀、查询串、请求头与请求体逐项与发送值相等；两条 `Set-Cookie` 按顺序返回；405 带 `Allow`；`/apix` 不命中 `/api` |
| `AU-20` | 每种语言上传与下载 64 MiB 文件 | 处理方写出的文件与源文件逐字节相等，下载结果与源文件逐字节相等；传输期间 http 进程常驻内存增量小于 32 MiB；超过路由上限时返回 413 |
| `AU-21` | 每秒 50 个请求持续发送期间，替换 Rust 处理方 3 次，再替换为 `apply` 必定失败的候选；最后卸下该插件 | 替换与回滚期间全部请求得到 200，无连接错误、无 503；回滚后由旧版本响应；卸下后返回 404 |
| `AU-22` | 处理方返回错误、超时不返回、panic、进程被 `SIGKILL`；对 http 发送 `SIGKILL` | 依次得到 500、504、503；核心与其他前缀持续正常；http 重启后各处理方重新登记，请求恢复 200，页面自动重连 |
| `AU-23` | 登记冲突与公开来源 | 重复前缀、`/_cordis` 前缀、与网页挂载重叠的前缀返回冲突错误且原路由不变；`public_origins` 中的 Origin 握手成功，其他 Origin 被拒绝；经测试内启动的本机反向代理，页面、WebSocket 与路由在同一 origin 下工作 |
| `AU-24` | 字节流：Rust 插件把 64 MiB 文件以 `cordis.byte-stream@1.0` 交给另一分组的 Rust 插件、Python 插件与 TypeScript 插件；中途释放流对象 | 接收方写出的文件与源文件逐字节相等；单条 UP/1 消息不超过 4 MiB 加封装；释放后发送方的文件句柄在效应清理后关闭 |
| `AU-25` | 页面调用来源：两个页面分别携带不同 Cookie 连接，调用同一个声明 `scoped` 的 Rust 服务；后端插件调用同一服务；页面伪造 `cordis.page` 内容；页面断开 | 服务对两个页面查得各自的 Cookie；后端调用没有 `cordis.page`；伪造内容不改变服务所见的页面键；断开后 `attributes` 返回 `None`；核心日志与 `status.json` 中不出现 Cookie 值 |
| `AU-26` | 密钥插件：权限 0644 的密钥文件；两个使用方在 `grants` 中各自的列表；请求列表外的名字；一个插件经 `context_derive` 的 `intercept` 写入 `cordis.caller` 冒充另一实例；一个页面写入 `cordis.page`；一个全栈插件的后端实例列在 `grants` 中，它的页面子实例调用 `get`；该后端在处理页面调用期间调用 `get`；`cordis service install --env PATH=... --env API_TOKEN=x` | 0644 时激活失败且错误中无文件内容；每个使用方只取得 `grants` 中自己名下的值；列表外返回权限错误；写入保留键的插件和页面得到权限错误，服务所见的元数据不变；页面子实例的 `cordis.caller` 为 `{ page_of }`，取密钥得到权限错误；后端在处理页面调用期间取得自己名下的密钥；含 `TOKEN` 的变量被拒绝，`PATH` 写入 plist；`status.json`、日志、检查点、`cordis pack` 产物中不出现密钥值 |
| `AU-8` 复查 | 核心组成 | `git diff df05064 -- crates/cordis crates/cordis-up crates/cordis-channels crates/cordis-types crates/cordis-process crates/cordis-abi` 为空；cordis-routing 的改动只来自 §3.11 第 4 条的提取；cordis-supervisor 的改动只涉及提取与受信调用元数据，代码行不超过 5450；cordis-web-deployment 只涉及页面键，cordis-runtime 只涉及 `service install`；核心 crate 中不出现 HTTP 请求头、Cookie 或密钥的解析代码，也不出现密钥授权逻辑；unsafe 审计通过 |

| 编号 | 指标 | 测量方法 |
|---|---|---|
| `RM-11` | 一次路由请求的往返时延 | 本机客户端对 Rust、Python、TypeScript 处理方各发送 10000 个小 GET，记录中位数与 99 分位；本里程碑只记录，不设上限 |

全部原有测试、177 条上游用例、unsafe 审计、fmt 与 clippy 保持通过。

## 6 后果

1. MathBook 的 `/api`、Cookie 登录、角色令牌与 PDF 上传下载由一个路由插件承担，页面、WebSocket 与 `/api` 共用一个端口和 origin。
2. 每个路由请求经过 http 插件、核心路由与处理方三段进程边界，时延高于自行监听端口的插件；`RM-11` 给出实际数值。
3. 内核、channels 与 ABI 不变。核心新增两项职责，都是只有核心掌握的事实：标明调用来自哪个页面连接（§3.9）；提供可信的调用方实例与页面身份，`cordis.` 保留元数据只由核心写入（§3.11）；授权策略留在各插件。supervisor 先提取再扩展，完成后留出至少 50 行余量。核心另在 `service install` 中写入服务管理器的环境变量，这属于它已有的服务安装职责。
4. 密钥不再经过环境变量，xelatex 等子进程不会继承密钥。
5. 不在本 ADR 范围内：HTTP/2 推送、WebSocket 路由（处理方自定义的 WebSocket）、TLS 终结、限流与鉴权中间件。这些列为后续事项。
