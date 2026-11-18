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

升级时保留部署配置、已确认的程序来源、业务数据及需要恢复的状态。宿主旧库副本在活动库和重启计划释放引用后可由运行时回收。手动回收旧版缓存前须停止相关服务并确认恢复来源可用。

## 应用插件

同一 `group` 中的原生插件共享一个宿主。服务依赖、正常替换与状态交接按实例处理。Python 与 Node 插件通过 UP/1 连接核心。程序包采用 ad-hoc 签名；使用脚本语言时还需部署相应解释器和依赖。

`secrets` 插件从运行用户拥有、权限为 `0600` 的文件读取获准键名。应用通过 `cordis.secrets@1.0` 取得获准的值。部署配置声明密钥文件位置和实例授权，凭据在目标环境配置。

[本版本使用指南](https://github.com/Proton1917/Rust-Cordis/blob/{{version}}/docs/guide.md) · [MIT 许可证](LICENSE)
