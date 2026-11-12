//! DOC-11 §8、ADR-0017：契约套件逐项报告断言，由核心协调状态往返和最终结果。

use cordis_sdk::{Context, Result, Service, builtin::Contract};
use std::fmt::Debug;

/// DOC-11 §8：套件当前纪元的报告接口，运行环境提供独立被测实例。
pub struct Suite {
    contract: Service<dyn Contract>,
}

impl Suite {
    /// ADR-0017：套件声明注入 cordis.contract@1，加载时取得本次验证的报告对象。
    pub fn new(context: &Context) -> Result<Self> {
        Ok(Self {
            contract: context.get::<dyn Contract>()?,
        })
    }

    /// DOC-11 §8.1：每个名称对应一个可观察行为，断言真假均提交给核心。
    pub fn check(&self, name: &str, passed: bool, message: &str) -> Result<()> {
        self.contract.report(name, passed, message)
    }

    /// DOC-11 §8.1：以接口契约给出的预期值比较结果，诊断同时包含两者。
    pub fn equal<T: PartialEq + Debug>(&self, name: &str, actual: &T, expected: &T) -> Result<()> {
        self.check(
            name,
            actual == expected,
            &format!("预期 {expected:?}，实际 {actual:?}"),
        )
    }

    /// ADR-0017：阶段 0 执行接口断言，阶段 1 读取继承后的实际结果。
    pub fn stage(&self) -> Result<u32> {
        self.contract.stage()
    }

    /// ADR-0017：请求后交回 apply，核心停用套件后执行被测实例的导出与重启。
    pub fn state_roundtrip(&self) -> Result<()> {
        self.contract.request_state_roundtrip()
    }

    /// DOC-11 §8.2：报告最终结束，核心根据全部真实断言决定验证结果。
    pub fn finish(&self) -> Result<()> {
        self.contract.finish()
    }
}
