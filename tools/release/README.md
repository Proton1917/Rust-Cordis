# Cordis {{version}}

{{components}}
| cordis.runtime | 1.5 |
| 离线程序绑定 JSON | 1 |

发行版本代表组件的组合；本版提供核心 0.1.4、宿主 0.1.2、开发工具 0.2.4 和 Rust SDK 0.2.1。各组件版本及协议版本以表格为准。

平台为 macOS Apple Silicon（aarch64-apple-darwin）。本包包含 cordis、cordis-host、cordis-dev，以及 HTTP、watch、window、secrets 官方插件。

## 离线程序绑定查询

核心运行期间和停机后，通过官方命令读取每个实例最后持久确认的程序绑定：

```sh
cordis status --offline --config /srv/example/cordis.toml
```

输出为带版本的 JSON：

```json
{
  "schema_version": 1,
  "instances": {
    "store": {
      "declared_source": "store",
      "installed_reference": "installed-7",
      "plugin_directory": "/srv/example/plugins"
    }
  }
}
```

实例映射保存确认时的声明来源、已安装引用及绝对安装根目录。查询保持配置和状态文件原样；候选确认期返回上一份已提交绑定，确认并持久化后返回新版。停机后修改当前配置或移动程序文件时，查询仍返回原绑定事实。

退出码 0 表示成功；配置或记录读取、解析失败返回 1，诊断写入标准错误；命令行参数错误返回 2。有效空记录输出 `instances: {}`。JSON 版本 1 允许追加字段，删除字段或改变字段类型、含义时提升版本。部署工具检查版本后将这些引用加入保留集合，并结合回滚版本、分发来源和其他部署引用决定回收范围。

本次更新位于核心命令行与程序绑定模块，设计见 ADR-0054。升级核心程序即可使用查询，ABI 1.1、UP/1 和 cordis.runtime@1.5 沿用现有契约。

## 原生库缓存回收

宿主描述、装载和替换原生插件时生成的缓存副本，现在由宿主代码租约与核心重启计划共同保护。最后一个引用释放后，后续缓存操作回收副本；描述结束及有序退出也执行回收。进程异常退出时操作系统释放文件锁，后续扫描保留仍被重启计划引用的来源文件。

文件锁等待具有截止时间，核心的文件操作经阻塞执行入口处理；锁超时返回 code 6。旧副本回收失败会留下诊断，新副本继续分配。watch 0.1.2 使用稳定的已安装引用或自行保存的内容进行候选比较，兼容旧记录引用的副本已经回收的情况。

升级时配套替换 cordis、cordis-host 与 watch，并保留部署配置、安装版本及业务状态。宿主缓存目录中的副本可从部署输入重新生成，备份时可排除这些副本，同时保留部署输入、安装版本与检查点。旧版本生成且缺少租约标记的副本继续保留；手动清理前停止托管服务、核心和全部相关宿主，以 `lsof -nP +D <缓存目录>` 确认目录空闲，并确认恢复输入完整。旧 watch 的拒绝记录仍引用副本时，先升级 watch 或保留该副本。

## C/C++ 与 Apple 链接路径映射

cordis-dev 0.2.4 将同一套 HOME、CARGO_HOME、sysroot 和项目路径映射传给 Rust 与遵循 CFLAGS/CXXFLAGS 的 C/C++ 构建脚本。`-ffile-prefix-map` 与 `-fdebug-prefix-map` 覆盖原生依赖的 `__FILE__` 和调试路径，Apple 目标的 `-Wl,-oso_prefix` 处理链接器 OSO 路径。已有环境参数及引号语义继续保留。

源码集成打包脚本时，以 `tools/build_paths.py` 的 `build_environment(project, target)` 返回值更新 Cargo 子进程环境，可同时取得 Rust、C/C++ 与 Apple 链接参数。旧入口 `encoded_flags(project)` 继续返回 Rust 参数字符串。`cordis pack` 和本仓库发布脚本使用新环境入口。

打包末尾继续对产物完整字节检查 HOME 与用户名，命中即失败；显式 `--allow-private-paths` 可降为警告。预构建库及自定义构建脚本的产物也接受最终检查。

缓存回收与路径映射由宿主、进程支持、监管器、watch 和打包工具分别承担。内核库、Rust SDK 0.2.1、ABI 1.1、UP/1 和 cordis.runtime@1.5 保持既有契约，已编译插件可继续使用。发布包的路径结构沿用 v0.2.4。

## 替换状态与调用生命周期

依赖方随提供方重建时共同导出并继承最新状态，覆盖进程分组替换、原生组内换库、批量替换、配置更新和故障恢复。确认期回滚恢复替换前的原快照，报告通过 `discarded_confirmation` 列出涉及实例及确认时长。业务写入的持久性由数据所有者在应答前写入持久存储，并保持新旧版本可读的数据格式。

提供方开始清理时，经核心转交的在途调用与字节流立即返回 code 4。HTTP 0.1.1 对空正文 GET/HEAD 在正常替换宽限内向新登记处理方再提交一次，沿用原请求截止时间；进程故障保持 503，其他请求由调用方依接口的幂等约定处理。

运行时接口 1.5 兼容 1.3、1.4 读取方。Rust SDK 0.2.1、ABI 1.1 与 UP/1 保持，已有插件继续使用原编译产物。感谢外部审查者复现状态继承和确认期回滚问题，并推动相关修复。

## 同步等待与宿主诊断

Rust SDK 0.2.1 提供宿主协同的同步等待入口：

```rust
pub fn wait<F, T>(&self, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>;
```

在同步服务方法中使用 `ctx.wait(service.async_method())`。Future 可借用调用参数及局部值，支持嵌套等待。异步方法和 `ctx.spawn` 任务优先使用 `.await`。

该入口要求 cordis-host 0.1.1（ABI 1.1）及以上。宿主先交出原生工作线程，再驱动 Future，从而修复多个同步等待耗尽执行线程的卡死问题。旧宿主在轮询 Future 前返回 code 3。跨进程调用继续遵守既有截止时间。

宿主发现调用取消且工作线程持续占满时，报告分组、方法名、占用线程数和持续时间，提示检查阻塞等待或长时间同步计算。同一占满阶段报告一次。任务监测条目在任务结束时回收。

ABI 1.1 在 HostV1 表尾追加等待槽，v1.0 的布局及已有槽位保持；旧 SDK 插件可由新宿主装载。开发工具 0.2.4 生成的项目将 SDK 固定到本次发布标签的源码提交，依赖版本为 0.2.1。

watch 0.1.2 继续使用 cordis.runtime@1.4 的 page_of 识别页面投影，接口的最低核心版本为 0.1.1；缓存回收功能按本次发布的核心、宿主与 watch 配套升级。打包继续采用编译路径映射、相对动态库安装名称及完整字节检查。

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

动态调用按接口声明使用 `cordis_sdk::Bytes` 编码字节字段；从 JSON 转换参数时也按接口类型转换。整数数组遵循 UP/1 每集合 65,536 项的上限。

使用指南：https://github.com/Proton1917/Rust-Cordis/blob/{{version}}/docs/guide.md

许可证：MIT，见 LICENSE。
