# 第三阶段验收

M21 在 M20 的插件平台上补齐三语言总验收、完整解释器发布包和插件编写手册。当前工作区的 325 条运行用例、2 条文档用例一次通过；同一次全量负载期间，完整 native 套件另连续执行五轮，各 14 条通过。运行过程和时间交叠记录见 [native 稳定性](m21-native-stability.json)。

运行记录采用当前正式实现及已核对源码范围的阶段证据。未改变的 ABI、窗口代码和 Runtime 解析源沿已有结果复用；独立运行时打包、跨语言组合和本次测试等待修正采用本阶段实际运行结果。

## AU-1–AU-18

| 场景 | 结果及证据 |
|---|---|
| AU-1 原生故障隔离 | 全量 native 套件及额外五轮通过。非 spin 的故障触发使用场景总预算，先订阅当前状态，再等待连接结束、依赖 Pending 和 settled；spin 保持连续超时触发路径。段错误、栈溢出、abort、exit、死循环及 panic 场景均运行于真实独立宿主 |
| AU-2 进程协议故障 | workspace 的 process 场景通过：超大帧、畸形 CBOR、洪泛与超时使故障进程退役，其他进程继续响应 |
| AU-3 三语言同核 | languages 场景使 Rust、Python、TypeScript 三个 KV 与两个 probe 同时 Active，核对 native/process 插口及实际分组 |
| AU-4 六方向调用 | Rust→Python、Rust→TypeScript 由原生 kv-contract 实际调用；Python→Rust、TypeScript→Rust 由各语言 probe 调用；新增 Python→TypeScript、TypeScript→Python 共存场景真实执行读写、keys、flush，读回字节 [0,127,255] 并完成清理 |
| AU-5 连续计数迁移 | 同一 Rust counter 持续写入，提供方依次为 Rust、Python、TypeScript；替换前后计数继续增加，依赖方以新 context 激活，核心 PID 保持 |
| AU-6 三语言契约 | Rust、Python、TypeScript 的真实 KV 各通过原生 kv-contract 的 8 项断言，包含两阶段状态继承 |
| AU-7 全栈回退 | web 的 fullstack 与 batch 场景通过：后端和各页共同提交或恢复，页面文本、计数与共享存储保持，performance.timeOrigin 保持 |
| AU-8 核心组成 | 核心产品依赖树与 ABI、SDK、WIT 解析、文件监视、Python/JavaScript 运行时分离；unsafe 审计通过。解释器及原生扩展位于发布包中的独立插件运行路径 |
| AU-9 同组性能 | 原正式实现的停载 RM-9 满足原 1.05/1.15 判据，完整首次与重测样本见 [性能记录](m21-performance/README.md) |
| AU-10 50 次替换 | workspace 的 native-stress 场景实际执行 8 线程调用和 50 轮替换，检查版本、状态及分组阈值重启；沿 M14 的固定场景 |
| AU-11 核心恢复 | workspace 的临时 launchd 场景通过：SIGKILL 后恢复检查点、终止旧宿主并撤回临时服务。服务配置和测试现场位于工作区 |
| AU-12 独立发布 | 完整 Python 3.12.16 与 Node 22.23.5 签名 tar 包移动解开后实际运行；Python 加载包内 _cbor2 与 SSL，Node 加载原生 CBOR 模块并执行动态代码；两组 KV 读写和全部进程退出通过。实际签名身份为 ad-hoc (-) |
| AU-13 输入解析 | 四个指定目标均具备固定 30 分钟结果。M20 UP/1 目标包含通道载荷与内层消息，1801 秒、1,663,834 次、退出 0；其他解析源保持 M12，见下表 |
| AU-14 页面依赖 | 两页同时使用 Rust、Python、TypeScript 提供的服务；逐项撤回使两页 Pending 并清理 DOM，插回后恢复。关掉一页后另一页 context/revision 保持；同名派生域的两页值保持独立 |
| AU-15 页面故障与重连 | 超大帧、畸形 CBOR、嵌套转发及 128 个调用洪泛关闭故障页，健康页继续工作；HTTP SIGKILL 后沿同一 origin 轮换令牌并恢复两页 |
| AU-16 管理授权 | workspace 的管理、票据及重启恢复场景通过，授权按实例和方法生效，确认版本保存在叠加配置与来源记录中 |
| AU-17 窗口 | 本阶段 workspace 自动窗口恢复与签名场景通过；截图、Cmd-Q、单 Regular 应用、检查点和退出复用 [M16 用户实际验收](m16-au17-user.json)，window 源码保持同一版本 |
| AU-18 域与组合 | workspace 的原生同组/跨组 Scoped、runtime 租户隔离、Python/TypeScript SDK 组合场景通过；同组调用维持 typed 路径，父级释放等待子组件及私域资源回收 |

