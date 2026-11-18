# ADR-0019 本地执行与跨进程服务

同宿主调用可以就地分派。跨进程获取服务时，核心建立带所有权的对象编号，连接按 UP/1 发送方法、参数与结果。

既有部署缺省按插件库分组，新布局采用共享原生宿主。两类部署读取各自版本的分组规则，业务接口相同。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0019-two-sockets.md)
