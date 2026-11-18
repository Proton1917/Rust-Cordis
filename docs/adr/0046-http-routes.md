# ADR-0046 HTTP、流和可信身份

HTTP 插件负责路由匹配、请求头、正文、页面连接和终端用户认证；核心从连接确定直接调用实例或页面来源。业务权限由掌握数据的服务决定。

已知的小正文可以内联，增长型数据通过 cordis.byte-stream@1.0 分块读取。页面使用 page_of 身份，经实例授权的密钥由 secrets 插件按授权名单读取。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0046-http-routes.md)

## HTTP 插件的现行接口

| 接口 | 提供方 | 方法 |
|---|---|---|
| `cordis.http@1.0` | http 插件 | 异步 `route(spec: RouteSpec, handler: Service<HttpHandler>) -> u64`；异步 `unroute(id: u64)` |
| `cordis.http-handler@1.0` | 路由处理方插件 | 异步 `handle(request: HttpRequest) -> HttpResponse` |
| `cordis.byte-stream@1.0`（§3.8） | 发送字节的一方（上传时为 http，下载时为处理方） | 异步 `read(max_bytes: u32) -> Option<Vec<u8>>`；返回 `None` 表示结束 |

## 正文和路由配置

| 配置（http 实例） | 缺省值 | 含义 |
|---|---|---|
| `inline_body_bytes` | 1048576（1 MiB） | 已知长度不超过该值的请求体内联传输；不得超过 4 MiB |
| `max_body_bytes` | 16777216（16 MiB） | 全局请求体上限；`RouteSpec.max_body_bytes` 只能更小 |
| `stream_chunk_bytes` | 1048576（1 MiB） | 每次 `read` 的上限；不得超过 4 MiB |
| `route_timeout_ms` | 30000 | 处理方返回响应头的截止时间；`RouteSpec.timeout_ms` 可覆盖 |
| `body_idle_ms` | 30000 | 流式读取两次 `read` 之间的最长间隔 |