网页八场景的原始目录与结果见 [M20 网页记录](m20-web-acceptance.json)，实际状态图见 [网页插件状态与替换流程](../design/flow/index.html)。

## 解析器与 ABI 复用范围

| 目标 | 已执行结果 | 对应源码 |
|---|---|---|
| up1_decoder | [M20 记录](m20-up1-fuzz.json)，1,663,834 次 / 1801 秒 / exit 0 | cordis-up/src 和 fuzz target；被测源与正式源通过完整字节比较 |
| configuration | 15,151,009 次 / 1801 秒 / exit 0 | Runtime config.rs、config/scopes.rs，保持 M12 的正式源 |
| plugin_manifest | 14,687,854 次 / 1801 秒 / exit 0 | Runtime manifest.rs 及上述配置模块，保持 M12 的正式源 |
| state_block | 18,529,539 次 / 1801 秒 / exit 0 | Runtime storage.rs 及上述配置模块，保持 M12 的正式源 |
| ABI v1.0 | M17 的 13 条 Miri 边界用例和布局比较；本次 workspace 再次执行原生布局与 unsafe 审计 | ABI 实现、生产宏及 v1-layout.txt 保持；静态分派候选留在后续事项 |

M12 的正式源码与模糊测试对应记录位于 PROGRESS 的 M12 节。源码复用按文件差异判断；本阶段的打包和测试组织变动分别由对应真实包、语言与稳定性场景覆盖。

## 技能与发布

正式 skills/cordis-plugin 保留 Rust 接口、资源、状态和契约资料，新增 Python、TypeScript、网页/全栈与发布章节。Python、TypeScript、网页模板沿 cordis-dev 的实际维护源，避免独立版本漂移。技能 frontmatter 校验通过，命令与实际 SDK、脚手架、网页和完整运行时运行记录一致。

独立新会话的 Rust 插件与契约验收沿 M15 SK-1b/SK-2；共享 Rust 模板继续由 workspace 用例编译。既有 Codex 用户技能链接指向仓库技能目录，本阶段更新保存在仓库。

ADR-0043 记录独立解释器的路径、扩展和签名职责。Node 的 JIT 权限、ad-hoc 解释器装载包内扩展、深层 Mach-O 到外层 Bundle 的签名顺序均按真实测试处理。Developer ID 的实现按同团队规则设置；本次完整运行时包的实际记录对应 ad-hoc tar。

## 性能与方法范围

[RM-7/8/9](m21-performance/README.md) 保存编译器、平台、调用形态、原始样本和两个 RM-9 窗口。RPC 每向固定 10000 次，三条替换各一次，直接调用每实现固定 9 轮。

首轮 RM-9 与设计方全量复核重叠，中位数比 1.07711183；各方停载后，原正式实现重测为 0.95774113，最大值比 0.90793525，满足原判据。两轮原始数据全部保留。前 1–4 轮偏高、两个窗口比值相差约 12% 是本方法的已知变动，预热长度与交替采样方式留作后续研究。ADR-0045 静态分派候选保持暂停，SDK 和 Service 保持现行实现。

退出检查、源码行数、清理结果及提交主题统一记录在 [PROGRESS](../PROGRESS.md) 的 M21 节。
