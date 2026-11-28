# 生产部署与运维

本篇讲已经能在开发机上 `cordis run` 的项目如何长期运行：选平台、装服务、定分组、看日志、处理崩溃、备份。打包见[打包](packaging.md)，开发时的替换与契约验证见[验证与部署](verification-deployment.md)。

## 平台状态

| 平台 | 状态 |
|---|---|
| macOS（Apple Silicon） | 完整开发、实测与官方发布包验证：构建、运行、热替换、回滚、`service install`（launchd）、`cordis pack` 发布包 |
| Windows（x86_64 MSVC） | 完整真机验证：Win32 Job Objects 进程树回收、`.dll` 原生宿主与跨版 ABI 1.2 热替换/回滚、`service install`（Task Scheduler）、`cordis pack`（PE 校验与路径隐私检查）、`secrets` NTFS DACL 权限校验、Python/TypeScript SDK 与三语言 IDL 绑定 |
| Linux | 核心、协议、多语言 SDK、形式化验证与一致性矩阵已在 Ubuntu CI 持续验证；`.so` 插件、systemd 用户单元与 ELF 发布包在目标机首次上线前仍需按“首次上线检查”完整走一遍 |

不同平台首次上线前都应按末尾“首次上线检查”在目标机走一遍。

## 安装为服务

在部署项目目录执行：

```sh
cordis service install --name com.example.app \
  --env PATH=/usr/local/bin:/usr/bin:/bin \
  --inherit-env LANG
```

| 项 | macOS | Linux | Windows |
|---|---|---|---|
| 服务配置 | `~/Library/LaunchAgents/<name>.plist`；root 用 `/Library/LaunchDaemons/` | systemd 用户单元目录（`~/.config/systemd/user/`），经 `systemctl --user` 启用 | `%LOCALAPPDATA%\Cordis\service\<name>.xml`，经 `schtasks` 注册为 `Cordis\<name>` 计划任务 |
| 核心退出后 | launchd `KeepAlive` 重新启动 | systemd `Restart=always`，间隔 1 秒 | 计划任务 `RestartOnFailure` 自动重启；无附加环境变量时直接以 `cordis.exe` 为顶层任务进程，停用时由 Win32 Job Object 回收全部插件子进程 |
| 核心的标准输出与标准错误 | 项目的 `service/stdout.log`、`service/stderr.log` | systemd 日志，`journalctl --user -u <单元名>` | 使用附加 `--env` 包装时写入 `service\stdout.log`、`service\stderr.log`；推荐配合 `console-log` 插件输出日志 |

- 先用 `--dry-run` 或 `--output service/app.plist` 生成配置检查一遍，再正式安装。
- `--env` 与 `--inherit-env` 只放 PATH、LANG 这类普通变量。变量名含 SECRET、TOKEN、PASSWORD 或 KEY 时命令报错；机密走 secrets 插件（见 [HTTP](http.md)“页面来源与密钥”）。
- Linux 用户单元默认只在该用户登录期间运行。服务器上需要开机即运行、注销后不停止时，按 systemd 的常规做法为运行用户执行 `loginctl enable-linger <用户>`。这一点是 systemd 自身的行为，Rust-cordis 没有在 Linux 上验证过。
- `cordis service uninstall --name ...` 停止服务并删除配置，输出实际删除的路径。

对外提供 HTTP 时，http 插件只监听本机地址，TLS 由反向代理终结；浏览器实际访问的外部地址写进 http 实例配置的 `public_origins`。

## 分组与容量

部署配置掌握“代码在哪个进程运行”这一事实，插件源码不需要知道自己在哪个分组。

| 插件形态 | 进程 |
|---|---|
| Rust 原生插件 | 每组一个 `cordis-host`。省略 `group` 时，格式 1 按插件名分组，格式 2 默认使用 `native` 组。显式指定同一 `group` 的原生插件可共用宿主 |
| Python、TypeScript 进程插件 | 每个插件包一个独立进程 |
| 网页组件 | 运行在浏览器页面里，服务端不占进程 |

分组的取舍：

- **同组：** 同组原生插件之间直接调用，不经过核心，不受 `invoke_timeout_ms` 限制，也不经过 UP/1 编码，不受单次调用的容量上限（`max_frame` 16 MiB、`max_values` 262144 个值节点）约束；代价是共享故障域，一个插件进程级崩溃（段错误、`abort`、栈溢出），整组重启。
- **分开：** 每组多一个进程，调用经核心转交。每次跨组调用的参数和返回值都要编码成 UP/1 消息，受上述容量上限约束；超限时返回 code 8，错误说明方法名、方向和超出的预算。
- 新写的、调用外部网络或 C 库的、可能长时间阻塞的插件单独成组。频繁互调、彼此信任、都经过验证的插件可以合组。
- 按故障域和信任边界划分原生分组。每增加一个独立分组就多一个宿主进程，跨组调用还需要 UP/1 编解码。共用宿主的原生插件仍各有动态库、部署声明及可能存在的安装和热替换副本。
- 插件之间需要传递大块数据（文件、清单、历史记录）时，无论是否同组，都用 `cordis.byte-stream@1.0` 等流式接口分块传递，不要放进一次调用的参数或返回值。

