# ADR-0049 页面投影身份与发布路径

浏览器投影属于后端部署实例。运行状态增加 page_of，使 watch 能把网页变化归到真正的部署目标，已安装旧页面可随后恢复草稿。

发布构建统一路径映射，macOS 库使用可重定位的安装名称，包内清单与二进制接受实际路径检查。构建工具掌握这些平台细节。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0049-page-projections-and-package-paths.md)
