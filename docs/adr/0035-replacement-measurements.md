# ADR-0035 插件替换停顿的测量范围

RM-3 统计实际受影响的实例。RM-4 从串行事务开始准备候选计时，到相关实例恢复稳定为止；阈值重启引起的额外重建计入影响。

完整管理调用还包含确认等待和应答传输。比较时分别报告暂停与调用总耗时，依据 M14 的同一批原始样本解释指标。

[相关规格](../14-roadmap-phase2.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0035-replacement-measurements.md)