容量按进程累加：核心一个进程，每个原生分组一个 `cordis-host`，每个进程插件一个解释器进程。在 macOS arm64（v0.3.0+）实测：空核心约 **2.1 MiB**，单个装有插件的空闲 `cordis-host` 约 **3.6–4.5 MiB**；3 个插件共享 1 个宿主总物理足迹约 **5.74 MiB / 26 线程**，拆成 3 个独立宿主会升至 **9.74 MiB / 41.5 线程**。

如果观察到 `cordis-host` 内存或线程持续偏高，通常由以下四个因素叠加引起，按顺序排查和控制：

1. **宿主过多：** 新工程使用 `format_version = 2`（省略 `group` 默认共享 `native` 组），把可信、稳定的原生插件合组，只给需要独立故障域的重型或实验插件单独分 `group`。
2. **插件重复自带 Tokio：** 不调用第三方 Tokio 生态的原生插件一律在 `export_plugin!` 声明 `tokio = false`（并关闭 `cordis-sdk` 默认 `tokio` feature），避免每个 `.dylib` 各自拉起一套 Tokio 运行时。
3. **大载荷顶高分配器水位：** 单次跨宿主大消息（如数 MiB 并发载荷）同时经历 `AbiBuf`、CBOR 值树与 UP/1 帧，会把系统 malloc arena 推到数十至上百 MiB 且不易立刻回落操作系统；大对象（文件、长清单、二进制产物）必须改走 `cordis.byte-stream@1.0` 或落盘传引用，RPC 只传小参数和小回执。
4. **热替换累积退役 `.dylib` 映像：** `cordis-host` 默认 `unload = "never"`，热替换后的旧版动态库保留在地址空间直到宿主退出。小内存或高频热替换环境调低 `retired_image_limit`（例如 4–8），让监管器在退役映像达到阈值时自动平滑重启宿主，彻底归还旧映像和分配器高水位。

小内存机器上线前：

1. 先在目标机器上 `cordis run`，用 `ps` / `top` / `footprint` 记录稳态与压测后的常驻内存；
2. 合并可信的原生插件分组，并核对无用 `tokio` 的插件已设为 `tokio = false`；
3. Python 与 Node 进程插件按包启动解释器进程。同一业务能力的小职责优先在一个包内用模块或组件组织。发布包可为同语言插件共用解释器目录，各插件目录中的依赖按实际装配计入安装体积。

## 运行时参数

`cordis.toml` 顶层推荐使用 `format_version = 2`（原生插件缺省归入共享 `native` 组）；`[runtime]` 段括号内为缺省值：

| 参数 | 含义 | 影响 |
|---|---|---|
| `invoke_timeout_ms`（30000） | 经核心转交的一次调用的截止时间 | 超时返回错误码 6；同一连接连续 3 次超时，核心判定该进程失效并终止它。长任务的写法见[调用、超时与长任务](calls.md) |
| `lifecycle_timeout_ms`（30000） | 一次 apply、一次效应清理、一次状态导出、一次替换等待各自的上限 | apply 里不做长时间等待；清理中只关闭和等待自己的资源；导出器只复制内存状态，大块数据放数据目录，状态里只存引用 |
| `checkpoint_ms`（60000，0 表示关闭） | 定期检查点间隔 | 核心崩溃时最多丢失这段时间内的内存状态；有序退出总会写最终状态 |
| `restart_max`（5）、`restart_backoff_ms`（500） | 原生分组崩溃后的重启次数与首次退避，退避逐次翻倍 | 进程插件在自己的清单中用 `restart = { max = 5, backoff_ms = 500 }` |
| `unload`（`never`） | 原生库退役策略：`never` 保留映像至宿主退出，`when-unleased` 在租约归零后尝试 `dlclose` | 默认 `never` 最稳妥，配合 `retired_image_limit` 回收内存 |
| `retired_image_limit`（20）、`retired_image_basis`（`retired`） | 触发原生宿主组平滑重启的退役库阈值与计量口径（`retired` 累计退役、`unclosed` 未关闭句柄、`mapped` 物理映射） | 高频热替换或小内存机器建议把阈值调低到 4–8，定期通过平滑重启释放旧库和内存水位 |
| `require_verified`（false） | 只装载通过契约验证的库 | 生产环境建议开启，见[验证与部署](verification-deployment.md) |

全局参数影响所有插件。不要为了一个慢方法调大 `invoke_timeout_ms`：它同时推迟了对所有卡死插件的发现。

## 崩溃与恢复

