# ADR-0055 原生插件的 Tokio 执行上下文

动态库有独立编译的 Tokio 及线程局部状态。主分支 SDK 的 tokio 特性在首次进入库时创建一份单工作线程运行时，提供调用第三方网络库所需的上下文。

宿主承诺保留映像时，SDK 在描述符释放时发起后台关闭。宿主允许卸载时，SDK 等待库级 Tokio 执行器和已启动的阻塞任务退出，再返回释放结果。旧宿主与当前 SDK 混用时，SDK 采用等待退出的方式。插件自行建立的系统线程由插件在效应清理时结束。[宿主卸载约定](0012-unload-semantics.md)

[相关规格](../11-plugin-model.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0055-plugin-tokio-context.md)
