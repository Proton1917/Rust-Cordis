# 页面投影与发布路径验收

环境为 macOS arm64、Rust 1.96.0。实现依据为 ADR-0049；核心与 watch 为 0.1.1，cordis-types 为 0.1.1，cordis-dev 为 0.2.1。

| 行为 | 结果 | 证据 |
|---|---|---|
| 两个页面在 watch 批次替换后继续使用 | succeeded；两份草稿、document 和应用外壳身份保持，page_of 指向部署实例 | [页面投影](fix-watch-projections.json) |
| 显式提交页面投影作为批次目标 | rejected，code 4 | 同上 |
| 1.3 读取方在 1.4 核心运行 | 旧 watch Active，实际自替换批次 succeeded | target/test-evidence/watch-runtime-compat-72218-0/watch-compat/watch-runtime-compat-72218-0/result.json |
| 新 watch 在旧核心运行 | Pending；inject 描述符列出 cordis.runtime@1.4；旧核心的 error 为 null | 同上 |
| 新 DTO 读取实际旧快照 | page_of 为 None | 同上 |
| cordis new 加 Rust HTTP 处理方后打包 | 六个程序/动态库完整字节检查通过，strings 用户名与路径匹配为 0 | [发布路径](fix-pack-paths.json) |
| 用户 RUSTFLAGS 合并 | 真实源码的编译条件生效，自定义参数保留 | target/fix-watch-privacy/evidence/workspace.log |
| 签名与装载 | 六项签名有效，四个动态库经 host --describe 成功装载 | [发布路径](fix-pack-paths.json) |
| Mach-O 装载命令 | LC_ID_DYLIB 使用 @rpath/<库名>；LC_LOAD_DYLIB 均指向系统库，LC_RPATH 相对条目已保存 | 同上 |
| 清单携带 HOME | 默认打包失败；显式 --allow-private-paths 输出警告并交付，诊断列文件名与次数 | target/fix-watch-privacy/evidence/workspace.log |

全量命令为 `cargo test --workspace --no-fail-fast -- --skip readme_first_plugin_builds_from_pinned_git_sdk`。首次执行记录 340 项通过、3 项失败，RM-11 按既有定义在独立测量时运行；固定 Git 脚手架用例按提交推送后的源码修订执行。三个失败均来自本次默认隐私检查与旧夹具的组合：HTTP 夹具使用带链接器对象路径的 debug 产物；应用和窗口夹具的状态目录改为相对路径后触发既有 Bundle 路径检查。修正后的发布夹具使用 release 产物，应用状态通过工作区内的 HOME 使用标准默认目录。

修正后按受影响范围执行：

| 命令 | 结果 | 日志 |
|---|---|---|
| `cargo test -p cordis-dev --test http -- --nocapture` | 9 通过，RM-11 保持独立测量，63.44 秒 | target/fix-watch-privacy/evidence/http-release-fixtures.log |
| `cargo test -p cordis-dev --test pack application_keeps_signed_resources_and_selects_multiple_verified_roots -- --exact --nocapture` | 1 通过，64.42 秒 | target/fix-watch-privacy/evidence/pack-app-home.log |
| `cargo test -p cordis-runtime --test window -- --nocapture` | 1 通过，61.46 秒 | target/fix-watch-privacy/evidence/window-home.log |

合并有效结果为 343 项通过。其余 pack、pack_runtimes、watch、batch、web、native 与压力场景保留全量执行中的成功证据。固定 Git 用例的命令为 `CORDIS_SOURCE_REVISION=<修复提交> cargo test -p cordis-runtime --test acceptance readme_first_plugin_builds_from_pinned_git_sdk -- --exact --nocapture`，输出位置为 target/fix-watch-privacy/evidence/pinned-git.log；该用例使用 GitHub 上实际推送的源码。

`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、177 条矩阵及 unsafe 审计通过。夹具调整后，针对 cordis-dev、cordis-runtime 的 clippy 与三个修改测试文件的 rustfmt 检查通过。内核、up、routing、channels、process、ABI 的 diff 为空；supervisor 只补一行 None，5446/5897 行。runtime 为 5414/5947 行；dev 源目录为 2694/2941 行，加共享 tools/build_paths.rs 的 45/49 行后为 2739/2990 行。

已查看页面截图：target/test-evidence/web-watch-replace-71559-0/web-sdk/web-watch-replace-71559-0/case/watch-page-1.png 和 watch-page-2.png，覆盖桌面与移动视口。

既有 v0.1.0、v0.2.0 发布包的完整字节复查另见 [装载路径记录](release-path-load-commands.json)。该记录补充先前 strings 检查的覆盖范围，现有发布资产按本次授权保持原状。
