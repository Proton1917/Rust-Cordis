# 框架组件的事实与资源所有权

Rust-Cordis 的组件边界依据[四条推论](../skills/cordis-first-principles/SKILL.md)：职责跟随事实；事实由其所有者写入、由读者向所有者取得；资源归取得它的实例并随其释放；跨插件与进程交互采用带版本契约。[推论详解](../skills/cordis-first-principles/references/principles.md)将核心不执行插件代码、单一内核、协议归终结者等维护判据归入这四条规则。

这里的框架组件指以 `Context`、声明依赖和 `Effect` 表达自身寿命的核心装配对象。内核掌握加载纪元和效应集合，监管器掌握连接和部署身份，网页装配持有其实际取得的 HTTP 登记；这些事实沿同一条组件树组合。基线为 `7e4dd5e41445be82af10def53fae315b5b83f7ea`，生命周期决策的证明域与实现精化义务见[形式化语义](formal-semantics.md)。

## 四条推论与组件入口

| 推论 | 事实与资源的所有者 | 实际入口和状态转换 | 接受证据 |
|---|---|---|---|
| 职责跟随事实 | `Kernel::State` 掌握 `alive`、目标纪元、运行纪元和效应归属 | `Context::for_epoch` 形成不可变视图；`create_effect` 在持有同一把状态锁时检查守卫并插入效应 | `R-CTX-1`；`epoch_context.rs`；`Admission` 与 `Scopes` 模型 |
| 只信所有者的事实 | Owner 的已提交依赖绑定决定 HTTP 提供方和后端加载代 | 网页 `activate` 从自己的 `Context` 读取 `cordis.web@1` 与 `InstanceLifecycle`；`Registration` 保存同一个远端对象用于登记、激活、撤回 | `bindings.rs` 的隔离域与全栈回放，各重复 16 代 |
| 资源随所有者归还 | 父挂载效应拥有子纤程；每代视图拥有其私有域租约；HTTP 登记归网页 Owner | 子挂载效应移交清理；提供方与等待中的消费者共享现有 `RealmLease`；最后一个视图归还域记录 | 预建子记录准入竞争、已准入回调清理交接、512 个历史域的归还回放 |
| 边界采用版本契约 | HTTP 插件拥有 HTTP 登记语义；监管器拥有 UP 连接和接口版本校验 | `remote_service_in` 对已提交绑定检查请求的次版本；网页继续调用 `cordis.web@1.1`，远端继续走 UP/1 | 真实 UP 请求的对象身份与调用序列；现行接口版本保持适用 |

