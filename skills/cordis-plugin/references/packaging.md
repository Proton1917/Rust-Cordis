# 打包与独立运行时

## 发布输入

`cordis pack` 在目标平台构建并装配当前部署。Cargo 项目使用 release 和指定 target 构建原生目标；Python 依赖、TypeScript 编译结果和网页资源由各自的准备命令生成。进程插件的发布输入保留入口、SDK、生产依赖和业务资源，目录内容由 pack 复制。

在项目 cordis.toml 中声明发布名称、版本和应用标识；[pack 配置模板](../templates/pack.toml) 同时给出 Python 与 Node 运行时来源。通用资源通过 pack.assets 列出，网页资源来自各插件的 web.assets。

```sh
cordis pack --project . --target aarch64-apple-darwin --out dist
cordis pack --project . --target aarch64-apple-darwin --out dist --app
```

`--target-dir` 指定 Cargo 产物目录，相对路径按项目解析；`--runtime-dir` 指定发布核心 cordis 与 cordis-host 的所在目录。目标平台检查使用实际可执行文件的机器类型，核心版本须满足项目 requires_core。

cordis-dev 0.2.1 默认重映射 Cargo 构建的 HOME、CARGO_HOME、sysroot 和项目路径，并保留用户环境编译参数。交付前按完整字节检查所有 Mach-O、ELF 及清单，检测到当前 HOME 或用户名时返回错误，诊断列出相对文件名和命中次数。预构建的核心、宿主及第三方程序同样参与检查；用官方发布的核心与宿主即可，自行构建时采用发布构建和同一套路径映射。用户明确接受路径披露时可使用 --allow-private-paths，将检查降为警告。

普通发布输出 tar.gz，包内包含核心、宿主、插件、配置、资源和服务入口。macOS `.app` 把程序放在 Contents/MacOS，把部署文件放在 Contents/Resources；桌面应用配置 HTTP 与 window 插件，由窗口显示同一网页宿主。

## Python 与 Node

pack.runtimes 选择 python、node，pack.runtime_paths 指向各自完整运行时根目录。当前约定的解释器入口为 `bin/python3`、`bin/node`；标准库、扩展模块、动态库和许可证随对应根目录保存。

打包器把运行时复制到发布部署的 runtimes/python、runtimes/node，并把进程清单中 python、python3、node 的启动项改为指向包内解释器的相对路径。来源中的符号链接需要指向该来源根目录内部，复制后转换为包内相对链接。

选择与目标系统、CPU 架构和 SDK 版本范围匹配的独立分发。Python 使用可重定位的完整发行目录，Node 使用官方目标平台归档。单个解释器可执行文件之外的依赖同样属于发布输入；用实际解释器查询其模块搜索路径和动态库依赖，确认来源目录覆盖运行所需文件。

Python 插件依赖由打包解释器匹配的 wheel 或源构建提供，脚手架入口从插件 `.dependencies` 载入。TypeScript 先编译为 JavaScript，再把生产 npm 依赖随插件交付。需要精简开发依赖时，在发布准备目录采用 npm 的依赖管理命令，保持开发项目的构建环境。

## 签名与实际运行

macOS 的显式 `--sign` 为发布代码选择签名身份；本地 ad-hoc 发布使用 `--sign -`，Developer ID 发布使用对应身份。`--allow-third-party` 与签名参数配合，为宿主按其动态装载职责设置 Library Validation 权限。`.app` 的外层签名在资源装配之后执行，工具同时运行签名校验。完整运行时随签名 tar 包交付时可采用：

```sh
cordis pack --project . --target aarch64-apple-darwin --out dist --sign -
```

显式签名时，Node 解释器采用 allow-jit 与 allow-unsigned-executable-memory 运行权限。`--sign -` 还为 Python 与 Node 解释器设置 disable-library-validation，以装入包内扩展；Developer ID 身份沿同一团队校验。发布树中的 Mach-O 文件按实际路径从深层目录向外签名，最后签署外层 Bundle；权限按解析后的解释器入口归属。外层 `.app` 的签名与内部解释器的角色权限分别由对应步骤处理。

发布验收从新目录解开实际产物，限制 PATH 为包内程序及系统工具，启动真实 Python、Node 或网页插件，检查服务调用、状态、窗口和有序退出。记录签名身份、产物形式、解释器路径、版本、实际加载的模块路径、核心与插件状态以及退出结果。Python 标准库和第三方扩展、Node 执行与网页脚本资源分别通过真实业务路径观察。

桌面包包含窗口时，还要观察窗口加载真实页面、窗口异常退出后的恢复及 Cmd-Q 对核心和插件的有序关闭。签名、构建与运行分别记录，使发布结论对应实际覆盖的行为。
