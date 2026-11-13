# 模板来源与生成路径

插件工程通过当前 cordis-dev 生成，SDK 修订与模板来自同一次工具构建。修改模板时维护下表中的源文件，再重新构建开发工具。

| 内容 | 生成入口或位置 | 仓库维护源 |
|---|---|---|
| Rust 插件 | `cordis new-plugin hello --rust` | skills/cordis-plugin/templates/plugin.rs、plugin.toml；scaffold 模块读取相同源 |
| Rust 接口、提供方、契约 | interfaces、plugins、contracts 中的独立 crate | 本技能 templates/interface、provider、contract 的 `.rs` 与 `.toml` |
| Python 插件 | plugins/hello/main.py、cordis_sdk、requirements.txt | crates/cordis-dev/templates/python/main.py；SDK 来自 sdks/python/cordis_sdk；scaffold/process.rs 生成清单 |
| TypeScript 插件 | plugins/hello/src、package.json、tsconfig.json | crates/cordis-dev/templates/typescript；SDK 来自 sdks/typescript/cordis-sdk/src |
| 网页部分 | plugins/hello/web | crates/cordis-dev/templates/web；scaffold/web.rs 写入组件名称与清单 |
| 原生全栈构建 | plugins/hello/build.sh | crates/cordis-dev/templates/web/build.sh |
| WIT 绑定 | idl gen 的 output 参数指定路径 | 当前 cordis-dev 的 WIT 解析与语言生成器 |

本技能的 Python、TypeScript 和网页章节直接使用上述脚手架。新增语言入口和 SDK 片段在开发工具源中维护，使生成项目与文档引用共用实现。pack.toml 和 web-deployment.toml 是部署配置片段，按当前项目的实例名、资源目录及目标平台填写。
