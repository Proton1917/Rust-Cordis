# ADR-0049 页面投影的替换身份与发布路径隐私

| 项 | 内容 |
|---|---|
| 状态 | 修复依据；设计方 442c17b7、e1b0b8b7，扩展边界经 2664e25b、c5fe35bb 批准 |
| 影响 | cordis-types、runtime 状态构造、web-deployment 的 child_names 可见性、watch、cordis-dev pack 与发布工具 |

## 页面投影身份

watch 从 RuntimeSnapshot 收集同名插件的替换目标。快照同时包含部署实例和页面投影，页面投影属于网页部署的子实例，批次入口按部署实例表查找，直接提交投影名称会得到缺席错误。

InstanceInfo 增加带 serde 默认值的 page_of，页面投影填写所属部署实例名。归属事实来自 web-deployment 已有的 live/children 表，runtime 通过开放的 child_names 读取后填写。普通实例为 None。supervisor 与网页部署的现有结构体构造点机械补齐 None，归属判断保持在 runtime。

watch 声明 cordis.runtime@1.4，并继续复用原 Runtime 类型化代理的方法签名；过滤 page_of 为 Some 的实例后沿现有逻辑按插件和组去重。管理批次的目标校验保持严格，显式提交缺席部署名继续整批拒绝。

DOC-11 §3.2 第 2 条要求追加默认字段时提升接口次版本，因此提供方升至 1.4。DOC-12 §7.3 的自描述编码保留 1.3 读取方兼容；新 DTO 读取旧快照时把 page_of 解为 None。watch 0.1.1 的最低核心版本为 0.1.1。

cordis-types 采用修订版本 0.1.1：传输字段为可选输出，旧版本读取方继续工作；Rust 源码中直接构造 InstanceInfo 的调用方需填写新增字段。旧核心上，新 watch 保持 Pending，组件 inject 描述符列出所需的 cordis.runtime@1.4。新建项目的 requires_core 同步为 >=0.1.1,<0.2.0。

## 发布路径

pack 构建项目 workspace，再装配其原生库、进程程序与已提供的核心和宿主。官方发布脚本此前设置了路径映射，pack 的构建入口缺少相同规则。

tools/path-remapping.json 是映射角色、顺序与中性前缀的共同定义。Rust 的发布构建和测试辅助，以及 Python 官方发布脚本读取同一份定义，映射 HOME、CARGO_HOME（缺省 HOME/.cargo）、rustc 报告的 sysroot 与项目根目录。构建目录沿既有选项选择。

现有 CARGO_ENCODED_RUSTFLAGS 优先于 RUSTFLAGS；将默认映射置于前部，保留有效环境参数的顺序并追加在后部，以 CARGO_ENCODED_RUSTFLAGS 传递完整参数。该编码保留参数中的空格。规则依据 [Cargo 构建参数](https://doc.rust-lang.org/cargo/reference/config.html#buildrustflags) 与 [rustc 路径映射](https://doc.rust-lang.org/rustc/remap-source-paths.html)。

Mach-O 的 LC_ID_DYLIB 由链接器产生。共享规则加入 -Crpath=yes，使安装名称在编译期成为 @rpath/<库名>；签名与验证使用该产物的实际字节。产物验收保存 otool 的 LC_ID_DYLIB、LC_RPATH、LC_LOAD_DYLIB 条目类别和相对路径，并验证装载依赖维持系统库范围。宿主继续按既有唯一路径打开各个版本的库（SW-1）。

交付树在签名、清单与服务脚本生成之后、归档之前检查。检查覆盖全部 Mach-O、ELF 以及 TOML、JSON、plist 清单文件，按完整字节检索 HOME 和当前用户名。诊断列相对文件名及各模式的命中次数；默认终止打包，显式 --allow-private-paths 降为警告。

预先构建的核心和宿主同样参与成品检查。使用方可选择已清理路径的官方运行程序、采用同一规则重建 runtime，或明确接受警告。watch 0.1.1 使用支持运行时接口 1.4 的核心 0.1.1。

## 验收

两个真实页面中的输入在 watch 批次替换后继承，应用外壳与页面文档身份保持，快照中的 page_of 指向部署实例。旧快照解码与 1.3 读取方、新 watch 对旧核心的版本诊断分别核验。

真实脚手架应用加入原生 HTTP 处理方，经 pack 构建后逐个检查二进制字符串；用户编译参数在实际编译中生效。带私有路径的清单验证默认拒绝及显式警告，原有 pack、watch、batch、web 和全量检查继续执行。
