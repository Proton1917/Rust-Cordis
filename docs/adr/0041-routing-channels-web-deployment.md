# ADR-0041 服务路由、通道与页面部署

cordis-routing 处理对象编号、转交及调用。cordis-channels 管理转发授权与通道寿命，cordis-web-deployment 协调页面资源、实例与替换。

独立页面连接丢失时，由内核撤回其资源；全栈更新时网页模块和后端各由执行端替换，运行时协调共同结果。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0041-routing-channels-web-deployment.md)
