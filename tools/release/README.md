# Rust-cordis {{version}}

这个发行包面向 macOS Apple Silicon。`bin/` 包含 `cordis`、原生宿主 `cordis-host` 和开发工具 `cordis-dev`。`plugins/` 包含 HTTP、watch、secrets 和窗口插件。发行标签标识这一组程序，组件有各自的版本。

{{components}}

## 启动示例

```sh
tar -xzf cordis-{{version}}-aarch64-apple-darwin.tar.gz
cd cordis-{{version}}-aarch64-apple-darwin
export PATH="$PWD/bin:$PATH"
cordis version
cordis run
```

包内的 `cordis.toml` 启动 HTTP 插件，监听 `127.0.0.1:8080`。在另一终端执行 `curl -i http://127.0.0.1:8080/` 可读取页面响应。按 Ctrl-C 结束服务，核心执行实例清理并保存检查点。

## 查询与升级

在部署目录执行 `cordis status` 查看实例状态。`cordis status --offline --config cordis.toml` 在核心运行时及停机后均可读取最后确认的程序绑定。成功时输出含 `schema_version` 和 `instances` 的 JSON；每个实例对应声明来源、已安装引用及插件目录。配置或记录读取失败时命令返回非零退出码。

升级前先有序停机，并一起保留部署配置、完整状态目录、已确认的插件安装目录和业务数据。状态目录含已接纳的基础配置与恢复记录。回退核心时，使用与旧程序配套的升级前配置、状态和插件副本。出现 `recovery_required` 时，保留当前现场并用同一版本完成恢复，再通过离线查询核对程序来源。

## 应用插件

格式 1 默认按原生插件名分组。格式 2 默认把省略 `group` 的原生插件放入同一宿主。开发工具生成格式 2 工程。管理接口 `cordis.manage@2.0` 为部署操作返回票据，按调用实例和票据 ID 用 `cordis status --caller <实例名> --ticket <票据 ID>` 查询。旧接口保留 `install`、`replace_batch` 和 `shutdown`。[管理操作与授权](https://github.com/Proton1917/Rust-Cordis/blob/{{version}}/docs/guide.md#提交管理操作与查询结果)

Python 与 Node 插件通过 UP/1 连接核心，使用时部署相应的解释器和依赖。程序包采用 ad-hoc 签名。本发行资产与运行验收面向 macOS arm64。[测量条件与统计口径](https://github.com/Proton1917/Rust-Cordis/blob/{{version}}/docs/measurements/deployment-revision.md)

`secrets` 插件从运行用户拥有、权限为 `0600` 的文件读取获准键名。应用通过 `cordis.secrets@1.0` 取得获准的值。部署配置声明密钥文件位置和实例授权，凭据在目标环境配置。

[本版本使用指南](https://github.com/Proton1917/Rust-Cordis/blob/{{version}}/docs/guide.md) · [MIT 许可证](LICENSE)
