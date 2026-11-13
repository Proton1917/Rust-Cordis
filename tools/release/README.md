# Cordis {{version}}

平台为 macOS Apple Silicon（aarch64-apple-darwin）。本包包含 cordis、cordis-host、cordis-dev，以及 HTTP、watch、window、secrets 官方插件。

## 最小运行

```sh
tar -xzf cordis-{{version}}-aarch64-apple-darwin.tar.gz
cd cordis-{{version}}-aarch64-apple-darwin
export PATH="$PWD/bin:$PATH"
cordis version
cordis run
```

另开终端执行 `curl -i http://127.0.0.1:8080/`。按 Ctrl-C 有序退出。状态与检查点保存在当前目录的 state/，插件目录由 `[runtime]` 的 plugin_dir 指定。长期使用时把本目录的 bin 路径写入 shell 的 PATH 配置。

## 密钥服务

由部署系统创建 secrets.env，内容为 `NAME=VALUE`，文件属主使用运行服务的用户，并设置权限：

```sh
chmod 0600 secrets.env
```

应用配置示例：

```toml
[instance.secrets]
plugin = "libcordis_secrets.dylib"
component = "secrets"
config = { file = "secrets.env", grants = { api = ["PAYMENT_TOKEN"] } }
```

api 是获得 PAYMENT_TOKEN 读取权限的后端实例名。使用方声明 `cordis.secrets@1.0` 依赖，通过 get 取得密钥。页面使用 page_of 身份，密钥插件按后端 instance 授权。密钥文件权限须为 0600，值保持字面文本；服务管理器的 PATH 等普通变量通过 `service install --env` 设置。

## 签名与兼容范围

本包采用 ad-hoc 签名，Developer ID 签名和公证待配置。从网页下载后，可右键选择“打开”，或针对程序执行 `xattr -d com.apple.quarantine <文件>`；目录方式为 `xattr -r -d com.apple.quarantine bin plugins`。

Linux、Windows 待实测，crates.io 发布待安排。v0.x 期间 WIT list<u8> 写出为 CBOR 字节串，读取兼容字节串及旧整数数组。Rust、Python、TypeScript 插件的构建环境按语言准备。

使用指南：https://github.com/Proton1917/Rust-Cordis/blob/{{version}}/docs/guide.md

许可证：MIT，见 LICENSE。
