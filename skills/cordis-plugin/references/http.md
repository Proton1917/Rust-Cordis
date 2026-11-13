# HTTP 路由、字节流与调用身份

## HTTP 处理方

`http` 插件终结 HTTP。处理方声明 `cordis.http@1.0` 依赖，使用 `http-api` 的 `HttpHandler` 对象登记前缀。Rust 的 `http_api::register`、Python 的 `cordis_sdk.http.register_route`、TypeScript 的 `registerRoute` 把登记与撤回放在同一效应中。

`RouteSpec` 包含 `prefix`、`methods`、`max_body_bytes`、`timeout_ms`。前缀按路径段取最长匹配；相同前缀、协议前缀和与网页挂载重叠的登记返回冲突错误。方法集合为空时接受全部方法，集合外的方法返回 405 和 Allow。显式卸下后先经过 `replace_grace_ms`，随后恢复静态资源匹配；宽限中的请求等待重新登记。

请求携带方法、原编码路径、命中前缀、查询串、重复头部、直接对端和正文。认证由处理方实施。HTTP 提供方过滤逐跳头，保持 Cookie、Authorization、X-Forwarded 字段；处理方根据自己的部署边界决定转发头的信任方式。响应状态、端到端头部与正文由处理方返回，Content-Length 由 HTTP 提供方生成。

通过 `cordis new-plugin api --rust --http` 生成原生处理方；也可选择 `--python`、`--ts`。运行目录同时配置官方 HTTP 提供方。示例 `plugins/http-echo`、`http-echo-py`、`http-echo-ts` 使用相同的回显与 `/file` 上传下载接口。

## 通用字节流

`stream-api` 的 `cordis.byte-stream@1.0.read(max_bytes)` 返回一块字节或结束值。读取上限接受 1 字节至 4 MiB，HTTP 默认每块 1 MiB。请求长度超过内联阈值或使用分块传输时，处理方收到流对象。以文件或回调消费流，内存随块长增长。

| 语言 | 构造 | 消费 |
|---|---|---|
| Rust | `stream_api::from_file`、`from_reader`、`from_chunks` | `copy_to`、`to_file` |
| Python | `cordis_sdk.streams.from_file`、`from_chunks` | `copy_to`、`to_file` |
| TypeScript | `fromFile`、`fromChunks` | `copyStream`、`streamToFile` |

流通过普通参数、记录字段或返回值传递。接收者归还最后一份对象引用时，发送者关闭其资源；完整读取到结束也关闭文件。Python 和 TypeScript 的文件流通过 `OwnedResource` 跟踪实际出口份额。

WIT 的 `list<u8>` 在各个位置都写成 CBOR 字节串，读取兼容整数数组。Rust 生成的聚合字段使用 `cordis_sdk::Bytes`；手写结构中的字节字段使用同一类型。方法位置的 `Vec<u8>` 保持原类型。Python 使用 `bytes`，TypeScript 使用 `Uint8Array`。

Rust 自有线程和 Tokio 运行时可以调用 SDK；跨进程调用和宿主计时器由宿主执行器处理。普通调用 Future 丢弃时传播取消，已受理的装入与清理继续完成。把线程退出等待放入插件效应清理，退出前归还服务与资源引用。

## 页面来源与密钥

Scoped 提供方通过 `caller(metadata)` 读取可信身份：后端是 `{ instance }`，页面是 `{ page_of }`。服务页面的插件按 `page_of` 显式授权。身份描述直接调用方；后端处理页面请求时调用其他服务，使用后端自身的身份。

页面元数据中的 `cordis.page` 保存 HTTP 提供方实例名和随机页面键。HTTP 属性查询辅助函数使用该键，从对应依赖域的 `cordis.http-page@1.0` 取得连接建立时的请求头与对端快照。Cookie 登录或退出成功后执行页面导航，以新连接建立新的属性快照。页面关闭后原键失效。

`cordis.` 元数据键由核心建立，业务拦截使用其他键名。密钥插件按自身 `config.grants` 中的后端实例名授权；页面身份得到权限错误。密钥文件由部署系统写入，属主为服务用户、权限至多 0600；值保持字面形式。`cordis pack` 根据实际密钥服务声明排除这些文件。服务管理器的 PATH 等普通环境变量通过 `service install --env` 或 `--inherit-env` 设置。