`Context::for_epoch` 的事实全部属于内核：检查所需的三个状态以及接收新效应的集合都在 `State` 中。把这项检查放在效应登记的锁内，形成一个通用的准入能力。框架消费者继续通过现有组件、服务隔离和效应组合使用它。公开 Rust 契约由[行为规格 `R-CTX-1` 至 `R-CTX-3`](01-semantics.md#第四部分-rust-上下文准入)定义。

## 九个架构域的归属

下表从实际入口标出事实的写入位置、派生路径和边界。配置、宿主、路由、传输、SDK 与部署工具沿各自已有契约运行；运行时资源的成组改动见 [PR #22](https://github.com/Proton1917/Rust-Cordis/pull/22)、[PR #23](https://github.com/Proton1917/Rust-Cordis/pull/23)、[PR #25](https://github.com/Proton1917/Rust-Cordis/pull/25)。该映射用于检查这些 PR 与组件装配组合后的所有权。

| 架构域 | 唯一权威事实 | 入口与派生结果 | 资源或跨边界契约 | 回归证据 |
|---|---|---|---|---|
| 配置与持久化 | 配置文件给出部署意图；`Store` 保存已提交决定 | `Configuration::parse` → `Controller::execute` → `commit_deployment` / `recover_deployment` | 配置格式 1/2；目录 claim 保持至运行根清理完成；[部署语义](deployment-semantics.md) | durable overlay、offline 恢复及取消运行根 |
| 内核 | `Kernel::State` 保存实例、已提交依赖、纪元和效应 | `Context` → `create_effect` → 纤程调和 → 效应清理 | 泛用 Rust 上下文与效应契约；纯决策模块只消费事实 | Lean 决策/所有权/准入证明；174 个适用上游矩阵条目 |
| 运行时 | 控制器持有授权配置和部署事务；运行根拥有状态发布和目录 claim | `provide_management` 逐项移交 Effect；`StatusWriter` 的父效应停止并等待发布者；`RuntimeRoot` 等待根清理 | 管理接口版本、最终 `stopped` 快照、取得目录锁至实际进程清理完成 | 权限前缀失败、释放等待取消、状态最终快照、真实进程取消 |
| 监管器 | `Manager` 掌握连接和部署身份；内核决定加载与清理 | `Manager::new` 挂载 supervisor 组件；`Generation` 持有该次加载的绑定上下文；`InstanceLifecycle` 作为普通依赖提供 | UP/1；内部类型化生命周期键；现有域租约 | 真实后台换代与排队页面；私有域最后租约归还 |
| 原生宿主 | 宿主掌握执行线程、动态库、对象及缓冲寿命 | `Host::apply` → `NativePool` / `Jobs`；`Lease::register` / `acquire` / `release` / `reap` | 原生 ABI 与 `LibraryStatus` 版本协议；监管器读取宿主指标 | 原生替换、在途调用与库退役回放；[ABI 规格](12-abi-v1.md) |
| 路由 | 路由表拥有对象编号、持有者和来源寿命 | `Router::export` → `Batch::outgoing`；撤回时在锁内移出记录、锁外归还资源 | 对象引用与转交协议；同锁维护的索引用于定位权威表记录 | `cordis-routing` directory 释放与重入回归 |
| UP/1 | `Connection` 拥有连接健康、请求截止时间、唯一应答终态 | `request` → `Budget::encode` → `write_frames` → `pending.reply` | RequestWrite 首字节与取消共享锁；Lease 贯穿编码、队列与部分写入 | 排队截止时间、字节准入、共享预算；[协议规格](16-up1-protocol.md) |
| SDK | 语言 Context 保存核心签发的上下文身份；执行任务拥有回调槽位 | Rust 任务经宿主入口；脚本任务退出时归还实际执行额度 | 版本化服务、UP/1、状态模式；SDK 交回观察到的执行结果 | callback admission 与 terminal ownership；[插件模型](11-plugin-model.md) |
| 部署工具与网页 | 工具掌握产物树和安装目标；网页 Owner 掌握已取得的登记 | 打包/安装共用产物树策略；Inventory 摘要定位后完整字节比较；Owner 从已提交依赖装配页面 | 版本化清单、`cordis.web@1.1`；固定 `InstanceLifecycle` 属核心同进程装配接口 | 产物树与安装回放；`bindings.rs` 的 16 代 HTTP 与后端绑定 |

监管器在基线已经用 `ComponentDef` 和 `effect_async` 管理自己的 worker。组件化的实际改动集中于框架装配越过已提交绑定、按名称重新选择后端以及资源归属未交接的入口。HTTP 认证与协议处理继续由 HTTP 插件决定，原生插件继续由宿主执行。

## 加载代视图的准入与清理

`Context::for_epoch(Arc<Epoch>)` 捕获一个不可变纪元条件。`extend`、`isolate`、`intercept` 和再次绑定都保留同纤程上的已有条件，多个条件按合取判断。`has_epoch_guard` 只缓存视图链是否含这种条件；活动状态始终从内核锁内读取。

普通上下文的新效应沿用存活性检查。绑定视图先共享一次目标与运行纪元比较，再逐一比较捕获纪元。`Context::is_current` 在绑定视图上复用同一判断，在普通视图上保持原有当前性查询。服务读取继续使用已提交绑定，因此组件清理能够对原提供方完成撤回。

子挂载先创建子记录，再登记父级 `ctx.use()` 效应。目标换代发生在两步之间时，第二步的锁内准入拒绝旧视图并释放预建记录。准入成功后，效应的 `running` 状态使父级清理等待执行体交回子清理。子纤程的视图从自己的纪元开始，其寿命通过父挂载效应保持。`Fiber::context()` 继续提供用于跨加载代管理该纤程的普通视图。

监管器的 `Generation` 通过绑定后的 `context` 读取当前性。捕获纪元由这份上下文唯一持有，省去额外 `Generation.epoch` 字段及同值 Arc 克隆。Web Owner 保存从已提交 `InstanceLifecycle` 取得的 `InstanceGeneration`；其中弱引用固定指向原来的远端加载代。网页组件名和父上下文都由同一个代给出。

## 路径与成本

| 对象 | 固定基线或先前候选 | 当前实现及代价 |
|---|---|---|
| HTTP 提供方 | Owner 声明依赖后再做 Manager 全局查找，实际调用可跨越隔离域 | 直接读取 Owner 已提交绑定，登记/激活/撤回持有同一远端对象 |
| 全栈后端 | 激活复制全实例目录再查找组件；挂页时按名字重新找当前父实例 | 激活取得固定代身份；之后从该身份取得组件和绑定上下文，省去全目录快照和第二次按名选择 |
| 生命周期服务名 | 每个历史实例创建 `cordis.instance.<name>` 名字并保留名称；PR #12 曾引入 transient 名称表与分支 | 固定类型键 `cordis.instance` 与实例私有域；复用现有域租约机制，最后视图释放后移出该域记录；PR #12 的特殊表与 API 可整体移除 |
| 代际查询 | 组合当前性与目标纪元读取，重复加锁并克隆依赖向量 | 绑定上下文一次锁读取；捕获纪元归上下文，`Generation` 保留该上下文 |
| 普通视图新效应 | 存活性常数时间检查 | 128 层派生视图实测祖先访问 0 次，继续为常数时间 |
| 绑定视图新效应 | 使用方自行检查与登记，跨锁期间状态可改变 | 同锁检查，公共当前性比较一次；扫描同纤程视图链，成本随链深度增长 |
| 视图记录 | Rust 1.96、x86_64 下 `Scope` 布局 64 字节 | 加入不可变守卫形状后为 72 字节，每条视图记录增加 8 字节；捕获纪元按加载代共享 |

`for_epoch` 每次绑定创建一个持久视图节点。监管器和 Web Owner 在加载开始时各捕获一次纪元并放入 `Arc<Epoch>`，后续视图共享这个条件；调用不再逐次复制依赖向量。该绑定保留与加载寿命相同的节点和纪元分配，普通派生视图仍承担表中的每条记录增加 8 字节的布局成本。

这组改动增加了通用准入能力、回归用例和证明，当前生产代码与全仓源码的净行数均增加；代码规模净减仍是待达成的接受项。可归因的精简是删除特殊名称机制、全目录复制、重复目标读取及额外代际字段，并使已退役私有域随实际所有者归还。进程常驻内存、吞吐与尾延迟需要在相同部署和负载下另外测量。

## 回归与复现

Linux x86_64、Rust 1.96.0 下，核心准入用例覆盖普通视图、生存状态为 Pending 的实例、派生约束合取、独立子纪元、旧绑定清理读取、已准入回调的最后清理交接，以及父换代发生在子记录创建与父效应登记之间的确定性竞争。回放恢复旧的仅检查 `alive` 的登记路径时，竞争用例稳定失败；当前路径回收子 Fiber、运行时和效应，数量回到挂载前。

Web 用例通过实际 UP/1 连接同时提供全局与私有 `cordis.web@1.1`。纯网页和全栈 Owner 各经历 16 次提供方加载，核验每代仅一个登记、服务数量稳定，且 register/activate/unregister 三步使用相同对象。恢复旧的全局查找后，私有 Owner 错选全局提供方，断言失败。另一个真实监管器用例在同一 Fiber 已换代后提交旧父视图，核验请求以 code 4 结束且旧页面归还。域租约用例逐个创建并释放 512 个历史实例域，每次最后视图释放后登记数归零。

```sh
cargo test -p cordis --lib --test epoch_context --test current_epoch
cargo test -p cordis-web-deployment --lib --tests
cargo test -p cordis-supervisor --lib lifecycle_realms_are_private_and_released_with_the_last_view
cargo test -p cordis-supervisor --test manager core_owner_tracks_backend_presence_and_releases_page
python3 tools/verify_semantics.py --evidence target/formal-semantics.json
python3 tools/conformance_run.py --output target/conformance-report.json
```

CI 对实际执行结果和证明分别保存证据。守卫集合、不可变缓存和准入条件的数学关系由 Lean 检查；Rust 锁、字段读取、引用计数、回调执行和外部协议的实现对应关系继续由源码审查与真实回归支持，具体证明义务列在[形式化语义](formal-semantics.md#实现精化义务)。
