# ADR-0029 核心与开发工具共享进程执行层

| 项 | 内容 |
|---|---|
| 状态 | 已接受（2026-11-12） |
| 影响 | DOC-13 crate 划分；ADR-0024、ADR-0027；M11–M12 |

## 问题

运行时需要启动插件分组，cordis-dev 需要调用 cordis-host --describe。两者都要限制输出与执行时间、管理进程组，并保持组信号先于 PID 回收。开发工具的依赖树按 ADR-0024 与内核分离，直接依赖包含内核协调器的 supervisor 会破坏这个边界。

## 决定

提取 cordis-process，承载已实现的 Peer、进程组生命周期与 run_capture。该 crate 使用安全 Rust，依赖 cordis-up、Tokio、nix 与 rustix，产品代码目标 400 行、上限 700 行。

cordis-supervisor 依赖并重导出 Peer，继续负责实例与依赖决策、恢复策略、资源路由、替换事务和状态迁移。cordis-dev 直接复用 cordis-process 的受限探测。运行时可沿 supervisor 的兼容导出使用该入口。

## 后果

PID 保有、信号与回收机制有一个实现来源；开发工具获取同一组清理保证且保持依赖边界。supervisor 的 4500 行上限继续有效，进程执行层按自身职责计量。