| 发生了什么 | 核心的处理 | 运维要做的 |
|---|---|---|
| 插件进程退出、协议错误或连续超时 | 终止该进程，撤回它的服务与效应，依赖它的实例转为 Pending；按退避重启，恢复的实例继承最近一次成功导出的状态 | 看日志找原因 |
| 重启次数耗尽 | 实例保持失败状态，不再重启 | 修复后重新构建（watch 会提交替换）或重启服务 |
| 核心退出 | 插件宿主在标准输入关闭时自行退出；服务管理器重新启动核心，实例继承最近一次检查点 | 检查检查点间隔是否满足可接受的丢失窗口 |
| 原生插件 panic | `panic = "unwind"` 时作为错误码 5 返回给调用方；`abort` 时整组退出 | 发布配置保持 `unwind` |

## 日志与状态

- 插件日志：Rust `ctx.log`、Python 与 TypeScript `context.log` 发出日志事件。部署 `console-log` 插件，日志事件才会写到核心的标准错误。进程插件写到自己标准错误的行，核心以 `[stderr <实例名>]` 前缀转写。进程插件的标准输出承载 UP/1，不能 `print`。
- 实例状态：`cordis status` 读取状态目录中每 100 ms 刷新一次的 `status.json`，列出实例、状态、错误、分组和最近一次替换批次。插件内部用 `cordis.runtime` 服务读取同一份快照。
- 排障顺序：`cordis status` 找到失败或 Pending 的实例和它的错误 → 核心标准错误中该实例的日志 → 缺依赖时核对接口名与主版本 → 反复重启时看退避是否已耗尽。

## 数据与备份

状态目录（缺省 `state/`）保存检查点、运行中管理操作写入的叠加配置 `state/instances.toml`，以及各插件的数据目录。

核心组件 0.1.4 起，部署工具通过 `cordis status --offline --config <配置文件>` 获取最后持久确认的逐实例程序绑定。先检查进程退出码为 0 和 `schema_version` 为 1，再读取 `instances` 中的 `declared_source`、`installed_reference`、`plugin_directory`；目录为绝对安装根目录。将这些引用加入升级保留集合，并结合回滚版本、分发来源和其他部署的引用决定回收范围。记录缺席或损坏时查询失败，部署工具保留已有版本并报告错误；有效空记录返回空映射。查询适用于核心运行期间和停机后，确认期返回原绑定。输出示例、版本规则与错误码见[运行时指南](../../../docs/guide.md#停机后的程序绑定查询)。

- 备份检查点、叠加配置、插件业务数据、部署文件、已安装插件引用与验证凭据。先停止服务取得最终检查点，再复制和重新启动；备份时排除下节的宿主工作副本目录。
- 恢复到另一台机器时，插件的状态模式名与版本必须被新机器上的插件接受。
- 密钥文件由部署系统单独管理，不放进状态目录的备份，也不进入发布包（`cordis pack` 会按密钥服务的声明排除它）。

### 原生库工作副本

普通部署的缓存位于项目的 `.cordis/lib/`，`.app` 部署位于 `state_dir/.cordis/lib/`。其中 `discovery/` 保存描述探测副本，各分组目录保存实际装载和重启使用的副本。部署输入、已安装插件引用及 `.cordis/verified/` 是恢复所需材料；宿主副本可由这些输入重建，备份可以排除整个 `.cordis/lib/`。

核心 0.1.3 与宿主 0.1.2 的源码通过文件租约管理这些副本，部署时配套升级核心、宿主与 watch 0.1.2。watch 的比较依据使用已安装引用或自行持有的候选内容，旧记录引用缺席时重新核验候选。宿主的代码租约、复制中的输入和核心重启计划共同保护文件；描述结束、库静止、计划更新和后续缓存操作会回收失去全部引用的副本。`Never` 策略继续保留已映射代码，进程退出释放最终映射；退役映像阈值沿现有配置触发宿主重启。回收失败记录诊断，新的装载继续处理。发行版本以项目发布页为准。

v0.2.3 及此前的副本采用手动清理：先停止服务管理器托管的核心，等待全部相关宿主退出；对实际缓存目录执行 `lsof -nP +D <缓存目录>`，确认打开文件查询为空，再移除缓存。保留插件来源、已安装版本、验证凭据、检查点与业务数据；旧 watch 拒绝记录仍引用的缓存副本也需保留，或先升级到能处理缺席引用的 watch。开发时原构建文件已经移走且缓存仍是所需版本的唯一副本时，先将该版本保存为正式部署输入。旧版本留下的无租约标记文件继续按这组停机条件处理。

## 首次上线检查

在目标机器上逐项确认，Linux 上尤其不能省略：

1. 从新目录解包发布包，`cordis version` 输出预期版本。
2. `cordis run` 前台启动，`cordis status` 中全部实例为 Active。
3. 走一遍关键业务流程，包括至少一次超过数秒的长任务。
4. 杀掉一个插件进程，确认它按退避恢复，依赖方回到 Active。
5. `service install` 后重启机器（Linux 上同时验证注销后服务仍在运行），确认服务自动启动。
6. 停止服务，确认有序退出并写出检查点；再启动，确认状态被继承。
7. 记录各进程常驻内存，确认在机器容量之内。
