# M22 外部接入验收

环境为 macOS 27.2、Apple Silicon、14 个逻辑处理器、48 GiB 内存，Rust 1.96.0。测试启动真实核心、原生宿主、Python/Node 进程和 TCP 客户端；页面场景使用真实 Chrome 与本机反向代理。

| 项目 | 实际结果 | 记录 |
|---|---|---|
| AU-19 | 三语言 GET/POST/PUT/DELETE 的路径、查询串、头部、Cookie 与正文保持；重复 Set-Cookie 保序；405/Allow、段匹配和默认 1 MiB 内联体通过 | [路由](m22-routes.json) |
| AU-20 | 每语言各 64 MiB 上传下载逐字节相同，已知长度和分块超限返回 413；HTTP RSS 增量 9,682,944 字节，低于 32 MiB | [文件](m22-files.json) |
| AU-21 | 每秒 50 次请求期间三次替换和失败回滚均持续返回 200；回复后可见当前版本；卸下宽限结束后返回 404 | [生命周期](m22-lifecycle.json) |
| AU-22 | 处理错误与 ABI 捕获的 panic 返回 500，超时返回 504，处理方被强杀返回 503；其他前缀继续运行，HTTP 重启后路由与页面重建 | [生命周期](m22-lifecycle.json)、[页面](m22-web.json) |
| AU-23 | 重复、协议前缀、两种登记顺序的网页重叠均返回冲突；最长路径段前缀有效；公开 Origin 成功、其他 Origin 返回 403；反向代理同源访问通过 | [页面](m22-web.json) |
| AU-24 | 同组与跨组 Rust、Python、TypeScript 的 64 MiB 字节流逐字节相同；受限帧连接通过，中途释放后发送文件描述符关闭 | [通用流](m22-streams.json) |
| AU-25 | 两页按各自键取得不同 Cookie；后端调用身份独立；保留键伪造返回权限错误；页面关闭后属性撤回；核心输出的测试机密搜索通过 | [页面](m22-web.json) |
| AU-26 | 文件权限、实例 grants、子组件继承、页面取密钥拒绝与后端授权读取通过；普通服务环境变量写入、机密名称拒绝；实际发布包排除密钥文件 | [密钥](m22-secrets.json)、[页面](m22-web.json) |
| 字节兼容 | 三语言 record/variant/map/option/result 各 1 MiB 往返；M21 KV 检查点三语言恢复；原有 KV 契约与跨语言迁移通过 | [聚合](m22-bytes.json)、PROGRESS 的工作区记录 |
| 宿主线程 | 普通线程与插件自有 Tokio 调用、远端取消、取消后继续调用、计时与生命周期操作通过 | [线程](m22-threads.json) |
| 脚手架 | Rust/Python/TypeScript 的 `new-plugin --http` 项目真实构建并返回 HTTP 200 | [脚手架](m22-scaffold.json) |
| 页面外壳 | 中性默认页与 404、自定义 assets/index、组件替换保持外壳、桌面和移动画面均已查看 | [页面流程](../design/flow/index.html) |

RM-11 使用同一条 HTTP/1.1 keep-alive 连接对每种语言固定发送 10000 个小 GET，计时从发送前到完整响应读取后，包含 HTTP、宿主、核心转发与处理方往返。测量期间独立运行，完整样本保存于 [m22-latency.json](m22-latency.json)。本指标按 ADR-0046 记录观测值。

| 处理方 | 次数 | 中位数 µs | P99 µs |
|---|---:|---:|---:|
| Rust | 10000 | 472.3125 | 611.750 |
| Python | 10000 | 401.0420 | 478.291 |
| TypeScript | 10000 | 348.3750 | 451.667 |

工作区共 336 项用例通过，含 2 项文档用例；RM-11 为独立执行的测量用例。初次完整运行暴露的 413 客户端时序、字节格式与可信身份夹具问题，按受影响目标修复验证，成功结果与剩余目标结果共同覆盖工作区。原始命令、各轮结果与日志路径见 [PROGRESS 的 M22 节](../PROGRESS.md#m22-外部接入)。fmt、clippy、177 条上游矩阵、unsafe 审计、Miri 13 条边界用例和布局检查通过。

AU-8 的冻结 crate 相对 df05064 保持：cordis、cordis-up、cordis-channels、cordis-types、cordis-process 与 cordis-abi。supervisor/routing 的提取和受信身份、web-deployment 的页面来源、runtime 的服务安装分别处于批准范围。核心九个 crate 合计 19137 代码行，supervisor 为 5444 行；两侧注释合计由 227 行增加至 236 行。原生宿主自有线程修复采用既有执行器，ABI v1.0 布局保持。
