# 打包与独立运行时

## 发布输入

`cordis pack` 在目标平台构建并装配当前部署。Cargo 项目使用 release 和指定 target 构建原生目标；Python 依赖、TypeScript 编译结果和网页资源由各自的准备命令生成。进程插件的发布输入保留入口、SDK、生产依赖和业务资源，目录内容由 pack 复制。

在项目 cordis.toml 中声明发布名称、版本和应用标识；[pack 配置模板](../templates/pack.toml) 同时给出 Python 与 Node 运行时来源。通用资源通过 pack.assets 列出，网页资源来自各插件的 web.assets。

```sh
cordis pack --project . --target aarch64-apple-darwin --out dist
cordis pack --project . --target aarch64-apple-darwin --out dist --app
cordis pack --project . --target x86_64-pc-windows-msvc --out dist
```

`--target-dir` 指定 Cargo 产物目录，相对路径按项目解析；`--runtime-dir` 指定发布核心 cordis 与 cordis-host 的所在目录。目标平台检查使用实际可执行文件的机器类型，核心版本须满足项目 requires_core。

cordis-dev 0.2.1 默认重映射 Cargo 构建的 HOME、CARGO_HOME、sysroot 和项目路径，并保留用户环境编译参数。交付前按完整字节检查所有 Mach-O、ELF 及清单，检测到当前 HOME 或用户名时返回错误，诊断列出相对文件名和命中次数。预构建的核心、宿主及第三方程序同样参与检查；用官方发布的核心与宿主即可，自行构建时采用发布构建和同一套路径映射。用户明确接受路径披露时可使用 --allow-private-paths，将检查降为警告。

cordis-dev 0.2.4 的源码将同一套前缀映射同时传给 Rust 与遵循 `CFLAGS`/`CXXFLAGS` 的 C/C++ 构建脚本。原生参数包含 `-ffile-prefix-map` 和 `-fdebug-prefix-map`，覆盖 `cc` crate 编译的 `__FILE__` 字符串与调试信息；Apple 目标另传 `-Clink-arg=-Wl,-oso_prefix,$HOME/` 处理链接器 OSO 条目。已有 Rust 参数及各级 CFLAGS 保持后置顺序，CFLAGS 的引号由标准 shell 参数解析处理。发行进度以项目发布页为准。

v0.2.3 的默认 Rust 映射会遗漏这两类路径：实测的 release C 诊断插件命中一次 HOME，带 release 行表时该插件命中 27 次、另一个 Rust 插件命中 70 次。修复后的同一构建通过完整字节检查，25 条 OSO 记录保留且 HOME 命中为 0。采用支持新环境入口的源码构建脚本时，使用 `tools/build_paths.py` 的 `build_environment(project, target)` 更新 Cargo 环境；`encoded_flags(project)` 继续返回 Rust 参数字符串。

使用 v0.2.3 工具时，可以为本次打包显式补充参数。下面的示例保留现有环境值，项目路径和 HOME 各有对应的中性前缀：

```sh
cordis_native_maps="-ffile-prefix-map=$HOME=/home/build -fdebug-prefix-map=$HOME=/home/build -ffile-prefix-map=$PWD=/src -fdebug-prefix-map=$PWD=/src"
export CFLAGS="${CFLAGS:+$CFLAGS }$cordis_native_maps"
export CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$cordis_native_maps"
cordis_oso_flag="-Clink-arg=-Wl,-oso_prefix,$HOME/"
if [ "${CARGO_ENCODED_RUSTFLAGS+x}" = x ]; then
  cordis_separator=$(printf '\037')
  export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}${CARGO_ENCODED_RUSTFLAGS:+$cordis_separator}$cordis_oso_flag"
else
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$cordis_oso_flag"
fi
cordis pack --project . --target aarch64-apple-darwin --out dist
```

该示例适用于路径以单个命令参数表达的目录；含空格路径或带引号的自定义 CFLAGS，优先使用新工具的环境入口。预构建库、忽略这些环境参数的自定义构建脚本及显式用户覆盖参数仍按最终产物的完整字节检查判定。

普通发布输出 tar.gz，包内包含核心、宿主、插件、配置、资源和服务入口（Unix 为 `service/install.sh`，Windows 为 `service/install.cmd`；PE/Mach-O/ELF 二进制均经过目标架构与完整字节隐私路径检查）。macOS `.app` 把程序放在 Contents/MacOS，把部署文件放在 Contents/Resources；桌面应用配置 HTTP 与 window 插件，由窗口显示同一网页宿主。

## Python 与 Node

pack.runtimes 选择 python、node，pack.runtime_paths 指向各自完整运行时根目录。Unix 约定的解释器入口为 `bin/python3`、`bin/node`；Windows 兼容根目录或 `bin/` 下的 `python.exe`、`node.exe`；标准库、扩展模块、动态库和许可证随对应根目录保存。

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
