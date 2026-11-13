# watch

watch 0.1.1 要求核心版本至少为 0.1.1，其必需依赖为 `cordis.runtime@1.4`。快照中的 `page_of` 由核心网页部署填写；watch 选择部署实例提交批次，页面投影随所属部署更新。

```toml
requires_core = ">=0.1.1,<0.2.0"

[instance.watch]
plugin = "libcordis_watch.dylib"
component = "watch"
grants = { "cordis.manage@1" = ["install", "replace_batch"] }
config = { paths = ["candidates"], configuration = "cordis.toml", debounce_ms = 300 }
```

核心 0.1.0 提供的运行时接口为 1.3；watch 0.1.1 在该核心中保持 Pending，快照描述符列出所需的 `cordis.runtime@1.4`。应用的 `requires_core` 可以在启动时直接报告最低核心版本要求。

旧版 watch 的 1.3 读取方可继续使用核心 0.1.1 提供的 1.4 快照。显式管理请求仍使用部署实例名，批次目标缺席时整批拒绝。
