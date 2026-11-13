---
name: cordis-plugin
description: 编写、验证和部署 Cordis Rust、Python、TypeScript 与网页插件，处理共享接口、效应清理、状态继承、运行中替换和附带解释器的发布包。
---

# Cordis 插件

核心通过 UP/1 管理组件的依赖、加载、效应和替换。Rust 动态库在 cordis-host 中运行；Python 与 Node 插件以独立进程接入；浏览器组件经 HTTP 插件转发的 WebSocket 接入同一个核心。组件通过 Context 声明资源归属与状态导出，核心按这些声明执行清理和迁移。

先检查当前项目的 cordis.toml、插件清单和 SDK 修订。`cordis version` 显示核心版本，`cordis-dev version` 显示开发工具支持范围。沿用项目现有接口与语言选择；新的公开接口通过 WIT 或共享 Rust 接口 crate 管理。

## 选择入口

新项目用 `cordis new demo`，插件名采用小写字母开头的小写字母、数字和短横线。生成插件时选择后端语言，并按需求加入网页部分：

| 命令 | 组件形态 | 按需阅读 |
|---|---|---|
| `cordis new-plugin hello --rust` | Rust 原生插件 | [Rust 接口](references/rust-api.md) |
| `cordis new-plugin hello --python` | Python 进程插件 | [Python 插件](references/python.md) |
| `cordis new-plugin hello --ts` | Node 中的 TypeScript 插件 | [TypeScript 插件](references/typescript.md) |
| `cordis new-plugin hello --web` | 浏览器组件 | [网页与全栈插件](references/web.md) |
| `cordis new-plugin hello --rust --web` | Rust 后端与浏览器组件 | 同上；后端也可选择 `--python`、`--ts` |
| `cordis new-plugin api --rust --http` | HTTP 处理方；也可选择 Python、TypeScript | [HTTP、流与身份](references/http.md) |

创建项目与插件由 cordis-dev 的现有脚手架完成。[模板来源](references/templates.md) 说明共用模板与生成路径。Rust 原生插件和接口库支持 Rust 1.85，Python SDK 使用 Python 3.11 及以上，TypeScript 进程 SDK 使用 Node 22 及以上。

## 生命周期与接口

组件声明名称、配置、必需或可选依赖、所提供服务、监听事件和状态版本。配置检查放在 resolve，激活行为放在 apply。必需服务缺席时，核心把组件保持为 Pending；服务恢复后继续加载。

使用 Context 取得和提供服务，通过效应取得资源并登记清理；后台工作与其取消、等待共同归属该效应。需要状态迁移时声明状态模式，在导出时冻结写入，在激活时读取继承状态。[资源与状态](references/resources-state.md) 保留 Rust 的具体写法，各语言章节给出对应 API。

服务方法、事件和资源参数按接口声明编码。通过 `cordis idl gen` 从同一 WIT 生成 Rust、Python 或 TypeScript 绑定，提供方和使用方共同维护该定义。替换保持逻辑插件名、组件名及服务接口主版本；兼容追加方法时提升接口次版本。

结构中的字节字段采用 `cordis_sdk::Bytes`、Python `bytes` 或 TypeScript `Uint8Array`；WIT 绑定统一写出字节串并兼容旧数组。超过消息上限的数据经通用字节流分块传递。外部 HTTP 调用、页面身份与密钥授权见 [HTTP、流与身份](references/http.md)。

## 运行与交付

用 `cordis run` 启动部署，以 `cordis status` 观察实例和替换报告，并通过真实使用方确认服务行为、清理和状态继承。watch 将同一防抖窗口中的变化提交为完整批次；管理授权写在基础 cordis.toml 的 grants。票据与最终结果的判断见[验证与部署](references/verification-deployment.md)。

原生契约套件、进程提供方的契约部署和故障回退沿既有验证入口执行。发布应用时阅读[打包与独立运行时](references/packaging.md)，按目标平台准备实际运行文件、解释器、依赖及签名材料。交付记录说明使用的 SDK 修订、构建和运行命令、实际行为与验证范围。
