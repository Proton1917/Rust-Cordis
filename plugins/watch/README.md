# 文件变更与插件替换

`watch` 把一个防抖窗口内的文件变更合为一次部署请求。它按运行时快照选择部署实例；浏览器中的页面投影随所属实例更新。运行时负责切换和失败恢复，文件监视插件负责提交候选并读取批次结果。

## 部署

以下是 macOS 原生包的配置示例。监视部署配置时，需要同时授予 `reload` 权限。

```toml
[instance.watch]
plugin = "libcordis_watch.dylib"
component = "watch"
grants = { "cordis.manage@1" = ["install", "replace_batch", "reload"] }
config = { paths = ["candidates"], configuration = "cordis.toml", debounce_ms = 300 }
```

v0.2.5 提供 watch 0.1.2，页面归属依赖 `cordis.runtime@1.4` 的 `page_of`。确认一次部署时可查询 `last_batch` 中相应的请求标识。主分支使用 `max_submission_retries` 限制受理确认重提次数，默认值为 1；原配置名 `max_interrupted_retries` 仍可读取。[批次语义](../../docs/adr/0031-batch-replacement.md) · [页面投影与部署实例](../../docs/adr/0049-page-projections-and-package-paths.md)
