# ADR-0036 发布包的路径、签名与验证副本

| 项 | 内容 |
|---|---|
| 状态 | M16 实施依据 |
| 影响 | DOC-15 §12.2、AU-12；cordis-dev pack；Host 验证副本路径 |

## 问题

发布包需要在目标机器的任意目录启动。开发项目的构建目录、原始运行时路径和验证副本绝对路径都具有本机位置；代码签名还会改变动态库字节。因此打包同时确定产物来源、包内路径与签名后的验证顺序。

## 决定

打包工具在目标平台运行，支持 `aarch64-apple-darwin` 和 `x86_64-unknown-linux-gnu`。Rust 工作区按指定 target 构建 release，Cargo JSON 产物记录提供库与可执行文件路径；核心与宿主来自同目录安装或显式 `--runtime-dir`。object 读取实际机器类型，核心版本与部署 requires_core 通过 semver 比较，原生描述符由 cordis-host 子进程检查，进程清单使用 UP/1。

`[pack]` 可设置 name、version、identifier、runtimes、runtime_paths、assets、state_dir。name 缺省取项目目录名，version 缺省 0.1.0；identifier 缺省为 `org.cordis.<name>`。runtime_paths 将 python、node 映射到本机独立运行时目录，分别提供 bin/python3、bin/node。运行时目录随包复制，进程清单中的相应命令改为包内相对路径。Rust 进程插件的发布内容取 manifest、可执行文件和 web assets；其他语言目录保留运行源码与依赖。符号链接保持指向包内目标。

assets 是项目内相对路径数组，列出的文件和目录保持相同部署路径，供 HTTP 静态目录等配置引用；例如 HTTP 配置 assets='web' 与 pack.assets=['web'] 共同交付页面资源。资源路径与运行程序、插件、状态及缓存目录分别占用各自名称。

普通发布产物为 tar.gz，内含 cordis、cordis-host、cordis.toml、plugins、可选 runtimes 与 service。service/install.sh 从发布包当前位置调用核心生成持久服务配置。`--app` 输出 macOS app，核心和宿主位于 Contents/MacOS，部署与插件位于 Contents/Resources。窗口可执行文件为 Contents/MacOS/cordis-window，进程清单使用指向它的相对路径。Info.plist 指定核心为主程序并设置 LSUIElement；核心从自身位置取得部署入口，窗口进程通过主线程界面策略显示 Dock 图标。

发布配置记录 pack.app 与 pack.identifier。app 默认状态位置由 bundle 入口选择用户 Application Support 下的 identifier 目录，pack.state_dir 可显式指定包外目录。Locations 将只读 Resources/plugins 与外部状态目录分开：管理安装副本位于 state_dir/plugins，宿主缓存位于 state_dir/.cordis/lib；Catalog 同时发现源与已安装的插件。源进程插件保持源目录为工作目录，窗口相对启动路径仍指向 MacOS。应用的基础版本更新通过重新打包交付，运行中更新通过管理安装副本完成。实际验收明确传入工作区内状态目录。

默认保持产物已有签名；显式 `--sign` 使用同一身份签署核心、宿主和全部原生插件，并启用 hardened runtime。启用第三方库选项时，将 disable-library-validation entitlement 赋给 Host。签名和签名验证使用系统 codesign。

启用 require_verified 的发布配置保持完整字节认证。字节保持时复制现有验证副本与真实套件结果；签名改变字节时，对签名后的库重新执行原契约套件。套件由项目 release 产物按原印记中的名称和版本选取；实例配置作为契约会话的被测配置。成功印记随包保存，缺少所需套件时报告相应名称、版本和库路径。

VerificationReceipt 的 verified_copy 支持两种路径：既有绝对路径；相对验证根的包内路径。Host 的 --verified-dir 可以重复，app 同时允许只读 Resources/.cordis/verified 和可写 state_dir/.cordis/verified。绝对引用归属于一个配置根；相对引用按各根查找实际副本，以完整字节匹配确定来源。每个解析后的副本都位于相应根中，缺席的可写目录保持可选。缓存库保留同一引用，发布包移动、运行中更新和宿主重启使用同一规则。

## 依赖与职责

| 依赖 | 用途 |
|---|---|
| object 0.41.0 | 读取 ELF、Mach-O 的目标体系结构 |
| tar 0.4.46、flate2 1.1.11 | 生成压缩归档 |
| plist 1.10.3 | 生成 macOS Info.plist 与 entitlement |
| pathdiff 0.2.3 | 生成运行时、符号链接与发布目录之间的相对路径 |
| 既有 toml、serde_json、semver、cordis-process | 配置、Cargo 产物、版本与受控子进程 |

这些依赖归 cordis-dev；核心进程保持 Q0 的依赖边界。窗口库与主线程事件循环由窗口插件的独立 ADR 决定。监管器的新增职责按 DOC-13 先提取 cordis-routing。

## 验收

AU-12 在工作区内独立目录解开压缩包，只保留发布程序与插件，使用受限 PATH 启动核心并观察全部实例 Active。macOS 对实际 app 和 Mach-O 文件执行签名验证。运行时命令改写使用实际 Rust 进程插件验证路径及 UP/1 激活；Python、Node 完整解释器随 M21 的真实语言发布包验收。验证印记的移动、签名后重验、缺少套件与内容差异均沿真实库和契约执行路径检查。
