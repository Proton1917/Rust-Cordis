# ADR-0043 独立脚本运行时的发布链接与签名

| 项 | 内容 |
|---|---|
| 状态 | M21 实施依据，完整发行包验证通过 |
| 影响 | DOC-15 §12.2、cordis-dev pack、Python/Node 独立运行时 |

## 问题

Python 与 Node 官方发行目录包含指向目录内文件的相对符号链接；pack.runtime_paths 采用完整发行根目录。入口 bin/python3、bin/node 及其中的原生扩展需要在发布位置运行。macOS 的签名过程还需按 Node 的 JIT 执行职责设置权限，并覆盖解释器目录与插件依赖中的原生模块。

## 决定

运行时保持完整发行目录与许可文件。复制链接时按真实来源目录解析目标，再把目标映射为发布根目录内的相对链接；同一目录中的 Python 版本链接、Node npm 入口链接沿此规则处理。链接的目标属于当前发布来源树，实际启动入口由复制后的路径解析。

macOS 收集发布树中的真实 Mach-O 文件，嵌套扩展和动态库先签，再签可执行程序与外层 Bundle。Node 可执行程序按 JIT 职责使用 allow-jit 与 allow-unsigned-executable-memory，解释器的扩展模块随发布代码采用同一签名身份。核心保持既有权限；Host 的第三方库选项继续按 ADR-0036 管理。

签名身份为 ad-hoc（`-`）时，Python 与 Node 解释器增加 disable-library-validation，以便装入同包的已签名扩展。该模式的运行程序与扩展缺少开发者团队身份；Developer ID 签名继续采用同一团队校验。首轮真实 Python `_cbor2` 导入在 ad-hoc hardened runtime 下报告不同 Team IDs，证据为 pack-runtimes/m21-pack-runtimes-34415-0/case/python-runtime.json。

签名对象来自解析后的实际运行时入口和文件类型，目录中的文本脚本、许可文件和资源保留为普通资源。运行时及其嵌套原生模块随核心、插件统一进行签名校验。

## 验证

固定采用 CPython 3.12.16 aarch64-apple-darwin install_only_stripped 与 Node 22.23.5 darwin-arm64 官方归档。解开完整发行包，保留许可证，按既有配置装配至 tar.gz 发布包。在移动后的新目录以包内解释器运行真实 Python、TypeScript KV 插件，通过既有 UP/1 使用方读写并验证状态、有序退出和子进程结束。Python 第三方扩展及 Node 原生 CBOR 模块进入实际调用路径，Node 动态代码执行与签名权限一并观察。

发布装配与签名沿既有命令和配置入口实现。
